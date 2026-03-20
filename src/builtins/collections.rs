use crate::interpreter::value::{Value, new_list, new_object};
use crate::interpreter::Interpreter;
use super::registry::{BuiltinRegistry, Param, Type};

pub fn register(reg: &mut BuiltinRegistry) -> Result<(), String> {
    // Pure functions
    reg.add("len", &[Param::Required(Type::Dyn)], Type::Int, |args| {
        match &args[0] {
            Value::List(l) => {
                let len = i64::try_from(l.borrow().len()).map_err(|_| "List length overflows i64".to_string())?;
                Ok(Value::Int(len))
            }
            Value::String(s) => {
                let len = i64::try_from(s.len()).map_err(|_| "String length overflows i64".to_string())?;
                Ok(Value::Int(len))
            }
            Value::Object(m) => {
                let len = i64::try_from(m.borrow().fields.len()).map_err(|_| "Object length overflows i64".to_string())?;
                Ok(Value::Int(len))
            }
            other => Err(format!("Cannot get length of {}", other.type_name())),
        }
    })?;

    reg.add("push", &[Param::Required(Type::List), Param::Required(Type::Dyn)], Type::Void, |args| {
        let Value::List(list) = &args[0] else { unreachable!() };
        list.borrow_mut().push(args[1].clone());
        Ok(Value::Void)
    })?;

    reg.add("pop", &[Param::Required(Type::List)], Type::Dyn, |args| {
        let Value::List(list) = &args[0] else { unreachable!() };
        let mut list = list.borrow_mut();
        if list.is_empty() {
            return Err("Cannot pop from empty list".to_string());
        }
        Ok(list.pop().unwrap_or(Value::Void))
    })?;

    reg.add("has", &[Param::Required(Type::Object), Param::Required(Type::String)], Type::Bool, |args| {
        let Value::Object(map) = &args[0] else { unreachable!() };
        let Value::String(key) = &args[1] else { unreachable!() };
        Ok(Value::Bool(map.borrow().fields.contains_key(&**key)))
    })?;

    reg.add("sort", &[Param::Required(Type::List)], Type::List, |args| {
        let Value::List(l) = &args[0] else { unreachable!() };
        let mut sorted = l.borrow().clone();
        sorted.sort_by(compare_values);
        Ok(new_list(sorted))
    })?;

    reg.add("index", &[Param::Required(Type::List), Param::Required(Type::Dyn)], Type::Int, |args| {
        let Value::List(l) = &args[0] else { unreachable!() };
        let items = l.borrow().clone();
        let needle = &args[1];
        for (i, item) in items.iter().enumerate() {
            if values_equal(item, needle) {
                return Ok(Value::Int(i64::try_from(i).unwrap_or(i64::MAX)));
            }
        }
        Ok(Value::Int(-1))
    })?;

    reg.add("flat", &[Param::Required(Type::List)], Type::List, |args| {
        let Value::List(l) = &args[0] else { unreachable!() };
        let items = l.borrow().clone();
        let mut result = Vec::new();
        for item in &items {
            match item {
                Value::List(inner) => result.extend(inner.borrow().iter().cloned()),
                other => result.push(other.clone()),
            }
        }
        Ok(new_list(result))
    })?;

    reg.add("unique", &[Param::Required(Type::List)], Type::List, |args| {
        let Value::List(l) = &args[0] else { unreachable!() };
        let items = l.borrow().clone();
        let mut result: Vec<Value> = Vec::new();
        for item in &items {
            if !result.iter().any(|existing| values_equal(existing, item)) {
                result.push(item.clone());
            }
        }
        Ok(new_list(result))
    })?;

    reg.add("zip", &[Param::Required(Type::List), Param::Required(Type::List)], Type::List, |args| {
        let Value::List(la) = &args[0] else { unreachable!() };
        let Value::List(lb) = &args[1] else { unreachable!() };
        let a = la.borrow().clone();
        let b = lb.borrow().clone();
        let result: Vec<Value> = a.iter().zip(b.iter())
            .map(|(x, y)| new_list(vec![x.clone(), y.clone()]))
            .collect();
        Ok(new_list(result))
    })?;

    reg.add("range", &[Param::Required(Type::Int), Param::Required(Type::Int), Param::Optional(Type::Int)], Type::List, builtin_range)?;

    reg.add("slice", &[Param::Required(Type::List), Param::Required(Type::Int), Param::Required(Type::Int)], Type::List, |args| {
        let Value::List(l) = &args[0] else { unreachable!() };
        let Value::Int(start) = &args[1] else { unreachable!() };
        let Value::Int(end) = &args[2] else { unreachable!() };
        let items = l.borrow().clone();
        let start = *start;
        let end = *end;
        let start = usize::try_from(start).map_err(|_| "Invalid start".to_string())?;
        let end = usize::try_from(end).map_err(|_| "Invalid end".to_string())?;
        let end = end.min(items.len());
        let start = start.min(end);
        Ok(new_list(items[start..end].to_vec()))
    })?;

    reg.add("insert", &[Param::Required(Type::List), Param::Required(Type::Int), Param::Required(Type::Dyn)], Type::Void, |args| {
        let Value::List(list) = &args[0] else { unreachable!() };
        let Value::Int(i) = &args[1] else { unreachable!() };
        let idx = usize::try_from(*i).map_err(|_| "Invalid index".to_string())?;
        let mut list = list.borrow_mut();
        if idx > list.len() {
            return Err(format!("insert() index {idx} out of bounds (len {})", list.len()));
        }
        list.insert(idx, args[2].clone());
        Ok(Value::Void)
    })?;

    reg.add("remove", &[Param::Required(Type::List), Param::Required(Type::Int)], Type::Dyn, |args| {
        let Value::List(list) = &args[0] else { unreachable!() };
        let Value::Int(i) = &args[1] else { unreachable!() };
        let idx = usize::try_from(*i).map_err(|_| "Invalid index".to_string())?;
        let mut list = list.borrow_mut();
        if idx >= list.len() {
            return Err(format!("remove() index {idx} out of bounds (len {})", list.len()));
        }
        Ok(list.remove(idx))
    })?;

    reg.add("merge", &[Param::Required(Type::Object), Param::Required(Type::Object)], Type::Object, |args| {
        let Value::Object(a) = &args[0] else { unreachable!() };
        let Value::Object(b) = &args[1] else { unreachable!() };
        let mut merged = a.borrow().fields.clone();
        for (k, v) in b.borrow().fields.iter() {
            merged.insert(k.clone(), v.clone());
        }
        Ok(new_object(merged))
    })?;

    reg.add("sum", &[Param::Required(Type::List)], Type::Number, builtin_sum)?;
    reg.add("min", &[Param::Required(Type::List)], Type::Dyn, builtin_min)?;
    reg.add("max", &[Param::Required(Type::List)], Type::Dyn, builtin_max)?;

    // Interpreter-dependent functions
    reg.add_interp("map", &[Param::Required(Type::List), Param::Required(Type::Lambda)], Type::List, builtin_map)?;
    reg.add_interp("filter", &[Param::Required(Type::List), Param::Required(Type::Lambda)], Type::List, builtin_filter)?;
    reg.add_interp("reduce", &[Param::Required(Type::List), Param::Required(Type::Lambda), Param::Required(Type::Dyn)], Type::Dyn, builtin_reduce)?;
    reg.add_interp("sort_by", &[Param::Required(Type::List), Param::Required(Type::Lambda)], Type::List, builtin_sort_by)?;
    reg.add_interp("find", &[Param::Required(Type::List), Param::Required(Type::Lambda)], Type::Dyn, builtin_find)?;
    reg.add_interp("count", &[Param::Required(Type::List), Param::Required(Type::Lambda)], Type::Int, builtin_count)?;
    reg.add_interp("any", &[Param::Required(Type::List), Param::Required(Type::Lambda)], Type::Bool, builtin_any)?;
    reg.add_interp("all", &[Param::Required(Type::List), Param::Required(Type::Lambda)], Type::Bool, builtin_all)?;

    Ok(())
}

// --- Named pure functions (complex logic) ---

fn builtin_range(args: &[Value]) -> Result<Value, String> {
    let Value::Int(start) = &args[0] else { unreachable!() };
    let Value::Int(end) = &args[1] else { unreachable!() };
    let step = if args.len() > 2 {
        let Value::Int(s) = &args[2] else { unreachable!() };
        *s
    } else {
        1
    };
    let start = *start;
    let end = *end;
    if step == 0 {
        return Err("range() step cannot be 0".to_string());
    }
    let mut items = Vec::new();
    let mut i = start;
    if step > 0 {
        while i <= end {
            items.push(Value::Int(i));
            i += step;
        }
    } else {
        while i >= end {
            items.push(Value::Int(i));
            i += step;
        }
    }
    Ok(new_list(items))
}

fn builtin_sum(args: &[Value]) -> Result<Value, String> {
    let Value::List(l) = &args[0] else { unreachable!() };
    let items = l.borrow().clone();
    let mut int_sum: i64 = 0;
    let mut is_float = false;
    let mut float_sum: f64 = 0.0;
    for item in &items {
        match item {
            Value::Int(n) => {
                if is_float {
                    float_sum += *n as f64;
                } else {
                    int_sum += n;
                }
            }
            Value::Float(n) => {
                if !is_float {
                    float_sum = int_sum as f64;
                    is_float = true;
                }
                float_sum += n;
            }
            other => return Err(format!("sum(): cannot add {}", other.type_name())),
        }
    }
    if is_float { Ok(Value::Float(float_sum)) } else { Ok(Value::Int(int_sum)) }
}

fn builtin_min(args: &[Value]) -> Result<Value, String> {
    let Value::List(l) = &args[0] else { unreachable!() };
    let items = l.borrow().clone();
    if items.is_empty() {
        return Err("min() on empty list".to_string());
    }
    let mut best = &items[0];
    for item in &items[1..] {
        if compare_values(item, best) == std::cmp::Ordering::Less {
            best = item;
        }
    }
    Ok(best.clone())
}

fn builtin_max(args: &[Value]) -> Result<Value, String> {
    let Value::List(l) = &args[0] else { unreachable!() };
    let items = l.borrow().clone();
    if items.is_empty() {
        return Err("max() on empty list".to_string());
    }
    let mut best = &items[0];
    for item in &items[1..] {
        if compare_values(item, best) == std::cmp::Ordering::Greater {
            best = item;
        }
    }
    Ok(best.clone())
}

// --- Named interpreter-dependent functions ---

fn builtin_map(args: &[Value], interp: &mut Interpreter) -> Result<Value, String> {
    let Value::List(l) = &args[0] else { unreachable!() };
    let items = l.borrow().clone();
    let lambda = &args[1];
    let mut result = Vec::with_capacity(items.len());
    for item in &items {
        result.push(interp.call_lambda(lambda, vec![item.clone()])?);
    }
    Ok(new_list(result))
}

fn builtin_filter(args: &[Value], interp: &mut Interpreter) -> Result<Value, String> {
    let Value::List(l) = &args[0] else { unreachable!() };
    let items = l.borrow().clone();
    let lambda = &args[1];
    let mut result = Vec::new();
    for item in &items {
        let val = interp.call_lambda(lambda, vec![item.clone()])?;
        if val.is_truthy() {
            result.push(item.clone());
        }
    }
    Ok(new_list(result))
}

fn builtin_reduce(args: &[Value], interp: &mut Interpreter) -> Result<Value, String> {
    let Value::List(l) = &args[0] else { unreachable!() };
    let items = l.borrow().clone();
    let lambda = &args[1];
    let mut acc = args[2].clone();
    for item in &items {
        acc = interp.call_lambda(lambda, vec![acc, item.clone()])?;
    }
    Ok(acc)
}

fn builtin_sort_by(args: &[Value], interp: &mut Interpreter) -> Result<Value, String> {
    let Value::List(l) = &args[0] else { unreachable!() };
    let items = l.borrow().clone();
    let lambda = &args[1];
    let mut keyed: Vec<(Value, Value)> = Vec::with_capacity(items.len());
    for item in &items {
        let key = interp.call_lambda(lambda, vec![item.clone()])?;
        keyed.push((item.clone(), key));
    }
    keyed.sort_by(|a, b| compare_values(&a.1, &b.1));
    let sorted: Vec<Value> = keyed.into_iter().map(|(item, _)| item).collect();
    Ok(new_list(sorted))
}

fn builtin_find(args: &[Value], interp: &mut Interpreter) -> Result<Value, String> {
    let Value::List(l) = &args[0] else { unreachable!() };
    let items = l.borrow().clone();
    let lambda = &args[1];
    for item in &items {
        let val = interp.call_lambda(lambda, vec![item.clone()])?;
        if val.is_truthy() {
            return Ok(item.clone());
        }
    }
    Err("find(): no element matches".to_string())
}

fn builtin_count(args: &[Value], interp: &mut Interpreter) -> Result<Value, String> {
    let Value::List(l) = &args[0] else { unreachable!() };
    let items = l.borrow().clone();
    let lambda = &args[1];
    let mut n: i64 = 0;
    for item in &items {
        let val = interp.call_lambda(lambda, vec![item.clone()])?;
        if val.is_truthy() {
            n += 1;
        }
    }
    Ok(Value::Int(n))
}

fn builtin_any(args: &[Value], interp: &mut Interpreter) -> Result<Value, String> {
    let Value::List(l) = &args[0] else { unreachable!() };
    let items = l.borrow().clone();
    let lambda = &args[1];
    for item in &items {
        let val = interp.call_lambda(lambda, vec![item.clone()])?;
        if val.is_truthy() {
            return Ok(Value::Bool(true));
        }
    }
    Ok(Value::Bool(false))
}

fn builtin_all(args: &[Value], interp: &mut Interpreter) -> Result<Value, String> {
    let Value::List(l) = &args[0] else { unreachable!() };
    let items = l.borrow().clone();
    let lambda = &args[1];
    for item in &items {
        let val = interp.call_lambda(lambda, vec![item.clone()])?;
        if !val.is_truthy() {
            return Ok(Value::Bool(false));
        }
    }
    Ok(Value::Bool(true))
}

// --- Helpers ---

fn compare_values(a: &Value, b: &Value) -> std::cmp::Ordering {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x.cmp(y),
        (Value::Float(x), Value::Float(y)) => x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal),
        (Value::String(x), Value::String(y)) => x.cmp(y),
        _ => std::cmp::Ordering::Equal,
    }
}

fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => (x - y).abs() < f64::EPSILON,
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        _ => false,
    }
}
