use std::rc::Rc;
use crate::interpreter::value::{Value, new_list, new_object};
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
    error_msg: Option<String>,
}

pub struct VM {
    pub(crate) stack: Vec<Value>,
    pub(crate) frames: Vec<CallFrame>,
    pub(crate) chunks: Vec<Chunk>,
    globals: std::collections::HashMap<String, Value>,
    error_vars: std::collections::HashMap<String, String>,
    ok_vars: std::collections::HashSet<String>,
    fn_table: std::collections::HashMap<String, usize>,
    send_stack: Vec<Option<Value>>,
    try_stack: Vec<TryPoint>,
    last_error: Option<String>,
    jit: Option<super::jit::JitManager>,
}

impl VM {
    pub fn new() -> Self {
        Self {
            stack: Vec::with_capacity(4096),
            frames: Vec::with_capacity(256),
            chunks: Vec::new(),
            globals: std::collections::HashMap::new(),
            error_vars: std::collections::HashMap::new(),
            ok_vars: std::collections::HashSet::new(),
            fn_table: std::collections::HashMap::new(),
            send_stack: Vec::new(),
            try_stack: Vec::new(),
            last_error: None,
            jit: None,
        }
    }

    pub fn execute(&mut self, chunks: Vec<Chunk>, interp: &mut Interpreter) -> Result<(), String> {
        let chunk_count = chunks.len();
        self.chunks = chunks;
        if interp.is_jit_mode() {
            self.jit = Some(super::jit::JitManager::new(chunk_count));
        }
        self.frames.push(CallFrame { chunk_idx: 0, ip: 0, base: 0 });
        self.run(interp)
    }

    /// Run a single frame and return its result value. Used by JIT helpers.
    pub(crate) fn run_frame(&mut self, interp: &mut Interpreter) -> Result<Value, String> {
        let target_depth = self.frames.len() - 1;
        self.run(interp)?;
        // The return value should be on the stack
        Ok(self.stack.pop().unwrap_or(Value::Void))
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
                self.stack.push(Value::Void);
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
                    self.stack.push(Value::Int(val));
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
                        self.stack.resize(idx + 1, Value::Void);
                    }
                    self.stack[idx] = val;
                }
                Op::AddInt => {
                    let b = self.pop_int()?;
                    let a = self.pop_int()?;
                    self.stack.push(Value::Int(a + b));
                }
                Op::SubInt => {
                    let b = self.pop_int()?;
                    let a = self.pop_int()?;
                    self.stack.push(Value::Int(a - b));
                }
                Op::LteInt => {
                    let b = self.pop_int()?;
                    let a = self.pop_int()?;
                    self.stack.push(Value::Bool(a <= b));
                }
                Op::LtInt => {
                    let b = self.pop_int()?;
                    let a = self.pop_int()?;
                    self.stack.push(Value::Bool(a < b));
                }
                Op::GtInt => {
                    let b = self.pop_int()?;
                    let a = self.pop_int()?;
                    self.stack.push(Value::Bool(a > b));
                }
                Op::GteInt => {
                    let b = self.pop_int()?;
                    let a = self.pop_int()?;
                    self.stack.push(Value::Bool(a >= b));
                }
                Op::EqInt => {
                    let b = self.pop_int()?;
                    let a = self.pop_int()?;
                    self.stack.push(Value::Bool(a == b));
                }
                Op::NeqInt => {
                    let b = self.pop_int()?;
                    let a = self.pop_int()?;
                    self.stack.push(Value::Bool(a != b));
                }
                Op::MulInt => {
                    let b = self.pop_int()?;
                    let a = self.pop_int()?;
                    self.stack.push(Value::Int(a * b));
                }
                Op::DivInt => {
                    let b = self.pop_int()?;
                    if b == 0 { return Err("Division by zero".to_string()); }
                    let a = self.pop_int()?;
                    self.stack.push(Value::Int(a / b));
                }
                Op::ModInt => {
                    let b = self.pop_int()?;
                    if b == 0 { return Err("Modulo by zero".to_string()); }
                    let a = self.pop_int()?;
                    self.stack.push(Value::Int(a % b));
                }
                Op::NegInt => {
                    let a = self.pop_int()?;
                    self.stack.push(Value::Int(-a));
                }
                Op::JumpIfFalse => {
                    let offset = read_i32!();
                    let val = self.stack.last().ok_or("VM: stack underflow")?;
                    if !is_truthy(val) {
                        let cur_ip = ip!();
                        self.frames[frame_idx].ip = (cur_ip as i32 + offset) as usize;
                    }
                }
                Op::JumpIfTrue => {
                    let offset = read_i32!();
                    let val = self.stack.last().ok_or("VM: stack underflow")?;
                    if is_truthy(val) {
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

                    // JIT: check if this function is hot and JIT'd
                    if let Some(ref mut jit) = self.jit {
                        if let Some(ptr) = jit.check_and_compile(target_chunk, &self.chunks) {
                            // Single int arg → call native
                            if argc == 1 {
                                if let Some(Value::Int(arg)) = self.stack.last() {
                                    let arg = *arg;
                                    self.stack.pop();
                                    let result = unsafe { jit.call_int_fn(ptr, arg) };
                                    self.stack.push(Value::Int(result));
                                    continue;
                                }
                            }
                        }
                    }

                    let new_base = self.stack.len() - argc as usize;
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
                    if self.frames.is_empty() {
                        return Ok(());
                    }
                    frame_idx -= 1;
                    self.stack.truncate(old_base);
                    self.stack.push(Value::Void);
                }

                // ============================================================
                // GENERIC ARITHMETIC (with int fast path)
                // ============================================================
                Op::Add => {
                    let len = self.stack.len();
                    if len >= 2 {
                        if let (Value::Int(a), Value::Int(b)) = (&self.stack[len-2], &self.stack[len-1]) {
                            let r = *a + *b;
                            self.stack.pop();
                            if let Some(v) = self.stack.last_mut() { *v = Value::Int(r); }
                            continue;
                        }
                    }
                    self.binary_op(|a, b| generic_add(a, b))?;
                }
                Op::Sub => {
                    let len = self.stack.len();
                    if len >= 2 {
                        if let (Value::Int(a), Value::Int(b)) = (&self.stack[len-2], &self.stack[len-1]) {
                            let r = *a - *b;
                            self.stack.pop();
                            if let Some(v) = self.stack.last_mut() { *v = Value::Int(r); }
                            continue;
                        }
                    }
                    self.binary_op(|a, b| generic_sub(a, b))?;
                }
                Op::Mul => {
                    let len = self.stack.len();
                    if len >= 2 {
                        if let (Value::Int(a), Value::Int(b)) = (&self.stack[len-2], &self.stack[len-1]) {
                            let r = *a * *b;
                            self.stack.pop();
                            if let Some(v) = self.stack.last_mut() { *v = Value::Int(r); }
                            continue;
                        }
                    }
                    self.binary_op(|a, b| generic_mul(a, b))?;
                }
                Op::Div => { self.binary_op(|a, b| generic_div(a, b))?; }
                Op::Mod => { self.binary_op(|a, b| generic_mod(a, b))?; }
                Op::Pow => { self.binary_op(|a, b| generic_pow(a, b))?; }
                Op::Neg => {
                    match self.stack.pop() {
                        Some(Value::Int(n)) => self.stack.push(Value::Int(-n)),
                        Some(Value::Float(n)) => self.stack.push(Value::Float(-n)),
                        Some(v) => return Err(format!("Cannot negate {}", v.type_name())),
                        None => return Err("VM: stack underflow".to_string()),
                    }
                }

                // ============================================================
                // GENERIC COMPARISON (with int fast path)
                // ============================================================
                Op::Eq => { self.binary_op(|a, b| Ok(Value::Bool(values_equal(&a, &b))))?; }
                Op::Neq => { self.binary_op(|a, b| Ok(Value::Bool(!values_equal(&a, &b))))?; }
                Op::Lt => {
                    let len = self.stack.len();
                    if len >= 2 {
                        if let (Value::Int(a), Value::Int(b)) = (&self.stack[len-2], &self.stack[len-1]) {
                            let r = *a < *b;
                            self.stack.pop();
                            if let Some(v) = self.stack.last_mut() { *v = Value::Bool(r); }
                            continue;
                        }
                    }
                    self.binary_op(|a, b| generic_compare(a, b, |o| o.is_lt()))?;
                }
                Op::Gt => {
                    let len = self.stack.len();
                    if len >= 2 {
                        if let (Value::Int(a), Value::Int(b)) = (&self.stack[len-2], &self.stack[len-1]) {
                            let r = *a > *b;
                            self.stack.pop();
                            if let Some(v) = self.stack.last_mut() { *v = Value::Bool(r); }
                            continue;
                        }
                    }
                    self.binary_op(|a, b| generic_compare(a, b, |o| o.is_gt()))?;
                }
                Op::Lte => {
                    let len = self.stack.len();
                    if len >= 2 {
                        if let (Value::Int(a), Value::Int(b)) = (&self.stack[len-2], &self.stack[len-1]) {
                            let r = *a <= *b;
                            self.stack.pop();
                            if let Some(v) = self.stack.last_mut() { *v = Value::Bool(r); }
                            continue;
                        }
                    }
                    self.binary_op(|a, b| generic_compare(a, b, |o| o.is_le()))?;
                }
                Op::Gte => {
                    let len = self.stack.len();
                    if len >= 2 {
                        if let (Value::Int(a), Value::Int(b)) = (&self.stack[len-2], &self.stack[len-1]) {
                            let r = *a >= *b;
                            self.stack.pop();
                            if let Some(v) = self.stack.last_mut() { *v = Value::Bool(r); }
                            continue;
                        }
                    }
                    self.binary_op(|a, b| generic_compare(a, b, |o| o.is_ge()))?;
                }

                // ============================================================
                // LOGICAL / BITWISE
                // ============================================================
                Op::Not => {
                    let val = self.stack.pop().ok_or("VM: stack underflow")?;
                    self.stack.push(Value::Bool(!is_truthy(&val)));
                }
                Op::BitAnd => { let b = self.pop_int()?; let a = self.pop_int()?; self.stack.push(Value::Int(a & b)); }
                Op::BitOr => { let b = self.pop_int()?; let a = self.pop_int()?; self.stack.push(Value::Int(a | b)); }
                Op::BitXor => { let b = self.pop_int()?; let a = self.pop_int()?; self.stack.push(Value::Int(a ^ b)); }
                Op::BitNot => { let a = self.pop_int()?; self.stack.push(Value::Int(!a)); }
                Op::Shl => { let b = self.pop_int()?; let a = self.pop_int()?; self.stack.push(Value::Int(a << b)); }
                Op::Shr => { let b = self.pop_int()?; let a = self.pop_int()?; self.stack.push(Value::Int(a >> b)); }

                // ============================================================
                // INC/DEC
                // ============================================================
                Op::IncLocal => {
                    let slot = read_u16!() as usize;
                    if let Value::Int(n) = &mut self.stack[base!() + slot] { *n += 1; }
                }
                Op::DecLocal => {
                    let slot = read_u16!() as usize;
                    if let Value::Int(n) = &mut self.stack[base!() + slot] { *n -= 1; }
                }
                Op::CompoundAddInt => {
                    let slot = read_u16!() as usize;
                    let rhs = self.pop_int()?;
                    if let Value::Int(n) = &mut self.stack[base!() + slot] { *n += rhs; }
                }
                Op::CompoundSubInt => {
                    let slot = read_u16!() as usize;
                    let rhs = self.pop_int()?;
                    if let Value::Int(n) = &mut self.stack[base!() + slot] { *n -= rhs; }
                }
                Op::PostIncLocal => {
                    let slot = read_u16!() as usize;
                    let old = self.stack[base!() + slot].clone();
                    if let Value::Int(n) = &mut self.stack[base!() + slot] { *n += 1; }
                    self.stack.push(old);
                }
                Op::PostDecLocal => {
                    let slot = read_u16!() as usize;
                    let old = self.stack[base!() + slot].clone();
                    if let Value::Int(n) = &mut self.stack[base!() + slot] { *n -= 1; }
                    self.stack.push(old);
                }
                Op::PreIncLocal => {
                    let slot = read_u16!() as usize;
                    if let Value::Int(n) = &mut self.stack[base!() + slot] { *n += 1; }
                    self.stack.push(self.stack[base!() + slot].clone());
                }
                Op::PreDecLocal => {
                    let slot = read_u16!() as usize;
                    if let Value::Int(n) = &mut self.stack[base!() + slot] { *n -= 1; }
                    self.stack.push(self.stack[base!() + slot].clone());
                }

                // ============================================================
                // CONSTANTS
                // ============================================================
                Op::LoadFloat => {
                    let val = read_f64!();
                    self.stack.push(Value::Float(val));
                }
                Op::LoadTrue => self.stack.push(Value::Bool(true)),
                Op::LoadFalse => self.stack.push(Value::Bool(false)),
                Op::LoadVoid => self.stack.push(Value::Void),
                Op::LoadConst => {
                    let idx = read_u16!();
                    let s = self.chunks[ci!()].constants.get(idx).clone();
                    self.stack.push(Value::String(s));
                }

                // ============================================================
                // GLOBALS
                // ============================================================
                Op::GetGlobal => {
                    let idx = read_u16!();
                    let name = self.chunks[ci!()].constants.get(idx).clone();
                    let val = self.globals.get(name.as_ref())
                        .cloned()
                        .ok_or_else(|| format!("Undefined variable: '{name}'"))?;
                    self.stack.push(val);
                }
                Op::SetGlobal => {
                    let idx = read_u16!();
                    let name = self.chunks[ci!()].constants.get(idx).clone();
                    let val = self.stack.pop().ok_or("VM: stack underflow")?;
                    self.globals.insert(name.to_string(), val);
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
                        if let Value::Lambda { name: fn_name, resolution, bound_args } = val {
                            lambda_found = true;
                            let arg_start = self.stack.len() - argc as usize;
                            let call_args: Vec<Value> = if bound_args.is_empty() {
                                self.stack.drain(arg_start..).collect()
                            } else {
                                self.stack.truncate(arg_start);
                                bound_args
                            };
                            let resolution = match resolution {
                                1 => Resolution::OwnFirst,
                                2 => Resolution::SystemOnly,
                                _ => Resolution::Normal,
                            };
                            match interp.call_resolved(&fn_name, resolution, call_args) {
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
                    if let Some(val) = self.globals.get(name.as_ref()).cloned() {
                        if let Value::Lambda { name: fn_name, resolution, bound_args } = val {
                            let arg_start = self.stack.len() - argc as usize;
                            let call_args: Vec<Value> = if bound_args.is_empty() {
                                self.stack.drain(arg_start..).collect()
                            } else {
                                self.stack.truncate(arg_start);
                                bound_args
                            };
                            let resolution = match resolution {
                                1 => Resolution::OwnFirst,
                                2 => Resolution::SystemOnly,
                                _ => Resolution::Normal,
                            };
                            match interp.call_resolved(&fn_name, resolution, call_args) {
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
                    let argc = read_u8!();
                    let name = self.chunks[ci!()].constants.get(name_idx).clone();
                    let arg_start = self.stack.len() - argc as usize;
                    let args: Vec<Value> = self.stack.drain(arg_start..).collect();
                    match interp.call_resolved(&name, Resolution::SystemOnly, args) {
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
                        if let Value::String(k) = &pair[0] {
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
                    let field = self.chunks[ci!()].constants.get(idx).clone();
                    let obj = self.stack.pop().ok_or("VM: stack underflow")?;
                    match &obj {
                        Value::Object(rc) => {
                            let val = rc.borrow().fields.get(field.as_ref()).cloned()
                                .ok_or_else(|| format!("Field '{field}' not found"))?;
                            self.stack.push(val);
                        }
                        Value::CommandResult { status, out, err } => {
                            match field.as_ref() {
                                "status" => self.stack.push(Value::Int(i64::from(*status))),
                                "out" => self.stack.push(Value::String(Rc::from(out.as_str()))),
                                "err" => self.stack.push(Value::String(Rc::from(err.as_str()))),
                                _ => return Err(format!("CommandResult has no field '{field}'")),
                            }
                        }
                        _ => return Err(format!("Cannot access field on {}", obj.type_name())),
                    }
                }
                Op::MakeString => {
                    let count = read_u16!() as usize;
                    let start = self.stack.len() - count;
                    let parts: Vec<Value> = self.stack.drain(start..).collect();
                    let mut result = String::new();
                    for part in &parts {
                        result.push_str(&format!("{part}"));
                    }
                    self.stack.push(Value::String(Rc::from(result)));
                }
                Op::MakeRange => {
                    let end = self.pop_int()?;
                    let start = self.pop_int()?;
                    let items: Vec<Value> = (start..=end).map(Value::Int).collect();
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
                    let idx = read_u16!();
                    let name = self.chunks[ci!()].constants.get(idx).clone();
                    self.globals.remove(name.as_ref());
                }
                Op::Throw => {
                    let val = self.stack.pop().ok_or("VM: stack underflow")?;
                    let msg = format!("{val}");
                    if let Some(tp) = self.try_stack.pop() {
                        self.last_error = Some(msg);
                        // Unwind frames back to the try-point's frame
                        while self.frames.len() > tp.frame_idx + 1 {
                            let old_base = self.frames.last().unwrap().base;
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
                    if let Some(flag) = interp.cancel_flag {
                        if flag.load(std::sync::atomic::Ordering::Relaxed) {
                            return Err("Cancelled".to_string());
                        }
                    }
                }

                // ============================================================
                // SUPERINSTRUCTIONS
                // ============================================================
                Op::SubLocalImm => {
                    let slot = read_u16!() as usize;
                    let imm = read_i64!();
                    if let Value::Int(n) = &self.stack[base!() + slot] {
                        self.stack.push(Value::Int(*n - imm));
                    } else {
                        return Err("SubLocalImm: expected int local".to_string());
                    }
                }
                Op::AddLocalImm => {
                    let slot = read_u16!() as usize;
                    let imm = read_i64!();
                    if let Value::Int(n) = &self.stack[base!() + slot] {
                        self.stack.push(Value::Int(*n + imm));
                    } else {
                        return Err("AddLocalImm: expected int local".to_string());
                    }
                }
                Op::BranchIfLocalGtImm => {
                    let slot = read_u16!() as usize;
                    let imm = read_i64!();
                    let offset = read_i32!();
                    if let Value::Int(n) = &self.stack[base!() + slot] {
                        if *n > imm {
                            let cur_ip = ip!();
                            self.frames[frame_idx].ip = (cur_ip as i32 + offset) as usize;
                        }
                    }
                }
                Op::BranchIfLocalLteImm => {
                    let slot = read_u16!() as usize;
                    let imm = read_i64!();
                    let offset = read_i32!();
                    if let Value::Int(n) = &self.stack[base!() + slot] {
                        if *n <= imm {
                            let cur_ip = ip!();
                            self.frames[frame_idx].ip = (cur_ip as i32 + offset) as usize;
                        }
                    }
                }
                Op::GetLocalInt => {
                    let slot = read_u16!() as usize;
                    let val = self.stack[base!() + slot].clone();
                    self.stack.push(val);
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
                    if let Value::List(l) = &send_val {
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
                    if let Value::Object(rc) = &send_val {
                        let val = rc.borrow().fields.get(field.as_ref()).cloned()
                            .ok_or_else(|| format!("${field} not found"))?;
                        self.stack.push(val);
                    } else if let Value::CommandResult { status, out, err } = &send_val {
                        match field.as_ref() {
                            "status" => self.stack.push(Value::Int(i64::from(*status))),
                            "out" => self.stack.push(Value::String(Rc::from(out.as_str()))),
                            "err" => self.stack.push(Value::String(Rc::from(err.as_str()))),
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
                    self.stack.push(Value::Lambda {
                        name: name.to_string(),
                        resolution: res,
                        bound_args,
                    });
                }

                // ============================================================
                // ERROR HANDLING
                // ============================================================
                Op::ErrorCheck => {
                    let name_idx = read_u16!();
                    let name = self.chunks[ci!()].constants.get(name_idx).clone();
                    let is_ok = self.ok_vars.contains(name.as_ref()) ||
                        (!self.error_vars.contains_key(name.as_ref()) &&
                         self.globals.get(name.as_ref()).map_or(false, |v| !matches!(v, Value::Void)));
                    self.stack.push(Value::Bool(is_ok));
                }
                Op::ErrorField => {
                    let name_idx = read_u16!();
                    let _field_idx = read_u16!();
                    let name = self.chunks[ci!()].constants.get(name_idx).clone();
                    let msg = self.error_vars.get(name.as_ref())
                        .or(self.last_error.as_ref())
                        .cloned()
                        .unwrap_or_default();
                    self.stack.push(Value::String(Rc::from(msg)));
                }
                Op::SetErrorTolerant => {
                    // Mark variable as OK (no error) — called after successful assignment
                    let name_idx = read_u16!();
                    let name = self.chunks[ci!()].constants.get(name_idx).clone();
                    self.ok_vars.insert(name.to_string());
                    self.error_vars.remove(name.as_ref());
                }
                Op::RecordError => {
                    // Store last_error into error_vars for the named variable
                    let name_idx = read_u16!();
                    let name = self.chunks[ci!()].constants.get(name_idx).clone();
                    if let Some(ref err) = self.last_error {
                        self.error_vars.insert(name.to_string(), err.clone());
                    }
                    self.ok_vars.remove(name.as_ref());
                }
                Op::OptionalCheck => {
                    let name_idx = read_u16!();
                    let name = self.chunks[ci!()].constants.get(name_idx).clone();
                    // Check if variable is Void (not provided)
                    if let Some(slot) = self.locals_lookup(&name) {
                        let val = &self.stack[base!() + slot as usize];
                        self.stack.push(Value::Bool(!matches!(val, Value::Void)));
                    } else if let Some(val) = self.globals.get(name.as_ref()) {
                        self.stack.push(Value::Bool(!matches!(val, Value::Void)));
                    } else {
                        self.stack.push(Value::Bool(false));
                    }
                }

                // ============================================================
                // INDEX/FIELD ASSIGNMENT
                // ============================================================
                Op::IndexSet => {
                    let val = self.stack.pop().ok_or("VM: stack underflow")?;
                    let index = self.stack.pop().ok_or("VM: stack underflow")?;
                    let target = self.stack.pop().ok_or("VM: stack underflow")?;
                    match (target, index) {
                        (Value::List(l), Value::Int(i)) => {
                            let mut list = l.borrow_mut();
                            let idx = if i < 0 { list.len() as i64 + i } else { i } as usize;
                            if idx < list.len() {
                                list[idx] = val;
                            } else {
                                return Err(format!("Index {i} out of bounds"));
                            }
                        }
                        (Value::Object(rc), Value::String(key)) => {
                            rc.borrow_mut().fields.insert(key.to_string(), val);
                        }
                        (t, i) => return Err(format!("Cannot index-assign {} with {}", t.type_name(), i.type_name())),
                    }
                }
                Op::FieldSet => {
                    let field_idx = read_u16!();
                    let field = self.chunks[ci!()].constants.get(field_idx).clone();
                    let val = self.stack.pop().ok_or("VM: stack underflow")?;
                    let target = self.stack.pop().ok_or("VM: stack underflow")?;
                    if let Value::Object(rc) = target {
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

                    // Patch all chunk-index references in sub-chunks by adding base_idx
                    for chunk in &mut sub_chunks {
                        patch_chunk_indices(&mut chunk.code, base_idx as u16);
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
                    let name_idx = read_u16!();
                    let variant_count = read_u16!() as usize;
                    let name = self.chunks[ci!()].constants.get(name_idx).clone();
                    let mut map = indexmap::IndexMap::new();
                    for i in 0..variant_count {
                        let vidx = read_u16!();
                        let vname = self.chunks[ci!()].constants.get(vidx).clone();
                        map.insert(vname.to_string(), Value::Int(i as i64));
                    }
                    self.globals.insert(name.to_string(), new_object(map));
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
                    self.stack.push(Value::Atomic(crate::interpreter::value::AtomicValue::new(&val)));
                }
                Op::TryBegin => {
                    let offset = read_i32!();
                    let cur_ip = ip!();
                    let error_ip = (cur_ip as i32 + offset) as usize;
                    self.try_stack.push(TryPoint {
                        frame_idx,
                        error_ip,
                        stack_depth: self.stack.len(),
                        error_msg: None,
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
            if local.name == name.as_ref() {
                return Some(local.slot);
            }
        }
        None
    }

    #[inline(always)]
    fn pop_int(&mut self) -> Result<i64, String> {
        match self.stack.pop() {
            Some(Value::Int(n)) => Ok(n),
            Some(other) => Err(format!("Expected int, got {}", other.type_name())),
            None => Err("VM: stack underflow".to_string()),
        }
    }

    fn binary_op(&mut self, f: impl FnOnce(Value, Value) -> Result<Value, String>) -> Result<(), String> {
        let b = self.stack.pop().ok_or("VM: stack underflow")?;
        let a = self.stack.pop().ok_or("VM: stack underflow")?;
        self.stack.push(f(a, b)?);
        Ok(())
    }
}

// --- Helpers ---

#[inline(always)]
fn is_truthy(val: &Value) -> bool {
    match val {
        Value::Bool(b) => *b,
        Value::Int(n) => *n != 0,
        Value::Float(n) => *n != 0.0,
        Value::String(s) => !s.is_empty(),
        Value::Void => false,
        _ => true,
    }
}

fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => (x - y).abs() < f64::EPSILON,
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Void, Value::Void) => true,
        _ => false,
    }
}

fn generic_add(a: Value, b: Value) -> Result<Value, String> {
    match (&a, &b) {
        (Value::Int(x), Value::Int(y)) => Ok(Value::Int(x + y)),
        (Value::Float(x), Value::Float(y)) => Ok(Value::Float(x + y)),
        (Value::Int(x), Value::Float(y)) => Ok(Value::Float(*x as f64 + y)),
        (Value::Float(x), Value::Int(y)) => Ok(Value::Float(x + *y as f64)),
        (Value::String(x), Value::String(y)) => Ok(Value::String(Rc::from(format!("{x}{y}")))),
        _ => Err(format!("Cannot add {} and {}", a.type_name(), b.type_name())),
    }
}

fn generic_sub(a: Value, b: Value) -> Result<Value, String> {
    match (&a, &b) {
        (Value::Int(x), Value::Int(y)) => Ok(Value::Int(x - y)),
        (Value::Float(x), Value::Float(y)) => Ok(Value::Float(x - y)),
        (Value::Int(x), Value::Float(y)) => Ok(Value::Float(*x as f64 - y)),
        (Value::Float(x), Value::Int(y)) => Ok(Value::Float(x - *y as f64)),
        _ => Err(format!("Cannot subtract {} from {}", b.type_name(), a.type_name())),
    }
}

fn generic_mul(a: Value, b: Value) -> Result<Value, String> {
    match (&a, &b) {
        (Value::Int(x), Value::Int(y)) => Ok(Value::Int(x * y)),
        (Value::Float(x), Value::Float(y)) => Ok(Value::Float(x * y)),
        (Value::Int(x), Value::Float(y)) => Ok(Value::Float(*x as f64 * y)),
        (Value::Float(x), Value::Int(y)) => Ok(Value::Float(x * *y as f64)),
        _ => Err(format!("Cannot multiply {} and {}", a.type_name(), b.type_name())),
    }
}

fn generic_div(a: Value, b: Value) -> Result<Value, String> {
    match (&a, &b) {
        (Value::Int(x), Value::Int(y)) => {
            if *y == 0 { return Err("Division by zero".to_string()); }
            Ok(Value::Int(x / y))
        }
        (Value::Float(x), Value::Float(y)) => Ok(Value::Float(x / y)),
        (Value::Int(x), Value::Float(y)) => Ok(Value::Float(*x as f64 / y)),
        (Value::Float(x), Value::Int(y)) => Ok(Value::Float(x / *y as f64)),
        _ => Err(format!("Cannot divide {} by {}", a.type_name(), b.type_name())),
    }
}

fn generic_mod(a: Value, b: Value) -> Result<Value, String> {
    match (&a, &b) {
        (Value::Int(x), Value::Int(y)) => {
            if *y == 0 { return Err("Modulo by zero".to_string()); }
            Ok(Value::Int(x % y))
        }
        _ => Err(format!("Cannot modulo {} by {}", a.type_name(), b.type_name())),
    }
}

fn generic_pow(a: Value, b: Value) -> Result<Value, String> {
    match (&a, &b) {
        (Value::Int(base), Value::Int(exp)) => {
            if let Ok(e) = u32::try_from(*exp) {
                Ok(Value::Int(base.pow(e)))
            } else {
                Ok(Value::Float((*base as f64).powf(*exp as f64)))
            }
        }
        (Value::Float(x), Value::Float(y)) => Ok(Value::Float(x.powf(*y))),
        (Value::Int(x), Value::Float(y)) => Ok(Value::Float((*x as f64).powf(*y))),
        (Value::Float(x), Value::Int(y)) => Ok(Value::Float(x.powf(*y as f64))),
        _ => Err(format!("Cannot exponentiate {} by {}", a.type_name(), b.type_name())),
    }
}

fn generic_compare(a: Value, b: Value, pred: impl FnOnce(std::cmp::Ordering) -> bool) -> Result<Value, String> {
    let ord = match (&a, &b) {
        (Value::Int(x), Value::Int(y)) => x.cmp(y),
        (Value::Float(x), Value::Float(y)) => x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal),
        (Value::String(x), Value::String(y)) => x.cmp(y),
        _ => return Err(format!("Cannot compare {} and {}", a.type_name(), b.type_name())),
    };
    Ok(Value::Bool(pred(ord)))
}

fn vm_index(target: &Value, index: &Value) -> Result<Value, String> {
    match (target, index) {
        (Value::List(l), Value::Int(i)) => {
            let list = l.borrow();
            let idx = if *i < 0 { list.len() as i64 + i } else { *i } as usize;
            list.get(idx).cloned().ok_or_else(|| format!("Index {i} out of bounds"))
        }
        (Value::Object(rc), Value::String(key)) => {
            rc.borrow().fields.get(key.as_ref()).cloned()
                .ok_or_else(|| format!("Field '{key}' not found"))
        }
        _ => Err(format!("Cannot index {} with {}", target.type_name(), index.type_name())),
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
            | Op::GetDollarIndex | Op::GetDollarField | Op::ErrorCheck
            | Op::OptionalCheck | Op::SetErrorTolerant | Op::RecordError | Op::Import | Op::Free
            | Op::GetLocalInt => pc += 2,
            _ => {} // 0-operand opcodes
        }
    }
}
