use crate::lexer::token::Span;

/// A type annotation on a declaration.
/// Annotations go on the LEFT side (the declaration), never on the right (the value).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeAnnotation {
    /// A simple type name, e.g. `int`, `string`, `bool`, etc.
    Simple(String),
    /// An object shape annotation, e.g. `{ name: string, age: int }`.
    /// Each entry is (field_name, type_name).
    Object(Vec<(String, String)>),
}

impl TypeAnnotation {
    pub fn type_name(&self) -> &str {
        match self {
            Self::Simple(s) => s,
            Self::Object(_) => "object",
        }
    }
}

/// How a function call should be resolved
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// `fn()` — exe → own → system
    Normal,
    /// fn!() — own → system
    OwnFirst,
    /// fn!!() — system only
    SystemOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add, Sub, Mul, Div, Mod, Pow,
    Eq, NotEq, Lt, Gt, LtEq, GtEq,
    And, Or,
    BitAnd, BitOr, BitXor, Shl, Shr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,    // -
    Not,    // !
    BitNot, // ~
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompoundOp {
    Add, Sub, Mul, Div, Mod, Pow,
    BitAnd, BitOr, BitXor, Shl, Shr,
}

/// Dollar reference in send context
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DollarRef {
    /// $ — entire value
    Whole,
    /// $0, $1, ... — list index
    Index(usize),
    /// $field — object field access
    Field(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum StringPart {
    Literal(String),
    Expr(Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Int(i64),
    Float(f64),
    String(Vec<StringPart>),
    Bool(bool),
    List(Vec<Expr>),
    Object(Vec<(String, Expr)>),

    /// Variable reference
    Ident(String),

    /// Binary operation
    BinaryOp {
        left: Box<Expr>,
        op: BinOp,
        right: Box<Expr>,
    },

    /// Unary operation (!, -, ~)
    UnaryOp {
        op: UnaryOp,
        expr: Box<Expr>,
    },

    /// Function call: name(args) / name!(args) / name!!(args)
    Call {
        name: String,
        resolution: Resolution,
        args: Vec<Expr>,
    },

    /// Indexing: expr[index]
    Index {
        expr: Box<Expr>,
        index: Box<Expr>,
    },

    /// Field access: expr.field
    FieldAccess {
        expr: Box<Expr>,
        field: String,
    },

    /// Range: start..end
    Range {
        start: Box<Expr>,
        end: Box<Expr>,
    },

    /// Send: expr -> expr
    Send {
        left: Box<Expr>,
        right: Box<Expr>,
    },

    /// Safe send: expr ?> expr — stops chain on error
    SafeSend {
        left: Box<Expr>,
        right: Box<Expr>,
    },

    /// Lambda reference: @fn or @fn(args)
    Lambda {
        name: String,
        resolution: Resolution,
        bound_args: Vec<Expr>,
    },

    /// Dollar reference in send context
    DollarRef(DollarRef),

    /// Error check: x? — returns true if x is ok, false if error
    ErrorCheck(String),

    /// Error field access: x?.error, x?.message etc.
    ErrorField {
        name: String,
        field: String,
    },

    /// Optional param check: <param> — returns true if the optional param was provided
    OptionalCheck(String),

    /// `atomic <expr>` — wrap a value in an AtomicValue
    Atomic(Box<Expr>),

    /// Post-increment/decrement: i++, i-- — increments/decrements, returns the OLD value
    PostIncDec {
        name: String,
        increment: bool, // true = ++, false = --
    },

    /// Pre-increment/decrement: ++i, --i — increments/decrements, returns the NEW value
    PreIncDec {
        name: String,
        increment: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    /// x = expr or x? = expr, with optional type annotation: x: type = expr
    Assign {
        name: String,
        error_tolerant: bool,
        /// Optional type annotation on the variable declaration
        type_ann: Option<TypeAnnotation>,
        /// `dyn x = expr` — allows reassignment with different types
        is_dyn: bool,
        expr: Expr,
    },

    /// x += expr, x -= expr, etc.
    CompoundAssign {
        name: String,
        op: CompoundOp,
        expr: Expr,
    },

    /// list[idx] = expr
    IndexAssign {
        target: Expr,
        index: Expr,
        value: Expr,
    },

    /// obj.field = expr
    FieldAssign {
        target: Expr,
        field: String,
        value: Expr,
    },

    /// x++ or x--
    PostIncDec {
        name: String,
        increment: bool, // true = ++, false = --
    },

    /// ++x or --x
    PreIncDec {
        name: String,
        increment: bool,
    },

    /// Expression as statement (function calls, etc.)
    ExprStmt(Expr),

    /// Function definition
    FnDef {
        name: String,
        /// Required parameters: (name, type_annotation, is_dyn)
        params: Vec<(String, Option<TypeAnnotation>, bool)>,
        /// Optional parameters: (name, type_annotation, is_dyn)
        optional_params: Vec<(String, Option<TypeAnnotation>, bool)>,
        /// Optional return type annotation
        return_type_ann: Option<TypeAnnotation>,
        body: Vec<Stmt>,
    },

    /// if condition\n    body\n else\n    body
    If {
        condition: Expr,
        body: Vec<Stmt>,
        else_body: Option<Vec<Stmt>>,
    },

    /// while condition\n    body
    While {
        condition: Expr,
        body: Vec<Stmt>,
    },

    /// for var in iter\n    body
    For {
        var: String,
        iter: Expr,
        body: Vec<Stmt>,
    },

    /// return expr / dyn return expr
    Return {
        expr: Option<Expr>,
        is_dyn: bool,
    },

    /// continue — skip to next loop iteration
    Continue,

    /// break — exit loop
    Break,

    /// import "file"
    Import(String),

    /// free(varname) — release a variable
    Free(String),

    /// use "path" or use "path" as alias
    Use {
        path: String,
        alias: Option<String>,
    },

    /// throw expr — throw an error
    Throw(Expr),

    /// enum Name\n    VARIANT1\n    VARIANT2\n...
    EnumDef {
        name: String,
        variants: Vec<String>,
    },

    /// match expr\n    pattern\n        body\n    pattern\n        body\n...
    Match {
        expr: Expr,
        arms: Vec<MatchArm>,
    },

    /// alias name = "target" — maps a call name to an executable string
    Alias {
        name: String,
        target: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    /// None = catch-all (`default`)
    pub pattern: Option<Expr>,
    pub body: Vec<Stmt>,
}
