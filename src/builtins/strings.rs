use std::rc::Rc;
use crate::interpreter::value::{Value, new_list};
use super::registry::{BuiltinRegistry, Param, Type};

fn replace(args: &[Value]) -> Result<Value, String> {
    let Value::String(s) = &args[0] else { unreachable!() };
    let Value::String(old) = &args[1] else { unreachable!() };
    let Value::String(new) = &args[2] else { unreachable!() };
    Ok(Value::String(Rc::from(s.replace(&**old, new))))
}

fn contains(args: &[Value]) -> Result<Value, String> {
    match (&args[0], &args[1]) {
        (Value::String(s), Value::String(sub)) => {
            Ok(Value::Bool(s.contains(&**sub)))
        }
        (Value::List(list), val) => {
            let list = list.borrow();
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

fn substr(args: &[Value]) -> Result<Value, String> {
    let Value::String(s) = &args[0] else { unreachable!() };
    let Value::Int(start) = &args[1] else { unreachable!() };
    let Value::Int(len) = &args[2] else { unreachable!() };
    let start = usize::try_from(*start).map_err(|_| format!("Invalid start index: {start}"))?;
    let len = usize::try_from(*len).map_err(|_| format!("Invalid length: {len}"))?;
    let result: String = s.chars().skip(start).take(len).collect();
    Ok(Value::String(Rc::from(result)))
}

fn index_of(args: &[Value]) -> Result<Value, String> {
    let Value::String(s) = &args[0] else { unreachable!() };
    let Value::String(sub) = &args[1] else { unreachable!() };
    s.find(&**sub).map_or_else(|| Ok(Value::Int(-1)), |pos| {
        let char_pos = s[..pos].chars().count();
        Ok(Value::Int(i64::try_from(char_pos).unwrap_or(i64::MAX)))
    })
}

fn pad_left(args: &[Value]) -> Result<Value, String> {
    let Value::String(s) = &args[0] else { unreachable!() };
    let Value::Int(width) = &args[1] else { unreachable!() };
    let Value::String(pad) = &args[2] else { unreachable!() };
    let width = usize::try_from(*width).map_err(|_| format!("Invalid width: {width}"))?;
    let pad_char = pad.chars().next().unwrap_or(' ');
    let current_len = s.chars().count();
    if current_len >= width {
        Ok(Value::String(s.clone()))
    } else {
        let padding: String = std::iter::repeat_n(pad_char, width - current_len).collect();
        Ok(Value::String(Rc::from(format!("{padding}{s}"))))
    }
}

fn pad_right(args: &[Value]) -> Result<Value, String> {
    let Value::String(s) = &args[0] else { unreachable!() };
    let Value::Int(width) = &args[1] else { unreachable!() };
    let Value::String(pad) = &args[2] else { unreachable!() };
    let width = usize::try_from(*width).map_err(|_| format!("Invalid width: {width}"))?;
    let pad_char = pad.chars().next().unwrap_or(' ');
    let current_len = s.chars().count();
    if current_len >= width {
        Ok(Value::String(s.clone()))
    } else {
        let padding: String = std::iter::repeat_n(pad_char, width - current_len).collect();
        Ok(Value::String(Rc::from(format!("{s}{padding}"))))
    }
}

fn repeat(args: &[Value]) -> Result<Value, String> {
    let Value::String(s) = &args[0] else { unreachable!() };
    let Value::Int(n) = &args[1] else { unreachable!() };
    let n = usize::try_from(*n).map_err(|_| format!("Invalid repeat count: {n}"))?;
    Ok(Value::String(Rc::from(s.repeat(n))))
}

fn reverse(args: &[Value]) -> Result<Value, String> {
    match &args[0] {
        Value::String(s) => {
            let rev: String = s.chars().rev().collect();
            Ok(Value::String(Rc::from(rev)))
        }
        Value::List(list) => {
            let mut items = list.borrow().clone();
            items.reverse();
            Ok(new_list(items))
        }
        other => Err(format!("Cannot reverse {}", other.type_name())),
    }
}

fn match_regex(args: &[Value]) -> Result<Value, String> {
    let Value::String(s) = &args[0] else { unreachable!() };
    let Value::String(pattern) = &args[1] else { unreachable!() };
    let re = regex::Regex::new(pattern)
        .map_err(|e| format!("Invalid regex '{pattern}': {e}"))?;
    re.captures(s).map_or_else(|| Ok(Value::Bool(false)), |caps| {
        let mut groups: Vec<Value> = Vec::new();
        for i in 0..caps.len() {
            let m = caps.get(i).map_or(Value::Void, |m| Value::String(Rc::from(m.as_str())));
            groups.push(m);
        }
        Ok(new_list(groups))
    })
}

fn match_all(args: &[Value]) -> Result<Value, String> {
    let Value::String(s) = &args[0] else { unreachable!() };
    let Value::String(pattern) = &args[1] else { unreachable!() };
    let re = regex::Regex::new(pattern)
        .map_err(|e| format!("Invalid regex '{pattern}': {e}"))?;
    let matches: Vec<Value> = re.find_iter(s)
        .map(|m| Value::String(Rc::from(m.as_str())))
        .collect();
    Ok(new_list(matches))
}

pub fn register(reg: &mut BuiltinRegistry) -> Result<(), String> {
    reg.add("split", &[Param::Required(Type::String), Param::Required(Type::String)], Type::List, |args| {
        let Value::String(s) = &args[0] else { unreachable!() };
        let Value::String(delim) = &args[1] else { unreachable!() };
        let parts: Vec<Value> = s.split(&**delim).map(|p| Value::String(Rc::from(p))).collect();
        Ok(new_list(parts))
    })?;

    reg.add("join", &[Param::Required(Type::List), Param::Required(Type::String)], Type::String, |args| {
        let Value::List(items) = &args[0] else { unreachable!() };
        let Value::String(delim) = &args[1] else { unreachable!() };
        let items = items.borrow();
        let parts: Vec<String> = items.iter().map(ToString::to_string).collect();
        Ok(Value::String(Rc::from(parts.join(&**delim))))
    })?;

    reg.add("trim", &[Param::Required(Type::String)], Type::String, |args| {
        let Value::String(s) = &args[0] else { unreachable!() };
        Ok(Value::String(Rc::from(s.trim())))
    })?;

    reg.add("upper", &[Param::Required(Type::String)], Type::String, |args| {
        let Value::String(s) = &args[0] else { unreachable!() };
        Ok(Value::String(Rc::from(s.to_uppercase())))
    })?;

    reg.add("lower", &[Param::Required(Type::String)], Type::String, |args| {
        let Value::String(s) = &args[0] else { unreachable!() };
        Ok(Value::String(Rc::from(s.to_lowercase())))
    })?;

    reg.add("replace", &[Param::Required(Type::String), Param::Required(Type::String), Param::Required(Type::String)], Type::String, replace)?;

    reg.add("contains", &[Param::Required(Type::Dyn), Param::Required(Type::Dyn)], Type::Bool, contains)?;

    reg.add("starts_with", &[Param::Required(Type::String), Param::Required(Type::String)], Type::Bool, |args| {
        let Value::String(s) = &args[0] else { unreachable!() };
        let Value::String(prefix) = &args[1] else { unreachable!() };
        Ok(Value::Bool(s.starts_with(&**prefix)))
    })?;

    reg.add("ends_with", &[Param::Required(Type::String), Param::Required(Type::String)], Type::Bool, |args| {
        let Value::String(s) = &args[0] else { unreachable!() };
        let Value::String(suffix) = &args[1] else { unreachable!() };
        Ok(Value::Bool(s.ends_with(&**suffix)))
    })?;

    reg.add("substr", &[Param::Required(Type::String), Param::Required(Type::Int), Param::Required(Type::Int)], Type::String, substr)?;
    reg.add("index_of", &[Param::Required(Type::String), Param::Required(Type::String)], Type::Int, index_of)?;
    reg.add("pad_left", &[Param::Required(Type::String), Param::Required(Type::Int), Param::Required(Type::String)], Type::String, pad_left)?;
    reg.add("pad_right", &[Param::Required(Type::String), Param::Required(Type::Int), Param::Required(Type::String)], Type::String, pad_right)?;
    reg.add("repeat", &[Param::Required(Type::String), Param::Required(Type::Int)], Type::String, repeat)?;
    reg.add("reverse", &[Param::Required(Type::Dyn)], Type::Dyn, reverse)?;

    reg.add("chars", &[Param::Required(Type::String)], Type::List, |args| {
        let Value::String(s) = &args[0] else { unreachable!() };
        let chars: Vec<Value> = s.chars().map(|c| Value::String(Rc::from(c.to_string()))).collect();
        Ok(new_list(chars))
    })?;

    reg.add("match", &[Param::Required(Type::String), Param::Required(Type::String)], Type::Dyn, match_regex)?;
    reg.add("match_all", &[Param::Required(Type::String), Param::Required(Type::String)], Type::List, match_all)?;

    Ok(())
}
