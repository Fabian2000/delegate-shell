use crate::lexer::token::{Token, SpannedToken, Span};
use crate::parser::ast::{Stmt, StmtKind, ExprKind, Expr, CompoundOp, TypeAnnotation};
use crate::parser::expr::ExprParser;

pub struct StmtParser<'a> {
    tokens: &'a [SpannedToken],
    pos: usize,
}

impl<'a> StmtParser<'a> {
    #[must_use]
    pub const fn new(tokens: &'a [SpannedToken]) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> &Token {
        self.skip_newlines_peek()
    }

    fn peek_raw(&self) -> &Token {
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

    fn skip_newlines_peek(&self) -> &Token {
        let mut p = self.pos;
        while p < self.tokens.len() {
            if self.tokens[p].token != Token::Newline {
                return &self.tokens[p].token;
            }
            p += 1;
        }
        &Token::Eof
    }

    fn skip_newlines(&mut self) {
        while self.pos < self.tokens.len() && self.tokens[self.pos].token == Token::Newline {
            self.pos += 1;
        }
    }

    fn advance(&mut self) -> &SpannedToken {
        let tok = &self.tokens[self.pos];
        self.pos += 1;
        tok
    }

    fn expect(&mut self, expected: &Token) -> Result<(), String> {
        self.skip_newlines();
        if self.peek_raw() == expected {
            self.advance();
            Ok(())
        } else {
            Err(format!("Expected {:?}, got {:?}", expected, self.peek_raw()))
        }
    }

    /// Parses the full token stream into a list of statements.
    ///
    /// # Errors
    ///
    /// Returns an error string if the token stream contains invalid syntax.
    pub fn parse_program(&mut self) -> Result<Vec<Stmt>, String> {
        let mut stmts = Vec::new();
        while *self.peek() != Token::Eof {
            self.skip_newlines();
            if *self.peek_raw() == Token::Eof {
                break;
            }
            stmts.push(self.parse_stmt()?);
        }
        Ok(stmts)
    }

    fn parse_stmt(&mut self) -> Result<Stmt, String> {
        self.skip_newlines();
        let span = self.peek_span();

        match self.peek_raw().clone() {
            Token::If => self.parse_if(),
            Token::While => self.parse_while(),
            Token::For => self.parse_for(),
            Token::Return => self.parse_return(),
            Token::Continue => {
                let span = self.peek_span();
                self.advance();
                Ok(Stmt { kind: StmtKind::Continue, span })
            }
            Token::Break => {
                let span = self.peek_span();
                self.advance();
                Ok(Stmt { kind: StmtKind::Break, span })
            }
            Token::Import => self.parse_import(),
            Token::Free => self.parse_free(),
            Token::Use => self.parse_use(),
            Token::Throw => self.parse_throw(),
            Token::Enum => self.parse_enum(),
            Token::Match => self.parse_match(),
            Token::Alias => self.parse_alias(),
            Token::Dyn => self.parse_dyn(),
            Token::Increment => {
                // pre-increment: ++x
                self.advance();
                if let Token::Ident(name) = self.peek_raw().clone() {
                    self.advance();
                    return Ok(Stmt { kind: StmtKind::PreIncDec { name, increment: true }, span });
                }
                Err("Expected identifier after ++".to_string())
            }
            Token::Decrement => {
                // pre-decrement: --x
                self.advance();
                if let Token::Ident(name) = self.peek_raw().clone() {
                    self.advance();
                    return Ok(Stmt { kind: StmtKind::PreIncDec { name, increment: false }, span });
                }
                Err("Expected identifier after --".to_string())
            }
            Token::Ident(name) => self.parse_ident_stmt(name, span),
            _ => {
                let expr = self.parse_expression()?;
                Ok(Stmt { kind: StmtKind::ExprStmt(expr), span })
            }
        }
    }

    fn parse_ident_stmt(&mut self, name: String, span: Span) -> Result<Stmt, String> {
        // Could be: assignment, compound assign, fn def, post inc/dec, or expr stmt
        let saved_pos = self.pos;

        self.advance(); // consume ident

        // Check for post-increment/decrement: x++ or x--
        match self.peek_raw() {
            Token::Increment => {
                self.advance();
                return Ok(Stmt { kind: StmtKind::PostIncDec { name, increment: true }, span });
            }
            Token::Decrement => {
                self.advance();
                return Ok(Stmt { kind: StmtKind::PostIncDec { name, increment: false }, span });
            }
            _ => {}
        }

        // Check for type annotation: x: type = ...
        // This must come before the `?` check since `x: type? = ...` is not supported.
        let type_ann = if *self.peek_raw() == Token::Colon {
            self.try_parse_type_annotation()?
        } else {
            None
        };

        // Check for ? (error-tolerant assignment)
        let error_tolerant = if *self.peek_raw() == Token::Question {
            self.advance();
            true
        } else {
            false
        };

        let peeked = self.peek_raw().clone();
        match peeked {
            Token::Assign => {
                self.advance();
                let expr = self.parse_expression()?;
                Ok(Stmt {
                    kind: StmtKind::Assign { name, error_tolerant, type_ann, is_dyn: false, expr },
                    span,
                })
            }
            Token::PlusAssign | Token::MinusAssign | Token::StarAssign |
            Token::SlashAssign | Token::PercentAssign | Token::PowerAssign |
            Token::BitAndAssign | Token::BitOrAssign | Token::BitXorAssign |
            Token::ShlAssign | Token::ShrAssign => {
                if type_ann.is_some() {
                    return Err(format!(
                        "Type annotation on '{name}' is only allowed with '=' assignment, not compound assignment"
                    ));
                }
                let op_tok = self.advance().token.clone();
                let op = token_to_compound_op(&op_tok)?;
                let expr = self.parse_expression()?;
                Ok(Stmt {
                    kind: StmtKind::CompoundAssign { name, op, expr },
                    span,
                })
            }
            Token::LParen => {
                if type_ann.is_some() {
                    return Err(format!(
                        "Type annotation on '{name}' is only allowed with '=' assignment, not a function definition or call"
                    ));
                }
                self.pos = saved_pos;
                self.parse_possible_fn_def_or_expr()
            }
            Token::Bang => {
                if type_ann.is_some() {
                    return Err(format!(
                        "Type annotation on '{name}' is only allowed with '=' assignment"
                    ));
                }
                self.pos = saved_pos;
                let expr = self.parse_expression()?;
                Ok(Stmt { kind: StmtKind::ExprStmt(expr), span })
            }
            Token::LBracket | Token::Dot if !error_tolerant => {
                if type_ann.is_some() {
                    return Err(format!(
                        "Type annotation on '{name}' is only allowed with '=' assignment"
                    ));
                }
                self.pos = saved_pos;
                let expr = self.parse_expression()?;
                if *self.peek_raw() == Token::Assign {
                    self.advance();
                    let value = self.parse_expression()?;
                    match expr.kind {
                        ExprKind::Index { expr: target, index } => {
                            return Ok(Stmt {
                                kind: StmtKind::IndexAssign { target: *target, index: *index, value },
                                span,
                            });
                        }
                        ExprKind::FieldAccess { expr: target, field } => {
                            return Ok(Stmt {
                                kind: StmtKind::FieldAssign { target: *target, field, value },
                                span,
                            });
                        }
                        _ => return Err("Invalid assignment target".to_string()),
                    }
                }
                Ok(Stmt { kind: StmtKind::ExprStmt(expr), span })
            }
            _ => {
                if error_tolerant {
                    return Err("'?' requires an assignment (x? = ...)".to_string());
                }
                if type_ann.is_some() {
                    return Err(format!(
                        "Type annotation on '{name}' requires an '=' assignment"
                    ));
                }
                self.pos = saved_pos;
                let expr = self.parse_expression()?;
                Ok(Stmt { kind: StmtKind::ExprStmt(expr), span })
            }
        }
    }

    fn parse_possible_fn_def_or_expr(&mut self) -> Result<Stmt, String> {
        let span = self.peek_span();

        // Check if Indent follows — then it's a function definition.
        // We need to check for optional params <ident> before parsing as expression,
        // since the expression parser won't handle <ident> as a param in this context.
        // Parse the function name and param list manually if the lookahead suggests fn def.
        if let Some(fn_def) = self.try_parse_fn_def(span)? {
            return Ok(fn_def);
        }

        // Parse as expression
        let expr = self.parse_expression()?;
        Ok(Stmt { kind: StmtKind::ExprStmt(expr), span })
    }

    /// Try to parse a function definition: `name(params...) INDENT body DEDENT`.
    /// Handles both required params (ident) and optional params (<ident>).
    /// Returns None if this doesn't look like a function definition.
    fn try_parse_fn_def(&mut self, span: crate::lexer::token::Span) -> Result<Option<Stmt>, String> {
        // We need: Ident LParen ... RParen Newline Indent
        // Peek ahead to see if there's a definition block
        let saved_pos = self.pos;

        // Expect identifier (function name)
        let name = if let Token::Ident(n) = self.peek_raw().clone() {
            self.advance();
            n
        } else {
            self.pos = saved_pos;
            return Ok(None);
        };

        // Expect '('
        if *self.peek_raw() != Token::LParen {
            self.pos = saved_pos;
            return Ok(None);
        }
        self.advance();

        // Parse parameters: required (ident) or optional (<ident>), each with optional `: type`
        let mut params: Vec<(String, Option<TypeAnnotation>, bool)> = Vec::new();
        let mut optional_params: Vec<(String, Option<TypeAnnotation>, bool)> = Vec::new();
        let mut seen_optional = false;

        loop {
            // Skip any whitespace/newlines inside parens
            if *self.peek_raw() == Token::RParen {
                self.advance();
                break;
            }

            // Check for dyn modifier on param
            let param_is_dyn = if *self.peek_raw() == Token::Dyn {
                self.advance();
                true
            } else {
                false
            };

            // Check for optional param: < ident >
            if *self.peek_raw() == Token::Lt {
                let lt_pos = self.pos;
                self.advance(); // consume '<'
                if let Token::Ident(param_name) = self.peek_raw().clone() {
                    self.advance(); // consume ident
                    // Optional type annotation on optional param: <name: type>
                    let ann = if *self.peek_raw() == Token::Colon {
                        if param_is_dyn {
                            return Err(format!("'dyn' and type annotation on '{param_name}' are incompatible"));
                        }
                        match self.try_parse_type_annotation() {
                            Ok(a) => a,
                            Err(_) => {
                                self.pos = lt_pos;
                                self.pos = saved_pos;
                                return Ok(None);
                            }
                        }
                    } else {
                        None
                    };
                    if *self.peek_raw() == Token::Gt {
                        self.advance(); // consume '>'
                        seen_optional = true;
                        optional_params.push((param_name, ann, param_is_dyn));
                    } else {
                        // Not <ident> — restore and bail
                        self.pos = lt_pos;
                        self.pos = saved_pos;
                        return Ok(None);
                    }
                } else {
                    self.pos = lt_pos;
                    self.pos = saved_pos;
                    return Ok(None);
                }
            } else if let Token::Ident(param_name) = self.peek_raw().clone() {
                if seen_optional {
                    self.pos = saved_pos;
                    return Err("Required parameters must come before optional parameters".to_string());
                }
                self.advance();
                // Optional type annotation: name: type
                let ann = if *self.peek_raw() == Token::Colon {
                    if param_is_dyn {
                        return Err(format!("'dyn' and type annotation on '{param_name}' are incompatible"));
                    }
                    self.try_parse_type_annotation()?
                } else {
                    None
                };
                params.push((param_name, ann, param_is_dyn));
            } else {
                // Not a param — bail, this isn't a fn def
                self.pos = saved_pos;
                return Ok(None);
            }

            // Expect comma or RParen
            if *self.peek_raw() == Token::Comma {
                self.advance();
            }
        }

        // Optional return type annotation after ')': ): type
        let return_type_ann = if *self.peek_raw() == Token::Colon {
            self.try_parse_type_annotation()?
        } else {
            None
        };

        // Now skip newlines and check for Indent
        self.skip_newlines();
        if *self.peek_raw() != Token::Indent {
            // Not a function definition — restore and let normal expression parse handle it
            self.pos = saved_pos;
            return Ok(None);
        }
        self.advance(); // consume Indent

        let body = self.parse_block()?;

        // Validate: either ALL returns are `dyn return` or NONE
        validate_dyn_returns(&name, &body)?;

        Ok(Some(Stmt {
            kind: StmtKind::FnDef { name, params, optional_params, return_type_ann, body },
            span,
        }))
    }

    fn parse_block(&mut self) -> Result<Vec<Stmt>, String> {
        let mut stmts = Vec::new();
        loop {
            self.skip_newlines();
            match self.peek_raw() {
                Token::Dedent => {
                    self.advance();
                    break;
                }
                Token::Eof => break,
                _ => {
                    stmts.push(self.parse_stmt()?);
                }
            }
        }
        Ok(stmts)
    }

    fn parse_if(&mut self) -> Result<Stmt, String> {
        let span = self.peek_span();
        self.advance(); // consume 'if'

        let condition = self.parse_expression()?;

        self.skip_newlines();
        self.expect(&Token::Indent)?;
        let body = self.parse_block()?;

        self.skip_newlines();
        let else_body = if *self.peek_raw() == Token::Else {
            self.advance();
            self.skip_newlines();
            if *self.peek_raw() == Token::If {
                // else if — parse as single if statement in else body
                let else_if = self.parse_if()?;
                Some(vec![else_if])
            } else {
                self.expect(&Token::Indent)?;
                Some(self.parse_block()?)
            }
        } else {
            None
        };

        Ok(Stmt {
            kind: StmtKind::If { condition, body, else_body },
            span,
        })
    }

    fn parse_while(&mut self) -> Result<Stmt, String> {
        let span = self.peek_span();
        self.advance(); // consume 'while'

        let condition = self.parse_expression()?;

        self.skip_newlines();
        self.expect(&Token::Indent)?;
        let body = self.parse_block()?;

        Ok(Stmt {
            kind: StmtKind::While { condition, body },
            span,
        })
    }

    fn parse_for(&mut self) -> Result<Stmt, String> {
        let span = self.peek_span();
        self.advance(); // consume 'for'

        let var = if let Token::Ident(name) = self.peek_raw().clone() {
            self.advance();
            name
        } else {
            return Err(format!("Expected variable name after 'for', got {:?}", self.peek_raw()));
        };

        self.expect(&Token::In)?;
        let iter = self.parse_expression()?;

        self.skip_newlines();
        self.expect(&Token::Indent)?;
        let body = self.parse_block()?;

        Ok(Stmt {
            kind: StmtKind::For { var, iter, body },
            span,
        })
    }

    fn parse_return(&mut self) -> Result<Stmt, String> {
        let span = self.peek_span();
        self.advance(); // consume 'return'

        // Check if there's an expression following (not a newline/dedent/eof)
        let value = match self.peek_raw() {
            Token::Newline | Token::Dedent | Token::Eof | Token::Semicolon => None,
            _ => Some(self.parse_expression()?),
        };

        Ok(Stmt { kind: StmtKind::Return { expr: value, is_dyn: false }, span })
    }

    fn parse_import(&mut self) -> Result<Stmt, String> {
        let span = self.peek_span();
        self.advance(); // consume 'import'

        if let Token::String(parts) = self.peek_raw().clone() {
            self.advance();
            if parts.len() == 1
                && let crate::lexer::token::StringPart::Literal(path) = &parts[0]
            {
                return Ok(Stmt { kind: StmtKind::Import(path.clone()), span });
            }
            Err("Import path must be a simple string literal".to_string())
        } else {
            Err(format!("Expected string after 'import', got {:?}", self.peek_raw()))
        }
    }

    fn parse_match(&mut self) -> Result<Stmt, String> {
        let span = self.peek_span();
        self.advance(); // consume 'match'

        let expr = self.parse_expression()?;

        // Expect newline + indent
        if *self.peek_raw() != Token::Newline {
            return Err("Expected newline after match expression".to_string());
        }
        self.advance();
        if *self.peek_raw() != Token::Indent {
            return Err("Expected indented block with match arms".to_string());
        }
        self.advance();

        let mut arms = Vec::new();
        loop {
            match self.peek_raw().clone() {
                Token::Dedent | Token::Eof => {
                    if *self.peek_raw() == Token::Dedent {
                        self.advance();
                    }
                    break;
                }
                _ => {
                    // Parse pattern: `default` for catch-all, or an expression
                    let pattern = if *self.peek_raw() == Token::Default {
                        self.advance();
                        None
                    } else {
                        Some(self.parse_expression()?)
                    };

                    // Expect newline + indent for body
                    if *self.peek_raw() != Token::Newline {
                        return Err("Expected newline after match pattern".to_string());
                    }
                    self.advance();
                    if *self.peek_raw() != Token::Indent {
                        return Err("Expected indented body for match arm".to_string());
                    }
                    self.advance();

                    let mut body = Vec::new();
                    loop {
                        match self.peek_raw() {
                            Token::Dedent | Token::Eof => {
                                if *self.peek_raw() == Token::Dedent {
                                    self.advance();
                                }
                                break;
                            }
                            _ => {
                                body.push(self.parse_stmt()?);
                                if *self.peek_raw() == Token::Newline {
                                    self.advance();
                                }
                            }
                        }
                    }

                    arms.push(crate::parser::ast::MatchArm { pattern, body });

                    // Skip newline between arms
                    if *self.peek_raw() == Token::Newline {
                        self.advance();
                    }
                }
            }
        }

        Ok(Stmt { kind: StmtKind::Match { expr, arms }, span })
    }

    fn parse_enum(&mut self) -> Result<Stmt, String> {
        let span = self.peek_span();
        self.advance(); // consume 'enum'

        let name = if let Token::Ident(n) = self.peek_raw().clone() {
            self.advance();
            n
        } else {
            return Err(format!("Expected enum name, got {:?}", self.peek_raw()));
        };

        // Expect newline + indent
        if *self.peek_raw() != Token::Newline {
            return Err("Expected newline after enum name".to_string());
        }
        self.advance(); // consume newline

        if *self.peek_raw() != Token::Indent {
            return Err("Expected indented block with enum variants".to_string());
        }
        self.advance(); // consume indent

        let mut variants = Vec::new();
        loop {
            match self.peek_raw().clone() {
                Token::Ident(variant) => {
                    self.advance();
                    variants.push(variant);
                    // Skip newline between variants
                    if *self.peek_raw() == Token::Newline {
                        self.advance();
                    }
                }
                Token::Dedent | Token::Eof => {
                    if *self.peek_raw() == Token::Dedent {
                        self.advance();
                    }
                    break;
                }
                _ => return Err(format!("Expected variant name in enum, got {:?}", self.peek_raw())),
            }
        }

        if variants.is_empty() {
            return Err("Enum must have at least one variant".to_string());
        }

        Ok(Stmt { kind: StmtKind::EnumDef { name, variants }, span })
    }

    fn parse_throw(&mut self) -> Result<Stmt, String> {
        let span = self.peek_span();
        self.advance(); // consume 'throw'
        let expr = self.parse_expression()?;
        Ok(Stmt { kind: StmtKind::Throw(expr), span })
    }

    fn parse_alias(&mut self) -> Result<Stmt, String> {
        let span = self.peek_span();
        self.advance(); // consume 'alias'

        let name = if let Token::Ident(name) = self.peek_raw().clone() {
            self.advance();
            name
        } else {
            return Err(format!("Expected identifier after 'alias', got {:?}", self.peek_raw()));
        };

        if *self.peek_raw() != Token::Assign {
            return Err(format!("Expected '=' after alias name, got {:?}", self.peek_raw()));
        }
        self.advance(); // consume '='

        if let Token::String(parts) = self.peek_raw().clone() {
            self.advance();
            if parts.len() == 1
                && let crate::lexer::token::StringPart::Literal(target) = &parts[0]
            {
                return Ok(Stmt { kind: StmtKind::Alias { name, target: target.clone() }, span });
            }
            Err("Alias target must be a simple string literal".to_string())
        } else {
            Err(format!("Expected string after '=', got {:?}", self.peek_raw()))
        }
    }

    fn parse_dyn(&mut self) -> Result<Stmt, String> {
        let span = self.peek_span();
        self.advance(); // consume 'dyn'

        match self.peek_raw().clone() {
            Token::Return => {
                self.advance(); // consume 'return'
                let value = match self.peek_raw() {
                    Token::Newline | Token::Dedent | Token::Eof | Token::Semicolon => None,
                    _ => Some(self.parse_expression()?),
                };
                Ok(Stmt { kind: StmtKind::Return { expr: value, is_dyn: true }, span })
            }
            Token::Ident(name) => {
                self.advance(); // consume ident

                // Type annotation — incompatible with dyn
                if *self.peek_raw() == Token::Colon {
                    return Err(format!("'dyn' and type annotation on '{name}' are incompatible"));
                }
                let type_ann: Option<TypeAnnotation> = None;

                // Error-tolerant
                let error_tolerant = if *self.peek_raw() == Token::Question {
                    self.advance();
                    true
                } else {
                    false
                };

                if *self.peek_raw() != Token::Assign {
                    return Err(format!("Expected '=' after 'dyn {name}'"));
                }
                self.advance();
                let expr = self.parse_expression()?;
                Ok(Stmt {
                    kind: StmtKind::Assign { name, error_tolerant, type_ann, is_dyn: true, expr },
                    span,
                })
            }
            other => Err(format!("Expected identifier or 'return' after 'dyn', got {other:?}")),
        }
    }

    fn parse_use(&mut self) -> Result<Stmt, String> {
        let span = self.peek_span();
        self.advance(); // consume 'use'

        if let Token::String(parts) = self.peek_raw().clone() {
            self.advance();
            if parts.len() == 1
                && let crate::lexer::token::StringPart::Literal(path) = &parts[0]
            {
                let alias = if *self.peek_raw() == Token::As {
                    self.advance(); // consume 'as'
                    if let Token::Ident(name) = self.peek_raw().clone() {
                        self.advance();
                        Some(name)
                    } else {
                        return Err(format!("Expected identifier after 'as', got {:?}", self.peek_raw()));
                    }
                } else {
                    None
                };
                return Ok(Stmt { kind: StmtKind::Use { path: path.clone(), alias }, span });
            }
            Err("Use path must be a simple string literal".to_string())
        } else {
            Err(format!("Expected string after 'use', got {:?}", self.peek_raw()))
        }
    }

    fn parse_free(&mut self) -> Result<Stmt, String> {
        let span = self.peek_span();
        self.advance(); // consume 'free'
        if let Token::Ident(name) = self.peek_raw().clone() {
            self.advance();
            Ok(Stmt { kind: StmtKind::Free(name), span })
        } else {
            Err(format!("Expected variable name after 'free', got {:?}", self.peek_raw()))
        }
    }

    /// Attempt to parse an optional type annotation: `: type` or `: { field: type, ... }`.
    /// Only consumes tokens if a valid annotation is found.
    /// Returns `None` if there is no `:` at the current position.
    fn try_parse_type_annotation(&mut self) -> Result<Option<TypeAnnotation>, String> {
        if *self.peek_raw() != Token::Colon {
            return Ok(None);
        }
        self.advance(); // consume ':'

        // Object shape annotation: { field: type, ... }
        if *self.peek_raw() == Token::LBrace {
            self.advance(); // consume '{'
            let mut fields = Vec::new();
            loop {
                if *self.peek_raw() == Token::RBrace {
                    self.advance();
                    break;
                }
                let field_name = if let Token::Ident(n) = self.peek_raw().clone() {
                    self.advance();
                    n
                } else {
                    return Err(format!(
                        "Expected field name in object type annotation, got {:?}",
                        self.peek_raw()
                    ));
                };
                if *self.peek_raw() != Token::Colon {
                    return Err(format!(
                        "Expected ':' after field name '{}' in object type annotation, got {:?}",
                        field_name, self.peek_raw()
                    ));
                }
                self.advance(); // consume ':'
                let type_name = if let Token::Ident(t) = self.peek_raw().clone() {
                    self.advance();
                    t
                } else {
                    return Err(format!(
                        "Expected type name in object type annotation, got {:?}",
                        self.peek_raw()
                    ));
                };
                fields.push((field_name, type_name));
                if *self.peek_raw() == Token::Comma {
                    self.advance();
                }
            }
            return Ok(Some(TypeAnnotation::Object(fields)));
        }

        // Simple type annotation: ident
        if let Token::Ident(type_name) = self.peek_raw().clone() {
            self.advance();
            return Ok(Some(TypeAnnotation::Simple(type_name)));
        }

        Err(format!(
            "Expected type name after ':', got {:?}",
            self.peek_raw()
        ))
    }

    fn parse_expression(&mut self) -> Result<Expr, String> {
        let mut ep = ExprParser::new(self.tokens, self.pos);
        let expr = ep.parse_expr(0)?;
        self.pos = ep.pos();
        Ok(expr)
    }
}

/// Walk a function body recursively and ensure all returns are either `dyn` or none are.
fn validate_dyn_returns(fn_name: &str, stmts: &[Stmt]) -> Result<(), String> {
    let mut has_dyn = false;
    let mut has_plain = false;
    collect_returns(stmts, &mut has_dyn, &mut has_plain);
    if has_dyn && has_plain {
        return Err(format!(
            "Function '{fn_name}': mixing 'dyn return' and 'return' is not allowed. All returns must be 'dyn return' or none."
        ));
    }
    Ok(())
}

fn collect_returns(stmts: &[Stmt], has_dyn: &mut bool, has_plain: &mut bool) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Return { is_dyn: true, .. } => *has_dyn = true,
            StmtKind::Return { is_dyn: false, .. } => *has_plain = true,
            StmtKind::If { body, else_body, .. } => {
                collect_returns(body, has_dyn, has_plain);
                if let Some(eb) = else_body {
                    collect_returns(eb, has_dyn, has_plain);
                }
            }
            StmtKind::While { body, .. } | StmtKind::For { body, .. } => {
                collect_returns(body, has_dyn, has_plain);
            }
            StmtKind::Match { arms, .. } => {
                for arm in arms {
                    collect_returns(&arm.body, has_dyn, has_plain);
                }
            }
            _ => {}
        }
    }
}

fn token_to_compound_op(tok: &Token) -> Result<CompoundOp, String> {
    match tok {
        Token::PlusAssign => Ok(CompoundOp::Add),
        Token::MinusAssign => Ok(CompoundOp::Sub),
        Token::StarAssign => Ok(CompoundOp::Mul),
        Token::SlashAssign => Ok(CompoundOp::Div),
        Token::PercentAssign => Ok(CompoundOp::Mod),
        Token::PowerAssign => Ok(CompoundOp::Pow),
        Token::BitAndAssign => Ok(CompoundOp::BitAnd),
        Token::BitOrAssign => Ok(CompoundOp::BitOr),
        Token::BitXorAssign => Ok(CompoundOp::BitXor),
        Token::ShlAssign => Ok(CompoundOp::Shl),
        Token::ShrAssign => Ok(CompoundOp::Shr),
        _ => Err(format!("Not a compound assignment operator: {tok:?}")),
    }
}
