//! Thin C ABI shim around `wasmer` + `wasmer-wasix`. One `Session` per
//! worker, persistent Python interpreter for our UDF dispatch model.
//!
//! Lifted directly from rust-wasix-python3/src/bin/embed-python.rs — the
//! prototype's "drive Python via the C API" path. Don't deviate from that
//! recipe; the README's "Design notes" section spells out why each step is
//! needed (fs overlay, ctors, TLS init, memory-via-WasiEnv).

use std::cell::RefCell;
use std::ffi::{c_char, CStr};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use wasmer::{Engine, Function, Instance, Memory, Module, Store, Value};
use wasmer_wasix::bin_factory::BinaryPackage;
use wasmer_wasix::runners::wasi::{PackageOrHash, RuntimeOrEngine, WasiRunner};
use wasmer_wasix::runners::MappedDirectory;
use wasmer_wasix::runtime::module_cache::{FileSystemCache, ModuleCache, SharedCache};
use wasmer_wasix::runtime::package_loader::BuiltinPackageLoader;
use wasmer_wasix::runtime::task_manager::tokio::TokioTaskManager;
use wasmer_wasix::{PluggableRuntime, Runtime};
use webc::metadata::annotations::Wasi;
use webc::{detect, Container};

const GUEST_SITE_PACKAGES: &str = "/site-packages";

// ─── thread-local last-error string ────────────────────────────────────────
thread_local! {
    static LAST_ERROR: RefCell<Option<std::ffi::CString>> = const { RefCell::new(None) };
}

fn set_last_error<E: std::fmt::Display>(e: E) {
    let s = format!("{e}");
    LAST_ERROR.with(|cell| {
        *cell.borrow_mut() = std::ffi::CString::new(s).ok();
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn lingodb_wasix_last_error() -> *const c_char {
    LAST_ERROR.with(|cell| match &*cell.borrow() {
        Some(s) => s.as_ptr(),
        None => std::ptr::null(),
    })
}

// ─── value type matching wasm.h's wasm_val_t (kind + 8-byte union) ─────────
#[repr(C)]
#[derive(Copy, Clone)]
pub union WasmValOf {
    pub i32: i32,
    pub i64: i64,
    pub f32: f32,
    pub f64: f64,
    pub _ref: *mut std::ffi::c_void,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct WasmVal {
    pub kind: u8, // 0=i32 1=i64 2=f32 3=f64
    pub _pad: [u8; 7],
    pub of: WasmValOf,
}

const WASM_I32: u8 = 0;
const WASM_I64: u8 = 1;
const WASM_F32: u8 = 2;
const WASM_F64: u8 = 3;

unsafe fn wasm_val_to_wasmer(v: &WasmVal) -> Value {
    unsafe {
        match v.kind {
            WASM_I32 => Value::I32(v.of.i32),
            WASM_I64 => Value::I64(v.of.i64),
            WASM_F32 => Value::F32(v.of.f32),
            WASM_F64 => Value::F64(v.of.f64),
            other => panic!("unknown wasm_val kind {other}"),
        }
    }
}

fn wasmer_to_wasm_val(v: &Value) -> WasmVal {
    match v {
        Value::I32(x) => WasmVal {
            kind: WASM_I32,
            _pad: [0; 7],
            of: WasmValOf { i32: *x },
        },
        Value::I64(x) => WasmVal {
            kind: WASM_I64,
            _pad: [0; 7],
            of: WasmValOf { i64: *x },
        },
        Value::F32(x) => WasmVal {
            kind: WASM_F32,
            _pad: [0; 7],
            of: WasmValOf { f32: *x },
        },
        Value::F64(x) => WasmVal {
            kind: WASM_F64,
            _pad: [0; 7],
            of: WasmValOf { f64: *x },
        },
        other => panic!("unsupported result type {other:?}"),
    }
}

// ─── Session ───────────────────────────────────────────────────────────────
pub struct Session {
    // Drop order matters: anything holding tokio handles must drop before
    // tokio_rt itself. Rust drops fields top-to-bottom, so put tokio_rt last.
    store: Store,
    instance: Instance,
    memory: Memory,
    funcs: Vec<Function>,
    func_names: Vec<String>,
    _runtime: Arc<dyn Runtime + Send + Sync>,
    _pkg: BinaryPackage,
    tokio_rt: tokio::runtime::Runtime,
}

fn build_session(
    webc_path: &str,
    site_packages: Option<&str>,
    cache_dir: Option<&str>,
) -> Result<Session> {
    let tokio_rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;

    let engine = Engine::default();
    tracing::info!("engine deterministic_id='{}'", engine.deterministic_id());

    // Read+parse webc.
    let bytes = std::fs::read(webc_path).with_context(|| format!("read webc {webc_path}"))?;
    let version = detect(bytes.as_slice()).context("detect webc version")?;
    let container = Container::from_bytes_and_version(webc::bytes::Bytes::from(bytes), version)
        .context("parse webc container")?;

    // Build runtime + load BinaryPackage + fetch module (cached).
    let (runtime, pkg, module) = {
        let _g = tokio_rt.enter();
        let task_manager = Arc::new(TokioTaskManager::new(tokio_rt.handle().clone()));
        let mut rt = PluggableRuntime::new(task_manager.clone());
        rt.set_engine(engine.clone());
        rt.set_package_loader(BuiltinPackageLoader::new());
        if let Some(dir) = cache_dir {
            std::fs::create_dir_all(dir).with_context(|| format!("mkdir {dir}"))?;
            let fs_cache = FileSystemCache::new(PathBuf::from(dir), task_manager);
            // SharedCache primary (in-memory), FileSystemCache fallback.
            rt.set_module_cache(SharedCache::new().with_fallback(fs_cache));
        }
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(rt);
        let pkg = tokio_rt
            .block_on(BinaryPackage::from_webc(&container, runtime.as_ref()))
            .context("BinaryPackage::from_webc")?;
        let cmd = pkg
            .get_command("python")
            .context("webc has no 'python' command")?;
        let module = tokio_rt
            .block_on(runtime.load_command_module(cmd))
            .context("load python module (compile or cache hit)")?;
        (runtime, pkg, module)
    };

    // Instantiate WITHOUT running _start. WasiRunner::prepare_webc_env is the
    // *only* path that overlays the package's bundled filesystem (Python
    // stdlib) into the WASI root + applies the command's manifest env vars
    // (PYTHONHOME=/cpython etc.). Without it Python fails at the first
    // stdlib import.
    let mut store = Store::new(engine.clone());
    let _g = tokio_rt.enter();

    let mut runner = WasiRunner::new();
    if let Some(host) = site_packages {
        let host_path = PathBuf::from(host)
            .canonicalize()
            .with_context(|| format!("--site-packages {host} not found"))?;
        runner
            .with_envs([("PYTHONPATH", GUEST_SITE_PACKAGES)])
            .with_mapped_directories([MappedDirectory {
                host: host_path,
                guest: GUEST_SITE_PACKAGES.to_string(),
            }]);
    }

    let cmd = pkg
        .get_command("python")
        .context("webc has no 'python' command")?;
    let wasi_meta: Wasi = cmd
        .metadata()
        .annotation("wasi")
        .ok()
        .flatten()
        .unwrap_or_else(|| Wasi::new("python".to_string()));
    let exec_name = wasi_meta.exec_name.as_deref().unwrap_or("python");

    let builder = runner
        .prepare_webc_env(
            exec_name,
            &wasi_meta,
            PackageOrHash::Package(&pkg),
            RuntimeOrEngine::Runtime(runtime.clone()),
            None,
        )
        .context("prepare_webc_env failed")?;

    let (instance, wasi_func_env) = builder
        .instantiate(module, &mut store)
        .context("WasiEnvBuilder::instantiate failed")?;

    // Drop the tokio guard; subsequent work is just calling wasm exports,
    // which routes through WasiEnv (no active tokio runtime needed).
    drop(_g);

    // 1) __wasm_call_ctors — runs C/C++ static constructors. Mandatory.
    //    Without it the first print() segfaults.
    let ctors = instance
        .exports
        .get_typed_function::<(), ()>(&store, "__wasm_call_ctors")
        .context("expected __wasm_call_ctors export")?;
    ctors.call(&mut store).context("__wasm_call_ctors trapped")?;

    // 2) __wasi_init_tp — installs the per-thread TLS block. Subtler:
    //    skipping it lets simple Python boot, then traps with OOB memory
    //    access on TLS-heavy paths (numpy's BitGenerator -> mimalloc TLS).
    if let Ok(init_tp) = instance
        .exports
        .get_typed_function::<(), ()>(&store, "__wasi_init_tp")
    {
        init_tp.call(&mut store).context("__wasi_init_tp trapped")?;
    }

    // 3) Py_Initialize — boots the interpreter.
    let py_init = instance
        .exports
        .get_typed_function::<(), ()>(&store, "Py_Initialize")
        .context("expected Py_Initialize export")?;
    py_init.call(&mut store).context("Py_Initialize trapped")?;

    // The python.wasm imports memory from env.memory rather than exporting
    // one. Pull it out of the WasiFunctionEnv (where wasmer-wasix attached
    // the imported memory during instantiate).
    let memory: Memory = {
        let env = wasi_func_env.data(&store);
        let guard = env
            .try_memory()
            .ok_or_else(|| anyhow!("WasiEnv has no memory after instantiation"))?;
        (*guard).clone()
    };

    Ok(Session {
        store,
        instance,
        memory,
        funcs: Vec::new(),
        func_names: Vec::new(),
        _runtime: runtime,
        _pkg: pkg,
        tokio_rt,
    })
}

// ─── C ABI ─────────────────────────────────────────────────────────────────

/// # Safety
/// `webc_path` valid C string. `site_packages` and `cache_dir` may be NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lingodb_wasix_session_new(
    webc_path: *const c_char,
    site_packages: *const c_char,
    cache_dir: *const c_char,
) -> *mut Session {
    let webc = match unsafe { cstr_or_null(webc_path) } {
        Some(s) => s,
        None => {
            set_last_error("webc_path is NULL");
            return std::ptr::null_mut();
        }
    };
    let sp = unsafe { cstr_or_null(site_packages) };
    let cache = unsafe { cstr_or_null(cache_dir) };
    match build_session(webc, sp, cache) {
        Ok(sess) => Box::into_raw(Box::new(sess)),
        Err(e) => {
            set_last_error(format!("{e:#}"));
            std::ptr::null_mut()
        }
    }
}

/// # Safety
/// `sess` must be NULL or a pointer from `lingodb_wasix_session_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lingodb_wasix_session_free(sess: *mut Session) {
    if !sess.is_null() {
        unsafe { drop(Box::from_raw(sess)) };
    }
}

/// Look up an exported function by name. Returns a session-local index on
/// success, -1 if missing. Caching the index avoids the per-call name lookup.
///
/// # Safety
/// `sess` live, `name` valid C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lingodb_wasix_lookup_func(
    sess: *mut Session,
    name: *const c_char,
) -> i32 {
    let sess = unsafe { &mut *sess };
    let n = match unsafe { cstr_or_null(name) } {
        Some(s) => s,
        None => return -1,
    };
    if let Some(idx) = sess.func_names.iter().position(|s| s == n) {
        return idx as i32;
    }
    let f = match sess.instance.exports.get_function(n) {
        Ok(f) => f.clone(),
        Err(_) => return -1,
    };
    sess.funcs.push(f);
    sess.func_names.push(n.to_string());
    (sess.funcs.len() - 1) as i32
}

/// 0 on success, -1 on trap/error.
///
/// # Safety
/// `sess` live, `args`/`results` valid for the given counts (or NULL when 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lingodb_wasix_call(
    sess: *mut Session,
    func_idx: i32,
    args: *const WasmVal,
    nargs: usize,
    results: *mut WasmVal,
    nresults: usize,
) -> i32 {
    let sess = unsafe { &mut *sess };
    let func = match sess.funcs.get(func_idx as usize) {
        Some(f) => f.clone(),
        None => {
            set_last_error(format!("invalid func_idx {func_idx}"));
            return -1;
        }
    };
    let arg_slice: &[WasmVal] = if nargs == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(args, nargs) }
    };
    let arg_values: Vec<Value> = arg_slice
        .iter()
        .map(|v| unsafe { wasm_val_to_wasmer(v) })
        .collect();
    match func.call(&mut sess.store, &arg_values) {
        Ok(out) => {
            if out.len() != nresults {
                set_last_error(format!(
                    "result arity mismatch: wasm returned {}, caller asked for {}",
                    out.len(),
                    nresults
                ));
                return -1;
            }
            if nresults > 0 {
                let res_slice = unsafe { std::slice::from_raw_parts_mut(results, nresults) };
                for (dst, src) in res_slice.iter_mut().zip(out.iter()) {
                    *dst = wasmer_to_wasm_val(src);
                }
            }
            0
        }
        Err(e) => {
            set_last_error(format!("trap: {e}"));
            -1
        }
    }
}

/// Pointer to the start of the guest's linear memory. Invalidated by any
/// guest `memory.grow`; recompute before each access.
///
/// # Safety
/// `sess` live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lingodb_wasix_memory_base(sess: *mut Session) -> *mut u8 {
    let sess = unsafe { &mut *sess };
    sess.memory.view(&sess.store).data_ptr()
}

/// # Safety
/// `sess` live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lingodb_wasix_memory_size(sess: *mut Session) -> usize {
    let sess = unsafe { &mut *sess };
    sess.memory.view(&sess.store).data_size() as usize
}

// ─── helpers ───────────────────────────────────────────────────────────────
unsafe fn cstr_or_null<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() {
        None
    } else {
        unsafe { CStr::from_ptr(p) }.to_str().ok()
    }
}
