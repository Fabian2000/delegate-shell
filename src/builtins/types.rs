use std::rc::Rc;
use crate::interpreter::value::Value;
use super::expect_args;

pub fn builtin_str(args: &[Value]) -> Result<Value, String> {
    expect_args("str", args, 1)?;
    Ok(Value::String(Rc::from(args[0].to_string())))
}

pub fn builtin_int(args: &[Value]) -> Result<Value, String> {
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

pub fn builtin_float(args: &[Value]) -> Result<Value, String> {
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

pub fn builtin_bool(args: &[Value]) -> Result<Value, String> {
    expect_args("bool", args, 1)?;
    Ok(Value::Bool(args[0].is_truthy()))
}

pub fn builtin_type(args: &[Value]) -> Result<Value, String> {
    expect_args("type", args, 1)?;
    Ok(Value::String(Rc::from(args[0].type_name())))
}
