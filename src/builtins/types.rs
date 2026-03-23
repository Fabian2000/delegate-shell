use crate::interpreter::value::{Value, ValueKind as VK, new_list, new_object_with_dyn, AtomicValue};
use super::registry::{BuiltinRegistry, Param, Type};

fn deep_clone(val: &Value) -> Result<Value, String> {
    if let Some(list_ref) = val.as_list_ref() {
        let list = list_ref.borrow();
        let cloned: Vec<Value> = list.iter().map(|item| deep_clone(item)).collect::<Result<_, _>>()?;
        return Ok(new_list(cloned));
    }
    if let Some(obj_ref) = val.as_object_ref() {
        let obj = obj_ref.borrow();
        let mut cloned_fields = indexmap::IndexMap::new();
        for (k, v) in obj.fields.iter() {
            cloned_fields.insert(k.clone(), deep_clone(v)?);
        }
        return Ok(new_object_with_dyn(cloned_fields, obj.dyn_fields.clone()));
    }
    if let Some(atomic) = val.as_atomic() {
        let current = atomic.load();
        return Ok(Value::atomic(AtomicValue::new(&current)));
    }
    // Primitives (int, float, bool, string, void) — clone is already a value copy
    Ok(val.clone())
}

/// Parse a path like "field", "nested.inner", "items[0]", "users[2].name"
/// into segments: Field("field"), Index(0), Field("inner"), etc.
enum PathSeg {
    Field(String),
    Index(usize),
}

fn parse_path(path: &str) -> Result<Vec<PathSeg>, String> {
    let mut segments = Vec::new();
    for part in path.split('.') {
        if part.is_empty() {
            return Err("retype: empty path segment".to_string());
        }
        if let Some(bracket) = part.find('[') {
            let field = &part[..bracket];
            if !field.is_empty() {
                segments.push(PathSeg::Field(field.to_string()));
            }
            let rest = &part[bracket..];
            let mut i = 0;
            while i < rest.len() {
                if rest.as_bytes()[i] == b'[' {
                    let end = rest[i..].find(']').ok_or("retype: unclosed bracket".to_string())?;
                    let idx_str = &rest[i + 1..i + end];
                    let idx: usize = idx_str.parse().map_err(|_| format!("retype: invalid index '{idx_str}'"))?;
                    segments.push(PathSeg::Index(idx));
                    i += end + 1;
                } else {
                    return Err(format!("retype: unexpected char in path at '{}'", &rest[i..]));
                }
            }
        } else {
            segments.push(PathSeg::Field(part.to_string()));
        }
    }
    if segments.is_empty() {
        return Err("retype: empty path".to_string());
    }
    Ok(segments)
}

fn retype_at_path(val: &Value, segments: &[PathSeg], new_default: &Value) -> Result<Value, String> {
    if segments.is_empty() {
        return Ok(new_default.clone());
    }

    match &segments[0] {
        PathSeg::Field(name) => {
            let obj_ref = val.as_object_ref().ok_or_else(|| format!("retype: expected object, got {}", val.type_name()))?;
            let obj = obj_ref.borrow();
            let field_val = obj.fields.get(name.as_str())
                .ok_or_else(|| format!("retype: field '{}' not found", name))?;
            let new_val = retype_at_path(field_val, &segments[1..], new_default)?;
            // Build a new object with the updated field
            let mut new_fields = obj.fields.clone();
            new_fields[name.as_str()] = new_val;
            drop(obj);
            Ok(Value::new_object_with_dyn(new_fields, val.as_object_ref().unwrap().borrow().dyn_fields.clone()))
        }
        PathSeg::Index(idx) => {
            let list_ref = val.as_list_ref().ok_or_else(|| format!("retype: expected list, got {}", val.type_name()))?;
            let list = list_ref.borrow();
            let item = list.get(*idx)
                .ok_or_else(|| format!("retype: index {} out of bounds (len {})", idx, list.len()))?;
            let new_val = retype_at_path(item, &segments[1..], new_default)?;
            // Build a new list with the updated element
            let mut new_list = list.clone();
            drop(list);
            new_list[*idx] = new_val;
            Ok(Value::new_list(new_list))
        }
    }
}

pub fn register(reg: &mut BuiltinRegistry) -> Result<(), String> {
    reg.add("str", &[Param::Required(Type::Dyn)], Type::String, |args| {
        Ok(Value::string_from(&args[0].to_string()))
    })?;

    reg.add("int", &[Param::Required(Type::Dyn)], Type::Int, |args| {
        match args[0].kind() {
            VK::Int(n) => Ok(Value::int(n)),
            VK::Float(n) => Ok(Value::int(n as i64)),
            VK::String(s) => s.parse::<i64>().map(Value::int).map_err(|_| format!("Cannot convert '{s}' to int")),
            VK::Bool(b) => Ok(Value::int(i64::from(b))),
            _ => Err(format!("Cannot convert {} to int", args[0].type_name())),
        }
    })?;

    reg.add("float", &[Param::Required(Type::Dyn)], Type::Float, |args| {
        match args[0].kind() {
            VK::Float(n) => Ok(Value::float(n)),
            VK::Int(n) => Ok(Value::float(n as f64)),
            VK::String(s) => s.parse::<f64>().map(Value::float).map_err(|_| format!("Cannot convert '{s}' to float")),
            _ => Err(format!("Cannot convert {} to float", args[0].type_name())),
        }
    })?;

    reg.add("bool", &[Param::Required(Type::Dyn)], Type::Bool, |args| {
        Ok(Value::bool(args[0].is_truthy()))
    })?;

    reg.add("type", &[Param::Required(Type::Dyn)], Type::String, |args| {
        Ok(Value::string_from(args[0].type_name()))
    })?;

    reg.add("retype", &[Param::Required(Type::Object), Param::Required(Type::String), Param::Required(Type::Dyn)], Type::Object, |args| {
        let path_str = args[1].as_str_ref().ok_or_else(|| "retype: path must be a string".to_string())?;
        let segments = parse_path(path_str)?;
        retype_at_path(&args[0], &segments, &args[2])
    })?;

    reg.add("clone", &[Param::Required(Type::Dyn)], Type::Dyn, |args| {
        deep_clone(&args[0])
    })?;

    Ok(())
}
