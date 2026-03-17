pub mod ast;
pub mod expr;
pub mod stmt;

use crate::lexer::token::SpannedToken;
use ast::Stmt;
use stmt::StmtParser;

/// Parses a token stream into a list of statements.
///
/// # Errors
///
/// Returns an error string if the token stream contains invalid syntax.
pub fn parse(tokens: &[SpannedToken]) -> Result<Vec<Stmt>, String> {
    let mut parser = StmtParser::new(tokens);
    parser.parse_program()
}
