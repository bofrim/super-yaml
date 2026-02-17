use crate::error::SyamlError;

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    Number(f64),
    String(String),
    Bool(bool),
    Null,
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    LParen,
    RParen,
    Comma,
    Dot,
    Bang,
    EqEq,
    NotEq,
    Lt,
    Lte,
    Gt,
    Gte,
    AndAnd,
    OrOr,
    Eof,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub pos: usize,
}

pub fn tokenize(input: &str) -> Result<Vec<Token>, SyamlError> {
    let mut chars = input.char_indices().peekable();
    let mut tokens = Vec::new();

    while let Some((idx, ch)) = chars.peek().copied() {
        if ch.is_whitespace() {
            chars.next();
            continue;
        }

        match ch {
            '0'..='9' => {
                let start = idx;
                let mut end = idx;
                let mut seen_dot = false;
                while let Some((i, c)) = chars.peek().copied() {
                    if c.is_ascii_digit() {
                        end = i;
                        chars.next();
                    } else if c == '.' && !seen_dot {
                        seen_dot = true;
                        end = i;
                        chars.next();
                    } else {
                        break;
                    }
                }
                let raw = &input[start..=end];
                let n: f64 = raw.parse().map_err(|e| {
                    SyamlError::ExpressionError(format!(
                        "invalid number literal '{raw}' at {start}: {e}"
                    ))
                })?;
                tokens.push(Token {
                    kind: TokenKind::Number(n),
                    pos: start,
                });
            }
            '"' => {
                chars.next();
                let start = idx;
                let mut out = String::new();
                let mut closed = false;
                while let Some((_, c)) = chars.next() {
                    if c == '"' {
                        closed = true;
                        break;
                    }
                    if c == '\\' {
                        if let Some((_, esc)) = chars.next() {
                            match esc {
                                '"' => out.push('"'),
                                '\\' => out.push('\\'),
                                'n' => out.push('\n'),
                                't' => out.push('\t'),
                                'r' => out.push('\r'),
                                other => out.push(other),
                            }
                        } else {
                            return Err(SyamlError::ExpressionError(
                                "unterminated escape sequence".to_string(),
                            ));
                        }
                    } else {
                        out.push(c);
                    }
                }

                if !closed {
                    return Err(SyamlError::ExpressionError(format!(
                        "unterminated string literal starting at {start}"
                    )));
                }

                tokens.push(Token {
                    kind: TokenKind::String(out),
                    pos: start,
                });
            }
            '+' => {
                chars.next();
                tokens.push(Token {
                    kind: TokenKind::Plus,
                    pos: idx,
                });
            }
            '-' => {
                chars.next();
                tokens.push(Token {
                    kind: TokenKind::Minus,
                    pos: idx,
                });
            }
            '*' => {
                chars.next();
                tokens.push(Token {
                    kind: TokenKind::Star,
                    pos: idx,
                });
            }
            '/' => {
                chars.next();
                tokens.push(Token {
                    kind: TokenKind::Slash,
                    pos: idx,
                });
            }
            '%' => {
                chars.next();
                tokens.push(Token {
                    kind: TokenKind::Percent,
                    pos: idx,
                });
            }
            '(' => {
                chars.next();
                tokens.push(Token {
                    kind: TokenKind::LParen,
                    pos: idx,
                });
            }
            ')' => {
                chars.next();
                tokens.push(Token {
                    kind: TokenKind::RParen,
                    pos: idx,
                });
            }
            ',' => {
                chars.next();
                tokens.push(Token {
                    kind: TokenKind::Comma,
                    pos: idx,
                });
            }
            '.' => {
                chars.next();
                tokens.push(Token {
                    kind: TokenKind::Dot,
                    pos: idx,
                });
            }
            '!' => {
                chars.next();
                if matches!(chars.peek(), Some((_, '='))) {
                    chars.next();
                    tokens.push(Token {
                        kind: TokenKind::NotEq,
                        pos: idx,
                    });
                } else {
                    tokens.push(Token {
                        kind: TokenKind::Bang,
                        pos: idx,
                    });
                }
            }
            '=' => {
                chars.next();
                if matches!(chars.peek(), Some((_, '='))) {
                    chars.next();
                    tokens.push(Token {
                        kind: TokenKind::EqEq,
                        pos: idx,
                    });
                } else {
                    return Err(SyamlError::ExpressionError(format!(
                        "unexpected '=' at {idx}; use '==' for equality"
                    )));
                }
            }
            '<' => {
                chars.next();
                if matches!(chars.peek(), Some((_, '='))) {
                    chars.next();
                    tokens.push(Token {
                        kind: TokenKind::Lte,
                        pos: idx,
                    });
                } else {
                    tokens.push(Token {
                        kind: TokenKind::Lt,
                        pos: idx,
                    });
                }
            }
            '>' => {
                chars.next();
                if matches!(chars.peek(), Some((_, '='))) {
                    chars.next();
                    tokens.push(Token {
                        kind: TokenKind::Gte,
                        pos: idx,
                    });
                } else {
                    tokens.push(Token {
                        kind: TokenKind::Gt,
                        pos: idx,
                    });
                }
            }
            '&' => {
                chars.next();
                if matches!(chars.peek(), Some((_, '&'))) {
                    chars.next();
                    tokens.push(Token {
                        kind: TokenKind::AndAnd,
                        pos: idx,
                    });
                } else {
                    return Err(SyamlError::ExpressionError(format!(
                        "unexpected '&' at {idx}; expected '&&'"
                    )));
                }
            }
            '|' => {
                chars.next();
                if matches!(chars.peek(), Some((_, '|'))) {
                    chars.next();
                    tokens.push(Token {
                        kind: TokenKind::OrOr,
                        pos: idx,
                    });
                } else {
                    return Err(SyamlError::ExpressionError(format!(
                        "unexpected '|' at {idx}; expected '||'"
                    )));
                }
            }
            c if is_ident_start(c) => {
                let start = idx;
                let mut end = idx;
                while let Some((i, cc)) = chars.peek().copied() {
                    if is_ident_continue(cc) {
                        end = i;
                        chars.next();
                    } else {
                        break;
                    }
                }
                let raw = &input[start..=end];
                let kind = match raw {
                    "true" => TokenKind::Bool(true),
                    "false" => TokenKind::Bool(false),
                    "null" => TokenKind::Null,
                    _ => TokenKind::Ident(raw.to_string()),
                };
                tokens.push(Token { kind, pos: start });
            }
            _ => {
                return Err(SyamlError::ExpressionError(format!(
                    "unexpected character '{}' at {}",
                    ch, idx
                )))
            }
        }
    }

    tokens.push(Token {
        kind: TokenKind::Eof,
        pos: input.len(),
    });
    Ok(tokens)
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_ident_continue(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}
