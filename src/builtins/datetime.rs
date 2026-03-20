use chrono::{Local, TimeZone, NaiveDateTime};
use indexmap::IndexMap;
use crate::interpreter::value::{Value, new_object};
use super::registry::{BuiltinRegistry, Param, Type};

fn now(_args: &[Value]) -> Result<Value, String> {
    let now = Local::now();
    let mut obj = IndexMap::new();
    obj.insert("year".to_string(), Value::int(i64::from(now.format("%Y").to_string().parse::<i32>().unwrap_or(0))));
    obj.insert("month".to_string(), Value::int(i64::from(now.format("%m").to_string().parse::<u32>().unwrap_or(0))));
    obj.insert("day".to_string(), Value::int(i64::from(now.format("%d").to_string().parse::<u32>().unwrap_or(0))));
    obj.insert("hour".to_string(), Value::int(i64::from(now.format("%H").to_string().parse::<u32>().unwrap_or(0))));
    obj.insert("min".to_string(), Value::int(i64::from(now.format("%M").to_string().parse::<u32>().unwrap_or(0))));
    obj.insert("sec".to_string(), Value::int(i64::from(now.format("%S").to_string().parse::<u32>().unwrap_or(0))));
    obj.insert("unix".to_string(), Value::int(now.timestamp()));
    Ok(new_object(obj))
}

fn date_format(args: &[Value]) -> Result<Value, String> {
    let Some(ts) = args[0].as_int() else { unreachable!() };
    let Some(pattern) = args[1].as_str_ref() else { unreachable!() };
    let dt = Local.timestamp_opt(ts, 0)
        .single()
        .ok_or_else(|| format!("Invalid timestamp: {ts}"))?;
    Ok(Value::string_from(&dt.format(pattern).to_string()))
}

fn date_parse(args: &[Value]) -> Result<Value, String> {
    let Some(s) = args[0].as_str_ref() else { unreachable!() };
    let Some(pattern) = args[1].as_str_ref() else { unreachable!() };
    let naive = NaiveDateTime::parse_from_str(s, pattern)
        .map_err(|e| format!("date_parse('{s}', '{pattern}'): {e}"))?;
    let local = Local.from_local_datetime(&naive)
        .single()
        .ok_or_else(|| format!("Ambiguous datetime: {s}"))?;
    Ok(Value::int(local.timestamp()))
}

pub fn register(reg: &mut BuiltinRegistry) -> Result<(), String> {
    reg.add("now", &[], Type::Object, now)?;
    reg.add("timestamp", &[], Type::Int, |_args| {
        Ok(Value::int(Local::now().timestamp()))
    })?;
    reg.add("date_format", &[Param::Required(Type::Int), Param::Required(Type::String)], Type::String, date_format)?;
    reg.add("date_parse", &[Param::Required(Type::String), Param::Required(Type::String)], Type::Int, date_parse)?;
    reg.add("elapsed", &[Param::Required(Type::Int), Param::Required(Type::Int)], Type::Int, |args| {
        let Some(start) = args[0].as_int() else { unreachable!() };
        let Some(end) = args[1].as_int() else { unreachable!() };
        Ok(Value::int(end - start))
    })?;

    Ok(())
}
