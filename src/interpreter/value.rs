use std::fmt;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicI64, Ordering};
use std::thread::JoinHandle;
use std::io::BufReader;
use std::fs::File;
use indexmap::IndexMap;

use std::collections::HashSet;

pub(crate) type SharedList = Rc<RefCell<Vec<Value>>>;
pub(crate) type SharedObject = Rc<RefCell<ObjectData>>;

#[derive(Debug, Clone)]
pub struct ObjectData {
    pub fields: IndexMap<String, Value>,
    /// Fields marked as `dyn` — skip type check when Void, normal check when set
    pub dyn_fields: HashSet<String>,
}

#[inline]
#[must_use]
pub fn new_list(items: Vec<Value>) -> Value {
    Value::List(Rc::new(RefCell::new(items)))
}

#[inline]
#[must_use]
pub fn new_object(map: IndexMap<String, Value>) -> Value {
    Value::Object(Rc::new(RefCell::new(ObjectData { fields: map, dyn_fields: HashSet::new() })))
}

#[inline]
#[must_use]
pub fn new_object_with_dyn(map: IndexMap<String, Value>, dyn_fields: HashSet<String>) -> Value {
    Value::Object(Rc::new(RefCell::new(ObjectData { fields: map, dyn_fields })))
}

/// A thread-safe version of Value for passing between threads.
#[derive(Debug, Clone)]
pub enum SendableValue {
    Int(i64),
    Float(f64),
    String(String),
    Bool(bool),
    List(Vec<Self>),
    Object(IndexMap<String, Self>),
    Void,
    Lambda { name: String, resolution: u8, bound_args: Vec<Self> },
    CommandResult { status: i32, out: String, err: String },
    Bytes(Vec<u8>),
    Atomic(AtomicValue),
}

/// Wrapper for a thread join handle, stored inside Value.
pub struct ThreadJoinHandle {
    pub handle: Option<JoinHandle<Result<SendableValue, String>>>,
}

impl fmt::Debug for ThreadJoinHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ThreadHandle(..)")
    }
}

/// Wrapper for a file handle with buffered reading.
pub struct FileHandleInner {
    pub reader: BufReader<File>,
}

/// Atomic value — thread-safe, shared across threads without copying.
/// Uses `AtomicI64` for ints (lock-free), `Arc<Mutex<SendableValue>>` for everything else.
#[derive(Clone)]
pub enum AtomicValue {
    Int(Arc<AtomicI64>),
    General(Arc<Mutex<SendableValue>>),
}

impl fmt::Debug for AtomicValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(a) => write!(f, "Atomic({})", a.load(Ordering::SeqCst)),
            Self::General(m) => write!(f, "Atomic({:?})", m.lock().unwrap_or_else(|e| e.into_inner())),
        }
    }
}

impl AtomicValue {
    #[must_use]
    pub fn new(val: &Value) -> Self {
        match val {
            Value::Int(n) => Self::Int(Arc::new(AtomicI64::new(*n))),
            other => Self::General(Arc::new(Mutex::new(other.to_sendable()))),
        }
    }

    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    #[must_use]
    pub fn load(&self) -> Value {
        match self {
            Self::Int(a) => Value::Int(a.load(Ordering::SeqCst)),
            Self::General(m) => Value::from_sendable(m.lock().unwrap_or_else(|e| e.into_inner()).clone()),
        }
    }

    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    pub fn store(&self, val: &Value) {
        match (self, val) {
            (Self::Int(a), Value::Int(n)) => { a.store(*n, Ordering::SeqCst); }
            (Self::General(m), _) => { *m.lock().unwrap_or_else(|e| e.into_inner()) = val.to_sendable(); }
            _ => {} // type mismatch silently ignored — runtime checks elsewhere
        }
    }

    /// Atomic add for ints. Returns old value.
    #[must_use]
    pub fn fetch_add(&self, n: i64) -> i64 {
        match self {
            Self::Int(a) => a.fetch_add(n, Ordering::SeqCst),
            Self::General(_) => 0,
        }
    }
}

impl fmt::Debug for FileHandleInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FileHandle")
    }
}

#[derive(Debug, Clone)]
pub enum Value {
    Int(i64),
    Float(f64),
    String(Rc<str>),
    Bool(bool),
    List(SharedList),
    Object(SharedObject),
    Void,
    /// Function reference: @fn or @fn(args)
    Lambda {
        name: String,
        resolution: u8, // 0=Normal, 1=OwnFirst, 2=SystemOnly
        bound_args: Vec<Self>,
    },
    /// Result from external command
    CommandResult {
        status: i32,
        out: String,
        err: String,
    },
    /// Thread handle
    ThreadHandle(Arc<Mutex<ThreadJoinHandle>>),
    /// Raw bytes
    Bytes(Vec<u8>),
    /// File handle for streaming I/O
    FileHandle(Arc<Mutex<FileHandleInner>>),
    /// Atomic value — thread-safe, shared across threads
    Atomic(AtomicValue),
}

/// A variable slot that may hold a value or an error (from ? assignment)
#[derive(Debug, Clone)]
pub enum MaybeError {
    Ok(Value),
    Err(ErrorInfo),
}

#[derive(Debug, Clone)]
pub struct ErrorInfo {
    pub message: String,
}

impl Value {
    // --- Convenience constructors for external API ---

    /// Create a string value.
    pub fn string(s: &str) -> Self {
        Self::String(Rc::from(s))
    }

    /// Create a list value from items.
    pub fn list(items: Vec<Value>) -> Self {
        new_list(items)
    }

    /// Create an object from key-value pairs.
    pub fn object<const N: usize>(fields: [(&str, Value); N]) -> Self {
        let mut map = IndexMap::new();
        for (k, v) in fields {
            map.insert(k.to_string(), v);
        }
        new_object(map)
    }

    // --- Type introspection ---

    #[must_use]
    pub const fn type_name(&self) -> &'static str {
        match self {
            Self::Int(_) => "int",
            Self::Float(_) => "float",
            Self::String(_) => "string",
            Self::Bool(_) => "bool",
            Self::List(_) => "list",
            Self::Object(_) => "object",
            Self::Void => "void",
            Self::Lambda { .. } => "ref",
            Self::CommandResult { .. } => "result",
            Self::ThreadHandle(_) => "thread",
            Self::Bytes(_) => "bytes",
            Self::FileHandle(_) => "filehandle",
            Self::Atomic(_) => "atomic",
        }
    }

    /// Convert to a thread-safe deep copy.
    pub fn to_sendable(&self) -> SendableValue {
        match self {
            Self::Int(n) => SendableValue::Int(*n),
            Self::Float(n) => SendableValue::Float(*n),
            Self::String(s) => SendableValue::String((*s).to_string()),
            Self::Bool(b) => SendableValue::Bool(*b),
            Self::List(l) => SendableValue::List(l.borrow().iter().map(Self::to_sendable).collect()),
            Self::Object(m) => SendableValue::Object(m.borrow().fields.iter().map(|(k, v)| (k.clone(), v.to_sendable())).collect()),
            Self::Void | Self::ThreadHandle(_) | Self::FileHandle(_) => SendableValue::Void,
            Self::Lambda { name, resolution, bound_args } => SendableValue::Lambda {
                name: name.clone(), resolution: *resolution,
                bound_args: bound_args.iter().map(Self::to_sendable).collect(),
            },
            Self::CommandResult { status, out, err } => SendableValue::CommandResult {
                status: *status, out: out.clone(), err: err.clone(),
            },
            Self::Bytes(b) => SendableValue::Bytes(b.clone()),
            Self::Atomic(a) => SendableValue::Atomic(a.clone()),
        }
    }

    /// Convert from a thread-safe value back to Value.
    pub fn from_sendable(sv: SendableValue) -> Self {
        match sv {
            SendableValue::Int(n) => Self::Int(n),
            SendableValue::Float(n) => Self::Float(n),
            SendableValue::String(s) => Self::String(Rc::from(s)),
            SendableValue::Bool(b) => Self::Bool(b),
            SendableValue::List(items) => new_list(items.into_iter().map(Self::from_sendable).collect()),
            SendableValue::Object(m) => new_object(m.into_iter().map(|(k, v)| (k, Self::from_sendable(v))).collect()),
            SendableValue::Void => Self::Void,
            SendableValue::Lambda { name, resolution, bound_args } => Self::Lambda {
                name, resolution, bound_args: bound_args.into_iter().map(Self::from_sendable).collect(),
            },
            SendableValue::CommandResult { status, out, err } => Self::CommandResult { status, out, err },
            SendableValue::Bytes(b) => Self::Bytes(b),
            SendableValue::Atomic(a) => Self::Atomic(a),
        }
    }

    #[must_use]
    pub fn is_truthy(&self) -> bool {
        match self {
            Self::Bool(b) => *b,
            Self::Int(n) => *n != 0,
            Self::Float(n) => *n != 0.0,
            Self::String(s) => !s.is_empty(),
            Self::List(l) => !l.borrow().is_empty(),
            Self::Void => false,
            Self::Bytes(b) => !b.is_empty(),
            Self::Atomic(a) => a.load().is_truthy(),
            _ => true,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(n) => write!(f, "{n}"),
            Self::Float(n) => write!(f, "{n}"),
            Self::String(s) => write!(f, "{s}"),
            Self::Bool(b) => write!(f, "{b}"),
            Self::List(items) => {
                let items = items.borrow();
                write!(f, "[")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{item}")?;
                }
                write!(f, "]")
            }
            Self::Object(map) => {
                let map = map.borrow();
                write!(f, "{{ ")?;
                for (i, (k, v)) in map.fields.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{k} = {v}")?;
                }
                write!(f, " }}")
            }
            Self::Void => write!(f, "void"),
            Self::Lambda { name, resolution: _, bound_args } => {
                write!(f, "@{name}")?;
                if !bound_args.is_empty() {
                    write!(f, "(")?;
                    for (i, arg) in bound_args.iter().enumerate() {
                        if i > 0 { write!(f, ", ")?; }
                        write!(f, "{arg}")?;
                    }
                    write!(f, ")")?;
                }
                Ok(())
            }
            Self::CommandResult { status, out, err } => {
                write!(f, "Result {{ status = {status}, out = \"{out}\", err = \"{err}\" }}")
            }
            Self::ThreadHandle(_) => write!(f, "thread(..)"),
            Self::Bytes(b) => write!(f, "bytes({})", b.len()),
            Self::FileHandle(_) => write!(f, "filehandle(..)"),
            Self::Atomic(a) => write!(f, "{}", a.load()),
        }
    }
}
