//! AOT (Ahead-of-Time) compilation: compile .dgsh scripts to standalone native binaries.

pub mod codegen;
pub mod linker;
pub mod runtime_helpers;

use crate::vm::compiler::Compiler;

/// Compile a dgsh source file to a standalone native binary.
///
/// Returns the path to the output binary on success.
pub fn compile_to_binary(source: &str, script_path: &str) -> Result<String, String> {
    // 1. Parse source to AST
    let mut lexer = crate::lexer::Lexer::new(source);
    let tokens = lexer.tokenize();
    let stmts = crate::parser::parse(&tokens)?;

    // 2. Resolve imports recursively
    let stmts = resolve_imports(stmts, script_path)?;

    // 3. Compile AST to bytecode chunks
    let chunks = Compiler::compile(&stmts)
        .map_err(|e| format!("Bytecode compilation error: {e}"))?;

    // 4. Extract teach statements from source for runtime init
    let teach_lines: Vec<&str> = source.lines()
        .filter(|l| l.trim_start().starts_with("teach "))
        .collect();
    let teach_source = teach_lines.join("\n");

    // 5. Generate native code for ALL chunks via Cranelift ObjectModule
    let object_bytes = codegen::compile_chunks_to_object(&chunks, &teach_source)?;

    // 5. Write object file to temp path
    let output_name = script_path
        .trim_end_matches(".dgsh")
        .rsplit('/')
        .next()
        .unwrap_or("a.out");
    let obj_path = format!("/tmp/dgsh_aot_{}.o", output_name);
    std::fs::write(&obj_path, &object_bytes)
        .map_err(|e| format!("Failed to write object file: {e}"))?;

    // 6. Detect system linker and link
    let output_dir = std::path::Path::new(script_path)
        .parent()
        .unwrap_or(std::path::Path::new("."));
    let output_path = output_dir.join(output_name).to_string_lossy().to_string();
    linker::link(&obj_path, &output_path)?;

    // Clean up object file
    let _ = std::fs::remove_file(&obj_path);

    Ok(output_path)
}

/// Resolve `import` statements by reading files, parsing, and merging into the AST.
fn resolve_imports(
    stmts: Vec<crate::parser::ast::Stmt>,
    script_path: &str,
) -> Result<Vec<crate::parser::ast::Stmt>, String> {
    use crate::parser::ast::StmtKind;

    let base_dir = std::path::Path::new(script_path)
        .parent()
        .unwrap_or(std::path::Path::new("."));

    let mut result = Vec::new();
    for stmt in stmts {
        if let StmtKind::Import(path) = &stmt.kind {
            let import_path = base_dir.join(path);
            let source = std::fs::read_to_string(&import_path)
                .map_err(|e| format!("Failed to read import '{}': {e}", import_path.display()))?;
            let mut lexer = crate::lexer::Lexer::new(&source);
            let tokens = lexer.tokenize();
            let imported_stmts = crate::parser::parse(&tokens)?;
            let resolved = resolve_imports(imported_stmts, &import_path.to_string_lossy())?;
            result.extend(resolved);
        } else {
            result.push(stmt);
        }
    }
    Ok(result)
}
