use crate::interpreter::value::Value;
use super::registry::{BuiltinRegistry, Type};

pub fn register(reg: &mut BuiltinRegistry) -> Result<(), String> {
    reg.add("refresh_env", &[], Type::Void, |_args| {
        crate::exec::clear_cache();
        Ok(Value::void())
    })?;

    Ok(())
}
