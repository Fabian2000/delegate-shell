use std::collections::HashMap;

use crate::interpreter::value::Value;
use crate::interpreter::Runtime;

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
            Self::Int => val.is_int(),
            Self::Float => val.is_float(),
            Self::Number => val.is_int() || val.is_float(),
            Self::String => val.is_string(),
            Self::Bool => val.is_bool(),
            Self::List => val.is_list(),
            Self::Object => val.is_object(),
            Self::Lambda => val.is_lambda(),
            Self::FileHandle => val.is_file_handle(),
            Self::Bytes => val.is_bytes(),
            Self::ThreadHandle => val.is_thread_handle(),
            Self::Void => val.is_void(),
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

type BuiltinHandler = std::rc::Rc<dyn Fn(&[Value], &mut Runtime) -> Result<Value, String>>;

struct Entry {
    params: &'static [Param],
    returns: Type,
    handler: BuiltinHandler,
}

pub struct BuiltinRegistry {
    defs: HashMap<String, Entry>,
}

impl Default for BuiltinRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl BuiltinRegistry {
    pub fn new() -> Self {
        Self {
            defs: HashMap::new(),
        }
    }

    /// Create an empty registry with no builtins registered.
    pub fn empty() -> Self {
        Self::new()
    }

    /// Register a builtin function. Returns `Err` if the name is already taken.
    pub fn register(
        &mut self,
        name: &str,
        params: &'static [Param],
        returns: Type,
        f: impl Fn(&[Value], &mut Runtime) -> Result<Value, String> + 'static,
    ) -> Result<(), String> {
        if self.defs.contains_key(name) {
            return Err(format!("Duplicate builtin: '{name}'"));
        }
        self.defs.insert(name.to_owned(), Entry { params, returns, handler: std::rc::Rc::new(f) });
        Ok(())
    }

    pub fn register_override(
        &mut self,
        name: &str,
        params: &'static [Param],
        returns: Type,
        f: impl Fn(&[Value], &mut Runtime) -> Result<Value, String> + 'static,
    ) -> Result<(), String> {
        self.defs.insert(name.to_owned(), Entry { params, returns, handler: std::rc::Rc::new(f) });
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
        f: fn(&[Value], &mut Runtime) -> Result<Value, String>,
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

    /// Get all registered builtin names.
    pub fn names(&self) -> Vec<String> {
        self.defs.keys().cloned().collect()
    }

    /// Check whether a name is a registered builtin.
    pub fn is_builtin(&self, name: &str) -> bool {
        self.defs.contains_key(name)
    }

    /// Validate args and return a cloned handler. Returns `None` if not registered.
    /// This separates the immutable lookup from the mutable call, avoiding borrow conflicts.
    pub fn validate_and_get_handler(
        &self,
        name: &str,
        args: &[Value],
    ) -> Option<Result<BuiltinHandler, String>> {
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

        Some(Ok(entry.handler.clone()))
    }

    /// Call a builtin by name. Returns `None` if not registered.
    pub fn call(
        &self,
        name: &str,
        args: &[Value],
        interpreter: &mut Runtime,
    ) -> Option<Result<Value, String>> {
        match self.validate_and_get_handler(name, args)? {
            Ok(handler) => Some(handler(args, interpreter)),
            Err(e) => Some(Err(e)),
        }
    }
}

// ---------------------------------------------------------------------------
// Build default registry with all standard builtins
// ---------------------------------------------------------------------------

/// Controls which builtins are available.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinAccess {
    /// All builtins (default).
    All,
    /// Core only: types, strings, collections, math, threads (if allowed).
    Core,
    /// Nothing. Only user-registered functions available.
    None,
}

pub(crate) fn build_registry(access: BuiltinAccess, allow_threads: bool) -> Result<BuiltinRegistry, String> {
    let mut reg = BuiltinRegistry::new();

    if access == BuiltinAccess::None {
        return Ok(reg);
    }

    // Core: always included for Core and All
    super::types::register(&mut reg)?;
    super::strings::register(&mut reg)?;
    super::collections::register(&mut reg)?;
    super::math::register(&mut reg)?;

    if allow_threads {
        super::threads::register(&mut reg)?;
    }

    if access == BuiltinAccess::All {
        super::io::register(&mut reg)?;
        super::filesystem::register(&mut reg)?;
        super::system::register(&mut reg)?;
        super::fileio::register(&mut reg)?;
        super::network::register(&mut reg)?;
        super::hashing::register(&mut reg)?;
        super::datetime::register(&mut reg)?;
        super::formats::register(&mut reg)?;
        super::terminal::register(&mut reg)?;
        super::process::register(&mut reg)?;
        super::dispatch::register(&mut reg)?;
    }

    Ok(reg)
}
