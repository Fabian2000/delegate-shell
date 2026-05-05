use crate::lexer::token::{Token, SpannedToken, Span};
use crate::parser::ast::{Expr, ExprKind, UnaryOp, DollarRef, BinOp, Resolution, StringPart, Stmt, StmtKind};
use std::sync::atomic::{AtomicUsize, Ordering};

static LAMBDA_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn next_lambda_name() -> String {
    let n = LAMBDA_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("__lambda_{n}")
}

enum PostfixResult {
    Continue,
    Break,
    None,
}

/// Operator precedence levels (lower = binds less tightly)
const fn prefix_binding_power(op: &Token) -> Option<((), u8)> {
    match op {
        Token::Bang | Token::BitNot | Token::Minus => Some(((), 27)),
        _ => None,
    }
}

const fn infix_binding_power(op: &Token) -> Option<(u8, u8)> {
    match op {
        Token::Or => Some((2, 3)),
        Token::And => Some((4, 5)),
        Token::BitOr => Some((6, 7)),
        Token::BitXor => Some((8, 9)),
        Token::BitAnd => Some((10, 11)),
        Token::Eq | Token::NotEq => Some((12, 13)),
        Token::Lt | Token::Gt | Token::LtEq | Token::GtEq => Some((14, 15)),
        Token::Shl | Token::Shr => Some((16, 17)),
        Token::Plus | Token::Minus => Some((18, 19)),
        Token::Star | Token::Slash | Token::Percent => Some((20, 21)),
        Token::Power => Some((23, 22)), // right-associative
        Token::Send | Token::SafeSend => Some((1, 1)),    // lowest, left-associative
        Token::Range => Some((24, 25)), // tighter than power
        _ => None,
    }
}

pub struct ExprParser<'a> {
    tokens: &'a [SpannedToken],
    pos: usize,
    /// Synthetic top-level function definitions produced by inline lambda
    /// desugaring. The caller (StmtParser) drains this after each parse_expr
    /// call and prepends them to the program.
    pub synthetic_fns: Vec<Stmt>,
}

impl<'a> ExprParser<'a> {
    #[must_use]
    pub const fn new(tokens: &'a [SpannedToken], pos: usize) -> Self {
        Self { tokens, pos, synthetic_fns: Vec::new() }
    }

    #[must_use]
    pub const fn pos(&self) -> usize {
        self.pos
    }

    fn peek(&self) -> &Token {
        if self.pos < self.tokens.len() {
            &self.tokens[self.pos].token
        } else {
            &Token::Eof
        }
    }

    fn peek_span(&self) -> Span {
        if self.pos < self.tokens.len() {
            self.tokens[self.pos].span
        } else {
            Span { start: 0, end: 0 }
        }
    }

    fn advance(&mut self) -> &SpannedToken {
        let tok = &self.tokens[self.pos];
        self.pos += 1;
        tok
    }

    fn expect(&mut self, expected: &Token) -> Result<&SpannedToken, String> {
        if self.peek() == expected {
            Ok(self.advance())
        } else {
            Err(format!("Expected {:?}, got {:?}", expected, self.peek()))
        }
    }

    /// Skip layout tokens (Newline, Indent, Dedent). Used inside bracket pairs
    /// so that list/object/argument lists can span multiple lines.
    fn skip_layout(&mut self) {
        while matches!(self.peek(), Token::Newline | Token::Indent | Token::Dedent) {
            self.pos += 1;
        }
    }

    /// Parses an expression with the given minimum binding power.
    ///
    /// # Errors
    ///
    /// Returns an error string if the token stream contains invalid syntax.
    pub fn parse_expr(&mut self, min_bp: u8) -> Result<Expr, String> {
        let mut lhs = self.parse_atom()?;

        loop {
            // Postfix: function call, indexing, field access
            match self.try_parse_postfix(&mut lhs)? {
                PostfixResult::Continue => continue,
                PostfixResult::Break => break,
                PostfixResult::None => {}
            }

            // Infix operators
            if let Some(new_lhs) = self.try_parse_infix(lhs.clone(), min_bp)? {
                lhs = new_lhs;
                continue;
            }

            break;
        }

        Ok(lhs)
    }

    fn try_parse_postfix(&mut self, lhs: &mut Expr) -> Result<PostfixResult, String> {
        match self.peek() {
            Token::LParen => Ok(PostfixResult::Break),
            Token::Increment | Token::Decrement => {
                // Post-increment/decrement: ident++ or ident--
                if let ExprKind::Ident(name) = &lhs.kind {
                    let name = name.clone();
                    let increment = *self.peek() == Token::Increment;
                    let span_end = self.peek_span().end;
                    self.advance();
                    *lhs = Expr {
                        kind: ExprKind::PostIncDec { name, increment },
                        span: Span { start: lhs.span.start, end: span_end },
                    };
                    Ok(PostfixResult::Continue)
                } else {
                    Ok(PostfixResult::Break)
                }
            }
            Token::Question => {
                if let ExprKind::Ident(name) = &lhs.kind {
                    let name = name.clone();
                    let span_end = self.peek_span().end;
                    self.advance();
                    if *self.peek() == Token::Dot {
                        self.advance();
                        if let Token::Ident(field) = self.peek().clone() {
                            let field_end = self.peek_span().end;
                            self.advance();
                            *lhs = Expr {
                                kind: ExprKind::ErrorField { name, field },
                                span: Span { start: lhs.span.start, end: field_end },
                            };
                        } else {
                            return Err(format!("Expected field name after '?.', got {:?}", self.peek()));
                        }
                    } else {
                        *lhs = Expr {
                            kind: ExprKind::ErrorCheck(name),
                            span: Span { start: lhs.span.start, end: span_end },
                        };
                    }
                    Ok(PostfixResult::Continue)
                } else {
                    Ok(PostfixResult::Break)
                }
            }
            Token::LBracket => {
                self.advance();
                let index = self.parse_expr(0)?;
                self.expect(&Token::RBracket)?;
                let span = Span { start: lhs.span.start, end: self.peek_span().start };
                *lhs = Expr {
                    kind: ExprKind::Index {
                        expr: Box::new(lhs.clone()),
                        index: Box::new(index),
                    },
                    span,
                };
                Ok(PostfixResult::Continue)
            }
            Token::Dot => {
                self.advance();
                if let Token::Ident(field) = self.peek().clone() {
                    let span_start = lhs.span.start;
                    let span_end = self.peek_span().end;
                    self.advance();
                    *lhs = Expr {
                        kind: ExprKind::FieldAccess {
                            expr: Box::new(lhs.clone()),
                            field,
                        },
                        span: Span { start: span_start, end: span_end },
                    };
                    Ok(PostfixResult::Continue)
                } else {
                    Err(format!("Expected field name after '.', got {:?}", self.peek()))
                }
            }
            _ => Ok(PostfixResult::None),
        }
    }

    fn try_parse_infix(&mut self, lhs: Expr, min_bp: u8) -> Result<Option<Expr>, String> {
        let op = self.peek().clone();
        if let Some((l_bp, r_bp)) = infix_binding_power(&op) {
            if l_bp < min_bp {
                return Ok(None);
            }
            self.advance();
            let rhs = self.parse_expr(r_bp)?;
            let span = Span { start: lhs.span.start, end: rhs.span.end };
            let kind = match op {
                Token::Send => ExprKind::Send {
                    left: Box::new(lhs),
                    right: Box::new(rhs),
                },
                Token::SafeSend => ExprKind::SafeSend {
                    left: Box::new(lhs),
                    right: Box::new(rhs),
                },
                Token::Range => ExprKind::Range {
                    start: Box::new(lhs),
                    end: Box::new(rhs),
                },
                _ => {
                    let bin_op = token_to_binop(&op).ok_or_else(|| {
                        format!("Unknown binary operator: {op:?}")
                    })?;
                    ExprKind::BinaryOp {
                        left: Box::new(lhs),
                        op: bin_op,
                        right: Box::new(rhs),
                    }
                }
            };
            Ok(Some(Expr { kind, span }))
        } else {
            Ok(None)
        }
    }

    fn parse_atom(&mut self) -> Result<Expr, String> {
        let tok = self.peek().clone();
        let span = self.peek_span();

        match tok {
            // Prefix operators
            Token::Bang | Token::BitNot | Token::Minus if is_prefix_context(&tok) => {
                self.parse_prefix(&tok, span)
            }

            // Pre-increment/decrement: ++ident or --ident
            Token::Increment | Token::Decrement => {
                let increment = tok == Token::Increment;
                self.advance(); // consume ++ or --
                if let Token::Ident(name) = self.peek().clone() {
                    let name_span_end = self.peek_span().end;
                    self.advance(); // consume ident
                    Ok(Expr {
                        kind: ExprKind::PreIncDec { name, increment },
                        span: Span { start: span.start, end: name_span_end },
                    })
                } else {
                    Err(format!("Expected identifier after {}, got {:?}",
                        if increment { "++" } else { "--" }, self.peek()))
                }
            }

            Token::Int(v) => {
                self.advance();
                Ok(Expr { kind: ExprKind::Int(v), span })
            }
            Token::Float(v) => {
                self.advance();
                Ok(Expr { kind: ExprKind::Float(v), span })
            }
            Token::String(parts) => {
                self.advance();
                let ast_parts = convert_string_parts(&parts, &mut self.synthetic_fns)?;
                Ok(Expr { kind: ExprKind::String(ast_parts), span })
            }
            Token::Bool(v) => {
                self.advance();
                Ok(Expr { kind: ExprKind::Bool(v), span })
            }

            Token::Void => {
                self.advance();
                Ok(Expr { kind: ExprKind::VoidLit, span })
            }

            Token::LBracket => self.parse_list_literal(span),
            Token::LBrace => self.parse_object_literal(span),

            // Parenthesized expression
            Token::LParen => {
                self.advance();
                let expr = self.parse_expr(0)?;
                self.expect(&Token::RParen)?;
                Ok(expr)
            }

            // Dollar references
            Token::Dollar => {
                self.advance();
                Ok(Expr { kind: ExprKind::DollarRef(DollarRef::Whole), span })
            }
            Token::DollarIndex(idx) => {
                self.advance();
                Ok(Expr { kind: ExprKind::DollarRef(DollarRef::Index(idx)), span })
            }
            Token::DollarField(ref field) => {
                let field = field.clone();
                self.advance();
                Ok(Expr { kind: ExprKind::DollarRef(DollarRef::Field(field)), span })
            }

            Token::At => self.parse_lambda(span),
            Token::Ident(name) => self.parse_ident_or_call(name, span),

            // Optional param check: <ident> — check if optional param was provided
            Token::Lt => self.parse_optional_check(span),

            // atomic keyword: `atomic <expr>`
            Token::Atomic => {
                self.advance();
                let inner = self.parse_expr(0)?;
                let end = inner.span.end;
                Ok(Expr {
                    kind: ExprKind::Atomic(Box::new(inner)),
                    span: Span { start: span.start, end },
                })
            }

            _ => Err(format!("Unexpected token: {tok:?}")),
        }
    }

    fn parse_optional_check(&mut self, span: Span) -> Result<Expr, String> {
        // We've already peeked Token::Lt. Try to match <ident> pattern.
        // Save position in case this is not <ident> (e.g. a bare '<' that shouldn't be here).
        let saved_pos = self.pos;
        self.advance(); // consume '<'

        if let Token::Ident(name) = self.peek().clone() {
            self.advance(); // consume ident
            if *self.peek() == Token::Gt {
                let end = self.peek_span().end;
                self.advance(); // consume '>'
                return Ok(Expr {
                    kind: ExprKind::OptionalCheck(name),
                    span: Span { start: span.start, end },
                });
            }
        }

        // Not <ident> — restore position and report error
        self.pos = saved_pos;
        Err(format!("Unexpected token: {:?}", Token::Lt))
    }

    fn parse_prefix(&mut self, tok: &Token, span: Span) -> Result<Expr, String> {
        self.advance();
        let ((), r_bp) = prefix_binding_power(tok)
            .ok_or_else(|| format!("Unexpected prefix operator: {tok:?}"))?;
        let expr = self.parse_expr(r_bp)?;
        let op = match tok {
            Token::Bang => UnaryOp::Not,
            Token::BitNot => UnaryOp::BitNot,
            Token::Minus => UnaryOp::Neg,
            _ => return Err("internal: unexpected state".to_string()),
        };
        Ok(Expr {
            span: Span { start: span.start, end: expr.span.end },
            kind: ExprKind::UnaryOp {
                op,
                expr: Box::new(expr),
            },
        })
    }

    fn parse_list_literal(&mut self, span: Span) -> Result<Expr, String> {
        self.advance();
        self.skip_layout();
        let mut elements = Vec::new();
        while *self.peek() != Token::RBracket {
            elements.push(self.parse_expr(0)?);
            self.skip_layout();
            if *self.peek() == Token::Comma {
                self.advance();
                self.skip_layout();
            }
        }
        let end = self.peek_span().end;
        self.expect(&Token::RBracket)?;
        Ok(Expr {
            kind: ExprKind::List(elements),
            span: Span { start: span.start, end },
        })
    }

    fn parse_object_literal(&mut self, span: Span) -> Result<Expr, String> {
        self.advance();
        self.skip_layout();
        let mut fields = Vec::new();
        while *self.peek() != Token::RBrace {
            if let Token::Ident(name) = self.peek().clone() {
                self.advance();
                if *self.peek() == Token::Assign {
                    self.advance();
                    let value = self.parse_expr(0)?;
                    fields.push((name, value));
                } else {
                    // Shorthand: { name } is { name = name }
                    fields.push((name.clone(), Expr {
                        kind: ExprKind::Ident(name),
                        span,
                    }));
                }
            } else {
                return Err(format!("Expected field name in object, got {:?}", self.peek()));
            }
            self.skip_layout();
            if *self.peek() == Token::Comma {
                self.advance();
                self.skip_layout();
            }
        }
        let end = self.peek_span().end;
        self.expect(&Token::RBrace)?;
        Ok(Expr {
            kind: ExprKind::Object(fields),
            span: Span { start: span.start, end },
        })
    }

    fn parse_lambda(&mut self, span: Span) -> Result<Expr, String> {
        self.advance();

        // Inline lambda: @(params) body_expr — desugared to a synthetic top-level
        // function `__lambda_N(dyn p1, dyn p2, ...) { return body }` and replaced
        // with a normal Lambda reference to that name.
        if *self.peek() == Token::LParen {
            self.advance();
            self.skip_layout();
            let mut params: Vec<(String, Option<crate::parser::ast::TypeAnnotation>, bool)> = Vec::new();
            while *self.peek() != Token::RParen {
                if let Token::Ident(pname) = self.peek().clone() {
                    self.advance();
                    params.push((pname, None, true)); // dyn
                } else {
                    return Err(format!("Expected parameter name in inline lambda, got {:?}", self.peek()));
                }
                self.skip_layout();
                if *self.peek() == Token::Comma {
                    self.advance();
                    self.skip_layout();
                }
            }
            self.expect(&Token::RParen)?;
            let body_expr = self.parse_expr(0)?;
            let body_span = body_expr.span;
            let lambda_name = next_lambda_name();
            let body_stmt = Stmt {
                kind: StmtKind::Return { expr: Some(body_expr), is_dyn: true },
                span: body_span,
            };
            let fn_def = Stmt {
                kind: StmtKind::FnDef {
                    name: lambda_name.clone(),
                    params,
                    optional_params: Vec::new(),
                    return_type_ann: None,
                    body: vec![body_stmt],
                },
                span,
            };
            self.synthetic_fns.push(fn_def);
            let end = body_span.end;
            return Ok(Expr {
                kind: ExprKind::Lambda { name: lambda_name, resolution: Resolution::Normal, bound_args: Vec::new() },
                span: Span { start: span.start, end },
            });
        }

        if let Token::Ident(name) = self.peek().clone() {
            self.advance();
            let (resolution, name) = parse_resolution_suffix(name, self);
            let mut bound_args = Vec::new();
            if *self.peek() == Token::LParen {
                self.advance();
                self.skip_layout();
                while *self.peek() != Token::RParen {
                    bound_args.push(self.parse_expr(0)?);
                    self.skip_layout();
                    if *self.peek() == Token::Comma {
                        self.advance();
                        self.skip_layout();
                    }
                }
                self.expect(&Token::RParen)?;
            }
            let end = self.peek_span().start;
            Ok(Expr {
                kind: ExprKind::Lambda { name, resolution, bound_args },
                span: Span { start: span.start, end },
            })
        } else {
            Err(format!("Expected function name after '@', got {:?}", self.peek()))
        }
    }

    fn parse_ident_or_call(&mut self, name: String, span: Span) -> Result<Expr, String> {
        self.advance();

        // Check for ! or !! suffix (function resolution)
        let (resolution, clean_name) = parse_resolution_suffix(name, self);

        // Check if it's a function call
        if *self.peek() == Token::LParen {
            self.advance();
            self.skip_layout();
            let mut args = Vec::new();
            while *self.peek() != Token::RParen {
                args.push(self.parse_expr(0)?);
                self.skip_layout();
                if *self.peek() == Token::Comma {
                    self.advance();
                    self.skip_layout();
                }
            }
            let end = self.peek_span().end;
            self.expect(&Token::RParen)?;
            Ok(Expr {
                kind: ExprKind::Call { name: clean_name, resolution, args },
                span: Span { start: span.start, end },
            })
        } else {
            Ok(Expr { kind: ExprKind::Ident(clean_name), span })
        }
    }
}

const fn is_prefix_context(_tok: &Token) -> bool {
    true // simplified — the parser context determines this
}

const fn token_to_binop(tok: &Token) -> Option<BinOp> {
    match tok {
        Token::Plus => Some(BinOp::Add),
        Token::Minus => Some(BinOp::Sub),
        Token::Star => Some(BinOp::Mul),
        Token::Slash => Some(BinOp::Div),
        Token::Percent => Some(BinOp::Mod),
        Token::Power => Some(BinOp::Pow),
        Token::Eq => Some(BinOp::Eq),
        Token::NotEq => Some(BinOp::NotEq),
        Token::Lt => Some(BinOp::Lt),
        Token::Gt => Some(BinOp::Gt),
        Token::LtEq => Some(BinOp::LtEq),
        Token::GtEq => Some(BinOp::GtEq),
        Token::And => Some(BinOp::And),
        Token::Or => Some(BinOp::Or),
        Token::BitAnd => Some(BinOp::BitAnd),
        Token::BitOr => Some(BinOp::BitOr),
        Token::BitXor => Some(BinOp::BitXor),
        Token::Shl => Some(BinOp::Shl),
        Token::Shr => Some(BinOp::Shr),
        _ => None,
    }
}

/// Parse ! or !! after identifier for resolution level
fn parse_resolution_suffix(name: String, parser: &mut ExprParser) -> (Resolution, String) {
    if *parser.peek() == Token::Bang {
        parser.advance();
        if *parser.peek() == Token::Bang {
            parser.advance();
            (Resolution::SystemOnly, name)
        } else {
            (Resolution::OwnFirst, name)
        }
    } else {
        (Resolution::Normal, name)
    }
}

fn convert_string_parts(
    parts: &[crate::lexer::token::StringPart],
    synthetic_fns: &mut Vec<Stmt>,
) -> Result<Vec<StringPart>, String> {
    let mut result = Vec::new();
    for part in parts {
        match part {
            crate::lexer::token::StringPart::Literal(s) => {
                result.push(StringPart::Literal(s.clone()));
            }
            crate::lexer::token::StringPart::Interpolation(tokens) => {
                let mut ep = ExprParser::new(tokens, 0);
                let expr = ep.parse_expr(0)?;
                synthetic_fns.append(&mut ep.synthetic_fns);
                result.push(StringPart::Expr(Box::new(expr)));
            }
        }
    }
    Ok(result)
}
