use crate::lexer::token::Span;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ShellError {
    #[error("Runtime error at {span:?}: {message}")]
    Runtime { message: String, span: Span },

    #[error("Parse error at {span:?}: {message}")]
    Parse { message: String, span: Span },

    #[error("Type error: {message}")]
    Type { message: String },

    #[error("Undefined: {name}")]
    Undefined { name: String },

    #[error("Null access: variable '{name}' is in error state")]
    NullAccess { name: String },
}
