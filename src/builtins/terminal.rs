use crate::interpreter::value::Value;
use super::expect_args;
use std::io::Write;

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

pub fn builtin_set_color(args: &[Value]) -> Result<Value, String> {
    expect_args("set_color", args, 1)?;
    if let Value::String(name) = &args[0] {
        let code = color_code(name, false)?;
        print!("{ESC}{code}m");
        let _ = std::io::stdout().flush();
        Ok(Value::Void)
    } else {
        Err(format!("set_color() expects string, got {}", args[0].type_name()))
    }
}

pub fn builtin_set_bg(args: &[Value]) -> Result<Value, String> {
    expect_args("set_bg", args, 1)?;
    if let Value::String(name) = &args[0] {
        let code = color_code(name, true)?;
        print!("{ESC}{code}m");
        let _ = std::io::stdout().flush();
        Ok(Value::Void)
    } else {
        Err(format!("set_bg() expects string, got {}", args[0].type_name()))
    }
}

pub fn builtin_set_bold(args: &[Value]) -> Result<Value, String> {
    expect_args("set_bold", args, 1)?;
    if args[0].is_truthy() {
        print!("{ESC}1m");
    } else {
        print!("{ESC}22m");
    }
    let _ = std::io::stdout().flush();
    Ok(Value::Void)
}

pub fn builtin_set_dim(args: &[Value]) -> Result<Value, String> {
    expect_args("set_dim", args, 1)?;
    if args[0].is_truthy() {
        print!("{ESC}2m");
    } else {
        print!("{ESC}22m");
    }
    let _ = std::io::stdout().flush();
    Ok(Value::Void)
}

pub fn builtin_set_underline(args: &[Value]) -> Result<Value, String> {
    expect_args("set_underline", args, 1)?;
    if args[0].is_truthy() {
        print!("{ESC}4m");
    } else {
        print!("{ESC}24m");
    }
    let _ = std::io::stdout().flush();
    Ok(Value::Void)
}

pub fn builtin_reset_style(_args: &[Value]) -> Value {
    print!("{RESET}");
    let _ = std::io::stdout().flush();
    Value::Void
}

pub fn builtin_clear(_args: &[Value]) -> Value {
    print!("{ESC}2J{ESC}H");
    let _ = std::io::stdout().flush();
    Value::Void
}

pub fn builtin_cursor_pos(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("cursor_pos() expects 2 args, got {}", args.len()));
    }
    if let (Value::Int(row), Value::Int(col)) = (&args[0], &args[1]) {
        print!("{ESC}{row};{col}H");
        let _ = std::io::stdout().flush();
        Ok(Value::Void)
    } else {
        Err("cursor_pos() expects (int, int)".to_string())
    }
}

pub fn builtin_term_size(_args: &[Value]) -> Value {
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
            return crate::interpreter::value::new_object(map);
        }
    }
    // Fallback
    let mut map = indexmap::IndexMap::new();
    map.insert("width".to_string(), Value::Int(80));
    map.insert("height".to_string(), Value::Int(24));
    crate::interpreter::value::new_object(map)
}

#[cfg(unix)]
unsafe extern "C" {
    #[link_name = "ioctl"]
    fn libc_ioctl(fd: i32, request: u64, ...) -> i32;
}
