use std::rc::Rc;
use indexmap::IndexMap;
use crate::interpreter::value::{Value, new_object};
use std::io::Read;

pub fn builtin_http_get(args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err(format!("http_get() expects 1-2 args, got {}", args.len()));
    }
    let url = expect_str("http_get", &args[0])?;
    let agent = ureq::Agent::new_with_defaults();
    let mut req = agent.get(url);
    if let Some(hdrs) = args.get(1).and_then(extract_headers) {
        for (k, v) in hdrs {
            if let Value::String(val) = v { req = req.header(k, &*val); }
        }
    }
    let resp = req.call()
        .map_err(|e| format!("http_get('{url}'): {e}"))?;
    Ok(body_response(resp))
}

pub fn builtin_http_post(args: &[Value]) -> Result<Value, String> {
    if args.len() < 2 || args.len() > 3 {
        return Err(format!("http_post() expects 2-3 args, got {}", args.len()));
    }
    let url = expect_str("http_post", &args[0])?;
    let body = expect_str("http_post", &args[1])?;
    let agent = ureq::Agent::new_with_defaults();
    let mut req = agent.post(url).header("Content-Type", "application/json");
    if let Some(hdrs) = args.get(2).and_then(extract_headers) {
        for (k, v) in hdrs {
            if let Value::String(val) = v { req = req.header(k, &*val); }
        }
    }
    let resp = req.send(body.as_bytes())
        .map_err(|e| format!("http_post('{url}'): {e}"))?;
    Ok(body_response(resp))
}

pub fn builtin_http_put(args: &[Value]) -> Result<Value, String> {
    if args.len() < 2 || args.len() > 3 {
        return Err(format!("http_put() expects 2-3 args, got {}", args.len()));
    }
    let url = expect_str("http_put", &args[0])?;
    let body = expect_str("http_put", &args[1])?;
    let agent = ureq::Agent::new_with_defaults();
    let mut req = agent.put(url).header("Content-Type", "application/json");
    if let Some(hdrs) = args.get(2).and_then(extract_headers) {
        for (k, v) in hdrs {
            if let Value::String(val) = v { req = req.header(k, &*val); }
        }
    }
    let resp = req.send(body.as_bytes())
        .map_err(|e| format!("http_put('{url}'): {e}"))?;
    Ok(body_response(resp))
}

pub fn builtin_http_delete(args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err(format!("http_delete() expects 1-2 args, got {}", args.len()));
    }
    let url = expect_str("http_delete", &args[0])?;
    let agent = ureq::Agent::new_with_defaults();
    let mut req = agent.delete(url);
    if let Some(hdrs) = args.get(1).and_then(extract_headers) {
        for (k, v) in hdrs {
            if let Value::String(val) = v { req = req.header(k, &*val); }
        }
    }
    let resp = req.call()
        .map_err(|e| format!("http_delete('{url}'): {e}"))?;
    Ok(body_response(resp))
}

pub fn builtin_download(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("download() expects 2 args, got {}", args.len()));
    }
    let url = expect_str("download", &args[0])?;
    let path = expect_str("download", &args[1])?;
    let resp = ureq::Agent::new_with_defaults()
        .get(url)
        .call()
        .map_err(|e| format!("download('{url}'): {e}"))?;
    let mut bytes = Vec::new();
    resp.into_body().as_reader().read_to_end(&mut bytes)
        .map_err(|e| format!("download('{url}'): read error: {e}"))?;
    std::fs::write(path, &bytes)
        .map_err(|e| format!("download() write '{path}': {e}"))?;
    Ok(Value::Void)
}

pub fn builtin_hostname(_args: &[Value]) -> Value {
    let name = hostname::get()
        .map_or_else(|_| "unknown".to_string(), |n| n.to_string_lossy().to_string());
    Value::String(Rc::from(name))
}

pub fn builtin_ip(_args: &[Value]) -> Result<Value, String> {
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
    #[expect(clippy::cast_lossless)]
    obj.insert("status".to_string(), Value::Int(status as i64));
    obj.insert("body".to_string(), Value::String(Rc::from(body_str)));
    new_object(obj)
}

fn extract_headers(val: &Value) -> Option<IndexMap<String, Value>> {
    if let Value::Object(rc) = val { Some(rc.borrow().clone()) } else { None }
}

fn expect_str<'a>(name: &str, val: &'a Value) -> Result<&'a str, String> {
    if let Value::String(s) = val { Ok(&**s) }
    else { Err(format!("{name}() expects string, got {}", val.type_name())) }
}
