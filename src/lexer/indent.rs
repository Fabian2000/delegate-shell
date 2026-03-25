use super::token::{Token, SpannedToken, Span};

/// Processes raw tokens and emits synthetic Indent/Dedent/Newline tokens
/// based on indentation levels. Also handles one-liner syntax:
/// `;` → Newline, `:` → Indent (with auto Dedent at next Newline).
#[must_use]
pub fn process_indentation(raw_tokens: &[SpannedToken]) -> Vec<SpannedToken> {
    let mut output: Vec<SpannedToken> = Vec::new();
    let mut indent_stack: Vec<usize> = vec![0];
    let mut i = 0;
    let mut colon_depth: usize = 0; // tracks `:` induced indentation

    while i < raw_tokens.len() {
        let tok = &raw_tokens[i];

        match &tok.token {
            Token::Newline => {
                // Emit pending colon dedents
                while colon_depth > 0 {
                    output.push(make_synthetic(Token::Dedent, tok.span));
                    colon_depth -= 1;
                    indent_stack.pop();
                }
                output.push(tok.clone());
                i += 1;
            }
            Token::Semicolon => {
                // `;` acts as newline in one-liners
                // First, close any colon-induced indentation
                while colon_depth > 0 {
                    output.push(make_synthetic(Token::Dedent, tok.span));
                    colon_depth -= 1;
                    indent_stack.pop();
                }
                output.push(make_synthetic(Token::Newline, tok.span));
                i += 1;
            }
            Token::Colon => {
                // A `:` can mean two things:
                //   1. One-liner block intro:  `if cond: stmt`
                //   2. Type annotation:        `x: int = 42`  or  `x: { f: int }`
                //
                // Heuristic: if the next token is a known type name (identifier
                // matching a built-in type) or `{` (object shape annotation),
                // treat this as a type annotation colon and keep it as-is.
                // Otherwise emit a synthetic Indent (one-liner block).
                let is_type_ann = match raw_tokens.get(i + 1).map(|t| &t.token) {
                    Some(Token::Ident(name)) => is_type_name(name),
                    Some(Token::LBrace) => true,
                    _ => false,
                };
                if is_type_ann {
                    // Keep as literal colon (type annotation context)
                    output.push(tok.clone());
                } else {
                    // Block-intro colon → emit synthetic Indent
                    let current = indent_stack.last().copied().unwrap_or(0);
                    let new_level = current + 4; // virtual indent
                    indent_stack.push(new_level);
                    colon_depth += 1;
                    output.push(make_synthetic(Token::Indent, tok.span));
                }
                i += 1;
            }
            _ => {
                output.push(tok.clone());
                i += 1;
            }
        }
    }

    // Close any remaining colon depths
    let span = Span { start: 0, end: 0 };
    while colon_depth > 0 {
        output.push(make_synthetic(Token::Dedent, span));
        colon_depth -= 1;
    }

    // Close any remaining indentation
    while indent_stack.len() > 1 {
        output.push(make_synthetic(Token::Dedent, span));
        indent_stack.pop();
    }

    output
}

const fn make_synthetic(token: Token, span: Span) -> SpannedToken {
    SpannedToken { token, span }
}

/// Returns true if `name` is a valid type annotation name.
/// These match the names returned by `Value::type_name()`.
fn is_type_name(name: &str) -> bool {
    matches!(
        name,
        "int" | "float" | "string" | "bool" | "list" | "object"
            | "void" | "ref" | "result" | "thread" | "bytes"
            | "filehandle" | "atomic" | "handle"
    )
}
