//! Expression language used by derived values and constraints.

/// Expression evaluator and runtime support.
pub mod eval;
/// Tokenizer for expression source text.
pub mod lexer;
/// Parser and expression AST definitions.
pub mod parser;

use crate::error::SyamlError;

/// Parses expression source into an AST.
pub fn parse_expression(input: &str) -> Result<parser::Expr, SyamlError> {
    let tokens = lexer::tokenize(input)?;
    parser::parse(&tokens)
}
