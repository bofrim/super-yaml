//! Error definitions for all `super_yaml` compilation stages.

use thiserror::Error;

#[derive(Debug, Error)]
/// Top-level error type returned by public APIs.
pub enum SyamlError {
    /// Invalid or missing marker line (for example missing `---!syaml/v0`).
    #[error("marker error: {0}")]
    MarkerError(String),
    /// Invalid sections, ordering, or duplicate/unknown sections.
    #[error("section error: {0}")]
    SectionError(String),
    /// YAML subset parsing failure in a specific section.
    #[error("yaml parse error in section '{section}': {message}")]
    YamlParseError { section: String, message: String },
    /// Schema shape or schema-validation failure.
    #[error("schema error: {0}")]
    SchemaError(String),
    /// Type-hint extraction or resolution failure.
    #[error("type hint error: {0}")]
    TypeHintError(String),
    /// Expression parsing or evaluation failure.
    #[error("expression error: {0}")]
    ExpressionError(String),
    /// Constraint parsing or evaluation failure.
    #[error("constraint error: {0}")]
    ConstraintError(String),
    /// Environment binding resolution failure.
    #[error("environment error: {0}")]
    EnvError(String),
    /// Dependency cycle detected during expression resolution.
    #[error("cycle error: {0}")]
    CycleError(String),
    /// Import parsing, resolution, or loading failure.
    #[error("import error: {0}")]
    ImportError(String),
    /// Template expansion, variable binding, or template invocation failure.
    #[error("template error: {0}")]
    TemplateError(String),
    /// Output serialization failure.
    #[error("serialization error: {0}")]
    SerializationError(String),
    /// Hash verification mismatch or unsupported algorithm.
    #[error("hash verification failed: {0}")]
    HashError(String),
    /// Ed25519 signature verification failure.
    #[error("signature verification failed: {0}")]
    SignatureError(String),
    /// Semver version requirement not satisfied by imported file.
    #[error("version requirement not satisfied: {0}")]
    VersionError(String),
    /// Version field lifecycle rule violation (since/deprecated/removed checks).
    #[error("version field error: {0}")]
    VersionFieldError(String),
    /// HTTP fetch or remote-import failure.
    #[error("fetch error: {0}")]
    FetchError(String),
    /// Filesystem I/O error from CLI or callers that propagate I/O.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
