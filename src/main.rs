use std::env;
use std::fs;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

use delegate_shell::Runtime;

mod shell;

static CANCELLED: AtomicBool = AtomicBool::new(false);
static INTERACTIVE_CHILD: AtomicBool = AtomicBool::new(false);

fn main() {
    ctrlc_handler();

    let raw_args: Vec<String> = env::args().collect();

    // Handle flags before filtering
    if raw_args.iter().any(|a| a == "--version" || a == "-v") {
        println!("dgsh {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if raw_args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return;
    }
    if raw_args.iter().any(|a| a == "--mcp") {
        shell::mcp::run_mcp_server();
        return;
    }

    let args: Vec<String> = std::iter::once(raw_args[0].clone())
        .chain(raw_args.iter().skip(1).filter(|a| !a.starts_with("--")).cloned())
        .collect();

    if args.len() < 2 {
        let mut engine = make_engine(&raw_args);
        shell::repl::run(&mut engine);
        return;
    }

    if args[1] == "-migrate" {
        run_migrate(&args);
        return;
    }

    let source = if args[1] == "-c" {
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

    let mut engine = make_engine(&raw_args);
    engine.cancel_flag = Some(&CANCELLED);
    // Set debug file name if running a file (not -c)
    if args[1] != "-c" && args[1] != "-e" {
        engine.set_debug_file(&args[1]);
    }
    if let Err(e) = engine.run_source(&source) {
        if let Some(code_str) = e.strip_prefix("\x00EXIT\x00") {
            let code: i32 = code_str.parse().unwrap_or(1);
            std::process::exit(code);
        }
        eprintln!("Runtime error: {e}");
        std::process::exit(1);
    }
}

fn make_engine(raw_args: &[String]) -> Runtime {
    let mut engine = Runtime::new().unwrap_or_else(|e| {
        eprintln!("Failed to initialize: {e}");
        std::process::exit(1);
    });
    let is_debug = raw_args.iter().any(|a| a == "--debug");
    if is_debug {
        // Debug mode forces tree-walking for full stepping support
        let _ = engine.set_execution_mode(delegate_shell::ExecutionMode::TreeWalk);
        let highlighter = shell::highlight::DgshHighlighter::new(engine.builtin_names());
        engine.on_debug(move |ctx| {
            use delegate_shell::DebugAction;
            // File header
            let fn_info = if ctx.function_name.is_empty() {
                String::new()
            } else {
                let params: String = ctx.function_params.iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(" > {}({})", ctx.function_name, params)
            };
            let header = format!("file {}:{}{}", ctx.file, ctx.line, fn_info);
            let separator = "-".repeat(header.len().max(40));
            eprintln!("\n\x1b[1;33m{header}\x1b[0m");
            eprintln!("\x1b[90m{separator}\x1b[0m");
            // Source lines with syntax highlighting
            if !ctx.source_context.is_empty() {
                let max_num = ctx.source_context.last().map(|(n, _, _)| *n).unwrap_or(0);
                let width = format!("{max_num}").len();
                for (line_num, content, is_current) in &ctx.source_context {
                    if *is_current {
                        // Yellow background with highlighted code
                        let highlighted = highlighter.highlight_line(content);
                        eprintln!("\x1b[43;30m {line_num:>width$} | {content:<60}\x1b[0m");
                        let _ = highlighted; // TODO: use highlighted within background color
                    } else {
                        let highlighted = highlighter.highlight_line(content);
                        eprintln!(" \x1b[90m{line_num:>width$} |\x1b[0m {highlighted}");
                    }
                }
            }
            eprintln!("\x1b[90m{separator}\x1b[0m");
            // Call stack
            if !ctx.call_stack.is_empty() {
                eprintln!("\x1b[90mstack: {}\x1b[0m", ctx.call_stack.join(" > "));
            }
            // Variables
            if !ctx.variables.is_empty() {
                for (name, val, type_name) in &ctx.variables {
                    eprintln!("\x1b[36m{name}\x1b[0m: \x1b[90m{type_name}\x1b[0m = {val}");
                }
            }
            // Single-key input (raw mode)
            eprint!("\n\x1b[32m[n]ext [s]tep-into [c]ontinue [q]uit\x1b[0m ");
            let _ = io::stderr().flush();
            let key = read_debug_key();
            eprintln!();
            match key {
                b'c' => DebugAction::Continue,
                b's' => DebugAction::StepInto,
                b'q' => DebugAction::Quit,
                _ => DebugAction::StepOver, // n, Enter, anything else = next
            }
        });
    } else if raw_args.iter().any(|a| a == "--vm") {
        let _ = engine.set_execution_mode(delegate_shell::ExecutionMode::Vm);
    } else if raw_args.iter().any(|a| a == "--jit") {
        let _ = engine.set_execution_mode(delegate_shell::ExecutionMode::Jit);
    } else if raw_args.iter().any(|a| a == "--tw") {
        let _ = engine.set_execution_mode(delegate_shell::ExecutionMode::TreeWalk);
    }
    engine
}

/// Read a single key from stdin without waiting for Enter.
fn read_debug_key() -> u8 {
    use crossterm::event::{self, Event, KeyCode, KeyModifiers};
    use crossterm::terminal;

    let _ = terminal::enable_raw_mode();
    let key = loop {
        if let Ok(Event::Key(key_event)) = event::read() {
            // Ctrl+C = quit
            if key_event.modifiers.contains(KeyModifiers::CONTROL) && key_event.code == KeyCode::Char('c') {
                break b'q';
            }
            match key_event.code {
                KeyCode::Char(c) => break c as u8,
                KeyCode::Enter => break b'n', // Enter = next
                KeyCode::Esc => break b'q',
                _ => {}
            }
        }
    };
    let _ = terminal::disable_raw_mode();
    key
}

fn print_help() {
    eprintln!("Usage: dgsh [options] [script.dgsh]");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  -c '<code>'     Execute code string");
    eprintln!("  -migrate <file> Migrate a bash script to dgsh");
    eprintln!("  --vm            Force bytecode VM execution");
    eprintln!("  --jit           Force JIT compilation");
    eprintln!("  --tw            Force tree-walk interpretation");
    eprintln!("  --mcp           Start MCP server (Model Context Protocol over stdio)");
    eprintln!("  --version, -v   Show version");
    eprintln!("  --help, -h      Show this help");
    eprintln!();
    eprintln!("Without arguments, starts an interactive REPL.");
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

#[cfg(unix)]
fn ctrlc_handler() {
    unsafe extern "C" fn handler(_sig: i32) {
        if !INTERACTIVE_CHILD.load(Ordering::Relaxed) {
            CANCELLED.store(true, Ordering::Relaxed);
        }
        // When INTERACTIVE_CHILD is true, the child process gets SIGINT
        // directly from the terminal — we don't cancel dgsh.
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
            if !INTERACTIVE_CHILD.load(Ordering::Relaxed) {
                CANCELLED.store(true, Ordering::Relaxed);
            }
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
