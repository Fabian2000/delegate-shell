use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::sync::LazyLock;
use std::sync::Mutex;
use crate::interpreter::value::Value;

/// Cache for PATH lookups: name -> Option<path>.
static CMD_CACHE: LazyLock<Mutex<HashMap<String, Option<PathBuf>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Clear the PATH lookup cache -- called by `refresh_env()`.
///
/// # Panics
///
/// Panics if the internal mutex is poisoned.
pub fn clear_cache() {
    CMD_CACHE.lock().unwrap().clear();
}

/// Check if a command exists on PATH without spawning a process.
fn find_on_path(name: &str) -> Option<PathBuf> {
    let mut cache = CMD_CACHE.lock().unwrap();
    if let Some(cached) = cache.get(name) {
        return cached.clone();
    }
    let result = env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths).find_map(|dir| {
            let full = dir.join(name);
            if full.is_file() { Some(full) } else { None }
        })
    });
    cache.insert(name.to_owned(), result.clone());
    result
}

/// Try to execute an external command. Returns None if the command is not found on PATH.
#[must_use]
pub fn try_exec_command(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    let path = find_on_path(name)?;
    Some(exec_command(path, name, args))
}

/// Execute a command at a specific path (used by `use` keyword).
///
/// # Errors
///
/// Returns an error if the command fails to execute.
pub fn exec_path(path: &str, args: &[Value]) -> Result<Value, String> {
    exec_command(PathBuf::from(path), path, args)
}

fn exec_command(path: PathBuf, name: &str, args: &[Value]) -> Result<Value, String> {
    let str_args: Vec<String> = args.iter().map(value_to_arg).collect();

    let output = Command::new(path)
        .args(&str_args)
        .output()
        .map_err(|e| format!("Failed to execute '{name}': {e}"))?;

    let status = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    // Exes never throw — always return Result { status, out, err }
    Ok(Value::CommandResult {
        status,
        out: stdout,
        err: stderr,
    })
}

fn value_to_arg(val: &Value) -> String {
    match val {
        Value::String(s) => s.to_string(),
        Value::Int(n) => n.to_string(),
        Value::Float(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        other => other.to_string(),
    }
}
