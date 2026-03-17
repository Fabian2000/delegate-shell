use std::process::Command;
use crate::interpreter::value::Value;

/// Try to execute an external command. Returns None if the command is not found on PATH.
#[must_use]
pub fn try_exec_command(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    // Check if command exists on PATH
    let which = Command::new("which")
        .arg(name)
        .output();

    match which {
        Ok(output) if output.status.success() => {
            // Command exists — execute it
            Some(exec_command(name, args))
        }
        _ => None, // Not found on PATH
    }
}

fn exec_command(name: &str, args: &[Value]) -> Result<Value, String> {
    let str_args: Vec<String> = args.iter().map(value_to_arg).collect();

    let output = Command::new(name)
        .args(&str_args)
        .output()
        .map_err(|e| format!("Failed to execute '{name}': {e}"))?;

    let status = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    // If status != 0 and we're not in error-tolerant mode, this is an error
    if status != 0 {
        return Err(format!(
            "'{name}' exited with status {status}: {}",
            stderr.trim()
        ));
    }

    Ok(Value::CommandResult {
        status,
        out: stdout,
        err: stderr,
    })
}

fn value_to_arg(val: &Value) -> String {
    match val {
        Value::String(s) => s.clone(),
        Value::Int(n) => n.to_string(),
        Value::Float(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        other => other.to_string(),
    }
}
