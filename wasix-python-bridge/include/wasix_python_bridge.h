/* Public C header for the wasix-python-bridge Rust crate.
 *
 * The crate wraps wasmer + wasmer-wasix and exposes a minimal C ABI for the
 * lingodb runtime to drive a persistent WASIX-CPython interpreter. See
 * src/lib.rs for the implementation; the layout here mirrors WasmVal in that
 * file byte-for-byte. */

#ifndef LINGODB_WASIX_PYTHON_BRIDGE_H
#define LINGODB_WASIX_PYTHON_BRIDGE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Opaque session handle (one per worker, holds the wasm Store + Instance +
 * WasiEnv + cached function table). */
typedef struct lingodb_wasix_session lingodb_wasix_session_t;

/* Mirrors wasm.h's wasm_val_t: 1-byte kind + 7B padding + 8B union. */
typedef struct {
   uint8_t kind; /* 0=I32 1=I64 2=F32 3=F64 (matches WASM_I32/I64/F32/F64) */
   uint8_t _pad[7];
   union {
      int32_t i32;
      int64_t i64;
      float f32;
      double f64;
      void* _ref;
   } of;
} lingodb_wasm_val_t;

#define LINGODB_WASM_I32 0
#define LINGODB_WASM_I64 1
#define LINGODB_WASM_F32 2
#define LINGODB_WASM_F64 3

/* Construct a session: load the webc, mount optional site-packages, set
 * up the module cache, instantiate, run __wasm_call_ctors / __wasi_init_tp /
 * Py_Initialize. Returns NULL on failure (use lingodb_wasix_last_error). */
lingodb_wasix_session_t* lingodb_wasix_session_new(
   const char* webc_path,
   const char* site_packages_host_dir, /* nullable */
   const char* module_cache_dir);       /* nullable, enables FileSystemCache */

void lingodb_wasix_session_free(lingodb_wasix_session_t* sess);

/* Resolve an exported function by name. Returns a session-local index (used
 * with lingodb_wasix_call) or -1 if missing. Cache the index — the lookup
 * walks the module's exports linearly. */
int32_t lingodb_wasix_lookup_func(
   lingodb_wasix_session_t* sess, const char* name);

/* Invoke an exported function. 0 on success, -1 on trap/error. Trap message
 * via lingodb_wasix_last_error. args/results may be NULL when the
 * corresponding count is 0. */
int lingodb_wasix_call(
   lingodb_wasix_session_t* sess,
   int32_t func_idx,
   const lingodb_wasm_val_t* args, size_t nargs,
   lingodb_wasm_val_t* results, size_t nresults);

/* Native pointer to the start of the guest's linear memory. Invalidated by
 * any guest memory.grow — recompute before each access. */
uint8_t* lingodb_wasix_memory_base(lingodb_wasix_session_t* sess);
size_t   lingodb_wasix_memory_size(lingodb_wasix_session_t* sess);

/* Thread-local last error message. NULL when nothing was set. */
const char* lingodb_wasix_last_error(void);

#ifdef __cplusplus
}
#endif

#endif /* LINGODB_WASIX_PYTHON_BRIDGE_H */
