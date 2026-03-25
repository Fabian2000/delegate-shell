use rustyline::error::ReadlineError;
use rustyline::validate::MatchingBracketValidator;
use rustyline::{CompletionType, Config, EditMode, Editor};
use rustyline_derive::{Completer, Helper, Highlighter, Hinter, Validator};

use delegate_shell::Runtime;

use super::completion::DgshCompleter;
use super::highlight::DgshHighlighter;
use super::jobs::JobManager;
use super::rc;

#[derive(Helper, Highlighter, Hinter, Validator, Completer)]
struct DgshHelper {
    #[rustyline(Completer)]
    completer: DgshCompleter,
    #[rustyline(Highlighter)]
    highlighter: DgshHighlighter,
    #[rustyline(Validator)]
    validator: MatchingBracketValidator,
}

pub fn run(engine: &mut Runtime) {
    // Register shell-only builtins + REPL overrides
    register_shell_builtins(engine);
    register_repl_overrides(engine);

    // Load .dgshrc
    rc::load_rc(engine);

    let builtin_names = engine.builtin_names();
    let helper = DgshHelper {
        completer: DgshCompleter::new(builtin_names.clone()),
        highlighter: DgshHighlighter::new(builtin_names),
        validator: MatchingBracketValidator::new(),
    };

    let config = Config::builder()
        .completion_type(CompletionType::List)
        .edit_mode(EditMode::Emacs)
        .build();

    let mut rl = match Editor::with_config(config) {
        Ok(rl) => rl,
        Err(e) => {
            eprintln!("Failed to initialize REPL: {e}");
            return;
        }
    };
    rl.set_helper(Some(helper));

    // Load history
    let history_path = history_file();
    if let Some(ref path) = history_path {
        let _ = rl.load_history(path);
    }

    let mut jobs = JobManager::new();

    eprintln!("dgsh REPL -- type 'exit' to quit");

    loop {
        // Report completed jobs
        for job in jobs.collect_done() {
            let status = job.status.lock().unwrap_or_else(|p| p.into_inner());
            eprintln!("[{}] {} ({})", job.id, job.name, status);
        }

        let prompt = build_prompt();
        match rl.readline(&prompt) {
            Ok(line) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                rl.add_history_entry(trimmed).ok();

                if trimmed == "exit" || trimmed == "exit()" {
                    break;
                }

                // Built-in shell commands
                if trimmed == "jobs" || trimmed == "jobs()" {
                    for (id, name, status) in jobs.list() {
                        eprintln!("[{id}] {status} {name}");
                    }
                    continue;
                }

                if let Some(rest) = trimmed.strip_prefix("bg ") {
                    let source = rest.to_string();
                    let name = if source.len() > 40 {
                        format!("{}...", &source[..40])
                    } else {
                        source.clone()
                    };
                    let id = jobs.spawn(name, source);
                    eprintln!("[{id}] started");
                    continue;
                }

                // Collect multi-line blocks
                let mut source = line.clone();
                if needs_continuation(trimmed) {
                    loop {
                        match rl.readline(".. ") {
                            Ok(next) => {
                                if next.trim().is_empty() {
                                    break;
                                }
                                source.push('\n');
                                source.push_str(&next);
                            }
                            Err(_) => break,
                        }
                    }
                }

                if let Err(e) = engine.run_source(&source) {
                    if let Some(code_str) = e.strip_prefix("\x00EXIT\x00") {
                        let code: i32 = code_str.parse().unwrap_or(0);
                        std::process::exit(code);
                    }
                    eprintln!("Error: {e}");
                }
            }
            Err(ReadlineError::Interrupted) => {
                eprintln!("^C");
                continue;
            }
            Err(ReadlineError::Eof) => {
                break;
            }
            Err(e) => {
                eprintln!("Read error: {e}");
                break;
            }
        }
    }

    // Save history
    if let Some(ref path) = history_path {
        let _ = rl.save_history(path);
    }
}

fn needs_continuation(trimmed: &str) -> bool {
    trimmed.ends_with(':')
        || trimmed.starts_with("if ")
        || trimmed.starts_with("for ")
        || trimmed.starts_with("while ")
        || trimmed.starts_with("match ")
        || trimmed.ends_with('\\')
}

fn build_prompt() -> String {
    let cwd = std::env::current_dir()
        .map(|p| {
            if let Some(home) = std::env::var("HOME").ok() {
                let home_path = std::path::Path::new(&home);
                if let Ok(rel) = p.strip_prefix(home_path) {
                    return format!("~/{}", rel.display());
                }
            }
            p.display().to_string()
        })
        .unwrap_or_else(|_| "?".to_string());
    format!("{cwd} >> ")
}

fn history_file() -> Option<String> {
    #[cfg(unix)]
    {
        std::env::var("HOME")
            .ok()
            .map(|h| format!("{h}/.dgsh_history"))
    }
    #[cfg(windows)]
    {
        std::env::var("USERPROFILE")
            .ok()
            .map(|h| format!("{h}\\.dgsh_history"))
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

pub fn register_shell_builtins(engine: &mut Runtime) {
    use delegate_shell::builtins::registry::{Param, Type};
    use delegate_shell::interpreter::value::Value;

    let _ = engine.register(
        "run",
        &[Param::Required(Type::Lambda)],
        Type::Dyn,
        |args, interp| {
            interp.set_interactive(true);
            crate::INTERACTIVE_CHILD.store(true, std::sync::atomic::Ordering::Relaxed);
            let result = interp.call_lambda(&args[0], vec![]);
            crate::INTERACTIVE_CHILD.store(false, std::sync::atomic::Ordering::Relaxed);
            interp.set_interactive(false);
            result
        },
    );

    // cd: change directory
    let _ = engine.register(
        "cd",
        &[Param::Required(Type::String)],
        Type::Void,
        |args, _| {
            let path = args[0].as_str_ref().ok_or("cd: expected string")?;
            let target = if path == "~" || path == "" {
                std::env::var("HOME")
                    .or_else(|_| std::env::var("USERPROFILE"))
                    .unwrap_or_else(|_| "/".to_string())
            } else if let Some(rest) = path.strip_prefix("~/") {
                let home = std::env::var("HOME")
                    .or_else(|_| std::env::var("USERPROFILE"))
                    .unwrap_or_else(|_| "/".to_string());
                format!("{home}/{rest}")
            } else {
                path.to_string()
            };
            std::env::set_current_dir(&target)
                .map_err(|e| format!("cd: {target}: {e}"))?;
            Ok(Value::void())
        },
    );

    // http_serve(port, handler) — minimal HTTP server
    let _ = engine.register(
        "http_serve",
        &[Param::Required(Type::Int), Param::Required(Type::Lambda)],
        Type::Void,
        |args, interp| {
            use std::net::TcpListener;
            use std::io::{Read as IoRead, Write as IoWrite, BufRead, BufReader};

            let port = args[0].as_int().ok_or("http_serve() arg 1 expects int")?;
            let handler = args[1].clone();

            let listener = TcpListener::bind(format!("0.0.0.0:{port}"))
                .map_err(|e| format!("http_serve: {e}"))?;
            eprintln!("dgsh http server listening on http://localhost:{port}");

            for stream in listener.incoming() {
                let mut stream = match stream {
                    Ok(s) => s,
                    Err(_) => continue,
                };

                // Read request
                let mut reader = BufReader::new(&stream);
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() { continue; }

                // Parse method and path
                let parts: Vec<&str> = request_line.trim().split_whitespace().collect();
                if parts.len() < 2 { continue; }
                let method = parts[0];
                let path = parts[1];

                // Read headers
                let mut content_length: usize = 0;
                let mut headers_map = indexmap::IndexMap::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() { break; }
                    let trimmed = line.trim();
                    if trimmed.is_empty() { break; }
                    if let Some((k, v)) = trimmed.split_once(':') {
                        let key = k.trim().to_lowercase();
                        let val = v.trim().to_string();
                        if key == "content-length" {
                            content_length = val.parse().unwrap_or(0);
                        }
                        headers_map.insert(key, Value::string_from(&val));
                    }
                }

                // Read body
                let body = if content_length > 0 {
                    let mut buf = vec![0u8; content_length];
                    let _ = reader.read_exact(&mut buf);
                    String::from_utf8_lossy(&buf).to_string()
                } else {
                    String::new()
                };

                // Build request object
                let mut req = indexmap::IndexMap::new();
                req.insert("method".to_string(), Value::string_from(method));
                req.insert("path".to_string(), Value::string_from(path));
                req.insert("body".to_string(), Value::string_from(&body));
                req.insert("headers".to_string(), delegate_shell::interpreter::value::new_object(headers_map));
                let req_val = delegate_shell::interpreter::value::new_object(req);

                // Call handler
                let response = interp.call_lambda(&handler, vec![req_val]);

                // Build HTTP response
                let (status, resp_body, content_type) = match response {
                    Ok(ref val) => {
                        let status = val.as_object_ref()
                            .and_then(|o| o.borrow().fields.get("status").cloned())
                            .and_then(|v| v.as_int())
                            .unwrap_or(200);
                        let body = val.as_object_ref()
                            .and_then(|o| o.borrow().fields.get("body").cloned())
                            .map(|v| v.to_string())
                            .unwrap_or_default();
                        let ct = val.as_object_ref()
                            .and_then(|o| o.borrow().fields.get("headers").cloned())
                            .and_then(|h| h.as_object_ref()
                                .and_then(|o| o.borrow().fields.get("content_type").cloned()))
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "text/plain".to_string());
                        (status, body, ct)
                    }
                    Err(e) => (500, format!("{{\"error\": \"{e}\"}}"), "application/json".to_string()),
                };

                let status_text = match status {
                    200 => "OK", 201 => "Created", 204 => "No Content",
                    400 => "Bad Request", 404 => "Not Found", 500 => "Internal Server Error",
                    _ => "OK",
                };

                let response_str = format!(
                    "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    status, status_text, content_type, resp_body.len(), resp_body
                );
                let _ = stream.write_all(response_str.as_bytes());
                let _ = stream.flush();
            }

            Ok(Value::void())
        },
    );

}

/// REPL-only overrides (not for script mode)
pub fn register_repl_overrides(engine: &mut Runtime) {
    use std::io::Write;
    use delegate_shell::builtins::registry::{Param, Type};
    use delegate_shell::interpreter::value::Value;

    // REPL: print() adds newline if missing (like Python's interactive mode)
    let _ = engine.register_override(
        "print",
        &[Param::Required(Type::String)],
        Type::Void,
        |args, _| {
            let s = args[0].to_string();
            print!("{s}");
            if !s.ends_with('\n') {
                println!();
            }
            let _ = std::io::stdout().flush();
            Ok(Value::void())
        },
    );
}
