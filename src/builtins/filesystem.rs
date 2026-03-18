use std::rc::Rc;
use std::fs;
use std::path::{Path, PathBuf};
use indexmap::IndexMap;
use crate::interpreter::value::{Value, new_list};
use super::expect_args;

pub fn builtin_read(args: &[Value]) -> Result<Value, String> {
    expect_args("read", args, 1)?;
    let path = expect_string("read", &args[0])?;
    fs::read_to_string(path)
        .map(|s| Value::String(Rc::from(s)))
        .map_err(|e| format!("read('{path}'): {e}"))
}

pub fn builtin_write(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("write() expects 2 args, got {}", args.len()));
    }
    let path = expect_string("write", &args[0])?;
    let content = args[1].to_string();
    fs::write(path, &content)
        .map(|()| Value::Void)
        .map_err(|e| format!("write('{path}'): {e}"))
}

pub fn builtin_append(args: &[Value]) -> Result<Value, String> {
    use std::io::Write;
    if args.len() != 2 {
        return Err(format!("append() expects 2 args, got {}", args.len()));
    }
    let path = expect_string("append", &args[0])?;
    let content = args[1].to_string();
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("append('{path}'): {e}"))?;
    file.write_all(content.as_bytes())
        .map(|()| Value::Void)
        .map_err(|e| format!("append('{path}'): {e}"))
}

pub fn builtin_exists(args: &[Value]) -> Result<Value, String> {
    expect_args("exists", args, 1)?;
    let path = expect_string("exists", &args[0])?;
    Ok(Value::Bool(Path::new(path).exists()))
}

pub fn builtin_is_file(args: &[Value]) -> Result<Value, String> {
    expect_args("is_file", args, 1)?;
    let path = expect_string("is_file", &args[0])?;
    Ok(Value::Bool(Path::new(path).is_file()))
}

pub fn builtin_is_dir(args: &[Value]) -> Result<Value, String> {
    expect_args("is_dir", args, 1)?;
    let path = expect_string("is_dir", &args[0])?;
    Ok(Value::Bool(Path::new(path).is_dir()))
}

pub fn builtin_mkdir(args: &[Value]) -> Result<Value, String> {
    expect_args("mkdir", args, 1)?;
    let path = expect_string("mkdir", &args[0])?;
    fs::create_dir_all(path)
        .map(|()| Value::Void)
        .map_err(|e| format!("mkdir('{path}'): {e}"))
}

pub fn builtin_rm(args: &[Value]) -> Result<Value, String> {
    expect_args("rm", args, 1)?;
    let path = expect_string("rm", &args[0])?;
    let p = Path::new(path);
    if p.is_dir() {
        fs::remove_dir_all(p)
    } else {
        fs::remove_file(p)
    }
    .map(|()| Value::Void)
    .map_err(|e| format!("rm('{path}'): {e}"))
}

pub fn builtin_cp(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("cp() expects 2 args, got {}", args.len()));
    }
    let src = expect_string("cp", &args[0])?;
    let dst = expect_string("cp", &args[1])?;
    let src_path = Path::new(src);
    if src_path.is_dir() {
        copy_dir_recursive(src_path, Path::new(dst))
            .map(|()| Value::Void)
            .map_err(|e| format!("cp('{src}', '{dst}'): {e}"))
    } else {
        fs::copy(src, dst)
            .map(|_| Value::Void)
            .map_err(|e| format!("cp('{src}', '{dst}'): {e}"))
    }
}

pub fn builtin_mv(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("mv() expects 2 args, got {}", args.len()));
    }
    let src = expect_string("mv", &args[0])?;
    let dst = expect_string("mv", &args[1])?;
    fs::rename(src, dst)
        .map(|()| Value::Void)
        .map_err(|e| format!("mv('{src}', '{dst}'): {e}"))
}

pub fn builtin_ls(args: &[Value]) -> Result<Value, String> {
    let path = if args.is_empty() {
        "."
    } else {
        expect_string("ls", &args[0])?
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
        #[expect(clippy::cast_possible_wrap)]
        obj.insert("size".to_string(), Value::Int(meta.len() as i64));
        obj.insert("type".to_string(), Value::String(Rc::from(file_type)));
        items.push(crate::interpreter::value::new_object(obj));
    }
    Ok(new_list(items))
}

pub fn builtin_cwd(_args: &[Value]) -> Result<Value, String> {
    std::env::current_dir()
        .map(|p| Value::String(Rc::from(p.to_string_lossy().to_string())))
        .map_err(|e| format!("cwd(): {e}"))
}

pub fn builtin_cd(args: &[Value]) -> Result<Value, String> {
    expect_args("cd", args, 1)?;
    let path = expect_string("cd", &args[0])?;
    std::env::set_current_dir(path)
        .map(|()| Value::Void)
        .map_err(|e| format!("cd('{path}'): {e}"))
}

pub fn builtin_basename(args: &[Value]) -> Result<Value, String> {
    expect_args("basename", args, 1)?;
    let path = expect_string("basename", &args[0])?;
    let name = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    Ok(Value::String(Rc::from(name)))
}

pub fn builtin_dirname(args: &[Value]) -> Result<Value, String> {
    expect_args("dirname", args, 1)?;
    let path = expect_string("dirname", &args[0])?;
    let dir = Path::new(path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    Ok(Value::String(Rc::from(dir)))
}

pub fn builtin_ext(args: &[Value]) -> Result<Value, String> {
    expect_args("ext", args, 1)?;
    let path = expect_string("ext", &args[0])?;
    let extension = Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_default();
    Ok(Value::String(Rc::from(extension)))
}

pub fn builtin_abs(args: &[Value]) -> Result<Value, String> {
    expect_args("abs", args, 1)?;
    let path = expect_string("abs", &args[0])?;
    let abs = fs::canonicalize(path)
        .map_err(|e| format!("abs('{path}'): {e}"))?;
    Ok(Value::String(Rc::from(abs.to_string_lossy().to_string())))
}

pub fn builtin_glob(args: &[Value]) -> Result<Value, String> {
    expect_args("glob", args, 1)?;
    let pattern = expect_string("glob", &args[0])?;
    let mut items = Vec::new();
    // Simple glob implementation using walkdir-like approach
    // For now, support basic patterns with * and **
    let paths = glob_match(pattern)?;
    for p in paths {
        items.push(Value::String(Rc::from(p)));
    }
    Ok(new_list(items))
}

pub fn builtin_tempfile(_args: &[Value]) -> Result<Value, String> {
    let mut path = std::env::temp_dir();
    path.push(format!("dgsh_{}", std::process::id()));
    let unique = format!("{}_{}", path.to_string_lossy(), timestamp_nanos());
    fs::write(&unique, "")
        .map_err(|e| format!("tempfile(): {e}"))?;
    Ok(Value::String(Rc::from(unique)))
}

pub fn builtin_tempdir(_args: &[Value]) -> Result<Value, String> {
    let mut path = std::env::temp_dir();
    path.push(format!("dgsh_{}_{}", std::process::id(), timestamp_nanos()));
    fs::create_dir_all(&path)
        .map_err(|e| format!("tempdir(): {e}"))?;
    Ok(Value::String(Rc::from(path.to_string_lossy().to_string())))
}

pub fn builtin_filesize(args: &[Value]) -> Result<Value, String> {
    expect_args("filesize", args, 1)?;
    let path = expect_string("filesize", &args[0])?;
    let meta = fs::metadata(path)
        .map_err(|e| format!("filesize('{path}'): {e}"))?;
    #[expect(clippy::cast_possible_wrap)]
    Ok(Value::Int(meta.len() as i64))
}


// --- Helpers ---

fn expect_string<'a>(name: &str, val: &'a Value) -> Result<&'a str, String> {
    if let Value::String(s) = val {
        Ok(&**s)
    } else {
        Err(format!("{name}() expects string, got {}", val.type_name()))
    }
}

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
    // Simple glob: split pattern into dir + file pattern
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
