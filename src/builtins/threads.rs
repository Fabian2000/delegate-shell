use std::sync::{Arc, Mutex};
use std::thread;
use crate::interpreter::value::{Value, ThreadJoinHandle, new_list};
use crate::interpreter::Interpreter;
use crate::parser::ast::Resolution;
use super::registry::{BuiltinRegistry, Param, Type};

pub fn register(reg: &mut BuiltinRegistry) -> Result<(), String> {
    reg.add_interp("thread", &[Param::Required(Type::Lambda)], Type::ThreadHandle, builtin_thread)?;
    reg.add("wait", &[Param::Required(Type::ThreadHandle)], Type::Dyn, builtin_wait)?;
    reg.add("wait_all", &[Param::Required(Type::List)], Type::List, builtin_wait_all)?;
    reg.add("wait_any", &[Param::Required(Type::List)], Type::Dyn, builtin_wait_any)?;

    Ok(())
}

fn builtin_thread(args: &[Value], interp: &mut Interpreter) -> Result<Value, String> {
    let lambda = match &args[0] {
        Value::Lambda { name, resolution, bound_args } => {
            (name.clone(), *resolution, bound_args.iter().map(Value::to_sendable).collect::<Vec<_>>())
        }
        other => return Err(format!("thread() expects lambda, got {}", other.type_name())),
    };

    // Clone user functions so the thread has its own copy
    let user_fns = interp.env.clone_fns();

    let (fn_name, res_code, sendable_args) = lambda;

    let handle = thread::spawn(move || {
        let mut thread_interp = Interpreter::new()
            .map_err(|e| format!("thread init failed: {e}"))?;
        thread_interp.env.restore_fns(user_fns);

        let call_args: Vec<Value> = sendable_args.into_iter().map(Value::from_sendable).collect();
        let resolution = match res_code {
            1 => Resolution::OwnFirst,
            2 => Resolution::SystemOnly,
            _ => Resolution::Normal,
        };
        let result = thread_interp.call_resolved(&fn_name, resolution, call_args);
        result.map(|v| v.to_sendable())
    });

    Ok(Value::ThreadHandle(Arc::new(Mutex::new(ThreadJoinHandle {
        handle: Some(handle),
    }))))
}

fn builtin_wait(args: &[Value]) -> Result<Value, String> {
    if let Value::ThreadHandle(th) = &args[0] {
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
    let handles = match &args[0] {
        Value::List(l) => l.borrow().clone(),
        other => return Err(format!("wait_all() expects list, got {}", other.type_name())),
    };
    let mut results = Vec::with_capacity(handles.len());
    for h in &handles {
        if let Value::ThreadHandle(th) = h {
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
    let handles = match &args[0] {
        Value::List(l) => l.borrow().clone(),
        other => return Err(format!("wait_any() expects list, got {}", other.type_name())),
    };
    // Simple polling approach: check each thread until one is done
    loop {
        for h in &handles {
            if let Value::ThreadHandle(th) = h {
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
