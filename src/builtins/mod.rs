use crate::interpreter::value::Value;
use crate::interpreter::Interpreter;

/// Try to call a built-in function. Returns None if not a built-in.
pub fn call_builtin(name: &str, args: &[Value], _interp: &mut Interpreter) -> Option<Result<Value, String>> {
    match name {
        // I/O
        "echo" | "print" => Some(Ok(builtin_echo(args))),

        // Type conversions
        "str" => Some(builtin_str(args)),
        "int" => Some(builtin_int(args)),
        "float" => Some(builtin_float(args)),
        "bool" => Some(builtin_bool(args)),

        // Type checking
        "type" => Some(builtin_type(args)),

        // Collections
        "len" => Some(builtin_len(args)),
        "push" => Some(builtin_push(args)),
        "pop" => Some(builtin_pop(args)),
        "has" => Some(builtin_has(args)),
        "keys" => Some(builtin_keys(args)),
        "values" => Some(builtin_values(args)),

        // String operations
        "split" => Some(builtin_split(args)),
        "join" => Some(builtin_join(args)),
        "trim" => Some(builtin_trim(args)),
        "upper" => Some(builtin_upper(args)),
        "lower" => Some(builtin_lower(args)),
        "replace" => Some(builtin_replace(args)),
        "contains" => Some(builtin_contains(args)),

        // System
        "env" => Some(builtin_env(args)),
        "exit" => Some(builtin_exit(args)),
        "os" => Some(Ok(builtin_os())),
        "sleep" => Some(builtin_sleep(args)),

        _ => None,
    }
}

fn builtin_echo(args: &[Value]) -> Value {
    let parts: Vec<String> = args.iter().map(ToString::to_string).collect();
    println!("{}", parts.join(" "));
    Value::Void
}

fn builtin_str(args: &[Value]) -> Result<Value, String> {
    expect_args("str", args, 1)?;
    Ok(Value::String(args[0].to_string()))
}

fn builtin_int(args: &[Value]) -> Result<Value, String> {
    expect_args("int", args, 1)?;
    match &args[0] {
        Value::Int(n) => Ok(Value::Int(*n)),
        Value::Float(n) => {
            #[expect(clippy::cast_possible_truncation)]
            let i = *n as i64;
            Ok(Value::Int(i))
        }
        Value::String(s) => s.parse::<i64>()
            .map(Value::Int)
            .map_err(|_| format!("Cannot convert '{s}' to int")),
        Value::Bool(b) => Ok(Value::Int(i64::from(*b))),
        other => Err(format!("Cannot convert {} to int", other.type_name())),
    }
}

fn builtin_float(args: &[Value]) -> Result<Value, String> {
    expect_args("float", args, 1)?;
    match &args[0] {
        Value::Float(n) => Ok(Value::Float(*n)),
        Value::Int(n) => {
            #[expect(clippy::cast_precision_loss)]
            let f = *n as f64;
            Ok(Value::Float(f))
        }
        Value::String(s) => s.parse::<f64>()
            .map(Value::Float)
            .map_err(|_| format!("Cannot convert '{s}' to float")),
        other => Err(format!("Cannot convert {} to float", other.type_name())),
    }
}

fn builtin_bool(args: &[Value]) -> Result<Value, String> {
    expect_args("bool", args, 1)?;
    Ok(Value::Bool(args[0].is_truthy()))
}

fn builtin_type(args: &[Value]) -> Result<Value, String> {
    expect_args("type", args, 1)?;
    Ok(Value::String(args[0].type_name().to_string()))
}

fn builtin_len(args: &[Value]) -> Result<Value, String> {
    expect_args("len", args, 1)?;
    match &args[0] {
        Value::List(l) => {
            let len = i64::try_from(l.len()).map_err(|_| "List length overflows i64".to_string())?;
            Ok(Value::Int(len))
        }
        Value::String(s) => {
            let len = i64::try_from(s.len()).map_err(|_| "String length overflows i64".to_string())?;
            Ok(Value::Int(len))
        }
        Value::Object(m) => {
            let len = i64::try_from(m.len()).map_err(|_| "Object length overflows i64".to_string())?;
            Ok(Value::Int(len))
        }
        other => Err(format!("Cannot get length of {}", other.type_name())),
    }
}

fn builtin_push(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("push() expects 2 args, got {}", args.len()));
    }
    if let Value::List(list) = &args[0] {
        let mut new_list = list.clone();
        new_list.push(args[1].clone());
        Ok(Value::List(new_list))
    } else {
        Err(format!("Cannot push to {}", args[0].type_name()))
    }
}

fn builtin_pop(args: &[Value]) -> Result<Value, String> {
    expect_args("pop", args, 1)?;
    if let Value::List(list) = &args[0] {
        if list.is_empty() {
            return Err("Cannot pop from empty list".to_string());
        }
        let mut new_list = list.clone();
        new_list.pop();
        Ok(Value::List(new_list))
    } else {
        Err(format!("Cannot pop from {}", args[0].type_name()))
    }
}

fn builtin_has(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("has() expects 2 args, got {}", args.len()));
    }
    if let (Value::Object(map), Value::String(key)) = (&args[0], &args[1]) {
        Ok(Value::Bool(map.contains_key(key)))
    } else {
        Err("has() expects (object, string)".to_string())
    }
}

fn builtin_keys(args: &[Value]) -> Result<Value, String> {
    expect_args("keys", args, 1)?;
    if let Value::Object(map) = &args[0] {
        let keys: Vec<Value> = map.keys().map(|k| Value::String(k.clone())).collect();
        Ok(Value::List(keys))
    } else {
        Err(format!("Cannot get keys of {}", args[0].type_name()))
    }
}

fn builtin_values(args: &[Value]) -> Result<Value, String> {
    expect_args("values", args, 1)?;
    if let Value::Object(map) = &args[0] {
        let vals: Vec<Value> = map.values().cloned().collect();
        Ok(Value::List(vals))
    } else {
        Err(format!("Cannot get values of {}", args[0].type_name()))
    }
}

fn builtin_split(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("split() expects 2 args, got {}", args.len()));
    }
    if let (Value::String(s), Value::String(delim)) = (&args[0], &args[1]) {
        let parts: Vec<Value> = s.split(delim.as_str()).map(|p| Value::String(p.to_string())).collect();
        Ok(Value::List(parts))
    } else {
        Err("split() expects (string, string)".to_string())
    }
}

fn builtin_join(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("join() expects 2 args, got {}", args.len()));
    }
    if let (Value::List(items), Value::String(delim)) = (&args[0], &args[1]) {
        let parts: Vec<String> = items.iter().map(ToString::to_string).collect();
        Ok(Value::String(parts.join(delim)))
    } else {
        Err("join() expects (list, string)".to_string())
    }
}

fn builtin_trim(args: &[Value]) -> Result<Value, String> {
    expect_args("trim", args, 1)?;
    if let Value::String(s) = &args[0] {
        Ok(Value::String(s.trim().to_string()))
    } else {
        Err(format!("Cannot trim {}", args[0].type_name()))
    }
}

fn builtin_upper(args: &[Value]) -> Result<Value, String> {
    expect_args("upper", args, 1)?;
    if let Value::String(s) = &args[0] {
        Ok(Value::String(s.to_uppercase()))
    } else {
        Err(format!("Cannot uppercase {}", args[0].type_name()))
    }
}

fn builtin_lower(args: &[Value]) -> Result<Value, String> {
    expect_args("lower", args, 1)?;
    if let Value::String(s) = &args[0] {
        Ok(Value::String(s.to_lowercase()))
    } else {
        Err(format!("Cannot lowercase {}", args[0].type_name()))
    }
}

fn builtin_replace(args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err(format!("replace() expects 3 args, got {}", args.len()));
    }
    if let (Value::String(s), Value::String(old), Value::String(new)) = (&args[0], &args[1], &args[2]) {
        Ok(Value::String(s.replace(old.as_str(), new.as_str())))
    } else {
        Err("replace() expects (string, string, string)".to_string())
    }
}

fn builtin_contains(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("contains() expects 2 args, got {}", args.len()));
    }
    match (&args[0], &args[1]) {
        (Value::String(s), Value::String(sub)) => {
            Ok(Value::Bool(s.contains(sub.as_str())))
        }
        (Value::List(list), val) => {
            let found = list.iter().any(|item| {
                match (item, val) {
                    (Value::Int(a), Value::Int(b)) => a == b,
                    (Value::String(a), Value::String(b)) => a == b,
                    (Value::Bool(a), Value::Bool(b)) => a == b,
                    (Value::Float(a), Value::Float(b)) => (a - b).abs() < f64::EPSILON,
                    _ => false,
                }
            });
            Ok(Value::Bool(found))
        }
        _ => Err("contains() expects (string, string) or (list, value)".to_string()),
    }
}

fn builtin_env(args: &[Value]) -> Result<Value, String> {
    expect_args("env", args, 1)?;
    if let Value::String(key) = &args[0] {
        std::env::var(key).map(Value::String)
            .map_err(|_| format!("Environment variable '{key}' not set"))
    } else {
        Err(format!("env() expects string, got {}", args[0].type_name()))
    }
}

fn builtin_exit(args: &[Value]) -> Result<Value, String> {
    let code = if args.is_empty() {
        0
    } else if let Value::Int(n) = &args[0] {
        i32::try_from(*n).unwrap_or(1)
    } else {
        1
    };
    std::process::exit(code);
}

fn builtin_os() -> Value {
    let os = if cfg!(target_os = "windows") { "windows" }
        else if cfg!(target_os = "macos") { "macos" }
        else if cfg!(target_os = "linux") { "linux" }
        else { "unknown" };
    Value::String(os.to_string())
}

fn builtin_sleep(args: &[Value]) -> Result<Value, String> {
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

fn expect_args(name: &str, args: &[Value], expected: usize) -> Result<(), String> {
    if args.len() == expected {
        return Ok(());
    }
    Err(format!("{name}() expects {expected} arg(s), got {}", args.len()))
}
