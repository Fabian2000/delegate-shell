use crate::interpreter::value::{Value, ValueKind as VK};
use super::registry::{BuiltinRegistry, Param, Type};

pub fn register(reg: &mut BuiltinRegistry) -> Result<(), String> {
    reg.add("str", &[Param::Required(Type::Dyn)], Type::String, |args| {
        Ok(Value::string_from(&args[0].to_string()))
    })?;

    reg.add("int", &[Param::Required(Type::Dyn)], Type::Int, |args| {
        match args[0].kind() {
            VK::Int(n) => Ok(Value::int(n)),
            VK::Float(n) => Ok(Value::int(n as i64)),
            VK::String(s) => s.parse::<i64>().map(Value::int).map_err(|_| format!("Cannot convert '{s}' to int")),
            VK::Bool(b) => Ok(Value::int(i64::from(b))),
            _ => Err(format!("Cannot convert {} to int", args[0].type_name())),
        }
    })?;

    reg.add("float", &[Param::Required(Type::Dyn)], Type::Float, |args| {
        match args[0].kind() {
            VK::Float(n) => Ok(Value::float(n)),
            VK::Int(n) => Ok(Value::float(n as f64)),
            VK::String(s) => s.parse::<f64>().map(Value::float).map_err(|_| format!("Cannot convert '{s}' to float")),
            _ => Err(format!("Cannot convert {} to float", args[0].type_name())),
        }
    })?;

    reg.add("bool", &[Param::Required(Type::Dyn)], Type::Bool, |args| {
        Ok(Value::bool(args[0].is_truthy()))
    })?;

    reg.add("type", &[Param::Required(Type::Dyn)], Type::String, |args| {
        Ok(Value::string_from(args[0].type_name()))
    })?;

    Ok(())
}
