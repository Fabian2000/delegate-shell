use std::collections::{HashMap, HashSet};
use indexmap::IndexMap;
use super::value::{Value, MaybeError, ErrorInfo, new_list, new_object, ValueKind as VK};
use super::env::{Environment, UserFn};
use crate::parser::ast::{
    BinOp, CompoundOp, DollarRef, Expr, ExprKind, Resolution, Stmt, StmtKind, StringPart,
    TypeAnnotation, UnaryOp,
};
use crate::exec;

pub struct Interpreter {
    pub(crate) env: Environment,
    /// Builtin function registry — owned per instance
    pub(crate) registry: crate::builtins::registry::BuiltinRegistry,
    /// Current dollar value in send context
    send_value: Option<Value>,
    /// Optional cancel flag — checked periodically during execution
    pub cancel_flag: Option<&'static std::sync::atomic::AtomicBool>,
    /// Execution mode (TreeWalk, Vm, Jit)
    execution_mode: crate::vm::ExecutionMode,
    /// Whether any code has been executed (locks execution mode)
    has_executed: bool,
}

/// Return control flow signal
enum FlowSignal {
    None,
    Return(Option<Value>),
    Continue,
    Break,
}

impl Interpreter {
    /// Create a new interpreter with all standard builtins.
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            env: Environment::new(),
            registry: crate::builtins::registry::build_default_registry()?,
            send_value: None,
            cancel_flag: None,
            execution_mode: crate::vm::ExecutionMode::Auto,
            has_executed: false,
        })
    }

    /// Set the execution mode. Must be called before any code is executed.
    pub fn set_execution_mode(&mut self, mode: crate::vm::ExecutionMode) -> Result<(), String> {
        if self.has_executed {
            return Err("Cannot change execution mode after code has been executed".to_string());
        }
        self.execution_mode = mode;
        Ok(())
    }

    pub(crate) fn is_jit_mode(&self) -> bool {
        self.execution_mode == crate::vm::ExecutionMode::Jit
    }

    /// Register a custom builtin function. Returns `Err` if the name is already taken.
    pub fn register(
        &mut self,
        name: &str,
        params: &'static [crate::builtins::registry::Param],
        returns: crate::builtins::registry::Type,
        f: impl Fn(&[Value], &mut Interpreter) -> Result<Value, String> + 'static,
    ) -> Result<(), String> {
        self.registry.register(name, params, returns, f)
    }

    /// Parse and execute source code directly.
    ///
    /// # Errors
    ///
    /// Returns an error if parsing or execution fails.
    pub fn run_source(&mut self, source: &str) -> Result<(), String> {
        let mut lexer = crate::lexer::Lexer::new(source);
        let tokens = lexer.tokenize();
        let stmts = crate::parser::parse(&tokens)?;
        self.run(&stmts)
    }

    /// Read and execute a file.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read, or parsing/execution fails.
    pub fn run_file(&mut self, path: &str) -> Result<(), String> {
        let source = std::fs::read_to_string(path)
            .map_err(|e| format!("Error reading '{path}': {e}"))?;
        self.run_source(&source)
    }

    /// Execute a list of pre-parsed statements.
    ///
    /// # Errors
    ///
    /// Returns an error if any statement fails to execute.
    pub fn run(&mut self, stmts: &[Stmt]) -> Result<(), String> {
        self.has_executed = true;

        // Auto mode: compile first, analyze, then decide
        let mode = if self.execution_mode == crate::vm::ExecutionMode::Auto {
            let chunks = crate::vm::compiler::Compiler::compile(stmts)?;
            let chosen = crate::vm::auto_mode::choose_mode(&chunks);
            // If Auto chose TreeWalk, fall through to tree-walker
            // Otherwise run via VM/JIT with the already-compiled chunks
            if chosen == crate::vm::ExecutionMode::TreeWalk {
                crate::vm::ExecutionMode::TreeWalk
            } else {
                self.pre_register_functions(stmts);
                let mut vm = crate::vm::machine::VM::new();
                if chosen == crate::vm::ExecutionMode::Jit {
                    self.execution_mode = crate::vm::ExecutionMode::Jit;
                }
                return vm.execute(chunks, self);
            }
        } else {
            self.execution_mode
        };

        match mode {
            crate::vm::ExecutionMode::TreeWalk | crate::vm::ExecutionMode::Auto => {
                for stmt in stmts {
                    if let FlowSignal::Return(_) = self.exec_stmt(stmt)? {
                        break;
                    }
                }
                Ok(())
            }
            crate::vm::ExecutionMode::Vm | crate::vm::ExecutionMode::Jit => {
                self.pre_register_functions(stmts);
                let chunks = crate::vm::compiler::Compiler::compile(stmts)?;
                let mut vm = crate::vm::machine::VM::new();
                vm.execute(chunks, self)
            }
        }
    }

    /// Pre-register function definitions in the tree-walker env
    /// so builtins (map, filter, etc.) can find and call them.
    fn pre_register_functions(&mut self, stmts: &[Stmt]) {
        for stmt in stmts {
            if let StmtKind::FnDef { name, params, optional_params, return_type_ann, body } = &stmt.kind {
                self.env.define_fn(crate::interpreter::env::UserFn {
                    name: name.clone(),
                    params: params.clone(),
                    optional_params: optional_params.clone(),
                    declared_return_type: return_type_ann.clone(),
                    has_dyn_return: body_has_dyn_return(body),
                    inferred_types: std::collections::HashMap::new(),
                    body_inferred_params: std::collections::HashSet::new(),
                    body: body.clone(),
                    return_type: None,
                });
            }
        }
    }

    fn exec_stmt(&mut self, stmt: &Stmt) -> Result<FlowSignal, String> {
        // Check for cancellation (Ctrl+C)
        if self.cancel_flag.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed)) {
            return Err("Cancelled".to_string());
        }
        match &stmt.kind {
            StmtKind::Assign { name, error_tolerant, type_ann, is_dyn, expr } => {
                self.exec_assign(name, *error_tolerant, type_ann.as_ref(), *is_dyn, expr)
            }

            StmtKind::CompoundAssign { name, op, expr } => {
                self.exec_compound_assign(name, *op, expr)
            }

            StmtKind::IndexAssign { target, index, value } => {
                self.exec_index_assign(target, index, value)
            }
            StmtKind::FieldAssign { target, field, value } => {
                self.exec_field_assign(target, field, value)
            }
            StmtKind::PostIncDec { name, increment } | StmtKind::PreIncDec { name, increment } => {
                self.exec_inc_dec(name, *increment)
            }

            StmtKind::ExprStmt(expr) => {
                self.eval_expr(expr)?;
                Ok(FlowSignal::None)
            }

            StmtKind::FnDef { name, params, optional_params, return_type_ann, body } => {
                let (inferred, body_inferred) = infer_param_types_from_body(body, params, optional_params, &self.env, &self.registry);
                self.env.define_fn(UserFn {
                    name: name.clone(),
                    params: params.clone(),
                    optional_params: optional_params.clone(),
                    declared_return_type: return_type_ann.clone(),
                    has_dyn_return: body_has_dyn_return(body),
                    inferred_types: inferred,
                    body_inferred_params: body_inferred,
                    body: body.clone(),
                    return_type: None,
                });
                Ok(FlowSignal::None)
            }

            StmtKind::If { condition, body, else_body } => {
                self.exec_if(condition, body, else_body.as_deref())
            }

            StmtKind::While { condition, body } => {
                self.exec_while(condition, body)
            }

            StmtKind::For { var, iter, body } => {
                self.exec_for(var, iter, body)
            }

            StmtKind::Return { expr, is_dyn: _ } => {
                let val = expr.as_ref().map(|e| self.eval_expr(e)).transpose()?;
                Ok(FlowSignal::Return(val))
            }

            StmtKind::Import(path) => {
                let content = std::fs::read_to_string(path)
                    .map_err(|e| format!("Cannot import '{path}': {e}"))?;
                let mut lexer = crate::lexer::Lexer::new(&content);
                let tokens = lexer.tokenize();
                let stmts = crate::parser::parse(&tokens)?;
                self.run(&stmts)?;
                Ok(FlowSignal::None)
            }

            StmtKind::Free(name) => {
                self.env.remove(name);
                Ok(FlowSignal::None)
            }

            StmtKind::Use { path, alias } => self.exec_use(path, alias.as_deref()),

            StmtKind::Throw(expr) => {
                let val = self.eval_expr(expr)?;
                Err(val.to_string())
            }

            StmtKind::Continue => Ok(FlowSignal::Continue),
            StmtKind::Break => Ok(FlowSignal::Break),
            StmtKind::Match { expr, arms } => self.exec_match(expr, arms),

            StmtKind::EnumDef { name, variants } => {
                let mut map = indexmap::IndexMap::new();
                for (i, variant) in variants.iter().enumerate() {
                    map.insert(variant.clone(), Value::int(i64::try_from(i).unwrap_or(i64::MAX)));
                }
                self.env.set(name, MaybeError::Ok(new_object(map)))?;
                Ok(FlowSignal::None)
            }

            StmtKind::Alias { name, target } => {
                self.env.aliases.insert(name.to_ascii_lowercase(), target.clone());
                Ok(FlowSignal::None)
            }
        }
    }

    fn exec_index_assign(&mut self, target: &Expr, index: &Expr, value: &Expr) -> Result<FlowSignal, String> {
        let target_val = self.eval_expr(target)?;
        let idx = self.eval_expr(index)?;
        let val = self.eval_expr(value)?;
        match (target_val.kind(), idx.kind()) {
            (VK::List(list), VK::Int(i)) => {
                let mut list = list.borrow_mut();
                let idx = usize::try_from(i).map_err(|_| format!("Negative index {i}"))?;
                if idx >= list.len() {
                    return Err(format!("Index {idx} out of bounds (len {})", list.len()));
                }
                list[idx] = val;
            }
            _ => return Err(format!("Cannot index-assign {} with {}", target_val.type_name(), idx.type_name())),
        }
        Ok(FlowSignal::None)
    }

    fn exec_field_assign(&mut self, target: &Expr, field: &str, value: &Expr) -> Result<FlowSignal, String> {
        let target_val = self.eval_expr(target)?;
        let val = self.eval_expr(value)?;
        if let Some(rc) = target_val.as_object_ref() {
            let mut map = rc.borrow_mut();
            if let Some(existing) = map.fields.get(field) {
                let old_type = existing.type_name();
                let new_type = val.type_name();
                if old_type != new_type {
                    return Err(format!(
                        "Type mismatch: field '{field}' is {old_type}, cannot assign {new_type}"
                    ));
                }
            }
            map.fields.insert(field.to_owned(), val);
        } else {
            return Err(format!("Cannot field-assign on {}", target_val.type_name()));
        }
        Ok(FlowSignal::None)
    }

    fn exec_inc_dec(&mut self, name: &str, increment: bool) -> Result<FlowSignal, String> {
        let current = self.get_var(name)?;
        match current.kind() {
            VK::Int(n) => {
                let new_val = if increment { n + 1 } else { n - 1 };
                self.env.set(name, MaybeError::Ok(Value::int(new_val)))?;
            }
            VK::Float(n) => {
                let new_val = if increment { n + 1.0 } else { n - 1.0 };
                self.env.set(name, MaybeError::Ok(Value::float(new_val)))?;
            }
            _ => return Err(format!("Cannot increment/decrement {}", current.type_name())),
        }
        Ok(FlowSignal::None)
    }

    /// Post-increment/decrement: returns the OLD value, then mutates the variable.
    fn eval_post_inc_dec(&mut self, name: &str, increment: bool) -> Result<Value, String> {
        let current = self.get_var(name)?;
        let new_val = Self::compute_inc_dec(&current, increment)?;
        self.env.set(name, MaybeError::Ok(new_val))?;
        Ok(current) // return OLD value
    }

    /// Pre-increment/decrement: mutates the variable, then returns the NEW value.
    fn eval_pre_inc_dec(&mut self, name: &str, increment: bool) -> Result<Value, String> {
        let current = self.get_var(name)?;
        let new_val = Self::compute_inc_dec(&current, increment)?;
        self.env.set(name, MaybeError::Ok(new_val.clone()))?;
        Ok(new_val) // return NEW value
    }

    /// Compute the incremented/decremented value without modifying state.
    fn compute_inc_dec(val: &Value, increment: bool) -> Result<Value, String> {
        match val.kind() {
            VK::Int(n) => Ok(Value::int(if increment { n + 1 } else { n - 1 })),
            VK::Float(n) => Ok(Value::float(if increment { n + 1.0 } else { n - 1.0 })),
            _ => Err(format!("Cannot increment/decrement {}", val.type_name())),
        }
    }

    fn exec_assign(&mut self, name: &str, error_tolerant: bool, type_ann: Option<&TypeAnnotation>, is_dyn: bool, expr: &Expr) -> Result<FlowSignal, String> {
        let result = self.eval_expr(expr);
        if error_tolerant {
            match result {
                Ok(val) => {
                    if !is_dyn {
                        if let Some(ann) = type_ann {
                            check_type_annotation(ann, &val, name)?;
                        }
                    }
                    self.env.set_dyn(name, MaybeError::Ok(val), is_dyn)?;
                }
                Err(msg) if msg.starts_with("\x00FATAL\x00") => {
                    return Err(msg.trim_start_matches("\x00FATAL\x00").to_string());
                }
                Err(msg) => self.env.set_dyn(name, MaybeError::Err(ErrorInfo { message: msg }), is_dyn)?,
            }
        } else {
            let val = match result {
                Ok(v) => v,
                Err(msg) => return Err(msg.trim_start_matches("\x00FATAL\x00").to_string()),
            };
            if !is_dyn {
                if let Some(ann) = type_ann {
                    check_type_annotation(ann, &val, name)?;
                }
            }
            self.env.set_dyn(name, MaybeError::Ok(val), is_dyn)?;
        }
        Ok(FlowSignal::None)
    }

    fn exec_compound_assign(&mut self, name: &str, op: CompoundOp, expr: &Expr) -> Result<FlowSignal, String> {
        // Fast path: atomic int += int (lock-free)
        if op == CompoundOp::Add {
            let is_atomic = self.env.get(name).map_or(false, |v| matches!(v, MaybeError::Ok(v) if v.is_atomic()));
            if is_atomic {
                let rhs = self.eval_expr(expr)?;
                if let (Some(MaybeError::Ok(current)), Some(b)) = (self.env.get(name), rhs.as_int()) {
                    if let Some(a) = current.as_atomic() {
                        let _ = a.fetch_add(b);
                        return Ok(FlowSignal::None);
                    }
                }
            }
        }
        // Fast path: int += int, string += string (very common in loops)
        if op == CompoundOp::Add {
            let rhs = self.eval_expr(expr)?;
            // Determine types first, then drop the borrow
            let fast_path = if let Some(MaybeError::Ok(current)) = self.env.get(name) {
                match (current.kind(), rhs.kind()) {
                    (VK::Int(a), VK::Int(b)) => Some((true, a, b)),
                    (VK::String(_), VK::String(_)) => Some((false, 0, 0)),
                    _ => None,
                }
            } else {
                None
            };
            match fast_path {
                Some((true, a, b)) => {
                    self.env.set(name, MaybeError::Ok(Value::int(a + b)))?;
                    return Ok(FlowSignal::None);
                }
                Some((false, _, _)) => {
                    // String += string: try in-place append
                    let rhs_str = rhs.as_str_ref().unwrap_or("").to_string();
                    if let Some(MaybeError::Ok(current_val)) = self.env.get_mut(name) {
                        if current_val.try_string_append_in_place(&rhs_str) {
                            return Ok(FlowSignal::None);
                        }
                    }
                    // Fallback: allocate new string
                    if let Some(MaybeError::Ok(current_val)) = self.env.get(name) {
                        if let Some(a_str) = current_val.as_str_ref() {
                            let mut new_s = String::with_capacity(a_str.len() + rhs_str.len());
                            new_s.push_str(a_str);
                            new_s.push_str(&rhs_str);
                            self.env.set(name, MaybeError::Ok(Value::string_owned(new_s)))?;
                            return Ok(FlowSignal::None);
                        }
                    }
                    return Ok(FlowSignal::None);
                }
                None => {}
            }
            let current = self.get_var(name)?;
            let result = Self::apply_binop(&current, BinOp::Add, &rhs)?;
            self.env.set(name, MaybeError::Ok(result))?;
            return Ok(FlowSignal::None);
        }
        let current = self.get_var(name)?;
        let rhs = self.eval_expr(expr)?;
        let result = Self::apply_compound_op(&current, op, &rhs)?;
        self.env.set(name, MaybeError::Ok(result))?;
        Ok(FlowSignal::None)
    }

    fn exec_if(&mut self, condition: &Expr, body: &[Stmt], else_body: Option<&[Stmt]>) -> Result<FlowSignal, String> {
        let cond = self.eval_expr(condition)?;
        if cond.is_truthy() {
            self.exec_block_flow(body)
        } else if let Some(else_stmts) = else_body {
            self.exec_block_flow(else_stmts)
        } else {
            Ok(FlowSignal::None)
        }
    }

    fn exec_block_flow(&mut self, stmts: &[Stmt]) -> Result<FlowSignal, String> {
        for s in stmts {
            let flow = self.exec_stmt(s)?;
            match flow {
                FlowSignal::None => {}
                _ => return Ok(flow),
            }
        }
        Ok(FlowSignal::None)
    }

    fn exec_while(&mut self, condition: &Expr, body: &[Stmt]) -> Result<FlowSignal, String> {
        loop {
            let cond = self.eval_expr(condition)?;
            if !cond.is_truthy() { break; }
            match self.exec_loop_body(body)? {
                FlowSignal::Break => break,
                FlowSignal::Return(v) => return Ok(FlowSignal::Return(v)),
                FlowSignal::Continue | FlowSignal::None => {}
            }
        }
        Ok(FlowSignal::None)
    }

    fn exec_for(&mut self, var: &str, iter: &Expr, body: &[Stmt]) -> Result<FlowSignal, String> {
        if let ExprKind::Call { name, args, .. } = &iter.kind
            && name == "range" && (args.len() == 2 || args.len() == 3) && let Some((start, end, step)) = self.try_eval_range_args(args)?
        {
            return self.exec_range_for_loop(var, start, end, step, body);
        }

        let iterable = self.eval_expr(iter)?;
        match iterable.kind() {
            VK::List(rc) => {
                let len = rc.borrow().len();
                for idx in 0..len {
                    let item = rc.borrow()[idx].clone();
                    self.env.set(var, MaybeError::Ok(item))?;
                    match self.exec_loop_body(body)? {
                        FlowSignal::Break => break,
                        FlowSignal::Return(v) => return Ok(FlowSignal::Return(v)),
                        FlowSignal::Continue | FlowSignal::None => {}
                    }
                }
            }
            VK::String(s) => {
                let chars: Vec<char> = s.chars().collect();
                for c in chars {
                    self.env.set(var, MaybeError::Ok(Value::string_from(&c.to_string())))?;
                    match self.exec_loop_body(body)? {
                        FlowSignal::Break => break,
                        FlowSignal::Return(v) => return Ok(FlowSignal::Return(v)),
                        FlowSignal::Continue | FlowSignal::None => {}
                    }
                }
            }
            _ => return Err(format!("Cannot iterate over {}", iterable.type_name())),
        }
        Ok(FlowSignal::None)
    }

    fn exec_use(&mut self, path: &str, alias: Option<&str>) -> Result<FlowSignal, String> {
        let p = std::path::Path::new(path);
        if p.is_file() {
            let name = alias.unwrap_or_else(||
                p.file_stem().and_then(|s| s.to_str()).unwrap_or(path)
            );
            self.env.use_paths.insert(name.to_ascii_lowercase(), path.to_owned());
        } else if p.is_dir() {
            let entries = std::fs::read_dir(p)
                .map_err(|e| format!("use '{path}': {e}"))?;
            for entry in entries.flatten() {
                let ep = entry.path();
                if ep.is_file() && let Some(stem) = ep.file_stem().and_then(|s| s.to_str()) {
                    self.env.use_paths.insert(
                        stem.to_ascii_lowercase(),
                        ep.to_string_lossy().to_string(),
                    );
                }
            }
        } else {
            return Err(format!("use '{path}': path does not exist"));
        }
        Ok(FlowSignal::None)
    }

    fn exec_match(&mut self, expr: &Expr, arms: &[crate::parser::ast::MatchArm]) -> Result<FlowSignal, String> {
        let val = self.eval_expr(expr)?;
        for arm in arms {
            let matches = match &arm.pattern {
                None => true,
                Some(pattern_expr) => {
                    let pattern_val = self.eval_expr(pattern_expr)?;
                    values_match(&val, &pattern_val)
                }
            };
            if matches {
                for s in &arm.body {
                    let flow = self.exec_stmt(s)?;
                    match flow {
                        FlowSignal::None => {}
                        _ => return Ok(flow),
                    }
                }
                break;
            }
        }
        Ok(FlowSignal::None)
    }

    /// Execute a loop body. Returns: None=continue loop, Continue=skip to next, Break=exit loop, Return=propagate up
    fn exec_loop_body(&mut self, body: &[Stmt]) -> Result<FlowSignal, String> {
        for s in body {
            let flow = self.exec_stmt(s)?;
            match flow {
                FlowSignal::None => {}
                FlowSignal::Continue => return Ok(FlowSignal::Continue),
                FlowSignal::Break => return Ok(FlowSignal::Break),
                FlowSignal::Return(_) => return Ok(flow),
            }
        }
        Ok(FlowSignal::None)
    }

    fn get_var(&self, name: &str) -> Result<Value, String> {
        match self.env.get(name) {
            Some(MaybeError::Ok(val)) => {
                // Transparent atomic load for operations
                if let Some(a) = val.as_atomic() {
                    Ok(a.load())
                } else {
                    Ok(val.clone())
                }
            }
            Some(MaybeError::Err(err)) => {
                Err(format!("Variable '{name}' is in error state: {}", err.message))
            }
            None => Err(format!("Undefined variable: '{name}'")),
        }
    }

    /// Get raw variable value without unwrapping Atomic — used for lambda binding.
    fn get_var_raw(&self, name: &str) -> Result<Value, String> {
        match self.env.get(name) {
            Some(MaybeError::Ok(val)) => Ok(val.clone()),
            Some(MaybeError::Err(err)) => {
                Err(format!("Variable '{name}' is in error state: {}", err.message))
            }
            None => Err(format!("Undefined variable: '{name}'")),
        }
    }

    /// Evaluate an expression.
    ///
    /// # Errors
    ///
    /// Returns an error if the expression cannot be evaluated.
    pub fn eval_expr(&mut self, expr: &Expr) -> Result<Value, String> {
        match &expr.kind {
            ExprKind::Int(n) => Ok(Value::int(*n)),
            ExprKind::Float(n) => Ok(Value::float(*n)),
            ExprKind::Bool(b) => Ok(Value::bool(*b)),
            ExprKind::String(parts) => self.eval_string_parts(parts),
            ExprKind::List(elements) => self.eval_list(elements),
            ExprKind::Object(fields) => self.eval_object(fields),
            ExprKind::Ident(name) => self.get_var(name),
            ExprKind::BinaryOp { left, op, right } => self.eval_binary_op(left, *op, right),
            ExprKind::UnaryOp { op, expr: inner } => self.eval_unary_op(*op, inner),
            ExprKind::Call { name, resolution, args } => self.eval_call(name, *resolution, args),
            ExprKind::Index { expr: inner, index } => self.eval_index(inner, index),
            ExprKind::FieldAccess { expr: inner, field } => self.eval_field_access(inner, field),
            ExprKind::Range { start, end } => self.eval_range(start, end),
            ExprKind::Send { left, right } => self.eval_send(left, right),
            ExprKind::SafeSend { left, right } => self.eval_safe_send(left, right),
            ExprKind::Lambda { name, resolution, bound_args } => self.eval_lambda(name, *resolution, bound_args),
            ExprKind::ErrorCheck(name) => self.eval_error_check(name),
            ExprKind::ErrorField { name, field } => self.eval_error_field(name, field),
            ExprKind::DollarRef(dollar) => self.eval_dollar_ref(dollar),
            ExprKind::OptionalCheck(name) => self.eval_optional_check(name),
            ExprKind::Atomic(inner) => {
                let val = self.eval_expr(inner)?;
                Ok(Value::atomic(crate::interpreter::value::AtomicValue::new(&val)))
            }
            ExprKind::PostIncDec { name, increment } => {
                self.eval_post_inc_dec(name, *increment)
            }
            ExprKind::PreIncDec { name, increment } => {
                self.eval_pre_inc_dec(name, *increment)
            }
        }
    }

    fn eval_string_parts(&mut self, parts: &[StringPart]) -> Result<Value, String> {
        let mut result = String::new();
        for part in parts {
            match part {
                StringPart::Literal(s) => result.push_str(s),
                StringPart::Expr(e) => {
                    let val = self.eval_expr(e)?;
                    result.push_str(&val.to_string());
                }
            }
        }
        // Tilde expansion: ~/... → home/...
        if result.starts_with("~/") || result == "~" {
            let home = std::env::var("HOME")
                .or_else(|_| std::env::var("USERPROFILE"))
                .unwrap_or_default();
            result = if result == "~" {
                home
            } else {
                format!("{}{}", home, &result[1..])
            };
        }
        Ok(Value::string_from(&result))
    }

    fn eval_list(&mut self, elements: &[Expr]) -> Result<Value, String> {
        let mut items = Vec::with_capacity(elements.len());
        for e in elements {
            items.push(self.eval_expr(e)?);
        }
        Ok(new_list(items))
    }

    fn eval_object(&mut self, fields: &[(String, Expr)]) -> Result<Value, String> {
        let mut map = IndexMap::with_capacity(fields.len());
        for (key, val_expr) in fields {
            let val = self.eval_expr(val_expr)?;
            map.insert(key.clone(), val);
        }
        Ok(new_object(map))
    }

    fn eval_binary_op(&mut self, left: &Expr, op: BinOp, right: &Expr) -> Result<Value, String> {
        // Short-circuit for logical operators
        if op == BinOp::And {
            let l = self.eval_expr(left)?;
            if !l.is_truthy() { return Ok(Value::bool(false)); }
            let r = self.eval_expr(right)?;
            return Ok(Value::bool(r.is_truthy()));
        }
        if op == BinOp::Or {
            let l = self.eval_expr(left)?;
            if l.is_truthy() { return Ok(l); }
            return self.eval_expr(right);
        }

        // Fast path: obj.field == "literal" — avoid cloning the field value
        if (op == BinOp::Eq || op == BinOp::NotEq) && let Some(result) = self.try_fast_field_compare(left, op, right) {
            return Ok(result);
        }

        let l = self.eval_expr(left)?;
        let r = self.eval_expr(right)?;
        Self::apply_binop(&l, op, &r)
    }

    fn eval_unary_op(&mut self, op: UnaryOp, expr: &Expr) -> Result<Value, String> {
        let val = self.eval_expr(expr)?;
        match op {
            UnaryOp::Neg => match val.kind() {
                VK::Int(n) => Ok(Value::int(-n)),
                VK::Float(n) => Ok(Value::float(-n)),
                _ => Err(format!("Cannot negate {}", val.type_name())),
            },
            UnaryOp::Not => Ok(Value::bool(!val.is_truthy())),
            UnaryOp::BitNot => match val.kind() {
                VK::Int(n) => Ok(Value::int(!n)),
                _ => Err(format!("Cannot bitwise NOT {}", val.type_name())),
            },
        }
    }

    /// Fast path for `obj.field == "literal"` comparisons — avoids cloning.
    /// Returns None if the pattern doesn't match (fall through to normal path).
    fn try_fast_field_compare(&self, left: &Expr, op: BinOp, right: &Expr) -> Option<Value> {
        // Pattern: FieldAccess on an Ident, compared to a string/int/bool literal
        let (obj_name, field) = match &left.kind {
            ExprKind::FieldAccess { expr, field } => {
                if let ExprKind::Ident(name) = &expr.kind {
                    (name.as_str(), field.as_str())
                } else {
                    return None;
                }
            }
            _ => return None,
        };

        // Get the object without cloning
        let Some(MaybeError::Ok(obj)) = self.env.get(obj_name) else { return None; };
        let rc = obj.as_object_ref()?;

        let map = rc.borrow();
        let field_val = map.fields.get(field)?;

        // Now compare with the right side without cloning
        let eq = match (&right.kind, field_val.kind()) {
            (ExprKind::String(parts), VK::String(s)) => {
                if parts.len() == 1 {
                    if let StringPart::Literal(lit) = &parts[0] {
                        s == lit
                    } else {
                        return None;
                    }
                } else {
                    return None;
                }
            }
            (ExprKind::Int(n), VK::Int(v)) => *n == v,
            (ExprKind::Bool(b), VK::Bool(v)) => *b == v,
            _ => return None,
        };

        let result = if op == BinOp::Eq { eq } else { !eq };
        Some(Value::bool(result))
    }

    fn eval_call(&mut self, name: &str, resolution: Resolution, args: &[Expr]) -> Result<Value, String> {
        let mut eval_args = Vec::with_capacity(args.len());
        for arg in args {
            eval_args.push(self.eval_expr(arg)?);
        }
        self.call_resolved(name, resolution, eval_args)
    }

    fn eval_index(&mut self, expr: &Expr, index: &Expr) -> Result<Value, String> {
        let val = self.eval_expr(expr)?;
        let idx = self.eval_expr(index)?;
        match (val.kind(), idx.kind()) {
            (VK::List(list), VK::Int(i)) => {
                let list = list.borrow();
                let idx = usize::try_from(i).map_err(|_| format!("Negative index {i}"))?;
                list.get(idx).cloned().ok_or_else(|| format!("Index {idx} out of bounds (len {})", list.len()))
            }
            (VK::String(s), VK::Int(i)) => {
                let idx = usize::try_from(i).map_err(|_| format!("Negative index {i}"))?;
                s.chars().nth(idx)
                    .map(|c| Value::string_from(&c.to_string()))
                    .ok_or_else(|| format!("Index {idx} out of bounds"))
            }
            (VK::Object(rc), VK::String(key)) => {
                rc.borrow().fields.get(key).cloned().ok_or_else(|| format!("Field '{key}' not found"))
            }
            _ => Err(format!("Cannot index {} with {}", val.type_name(), idx.type_name())),
        }
    }

    fn eval_field_access(&mut self, expr: &Expr, field: &str) -> Result<Value, String> {
        let val = self.eval_expr(expr)?;
        match val.kind() {
            VK::Object(rc) => {
                rc.borrow().fields.get(field).cloned().ok_or_else(|| format!("Field '{field}' not found"))
            }
            VK::CommandResult(data) => {
                match field {
                    "status" => Ok(Value::int(i64::from(data.status))),
                    "out" => Ok(Value::string_from(&data.out)),
                    "err" => Ok(Value::string_from(&data.err)),
                    _ => Err(format!("CommandResult has no field '{field}'")),
                }
            }
            _ => Err(format!("Cannot access field on {}", val.type_name())),
        }
    }

    fn eval_range(&mut self, start: &Expr, end: &Expr) -> Result<Value, String> {
        let s = self.eval_expr(start)?;
        let e = self.eval_expr(end)?;
        match (s.kind(), e.kind()) {
            (VK::Int(a), VK::Int(b)) => {
                let items: Vec<Value> = (a..=b).map(Value::int).collect();
                Ok(new_list(items))
            }
            _ => Err(format!("Range requires int..int, got {}..{}", s.type_name(), e.type_name())),
        }
    }

    fn eval_send(&mut self, left: &Expr, right: &Expr) -> Result<Value, String> {
        let lhs_val = match self.eval_expr(left) {
            Ok(val) => val,
            Err(e) => return Err(format!("\x00FATAL\x00{e}")),
        };
        let prev_send = self.send_value.take();
        self.send_value = Some(lhs_val);
        let result = match self.eval_expr(right) {
            Ok(val) => Ok(val),
            Err(e) => Err(format!("\x00FATAL\x00{e}")),
        };
        self.send_value = prev_send;
        result
    }

    fn eval_safe_send(&mut self, left: &Expr, right: &Expr) -> Result<Value, String> {
        let lhs_val = self.eval_expr(left)?;
        let prev_send = self.send_value.take();
        self.send_value = Some(lhs_val);
        let result = self.eval_expr(right);
        self.send_value = prev_send;
        result
    }

    fn eval_lambda(&mut self, name: &str, resolution: Resolution, bound_args: &[Expr]) -> Result<Value, String> {
        let mut eval_args = Vec::with_capacity(bound_args.len());
        for arg in bound_args {
            // For lambda binding, preserve Atomic values (don't unwrap)
            if let ExprKind::Ident(var_name) = &arg.kind {
                eval_args.push(self.get_var_raw(var_name)?);
            } else {
                eval_args.push(self.eval_expr(arg)?);
            }
        }
        let res_code = match resolution {
            Resolution::Normal => 0,
            Resolution::OwnFirst => 1,
            Resolution::SystemOnly => 2,
        };
        Ok(Value::lambda(super::value::LambdaData {
            name: name.to_owned(),
            resolution: res_code,
            bound_args: eval_args,
        }))
    }

    fn eval_error_check(&self, name: &str) -> Result<Value, String> {
        match self.env.get(name) {
            Some(MaybeError::Ok(_)) => Ok(Value::bool(true)),
            Some(MaybeError::Err(_)) => Ok(Value::bool(false)),
            None => Err(format!("Undefined variable: '{name}'")),
        }
    }

    fn eval_optional_check(&self, name: &str) -> Result<Value, String> {
        // <param> evaluates to true if the optional param was provided (not void), false otherwise.
        match self.env.get(name) {
            Some(MaybeError::Ok(v)) if v.is_void() => Ok(Value::bool(false)),
            Some(MaybeError::Ok(_)) => Ok(Value::bool(true)),
            Some(MaybeError::Err(_)) => Ok(Value::bool(true)), // provided but error
            None => Err(format!("'<{name}>' used outside of a function that declares '{name}' as optional")),
        }
    }

    fn eval_error_field(&self, name: &str, field: &str) -> Result<Value, String> {
        match self.env.get(name) {
            Some(MaybeError::Err(err)) => match field {
                "error" | "message" => Ok(Value::string_from(&err.message)),
                _ => Err(format!("Error has no field '{field}', use 'error' or 'message'")),
            },
            Some(MaybeError::Ok(_)) => Err(format!("'{name}' is not in error state")),
            None => Err(format!("Undefined variable: '{name}'")),
        }
    }

    fn eval_dollar_ref(&self, dollar: &DollarRef) -> Result<Value, String> {
        let send_val = self.send_value.as_ref()
            .ok_or("$ used outside of send (->) context")?;
        match dollar {
            DollarRef::Whole => Ok(send_val.clone()),
            DollarRef::Index(i) => {
                if let Some(list) = send_val.as_list_ref() {
                    list.borrow().get(*i).cloned().ok_or_else(|| format!("${i} out of bounds"))
                } else {
                    Err(format!("${i} requires a list, got {}", send_val.type_name()))
                }
            }
            DollarRef::Field(field) => {
                match send_val.kind() {
                    VK::Object(rc) => {
                        rc.borrow().fields.get(field).cloned().ok_or_else(|| format!("${field} not found"))
                    }
                    VK::CommandResult(data) => {
                        match field.as_str() {
                            "status" => Ok(Value::int(i64::from(data.status))),
                            "out" => Ok(Value::string_from(&data.out)),
                            "err" => Ok(Value::string_from(&data.err)),
                            _ => Err(format!("${field} not found on CommandResult")),
                        }
                    }
                    _ => Err(format!("${field} requires an object, got {}", send_val.type_name())),
                }
            }
        }
    }

    /// Call a builtin by name. Uses validate_and_get_handler to avoid borrow conflicts.
    fn call_builtin(&mut self, name: &str, args: &[Value]) -> Option<Result<Value, String>> {
        let handler = match self.registry.validate_and_get_handler(name, args)? {
            Ok(h) => h,
            Err(e) => return Some(Err(e)),
        };
        Some(handler(args, self))
    }

    /// Call a lambda value with given arguments. Used by builtins like map/filter.
    ///
    /// # Errors
    ///
    /// Returns an error if the lambda call fails.
    pub fn call_lambda(&mut self, lambda: &Value, args: Vec<Value>) -> Result<Value, String> {
        if let Some(data) = lambda.as_lambda() {
            if !data.bound_args.is_empty() && !args.is_empty() {
                return Err(format!("Lambda @{} already has bound args", data.name));
            }
            let call_args = if data.bound_args.is_empty() { args } else { data.bound_args.clone() };
            let res = match data.resolution {
                1 => Resolution::OwnFirst,
                2 => Resolution::SystemOnly,
                _ => Resolution::Normal,
            };
            self.call_resolved(&data.name, res, call_args)
        } else {
            Err(format!("Expected lambda, got {}", lambda.type_name()))
        }
    }

    /// Call a dgsh function by name. Looks up user functions first, then builtins.
    pub fn call_function(&mut self, name: &str, args: Vec<Value>) -> Result<Value, String> {
        let lower_cow = crate::interpreter::env::to_lower_pub(name);
        let lower = lower_cow.as_ref();
        if let Some(result) = self.call_user_fn(lower, args.clone()) {
            return result;
        }
        if let Some(result) = self.call_builtin(lower, &args) {
            return result;
        }
        Err(format!("Undefined function: '{name}'"))
    }

    /// Internal: call with full resolution (alias → exe → own → system).
    pub(crate) fn call_resolved(&mut self, name: &str, resolution: Resolution, args: Vec<Value>) -> Result<Value, String> {
        let lower_cow = crate::interpreter::env::to_lower_pub(name);
        let lower = lower_cow.as_ref();

        // Check if name is a variable holding a lambda
        if let Some(MaybeError::Ok(val)) = self.env.get(lower) {
            if let Some(data) = val.as_lambda() {
                let data = data.clone();
                if !data.bound_args.is_empty() && !args.is_empty() {
                    return Err(format!("Lambda '{name}' already has bound args, cannot pass additional args"));
                }
                let call_args = if data.bound_args.is_empty() { args } else { data.bound_args };
                let lambda_resolution = match data.resolution {
                    1 => Resolution::OwnFirst,
                    2 => Resolution::SystemOnly,
                    _ => Resolution::Normal,
                };
                return self.call_resolved(&data.name, lambda_resolution, call_args);
            }
        }

        match resolution {
            Resolution::Normal => {
                // alias → use_paths → exe → own → system
                if let Some(target) = self.env.aliases.get(lower).cloned() {
                    return exec::exec_path(&target, &args);
                }
                if let Some(use_path) = self.env.use_paths.get(lower).cloned() {
                    return exec::exec_path(&use_path, &args);
                }
                if let Some(result) = exec::try_exec_command(lower, &args) {
                    return result;
                }
                if let Some(result) = self.call_user_fn(lower, args.clone()) {
                    return result;
                }
                if let Some(result) = self.call_builtin(lower, &args) {
                    return result;
                }
                Err(format!("Undefined: '{name}' (not found as exe, function, or built-in)"))
            }
            Resolution::OwnFirst => {
                // own → system
                if let Some(result) = self.call_user_fn(lower, args.clone()) {
                    return result;
                }
                if let Some(result) = self.call_builtin(lower, &args) {
                    return result;
                }
                Err(format!("Undefined: '{name}' (not found as function or built-in)"))
            }
            Resolution::SystemOnly => {
                // system only
                if let Some(result) = self.call_builtin(lower, &args) {
                    return result;
                }
                Err(format!("Undefined: '{name}' (not a built-in function)"))
            }
        }
    }

    fn call_user_fn(&mut self, name: &str, args: Vec<Value>) -> Option<Result<Value, String>> {
        let func = self.env.get_fn(name)?.clone();

        let required_count = func.params.len();
        let optional_count = func.optional_params.len();
        let total_count = required_count + optional_count;

        if args.len() < required_count {
            return Some(Err(format!(
                "'{}' expects at least {} arg(s), got {}",
                name, required_count, args.len()
            )));
        }
        if args.len() > total_count {
            return Some(Err(format!(
                "'{}' expects at most {} arg(s), got {}",
                name, total_count, args.len()
            )));
        }

        self.env.push_scope();
        // Bind required params: check annotation > inferred type (skip if dyn)
        for ((param, ann, is_dyn), val) in func.params.iter().zip(args.iter()) {
            if !is_dyn {
                if let Some(ann) = ann {
                    if let Err(e) = check_type_annotation(ann, val, param) {
                        self.env.pop_scope();
                        return Some(Err(e));
                    }
                } else if let Some(inferred) = func.inferred_types.get(param) {
                    if let Err(_) = check_type_annotation(inferred, val, param) {
                        let source = if func.body_inferred_params.contains(param) {
                            "inferred from body"
                        } else {
                            "inferred from first call"
                        };
                        self.env.pop_scope();
                        return Some(Err(format!(
                            "Type error: '{}' of '{}' {} as {}, got {}",
                            param, name, source, inferred.type_name(), val.type_name()
                        )));
                    }
                }
            }
            self.env.set_local(param, MaybeError::Ok(val.clone()));
        }
        // Bind optional params: check annotation > inferred type (skip if dyn)
        for (i, (opt_param, ann, is_dyn)) in func.optional_params.iter().enumerate() {
            let val = args.get(required_count + i).cloned().unwrap_or(Value::void());
            if !is_dyn && !val.is_void() {
                if let Some(ann) = ann {
                    if let Err(e) = check_type_annotation(ann, &val, opt_param) {
                        self.env.pop_scope();
                        return Some(Err(e));
                    }
                } else if let Some(inferred) = func.inferred_types.get(opt_param) {
                    if let Err(_) = check_type_annotation(inferred, &val, opt_param) {
                        let source = if func.body_inferred_params.contains(opt_param) {
                            "inferred from body"
                        } else {
                            "inferred from first call"
                        };
                        self.env.pop_scope();
                        return Some(Err(format!(
                            "Type error: '{}' of '{}' {} as {}, got {}",
                            opt_param, name, source, inferred.type_name(), val.type_name()
                        )));
                    }
                }
            }
            self.env.set_local(opt_param, MaybeError::Ok(val));
        }

        let mut return_val = Value::void();
        for stmt in &func.body {
            match self.exec_stmt(stmt) {
                Ok(FlowSignal::Return(Some(val))) => {
                    return_val = val;
                    break;
                }
                Ok(FlowSignal::Return(None)) => break,
                Ok(FlowSignal::None | FlowSignal::Continue | FlowSignal::Break) => {}
                Err(e) => {
                    self.env.pop_scope();
                    return Some(Err(e));
                }
            }
        }

        self.env.pop_scope();

        // Enforce declared return type annotation using the already-cloned func snapshot
        if let Some(ann) = &func.declared_return_type
            && !return_val.is_void()
            && let Err(e) = check_type_annotation(ann, &return_val, &format!("return value of '{name}'")) {
                return Some(Err(e));
            }

        // Enforce return type consistency (inferred, for legacy behaviour)
        // Skip if the function uses `dyn return` — different calls may return different types.
        if func.declared_return_type.is_none() && !func.has_dyn_return {
            let ret_type = return_val.type_name();
            // Fast path: type is already recorded in the snapshot and matches — skip lookup.
            let already_matches = func.return_type.as_deref()
                .map(|expected| ret_type == expected || ret_type == "void" || expected == "void")
                .unwrap_or(false);
            if !already_matches {
                let fn_name_lower = name.to_ascii_lowercase();
                if let Some(live_func) = self.env.functions.get_mut(&fn_name_lower) {
                    if let Some(ref expected) = live_func.return_type {
                        if ret_type != *expected && ret_type != "void" && expected != "void" {
                            return Some(Err(format!(
                                "Function '{name}' return type mismatch: expected {expected}, got {ret_type}"
                            )));
                        }
                    } else {
                        live_func.return_type = Some(ret_type.to_string());
                    }
                }
            }
        }

        // Call-site inference: record types for unannotated, non-dyn params on first call
        {
            let fn_name_lower = name.to_ascii_lowercase();
            if let Some(live_func) = self.env.functions.get_mut(&fn_name_lower) {
                for ((param, ann, is_dyn), val) in func.params.iter().zip(args.iter()) {
                    if *is_dyn || ann.is_some() || live_func.inferred_types.contains_key(param) {
                        continue;
                    }
                    live_func.inferred_types.insert(
                        param.clone(),
                        TypeAnnotation::Simple(val.type_name().to_string()),
                    );
                }
                for (i, (param, ann, is_dyn)) in func.optional_params.iter().enumerate() {
                    if *is_dyn || ann.is_some() || live_func.inferred_types.contains_key(param) {
                        continue;
                    }
                    if let Some(val) = args.get(required_count + i) {
                        if !val.is_void() {
                            live_func.inferred_types.insert(
                                param.clone(),
                                TypeAnnotation::Simple(val.type_name().to_string()),
                            );
                        }
                    }
                }
            }
        }

        Some(Ok(return_val))
    }

    /// Try to evaluate `range()` args to (start, end, step) integers.
    /// Returns Ok(None) if args aren't all ints (fall through to normal path).
    fn try_eval_range_args(&mut self, args: &[Expr]) -> Result<Option<(i64, i64, i64)>, String> {
        let start = self.eval_expr(&args[0])?;
        let end = self.eval_expr(&args[1])?;
        let step = if args.len() == 3 {
            self.eval_expr(&args[2])?
        } else {
            Value::int(1)
        };
        match (start.kind(), end.kind(), step.kind()) {
            (VK::Int(s), VK::Int(e), VK::Int(st)) => {
                if st == 0 {
                    return Err("range() step cannot be 0".to_string());
                }
                Ok(Some((s, e, st)))
            }
            _ => Ok(None),
        }
    }

    /// Execute a for loop with a direct integer range — no list allocation.
    fn exec_range_for_loop(
        &mut self,
        var: &str,
        start: i64,
        end: i64,
        step: i64,
        body: &[Stmt],
    ) -> Result<FlowSignal, String> {
        let mut i = start;
        if step > 0 {
            while i <= end {
                self.env.set(var, MaybeError::Ok(Value::int(i)))?;
                match self.exec_loop_body(body)? {
                    FlowSignal::Break => break,
                    FlowSignal::Return(v) => return Ok(FlowSignal::Return(v)),
                    FlowSignal::Continue | FlowSignal::None => {}
                }
                i += step;
            }
        } else {
            while i >= end {
                self.env.set(var, MaybeError::Ok(Value::int(i)))?;
                match self.exec_loop_body(body)? {
                    FlowSignal::Break => break,
                    FlowSignal::Return(v) => return Ok(FlowSignal::Return(v)),
                    FlowSignal::Continue | FlowSignal::None => {}
                }
                i += step;
            }
        }
        Ok(FlowSignal::None)
    }

    fn apply_binop(left: &Value, op: BinOp, right: &Value) -> Result<Value, String> {
        // Type-strict comparisons
        match op {
            BinOp::Eq => {
                if left.type_name() != right.type_name() {
                    return Err(format!("Type mismatch: cannot compare {} with {}", left.type_name(), right.type_name()));
                }
                return Ok(Value::bool(values_equal(left, right)));
            }
            BinOp::NotEq => {
                if left.type_name() != right.type_name() {
                    return Err(format!("Type mismatch: cannot compare {} with {}", left.type_name(), right.type_name()));
                }
                return Ok(Value::bool(!values_equal(left, right)));
            }
            _ => {}
        }

        match (left.kind(), right.kind()) {
            // Int arithmetic
            (VK::Int(a), VK::Int(b)) => Self::apply_int_op(a, op, b),

            // Float arithmetic
            (VK::Float(a), VK::Float(b)) => apply_float_op(a, op, b),

            // Int + Float promotion
            (VK::Int(a), VK::Float(b)) => {
                let a_f64 = a as f64;
                apply_float_op(a_f64, op, b)
            }
            (VK::Float(a), VK::Int(b)) => {
                let b_f64 = b as f64;
                apply_float_op(a, op, b_f64)
            }

            // String concatenation
            (VK::String(a), VK::String(b)) => match op {
                BinOp::Add => {
                    let mut s = String::with_capacity(a.len() + b.len());
                    s.push_str(a);
                    s.push_str(b);
                    Ok(Value::string_owned(s))
                }
                BinOp::Lt => Ok(Value::bool(a < b)),
                BinOp::Gt => Ok(Value::bool(a > b)),
                BinOp::LtEq => Ok(Value::bool(a <= b)),
                BinOp::GtEq => Ok(Value::bool(a >= b)),
                _ => Err(format!("Unsupported operation: string {op:?} string")),
            },

            // List concatenation
            (VK::List(a), VK::List(b)) => match op {
                BinOp::Add => {
                    let mut result = a.borrow().clone();
                    result.extend(b.borrow().iter().cloned());
                    Ok(new_list(result))
                }
                _ => Err(format!("Unsupported operation: list {op:?} list")),
            },

            _ => Err(format!(
                "Type mismatch: cannot apply {op:?} to {} and {}",
                left.type_name(), right.type_name()
            )),
        }
    }

    fn apply_int_op(a: i64, op: BinOp, b: i64) -> Result<Value, String> {
        match op {
            BinOp::Add => Ok(Value::int(a + b)),
            BinOp::Sub => Ok(Value::int(a - b)),
            BinOp::Mul => Ok(Value::int(a * b)),
            BinOp::Div => {
                if b == 0 { return Err("Division by zero".to_string()); }
                Ok(Value::int(a / b))
            }
            BinOp::Mod => {
                if b == 0 { return Err("Modulo by zero".to_string()); }
                Ok(Value::int(a % b))
            }
            BinOp::Pow => {
                let exp = u32::try_from(b).map_err(|_| format!("Exponent {b} out of range for integer pow"))?;
                Ok(Value::int(a.pow(exp)))
            }
            BinOp::Lt => Ok(Value::bool(a < b)),
            BinOp::Gt => Ok(Value::bool(a > b)),
            BinOp::LtEq => Ok(Value::bool(a <= b)),
            BinOp::GtEq => Ok(Value::bool(a >= b)),
            BinOp::BitAnd => Ok(Value::int(a & b)),
            BinOp::BitOr => Ok(Value::int(a | b)),
            BinOp::BitXor => Ok(Value::int(a ^ b)),
            BinOp::Shl => Ok(Value::int(a << b)),
            BinOp::Shr => Ok(Value::int(a >> b)),
            _ => Err(format!("Unsupported operation: int {op:?} int")),
        }
    }

    fn apply_compound_op(current: &Value, op: CompoundOp, rhs: &Value) -> Result<Value, String> {
        let bin_op = match op {
            CompoundOp::Add => BinOp::Add,
            CompoundOp::Sub => BinOp::Sub,
            CompoundOp::Mul => BinOp::Mul,
            CompoundOp::Div => BinOp::Div,
            CompoundOp::Mod => BinOp::Mod,
            CompoundOp::Pow => BinOp::Pow,
            CompoundOp::BitAnd => BinOp::BitAnd,
            CompoundOp::BitOr => BinOp::BitOr,
            CompoundOp::BitXor => BinOp::BitXor,
            CompoundOp::Shl => BinOp::Shl,
            CompoundOp::Shr => BinOp::Shr,
        };
        Self::apply_binop(current, bin_op, rhs)
    }
}

fn apply_float_op(a: f64, op: BinOp, b: f64) -> Result<Value, String> {
    match op {
        BinOp::Add => Ok(Value::float(a + b)),
        BinOp::Sub => Ok(Value::float(a - b)),
        BinOp::Mul => Ok(Value::float(a * b)),
        BinOp::Div => Ok(Value::float(a / b)),
        BinOp::Mod => Ok(Value::float(a % b)),
        BinOp::Pow => Ok(Value::float(a.powf(b))),
        BinOp::Lt => Ok(Value::bool(a < b)),
        BinOp::Gt => Ok(Value::bool(a > b)),
        BinOp::LtEq => Ok(Value::bool(a <= b)),
        BinOp::GtEq => Ok(Value::bool(a >= b)),
        _ => Err(format!("Unsupported operation: float {op:?} float")),
    }
}

fn values_match(a: &Value, b: &Value) -> bool {
    values_equal(a, b)
}

/// Validate `val` against a `TypeAnnotation`, returning a non-catchable error on mismatch.
/// The `context` string is used in the error message (e.g. a variable name or "return value of 'fn'").
fn check_type_annotation(ann: &TypeAnnotation, val: &Value, context: &str) -> Result<(), String> {
    match ann {
        TypeAnnotation::Simple(expected_type) => {
            let actual_type = val.type_name();
            let matches = actual_type == expected_type
                || (expected_type == "number" && (actual_type == "int" || actual_type == "float"));
            if !matches {
                return Err(format!(
                    "Type error: '{context}' declared as {expected_type}, got {actual_type}"
                ));
            }
        }
        TypeAnnotation::Object(fields) => {
            let Some(rc) = val.as_object_ref() else {
                return Err(format!(
                    "Type error: '{context}' declared as object, got {}",
                    val.type_name()
                ));
            };
            let map = rc.borrow();
            for (field_name, expected_type) in fields {
                match map.fields.get(field_name) {
                    None => {
                        return Err(format!(
                            "Type error: '{context}' object is missing field '{field_name}'"
                        ));
                    }
                    Some(field_val) => {
                        let actual_type = field_val.type_name();
                        if actual_type != expected_type {
                            return Err(format!(
                                "Type error: field '{field_name}' of '{context}' declared as {expected_type}, got {actual_type}"
                            ));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn values_equal(a: &Value, b: &Value) -> bool {
    match (a.kind(), b.kind()) {
        (VK::Int(x), VK::Int(y)) => x == y,
        (VK::Float(x), VK::Float(y)) => (x - y).abs() < f64::EPSILON,
        (VK::String(x), VK::String(y)) => x == y,
        (VK::Bool(x), VK::Bool(y)) => x == y,
        (VK::Void, VK::Void) => true,
        _ => false,
    }
}

/// Recursively check if a function body contains any `dyn return` statement.
fn body_has_dyn_return(stmts: &[Stmt]) -> bool {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Return { is_dyn: true, .. } => return true,
            StmtKind::If { body, else_body, .. } => {
                if body_has_dyn_return(body) { return true; }
                if let Some(eb) = else_body {
                    if body_has_dyn_return(eb) { return true; }
                }
            }
            StmtKind::While { body, .. } | StmtKind::For { body, .. } => {
                if body_has_dyn_return(body) { return true; }
            }
            StmtKind::Match { arms, .. } => {
                for arm in arms {
                    if body_has_dyn_return(&arm.body) { return true; }
                }
            }
            _ => {}
        }
    }
    false
}

/// Infer parameter types by walking the function body and checking what builtins/user-fns
/// each parameter is passed to. Returns (inferred_types, body_inferred_param_names).
fn infer_param_types_from_body(
    body: &[Stmt],
    params: &[(String, Option<TypeAnnotation>, bool)],
    optional_params: &[(String, Option<TypeAnnotation>, bool)],
    env: &Environment,
    reg: &crate::builtins::registry::BuiltinRegistry,
) -> (HashMap<String, TypeAnnotation>, HashSet<String>) {
    // Collect candidates: unannotated, non-dyn params
    let mut candidates: HashSet<String> = HashSet::new();
    for (name, ann, is_dyn) in params.iter().chain(optional_params.iter()) {
        if !is_dyn && ann.is_none() {
            candidates.insert(name.clone());
        }
    }
    if candidates.is_empty() {
        return (HashMap::new(), HashSet::new());
    }

    let mut inferred: HashMap<String, TypeAnnotation> = HashMap::new();
    walk_stmts_for_inference(body, &candidates, &mut inferred, env, reg);

    let body_inferred: HashSet<String> = inferred.keys().cloned().collect();
    (inferred, body_inferred)
}

fn walk_stmts_for_inference(
    stmts: &[Stmt],
    candidates: &HashSet<String>,
    inferred: &mut HashMap<String, TypeAnnotation>,
    env: &Environment,
    reg: &crate::builtins::registry::BuiltinRegistry,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::ExprStmt(expr) => walk_expr_for_inference(expr, candidates, inferred, env, reg),
            StmtKind::Assign { expr, .. } => walk_expr_for_inference(expr, candidates, inferred, env, reg),
            StmtKind::Return { expr: Some(expr), .. } => walk_expr_for_inference(expr, candidates, inferred, env, reg),
            StmtKind::If { condition, body, else_body } => {
                walk_expr_for_inference(condition, candidates, inferred, env, reg);
                walk_stmts_for_inference(body, candidates, inferred, env, reg);
                if let Some(eb) = else_body {
                    walk_stmts_for_inference(eb, candidates, inferred, env, reg);
                }
            }
            StmtKind::While { condition, body } => {
                walk_expr_for_inference(condition, candidates, inferred, env, reg);
                walk_stmts_for_inference(body, candidates, inferred, env, reg);
            }
            StmtKind::For { iter, body, .. } => {
                walk_expr_for_inference(iter, candidates, inferred, env, reg);
                walk_stmts_for_inference(body, candidates, inferred, env, reg);
            }
            StmtKind::Match { expr, arms } => {
                walk_expr_for_inference(expr, candidates, inferred, env, reg);
                for arm in arms {
                    walk_stmts_for_inference(&arm.body, candidates, inferred, env, reg);
                }
            }
            StmtKind::Throw(expr) => walk_expr_for_inference(expr, candidates, inferred, env, reg),
            _ => {}
        }
    }
}

fn walk_expr_for_inference(
    expr: &Expr,
    candidates: &HashSet<String>,
    inferred: &mut HashMap<String, TypeAnnotation>,
    env: &Environment,
    reg: &crate::builtins::registry::BuiltinRegistry,
) {

    match &expr.kind {
        ExprKind::Call { name, args, .. } => {
            // Check builtin signatures
            if let Some(params) = reg.params(name) {
                for (i, arg) in args.iter().enumerate() {
                    if let ExprKind::Ident(ident) = &arg.kind {
                        if i < params.len() && candidates.contains(ident) && !inferred.contains_key(ident) {
                            let ty = params[i].param_type();
                            if ty != crate::builtins::registry::Type::Dyn {
                                inferred.insert(ident.clone(), TypeAnnotation::Simple(ty.name().to_string()));
                            }
                        }
                    }
                    walk_expr_for_inference(arg, candidates, inferred, env, reg);
                }
            } else if let Some(user_fn) = env.get_fn(name) {
                let all_params: Vec<_> = user_fn.params.iter().chain(user_fn.optional_params.iter()).collect();
                for (i, arg) in args.iter().enumerate() {
                    if let ExprKind::Ident(ident) = &arg.kind {
                        if i < all_params.len() && candidates.contains(ident) && !inferred.contains_key(ident) {
                            let (_, ann, _) = all_params[i];
                            if let Some(ann) = ann {
                                inferred.insert(ident.clone(), ann.clone());
                            } else if let Some(inf) = user_fn.inferred_types.get(&all_params[i].0) {
                                inferred.insert(ident.clone(), inf.clone());
                            }
                        }
                    }
                    walk_expr_for_inference(arg, candidates, inferred, env, reg);
                }
            } else {
                for arg in args {
                    walk_expr_for_inference(arg, candidates, inferred, env, reg);
                }
            }
        }
        ExprKind::BinaryOp { left, right, .. } => {
            walk_expr_for_inference(left, candidates, inferred, env, reg);
            walk_expr_for_inference(right, candidates, inferred, env, reg);
        }
        ExprKind::UnaryOp { expr: inner, .. } => {
            walk_expr_for_inference(inner, candidates, inferred, env, reg);
        }
        ExprKind::Index { expr: inner, index } => {
            walk_expr_for_inference(inner, candidates, inferred, env, reg);
            walk_expr_for_inference(index, candidates, inferred, env, reg);
        }
        ExprKind::FieldAccess { expr: inner, .. } => {
            walk_expr_for_inference(inner, candidates, inferred, env, reg);
        }
        ExprKind::Send { left, right } | ExprKind::SafeSend { left, right } => {
            walk_expr_for_inference(left, candidates, inferred, env, reg);
            walk_expr_for_inference(right, candidates, inferred, env, reg);
        }
        ExprKind::List(items) => {
            for item in items {
                walk_expr_for_inference(item, candidates, inferred, env, reg);
            }
        }
        ExprKind::Object(fields) => {
            for (_, val) in fields {
                walk_expr_for_inference(val, candidates, inferred, env, reg);
            }
        }
        _ => {}
    }
}
