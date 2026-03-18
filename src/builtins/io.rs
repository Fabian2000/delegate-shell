use std::io::Write;
use crate::interpreter::value::Value;

pub fn builtin_print(args: &[Value]) -> Value {
    let parts: Vec<String> = args.iter().map(ToString::to_string).collect();
    print!("{}", parts.join(" "));
    let _ = std::io::stdout().flush();
    Value::Void
}

pub fn builtin_println(args: &[Value]) -> Value {
    let parts: Vec<String> = args.iter().map(ToString::to_string).collect();
    println!("{}", parts.join(" "));
    Value::Void
}

pub fn builtin_errprint(args: &[Value]) -> Value {
    let parts: Vec<String> = args.iter().map(ToString::to_string).collect();
    eprint!("{}", parts.join(" "));
    let _ = std::io::stderr().flush();
    Value::Void
}

pub fn builtin_errprintln(args: &[Value]) -> Value {
    let parts: Vec<String> = args.iter().map(ToString::to_string).collect();
    eprintln!("{}", parts.join(" "));
    Value::Void
}
