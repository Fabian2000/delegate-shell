#[derive(Debug, Clone, PartialEq)]
pub enum StringPart {
    Literal(String),
    Interpolation(Vec<SpannedToken>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpannedToken {
    pub token: Token,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // Literals
    Int(i64),
    Float(f64),
    String(Vec<StringPart>),
    Bool(bool),
    Void,

    // Identifier
    Ident(String),

    // Keywords
    If,
    Else,
    For,
    While,
    In,
    Return,
    Import,
    Free,
    Use,
    As,
    Throw,
    Enum,
    Match,
    Default,
    Continue,
    Break,
    True,
    False,
    Atomic,
    Alias,
    Dyn,
    Teach,
    From,
    On,
    Unsafe,
    Not, // used contextually — parser handles !expr vs fn!()

    // Arithmetic
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Power, // **

    // Comparison
    Eq,       // ==
    NotEq,    // !=
    Lt,       // <
    Gt,       // >
    LtEq,    // <=
    GtEq,    // >=

    // Logical
    And, // &&
    Or,  // ||
    Bang, // ! (logical NOT before expr, or function modifier after ident)

    // Bitwise
    BitAnd, // &
    BitOr,  // |
    BitXor, // ^
    BitNot, // ~
    Shl,    // <<
    Shr,    // >>

    // Assignment
    Assign,       // =
    PlusAssign,   // +=
    MinusAssign,  // -=
    StarAssign,   // *=
    SlashAssign,  // /=
    PercentAssign,// %=
    PowerAssign,  // **=
    BitAndAssign, // &=
    BitOrAssign,  // |=
    BitXorAssign, // ^=
    ShlAssign,    // <<=
    ShrAssign,    // >>=

    // Increment/Decrement
    Increment, // ++
    Decrement, // --

    // Special operators
    Send,              // ->
    SafeSend,          // ?>
    Dollar,            // $ (bare)
    DollarIndex(usize),// $0, $1, ...
    DollarField(String),// $field
    At,                // @
    Question,          // ?
    Range,             // ..

    // Delimiters
    LParen,   // (
    RParen,   // )
    LBracket, // [
    RBracket, // ]
    LBrace,   // {
    RBrace,   // }
    Comma,    // ,
    Dot,      // .
    Colon,    // :
    Semicolon,// ;

    // Indentation (synthetic)
    Newline,
    Indent,
    Dedent,

    // End of file
    Eof,
}
