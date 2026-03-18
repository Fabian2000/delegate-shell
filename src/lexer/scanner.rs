use super::token::{Token, SpannedToken, Span, StringPart};
use super::indent;

pub struct Lexer {
    source: Vec<char>,
    pos: usize,
    line: usize,
    col: usize,
}

impl Lexer {
    #[must_use]
    pub fn new(source: &str) -> Self {
        Self {
            source: source.chars().collect(),
            pos: 0,
            line: 1,
            col: 0,
        }
    }

    #[must_use]
    pub fn tokenize(&mut self) -> Vec<SpannedToken> {
        let mut raw_tokens = Vec::new();
        let mut indent_stack: Vec<usize> = vec![0];
        let mut at_line_start = true;

        // Skip shebang
        if self.pos < self.source.len() && self.peek() == Some('#') && self.peek_at(1) == Some('!') {
            while self.pos < self.source.len() && self.peek() != Some('\n') {
                self.advance();
            }
        }

        while self.pos < self.source.len() {
            // Handle line starts — compute indentation
            if at_line_start {
                let indent_start = self.pos;
                let mut spaces = 0;
                while self.pos < self.source.len() {
                    match self.peek() {
                        Some(' ') => { spaces += 1; self.advance(); }
                        Some('\t') => { spaces += 4; self.advance(); }
                        _ => break,
                    }
                }

                // Skip blank lines
                if self.peek() == Some('\n') {
                    self.advance();
                    continue;
                }
                // Skip comment-only lines (but still emit the indentation changes)
                if self.peek() == Some('#') {
                    // still process indentation before the comment
                }

                let current_indent = indent_stack.last().copied().unwrap_or(0);
                let span = Span { start: indent_start, end: self.pos };

                if spaces > current_indent {
                    indent_stack.push(spaces);
                    raw_tokens.push(SpannedToken { token: Token::Indent, span });
                } else {
                    while spaces < indent_stack.last().copied().unwrap_or(0) {
                        indent_stack.pop();
                        raw_tokens.push(SpannedToken { token: Token::Dedent, span });
                    }
                }

                at_line_start = false;
            }

            match self.peek() {
                None => break,
                Some('\n') => {
                    let start = self.pos;
                    self.advance();
                    raw_tokens.push(SpannedToken {
                        token: Token::Newline,
                        span: Span { start, end: self.pos },
                    });
                    at_line_start = true;
                }
                Some(' ' | '\t' | '\r') => {
                    self.advance(); // skip whitespace mid-line
                }
                Some('#') => {
                    // Comment — skip to end of line
                    while self.pos < self.source.len() && self.peek() != Some('\n') {
                        self.advance();
                    }
                }
                Some('"') => {
                    raw_tokens.push(self.scan_string());
                }
                Some('\'') => {
                    raw_tokens.push(self.scan_raw_string());
                }
                Some(c) if c.is_ascii_digit() => {
                    raw_tokens.push(self.scan_number());
                }
                Some(c) if c.is_ascii_alphabetic() || c == '_' => {
                    raw_tokens.push(self.scan_identifier());
                }
                Some('$') => {
                    raw_tokens.push(self.scan_dollar());
                }
                Some(_) => {
                    raw_tokens.push(self.scan_operator());
                }
            }
        }

        // Close remaining indentation
        let eof_span = Span { start: self.pos, end: self.pos };
        while indent_stack.len() > 1 {
            indent_stack.pop();
            raw_tokens.push(SpannedToken { token: Token::Dedent, span: eof_span });
        }
        raw_tokens.push(SpannedToken { token: Token::Eof, span: eof_span });

        // Process one-liner `;` and `:` tokens
        indent::process_indentation(&raw_tokens)
    }

    fn peek(&self) -> Option<char> {
        self.source.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.source.get(self.pos + offset).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let c = self.source.get(self.pos).copied();
        if let Some(ch) = c {
            self.pos += 1;
            if ch == '\n' {
                self.line += 1;
                self.col = 0;
            } else {
                self.col += 1;
            }
        }
        c
    }

    fn scan_number(&mut self) -> SpannedToken {
        let start = self.pos;
        let mut is_float = false;

        // Check for 0b, 0x, 0o prefixes
        if self.peek() == Some('0') {
            match self.peek_at(1) {
                Some('b' | 'B') => {
                    self.advance(); self.advance(); // skip 0b
                    while let Some(c) = self.peek() {
                        if c == '0' || c == '1' || c == '_' { self.advance(); } else { break; }
                    }
                    let s: String = self.source[start..self.pos].iter().collect();
                    let s_clean = s.replace('_', "");
                    let val = i64::from_str_radix(&s_clean[2..], 2).unwrap_or(0);
                    return SpannedToken { token: Token::Int(val), span: Span { start, end: self.pos } };
                }
                Some('x' | 'X') => {
                    self.advance(); self.advance();
                    while let Some(c) = self.peek() {
                        if c.is_ascii_hexdigit() || c == '_' { self.advance(); } else { break; }
                    }
                    let s: String = self.source[start..self.pos].iter().collect();
                    let s_clean = s.replace('_', "");
                    let val = i64::from_str_radix(&s_clean[2..], 16).unwrap_or(0);
                    return SpannedToken { token: Token::Int(val), span: Span { start, end: self.pos } };
                }
                Some('o' | 'O') => {
                    self.advance(); self.advance();
                    while let Some(c) = self.peek() {
                        if ('0'..='7').contains(&c) || c == '_' { self.advance(); } else { break; }
                    }
                    let s: String = self.source[start..self.pos].iter().collect();
                    let s_clean = s.replace('_', "");
                    let val = i64::from_str_radix(&s_clean[2..], 8).unwrap_or(0);
                    return SpannedToken { token: Token::Int(val), span: Span { start, end: self.pos } };
                }
                _ => {}
            }
        }

        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || c == '_' {
                self.advance();
            } else if c == '.' && !is_float {
                // Check it's not `..` (range)
                if self.peek_at(1) == Some('.') {
                    break;
                }
                is_float = true;
                self.advance();
            } else {
                break;
            }
        }

        let s: String = self.source[start..self.pos].iter().collect();
        let s_clean = s.replace('_', "");
        let span = Span { start, end: self.pos };

        if is_float {
            let val: f64 = s_clean.parse().unwrap_or(0.0);
            SpannedToken { token: Token::Float(val), span }
        } else {
            let val: i64 = s_clean.parse().unwrap_or(0);
            SpannedToken { token: Token::Int(val), span }
        }
    }

    fn scan_identifier(&mut self) -> SpannedToken {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '_' {
                self.advance();
            } else {
                break;
            }
        }

        let raw: String = self.source[start..self.pos].iter().collect();
        let lower = raw.to_ascii_lowercase();
        let span = Span { start, end: self.pos };

        let token = match lower.as_str() {
            "if" => Token::If,
            "else" => Token::Else,
            "for" => Token::For,
            "while" => Token::While,
            "in" => Token::In,
            "return" => Token::Return,
            "import" => Token::Import,
            "free" => Token::Free,
            "use" => Token::Use,
            "as" => Token::As,
            "throw" => Token::Throw,
            "enum" => Token::Enum,
            "match" => Token::Match,
            "continue" => Token::Continue,
            "break" => Token::Break,
            "true" => Token::Bool(true),
            "false" => Token::Bool(false),
            _ => Token::Ident(lower),
        };

        SpannedToken { token, span }
    }

    fn scan_string(&mut self) -> SpannedToken {
        let start = self.pos;
        self.advance(); // skip opening "

        // Check for """ multiline string
        if self.peek() == Some('"') && self.peek_at(1) == Some('"') {
            self.advance(); // skip second "
            self.advance(); // skip third "
            return self.scan_multiline_string(start);
        }

        let mut parts: Vec<StringPart> = Vec::new();
        let mut current = String::new();

        while let Some(c) = self.peek() {
            match c {
                '"' => {
                    self.advance();
                    break;
                }
                '\\' => {
                    self.advance();
                    match self.peek() {
                        Some('n') => { current.push('\n'); self.advance(); }
                        Some('t') => { current.push('\t'); self.advance(); }
                        Some('r') => { current.push('\r'); self.advance(); }
                        Some('\\') => { current.push('\\'); self.advance(); }
                        Some('"') => { current.push('"'); self.advance(); }
                        Some('{') => { current.push('{'); self.advance(); }
                        Some('}') => { current.push('}'); self.advance(); }
                        Some(c) => { current.push('\\'); current.push(c); self.advance(); }
                        None => { current.push('\\'); }
                    }
                }
                '{' => {
                    // String interpolation — tokenize inner expression
                    if !current.is_empty() {
                        parts.push(StringPart::Literal(current.clone()));
                        current.clear();
                    }
                    self.advance(); // skip {
                    let interp_tokens = self.scan_interpolation();
                    parts.push(StringPart::Interpolation(interp_tokens));
                }
                _ => {
                    current.push(c);
                    self.advance();
                }
            }
        }

        if !current.is_empty() {
            parts.push(StringPart::Literal(current));
        }

        SpannedToken {
            token: Token::String(parts),
            span: Span { start, end: self.pos },
        }
    }

    fn scan_multiline_string(&mut self, start: usize) -> SpannedToken {
        // Skip leading newline after opening """
        if self.peek() == Some('\n') {
            self.advance();
        }
        let mut parts: Vec<StringPart> = Vec::new();
        let mut current = String::new();

        while let Some(c) = self.peek() {
            // Check for closing """
            if c == '"' && self.peek_at(1) == Some('"') && self.peek_at(2) == Some('"') {
                self.advance(); // skip first "
                self.advance(); // skip second "
                self.advance(); // skip third "
                break;
            }
            match c {
                '\\' => {
                    self.advance();
                    match self.peek() {
                        Some('n') => { current.push('\n'); self.advance(); }
                        Some('t') => { current.push('\t'); self.advance(); }
                        Some('r') => { current.push('\r'); self.advance(); }
                        Some('\\') => { current.push('\\'); self.advance(); }
                        Some('"') => { current.push('"'); self.advance(); }
                        Some('{') => { current.push('{'); self.advance(); }
                        Some('}') => { current.push('}'); self.advance(); }
                        Some(ch) => { current.push('\\'); current.push(ch); self.advance(); }
                        None => { current.push('\\'); }
                    }
                }
                '{' => {
                    if !current.is_empty() {
                        parts.push(StringPart::Literal(current.clone()));
                        current.clear();
                    }
                    self.advance();
                    let interp_tokens = self.scan_interpolation();
                    parts.push(StringPart::Interpolation(interp_tokens));
                }
                _ => {
                    current.push(c);
                    self.advance();
                }
            }
        }

        // Trim trailing newline before closing """
        if current.ends_with('\n') {
            current.pop();
        }

        if !current.is_empty() {
            parts.push(StringPart::Literal(current));
        }

        SpannedToken {
            token: Token::String(parts),
            span: Span { start, end: self.pos },
        }
    }

    fn scan_raw_string(&mut self) -> SpannedToken {
        let start = self.pos;
        self.advance(); // skip opening '
        let mut s = String::new();

        while let Some(c) = self.peek() {
            match c {
                '\'' => { self.advance(); break; }
                '\\' => {
                    self.advance();
                    match self.peek() {
                        Some('\'') => { s.push('\''); self.advance(); }
                        Some('\\') => { s.push('\\'); self.advance(); }
                        Some(c) => { s.push('\\'); s.push(c); self.advance(); }
                        None => { s.push('\\'); }
                    }
                }
                _ => { s.push(c); self.advance(); }
            }
        }

        SpannedToken {
            token: Token::String(vec![StringPart::Literal(s)]),
            span: Span { start, end: self.pos },
        }
    }

    fn scan_interpolation(&mut self) -> Vec<SpannedToken> {
        let mut tokens = Vec::new();
        let mut depth = 1;

        while self.pos < self.source.len() && depth > 0 {
            match self.peek() {
                Some('{') => {
                    depth += 1;
                    tokens.push(SpannedToken {
                        token: Token::LBrace,
                        span: Span { start: self.pos, end: self.pos + 1 },
                    });
                    self.advance();
                }
                Some('}') => {
                    depth -= 1;
                    if depth == 0 {
                        self.advance(); // consume closing }
                        break;
                    }
                    tokens.push(SpannedToken {
                        token: Token::RBrace,
                        span: Span { start: self.pos, end: self.pos + 1 },
                    });
                    self.advance();
                }
                Some(' ' | '\t') => { self.advance(); }
                Some(c) if c.is_ascii_digit() => {
                    tokens.push(self.scan_number());
                }
                Some(c) if c.is_ascii_alphabetic() || c == '_' => {
                    tokens.push(self.scan_identifier());
                }
                Some('$') => {
                    tokens.push(self.scan_dollar());
                }
                Some('.') => {
                    let start = self.pos;
                    self.advance();
                    tokens.push(SpannedToken { token: Token::Dot, span: Span { start, end: self.pos } });
                }
                Some(_) => {
                    tokens.push(self.scan_operator());
                }
                None => break,
            }
        }

        tokens
    }

    fn scan_dollar(&mut self) -> SpannedToken {
        let start = self.pos;
        self.advance(); // skip $

        let suffix_start = self.pos;

        // Check for $0, $1, etc. or $fieldname
        let token = match self.peek() {
            Some(c) if c.is_ascii_digit() => {
                while let Some(c) = self.peek() {
                    if c.is_ascii_digit() { self.advance(); } else { break; }
                }
                let num_str: String = self.source[suffix_start..self.pos].iter().collect();
                let idx = num_str.parse::<usize>().unwrap_or(0);
                Token::DollarIndex(idx)
            }
            Some(c) if c.is_ascii_alphabetic() || c == '_' => {
                while let Some(c) = self.peek() {
                    if c.is_ascii_alphanumeric() || c == '_' { self.advance(); } else { break; }
                }
                let field: String = self.source[suffix_start..self.pos].iter().collect();
                Token::DollarField(field.to_ascii_lowercase())
            }
            _ => Token::Dollar, // bare $
        };

        SpannedToken {
            token,
            span: Span { start, end: self.pos },
        }
    }

    fn scan_operator(&mut self) -> SpannedToken {
        let start = self.pos;
        let c = self.advance().unwrap();

        let token = match c {
            '+' | '-' | '*' | '/' | '%' => self.scan_arithmetic_op(c),
            '=' | '!' | '<' | '>' => self.scan_comparison_op(c),
            '&' | '|' | '^' | '~' => self.scan_bitwise_op(c),
            '(' => Token::LParen,
            ')' => Token::RParen,
            '[' => Token::LBracket,
            ']' => Token::RBracket,
            '{' => Token::LBrace,
            '}' => Token::RBrace,
            ',' => Token::Comma,
            '.' => {
                if self.peek() == Some('.') { self.advance(); Token::Range }
                else { Token::Dot }
            }
            ':' => Token::Colon,
            ';' => Token::Semicolon,
            '$' => unreachable!("$ handled by scan_dollar"),
            '@' => Token::At,
            '?' => {
                if self.peek() == Some('>') { self.advance(); Token::SafeSend }
                else { Token::Question }
            }
            _ => Token::Ident(format!("__unknown_{c}")),
        };

        SpannedToken {
            token,
            span: Span { start, end: self.pos },
        }
    }

    fn scan_arithmetic_op(&mut self, c: char) -> Token {
        match c {
            '+' => {
                if self.peek() == Some('+') { self.advance(); Token::Increment }
                else if self.peek() == Some('=') { self.advance(); Token::PlusAssign }
                else { Token::Plus }
            }
            '-' => {
                if self.peek() == Some('-') { self.advance(); Token::Decrement }
                else if self.peek() == Some('>') { self.advance(); Token::Send }
                else if self.peek() == Some('=') { self.advance(); Token::MinusAssign }
                else { Token::Minus }
            }
            '*' => {
                if self.peek() == Some('*') {
                    self.advance();
                    if self.peek() == Some('=') { self.advance(); Token::PowerAssign }
                    else { Token::Power }
                } else if self.peek() == Some('=') {
                    self.advance(); Token::StarAssign
                } else {
                    Token::Star
                }
            }
            '/' => {
                if self.peek() == Some('=') { self.advance(); Token::SlashAssign }
                else { Token::Slash }
            }
            _ => {
                // '%'
                if self.peek() == Some('=') { self.advance(); Token::PercentAssign }
                else { Token::Percent }
            }
        }
    }

    fn scan_comparison_op(&mut self, c: char) -> Token {
        match c {
            '=' => {
                if self.peek() == Some('=') { self.advance(); Token::Eq }
                else { Token::Assign }
            }
            '!' => {
                if self.peek() == Some('=') { self.advance(); Token::NotEq }
                else { Token::Bang }
            }
            '<' => {
                if self.peek() == Some('=') { self.advance(); Token::LtEq }
                else if self.peek() == Some('<') {
                    self.advance();
                    if self.peek() == Some('=') { self.advance(); Token::ShlAssign }
                    else { Token::Shl }
                }
                else { Token::Lt }
            }
            _ => {
                // '>'
                if self.peek() == Some('=') { self.advance(); Token::GtEq }
                else if self.peek() == Some('>') {
                    self.advance();
                    if self.peek() == Some('=') { self.advance(); Token::ShrAssign }
                    else { Token::Shr }
                }
                else { Token::Gt }
            }
        }
    }

    fn scan_bitwise_op(&mut self, c: char) -> Token {
        match c {
            '&' => {
                if self.peek() == Some('&') { self.advance(); Token::And }
                else if self.peek() == Some('=') { self.advance(); Token::BitAndAssign }
                else { Token::BitAnd }
            }
            '|' => {
                if self.peek() == Some('|') { self.advance(); Token::Or }
                else if self.peek() == Some('=') { self.advance(); Token::BitOrAssign }
                else { Token::BitOr }
            }
            '^' => {
                if self.peek() == Some('=') { self.advance(); Token::BitXorAssign }
                else { Token::BitXor }
            }
            _ => Token::BitNot, // '~'
        }
    }
}
