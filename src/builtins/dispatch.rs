use crate::interpreter::value::Value;
use crate::interpreter::Runtime;
use super::registry::{BuiltinRegistry, Param, Type};

pub fn register(reg: &mut BuiltinRegistry) -> Result<(), String> {
    reg.add("refresh_env", &[], Type::Void, |_args| {
        crate::exec::clear_cache();
        Ok(Value::void())
    })?;

    reg.add_interp("on_event", &[Param::Required(Type::String), Param::Required(Type::Lambda)], Type::Void, builtin_on_event)?;

    Ok(())
}

fn builtin_on_event(args: &[Value], interp: &mut Runtime) -> Result<Value, String> {
    let event_name = args[0].as_str_ref()
        .ok_or_else(|| format!("on_event() arg 1 expects string, got {}", args[0].type_name()))?
        .to_string();
    if !args[1].is_lambda() {
        return Err(format!("on_event() arg 2 expects lambda, got {}", args[1].type_name()));
    }
    interp.event_handlers.entry(event_name).or_default().push(args[1].clone());
    Ok(Value::void())
}
