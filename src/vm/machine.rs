use std::rc::Rc;
use crate::interpreter::value::{Value, ValueKind as VK, new_list, new_object};
use crate::interpreter::Interpreter;
use crate::parser::ast::Resolution;
use super::bytecode::{Op, Chunk};

/// A call frame on the VM's call stack.
#[derive(Debug)]
pub(crate) struct CallFrame {
    pub(crate) chunk_idx: usize,
    pub(crate) ip: usize,
    pub(crate) base: usize,
}

/// Stack-based bytecode virtual machine.
/// Error recovery point for try/catch blocks.
struct TryPoint {
    /// Frame index to restore on error.
    frame_idx: usize,
    /// IP to jump to on error.
    error_ip: usize,
    /// Stack depth to restore.
    stack_depth: usize,
    /// Last error message (set when error is caught).
    _error_msg: Option<String>,
}

pub struct VM {
    pub(crate) stack: Vec<Value>,
    pub(crate) frames: Vec<CallFrame>,
    pub(crate) chunks: Vec<Chunk>,
    globals: Vec<Value>,
    /// Reverse mapping: global slot index → variable name (for error messages & lambda lookup)
    global_names: Vec<Rc<str>>,
    /// Name → global slot index (for runtime name-based lookup, e.g. lambda in globals)
    global_name_to_slot: std::collections::HashMap<String, u16>,
    error_vars: std::collections::HashMap<u16, String>,
    ok_vars: std::collections::HashSet<u16>,
    fn_table: std::collections::HashMap<String, usize>,
    send_stack: Vec<Option<Value>>,
    try_stack: Vec<TryPoint>,
    pub(crate) last_error: Option<String>,
    jit: Option<super::jit::JitManager>,
    pub(crate) call_depth: usize,
}

impl Default for VM {
    fn default() -> Self {
        Self::new()
    }
}

impl VM {
    pub fn new() -> Self {
        Self {
            stack: Vec::with_capacity(4096),
            frames: Vec::with_capacity(256),
            chunks: Vec::new(),
            globals: Vec::new(),
            global_names: Vec::new(),
            global_name_to_slot: std::collections::HashMap::new(),
            error_vars: std::collections::HashMap::new(),
            ok_vars: std::collections::HashSet::new(),
            fn_table: std::collections::HashMap::new(),
            send_stack: Vec::new(),
            try_stack: Vec::new(),
            last_error: None,
            jit: None,
            call_depth: 0,
        }
    }

    pub fn execute(&mut self, chunks: Vec<Chunk>, interp: &mut Interpreter) -> Result<(), String> {
        let chunk_count = chunks.len();
        // Initialize global variable slots from the top-level chunk's metadata
        if !chunks.is_empty() {
            let num_globals = chunks[0].global_names.len();
            self.globals.resize(num_globals, Value::void());
            self.global_names = chunks[0].global_names.clone();
            self.global_name_to_slot = chunks[0].global_slots.clone();
        }
        self.chunks = chunks;
        if interp.is_jit_mode() {
            self.jit = Some(super::jit::JitManager::new(chunk_count));
        }
        self.frames.push(CallFrame { chunk_idx: 0, ip: 0, base: 0 });
        self.run(interp)
    }

    /// Run a single frame and return its result value. Used by JIT helpers.
    pub(crate) fn run_frame(&mut self, interp: &mut Interpreter) -> Result<Value, String> {
        let _target_depth = self.frames.len() - 1;
        self.run(interp)?;
        // The return value should be on the stack
        Ok(self.stack.pop().unwrap_or(Value::void()))
    }

    #[inline(never)]
    fn run(&mut self, interp: &mut Interpreter) -> Result<(), String> {
        // Cache hot values in locals for the dispatch loop
        let mut frame_idx = self.frames.len() - 1;

        macro_rules! ip { () => { self.frames[frame_idx].ip } }
        macro_rules! base { () => { self.frames[frame_idx].base } }
        macro_rules! ci { () => { self.frames[frame_idx].chunk_idx } }
        macro_rules! code { () => { &self.chunks[ci!()] } }

        macro_rules! read_u8 {
            () => {{
                let v = self.chunks[ci!()].code[ip!()];
                self.frames[frame_idx].ip += 1;
                v
            }}
        }
        macro_rules! read_u16 {
            () => {{
                let v = self.chunks[ci!()].read_u16(ip!());
                self.frames[frame_idx].ip += 2;
                v
            }}
        }
        macro_rules! read_i32 {
            () => {{
                let v = self.chunks[ci!()].read_i32(ip!());
                self.frames[frame_idx].ip += 4;
                v
            }}
        }
        macro_rules! read_i64 {
            () => {{
                let v = self.chunks[ci!()].read_i64(ip!());
                self.frames[frame_idx].ip += 8;
                v
            }}
        }
        macro_rules! read_f64 {
            () => {{
                let v = self.chunks[ci!()].read_f64(ip!());
                self.frames[frame_idx].ip += 8;
                v
            }}
        }

        loop {
            if ip!() >= code!().code.len() {
                if self.frames.len() <= 1 {
                    return Ok(());
                }
                let old_base = base!();
                self.frames.pop();
                frame_idx -= 1;
                self.stack.truncate(old_base);
                self.stack.push(Value::void());
                continue;
            }

            let op_byte = read_u8!();
            let op: Op = unsafe { std::mem::transmute(op_byte) };


            match op {
                // ============================================================
                // HOT PATH — these opcodes dominate fibonacci-like workloads
                // ============================================================
                Op::LoadInt => {
                    let val = read_i64!();
                    self.stack.push(Value::int(val));
                }
                Op::GetLocal => {
                    let slot = read_u16!() as usize;
                    let val = self.stack[base!() + slot].clone();
                    self.stack.push(val);
                }
                Op::SetLocal => {
                    let slot = read_u16!() as usize;
                    let val = self.stack.pop().ok_or("VM: stack underflow")?;
                    let idx = base!() + slot;
                    if idx >= self.stack.len() {
                        self.stack.resize(idx + 1, Value::void());
                    }
                    self.stack[idx] = val;
                }
                Op::AddInt => {
                    let b = self.pop_int()?;
                    let a = self.pop_int()?;
                    self.stack.push(Value::int(a + b));
                }
                Op::SubInt => {
                    let b = self.pop_int()?;
                    let a = self.pop_int()?;
                    self.stack.push(Value::int(a - b));
                }
                Op::LteInt => {
                    let b = self.pop_int()?;
                    let a = self.pop_int()?;
                    self.stack.push(Value::bool(a <= b));
                }
                Op::LtInt => {
                    let b = self.pop_int()?;
                    let a = self.pop_int()?;
                    self.stack.push(Value::bool(a < b));
                }
                Op::GtInt => {
                    let b = self.pop_int()?;
                    let a = self.pop_int()?;
                    self.stack.push(Value::bool(a > b));
                }
                Op::GteInt => {
                    let b = self.pop_int()?;
                    let a = self.pop_int()?;
                    self.stack.push(Value::bool(a >= b));
                }
                Op::EqInt => {
                    let b = self.pop_int()?;
                    let a = self.pop_int()?;
                    self.stack.push(Value::bool(a == b));
                }
                Op::NeqInt => {
                    let b = self.pop_int()?;
                    let a = self.pop_int()?;
                    self.stack.push(Value::bool(a != b));
                }
                Op::MulInt => {
                    let b = self.pop_int()?;
                    let a = self.pop_int()?;
                    self.stack.push(Value::int(a * b));
                }
                Op::DivInt => {
                    let b = self.pop_int()?;
                    if b == 0 { return Err("Division by zero".to_string()); }
                    let a = self.pop_int()?;
                    self.stack.push(Value::int(a / b));
                }
                Op::ModInt => {
                    let b = self.pop_int()?;
                    if b == 0 { return Err("Modulo by zero".to_string()); }
                    let a = self.pop_int()?;
                    self.stack.push(Value::int(a % b));
                }
                Op::NegInt => {
                    let a = self.pop_int()?;
                    self.stack.push(Value::int(-a));
                }
                Op::JumpIfFalse => {
                    let offset = read_i32!();
                    let val = self.stack.last().ok_or("VM: stack underflow")?;
                    if !val.is_truthy() {
                        let cur_ip = ip!();
                        self.frames[frame_idx].ip = (cur_ip as i32 + offset) as usize;
                    }
                }
                Op::JumpIfTrue => {
                    let offset = read_i32!();
                    let val = self.stack.last().ok_or("VM: stack underflow")?;
                    if val.is_truthy() {
                        let cur_ip = ip!();
                        self.frames[frame_idx].ip = (cur_ip as i32 + offset) as usize;
                    }
                }
                Op::Jump => {
                    let offset = read_i32!();
                    let cur_ip = ip!();
                    self.frames[frame_idx].ip = (cur_ip as i32 + offset) as usize;
                }
                Op::Loop => {
                    let offset = read_i32!();
                    let cur_ip = ip!();
                    self.frames[frame_idx].ip = (cur_ip as i32 - offset) as usize;
                }
                Op::Pop => { self.stack.pop(); }
                Op::CallLocal => {
                    let target_chunk = read_u16!() as usize;
                    let argc = read_u8!();

                    self.call_depth += 1;
                    if self.call_depth > 10000 {
                        return Err("maximum recursion depth exceeded (limit: 10000)".to_string());
                    }

                    // JIT: check if this function is hot and JIT'd
                    {
                        let jit_ptr = if let Some(ref mut jit) = self.jit {
                            jit.check_and_compile(target_chunk, &self.chunks)
                        } else {
                            None
                        };
                        if let Some(ptr) = jit_ptr
                            && (1..=8).contains(&argc) {
                                let mut raw_args = Vec::with_capacity(argc as usize);
                                let start = self.stack.len() - argc as usize;
                                for i in 0..argc as usize {
                                    raw_args.push(self.stack[start + i].raw());
                                }
                                for _ in 0..argc {
                                    let v = self.stack.pop().ok_or("VM: stack underflow")?;
                                    std::mem::forget(v);
                                }
                                let vm_ptr = self as *mut VM;
                                let interp_ptr = interp as *mut Interpreter;
                                let chunks_ptr = &self.chunks as *const Vec<Chunk>;
                                crate::vm::jit::set_jit_context(vm_ptr, interp_ptr, chunks_ptr, target_chunk);
                                let result_raw = unsafe { self.jit.as_ref().ok_or("VM: JIT not initialized")?.call_jit_fn(ptr, &raw_args, self.call_depth as u64) };
                                self.call_depth -= 1;
                                if let Some(err_msg) = self.last_error.take() {
                                    // Same error handling as Op::Throw
                                    if let Some(tp) = self.try_stack.pop() {
                                        self.last_error = Some(err_msg);
                                        while self.frames.len() > tp.frame_idx + 1 {
                                            self.frames.pop();
                                        }
                                        frame_idx = tp.frame_idx;
                                        self.stack.truncate(tp.stack_depth);
                                        self.frames[frame_idx].ip = tp.error_ip;
                                        continue;
                                    }
                                    return Err(err_msg);
                                }
                                self.stack.push(Value::from_raw(result_raw));
                                continue;
                            }
                    }

                    // Pad missing optional params with Void
                    let expected = self.chunks[target_chunk].param_count as usize;
                    let actual = argc as usize;
                    for _ in actual..expected {
                        self.stack.push(Value::void());
                    }

                    let new_base = self.stack.len() - expected;
                    self.frames.push(CallFrame {
                        chunk_idx: target_chunk,
                        ip: 0,
                        base: new_base,
                    });
                    frame_idx += 1;
                }
                Op::Return => {
                    let val = self.stack.pop().ok_or("VM: stack underflow on return")?;
                    let old_base = base!();
                    self.frames.pop();
                    if self.call_depth > 0 { self.call_depth -= 1; }
                    frame_idx -= 1;
                    self.stack.truncate(old_base);
                    if self.frames.is_empty() {
                        return Ok(());
                    }
                    self.stack.push(val);
                }
                Op::ReturnVoid => {
                    let old_base = base!();
                    self.frames.pop();
                    if self.call_depth > 0 { self.call_depth -= 1; }
                    if self.frames.is_empty() {
                        return Ok(());
                    }
                    frame_idx -= 1;
                    self.stack.truncate(old_base);
                    self.stack.push(Value::void());
                }

                // ============================================================
                // GENERIC ARITHMETIC (with int fast path)
                // ============================================================
                Op::Add => {
                    let len = self.stack.len();
                    if len >= 2 && let (Some(a), Some(b)) = (self.stack[len-2].as_int(), self.stack[len-1].as_int()) {
                        let r = a + b;
                        self.stack.pop();
                        if let Some(v) = self.stack.last_mut() { *v = Value::int(r); }
                        continue;
                    }
                    // String fast path: take left operand to get sole ownership for in-place append
                    if len >= 2 && self.stack[len-2].is_string() && self.stack[len-1].is_string() {
                        let b = self.stack.pop().ok_or("VM: stack underflow")?;
                        let mut a = std::mem::replace(self.stack.last_mut().ok_or("VM: stack underflow")?, Value::void());
                        if let Some(b_str) = b.as_str_ref() {
                            if a.try_string_append_in_place(b_str) {
                                *self.stack.last_mut().ok_or("VM: stack underflow")? = a;
                                continue;
                            }
                            if let Some(a_str) = a.as_str_ref() {
                                let mut s = String::with_capacity(a_str.len() + b_str.len());
                                s.push_str(a_str);
                                s.push_str(b_str);
                                *self.stack.last_mut().ok_or("VM: stack underflow")? = Value::string_owned(s);
                                continue;
                            }
                        }
                        *self.stack.last_mut().ok_or("VM: stack underflow")? = generic_add(a, b)?;
                        continue;
                    }
                    self.binary_op(generic_add)?;
                }
                Op::Sub => {
                    let len = self.stack.len();
                    if len >= 2 && let (Some(a), Some(b)) = (self.stack[len-2].as_int(), self.stack[len-1].as_int()) {
                        let r = a - b;
                        self.stack.pop();
                        if let Some(v) = self.stack.last_mut() { *v = Value::int(r); }
                        continue;
                    }
                    self.binary_op(generic_sub)?;
                }
                Op::Mul => {
                    let len = self.stack.len();
                    if len >= 2 && let (Some(a), Some(b)) = (self.stack[len-2].as_int(), self.stack[len-1].as_int()) {
                        self.stack.pop();
                        if let Some(v) = self.stack.last_mut() { *v = Value::int(a * b); }
                        continue;
                    }
                    self.binary_op(generic_mul)?;
                }
                Op::Div => { self.binary_op(generic_div)?; }
                Op::Mod => { self.binary_op(generic_mod)?; }
                Op::Pow => { self.binary_op(generic_pow)?; }
                Op::Neg => {
                    let val = self.stack.pop().ok_or("VM: stack underflow")?;
                    if let Some(n) = val.as_int() {
                        self.stack.push(Value::int(-n));
                    } else if let Some(f) = val.as_float() {
                        self.stack.push(Value::float(-f));
                    } else {
                        return Err(format!("Cannot negate {}", val.type_name()));
                    }
                }

                // ============================================================
                // GENERIC COMPARISON (with int fast path)
                // ============================================================
                Op::Eq => { self.binary_op(|a, b| Ok(Value::bool(values_equal(&a, &b))))?; }
                Op::Neq => { self.binary_op(|a, b| Ok(Value::bool(!values_equal(&a, &b))))?; }
                Op::Lt => {
                    let len = self.stack.len();
                    if len >= 2 && let (Some(a), Some(b)) = (self.stack[len-2].as_int(), self.stack[len-1].as_int()) {
                        self.stack.pop();
                        if let Some(v) = self.stack.last_mut() { *v = Value::bool(a < b); }
                        continue;
                    }
                    self.binary_op(|a, b| generic_compare(a, b, |o| o.is_lt()))?;
                }
                Op::Gt => {
                    let len = self.stack.len();
                    if len >= 2 && let (Some(a), Some(b)) = (self.stack[len-2].as_int(), self.stack[len-1].as_int()) {
                        self.stack.pop();
                        if let Some(v) = self.stack.last_mut() { *v = Value::bool(a > b); }
                        continue;
                    }
                    self.binary_op(|a, b| generic_compare(a, b, |o| o.is_gt()))?;
                }
                Op::Lte => {
                    let len = self.stack.len();
                    if len >= 2 && let (Some(a), Some(b)) = (self.stack[len-2].as_int(), self.stack[len-1].as_int()) {
                        self.stack.pop();
                        if let Some(v) = self.stack.last_mut() { *v = Value::bool(a <= b); }
                        continue;
                    }
                    self.binary_op(|a, b| generic_compare(a, b, |o| o.is_le()))?;
                }
                Op::Gte => {
                    let len = self.stack.len();
                    if len >= 2 && let (Some(a), Some(b)) = (self.stack[len-2].as_int(), self.stack[len-1].as_int()) {
                        self.stack.pop();
                        if let Some(v) = self.stack.last_mut() { *v = Value::bool(a >= b); }
                        continue;
                    }
                    self.binary_op(|a, b| generic_compare(a, b, |o| o.is_ge()))?;
                }

                // ============================================================
                // LOGICAL / BITWISE
                // ============================================================
                Op::Not => {
                    let val = self.stack.pop().ok_or("VM: stack underflow")?;
                    self.stack.push(Value::bool(!val.is_truthy()));
                }
                Op::BitAnd => { let b = self.pop_int()?; let a = self.pop_int()?; self.stack.push(Value::int(a & b)); }
                Op::BitOr => { let b = self.pop_int()?; let a = self.pop_int()?; self.stack.push(Value::int(a | b)); }
                Op::BitXor => { let b = self.pop_int()?; let a = self.pop_int()?; self.stack.push(Value::int(a ^ b)); }
                Op::BitNot => { let a = self.pop_int()?; self.stack.push(Value::int(!a)); }
                Op::Shl => { let b = self.pop_int()?; let a = self.pop_int()?; self.stack.push(Value::int(a << b)); }
                Op::Shr => { let b = self.pop_int()?; let a = self.pop_int()?; self.stack.push(Value::int(a >> b)); }

                // ============================================================
                // INC/DEC
                // ============================================================
                Op::IncLocal => {
                    let slot = read_u16!() as usize;
                    let idx = base!() + slot;
                    if let Some(n) = self.stack[idx].as_int() {
                        self.stack[idx] = Value::int(n + 1);
                    }
                }
                Op::DecLocal => {
                    let slot = read_u16!() as usize;
                    let idx = base!() + slot;
                    if let Some(n) = self.stack[idx].as_int() {
                        self.stack[idx] = Value::int(n - 1);
                    }
                }
                Op::CompoundAddInt => {
                    let slot = read_u16!() as usize;
                    let rhs = self.pop_int()?;
                    let idx = base!() + slot;
                    if let Some(n) = self.stack[idx].as_int() {
                        self.stack[idx] = Value::int(n + rhs);
                    }
                }
                Op::CompoundSubInt => {
                    let slot = read_u16!() as usize;
                    let rhs = self.pop_int()?;
                    let idx = base!() + slot;
                    if let Some(n) = self.stack[idx].as_int() {
                        self.stack[idx] = Value::int(n - rhs);
                    }
                }
                Op::StringAppendLocal => {
                    let slot = read_u16!() as usize;
                    let rhs = self.stack.pop().ok_or("VM: stack underflow")?;
                    let idx = base!() + slot;
                    // String + String fast path: take value from slot for sole ownership
                    if self.stack[idx].is_string() && rhs.is_string()
                        && let Some(rhs_str) = rhs.as_str_ref() {
                            // Take value out of local slot to get refcount 1
                            let mut local_val = std::mem::replace(&mut self.stack[idx], Value::void());
                            if local_val.try_string_append_in_place(rhs_str) {
                                self.stack[idx] = local_val;
                                continue;
                            }
                            // Fallback: allocate with capacity
                            if let Some(a_str) = local_val.as_str_ref() {
                                let mut s = String::with_capacity(a_str.len() + rhs_str.len());
                                s.push_str(a_str);
                                s.push_str(rhs_str);
                                self.stack[idx] = Value::string_owned(s);
                                continue;
                            }
                            // Put it back if somehow not a string
                            self.stack[idx] = local_val;
                        }
                    // Fallback: int + int or other type combinations
                    if let (Some(a), Some(b)) = (self.stack[idx].as_int(), rhs.as_int()) {
                        self.stack[idx] = Value::int(a + b);
                    } else {
                        let a = std::mem::replace(&mut self.stack[idx], Value::void());
                        self.stack[idx] = generic_add(a, rhs)?;
                    }
                }
                Op::PostIncLocal => {
                    let slot = read_u16!() as usize;
                    let idx = base!() + slot;
                    let old = self.stack[idx].clone();
                    if let Some(n) = self.stack[idx].as_int() {
                        self.stack[idx] = Value::int(n + 1);
                    }
                    self.stack.push(old);
                }
                Op::PostDecLocal => {
                    let slot = read_u16!() as usize;
                    let idx = base!() + slot;
                    let old = self.stack[idx].clone();
                    if let Some(n) = self.stack[idx].as_int() {
                        self.stack[idx] = Value::int(n - 1);
                    }
                    self.stack.push(old);
                }
                Op::PreIncLocal => {
                    let slot = read_u16!() as usize;
                    let idx = base!() + slot;
                    if let Some(n) = self.stack[idx].as_int() {
                        self.stack[idx] = Value::int(n + 1);
                    }
                    self.stack.push(self.stack[base!() + slot].clone());
                }
                Op::PreDecLocal => {
                    let slot = read_u16!() as usize;
                    let idx = base!() + slot;
                    if let Some(n) = self.stack[idx].as_int() {
                        self.stack[idx] = Value::int(n - 1);
                    }
                    self.stack.push(self.stack[base!() + slot].clone());
                }

                // ============================================================
                // CONSTANTS
                // ============================================================
                Op::LoadFloat => {
                    let val = read_f64!();
                    self.stack.push(Value::float(val));
                }
                Op::LoadTrue => self.stack.push(Value::bool(true)),
                Op::LoadFalse => self.stack.push(Value::bool(false)),
                Op::LoadVoid => self.stack.push(Value::void()),
                Op::LoadConst => {
                    let idx = read_u16!();
                    let s = self.chunks[ci!()].constants.get(idx).clone();
                    self.stack.push(Value::string(Rc::from(s.as_ref())));
                }

                // ============================================================
                // GLOBALS
                // ============================================================
                Op::GetGlobal => {
                    let idx = read_u16!() as usize;
                    if idx >= self.globals.len() {
                        let name = if idx < self.global_names.len() {
                            self.global_names[idx].to_string()
                        } else {
                            format!("#{idx}")
                        };
                        return Err(format!("Undefined variable: '{name}'"));
                    }
                    let val = self.globals[idx].clone();
                    if val.is_void() {
                        let name = if idx < self.global_names.len() {
                            self.global_names[idx].to_string()
                        } else {
                            format!("#{idx}")
                        };
                        return Err(format!("Undefined variable: '{name}'"));
                    }
                    self.stack.push(val);
                }
                Op::SetGlobal => {
                    let idx = read_u16!() as usize;
                    if idx >= self.globals.len() {
                        self.globals.resize(idx + 1, Value::void());
                    }
                    let val = self.stack.pop().ok_or("VM: stack underflow")?;
                    // Type check: prevent reassigning to incompatible type
                    let existing = &self.globals[idx];
                    if !existing.is_void() && existing.type_name() != val.type_name()
                        && existing.type_name() != "void" && val.type_name() != "void"
                    {
                        let name = if idx < self.global_names.len() {
                            self.global_names[idx].to_string()
                        } else {
                            format!("#{idx}")
                        };
                        return Err(format!(
                            "Type mismatch: variable '{}' is {}, cannot assign {} (use 'free {}' first)",
                            name, existing.type_name(), val.type_name(), name
                        ));
                    }
                    self.globals[idx] = val;
                }

                // ============================================================
                // GENERIC CALLS
                // ============================================================
                Op::Call => {
                    let name_idx = read_u16!();
                    let argc = read_u8!();
                    let res_byte = read_u8!();
                    let name = self.chunks[ci!()].constants.get(name_idx).clone();

                    // Check if name is a local variable holding a lambda
                    let mut lambda_found = false;
                    if let Some(slot) = self.locals_lookup(&name) {
                        let val = self.stack[base!() + slot as usize].clone();
                        if let Some(data) = val.as_lambda() {
                            lambda_found = true;
                            let arg_start = self.stack.len() - argc as usize;
                            let call_args: Vec<Value> = if data.bound_args.is_empty() {
                                self.stack.drain(arg_start..).collect()
                            } else {
                                self.stack.truncate(arg_start);
                                data.bound_args.clone()
                            };
                            let resolution = match data.resolution {
                                1 => Resolution::OwnFirst,
                                2 => Resolution::SystemOnly,
                                _ => Resolution::Normal,
                            };
                            let lambda_name = data.name.clone();
                            match interp.call_resolved(&lambda_name, resolution, call_args) {
                                Ok(val) => self.stack.push(val),
                                Err(e) => {
                            if let Some(tp) = self.try_stack.pop() {
                                self.last_error = Some(e);
                                self.stack.truncate(tp.stack_depth);
                                frame_idx = tp.frame_idx;
                                self.frames[frame_idx].ip = tp.error_ip;
                                continue;
                            }
                            return Err(e);
                        }
                            }
                        }
                    }
                    if lambda_found { continue; }

                    // Also check globals for lambdas
                    let global_val = self.global_name_to_slot.get(name.as_ref())
                        .and_then(|&slot| self.globals.get(slot as usize))
                        .filter(|v| !v.is_void())
                        .cloned();
                    if let Some(val) = global_val
                        && let Some(data) = val.as_lambda() {
                            let arg_start = self.stack.len() - argc as usize;
                            let call_args: Vec<Value> = if data.bound_args.is_empty() {
                                self.stack.drain(arg_start..).collect()
                            } else {
                                self.stack.truncate(arg_start);
                                data.bound_args.clone()
                            };
                            let resolution = match data.resolution {
                                1 => Resolution::OwnFirst,
                                2 => Resolution::SystemOnly,
                                _ => Resolution::Normal,
                            };
                            let lambda_name = data.name.clone();
                            match interp.call_resolved(&lambda_name, resolution, call_args) {
                                Ok(val) => self.stack.push(val),
                                Err(e) => {
                            if let Some(tp) = self.try_stack.pop() {
                                self.last_error = Some(e);
                                self.stack.truncate(tp.stack_depth);
                                frame_idx = tp.frame_idx;
                                self.frames[frame_idx].ip = tp.error_ip;
                                continue;
                            }
                            return Err(e);
                        }
                            }
                            continue;
                        }

                    // Try fn_table (user function)
                    if let Some(&fn_chunk) = self.fn_table.get(name.as_ref()) {
                        let new_base = self.stack.len() - argc as usize;
                        self.frames.push(CallFrame {
                            chunk_idx: fn_chunk,
                            ip: 0,
                            base: new_base,
                        });
                        frame_idx += 1;
                        continue;
                    }

                    // Fall through to interpreter
                    let arg_start = self.stack.len() - argc as usize;
                    let args: Vec<Value> = self.stack.drain(arg_start..).collect();
                    let resolution = match res_byte {
                        1 => Resolution::OwnFirst,
                        2 => Resolution::SystemOnly,
                        _ => Resolution::Normal,
                    };
                    match interp.call_resolved(&name, resolution, args) {
                        Ok(val) => self.stack.push(val),
                        Err(e) => {
                            if let Some(tp) = self.try_stack.pop() {
                                self.last_error = Some(e);
                                self.stack.truncate(tp.stack_depth);
                                frame_idx = tp.frame_idx;
                                self.frames[frame_idx].ip = tp.error_ip;
                                continue;
                            }
                            return Err(e);
                        }
                    }
                }
                Op::CallBuiltin => {
                    let name_idx = read_u16!();
                    let argc = read_u8!() as usize;
                    let name = self.chunks[ci!()].constants.get(name_idx).clone();
                    let arg_start = self.stack.len() - argc;
                    // Validate and get handler using stack slice directly — zero allocation
                    let handler = match interp.registry.validate_and_get_handler(&name, &self.stack[arg_start..]) {
                        Some(Ok(h)) => h,
                        Some(Err(e)) => {
                            self.stack.truncate(arg_start);
                            if let Some(tp) = self.try_stack.pop() {
                                self.last_error = Some(e);
                                self.stack.truncate(tp.stack_depth);
                                frame_idx = tp.frame_idx;
                                self.frames[frame_idx].ip = tp.error_ip;
                                continue;
                            }
                            return Err(e);
                        }
                        None => {
                            self.stack.truncate(arg_start);
                            return Err(format!("Undefined builtin: '{name}'"));
                        }
                    };
                    match handler(&self.stack[arg_start..], interp) {
                        Ok(val) => {
                            self.stack.truncate(arg_start);
                            self.stack.push(val);
                        }
                        Err(e) => {
                            self.stack.truncate(arg_start);
                            if let Some(tp) = self.try_stack.pop() {
                                self.last_error = Some(e);
                                self.stack.truncate(tp.stack_depth);
                                frame_idx = tp.frame_idx;
                                self.frames[frame_idx].ip = tp.error_ip;
                                continue;
                            }
                            return Err(e);
                        }
                    }
                }

                // ============================================================
                // COLLECTIONS
                // ============================================================
                Op::MakeList => {
                    let count = read_u16!() as usize;
                    let start = self.stack.len() - count;
                    let items: Vec<Value> = self.stack.drain(start..).collect();
                    self.stack.push(new_list(items));
                }
                Op::MakeObject => {
                    let count = read_u16!() as usize;
                    let mut map = indexmap::IndexMap::new();
                    let start = self.stack.len() - count * 2;
                    let pairs: Vec<Value> = self.stack.drain(start..).collect();
                    for pair in pairs.chunks(2) {
                        if let Some(k) = pair[0].as_str_ref() {
                            map.insert(k.to_string(), pair[1].clone());
                        }
                    }
                    self.stack.push(new_object(map));
                }
                Op::Index => {
                    let index = self.stack.pop().ok_or("VM: stack underflow")?;
                    let target = self.stack.pop().ok_or("VM: stack underflow")?;
                    self.stack.push(vm_index(&target, &index)?);
                }
                Op::FieldGet => {
                    let idx = read_u16!();
                    let field: &str = self.chunks[ci!()].constants.get(idx);
                    let obj = self.stack.pop().ok_or("VM: stack underflow")?;
                    if let Some(rc) = obj.as_object_ref() {
                        let val = rc.borrow().fields.get(field).cloned()
                            .ok_or_else(|| format!("Field '{field}' not found"))?;
                        self.stack.push(val);
                    } else if let Some(data) = obj.as_command_result() {
                        match field {
                            "status" => self.stack.push(Value::int(i64::from(data.status))),
                            "out" => self.stack.push(Value::string_from(&data.out)),
                            "err" => self.stack.push(Value::string_from(&data.err)),
                            _ => return Err(format!("CommandResult has no field '{field}'")),
                        }
                    } else {
                        return Err(format!("Cannot access field on {}", obj.type_name()));
                    }
                }
                Op::MakeString => {
                    let count = read_u16!() as usize;
                    let start = self.stack.len() - count;
                    // Build string directly from stack slice — no Vec allocation
                    let mut result = String::new();
                    for i in start..start + count {
                        use std::fmt::Write;
                        let _ = write!(result, "{}", self.stack[i]);
                    }
                    self.stack.truncate(start);
                    self.stack.push(Value::string_owned(result));
                }
                Op::MakeRange => {
                    let end = self.pop_int()?;
                    let start = self.pop_int()?;
                    let items: Vec<Value> = (start..=end).map(Value::int).collect();
                    self.stack.push(new_list(items));
                }

                // ============================================================
                // STATEMENTS
                // ============================================================
                Op::DefineFunction => {
                    let name_idx = read_u16!();
                    let fn_chunk_idx = read_u16!() as usize;
                    let name = self.chunks[ci!()].constants.get(name_idx).clone();
                    self.fn_table.insert(name.to_string(), fn_chunk_idx);
                    // Functions are pre-registered in interp.env by engine.rs run()
                    // so builtins (map, filter, etc.) can call them via the tree-walker.
                }
                Op::Free => {
                    let idx = read_u16!() as usize;
                    if idx < self.globals.len() {
                        self.globals[idx] = Value::void();
                    }
                }
                Op::Throw => {
                    let val = self.stack.pop().ok_or("VM: stack underflow")?;
                    let msg = format!("{val}");
                    if let Some(tp) = self.try_stack.pop() {
                        self.last_error = Some(msg);
                        // Unwind frames back to the try-point's frame
                        while self.frames.len() > tp.frame_idx + 1 {
                            let _old_base = self.frames.last().ok_or("VM: no call frame")?.base;
                            self.frames.pop();
                        }
                        frame_idx = tp.frame_idx;
                        self.stack.truncate(tp.stack_depth);
                        self.frames[frame_idx].ip = tp.error_ip;
                        continue;
                    }
                    return Err(msg);
                }
                Op::Dup => {
                    let val = self.stack.last().cloned().ok_or("VM: stack underflow")?;
                    self.stack.push(val);
                }
                Op::CheckCancel => {
                    if let Some(flag) = interp.cancel_flag
                        && flag.load(std::sync::atomic::Ordering::Relaxed) {
                            return Err("Cancelled".to_string());
                        }
                }

                // ============================================================
                // SUPERINSTRUCTIONS
                // ============================================================
                Op::SubLocalImm => {
                    let slot = read_u16!() as usize;
                    let imm = read_i64!();
                    let idx = base!() + slot;
                    if let Some(n) = self.stack[idx].as_int() {
                        self.stack.push(Value::int(n - imm));
                    } else {
                        return Err("SubLocalImm: expected int local".to_string());
                    }
                }
                Op::AddLocalImm => {
                    let slot = read_u16!() as usize;
                    let imm = read_i64!();
                    let idx = base!() + slot;
                    if let Some(n) = self.stack[idx].as_int() {
                        self.stack.push(Value::int(n + imm));
                    } else {
                        return Err("AddLocalImm: expected int local".to_string());
                    }
                }
                Op::BranchIfLocalGtImm => {
                    let slot = read_u16!() as usize;
                    let imm = read_i64!();
                    let offset = read_i32!();
                    let idx = base!() + slot;
                    if let Some(n) = self.stack[idx].as_int()
                        && n > imm {
                            let cur_ip = ip!();
                            self.frames[frame_idx].ip = (cur_ip as i32 + offset) as usize;
                        }
                }
                Op::BranchIfLocalLteImm => {
                    let slot = read_u16!() as usize;
                    let imm = read_i64!();
                    let offset = read_i32!();
                    let idx = base!() + slot;
                    if let Some(n) = self.stack[idx].as_int()
                        && n <= imm {
                            let cur_ip = ip!();
                            self.frames[frame_idx].ip = (cur_ip as i32 + offset) as usize;
                        }
                }
                Op::GetLocalInt => {
                    let slot = read_u16!() as usize;
                    let val = self.stack[base!() + slot].clone();
                    self.stack.push(val);
                }

                Op::IntToFloat => {
                    let len = self.stack.len();
                    if len > 0 {
                        if let Some(n) = self.stack[len - 1].as_int() {
                            self.stack[len - 1] = Value::float(n as f64);
                        }
                    }
                }

                // ============================================================
                // SEND OPERATOR
                // ============================================================
                Op::PushSendCtx => {
                    let val = self.stack.pop().ok_or("VM: stack underflow")?;
                    self.send_stack.push(Some(val));
                }
                Op::PopSendCtx => {
                    self.send_stack.pop();
                }
                Op::GetDollar => {
                    let val = self.send_stack.last()
                        .and_then(|v| v.clone())
                        .ok_or("$ used outside of send context")?;
                    self.stack.push(val);
                }
                Op::GetDollarIndex => {
                    let idx = read_u16!() as usize;
                    let send_val = self.send_stack.last()
                        .and_then(|v| v.clone())
                        .ok_or("$ used outside of send context")?;
                    if let Some(l) = send_val.as_list_ref() {
                        let val = l.borrow().get(idx).cloned()
                            .ok_or_else(|| format!("${idx} out of bounds"))?;
                        self.stack.push(val);
                    } else {
                        return Err(format!("${idx} requires a list, got {}", send_val.type_name()));
                    }
                }
                Op::GetDollarField => {
                    let field_idx = read_u16!();
                    let field = self.chunks[ci!()].constants.get(field_idx).clone();
                    let send_val = self.send_stack.last()
                        .and_then(|v| v.clone())
                        .ok_or("$ used outside of send context")?;
                    if let Some(rc) = send_val.as_object_ref() {
                        let val = rc.borrow().fields.get(field.as_ref()).cloned()
                            .ok_or_else(|| format!("${field} not found"))?;
                        self.stack.push(val);
                    } else if let Some(data) = send_val.as_command_result() {
                        match field.as_ref() {
                            "status" => self.stack.push(Value::int(i64::from(data.status))),
                            "out" => self.stack.push(Value::string_from(&data.out)),
                            "err" => self.stack.push(Value::string_from(&data.err)),
                            _ => return Err(format!("${field} not found on CommandResult")),
                        }
                    } else {
                        return Err(format!("${field} not found on {}", send_val.type_name()));
                    }
                }

                // ============================================================
                // LAMBDA
                // ============================================================
                Op::MakeLambda => {
                    let name_idx = read_u16!();
                    let res = read_u8!();
                    let bound_count = read_u8!() as usize;
                    let name = self.chunks[ci!()].constants.get(name_idx).clone();
                    let start = self.stack.len() - bound_count;
                    let bound_args: Vec<Value> = self.stack.drain(start..).collect();
                    self.stack.push(Value::lambda(crate::interpreter::value::LambdaData {
                        name: name.to_string(),
                        resolution: res,
                        bound_args,
                    }));
                }

                // ============================================================
                // ERROR HANDLING
                // ============================================================
                Op::ErrorCheck => {
                    let slot = read_u16!();
                    let is_ok = self.ok_vars.contains(&slot) ||
                        (!self.error_vars.contains_key(&slot) &&
                         self.globals.get(slot as usize).is_some_and(|v| !v.is_void()));
                    self.stack.push(Value::bool(is_ok));
                }
                Op::ErrorField => {
                    let slot = read_u16!();
                    let _field_idx = read_u16!();
                    let msg = self.error_vars.get(&slot)
                        .or(self.last_error.as_ref())
                        .cloned()
                        .unwrap_or_default();
                    self.stack.push(Value::string(Rc::from(msg)));
                }
                Op::SetErrorTolerant => {
                    // Mark variable as OK (no error) — called after successful assignment
                    let slot = read_u16!();
                    self.ok_vars.insert(slot);
                    self.error_vars.remove(&slot);
                }
                Op::RecordError => {
                    // Store last_error into error_vars for the named variable, then clear it
                    let slot = read_u16!();
                    if let Some(err) = self.last_error.take() {
                        self.error_vars.insert(slot, err);
                    }
                    self.ok_vars.remove(&slot);
                }
                Op::OptionalCheck => {
                    // Now only used for global variables (locals are resolved at compile time)
                    let slot = read_u16!() as usize;
                    let is_present = self.globals.get(slot)
                        .is_some_and(|v| !v.is_void());
                    self.stack.push(Value::bool(is_present));
                }

                // ============================================================
                // INDEX/FIELD ASSIGNMENT
                // ============================================================
                Op::IndexSet => {
                    let val = self.stack.pop().ok_or("VM: stack underflow")?;
                    let index = self.stack.pop().ok_or("VM: stack underflow")?;
                    let target = self.stack.pop().ok_or("VM: stack underflow")?;
                    if let (Some(l), Some(i)) = (target.as_list_ref(), index.as_int()) {
                        let mut list = l.borrow_mut();
                        let idx = if i < 0 { list.len() as i64 + i } else { i } as usize;
                        if idx < list.len() {
                            list[idx] = val;
                        } else {
                            return Err(format!("Index {i} out of bounds"));
                        }
                    } else if let (Some(rc), Some(key)) = (target.as_object_ref(), index.as_str_ref()) {
                        rc.borrow_mut().fields.insert(key.to_string(), val);
                    } else {
                        return Err(format!("Cannot index-assign {} with {}", target.type_name(), index.type_name()));
                    }
                }
                Op::FieldSet => {
                    let field_idx = read_u16!();
                    let field: &str = self.chunks[ci!()].constants.get(field_idx);
                    let val = self.stack.pop().ok_or("VM: stack underflow")?;
                    let target = self.stack.pop().ok_or("VM: stack underflow")?;
                    if let Some(rc) = target.as_object_ref() {
                        rc.borrow_mut().fields.insert(field.to_string(), val);
                    } else {
                        return Err(format!("Cannot field-assign on {}", target.type_name()));
                    }
                }

                // ============================================================
                // IMPORT / USE / ENUM / ALIAS / ATOMIC
                // ============================================================
                Op::Import => {
                    let path_idx = read_u16!();
                    let path = self.chunks[ci!()].constants.get(path_idx).clone();
                    let content = std::fs::read_to_string(path.as_ref())
                        .map_err(|e| format!("Cannot import '{}': {e}", path))?;
                    let mut lexer = crate::lexer::Lexer::new(&content);
                    let tokens = lexer.tokenize();
                    let stmts = crate::parser::parse(&tokens)?;

                    // Register functions in tree-walker env (for builtins like map/filter)
                    for stmt in &stmts {
                        if let crate::parser::ast::StmtKind::FnDef { name, params, optional_params, return_type_ann, body } = &stmt.kind {
                            interp.env.define_fn(crate::interpreter::env::UserFn {
                                name: name.clone(),
                                params: params.clone(),
                                optional_params: optional_params.clone(),
                                declared_return_type: return_type_ann.clone(),
                                has_dyn_return: false,
                                inferred_types: std::collections::HashMap::new(),
                                body_inferred_params: std::collections::HashSet::new(),
                                body: body.clone(),
                                return_type: None,
                            });
                        }
                    }

                    let mut sub_chunks = crate::vm::compiler::Compiler::compile(&stmts)?;
                    let base_idx = self.chunks.len();

                    // Build a mapping from sub-script global slots → main VM global slots
                    let mut slot_remap: std::collections::HashMap<u16, u16> = std::collections::HashMap::new();
                    if !sub_chunks.is_empty() {
                        for (name, &sub_slot) in &sub_chunks[0].global_slots {
                            let main_slot = if let Some(&existing) = self.global_name_to_slot.get(name) {
                                existing
                            } else {
                                let new_slot = self.global_names.len() as u16;
                                self.global_names.push(Rc::from(name.as_str()));
                                self.global_name_to_slot.insert(name.clone(), new_slot);
                                self.globals.push(Value::void());
                                new_slot
                            };
                            slot_remap.insert(sub_slot, main_slot);
                        }
                    }

                    // Patch all chunk-index references and global slot indices in sub-chunks
                    for chunk in &mut sub_chunks {
                        patch_chunk_indices(&mut chunk.code, base_idx as u16);
                        patch_global_slots(&mut chunk.code, &slot_remap);
                    }

                    // Register function chunks in fn_table
                    for (i, chunk) in sub_chunks.iter().enumerate() {
                        if i > 0 && !chunk.name.is_empty() {
                            self.fn_table.insert(chunk.name.to_ascii_lowercase(), base_idx + i);
                        }
                    }

                    self.chunks.extend(sub_chunks);
                    self.frames.push(CallFrame { chunk_idx: base_idx, ip: 0, base: self.stack.len() });
                    frame_idx += 1;
                }
                Op::Use => {
                    let path_idx = read_u16!();
                    let alias_idx = read_u16!();
                    let path = self.chunks[ci!()].constants.get(path_idx).clone();
                    let alias = if alias_idx == 0xFFFF {
                        // Derive name from path
                        std::path::Path::new(path.as_ref())
                            .file_stem()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_else(|| path.to_string())
                    } else {
                        self.chunks[ci!()].constants.get(alias_idx).to_string()
                    };
                    interp.env.use_paths.insert(alias.to_ascii_lowercase(), path.to_string());
                }
                Op::DefineEnum => {
                    let global_slot = read_u16!() as usize;
                    let variant_count = read_u16!() as usize;
                    let mut map = indexmap::IndexMap::new();
                    for i in 0..variant_count {
                        let vidx = read_u16!();
                        let vname = self.chunks[ci!()].constants.get(vidx).clone();
                        map.insert(vname.to_string(), Value::int(i as i64));
                    }
                    if global_slot >= self.globals.len() {
                        self.globals.resize(global_slot + 1, Value::void());
                    }
                    self.globals[global_slot] = new_object(map);
                }
                Op::Alias => {
                    let name_idx = read_u16!();
                    let target_idx = read_u16!();
                    let name = self.chunks[ci!()].constants.get(name_idx).clone();
                    let target = self.chunks[ci!()].constants.get(target_idx).clone();
                    interp.env.aliases.insert(name.to_string(), target.to_string());
                }
                Op::Atomic => {
                    let val = self.stack.pop().ok_or("VM: stack underflow")?;
                    self.stack.push(Value::atomic(crate::interpreter::value::AtomicValue::new(&val)));
                }
                Op::TryBegin => {
                    let offset = read_i32!();
                    let cur_ip = ip!();
                    let error_ip = (cur_ip as i32 + offset) as usize;
                    self.try_stack.push(TryPoint {
                        frame_idx,
                        error_ip,
                        stack_depth: self.stack.len(),
                        _error_msg: None,
                    });
                }
                Op::TryEnd => {
                    let offset = read_i32!();
                    let cur_ip = ip!();
                    self.frames[frame_idx].ip = (cur_ip as i32 + offset) as usize;
                    self.try_stack.pop();
                }
                Op::PushScope => {}
                Op::PopScope => {}
            }
        }
    }

    /// Look up a local variable by name in the current chunk's locals metadata.
    fn locals_lookup(&self, name: &str) -> Option<u16> {
        let chunk_idx = self.frames.last()?.chunk_idx;
        let chunk = &self.chunks[chunk_idx];
        for local in &chunk.locals {
            if local.name == name {
                return Some(local.slot);
            }
        }
        None
    }

    #[inline(always)]
    fn pop_int(&mut self) -> Result<i64, String> {
        match self.stack.pop() {
            Some(ref val) => val.as_int().ok_or_else(|| format!("Expected int, got {}", val.type_name())),
            None => Err("VM: stack underflow".to_string()),
        }
    }

    fn binary_op(&mut self, f: impl FnOnce(Value, Value) -> Result<Value, String>) -> Result<(), String> {
        let b = self.stack.pop().ok_or("VM: stack underflow")?;
        let a = self.stack.pop().ok_or("VM: stack underflow")?;
        self.stack.push(f(a, b)?);
        Ok(())
    }

    // --- Public helpers for JIT extern "C" functions ---

    pub(crate) fn get_global(&self, idx: usize) -> Value {
        if idx < self.globals.len() {
            self.globals[idx].clone()
        } else {
            Value::void()
        }
    }

    pub(crate) fn set_global(&mut self, idx: usize, val: Value) {
        if idx >= self.globals.len() {
            self.globals.resize(idx + 1, Value::void());
        }
        // Type check (same as Op::SetGlobal inline)
        let existing = &self.globals[idx];
        if !existing.is_void() && existing.type_name() != val.type_name()
            && existing.type_name() != "void" && val.type_name() != "void"
        {
            // Silently reject — JIT callers can't propagate errors easily
            // The type system should catch this at compile time in the future
            return;
        }
        self.globals[idx] = val;
    }

    pub(crate) fn fn_table_lookup(&self, name: &str) -> Option<&usize> {
        self.fn_table.get(name)
    }

    pub(crate) fn register_fn(&mut self, name: &str, chunk_idx: usize) {
        self.fn_table.insert(name.to_string(), chunk_idx);
    }

    pub(crate) fn error_check(&self, slot: u16) -> bool {
        self.ok_vars.contains(&slot) ||
            (!self.error_vars.contains_key(&slot) &&
             self.globals.get(slot as usize).is_some_and(|v| !v.is_void()))
    }

    pub(crate) fn error_field(&self, slot: u16) -> String {
        self.error_vars.get(&slot)
            .or(self.last_error.as_ref())
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn set_error_tolerant(&mut self, slot: u16) {
        self.ok_vars.insert(slot);
        self.error_vars.remove(&slot);
    }

    pub(crate) fn record_error(&mut self, slot: u16) {
        if let Some(ref err) = self.last_error {
            self.error_vars.insert(slot, err.clone());
        }
        self.ok_vars.remove(&slot);
    }

    pub(crate) fn optional_check(&self, slot: usize) -> bool {
        self.globals.get(slot).is_some_and(|v| !v.is_void())
    }

    pub(crate) fn get_dollar_index(&self, idx: usize) -> Option<Value> {
        let send_val = self.send_stack.last()?.clone()?;
        if let Some(l) = send_val.as_list_ref() {
            l.borrow().get(idx).cloned()
        } else {
            None
        }
    }

    pub(crate) fn get_dollar_field(&self, field: &str) -> Option<Value> {
        let send_val = self.send_stack.last()?.clone()?;
        if let Some(rc) = send_val.as_object_ref() {
            rc.borrow().fields.get(field).cloned()
        } else if let Some(data) = send_val.as_command_result() {
            match field {
                "status" => Some(Value::int(i64::from(data.status))),
                "out" => Some(Value::string_from(&data.out)),
                "err" => Some(Value::string_from(&data.err)),
                _ => None,
            }
        } else {
            None
        }
    }

    pub(crate) fn get_dollar(&self) -> Option<Value> {
        self.send_stack.last()?.clone()
    }

    pub(crate) fn push_send_ctx(&mut self, val: Value) {
        self.send_stack.push(Some(val));
    }

    pub(crate) fn pop_send_ctx(&mut self) {
        self.send_stack.pop();
    }
}

// --- Helpers ---

pub(crate) fn values_equal(a: &Value, b: &Value) -> bool {
    match (a.kind(), b.kind()) {
        (VK::Int(x), VK::Int(y)) => x == y,
        (VK::Float(x), VK::Float(y)) => (x - y).abs() < f64::EPSILON,
        (VK::String(x), VK::String(y)) => x == y,
        (VK::Bool(x), VK::Bool(y)) => x == y,
        (VK::Void, VK::Void) => true,
        _ => false,
    }
}

pub(crate) fn generic_add(mut a: Value, b: Value) -> Result<Value, String> {
    // Fast path: try in-place string append when left operand is uniquely owned
    if a.is_string() && b.is_string()
        && let Some(b_str) = b.as_str_ref() {
            if a.try_string_append_in_place(b_str) {
                return Ok(a);
            }
            // Fallback: allocate new string with pre-sized capacity
            if let Some(a_str) = a.as_str_ref() {
                let mut s = String::with_capacity(a_str.len() + b_str.len());
                s.push_str(a_str);
                s.push_str(b_str);
                return Ok(Value::string_owned(s));
            }
        }
    match (a.kind(), b.kind()) {
        (VK::Int(x), VK::Int(y)) => Ok(Value::int(x + y)),
        (VK::Float(x), VK::Float(y)) => Ok(Value::float(x + y)),
        (VK::Int(x), VK::Float(y)) => Ok(Value::float(x as f64 + y)),
        (VK::Float(x), VK::Int(y)) => Ok(Value::float(x + y as f64)),
        (VK::String(x), VK::String(y)) => {
            let mut s = String::with_capacity(x.len() + y.len());
            s.push_str(x);
            s.push_str(y);
            Ok(Value::string_owned(s))
        }
        _ => Err(format!("Cannot add {} and {}", a.type_name(), b.type_name())),
    }
}

pub(crate) fn generic_sub(a: Value, b: Value) -> Result<Value, String> {
    match (a.kind(), b.kind()) {
        (VK::Int(x), VK::Int(y)) => Ok(Value::int(x - y)),
        (VK::Float(x), VK::Float(y)) => Ok(Value::float(x - y)),
        (VK::Int(x), VK::Float(y)) => Ok(Value::float(x as f64 - y)),
        (VK::Float(x), VK::Int(y)) => Ok(Value::float(x - y as f64)),
        _ => Err(format!("Cannot subtract {} from {}", b.type_name(), a.type_name())),
    }
}

pub(crate) fn generic_mul(a: Value, b: Value) -> Result<Value, String> {
    match (a.kind(), b.kind()) {
        (VK::Int(x), VK::Int(y)) => Ok(Value::int(x * y)),
        (VK::Float(x), VK::Float(y)) => Ok(Value::float(x * y)),
        (VK::Int(x), VK::Float(y)) => Ok(Value::float(x as f64 * y)),
        (VK::Float(x), VK::Int(y)) => Ok(Value::float(x * y as f64)),
        _ => Err(format!("Cannot multiply {} and {}", a.type_name(), b.type_name())),
    }
}

pub(crate) fn generic_div(a: Value, b: Value) -> Result<Value, String> {
    match (a.kind(), b.kind()) {
        (VK::Int(x), VK::Int(y)) => {
            if y == 0 { return Err("Division by zero".to_string()); }
            Ok(Value::int(x / y))
        }
        (VK::Float(x), VK::Float(y)) => Ok(Value::float(x / y)),
        (VK::Int(x), VK::Float(y)) => Ok(Value::float(x as f64 / y)),
        (VK::Float(x), VK::Int(y)) => Ok(Value::float(x / y as f64)),
        _ => Err(format!("Cannot divide {} by {}", a.type_name(), b.type_name())),
    }
}

pub(crate) fn generic_mod(a: Value, b: Value) -> Result<Value, String> {
    match (a.kind(), b.kind()) {
        (VK::Int(x), VK::Int(y)) => {
            if y == 0 { return Err("Modulo by zero".to_string()); }
            Ok(Value::int(x % y))
        }
        _ => Err(format!("Cannot modulo {} by {}", a.type_name(), b.type_name())),
    }
}

fn generic_pow(a: Value, b: Value) -> Result<Value, String> {
    match (a.kind(), b.kind()) {
        (VK::Int(base), VK::Int(exp)) => {
            if let Ok(e) = u32::try_from(exp) {
                Ok(Value::int(base.pow(e)))
            } else {
                Ok(Value::float((base as f64).powf(exp as f64)))
            }
        }
        (VK::Float(x), VK::Float(y)) => Ok(Value::float(x.powf(y))),
        (VK::Int(x), VK::Float(y)) => Ok(Value::float((x as f64).powf(y))),
        (VK::Float(x), VK::Int(y)) => Ok(Value::float(x.powf(y as f64))),
        _ => Err(format!("Cannot exponentiate {} by {}", a.type_name(), b.type_name())),
    }
}

pub(crate) fn generic_compare(a: Value, b: Value, pred: impl FnOnce(std::cmp::Ordering) -> bool) -> Result<Value, String> {
    let ord = match (a.kind(), b.kind()) {
        (VK::Int(x), VK::Int(y)) => x.cmp(&y),
        (VK::Float(x), VK::Float(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        (VK::String(x), VK::String(y)) => x.cmp(y),
        _ => return Err(format!("Cannot compare {} and {}", a.type_name(), b.type_name())),
    };
    Ok(Value::bool(pred(ord)))
}

fn vm_index(target: &Value, index: &Value) -> Result<Value, String> {
    if let (Some(l), Some(i)) = (target.as_list_ref(), index.as_int()) {
        let list = l.borrow();
        let idx = if i < 0 { list.len() as i64 + i } else { i } as usize;
        list.get(idx).cloned().ok_or_else(|| format!("Index {i} out of bounds"))
    } else if let (Some(rc), Some(key)) = (target.as_object_ref(), index.as_str_ref()) {
        rc.borrow().fields.get(key).cloned()
            .ok_or_else(|| format!("Field '{key}' not found"))
    } else {
        Err(format!("Cannot index {} with {}", target.type_name(), index.type_name()))
    }
}

/// Remap global slot indices in bytecode using a slot_remap table.
fn patch_global_slots(code: &mut [u8], slot_remap: &std::collections::HashMap<u16, u16>) {
    if slot_remap.is_empty() { return; }
    fn remap(code: &mut [u8], pc: usize, slot_remap: &std::collections::HashMap<u16, u16>) {
        let old = u16::from_le_bytes([code[pc], code[pc+1]]);
        if let Some(&new) = slot_remap.get(&old) {
            code[pc..pc+2].copy_from_slice(&new.to_le_bytes());
        }
    }
    let mut pc = 0;
    while pc < code.len() {
        let op: Op = unsafe { std::mem::transmute(code[pc]) };
        pc += 1;
        match op {
            // Opcodes whose u16 operand is a global slot index
            Op::GetGlobal | Op::SetGlobal | Op::Free
            | Op::ErrorCheck | Op::SetErrorTolerant | Op::RecordError
            | Op::OptionalCheck => {
                remap(code, pc, slot_remap);
                pc += 2;
            }
            Op::ErrorField => {
                remap(code, pc, slot_remap); // first u16 is global slot
                pc += 4; // skip both u16s
            }
            Op::DefineEnum => {
                remap(code, pc, slot_remap); // first u16 is global slot
                pc += 2;
                let count = u16::from_le_bytes([code[pc], code[pc+1]]) as usize;
                pc += 2;
                pc += count * 2; // variant name indices (constant pool, not remapped)
            }
            // Skip operands for non-global opcodes (same sizes as patch_chunk_indices)
            Op::DefineFunction => pc += 4,
            Op::CallLocal => pc += 3,
            Op::LoadInt | Op::LoadFloat => pc += 8,
            Op::SubLocalImm | Op::AddLocalImm => pc += 10,
            Op::BranchIfLocalGtImm | Op::BranchIfLocalLteImm => pc += 14,
            Op::Jump | Op::JumpIfFalse | Op::JumpIfTrue | Op::Loop
            | Op::TryBegin | Op::TryEnd => pc += 4,
            Op::Call => pc += 4,
            Op::CallBuiltin => pc += 3,
            Op::MakeLambda => pc += 4,
            Op::Alias | Op::Use => pc += 4,
            Op::GetLocal | Op::SetLocal
            | Op::LoadConst | Op::FieldGet | Op::FieldSet | Op::MakeList
            | Op::MakeObject | Op::MakeString | Op::MakeRange
            | Op::IncLocal | Op::DecLocal | Op::PostIncLocal | Op::PostDecLocal
            | Op::PreIncLocal | Op::PreDecLocal | Op::CompoundAddInt | Op::CompoundSubInt
            | Op::StringAppendLocal
            | Op::GetDollarIndex | Op::GetDollarField
            | Op::Import | Op::GetLocalInt => pc += 2,
            Op::IntToFloat => {},
            _ => {} // 0-operand opcodes
        }
    }
}

/// Patch all chunk-index references (DefineFunction, CallLocal) by adding an offset.
fn patch_chunk_indices(code: &mut [u8], offset: u16) {
    let mut pc = 0;
    while pc < code.len() {
        let op: Op = unsafe { std::mem::transmute(code[pc]) };
        pc += 1;
        match op {
            Op::DefineFunction => {
                pc += 2; // skip name_idx
                let old = u16::from_le_bytes([code[pc], code[pc+1]]);
                let new = old + offset;
                code[pc..pc+2].copy_from_slice(&new.to_le_bytes());
                pc += 2;
            }
            Op::CallLocal => {
                let old = u16::from_le_bytes([code[pc], code[pc+1]]);
                let new = old + offset;
                code[pc..pc+2].copy_from_slice(&new.to_le_bytes());
                pc += 3; // u16 chunk + u8 argc
            }
            // Skip operands for other opcodes
            Op::LoadInt | Op::LoadFloat => pc += 8,
            Op::SubLocalImm | Op::AddLocalImm => pc += 10,
            Op::BranchIfLocalGtImm | Op::BranchIfLocalLteImm => pc += 14,
            Op::Jump | Op::JumpIfFalse | Op::JumpIfTrue | Op::Loop
            | Op::TryBegin | Op::TryEnd => pc += 4,
            Op::Call => pc += 4,
            Op::CallBuiltin => pc += 3,
            Op::MakeLambda => pc += 4,
            Op::ErrorField | Op::Alias | Op::Use => pc += 4,
            Op::DefineEnum => {
                pc += 2;
                let count = u16::from_le_bytes([code[pc], code[pc+1]]) as usize;
                pc += 2;
                pc += count * 2;
            }
            Op::GetLocal | Op::SetLocal | Op::GetGlobal | Op::SetGlobal
            | Op::LoadConst | Op::FieldGet | Op::FieldSet | Op::MakeList
            | Op::MakeObject | Op::MakeString | Op::MakeRange
            | Op::IncLocal | Op::DecLocal | Op::PostIncLocal | Op::PostDecLocal
            | Op::PreIncLocal | Op::PreDecLocal | Op::CompoundAddInt | Op::CompoundSubInt
            | Op::StringAppendLocal
            | Op::GetDollarIndex | Op::GetDollarField | Op::ErrorCheck
            | Op::OptionalCheck | Op::SetErrorTolerant | Op::RecordError | Op::Import | Op::Free
            | Op::GetLocalInt => pc += 2,
            _ => {} // 0-operand opcodes
        }
    }
}
