use std::collections::HashMap;
use std::rc::Rc;

/// Bytecode opcodes. Variable-length encoding: 1 byte opcode + inline operands.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    // --- Constants ---
    LoadInt,          // +8 bytes (i64 LE)
    LoadFloat,        // +8 bytes (f64 LE)
    LoadTrue,
    LoadFalse,
    LoadVoid,
    LoadConst,        // +2 bytes (u16 constant pool index)

    // --- Variables ---
    GetLocal,         // +2 bytes (u16 slot)
    SetLocal,         // +2 bytes (u16 slot)
    GetGlobal,        // +2 bytes (u16 name index)
    SetGlobal,        // +2 bytes (u16 name index)

    // --- Specialized int arithmetic ---
    AddInt,
    SubInt,
    MulInt,
    DivInt,
    ModInt,
    NegInt,

    // --- Specialized int comparison ---
    EqInt,
    NeqInt,
    LtInt,
    GtInt,
    LteInt,
    GteInt,

    // --- Generic arithmetic (runtime type check) ---
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Neg,

    // --- Generic comparison ---
    Eq,
    Neq,
    Lt,
    Gt,
    Lte,
    Gte,

    // --- Logical ---
    Not,

    // --- Bitwise ---
    BitAnd,
    BitOr,
    BitXor,
    BitNot,
    Shl,
    Shr,

    // --- Control flow ---
    Jump,             // +4 bytes (i32 offset)
    JumpIfFalse,      // +4 bytes (i32 offset)
    JumpIfTrue,       // +4 bytes (i32 offset)
    Loop,             // +4 bytes (i32 negative offset)

    // --- Function calls ---
    Call,             // +2 bytes (name index) + 1 byte (argc) + 1 byte (resolution)
    CallLocal,        // +2 bytes (chunk index) + 1 byte (argc)
    CallBuiltin,      // +2 bytes (name index) + 1 byte (argc)
    Return,
    ReturnVoid,

    // --- Increment/Decrement ---
    IncLocal,         // +2 bytes (slot)
    DecLocal,         // +2 bytes (slot)
    PostIncLocal,     // +2 bytes (slot) — push old, then inc
    PostDecLocal,     // +2 bytes (slot)
    PreIncLocal,      // +2 bytes (slot) — inc, then push new
    PreDecLocal,      // +2 bytes (slot)

    // --- Compound assignment ---
    CompoundAddInt,   // +2 bytes (slot) — slot += TOS (int fast path)
    CompoundSubInt,   // +2 bytes (slot)

    // --- Collections ---
    MakeList,         // +2 bytes (element count)
    MakeObject,       // +2 bytes (field count) — keys are const pool indices on stack
    Index,            // pop [collection, index]
    FieldGet,         // +2 bytes (field name index)
    IndexSet,         // pop [collection, index, value]
    FieldSet,         // +2 bytes (field name index)

    // --- Strings ---
    MakeString,       // +2 bytes (part count) — pop N parts

    // --- Range ---
    MakeRange,        // pop [start, end]

    // --- Send ---
    PushSendCtx,
    PopSendCtx,
    GetDollar,
    GetDollarIndex,   // +2 bytes (usize as u16)
    GetDollarField,   // +2 bytes (field name index)

    // --- Lambda ---
    MakeLambda,       // +2 bytes (name index) + 1 byte (resolution) + 1 byte (bound argc)

    // --- Error handling ---
    ErrorCheck,       // +2 bytes (var name index)
    ErrorField,       // +2 bytes (var name index) + 2 bytes (field name index)
    SetErrorTolerant, // +2 bytes (slot) — ?= assignment
    RecordError,      // +2 bytes (var name index) — store last_error into error_vars[name]
    Throw,

    // --- Optional param ---
    OptionalCheck,    // +2 bytes (var name index)

    // --- Scope ---
    PushScope,
    PopScope,

    // --- Statements ---
    DefineFunction,   // +2 bytes (name index) + 2 bytes (chunk index)
    DefineEnum,       // +2 bytes (name index) + 2 bytes (variant count)
    Import,           // +2 bytes (path index)
    Free,             // +2 bytes (name index)
    Alias,            // +2 bytes (name index) + 2 bytes (target index)
    Use,              // +2 bytes (path index) + 2 bytes (alias index, 0xFFFF = none)
    Atomic,           // wrap TOS in AtomicValue

    // --- Misc ---
    Pop,
    Dup,
    CheckCancel,

    // --- Error handling ---
    /// Begin try block: if any error occurs before EndTry, jump to offset
    TryBegin,         // +4 bytes (i32 offset to error handler)
    /// End try block (normal execution reached here — skip error handler)
    TryEnd,           // +4 bytes (i32 offset to skip error handler)

    // --- Superinstructions (fused common patterns) ---
    /// push local[slot] - imm (fuses GetLocal + LoadInt + SubInt)
    SubLocalImm,      // +2 bytes (slot) + 8 bytes (i64 imm)
    /// push local[slot] + imm (fuses GetLocal + LoadInt + AddInt)
    AddLocalImm,      // +2 bytes (slot) + 8 bytes (i64 imm)
    /// if local[slot] <= imm: jump (fuses GetLocal + LoadInt + LteInt + JumpIfFalse)
    BranchIfLocalGtImm,  // +2 bytes (slot) + 8 bytes (i64 imm) + 4 bytes (offset)
    /// if local[slot] > imm: jump
    BranchIfLocalLteImm, // +2 bytes (slot) + 8 bytes (i64 imm) + 4 bytes (offset)
    /// push local[slot] (int only, no clone overhead)
    GetLocalInt,      // +2 bytes (slot)
}

/// Constant pool: interned strings shared across a chunk.
#[derive(Debug, Clone)]
pub struct ConstantPool {
    pub strings: Vec<Rc<str>>,
    index: HashMap<String, u16>,
}

impl ConstantPool {
    pub fn new() -> Self {
        Self { strings: Vec::new(), index: HashMap::new() }
    }

    /// Add a string, deduplicating. Returns its index.
    pub fn add(&mut self, s: &str) -> u16 {
        if let Some(&idx) = self.index.get(s) {
            return idx;
        }
        let idx = self.strings.len() as u16;
        self.strings.push(Rc::from(s));
        self.index.insert(s.to_owned(), idx);
        idx
    }

    pub fn get(&self, idx: u16) -> &Rc<str> {
        &self.strings[idx as usize]
    }
}

/// Info about a local variable in a compiled function.
#[derive(Debug, Clone)]
pub struct LocalInfo {
    pub name: String,
    pub slot: u16,
    pub depth: u16,
    pub is_dyn: bool,
}

/// A compiled bytecode chunk — one per function or top-level script.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub code: Vec<u8>,
    pub constants: ConstantPool,
    pub locals: Vec<LocalInfo>,
    /// Bytecode offset → source line number (for error messages).
    pub line_map: Vec<(usize, u32)>,
    /// Number of parameters (for function chunks).
    pub param_count: u8,
    /// Function name (empty for top-level).
    pub name: String,
    /// Global variable name → slot index. Only populated on the top-level chunk (index 0).
    pub global_slots: HashMap<String, u16>,
    /// Reverse mapping: global slot index → variable name. Only on top-level chunk.
    pub global_names: Vec<Rc<str>>,
}

impl Chunk {
    pub fn new(name: String, param_count: u8) -> Self {
        Self {
            code: Vec::new(),
            constants: ConstantPool::new(),
            locals: Vec::new(),
            line_map: Vec::new(),
            param_count,
            name,
            global_slots: HashMap::new(),
            global_names: Vec::new(),
        }
    }

    /// Emit a single opcode byte.
    pub fn emit(&mut self, op: Op, line: u32) {
        self.line_map.push((self.code.len(), line));
        self.code.push(op as u8);
    }

    /// Emit opcode + u16 operand.
    pub fn emit_u16(&mut self, op: Op, val: u16, line: u32) {
        self.line_map.push((self.code.len(), line));
        self.code.push(op as u8);
        self.code.extend_from_slice(&val.to_le_bytes());
    }

    /// Emit opcode + i64 operand.
    pub fn emit_i64(&mut self, op: Op, val: i64, line: u32) {
        self.line_map.push((self.code.len(), line));
        self.code.push(op as u8);
        self.code.extend_from_slice(&val.to_le_bytes());
    }

    /// Emit opcode + f64 operand.
    pub fn emit_f64(&mut self, op: Op, val: f64, line: u32) {
        self.line_map.push((self.code.len(), line));
        self.code.push(op as u8);
        self.code.extend_from_slice(&val.to_le_bytes());
    }

    /// Emit opcode + i32 operand (for jumps). Returns the offset of the i32 for patching.
    pub fn emit_jump(&mut self, op: Op, line: u32) -> usize {
        self.line_map.push((self.code.len(), line));
        self.code.push(op as u8);
        let offset = self.code.len();
        self.code.extend_from_slice(&0i32.to_le_bytes()); // placeholder
        offset
    }

    /// Patch a previously emitted jump to point to the current position.
    pub fn patch_jump(&mut self, offset: usize) {
        let target = self.code.len() as i32 - (offset as i32 + 4);
        let bytes = target.to_le_bytes();
        self.code[offset..offset + 4].copy_from_slice(&bytes);
    }

    /// Emit a backward loop jump to `loop_start`.
    pub fn emit_loop(&mut self, loop_start: usize, line: u32) {
        self.line_map.push((self.code.len(), line));
        self.code.push(Op::Loop as u8);
        let offset = (self.code.len() + 4) as i32 - loop_start as i32;
        self.code.extend_from_slice(&offset.to_le_bytes());
    }

    /// Current code position.
    pub fn pos(&self) -> usize {
        self.code.len()
    }

    /// Read a u16 at the given offset.
    pub fn read_u16(&self, offset: usize) -> u16 {
        u16::from_le_bytes([self.code[offset], self.code[offset + 1]])
    }

    /// Read an i32 at the given offset.
    pub fn read_i32(&self, offset: usize) -> i32 {
        i32::from_le_bytes([
            self.code[offset], self.code[offset + 1],
            self.code[offset + 2], self.code[offset + 3],
        ])
    }

    /// Read an i64 at the given offset.
    pub fn read_i64(&self, offset: usize) -> i64 {
        i64::from_le_bytes([
            self.code[offset], self.code[offset + 1],
            self.code[offset + 2], self.code[offset + 3],
            self.code[offset + 4], self.code[offset + 5],
            self.code[offset + 6], self.code[offset + 7],
        ])
    }

    /// Read an f64 at the given offset.
    pub fn read_f64(&self, offset: usize) -> f64 {
        f64::from_le_bytes([
            self.code[offset], self.code[offset + 1],
            self.code[offset + 2], self.code[offset + 3],
            self.code[offset + 4], self.code[offset + 5],
            self.code[offset + 6], self.code[offset + 7],
        ])
    }

    /// Look up the source line for a bytecode offset.
    pub fn line_at(&self, offset: usize) -> u32 {
        for &(off, line) in self.line_map.iter().rev() {
            if off <= offset {
                return line;
            }
        }
        0
    }
}
