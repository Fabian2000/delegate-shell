use crate::interpreter::value::{Value, ValueKind as VK};
use super::registry::{BuiltinRegistry, Param, Type};

fn to_f64(val: &Value) -> f64 {
    match val.kind() {
        VK::Int(n) => n as f64,
        VK::Float(n) => n,
        _ => unreachable!(),
    }
}

fn round(args: &[Value]) -> Result<Value, String> {
    let decimals = if args.len() == 2 {
        let Some(n) = args[1].as_int() else { unreachable!() };
        u32::try_from(n).map_err(|_| format!("Invalid decimals: {n}"))?
    } else {
        0
    };
    match args[0].kind() {
        VK::Float(n) => {
            if decimals == 0 {
                return Ok(Value::int(n.round() as i64));
            }
            let factor = 10_f64.powi(decimals.cast_signed());
            Ok(Value::float((n * factor).round() / factor))
        }
        VK::Int(n) => Ok(Value::int(n)),
        _ => unreachable!(),
    }
}

fn pow(args: &[Value]) -> Result<Value, String> {
    if let (Some(base), Some(exp)) = (args[0].as_int(), args[1].as_int()) {
        u32::try_from(exp).map_or_else(
            |_| Ok(Value::float((base as f64).powf(exp as f64))),
            |e| Ok(Value::int(base.pow(e))),
        )
    } else {
        let base = to_f64(&args[0]);
        let exp = to_f64(&args[1]);
        Ok(Value::float(base.powf(exp)))
    }
}

fn random_int(args: &[Value]) -> Result<Value, String> {
    let Some(min) = args[0].as_int() else { unreachable!() };
    let Some(max) = args[1].as_int() else { unreachable!() };
    if min > max {
        return Err(format!("random_int(): min ({min}) > max ({max})"));
    }
    let seed = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ).unwrap_or(0);
    let range = (max - min + 1).cast_unsigned();
    let val = min + (seed % range).cast_signed();
    Ok(Value::int(val))
}

pub fn register(reg: &mut BuiltinRegistry) -> Result<(), String> {
    reg.add("abs_num", &[Param::Required(Type::Number)], Type::Number, |args| {
        match args[0].kind() {
            VK::Int(n) => Ok(Value::int(n.abs())),
            VK::Float(n) => Ok(Value::float(n.abs())),
            _ => unreachable!(),
        }
    })?;

    reg.add("ceil", &[Param::Required(Type::Number)], Type::Int, |args| {
        match args[0].kind() {
            VK::Float(n) => Ok(Value::int(n.ceil() as i64)),
            VK::Int(n) => Ok(Value::int(n)),
            _ => unreachable!(),
        }
    })?;

    reg.add("floor", &[Param::Required(Type::Number)], Type::Int, |args| {
        match args[0].kind() {
            VK::Float(n) => Ok(Value::int(n.floor() as i64)),
            VK::Int(n) => Ok(Value::int(n)),
            _ => unreachable!(),
        }
    })?;

    reg.add("round", &[Param::Required(Type::Number), Param::Optional(Type::Int)], Type::Number, round)?;

    reg.add("sqrt", &[Param::Required(Type::Number)], Type::Float, |args| {
        let n = to_f64(&args[0]);
        if n < 0.0 {
            return Err("sqrt() of negative number".to_string());
        }
        Ok(Value::float(n.sqrt()))
    })?;

    reg.add("pow", &[Param::Required(Type::Number), Param::Required(Type::Number)], Type::Number, pow)?;

    reg.add("log", &[Param::Required(Type::Number)], Type::Float, |args| {
        Ok(Value::float(to_f64(&args[0]).ln()))
    })?;

    reg.add("log10", &[Param::Required(Type::Number)], Type::Float, |args| {
        Ok(Value::float(to_f64(&args[0]).log10()))
    })?;

    reg.add("sin", &[Param::Required(Type::Number)], Type::Float, |args| {
        Ok(Value::float(to_f64(&args[0]).sin()))
    })?;

    reg.add("cos", &[Param::Required(Type::Number)], Type::Float, |args| {
        Ok(Value::float(to_f64(&args[0]).cos()))
    })?;

    reg.add("tan", &[Param::Required(Type::Number)], Type::Float, |args| {
        Ok(Value::float(to_f64(&args[0]).tan()))
    })?;

    reg.add("random", &[], Type::Float, |_args| {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let val = ((seed ^ (seed >> 16)) & 0xFFFF_FFFF) as f64 / 0xFFFF_FFFF_u64 as f64;
        Ok(Value::float(val))
    })?;

    reg.add("random_int", &[Param::Required(Type::Int), Param::Required(Type::Int)], Type::Int, random_int)?;

    reg.add("pi", &[], Type::Float, |_args| {
        Ok(Value::float(std::f64::consts::PI))
    })?;

    reg.add("infinity", &[], Type::Float, |_args| {
        Ok(Value::float(f64::INFINITY))
    })?;

    Ok(())
}
