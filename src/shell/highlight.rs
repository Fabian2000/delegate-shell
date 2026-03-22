use rustyline::highlight::{CmdKind, Highlighter};
use std::borrow::Cow;

const RESET: &str = "\x1b[0m";
const KEYWORD: &str = "\x1b[38;5;204m";   // pink/red
const STRING: &str = "\x1b[38;5;114m";    // green
const NUMBER: &str = "\x1b[38;5;209m";    // orange
const COMMENT: &str = "\x1b[38;5;243m";   // gray
const OPERATOR: &str = "\x1b[38;5;81m";   // cyan
const BUILTIN: &str = "\x1b[38;5;75m";    // blue
const BOOL: &str = "\x1b[38;5;209m";      // orange (same as numbers)

const KEYWORDS: &[&str] = &[
    "if", "else", "while", "for", "in", "return", "throw", "match", "default",
    "import", "free", "alias", "use", "enum", "atomic", "dyn",
    "true", "false", "and", "or", "not",
];

pub struct DgshHighlighter {
    builtins: Vec<String>,
}

impl DgshHighlighter {
    pub fn new(builtins: Vec<String>) -> Self {
        Self { builtins }
    }

    fn highlight_line(&self, line: &str) -> String {
        let mut result = String::with_capacity(line.len() * 2);
        let chars: Vec<char> = line.chars().collect();
        let len = chars.len();
        let mut i = 0;

        while i < len {
            let c = chars[i];

            // Comment — rest of line
            if c == '#' {
                result.push_str(COMMENT);
                for &ch in &chars[i..] {
                    result.push(ch);
                }
                result.push_str(RESET);
                break;
            }

            // Double-quoted string
            if c == '"' {
                result.push_str(STRING);
                result.push(c);
                i += 1;
                while i < len {
                    let sc = chars[i];
                    result.push(sc);
                    i += 1;
                    if sc == '"' { break; }
                    if sc == '\\' && i < len {
                        result.push(chars[i]);
                        i += 1;
                    }
                }
                result.push_str(RESET);
                continue;
            }

            // Single-quoted string
            if c == '\'' {
                result.push_str(STRING);
                result.push(c);
                i += 1;
                while i < len {
                    let sc = chars[i];
                    result.push(sc);
                    i += 1;
                    if sc == '\'' { break; }
                }
                result.push_str(RESET);
                continue;
            }

            // Number
            if c.is_ascii_digit() || (c == '-' && i + 1 < len && chars[i + 1].is_ascii_digit() && (i == 0 || !chars[i - 1].is_alphanumeric())) {
                result.push_str(NUMBER);
                result.push(c);
                i += 1;
                while i < len && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    result.push(chars[i]);
                    i += 1;
                }
                result.push_str(RESET);
                continue;
            }

            // Multi-char operators
            if c == '-' && i + 1 < len && chars[i + 1] == '>' {
                result.push_str(OPERATOR);
                result.push_str("->");
                result.push_str(RESET);
                i += 2;
                continue;
            }
            if c == '?' && i + 1 < len && chars[i + 1] == '=' {
                result.push_str(OPERATOR);
                result.push_str("?=");
                result.push_str(RESET);
                i += 2;
                continue;
            }
            if c == '!' && i + 1 < len && chars[i + 1] == '!' {
                result.push_str(OPERATOR);
                result.push_str("!!");
                result.push_str(RESET);
                i += 2;
                continue;
            }
            if c == '+' && i + 1 < len && chars[i + 1] == '+' {
                result.push_str(OPERATOR);
                result.push_str("++");
                result.push_str(RESET);
                i += 2;
                continue;
            }
            if c == '-' && i + 1 < len && chars[i + 1] == '-' {
                result.push_str(OPERATOR);
                result.push_str("--");
                result.push_str(RESET);
                i += 2;
                continue;
            }

            // Single-char operators
            if matches!(c, '!' | '?' | '@' | '=' | '<' | '>' | '+' | '-' | '*' | '/' | '%' | '|' | '&' | '^' | '~') {
                result.push_str(OPERATOR);
                result.push(c);
                result.push_str(RESET);
                i += 1;
                continue;
            }

            // Word (identifier or keyword)
            if c.is_alphabetic() || c == '_' {
                let start = i;
                while i < len && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();

                if word == "true" || word == "false" {
                    result.push_str(BOOL);
                    result.push_str(&word);
                    result.push_str(RESET);
                } else if KEYWORDS.contains(&word.as_str()) {
                    result.push_str(KEYWORD);
                    result.push_str(&word);
                    result.push_str(RESET);
                } else if self.builtins.iter().any(|b| b == &word) {
                    result.push_str(BUILTIN);
                    result.push_str(&word);
                    result.push_str(RESET);
                } else {
                    result.push_str(&word);
                }
                continue;
            }

            // Everything else
            result.push(c);
            i += 1;
        }

        result
    }
}

impl Highlighter for DgshHighlighter {
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
        if line.is_empty() {
            return Cow::Borrowed(line);
        }
        Cow::Owned(self.highlight_line(line))
    }

    fn highlight_char(&self, _line: &str, _pos: usize, _kind: CmdKind) -> bool {
        true
    }

    fn highlight_prompt<'b, 's: 'b, 'p: 'b>(&'s self, prompt: &'p str, _default: bool) -> Cow<'b, str> {
        // Prompt in dim cyan
        Cow::Owned(format!("\x1b[38;5;243m{}{}", prompt, RESET))
    }
}
