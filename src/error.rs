use thiserror::Error;

#[derive(Debug, Error)]
pub enum SyamlError {
    #[error("marker error: {0}")]
    MarkerError(String),
    #[error("section error: {0}")]
    SectionError(String),
    #[error("yaml parse error in section '{section}': {message}")]
    YamlParseError { section: String, message: String },
    #[error("schema error: {0}")]
    SchemaError(String),
    #[error("type hint error: {0}")]
    TypeHintError(String),
    #[error("expression error: {0}")]
    ExpressionError(String),
    #[error("constraint error: {0}")]
    ConstraintError(String),
    #[error("environment error: {0}")]
    EnvError(String),
    #[error("cycle error: {0}")]
    CycleError(String),
    #[error("serialization error: {0}")]
    SerializationError(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
