pub mod bytecode;
pub mod compiler;
pub mod machine;
pub mod jit;
pub mod auto_mode;

/// Execution mode for the interpreter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    /// Automatically selects the best mode per function.
    Auto,
    /// Tree-walking interpreter. Best for IO/object-heavy code.
    TreeWalk,
    /// Bytecode VM. Faster for compute-heavy workloads.
    Vm,
    /// Bytecode VM with JIT compilation for hot functions. Fastest for numeric code.
    Jit,
}
