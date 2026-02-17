//! Expression language used by derived values and constraints.

/// Expression evaluator and runtime support.
pub mod eval;
/// Tokenizer for expression source text.
pub mod lexer;
/// Parser and expression AST definitions.
pub mod parser;

use crate::error::SyamlError;

const MAX_EXPRESSION_TOKENS: usize = 1024;

/// Parses expression source into an AST.
pub fn parse_expression(input: &str) -> Result<parser::Expr, SyamlError> {
    let tokens = lexer::tokenize(input)?;
    if tokens.len() > MAX_EXPRESSION_TOKENS {
        return Err(SyamlError::ExpressionError(format!(
            "expression exceeds max token count ({MAX_EXPRESSION_TOKENS})"
        )));
    }
    parser::parse(&tokens)
}
