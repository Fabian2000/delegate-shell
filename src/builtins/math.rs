use crate::interpreter::value::Value;
use super::expect_args;

pub fn builtin_abs_num(args: &[Value]) -> Result<Value, String> {
    expect_args("abs_num", args, 1)?;
    match &args[0] {
        Value::Int(n) => Ok(Value::Int(n.abs())),
        Value::Float(n) => Ok(Value::Float(n.abs())),
        other => Err(format!("abs_num() expects number, got {}", other.type_name())),
    }
}

pub fn builtin_ceil(args: &[Value]) -> Result<Value, String> {
    expect_args("ceil", args, 1)?;
    match &args[0] {
        Value::Float(n) => {
            #[expect(clippy::cast_possible_truncation)]
            Ok(Value::Int(n.ceil() as i64))
        }
        Value::Int(n) => Ok(Value::Int(*n)),
        other => Err(format!("ceil() expects number, got {}", other.type_name())),
    }
}

pub fn builtin_floor(args: &[Value]) -> Result<Value, String> {
    expect_args("floor", args, 1)?;
    match &args[0] {
        Value::Float(n) => {
            #[expect(clippy::cast_possible_truncation)]
            Ok(Value::Int(n.floor() as i64))
        }
        Value::Int(n) => Ok(Value::Int(*n)),
        other => Err(format!("floor() expects number, got {}", other.type_name())),
    }
}

pub fn builtin_round(args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err(format!("round() expects 1-2 args, got {}", args.len()));
    }
    let decimals = if args.len() == 2 {
        match &args[1] {
            Value::Int(n) => u32::try_from(*n).map_err(|_| format!("Invalid decimals: {n}"))?,
            _ => return Err(format!("round() decimals must be int, got {}", args[1].type_name())),
        }
    } else {
        0
    };
    match &args[0] {
        Value::Float(n) => {
            if decimals == 0 {
                #[expect(clippy::cast_possible_truncation)]
                return Ok(Value::Int(n.round() as i64));
            }
            let factor = 10_f64.powi(decimals.cast_signed());
            Ok(Value::Float((n * factor).round() / factor))
        }
        Value::Int(n) => Ok(Value::Int(*n)),
        other => Err(format!("round() expects number, got {}", other.type_name())),
    }
}

pub fn builtin_sqrt(args: &[Value]) -> Result<Value, String> {
    expect_args("sqrt", args, 1)?;
    let n = to_f64("sqrt", &args[0])?;
    if n < 0.0 {
        return Err("sqrt() of negative number".to_string());
    }
    Ok(Value::Float(n.sqrt()))
}

pub fn builtin_pow(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("pow() expects 2 args, got {}", args.len()));
    }
    if let (Value::Int(base), Value::Int(exp)) = (&args[0], &args[1]) {
        u32::try_from(*exp).map_or_else(
            |_| {
                #[expect(clippy::cast_precision_loss)]
                Ok(Value::Float((*base as f64).powf(*exp as f64)))
            },
            |e| Ok(Value::Int(base.pow(e))),
        )
    } else {
        let base = to_f64("pow", &args[0])?;
        let exp = to_f64("pow", &args[1])?;
        Ok(Value::Float(base.powf(exp)))
    }
}

pub fn builtin_log(args: &[Value]) -> Result<Value, String> {
    expect_args("log", args, 1)?;
    let n = to_f64("log", &args[0])?;
    Ok(Value::Float(n.ln()))
}

pub fn builtin_log10(args: &[Value]) -> Result<Value, String> {
    expect_args("log10", args, 1)?;
    let n = to_f64("log10", &args[0])?;
    Ok(Value::Float(n.log10()))
}

pub fn builtin_sin(args: &[Value]) -> Result<Value, String> {
    expect_args("sin", args, 1)?;
    Ok(Value::Float(to_f64("sin", &args[0])?.sin()))
}

pub fn builtin_cos(args: &[Value]) -> Result<Value, String> {
    expect_args("cos", args, 1)?;
    Ok(Value::Float(to_f64("cos", &args[0])?.cos()))
}

pub fn builtin_tan(args: &[Value]) -> Result<Value, String> {
    expect_args("tan", args, 1)?;
    Ok(Value::Float(to_f64("tan", &args[0])?.tan()))
}

pub fn builtin_random(_args: &[Value]) -> Value {
    // Simple LCG random - good enough for shell scripting
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    #[expect(clippy::cast_precision_loss)]
    let val = ((seed ^ (seed >> 16)) & 0xFFFF_FFFF) as f64 / 0xFFFF_FFFF_u64 as f64;
    Value::Float(val)
}

pub fn builtin_random_int(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("random_int() expects 2 args, got {}", args.len()));
    }
    let min = match &args[0] {
        Value::Int(n) => *n,
        other => return Err(format!("random_int() min must be int, got {}", other.type_name())),
    };
    let max = match &args[1] {
        Value::Int(n) => *n,
        other => return Err(format!("random_int() max must be int, got {}", other.type_name())),
    };
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
    Ok(Value::Int(val))
}

pub const fn builtin_pi(_args: &[Value]) -> Value {
    Value::Float(std::f64::consts::PI)
}

pub const fn builtin_infinity(_args: &[Value]) -> Value {
    Value::Float(f64::INFINITY)
}

// --- Helper ---

fn to_f64(name: &str, val: &Value) -> Result<f64, String> {
    match val {
        #[expect(clippy::cast_precision_loss)]
        Value::Int(n) => Ok(*n as f64),
        Value::Float(n) => Ok(*n),
        other => Err(format!("{name}() expects number, got {}", other.type_name())),
    }
}
