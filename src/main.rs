use std::env;
use std::fs;
use std::io::{self, Write, BufRead};
use std::sync::atomic::{AtomicBool, Ordering};

static CANCELLED: AtomicBool = AtomicBool::new(false);

fn main() {
    // Set up Ctrl+C handler
    ctrlc_handler();

    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        // No args → REPL mode
        run_repl();
        return;
    }

    // Migration mode
    if args[1] == "-migrate" {
        if args.len() < 3 {
            eprintln!("Usage: dgsh -migrate <script.sh> [output.dgsh]");
            std::process::exit(1);
        }

        // Warning prompt
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
            match fs::write(output_path, &result) {
                Ok(()) => {}
                Err(e) => {
                    eprintln!("Error writing '{output_path}': {e}");
                    std::process::exit(1);
                }
            }
            eprintln!("  {input_lines} lines read");
            eprintln!("  {output_lines} lines written → '{output_path}'");
            if todo_count > 0 {
                eprintln!("  {todo_count} items need manual review (marked with # TODO)");
            } else {
                eprintln!("  No manual review items — but still check the output!");
            }
        } else {
            print!("{result}");
            eprintln!("  {input_lines} lines read, {output_lines} lines output, {todo_count} TODOs");
        }
        return;
    }

    let source = if args[1] == "-c" {
        if args.len() < 3 {
            eprintln!("Usage: dgsh -c '<code>'");
            std::process::exit(1);
        }
        args[2].clone()
    } else if args[1] == "-e" {
        // Keep -e as alias for -c (backwards compat)
        if args.len() < 3 {
            eprintln!("Usage: dgsh -e '<code>'");
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

    run_source(&source);
}

fn run_source(source: &str) {
    let mut lexer = delegate_shell::lexer::Lexer::new(source);
    let tokens = lexer.tokenize();

    let stmts = match delegate_shell::parser::parse(&tokens) {
        Ok(stmts) => stmts,
        Err(e) => {
            eprintln!("Parse error: {e}");
            std::process::exit(1);
        }
    };

    let mut interpreter = delegate_shell::interpreter::Interpreter::new();
    interpreter.cancel_flag = Some(&CANCELLED);
    let result = interpreter.run(&stmts);
    if CANCELLED.load(Ordering::Relaxed) {
        interpreter.fire_event("cancel");
    }
    interpreter.fire_event("exit");
    if let Err(e) = result {
        eprintln!("Runtime error: {e}");
        std::process::exit(1);
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
            // CTRL_C_EVENT = 0, CTRL_BREAK_EVENT = 1
            CANCELLED.store(true, Ordering::Relaxed);
            1 // handled
        } else {
            0 // not handled
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
fn ctrlc_handler() {
    // Unsupported platform — cancel event won't fire
}

fn run_repl() {
    let stdin = io::stdin();
    let mut interpreter = delegate_shell::interpreter::Interpreter::new();

    eprintln!("dgsh REPL — type 'exit' to quit");

    loop {
        eprint!(">> ");
        let _ = io::stderr().flush();

        let mut line = String::new();
        let read_result = stdin.lock().read_line(&mut line);
        match read_result {
            Ok(0) => break, // EOF
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

        // Check if we need more lines (indented block)
        let mut source = line.clone();
        if trimmed.ends_with(':') || trimmed.starts_with("if ") || trimmed.starts_with("for ")
            || trimmed.starts_with("while ") || is_fn_def(trimmed)
        {
            // Multi-line mode: collect until empty line
            loop {
                eprint!(".. ");
                let _ = io::stderr().flush();
                let mut next = String::new();
                let next_result = stdin.lock().read_line(&mut next);
                match next_result {
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

        let mut lexer = delegate_shell::lexer::Lexer::new(&source);
        let tokens = lexer.tokenize();

        match delegate_shell::parser::parse(&tokens) {
            Ok(stmts) => {
                if let Err(e) = interpreter.run(&stmts) {
                    eprintln!("Error: {e}");
                }
            }
            Err(e) => eprintln!("Parse error: {e}"),
        }
    }
}

/// Check if a line looks like a function definition that needs a body.
/// A fn def in our language is: name(params) followed by an indented block.
/// A fn call like print(x) should NOT trigger multi-line mode.
/// We detect defs by checking: no `=` and no `.` and the ident before ( is a simple name.
const fn is_fn_def(_line: &str) -> bool {
    // In REPL, we don't support multi-line function defs for now.
    // Users can use one-liners with ; and : syntax.
    false
}
