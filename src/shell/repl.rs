use rustyline::error::ReadlineError;
use rustyline::validate::MatchingBracketValidator;
use rustyline::{CompletionType, Config, EditMode, Editor};
use rustyline_derive::{Completer, Helper, Highlighter, Hinter, Validator};

use delegate_shell::Interpreter;

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

pub fn run(engine: &mut Interpreter) {
    // Register shell-only builtins
    register_shell_builtins(engine);

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

fn register_shell_builtins(engine: &mut Interpreter) {
    use std::io::Write;
    use delegate_shell::builtins::registry::{Param, Type};
    use delegate_shell::interpreter::value::Value;

    let _ = engine.register(
        "run",
        &[Param::Required(Type::Lambda)],
        Type::Dyn,
        |args, interp| {
            interp.set_interactive(true);
            let result = interp.call_lambda(&args[0], vec![]);
            interp.set_interactive(false);
            result
        },
    );

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
