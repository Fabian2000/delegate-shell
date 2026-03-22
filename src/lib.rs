pub mod lexer;
pub mod parser;
pub mod interpreter;
pub mod builtins;
pub mod exec;
pub mod errors;
pub mod migrate;
pub mod vm;

// Public API
pub use interpreter::Interpreter;
pub use interpreter::engine::{DebugContext, DebugAction};
pub use interpreter::value::Value;
pub use builtins::registry::{BuiltinAccess, Param, Type};
pub use vm::ExecutionMode;
