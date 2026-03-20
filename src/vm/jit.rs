use cranelift_codegen::ir::{types, AbiParam, Function, InstBuilder, UserFuncName};
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module};

use std::collections::{HashMap, HashSet};
use super::bytecode::{Chunk, Op};
use crate::interpreter::value::Value;
use crate::interpreter::Interpreter;

// ---------------------------------------------------------------------------
// extern "C" helpers — called from JIT'd code for non-int operations
// ---------------------------------------------------------------------------

/// Call a VM function by chunk index. Returns i64 result (for int functions).
unsafe extern "C" fn jit_vm_call(
    vm_ptr: *mut super::machine::VM,
    interp_ptr: *mut Interpreter,
    chunk_idx: usize,
    args_ptr: *const i64,
    argc: usize,
) -> i64 {
    unsafe {
        let vm = &mut *vm_ptr;
        let interp = &mut *interp_ptr;
        // Push args as Value::Int onto VM stack
        let args = std::slice::from_raw_parts(args_ptr, argc);
        let base = vm.stack.len();
        for &arg in args {
            vm.stack.push(Value::Int(arg));
        }
        vm.frames.push(super::machine::CallFrame {
            chunk_idx,
            ip: 0,
            base,
        });
        // Run the VM for this call
        match vm.run_frame(interp) {
            Ok(Value::Int(n)) => n,
            _ => 0,
        }
    }
}

/// Call a builtin by name. Args/result are Value pointers.
unsafe extern "C" fn jit_call_builtin(
    interp_ptr: *mut Interpreter,
    name_ptr: *const u8,
    name_len: usize,
    args_ptr: *const Value,
    argc: usize,
    result_ptr: *mut Value,
) -> i32 {
    unsafe {
        let interp = &mut *interp_ptr;
        let name = std::str::from_utf8_unchecked(std::slice::from_raw_parts(name_ptr, name_len));
        let args = std::slice::from_raw_parts(args_ptr, argc);

        // Temporarily take registry out
        let reg = std::mem::replace(
            &mut interp.registry,
            crate::builtins::registry::BuiltinRegistry::new(),
        );
        let result = reg.call(name, args, interp);
        interp.registry = reg;

        match result {
            Some(Ok(val)) => { *result_ptr = val; 0 }
            Some(Err(_)) => -1,
            None => -1,
        }
    }
}

// ---------------------------------------------------------------------------
// JIT Manager
// ---------------------------------------------------------------------------

pub struct JitManager {
    pub call_counts: Vec<u32>,
    pub compiled: Vec<Option<Option<*const u8>>>,
    pub threshold: u32,
    module: Option<JITModule>,
    builder_ctx: FunctionBuilderContext,
    /// Declared function references for recursive calls
    func_ids: HashMap<usize, cranelift_module::FuncId>,
}

unsafe impl Send for JitManager {}
unsafe impl Sync for JitManager {}

impl JitManager {
    pub fn new(chunk_count: usize) -> Self {
        let module = {
            let mut flag_builder = settings::builder();
            let _ = flag_builder.set("use_colocated_libcalls", "false");
            let _ = flag_builder.set("is_pic", "false");
            let isa = cranelift_native::builder()
                .ok()
                .and_then(|b| b.finish(settings::Flags::new(flag_builder)).ok());
            isa.map(|isa| {
                let mut builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
                // Register helper functions
                builder.symbol("jit_vm_call", jit_vm_call as *const u8);
                builder.symbol("jit_call_builtin", jit_call_builtin as *const u8);
                JITModule::new(builder)
            })
        };

        Self {
            call_counts: vec![0; chunk_count],
            compiled: vec![None; chunk_count],
            threshold: 50,
            module,
            builder_ctx: FunctionBuilderContext::new(),
            func_ids: HashMap::new(),
        }
    }

    pub fn check_and_compile(&mut self, chunk_idx: usize, chunks: &[Chunk]) -> Option<*const u8> {
        if chunk_idx >= self.call_counts.len() {
            self.call_counts.resize(chunk_idx + 1, 0);
            self.compiled.resize(chunk_idx + 1, None);
        }

        self.call_counts[chunk_idx] += 1;

        if let Some(ref result) = self.compiled[chunk_idx] {
            return *result;
        }

        if self.call_counts[chunk_idx] < self.threshold {
            return None;
        }

        let ptr = self.try_compile(chunk_idx, chunks);
        self.compiled[chunk_idx] = Some(ptr);
        ptr
    }

    fn try_compile(&mut self, chunk_idx: usize, chunks: &[Chunk]) -> Option<*const u8> {
        let module = self.module.as_mut()?;
        let chunk = &chunks[chunk_idx];

        if chunk.param_count == 0 || chunk.param_count > 8 {
            return None;
        }

        // Build signature: all params i64, return i64
        // (for now, only int-param functions get JIT'd — others fall back to VM)
        let mut sig = module.make_signature();
        for _ in 0..chunk.param_count {
            sig.params.push(AbiParam::new(types::I64));
        }
        sig.returns.push(AbiParam::new(types::I64));

        let func_name = format!("jit_{}", chunk_idx);
        let func_id = module.declare_function(&func_name, Linkage::Local, &sig).ok()?;
        self.func_ids.insert(chunk_idx, func_id);

        let mut func = Function::with_name_signature(UserFuncName::default(), sig.clone());
        let self_ref = module.declare_func_in_func(func_id, &mut func);

        let mut builder = FunctionBuilder::new(&mut func, &mut self.builder_ctx);
        let ok = GenericJitCompiler::compile(&mut builder, chunk, self_ref);
        builder.finalize();

        if !ok {
            return None;
        }

        let mut ctx = Context::for_function(func);
        module.define_function(func_id, &mut ctx).ok()?;
        module.clear_context(&mut ctx);
        module.finalize_definitions().ok()?;

        Some(module.get_finalized_function(func_id))
    }

    pub unsafe fn call_int_fn(&self, ptr: *const u8, arg: i64) -> i64 {
        unsafe {
            let func: unsafe extern "C" fn(i64) -> i64 = std::mem::transmute(ptr);
            func(arg)
        }
    }

    pub unsafe fn call_int_fn2(&self, ptr: *const u8, a: i64, b: i64) -> i64 {
        unsafe {
            let func: unsafe extern "C" fn(i64, i64) -> i64 = std::mem::transmute(ptr);
            func(a, b)
        }
    }
}

// ---------------------------------------------------------------------------
// Generic Bytecode-to-IR Compiler
// ---------------------------------------------------------------------------

struct GenericJitCompiler;

impl GenericJitCompiler {
    fn compile(
        builder: &mut FunctionBuilder,
        chunk: &Chunk,
        self_ref: cranelift_codegen::ir::FuncRef,
    ) -> bool {
        let code = &chunk.code;

        // --- Phase 1: Pre-scan for jump targets to create block boundaries ---
        let mut jump_targets: HashSet<usize> = HashSet::new();
        {
            let mut pc = 0;
            while pc < code.len() {
                let op: Op = unsafe { std::mem::transmute(code[pc]) };
                pc += 1;
                match op {
                    Op::Jump => {
                        let offset = chunk.read_i32(pc);
                        let target = (pc as i32 + 4 + offset) as usize;
                        jump_targets.insert(target);
                        pc += 4;
                    }
                    Op::JumpIfFalse | Op::JumpIfTrue => {
                        let offset = chunk.read_i32(pc);
                        let target = (pc as i32 + 4 + offset) as usize;
                        jump_targets.insert(target);
                        jump_targets.insert(pc + 4); // fall-through is also a target
                        pc += 4;
                    }
                    Op::Loop => {
                        let offset = chunk.read_i32(pc);
                        let target = (pc as i32 + 4 - offset) as usize;
                        jump_targets.insert(target);
                        pc += 4;
                    }
                    Op::BranchIfLocalGtImm | Op::BranchIfLocalLteImm => {
                        pc += 2 + 8; // slot + imm
                        let offset = chunk.read_i32(pc);
                        let target = (pc as i32 + 4 + offset) as usize;
                        jump_targets.insert(target);
                        jump_targets.insert(pc + 4);
                        pc += 4;
                    }
                    // Skip operands
                    Op::LoadInt | Op::LoadFloat => pc += 8,
                    Op::SubLocalImm | Op::AddLocalImm => pc += 10,
                    Op::Call => pc += 4,
                    Op::CallLocal | Op::CallBuiltin => pc += 3,
                    Op::MakeLambda => pc += 4,
                    Op::ErrorField | Op::Alias | Op::Use => pc += 4,
                    Op::DefineFunction => pc += 4,
                    Op::DefineEnum => {
                        pc += 2;
                        if pc + 1 < code.len() {
                            let count = u16::from_le_bytes([code[pc], code[pc+1]]) as usize;
                            pc += 2 + count * 2;
                        }
                    }
                    Op::TryBegin | Op::TryEnd => pc += 4,
                    Op::GetLocal | Op::SetLocal | Op::GetGlobal | Op::SetGlobal
                    | Op::LoadConst | Op::FieldGet | Op::FieldSet | Op::MakeList
                    | Op::MakeObject | Op::MakeString | Op::MakeRange
                    | Op::IncLocal | Op::DecLocal | Op::PostIncLocal | Op::PostDecLocal
                    | Op::PreIncLocal | Op::PreDecLocal | Op::CompoundAddInt | Op::CompoundSubInt
                    | Op::GetDollarIndex | Op::GetDollarField | Op::ErrorCheck
                    | Op::OptionalCheck | Op::SetErrorTolerant | Op::Import | Op::Free
                    | Op::GetLocalInt => pc += 2,
                    _ => {} // 0-operand
                }
            }
        }

        // --- Phase 2: Create Cranelift blocks for each jump target ---
        let mut block_map: HashMap<usize, cranelift_codegen::ir::Block> = HashMap::new();
        for &target in &jump_targets {
            block_map.insert(target, builder.create_block());
        }

        // Entry block
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);

        // Declare variables for locals
        let max_locals = chunk.locals.len().max(chunk.param_count as usize).max(16);
        let mut vars: Vec<Variable> = Vec::new();
        for i in 0..max_locals {
            let var = Variable::from_u32(i as u32);
            builder.declare_var(var, types::I64);
            vars.push(var);
        }

        // Init params
        for i in 0..chunk.param_count as usize {
            let param_val = builder.block_params(entry)[i];
            builder.def_var(vars[i], param_val);
        }
        let zero = builder.ins().iconst(types::I64, 0);
        for i in chunk.param_count as usize..max_locals {
            builder.def_var(vars[i], zero);
        }

        // Seal entry block (all predecessors known — just the function entry)
        builder.seal_block(entry);

        // Virtual operand stack
        let mut vstack: Vec<cranelift_codegen::ir::Value> = Vec::new();

        // --- Phase 3: Translate opcodes ---
        let mut pc = 0;
        let mut block_sealed: HashSet<cranelift_codegen::ir::Block> = HashSet::new();
        let mut terminated = false; // current block has a terminator

        while pc < code.len() {
            // Check if this PC is a jump target — switch to its block
            if let Some(&block) = block_map.get(&pc) {
                if !terminated {
                    // Jump from previous block to this one
                    builder.ins().jump(block, &[]);
                }
                builder.switch_to_block(block);
                // Don't seal yet — predecessors might not all be known
                terminated = false;
            }

            let op: Op = unsafe { std::mem::transmute(code[pc]) };
            pc += 1;

            if terminated {
                // Skip dead code after terminator until next block
                match op {
                    Op::LoadInt | Op::LoadFloat => pc += 8,
                    Op::SubLocalImm | Op::AddLocalImm => pc += 10,
                    Op::BranchIfLocalGtImm | Op::BranchIfLocalLteImm => pc += 14,
                    Op::Jump | Op::JumpIfFalse | Op::JumpIfTrue | Op::Loop
                    | Op::TryBegin | Op::TryEnd => pc += 4,
                    Op::Call => pc += 4,
                    Op::CallLocal | Op::CallBuiltin => pc += 3,
                    Op::MakeLambda => pc += 4,
                    Op::ErrorField | Op::Alias | Op::Use | Op::DefineFunction => pc += 4,
                    Op::GetLocal | Op::SetLocal | Op::GetGlobal | Op::SetGlobal
                    | Op::LoadConst | Op::FieldGet | Op::FieldSet | Op::MakeList
                    | Op::MakeObject | Op::MakeString | Op::MakeRange
                    | Op::IncLocal | Op::DecLocal | Op::PostIncLocal | Op::PostDecLocal
                    | Op::PreIncLocal | Op::PreDecLocal | Op::CompoundAddInt | Op::CompoundSubInt
                    | Op::GetDollarIndex | Op::GetDollarField | Op::ErrorCheck
                    | Op::OptionalCheck | Op::SetErrorTolerant | Op::Import | Op::Free
                    | Op::GetLocalInt => pc += 2,
                    _ => {}
                }
                continue;
            }

            match op {
                Op::LoadInt => {
                    let val = chunk.read_i64(pc);
                    pc += 8;
                    vstack.push(builder.ins().iconst(types::I64, val));
                }
                Op::LoadTrue => vstack.push(builder.ins().iconst(types::I64, 1)),
                Op::LoadFalse | Op::LoadVoid => vstack.push(builder.ins().iconst(types::I64, 0)),
                Op::GetLocal => {
                    let slot = chunk.read_u16(pc) as usize; pc += 2;
                    if slot >= vars.len() { return false; }
                    vstack.push(builder.use_var(vars[slot]));
                }
                Op::SetLocal => {
                    let slot = chunk.read_u16(pc) as usize; pc += 2;
                    let val = match vstack.pop() { Some(v) => v, None => return false };
                    if slot >= vars.len() { return false; }
                    builder.def_var(vars[slot], val);
                }
                // --- Arithmetic ---
                Op::AddInt | Op::Add => { binop!(vstack, builder, iadd); }
                Op::SubInt | Op::Sub => { binop!(vstack, builder, isub); }
                Op::MulInt | Op::Mul => { binop!(vstack, builder, imul); }
                Op::DivInt | Op::Div => { binop!(vstack, builder, sdiv); }
                Op::ModInt | Op::Mod => { binop!(vstack, builder, srem); }
                Op::NegInt => {
                    let a = match vstack.pop() { Some(v) => v, None => return false };
                    vstack.push(builder.ins().ineg(a));
                }
                Op::Pow => {
                    // No native pow — bail for now
                    return false;
                }
                // --- Comparison ---
                Op::EqInt | Op::Eq => { cmpop!(vstack, builder, IntCC::Equal); }
                Op::NeqInt | Op::Neq => { cmpop!(vstack, builder, IntCC::NotEqual); }
                Op::LtInt | Op::Lt => { cmpop!(vstack, builder, IntCC::SignedLessThan); }
                Op::GtInt | Op::Gt => { cmpop!(vstack, builder, IntCC::SignedGreaterThan); }
                Op::LteInt | Op::Lte => { cmpop!(vstack, builder, IntCC::SignedLessThanOrEqual); }
                Op::GteInt | Op::Gte => { cmpop!(vstack, builder, IntCC::SignedGreaterThanOrEqual); }
                // --- Logic ---
                Op::Not => {
                    let a = match vstack.pop() { Some(v) => v, None => return false };
                    let z = builder.ins().iconst(types::I64, 0);
                    let cmp = builder.ins().icmp(IntCC::Equal, a, z);
                    vstack.push(builder.ins().uextend(types::I64, cmp));
                }
                // --- Bitwise ---
                Op::BitAnd => { binop!(vstack, builder, band); }
                Op::BitOr => { binop!(vstack, builder, bor); }
                Op::BitXor => { binop!(vstack, builder, bxor); }
                Op::BitNot => {
                    let a = match vstack.pop() { Some(v) => v, None => return false };
                    vstack.push(builder.ins().bnot(a));
                }
                Op::Shl => { binop!(vstack, builder, ishl); }
                Op::Shr => { binop!(vstack, builder, sshr); }
                // --- Superinstructions ---
                Op::SubLocalImm => {
                    let slot = chunk.read_u16(pc) as usize; pc += 2;
                    let imm = chunk.read_i64(pc); pc += 8;
                    if slot >= vars.len() { return false; }
                    let v = builder.use_var(vars[slot]);
                    vstack.push(builder.ins().iadd_imm(v, -imm));
                }
                Op::AddLocalImm => {
                    let slot = chunk.read_u16(pc) as usize; pc += 2;
                    let imm = chunk.read_i64(pc); pc += 8;
                    if slot >= vars.len() { return false; }
                    let v = builder.use_var(vars[slot]);
                    vstack.push(builder.ins().iadd_imm(v, imm));
                }
                Op::BranchIfLocalGtImm => {
                    let slot = chunk.read_u16(pc) as usize; pc += 2;
                    let imm = chunk.read_i64(pc); pc += 8;
                    let offset = chunk.read_i32(pc); pc += 4;
                    if slot >= vars.len() { return false; }
                    let v = builder.use_var(vars[slot]);
                    let thr = builder.ins().iconst(types::I64, imm);
                    let cmp = builder.ins().icmp(IntCC::SignedGreaterThan, v, thr);
                    let target_pc = (pc as i32 + offset) as usize;
                    let target_block = match block_map.get(&target_pc) { Some(&b) => b, None => return false };
                    let fall_block = match block_map.get(&pc) { Some(&b) => b, None => return false };
                    builder.ins().brif(cmp, target_block, &[], fall_block, &[]);
                    terminated = true;
                }
                Op::BranchIfLocalLteImm => {
                    let slot = chunk.read_u16(pc) as usize; pc += 2;
                    let imm = chunk.read_i64(pc); pc += 8;
                    let offset = chunk.read_i32(pc); pc += 4;
                    if slot >= vars.len() { return false; }
                    let v = builder.use_var(vars[slot]);
                    let thr = builder.ins().iconst(types::I64, imm);
                    let cmp = builder.ins().icmp(IntCC::SignedLessThanOrEqual, v, thr);
                    let target_pc = (pc as i32 + offset) as usize;
                    let target_block = match block_map.get(&target_pc) { Some(&b) => b, None => return false };
                    let fall_block = match block_map.get(&pc) { Some(&b) => b, None => return false };
                    builder.ins().brif(cmp, target_block, &[], fall_block, &[]);
                    terminated = true;
                }
                Op::IncLocal => {
                    let slot = chunk.read_u16(pc) as usize; pc += 2;
                    if slot >= vars.len() { return false; }
                    let v = builder.use_var(vars[slot]);
                    let new_v = builder.ins().iadd_imm(v, 1);
                    builder.def_var(vars[slot], new_v);
                }
                Op::DecLocal => {
                    let slot = chunk.read_u16(pc) as usize; pc += 2;
                    if slot >= vars.len() { return false; }
                    let v = builder.use_var(vars[slot]);
                    let new_v = builder.ins().iadd_imm(v, -1);
                    builder.def_var(vars[slot], new_v);
                }
                Op::CompoundAddInt => {
                    let slot = chunk.read_u16(pc) as usize; pc += 2;
                    let rhs = match vstack.pop() { Some(v) => v, None => return false };
                    if slot >= vars.len() { return false; }
                    let v = builder.use_var(vars[slot]);
                    let new_v = builder.ins().iadd(v, rhs);
                    builder.def_var(vars[slot], new_v);
                }
                Op::CompoundSubInt => {
                    let slot = chunk.read_u16(pc) as usize; pc += 2;
                    let rhs = match vstack.pop() { Some(v) => v, None => return false };
                    if slot >= vars.len() { return false; }
                    let v = builder.use_var(vars[slot]);
                    let new_v = builder.ins().isub(v, rhs);
                    builder.def_var(vars[slot], new_v);
                }
                Op::PostIncLocal | Op::PreIncLocal => {
                    let slot = chunk.read_u16(pc) as usize; pc += 2;
                    if slot >= vars.len() { return false; }
                    let old = builder.use_var(vars[slot]);
                    let new_v = builder.ins().iadd_imm(old, 1);
                    builder.def_var(vars[slot], new_v);
                    vstack.push(if op == Op::PostIncLocal { old } else { new_v });
                }
                Op::PostDecLocal | Op::PreDecLocal => {
                    let slot = chunk.read_u16(pc) as usize; pc += 2;
                    if slot >= vars.len() { return false; }
                    let old = builder.use_var(vars[slot]);
                    let new_v = builder.ins().iadd_imm(old, -1);
                    builder.def_var(vars[slot], new_v);
                    vstack.push(if op == Op::PostDecLocal { old } else { new_v });
                }
                // --- Control flow ---
                Op::Jump => {
                    let offset = chunk.read_i32(pc); pc += 4;
                    let target_pc = (pc as i32 + offset) as usize;
                    if let Some(&block) = block_map.get(&target_pc) {
                        builder.ins().jump(block, &[]);
                        terminated = true;
                    } else {
                        return false;
                    }
                }
                Op::JumpIfFalse => {
                    let offset = chunk.read_i32(pc); pc += 4;
                    let cond = match vstack.pop() { Some(v) => v, None => return false };
                    let target_pc = (pc as i32 + offset) as usize;
                    let target_block = match block_map.get(&target_pc) { Some(&b) => b, None => return false };
                    let fall_block = match block_map.get(&pc) { Some(&b) => b, None => return false };
                    let z = builder.ins().iconst(types::I64, 0);
                    let cmp = builder.ins().icmp(IntCC::Equal, cond, z);
                    builder.ins().brif(cmp, target_block, &[], fall_block, &[]);
                    terminated = true;
                }
                Op::JumpIfTrue => {
                    let offset = chunk.read_i32(pc); pc += 4;
                    let cond = match vstack.pop() { Some(v) => v, None => return false };
                    let target_pc = (pc as i32 + offset) as usize;
                    let target_block = match block_map.get(&target_pc) { Some(&b) => b, None => return false };
                    let fall_block = match block_map.get(&pc) { Some(&b) => b, None => return false };
                    let z = builder.ins().iconst(types::I64, 0);
                    let cmp = builder.ins().icmp(IntCC::NotEqual, cond, z);
                    builder.ins().brif(cmp, target_block, &[], fall_block, &[]);
                    terminated = true;
                }
                Op::Loop => {
                    let offset = chunk.read_i32(pc); pc += 4;
                    let target_pc = (pc as i32 + 4 - offset) as usize;
                    if let Some(&block) = block_map.get(&target_pc) {
                        builder.ins().jump(block, &[]);
                        terminated = true;
                    } else {
                        return false;
                    }
                }
                // --- Calls ---
                Op::CallLocal => {
                    let _target = chunk.read_u16(pc); pc += 2;
                    let argc = code[pc]; pc += 1;
                    let mut args = Vec::new();
                    for _ in 0..argc {
                        match vstack.pop() { Some(v) => args.push(v), None => return false }
                    }
                    args.reverse();
                    let call = builder.ins().call(self_ref, &args);
                    vstack.push(builder.inst_results(call)[0]);
                }
                Op::Return => {
                    let val = match vstack.pop() { Some(v) => v, None => return false };
                    builder.ins().return_(&[val]);
                    terminated = true;
                }
                Op::ReturnVoid => {
                    let z = builder.ins().iconst(types::I64, 0);
                    builder.ins().return_(&[z]);
                    terminated = true;
                }
                Op::Pop => { vstack.pop(); }
                Op::Dup => {
                    if let Some(&v) = vstack.last() { vstack.push(v); }
                    else { return false; }
                }
                Op::CheckCancel => {} // skip in JIT

                // --- Anything we can't handle natively → bail ---
                _ => return false,
            }
        }

        // If we reach the end without a terminator
        if !terminated {
            if let Some(val) = vstack.pop() {
                builder.ins().return_(&[val]);
            } else {
                let z = builder.ins().iconst(types::I64, 0);
                builder.ins().return_(&[z]);
            }
        }

        // Seal all blocks
        for (_, &block) in &block_map {
            if !block_sealed.contains(&block) {
                builder.seal_block(block);
            }
        }

        true
    }
}

// --- Helper macros ---

macro_rules! binop {
    ($vstack:expr, $builder:expr, $op:ident) => {{
        let b = match $vstack.pop() { Some(v) => v, None => return false };
        let a = match $vstack.pop() { Some(v) => v, None => return false };
        $vstack.push($builder.ins().$op(a, b));
    }};
}
use binop;

macro_rules! cmpop {
    ($vstack:expr, $builder:expr, $cc:expr) => {{
        let b = match $vstack.pop() { Some(v) => v, None => return false };
        let a = match $vstack.pop() { Some(v) => v, None => return false };
        let cmp = $builder.ins().icmp($cc, a, b);
        let ext = $builder.ins().uextend(types::I64, cmp);
        $vstack.push(ext);
    }};
}
use cmpop;
