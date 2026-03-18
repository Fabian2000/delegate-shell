use crate::interpreter::value::{Value, new_list, new_object};
use crate::interpreter::Interpreter;
use super::expect_args;

pub fn builtin_len(args: &[Value]) -> Result<Value, String> {
    expect_args("len", args, 1)?;
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
            let len = i64::try_from(m.borrow().len()).map_err(|_| "Object length overflows i64".to_string())?;
            Ok(Value::Int(len))
        }
        other => Err(format!("Cannot get length of {}", other.type_name())),
    }
}

pub fn builtin_push(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("push() expects 2 args, got {}", args.len()));
    }
    if let Value::List(list) = &args[0] {
        list.borrow_mut().push(args[1].clone());
        Ok(Value::Void)
    } else {
        Err(format!("Cannot push to {}", args[0].type_name()))
    }
}

pub fn builtin_pop(args: &[Value]) -> Result<Value, String> {
    expect_args("pop", args, 1)?;
    if let Value::List(list) = &args[0] {
        let mut list = list.borrow_mut();
        if list.is_empty() {
            return Err("Cannot pop from empty list".to_string());
        }
        let val = list.pop().unwrap_or(Value::Void);
        Ok(val)
    } else {
        Err(format!("Cannot pop from {}", args[0].type_name()))
    }
}

pub fn builtin_has(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("has() expects 2 args, got {}", args.len()));
    }
    if let (Value::Object(map), Value::String(key)) = (&args[0], &args[1]) {
        Ok(Value::Bool(map.borrow().contains_key(&**key)))
    } else {
        Err("has() expects (object, string)".to_string())
    }
}

// --- Functions that need the interpreter for lambda callbacks ---

pub fn builtin_map(args: &[Value], interp: &mut Interpreter) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("map() expects 2 args, got {}", args.len()));
    }
    let items = expect_list("map", &args[0])?;
    let lambda = &args[1];
    let mut result = Vec::with_capacity(items.len());
    for item in &items {
        result.push(interp.call_lambda(lambda, vec![item.clone()])?);
    }
    Ok(new_list(result))
}

pub fn builtin_filter(args: &[Value], interp: &mut Interpreter) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("filter() expects 2 args, got {}", args.len()));
    }
    let items = expect_list("filter", &args[0])?;
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

pub fn builtin_reduce(args: &[Value], interp: &mut Interpreter) -> Result<Value, String> {
    if args.len() != 3 {
        return Err(format!("reduce() expects 3 args, got {}", args.len()));
    }
    let items = expect_list("reduce", &args[0])?;
    let lambda = &args[1];
    let mut acc = args[2].clone();
    for item in &items {
        acc = interp.call_lambda(lambda, vec![acc, item.clone()])?;
    }
    Ok(acc)
}

pub fn builtin_sort(args: &[Value]) -> Result<Value, String> {
    expect_args("sort", args, 1)?;
    let items = expect_list("sort", &args[0])?;
    let mut sorted = items;
    sorted.sort_by(compare_values);
    Ok(new_list(sorted))
}

pub fn builtin_sort_by(args: &[Value], interp: &mut Interpreter) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("sort_by() expects 2 args, got {}", args.len()));
    }
    let items = expect_list("sort_by", &args[0])?;
    let lambda = &args[1];
    // Pre-compute keys
    let mut keyed: Vec<(Value, Value)> = Vec::with_capacity(items.len());
    for item in &items {
        let key = interp.call_lambda(lambda, vec![item.clone()])?;
        keyed.push((item.clone(), key));
    }
    keyed.sort_by(|a, b| compare_values(&a.1, &b.1));
    let sorted: Vec<Value> = keyed.into_iter().map(|(item, _)| item).collect();
    Ok(new_list(sorted))
}

pub fn builtin_find(args: &[Value], interp: &mut Interpreter) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("find() expects 2 args, got {}", args.len()));
    }
    let items = expect_list("find", &args[0])?;
    let lambda = &args[1];
    for item in &items {
        let val = interp.call_lambda(lambda, vec![item.clone()])?;
        if val.is_truthy() {
            return Ok(item.clone());
        }
    }
    Err("find(): no element matches".to_string())
}

pub fn builtin_index(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("index() expects 2 args, got {}", args.len()));
    }
    let items = expect_list("index", &args[0])?;
    let needle = &args[1];
    for (i, item) in items.iter().enumerate() {
        if values_equal(item, needle) {
            #[expect(clippy::cast_possible_wrap)]
            return Ok(Value::Int(i as i64));
        }
    }
    Ok(Value::Int(-1))
}

pub fn builtin_flat(args: &[Value]) -> Result<Value, String> {
    expect_args("flat", args, 1)?;
    let items = expect_list("flat", &args[0])?;
    let mut result = Vec::new();
    for item in &items {
        match item {
            Value::List(inner) => result.extend(inner.borrow().iter().cloned()),
            other => result.push(other.clone()),
        }
    }
    Ok(new_list(result))
}

pub fn builtin_unique(args: &[Value]) -> Result<Value, String> {
    expect_args("unique", args, 1)?;
    let items = expect_list("unique", &args[0])?;
    let mut result: Vec<Value> = Vec::new();
    for item in &items {
        if !result.iter().any(|existing| values_equal(existing, item)) {
            result.push(item.clone());
        }
    }
    Ok(new_list(result))
}

pub fn builtin_zip(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("zip() expects 2 args, got {}", args.len()));
    }
    let a = expect_list("zip", &args[0])?;
    let b = expect_list("zip", &args[1])?;
    let result: Vec<Value> = a.iter().zip(b.iter())
        .map(|(x, y)| new_list(vec![x.clone(), y.clone()]))
        .collect();
    Ok(new_list(result))
}

pub fn builtin_range(args: &[Value]) -> Result<Value, String> {
    if args.len() < 2 || args.len() > 3 {
        return Err(format!("range() expects 2-3 args, got {}", args.len()));
    }
    let start = expect_int("range", &args[0])?;
    let end = expect_int("range", &args[1])?;
    let step = if args.len() == 3 { expect_int("range", &args[2])? } else { 1 };
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

pub fn builtin_slice(args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err(format!("slice() expects 3 args, got {}", args.len()));
    }
    let items = expect_list("slice", &args[0])?;
    let start = usize::try_from(expect_int("slice", &args[1])?).map_err(|_| "Invalid start".to_string())?;
    let end = usize::try_from(expect_int("slice", &args[2])?).map_err(|_| "Invalid end".to_string())?;
    let end = end.min(items.len());
    let start = start.min(end);
    Ok(new_list(items[start..end].to_vec()))
}

pub fn builtin_insert(args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err(format!("insert() expects 3 args, got {}", args.len()));
    }
    if let Value::List(list) = &args[0] {
        let idx = usize::try_from(expect_int("insert", &args[1])?).map_err(|_| "Invalid index".to_string())?;
        let mut list = list.borrow_mut();
        if idx > list.len() {
            return Err(format!("insert() index {idx} out of bounds (len {})", list.len()));
        }
        list.insert(idx, args[2].clone());
        Ok(Value::Void)
    } else {
        Err(format!("Cannot insert into {}", args[0].type_name()))
    }
}

pub fn builtin_remove(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("remove() expects 2 args, got {}", args.len()));
    }
    if let Value::List(list) = &args[0] {
        let idx = usize::try_from(expect_int("remove", &args[1])?).map_err(|_| "Invalid index".to_string())?;
        let mut list = list.borrow_mut();
        if idx >= list.len() {
            return Err(format!("remove() index {idx} out of bounds (len {})", list.len()));
        }
        Ok(list.remove(idx))
    } else {
        Err(format!("Cannot remove from {}", args[0].type_name()))
    }
}

pub fn builtin_merge(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("merge() expects 2 args, got {}", args.len()));
    }
    if let (Value::Object(a), Value::Object(b)) = (&args[0], &args[1]) {
        let mut merged = a.borrow().clone();
        for (k, v) in b.borrow().iter() {
            merged.insert(k.clone(), v.clone());
        }
        Ok(new_object(merged))
    } else {
        Err("merge() expects (object, object)".to_string())
    }
}

pub fn builtin_count(args: &[Value], interp: &mut Interpreter) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("count() expects 2 args, got {}", args.len()));
    }
    let items = expect_list("count", &args[0])?;
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

pub fn builtin_any(args: &[Value], interp: &mut Interpreter) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("any() expects 2 args, got {}", args.len()));
    }
    let items = expect_list("any", &args[0])?;
    let lambda = &args[1];
    for item in &items {
        let val = interp.call_lambda(lambda, vec![item.clone()])?;
        if val.is_truthy() {
            return Ok(Value::Bool(true));
        }
    }
    Ok(Value::Bool(false))
}

pub fn builtin_all(args: &[Value], interp: &mut Interpreter) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("all() expects 2 args, got {}", args.len()));
    }
    let items = expect_list("all", &args[0])?;
    let lambda = &args[1];
    for item in &items {
        let val = interp.call_lambda(lambda, vec![item.clone()])?;
        if !val.is_truthy() {
            return Ok(Value::Bool(false));
        }
    }
    Ok(Value::Bool(true))
}

pub fn builtin_sum(args: &[Value]) -> Result<Value, String> {
    expect_args("sum", args, 1)?;
    let items = expect_list("sum", &args[0])?;
    let mut int_sum: i64 = 0;
    let mut is_float = false;
    let mut float_sum: f64 = 0.0;
    for item in &items {
        match item {
            Value::Int(n) => {
                if is_float {
                    #[expect(clippy::cast_precision_loss)]
                    { float_sum += *n as f64; }
                } else {
                    int_sum += n;
                }
            }
            Value::Float(n) => {
                if !is_float {
                    #[expect(clippy::cast_precision_loss)]
                    { float_sum = int_sum as f64; }
                    is_float = true;
                }
                float_sum += n;
            }
            other => return Err(format!("sum(): cannot add {}", other.type_name())),
        }
    }
    if is_float { Ok(Value::Float(float_sum)) } else { Ok(Value::Int(int_sum)) }
}

pub fn builtin_min(args: &[Value]) -> Result<Value, String> {
    expect_args("min", args, 1)?;
    let items = expect_list("min", &args[0])?;
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

pub fn builtin_max(args: &[Value]) -> Result<Value, String> {
    expect_args("max", args, 1)?;
    let items = expect_list("max", &args[0])?;
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

// --- Helpers ---

fn expect_list(name: &str, val: &Value) -> Result<Vec<Value>, String> {
    if let Value::List(list) = val {
        Ok(list.borrow().clone())
    } else {
        Err(format!("{name}() expects list, got {}", val.type_name()))
    }
}

fn expect_int(name: &str, val: &Value) -> Result<i64, String> {
    if let Value::Int(n) = val {
        Ok(*n)
    } else {
        Err(format!("{name}() expects int, got {}", val.type_name()))
    }
}

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
