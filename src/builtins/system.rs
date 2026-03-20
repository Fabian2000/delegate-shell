use std::process::{Command, Stdio};
use std::io::Write;
use indexmap::IndexMap;
use crate::interpreter::value::{Value, ValueKind as VK, new_list};
use super::registry::{BuiltinRegistry, Param, Type};

pub fn register(reg: &mut BuiltinRegistry) -> Result<(), String> {
    reg.add("env", &[Param::Required(Type::String)], Type::String, |args| {
        let Some(key) = args[0].as_str_ref() else { unreachable!() };
        std::env::var(key).map(|s| Value::string_from(&s))
            .map_err(|_| format!("Environment variable '{key}' not set"))
    })?;

    reg.add("exit", &[Param::Optional(Type::Int)], Type::Void, builtin_exit)?;

    reg.add("os", &[], Type::String, |_args| {
        let os = if cfg!(target_os = "windows") { "windows" }
            else if cfg!(target_os = "macos") { "macos" }
            else if cfg!(target_os = "linux") { "linux" }
            else { "unknown" };
        Ok(Value::string_from(os))
    })?;

    reg.add("sleep", &[Param::Required(Type::Number)], Type::Void, |args| {
        match args[0].kind() {
            VK::Int(ms) => {
                let millis = u64::try_from(ms).map_err(|_| format!("sleep() expects non-negative number, got {ms}"))?;
                std::thread::sleep(std::time::Duration::from_millis(millis));
                Ok(Value::void())
            }
            VK::Float(s) => {
                std::thread::sleep(std::time::Duration::from_secs_f64(s));
                Ok(Value::void())
            }
            _ => unreachable!(),
        }
    })?;

    reg.add("env_set", &[Param::Required(Type::String), Param::Required(Type::String)], Type::Void, |args| {
        let Some(key) = args[0].as_str_ref() else { unreachable!() };
        let Some(val) = args[1].as_str_ref() else { unreachable!() };
        // SAFETY: We are single-threaded at this point in the interpreter
        unsafe { std::env::set_var(key, val); }
        Ok(Value::void())
    })?;

    reg.add("env_all", &[], Type::Object, |_args| {
        let mut map = IndexMap::new();
        for (key, val) in std::env::vars() {
            map.insert(key, Value::string_from(&val));
        }
        Ok(crate::interpreter::value::new_object(map))
    })?;

    reg.add("pid", &[], Type::Int, |_args| {
        Ok(Value::int(i64::from(std::process::id())))
    })?;

    reg.add("arch", &[], Type::String, |_args| {
        Ok(Value::string_from(std::env::consts::ARCH))
    })?;

    reg.add("which", &[Param::Required(Type::String)], Type::String, builtin_which)?;

    reg.add("args", &[], Type::List, |_args| {
        let args: Vec<Value> = std::env::args().skip(2)
            .map(|s| Value::string_from(&s))
            .collect();
        Ok(new_list(args))
    })?;

    reg.add("input", &[Param::Required(Type::String)], Type::String, builtin_input)?;

    reg.add("exec", &[Param::Required(Type::String), Param::Required(Type::List)], Type::Dyn, builtin_exec)?;
    reg.add("exec_in", &[Param::Required(Type::String), Param::Required(Type::List), Param::Required(Type::Dyn)], Type::Dyn, builtin_exec_in)?;

    reg.add("home", &[], Type::String, |_args| {
        #[cfg(unix)]
        {
            if let Ok(home) = std::env::var("HOME") {
                return Ok(Value::string_from(&home));
            }
        }
        #[cfg(windows)]
        {
            if let Ok(profile) = std::env::var("USERPROFILE") {
                return Ok(Value::string_from(&profile));
            }
        }
        Err("Could not determine home directory".to_string())
    })?;

    Ok(())
}

// --- Named functions (complex logic) ---

fn builtin_exit(args: &[Value]) -> Result<Value, String> {
    let code = if args.is_empty() {
        0
    } else if let Some(n) = args[0].as_int() {
        i32::try_from(n).unwrap_or(1)
    } else {
        unreachable!()
    };
    std::process::exit(code);
}

fn builtin_which(args: &[Value]) -> Result<Value, String> {
    let Some(name) = args[0].as_str_ref() else { unreachable!() };
    let path_var = std::env::var("PATH").unwrap_or_default();
    let sep = if cfg!(windows) { ';' } else { ':' };
    for dir in path_var.split(sep) {
        let candidate = std::path::Path::new(dir).join(name);
        if candidate.is_file() {
            return Ok(Value::string_from(&candidate.to_string_lossy()));
        }
        if cfg!(windows) {
            for ext in &["exe", "cmd", "bat", "com"] {
                let with_ext = candidate.with_extension(ext);
                if with_ext.is_file() {
                    return Ok(Value::string_from(&with_ext.to_string_lossy()));
                }
            }
        }
    }
    Err(format!("which('{name}'): not found"))
}

fn builtin_input(args: &[Value]) -> Result<Value, String> {
    let Some(prompt) = args[0].as_str_ref() else { unreachable!() };
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    let mut buf = String::new();
    let bytes_read = std::io::stdin().read_line(&mut buf)
        .map_err(|e| format!("input(): {e}"))?;
    if bytes_read == 0 {
        return Err("input(): EOF".to_string());
    }
    if buf.ends_with('\n') { buf.pop(); }
    if buf.ends_with('\r') { buf.pop(); }
    Ok(Value::string_from(&buf))
}

fn builtin_exec(args: &[Value]) -> Result<Value, String> {
    let Some(path) = args[0].as_str_ref() else { unreachable!() };
    let Some(list) = args[1].as_list_ref() else { unreachable!() };
    let str_args: Vec<String> = list.borrow().iter().map(ToString::to_string).collect();

    let output = Command::new(path)
        .args(&str_args)
        .output()
        .map_err(|e| format!("exec('{path}'): {e}"))?;

    let status = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    Ok(Value::command_result(crate::interpreter::value::CommandResultData {
        status,
        out: stdout,
        err: stderr,
    }))
}

fn builtin_exec_in(args: &[Value]) -> Result<Value, String> {
    let Some(path) = args[0].as_str_ref() else { unreachable!() };
    let Some(list) = args[1].as_list_ref() else { unreachable!() };
    let str_args: Vec<String> = list.borrow().iter().map(ToString::to_string).collect();

    let stdin_data = if let Some(s) = args[2].as_str_ref() {
        s.to_string()
    } else if let Some(b) = args[2].as_bytes_ref() {
        String::from_utf8_lossy(b).to_string()
    } else {
        unreachable!()
    };

    let mut child = Command::new(path)
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

    Ok(Value::command_result(crate::interpreter::value::CommandResultData {
        status,
        out: stdout,
        err: stderr,
    }))
}
