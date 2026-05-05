//! End-to-end smoke test: load python.webc via the C ABI, then drive Python
//! via Py_Initialize + malloc + PyRun_SimpleString — the same lifecycle the
//! C++ runtime will use.

use std::ffi::{CStr, CString};
use std::ptr;

use wasix_python_bridge::{
    lingodb_wasix_call, lingodb_wasix_last_error, lingodb_wasix_lookup_func,
    lingodb_wasix_memory_base, lingodb_wasix_memory_size, lingodb_wasix_session_free,
    lingodb_wasix_session_new, Session, WasmVal,
};

const WASM_I32: u8 = 0;

fn last_err() -> String {
    unsafe {
        let p = lingodb_wasix_last_error();
        if p.is_null() {
            "(no error)".into()
        } else {
            CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    }
}

fn die(msg: &str) -> ! {
    eprintln!("{msg}: {}", last_err());
    std::process::exit(1)
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let webc = CString::new("/tmp/test/python--python@3.13.5.webc").unwrap();
    let cache = CString::new("/tmp/lingodb-wasix-cache").unwrap();
    let site = std::env::args().nth(1).map(|s| CString::new(s).unwrap());

    eprintln!("[1] session_new");
    let sess: *mut Session = unsafe {
        lingodb_wasix_session_new(
            webc.as_ptr(),
            site.as_ref().map(|c| c.as_ptr()).unwrap_or(ptr::null()),
            cache.as_ptr(),
        )
    };
    if sess.is_null() {
        die("session_new failed");
    }
    eprintln!("    session_new OK (Py_Initialize already called)");

    // Look up the Py_* + malloc/free exports we need.
    let lookup = |name: &str| -> i32 {
        let n = CString::new(name).unwrap();
        let idx = unsafe { lingodb_wasix_lookup_func(sess, n.as_ptr()) };
        if idx < 0 {
            eprintln!("missing export: {name}");
            std::process::exit(1);
        }
        idx
    };
    let malloc_idx = lookup("malloc");
    let free_idx = lookup("free");
    let pyrun_idx = lookup("PyRun_SimpleString");

    let code = if site.is_some() {
        b"import sys\nsys.path.insert(0, '/site-packages')\nimport numpy as np\na = np.arange(1,11,dtype=np.float64)\nprint('numpy', np.__version__, 'a@a =', a @ a)\n".to_vec()
    } else {
        b"import sys, platform\nprint('python', platform.python_version(), 'argv', sys.argv)\n".to_vec()
    };
    let len_with_nul = (code.len() as u32) + 1;

    // malloc(N)
    let mut malloc_args = [WasmVal {
        kind: WASM_I32,
        _pad: [0; 7],
        of: wasix_python_bridge::WasmValOf {
            i32: len_with_nul as i32,
        },
    }];
    let mut malloc_res = [WasmVal {
        kind: WASM_I32,
        _pad: [0; 7],
        of: wasix_python_bridge::WasmValOf { i32: 0 },
    }];
    if unsafe {
        lingodb_wasix_call(
            sess,
            malloc_idx,
            malloc_args.as_mut_ptr(),
            1,
            malloc_res.as_mut_ptr(),
            1,
        )
    } != 0
    {
        die("malloc trapped");
    }
    let ptr_in_guest = unsafe { malloc_res[0].of.i32 as u32 };
    eprintln!("[2] malloc({len_with_nul}) -> 0x{ptr_in_guest:x}");

    // Copy code into linear memory.
    unsafe {
        let base = lingodb_wasix_memory_base(sess);
        let size = lingodb_wasix_memory_size(sess);
        assert!((ptr_in_guest as usize) + (len_with_nul as usize) <= size);
        let dst = base.add(ptr_in_guest as usize);
        std::ptr::copy_nonoverlapping(code.as_ptr(), dst, code.len());
        *dst.add(code.len()) = 0;
    }

    // PyRun_SimpleString(ptr)
    let mut run_args = [WasmVal {
        kind: WASM_I32,
        _pad: [0; 7],
        of: wasix_python_bridge::WasmValOf {
            i32: ptr_in_guest as i32,
        },
    }];
    let mut run_res = [WasmVal {
        kind: WASM_I32,
        _pad: [0; 7],
        of: wasix_python_bridge::WasmValOf { i32: 0 },
    }];
    eprintln!("[3] PyRun_SimpleString — output below ↓");
    if unsafe {
        lingodb_wasix_call(
            sess,
            pyrun_idx,
            run_args.as_mut_ptr(),
            1,
            run_res.as_mut_ptr(),
            1,
        )
    } != 0
    {
        die("PyRun_SimpleString trapped");
    }
    let rc = unsafe { run_res[0].of.i32 };
    eprintln!("    PyRun_SimpleString rc = {rc}");

    // free(ptr)
    let mut free_args = [WasmVal {
        kind: WASM_I32,
        _pad: [0; 7],
        of: wasix_python_bridge::WasmValOf {
            i32: ptr_in_guest as i32,
        },
    }];
    if unsafe {
        lingodb_wasix_call(sess, free_idx, free_args.as_mut_ptr(), 1, ptr::null_mut(), 0)
    } != 0
    {
        die("free trapped");
    }

    eprintln!("[4] session_free");
    unsafe { lingodb_wasix_session_free(sess) };
    eprintln!("smoke OK");
    if rc != 0 {
        std::process::exit(1);
    }
}
