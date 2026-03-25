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
            Token::Teach => self.parse_teach(),
            Token::Unsafe => self.parse_unsafe_stmt(),
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

    fn parse_teach(&mut self) -> Result<Stmt, String> {
        use crate::parser::ast::TeachType;
        let span = self.peek_span();
        self.advance(); // consume 'teach'

        // teach "C signature" from "library" [on "platform"] [as alias]
        let c_sig = if let Token::String(parts) = self.peek_raw().clone() {
            self.advance();
            if parts.len() == 1 {
                if let crate::lexer::token::StringPart::Literal(s) = &parts[0] {
                    s.clone()
                } else {
                    return Err("teach signature must be a plain string".to_string());
                }
            } else {
                return Err("teach signature must be a plain string".to_string());
            }
        } else {
            return Err(format!("Expected C signature string after 'teach', got {:?}", self.peek_raw()));
        };

        // Parse the C signature string
        let (return_type, name, params) = Self::parse_c_signature(&c_sig)?;

        // Parse 'from "library"'
        if *self.peek_raw() != Token::From {
            return Err(format!("Expected 'from' after teach declaration, got {:?}", self.peek_raw()));
        }
        self.advance(); // consume 'from'

        let library = if let Token::String(parts) = self.peek_raw().clone() {
            self.advance();
            if parts.len() == 1 {
                if let crate::lexer::token::StringPart::Literal(lib) = &parts[0] {
                    lib.clone()
                } else {
                    return Err("Library name must be a simple string literal".to_string());
                }
            } else {
                return Err("Library name must be a simple string literal".to_string());
            }
        } else {
            return Err(format!("Expected library string after 'from', got {:?}", self.peek_raw()));
        };

        // Optional: on "platform"
        let platform = if *self.peek_raw() == Token::On {
            self.advance(); // consume 'on'
            if let Token::String(parts) = self.peek_raw().clone() {
                self.advance();
                if parts.len() == 1 {
                    if let crate::lexer::token::StringPart::Literal(p) = &parts[0] {
                        Some(p.clone())
                    } else {
                        return Err("Platform must be a simple string literal".to_string());
                    }
                } else {
                    return Err("Platform must be a simple string literal".to_string());
                }
            } else {
                return Err(format!("Expected platform string after 'on', got {:?}", self.peek_raw()));
            }
        } else {
            None
        };

        // Optional: as "alias"
        let alias = if *self.peek_raw() == Token::As {
            self.advance(); // consume 'as'
            if let Token::Ident(a) = self.peek_raw().clone() {
                self.advance();
                Some(a)
            } else {
                return Err(format!("Expected identifier after 'as', got {:?}", self.peek_raw()));
            }
        } else {
            None
        };

        Ok(Stmt {
            kind: StmtKind::Teach { return_type, name, params, library, platform, alias },
            span,
        })
    }

    /// Parse a C function signature string like "int sqlite3_open(const char *filename, sqlite3 **ppDb)"
    fn parse_c_signature(sig: &str) -> Result<(crate::parser::ast::TeachType, String, Vec<(String, crate::parser::ast::TeachType, bool)>), String> {
        use crate::parser::ast::TeachType;
        let sig = sig.trim();

        // Find the opening parenthesis
        let paren_pos = sig.find('(')
            .ok_or_else(|| format!("Invalid C signature: missing '(' in '{sig}'"))?;
        let close_paren = sig.rfind(')')
            .ok_or_else(|| format!("Invalid C signature: missing ')' in '{sig}'"))?;

        // Everything before '(' is "return_type name" or "return_type *name"
        let before_paren = sig[..paren_pos].trim();
        let params_str = sig[paren_pos + 1..close_paren].trim();

        // Split return type and function name from before_paren
        // Handle: "int func", "const char *func", "sqlite3 *func", "void func"
        let (return_type, func_name) = Self::split_return_and_name(before_paren)?;

        // Parse parameters
        let mut params = Vec::new();
        if !params_str.is_empty() && params_str != "void" {
            for param_str in params_str.split(',') {
                let param = param_str.trim();
                if param.is_empty() { continue; }
                // Skip function pointer params (callbacks) — treat as handle
                if param.contains("(*)") || param.contains("(*") {
                    params.push(("_callback".to_string(), TeachType::Handle, false));
                    continue;
                }
                let (ptype, pname, is_output) = Self::parse_c_param(param)?;
                params.push((pname, ptype, is_output));
            }
        }

        Ok((return_type, func_name, params))
    }

    /// Split "const char *func_name" into (TeachType, "func_name")
    fn split_return_and_name(s: &str) -> Result<(crate::parser::ast::TeachType, String), String> {
        use crate::parser::ast::TeachType;
        // Work backwards: last token is the name (possibly with * prefix)
        let s = s.trim();

        // Find the function name — last identifier
        let mut name_start = s.len();
        for (i, c) in s.char_indices().rev() {
            if c.is_alphanumeric() || c == '_' {
                name_start = i;
            } else {
                break;
            }
        }
        let func_name = s[name_start..].trim().to_string();
        let type_part = s[..name_start].trim().trim_end_matches('*').trim();
        let has_pointer = s[..name_start].contains('*');

        let base_type = Self::map_c_type(type_part);
        if has_pointer {
            // Return type is a pointer — could be string (char*) or handle
            if type_part == "char" || type_part == "const char" {
                Ok((TeachType::String, func_name))
            } else {
                Ok((TeachType::Handle, func_name))
            }
        } else {
            Ok((base_type, func_name))
        }
    }

    /// Parse a single C parameter like "const char *name" or "sqlite3 **ppDb" or "int n"
    /// Returns (type, name, is_output)
    fn parse_c_param(param: &str) -> Result<(crate::parser::ast::TeachType, String, bool), String> {
        use crate::parser::ast::TeachType;
        let param = param.trim();

        // Count pointer stars
        let star_count = param.chars().filter(|&c| c == '*').count();

        // Remove stars and const
        let clean = param.replace('*', " ").replace("const ", "");
        let tokens: Vec<&str> = clean.split_whitespace().collect();

        if tokens.is_empty() {
            return Err("Empty parameter in teach signature".to_string());
        }

        // Last token is the name (or the type if unnamed like "int")
        let (type_tokens, param_name) = if tokens.len() == 1 {
            (&tokens[..1], format!("_p{}", 0))
        } else {
            (&tokens[..tokens.len() - 1], tokens[tokens.len() - 1].to_string())
        };

        let type_str = type_tokens.join(" ");
        let base_type = Self::map_c_type(&type_str);

        if star_count == 0 {
            // Plain value — input parameter
            Ok((base_type, param_name, false))
        } else if star_count == 1 {
            // Single pointer
            match base_type {
                TeachType::String => Ok((TeachType::String, param_name, false)), // char* → string input
                TeachType::Void => Ok((TeachType::Handle, param_name, false)),   // void* → generic handle input
                TeachType::Int | TeachType::Float => Ok((base_type, param_name, false)), // int*/double* → pass value
                _ => Ok((TeachType::Handle, param_name, false)), // unknown* → typed handle input
            }
        } else {
            // Double pointer (**) — output parameter: C writes into the pointed-to location
            // The base type determines what gets written back:
            // char** → string, int** → int, float/double** → float, unknown** → handle
            let out_type = match base_type {
                TeachType::String => TeachType::String,   // char** → output string
                TeachType::Int => TeachType::Int,         // int** → output int
                TeachType::Float => TeachType::Float,     // double** → output float
                TeachType::Void => TeachType::Handle,     // void** → output handle
                _ => TeachType::Handle,                   // unknown** → output handle
            };
            Ok((out_type, param_name, true))
        }
    }

    /// Map a C type string to TeachType
    fn map_c_type(s: &str) -> crate::parser::ast::TeachType {
        use crate::parser::ast::TeachType;
        match s.trim() {
            "void" => TeachType::Void,
            "int" | "long" | "long long" | "int64_t" | "size_t" | "ssize_t"
            | "unsigned int" | "unsigned long" | "uint64_t" | "int32_t" | "uint32_t" => TeachType::Int,
            "double" | "float" => TeachType::Float,
            "char" | "const char" => TeachType::String,
            _ => TeachType::Handle, // Unknown type → handle
        }
    }

    fn parse_unsafe_stmt(&mut self) -> Result<Stmt, String> {
        let span = self.peek_span();
        self.advance(); // consume 'unsafe'
        let inner = self.parse_stmt()?;
        Ok(Stmt { kind: StmtKind::UnsafeStmt(Box::new(inner)), span })
    }

    fn parse_teach_type(&mut self) -> Result<crate::parser::ast::TeachType, String> {
        use crate::parser::ast::TeachType;
        if let Token::Ident(t) = self.peek_raw().clone() {
            let tt = match t.as_str() {
                "void" => TeachType::Void,
                "int" => TeachType::Int,
                "float" => TeachType::Float,
                "string" => TeachType::String,
                "handle" => TeachType::Handle,
                _ => return Err(format!("Unknown teach type '{}'. Valid types: void, int, float, string, handle", t)),
            };
            self.advance();
            Ok(tt)
        } else {
            Err(format!("Expected type name, got {:?}", self.peek_raw()))
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
