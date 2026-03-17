use std::fmt;
use indexmap::IndexMap;

#[derive(Debug, Clone)]
pub enum Value {
    Int(i64),
    Float(f64),
    String(String),
    Bool(bool),
    List(Vec<Self>),
    Object(IndexMap<String, Self>),
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
        }
    }

    #[must_use]
    pub fn is_truthy(&self) -> bool {
        match self {
            Self::Bool(b) => *b,
            Self::Int(n) => *n != 0,
            Self::Float(n) => *n != 0.0,
            Self::String(s) => !s.is_empty(),
            Self::List(l) => !l.is_empty(),
            Self::Void => false,
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
                write!(f, "[")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{item}")?;
                }
                write!(f, "]")
            }
            Self::Object(map) => {
                write!(f, "{{ ")?;
                for (i, (k, v)) in map.iter().enumerate() {
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
        }
    }
}
