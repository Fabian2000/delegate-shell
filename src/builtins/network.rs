use std::rc::Rc;
use indexmap::IndexMap;
use crate::interpreter::value::{Value, new_object};
use std::io::Read;
use super::registry::{BuiltinRegistry, Param, Type};

fn http_get(args: &[Value]) -> Result<Value, String> {
    let Value::String(url) = &args[0] else { unreachable!() };
    let agent = ureq::Agent::new_with_defaults();
    let mut req = agent.get(&**url);
    if args.len() == 2
        && let Some(hdrs) = extract_headers(&args[1]) {
        for (k, v) in hdrs {
            if let Value::String(val) = v { req = req.header(k, &*val); }
        }
    }
    let resp = req.call()
        .map_err(|e| format!("http_get('{url}'): {e}"))?;
    Ok(body_response(resp))
}

fn http_post(args: &[Value]) -> Result<Value, String> {
    let Value::String(url) = &args[0] else { unreachable!() };
    let Value::String(body) = &args[1] else { unreachable!() };
    let agent = ureq::Agent::new_with_defaults();
    let mut req = agent.post(&**url).header("Content-Type", "application/json");
    if args.len() == 3
        && let Some(hdrs) = extract_headers(&args[2]) {
        for (k, v) in hdrs {
            if let Value::String(val) = v { req = req.header(k, &*val); }
        }
    }
    let resp = req.send(body.as_bytes())
        .map_err(|e| format!("http_post('{url}'): {e}"))?;
    Ok(body_response(resp))
}

fn http_put(args: &[Value]) -> Result<Value, String> {
    let Value::String(url) = &args[0] else { unreachable!() };
    let Value::String(body) = &args[1] else { unreachable!() };
    let agent = ureq::Agent::new_with_defaults();
    let mut req = agent.put(&**url).header("Content-Type", "application/json");
    if args.len() == 3
        && let Some(hdrs) = extract_headers(&args[2]) {
        for (k, v) in hdrs {
            if let Value::String(val) = v { req = req.header(k, &*val); }
        }
    }
    let resp = req.send(body.as_bytes())
        .map_err(|e| format!("http_put('{url}'): {e}"))?;
    Ok(body_response(resp))
}

fn http_delete(args: &[Value]) -> Result<Value, String> {
    let Value::String(url) = &args[0] else { unreachable!() };
    let agent = ureq::Agent::new_with_defaults();
    let mut req = agent.delete(&**url);
    if args.len() == 2
        && let Some(hdrs) = extract_headers(&args[1]) {
        for (k, v) in hdrs {
            if let Value::String(val) = v { req = req.header(k, &*val); }
        }
    }
    let resp = req.call()
        .map_err(|e| format!("http_delete('{url}'): {e}"))?;
    Ok(body_response(resp))
}

fn download(args: &[Value]) -> Result<Value, String> {
    let Value::String(url) = &args[0] else { unreachable!() };
    let Value::String(path) = &args[1] else { unreachable!() };
    let resp = ureq::Agent::new_with_defaults()
        .get(&**url)
        .call()
        .map_err(|e| format!("download('{url}'): {e}"))?;
    let mut bytes = Vec::new();
    resp.into_body().as_reader().read_to_end(&mut bytes)
        .map_err(|e| format!("download('{url}'): read error: {e}"))?;
    std::fs::write(&**path, &bytes)
        .map_err(|e| format!("download() write '{path}': {e}"))?;
    Ok(Value::Void)
}

fn ip(args: &[Value]) -> Result<Value, String> {
    let _ = args;
    let socket = std::net::UdpSocket::bind("0.0.0.0:0")
        .map_err(|e| format!("ip(): {e}"))?;
    socket.connect("8.8.8.8:80")
        .map_err(|e| format!("ip(): {e}"))?;
    let addr = socket.local_addr()
        .map_err(|e| format!("ip(): {e}"))?;
    Ok(Value::String(Rc::from(addr.ip().to_string())))
}

// --- Helpers ---

fn body_response(resp: ureq::http::Response<ureq::Body>) -> Value {
    let status = resp.status().as_u16();
    let body_str = resp.into_body().read_to_string()
        .unwrap_or_default();
    let mut obj = IndexMap::new();
    obj.insert("status".to_string(), Value::Int(i64::from(status)));
    obj.insert("body".to_string(), Value::String(Rc::from(body_str)));
    new_object(obj)
}

fn extract_headers(val: &Value) -> Option<IndexMap<String, Value>> {
    if let Value::Object(rc) = val { Some(rc.borrow().fields.clone()) } else { None }
}

pub fn register(reg: &mut BuiltinRegistry) -> Result<(), String> {
    reg.add("http_get", &[Param::Required(Type::String), Param::Optional(Type::Object)], Type::Object, http_get)?;
    reg.add("http_post", &[Param::Required(Type::String), Param::Required(Type::String), Param::Optional(Type::Object)], Type::Object, http_post)?;
    reg.add("http_put", &[Param::Required(Type::String), Param::Required(Type::String), Param::Optional(Type::Object)], Type::Object, http_put)?;
    reg.add("http_delete", &[Param::Required(Type::String), Param::Optional(Type::Object)], Type::Object, http_delete)?;
    reg.add("download", &[Param::Required(Type::String), Param::Required(Type::String)], Type::Void, download)?;
    reg.add("hostname", &[], Type::String, |_args| {
        let name = hostname::get()
            .map_or_else(|_| "unknown".to_string(), |n| n.to_string_lossy().to_string());
        Ok(Value::String(Rc::from(name)))
    })?;
    reg.add("ip", &[], Type::String, ip)?;

    Ok(())
}
