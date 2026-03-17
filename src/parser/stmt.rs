use crate::lexer::token::{Token, SpannedToken, Span};
use crate::parser::ast::{Stmt, StmtKind, ExprKind, Expr, CompoundOp};
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
            Token::Import => self.parse_import(),
            Token::Ident(name) => {
                // Could be: assignment, compound assign, fn def, or expr stmt
                // Look ahead to determine
                let saved_pos = self.pos;

                self.advance(); // consume ident

                // Check for ? (error-tolerant assignment)
                let error_tolerant = if *self.peek_raw() == Token::Question {
                    self.advance();
                    true
                } else {
                    false
                };

                match self.peek_raw().clone() {
                    Token::Assign => {
                        self.advance();
                        let expr = self.parse_expression()?;
                        Ok(Stmt {
                            kind: StmtKind::Assign { name, error_tolerant, expr },
                            span,
                        })
                    }
                    Token::PlusAssign | Token::MinusAssign | Token::StarAssign |
                    Token::SlashAssign | Token::PercentAssign | Token::PowerAssign |
                    Token::BitAndAssign | Token::BitOrAssign | Token::BitXorAssign |
                    Token::ShlAssign | Token::ShrAssign => {
                        let op_tok = self.advance().token.clone();
                        let op = token_to_compound_op(&op_tok)?;
                        let expr = self.parse_expression()?;
                        Ok(Stmt {
                            kind: StmtKind::CompoundAssign { name, op, expr },
                            span,
                        })
                    }
                    Token::LParen => {
                        // Could be function call (expr stmt) or function definition
                        // Check if an Indent follows after the call
                        self.pos = saved_pos;
                        self.parse_possible_fn_def_or_expr()
                    }
                    _ => {
                        // Just an identifier expression (or error-tolerant check)
                        if error_tolerant {
                            return Err("'?' requires an assignment (x? = ...)".to_string());
                        }
                        self.pos = saved_pos;
                        let expr = self.parse_expression()?;
                        Ok(Stmt { kind: StmtKind::ExprStmt(expr), span })
                    }
                }
            }
            _ => {
                let expr = self.parse_expression()?;
                Ok(Stmt { kind: StmtKind::ExprStmt(expr), span })
            }
        }
    }

    fn parse_possible_fn_def_or_expr(&mut self) -> Result<Stmt, String> {
        let span = self.peek_span();

        // Parse as expression first
        let expr = self.parse_expression()?;

        // Check if Indent follows — then it's a function definition
        self.skip_newlines();
        if *self.peek_raw() == Token::Indent {
            // It's a function definition
            if let ExprKind::Call { name, args, .. } = &expr.kind {
                let params: Vec<String> = args.iter().map(|a| {
                    if let ExprKind::Ident(n) = &a.kind {
                        Ok(n.clone())
                    } else {
                        Err("Function parameters must be identifiers".to_string())
                    }
                }).collect::<Result<Vec<_>, _>>()?;

                self.advance(); // consume Indent
                let body = self.parse_block()?;

                return Ok(Stmt {
                    kind: StmtKind::FnDef { name: name.clone(), params, body },
                    span,
                });
            }
            return Err("Expected function call before definition block".to_string());
        }

        Ok(Stmt { kind: StmtKind::ExprStmt(expr), span })
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

        Ok(Stmt { kind: StmtKind::Return(value), span })
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

    fn parse_expression(&mut self) -> Result<Expr, String> {
        let mut ep = ExprParser::new(self.tokens, self.pos);
        let expr = ep.parse_expr(0)?;
        self.pos = ep.pos();
        Ok(expr)
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
