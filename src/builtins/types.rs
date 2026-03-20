use std::rc::Rc;
use crate::interpreter::value::Value;
use super::registry::{BuiltinRegistry, Param, Type};

pub fn register(reg: &mut BuiltinRegistry) -> Result<(), String> {
    reg.add("str", &[Param::Required(Type::Dyn)], Type::String, |args| {
        Ok(Value::String(Rc::from(args[0].to_string())))
    })?;

    reg.add("int", &[Param::Required(Type::Dyn)], Type::Int, |args| {
        match &args[0] {
            Value::Int(n) => Ok(Value::Int(*n)),
            Value::Float(n) => Ok(Value::Int(*n as i64)),
            Value::String(s) => s.parse::<i64>().map(Value::Int).map_err(|_| format!("Cannot convert '{s}' to int")),
            Value::Bool(b) => Ok(Value::Int(i64::from(*b))),
            other => Err(format!("Cannot convert {} to int", other.type_name())),
        }
    })?;

    reg.add("float", &[Param::Required(Type::Dyn)], Type::Float, |args| {
        match &args[0] {
            Value::Float(n) => Ok(Value::Float(*n)),
            Value::Int(n) => Ok(Value::Float(*n as f64)),
            Value::String(s) => s.parse::<f64>().map(Value::Float).map_err(|_| format!("Cannot convert '{s}' to float")),
            other => Err(format!("Cannot convert {} to float", other.type_name())),
        }
    })?;

    reg.add("bool", &[Param::Required(Type::Dyn)], Type::Bool, |args| {
        Ok(Value::Bool(args[0].is_truthy()))
    })?;

    reg.add("type", &[Param::Required(Type::Dyn)], Type::String, |args| {
        Ok(Value::String(Rc::from(args[0].type_name())))
    })?;

    Ok(())
}
