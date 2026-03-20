use std::collections::HashMap;

use crate::interpreter::value::Value;
use crate::interpreter::Interpreter;

// ---------------------------------------------------------------------------
// Type metadata
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Type {
    Int,
    Float,
    Number, // Int | Float
    String,
    Bool,
    List,
    Object,
    Lambda,
    FileHandle,
    Bytes,
    ThreadHandle,
    Void,
    Dyn,
}

impl Type {
    pub fn name(self) -> &'static str {
        match self {
            Self::Int => "int",
            Self::Float => "float",
            Self::Number => "number",
            Self::String => "string",
            Self::Bool => "bool",
            Self::List => "list",
            Self::Object => "object",
            Self::Lambda => "lambda",
            Self::FileHandle => "file_handle",
            Self::Bytes => "bytes",
            Self::ThreadHandle => "thread_handle",
            Self::Void => "void",
            Self::Dyn => "dyn",
        }
    }

    pub fn matches(self, val: &Value) -> bool {
        match self {
            Self::Dyn => true,
            Self::Int => matches!(val, Value::Int(_)),
            Self::Float => matches!(val, Value::Float(_)),
            Self::Number => matches!(val, Value::Int(_) | Value::Float(_)),
            Self::String => matches!(val, Value::String(_)),
            Self::Bool => matches!(val, Value::Bool(_)),
            Self::List => matches!(val, Value::List(_)),
            Self::Object => matches!(val, Value::Object(_)),
            Self::Lambda => matches!(val, Value::Lambda { .. }),
            Self::FileHandle => matches!(val, Value::FileHandle(_)),
            Self::Bytes => matches!(val, Value::Bytes(_)),
            Self::ThreadHandle => matches!(val, Value::ThreadHandle(_)),
            Self::Void => matches!(val, Value::Void),
        }
    }
}

// ---------------------------------------------------------------------------
// Parameter descriptor
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub enum Param {
    Required(Type),
    Optional(Type),
}

impl Param {
    pub const fn param_type(&self) -> Type {
        match self {
            Self::Required(t) | Self::Optional(t) => *t,
        }
    }

    pub const fn is_required(&self) -> bool {
        matches!(self, Self::Required(_))
    }
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

struct Entry {
    params: &'static [Param],
    returns: Type,
    handler: Box<dyn Fn(&[Value], &mut Interpreter) -> Result<Value, String>>,
}

pub struct BuiltinRegistry {
    defs: HashMap<String, Entry>,
}

impl BuiltinRegistry {
    pub(crate) fn new() -> Self {
        Self {
            defs: HashMap::new(),
        }
    }

    /// Register a builtin function. Returns `Err` if the name is already taken.
    pub fn register(
        &mut self,
        name: &str,
        params: &'static [Param],
        returns: Type,
        f: impl Fn(&[Value], &mut Interpreter) -> Result<Value, String> + 'static,
    ) -> Result<(), String> {
        if self.defs.contains_key(name) {
            return Err(format!("Duplicate builtin: '{name}'"));
        }
        self.defs.insert(name.to_owned(), Entry { params, returns, handler: Box::new(f) });
        Ok(())
    }

    /// Internal: register a pure builtin (no interpreter access needed).
    pub(crate) fn add(
        &mut self,
        name: &str,
        params: &'static [Param],
        returns: Type,
        f: fn(&[Value]) -> Result<Value, String>,
    ) -> Result<(), String> {
        self.register(name, params, returns, move |args, _| f(args))
    }

    /// Internal: register a builtin that needs interpreter access.
    pub(crate) fn add_interp(
        &mut self,
        name: &str,
        params: &'static [Param],
        returns: Type,
        f: fn(&[Value], &mut Interpreter) -> Result<Value, String>,
    ) -> Result<(), String> {
        self.register(name, params, returns, f)
    }

    /// Look up a builtin's return type.
    pub fn return_type(&self, name: &str) -> Option<Type> {
        self.defs.get(name).map(|e| e.returns)
    }

    /// Look up a builtin's parameter signature.
    pub fn params(&self, name: &str) -> Option<&'static [Param]> {
        self.defs.get(name).map(|e| e.params)
    }

    /// Check whether a name is a registered builtin.
    pub fn is_builtin(&self, name: &str) -> bool {
        self.defs.contains_key(name)
    }

    /// Call a builtin by name. Returns `None` if not registered.
    pub fn call(
        &self,
        name: &str,
        args: &[Value],
        interpreter: &mut Interpreter,
    ) -> Option<Result<Value, String>> {
        let entry = self.defs.get(name)?;

        // Validate argument count
        let min = entry.params.iter().filter(|p| p.is_required()).count();
        let max = entry.params.len();
        if args.len() < min || args.len() > max {
            return Some(Err(if min == max {
                format!("{}() expects {} arg(s), got {}", name, min, args.len())
            } else {
                format!("{}() expects {}-{} args, got {}", name, min, max, args.len())
            }));
        }

        // Validate argument types
        for (i, param) in entry.params.iter().enumerate() {
            if i < args.len() {
                let expected = param.param_type();
                if !expected.matches(&args[i]) {
                    return Some(Err(format!(
                        "{}() arg {} expects {}, got {}",
                        name, i + 1, expected.name(), args[i].type_name()
                    )));
                }
            }
        }

        Some((entry.handler)(args, interpreter))
    }
}

// ---------------------------------------------------------------------------
// Build default registry with all standard builtins
// ---------------------------------------------------------------------------

pub(crate) fn build_default_registry() -> Result<BuiltinRegistry, String> {
    let mut reg = BuiltinRegistry::new();
    super::io::register(&mut reg)?;
    super::types::register(&mut reg)?;
    super::strings::register(&mut reg)?;
    super::collections::register(&mut reg)?;
    super::math::register(&mut reg)?;
    super::filesystem::register(&mut reg)?;
    super::system::register(&mut reg)?;
    super::fileio::register(&mut reg)?;
    super::network::register(&mut reg)?;
    super::hashing::register(&mut reg)?;
    super::datetime::register(&mut reg)?;
    super::formats::register(&mut reg)?;
    super::terminal::register(&mut reg)?;
    super::threads::register(&mut reg)?;
    super::process::register(&mut reg)?;
    super::dispatch::register(&mut reg)?;
    Ok(reg)
}
