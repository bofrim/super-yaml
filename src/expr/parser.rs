//! Recursive-descent parser and AST for expressions.

use crate::error::SyamlError;

use super::lexer::{Token, TokenKind};

#[derive(Debug, Clone)]
/// Expression AST node.
pub enum Expr {
    /// Number literal.
    Number(f64),
    /// String literal.
    String(String),
    /// Boolean literal.
    Bool(bool),
    /// Null literal.
    Null,
    /// Variable reference path (for example `a.b.c`).
    Var(Vec<String>),
    /// Unary operation.
    Unary {
        /// Unary operator.
        op: UnaryOp,
        /// Operand expression.
        expr: Box<Expr>,
    },
    /// Binary operation.
    Binary {
        /// Binary operator.
        op: BinaryOp,
        /// Left operand.
        left: Box<Expr>,
        /// Right operand.
        right: Box<Expr>,
    },
    /// Function call expression.
    Call {
        /// Function name.
        name: String,
        /// Call argument expressions.
        args: Vec<Expr>,
    },
}

#[derive(Debug, Clone, Copy)]
/// Unary operators.
pub enum UnaryOp {
    /// Arithmetic negation (`-x`).
    Neg,
    /// Boolean negation (`!x`).
    Not,
}

#[derive(Debug, Clone, Copy)]
/// Binary operators in the expression language.
pub enum BinaryOp {
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
    /// `%`
    Mod,
    /// `==`
    Eq,
    /// `!=`
    NotEq,
    /// `<`
    Lt,
    /// `<=`
    Lte,
    /// `>`
    Gt,
    /// `>=`
    Gte,
    /// `&&`
    And,
    /// `||`
    Or,
}

/// Parses token stream into an expression AST.
pub fn parse(tokens: &[Token]) -> Result<Expr, SyamlError> {
    let mut parser = Parser { tokens, pos: 0 };
    let expr = parser.parse_or()?;
    if !matches!(parser.current().kind, TokenKind::Eof) {
        return Err(SyamlError::ExpressionError(format!(
            "unexpected token after expression at position {}",
            parser.current().pos
        )));
    }
    Ok(expr)
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn parse_or(&mut self) -> Result<Expr, SyamlError> {
        let mut expr = self.parse_and()?;
        while self.consume_if(|k| matches!(k, TokenKind::OrOr)).is_some() {
            let right = self.parse_and()?;
            expr = Expr::Binary {
                op: BinaryOp::Or,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_and(&mut self) -> Result<Expr, SyamlError> {
        let mut expr = self.parse_equality()?;
        while self
            .consume_if(|k| matches!(k, TokenKind::AndAnd))
            .is_some()
        {
            let right = self.parse_equality()?;
            expr = Expr::Binary {
                op: BinaryOp::And,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_equality(&mut self) -> Result<Expr, SyamlError> {
        let mut expr = self.parse_comparison()?;
        loop {
            let op = if self.consume_if(|k| matches!(k, TokenKind::EqEq)).is_some() {
                Some(BinaryOp::Eq)
            } else if self.consume_if(|k| matches!(k, TokenKind::NotEq)).is_some() {
                Some(BinaryOp::NotEq)
            } else {
                None
            };

            if let Some(op) = op {
                let right = self.parse_comparison()?;
                expr = Expr::Binary {
                    op,
                    left: Box::new(expr),
                    right: Box::new(right),
                };
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn parse_comparison(&mut self) -> Result<Expr, SyamlError> {
        let mut expr = self.parse_term()?;
        loop {
            let op = if self.consume_if(|k| matches!(k, TokenKind::Lt)).is_some() {
                Some(BinaryOp::Lt)
            } else if self.consume_if(|k| matches!(k, TokenKind::Lte)).is_some() {
                Some(BinaryOp::Lte)
            } else if self.consume_if(|k| matches!(k, TokenKind::Gt)).is_some() {
                Some(BinaryOp::Gt)
            } else if self.consume_if(|k| matches!(k, TokenKind::Gte)).is_some() {
                Some(BinaryOp::Gte)
            } else {
                None
            };

            if let Some(op) = op {
                let right = self.parse_term()?;
                expr = Expr::Binary {
                    op,
                    left: Box::new(expr),
                    right: Box::new(right),
                };
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn parse_term(&mut self) -> Result<Expr, SyamlError> {
        let mut expr = self.parse_factor()?;
        loop {
            let op = if self.consume_if(|k| matches!(k, TokenKind::Plus)).is_some() {
                Some(BinaryOp::Add)
            } else if self.consume_if(|k| matches!(k, TokenKind::Minus)).is_some() {
                Some(BinaryOp::Sub)
            } else {
                None
            };

            if let Some(op) = op {
                let right = self.parse_factor()?;
                expr = Expr::Binary {
                    op,
                    left: Box::new(expr),
                    right: Box::new(right),
                };
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn parse_factor(&mut self) -> Result<Expr, SyamlError> {
        let mut expr = self.parse_unary()?;
        loop {
            let op = if self.consume_if(|k| matches!(k, TokenKind::Star)).is_some() {
                Some(BinaryOp::Mul)
            } else if self.consume_if(|k| matches!(k, TokenKind::Slash)).is_some() {
                Some(BinaryOp::Div)
            } else if self
                .consume_if(|k| matches!(k, TokenKind::Percent))
                .is_some()
            {
                Some(BinaryOp::Mod)
            } else {
                None
            };

            if let Some(op) = op {
                let right = self.parse_unary()?;
                expr = Expr::Binary {
                    op,
                    left: Box::new(expr),
                    right: Box::new(right),
                };
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn parse_unary(&mut self) -> Result<Expr, SyamlError> {
        if self.consume_if(|k| matches!(k, TokenKind::Minus)).is_some() {
            let expr = self.parse_unary()?;
            return Ok(Expr::Unary {
                op: UnaryOp::Neg,
                expr: Box::new(expr),
            });
        }

        if self.consume_if(|k| matches!(k, TokenKind::Bang)).is_some() {
            let expr = self.parse_unary()?;
            return Ok(Expr::Unary {
                op: UnaryOp::Not,
                expr: Box::new(expr),
            });
        }

        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expr, SyamlError> {
        let token = self.current().clone();
        match token.kind {
            TokenKind::Number(n) => {
                self.pos += 1;
                Ok(Expr::Number(n))
            }
            TokenKind::String(ref s) => {
                self.pos += 1;
                Ok(Expr::String(s.clone()))
            }
            TokenKind::Bool(v) => {
                self.pos += 1;
                Ok(Expr::Bool(v))
            }
            TokenKind::Null => {
                self.pos += 1;
                Ok(Expr::Null)
            }
            TokenKind::Ident(ref s) => {
                let mut segments = vec![s.clone()];
                self.pos += 1;
                while self.consume_if(|k| matches!(k, TokenKind::Dot)).is_some() {
                    let ident = match &self.current().kind {
                        TokenKind::Ident(v) => v.clone(),
                        _ => {
                            return Err(SyamlError::ExpressionError(format!(
                                "expected identifier after '.' at {}",
                                self.current().pos
                            )))
                        }
                    };
                    self.pos += 1;
                    segments.push(ident);
                }

                if segments.len() == 1
                    && self
                        .consume_if(|k| matches!(k, TokenKind::LParen))
                        .is_some()
                {
                    let mut args = Vec::new();
                    if self
                        .consume_if(|k| matches!(k, TokenKind::RParen))
                        .is_none()
                    {
                        loop {
                            args.push(self.parse_or()?);
                            if self.consume_if(|k| matches!(k, TokenKind::Comma)).is_some() {
                                continue;
                            }
                            self.expect(
                                |k| matches!(k, TokenKind::RParen),
                                "expected ')' after call",
                            )?;
                            break;
                        }
                    }

                    Ok(Expr::Call {
                        name: segments[0].clone(),
                        args,
                    })
                } else {
                    Ok(Expr::Var(segments))
                }
            }
            TokenKind::LParen => {
                self.pos += 1;
                let expr = self.parse_or()?;
                self.expect(
                    |k| matches!(k, TokenKind::RParen),
                    "expected ')' after expression",
                )?;
                Ok(expr)
            }
            _ => Err(SyamlError::ExpressionError(format!(
                "unexpected token {:?} at {}",
                token.kind, token.pos
            ))),
        }
    }

    fn current(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn consume_if(&mut self, predicate: fn(&TokenKind) -> bool) -> Option<&Token> {
        if predicate(&self.current().kind) {
            let current = &self.tokens[self.pos];
            self.pos += 1;
            Some(current)
        } else {
            None
        }
    }

    fn expect(
        &mut self,
        predicate: fn(&TokenKind) -> bool,
        message: &str,
    ) -> Result<(), SyamlError> {
        if self.consume_if(predicate).is_some() {
            Ok(())
        } else {
            Err(SyamlError::ExpressionError(format!(
                "{} at {}",
                message,
                self.current().pos
            )))
        }
    }
}
