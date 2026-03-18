use std::rc::Rc;
use indexmap::IndexMap;
use std::process::{Command, Stdio};
use std::io::Write;
use crate::interpreter::value::{Value, new_list};
use super::expect_args;

pub fn builtin_env(args: &[Value]) -> Result<Value, String> {
    expect_args("env", args, 1)?;
    if let Value::String(key) = &args[0] {
        std::env::var(&**key).map(|s| Value::String(Rc::from(s)))
            .map_err(|_| format!("Environment variable '{key}' not set"))
    } else {
        Err(format!("env() expects string, got {}", args[0].type_name()))
    }
}

pub fn builtin_exit(args: &[Value]) -> Result<Value, String> {
    let code = if args.is_empty() {
        0
    } else if let Value::Int(n) = &args[0] {
        i32::try_from(*n).unwrap_or(1)
    } else {
        1
    };
    std::process::exit(code);
}

pub fn builtin_os() -> Value {
    let os = if cfg!(target_os = "windows") { "windows" }
        else if cfg!(target_os = "macos") { "macos" }
        else if cfg!(target_os = "linux") { "linux" }
        else { "unknown" };
    Value::String(Rc::from(os))
}

pub fn builtin_sleep(args: &[Value]) -> Result<Value, String> {
    expect_args("sleep", args, 1)?;
    match &args[0] {
        Value::Int(ms) => {
            let millis = u64::try_from(*ms).map_err(|_| format!("sleep() expects non-negative number, got {ms}"))?;
            std::thread::sleep(std::time::Duration::from_millis(millis));
            Ok(Value::Void)
        }
        Value::Float(s) => {
            std::thread::sleep(std::time::Duration::from_secs_f64(*s));
            Ok(Value::Void)
        }
        other => Err(format!("sleep() expects number, got {}", other.type_name())),
    }
}

// --- New system functions ---

pub fn builtin_env_set(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("env_set() expects 2 args, got {}", args.len()));
    }
    if let (Value::String(key), Value::String(val)) = (&args[0], &args[1]) {
        // SAFETY: We are single-threaded at this point in the interpreter
        unsafe { std::env::set_var(&**key, &**val); }
        Ok(Value::Void)
    } else {
        Err("env_set() expects (string, string)".to_string())
    }
}

pub fn builtin_env_all(_args: &[Value]) -> Value {
    let mut map = IndexMap::new();
    for (key, val) in std::env::vars() {
        map.insert(key, Value::String(Rc::from(val)));
    }
    crate::interpreter::value::new_object(map)
}

pub fn builtin_pid(_args: &[Value]) -> Value {
    Value::Int(i64::from(std::process::id()))
}

pub fn builtin_arch(_args: &[Value]) -> Value {
    Value::String(Rc::from(std::env::consts::ARCH))
}

pub fn builtin_which(args: &[Value]) -> Result<Value, String> {
    expect_args("which", args, 1)?;
    if let Value::String(name) = &args[0] {
        let path_var = std::env::var("PATH").unwrap_or_default();
        let sep = if cfg!(windows) { ';' } else { ':' };
        for dir in path_var.split(sep) {
            let candidate = std::path::Path::new(dir).join(&**name);
            if candidate.is_file() {
                return Ok(Value::String(Rc::from(candidate.to_string_lossy().to_string())));
            }
            if cfg!(windows) {
                for ext in &["exe", "cmd", "bat", "com"] {
                    let with_ext = candidate.with_extension(ext);
                    if with_ext.is_file() {
                        return Ok(Value::String(Rc::from(with_ext.to_string_lossy().to_string())));
                    }
                }
            }
        }
        Err(format!("which('{name}'): not found"))
    } else {
        Err(format!("which() expects string, got {}", args[0].type_name()))
    }
}

pub fn builtin_args(_args: &[Value]) -> Value {
    let args: Vec<Value> = std::env::args().skip(2) // skip binary name and script path
        .map(|s| Value::String(Rc::from(s)))
        .collect();
    new_list(args)
}

pub fn builtin_input(args: &[Value]) -> Result<Value, String> {
    // Print prompt if given
    if !args.is_empty() && let Value::String(prompt) = &args[0] {
        eprint!("{prompt}");
    }
    // Read from stdin — works for both piped and interactive
    let mut buf = String::new();
    std::io::stdin().read_line(&mut buf)
        .map_err(|e| format!("input(): {e}"))?;
    // Trim trailing newline
    if buf.ends_with('\n') { buf.pop(); }
    if buf.ends_with('\r') { buf.pop(); }
    Ok(Value::String(Rc::from(buf)))
}

pub fn builtin_home(_args: &[Value]) -> Result<Value, String> {
    #[cfg(unix)]
    {
        if let Ok(home) = std::env::var("HOME") {
            return Ok(Value::String(Rc::from(home)));
        }
    }
    #[cfg(windows)]
    {
        if let Ok(profile) = std::env::var("USERPROFILE") {
            return Ok(Value::String(Rc::from(profile)));
        }
    }
    Err("Could not determine home directory".to_string())
}

pub fn builtin_exec(args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err(format!("exec() expects 1-2 args, got {}", args.len()));
    }
    let path = if let Value::String(s) = &args[0] {
        s.to_string()
    } else {
        return Err(format!("exec() expects string path, got {}", args[0].type_name()));
    };

    let str_args: Vec<String> = if args.len() == 2 {
        if let Value::List(list) = &args[1] {
            list.borrow().iter().map(ToString::to_string).collect()
        } else {
            return Err(format!("exec() second arg must be list, got {}", args[1].type_name()));
        }
    } else {
        Vec::new()
    };

    let output = Command::new(&path)
        .args(&str_args)
        .output()
        .map_err(|e| format!("exec('{path}'): {e}"))?;

    let status = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    Ok(Value::CommandResult {
        status,
        out: stdout,
        err: stderr,
    })
}

pub fn builtin_exec_in(args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err(format!("exec_in() expects 3 args (path, args, stdin), got {}", args.len()));
    }
    let path = if let Value::String(s) = &args[0] {
        s.to_string()
    } else {
        return Err(format!("exec_in() expects string path, got {}", args[0].type_name()));
    };

    let str_args: Vec<String> = if let Value::List(list) = &args[1] {
        list.borrow().iter().map(ToString::to_string).collect()
    } else {
        return Err(format!("exec_in() second arg must be list, got {}", args[1].type_name()));
    };

    let stdin_data = if let Value::String(s) = &args[2] {
        s.to_string()
    } else if let Value::Bytes(b) = &args[2] {
        String::from_utf8_lossy(b).to_string()
    } else {
        return Err(format!("exec_in() third arg must be string or bytes, got {}", args[2].type_name()));
    };

    let mut child = Command::new(&path)
        .args(&str_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("exec_in('{path}'): {e}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(stdin_data.as_bytes());
    }

    let output = child.wait_with_output()
        .map_err(|e| format!("exec_in('{path}'): {e}"))?;

    let status = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    Ok(Value::CommandResult {
        status,
        out: stdout,
        err: stderr,
    })
}
