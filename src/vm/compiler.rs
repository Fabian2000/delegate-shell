use crate::parser::ast::*;
use crate::lexer::token::Span;
use super::bytecode::{Op, Chunk};

/// Compiles AST statements into bytecode chunks.
pub struct Compiler {
    chunk: Chunk,
    pub chunks: Vec<Chunk>,
    scope_depth: u16,
    locals: Vec<Local>,
    fn_chunks: std::collections::HashMap<String, u16>,
    /// Loop context for break/continue patching
    loop_stack: Vec<LoopCtx>,
    /// Global variable name → slot index (numeric, for Vec-based globals in VM)
    global_slots: std::collections::HashMap<String, u16>,
}

#[derive(Debug, Clone)]
struct Local {
    name: String,
    slot: u16,
    depth: u16,
}

struct LoopCtx {
    _start: usize,
    break_jumps: Vec<usize>,
    continue_jumps: Vec<usize>,
}

impl Default for Compiler {
    fn default() -> Self {
        Self::new()
    }
}

impl Compiler {
    pub fn new() -> Self {
        Self {
            chunk: Chunk::new("__top__".to_string(), 0),
            chunks: Vec::new(),
            scope_depth: 0,
            locals: Vec::new(),
            fn_chunks: std::collections::HashMap::new(),
            loop_stack: Vec::new(),
            global_slots: std::collections::HashMap::new(),
        }
    }

    pub fn compile(stmts: &[Stmt]) -> Result<Vec<Chunk>, String> {
        let mut c = Compiler::new();
        for stmt in stmts {
            c.compile_stmt(stmt)?;
        }
        c.chunk.emit(Op::ReturnVoid, 0);
        let mut top = std::mem::replace(&mut c.chunk, Chunk::new(String::new(), 0));
        // Store global slot mapping in the top-level chunk
        top.global_slots = c.global_slots.clone();
        let mut names = vec![std::rc::Rc::from(""); c.global_slots.len()];
        for (name, &slot) in &c.global_slots {
            names[slot as usize] = std::rc::Rc::from(name.as_str());
        }
        top.global_names = names;
        c.chunks.insert(0, top);
        Ok(c.chunks)
    }

    /// Get or create a global slot index for a variable name.
    fn global_slot(&mut self, name: &str) -> u16 {
        let lower = name.to_ascii_lowercase();
        if let Some(&slot) = self.global_slots.get(&lower) {
            slot
        } else {
            let slot = self.global_slots.len() as u16;
            self.global_slots.insert(lower, slot);
            slot
        }
    }

    fn line(&self, span: &Span) -> u32 { span.start as u32 }

    // ===================================================================
    // STATEMENTS — all StmtKind variants
    // ===================================================================

    fn compile_stmt(&mut self, stmt: &Stmt) -> Result<(), String> {
        let line = self.line(&stmt.span);
        match &stmt.kind {
            StmtKind::Assign { name, expr, is_dyn: _, error_tolerant, type_ann: _ } => {
                if *error_tolerant {
                    // Error-tolerant: wrap in TryBegin/TryEnd
                    let try_jump = self.chunk.emit_jump(Op::TryBegin, line);
                    self.compile_expr(expr)?;
                    self.set_variable(name, line);
                    // Mark variable as OK
                    let name_idx = self.global_slot(name);
                    self.chunk.emit_u16(Op::SetErrorTolerant, name_idx, line); // marks as ok
                    let end_jump = self.chunk.emit_jump(Op::TryEnd, line);
                    // Error handler target
                    self.chunk.patch_jump(try_jump);
                    // Store Void in variable and record error
                    self.chunk.emit(Op::LoadVoid, line);
                    self.set_variable(name, line);
                    let err_name_idx = self.global_slot(name);
                    self.chunk.emit_u16(Op::RecordError, err_name_idx, line);
                    self.chunk.patch_jump(end_jump);
                } else {
                    // Optimization: x = x + expr → StringAppendLocal (avoids Rc clone overhead)
                    if let ExprKind::BinaryOp { left, op: BinOp::Add, right } = &expr.kind
                        && let ExprKind::Ident(ref lhs_name) = left.kind
                            && lhs_name.eq_ignore_ascii_case(name)
                                && let Some(slot) = self.resolve_local(name) {
                                    self.compile_expr(right)?;
                                    self.chunk.emit_u16(Op::StringAppendLocal, slot, line);
                                    return Ok(());
                                }
                    self.compile_expr(expr)?;
                    self.set_variable(name, line);
                }
            }
            StmtKind::CompoundAssign { name, op, expr } => {
                self.compile_compound_assign(name, op, expr, line)?;
            }
            StmtKind::IndexAssign { target, index, value } => {
                self.compile_expr(target)?;
                self.compile_expr(index)?;
                self.compile_expr(value)?;
                self.chunk.emit(Op::IndexSet, line);
            }
            StmtKind::FieldAssign { target, field, value } => {
                self.compile_expr(target)?;
                self.compile_expr(value)?;
                let idx = self.chunk.constants.add(field);
                self.chunk.emit_u16(Op::FieldSet, idx, line);
            }
            StmtKind::ExprStmt(expr) => {
                self.compile_expr(expr)?;
                self.chunk.emit(Op::Pop, line);
            }
            StmtKind::FnDef { name, params, optional_params, return_type_ann: _, body } => {
                self.compile_fn_def(name, params, optional_params, body, line)?;
            }
            StmtKind::If { condition, body, else_body } => {
                self.compile_if(condition, body, else_body.as_deref(), line)?;
            }
            StmtKind::While { condition, body } => {
                self.compile_while(condition, body, line)?;
            }
            StmtKind::For { var, iter, body } => {
                self.compile_for(var, iter, body, line)?;
            }
            StmtKind::Return { expr, is_dyn: _ } => {
                if let Some(e) = expr {
                    self.compile_expr(e)?;
                    self.chunk.emit(Op::Return, line);
                } else {
                    self.chunk.emit(Op::ReturnVoid, line);
                }
            }
            StmtKind::PostIncDec { name, increment } | StmtKind::PreIncDec { name, increment } => {
                if let Some(slot) = self.resolve_local(name) {
                    self.chunk.emit_u16(if *increment { Op::IncLocal } else { Op::DecLocal }, slot, line);
                } else {
                    // Global: load, add/sub 1, store
                    self.get_variable(name, line);
                    self.chunk.emit_i64(Op::LoadInt, 1, line);
                    self.chunk.emit(if *increment { Op::Add } else { Op::Sub }, line);
                    let idx = self.global_slot(name);
                    self.chunk.emit_u16(Op::SetGlobal, idx, line);
                }
            }
            StmtKind::Continue => {
                let jump_offset = self.chunk.emit_jump(Op::Jump, line);
                if let Some(ctx) = self.loop_stack.last_mut() {
                    ctx.continue_jumps.push(jump_offset);
                }
            }
            StmtKind::Break => {
                let jump_offset = self.chunk.emit_jump(Op::Jump, line);
                if let Some(ctx) = self.loop_stack.last_mut() {
                    ctx.break_jumps.push(jump_offset);
                }
            }
            StmtKind::Free(name) => {
                let idx = self.global_slot(name);
                self.chunk.emit_u16(Op::Free, idx, line);
            }
            StmtKind::Throw(expr) => {
                self.compile_expr(expr)?;
                self.chunk.emit(Op::Throw, line);
            }
            StmtKind::Import(path) => {
                let idx = self.chunk.constants.add(path);
                self.chunk.emit_u16(Op::Import, idx, line);
            }
            StmtKind::Use { path, alias } => {
                let path_idx = self.chunk.constants.add(path);
                let alias_idx = alias.as_ref().map_or(0xFFFF, |a| self.chunk.constants.add(a));
                self.chunk.emit(Op::Use, line);
                self.chunk.code.extend_from_slice(&path_idx.to_le_bytes());
                self.chunk.code.extend_from_slice(&alias_idx.to_le_bytes());
            }
            StmtKind::EnumDef { name, variants } => {
                let name_idx = self.global_slot(name);
                self.chunk.emit(Op::DefineEnum, line);
                self.chunk.code.extend_from_slice(&name_idx.to_le_bytes());
                self.chunk.code.extend_from_slice(&(variants.len() as u16).to_le_bytes());
                for v in variants {
                    let vidx = self.chunk.constants.add(v);
                    self.chunk.code.extend_from_slice(&vidx.to_le_bytes());
                }
            }
            StmtKind::Match { expr, arms } => {
                self.compile_match(expr, arms, line)?;
            }
            StmtKind::Alias { name, target } => {
                let name_idx = self.chunk.constants.add(name);
                let target_idx = self.chunk.constants.add(target);
                self.chunk.emit(Op::Alias, line);
                self.chunk.code.extend_from_slice(&name_idx.to_le_bytes());
                self.chunk.code.extend_from_slice(&target_idx.to_le_bytes());
            }
        }
        Ok(())
    }

    // ===================================================================
    // EXPRESSIONS — all ExprKind variants
    // ===================================================================

    fn compile_expr(&mut self, expr: &Expr) -> Result<(), String> {
        let line = self.line(&expr.span);
        match &expr.kind {
            ExprKind::Int(n) => {
                self.chunk.emit_i64(Op::LoadInt, *n, line);
            }
            ExprKind::Float(n) => {
                self.chunk.emit_f64(Op::LoadFloat, *n, line);
            }
            ExprKind::Bool(b) => {
                self.chunk.emit(if *b { Op::LoadTrue } else { Op::LoadFalse }, line);
            }
            ExprKind::String(parts) => {
                self.compile_string(parts, line)?;
            }
            ExprKind::Ident(name) => {
                self.get_variable(name, line);
            }
            ExprKind::BinaryOp { left, op, right } => {
                self.compile_binary_op(left, *op, right, line)?;
            }
            ExprKind::UnaryOp { op, expr: inner } => {
                // Constant folding for unary ops on literals
                match (op, &inner.kind) {
                    (UnaryOp::Neg, ExprKind::Int(n)) => {
                        self.chunk.emit_i64(Op::LoadInt, n.wrapping_neg(), line);
                        return Ok(());
                    }
                    (UnaryOp::Neg, ExprKind::Float(n)) => {
                        self.chunk.emit_f64(Op::LoadFloat, -n, line);
                        return Ok(());
                    }
                    (UnaryOp::Not, ExprKind::Bool(b)) => {
                        self.chunk.emit(if *b { Op::LoadFalse } else { Op::LoadTrue }, line);
                        return Ok(());
                    }
                    (UnaryOp::BitNot, ExprKind::Int(n)) => {
                        self.chunk.emit_i64(Op::LoadInt, !n, line);
                        return Ok(());
                    }
                    _ => {}
                }
                // Fall through: compile normally
                self.compile_expr(inner)?;
                match op {
                    UnaryOp::Neg => {
                        if is_int_expr(&inner.kind) { self.chunk.emit(Op::NegInt, line); }
                        else { self.chunk.emit(Op::Neg, line); }
                    }
                    UnaryOp::Not => self.chunk.emit(Op::Not, line),
                    UnaryOp::BitNot => self.chunk.emit(Op::BitNot, line),
                }
            }
            ExprKind::Call { name, resolution, args } => {
                self.compile_call(name, *resolution, args, line)?;
            }
            ExprKind::List(items) => {
                for item in items {
                    self.compile_expr(item)?;
                }
                self.chunk.emit_u16(Op::MakeList, items.len() as u16, line);
            }
            ExprKind::Object(fields) => {
                for (key, val) in fields {
                    let idx = self.chunk.constants.add(key);
                    self.chunk.emit_u16(Op::LoadConst, idx, line);
                    self.compile_expr(val)?;
                }
                self.chunk.emit_u16(Op::MakeObject, fields.len() as u16, line);
            }
            ExprKind::Index { expr: target, index } => {
                self.compile_expr(target)?;
                self.compile_expr(index)?;
                self.chunk.emit(Op::Index, line);
            }
            ExprKind::FieldAccess { expr: target, field } => {
                self.compile_expr(target)?;
                let idx = self.chunk.constants.add(field);
                self.chunk.emit_u16(Op::FieldGet, idx, line);
            }
            ExprKind::Range { start, end } => {
                self.compile_expr(start)?;
                self.compile_expr(end)?;
                self.chunk.emit(Op::MakeRange, line);
            }
            ExprKind::Send { left, right } => {
                self.compile_expr(left)?;
                self.chunk.emit(Op::PushSendCtx, line);
                self.compile_expr(right)?;
                self.chunk.emit(Op::PopSendCtx, line);
            }
            ExprKind::SafeSend { left, right } => {
                // Same as Send but errors on left don't become FATAL
                self.compile_expr(left)?;
                self.chunk.emit(Op::PushSendCtx, line);
                self.compile_expr(right)?;
                self.chunk.emit(Op::PopSendCtx, line);
            }
            ExprKind::Lambda { name, resolution, bound_args } => {
                for arg in bound_args {
                    self.compile_expr(arg)?;
                }
                let name_idx = self.chunk.constants.add(name);
                let res = match resolution {
                    Resolution::Normal => 0u8,
                    Resolution::OwnFirst => 1,
                    Resolution::SystemOnly => 2,
                };
                self.chunk.emit(Op::MakeLambda, line);
                self.chunk.code.extend_from_slice(&name_idx.to_le_bytes());
                self.chunk.code.push(res);
                self.chunk.code.push(bound_args.len() as u8);
            }
            ExprKind::DollarRef(dref) => {
                match dref {
                    DollarRef::Whole => self.chunk.emit(Op::GetDollar, line),
                    DollarRef::Index(i) => self.chunk.emit_u16(Op::GetDollarIndex, *i as u16, line),
                    DollarRef::Field(f) => {
                        let idx = self.chunk.constants.add(f);
                        self.chunk.emit_u16(Op::GetDollarField, idx, line);
                    }
                }
            }
            ExprKind::ErrorCheck(name) => {
                let idx = self.global_slot(name);
                self.chunk.emit_u16(Op::ErrorCheck, idx, line);
            }
            ExprKind::ErrorField { name, field } => {
                let name_idx = self.global_slot(name);
                let field_idx = self.chunk.constants.add(field);
                self.chunk.emit(Op::ErrorField, line);
                self.chunk.code.extend_from_slice(&name_idx.to_le_bytes());
                self.chunk.code.extend_from_slice(&field_idx.to_le_bytes());
            }
            ExprKind::OptionalCheck(name) => {
                if let Some(slot) = self.resolve_local(name) {
                    // Local: just check if it's Void
                    self.chunk.emit_u16(Op::GetLocal, slot, line);
                    self.chunk.emit(Op::LoadVoid, line);
                    self.chunk.emit(Op::Neq, line);
                } else {
                    let idx = self.global_slot(name);
                    self.chunk.emit_u16(Op::OptionalCheck, idx, line);
                }
            }
            ExprKind::Atomic(inner) => {
                self.compile_expr(inner)?;
                self.chunk.emit(Op::Atomic, line);
            }
            ExprKind::PostIncDec { name, increment } => {
                if let Some(slot) = self.resolve_local(name) {
                    self.chunk.emit_u16(if *increment { Op::PostIncLocal } else { Op::PostDecLocal }, slot, line);
                } else {
                    // Global: push old value, then increment
                    self.get_variable(name, line);
                    self.chunk.emit(Op::Dup, line);
                    self.chunk.emit_i64(Op::LoadInt, 1, line);
                    self.chunk.emit(if *increment { Op::Add } else { Op::Sub }, line);
                    let idx = self.global_slot(name);
                    self.chunk.emit_u16(Op::SetGlobal, idx, line);
                    // Old value is still on stack
                }
            }
            ExprKind::PreIncDec { name, increment } => {
                if let Some(slot) = self.resolve_local(name) {
                    self.chunk.emit_u16(if *increment { Op::PreIncLocal } else { Op::PreDecLocal }, slot, line);
                } else {
                    // Global: increment, then push new value
                    self.get_variable(name, line);
                    self.chunk.emit_i64(Op::LoadInt, 1, line);
                    self.chunk.emit(if *increment { Op::Add } else { Op::Sub }, line);
                    self.chunk.emit(Op::Dup, line);
                    let idx = self.global_slot(name);
                    self.chunk.emit_u16(Op::SetGlobal, idx, line);
                    // New value is on stack
                }
            }
        }
        Ok(())
    }

    // ===================================================================
    // Binary ops with superinstructions
    // ===================================================================

    fn compile_binary_op(&mut self, left: &Expr, op: BinOp, right: &Expr, line: u32) -> Result<(), String> {
        // Short-circuit for && and ||
        match op {
            BinOp::And => {
                self.compile_expr(left)?;
                let jump = self.chunk.emit_jump(Op::JumpIfFalse, line);
                self.chunk.emit(Op::Pop, line);
                self.compile_expr(right)?;
                self.chunk.patch_jump(jump);
                return Ok(());
            }
            BinOp::Or => {
                self.compile_expr(left)?;
                let jump = self.chunk.emit_jump(Op::JumpIfTrue, line);
                self.chunk.emit(Op::Pop, line);
                self.compile_expr(right)?;
                self.chunk.patch_jump(jump);
                return Ok(());
            }
            _ => {}
        }

        // =============================================================
        // Constant folding: compute literal op literal at compile time
        // =============================================================

        // Int op Int
        if let (ExprKind::Int(a), ExprKind::Int(b)) = (&left.kind, &right.kind) {
            match op {
                BinOp::Add => { self.chunk.emit_i64(Op::LoadInt, a.wrapping_add(*b), line); return Ok(()); }
                BinOp::Sub => { self.chunk.emit_i64(Op::LoadInt, a.wrapping_sub(*b), line); return Ok(()); }
                BinOp::Mul => { self.chunk.emit_i64(Op::LoadInt, a.wrapping_mul(*b), line); return Ok(()); }
                BinOp::Div => {
                    if *b != 0 { self.chunk.emit_i64(Op::LoadInt, a.wrapping_div(*b), line); return Ok(()); }
                    // fall through for div-by-zero — let runtime handle it
                }
                BinOp::Mod => {
                    if *b != 0 { self.chunk.emit_i64(Op::LoadInt, a.wrapping_rem(*b), line); return Ok(()); }
                }
                BinOp::Pow => {
                    if *b >= 0 && *b <= 63 {
                        self.chunk.emit_i64(Op::LoadInt, a.wrapping_pow(*b as u32), line);
                        return Ok(());
                    }
                }
                BinOp::Eq    => { self.chunk.emit(if a == b { Op::LoadTrue } else { Op::LoadFalse }, line); return Ok(()); }
                BinOp::NotEq => { self.chunk.emit(if a != b { Op::LoadTrue } else { Op::LoadFalse }, line); return Ok(()); }
                BinOp::Lt    => { self.chunk.emit(if a <  b { Op::LoadTrue } else { Op::LoadFalse }, line); return Ok(()); }
                BinOp::Gt    => { self.chunk.emit(if a >  b { Op::LoadTrue } else { Op::LoadFalse }, line); return Ok(()); }
                BinOp::LtEq  => { self.chunk.emit(if a <= b { Op::LoadTrue } else { Op::LoadFalse }, line); return Ok(()); }
                BinOp::GtEq  => { self.chunk.emit(if a >= b { Op::LoadTrue } else { Op::LoadFalse }, line); return Ok(()); }
                BinOp::BitAnd => { self.chunk.emit_i64(Op::LoadInt, a & b, line); return Ok(()); }
                BinOp::BitOr  => { self.chunk.emit_i64(Op::LoadInt, a | b, line); return Ok(()); }
                BinOp::BitXor => { self.chunk.emit_i64(Op::LoadInt, a ^ b, line); return Ok(()); }
                BinOp::Shl => { self.chunk.emit_i64(Op::LoadInt, a.wrapping_shl(*b as u32), line); return Ok(()); }
                BinOp::Shr => { self.chunk.emit_i64(Op::LoadInt, a.wrapping_shr(*b as u32), line); return Ok(()); }
                _ => {}
            }
        }

        // Float op Float
        if let (ExprKind::Float(a), ExprKind::Float(b)) = (&left.kind, &right.kind) {
            match op {
                BinOp::Add => { self.chunk.emit_f64(Op::LoadFloat, a + b, line); return Ok(()); }
                BinOp::Sub => { self.chunk.emit_f64(Op::LoadFloat, a - b, line); return Ok(()); }
                BinOp::Mul => { self.chunk.emit_f64(Op::LoadFloat, a * b, line); return Ok(()); }
                BinOp::Div => { self.chunk.emit_f64(Op::LoadFloat, a / b, line); return Ok(()); }
                BinOp::Mod => { self.chunk.emit_f64(Op::LoadFloat, a % b, line); return Ok(()); }
                BinOp::Pow => { self.chunk.emit_f64(Op::LoadFloat, a.powf(*b), line); return Ok(()); }
                BinOp::Eq    => { self.chunk.emit(if a == b { Op::LoadTrue } else { Op::LoadFalse }, line); return Ok(()); }
                BinOp::NotEq => { self.chunk.emit(if a != b { Op::LoadTrue } else { Op::LoadFalse }, line); return Ok(()); }
                BinOp::Lt    => { self.chunk.emit(if a <  b { Op::LoadTrue } else { Op::LoadFalse }, line); return Ok(()); }
                BinOp::Gt    => { self.chunk.emit(if a >  b { Op::LoadTrue } else { Op::LoadFalse }, line); return Ok(()); }
                BinOp::LtEq  => { self.chunk.emit(if a <= b { Op::LoadTrue } else { Op::LoadFalse }, line); return Ok(()); }
                BinOp::GtEq  => { self.chunk.emit(if a >= b { Op::LoadTrue } else { Op::LoadFalse }, line); return Ok(()); }
                _ => {}
            }
        }

        // Int op Float / Float op Int (promote to float)
        if let Some(a) = match &left.kind { ExprKind::Int(n) => Some(*n as f64), ExprKind::Float(n) => Some(*n), _ => None }
            && let Some(b) = match &right.kind { ExprKind::Int(n) => Some(*n as f64), ExprKind::Float(n) => Some(*n), _ => None } {
                // Only fold mixed int/float cases (pure int and pure float already handled above)
                let is_mixed = matches!((&left.kind, &right.kind), (ExprKind::Int(_), ExprKind::Float(_)) | (ExprKind::Float(_), ExprKind::Int(_)));
                if is_mixed {
                    match op {
                        BinOp::Add => { self.chunk.emit_f64(Op::LoadFloat, a + b, line); return Ok(()); }
                        BinOp::Sub => { self.chunk.emit_f64(Op::LoadFloat, a - b, line); return Ok(()); }
                        BinOp::Mul => { self.chunk.emit_f64(Op::LoadFloat, a * b, line); return Ok(()); }
                        BinOp::Div => { self.chunk.emit_f64(Op::LoadFloat, a / b, line); return Ok(()); }
                        BinOp::Mod => { self.chunk.emit_f64(Op::LoadFloat, a % b, line); return Ok(()); }
                        BinOp::Pow => { self.chunk.emit_f64(Op::LoadFloat, a.powf(b), line); return Ok(()); }
                        BinOp::Eq    => { self.chunk.emit(if a == b { Op::LoadTrue } else { Op::LoadFalse }, line); return Ok(()); }
                        BinOp::NotEq => { self.chunk.emit(if a != b { Op::LoadTrue } else { Op::LoadFalse }, line); return Ok(()); }
                        BinOp::Lt    => { self.chunk.emit(if a <  b { Op::LoadTrue } else { Op::LoadFalse }, line); return Ok(()); }
                        BinOp::Gt    => { self.chunk.emit(if a >  b { Op::LoadTrue } else { Op::LoadFalse }, line); return Ok(()); }
                        BinOp::LtEq  => { self.chunk.emit(if a <= b { Op::LoadTrue } else { Op::LoadFalse }, line); return Ok(()); }
                        BinOp::GtEq  => { self.chunk.emit(if a >= b { Op::LoadTrue } else { Op::LoadFalse }, line); return Ok(()); }
                        _ => {}
                    }
                }
            }

        // String + String (concatenation)
        if op == BinOp::Add
            && let (ExprKind::String(left_parts), ExprKind::String(right_parts)) = (&left.kind, &right.kind)
                && left_parts.len() == 1 && right_parts.len() == 1
                    && let (StringPart::Literal(a), StringPart::Literal(b)) = (&left_parts[0], &right_parts[0]) {
                        let mut concat = String::with_capacity(a.len() + b.len());
                        concat.push_str(a);
                        concat.push_str(b);
                        let idx = self.chunk.constants.add(&concat);
                        self.chunk.emit_u16(Op::LoadConst, idx, line);
                        return Ok(());
                    }

        // Bool && Bool, Bool || Bool (already handled by short-circuit above, but
        // constant fold for completeness if both are literal bools)
        if let (ExprKind::Bool(a), ExprKind::Bool(b)) = (&left.kind, &right.kind) {
            match op {
                BinOp::Eq    => { self.chunk.emit(if a == b { Op::LoadTrue } else { Op::LoadFalse }, line); return Ok(()); }
                BinOp::NotEq => { self.chunk.emit(if a != b { Op::LoadTrue } else { Op::LoadFalse }, line); return Ok(()); }
                _ => {}
            }
        }

        // Superinstruction: local +/- int_literal
        if (op == BinOp::Sub || op == BinOp::Add)
            && let ExprKind::Ident(name) = &left.kind
            && let ExprKind::Int(imm) = &right.kind
            && let Some(slot) = self.resolve_local(name) {
                let super_op = if op == BinOp::Sub { Op::SubLocalImm } else { Op::AddLocalImm };
                self.chunk.emit(super_op, line);
                self.chunk.code.extend_from_slice(&slot.to_le_bytes());
                self.chunk.code.extend_from_slice(&imm.to_le_bytes());
                return Ok(());
            }

        self.compile_expr(left)?;
        self.compile_expr(right)?;

        let left_int = is_int_expr(&left.kind);
        let right_int = is_int_expr(&right.kind);
        let use_int = left_int && right_int;

        let opcode = match op {
            BinOp::Add => if use_int { Op::AddInt } else { Op::Add },
            BinOp::Sub => if use_int { Op::SubInt } else { Op::Sub },
            BinOp::Mul => if use_int { Op::MulInt } else { Op::Mul },
            BinOp::Div => if use_int { Op::DivInt } else { Op::Div },
            BinOp::Mod => if use_int { Op::ModInt } else { Op::Mod },
            BinOp::Pow => Op::Pow,
            BinOp::Eq => if use_int { Op::EqInt } else { Op::Eq },
            BinOp::NotEq => if use_int { Op::NeqInt } else { Op::Neq },
            BinOp::Lt => if use_int { Op::LtInt } else { Op::Lt },
            BinOp::Gt => if use_int { Op::GtInt } else { Op::Gt },
            BinOp::LtEq => if use_int { Op::LteInt } else { Op::Lte },
            BinOp::GtEq => if use_int { Op::GteInt } else { Op::Gte },
            BinOp::And | BinOp::Or => return Err("internal: unexpected state".to_string()),
            BinOp::BitAnd => Op::BitAnd,
            BinOp::BitOr => Op::BitOr,
            BinOp::BitXor => Op::BitXor,
            BinOp::Shl => Op::Shl,
            BinOp::Shr => Op::Shr,
        };
        self.chunk.emit(opcode, line);
        Ok(())
    }

    // ===================================================================
    // Helpers
    // ===================================================================

    fn compile_string(&mut self, parts: &[StringPart], line: u32) -> Result<(), String> {
        if parts.len() == 1
            && let StringPart::Literal(s) = &parts[0] {
                let idx = self.chunk.constants.add(s);
                self.chunk.emit_u16(Op::LoadConst, idx, line);
                return Ok(());
            }
        for part in parts {
            match part {
                StringPart::Literal(s) => {
                    let idx = self.chunk.constants.add(s);
                    self.chunk.emit_u16(Op::LoadConst, idx, line);
                }
                StringPart::Expr(expr) => {
                    self.compile_expr(expr)?;
                }
            }
        }
        self.chunk.emit_u16(Op::MakeString, parts.len() as u16, line);
        Ok(())
    }

    fn compile_call(&mut self, name: &str, resolution: Resolution, args: &[Expr], line: u32) -> Result<(), String> {
        for arg in args {
            self.compile_expr(arg)?;
        }
        let argc = args.len() as u8;
        let lower = name.to_ascii_lowercase();

        // Direct call for known user functions
        if (resolution == Resolution::OwnFirst || resolution == Resolution::Normal)
            && let Some(&chunk_idx) = self.fn_chunks.get(&lower) {
                self.chunk.emit(Op::CallLocal, line);
                self.chunk.code.extend_from_slice(&chunk_idx.to_le_bytes());
                self.chunk.code.push(argc);
                return Ok(());
            }

        // Generic call
        let name_idx = self.chunk.constants.add(&lower);
        let res_byte = match resolution {
            Resolution::Normal => 0u8,
            Resolution::OwnFirst => 1,
            Resolution::SystemOnly => 2,
        };
        self.chunk.emit(Op::Call, line);
        self.chunk.code.extend_from_slice(&name_idx.to_le_bytes());
        self.chunk.code.push(argc);
        self.chunk.code.push(res_byte);
        Ok(())
    }

    fn compile_fn_def(&mut self, name: &str, params: &[(String, Option<TypeAnnotation>, bool)], optional_params: &[(String, Option<TypeAnnotation>, bool)], body: &[Stmt], line: u32) -> Result<(), String> {
        let chunk_idx = self.chunks.len() as u16 + 1;
        let lower = name.to_ascii_lowercase();
        self.fn_chunks.insert(lower.clone(), chunk_idx);

        let total_params = params.len() + optional_params.len();
        let prev_chunk = std::mem::replace(&mut self.chunk, Chunk::new(name.to_string(), params.len() as u8));
        let prev_locals = std::mem::take(&mut self.locals);
        let prev_depth = self.scope_depth;
        self.scope_depth = 0;

        // Required params as locals
        for (i, (pname, _, _)) in params.iter().enumerate() {
            self.locals.push(Local { name: pname.to_ascii_lowercase(), slot: i as u16, depth: 0 });
        }
        // Optional params as locals (filled with Void if not provided)
        for (i, (pname, _, _)) in optional_params.iter().enumerate() {
            self.locals.push(Local { name: pname.to_ascii_lowercase(), slot: (params.len() + i) as u16, depth: 0 });
        }

        // Store total param info for the chunk
        self.chunk.param_count = total_params as u8;

        for stmt in body {
            self.compile_stmt(stmt)?;
        }

        // Implicit return void
        if self.chunk.code.is_empty() || self.chunk.code[self.chunk.code.len() - 1] != Op::Return as u8 {
            self.chunk.emit(Op::ReturnVoid, line);
        }

        // Copy locals info into the chunk for runtime lookup
        for local in &self.locals {
            self.chunk.locals.push(super::bytecode::LocalInfo {
                name: local.name.clone(),
                slot: local.slot,
                depth: local.depth,
                is_dyn: false,
            });
        }

        let fn_chunk = std::mem::replace(&mut self.chunk, prev_chunk);
        self.chunks.push(fn_chunk);
        self.locals = prev_locals;
        self.scope_depth = prev_depth;

        let name_idx = self.chunk.constants.add(&lower);
        self.chunk.emit(Op::DefineFunction, line);
        self.chunk.code.extend_from_slice(&name_idx.to_le_bytes());
        self.chunk.code.extend_from_slice(&chunk_idx.to_le_bytes());
        Ok(())
    }

    fn compile_if(&mut self, condition: &Expr, body: &[Stmt], else_body: Option<&[Stmt]>, line: u32) -> Result<(), String> {
        // Superinstruction: if local <= imm
        if let ExprKind::BinaryOp { left, op: BinOp::LtEq, right } = &condition.kind
            && let ExprKind::Ident(name) = &left.kind
            && let ExprKind::Int(imm) = &right.kind
            && let Some(slot) = self.resolve_local(name)
        {
            self.chunk.emit(Op::BranchIfLocalGtImm, line);
            self.chunk.code.extend_from_slice(&slot.to_le_bytes());
            self.chunk.code.extend_from_slice(&imm.to_le_bytes());
            let then_jump = self.chunk.code.len();
            self.chunk.code.extend_from_slice(&0i32.to_le_bytes());

            for stmt in body { self.compile_stmt(stmt)?; }

            if let Some(else_stmts) = else_body {
                let else_jump = self.chunk.emit_jump(Op::Jump, line);
                self.chunk.patch_jump(then_jump);
                for stmt in else_stmts { self.compile_stmt(stmt)?; }
                self.chunk.patch_jump(else_jump);
            } else {
                self.chunk.patch_jump(then_jump);
            }
            return Ok(());
        }

        // Superinstruction: if local < imm
        if let ExprKind::BinaryOp { left, op: BinOp::Lt, right } = &condition.kind
            && let ExprKind::Ident(name) = &left.kind
            && let ExprKind::Int(imm) = &right.kind
            && let Some(slot) = self.resolve_local(name)
        {
            self.chunk.emit(Op::BranchIfLocalGtImm, line); // >= is inverse of <
            self.chunk.code.extend_from_slice(&slot.to_le_bytes());
            self.chunk.code.extend_from_slice(&(imm - 1).to_le_bytes()); // n < imm ↔ n <= imm-1 ↔ !(n > imm-1)
            let then_jump = self.chunk.code.len();
            self.chunk.code.extend_from_slice(&0i32.to_le_bytes());
            for stmt in body { self.compile_stmt(stmt)?; }
            if let Some(else_stmts) = else_body {
                let else_jump = self.chunk.emit_jump(Op::Jump, line);
                self.chunk.patch_jump(then_jump);
                for stmt in else_stmts { self.compile_stmt(stmt)?; }
                self.chunk.patch_jump(else_jump);
            } else {
                self.chunk.patch_jump(then_jump);
            }
            return Ok(());
        }

        // Generic if
        self.compile_expr(condition)?;
        let then_jump = self.chunk.emit_jump(Op::JumpIfFalse, line);
        self.chunk.emit(Op::Pop, line);
        for stmt in body { self.compile_stmt(stmt)?; }
        if let Some(else_stmts) = else_body {
            let else_jump = self.chunk.emit_jump(Op::Jump, line);
            self.chunk.patch_jump(then_jump);
            self.chunk.emit(Op::Pop, line);
            for stmt in else_stmts { self.compile_stmt(stmt)?; }
            self.chunk.patch_jump(else_jump);
        } else {
            let skip_pop = self.chunk.emit_jump(Op::Jump, line);
            self.chunk.patch_jump(then_jump);
            self.chunk.emit(Op::Pop, line);
            self.chunk.patch_jump(skip_pop);
        }
        Ok(())
    }

    fn compile_while(&mut self, condition: &Expr, body: &[Stmt], line: u32) -> Result<(), String> {
        let loop_start = self.chunk.pos();
        self.chunk.emit(Op::CheckCancel, line);

        self.loop_stack.push(LoopCtx { _start: loop_start, break_jumps: Vec::new(), continue_jumps: Vec::new() });

        self.compile_expr(condition)?;
        let exit_jump = self.chunk.emit_jump(Op::JumpIfFalse, line);
        self.chunk.emit(Op::Pop, line);

        for stmt in body { self.compile_stmt(stmt)?; }

        self.chunk.emit_loop(loop_start, line);
        self.chunk.patch_jump(exit_jump);
        self.chunk.emit(Op::Pop, line);

        // Patch break/continue
        let ctx = self.loop_stack.pop().ok_or("Loop stack underflow")?;
        let end_pos = self.chunk.pos();
        for offset in ctx.break_jumps {
            let target = end_pos as i32 - (offset as i32 + 4);
            self.chunk.code[offset..offset+4].copy_from_slice(&target.to_le_bytes());
        }
        for offset in ctx.continue_jumps {
            let target = loop_start as i32 - (offset as i32 + 4);
            self.chunk.code[offset..offset+4].copy_from_slice(&target.to_le_bytes());
        }
        Ok(())
    }

    fn compile_for(&mut self, var: &str, iter: &Expr, body: &[Stmt], line: u32) -> Result<(), String> {
        // Evaluate iterator → list on stack
        self.compile_expr(iter)?;
        let iter_slot = self.add_local("__iter__");
        self.chunk.emit_u16(Op::SetLocal, iter_slot, line);

        // Counter
        self.chunk.emit_i64(Op::LoadInt, 0, line);
        let idx_slot = self.add_local("__idx__");
        self.chunk.emit_u16(Op::SetLocal, idx_slot, line);

        // Loop var
        self.chunk.emit(Op::LoadVoid, line);
        let var_slot = self.add_local(var);
        self.chunk.emit_u16(Op::SetLocal, var_slot, line);

        let loop_start = self.chunk.pos();
        self.chunk.emit(Op::CheckCancel, line);

        self.loop_stack.push(LoopCtx { _start: loop_start, break_jumps: Vec::new(), continue_jumps: Vec::new() });

        // Condition: idx < len(iter)
        // Emit: GetLocal(idx), GetLocal(iter), LenList, LtInt, JumpIfFalse
        self.chunk.emit_u16(Op::GetLocal, idx_slot, line);
        self.chunk.emit_u16(Op::GetLocal, iter_slot, line);
        // We need a builtin len call — use CallBuiltin for simplicity
        // Actually, let's add a specialized LenOp or just call len
        let len_idx = self.chunk.constants.add("len");
        self.chunk.emit(Op::Call, line);
        self.chunk.code.extend_from_slice(&len_idx.to_le_bytes());
        self.chunk.code.push(1); // argc
        self.chunk.code.push(2); // SystemOnly
        self.chunk.emit(Op::Lt, line);
        let exit_jump = self.chunk.emit_jump(Op::JumpIfFalse, line);
        self.chunk.emit(Op::Pop, line);

        // var = iter[idx]
        self.chunk.emit_u16(Op::GetLocal, iter_slot, line);
        self.chunk.emit_u16(Op::GetLocal, idx_slot, line);
        self.chunk.emit(Op::Index, line);
        self.chunk.emit_u16(Op::SetLocal, var_slot, line);

        // Body
        for stmt in body { self.compile_stmt(stmt)?; }

        // idx++
        self.chunk.emit_u16(Op::IncLocal, idx_slot, line);

        self.chunk.emit_loop(loop_start, line);
        self.chunk.patch_jump(exit_jump);
        self.chunk.emit(Op::Pop, line);

        // Patch break/continue
        let ctx = self.loop_stack.pop().ok_or("Loop stack underflow")?;
        let end_pos = self.chunk.pos();
        for offset in ctx.break_jumps {
            let target = end_pos as i32 - (offset as i32 + 4);
            self.chunk.code[offset..offset+4].copy_from_slice(&target.to_le_bytes());
        }
        for offset in ctx.continue_jumps {
            let target = loop_start as i32 - (offset as i32 + 4);
            self.chunk.code[offset..offset+4].copy_from_slice(&target.to_le_bytes());
        }
        Ok(())
    }

    fn compile_match(&mut self, expr: &Expr, arms: &[MatchArm], line: u32) -> Result<(), String> {
        self.compile_expr(expr)?;
        let mut end_jumps = Vec::new();

        for arm in arms {
            if let Some(pattern) = &arm.pattern {
                // Duplicate match value for comparison
                self.chunk.emit(Op::Dup, line);
                self.compile_expr(pattern)?;
                self.chunk.emit(Op::Eq, line);
                let skip_jump = self.chunk.emit_jump(Op::JumpIfFalse, line);
                self.chunk.emit(Op::Pop, line); // pop comparison result
                self.chunk.emit(Op::Pop, line); // pop match value

                for stmt in &arm.body { self.compile_stmt(stmt)?; }
                end_jumps.push(self.chunk.emit_jump(Op::Jump, line));

                self.chunk.patch_jump(skip_jump);
                self.chunk.emit(Op::Pop, line); // pop comparison result
            } else {
                // Default arm
                self.chunk.emit(Op::Pop, line); // pop match value
                for stmt in &arm.body { self.compile_stmt(stmt)?; }
                end_jumps.push(self.chunk.emit_jump(Op::Jump, line));
            }
        }

        // Pop match value if no arm matched
        self.chunk.emit(Op::Pop, line);

        for offset in end_jumps {
            self.chunk.patch_jump(offset);
        }
        Ok(())
    }

    fn compile_compound_assign(&mut self, name: &str, op: &CompoundOp, expr: &Expr, line: u32) -> Result<(), String> {
        if let Some(slot) = self.resolve_local(name) {
            // Superinstruction: int += literal / int -= literal
            if (*op == CompoundOp::Add || *op == CompoundOp::Sub) && matches!(&expr.kind, ExprKind::Int(_)) {
                self.compile_expr(expr)?;
                self.chunk.emit_u16(
                    if *op == CompoundOp::Add { Op::CompoundAddInt } else { Op::CompoundSubInt },
                    slot, line
                );
                return Ok(());
            }
            // Superinstruction: string += expr (in-place append)
            if *op == CompoundOp::Add {
                self.compile_expr(expr)?;
                self.chunk.emit_u16(Op::StringAppendLocal, slot, line);
                return Ok(());
            }
        }
        // Generic
        self.get_variable(name, line);
        self.compile_expr(expr)?;
        let opcode = match op {
            CompoundOp::Add => Op::Add,
            CompoundOp::Sub => Op::Sub,
            CompoundOp::Mul => Op::Mul,
            CompoundOp::Div => Op::Div,
            CompoundOp::Mod => Op::Mod,
            CompoundOp::Pow => Op::Pow,
            CompoundOp::BitAnd => Op::BitAnd,
            CompoundOp::BitOr => Op::BitOr,
            CompoundOp::BitXor => Op::BitXor,
            CompoundOp::Shl => Op::Shl,
            CompoundOp::Shr => Op::Shr,
        };
        self.chunk.emit(opcode, line);
        self.set_variable(name, line);
        Ok(())
    }

    fn _set_variable_error_tolerant(&mut self, name: &str, line: u32) {
        if let Some(slot) = self.resolve_local(name) {
            self.chunk.emit_u16(Op::SetErrorTolerant, slot, line);
        } else {
            // Global error-tolerant — emit as SetGlobal (simplified, no error capture in VM yet)
            let idx = self.global_slot(name);
            self.chunk.emit_u16(Op::SetGlobal, idx, line);
        }
    }

    // ===================================================================
    // Variable resolution
    // ===================================================================

    fn resolve_local(&self, name: &str) -> Option<u16> {
        let lower = name.to_ascii_lowercase();
        for local in self.locals.iter().rev() {
            if local.name == lower { return Some(local.slot); }
        }
        None
    }

    fn add_local(&mut self, name: &str) -> u16 {
        let lower = name.to_ascii_lowercase();
        let slot = self.locals.len() as u16;
        self.locals.push(Local { name: lower, slot, depth: self.scope_depth });
        slot
    }

    fn get_variable(&mut self, name: &str, line: u32) {
        if let Some(slot) = self.resolve_local(name) {
            self.chunk.emit_u16(Op::GetLocal, slot, line);
        } else {
            let idx = self.global_slot(name);
            self.chunk.emit_u16(Op::GetGlobal, idx, line);
        }
    }

    fn set_variable(&mut self, name: &str, line: u32) {
        if let Some(slot) = self.resolve_local(name) {
            self.chunk.emit_u16(Op::SetLocal, slot, line);
        } else if self.chunk.name != "__top__" {
            // Inside a function — create new local
            let slot = self.add_local(name);
            self.chunk.emit_u16(Op::SetLocal, slot, line);
        } else {
            // Top-level — always use globals
            let idx = self.global_slot(name);
            self.chunk.emit_u16(Op::SetGlobal, idx, line);
        }
    }
}

fn is_int_expr(kind: &ExprKind) -> bool {
    match kind {
        ExprKind::Int(_) => true,
        ExprKind::BinaryOp { left, op, right } => {
            matches!(op, BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod)
                && is_int_expr(&left.kind) && is_int_expr(&right.kind)
        }
        _ => false,
    }
}
