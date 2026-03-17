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
                // `:` acts as indent in one-liners
                let current = indent_stack.last().copied().unwrap_or(0);
                let new_level = current + 4; // virtual indent
                indent_stack.push(new_level);
                colon_depth += 1;
                output.push(make_synthetic(Token::Indent, tok.span));
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
