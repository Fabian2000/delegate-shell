use std::rc::Rc;
use indexmap::IndexMap;
use crate::interpreter::value::{Value, new_list, new_object};
use super::expect_args;

pub fn builtin_get_processes(_args: &[Value]) -> Result<Value, String> {
    let mut procs = Vec::new();

    #[cfg(unix)]
    {
        let entries = std::fs::read_dir("/proc")
            .map_err(|e| format!("get_processes(): {e}"))?;

        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            // Only numeric dirs (PIDs)
            if name_str.chars().all(|c| c.is_ascii_digit())
                && let Ok(proc) = read_proc_info(&name_str)
            {
                procs.push(proc);
            }
        }
    }

    #[cfg(windows)]
    {
        // Fallback: use tasklist command
        let output = std::process::Command::new("tasklist")
            .args(["/FO", "CSV", "/NH"])
            .output()
            .map_err(|e| format!("get_processes(): {e}"))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            let parts: Vec<&str> = line.split(',').collect();
            if parts.len() >= 2 {
                let name = parts[0].trim_matches('"').to_string();
                let pid_str = parts[1].trim_matches('"');
                if let Ok(pid) = pid_str.parse::<i64>() {
                    let mut map = IndexMap::new();
                    map.insert("name".to_string(), Value::String(Rc::from(name)));
                    map.insert("id".to_string(), Value::Int(pid));
                    procs.push(new_object(map));
                }
            }
        }
    }

    Ok(new_list(procs))
}

pub fn builtin_get_process_by_name(args: &[Value]) -> Result<Value, String> {
    expect_args("get_process_by_name", args, 1)?;
    let search = if let Value::String(s) = &args[0] {
        s.to_ascii_lowercase()
    } else {
        return Err(format!("get_process_by_name() expects string, got {}", args[0].type_name()));
    };

    let all = builtin_get_processes(&[])?;
    if let Value::List(list) = all {
        let list = list.borrow();
        let mut results = Vec::new();
        for proc in list.iter() {
            if let Value::Object(rc) = proc
                && let Some(Value::String(name)) = rc.borrow().get("name").cloned()
                && name.to_ascii_lowercase().contains(&search)
            {
                results.push(proc.clone());
            }
        }
        Ok(new_list(results))
    } else {
        Ok(new_list(vec![]))
    }
}

pub fn builtin_get_process_by_id(args: &[Value]) -> Result<Value, String> {
    expect_args("get_process_by_id", args, 1)?;
    let pid = if let Value::Int(n) = &args[0] {
        *n
    } else {
        return Err(format!("get_process_by_id() expects int, got {}", args[0].type_name()));
    };

    #[cfg(unix)]
    {
        let pid_str = pid.to_string();
        if let Ok(proc) = read_proc_info(&pid_str) {
            return Ok(proc);
        }
    }

    Err(format!("Process with id {pid} not found"))
}

pub fn builtin_kill_process(args: &[Value]) -> Result<Value, String> {
    expect_args("kill_process", args, 1)?;
    let pid = extract_pid(&args[0])?;

    #[cfg(unix)]
    {
        // SIGTERM = 15
        let ret = unsafe { kill(i32::try_from(pid).unwrap_or(-1), 15) };
        if ret == 0 {
            Ok(Value::Bool(true))
        } else {
            Err(format!("kill_process({pid}): failed (errno)"))
        }
    }

    #[cfg(windows)]
    {
        let output = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .output()
            .map_err(|e| format!("kill_process({pid}): {e}"))?;
        Ok(Value::Bool(output.status.success()))
    }

    #[cfg(not(any(unix, windows)))]
    Err("kill_process() not supported on this platform".to_string())
}

pub fn builtin_is_process_running(args: &[Value]) -> Result<Value, String> {
    expect_args("is_process_running", args, 1)?;
    let pid = extract_pid(&args[0])?;

    #[cfg(unix)]
    {
        // signal 0 = check if process exists
        let ret = unsafe { kill(i32::try_from(pid).unwrap_or(-1), 0) };
        Ok(Value::Bool(ret == 0))
    }

    #[cfg(windows)]
    {
        let output = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .map_err(|e| format!("is_process_running({pid}): {e}"))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(Value::Bool(stdout.contains(&pid.to_string())))
    }

    #[cfg(not(any(unix, windows)))]
    Ok(Value::Bool(false))
}

// --- Helpers ---

fn extract_pid(val: &Value) -> Result<i64, String> {
    match val {
        Value::Int(n) => Ok(*n),
        Value::Object(rc) => {
            if let Some(Value::Int(pid)) = rc.borrow().get("id").cloned() {
                Ok(pid)
            } else {
                Err("Process object has no 'id' field".to_string())
            }
        }
        _ => Err(format!("Expected process object or int, got {}", val.type_name())),
    }
}

#[cfg(unix)]
fn read_proc_info(pid_str: &str) -> Result<Value, String> {
    let comm_path = format!("/proc/{pid_str}/comm");
    let status_path = format!("/proc/{pid_str}/status");

    let name = std::fs::read_to_string(&comm_path)
        .map_err(|e| format!("read {comm_path}: {e}"))?
        .trim()
        .to_string();

    let pid: i64 = pid_str.parse().map_err(|_| "invalid pid".to_string())?;

    let mut map = IndexMap::new();
    map.insert("name".to_string(), Value::String(Rc::from(name)));
    map.insert("id".to_string(), Value::Int(pid));

    // Try to read memory info from status
    if let Ok(status) = std::fs::read_to_string(&status_path) {
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                let mem_str = rest.split_whitespace().next().unwrap_or("0");
                if let Ok(kb) = mem_str.parse::<i64>() {
                    map.insert("memory_kb".to_string(), Value::Int(kb));
                }
            }
        }
    }

    Ok(new_object(map))
}

#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}
