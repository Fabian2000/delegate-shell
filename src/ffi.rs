//! C-ABI FFI bindings for embedding dgsh in other languages.
//!
//! Usage from C:
//! ```c
//! DgshEngine* engine = dgsh_new();
//! dgsh_run_source(engine, "println(\"hello\")");
//! dgsh_free(engine);
//! ```

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::ptr;

use crate::interpreter::engine::Interpreter;
use crate::interpreter::value::Value;

/// Opaque engine handle for FFI consumers.
pub struct DgshEngine {
    interp: Interpreter,
    last_error: Option<CString>,
}

/// Opaque value handle for FFI consumers.
pub struct DgshValue {
    value: Value,
    /// Cached string representation (kept alive for dgsh_value_to_string)
    cached_str: Option<CString>,
}

// ---------------------------------------------------------------------------
// Engine lifecycle
// ---------------------------------------------------------------------------

/// Create a new dgsh engine with all builtins. Returns null on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dgsh_new() -> *mut DgshEngine {
    match Interpreter::new() {
        Ok(interp) => Box::into_raw(Box::new(DgshEngine {
            interp,
            last_error: None,
        })),
        Err(_) => ptr::null_mut(),
    }
}

/// Create a sandboxed engine (core builtins only, no exec, no network).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dgsh_new_sandboxed() -> *mut DgshEngine {
    match Interpreter::sandboxed() {
        Ok(interp) => Box::into_raw(Box::new(DgshEngine {
            interp,
            last_error: None,
        })),
        Err(_) => ptr::null_mut(),
    }
}

/// Free a dgsh engine. Must be called exactly once per engine.
///
/// # Safety
///
/// `engine` must be a valid pointer returned by `dgsh_new` or `dgsh_new_sandboxed`,
/// and must not have been freed already.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dgsh_free(engine: *mut DgshEngine) {
    if !engine.is_null() {
        drop(Box::from_raw(engine));
    }
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

/// Run dgsh source code. Returns 0 on success, -1 on error.
/// Use dgsh_last_error() to get the error message.
///
/// # Safety
///
/// `engine` must be a valid pointer. `source` must be a valid null-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dgsh_run_source(engine: *mut DgshEngine, source: *const c_char) -> i32 {
    if engine.is_null() || source.is_null() {
        return -1;
    }
    let engine = &mut *engine;
    let source = match CStr::from_ptr(source).to_str() {
        Ok(s) => s,
        Err(_) => {
            engine.last_error = CString::new("Invalid UTF-8 in source").ok();
            return -1;
        }
    };
    match engine.interp.run_source(source) {
        Ok(()) => {
            engine.last_error = None;
            0
        }
        Err(e) => {
            engine.last_error = CString::new(e).ok();
            -1
        }
    }
}

/// Run a dgsh script file. Returns 0 on success, -1 on error.
///
/// # Safety
///
/// `engine` must be a valid pointer. `path` must be a valid null-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dgsh_run_file(engine: *mut DgshEngine, path: *const c_char) -> i32 {
    if engine.is_null() || path.is_null() {
        return -1;
    }
    let engine = &mut *engine;
    let path = match CStr::from_ptr(path).to_str() {
        Ok(s) => s,
        Err(_) => {
            engine.last_error = CString::new("Invalid UTF-8 in path").ok();
            return -1;
        }
    };
    match engine.interp.run_file(path) {
        Ok(()) => {
            engine.last_error = None;
            0
        }
        Err(e) => {
            engine.last_error = CString::new(e).ok();
            -1
        }
    }
}

// ---------------------------------------------------------------------------
// Function calls
// ---------------------------------------------------------------------------

/// Call a dgsh function by name. Returns a DgshValue* on success, null on error.
///
/// # Safety
///
/// `engine` must be valid. `name` must be null-terminated UTF-8.
/// `args` must point to `argc` valid DgshValue pointers (or be null if argc == 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dgsh_call(
    engine: *mut DgshEngine,
    name: *const c_char,
    args: *const *mut DgshValue,
    argc: i32,
) -> *mut DgshValue {
    if engine.is_null() || name.is_null() {
        return ptr::null_mut();
    }
    let engine = &mut *engine;
    let name = match CStr::from_ptr(name).to_str() {
        Ok(s) => s,
        Err(_) => {
            engine.last_error = CString::new("Invalid UTF-8 in function name").ok();
            return ptr::null_mut();
        }
    };

    let mut rust_args = Vec::with_capacity(argc.max(0) as usize);
    for i in 0..argc {
        let arg_ptr = *args.add(i as usize);
        if arg_ptr.is_null() {
            rust_args.push(Value::void());
        } else {
            rust_args.push((*arg_ptr).value.clone());
        }
    }

    match engine.interp.call_function(name, rust_args) {
        Ok(val) => Box::into_raw(Box::new(DgshValue {
            value: val,
            cached_str: None,
        })),
        Err(e) => {
            engine.last_error = CString::new(e).ok();
            ptr::null_mut()
        }
    }
}

// ---------------------------------------------------------------------------
// Value creation
// ---------------------------------------------------------------------------

/// Create an integer value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dgsh_value_int(n: i64) -> *mut DgshValue {
    Box::into_raw(Box::new(DgshValue {
        value: Value::int(n),
        cached_str: None,
    }))
}

/// Create a float value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dgsh_value_float(n: f64) -> *mut DgshValue {
    Box::into_raw(Box::new(DgshValue {
        value: Value::float(n),
        cached_str: None,
    }))
}

/// Create a string value.
///
/// # Safety
///
/// `s` must be a valid null-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dgsh_value_string(s: *const c_char) -> *mut DgshValue {
    if s.is_null() {
        return Box::into_raw(Box::new(DgshValue {
            value: Value::string_from(""),
            cached_str: None,
        }));
    }
    let s = CStr::from_ptr(s).to_str().unwrap_or("");
    Box::into_raw(Box::new(DgshValue {
        value: Value::string_from(s),
        cached_str: None,
    }))
}

/// Create a boolean value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dgsh_value_bool(b: i32) -> *mut DgshValue {
    Box::into_raw(Box::new(DgshValue {
        value: Value::bool(b != 0),
        cached_str: None,
    }))
}

/// Create a void value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dgsh_value_void() -> *mut DgshValue {
    Box::into_raw(Box::new(DgshValue {
        value: Value::void(),
        cached_str: None,
    }))
}

// ---------------------------------------------------------------------------
// Value reading
// ---------------------------------------------------------------------------

/// Get string representation of a value. The returned pointer is valid until
/// the DgshValue is freed or this function is called again on the same value.
///
/// # Safety
///
/// `val` must be a valid DgshValue pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dgsh_value_to_string(val: *mut DgshValue) -> *const c_char {
    if val.is_null() {
        return ptr::null();
    }
    let val = &mut *val;
    let s = val.value.to_string();
    let cstr = CString::new(s).unwrap_or_default();
    let ptr = cstr.as_ptr();
    val.cached_str = Some(cstr);
    ptr
}

/// Get integer value. Returns 0 if not an int.
///
/// # Safety
///
/// `val` must be a valid DgshValue pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dgsh_value_to_int(val: *const DgshValue) -> i64 {
    if val.is_null() {
        return 0;
    }
    (*val).value.as_int().unwrap_or(0)
}

/// Get float value. Returns 0.0 if not a float.
///
/// # Safety
///
/// `val` must be a valid DgshValue pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dgsh_value_to_float(val: *const DgshValue) -> f64 {
    if val.is_null() {
        return 0.0;
    }
    (*val).value.as_float().unwrap_or(0.0)
}

/// Get boolean value. Returns 0 (false) if not a bool.
///
/// # Safety
///
/// `val` must be a valid DgshValue pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dgsh_value_to_bool(val: *const DgshValue) -> i32 {
    if val.is_null() {
        return 0;
    }
    match (*val).value.as_bool() {
        Some(true) => 1,
        _ => 0,
    }
}

/// Get the type name of a value ("int", "string", "list", etc.).
/// The returned pointer is valid until the DgshValue is freed.
///
/// # Safety
///
/// `val` must be a valid DgshValue pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dgsh_value_type(val: *mut DgshValue) -> *const c_char {
    if val.is_null() {
        return ptr::null();
    }
    let val = &mut *val;
    let type_name = (*val).value.type_name();
    let cstr = CString::new(type_name).unwrap_or_default();
    let ptr = cstr.as_ptr();
    val.cached_str = Some(cstr);
    ptr
}

/// Free a value. Must be called exactly once per value.
///
/// # Safety
///
/// `val` must be a valid pointer returned by dgsh_call, dgsh_value_int, etc.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dgsh_value_free(val: *mut DgshValue) {
    if !val.is_null() {
        drop(Box::from_raw(val));
    }
}

// ---------------------------------------------------------------------------
// Error handling
// ---------------------------------------------------------------------------

/// Get the last error message. Returns null if no error.
/// The returned pointer is valid until the next operation on the engine.
///
/// # Safety
///
/// `engine` must be a valid DgshEngine pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dgsh_last_error(engine: *const DgshEngine) -> *const c_char {
    if engine.is_null() {
        return ptr::null();
    }
    match &(*engine).last_error {
        Some(e) => e.as_ptr(),
        None => ptr::null(),
    }
}

// ---------------------------------------------------------------------------
// Custom function registration (callbacks)
// ---------------------------------------------------------------------------

/// Callback type for custom functions registered from C.
/// Receives: user_data pointer, args array, arg count.
/// Must return a DgshValue* (caller owns it) or null on error.
pub type DgshCallback = unsafe extern "C" fn(
    user_data: *mut std::ffi::c_void,
    args: *const *const DgshValue,
    argc: i32,
) -> *mut DgshValue;

/// Register a custom function. Returns 0 on success, -1 on error.
///
/// # Safety
///
/// `engine`, `name` must be valid. `callback` must be a valid function pointer.
/// `user_data` is passed through to the callback and must remain valid for the engine's lifetime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dgsh_register(
    engine: *mut DgshEngine,
    name: *const c_char,
    callback: DgshCallback,
    user_data: *mut std::ffi::c_void,
) -> i32 {
    if engine.is_null() || name.is_null() {
        return -1;
    }
    let engine = &mut *engine;
    let name = match CStr::from_ptr(name).to_str() {
        Ok(s) => s.to_string(),
        Err(_) => {
            engine.last_error = CString::new("Invalid UTF-8 in function name").ok();
            return -1;
        }
    };

    let ud = user_data as usize; // Cast to usize to make it Send-safe
    let cb = callback;

    let result = engine.interp.register(
        &name,
        &[],
        crate::builtins::registry::Type::Dyn,
        move |args: &[Value], _interp: &mut Interpreter| {
            let mut ffi_args: Vec<*const DgshValue> = Vec::with_capacity(args.len());
            let mut ffi_vals: Vec<Box<DgshValue>> = Vec::with_capacity(args.len());
            for arg in args {
                let boxed = Box::new(DgshValue {
                    value: arg.clone(),
                    cached_str: None,
                });
                ffi_args.push(&*boxed as *const DgshValue);
                ffi_vals.push(boxed);
            }
            let result_ptr = unsafe {
                cb(ud as *mut std::ffi::c_void, ffi_args.as_ptr(), args.len() as i32)
            };
            if result_ptr.is_null() {
                Err("Callback returned null".to_string())
            } else {
                let result = unsafe { Box::from_raw(result_ptr) };
                Ok(result.value.clone())
            }
        },
    );

    match result {
        Ok(()) => 0,
        Err(e) => {
            engine.last_error = CString::new(e).ok();
            -1
        }
    }
}

/// Register a custom function, overriding any existing one. Returns 0 on success, -1 on error.
///
/// # Safety
///
/// Same requirements as `dgsh_register`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dgsh_register_override(
    engine: *mut DgshEngine,
    name: *const c_char,
    callback: DgshCallback,
    user_data: *mut std::ffi::c_void,
) -> i32 {
    if engine.is_null() || name.is_null() {
        return -1;
    }
    let engine = &mut *engine;
    let name = match CStr::from_ptr(name).to_str() {
        Ok(s) => s.to_string(),
        Err(_) => {
            engine.last_error = CString::new("Invalid UTF-8 in function name").ok();
            return -1;
        }
    };

    let ud = user_data as usize;
    let cb = callback;

    let result = engine.interp.register_override(
        &name,
        &[],
        crate::builtins::registry::Type::Dyn,
        move |args: &[Value], _interp: &mut Interpreter| {
            let mut ffi_args: Vec<*const DgshValue> = Vec::with_capacity(args.len());
            let mut ffi_vals: Vec<Box<DgshValue>> = Vec::with_capacity(args.len());
            for arg in args {
                let boxed = Box::new(DgshValue {
                    value: arg.clone(),
                    cached_str: None,
                });
                ffi_args.push(&*boxed as *const DgshValue);
                ffi_vals.push(boxed);
            }
            let result_ptr = unsafe {
                cb(ud as *mut std::ffi::c_void, ffi_args.as_ptr(), args.len() as i32)
            };
            if result_ptr.is_null() {
                Err("Callback returned null".to_string())
            } else {
                let result = unsafe { Box::from_raw(result_ptr) };
                Ok(result.value.clone())
            }
        },
    );

    match result {
        Ok(()) => 0,
        Err(e) => {
            engine.last_error = CString::new(e).ok();
            -1
        }
    }
}

// ---------------------------------------------------------------------------
// Lambda calls
// ---------------------------------------------------------------------------

/// Call a lambda value. Returns a DgshValue* on success, null on error.
///
/// # Safety
///
/// `engine` must be valid. `lambda` must be a valid DgshValue containing a lambda.
/// `args` must point to `argc` valid DgshValue pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dgsh_call_lambda(
    engine: *mut DgshEngine,
    lambda: *const DgshValue,
    args: *const *mut DgshValue,
    argc: i32,
) -> *mut DgshValue {
    if engine.is_null() || lambda.is_null() {
        return ptr::null_mut();
    }
    let engine = &mut *engine;

    let mut rust_args = Vec::with_capacity(argc.max(0) as usize);
    for i in 0..argc {
        let arg_ptr = *args.add(i as usize);
        if arg_ptr.is_null() {
            rust_args.push(Value::void());
        } else {
            rust_args.push((*arg_ptr).value.clone());
        }
    }

    match engine.interp.call_lambda(&(*lambda).value, rust_args) {
        Ok(val) => Box::into_raw(Box::new(DgshValue {
            value: val,
            cached_str: None,
        })),
        Err(e) => {
            engine.last_error = CString::new(e).ok();
            ptr::null_mut()
        }
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Disable external executable access.
///
/// # Safety
///
/// `engine` must be a valid DgshEngine pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dgsh_set_allow_exec(engine: *mut DgshEngine, allow: i32) {
    if !engine.is_null() {
        (*engine).interp.set_allow_exec(allow != 0);
    }
}

/// Disable network access.
///
/// # Safety
///
/// `engine` must be a valid DgshEngine pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dgsh_set_allow_network(engine: *mut DgshEngine, allow: i32) {
    if !engine.is_null() {
        (*engine).interp.set_allow_network(allow != 0);
    }
}

/// Create an engine with a specific builtin access level.
/// 0 = All, 1 = Core, 2 = None. Returns null on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dgsh_new_with_access(level: i32) -> *mut DgshEngine {
    let access = match level {
        1 => crate::builtins::registry::BuiltinAccess::Core,
        2 => crate::builtins::registry::BuiltinAccess::None,
        _ => crate::builtins::registry::BuiltinAccess::All,
    };
    match Interpreter::with_access(access) {
        Ok(interp) => Box::into_raw(Box::new(DgshEngine {
            interp,
            last_error: None,
        })),
        Err(_) => ptr::null_mut(),
    }
}

/// Set interactive mode for executables (stdin/stdout/stderr inherited).
///
/// # Safety
///
/// `engine` must be a valid DgshEngine pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dgsh_set_interactive(engine: *mut DgshEngine, interactive: i32) {
    if !engine.is_null() {
        (*engine).interp.set_interactive(interactive != 0);
    }
}

/// Set execution mode. 0 = Auto, 1 = TreeWalk, 2 = VM, 3 = JIT.
/// Must be called before any code is executed. Returns 0 on success, -1 on error.
///
/// # Safety
///
/// `engine` must be a valid DgshEngine pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dgsh_set_execution_mode(engine: *mut DgshEngine, mode: i32) -> i32 {
    if engine.is_null() {
        return -1;
    }
    let engine = &mut *engine;
    let mode = match mode {
        1 => crate::vm::ExecutionMode::TreeWalk,
        2 => crate::vm::ExecutionMode::Vm,
        3 => crate::vm::ExecutionMode::Jit,
        _ => crate::vm::ExecutionMode::Auto,
    };
    match engine.interp.set_execution_mode(mode) {
        Ok(()) => 0,
        Err(e) => {
            engine.last_error = CString::new(e).ok();
            -1
        }
    }
}

/// Set the debug file name for display in debug output.
///
/// # Safety
///
/// `engine` must be valid. `file` must be null-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dgsh_set_debug_file(engine: *mut DgshEngine, file: *const c_char) {
    if engine.is_null() || file.is_null() {
        return;
    }
    if let Ok(f) = CStr::from_ptr(file).to_str() {
        (*engine).interp.set_debug_file(f);
    }
}

/// Debug callback type. Receives a JSON string with debug context.
/// Must return: 0 = Continue, 1 = StepOver, 2 = StepInto, 3 = Quit.
pub type DgshDebugCallback = unsafe extern "C" fn(
    user_data: *mut std::ffi::c_void,
    context_json: *const c_char,
) -> i32;

/// Set a debug handler. Called when debugger() is hit or when stepping.
///
/// # Safety
///
/// `engine` must be valid. `callback` must be a valid function pointer.
/// `user_data` is passed through and must remain valid for the engine's lifetime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dgsh_on_debug(
    engine: *mut DgshEngine,
    callback: DgshDebugCallback,
    user_data: *mut std::ffi::c_void,
) {
    if engine.is_null() {
        return;
    }
    let engine = &mut *engine;
    let ud = user_data as usize;
    let cb = callback;

    engine.interp.on_debug(move |ctx| {
        use crate::interpreter::engine::DebugAction;
        // Serialize context to JSON
        let vars: Vec<String> = ctx.variables.iter()
            .map(|(n, v, t)| format!("{{\"name\":\"{n}\",\"value\":\"{}\",\"type\":\"{t}\"}}", v.replace('"', "\\\"")))
            .collect();
        let source: Vec<String> = ctx.source_context.iter()
            .map(|(num, content, current)| format!("{{\"line\":{num},\"code\":\"{}\",\"current\":{current}}}", content.replace('"', "\\\"")))
            .collect();
        let params: Vec<String> = ctx.function_params.iter()
            .map(|(k, v)| format!("{{\"name\":\"{k}\",\"value\":\"{}\"}}", v.replace('"', "\\\"")))
            .collect();
        let json = format!(
            "{{\"line\":{},\"column\":{},\"file\":\"{}\",\"statement\":\"{}\",\"function\":\"{}\",\"params\":[{}],\"variables\":[{}],\"call_stack\":[{}],\"source\":[{}]}}",
            ctx.line,
            ctx.column,
            ctx.file.replace('"', "\\\""),
            ctx.statement.replace('"', "\\\""),
            ctx.function_name.replace('"', "\\\""),
            params.join(","),
            vars.join(","),
            ctx.call_stack.iter().map(|s| format!("\"{}\"", s.replace('"', "\\\""))).collect::<Vec<_>>().join(","),
            source.join(","),
        );
        let cstr = CString::new(json).unwrap_or_default();
        let result = unsafe { cb(ud as *mut std::ffi::c_void, cstr.as_ptr()) };
        match result {
            0 => DebugAction::Continue,
            1 => DebugAction::StepOver,
            2 => DebugAction::StepInto,
            3 => DebugAction::Quit,
            _ => DebugAction::Continue,
        }
    });
}

/// Get all registered builtin function names as a newline-separated string.
/// The returned pointer is valid until the next call to this function on the same engine.
///
/// # Safety
///
/// `engine` must be a valid DgshEngine pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dgsh_builtin_names(engine: *mut DgshEngine) -> *const c_char {
    if engine.is_null() {
        return ptr::null();
    }
    let engine = &mut *engine;
    let names = engine.interp.builtin_names().join("\n");
    let cstr = CString::new(names).unwrap_or_default();
    let ptr = cstr.as_ptr();
    engine.last_error = Some(cstr); // reuse last_error storage for lifetime
    ptr
}
