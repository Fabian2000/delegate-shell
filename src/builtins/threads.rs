use std::sync::{Arc, Mutex};
use std::thread;
use crate::interpreter::value::{Value, ThreadJoinHandle, new_list};
use crate::interpreter::Runtime;
use crate::parser::ast::Resolution;
use super::registry::{BuiltinRegistry, Param, Type};

pub fn register(reg: &mut BuiltinRegistry) -> Result<(), String> {
    reg.add_interp("thread", &[Param::Required(Type::Lambda)], Type::ThreadHandle, builtin_thread)?;
    reg.add("wait", &[Param::Required(Type::ThreadHandle)], Type::Dyn, builtin_wait)?;
    reg.add("wait_all", &[Param::Required(Type::List)], Type::List, builtin_wait_all)?;
    reg.add("wait_any", &[Param::Required(Type::List)], Type::Dyn, builtin_wait_any)?;

    Ok(())
}

fn builtin_thread(args: &[Value], interp: &mut Runtime) -> Result<Value, String> {
    let lambda = if let Some(data) = args[0].as_lambda() {
        (data.name.clone(), data.resolution, data.bound_args.iter().map(Value::to_sendable).collect::<Vec<_>>())
    } else {
        return Err(format!("thread() expects lambda, got {}", args[0].type_name()));
    };

    // Clone user functions so the thread has its own copy
    let user_fns = interp.env.clone_fns();

    // In AOT (and JIT) mode, user functions live as VM-compiled chunks rather
    // than AST entries — capture a snapshot here so the worker can construct
    // its own VM. None in pure tree-walk mode.
    let vm_snapshot = crate::vm::jit::snapshot_vm_state();

    // Inherit sandbox flags and name mappings from parent interpreter
    let parent_allow_exec = interp.allow_exec();
    let parent_allow_network = interp.allow_network();
    let parent_use_paths = interp.env.use_paths.clone();
    let parent_aliases = interp.env.aliases.clone();

    let (fn_name, res_code, sendable_args) = lambda;

    let handle = thread::spawn(move || {
        let mut thread_interp = Runtime::new()
            .map_err(|e| format!("thread init failed: {e}"))?;
        thread_interp.set_allow_exec(parent_allow_exec);
        thread_interp.set_allow_network(parent_allow_network);
        thread_interp.env.restore_fns(user_fns);
        thread_interp.env.use_paths = parent_use_paths;
        thread_interp.env.aliases = parent_aliases;

        // Build worker VM if we inherited an AOT/VM bytecode snapshot, and
        // wire up the thread-local JIT context so callbacks resolve user fns.
        // The Box keeps the VM alive for the duration of this closure.
        let mut _worker_vm: Option<Box<crate::vm::machine::VM>> = None;
        if let Some(snap) = vm_snapshot {
            let mut vm = Box::new(crate::vm::machine::VM::new());
            vm.chunks = snap.chunks;
            for (name, idx) in snap.fn_table {
                vm.register_fn(&name, idx);
            }
            unsafe {
                let vm_ptr: *mut crate::vm::machine::VM = &mut *vm;
                let interp_ptr: *mut Runtime = &mut thread_interp;
                let chunks_ptr: *const Vec<crate::vm::bytecode::Chunk> = &vm.chunks;
                crate::vm::jit::set_jit_context(vm_ptr, interp_ptr, chunks_ptr, 0);
            }
            _worker_vm = Some(vm);
        }

        let call_args: Vec<Value> = sendable_args.into_iter().map(Value::from_sendable).collect();
        let resolution = match res_code {
            1 => Resolution::OwnFirst,
            2 => Resolution::SystemOnly,
            _ => Resolution::Normal,
        };
        let result = thread_interp.call_resolved(&fn_name, resolution, call_args);
        result.map(|v| v.to_sendable())
    });

    Ok(Value::thread_handle(Arc::new(Mutex::new(ThreadJoinHandle {
        handle: Some(handle),
    }))))
}

fn builtin_wait(args: &[Value]) -> Result<Value, String> {
    if let Some(th) = args[0].as_thread_handle() {
        let mut guard = th.lock().map_err(|e| format!("wait(): lock error: {e}"))?;
        let handle = guard.handle.take()
            .ok_or("wait(): thread already joined")?;
        drop(guard);
        let result = handle.join()
            .map_err(|_| "wait(): thread panicked".to_string())?;
        result.map(Value::from_sendable)
    } else {
        Err(format!("wait() expects thread handle, got {}", args[0].type_name()))
    }
}

fn builtin_wait_all(args: &[Value]) -> Result<Value, String> {
    let handles = if let Some(l) = args[0].as_list_ref() {
        l.borrow().clone()
    } else {
        return Err(format!("wait_all() expects list, got {}", args[0].type_name()));
    };
    let mut results = Vec::with_capacity(handles.len());
    for h in &handles {
        if let Some(th) = h.as_thread_handle() {
            let mut guard = th.lock().map_err(|e| format!("wait_all(): lock error: {e}"))?;
            let handle = guard.handle.take()
                .ok_or("wait_all(): thread already joined")?;
            drop(guard);
            let result = handle.join()
                .map_err(|_| "wait_all(): thread panicked".to_string())?;
            results.push(Value::from_sendable(result?));
        } else {
            return Err(format!("wait_all() list must contain thread handles, got {}", h.type_name()));
        }
    }
    Ok(new_list(results))
}

fn builtin_wait_any(args: &[Value]) -> Result<Value, String> {
    let handles = if let Some(l) = args[0].as_list_ref() {
        l.borrow().clone()
    } else {
        return Err(format!("wait_any() expects list, got {}", args[0].type_name()));
    };
    // Simple polling approach: check each thread until one is done
    loop {
        for h in &handles {
            if let Some(th) = h.as_thread_handle() {
                let mut guard = th.lock().map_err(|e| format!("wait_any(): {e}"))?;
                if let Some(ref handle) = guard.handle
                    && handle.is_finished()
                {
                    let Some(handle) = guard.handle.take() else {
                        return Err("wait_any(): thread handle already consumed".to_string());
                    };
                    drop(guard);
                    let result = handle.join()
                        .map_err(|_| "wait_any(): thread panicked".to_string())?;
                    return result.map(Value::from_sendable);
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}
