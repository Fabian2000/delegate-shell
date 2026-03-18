use chrono::{Local, TimeZone, NaiveDateTime};
use indexmap::IndexMap;
use crate::interpreter::value::{Value, new_object};

pub fn builtin_now(_args: &[Value]) -> Value {
    let now = Local::now();
    let mut obj = IndexMap::new();
    obj.insert("year".to_string(), Value::Int(i64::from(now.format("%Y").to_string().parse::<i32>().unwrap_or(0))));
    obj.insert("month".to_string(), Value::Int(i64::from(now.format("%m").to_string().parse::<u32>().unwrap_or(0))));
    obj.insert("day".to_string(), Value::Int(i64::from(now.format("%d").to_string().parse::<u32>().unwrap_or(0))));
    obj.insert("hour".to_string(), Value::Int(i64::from(now.format("%H").to_string().parse::<u32>().unwrap_or(0))));
    obj.insert("min".to_string(), Value::Int(i64::from(now.format("%M").to_string().parse::<u32>().unwrap_or(0))));
    obj.insert("sec".to_string(), Value::Int(i64::from(now.format("%S").to_string().parse::<u32>().unwrap_or(0))));
    obj.insert("unix".to_string(), Value::Int(now.timestamp()));
    new_object(obj)
}

pub fn builtin_timestamp(_args: &[Value]) -> Value {
    let now = Local::now();
    Value::Int(now.timestamp())
}

pub fn builtin_date_format(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("date_format() expects 2 args, got {}", args.len()));
    }
    let ts = match &args[0] {
        Value::Int(n) => *n,
        other => return Err(format!("date_format() timestamp must be int, got {}", other.type_name())),
    };
    let pattern = match &args[1] {
        Value::String(s) => s.clone(),
        other => return Err(format!("date_format() pattern must be string, got {}", other.type_name())),
    };
    let dt = Local.timestamp_opt(ts, 0)
        .single()
        .ok_or_else(|| format!("Invalid timestamp: {ts}"))?;
    Ok(Value::String(std::rc::Rc::from(dt.format(&pattern).to_string())))
}

pub fn builtin_date_parse(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("date_parse() expects 2 args, got {}", args.len()));
    }
    let s = match &args[0] {
        Value::String(s) => &**s,
        other => return Err(format!("date_parse() input must be string, got {}", other.type_name())),
    };
    let pattern = match &args[1] {
        Value::String(s) => &**s,
        other => return Err(format!("date_parse() pattern must be string, got {}", other.type_name())),
    };
    let naive = NaiveDateTime::parse_from_str(s, pattern)
        .map_err(|e| format!("date_parse('{s}', '{pattern}'): {e}"))?;
    let local = Local.from_local_datetime(&naive)
        .single()
        .ok_or_else(|| format!("Ambiguous datetime: {s}"))?;
    Ok(Value::Int(local.timestamp()))
}

pub fn builtin_elapsed(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("elapsed() expects 2 args, got {}", args.len()));
    }
    let start = match &args[0] {
        Value::Int(n) => *n,
        other => return Err(format!("elapsed() start must be int, got {}", other.type_name())),
    };
    let end = match &args[1] {
        Value::Int(n) => *n,
        other => return Err(format!("elapsed() end must be int, got {}", other.type_name())),
    };
    Ok(Value::Int(end - start))
}
