use std::sync::{Arc, Mutex};
use std::thread;
use crate::interpreter::value::{Value, ThreadJoinHandle, new_list};
use crate::interpreter::Interpreter;
use crate::parser::ast::Resolution;
use super::expect_args;

pub fn builtin_thread(args: &[Value], interp: &Interpreter) -> Result<Value, String> {
    expect_args("thread", args, 1)?;
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
        let mut thread_interp = Interpreter::new();
        // Restore user functions in thread interpreter
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

pub fn builtin_wait(args: &[Value]) -> Result<Value, String> {
    expect_args("wait", args, 1)?;
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

pub fn builtin_wait_all(args: &[Value]) -> Result<Value, String> {
    expect_args("wait_all", args, 1)?;
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

pub fn builtin_wait_any(args: &[Value]) -> Result<Value, String> {
    expect_args("wait_any", args, 1)?;
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
                    let handle = guard.handle.take().unwrap();
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
