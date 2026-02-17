pub mod eval;
pub mod lexer;
pub mod parser;

use crate::error::SyamlError;

pub fn parse_expression(input: &str) -> Result<parser::Expr, SyamlError> {
    let tokens = lexer::tokenize(input)?;
    parser::parse(&tokens)
}
