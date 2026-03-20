use std::io::Write;
use crate::interpreter::value::Value;
use super::registry::{BuiltinRegistry, Param, Type};

pub fn register(reg: &mut BuiltinRegistry) -> Result<(), String> {
    reg.add("print", &[Param::Required(Type::String)], Type::Void, |args| {
        print!("{}", args[0]);
        let _ = std::io::stdout().flush();
        Ok(Value::void())
    })?;

    reg.add("println", &[Param::Required(Type::String)], Type::Void, |args| {
        println!("{}", args[0]);
        Ok(Value::void())
    })?;

    reg.add("errprint", &[Param::Required(Type::String)], Type::Void, |args| {
        eprint!("{}", args[0]);
        let _ = std::io::stderr().flush();
        Ok(Value::void())
    })?;

    reg.add("errprintln", &[Param::Required(Type::String)], Type::Void, |args| {
        eprintln!("{}", args[0]);
        Ok(Value::void())
    })?;

    Ok(())
}
