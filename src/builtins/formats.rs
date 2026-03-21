use std::rc::Rc;
use indexmap::IndexMap;
use crate::interpreter::value::{Value, ValueKind as VK, new_list, new_object};
use super::registry::{BuiltinRegistry, Param, Type};

pub fn register(reg: &mut BuiltinRegistry) -> Result<(), String> {
    reg.add("from_json", &[Param::Required(Type::String)], Type::Dyn, builtin_from_json)?;
    reg.add("to_json", &[Param::Required(Type::Dyn)], Type::String, builtin_to_json)?;
    reg.add("from_toml", &[Param::Required(Type::String)], Type::Dyn, builtin_from_toml)?;
    reg.add("to_toml", &[Param::Required(Type::Dyn)], Type::String, builtin_to_toml)?;
    reg.add("from_yaml", &[Param::Required(Type::String)], Type::Dyn, builtin_from_yaml)?;
    reg.add("to_yaml", &[Param::Required(Type::Dyn)], Type::String, builtin_to_yaml)?;
    reg.add("from_csv", &[Param::Required(Type::String)], Type::List, builtin_from_csv)?;
    reg.add("to_csv", &[Param::Required(Type::List)], Type::String, builtin_to_csv)?;
    reg.add("to_base64", &[Param::Required(Type::String)], Type::String, builtin_to_base64)?;
    reg.add("from_base64", &[Param::Required(Type::String)], Type::String, builtin_from_base64)?;
    reg.add("to_hex", &[Param::Required(Type::String)], Type::String, builtin_to_hex)?;
    reg.add("from_hex", &[Param::Required(Type::String)], Type::String, builtin_from_hex)?;
    reg.add("url_encode", &[Param::Required(Type::String)], Type::String, builtin_url_encode)?;
    reg.add("url_decode", &[Param::Required(Type::String)], Type::String, builtin_url_decode)?;

    Ok(())
}

// === JSON ===

fn builtin_from_json(args: &[Value]) -> Result<Value, String> {
    let Some(s) = args[0].as_str_ref() else {
        return Err(format!("expected string, got {}", args[0].type_name()));
    };
    let json: serde_json::Value = serde_json::from_str(s)
        .map_err(|e| format!("from_json(): {e}"))?;
    Ok(json_to_value(&json))
}

fn builtin_to_json(args: &[Value]) -> Result<Value, String> {
    let json = value_to_json(&args[0])?;
    let s = serde_json::to_string_pretty(&json)
        .map_err(|e| format!("to_json(): {e}"))?;
    Ok(Value::string(Rc::from(s)))
}

fn json_to_value(json: &serde_json::Value) -> Value {
    match json {
        serde_json::Value::Null => Value::void(),
        serde_json::Value::Bool(b) => Value::bool(*b),
        serde_json::Value::Number(n) => {
            n.as_i64().map_or_else(
                || Value::float(n.as_f64().unwrap_or(0.0)),
                Value::int,
            )
        }
        serde_json::Value::String(s) => Value::string_from(s.as_str()),
        serde_json::Value::Array(arr) => {
            let items: Vec<Value> = arr.iter().map(json_to_value).collect();
            new_list(items)
        }
        serde_json::Value::Object(obj) => {
            let mut map = IndexMap::new();
            for (k, v) in obj {
                map.insert(k.clone(), json_to_value(v));
            }
            new_object(map)
        }
    }
}

fn value_to_json(val: &Value) -> Result<serde_json::Value, String> {
    match val.kind() {
        VK::Int(n) => Ok(serde_json::Value::Number(n.into())),
        VK::Float(n) => serde_json::Number::from_f64(n)
            .map(serde_json::Value::Number)
            .ok_or_else(|| "Cannot convert float to JSON".to_string()),
        VK::String(s) => Ok(serde_json::Value::String(s.to_string())),
        VK::Bool(b) => Ok(serde_json::Value::Bool(b)),
        VK::Void => Ok(serde_json::Value::Null),
        VK::List(items) => {
            let arr: Result<Vec<_>, _> = items.borrow().iter().map(value_to_json).collect();
            Ok(serde_json::Value::Array(arr?))
        }
        VK::Object(rc) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in rc.borrow().fields.iter() {
                obj.insert(k.clone(), value_to_json(v)?);
            }
            Ok(serde_json::Value::Object(obj))
        }
        _ => Err(format!("Cannot convert {} to JSON", val.type_name())),
    }
}

// === TOML ===

fn builtin_from_toml(args: &[Value]) -> Result<Value, String> {
    let Some(s) = args[0].as_str_ref() else {
        return Err(format!("expected string, got {}", args[0].type_name()));
    };
    let toml_val: toml::Value = s.parse()
        .map_err(|e| format!("from_toml(): {e}"))?;
    Ok(toml_to_value(&toml_val))
}

fn builtin_to_toml(args: &[Value]) -> Result<Value, String> {
    let toml_val = value_to_toml(&args[0])?;
    let s = toml::to_string_pretty(&toml_val)
        .map_err(|e| format!("to_toml(): {e}"))?;
    Ok(Value::string(Rc::from(s)))
}

fn toml_to_value(t: &toml::Value) -> Value {
    match t {
        toml::Value::String(s) => Value::string_from(s.as_str()),
        toml::Value::Integer(n) => Value::int(*n),
        toml::Value::Float(n) => Value::float(*n),
        toml::Value::Boolean(b) => Value::bool(*b),
        toml::Value::Datetime(dt) => Value::string_from(&dt.to_string()),
        toml::Value::Array(arr) => {
            let items: Vec<Value> = arr.iter().map(toml_to_value).collect();
            new_list(items)
        }
        toml::Value::Table(table) => {
            let mut map = IndexMap::new();
            for (k, v) in table {
                map.insert(k.clone(), toml_to_value(v));
            }
            new_object(map)
        }
    }
}

fn value_to_toml(val: &Value) -> Result<toml::Value, String> {
    match val.kind() {
        VK::Int(n) => Ok(toml::Value::Integer(n)),
        VK::Float(n) => Ok(toml::Value::Float(n)),
        VK::String(s) => Ok(toml::Value::String(s.to_string())),
        VK::Bool(b) => Ok(toml::Value::Boolean(b)),
        VK::List(items) => {
            let arr: Result<Vec<_>, _> = items.borrow().iter().map(value_to_toml).collect();
            Ok(toml::Value::Array(arr?))
        }
        VK::Object(rc) => {
            let mut table = toml::map::Map::new();
            for (k, v) in rc.borrow().fields.iter() {
                table.insert(k.clone(), value_to_toml(v)?);
            }
            Ok(toml::Value::Table(table))
        }
        _ => Err(format!("Cannot convert {} to TOML", val.type_name())),
    }
}

// === YAML ===

fn builtin_from_yaml(args: &[Value]) -> Result<Value, String> {
    let Some(s) = args[0].as_str_ref() else {
        return Err(format!("expected string, got {}", args[0].type_name()));
    };
    let yaml: serde_yaml::Value = serde_yaml::from_str(s)
        .map_err(|e| format!("from_yaml(): {e}"))?;
    Ok(yaml_to_value(&yaml))
}

fn builtin_to_yaml(args: &[Value]) -> Result<Value, String> {
    let yaml = value_to_yaml(&args[0])?;
    let s = serde_yaml::to_string(&yaml)
        .map_err(|e| format!("to_yaml(): {e}"))?;
    Ok(Value::string(Rc::from(s)))
}

fn yaml_to_value(y: &serde_yaml::Value) -> Value {
    match y {
        serde_yaml::Value::Null => Value::void(),
        serde_yaml::Value::Bool(b) => Value::bool(*b),
        serde_yaml::Value::Number(n) => {
            n.as_i64().map_or_else(
                || Value::float(n.as_f64().unwrap_or(0.0)),
                Value::int,
            )
        }
        serde_yaml::Value::String(s) => Value::string_from(s.as_str()),
        serde_yaml::Value::Sequence(seq) => {
            let items: Vec<Value> = seq.iter().map(yaml_to_value).collect();
            new_list(items)
        }
        serde_yaml::Value::Mapping(map) => {
            let mut obj = IndexMap::new();
            for (k, v) in map {
                let key = match k {
                    serde_yaml::Value::String(s) => s.clone(),
                    other => format!("{other:?}"),
                };
                obj.insert(key, yaml_to_value(v));
            }
            new_object(obj)
        }
        serde_yaml::Value::Tagged(tagged) => yaml_to_value(&tagged.value),
    }
}

fn value_to_yaml(val: &Value) -> Result<serde_yaml::Value, String> {
    match val.kind() {
        VK::Int(n) => Ok(serde_yaml::Value::Number(n.into())),
        VK::Float(n) => Ok(serde_yaml::Value::Number(serde_yaml::Number::from(n))),
        VK::String(s) => Ok(serde_yaml::Value::String(s.to_string())),
        VK::Bool(b) => Ok(serde_yaml::Value::Bool(b)),
        VK::Void => Ok(serde_yaml::Value::Null),
        VK::List(items) => {
            let seq: Result<Vec<_>, _> = items.borrow().iter().map(value_to_yaml).collect();
            Ok(serde_yaml::Value::Sequence(seq?))
        }
        VK::Object(rc) => {
            let mut mapping = serde_yaml::Mapping::new();
            for (k, v) in rc.borrow().fields.iter() {
                mapping.insert(serde_yaml::Value::String(k.clone()), value_to_yaml(v)?);
            }
            Ok(serde_yaml::Value::Mapping(mapping))
        }
        _ => Err(format!("Cannot convert {} to YAML", val.type_name())),
    }
}

// === CSV ===

fn builtin_from_csv(args: &[Value]) -> Result<Value, String> {
    let Some(s) = args[0].as_str_ref() else {
        return Err(format!("expected string, got {}", args[0].type_name()));
    };
    let mut lines = s.lines();
    let header: Vec<&str> = match lines.next() {
        Some(h) => h.split(',').map(str::trim).collect(),
        None => return Ok(new_list(Vec::new())),
    };
    let mut rows = Vec::new();
    for line in lines {
        if line.trim().is_empty() { continue; }
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        let mut obj = IndexMap::new();
        for (i, key) in header.iter().enumerate() {
            let val = fields.get(i).copied().unwrap_or("");
            obj.insert((*key).to_string(), Value::string_from(val));
        }
        rows.push(new_object(obj));
    }
    Ok(new_list(rows))
}

fn builtin_to_csv(args: &[Value]) -> Result<Value, String> {
    let Some(items_ref) = args[0].as_list_ref() else {
        return Err(format!("expected list, got {}", args[0].type_name()));
    };
    let items = items_ref.borrow();
    if items.is_empty() {
        return Ok(Value::string_from(""));
    }
    // Get headers from first object
    let headers: Vec<String> = if let Some(rc) = items[0].as_object_ref() {
        rc.borrow().fields.keys().cloned().collect()
    } else {
        return Err("to_csv() expects list of objects".to_string());
    };
    let mut result = headers.join(",");
    result.push('\n');
    for item in items.iter() {
        if let Some(rc) = item.as_object_ref() {
            let map = rc.borrow();
            let row: Vec<String> = headers.iter()
                .map(|h| map.fields.get(h).map(ToString::to_string).unwrap_or_default())
                .collect();
            result.push_str(&row.join(","));
            result.push('\n');
        }
    }
    Ok(Value::string(Rc::from(result)))
}

// === Base64 ===

fn builtin_to_base64(args: &[Value]) -> Result<Value, String> {
    let Some(s) = args[0].as_str_ref() else {
        return Err(format!("expected string, got {}", args[0].type_name()));
    };
    Ok(Value::string_from(&base64_encode(s.as_bytes())))
}

fn builtin_from_base64(args: &[Value]) -> Result<Value, String> {
    let Some(s) = args[0].as_str_ref() else {
        return Err(format!("expected string, got {}", args[0].type_name()));
    };
    let bytes = base64_decode(s)?;
    String::from_utf8(bytes)
        .map(|s| Value::string_from(&s))
        .map_err(|e| format!("from_base64(): invalid UTF-8: {e}"))
}

const B64_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(data: &[u8]) -> String {
    let mut result = String::new();
    for chunk in data.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = if chunk.len() > 1 { u32::from(chunk[1]) } else { 0 };
        let b2 = if chunk.len() > 2 { u32::from(chunk[2]) } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        result.push(B64_CHARS[((triple >> 18) & 0x3F) as usize] as char);
        result.push(B64_CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 { result.push(B64_CHARS[((triple >> 6) & 0x3F) as usize] as char); } else { result.push('='); }
        if chunk.len() > 2 { result.push(B64_CHARS[(triple & 0x3F) as usize] as char); } else { result.push('='); }
    }
    result
}

fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim_end_matches('=');
    let mut result = Vec::new();
    let chars: Vec<u8> = s.bytes().map(|b| match b {
        b'A'..=b'Z' => b - b'A',
        b'a'..=b'z' => b - b'a' + 26,
        b'0'..=b'9' => b - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => 255,
    }).collect();
    for chunk in chars.chunks(4) {
        if chunk.contains(&255) {
            return Err("from_base64(): invalid character".to_string());
        }
        let c0 = u32::from(chunk[0]);
        let c1 = if chunk.len() > 1 { u32::from(chunk[1]) } else { 0 };
        let c2 = if chunk.len() > 2 { u32::from(chunk[2]) } else { 0 };
        let c3 = if chunk.len() > 3 { u32::from(chunk[3]) } else { 0 };
        let triple = (c0 << 18) | (c1 << 12) | (c2 << 6) | c3;
        result.push(((triple >> 16) & 0xFF) as u8);
        if chunk.len() > 2 { result.push(((triple >> 8) & 0xFF) as u8); }
        if chunk.len() > 3 { result.push((triple & 0xFF) as u8); }
    }
    Ok(result)
}

// === Hex ===

fn builtin_to_hex(args: &[Value]) -> Result<Value, String> {
    use std::fmt::Write;
    let Some(s) = args[0].as_str_ref() else {
        return Err(format!("expected string, got {}", args[0].type_name()));
    };
    let mut hex = String::with_capacity(s.len() * 2);
    for b in s.bytes() { let _ = write!(hex, "{b:02x}"); }
    Ok(Value::string_from(&hex))
}

fn builtin_from_hex(args: &[Value]) -> Result<Value, String> {
    let Some(s) = args[0].as_str_ref() else {
        return Err(format!("expected string, got {}", args[0].type_name()));
    };
    let mut bytes = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    for pair in chars.chunks(2) {
        if pair.len() != 2 {
            return Err("from_hex(): odd number of characters".to_string());
        }
        let byte = u8::from_str_radix(&format!("{}{}", pair[0], pair[1]), 16)
            .map_err(|_| format!("from_hex(): invalid hex '{}{}'", pair[0], pair[1]))?;
        bytes.push(byte);
    }
    String::from_utf8(bytes)
        .map(|s| Value::string_from(&s))
        .map_err(|e| format!("from_hex(): invalid UTF-8: {e}"))
}

// === URL Encoding ===

fn builtin_url_encode(args: &[Value]) -> Result<Value, String> {
    let Some(s) = args[0].as_str_ref() else {
        return Err(format!("expected string, got {}", args[0].type_name()));
    };
    let mut result = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(b as char);
            }
            _ => {
                {
                    use std::fmt::Write;
                    let _ = write!(result, "%{b:02X}");
                }
            }
        }
    }
    Ok(Value::string_from(&result))
}

fn builtin_url_decode(args: &[Value]) -> Result<Value, String> {
    let Some(s) = args[0].as_str_ref() else {
        return Err(format!("expected string, got {}", args[0].type_name()));
    };
    let mut result = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = &s[i + 1..i + 3];
            let byte = u8::from_str_radix(hex, 16)
                .map_err(|_| format!("url_decode(): invalid escape '%{hex}'"))?;
            result.push(byte);
            i += 3;
        } else if bytes[i] == b'+' {
            result.push(b' ');
            i += 1;
        } else {
            result.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(result)
        .map(|s| Value::string_from(&s))
        .map_err(|e| format!("url_decode(): invalid UTF-8: {e}"))
}
