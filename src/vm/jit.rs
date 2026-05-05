
use cranelift_codegen::ir::{types, AbiParam, Function, InstBuilder, UserFuncName, StackSlotData, StackSlotKind};
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module};

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::cell::Cell;
use super::bytecode::{Chunk, Op};
use crate::interpreter::value::{Value, ValueKind as VK, new_list, new_object};
use crate::interpreter::Runtime;
use crate::parser::ast::Resolution;

// Thread-local context for JIT helpers — avoids passing raw pointers through JIT'd code
thread_local! {
    static JIT_VM_PTR: Cell<*mut super::machine::VM> = const { Cell::new(std::ptr::null_mut()) };
    static JIT_INTERP_PTR: Cell<*mut Runtime> = const { Cell::new(std::ptr::null_mut()) };
    static JIT_CHUNKS_PTR: Cell<*const Vec<Chunk>> = const { Cell::new(std::ptr::null()) };
    static JIT_CHUNK_IDX: Cell<usize> = const { Cell::new(0) };
}

pub fn set_jit_context(vm: *mut super::machine::VM, interp: *mut Runtime, chunks: *const Vec<Chunk>, chunk_idx: usize) {
    JIT_VM_PTR.with(|c| c.set(vm));
    JIT_INTERP_PTR.with(|c| c.set(interp));
    JIT_CHUNKS_PTR.with(|c| c.set(chunks));
    JIT_CHUNK_IDX.with(|c| c.set(chunk_idx));
}

pub fn with_chunks_ptr<R>(f: impl FnOnce(*const Vec<Chunk>) -> R) -> R {
    JIT_CHUNKS_PTR.with(|c| f(c.get()))
}

fn get_jit_vm() -> *mut super::machine::VM {
    JIT_VM_PTR.with(|c| c.get())
}

fn get_jit_interp() -> *mut Runtime {
    JIT_INTERP_PTR.with(|c| c.get())
}

fn get_jit_chunks() -> *const Vec<Chunk> {
    JIT_CHUNKS_PTR.with(|c| c.get())
}

fn get_jit_chunk_idx() -> usize {
    JIT_CHUNK_IDX.with(|c| c.get())
}

pub fn set_jit_chunk_idx(idx: usize) {
    JIT_CHUNK_IDX.with(|c| c.set(idx));
}

/// Snapshot of the active VM's bytecode and function registry, used so a
/// freshly spawned thread can construct its own isolated VM (per the language
/// design: threads share nothing with the parent except atomics passed in).
///
/// `Chunk` contains `Rc<str>` constants (not `Send`), but the snapshot is a
/// fresh deep clone of immutable bytecode. Sending it to a worker is safe
/// because nothing in the parent will read or mutate this copy concurrently.
pub struct VmSnapshot {
    pub chunks: Vec<Chunk>,
    pub fn_table: std::collections::HashMap<String, usize>,
}
// Safety: Rc<str> is not Send by default, but the chunks here are a private,
// immutable clone — there are no other handles to these Rcs in the source
// thread, so transferring ownership across threads cannot race.
unsafe impl Send for VmSnapshot {}

/// Returns `None` when no JIT/AOT context is active in the calling thread.
pub fn snapshot_vm_state() -> Option<VmSnapshot> {
    let vm_ptr = get_jit_vm();
    if vm_ptr.is_null() {
        return None;
    }
    unsafe {
        let vm = &*vm_ptr;
        if vm.chunks.is_empty() {
            return None;
        }
        Some(VmSnapshot { chunks: vm.chunks.clone(), fn_table: vm.fn_table_snapshot() })
    }
}

/// Look up a user function in the active VM's function table and execute it.
///
/// Used by the interpreter as a fallback in call_user_fn: in AOT mode user
/// functions live as VM-compiled chunks (registered via Op::DefineFunction),
/// not as AST entries in `env.functions`. When a builtin like filter calls
/// back into a user function, the interpreter has no AST to walk, so it asks
/// the VM to execute the chunk directly.
///
/// Returns `None` when no JIT/AOT context is active or the name is unknown.
pub fn try_vm_call(name: &str, args: Vec<Value>) -> Option<Result<Value, String>> {
    let vm_ptr = get_jit_vm();
    let interp_ptr = get_jit_interp();
    if vm_ptr.is_null() || interp_ptr.is_null() {
        return None;
    }
    unsafe {
        let vm = &mut *vm_ptr;
        let fn_chunk = *vm.fn_table_lookup(name)?;
        let interp = &mut *interp_ptr;
        let base = vm.stack.len();
        for arg in &args {
            vm.stack.push(arg.clone());
        }
        vm.frames.push(super::machine::CallFrame {
            chunk_idx: fn_chunk,
            ip: 0,
            base,
        });
        Some(vm.run_frame(interp))
    }
}

// ---------------------------------------------------------------------------
// extern "C" helpers — called from JIT'd code
// ---------------------------------------------------------------------------

// Op codes for jit_binary_op
const BOP_ADD: u64 = 0;
const BOP_SUB: u64 = 1;
const BOP_MUL: u64 = 2;
const BOP_DIV: u64 = 3;
const BOP_MOD: u64 = 4;
const BOP_EQ: u64 = 5;
const BOP_NEQ: u64 = 6;
const BOP_LT: u64 = 7;
const BOP_GT: u64 = 8;
const BOP_LTE: u64 = 9;
const BOP_GTE: u64 = 10;
const BOP_BITAND: u64 = 11;
const BOP_BITOR: u64 = 12;
const BOP_BITXOR: u64 = 13;
const BOP_SHL: u64 = 14;
const BOP_SHR: u64 = 15;

/// Generic binary op for ANY types. Takes ownership of both values.
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_binary_op(left: u64, right: u64, op_code: u64) -> u64 {
    let l = Value::from_raw(left);
    let r = Value::from_raw(right);

    // Eq/Neq need refs, then we drop
    if op_code == BOP_EQ || op_code == BOP_NEQ {
        let eq = values_equal(&l, &r);
        drop(l); drop(r);
        let v = Value::bool(if op_code == BOP_EQ { eq } else { !eq });
        let raw = v.raw(); std::mem::forget(v); return raw;
    }

    // Bitwise ops — extract ints
    if op_code >= BOP_BITAND {
        let ai = l.as_int(); let bi = r.as_int();
        drop(l); drop(r);
        let v = match (ai, bi) {
            (Some(a), Some(b)) => match op_code {
                BOP_BITAND => Value::int(a & b),
                BOP_BITOR => Value::int(a | b),
                BOP_BITXOR => Value::int(a ^ b),
                BOP_SHL => Value::int(a << b),
                BOP_SHR => Value::int(a >> b),
                _ => Value::void(),
            },
            _ => Value::void(),
        };
        let raw = v.raw(); std::mem::forget(v); return raw;
    }

    // Arithmetic + comparisons: all consume values
    let result = match op_code {
        BOP_ADD => generic_add(l, r),
        BOP_SUB => generic_sub(l, r),
        BOP_MUL => generic_mul(l, r),
        BOP_DIV => generic_div(l, r),
        BOP_MOD => generic_mod(l, r),
        BOP_LT => generic_compare(l, r, |o| o.is_lt()),
        BOP_GT => generic_compare(l, r, |o| o.is_gt()),
        BOP_LTE => generic_compare(l, r, |o| o.is_le()),
        BOP_GTE => generic_compare(l, r, |o| o.is_ge()),
        _ => Ok(Value::void()),
    };

    match result {
        Ok(v) => { let raw = v.raw(); std::mem::forget(v); raw }
        Err(_) => Value::void().raw()
    }
}

/// Unary negate
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_negate(val: u64) -> u64 {
    {
        let v = Value::from_raw(val);
        let result = if let Some(n) = v.as_int() {
            Value::int(-n)
        } else if let Some(f) = v.as_float() {
            Value::float(-f)
        } else {
            Value::void()
        };
        std::mem::forget(v);
        let raw = result.raw();
        std::mem::forget(result);
        raw
    }
}

/// Unary bitwise not
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_bitnot(val: u64) -> u64 {
    {
        let v = Value::from_raw(val);
        let result = if let Some(n) = v.as_int() {
            Value::int(!n)
        } else {
            Value::void()
        };
        std::mem::forget(v);
        let raw = result.raw();
        std::mem::forget(result);
        raw
    }
}

/// Logical not
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_not(val: u64) -> u64 {
    {
        let v = Value::from_raw(val);
        let result = Value::bool(!v.is_truthy());
        std::mem::forget(v);
        let raw = result.raw();
        std::mem::forget(result);
        raw
    }
}

/// Pow
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_pow(base: u64, exp: u64) -> u64 {
    {
        let b = Value::from_raw(base);
        let e = Value::from_raw(exp);
        let result = generic_pow(b, e).unwrap_or_else(|_| Value::void());
        let raw = result.raw();
        std::mem::forget(result);
        raw
    }
}

/// Truthy check — returns 0 or 1 as raw i64
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_is_truthy(val: u64) -> u64 {
    {
        let v = Value::from_raw(val);
        let result = if v.is_truthy() { 1u64 } else { 0u64 };
        std::mem::forget(v);
        result
    }
}

/// Clone a NaN-boxed value (increment refcount if needed)
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_clone_value(val: u64) -> u64 {
    {
        let v = Value::from_raw(val);
        let cloned = v.clone();
        std::mem::forget(v);
        let raw = cloned.raw();
        std::mem::forget(cloned);
        raw
    }
}

/// Drop a NaN-boxed value (decrement refcount if needed)
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_drop_value(val: u64) {
    {
        let _ = Value::from_raw(val);
        // Value::drop runs here
    }
}


/// Load true
/// Get global variable
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_get_global(idx: u64) -> u64 {
    unsafe {
        let vm = &mut *get_jit_vm();
        let val = vm.get_global(idx as usize);
        let raw = val.raw();
        std::mem::forget(val);
        raw
    }
}

/// Set global variable
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_set_global(idx: u64, val: u64) {
    unsafe {
        let vm = &mut *get_jit_vm();
        let v = Value::from_raw(val);
        let cloned = v.clone();
        std::mem::forget(v);
        vm.set_global(idx as usize, cloned);
    }
}

/// Field get: obj.field
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_field_get(obj: u64, field_idx: u64) -> u64 {
    unsafe {
        let o = Value::from_raw(obj);
        let chunks = &*get_jit_chunks();
        let chunk_idx = get_jit_chunk_idx();
        let field: &str = chunks[chunk_idx].constants.get(field_idx as u16);

        let result = if let Some(rc) = o.as_object_ref() {
            rc.borrow().fields.get(field).cloned().unwrap_or_else(Value::void)
        } else if let Some(data) = o.as_command_result() {
            match field {
                "status" => Value::int(i64::from(data.status)),
                "out" => Value::string_from(&data.out),
                "err" => Value::string_from(&data.err),
                _ => Value::void(),
            }
        } else {
            Value::void()
        };

        std::mem::forget(o);
        let raw = result.raw();
        std::mem::forget(result);
        raw
    }
}

/// Field set: obj.field = val
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_field_set(obj: u64, field_idx: u64, val: u64) {
    unsafe {
        let o = Value::from_raw(obj);
        let v = Value::from_raw(val);
        let chunks = &*get_jit_chunks();
        let chunk_idx = get_jit_chunk_idx();
        let field: &str = chunks[chunk_idx].constants.get(field_idx as u16);
        let v_cloned = v.clone();

        if let Some(rc) = o.as_object_ref() {
            rc.borrow_mut().fields.insert(field.to_string(), v_cloned);
        }

        std::mem::forget(o);
        std::mem::forget(v);
    }
}

/// Index get: target[index]
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_index_get(target: u64, index: u64) -> u64 {
    {
        let t = Value::from_raw(target);
        let i = Value::from_raw(index);
        let result = vm_index(&t, &i).unwrap_or_else(|_| Value::void());
        std::mem::forget(t);
        std::mem::forget(i);
        let raw = result.raw();
        std::mem::forget(result);
        raw
    }
}

/// Index set: target[index] = val
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_index_set(target: u64, index: u64, val: u64) {
    {
        let t = Value::from_raw(target);
        let idx = Value::from_raw(index);
        let v = Value::from_raw(val);
        let v_cloned = v.clone();

        if let (Some(l), Some(i)) = (t.as_list_ref(), idx.as_int()) {
            let mut list = l.borrow_mut();
            let ix = if i < 0 { list.len() as i64 + i } else { i } as usize;
            if ix < list.len() {
                list[ix] = v_cloned;
            }
        } else if let (Some(rc), Some(key)) = (t.as_object_ref(), idx.as_str_ref()) {
            rc.borrow_mut().fields.insert(key.to_string(), v_cloned);
        }

        std::mem::forget(t);
        std::mem::forget(idx);
        std::mem::forget(v);
    }
}

/// Make list from items on a scratch buffer
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_make_list(items_ptr: *const u64, count: u64) -> u64 {
    unsafe {
        let raw_items = std::slice::from_raw_parts(items_ptr, count as usize);
        let items: Vec<Value> = raw_items.iter().map(|&bits| {
            let v = Value::from_raw(bits);
            let cloned = v.clone();
            std::mem::forget(v);
            cloned
        }).collect();
        let result = new_list(items);
        let raw = result.raw();
        std::mem::forget(result);
        raw
    }
}

/// Make object from key-value pairs (keys are Values, values are Values, interleaved)
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_make_object(pairs_ptr: *const u64, count: u64) -> u64 {
    unsafe {
        let raw_pairs = std::slice::from_raw_parts(pairs_ptr, (count as usize) * 2);
        let mut map = indexmap::IndexMap::new();
        for pair in raw_pairs.chunks(2) {
            let k = Value::from_raw(pair[0]);
            let v = Value::from_raw(pair[1]);
            if let Some(key_str) = k.as_str_ref() {
                let v_cloned = v.clone();
                map.insert(key_str.to_string(), v_cloned);
            }
            std::mem::forget(k);
            std::mem::forget(v);
        }
        let result = new_object(map);
        let raw = result.raw();
        std::mem::forget(result);
        raw
    }
}

/// Make string by concatenating parts
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_make_string(parts_ptr: *const u64, count: u64) -> u64 {
    unsafe {
        let raw_parts = std::slice::from_raw_parts(parts_ptr, count as usize);
        let mut result_str = String::new();
        for &bits in raw_parts {
            let v = Value::from_raw(bits);
            result_str.push_str(&format!("{v}"));
            std::mem::forget(v);
        }
        let result = Value::string_owned(result_str);
        let raw = result.raw();
        std::mem::forget(result);
        raw
    }
}

/// Make range (start..=end as list)
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_make_range(start: u64, end: u64) -> u64 {
    {
        let s = Value::from_raw(start);
        let e = Value::from_raw(end);
        let result = match (s.as_int(), e.as_int()) {
            (Some(a), Some(b)) => {
                let items: Vec<Value> = (a..=b).map(Value::int).collect();
                new_list(items)
            }
            _ => Value::void(),
        };
        std::mem::forget(s);
        std::mem::forget(e);
        let raw = result.raw();
        std::mem::forget(result);
        raw
    }
}

/// Generic call (user fn or builtin with full resolution)
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_generic_call(
    name_idx: u64,
    res: u64,
    args_ptr: *const u64,
    argc: u64,
) -> u64 {
    unsafe {
        let interp = &mut *get_jit_interp();
        let vm = &mut *get_jit_vm();
        let chunks = &*get_jit_chunks();
        let chunk_idx = get_jit_chunk_idx();
        let name = chunks[chunk_idx].constants.get(name_idx as u16).clone();

        let args: Vec<Value> = std::slice::from_raw_parts(args_ptr, argc as usize)
            .iter()
            .map(|&bits| {
                let v = Value::from_raw(bits);
                let cloned = v.clone();
                std::mem::forget(v);
                cloned
            })
            .collect();

        let resolution = match res {
            1 => Resolution::OwnFirst,
            2 => Resolution::SystemOnly,
            _ => Resolution::Normal,
        };

        // Check fn_table for user functions
        if let Some(&fn_chunk) = vm.fn_table_lookup(&name) {
            // Run via VM call frame
            let base = vm.stack.len();
            for arg in &args {
                vm.stack.push(arg.clone());
            }
            vm.frames.push(super::machine::CallFrame {
                chunk_idx: fn_chunk,
                ip: 0,
                base,
            });
            match vm.run_frame(interp) {
                Ok(val) => {
                    let raw = val.raw();
                    std::mem::forget(val);
                    return raw;
                }
                Err(_) => {
                    let v = Value::void();
                    let raw = v.raw();
                    std::mem::forget(v);
                    return raw;
                }
            }
        }

        match interp.call_resolved(&name, resolution, args) {
            Ok(val) => {
                let raw = val.raw();
                std::mem::forget(val);
                raw
            }
            Err(e) => {
                eprintln!("jit_generic_call error for '{}': {}", name, e);
                let v = Value::void();
                let raw = v.raw();
                std::mem::forget(v);
                raw
            }
        }
    }
}

/// Call builtin by name
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_call_builtin_v2(
    name_idx: u64,
    args_ptr: *const u64,
    argc: u64,
) -> u64 {
    unsafe {
        let interp = &mut *get_jit_interp();
        let chunks = &*get_jit_chunks();
        let chunk_idx = get_jit_chunk_idx();
        let name = chunks[chunk_idx].constants.get(name_idx as u16).clone();

        let args: Vec<Value> = std::slice::from_raw_parts(args_ptr, argc as usize)
            .iter()
            .map(|&bits| {
                let v = Value::from_raw(bits);
                let cloned = v.clone();
                std::mem::forget(v);
                cloned
            })
            .collect();

        // Temporarily take registry out to avoid borrow conflicts
        let reg = std::mem::take(&mut interp.registry);
        let result = reg.call(&name, &args, interp);
        interp.registry = reg;

        match result {
            Some(Ok(val)) => {
                let raw = val.raw();
                std::mem::forget(val);
                raw
            }
            _ => {
                let v = Value::void();
                let raw = v.raw();
                std::mem::forget(v);
                raw
            }
        }
    }
}

/// Make lambda
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_make_lambda(
    name_idx: u64,
    res: u64,
    args_ptr: *const u64,
    bound_count: u64,
) -> u64 {
    unsafe {
        let chunks = &*get_jit_chunks();
        let chunk_idx = get_jit_chunk_idx();
        let name = chunks[chunk_idx].constants.get(name_idx as u16).clone();

        let bound_args: Vec<Value> = std::slice::from_raw_parts(args_ptr, bound_count as usize)
            .iter()
            .map(|&bits| {
                let v = Value::from_raw(bits);
                let cloned = v.clone();
                std::mem::forget(v);
                cloned
            })
            .collect();

        let result = Value::lambda(crate::interpreter::value::LambdaData {
            name: name.to_string(),
            resolution: res as u8,
            bound_args,
        });
        let raw = result.raw();
        std::mem::forget(result);
        raw
    }
}

/// Increment/decrement helper for local values
/// op: 0=inc, 1=dec
/// mode: 0=inc(no push), 1=dec(no push), 2=post_inc, 3=post_dec, 4=pre_inc, 5=pre_dec
/// Returns: the value to push on stack (or 0 if nothing to push)
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_inc_dec(val: u64, op: u64) -> u64 {
    {
        let v = Value::from_raw(val);
        // Handle atomics: use fetch_add/fetch_sub for thread-safe increment
        if let Some(atomic) = v.as_atomic() {
            let delta: i64 = if op == 0 || op == 2 { 1 } else { -1 };
            let _ = atomic.fetch_add(delta);
            std::mem::forget(v);
            // Return the atomic value itself (not a new int) to preserve atomicity
            return val;
        }
        let result = match op {
            0 => { // inc (no push)
                if let Some(n) = v.as_int() { Value::int(n + 1) } else { v.clone() }
            }
            1 => { // dec (no push)
                if let Some(n) = v.as_int() { Value::int(n - 1) } else { v.clone() }
            }
            2 => { // post_inc (push old)
                if let Some(n) = v.as_int() { Value::int(n + 1) } else { v.clone() }
            }
            3 => { // post_dec (push old)
                if let Some(n) = v.as_int() { Value::int(n - 1) } else { v.clone() }
            }
            _ => v.clone(),
        };
        std::mem::forget(v);
        let raw = result.raw();
        std::mem::forget(result);
        raw
    }
}

/// String append for a local: local_val + rhs.
/// Takes ownership of local_val (caller must not forget it); forgets rhs.
/// If both are strings and local_val has refcount 1, appends in-place.
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_string_append_local(local_val: u64, rhs: u64) -> u64 {
    {
        let mut lv = Value::from_raw(local_val);
        let rv = Value::from_raw(rhs);
        let result = if lv.is_string() && rv.is_string() {
            if let Some(rhs_str) = rv.as_str_ref() {
                if lv.try_string_append_in_place(rhs_str) {
                    std::mem::forget(rv);
                    let raw = lv.raw();
                    std::mem::forget(lv);
                    return raw;
                }
                if let Some(a_str) = lv.as_str_ref() {
                    let mut s = String::with_capacity(a_str.len() + rhs_str.len());
                    s.push_str(a_str);
                    s.push_str(rhs_str);
                    Value::string_owned(s)
                } else {
                    lv.clone()
                }
            } else {
                lv.clone()
            }
        } else if let (Some(a), Some(b)) = (lv.as_int(), rv.as_int()) {
            Value::int(a + b)
        } else {
            // Generic add
            match generic_add(lv.clone(), rv.clone()) {
                Ok(v) => {
                    std::mem::forget(lv);
                    std::mem::forget(rv);
                    let r = v.raw();
                    std::mem::forget(v);
                    return r;
                }
                Err(_) => lv.clone(),
            }
        };
        std::mem::forget(lv);
        std::mem::forget(rv);
        let raw = result.raw();
        std::mem::forget(result);
        raw
    }
}

/// Compound add/sub for a local
/// op: 0=add, 1=sub
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_compound_op(local_val: u64, rhs: u64, op: u64) -> u64 {
    {
        let lv = Value::from_raw(local_val);
        let rv = Value::from_raw(rhs);
        let result = match op {
            0 => {
                if let (Some(a), Some(b)) = (lv.as_int(), rv.as_int()) {
                    Value::int(a + b)
                } else {
                    lv.clone()
                }
            }
            _ => {
                if let (Some(a), Some(b)) = (lv.as_int(), rv.as_int()) {
                    Value::int(a - b)
                } else {
                    lv.clone()
                }
            }
        };
        std::mem::forget(lv);
        std::mem::forget(rv);
        let raw = result.raw();
        std::mem::forget(result);
        raw
    }
}

/// SuperInstruction: SubLocalImm / AddLocalImm
/// Call a VM function by chunk index (used by CallLocal).
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_vm_call(
    chunk_idx: u64,
    args_ptr: *const u64,
    argc: u64,
) -> u64 {
    unsafe {
        let vm = &mut *get_jit_vm();
        let interp = &mut *get_jit_interp();

        vm.call_depth += 1;
        if vm.call_depth > 10000 {
            vm.call_depth -= 1;
            let v = Value::void();
            let raw = v.raw();
            std::mem::forget(v);
            return raw;
        }

        let args = std::slice::from_raw_parts(args_ptr, argc as usize);
        let base = vm.stack.len();
        for &arg_bits in args {
            let v = Value::from_raw(arg_bits);
            let cloned = v.clone();
            std::mem::forget(v);
            vm.stack.push(cloned);
        }
        vm.frames.push(super::machine::CallFrame {
            chunk_idx: chunk_idx as usize,
            ip: 0,
            base,
        });
        let result = match vm.run_frame(interp) {
            Ok(val) => {
                let raw = val.raw();
                std::mem::forget(val);
                raw
            }
            Err(msg) => {
                vm.last_error = Some(msg);
                let v = Value::void();
                let raw = v.raw();
                std::mem::forget(v);
                raw
            }
        };
        vm.call_depth -= 1;
        result
    }
}

/// Define function in fn_table
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_define_function(
    name_idx: u64,
    fn_chunk_idx: u64,
) {
    unsafe {
        let vm = &mut *get_jit_vm();
        let chunks = &*get_jit_chunks();
        let chunk_idx = get_jit_chunk_idx();
        let name = chunks[chunk_idx].constants.get(name_idx as u16).clone();
        vm.register_fn(&name, fn_chunk_idx as usize);
    }
}

/// Free a global
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_free_global(idx: u64) {
    unsafe {
        let vm = &mut *get_jit_vm();
        vm.set_global(idx as usize, Value::void());
    }
}

/// Throw (returns void; actual error handling is TODO)
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_recursion_overflow() { unsafe {
    let vm = &mut *get_jit_vm();
    vm.last_error = Some("maximum recursion depth exceeded (limit: 10000)".to_string());
}}

#[unsafe(no_mangle)]
unsafe extern "C" fn jit_throw(val: u64) -> u64 {
    {
        let v = Value::from_raw(val);
        // In a full implementation, this would propagate the error.
        // For now, just consume the value.
        let _ = format!("{v}");
        std::mem::forget(v);
        let result = Value::void();
        let raw = result.raw();
        std::mem::forget(result);
        raw
    }
}

/// ErrorCheck
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_error_check(slot: u64) -> u64 {
    unsafe {
        let vm = &*get_jit_vm();
        let is_ok = vm.error_check(slot as u16);
        let result = Value::bool(is_ok);
        let raw = result.raw();
        std::mem::forget(result);
        raw
    }
}

/// ErrorField
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_error_field(slot: u64) -> u64 {
    unsafe {
        let vm = &*get_jit_vm();
        let msg = vm.error_field(slot as u16);
        let result = Value::string(Rc::from(msg));
        let raw = result.raw();
        std::mem::forget(result);
        raw
    }
}

/// SetErrorTolerant
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_set_error_tolerant(slot: u64) {
    unsafe {
        let vm = &mut *get_jit_vm();
        vm.set_error_tolerant(slot as u16);
    }
}

/// RecordError
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_record_error(slot: u64) {
    unsafe {
        let vm = &mut *get_jit_vm();
        vm.record_error(slot as u16);
    }
}

/// OptionalCheck
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_optional_check(slot: u64) -> u64 {
    unsafe {
        let vm = &*get_jit_vm();
        let is_present = vm.optional_check(slot as usize);
        let result = Value::bool(is_present);
        let raw = result.raw();
        std::mem::forget(result);
        raw
    }
}

/// GetDollarIndex
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_get_dollar_index(idx: u64) -> u64 {
    unsafe {
        let vm = &*get_jit_vm();
        let result = vm.get_dollar_index(idx as usize).unwrap_or_else(Value::void);
        let raw = result.raw();
        std::mem::forget(result);
        raw
    }
}

/// GetDollarField
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_get_dollar_field(
    field_idx: u64,
) -> u64 {
    unsafe {
        let vm = &*get_jit_vm();
        let chunks = &*get_jit_chunks();
        let chunk_idx = get_jit_chunk_idx();
        let field = chunks[chunk_idx].constants.get(field_idx as u16).clone();
        let result = vm.get_dollar_field(&field).unwrap_or_else(Value::void);
        let raw = result.raw();
        std::mem::forget(result);
        raw
    }
}

/// GetDollar
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_get_dollar() -> u64 {
    unsafe {
        let vm = &*get_jit_vm();
        let result = vm.get_dollar().unwrap_or_else(Value::void);
        let raw = result.raw();
        std::mem::forget(result);
        raw
    }
}

/// PushSendCtx
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_push_send_ctx(val: u64) {
    unsafe {
        let vm = &mut *get_jit_vm();
        let v = Value::from_raw(val);
        let cloned = v.clone();
        std::mem::forget(v);
        vm.push_send_ctx(cloned);
    }
}

/// PopSendCtx
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_pop_send_ctx() {
    unsafe {
        let vm = &mut *get_jit_vm();
        vm.pop_send_ctx();
    }
}

/// Alias
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_alias(
    name_idx: u64,
    target_idx: u64,
) {
    unsafe {
        let interp = &mut *get_jit_interp();
        let chunks = &*get_jit_chunks();
        let chunk_idx = get_jit_chunk_idx();
        let name = chunks[chunk_idx].constants.get(name_idx as u16).clone();
        let target = chunks[chunk_idx].constants.get(target_idx as u16).clone();
        interp.env.aliases.insert(name.to_string(), target.to_string());
    }
}

/// Use
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_use(
    path_idx: u64,
    alias_idx: u64,
) {
    unsafe {
        let interp = &mut *get_jit_interp();
        let chunks = &*get_jit_chunks();
        let chunk_idx = get_jit_chunk_idx();
        let path = chunks[chunk_idx].constants.get(path_idx as u16).clone();
        let alias = if alias_idx == 0xFFFF {
            std::path::Path::new(path.as_ref())
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| path.to_string())
        } else {
            chunks[chunk_idx].constants.get(alias_idx as u16).to_string()
        };
        interp.env.use_paths.insert(alias.to_ascii_lowercase(), path.to_string());
    }
}

/// Atomic wrap
#[unsafe(no_mangle)]
unsafe extern "C" fn jit_atomic(val: u64) -> u64 {
    {
        let v = Value::from_raw(val);
        let result = Value::atomic(crate::interpreter::value::AtomicValue::new(&v));
        std::mem::forget(v);
        let raw = result.raw();
        std::mem::forget(result);
        raw
    }
}

// ---------------------------------------------------------------------------
// Arithmetic helpers — reuse from machine module, only keep JIT-specific ones
// ---------------------------------------------------------------------------
use super::machine::{generic_add, generic_sub, generic_mul, generic_div, generic_mod, generic_compare, values_equal};

fn generic_pow(a: Value, b: Value) -> Result<Value, String> {
    match (a.kind(), b.kind()) {
        (VK::Int(base), VK::Int(exp)) => {
            if let Ok(e) = u32::try_from(exp) {
                Ok(Value::int(base.pow(e)))
            } else {
                Ok(Value::float((base as f64).powf(exp as f64)))
            }
        }
        (VK::Float(x), VK::Float(y)) => Ok(Value::float(x.powf(y))),
        (VK::Int(x), VK::Float(y)) => Ok(Value::float((x as f64).powf(y))),
        (VK::Float(x), VK::Int(y)) => Ok(Value::float(x.powf(y as f64))),
        _ => Err(format!("Cannot exponentiate {} by {}", a.type_name(), b.type_name())),
    }
}

fn vm_index(target: &Value, index: &Value) -> Result<Value, String> {
    if let (Some(l), Some(i)) = (target.as_list_ref(), index.as_int()) {
        let list = l.borrow();
        let idx = if i < 0 { list.len() as i64 + i } else { i } as usize;
        list.get(idx).cloned().ok_or_else(|| format!("Index {i} out of bounds"))
    } else if let (Some(rc), Some(key)) = (target.as_object_ref(), index.as_str_ref()) {
        rc.borrow().fields.get(key).cloned()
            .ok_or_else(|| format!("Field '{key}' not found"))
    } else {
        Err(format!("Cannot index {} with {}", target.type_name(), index.type_name()))
    }
}

// ---------------------------------------------------------------------------
// Helper function name constants
// ---------------------------------------------------------------------------

const H_BINARY_OP: &str = "jit_binary_op";
const H_NEGATE: &str = "jit_negate";
const H_BITNOT: &str = "jit_bitnot";
const H_NOT: &str = "jit_not";
const H_POW: &str = "jit_pow";
const H_IS_TRUTHY: &str = "jit_is_truthy";
const H_CLONE_VALUE: &str = "jit_clone_value";
const H_DROP_VALUE: &str = "jit_drop_value";
const H_GET_GLOBAL: &str = "jit_get_global";
const H_SET_GLOBAL: &str = "jit_set_global";
const H_FIELD_GET: &str = "jit_field_get";
const H_FIELD_SET: &str = "jit_field_set";
const H_INDEX_GET: &str = "jit_index_get";
const H_INDEX_SET: &str = "jit_index_set";
const H_MAKE_LIST: &str = "jit_make_list";
const H_MAKE_OBJECT: &str = "jit_make_object";
const H_MAKE_STRING: &str = "jit_make_string";
const H_MAKE_RANGE: &str = "jit_make_range";
const H_GENERIC_CALL: &str = "jit_generic_call";
const H_CALL_BUILTIN: &str = "jit_call_builtin_v2";
const H_MAKE_LAMBDA: &str = "jit_make_lambda";
const H_INC_DEC: &str = "jit_inc_dec";
const H_COMPOUND_OP: &str = "jit_compound_op";
const H_STRING_APPEND_LOCAL: &str = "jit_string_append_local";
const H_VM_CALL: &str = "jit_vm_call";
const H_DEFINE_FUNCTION: &str = "jit_define_function";
const H_FREE_GLOBAL: &str = "jit_free_global";
const H_THROW: &str = "jit_throw";
const H_RECURSION_OVERFLOW: &str = "jit_recursion_overflow";
const H_ERROR_CHECK: &str = "jit_error_check";
const H_ERROR_FIELD: &str = "jit_error_field";
const H_SET_ERROR_TOLERANT: &str = "jit_set_error_tolerant";
const H_RECORD_ERROR: &str = "jit_record_error";
const H_OPTIONAL_CHECK: &str = "jit_optional_check";
const H_GET_DOLLAR_INDEX: &str = "jit_get_dollar_index";
const H_GET_DOLLAR_FIELD: &str = "jit_get_dollar_field";
const H_GET_DOLLAR: &str = "jit_get_dollar";
const H_PUSH_SEND_CTX: &str = "jit_push_send_ctx";
const H_POP_SEND_CTX: &str = "jit_pop_send_ctx";
const H_ALIAS: &str = "jit_alias";
const H_USE: &str = "jit_use";
const H_ATOMIC: &str = "jit_atomic";

// ---------------------------------------------------------------------------
// JIT Manager
// ---------------------------------------------------------------------------

pub struct JitManager {
    pub call_counts: Vec<u32>,
    pub compiled: Vec<Option<Option<*const u8>>>,
    pub threshold: u32,
    module: Option<JITModule>,
    builder_ctx: FunctionBuilderContext,
    /// Declared function references for recursive calls
    func_ids: HashMap<usize, cranelift_module::FuncId>,
}

unsafe impl Send for JitManager {}
unsafe impl Sync for JitManager {}

impl JitManager {
    pub fn new(chunk_count: usize) -> Self {
        let module = {
            let mut flag_builder = settings::builder();
            let _ = flag_builder.set("use_colocated_libcalls", "false");
            let _ = flag_builder.set("is_pic", "false");
            let isa = cranelift_native::builder()
                .ok()
                .and_then(|b| b.finish(settings::Flags::new(flag_builder)).ok());
            isa.map(|isa| {
                let mut builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
                // Register all helper functions as symbols
                builder.symbol(H_BINARY_OP, jit_binary_op as *const u8);
                builder.symbol(H_NEGATE, jit_negate as *const u8);
                builder.symbol(H_BITNOT, jit_bitnot as *const u8);
                builder.symbol(H_NOT, jit_not as *const u8);
                builder.symbol(H_POW, jit_pow as *const u8);
                builder.symbol(H_IS_TRUTHY, jit_is_truthy as *const u8);
                builder.symbol(H_CLONE_VALUE, jit_clone_value as *const u8);
                builder.symbol(H_DROP_VALUE, jit_drop_value as *const u8);
                builder.symbol(H_GET_GLOBAL, jit_get_global as *const u8);
                builder.symbol(H_SET_GLOBAL, jit_set_global as *const u8);
                builder.symbol(H_FIELD_GET, jit_field_get as *const u8);
                builder.symbol(H_FIELD_SET, jit_field_set as *const u8);
                builder.symbol(H_INDEX_GET, jit_index_get as *const u8);
                builder.symbol(H_INDEX_SET, jit_index_set as *const u8);
                builder.symbol(H_MAKE_LIST, jit_make_list as *const u8);
                builder.symbol(H_MAKE_OBJECT, jit_make_object as *const u8);
                builder.symbol(H_MAKE_STRING, jit_make_string as *const u8);
                builder.symbol(H_MAKE_RANGE, jit_make_range as *const u8);
                builder.symbol(H_GENERIC_CALL, jit_generic_call as *const u8);
                builder.symbol(H_CALL_BUILTIN, jit_call_builtin_v2 as *const u8);
                builder.symbol(H_MAKE_LAMBDA, jit_make_lambda as *const u8);
                builder.symbol(H_INC_DEC, jit_inc_dec as *const u8);
                builder.symbol(H_COMPOUND_OP, jit_compound_op as *const u8);
                builder.symbol(H_STRING_APPEND_LOCAL, jit_string_append_local as *const u8);
                builder.symbol(H_VM_CALL, jit_vm_call as *const u8);
                builder.symbol(H_DEFINE_FUNCTION, jit_define_function as *const u8);
                builder.symbol(H_FREE_GLOBAL, jit_free_global as *const u8);
                builder.symbol(H_THROW, jit_throw as *const u8);
                builder.symbol(H_RECURSION_OVERFLOW, jit_recursion_overflow as *const u8);
                builder.symbol(H_ERROR_CHECK, jit_error_check as *const u8);
                builder.symbol(H_ERROR_FIELD, jit_error_field as *const u8);
                builder.symbol(H_SET_ERROR_TOLERANT, jit_set_error_tolerant as *const u8);
                builder.symbol(H_RECORD_ERROR, jit_record_error as *const u8);
                builder.symbol(H_OPTIONAL_CHECK, jit_optional_check as *const u8);
                builder.symbol(H_GET_DOLLAR_INDEX, jit_get_dollar_index as *const u8);
                builder.symbol(H_GET_DOLLAR_FIELD, jit_get_dollar_field as *const u8);
                builder.symbol(H_GET_DOLLAR, jit_get_dollar as *const u8);
                builder.symbol(H_PUSH_SEND_CTX, jit_push_send_ctx as *const u8);
                builder.symbol(H_POP_SEND_CTX, jit_pop_send_ctx as *const u8);
                builder.symbol(H_ALIAS, jit_alias as *const u8);
                builder.symbol(H_USE, jit_use as *const u8);
                builder.symbol(H_ATOMIC, jit_atomic as *const u8);
                JITModule::new(builder)
            })
        };

        Self {
            call_counts: vec![0; chunk_count],
            compiled: vec![None; chunk_count],
            threshold: 50,
            module,
            builder_ctx: FunctionBuilderContext::new(),
            func_ids: HashMap::new(),
        }
    }

    pub fn check_and_compile(&mut self, chunk_idx: usize, chunks: &[Chunk]) -> Option<*const u8> {
        if chunk_idx >= self.call_counts.len() {
            self.call_counts.resize(chunk_idx + 1, 0);
            self.compiled.resize(chunk_idx + 1, None);
        }

        self.call_counts[chunk_idx] += 1;

        if let Some(ref result) = self.compiled[chunk_idx] {
            return *result;
        }

        if self.call_counts[chunk_idx] < self.threshold {
            return None;
        }

        let ptr = self.try_compile(chunk_idx, chunks);
        self.compiled[chunk_idx] = Some(ptr);
        ptr
    }

    fn try_compile(&mut self, chunk_idx: usize, chunks: &[Chunk]) -> Option<*const u8> {
        let module = self.module.as_mut()?;
        let chunk = &chunks[chunk_idx];

        if chunk.param_count == 0 || chunk.param_count > 8 {
            return None;
        }

        // Build signature: actual function params + depth counter, all i64, return i64.
        // Context (vm_ptr, interp_ptr, chunks_ptr, chunk_idx) is passed via thread-locals.
        let mut sig = module.make_signature();
        for _ in 0..chunk.param_count {
            sig.params.push(AbiParam::new(types::I64));
        }
        sig.params.push(AbiParam::new(types::I64)); // depth counter (last param)
        sig.returns.push(AbiParam::new(types::I64));

        let func_name = format!("jit_{}", chunk_idx);
        let func_id = module.declare_function(&func_name, Linkage::Local, &sig).ok()?;
        self.func_ids.insert(chunk_idx, func_id);

        let mut func = Function::with_name_signature(UserFuncName::default(), sig.clone());
        let self_ref = module.declare_func_in_func(func_id, &mut func);

        // Declare and import all helper function references
        let helpers = HelperRefs::declare(module, &mut func)?;

        let mut builder = FunctionBuilder::new(&mut func, &mut self.builder_ctx);
        let ok = GenericJitCompiler::compile(&mut builder, chunk, self_ref, &helpers, chunk_idx);
        builder.finalize();

        if !ok {
            return None;
        }

        let mut ctx = Context::for_function(func);
        module.define_function(func_id, &mut ctx).ok()?;
        module.clear_context(&mut ctx);
        module.finalize_definitions().ok()?;

        Some(module.get_finalized_function(func_id))
    }

    /// Call a JIT'd function. Context is set via thread-locals before calling.
    /// `args` contains the actual function arguments (1..=8). Depth is appended automatically.
    ///
    /// # Safety
    /// `ptr` must be a valid function pointer returned by `check_and_compile`.
    /// Thread-local JIT context must be set via `set_jit_context` before calling.
    pub unsafe fn call_jit_fn(&self, ptr: *const u8, args: &[u64], depth: u64) -> u64 {
        unsafe {
            match args.len() {
                1 => {
                    let func: unsafe extern "C" fn(u64,u64) -> u64 = std::mem::transmute(ptr);
                    func(args[0], depth)
                }
                2 => {
                    let func: unsafe extern "C" fn(u64,u64,u64) -> u64 = std::mem::transmute(ptr);
                    func(args[0],args[1], depth)
                }
                3 => {
                    let func: unsafe extern "C" fn(u64,u64,u64,u64) -> u64 = std::mem::transmute(ptr);
                    func(args[0],args[1],args[2], depth)
                }
                4 => {
                    let func: unsafe extern "C" fn(u64,u64,u64,u64,u64) -> u64 = std::mem::transmute(ptr);
                    func(args[0],args[1],args[2],args[3], depth)
                }
                5 => {
                    let func: unsafe extern "C" fn(u64,u64,u64,u64,u64,u64) -> u64 = std::mem::transmute(ptr);
                    func(args[0],args[1],args[2],args[3],args[4], depth)
                }
                6 => {
                    let func: unsafe extern "C" fn(u64,u64,u64,u64,u64,u64,u64) -> u64 = std::mem::transmute(ptr);
                    func(args[0],args[1],args[2],args[3],args[4],args[5], depth)
                }
                7 => {
                    let func: unsafe extern "C" fn(u64,u64,u64,u64,u64,u64,u64,u64) -> u64 = std::mem::transmute(ptr);
                    func(args[0],args[1],args[2],args[3],args[4],args[5],args[6], depth)
                }
                8 => {
                    let func: unsafe extern "C" fn(u64,u64,u64,u64,u64,u64,u64,u64,u64) -> u64 = std::mem::transmute(ptr);
                    func(args[0],args[1],args[2],args[3],args[4],args[5],args[6],args[7], depth)
                }
                _ => Value::void().raw(),
            }
        }
    }

    /// # Safety
    /// `ptr` must be a valid JIT-compiled function pointer.
    pub unsafe fn call_int_fn(&self, ptr: *const u8, arg: i64) -> i64 {
        unsafe {
            let func: unsafe extern "C" fn(i64) -> i64 = std::mem::transmute(ptr);
            func(arg)
        }
    }

    /// # Safety
    /// `ptr` must be a valid JIT-compiled function pointer.
    pub unsafe fn call_int_fn2(&self, ptr: *const u8, a: i64, b: i64) -> i64 {
        unsafe {
            let func: unsafe extern "C" fn(i64, i64) -> i64 = std::mem::transmute(ptr);
            func(a, b)
        }
    }
}

// ---------------------------------------------------------------------------
// Helper function references in Cranelift IR
// ---------------------------------------------------------------------------

struct HelperRefs {
    binary_op: cranelift_codegen::ir::FuncRef,    // (i64, i64, i64) -> i64
    negate: cranelift_codegen::ir::FuncRef,        // (i64) -> i64
    bitnot: cranelift_codegen::ir::FuncRef,        // (i64) -> i64
    not: cranelift_codegen::ir::FuncRef,           // (i64) -> i64
    pow: cranelift_codegen::ir::FuncRef,           // (i64, i64) -> i64
    is_truthy: cranelift_codegen::ir::FuncRef,     // (i64) -> i64
    clone_value: cranelift_codegen::ir::FuncRef,   // (i64) -> i64
    get_global: cranelift_codegen::ir::FuncRef,    // (i64) -> i64
    set_global: cranelift_codegen::ir::FuncRef,    // (i64, i64) -> void
    field_get: cranelift_codegen::ir::FuncRef,     // (i64, i64) -> i64
    field_set: cranelift_codegen::ir::FuncRef,     // (i64, i64, i64) -> void
    index_get: cranelift_codegen::ir::FuncRef,     // (i64, i64) -> i64
    index_set: cranelift_codegen::ir::FuncRef,     // (i64, i64, i64) -> void
    make_list: cranelift_codegen::ir::FuncRef,     // (i64, i64) -> i64
    make_object: cranelift_codegen::ir::FuncRef,   // (i64, i64) -> i64
    make_string: cranelift_codegen::ir::FuncRef,   // (i64, i64) -> i64
    make_range: cranelift_codegen::ir::FuncRef,    // (i64, i64) -> i64
    generic_call: cranelift_codegen::ir::FuncRef,  // (i64, i64, i64, i64) -> i64
    call_builtin: cranelift_codegen::ir::FuncRef,  // (i64, i64, i64) -> i64
    make_lambda: cranelift_codegen::ir::FuncRef,   // (i64, i64, i64, i64) -> i64
    inc_dec: cranelift_codegen::ir::FuncRef,       // (i64, i64) -> i64
    compound_op: cranelift_codegen::ir::FuncRef,   // (i64, i64, i64) -> i64
    string_append_local: cranelift_codegen::ir::FuncRef, // (i64, i64) -> i64
    vm_call: cranelift_codegen::ir::FuncRef,       // (i64, i64, i64) -> i64
    define_function: cranelift_codegen::ir::FuncRef, // (i64, i64) -> void
    free_global: cranelift_codegen::ir::FuncRef,   // (i64) -> void
    throw: cranelift_codegen::ir::FuncRef,         // (i64) -> i64
    recursion_overflow: cranelift_codegen::ir::FuncRef, // () -> void
    error_check: cranelift_codegen::ir::FuncRef,   // (i64) -> i64
    error_field: cranelift_codegen::ir::FuncRef,   // (i64) -> i64
    set_error_tolerant: cranelift_codegen::ir::FuncRef, // (i64) -> void
    record_error: cranelift_codegen::ir::FuncRef,  // (i64) -> void
    optional_check: cranelift_codegen::ir::FuncRef, // (i64) -> i64
    get_dollar_index: cranelift_codegen::ir::FuncRef, // (i64) -> i64
    get_dollar_field: cranelift_codegen::ir::FuncRef, // (i64) -> i64
    get_dollar: cranelift_codegen::ir::FuncRef,    // () -> i64
    push_send_ctx: cranelift_codegen::ir::FuncRef, // (i64) -> void
    pop_send_ctx: cranelift_codegen::ir::FuncRef,  // () -> void
    alias: cranelift_codegen::ir::FuncRef,         // (i64, i64) -> void
    use_fn: cranelift_codegen::ir::FuncRef,        // (i64, i64) -> void
    atomic: cranelift_codegen::ir::FuncRef,        // (i64) -> i64
}

impl HelperRefs {
    #[allow(unused_mut, unused_assignments)]
    fn declare(module: &mut JITModule, func: &mut Function) -> Option<Self> {
        let i64t = types::I64;

        // Helper to declare an external function and get its func ref
        macro_rules! decl {
            ($name:expr, [$($p:expr),*], [$($r:expr),*]) => {{
                let mut sig = module.make_signature();
                $(sig.params.push(AbiParam::new($p));)*
                $(sig.returns.push(AbiParam::new($r));)*
                let id = module.declare_function($name, Linkage::Import, &sig).ok()?;
                module.declare_func_in_func(id, func)
            }};
        }

        Some(Self {
            binary_op: decl!(H_BINARY_OP, [i64t, i64t, i64t], [i64t]),
            negate: decl!(H_NEGATE, [i64t], [i64t]),
            bitnot: decl!(H_BITNOT, [i64t], [i64t]),
            not: decl!(H_NOT, [i64t], [i64t]),
            pow: decl!(H_POW, [i64t, i64t], [i64t]),
            is_truthy: decl!(H_IS_TRUTHY, [i64t], [i64t]),
            clone_value: decl!(H_CLONE_VALUE, [i64t], [i64t]),
            get_global: decl!(H_GET_GLOBAL, [i64t], [i64t]),
            set_global: decl!(H_SET_GLOBAL, [i64t, i64t], []),
            field_get: decl!(H_FIELD_GET, [i64t, i64t], [i64t]),
            field_set: decl!(H_FIELD_SET, [i64t, i64t, i64t], []),
            index_get: decl!(H_INDEX_GET, [i64t, i64t], [i64t]),
            index_set: decl!(H_INDEX_SET, [i64t, i64t, i64t], []),
            make_list: decl!(H_MAKE_LIST, [i64t, i64t], [i64t]),
            make_object: decl!(H_MAKE_OBJECT, [i64t, i64t], [i64t]),
            make_string: decl!(H_MAKE_STRING, [i64t, i64t], [i64t]),
            make_range: decl!(H_MAKE_RANGE, [i64t, i64t], [i64t]),
            generic_call: decl!(H_GENERIC_CALL, [i64t, i64t, i64t, i64t], [i64t]),
            call_builtin: decl!(H_CALL_BUILTIN, [i64t, i64t, i64t], [i64t]),
            make_lambda: decl!(H_MAKE_LAMBDA, [i64t, i64t, i64t, i64t], [i64t]),
            inc_dec: decl!(H_INC_DEC, [i64t, i64t], [i64t]),
            compound_op: decl!(H_COMPOUND_OP, [i64t, i64t, i64t], [i64t]),
            string_append_local: decl!(H_STRING_APPEND_LOCAL, [i64t, i64t], [i64t]),
            vm_call: decl!(H_VM_CALL, [i64t, i64t, i64t], [i64t]),
            define_function: decl!(H_DEFINE_FUNCTION, [i64t, i64t], []),
            free_global: decl!(H_FREE_GLOBAL, [i64t], []),
            throw: decl!(H_THROW, [i64t], [i64t]),
            recursion_overflow: decl!(H_RECURSION_OVERFLOW, [], []),
            error_check: decl!(H_ERROR_CHECK, [i64t], [i64t]),
            error_field: decl!(H_ERROR_FIELD, [i64t], [i64t]),
            set_error_tolerant: decl!(H_SET_ERROR_TOLERANT, [i64t], []),
            record_error: decl!(H_RECORD_ERROR, [i64t], []),
            optional_check: decl!(H_OPTIONAL_CHECK, [i64t], [i64t]),
            get_dollar_index: decl!(H_GET_DOLLAR_INDEX, [i64t], [i64t]),
            get_dollar_field: decl!(H_GET_DOLLAR_FIELD, [i64t], [i64t]),
            get_dollar: decl!(H_GET_DOLLAR, [], [i64t]),
            push_send_ctx: decl!(H_PUSH_SEND_CTX, [i64t], []),
            pop_send_ctx: decl!(H_POP_SEND_CTX, [], []),
            alias: decl!(H_ALIAS, [i64t, i64t], []),
            use_fn: decl!(H_USE, [i64t, i64t], []),
            atomic: decl!(H_ATOMIC, [i64t], [i64t]),
        })
    }
}

// ---------------------------------------------------------------------------
// NaN-boxing constants for inline fast paths
// ---------------------------------------------------------------------------
const NB_TAG_INT: u64      = 0x7FF8_0000_0000_0000;
const NB_TAG_BOOL: u64     = 0x7FF9_0000_0000_0000;
const NB_TAG_MASK: u64     = 0xFFFF_0000_0000_0000;
const NB_PAYLOAD_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

/// What the inline int fast-path should compute.
#[derive(Clone, Copy)]
enum IntFastOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Neq,
    Lt,
    Gt,
    Lte,
    Gte,
}

// ---------------------------------------------------------------------------
// Generic Bytecode-to-IR Compiler
// ---------------------------------------------------------------------------

struct GenericJitCompiler;

impl GenericJitCompiler {
    /// Emit an inline int fast-path for a binary operation.
    ///
    /// Pattern:
    ///   if both operands have TAG_INT  ->  native arithmetic, re-tag result
    ///   else                           ->  call jit_binary_op helper
    ///
    /// For division/modulo the fast path also checks for zero divisor and
    /// falls back to the helper (which returns void).
    fn emit_int_binop(
        builder: &mut FunctionBuilder,
        helpers: &HelperRefs,
        a: cranelift_codegen::ir::Value,
        b: cranelift_codegen::ir::Value,
        fast_op: IntFastOp,
        bop_code: u64,
    ) -> cranelift_codegen::ir::Value {
        let tag_mask = builder.ins().iconst(types::I64, NB_TAG_MASK as i64);
        let tag_int  = builder.ins().iconst(types::I64, NB_TAG_INT as i64);

        let a_tag    = builder.ins().band(a, tag_mask);
        let b_tag    = builder.ins().band(b, tag_mask);
        let a_is_int = builder.ins().icmp(IntCC::Equal, a_tag, tag_int);
        let b_is_int = builder.ins().icmp(IntCC::Equal, b_tag, tag_int);
        let both_int = builder.ins().band(a_is_int, b_is_int);

        let fast_block  = builder.create_block();
        let slow_block  = builder.create_block();
        let merge_block = builder.create_block();
        builder.append_block_param(merge_block, types::I64);

        builder.ins().brif(both_int, fast_block, &[], slow_block, &[]);

        // ---- fast path ----
        builder.switch_to_block(fast_block);
        builder.seal_block(fast_block);

        let payload_mask_val = builder.ins().iconst(types::I64, NB_PAYLOAD_MASK as i64);
        let a_payload = builder.ins().band(a, payload_mask_val);
        let b_payload = builder.ins().band(b, payload_mask_val);

        // Sign-extend payloads from 48 bits to 64 bits for correct signed ops
        let a_ext = builder.ins().ishl_imm(a_payload, 16);
        let a_ext = builder.ins().sshr_imm(a_ext, 16);
        let b_ext = builder.ins().ishl_imm(b_payload, 16);
        let b_ext = builder.ins().sshr_imm(b_ext, 16);

        let fast_result = match fast_op {
            IntFastOp::Add => {
                let sum = builder.ins().iadd(a_ext, b_ext);
                let masked = builder.ins().band(sum, payload_mask_val);
                builder.ins().bor(masked, tag_int)
            }
            IntFastOp::Sub => {
                let diff = builder.ins().isub(a_ext, b_ext);
                let masked = builder.ins().band(diff, payload_mask_val);
                builder.ins().bor(masked, tag_int)
            }
            IntFastOp::Mul => {
                let prod = builder.ins().imul(a_ext, b_ext);
                let masked = builder.ins().band(prod, payload_mask_val);
                builder.ins().bor(masked, tag_int)
            }
            IntFastOp::Div => {
                // Zero check: if b_payload == 0, jump to slow path
                let zero = builder.ins().iconst(types::I64, 0);
                let b_is_zero = builder.ins().icmp(IntCC::Equal, b_ext, zero);
                let div_ok_block = builder.create_block();
                builder.ins().brif(b_is_zero, slow_block, &[], div_ok_block, &[]);

                builder.switch_to_block(div_ok_block);
                builder.seal_block(div_ok_block);

                let quot = builder.ins().sdiv(a_ext, b_ext);
                let masked = builder.ins().band(quot, payload_mask_val);
                builder.ins().bor(masked, tag_int)
            }
            IntFastOp::Mod => {
                // Zero check
                let zero = builder.ins().iconst(types::I64, 0);
                let b_is_zero = builder.ins().icmp(IntCC::Equal, b_ext, zero);
                let mod_ok_block = builder.create_block();
                builder.ins().brif(b_is_zero, slow_block, &[], mod_ok_block, &[]);

                builder.switch_to_block(mod_ok_block);
                builder.seal_block(mod_ok_block);

                let rem = builder.ins().srem(a_ext, b_ext);
                let masked = builder.ins().band(rem, payload_mask_val);
                builder.ins().bor(masked, tag_int)
            }
            IntFastOp::Eq => {
                // For int eq, just compare the raw NaN-boxed values directly
                let eq = builder.ins().icmp(IntCC::Equal, a, b);
                let true_raw  = builder.ins().iconst(types::I64, (NB_TAG_BOOL | 1) as i64);
                let false_raw = builder.ins().iconst(types::I64, NB_TAG_BOOL as i64);
                builder.ins().select(eq, true_raw, false_raw)
            }
            IntFastOp::Neq => {
                let ne = builder.ins().icmp(IntCC::NotEqual, a, b);
                let true_raw  = builder.ins().iconst(types::I64, (NB_TAG_BOOL | 1) as i64);
                let false_raw = builder.ins().iconst(types::I64, NB_TAG_BOOL as i64);
                builder.ins().select(ne, true_raw, false_raw)
            }
            IntFastOp::Lt => {
                let lt = builder.ins().icmp(IntCC::SignedLessThan, a_ext, b_ext);
                let true_raw  = builder.ins().iconst(types::I64, (NB_TAG_BOOL | 1) as i64);
                let false_raw = builder.ins().iconst(types::I64, NB_TAG_BOOL as i64);
                builder.ins().select(lt, true_raw, false_raw)
            }
            IntFastOp::Gt => {
                let gt = builder.ins().icmp(IntCC::SignedGreaterThan, a_ext, b_ext);
                let true_raw  = builder.ins().iconst(types::I64, (NB_TAG_BOOL | 1) as i64);
                let false_raw = builder.ins().iconst(types::I64, NB_TAG_BOOL as i64);
                builder.ins().select(gt, true_raw, false_raw)
            }
            IntFastOp::Lte => {
                let le = builder.ins().icmp(IntCC::SignedLessThanOrEqual, a_ext, b_ext);
                let true_raw  = builder.ins().iconst(types::I64, (NB_TAG_BOOL | 1) as i64);
                let false_raw = builder.ins().iconst(types::I64, NB_TAG_BOOL as i64);
                builder.ins().select(le, true_raw, false_raw)
            }
            IntFastOp::Gte => {
                let ge = builder.ins().icmp(IntCC::SignedGreaterThanOrEqual, a_ext, b_ext);
                let true_raw  = builder.ins().iconst(types::I64, (NB_TAG_BOOL | 1) as i64);
                let false_raw = builder.ins().iconst(types::I64, NB_TAG_BOOL as i64);
                builder.ins().select(ge, true_raw, false_raw)
            }
        };
        builder.ins().jump(merge_block, &[fast_result]);

        // ---- slow path ----
        builder.switch_to_block(slow_block);
        builder.seal_block(slow_block);
        let op_c = builder.ins().iconst(types::I64, bop_code as i64);
        let call = builder.ins().call(helpers.binary_op, &[a, b, op_c]);
        let slow_result = builder.inst_results(call)[0];
        builder.ins().jump(merge_block, &[slow_result]);

        // ---- merge ----
        builder.switch_to_block(merge_block);
        builder.seal_block(merge_block);
        builder.block_params(merge_block)[0]
    }

    fn compile(
        builder: &mut FunctionBuilder,
        chunk: &Chunk,
        self_ref: cranelift_codegen::ir::FuncRef,
        helpers: &HelperRefs,
        chunk_idx: usize,
    ) -> bool {
        let code = &chunk.code;

        // --- Phase 1: Pre-scan for jump targets to create block boundaries ---
        let mut jump_targets: HashSet<usize> = HashSet::new();
        {
            let mut pc = 0;
            while pc < code.len() {
                let op: Op = unsafe { std::mem::transmute(code[pc]) };
                pc += 1;
                match op {
                    Op::Jump => {
                        let offset = chunk.read_i32(pc);
                        let target = (pc as i32 + 4 + offset) as usize;
                        jump_targets.insert(target);
                        pc += 4;
                    }
                    Op::JumpIfFalse | Op::JumpIfTrue => {
                        let offset = chunk.read_i32(pc);
                        let target = (pc as i32 + 4 + offset) as usize;
                        jump_targets.insert(target);
                        jump_targets.insert(pc + 4); // fall-through
                        pc += 4;
                    }
                    Op::Loop => {
                        let offset = chunk.read_i32(pc);
                        let target = (pc as i32 + 4 - offset) as usize;
                        jump_targets.insert(target);
                        pc += 4;
                    }
                    Op::BranchIfLocalGtImm | Op::BranchIfLocalLteImm => {
                        pc += 2 + 8; // slot + imm
                        let offset = chunk.read_i32(pc);
                        let target = (pc as i32 + 4 + offset) as usize;
                        jump_targets.insert(target);
                        jump_targets.insert(pc + 4);
                        pc += 4;
                    }
                    // Skip operands
                    Op::LoadInt | Op::LoadFloat => pc += 8,
                    Op::SubLocalImm | Op::AddLocalImm => pc += 10,
                    Op::Call => pc += 4,
                    Op::CallLocal | Op::CallBuiltin => pc += 3,
                    Op::MakeLambda => pc += 4,
                    Op::ErrorField | Op::Alias | Op::Use => pc += 4,
                    Op::DefineFunction => pc += 4,
                    Op::DefineEnum => {
                        pc += 2;
                        if pc + 1 < code.len() {
                            let count = u16::from_le_bytes([code[pc], code[pc+1]]) as usize;
                            pc += 2 + count * 2;
                        }
                    }
                    Op::TryBegin | Op::TryEnd => pc += 4,
                    Op::GetLocal | Op::SetLocal | Op::GetGlobal | Op::SetGlobal
                    | Op::LoadConst | Op::FieldGet | Op::FieldSet | Op::MakeList
                    | Op::MakeObject | Op::MakeString | Op::MakeRange
                    | Op::IncLocal | Op::DecLocal | Op::PostIncLocal | Op::PostDecLocal
                    | Op::PreIncLocal | Op::PreDecLocal | Op::CompoundAddInt | Op::CompoundSubInt
                    | Op::StringAppendLocal
                    | Op::GetDollarIndex | Op::GetDollarField | Op::ErrorCheck
                    | Op::OptionalCheck | Op::SetErrorTolerant | Op::Import | Op::Free
                    | Op::GetLocalInt | Op::RecordError => pc += 2,
                    _ => {} // 0-operand
                }
            }
        }

        // --- Phase 2: Create Cranelift blocks for each jump target ---
        let mut block_map: HashMap<usize, cranelift_codegen::ir::Block> = HashMap::new();
        for &target in &jump_targets {
            block_map.insert(target, builder.create_block());
        }

        // Entry block
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);

        // Declare variables for locals
        let max_locals = chunk.locals.len().max(chunk.param_count as usize).max(16);
        let mut vars: Vec<Variable> = Vec::new();
        for i in 0..max_locals {
            let var = Variable::from_u32(i as u32);
            builder.declare_var(var, types::I64);
            vars.push(var);
        }

        // Init params (no offset — context is in thread-locals)
        for (i, var) in vars.iter().take(chunk.param_count as usize).enumerate() {
            let param_val = builder.block_params(entry)[i];
            builder.def_var(*var, param_val);
        }
        // Depth counter is last param
        let depth_var = Variable::from_u32(max_locals as u32);
        builder.declare_var(depth_var, types::I64);
        let depth_param = builder.block_params(entry)[chunk.param_count as usize];
        builder.def_var(depth_var, depth_param);

        // Depth check at function entry
        let depth_val = builder.use_var(depth_var);
        let limit = builder.ins().iconst(types::I64, 10000);
        let too_deep = builder.ins().icmp(IntCC::SignedGreaterThanOrEqual, depth_val, limit);
        let ok_block = builder.create_block();
        let overflow_block = builder.create_block();
        builder.ins().brif(too_deep, overflow_block, &[], ok_block, &[]);

        // Overflow: set error and return void
        builder.switch_to_block(overflow_block);
        builder.seal_block(overflow_block);
        builder.ins().call(helpers.recursion_overflow, &[]);
        let void_ret = builder.ins().iconst(types::I64, Value::void().raw() as i64);
        builder.ins().return_(&[void_ret]);

        // Normal path continues
        builder.switch_to_block(ok_block);
        builder.seal_block(ok_block);

        // Increment depth for this call
        let one = builder.ins().iconst(types::I64, 1);
        let new_depth = builder.ins().iadd(depth_val, one);
        builder.def_var(depth_var, new_depth);

        // Initialize remaining locals to void
        let void_raw = Value::void().raw();
        let void_const = builder.ins().iconst(types::I64, void_raw as i64);
        for var in &vars[chunk.param_count as usize..max_locals] {
            builder.def_var(*var, void_const);
        }

        // Seal entry block
        builder.seal_block(entry);

        // Virtual operand stack
        let mut vstack: Vec<cranelift_codegen::ir::Value> = Vec::new();

        // --- Phase 3: Translate opcodes ---
        let mut pc = 0;
        let block_sealed: HashSet<cranelift_codegen::ir::Block> = HashSet::new();
        let mut terminated = false;

        while pc < code.len() {
            // Check if this PC is a jump target — switch to its block
            if let Some(&block) = block_map.get(&pc) {
                if !terminated {
                    builder.ins().jump(block, &[]);
                }
                builder.switch_to_block(block);
                terminated = false;
            }

            let op: Op = unsafe { std::mem::transmute(code[pc]) };
            pc += 1;

            if terminated {
                // Skip dead code after terminator until next block
                match op {
                    Op::LoadInt | Op::LoadFloat => pc += 8,
                    Op::SubLocalImm | Op::AddLocalImm => pc += 10,
                    Op::BranchIfLocalGtImm | Op::BranchIfLocalLteImm => pc += 14,
                    Op::Jump | Op::JumpIfFalse | Op::JumpIfTrue | Op::Loop
                    | Op::TryBegin | Op::TryEnd => pc += 4,
                    Op::Call => pc += 4,
                    Op::CallLocal | Op::CallBuiltin => pc += 3,
                    Op::MakeLambda => pc += 4,
                    Op::ErrorField | Op::Alias | Op::Use | Op::DefineFunction => pc += 4,
                    Op::DefineEnum => {
                        pc += 2;
                        if pc + 1 < code.len() {
                            let count = u16::from_le_bytes([code[pc], code[pc+1]]) as usize;
                            pc += 2 + count * 2;
                        }
                    }
                    Op::GetLocal | Op::SetLocal | Op::GetGlobal | Op::SetGlobal
                    | Op::LoadConst | Op::FieldGet | Op::FieldSet | Op::MakeList
                    | Op::MakeObject | Op::MakeString | Op::MakeRange
                    | Op::IncLocal | Op::DecLocal | Op::PostIncLocal | Op::PostDecLocal
                    | Op::PreIncLocal | Op::PreDecLocal | Op::CompoundAddInt | Op::CompoundSubInt
                    | Op::StringAppendLocal
                    | Op::GetDollarIndex | Op::GetDollarField | Op::ErrorCheck
                    | Op::OptionalCheck | Op::SetErrorTolerant | Op::Import | Op::Free
                    | Op::GetLocalInt | Op::RecordError => pc += 2,
                    _ => {}
                }
                continue;
            }

            match op {
                // ============================================================
                // CONSTANTS
                // ============================================================
                Op::LoadInt => {
                    let val = chunk.read_i64(pc); pc += 8;
                    // This is a raw i64 from bytecode. We need to create a NaN-boxed Value::int.
                    // Value::int(val).raw() — we compute this at compile time for the JIT.
                    let nan_boxed = Value::int(val);
                    let raw = nan_boxed.raw();
                    std::mem::forget(nan_boxed);
                    vstack.push(builder.ins().iconst(types::I64, raw as i64));
                }
                Op::LoadFloat => {
                    // Float bits in bytecode ARE the NaN-boxed representation
                    // Value::float(f) stores f.to_bits() directly as the u64
                    let bits = chunk.read_i64(pc); pc += 8;
                    vstack.push(builder.ins().iconst(types::I64, bits));
                }
                Op::LoadTrue => {
                    let raw = Value::bool(true).raw();
                    vstack.push(builder.ins().iconst(types::I64, raw as i64));
                }
                Op::LoadFalse => {
                    let raw = Value::bool(false).raw();
                    vstack.push(builder.ins().iconst(types::I64, raw as i64));
                }
                Op::LoadVoid => {
                    vstack.push(builder.ins().iconst(types::I64, void_raw as i64));
                }
                Op::LoadConst => {
                    let idx = chunk.read_u16(pc) as usize; pc += 2;
                    // chunks_ptr is not available at JIT time — we use a sentinel approach.
                    // Actually, we need chunks_ptr at runtime. We'll pass it as... hmm.
                    // The JIT compiled function doesn't have access to VM/chunks pointers.
                    // For LoadConst, we can pre-resolve the string at compile time!
                    let s = chunk.constants.get(idx as u16).clone();
                    let result = Value::string(Rc::from(s.as_ref()));
                    let raw = result.raw();
                    // NOTE: we are leaking this value intentionally — it's a compile-time
                    // constant that lives for the duration of the JIT'd function.
                    // Each call to this JIT'd code will use this raw bits.
                    // Actually this is a problem — it creates a new Rc each time compile runs,
                    // but only runs once. The issue is the JIT'd code will use this raw pointer
                    // and we need to clone it each time it's used.
                    // So we must call clone_value on it at runtime.
                    std::mem::forget(result);
                    let raw_val = builder.ins().iconst(types::I64, raw as i64);
                    let call = builder.ins().call(helpers.clone_value, &[raw_val]);
                    vstack.push(builder.inst_results(call)[0]);
                }

                // ============================================================
                // VARIABLES
                // ============================================================
                Op::GetLocal | Op::GetLocalInt => {
                    let slot = chunk.read_u16(pc) as usize; pc += 2;
                    if slot >= vars.len() { return false; }
                    let v = builder.use_var(vars[slot]);
                    // Clone the value (in case it's an Rc type)
                    let call = builder.ins().call(helpers.clone_value, &[v]);
                    vstack.push(builder.inst_results(call)[0]);
                }
                Op::SetLocal => {
                    let slot = chunk.read_u16(pc) as usize; pc += 2;
                    let val = match vstack.pop() { Some(v) => v, None => return false };
                    if slot >= vars.len() { return false; }
                    // The old value in the slot needs to be dropped
                    // (but since JIT vars don't track ownership perfectly, we skip Drop for now —
                    //  the helpers handle cloning when reading, and ownership is managed at JIT boundaries)
                    builder.def_var(vars[slot], val);
                }

                Op::GetGlobal => {
                    let idx = chunk.read_u16(pc) as u64; pc += 2;
                    let idx_val = builder.ins().iconst(types::I64, idx as i64);
                    let call = builder.ins().call(helpers.get_global, &[idx_val]);
                    vstack.push(builder.inst_results(call)[0]);
                }
                Op::SetGlobal => {
                    let idx = chunk.read_u16(pc) as u64; pc += 2;
                    let val = match vstack.pop() { Some(v) => v, None => return false };
                    let idx_val = builder.ins().iconst(types::I64, idx as i64);
                    builder.ins().call(helpers.set_global, &[idx_val, val]);
                }

                // ============================================================
                // ARITHMETIC (all via helpers for NaN-box correctness)
                // ============================================================
                Op::Add | Op::AddInt => {
                    let b = match vstack.pop() { Some(v) => v, None => return false };
                    let a = match vstack.pop() { Some(v) => v, None => return false };
                    vstack.push(Self::emit_int_binop(builder, helpers, a, b, IntFastOp::Add, BOP_ADD));
                }
                Op::Sub | Op::SubInt => {
                    let b = match vstack.pop() { Some(v) => v, None => return false };
                    let a = match vstack.pop() { Some(v) => v, None => return false };
                    vstack.push(Self::emit_int_binop(builder, helpers, a, b, IntFastOp::Sub, BOP_SUB));
                }
                Op::Mul | Op::MulInt => {
                    let b = match vstack.pop() { Some(v) => v, None => return false };
                    let a = match vstack.pop() { Some(v) => v, None => return false };
                    vstack.push(Self::emit_int_binop(builder, helpers, a, b, IntFastOp::Mul, BOP_MUL));
                }
                Op::Div | Op::DivInt => {
                    let b = match vstack.pop() { Some(v) => v, None => return false };
                    let a = match vstack.pop() { Some(v) => v, None => return false };
                    vstack.push(Self::emit_int_binop(builder, helpers, a, b, IntFastOp::Div, BOP_DIV));
                }
                Op::Mod | Op::ModInt => {
                    let b = match vstack.pop() { Some(v) => v, None => return false };
                    let a = match vstack.pop() { Some(v) => v, None => return false };
                    vstack.push(Self::emit_int_binop(builder, helpers, a, b, IntFastOp::Mod, BOP_MOD));
                }
                Op::Pow => {
                    let b = match vstack.pop() { Some(v) => v, None => return false };
                    let a = match vstack.pop() { Some(v) => v, None => return false };
                    let call = builder.ins().call(helpers.pow, &[a, b]);
                    vstack.push(builder.inst_results(call)[0]);
                }
                Op::Neg | Op::NegInt => {
                    let a = match vstack.pop() { Some(v) => v, None => return false };
                    let call = builder.ins().call(helpers.negate, &[a]);
                    vstack.push(builder.inst_results(call)[0]);
                }

                // ============================================================
                // COMPARISON (all via helpers)
                // ============================================================
                Op::Eq | Op::EqInt => {
                    let b = match vstack.pop() { Some(v) => v, None => return false };
                    let a = match vstack.pop() { Some(v) => v, None => return false };
                    vstack.push(Self::emit_int_binop(builder, helpers, a, b, IntFastOp::Eq, BOP_EQ));
                }
                Op::Neq | Op::NeqInt => {
                    let b = match vstack.pop() { Some(v) => v, None => return false };
                    let a = match vstack.pop() { Some(v) => v, None => return false };
                    vstack.push(Self::emit_int_binop(builder, helpers, a, b, IntFastOp::Neq, BOP_NEQ));
                }
                Op::Lt | Op::LtInt => {
                    let b = match vstack.pop() { Some(v) => v, None => return false };
                    let a = match vstack.pop() { Some(v) => v, None => return false };
                    vstack.push(Self::emit_int_binop(builder, helpers, a, b, IntFastOp::Lt, BOP_LT));
                }
                Op::Gt | Op::GtInt => {
                    let b = match vstack.pop() { Some(v) => v, None => return false };
                    let a = match vstack.pop() { Some(v) => v, None => return false };
                    vstack.push(Self::emit_int_binop(builder, helpers, a, b, IntFastOp::Gt, BOP_GT));
                }
                Op::Lte | Op::LteInt => {
                    let b = match vstack.pop() { Some(v) => v, None => return false };
                    let a = match vstack.pop() { Some(v) => v, None => return false };
                    vstack.push(Self::emit_int_binop(builder, helpers, a, b, IntFastOp::Lte, BOP_LTE));
                }
                Op::Gte | Op::GteInt => {
                    let b = match vstack.pop() { Some(v) => v, None => return false };
                    let a = match vstack.pop() { Some(v) => v, None => return false };
                    vstack.push(Self::emit_int_binop(builder, helpers, a, b, IntFastOp::Gte, BOP_GTE));
                }

                // ============================================================
                // LOGICAL / BITWISE (via helpers)
                // ============================================================
                Op::Not => {
                    let a = match vstack.pop() { Some(v) => v, None => return false };
                    let call = builder.ins().call(helpers.not, &[a]);
                    vstack.push(builder.inst_results(call)[0]);
                }
                Op::BitAnd => {
                    let b = match vstack.pop() { Some(v) => v, None => return false };
                    let a = match vstack.pop() { Some(v) => v, None => return false };
                    let op_c = builder.ins().iconst(types::I64, BOP_BITAND as i64);
                    let call = builder.ins().call(helpers.binary_op, &[a, b, op_c]);
                    vstack.push(builder.inst_results(call)[0]);
                }
                Op::BitOr => {
                    let b = match vstack.pop() { Some(v) => v, None => return false };
                    let a = match vstack.pop() { Some(v) => v, None => return false };
                    let op_c = builder.ins().iconst(types::I64, BOP_BITOR as i64);
                    let call = builder.ins().call(helpers.binary_op, &[a, b, op_c]);
                    vstack.push(builder.inst_results(call)[0]);
                }
                Op::BitXor => {
                    let b = match vstack.pop() { Some(v) => v, None => return false };
                    let a = match vstack.pop() { Some(v) => v, None => return false };
                    let op_c = builder.ins().iconst(types::I64, BOP_BITXOR as i64);
                    let call = builder.ins().call(helpers.binary_op, &[a, b, op_c]);
                    vstack.push(builder.inst_results(call)[0]);
                }
                Op::BitNot => {
                    let a = match vstack.pop() { Some(v) => v, None => return false };
                    let call = builder.ins().call(helpers.bitnot, &[a]);
                    vstack.push(builder.inst_results(call)[0]);
                }
                Op::Shl => {
                    let b = match vstack.pop() { Some(v) => v, None => return false };
                    let a = match vstack.pop() { Some(v) => v, None => return false };
                    let op_c = builder.ins().iconst(types::I64, BOP_SHL as i64);
                    let call = builder.ins().call(helpers.binary_op, &[a, b, op_c]);
                    vstack.push(builder.inst_results(call)[0]);
                }
                Op::Shr => {
                    let b = match vstack.pop() { Some(v) => v, None => return false };
                    let a = match vstack.pop() { Some(v) => v, None => return false };
                    let op_c = builder.ins().iconst(types::I64, BOP_SHR as i64);
                    let call = builder.ins().call(helpers.binary_op, &[a, b, op_c]);
                    vstack.push(builder.inst_results(call)[0]);
                }

                // ============================================================
                // INC/DEC (via helpers)
                // ============================================================
                Op::IncLocal => {
                    let slot = chunk.read_u16(pc) as usize; pc += 2;
                    if slot >= vars.len() { return false; }
                    let v = builder.use_var(vars[slot]);
                    let op_c = builder.ins().iconst(types::I64, 0); // inc
                    let call = builder.ins().call(helpers.inc_dec, &[v, op_c]);
                    builder.def_var(vars[slot], builder.inst_results(call)[0]);
                }
                Op::DecLocal => {
                    let slot = chunk.read_u16(pc) as usize; pc += 2;
                    if slot >= vars.len() { return false; }
                    let v = builder.use_var(vars[slot]);
                    let op_c = builder.ins().iconst(types::I64, 1); // dec
                    let call = builder.ins().call(helpers.inc_dec, &[v, op_c]);
                    builder.def_var(vars[slot], builder.inst_results(call)[0]);
                }
                Op::PostIncLocal => {
                    let slot = chunk.read_u16(pc) as usize; pc += 2;
                    if slot >= vars.len() { return false; }
                    let old = builder.use_var(vars[slot]);
                    // Clone old for the push
                    let old_clone = builder.ins().call(helpers.clone_value, &[old]);
                    let old_cloned = builder.inst_results(old_clone)[0];
                    // Increment
                    let op_c = builder.ins().iconst(types::I64, 0);
                    let call = builder.ins().call(helpers.inc_dec, &[old, op_c]);
                    builder.def_var(vars[slot], builder.inst_results(call)[0]);
                    vstack.push(old_cloned); // push old value
                }
                Op::PostDecLocal => {
                    let slot = chunk.read_u16(pc) as usize; pc += 2;
                    if slot >= vars.len() { return false; }
                    let old = builder.use_var(vars[slot]);
                    let old_clone = builder.ins().call(helpers.clone_value, &[old]);
                    let old_cloned = builder.inst_results(old_clone)[0];
                    let op_c = builder.ins().iconst(types::I64, 1);
                    let call = builder.ins().call(helpers.inc_dec, &[old, op_c]);
                    builder.def_var(vars[slot], builder.inst_results(call)[0]);
                    vstack.push(old_cloned);
                }
                Op::PreIncLocal => {
                    let slot = chunk.read_u16(pc) as usize; pc += 2;
                    if slot >= vars.len() { return false; }
                    let v = builder.use_var(vars[slot]);
                    let op_c = builder.ins().iconst(types::I64, 0);
                    let call = builder.ins().call(helpers.inc_dec, &[v, op_c]);
                    let new_v = builder.inst_results(call)[0];
                    builder.def_var(vars[slot], new_v);
                    // Clone for push
                    let clone_call = builder.ins().call(helpers.clone_value, &[new_v]);
                    vstack.push(builder.inst_results(clone_call)[0]);
                }
                Op::PreDecLocal => {
                    let slot = chunk.read_u16(pc) as usize; pc += 2;
                    if slot >= vars.len() { return false; }
                    let v = builder.use_var(vars[slot]);
                    let op_c = builder.ins().iconst(types::I64, 1);
                    let call = builder.ins().call(helpers.inc_dec, &[v, op_c]);
                    let new_v = builder.inst_results(call)[0];
                    builder.def_var(vars[slot], new_v);
                    let clone_call = builder.ins().call(helpers.clone_value, &[new_v]);
                    vstack.push(builder.inst_results(clone_call)[0]);
                }
                Op::CompoundAddInt => {
                    let slot = chunk.read_u16(pc) as usize; pc += 2;
                    let rhs = match vstack.pop() { Some(v) => v, None => return false };
                    if slot >= vars.len() { return false; }
                    let v = builder.use_var(vars[slot]);
                    let op_c = builder.ins().iconst(types::I64, 0); // add
                    let call = builder.ins().call(helpers.compound_op, &[v, rhs, op_c]);
                    builder.def_var(vars[slot], builder.inst_results(call)[0]);
                }
                Op::CompoundSubInt => {
                    let slot = chunk.read_u16(pc) as usize; pc += 2;
                    let rhs = match vstack.pop() { Some(v) => v, None => return false };
                    if slot >= vars.len() { return false; }
                    let v = builder.use_var(vars[slot]);
                    let op_c = builder.ins().iconst(types::I64, 1); // sub
                    let call = builder.ins().call(helpers.compound_op, &[v, rhs, op_c]);
                    builder.def_var(vars[slot], builder.inst_results(call)[0]);
                }
                Op::StringAppendLocal => {
                    let slot = chunk.read_u16(pc) as usize; pc += 2;
                    let rhs = match vstack.pop() { Some(v) => v, None => return false };
                    if slot >= vars.len() { return false; }
                    let v = builder.use_var(vars[slot]);
                    let call = builder.ins().call(helpers.string_append_local, &[v, rhs]);
                    builder.def_var(vars[slot], builder.inst_results(call)[0]);
                }

                // ============================================================
                // SUPERINSTRUCTIONS (via helpers)
                // ============================================================
                Op::SubLocalImm => {
                    let slot = chunk.read_u16(pc) as usize; pc += 2;
                    let imm = chunk.read_i64(pc); pc += 8;
                    if slot >= vars.len() { return false; }
                    let v = builder.use_var(vars[slot]);
                    // Native: extract payload, subtract, re-tag
                    let payload = builder.ins().band_imm(v, NB_PAYLOAD_MASK as i64);
                    let sign_ext = builder.ins().ishl_imm(payload, 16);
                    let sign_ext = builder.ins().sshr_imm(sign_ext, 16);
                    let result = builder.ins().iadd_imm(sign_ext, -imm);
                    let masked = builder.ins().band_imm(result, NB_PAYLOAD_MASK as i64);
                    let tagged = builder.ins().bor_imm(masked, NB_TAG_INT as i64);
                    vstack.push(tagged);
                }
                Op::AddLocalImm => {
                    let slot = chunk.read_u16(pc) as usize; pc += 2;
                    let imm = chunk.read_i64(pc); pc += 8;
                    if slot >= vars.len() { return false; }
                    let v = builder.use_var(vars[slot]);
                    let payload = builder.ins().band_imm(v, NB_PAYLOAD_MASK as i64);
                    let sign_ext = builder.ins().ishl_imm(payload, 16);
                    let sign_ext = builder.ins().sshr_imm(sign_ext, 16);
                    let result = builder.ins().iadd_imm(sign_ext, imm);
                    let masked = builder.ins().band_imm(result, NB_PAYLOAD_MASK as i64);
                    let tagged = builder.ins().bor_imm(masked, NB_TAG_INT as i64);
                    vstack.push(tagged);
                }
                Op::BranchIfLocalGtImm => {
                    let slot = chunk.read_u16(pc) as usize; pc += 2;
                    let imm = chunk.read_i64(pc); pc += 8;
                    let offset = chunk.read_i32(pc); pc += 4;
                    if slot >= vars.len() { return false; }
                    let v = builder.use_var(vars[slot]);
                    // Native: extract, sign-extend, compare
                    let payload = builder.ins().band_imm(v, NB_PAYLOAD_MASK as i64);
                    let sign_ext = builder.ins().ishl_imm(payload, 16);
                    let sign_ext = builder.ins().sshr_imm(sign_ext, 16);
                    let imm_val = builder.ins().iconst(types::I64, imm);
                    let cmp = builder.ins().icmp(IntCC::SignedGreaterThan, sign_ext, imm_val);
                    let target_pc = (pc as i32 + offset) as usize;
                    let target_block = match block_map.get(&target_pc) { Some(&b) => b, None => return false };
                    let fall_block = match block_map.get(&pc) { Some(&b) => b, None => return false };
                    builder.ins().brif(cmp, target_block, &[], fall_block, &[]);
                    terminated = true;
                }
                Op::BranchIfLocalLteImm => {
                    let slot = chunk.read_u16(pc) as usize; pc += 2;
                    let imm = chunk.read_i64(pc); pc += 8;
                    let offset = chunk.read_i32(pc); pc += 4;
                    if slot >= vars.len() { return false; }
                    let v = builder.use_var(vars[slot]);
                    let payload = builder.ins().band_imm(v, NB_PAYLOAD_MASK as i64);
                    let sign_ext = builder.ins().ishl_imm(payload, 16);
                    let sign_ext = builder.ins().sshr_imm(sign_ext, 16);
                    let imm_val = builder.ins().iconst(types::I64, imm);
                    let cmp = builder.ins().icmp(IntCC::SignedLessThanOrEqual, sign_ext, imm_val);
                    let target_pc = (pc as i32 + offset) as usize;
                    let target_block = match block_map.get(&target_pc) { Some(&b) => b, None => return false };
                    let fall_block = match block_map.get(&pc) { Some(&b) => b, None => return false };
                    builder.ins().brif(cmp, target_block, &[], fall_block, &[]);
                    terminated = true;
                }

                // ============================================================
                // CONTROL FLOW
                // ============================================================
                Op::Jump => {
                    let offset = chunk.read_i32(pc); pc += 4;
                    let target_pc = (pc as i32 + offset) as usize;
                    if let Some(&block) = block_map.get(&target_pc) {
                        builder.ins().jump(block, &[]);
                        terminated = true;
                    } else {
                        return false;
                    }
                }
                Op::JumpIfFalse => {
                    let offset = chunk.read_i32(pc); pc += 4;
                    let cond = match vstack.pop() { Some(v) => v, None => return false };
                    let target_pc = (pc as i32 + offset) as usize;
                    let target_block = match block_map.get(&target_pc) { Some(&b) => b, None => return false };
                    let fall_block = match block_map.get(&pc) { Some(&b) => b, None => return false };
                    // Use is_truthy helper
                    let call = builder.ins().call(helpers.is_truthy, &[cond]);
                    let truthy = builder.inst_results(call)[0];
                    let z = builder.ins().iconst(types::I64, 0);
                    let cmp = builder.ins().icmp(IntCC::Equal, truthy, z);
                    builder.ins().brif(cmp, target_block, &[], fall_block, &[]);
                    terminated = true;
                }
                Op::JumpIfTrue => {
                    let offset = chunk.read_i32(pc); pc += 4;
                    let cond = match vstack.pop() { Some(v) => v, None => return false };
                    let target_pc = (pc as i32 + offset) as usize;
                    let target_block = match block_map.get(&target_pc) { Some(&b) => b, None => return false };
                    let fall_block = match block_map.get(&pc) { Some(&b) => b, None => return false };
                    let call = builder.ins().call(helpers.is_truthy, &[cond]);
                    let truthy = builder.inst_results(call)[0];
                    let z = builder.ins().iconst(types::I64, 0);
                    let cmp = builder.ins().icmp(IntCC::NotEqual, truthy, z);
                    builder.ins().brif(cmp, target_block, &[], fall_block, &[]);
                    terminated = true;
                }
                Op::Loop => {
                    let offset = chunk.read_i32(pc); pc += 4;
                    // pc now points after the operand; pre-scan used (pc_before + 4 - offset)
                    // which equals (pc - offset) since pc_before + 4 = pc
                    let target_pc = (pc as i32 - offset) as usize;
                    if let Some(&block) = block_map.get(&target_pc) {
                        builder.ins().jump(block, &[]);
                        terminated = true;
                    } else {
                        return false;
                    }
                }

                // ============================================================
                // CALLS
                // ============================================================
                Op::CallLocal => {
                    let target = chunk.read_u16(pc) as usize; pc += 2;
                    let argc = code[pc] as usize; pc += 1;
                    let mut args = Vec::new();
                    for _ in 0..argc {
                        match vstack.pop() { Some(v) => args.push(v), None => return false }
                    }
                    args.reverse();
                    if target == chunk_idx {
                        // Self-recursive call — pass depth+1 as last arg
                        let cur_depth = builder.use_var(depth_var);
                        args.push(cur_depth);
                        let call = builder.ins().call(self_ref, &args);
                        vstack.push(builder.inst_results(call)[0]);
                    } else {
                        // Call a different function via vm_call helper
                        let target_val = builder.ins().iconst(types::I64, target as i64);
                        if argc == 0 {
                            let null_ptr = builder.ins().iconst(types::I64, 0);
                            let argc_val = builder.ins().iconst(types::I64, 0);
                            let call = builder.ins().call(helpers.vm_call, &[target_val, null_ptr, argc_val]);
                            vstack.push(builder.inst_results(call)[0]);
                        } else {
                            let slot = builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, (argc * 8) as u32, 3));
                            for (i, val) in args.iter().enumerate() {
                                builder.ins().stack_store(*val, slot, (i * 8) as i32);
                            }
                            let ptr = builder.ins().stack_addr(types::I64, slot, 0);
                            let argc_val = builder.ins().iconst(types::I64, argc as i64);
                            let call = builder.ins().call(helpers.vm_call, &[target_val, ptr, argc_val]);
                            vstack.push(builder.inst_results(call)[0]);
                        }
                    }
                }
                Op::Call => {
                    let name_idx = chunk.read_u16(pc) as u64; pc += 2;
                    let argc = code[pc] as usize; pc += 1;
                    let res = code[pc] as u64; pc += 1;
                    let mut args = Vec::new();
                    for _ in 0..argc {
                        match vstack.pop() { Some(v) => args.push(v), None => return false }
                    }
                    args.reverse();
                    // Store args in a stack slot
                    if argc == 0 {
                        let null_ptr = builder.ins().iconst(types::I64, 0);
                        let name_idx_val = builder.ins().iconst(types::I64, name_idx as i64);
                        let res_val = builder.ins().iconst(types::I64, res as i64);
                        let argc_val = builder.ins().iconst(types::I64, 0);
                        let call = builder.ins().call(helpers.generic_call, &[name_idx_val, res_val, null_ptr, argc_val]);
                        vstack.push(builder.inst_results(call)[0]);
                    } else {
                        let slot = builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, (argc * 8) as u32, 3));
                        for (i, val) in args.iter().enumerate() {
                            builder.ins().stack_store(*val, slot, (i * 8) as i32);
                        }
                        let ptr = builder.ins().stack_addr(types::I64, slot, 0);
                        let name_idx_val = builder.ins().iconst(types::I64, name_idx as i64);
                        let res_val = builder.ins().iconst(types::I64, res as i64);
                        let argc_val = builder.ins().iconst(types::I64, argc as i64);
                        let call = builder.ins().call(helpers.generic_call, &[name_idx_val, res_val, ptr, argc_val]);
                        vstack.push(builder.inst_results(call)[0]);
                    }
                }
                Op::CallBuiltin => {
                    let name_idx = chunk.read_u16(pc) as u64; pc += 2;
                    let argc = code[pc] as usize; pc += 1;
                    let mut args = Vec::new();
                    for _ in 0..argc {
                        match vstack.pop() { Some(v) => args.push(v), None => return false }
                    }
                    args.reverse();
                    if argc == 0 {
                        let null_ptr = builder.ins().iconst(types::I64, 0);
                        let name_idx_val = builder.ins().iconst(types::I64, name_idx as i64);
                        let argc_val = builder.ins().iconst(types::I64, 0);
                        let call = builder.ins().call(helpers.call_builtin, &[name_idx_val, null_ptr, argc_val]);
                        vstack.push(builder.inst_results(call)[0]);
                    } else {
                        let slot = builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, (argc * 8) as u32, 3));
                        for (i, val) in args.iter().enumerate() {
                            builder.ins().stack_store(*val, slot, (i * 8) as i32);
                        }
                        let ptr = builder.ins().stack_addr(types::I64, slot, 0);
                        let name_idx_val = builder.ins().iconst(types::I64, name_idx as i64);
                        let argc_val = builder.ins().iconst(types::I64, argc as i64);
                        let call = builder.ins().call(helpers.call_builtin, &[name_idx_val, ptr, argc_val]);
                        vstack.push(builder.inst_results(call)[0]);
                    }
                }
                Op::Return => {
                    let val = match vstack.pop() { Some(v) => v, None => return false };
                    builder.ins().return_(&[val]);
                    terminated = true;
                }
                Op::ReturnVoid => {
                    let z = builder.ins().iconst(types::I64, void_raw as i64);
                    builder.ins().return_(&[z]);
                    terminated = true;
                }

                // ============================================================
                // STACK OPS
                // ============================================================
                Op::Pop => { vstack.pop(); }
                Op::Dup => {
                    if let Some(&v) = vstack.last() {
                        let call = builder.ins().call(helpers.clone_value, &[v]);
                        vstack.push(builder.inst_results(call)[0]);
                    } else {
                        return false;
                    }
                }
                Op::CheckCancel => {} // skip in JIT

                // ============================================================
                // COLLECTIONS — need VM/interp, bail if encountered in function chunk
                // These are uncommon in hot inner loops.
                // ============================================================
                Op::MakeList => {
                    let count = chunk.read_u16(pc) as usize; pc += 2;
                    if count == 0 {
                        // Empty list: pass null pointer with count 0
                        let null_ptr = builder.ins().iconst(types::I64, 0);
                        let count_val = builder.ins().iconst(types::I64, 0);
                        let call = builder.ins().call(helpers.make_list, &[null_ptr, count_val]);
                        vstack.push(builder.inst_results(call)[0]);
                    } else {
                        let slot = builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, (count * 8) as u32, 3));
                        // Pop items in reverse (last pushed = highest index)
                        let mut items = Vec::with_capacity(count);
                        for _ in 0..count {
                            match vstack.pop() { Some(v) => items.push(v), None => return false }
                        }
                        items.reverse();
                        for (i, val) in items.iter().enumerate() {
                            builder.ins().stack_store(*val, slot, (i * 8) as i32);
                        }
                        let ptr = builder.ins().stack_addr(types::I64, slot, 0);
                        let count_val = builder.ins().iconst(types::I64, count as i64);
                        let call = builder.ins().call(helpers.make_list, &[ptr, count_val]);
                        vstack.push(builder.inst_results(call)[0]);
                    }
                }
                Op::MakeObject => {
                    let count = chunk.read_u16(pc) as usize; pc += 2;
                    // count = number of key-value pairs, need count*2 slots
                    if count == 0 {
                        let null_ptr = builder.ins().iconst(types::I64, 0);
                        let count_val = builder.ins().iconst(types::I64, 0);
                        let call = builder.ins().call(helpers.make_object, &[null_ptr, count_val]);
                        vstack.push(builder.inst_results(call)[0]);
                    } else {
                        let slot = builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, (count * 2 * 8) as u32, 3));
                        // Pop pairs in reverse: each pair is (key, value) — value on top
                        // vstack top to bottom: val_n, key_n, ..., val_0, key_0
                        // We need in memory: key_0, val_0, key_1, val_1, ...
                        let mut pairs = Vec::with_capacity(count);
                        for _ in 0..count {
                            let val = match vstack.pop() { Some(v) => v, None => return false };
                            let key = match vstack.pop() { Some(v) => v, None => return false };
                            pairs.push((key, val));
                        }
                        pairs.reverse();
                        for (i, (key, val)) in pairs.iter().enumerate() {
                            builder.ins().stack_store(*key, slot, (i * 2 * 8) as i32);
                            builder.ins().stack_store(*val, slot, (i * 2 * 8 + 8) as i32);
                        }
                        let ptr = builder.ins().stack_addr(types::I64, slot, 0);
                        let count_val = builder.ins().iconst(types::I64, count as i64);
                        let call = builder.ins().call(helpers.make_object, &[ptr, count_val]);
                        vstack.push(builder.inst_results(call)[0]);
                    }
                }
                Op::MakeString => {
                    let count = chunk.read_u16(pc) as usize; pc += 2;
                    if count == 0 {
                        // Empty string
                        let null_ptr = builder.ins().iconst(types::I64, 0);
                        let count_val = builder.ins().iconst(types::I64, 0);
                        let call = builder.ins().call(helpers.make_string, &[null_ptr, count_val]);
                        vstack.push(builder.inst_results(call)[0]);
                    } else {
                        let slot = builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, (count * 8) as u32, 3));
                        let mut parts = Vec::with_capacity(count);
                        for _ in 0..count {
                            match vstack.pop() { Some(v) => parts.push(v), None => return false }
                        }
                        parts.reverse();
                        for (i, val) in parts.iter().enumerate() {
                            builder.ins().stack_store(*val, slot, (i * 8) as i32);
                        }
                        let ptr = builder.ins().stack_addr(types::I64, slot, 0);
                        let count_val = builder.ins().iconst(types::I64, count as i64);
                        let call = builder.ins().call(helpers.make_string, &[ptr, count_val]);
                        vstack.push(builder.inst_results(call)[0]);
                    }
                }
                Op::MakeRange => {
                    // MakeRange has no operand — just pops start and end from stack
                    let end_v = match vstack.pop() { Some(v) => v, None => return false };
                    let start_v = match vstack.pop() { Some(v) => v, None => return false };
                    let call = builder.ins().call(helpers.make_range, &[start_v, end_v]);
                    vstack.push(builder.inst_results(call)[0]);
                }
                Op::Index => {
                    let idx = match vstack.pop() { Some(v) => v, None => return false };
                    let target = match vstack.pop() { Some(v) => v, None => return false };
                    let call = builder.ins().call(helpers.index_get, &[target, idx]);
                    vstack.push(builder.inst_results(call)[0]);
                }
                Op::IndexSet => {
                    let val = match vstack.pop() { Some(v) => v, None => return false };
                    let idx = match vstack.pop() { Some(v) => v, None => return false };
                    let target = match vstack.pop() { Some(v) => v, None => return false };
                    builder.ins().call(helpers.index_set, &[target, idx, val]);
                }
                Op::FieldGet => {
                    let field_idx = chunk.read_u16(pc) as u64; pc += 2;
                    let obj = match vstack.pop() { Some(v) => v, None => return false };
                    let field_idx_val = builder.ins().iconst(types::I64, field_idx as i64);
                    let call = builder.ins().call(helpers.field_get, &[obj, field_idx_val]);
                    vstack.push(builder.inst_results(call)[0]);
                }
                Op::FieldSet => {
                    let field_idx = chunk.read_u16(pc) as u64; pc += 2;
                    let val = match vstack.pop() { Some(v) => v, None => return false };
                    let obj = match vstack.pop() { Some(v) => v, None => return false };
                    let field_idx_val = builder.ins().iconst(types::I64, field_idx as i64);
                    builder.ins().call(helpers.field_set, &[obj, field_idx_val, val]);
                }

                // ============================================================
                // OPCODES THAT NEED VM/INTERP — bail for function-level JIT
                // (These typically appear only in top-level chunks which have
                //  param_count=0 and are not JIT candidates anyway.)
                // ============================================================
                Op::DefineFunction => {
                    let name_idx = chunk.read_u16(pc) as u64; pc += 2;
                    let fn_chunk = chunk.read_u16(pc) as u64; pc += 2;
                    let name_idx_val = builder.ins().iconst(types::I64, name_idx as i64);
                    let fn_chunk_val = builder.ins().iconst(types::I64, fn_chunk as i64);
                    builder.ins().call(helpers.define_function, &[name_idx_val, fn_chunk_val]);
                }
                Op::DefineEnum => {
                    pc += 2; // global_slot
                    if pc + 1 < code.len() {
                        let count = u16::from_le_bytes([code[pc], code[pc+1]]) as usize;
                        let _ = 2 + count * 2; // skip enum data (return false follows)
                    }
                    return false; // needs vm
                }
                Op::Import => {
                    let _path_idx = chunk.read_u16(pc); 
                    return false; // complex — involves file loading, parsing, compilation
                }
                Op::Free => {
                    let idx = chunk.read_u16(pc) as u64; pc += 2;
                    let idx_val = builder.ins().iconst(types::I64, idx as i64);
                    builder.ins().call(helpers.free_global, &[idx_val]);
                }
                Op::Alias => {
                    let name_idx = chunk.read_u16(pc) as u64; pc += 2;
                    let target_idx = chunk.read_u16(pc) as u64; pc += 2;
                    let name_idx_val = builder.ins().iconst(types::I64, name_idx as i64);
                    let target_idx_val = builder.ins().iconst(types::I64, target_idx as i64);
                    builder.ins().call(helpers.alias, &[name_idx_val, target_idx_val]);
                }
                Op::Use => {
                    let path_idx = chunk.read_u16(pc) as u64; pc += 2;
                    let alias_idx = chunk.read_u16(pc) as u64; pc += 2;
                    let path_idx_val = builder.ins().iconst(types::I64, path_idx as i64);
                    let alias_idx_val = builder.ins().iconst(types::I64, alias_idx as i64);
                    builder.ins().call(helpers.use_fn, &[path_idx_val, alias_idx_val]);
                }
                Op::Throw => {
                    let val = match vstack.pop() { Some(v) => v, None => return false };
                    let call = builder.ins().call(helpers.throw, &[val]);
                    // Throw returns void; push result and continue (simplified error handling)
                    vstack.push(builder.inst_results(call)[0]);
                }
                Op::TryBegin => {
                    let _offset = chunk.read_i32(pc);
                    return false; // complex control flow
                }
                Op::TryEnd => {
                    let _offset = chunk.read_i32(pc);
                    return false;
                }
                Op::MakeLambda => {
                    let name_idx = chunk.read_u16(pc) as u64; pc += 2;
                    let res = code[pc] as u64; pc += 1;
                    let bound_count = code[pc] as usize; pc += 1;
                    let mut bound_args = Vec::new();
                    for _ in 0..bound_count {
                        match vstack.pop() { Some(v) => bound_args.push(v), None => return false }
                    }
                    bound_args.reverse();
                    if bound_count == 0 {
                        let null_ptr = builder.ins().iconst(types::I64, 0);
                        let name_idx_val = builder.ins().iconst(types::I64, name_idx as i64);
                        let res_val = builder.ins().iconst(types::I64, res as i64);
                        let count_val = builder.ins().iconst(types::I64, 0);
                        let call = builder.ins().call(helpers.make_lambda, &[name_idx_val, res_val, null_ptr, count_val]);
                        vstack.push(builder.inst_results(call)[0]);
                    } else {
                        let slot = builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, (bound_count * 8) as u32, 3));
                        for (i, val) in bound_args.iter().enumerate() {
                            builder.ins().stack_store(*val, slot, (i * 8) as i32);
                        }
                        let ptr = builder.ins().stack_addr(types::I64, slot, 0);
                        let name_idx_val = builder.ins().iconst(types::I64, name_idx as i64);
                        let res_val = builder.ins().iconst(types::I64, res as i64);
                        let count_val = builder.ins().iconst(types::I64, bound_count as i64);
                        let call = builder.ins().call(helpers.make_lambda, &[name_idx_val, res_val, ptr, count_val]);
                        vstack.push(builder.inst_results(call)[0]);
                    }
                }
                Op::ErrorCheck => {
                    let slot = chunk.read_u16(pc) as u64; pc += 2;
                    let slot_val = builder.ins().iconst(types::I64, slot as i64);
                    let call = builder.ins().call(helpers.error_check, &[slot_val]);
                    vstack.push(builder.inst_results(call)[0]);
                }
                Op::ErrorField => {
                    let slot = chunk.read_u16(pc) as u64; pc += 2;
                    let _field_idx = chunk.read_u16(pc); pc += 2;
                    let slot_val = builder.ins().iconst(types::I64, slot as i64);
                    let call = builder.ins().call(helpers.error_field, &[slot_val]);
                    vstack.push(builder.inst_results(call)[0]);
                }
                Op::SetErrorTolerant => {
                    let slot = chunk.read_u16(pc) as u64; pc += 2;
                    let slot_val = builder.ins().iconst(types::I64, slot as i64);
                    builder.ins().call(helpers.set_error_tolerant, &[slot_val]);
                }
                Op::RecordError => {
                    let slot = chunk.read_u16(pc) as u64; pc += 2;
                    let slot_val = builder.ins().iconst(types::I64, slot as i64);
                    builder.ins().call(helpers.record_error, &[slot_val]);
                }
                Op::OptionalCheck => {
                    let slot = chunk.read_u16(pc) as u64; pc += 2;
                    let slot_val = builder.ins().iconst(types::I64, slot as i64);
                    let call = builder.ins().call(helpers.optional_check, &[slot_val]);
                    vstack.push(builder.inst_results(call)[0]);
                }
                Op::Atomic => {
                    let val = match vstack.pop() { Some(v) => v, None => return false };
                    let call = builder.ins().call(helpers.atomic, &[val]);
                    vstack.push(builder.inst_results(call)[0]);
                }

                // ============================================================
                // SEND CONTEXT — needs vm
                // ============================================================
                Op::PushSendCtx => {
                    let val = match vstack.pop() { Some(v) => v, None => return false };
                    builder.ins().call(helpers.push_send_ctx, &[val]);
                }
                Op::PopSendCtx => {
                    builder.ins().call(helpers.pop_send_ctx, &[]);
                }
                Op::GetDollar => {
                    let call = builder.ins().call(helpers.get_dollar, &[]);
                    vstack.push(builder.inst_results(call)[0]);
                }
                Op::GetDollarIndex => {
                    let idx = chunk.read_u16(pc) as u64; pc += 2;
                    let idx_val = builder.ins().iconst(types::I64, idx as i64);
                    let call = builder.ins().call(helpers.get_dollar_index, &[idx_val]);
                    vstack.push(builder.inst_results(call)[0]);
                }
                Op::GetDollarField => {
                    let field_idx = chunk.read_u16(pc) as u64; pc += 2;
                    let field_idx_val = builder.ins().iconst(types::I64, field_idx as i64);
                    let call = builder.ins().call(helpers.get_dollar_field, &[field_idx_val]);
                    vstack.push(builder.inst_results(call)[0]);
                }

                // ============================================================
                // SCOPE (no-op in JIT)
                // ============================================================
                Op::PushScope | Op::PopScope => {}

                Op::IntToFloat => {
                    // Convert stack top: if int, widen to float
                    if let Some(v) = vstack.last().copied() {
                        // Check tag: is it TAG_INT?
                        let tag = builder.ins().band_imm(v, NB_TAG_MASK as i64);
                        let tag_int = builder.ins().iconst(types::I64, NB_TAG_INT as i64);
                        let is_int = builder.ins().icmp(IntCC::Equal, tag, tag_int);

                        let convert_block = builder.create_block();
                        let skip_block = builder.create_block();
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);

                        builder.ins().brif(is_int, convert_block, &[], skip_block, &[]);

                        // Convert: extract int, cast to f64, store as float bits
                        builder.switch_to_block(convert_block);
                        builder.seal_block(convert_block);
                        let payload = builder.ins().band_imm(v, NB_PAYLOAD_MASK as i64);
                        let sign_extended = builder.ins().ishl_imm(payload, 16);
                        let sign_extended = builder.ins().sshr_imm(sign_extended, 16);
                        let as_float = builder.ins().fcvt_from_sint(types::F64, sign_extended);
                        let float_bits = builder.ins().bitcast(types::I64, cranelift_codegen::ir::MemFlags::new(), as_float);
                        builder.ins().jump(merge_block, &[float_bits]);

                        // Skip: already float or other type, keep as-is
                        builder.switch_to_block(skip_block);
                        builder.seal_block(skip_block);
                        builder.ins().jump(merge_block, &[v]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        let result = builder.block_params(merge_block)[0];
                        *vstack.last_mut().expect("checked above") = result;
                    }
                }
            }
        }

        // If we reach the end without a terminator
        if !terminated {
            if let Some(val) = vstack.pop() {
                builder.ins().return_(&[val]);
            } else {
                let z = builder.ins().iconst(types::I64, void_raw as i64);
                builder.ins().return_(&[z]);
            }
        }

        // Seal all blocks
        for &block in block_map.values() {
            if !block_sealed.contains(&block) {
                builder.seal_block(block);
            }
        }

        true
    }
}
