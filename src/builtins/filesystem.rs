use std::rc::Rc;
use std::fs;
use std::path::{Path, PathBuf};
use indexmap::IndexMap;
use crate::interpreter::value::{Value, new_list};
use super::registry::{BuiltinRegistry, Param, Type};

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn timestamp_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn glob_match(pattern: &str) -> Result<Vec<String>, String> {
    let path = PathBuf::from(pattern);
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_pattern = path.file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();

    if !parent.exists() {
        return Ok(Vec::new());
    }

    let entries = fs::read_dir(parent)
        .map_err(|e| format!("glob('{pattern}'): {e}"))?;

    let mut results = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("glob: {e}"))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if simple_wildcard_match(&file_pattern, &name) {
            let full = entry.path().to_string_lossy().to_string();
            results.push(full);
        }
    }
    results.sort();
    Ok(results)
}

fn simple_wildcard_match(pattern: &str, text: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(suffix) = pattern.strip_prefix('*') {
        return text.ends_with(suffix);
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return text.starts_with(prefix);
    }
    if let Some(star_pos) = pattern.find('*') {
        let prefix = &pattern[..star_pos];
        let suffix = &pattern[star_pos + 1..];
        return text.starts_with(prefix) && text.ends_with(suffix);
    }
    pattern == text
}

fn builtin_cp(args: &[Value]) -> Result<Value, String> {
    let Value::String(src) = &args[0] else { unreachable!() };
    let Value::String(dst) = &args[1] else { unreachable!() };
    let src_path = Path::new(src.as_ref());
    if src_path.is_dir() {
        copy_dir_recursive(src_path, Path::new(dst.as_ref()))
            .map(|()| Value::Void)
            .map_err(|e| format!("cp('{src}', '{dst}'): {e}"))
    } else {
        fs::copy(src.as_ref(), dst.as_ref())
            .map(|_| Value::Void)
            .map_err(|e| format!("cp('{src}', '{dst}'): {e}"))
    }
}

fn builtin_ls(args: &[Value]) -> Result<Value, String> {
    let path: &str = if args.is_empty() {
        "."
    } else {
        let Value::String(s) = &args[0] else { unreachable!() };
        s
    };
    let entries = fs::read_dir(path)
        .map_err(|e| format!("ls('{path}'): {e}"))?;
    let mut items = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("ls('{path}'): {e}"))?;
        let meta = entry.metadata().map_err(|e| format!("ls: {e}"))?;
        let name = entry.file_name().to_string_lossy().to_string();
        let file_type = if meta.is_dir() { "dir" } else if meta.is_symlink() { "symlink" } else { "file" };
        let mut obj = IndexMap::new();
        obj.insert("name".to_string(), Value::String(Rc::from(name)));
        obj.insert("size".to_string(), Value::Int(i64::try_from(meta.len()).unwrap_or(i64::MAX)));
        obj.insert("type".to_string(), Value::String(Rc::from(file_type)));
        items.push(crate::interpreter::value::new_object(obj));
    }
    Ok(new_list(items))
}

fn builtin_glob(args: &[Value]) -> Result<Value, String> {
    let Value::String(pattern) = &args[0] else { unreachable!() };
    let paths = glob_match(pattern)?;
    let items: Vec<Value> = paths.into_iter().map(|p| Value::String(Rc::from(p))).collect();
    Ok(new_list(items))
}

pub fn register(reg: &mut BuiltinRegistry) -> Result<(), String> {
    reg.add("read", &[Param::Required(Type::String)], Type::String, |args| {
        let Value::String(path) = &args[0] else { unreachable!() };
        fs::read_to_string(path.as_ref())
            .map(|s| Value::String(Rc::from(s)))
            .map_err(|e| format!("read('{path}'): {e}"))
    })?;

    reg.add("write", &[Param::Required(Type::String), Param::Required(Type::Dyn)], Type::Void, |args| {
        let Value::String(path) = &args[0] else { unreachable!() };
        let content = args[1].to_string();
        fs::write(path.as_ref(), &content)
            .map(|()| Value::Void)
            .map_err(|e| format!("write('{path}'): {e}"))
    })?;

    reg.add("append", &[Param::Required(Type::String), Param::Required(Type::Dyn)], Type::Void, |args| {
        use std::io::Write;
        let Value::String(path) = &args[0] else { unreachable!() };
        let content = args[1].to_string();
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path.as_ref())
            .map_err(|e| format!("append('{path}'): {e}"))?;
        file.write_all(content.as_bytes())
            .map(|()| Value::Void)
            .map_err(|e| format!("append('{path}'): {e}"))
    })?;

    reg.add("exists", &[Param::Required(Type::String)], Type::Bool, |args| {
        let Value::String(s) = &args[0] else { unreachable!() };
        Ok(Value::Bool(Path::new(s.as_ref()).exists()))
    })?;

    reg.add("is_file", &[Param::Required(Type::String)], Type::Bool, |args| {
        let Value::String(s) = &args[0] else { unreachable!() };
        Ok(Value::Bool(Path::new(s.as_ref()).is_file()))
    })?;

    reg.add("is_dir", &[Param::Required(Type::String)], Type::Bool, |args| {
        let Value::String(s) = &args[0] else { unreachable!() };
        Ok(Value::Bool(Path::new(s.as_ref()).is_dir()))
    })?;

    reg.add("mkdir", &[Param::Required(Type::String)], Type::Void, |args| {
        let Value::String(path) = &args[0] else { unreachable!() };
        fs::create_dir_all(path.as_ref())
            .map(|()| Value::Void)
            .map_err(|e| format!("mkdir('{path}'): {e}"))
    })?;

    reg.add("rm", &[Param::Required(Type::String)], Type::Void, |args| {
        let Value::String(path) = &args[0] else { unreachable!() };
        let p = Path::new(path.as_ref());
        if p.is_dir() {
            fs::remove_dir_all(p)
        } else {
            fs::remove_file(p)
        }
        .map(|()| Value::Void)
        .map_err(|e| format!("rm('{path}'): {e}"))
    })?;

    reg.add("cp", &[Param::Required(Type::String), Param::Required(Type::String)], Type::Void, builtin_cp)?;

    reg.add("mv", &[Param::Required(Type::String), Param::Required(Type::String)], Type::Void, |args| {
        let Value::String(src) = &args[0] else { unreachable!() };
        let Value::String(dst) = &args[1] else { unreachable!() };
        fs::rename(src.as_ref(), dst.as_ref())
            .map(|()| Value::Void)
            .map_err(|e| format!("mv('{src}', '{dst}'): {e}"))
    })?;

    reg.add("ls", &[Param::Optional(Type::String)], Type::List, builtin_ls)?;

    reg.add("cwd", &[], Type::String, |_args| {
        std::env::current_dir()
            .map(|p| Value::String(Rc::from(p.to_string_lossy().to_string())))
            .map_err(|e| format!("cwd(): {e}"))
    })?;

    reg.add("cd", &[Param::Required(Type::String)], Type::Void, |args| {
        let Value::String(path) = &args[0] else { unreachable!() };
        std::env::set_current_dir(path.as_ref())
            .map(|()| Value::Void)
            .map_err(|e| format!("cd('{path}'): {e}"))
    })?;

    reg.add("basename", &[Param::Required(Type::String)], Type::String, |args| {
        let Value::String(s) = &args[0] else { unreachable!() };
        let name = Path::new(s.as_ref())
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        Ok(Value::String(Rc::from(name)))
    })?;

    reg.add("dirname", &[Param::Required(Type::String)], Type::String, |args| {
        let Value::String(s) = &args[0] else { unreachable!() };
        let dir = Path::new(s.as_ref())
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        Ok(Value::String(Rc::from(dir)))
    })?;

    reg.add("ext", &[Param::Required(Type::String)], Type::String, |args| {
        let Value::String(s) = &args[0] else { unreachable!() };
        let extension = Path::new(s.as_ref())
            .extension()
            .map(|e| e.to_string_lossy().to_string())
            .unwrap_or_default();
        Ok(Value::String(Rc::from(extension)))
    })?;

    reg.add("abs", &[Param::Required(Type::String)], Type::String, |args| {
        let Value::String(path) = &args[0] else { unreachable!() };
        let abs = fs::canonicalize(path.as_ref())
            .map_err(|e| format!("abs('{path}'): {e}"))?;
        Ok(Value::String(Rc::from(abs.to_string_lossy().to_string())))
    })?;

    reg.add("glob", &[Param::Required(Type::String)], Type::List, builtin_glob)?;

    reg.add("tempfile", &[], Type::String, |_args| {
        let mut path = std::env::temp_dir();
        path.push(format!("dgsh_{}", std::process::id()));
        let unique = format!("{}_{}", path.to_string_lossy(), timestamp_nanos());
        fs::write(&unique, "")
            .map_err(|e| format!("tempfile(): {e}"))?;
        Ok(Value::String(Rc::from(unique)))
    })?;

    reg.add("tempdir", &[], Type::String, |_args| {
        let mut path = std::env::temp_dir();
        path.push(format!("dgsh_{}_{}", std::process::id(), timestamp_nanos()));
        fs::create_dir_all(&path)
            .map_err(|e| format!("tempdir(): {e}"))?;
        Ok(Value::String(Rc::from(path.to_string_lossy().to_string())))
    })?;

    reg.add("filesize", &[Param::Required(Type::String)], Type::Int, |args| {
        let Value::String(path) = &args[0] else { unreachable!() };
        let meta = fs::metadata(path.as_ref())
            .map_err(|e| format!("filesize('{path}'): {e}"))?;
        Ok(Value::Int(i64::try_from(meta.len()).unwrap_or(i64::MAX)))
    })?;

    Ok(())
}
