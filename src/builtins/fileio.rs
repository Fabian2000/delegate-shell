use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::{Arc, Mutex};
use crate::interpreter::value::{Value, FileHandleInner};
use super::registry::{BuiltinRegistry, Param, Type};

fn builtin_read_chunk(args: &[Value]) -> Result<Value, String> {
    let Value::FileHandle(fh) = &args[0] else { unreachable!() };
    let Value::Int(n) = &args[1] else { unreachable!() };
    if *n <= 0 {
        return Err("read_chunk: size must be positive".to_string());
    }
    let size = usize::try_from(*n).unwrap_or(0);
    let mut buf = vec![0u8; size];
    let bytes_read = {
        let mut guard = fh.lock().map_err(|e| format!("read_chunk: {e}"))?;
        guard.reader.read(&mut buf)
            .map_err(|e| format!("read_chunk: {e}"))?
    };
    if bytes_read == 0 {
        return Err("read_chunk: EOF".to_string());
    }
    buf.truncate(bytes_read);
    Ok(Value::Bytes(buf))
}

fn builtin_byte_at(args: &[Value]) -> Result<Value, String> {
    let Value::Bytes(b) = &args[0] else { unreachable!() };
    let Value::Int(idx) = &args[1] else { unreachable!() };
    let ui = usize::try_from(*idx)
        .map_err(|_| format!("byte_at: index {idx} out of bounds (len {})", b.len()))?;
    if ui >= b.len() {
        return Err(format!("byte_at: index {idx} out of bounds (len {})", b.len()));
    }
    Ok(Value::Int(i64::from(b[ui])))
}

fn builtin_byte_slice(args: &[Value]) -> Result<Value, String> {
    let Value::Bytes(b) = &args[0] else { unreachable!() };
    let Value::Int(s) = &args[1] else { unreachable!() };
    let Value::Int(e) = &args[2] else { unreachable!() };
    let start = usize::try_from(*s).unwrap_or(0);
    let end = usize::try_from(*e).unwrap_or(0);
    if start > b.len() || end > b.len() || start > end {
        return Err(format!("byte_slice: range {start}..{end} out of bounds (len {})", b.len()));
    }
    Ok(Value::Bytes(b[start..end].to_vec()))
}

fn builtin_bytes_from_list(args: &[Value]) -> Result<Value, String> {
    let Value::List(list) = &args[0] else { unreachable!() };
    let list = list.borrow();
    let mut bytes = Vec::with_capacity(list.len());
    for item in list.iter() {
        if let Value::Int(n) = item {
            let b = u8::try_from(*n)
                .map_err(|_| format!("bytes_from_list(): value {n} out of byte range (0-255)"))?;
            bytes.push(b);
        } else {
            return Err(format!("bytes_from_list(): expected int, got {}", item.type_name()));
        }
    }
    Ok(Value::Bytes(bytes))
}

pub fn register(reg: &mut BuiltinRegistry) -> Result<(), String> {
    reg.add("open_file", &[Param::Required(Type::String)], Type::FileHandle, |args| {
        let Value::String(path) = &args[0] else { unreachable!() };
        let file = fs::File::open(&**path)
            .map_err(|e| format!("open_file('{path}'): {e}"))?;
        let reader = BufReader::new(file);
        Ok(Value::FileHandle(Arc::new(Mutex::new(FileHandleInner { reader }))))
    })?;

    reg.add("read_line", &[Param::Required(Type::FileHandle)], Type::String, |args| {
        let Value::FileHandle(fh) = &args[0] else { unreachable!() };
        let mut line = String::new();
        let bytes_read = {
            let mut guard = fh.lock().map_err(|e| format!("read_line: {e}"))?;
            guard.reader.read_line(&mut line)
                .map_err(|e| format!("read_line: {e}"))?
        };
        if bytes_read == 0 {
            Err("read_line: EOF".to_string())
        } else {
            if line.ends_with('\n') {
                line.pop();
                if line.ends_with('\r') {
                    line.pop();
                }
            }
            Ok(Value::String(std::rc::Rc::from(line)))
        }
    })?;

    reg.add("read_chunk", &[Param::Required(Type::FileHandle), Param::Required(Type::Int)], Type::Bytes, builtin_read_chunk)?;

    reg.add("close_file", &[Param::Required(Type::FileHandle)], Type::Void, |args| {
        let Value::FileHandle(_) = &args[0] else { unreachable!() };
        Ok(Value::Void)
    })?;

    reg.add("byte_len", &[Param::Required(Type::Bytes)], Type::Int, |args| {
        let Value::Bytes(b) = &args[0] else { unreachable!() };
        Ok(Value::Int(i64::try_from(b.len()).unwrap_or(i64::MAX)))
    })?;

    reg.add("byte_at", &[Param::Required(Type::Bytes), Param::Required(Type::Int)], Type::Int, builtin_byte_at)?;

    reg.add("byte_slice", &[Param::Required(Type::Bytes), Param::Required(Type::Int), Param::Required(Type::Int)], Type::Bytes, builtin_byte_slice)?;

    reg.add("to_bytes", &[Param::Required(Type::String)], Type::Bytes, |args| {
        let Value::String(s) = &args[0] else { unreachable!() };
        Ok(Value::Bytes(s.as_bytes().to_vec()))
    })?;

    reg.add("from_bytes", &[Param::Required(Type::Bytes)], Type::String, |args| {
        let Value::Bytes(b) = &args[0] else { unreachable!() };
        let s = String::from_utf8(b.clone())
            .map_err(|e| format!("from_bytes: invalid UTF-8: {e}"))?;
        Ok(Value::String(std::rc::Rc::from(s)))
    })?;

    reg.add("read_bytes", &[Param::Required(Type::String)], Type::Bytes, |args| {
        let Value::String(path) = &args[0] else { unreachable!() };
        let data = fs::read(&**path)
            .map_err(|e| format!("read_bytes('{path}'): {e}"))?;
        Ok(Value::Bytes(data))
    })?;

    reg.add("write_bytes", &[Param::Required(Type::String), Param::Required(Type::Bytes)], Type::Void, |args| {
        let Value::String(path) = &args[0] else { unreachable!() };
        let Value::Bytes(b) = &args[1] else { unreachable!() };
        fs::write(&**path, b)
            .map_err(|e| format!("write_bytes('{path}'): {e}"))?;
        Ok(Value::Void)
    })?;

    reg.add("append_bytes", &[Param::Required(Type::String), Param::Required(Type::Bytes)], Type::Void, |args| {
        let Value::String(path) = &args[0] else { unreachable!() };
        let Value::Bytes(b) = &args[1] else { unreachable!() };
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&**path)
            .map_err(|e| format!("append_bytes('{path}'): {e}"))?;
        file.write_all(b)
            .map(|()| Value::Void)
            .map_err(|e| format!("append_bytes('{path}'): {e}"))
    })?;

    reg.add("bytes_from_list", &[Param::Required(Type::List)], Type::Bytes, builtin_bytes_from_list)?;

    Ok(())
}
