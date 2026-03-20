use crate::interpreter::value::Value;
use std::io::Write;
use super::registry::{BuiltinRegistry, Param, Type};

// ANSI escape codes
const ESC: &str = "\x1b[";
const RESET: &str = "\x1b[0m";

fn color_code(name: &str, bg: bool) -> Result<&'static str, String> {
    match name {
        "black" => Ok(if bg { "40" } else { "30" }),
        "red" => Ok(if bg { "41" } else { "31" }),
        "green" => Ok(if bg { "42" } else { "32" }),
        "yellow" => Ok(if bg { "43" } else { "33" }),
        "blue" => Ok(if bg { "44" } else { "34" }),
        "magenta" => Ok(if bg { "45" } else { "35" }),
        "cyan" => Ok(if bg { "46" } else { "36" }),
        "white" => Ok(if bg { "47" } else { "37" }),
        "bright_black" | "gray" | "grey" => Ok(if bg { "100" } else { "90" }),
        "bright_red" => Ok(if bg { "101" } else { "91" }),
        "bright_green" => Ok(if bg { "102" } else { "92" }),
        "bright_yellow" => Ok(if bg { "103" } else { "93" }),
        "bright_blue" => Ok(if bg { "104" } else { "94" }),
        "bright_magenta" => Ok(if bg { "105" } else { "95" }),
        "bright_cyan" => Ok(if bg { "106" } else { "96" }),
        "bright_white" => Ok(if bg { "107" } else { "97" }),
        _ => Err(format!("Unknown color: '{name}'. Available: black, red, green, yellow, blue, magenta, cyan, white, gray/grey, bright_*")),
    }
}

pub fn register(reg: &mut BuiltinRegistry) -> Result<(), String> {
    reg.add("set_color", &[Param::Required(Type::String)], Type::Void, builtin_set_color)?;
    reg.add("set_bg", &[Param::Required(Type::String)], Type::Void, builtin_set_bg)?;
    reg.add("set_bold", &[Param::Required(Type::Bool)], Type::Void, builtin_set_bold)?;
    reg.add("set_dim", &[Param::Required(Type::Bool)], Type::Void, builtin_set_dim)?;
    reg.add("set_underline", &[Param::Required(Type::Bool)], Type::Void, builtin_set_underline)?;

    reg.add("reset_style", &[], Type::Void, |_args| {
        print!("{RESET}");
        let _ = std::io::stdout().flush();
        Ok(Value::Void)
    })?;

    reg.add("clear", &[], Type::Void, |_args| {
        print!("{ESC}2J{ESC}H");
        let _ = std::io::stdout().flush();
        Ok(Value::Void)
    })?;

    reg.add("cursor_pos", &[Param::Required(Type::Int), Param::Required(Type::Int)], Type::Void, builtin_cursor_pos)?;
    reg.add("term_size", &[], Type::Object, builtin_term_size)?;

    Ok(())
}

fn builtin_set_color(args: &[Value]) -> Result<Value, String> {
    let Value::String(name) = &args[0] else { unreachable!() };
    let code = color_code(name, false)?;
    print!("{ESC}{code}m");
    let _ = std::io::stdout().flush();
    Ok(Value::Void)
}

fn builtin_set_bg(args: &[Value]) -> Result<Value, String> {
    let Value::String(name) = &args[0] else { unreachable!() };
    let code = color_code(name, true)?;
    print!("{ESC}{code}m");
    let _ = std::io::stdout().flush();
    Ok(Value::Void)
}

fn builtin_set_bold(args: &[Value]) -> Result<Value, String> {
    if args[0].is_truthy() {
        print!("{ESC}1m");
    } else {
        print!("{ESC}22m");
    }
    let _ = std::io::stdout().flush();
    Ok(Value::Void)
}

fn builtin_set_dim(args: &[Value]) -> Result<Value, String> {
    if args[0].is_truthy() {
        print!("{ESC}2m");
    } else {
        print!("{ESC}22m");
    }
    let _ = std::io::stdout().flush();
    Ok(Value::Void)
}

fn builtin_set_underline(args: &[Value]) -> Result<Value, String> {
    if args[0].is_truthy() {
        print!("{ESC}4m");
    } else {
        print!("{ESC}24m");
    }
    let _ = std::io::stdout().flush();
    Ok(Value::Void)
}

fn builtin_cursor_pos(args: &[Value]) -> Result<Value, String> {
    let Value::Int(row) = &args[0] else { unreachable!() };
    let Value::Int(col) = &args[1] else { unreachable!() };
    print!("{ESC}{row};{col}H");
    let _ = std::io::stdout().flush();
    Ok(Value::Void)
}

fn builtin_term_size(_args: &[Value]) -> Result<Value, String> {
    // Try ioctl on Unix
    #[cfg(unix)]
    {
        use std::mem::MaybeUninit;
        #[repr(C)]
        struct TermWinsize {
            rows: u16,
            cols: u16,
            xpixel: u16,
            ypixel: u16,
        }
        let mut ws = MaybeUninit::<TermWinsize>::uninit();
        let ret = unsafe { libc_ioctl(1, 0x5413, ws.as_mut_ptr()) };
        if ret == 0 {
            let ws = unsafe { ws.assume_init() };
            let mut map = indexmap::IndexMap::new();
            map.insert("width".to_string(), Value::Int(i64::from(ws.cols)));
            map.insert("height".to_string(), Value::Int(i64::from(ws.rows)));
            return Ok(crate::interpreter::value::new_object(map));
        }
    }
    // Fallback
    let mut map = indexmap::IndexMap::new();
    map.insert("width".to_string(), Value::Int(80));
    map.insert("height".to_string(), Value::Int(24));
    Ok(crate::interpreter::value::new_object(map))
}

#[cfg(unix)]
unsafe extern "C" {
    #[link_name = "ioctl"]
    fn libc_ioctl(fd: i32, request: u64, ...) -> i32;
}
