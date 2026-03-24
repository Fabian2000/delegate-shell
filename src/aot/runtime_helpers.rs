//! Runtime helper functions exported for AOT-compiled binaries.
//!
//! These functions are linked into the standalone binary and called by the
//! generated native code. The existing JIT helpers in jit.rs are also exported
//! with `#[no_mangle]` so the linker can resolve them.

use crate::interpreter::Runtime;
use crate::vm::bytecode::{self, Chunk};
use crate::vm::machine::VM;
use crate::vm::jit::set_jit_context;

/// Context structure that keeps the AOT runtime alive.
struct AotContext {
    _vm: Box<VM>,
    _runtime: Box<Runtime>,
    _chunks: Box<Vec<Chunk>>,
}

/// Initialize the AOT runtime. Called by the generated `main` entry point.
///
/// `chunks_data` points to serialized chunk data embedded in the binary.
/// `chunks_len` is the byte length of that data.
///
/// Returns a pointer to the AotContext (opaque to the caller).
/// Sets up thread-local pointers so JIT helper functions can access VM/Runtime.
#[unsafe(no_mangle)]
pub extern "C" fn dgsh_aot_init(chunks_data: *const u8, chunks_len: u64) -> *mut std::ffi::c_void {
    // Deserialize chunks from embedded data
    let data = unsafe { std::slice::from_raw_parts(chunks_data, chunks_len as usize) };
    let chunks_vec = bytecode::deserialize_chunks(data).unwrap_or_else(|| {
        eprintln!("Failed to deserialize embedded bytecode chunks");
        std::process::exit(1);
    });

    let runtime = Box::new(Runtime::new().unwrap_or_else(|e| {
        eprintln!("Failed to initialize runtime: {e}");
        std::process::exit(1);
    }));
    let mut vm = Box::new(VM::new());
    let chunks = Box::new(chunks_vec);

    // Set up globals from chunk 0
    if !chunks.is_empty() {
        let num_globals = chunks[0].global_names.len();
        vm.init_globals_for_aot(num_globals, &chunks[0].global_slots, &chunks[0].global_names);
    }

    // Create context FIRST, then take stable pointers from the heap-allocated boxes
    let ctx = Box::into_raw(Box::new(AotContext {
        _vm: vm,
        _runtime: runtime,
        _chunks: chunks,
    }));

    // Now the pointers are stable because AotContext is pinned on the heap
    unsafe {
        let vm_ptr: *mut VM = &mut *(*ctx)._vm;
        let runtime_ptr: *mut Runtime = &mut *(*ctx)._runtime;
        let chunks_ptr: *const Vec<Chunk> = &*(*ctx)._chunks;
        set_jit_context(vm_ptr, runtime_ptr, chunks_ptr, 0);
    }

    ctx as *mut std::ffi::c_void
}

/// Set the current chunk index for constant pool lookups in JIT helpers.
#[unsafe(no_mangle)]
pub extern "C" fn dgsh_aot_set_chunk(idx: u64) {
    crate::vm::jit::set_jit_chunk_idx(idx as usize);
}

/// Load a string constant from the embedded chunks at runtime.
/// Returns a NaN-boxed Value::string.
#[unsafe(no_mangle)]
pub extern "C" fn dgsh_aot_load_const(chunk_idx: u64, const_idx: u64) -> u64 {
    use crate::interpreter::value::Value;
    crate::vm::jit::with_chunks_ptr(|chunks_ptr: *const Vec<crate::vm::bytecode::Chunk>| {
        if chunks_ptr.is_null() {
            return Value::void().raw();
        }
        let chunks = unsafe { &*chunks_ptr };
        if (chunk_idx as usize) < chunks.len() {
            let s = chunks[chunk_idx as usize].constants.get(const_idx as u16);
            let val = Value::string_from(s.as_ref());
            let raw = val.raw();
            std::mem::forget(val);
            raw
        } else {
            Value::void().raw()
        }
    })
}

/// Clean up the AOT runtime. Called after the compiled code finishes.
#[unsafe(no_mangle)]
pub extern "C" fn dgsh_aot_cleanup(ctx: *mut std::ffi::c_void) {
    if ctx.is_null() {
        return;
    }
    unsafe {
        let _ = Box::from_raw(ctx as *mut AotContext);
        // AotContext drops here, freeing VM, Runtime, and Chunks
    }
}
