use indexmap::IndexMap;
use crate::interpreter::value::{Value, new_object};
use crate::interpreter::Runtime;
use std::io::Read;
use super::registry::{BuiltinRegistry, Param, Type};

fn check_network(interp: &Runtime, fn_name: &str) -> Result<(), String> {
    if !interp.allow_network() {
        return Err(format!("{fn_name}() is disabled when network access is not allowed"));
    }
    Ok(())
}

fn http_get(args: &[Value], interp: &mut Runtime) -> Result<Value, String> {
    check_network(interp, "http_get")?;
    let Some(url) = args[0].as_str_ref() else {
        return Err(format!("expected string, got {}", args[0].type_name()));
    };
    let agent = ureq::Agent::new_with_defaults();
    let mut req = agent.get(url);
    if args.len() == 2
        && let Some(hdrs) = extract_headers(&args[1]) {
        for (k, v) in hdrs {
            if let Some(val) = v.as_str_ref() { req = req.header(k, val); }
        }
    }
    let resp = req.call()
        .map_err(|e| format!("http_get('{url}'): {e}"))?;
    Ok(body_response(resp))
}

fn http_post(args: &[Value], interp: &mut Runtime) -> Result<Value, String> {
    check_network(interp, "http_post")?;
    let Some(url) = args[0].as_str_ref() else {
        return Err(format!("expected string, got {}", args[0].type_name()));
    };
    let Some(body) = args[1].as_str_ref() else {
        return Err(format!("expected string, got {}", args[1].type_name()));
    };
    let agent = ureq::Agent::new_with_defaults();
    let mut req = agent.post(url).header("Content-Type", "application/json");
    if args.len() == 3
        && let Some(hdrs) = extract_headers(&args[2]) {
        for (k, v) in hdrs {
            if let Some(val) = v.as_str_ref() { req = req.header(k, val); }
        }
    }
    let resp = req.send(body.as_bytes())
        .map_err(|e| format!("http_post('{url}'): {e}"))?;
    Ok(body_response(resp))
}

fn http_put(args: &[Value], interp: &mut Runtime) -> Result<Value, String> {
    check_network(interp, "http_put")?;
    let Some(url) = args[0].as_str_ref() else {
        return Err(format!("expected string, got {}", args[0].type_name()));
    };
    let Some(body) = args[1].as_str_ref() else {
        return Err(format!("expected string, got {}", args[1].type_name()));
    };
    let agent = ureq::Agent::new_with_defaults();
    let mut req = agent.put(url).header("Content-Type", "application/json");
    if args.len() == 3
        && let Some(hdrs) = extract_headers(&args[2]) {
        for (k, v) in hdrs {
            if let Some(val) = v.as_str_ref() { req = req.header(k, val); }
        }
    }
    let resp = req.send(body.as_bytes())
        .map_err(|e| format!("http_put('{url}'): {e}"))?;
    Ok(body_response(resp))
}

fn http_delete(args: &[Value], interp: &mut Runtime) -> Result<Value, String> {
    check_network(interp, "http_delete")?;
    let Some(url) = args[0].as_str_ref() else {
        return Err(format!("expected string, got {}", args[0].type_name()));
    };
    let agent = ureq::Agent::new_with_defaults();
    let mut req = agent.delete(url);
    if args.len() == 2
        && let Some(hdrs) = extract_headers(&args[1]) {
        for (k, v) in hdrs {
            if let Some(val) = v.as_str_ref() { req = req.header(k, val); }
        }
    }
    let resp = req.call()
        .map_err(|e| format!("http_delete('{url}'): {e}"))?;
    Ok(body_response(resp))
}

fn download(args: &[Value], interp: &mut Runtime) -> Result<Value, String> {
    check_network(interp, "download")?;
    let Some(url) = args[0].as_str_ref() else {
        return Err(format!("expected string, got {}", args[0].type_name()));
    };
    let Some(path) = args[1].as_str_ref() else {
        return Err(format!("expected string, got {}", args[1].type_name()));
    };
    let resp = ureq::Agent::new_with_defaults()
        .get(url)
        .call()
        .map_err(|e| format!("download('{url}'): {e}"))?;
    let mut bytes = Vec::new();
    resp.into_body().as_reader().read_to_end(&mut bytes)
        .map_err(|e| format!("download('{url}'): read error: {e}"))?;
    std::fs::write(path, &bytes)
        .map_err(|e| format!("download() write '{path}': {e}"))?;
    Ok(Value::void())
}

fn ip(args: &[Value], interp: &mut Runtime) -> Result<Value, String> {
    check_network(interp, "ip")?;
    let _ = args;
    let socket = std::net::UdpSocket::bind("0.0.0.0:0")
        .map_err(|e| format!("ip(): {e}"))?;
    socket.connect("8.8.8.8:80")
        .map_err(|e| format!("ip(): {e}"))?;
    let addr = socket.local_addr()
        .map_err(|e| format!("ip(): {e}"))?;
    Ok(Value::string_from(&addr.ip().to_string()))
}

// --- Helpers ---

fn body_response(resp: ureq::http::Response<ureq::Body>) -> Value {
    let status = resp.status().as_u16();
    let body_str = resp.into_body().read_to_string()
        .unwrap_or_default();
    let mut obj = IndexMap::new();
    obj.insert("status".to_string(), Value::int(i64::from(status)));
    obj.insert("body".to_string(), Value::string_from(&body_str));
    new_object(obj)
}

fn extract_headers(val: &Value) -> Option<IndexMap<String, Value>> {
    val.as_object_ref().map(|rc| rc.borrow().fields.clone())
}

pub fn register(reg: &mut BuiltinRegistry) -> Result<(), String> {
    reg.add_interp("http_get", &[Param::Required(Type::String), Param::Optional(Type::Object)], Type::Object, http_get)?;
    reg.add_interp("http_post", &[Param::Required(Type::String), Param::Required(Type::String), Param::Optional(Type::Object)], Type::Object, http_post)?;
    reg.add_interp("http_put", &[Param::Required(Type::String), Param::Required(Type::String), Param::Optional(Type::Object)], Type::Object, http_put)?;
    reg.add_interp("http_delete", &[Param::Required(Type::String), Param::Optional(Type::Object)], Type::Object, http_delete)?;
    reg.add_interp("download", &[Param::Required(Type::String), Param::Required(Type::String)], Type::Void, download)?;
    reg.add("hostname", &[], Type::String, |_args| {
        let name = hostname::get()
            .map_or_else(|_| "unknown".to_string(), |n| n.to_string_lossy().to_string());
        Ok(Value::string_from(&name))
    })?;
    reg.add_interp("ip", &[], Type::String, ip)?;

    Ok(())
}
