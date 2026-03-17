use std::env;
use std::fs;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: dgsh <script.sh>");
        eprintln!("       dgsh -e '<code>'");
        std::process::exit(1);
    }

    let source = if args[1] == "-e" {
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

    // Lex
    let mut lexer = delegate_shell::lexer::Lexer::new(&source);
    let tokens = lexer.tokenize();

    // Parse
    let stmts = match delegate_shell::parser::parse(&tokens) {
        Ok(stmts) => stmts,
        Err(e) => {
            eprintln!("Parse error: {e}");
            std::process::exit(1);
        }
    };

    // Interpret
    let mut interpreter = delegate_shell::interpreter::Interpreter::new();
    if let Err(e) = interpreter.run(&stmts) {
        eprintln!("Runtime error: {e}");
        std::process::exit(1);
    }
}
