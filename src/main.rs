use std::env;
use std::fs;
use std::io::{self, Write, BufRead};
use std::sync::atomic::{AtomicBool, Ordering};

use delegate_shell::Interpreter;

static CANCELLED: AtomicBool = AtomicBool::new(false);

fn main() {
    ctrlc_handler();

    let raw_args: Vec<String> = env::args().collect();
    // Separate flags from positional args
    let args: Vec<String> = std::iter::once(raw_args[0].clone())
        .chain(raw_args.iter().skip(1).filter(|a| !a.starts_with("--")).cloned())
        .collect();

    if args.len() < 2 {
        run_repl();
        return;
    }

    if args[1] == "-migrate" {
        run_migrate(&args);
        return;
    }

    let source = if args[1] == "-c" || args[1] == "-e" {
        if args.len() < 3 {
            eprintln!("Usage: dgsh {} '<code>'", args[1]);
            std::process::exit(1);
        }
        args[2].clone()
    } else {
        match fs::read_to_string(&args[1]) {
            Ok(content) => content,
            Err(e) => {
                eprintln!("Error reading '{}': {}", args[1], e);
                std::process::exit(1);
            }
        }
    };

    let mut engine = Interpreter::new().unwrap_or_else(|e| {
        eprintln!("Failed to initialize: {e}");
        std::process::exit(1);
    });
    // Execution mode override (default: Auto)
    if raw_args.iter().any(|a| a == "--vm") {
        let _ = engine.set_execution_mode(delegate_shell::ExecutionMode::Vm);
    } else if raw_args.iter().any(|a| a == "--jit") {
        let _ = engine.set_execution_mode(delegate_shell::ExecutionMode::Jit);
    } else if raw_args.iter().any(|a| a == "--tw") {
        let _ = engine.set_execution_mode(delegate_shell::ExecutionMode::TreeWalk);
    }
    engine.cancel_flag = Some(&CANCELLED);
    if let Err(e) = engine.run_source(&source) {
        if let Some(code_str) = e.strip_prefix("\x00EXIT\x00") {
            let code: i32 = code_str.parse().unwrap_or(1);
            std::process::exit(code);
        }
        eprintln!("Runtime error: {e}");
        std::process::exit(1);
    }
}

fn run_migrate(args: &[String]) {
    if args.len() < 3 {
        eprintln!("Usage: dgsh -migrate <script.sh> [output.dgsh]");
        std::process::exit(1);
    }

    eprintln!("WARNING: The migration tool is experimental and may produce errors.");
    eprintln!("The output requires manual review before use.");
    eprint!("Continue? (y/n) ");
    let _ = io::stderr().flush();
    let mut confirm = String::new();
    if io::stdin().read_line(&mut confirm).is_err() || !confirm.trim().eq_ignore_ascii_case("y") {
        eprintln!("Aborted.");
        std::process::exit(0);
    }

    let input_path = &args[2];
    let input = match fs::read_to_string(input_path) {
        Ok(content) => content,
        Err(e) => {
            eprintln!("Error reading '{input_path}': {e}");
            std::process::exit(1);
        }
    };

    let input_lines = input.lines().count();
    eprintln!("Migrating '{input_path}'...");

    let result = delegate_shell::migrate::migrate_sh_to_dgsh(&input);

    let output_lines = result.lines().count();
    let todo_count = result.lines().filter(|l| l.contains("# TODO")).count();

    if args.len() >= 4 {
        let output_path = &args[3];
        if let Err(e) = fs::write(output_path, &result) {
            eprintln!("Error writing '{output_path}': {e}");
            std::process::exit(1);
        }
        eprintln!("  {input_lines} lines read");
        eprintln!("  {output_lines} lines written -> '{output_path}'");
        if todo_count > 0 {
            eprintln!("  {todo_count} items need manual review (marked with # TODO)");
        } else {
            eprintln!("  No manual review items -- but still check the output!");
        }
    } else {
        print!("{result}");
        eprintln!("  {input_lines} lines read, {output_lines} lines output, {todo_count} TODOs");
    }
}

fn run_repl() {
    let stdin = io::stdin();
    let mut interp = Interpreter::new().unwrap_or_else(|e| {
        eprintln!("Failed to initialize: {e}");
        std::process::exit(1);
    });

    eprintln!("dgsh REPL -- type 'exit' to quit");

    loop {
        eprint!(">> ");
        let _ = io::stderr().flush();

        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                eprintln!("Read error: {e}");
                break;
            }
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed == "exit" || trimmed == "exit()" {
            break;
        }

        // Collect multi-line blocks
        let mut source = line.clone();
        if trimmed.ends_with(':') || trimmed.starts_with("if ")
            || trimmed.starts_with("for ") || trimmed.starts_with("while ")
        {
            loop {
                eprint!(".. ");
                let _ = io::stderr().flush();
                let mut next = String::new();
                match stdin.lock().read_line(&mut next) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if next.trim().is_empty() {
                            break;
                        }
                        source.push_str(&next);
                    }
                }
            }
        }

        if let Err(e) = interp.run_source(&source) {
            if let Some(code_str) = e.strip_prefix("\x00EXIT\x00") {
                let code: i32 = code_str.parse().unwrap_or(1);
                std::process::exit(code);
            }
            eprintln!("Error: {e}");
        }
    }
}

#[cfg(unix)]
fn ctrlc_handler() {
    unsafe extern "C" fn handler(_sig: i32) {
        CANCELLED.store(true, Ordering::Relaxed);
    }
    unsafe {
        let _ = signal(2, handler as *const () as usize);
    }
}

#[cfg(unix)]
unsafe extern "C" {
    fn signal(sig: i32, handler: usize) -> usize;
}

#[cfg(windows)]
fn ctrlc_handler() {
    unsafe extern "system" fn handler(ctrl_type: u32) -> i32 {
        if ctrl_type == 0 || ctrl_type == 1 {
            CANCELLED.store(true, Ordering::Relaxed);
            1
        } else {
            0
        }
    }
    unsafe {
        SetConsoleCtrlHandler(Some(handler), 1);
    }
}

#[cfg(windows)]
unsafe extern "system" {
    fn SetConsoleCtrlHandler(handler: Option<unsafe extern "system" fn(u32) -> i32>, add: i32) -> i32;
}

#[cfg(not(any(unix, windows)))]
fn ctrlc_handler() {}
