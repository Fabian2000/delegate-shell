use crate::interpreter::value::Value;
use crate::interpreter::Interpreter;
use super::{io, types, collections, strings, system, filesystem, math, datetime, formats, network, hashing, threads, fileio, terminal, process};

pub fn call_builtin(name: &str, args: &[Value], interp: &mut Interpreter) -> Option<Result<Value, String>> {
    call_builtin_pure(name, args)
        .or_else(|| call_builtin_interp(name, args, interp))
}

fn call_builtin_pure(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    match name {
        "print" => Some(Ok(io::builtin_print(args))),
        "println" => Some(Ok(io::builtin_println(args))),
        "errprint" => Some(Ok(io::builtin_errprint(args))),
        "errprintln" => Some(Ok(io::builtin_errprintln(args))),
        "str" => Some(types::builtin_str(args)),
        "int" => Some(types::builtin_int(args)),
        "float" => Some(types::builtin_float(args)),
        "bool" => Some(types::builtin_bool(args)),
        "type" => Some(types::builtin_type(args)),
        "len" => Some(collections::builtin_len(args)),
        "push" => Some(collections::builtin_push(args)),
        "pop" => Some(collections::builtin_pop(args)),
        "has" => Some(collections::builtin_has(args)),
        "sort" => Some(collections::builtin_sort(args)),
        "index" => Some(collections::builtin_index(args)),
        "flat" => Some(collections::builtin_flat(args)),
        "unique" => Some(collections::builtin_unique(args)),
        "zip" => Some(collections::builtin_zip(args)),
        "range" => Some(collections::builtin_range(args)),
        "slice" => Some(collections::builtin_slice(args)),
        "insert" => Some(collections::builtin_insert(args)),
        "remove" => Some(collections::builtin_remove(args)),
        "merge" => Some(collections::builtin_merge(args)),
        "sum" => Some(collections::builtin_sum(args)),
        "min" => Some(collections::builtin_min(args)),
        "max" => Some(collections::builtin_max(args)),
        "split" => Some(strings::builtin_split(args)),
        "join" => Some(strings::builtin_join(args)),
        "trim" => Some(strings::builtin_trim(args)),
        "upper" => Some(strings::builtin_upper(args)),
        "lower" => Some(strings::builtin_lower(args)),
        "replace" => Some(strings::builtin_replace(args)),
        "contains" => Some(strings::builtin_contains(args)),
        "starts_with" => Some(strings::builtin_starts_with(args)),
        "ends_with" => Some(strings::builtin_ends_with(args)),
        "substr" => Some(strings::builtin_substr(args)),
        "index_of" => Some(strings::builtin_index_of(args)),
        "pad_left" => Some(strings::builtin_pad_left(args)),
        "pad_right" => Some(strings::builtin_pad_right(args)),
        "repeat" => Some(strings::builtin_repeat(args)),
        "reverse" => Some(strings::builtin_reverse(args)),
        "chars" => Some(strings::builtin_chars(args)),
        "match" => Some(strings::builtin_match(args)),
        "match_all" => Some(strings::builtin_match_all(args)),
        _ => call_builtin_system(name, args),
    }
}

fn call_builtin_system(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    match name {
        "env" => Some(system::builtin_env(args)),
        "exit" => Some(system::builtin_exit(args)),
        "os" => Some(Ok(system::builtin_os())),
        "sleep" => Some(system::builtin_sleep(args)),
        "read" => Some(filesystem::builtin_read(args)),
        "write" => Some(filesystem::builtin_write(args)),
        "append" => Some(filesystem::builtin_append(args)),
        "exists" => Some(filesystem::builtin_exists(args)),
        "is_file" => Some(filesystem::builtin_is_file(args)),
        "is_dir" => Some(filesystem::builtin_is_dir(args)),
        "mkdir" => Some(filesystem::builtin_mkdir(args)),
        "rm" => Some(filesystem::builtin_rm(args)),
        "cp" => Some(filesystem::builtin_cp(args)),
        "mv" => Some(filesystem::builtin_mv(args)),
        "ls" => Some(filesystem::builtin_ls(args)),
        "cwd" => Some(filesystem::builtin_cwd(args)),
        "cd" => Some(filesystem::builtin_cd(args)),
        "basename" => Some(filesystem::builtin_basename(args)),
        "dirname" => Some(filesystem::builtin_dirname(args)),
        "ext" => Some(filesystem::builtin_ext(args)),
        "abs" => Some(filesystem::builtin_abs(args)),
        "glob" => Some(filesystem::builtin_glob(args)),
        "tempfile" => Some(filesystem::builtin_tempfile(args)),
        "tempdir" => Some(filesystem::builtin_tempdir(args)),
        "filesize" => Some(filesystem::builtin_filesize(args)),
        "abs_num" => Some(math::builtin_abs_num(args)),
        "ceil" => Some(math::builtin_ceil(args)),
        "floor" => Some(math::builtin_floor(args)),
        "round" => Some(math::builtin_round(args)),
        "sqrt" => Some(math::builtin_sqrt(args)),
        "pow" => Some(math::builtin_pow(args)),
        "log" => Some(math::builtin_log(args)),
        "log10" => Some(math::builtin_log10(args)),
        "sin" => Some(math::builtin_sin(args)),
        "cos" => Some(math::builtin_cos(args)),
        "tan" => Some(math::builtin_tan(args)),
        "random" => Some(Ok(math::builtin_random(args))),
        "random_int" => Some(math::builtin_random_int(args)),
        "pi" => Some(Ok(math::builtin_pi(args))),
        "infinity" => Some(Ok(math::builtin_infinity(args))),
        "now" => Some(Ok(datetime::builtin_now(args))),
        "timestamp" => Some(Ok(datetime::builtin_timestamp(args))),
        "date_format" => Some(datetime::builtin_date_format(args)),
        "date_parse" => Some(datetime::builtin_date_parse(args)),
        "elapsed" => Some(datetime::builtin_elapsed(args)),
        _ => call_builtin_extended(name, args),
    }
}

fn call_builtin_extended(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    match name {
        "from_json" => Some(formats::builtin_from_json(args)),
        "to_json" => Some(formats::builtin_to_json(args)),
        "from_toml" => Some(formats::builtin_from_toml(args)),
        "to_toml" => Some(formats::builtin_to_toml(args)),
        "from_yaml" => Some(formats::builtin_from_yaml(args)),
        "to_yaml" => Some(formats::builtin_to_yaml(args)),
        "from_csv" => Some(formats::builtin_from_csv(args)),
        "to_csv" => Some(formats::builtin_to_csv(args)),
        "to_base64" => Some(formats::builtin_to_base64(args)),
        "from_base64" => Some(formats::builtin_from_base64(args)),
        "to_hex" => Some(formats::builtin_to_hex(args)),
        "from_hex" => Some(formats::builtin_from_hex(args)),
        "url_encode" => Some(formats::builtin_url_encode(args)),
        "url_decode" => Some(formats::builtin_url_decode(args)),
        "http_get" => Some(network::builtin_http_get(args)),
        "http_post" => Some(network::builtin_http_post(args)),
        "http_put" => Some(network::builtin_http_put(args)),
        "http_delete" => Some(network::builtin_http_delete(args)),
        "download" => Some(network::builtin_download(args)),
        "hostname" => Some(Ok(network::builtin_hostname(args))),
        "ip" => Some(network::builtin_ip(args)),
        "env_set" => Some(system::builtin_env_set(args)),
        "env_all" => Some(Ok(system::builtin_env_all(args))),
        "pid" => Some(Ok(system::builtin_pid(args))),
        "arch" => Some(Ok(system::builtin_arch(args))),
        "which" => Some(system::builtin_which(args)),
        "args" => Some(Ok(system::builtin_args(args))),
        "input" => Some(system::builtin_input(args)),
        "exec" => Some(system::builtin_exec(args)),
        "home" => Some(system::builtin_home(args)),
        "exec_in" => Some(system::builtin_exec_in(args)),
        "md5" => Some(hashing::builtin_md5(args)),
        "sha256" => Some(hashing::builtin_sha256(args)),
        "sha512" => Some(hashing::builtin_sha512(args)),
        "uuid" => Some(Ok(hashing::builtin_uuid(args))),
        "wait" => Some(threads::builtin_wait(args)),
        "wait_all" => Some(threads::builtin_wait_all(args)),
        "wait_any" => Some(threads::builtin_wait_any(args)),
        "open_file" => Some(fileio::builtin_open_file(args)),
        "read_line" => Some(fileio::builtin_read_line(args)),
        "read_chunk" => Some(fileio::builtin_read_chunk(args)),
        "close_file" => Some(fileio::builtin_close_file(args)),
        "byte_len" => Some(fileio::builtin_byte_len(args)),
        "byte_at" => Some(fileio::builtin_byte_at(args)),
        "byte_slice" => Some(fileio::builtin_byte_slice(args)),
        "to_bytes" => Some(fileio::builtin_to_bytes(args)),
        "from_bytes" => Some(fileio::builtin_from_bytes(args)),
        "read_bytes" => Some(fileio::builtin_read_bytes(args)),
        "write_bytes" => Some(fileio::builtin_write_bytes(args)),
        "append_bytes" => Some(fileio::builtin_append_bytes(args)),
        "bytes_from_list" => Some(fileio::builtin_bytes_from_list(args)),
        "set_color" => Some(terminal::builtin_set_color(args)),
        "set_bg" => Some(terminal::builtin_set_bg(args)),
        "set_bold" => Some(terminal::builtin_set_bold(args)),
        "set_dim" => Some(terminal::builtin_set_dim(args)),
        "set_underline" => Some(terminal::builtin_set_underline(args)),
        "reset_style" => Some(Ok(terminal::builtin_reset_style(args))),
        "clear" => Some(Ok(terminal::builtin_clear(args))),
        "cursor_pos" => Some(terminal::builtin_cursor_pos(args)),
        "term_size" => Some(Ok(terminal::builtin_term_size(args))),
        "refresh_env" => Some(Ok(builtin_refresh_env(args))),
        "atomic" => Some(builtin_atomic(args)),
        "get_processes" => Some(process::builtin_get_processes(args)),
        "get_process_by_name" => Some(process::builtin_get_process_by_name(args)),
        "get_process_by_id" => Some(process::builtin_get_process_by_id(args)),
        "kill_process" => Some(process::builtin_kill_process(args)),
        "is_process_running" => Some(process::builtin_is_process_running(args)),
        _ => None,
    }
}

fn call_builtin_interp(name: &str, args: &[Value], interp: &mut Interpreter) -> Option<Result<Value, String>> {
    match name {
        "map" => Some(collections::builtin_map(args, interp)),
        "filter" => Some(collections::builtin_filter(args, interp)),
        "reduce" => Some(collections::builtin_reduce(args, interp)),
        "sort_by" => Some(collections::builtin_sort_by(args, interp)),
        "find" => Some(collections::builtin_find(args, interp)),
        "count" => Some(collections::builtin_count(args, interp)),
        "any" => Some(collections::builtin_any(args, interp)),
        "all" => Some(collections::builtin_all(args, interp)),
        "on_event" => Some(builtin_on_event(args, interp)),
        "thread" => Some(threads::builtin_thread(args, interp)),
        _ => None,
    }
}

fn builtin_refresh_env(_args: &[Value]) -> Value {
    crate::exec::clear_cache();
    Value::Void
}

fn builtin_atomic(args: &[Value]) -> Result<Value, String> {
    expect_args("atomic", args, 1)?;
    Ok(Value::Atomic(crate::interpreter::value::AtomicValue::new(&args[0])))
}

fn builtin_on_event(args: &[Value], interp: &mut Interpreter) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("on_event() expects 2 args, got {}", args.len()));
    }
    let event_name = if let Value::String(s) = &args[0] {
        s.to_string()
    } else {
        return Err(format!("on_event() first arg must be string, got {}", args[0].type_name()));
    };
    match &args[1] {
        Value::Lambda { .. } => {
            interp.event_handlers.entry(event_name).or_default().push(args[1].clone());
            Ok(Value::Void)
        }
        _ => Err(format!("on_event() second arg must be lambda, got {}", args[1].type_name())),
    }
}


/// Check that the expected number of arguments was passed.
///
/// # Errors
///
/// Returns an error if the actual count does not match `expected`.
pub fn expect_args(name: &str, args: &[Value], expected: usize) -> Result<(), String> {
    if args.len() == expected {
        return Ok(());
    }
    Err(format!("{name}() expects {expected} arg(s), got {}", args.len()))
}
