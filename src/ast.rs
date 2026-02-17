use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::error::SyamlError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedDocument {
    pub version: String,
    pub front_matter: Option<FrontMatter>,
    pub schema: SchemaDoc,
    pub data: DataDoc,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompiledDocument {
    pub value: JsonValue,
}

impl CompiledDocument {
    pub fn to_json_string(&self, pretty: bool) -> Result<String, SyamlError> {
        if pretty {
            serde_json::to_string_pretty(&self.value)
                .map_err(|e| SyamlError::SerializationError(e.to_string()))
        } else {
            serde_json::to_string(&self.value)
                .map_err(|e| SyamlError::SerializationError(e.to_string()))
        }
    }

    pub fn to_yaml_string(&self) -> String {
        crate::yaml_writer::to_yaml_string(&self.value)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrontMatter {
    pub env: BTreeMap<String, EnvBinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvBinding {
    pub key: String,
    pub required: bool,
    pub default: Option<JsonValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaDoc {
    pub types: BTreeMap<String, JsonValue>,
    pub constraints: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataDoc {
    pub value: JsonValue,
    pub type_hints: BTreeMap<String, String>,
}
