use std::rc::Rc;
use crate::interpreter::value::{Value, new_list};
use super::expect_args;

pub fn builtin_split(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("split() expects 2 args, got {}", args.len()));
    }
    if let (Value::String(s), Value::String(delim)) = (&args[0], &args[1]) {
        let parts: Vec<Value> = s.split(&**delim).map(|p| Value::String(Rc::from(p))).collect();
        Ok(new_list(parts))
    } else {
        Err("split() expects (string, string)".to_string())
    }
}

pub fn builtin_join(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("join() expects 2 args, got {}", args.len()));
    }
    if let (Value::List(items), Value::String(delim)) = (&args[0], &args[1]) {
        let items = items.borrow();
        let parts: Vec<String> = items.iter().map(ToString::to_string).collect();
        Ok(Value::String(Rc::from(parts.join(&**delim))))
    } else {
        Err("join() expects (list, string)".to_string())
    }
}

pub fn builtin_trim(args: &[Value]) -> Result<Value, String> {
    expect_args("trim", args, 1)?;
    if let Value::String(s) = &args[0] {
        Ok(Value::String(Rc::from(s.trim())))
    } else {
        Err(format!("Cannot trim {}", args[0].type_name()))
    }
}

pub fn builtin_upper(args: &[Value]) -> Result<Value, String> {
    expect_args("upper", args, 1)?;
    if let Value::String(s) = &args[0] {
        Ok(Value::String(Rc::from(s.to_uppercase())))
    } else {
        Err(format!("Cannot uppercase {}", args[0].type_name()))
    }
}

pub fn builtin_lower(args: &[Value]) -> Result<Value, String> {
    expect_args("lower", args, 1)?;
    if let Value::String(s) = &args[0] {
        Ok(Value::String(Rc::from(s.to_lowercase())))
    } else {
        Err(format!("Cannot lowercase {}", args[0].type_name()))
    }
}

pub fn builtin_replace(args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err(format!("replace() expects 3 args, got {}", args.len()));
    }
    if let (Value::String(s), Value::String(old), Value::String(new)) = (&args[0], &args[1], &args[2]) {
        Ok(Value::String(Rc::from(s.replace(&**old, new))))
    } else {
        Err("replace() expects (string, string, string)".to_string())
    }
}

pub fn builtin_contains(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("contains() expects 2 args, got {}", args.len()));
    }
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

// --- New string functions ---

pub fn builtin_starts_with(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("starts_with() expects 2 args, got {}", args.len()));
    }
    if let (Value::String(s), Value::String(prefix)) = (&args[0], &args[1]) {
        Ok(Value::Bool(s.starts_with(&**prefix)))
    } else {
        Err("starts_with() expects (string, string)".to_string())
    }
}

pub fn builtin_ends_with(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("ends_with() expects 2 args, got {}", args.len()));
    }
    if let (Value::String(s), Value::String(suffix)) = (&args[0], &args[1]) {
        Ok(Value::Bool(s.ends_with(&**suffix)))
    } else {
        Err("ends_with() expects (string, string)".to_string())
    }
}

pub fn builtin_substr(args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err(format!("substr() expects 3 args, got {}", args.len()));
    }
    if let (Value::String(s), Value::Int(start), Value::Int(len)) = (&args[0], &args[1], &args[2]) {
        let start = usize::try_from(*start).map_err(|_| format!("Invalid start index: {start}"))?;
        let len = usize::try_from(*len).map_err(|_| format!("Invalid length: {len}"))?;
        let result: String = s.chars().skip(start).take(len).collect();
        Ok(Value::String(Rc::from(result)))
    } else {
        Err("substr() expects (string, int, int)".to_string())
    }
}

pub fn builtin_index_of(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("index_of() expects 2 args, got {}", args.len()));
    }
    if let (Value::String(s), Value::String(sub)) = (&args[0], &args[1]) {
        s.find(&**sub).map_or_else(|| Ok(Value::Int(-1)), |pos| {
            // Convert byte position to char position
            let char_pos = s[..pos].chars().count();
            #[expect(clippy::cast_possible_wrap)]
            Ok(Value::Int(char_pos as i64))
        })
    } else {
        Err("index_of() expects (string, string)".to_string())
    }
}

pub fn builtin_pad_left(args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err(format!("pad_left() expects 3 args, got {}", args.len()));
    }
    if let (Value::String(s), Value::Int(width), Value::String(pad)) = (&args[0], &args[1], &args[2]) {
        let width = usize::try_from(*width).map_err(|_| format!("Invalid width: {width}"))?;
        let pad_char = pad.chars().next().unwrap_or(' ');
        let current_len = s.chars().count();
        if current_len >= width {
            Ok(Value::String(s.clone()))
        } else {
            let padding: String = std::iter::repeat_n(pad_char, width - current_len).collect();
            Ok(Value::String(Rc::from(format!("{padding}{s}"))))
        }
    } else {
        Err("pad_left() expects (string, int, string)".to_string())
    }
}

pub fn builtin_pad_right(args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err(format!("pad_right() expects 3 args, got {}", args.len()));
    }
    if let (Value::String(s), Value::Int(width), Value::String(pad)) = (&args[0], &args[1], &args[2]) {
        let width = usize::try_from(*width).map_err(|_| format!("Invalid width: {width}"))?;
        let pad_char = pad.chars().next().unwrap_or(' ');
        let current_len = s.chars().count();
        if current_len >= width {
            Ok(Value::String(s.clone()))
        } else {
            let padding: String = std::iter::repeat_n(pad_char, width - current_len).collect();
            Ok(Value::String(Rc::from(format!("{s}{padding}"))))
        }
    } else {
        Err("pad_right() expects (string, int, string)".to_string())
    }
}

pub fn builtin_repeat(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("repeat() expects 2 args, got {}", args.len()));
    }
    if let (Value::String(s), Value::Int(n)) = (&args[0], &args[1]) {
        let n = usize::try_from(*n).map_err(|_| format!("Invalid repeat count: {n}"))?;
        Ok(Value::String(Rc::from(s.repeat(n))))
    } else {
        Err("repeat() expects (string, int)".to_string())
    }
}

pub fn builtin_reverse(args: &[Value]) -> Result<Value, String> {
    expect_args("reverse", args, 1)?;
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

pub fn builtin_chars(args: &[Value]) -> Result<Value, String> {
    expect_args("chars", args, 1)?;
    if let Value::String(s) = &args[0] {
        let chars: Vec<Value> = s.chars().map(|c| Value::String(Rc::from(c.to_string()))).collect();
        Ok(new_list(chars))
    } else {
        Err(format!("chars() expects string, got {}", args[0].type_name()))
    }
}

pub fn builtin_match(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("match() expects 2 args, got {}", args.len()));
    }
    if let (Value::String(s), Value::String(pattern)) = (&args[0], &args[1]) {
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
    } else {
        Err("match() expects (string, string)".to_string())
    }
}

pub fn builtin_match_all(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("match_all() expects 2 args, got {}", args.len()));
    }
    if let (Value::String(s), Value::String(pattern)) = (&args[0], &args[1]) {
        let re = regex::Regex::new(pattern)
            .map_err(|e| format!("Invalid regex '{pattern}': {e}"))?;
        let matches: Vec<Value> = re.find_iter(s)
            .map(|m| Value::String(Rc::from(m.as_str())))
            .collect();
        Ok(new_list(matches))
    } else {
        Err("match_all() expects (string, string)".to_string())
    }
}
