pub mod value;
pub mod env;

use indexmap::IndexMap;
use value::{Value, MaybeError, ErrorInfo};
use env::{Environment, UserFn};
use crate::parser::ast::{
    BinOp, CompoundOp, DollarRef, Expr, ExprKind, Resolution, Stmt, StmtKind, StringPart,
    UnaryOp,
};
use crate::builtins;
use crate::exec;

pub struct Interpreter {
    pub env: Environment,
    /// Current dollar value in send context
    send_value: Option<Value>,
}

/// Return control flow signal
enum FlowSignal {
    None,
    Return(Option<Value>),
}

impl Default for Interpreter {
    fn default() -> Self {
        Self::new()
    }
}

impl Interpreter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            env: Environment::new(),
            send_value: None,
        }
    }

    /// Execute a list of statements.
    ///
    /// # Errors
    ///
    /// Returns an error if any statement fails to execute.
    pub fn run(&mut self, stmts: &[Stmt]) -> Result<(), String> {
        for stmt in stmts {
            if let FlowSignal::Return(_) = self.exec_stmt(stmt)? {
                break;
            }
        }
        Ok(())
    }

    fn exec_stmt(&mut self, stmt: &Stmt) -> Result<FlowSignal, String> {
        match &stmt.kind {
            StmtKind::Assign { name, error_tolerant, expr } => {
                let result = self.eval_expr(expr);
                if *error_tolerant {
                    match result {
                        Ok(val) => self.env.set(name, MaybeError::Ok(val)),
                        Err(msg) => self.env.set(name, MaybeError::Err(ErrorInfo { message: msg })),
                    }
                } else {
                    let val = result?;
                    self.env.set(name, MaybeError::Ok(val));
                }
                Ok(FlowSignal::None)
            }

            StmtKind::CompoundAssign { name, op, expr } => {
                let current = self.get_var(name)?;
                let rhs = self.eval_expr(expr)?;
                let result = Self::apply_compound_op(&current, *op, &rhs)?;
                self.env.set(name, MaybeError::Ok(result));
                Ok(FlowSignal::None)
            }

            StmtKind::ExprStmt(expr) => {
                self.eval_expr(expr)?;
                Ok(FlowSignal::None)
            }

            StmtKind::FnDef { name, params, body } => {
                self.env.define_fn(UserFn {
                    name: name.clone(),
                    params: params.clone(),
                    body: body.clone(),
                });
                Ok(FlowSignal::None)
            }

            StmtKind::If { condition, body, else_body } => {
                let cond = self.eval_expr(condition)?;
                if cond.is_truthy() {
                    for s in body {
                        let flow = self.exec_stmt(s)?;
                        if let FlowSignal::Return(_) = &flow {
                            return Ok(flow);
                        }
                    }
                } else if let Some(else_stmts) = else_body {
                    for s in else_stmts {
                        let flow = self.exec_stmt(s)?;
                        if let FlowSignal::Return(_) = &flow {
                            return Ok(flow);
                        }
                    }
                }
                Ok(FlowSignal::None)
            }

            StmtKind::While { condition, body } => {
                loop {
                    let cond = self.eval_expr(condition)?;
                    if !cond.is_truthy() { break; }
                    for s in body {
                        let flow = self.exec_stmt(s)?;
                        if let FlowSignal::Return(_) = &flow {
                            return Ok(flow);
                        }
                    }
                }
                Ok(FlowSignal::None)
            }

            StmtKind::For { var, iter, body } => {
                let iterable = self.eval_expr(iter)?;
                let items = match iterable {
                    Value::List(items) => items,
                    Value::String(s) => s.chars().map(|c| Value::String(c.to_string())).collect(),
                    _ => return Err(format!("Cannot iterate over {}", iterable.type_name())),
                };
                for item in items {
                    self.env.set(var, MaybeError::Ok(item));
                    for s in body {
                        let flow = self.exec_stmt(s)?;
                        if let FlowSignal::Return(_) = &flow {
                            return Ok(flow);
                        }
                    }
                }
                Ok(FlowSignal::None)
            }

            StmtKind::Return(expr) => {
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
        }
    }

    fn get_var(&self, name: &str) -> Result<Value, String> {
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
            ExprKind::Int(n) => Ok(Value::Int(*n)),
            ExprKind::Float(n) => Ok(Value::Float(*n)),
            ExprKind::Bool(b) => Ok(Value::Bool(*b)),
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
            ExprKind::Lambda { name, resolution, bound_args } => self.eval_lambda(name, *resolution, bound_args),
            ExprKind::ErrorCheck(name) => self.eval_error_check(name),
            ExprKind::DollarRef(dollar) => self.eval_dollar_ref(dollar),
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
        Ok(Value::String(result))
    }

    fn eval_list(&mut self, elements: &[Expr]) -> Result<Value, String> {
        let mut items = Vec::with_capacity(elements.len());
        for e in elements {
            items.push(self.eval_expr(e)?);
        }
        Ok(Value::List(items))
    }

    fn eval_object(&mut self, fields: &[(String, Expr)]) -> Result<Value, String> {
        let mut map = IndexMap::with_capacity(fields.len());
        for (key, val_expr) in fields {
            let val = self.eval_expr(val_expr)?;
            map.insert(key.clone(), val);
        }
        Ok(Value::Object(map))
    }

    fn eval_binary_op(&mut self, left: &Expr, op: BinOp, right: &Expr) -> Result<Value, String> {
        // Short-circuit for logical operators
        if op == BinOp::And {
            let l = self.eval_expr(left)?;
            if !l.is_truthy() { return Ok(Value::Bool(false)); }
            let r = self.eval_expr(right)?;
            return Ok(Value::Bool(r.is_truthy()));
        }
        if op == BinOp::Or {
            let l = self.eval_expr(left)?;
            if l.is_truthy() { return Ok(l); }
            return self.eval_expr(right);
        }

        let l = self.eval_expr(left)?;
        let r = self.eval_expr(right)?;
        Self::apply_binop(&l, op, &r)
    }

    fn eval_unary_op(&mut self, op: UnaryOp, expr: &Expr) -> Result<Value, String> {
        let val = self.eval_expr(expr)?;
        match op {
            UnaryOp::Neg => match val {
                Value::Int(n) => Ok(Value::Int(-n)),
                Value::Float(n) => Ok(Value::Float(-n)),
                _ => Err(format!("Cannot negate {}", val.type_name())),
            },
            UnaryOp::Not => Ok(Value::Bool(!val.is_truthy())),
            UnaryOp::BitNot => match val {
                Value::Int(n) => Ok(Value::Int(!n)),
                _ => Err(format!("Cannot bitwise NOT {}", val.type_name())),
            },
        }
    }

    fn eval_call(&mut self, name: &str, resolution: Resolution, args: &[Expr]) -> Result<Value, String> {
        let mut eval_args = Vec::with_capacity(args.len());
        for arg in args {
            eval_args.push(self.eval_expr(arg)?);
        }
        self.call_function(name, resolution, eval_args)
    }

    fn eval_index(&mut self, expr: &Expr, index: &Expr) -> Result<Value, String> {
        let val = self.eval_expr(expr)?;
        let idx = self.eval_expr(index)?;
        match (&val, &idx) {
            (Value::List(list), Value::Int(i)) => {
                let idx = usize::try_from(*i).map_err(|_| format!("Negative index {i}"))?;
                list.get(idx).cloned().ok_or_else(|| format!("Index {idx} out of bounds (len {})", list.len()))
            }
            (Value::String(s), Value::Int(i)) => {
                let idx = usize::try_from(*i).map_err(|_| format!("Negative index {i}"))?;
                s.chars().nth(idx)
                    .map(|c| Value::String(c.to_string()))
                    .ok_or_else(|| format!("Index {idx} out of bounds"))
            }
            (Value::Object(map), Value::String(key)) => {
                map.get(key).cloned().ok_or_else(|| format!("Field '{key}' not found"))
            }
            _ => Err(format!("Cannot index {} with {}", val.type_name(), idx.type_name())),
        }
    }

    fn eval_field_access(&mut self, expr: &Expr, field: &str) -> Result<Value, String> {
        let val = self.eval_expr(expr)?;
        match &val {
            Value::Object(map) => {
                map.get(field).cloned().ok_or_else(|| format!("Field '{field}' not found"))
            }
            Value::CommandResult { status, out, err } => {
                match field {
                    "status" => Ok(Value::Int(i64::from(*status))),
                    "out" => Ok(Value::String(out.clone())),
                    "err" => Ok(Value::String(err.clone())),
                    _ => Err(format!("CommandResult has no field '{field}'")),
                }
            }
            _ => Err(format!("Cannot access field on {}", val.type_name())),
        }
    }

    fn eval_range(&mut self, start: &Expr, end: &Expr) -> Result<Value, String> {
        let s = self.eval_expr(start)?;
        let e = self.eval_expr(end)?;
        match (&s, &e) {
            (Value::Int(a), Value::Int(b)) => {
                let items: Vec<Value> = (*a..=*b).map(Value::Int).collect();
                Ok(Value::List(items))
            }
            _ => Err(format!("Range requires int..int, got {}..{}", s.type_name(), e.type_name())),
        }
    }

    fn eval_send(&mut self, left: &Expr, right: &Expr) -> Result<Value, String> {
        let lhs_val = self.eval_expr(left)?;
        // For command results, auto-extract .out
        let send_val = match &lhs_val {
            Value::CommandResult { out, .. } => Value::String(out.clone()),
            other => other.clone(),
        };
        let prev_send = self.send_value.take();
        self.send_value = Some(send_val);
        let result = self.eval_expr(right);
        self.send_value = prev_send;
        result
    }

    fn eval_lambda(&mut self, name: &str, resolution: Resolution, bound_args: &[Expr]) -> Result<Value, String> {
        let mut eval_args = Vec::with_capacity(bound_args.len());
        for arg in bound_args {
            eval_args.push(self.eval_expr(arg)?);
        }
        let res_code = match resolution {
            Resolution::Normal => 0,
            Resolution::OwnFirst => 1,
            Resolution::SystemOnly => 2,
        };
        Ok(Value::Lambda {
            name: name.to_owned(),
            resolution: res_code,
            bound_args: eval_args,
        })
    }

    fn eval_error_check(&self, name: &str) -> Result<Value, String> {
        match self.env.get(name) {
            Some(MaybeError::Ok(_)) => Ok(Value::Bool(true)),
            Some(MaybeError::Err(_)) => Ok(Value::Bool(false)),
            None => Err(format!("Undefined variable: '{name}'")),
        }
    }

    fn eval_dollar_ref(&self, dollar: &DollarRef) -> Result<Value, String> {
        let send_val = self.send_value.as_ref()
            .ok_or("$ used outside of send (->) context")?;
        match dollar {
            DollarRef::Whole => Ok(send_val.clone()),
            DollarRef::Index(i) => {
                if let Value::List(list) = send_val {
                    list.get(*i).cloned().ok_or_else(|| format!("${i} out of bounds"))
                } else {
                    Err(format!("${i} requires a list, got {}", send_val.type_name()))
                }
            }
            DollarRef::Field(field) => {
                match send_val {
                    Value::Object(map) => {
                        map.get(field).cloned().ok_or_else(|| format!("${field} not found"))
                    }
                    Value::CommandResult { status, out, err } => {
                        match field.as_str() {
                            "status" => Ok(Value::Int(i64::from(*status))),
                            "out" => Ok(Value::String(out.clone())),
                            "err" => Ok(Value::String(err.clone())),
                            _ => Err(format!("${field} not found on CommandResult")),
                        }
                    }
                    _ => Err(format!("${field} requires an object, got {}", send_val.type_name())),
                }
            }
        }
    }

    fn call_function(&mut self, name: &str, resolution: Resolution, args: Vec<Value>) -> Result<Value, String> {
        let lower = name.to_ascii_lowercase();

        // Check if name is a variable holding a lambda
        if let Some(MaybeError::Ok(Value::Lambda { name: fn_name, resolution: res_code, bound_args })) = self.env.get(&lower).cloned() {
            if !bound_args.is_empty() && !args.is_empty() {
                return Err(format!("Lambda '{name}' already has bound args, cannot pass additional args"));
            }
            let call_args = if bound_args.is_empty() { args } else { bound_args };
            let lambda_resolution = match res_code {
                1 => Resolution::OwnFirst,
                2 => Resolution::SystemOnly,
                _ => Resolution::Normal,
            };
            return self.call_function(&fn_name, lambda_resolution, call_args);
        }

        match resolution {
            Resolution::Normal => {
                // exe → own → system
                if let Some(result) = exec::try_exec_command(&lower, &args) {
                    return result;
                }
                if let Some(result) = self.call_user_fn(&lower, args.clone()) {
                    return result;
                }
                if let Some(result) = builtins::call_builtin(&lower, &args, self) {
                    return result;
                }
                Err(format!("Undefined: '{name}' (not found as exe, function, or built-in)"))
            }
            Resolution::OwnFirst => {
                // own → system
                if let Some(result) = self.call_user_fn(&lower, args.clone()) {
                    return result;
                }
                if let Some(result) = builtins::call_builtin(&lower, &args, self) {
                    return result;
                }
                Err(format!("Undefined: '{name}' (not found as function or built-in)"))
            }
            Resolution::SystemOnly => {
                // system only
                if let Some(result) = builtins::call_builtin(&lower, &args, self) {
                    return result;
                }
                Err(format!("Undefined: '{name}' (not a built-in function)"))
            }
        }
    }

    fn call_user_fn(&mut self, name: &str, args: Vec<Value>) -> Option<Result<Value, String>> {
        let func = self.env.get_fn(name)?.clone();

        if args.len() != func.params.len() {
            return Some(Err(format!(
                "'{}' expects {} args, got {}",
                name, func.params.len(), args.len()
            )));
        }

        self.env.push_scope();
        for (param, val) in func.params.iter().zip(args) {
            self.env.set_local(param, MaybeError::Ok(val));
        }

        let mut return_val = Value::Void;
        for stmt in &func.body {
            match self.exec_stmt(stmt) {
                Ok(FlowSignal::Return(Some(val))) => {
                    return_val = val;
                    break;
                }
                Ok(FlowSignal::Return(None)) => break,
                Ok(FlowSignal::None) => {}
                Err(e) => {
                    self.env.pop_scope();
                    return Some(Err(e));
                }
            }
        }

        self.env.pop_scope();
        Some(Ok(return_val))
    }

    fn apply_binop(left: &Value, op: BinOp, right: &Value) -> Result<Value, String> {
        // Type-strict comparisons
        match op {
            BinOp::Eq => {
                if std::mem::discriminant(left) != std::mem::discriminant(right) {
                    return Err(format!("Type mismatch: cannot compare {} with {}", left.type_name(), right.type_name()));
                }
                return Ok(Value::Bool(values_equal(left, right)));
            }
            BinOp::NotEq => {
                if std::mem::discriminant(left) != std::mem::discriminant(right) {
                    return Err(format!("Type mismatch: cannot compare {} with {}", left.type_name(), right.type_name()));
                }
                return Ok(Value::Bool(!values_equal(left, right)));
            }
            _ => {}
        }

        match (left, right) {
            // Int arithmetic
            (Value::Int(a), Value::Int(b)) => Self::apply_int_op(*a, op, *b),

            // Float arithmetic
            (Value::Float(a), Value::Float(b)) => apply_float_op(*a, op, *b),

            // Int + Float promotion
            (Value::Int(a), Value::Float(b)) => {
                #[expect(clippy::cast_precision_loss)]
                let a_f64 = *a as f64;
                apply_float_op(a_f64, op, *b)
            }
            (Value::Float(a), Value::Int(b)) => {
                #[expect(clippy::cast_precision_loss)]
                let b_f64 = *b as f64;
                apply_float_op(*a, op, b_f64)
            }

            // String concatenation
            (Value::String(a), Value::String(b)) => match op {
                BinOp::Add => Ok(Value::String(format!("{a}{b}"))),
                BinOp::Lt => Ok(Value::Bool(a < b)),
                BinOp::Gt => Ok(Value::Bool(a > b)),
                BinOp::LtEq => Ok(Value::Bool(a <= b)),
                BinOp::GtEq => Ok(Value::Bool(a >= b)),
                _ => Err(format!("Unsupported operation: string {op:?} string")),
            },

            // List concatenation
            (Value::List(a), Value::List(b)) => match op {
                BinOp::Add => {
                    let mut result = a.clone();
                    result.extend(b.iter().cloned());
                    Ok(Value::List(result))
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
            BinOp::Add => Ok(Value::Int(a + b)),
            BinOp::Sub => Ok(Value::Int(a - b)),
            BinOp::Mul => Ok(Value::Int(a * b)),
            BinOp::Div => {
                if b == 0 { return Err("Division by zero".to_string()); }
                Ok(Value::Int(a / b))
            }
            BinOp::Mod => {
                if b == 0 { return Err("Modulo by zero".to_string()); }
                Ok(Value::Int(a % b))
            }
            BinOp::Pow => {
                let exp = u32::try_from(b).map_err(|_| format!("Exponent {b} out of range for integer pow"))?;
                Ok(Value::Int(a.pow(exp)))
            }
            BinOp::Lt => Ok(Value::Bool(a < b)),
            BinOp::Gt => Ok(Value::Bool(a > b)),
            BinOp::LtEq => Ok(Value::Bool(a <= b)),
            BinOp::GtEq => Ok(Value::Bool(a >= b)),
            BinOp::BitAnd => Ok(Value::Int(a & b)),
            BinOp::BitOr => Ok(Value::Int(a | b)),
            BinOp::BitXor => Ok(Value::Int(a ^ b)),
            BinOp::Shl => Ok(Value::Int(a << b)),
            BinOp::Shr => Ok(Value::Int(a >> b)),
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
        BinOp::Add => Ok(Value::Float(a + b)),
        BinOp::Sub => Ok(Value::Float(a - b)),
        BinOp::Mul => Ok(Value::Float(a * b)),
        BinOp::Div => Ok(Value::Float(a / b)),
        BinOp::Mod => Ok(Value::Float(a % b)),
        BinOp::Pow => Ok(Value::Float(a.powf(b))),
        BinOp::Lt => Ok(Value::Bool(a < b)),
        BinOp::Gt => Ok(Value::Bool(a > b)),
        BinOp::LtEq => Ok(Value::Bool(a <= b)),
        BinOp::GtEq => Ok(Value::Bool(a >= b)),
        _ => Err(format!("Unsupported operation: float {op:?} float")),
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
