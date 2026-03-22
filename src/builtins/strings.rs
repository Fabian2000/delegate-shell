use std::rc::Rc;
use crate::interpreter::value::{Value, ValueKind as VK, new_list};
use super::registry::{BuiltinRegistry, Param, Type};

fn replace(args: &[Value]) -> Result<Value, String> {
    let Some(s) = args[0].as_str_ref() else {
        return Err(format!("expected string, got {}", args[0].type_name()));
    };
    let Some(old) = args[1].as_str_ref() else {
        return Err(format!("expected string, got {}", args[1].type_name()));
    };
    let Some(new) = args[2].as_str_ref() else {
        return Err(format!("expected string, got {}", args[2].type_name()));
    };
    Ok(Value::string_from(&s.replace(old, new)))
}

fn contains(args: &[Value]) -> Result<Value, String> {
    match (args[0].kind(), args[1].kind()) {
        (VK::String(s), VK::String(sub)) => {
            Ok(Value::bool(s.contains(sub)))
        }
        (VK::List(list), _) => {
            let list = list.borrow();
            let val = &args[1];
            let found = list.iter().any(|item| {
                match (item.kind(), val.kind()) {
                    (VK::Int(a), VK::Int(b)) => a == b,
                    (VK::String(a), VK::String(b)) => a == b,
                    (VK::Bool(a), VK::Bool(b)) => a == b,
                    (VK::Float(a), VK::Float(b)) => (a - b).abs() < f64::EPSILON,
                    _ => false,
                }
            });
            Ok(Value::bool(found))
        }
        _ => Err("contains() expects (string, string) or (list, value)".to_string()),
    }
}

fn substr(args: &[Value]) -> Result<Value, String> {
    let Some(s) = args[0].as_str_ref() else {
        return Err(format!("expected string, got {}", args[0].type_name()));
    };
    let Some(start) = args[1].as_int() else {
        return Err(format!("expected int, got {}", args[1].type_name()));
    };
    let Some(len) = args[2].as_int() else {
        return Err(format!("expected int, got {}", args[2].type_name()));
    };
    let start = usize::try_from(start).map_err(|_| format!("Invalid start index: {start}"))?;
    let len = usize::try_from(len).map_err(|_| format!("Invalid length: {len}"))?;
    let result: String = s.chars().skip(start).take(len).collect();
    Ok(Value::string(Rc::from(result)))
}

fn index_of(args: &[Value]) -> Result<Value, String> {
    let Some(s) = args[0].as_str_ref() else {
        return Err(format!("expected string, got {}", args[0].type_name()));
    };
    let Some(sub) = args[1].as_str_ref() else {
        return Err(format!("expected string, got {}", args[1].type_name()));
    };
    s.find(sub).map_or_else(|| Ok(Value::int(-1)), |pos| {
        let char_pos = s[..pos].chars().count();
        Ok(Value::int(i64::try_from(char_pos).unwrap_or(i64::MAX)))
    })
}

fn pad_left(args: &[Value]) -> Result<Value, String> {
    let Some(s) = args[0].as_str_ref() else {
        return Err(format!("expected string, got {}", args[0].type_name()));
    };
    let Some(width) = args[1].as_int() else {
        return Err(format!("expected int, got {}", args[1].type_name()));
    };
    let Some(pad) = args[2].as_str_ref() else {
        return Err(format!("expected string, got {}", args[2].type_name()));
    };
    let width = usize::try_from(width).map_err(|_| format!("Invalid width: {width}"))?;
    let pad_char = pad.chars().next().unwrap_or(' ');
    let current_len = s.chars().count();
    if current_len >= width {
        Ok(Value::string_from(s))
    } else {
        let padding: String = std::iter::repeat_n(pad_char, width - current_len).collect();
        Ok(Value::string_from(&format!("{padding}{s}")))
    }
}

fn pad_right(args: &[Value]) -> Result<Value, String> {
    let Some(s) = args[0].as_str_ref() else {
        return Err(format!("expected string, got {}", args[0].type_name()));
    };
    let Some(width) = args[1].as_int() else {
        return Err(format!("expected int, got {}", args[1].type_name()));
    };
    let Some(pad) = args[2].as_str_ref() else {
        return Err(format!("expected string, got {}", args[2].type_name()));
    };
    let width = usize::try_from(width).map_err(|_| format!("Invalid width: {width}"))?;
    let pad_char = pad.chars().next().unwrap_or(' ');
    let current_len = s.chars().count();
    if current_len >= width {
        Ok(Value::string_from(s))
    } else {
        let padding: String = std::iter::repeat_n(pad_char, width - current_len).collect();
        Ok(Value::string_from(&format!("{s}{padding}")))
    }
}

fn repeat(args: &[Value]) -> Result<Value, String> {
    let Some(s) = args[0].as_str_ref() else {
        return Err(format!("expected string, got {}", args[0].type_name()));
    };
    let Some(n) = args[1].as_int() else {
        return Err(format!("expected int, got {}", args[1].type_name()));
    };
    let n = usize::try_from(n).map_err(|_| format!("Invalid repeat count: {n}"))?;
    Ok(Value::string_from(&s.repeat(n)))
}

fn reverse(args: &[Value]) -> Result<Value, String> {
    match args[0].kind() {
        VK::String(s) => {
            let rev: String = s.chars().rev().collect();
            Ok(Value::string_from(&rev))
        }
        VK::List(list) => {
            let mut items = list.borrow().clone();
            items.reverse();
            Ok(new_list(items))
        }
        _ => Err(format!("Cannot reverse {}", args[0].type_name())),
    }
}

fn match_regex(args: &[Value]) -> Result<Value, String> {
    let Some(s) = args[0].as_str_ref() else {
        return Err(format!("expected string, got {}", args[0].type_name()));
    };
    let Some(pattern) = args[1].as_str_ref() else {
        return Err(format!("expected string, got {}", args[1].type_name()));
    };
    let re = regex::Regex::new(pattern)
        .map_err(|e| format!("Invalid regex '{pattern}': {e}"))?;
    re.captures(s).map_or_else(|| Ok(Value::bool(false)), |caps| {
        let mut groups: Vec<Value> = Vec::new();
        for i in 0..caps.len() {
            let m = caps.get(i).map_or_else(Value::void, |m| Value::string_from(m.as_str()));
            groups.push(m);
        }
        Ok(new_list(groups))
    })
}

fn match_all(args: &[Value]) -> Result<Value, String> {
    let Some(s) = args[0].as_str_ref() else {
        return Err(format!("expected string, got {}", args[0].type_name()));
    };
    let Some(pattern) = args[1].as_str_ref() else {
        return Err(format!("expected string, got {}", args[1].type_name()));
    };
    let re = regex::Regex::new(pattern)
        .map_err(|e| format!("Invalid regex '{pattern}': {e}"))?;
    let matches: Vec<Value> = re.find_iter(s)
        .map(|m| Value::string_from(m.as_str()))
        .collect();
    Ok(new_list(matches))
}

pub fn register(reg: &mut BuiltinRegistry) -> Result<(), String> {
    reg.add("split", &[Param::Required(Type::String), Param::Required(Type::String)], Type::List, |args| {
        let Some(s) = args[0].as_str_ref() else {
            return Err(format!("expected string, got {}", args[0].type_name()));
        };
        let Some(delim) = args[1].as_str_ref() else {
            return Err(format!("expected string, got {}", args[1].type_name()));
        };
        let parts: Vec<Value> = s.split(delim).map(Value::string_from).collect();
        Ok(new_list(parts))
    })?;

    reg.add("join", &[Param::Required(Type::List), Param::Required(Type::String)], Type::String, |args| {
        let Some(items) = args[0].as_list_ref() else {
            return Err(format!("expected list, got {}", args[0].type_name()));
        };
        let Some(delim) = args[1].as_str_ref() else {
            return Err(format!("expected string, got {}", args[1].type_name()));
        };
        let items = items.borrow();
        let parts: Vec<String> = items.iter().map(ToString::to_string).collect();
        Ok(Value::string_from(&parts.join(delim)))
    })?;

    reg.add("trim", &[Param::Required(Type::String)], Type::String, |args| {
        let Some(s) = args[0].as_str_ref() else {
            return Err(format!("expected string, got {}", args[0].type_name()));
        };
        Ok(Value::string_from(s.trim()))
    })?;

    reg.add("upper", &[Param::Required(Type::String)], Type::String, |args| {
        let Some(s) = args[0].as_str_ref() else {
            return Err(format!("expected string, got {}", args[0].type_name()));
        };
        Ok(Value::string_from(&s.to_uppercase()))
    })?;

    reg.add("lower", &[Param::Required(Type::String)], Type::String, |args| {
        let Some(s) = args[0].as_str_ref() else {
            return Err(format!("expected string, got {}", args[0].type_name()));
        };
        Ok(Value::string_from(&s.to_lowercase()))
    })?;

    reg.add("replace", &[Param::Required(Type::String), Param::Required(Type::String), Param::Required(Type::String)], Type::String, replace)?;

    reg.add("contains", &[Param::Required(Type::Dyn), Param::Required(Type::Dyn)], Type::Bool, contains)?;

    reg.add("starts_with", &[Param::Required(Type::String), Param::Required(Type::String)], Type::Bool, |args| {
        let Some(s) = args[0].as_str_ref() else {
            return Err(format!("expected string, got {}", args[0].type_name()));
        };
        let Some(prefix) = args[1].as_str_ref() else {
            return Err(format!("expected string, got {}", args[1].type_name()));
        };
        Ok(Value::bool(s.starts_with(prefix)))
    })?;

    reg.add("ends_with", &[Param::Required(Type::String), Param::Required(Type::String)], Type::Bool, |args| {
        let Some(s) = args[0].as_str_ref() else {
            return Err(format!("expected string, got {}", args[0].type_name()));
        };
        let Some(suffix) = args[1].as_str_ref() else {
            return Err(format!("expected string, got {}", args[1].type_name()));
        };
        Ok(Value::bool(s.ends_with(suffix)))
    })?;

    reg.add("substr", &[Param::Required(Type::String), Param::Required(Type::Int), Param::Required(Type::Int)], Type::String, substr)?;
    reg.add("index_of", &[Param::Required(Type::String), Param::Required(Type::String)], Type::Int, index_of)?;
    reg.add("pad_left", &[Param::Required(Type::String), Param::Required(Type::Int), Param::Required(Type::String)], Type::String, pad_left)?;
    reg.add("pad_right", &[Param::Required(Type::String), Param::Required(Type::Int), Param::Required(Type::String)], Type::String, pad_right)?;
    reg.add("repeat", &[Param::Required(Type::String), Param::Required(Type::Int)], Type::String, repeat)?;
    reg.add("reverse", &[Param::Required(Type::Dyn)], Type::Dyn, reverse)?;

    reg.add("chars", &[Param::Required(Type::String)], Type::List, |args| {
        let Some(s) = args[0].as_str_ref() else {
            return Err(format!("expected string, got {}", args[0].type_name()));
        };
        let chars: Vec<Value> = s.chars().map(|c| Value::string_from(&c.to_string())).collect();
        Ok(new_list(chars))
    })?;

    reg.add("match", &[Param::Required(Type::String), Param::Required(Type::String)], Type::Dyn, match_regex)?;
    reg.add("match_all", &[Param::Required(Type::String), Param::Required(Type::String)], Type::List, match_all)?;

    Ok(())
}
