//! Public document model used by parser and compiler APIs.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::error::SyamlError;

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Parsed `.syaml` document before expression and constraint resolution.
pub struct ParsedDocument {
    /// Format version extracted from the marker line (`---!syaml/v0` -> `v0`).
    pub version: String,
    /// Optional `front_matter` section containing external bindings.
    pub front_matter: Option<FrontMatter>,
    /// Parsed schema section.
    pub schema: SchemaDoc,
    /// Parsed data section plus extracted type hints.
    pub data: DataDoc,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Fully compiled output data.
pub struct CompiledDocument {
    /// Resolved JSON value after env + expression resolution and validation.
    pub value: JsonValue,
}

impl CompiledDocument {
    /// Serializes compiled data to JSON text.
    ///
    /// When `pretty` is `true`, output is formatted with indentation.
    pub fn to_json_string(&self, pretty: bool) -> Result<String, SyamlError> {
        if pretty {
            serde_json::to_string_pretty(&self.value)
                .map_err(|e| SyamlError::SerializationError(e.to_string()))
        } else {
            serde_json::to_string(&self.value)
                .map_err(|e| SyamlError::SerializationError(e.to_string()))
        }
    }

    /// Serializes compiled data to YAML text.
    pub fn to_yaml_string(&self) -> String {
        crate::yaml_writer::to_yaml_string(&self.value)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Optional `front_matter` section values.
pub struct FrontMatter {
    /// Symbol-to-environment binding map.
    pub env: BTreeMap<String, EnvBinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Single environment binding definition from `front_matter.env`.
pub struct EnvBinding {
    /// Environment variable key to read from process/provider.
    pub key: String,
    /// Whether missing env input is an error when no default is provided.
    pub required: bool,
    /// Default value used when env input is missing.
    pub default: Option<JsonValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Parsed schema section.
pub struct SchemaDoc {
    /// Named schema definitions under `schema.types`.
    pub types: BTreeMap<String, JsonValue>,
    /// Constraint expressions keyed by JSON path.
    pub constraints: BTreeMap<String, Vec<String>>,
    /// Type-local constraints keyed by type name, then by type-relative JSON path.
    #[serde(default)]
    pub type_constraints: BTreeMap<String, BTreeMap<String, Vec<String>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Parsed and normalized data section.
pub struct DataDoc {
    /// Data tree with canonical keys (type-hint suffixes removed).
    pub value: JsonValue,
    /// Extracted type hints keyed by normalized JSON path.
    pub type_hints: BTreeMap<String, String>,
}
