use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::{Arc, Mutex};
use crate::interpreter::value::{Value, FileHandleInner};
use super::expect_args;

pub fn builtin_open_file(args: &[Value]) -> Result<Value, String> {
    expect_args("open_file", args, 1)?;
    let path = expect_string("open_file", &args[0])?;
    let file = fs::File::open(path)
        .map_err(|e| format!("open_file('{path}'): {e}"))?;
    let reader = BufReader::new(file);
    Ok(Value::FileHandle(Arc::new(Mutex::new(FileHandleInner { reader }))))
}

pub fn builtin_read_line(args: &[Value]) -> Result<Value, String> {
    expect_args("read_line", args, 1)?;
    if let Value::FileHandle(fh) = &args[0] {
        let mut line = String::new();
        let bytes_read = {
            let mut guard = fh.lock().map_err(|e| format!("read_line: {e}"))?;
            guard.reader.read_line(&mut line)
                .map_err(|e| format!("read_line: {e}"))?
        };
        if bytes_read == 0 {
            Ok(Value::Bool(false))
        } else {
            // Trim trailing newline
            if line.ends_with('\n') {
                line.pop();
                if line.ends_with('\r') {
                    line.pop();
                }
            }
            Ok(Value::String(std::rc::Rc::from(line)))
        }
    } else {
        Err(format!("read_line() expects a filehandle, got {}", args[0].type_name()))
    }
}

pub fn builtin_read_chunk(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("read_chunk() expects 2 args, got {}", args.len()));
    }
    if let Value::FileHandle(fh) = &args[0] {
        let size = match &args[1] {
            Value::Int(n) => {
                if *n <= 0 {
                    return Err("read_chunk: size must be positive".to_string());
                }
                usize::try_from(*n).unwrap_or(0)
            }
            _ => return Err(format!("read_chunk() expects int size, got {}", args[1].type_name())),
        };
        let mut buf = vec![0u8; size];
        let bytes_read = {
            let mut guard = fh.lock().map_err(|e| format!("read_chunk: {e}"))?;
            guard.reader.read(&mut buf)
                .map_err(|e| format!("read_chunk: {e}"))?
        };
        buf.truncate(bytes_read);
        Ok(Value::Bytes(buf))
    } else {
        Err(format!("read_chunk() expects a filehandle, got {}", args[0].type_name()))
    }
}

pub fn builtin_close_file(args: &[Value]) -> Result<Value, String> {
    expect_args("close_file", args, 1)?;
    if let Value::FileHandle(_) = &args[0] {
        // Dropping the handle closes the file; we just return void.
        // The Arc will be dropped when no more references exist.
        Ok(Value::Void)
    } else {
        Err(format!("close_file() expects a filehandle, got {}", args[0].type_name()))
    }
}

pub fn builtin_byte_len(args: &[Value]) -> Result<Value, String> {
    expect_args("byte_len", args, 1)?;
    if let Value::Bytes(b) = &args[0] {
        #[expect(clippy::cast_possible_wrap)]
        Ok(Value::Int(b.len() as i64))
    } else {
        Err(format!("byte_len() expects bytes, got {}", args[0].type_name()))
    }
}

pub fn builtin_byte_at(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("byte_at() expects 2 args, got {}", args.len()));
    }
    if let Value::Bytes(b) = &args[0] {
        let idx = match &args[1] {
            Value::Int(n) => *n,
            _ => return Err(format!("byte_at() expects int index, got {}", args[1].type_name())),
        };
        let ui = usize::try_from(idx).map_err(|_| format!("byte_at: index {idx} out of bounds (len {})", b.len()))?;
        if ui >= b.len() {
            return Err(format!("byte_at: index {idx} out of bounds (len {})", b.len()));
        }
        Ok(Value::Int(i64::from(b[ui])))
    } else {
        Err(format!("byte_at() expects bytes, got {}", args[0].type_name()))
    }
}

pub fn builtin_byte_slice(args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err(format!("byte_slice() expects 3 args, got {}", args.len()));
    }
    if let Value::Bytes(b) = &args[0] {
        let start = match &args[1] {
            Value::Int(n) => usize::try_from(*n).unwrap_or(0),
            _ => return Err(format!("byte_slice() expects int start, got {}", args[1].type_name())),
        };
        let end = match &args[2] {
            Value::Int(n) => usize::try_from(*n).unwrap_or(0),
            _ => return Err(format!("byte_slice() expects int end, got {}", args[2].type_name())),
        };
        if start > b.len() || end > b.len() || start > end {
            return Err(format!("byte_slice: range {start}..{end} out of bounds (len {})", b.len()));
        }
        Ok(Value::Bytes(b[start..end].to_vec()))
    } else {
        Err(format!("byte_slice() expects bytes, got {}", args[0].type_name()))
    }
}

pub fn builtin_to_bytes(args: &[Value]) -> Result<Value, String> {
    expect_args("to_bytes", args, 1)?;
    if let Value::String(s) = &args[0] {
        Ok(Value::Bytes(s.as_bytes().to_vec()))
    } else {
        Err(format!("to_bytes() expects string, got {}", args[0].type_name()))
    }
}

pub fn builtin_from_bytes(args: &[Value]) -> Result<Value, String> {
    expect_args("from_bytes", args, 1)?;
    if let Value::Bytes(b) = &args[0] {
        let s = String::from_utf8(b.clone())
            .map_err(|e| format!("from_bytes: invalid UTF-8: {e}"))?;
        Ok(Value::String(std::rc::Rc::from(s)))
    } else {
        Err(format!("from_bytes() expects bytes, got {}", args[0].type_name()))
    }
}

pub fn builtin_read_bytes(args: &[Value]) -> Result<Value, String> {
    expect_args("read_bytes", args, 1)?;
    let path = expect_string("read_bytes", &args[0])?;
    let data = fs::read(path)
        .map_err(|e| format!("read_bytes('{path}'): {e}"))?;
    Ok(Value::Bytes(data))
}

pub fn builtin_write_bytes(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("write_bytes() expects 2 args, got {}", args.len()));
    }
    let path = expect_string("write_bytes", &args[0])?;
    if let Value::Bytes(b) = &args[1] {
        fs::write(path, b)
            .map_err(|e| format!("write_bytes('{path}'): {e}"))?;
        Ok(Value::Void)
    } else {
        Err(format!("write_bytes() expects bytes as second arg, got {}", args[1].type_name()))
    }
}

pub fn builtin_append_bytes(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("append_bytes() expects 2 args, got {}", args.len()));
    }
    let path = expect_string("append_bytes", &args[0])?;
    if let Value::Bytes(b) = &args[1] {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| format!("append_bytes('{path}'): {e}"))?;
        file.write_all(b)
            .map(|()| Value::Void)
            .map_err(|e| format!("append_bytes('{path}'): {e}"))
    } else {
        Err(format!("append_bytes() expects bytes as second arg, got {}", args[1].type_name()))
    }
}

pub fn builtin_bytes_from_list(args: &[Value]) -> Result<Value, String> {
    expect_args("bytes_from_list", args, 1)?;
    if let Value::List(list) = &args[0] {
        let list = list.borrow();
        let mut bytes = Vec::with_capacity(list.len());
        for item in list.iter() {
            if let Value::Int(n) = item {
                let b = u8::try_from(*n).map_err(|_| format!("bytes_from_list(): value {n} out of byte range (0-255)"))?;
                bytes.push(b);
            } else {
                return Err(format!("bytes_from_list(): expected int, got {}", item.type_name()));
            }
        }
        Ok(Value::Bytes(bytes))
    } else {
        Err(format!("bytes_from_list() expects list, got {}", args[0].type_name()))
    }
}

fn expect_string<'a>(name: &str, val: &'a Value) -> Result<&'a str, String> {
    if let Value::String(s) = val {
        Ok(&**s)
    } else {
        Err(format!("{name}() expects a string, got {}", val.type_name()))
    }
}
