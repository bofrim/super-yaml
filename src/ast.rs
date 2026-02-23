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
    /// Optional `meta` section containing file metadata and external bindings.
    pub meta: Option<Meta>,
    /// Parsed schema section.
    pub schema: SchemaDoc,
    /// Parsed data section plus extracted type hints.
    pub data: DataDoc,
    /// Optional parsed functional section.
    #[serde(default)]
    pub functional: Option<FunctionalDoc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Fully compiled output data.
pub struct CompiledDocument {
    /// Resolved JSON value after env + expression resolution and validation.
    pub value: JsonValue,
    /// Non-fatal diagnostic messages collected during compilation (e.g. deprecation warnings).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
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
/// Optional `meta` section values.
pub struct Meta {
    /// Optional file-level metadata attached to the document.
    #[serde(default)]
    pub file: BTreeMap<String, JsonValue>,
    /// Symbol-to-environment binding map.
    #[serde(default)]
    pub env: BTreeMap<String, EnvBinding>,
    /// Named imports for pulling in external `.syaml` documents.
    #[serde(default)]
    pub imports: BTreeMap<String, ImportBinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Single environment binding definition from `meta.env`.
pub struct EnvBinding {
    /// Environment variable key to read from process/provider.
    pub key: String,
    /// Whether missing env input is an error when no default is provided.
    pub required: bool,
    /// Default value used when env input is missing.
    pub default: Option<JsonValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Single import entry from `meta.imports`.
pub struct ImportBinding {
    /// Filesystem path or URL to another `.syaml` document.
    pub path: String,
    /// Optional content hash in `algorithm:hex` format (e.g. `sha256:abcdef...`).
    #[serde(default)]
    pub hash: Option<String>,
    /// Optional Ed25519 detached signature for content verification.
    #[serde(default)]
    pub signature: Option<SignatureBinding>,
    /// Optional semver version requirement (e.g. `^1.2.0`).
    #[serde(default)]
    pub version: Option<String>,
    /// Sections to import from the document. `None` (or omitted) means all sections.
    /// Valid names: `"schema"`, `"data"`, `"functional"`.
    /// Example: `sections: [schema]` imports only schema types; data is not available.
    #[serde(default)]
    pub sections: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Ed25519 signature binding for import verification.
pub struct SignatureBinding {
    /// Path or URL to the Ed25519 public key file (DER/PEM).
    pub public_key: String,
    /// Base64-encoded detached Ed25519 signature over the raw file bytes.
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Parsed schema section.
pub struct SchemaDoc {
    /// Named schema definitions from top-level keys in the `schema` section.
    pub types: BTreeMap<String, JsonValue>,
    /// Type-local constraints keyed by type name, then by type-relative JSON path.
    #[serde(default)]
    pub type_constraints: BTreeMap<String, BTreeMap<String, Vec<String>>>,
    /// Maps child type name → parent type name for types declared with extends syntax.
    #[serde(default)]
    pub extends: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Parsed and normalized data section.
pub struct DataDoc {
    /// Data tree with canonical keys (type-hint suffixes removed).
    pub value: JsonValue,
    /// Extracted type hints keyed by normalized JSON path.
    pub type_hints: BTreeMap<String, String>,
    /// Freeze markers: keys frozen with `^` suffix in source.
    #[serde(default)]
    pub freeze_markers: FreezeMarkers,
}

/// Mutability mode declared on a schema node.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MutabilityMode {
    Frozen,
    Replace,
    AppendOnly,
    MapPutOnly,
    MonotoneIncrease,
}

/// Map from normalized JSON path (`$.a.b`) to `true` when that key is frozen.
pub type FreezeMarkers = BTreeMap<String, bool>;

/// Single parameter definition in a functional function.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterDef {
    /// Type reference as a JSON schema fragment.
    pub type_ref: serde_json::Value,
    /// Whether this parameter is mutable.
    pub mutable: bool,
}

/// Capability-scoped permission block.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DataPermissions {
    #[serde(default)]
    pub read: Vec<String>,
    #[serde(default)]
    pub write: Vec<String>,
}

/// Full permissions block for a function.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PermissionsDef {
    #[serde(default)]
    pub file: Option<serde_json::Value>,
    #[serde(default)]
    pub network: Option<serde_json::Value>,
    #[serde(default)]
    pub env_perms: Option<serde_json::Value>,
    #[serde(default)]
    pub process: Option<serde_json::Value>,
    #[serde(default)]
    pub data: Option<DataPermissions>,
}

/// A set of conditions that can be semantic, strict, or both.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConditionSet {
    /// Human-readable annotation strings (no validation).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub semantic: Vec<String>,
    /// Evaluatable expression strings (syntax + scope validated at compile time).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub strict: Vec<String>,
}

/// Structured body of a `specification` block.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SpecificationDef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preconditions: Option<ConditionSet>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub postconditions: Option<ConditionSet>,
    /// Any other specification keys (description, etc.) — pass-through.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// Single function definition in the `---functional` section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDef {
    /// Named input parameters.
    pub inputs: BTreeMap<String, ParameterDef>,
    /// Return type schema (optional).
    #[serde(default)]
    pub output: Option<serde_json::Value>,
    /// Error variants schema (optional).
    #[serde(default)]
    pub errors: Option<serde_json::Value>,
    /// Capability permissions (optional).
    #[serde(default)]
    pub permissions: Option<PermissionsDef>,
    /// Specification block (optional).
    #[serde(default)]
    pub specification: Option<SpecificationDef>,
}

/// Parsed `---functional` section.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FunctionalDoc {
    pub functions: BTreeMap<String, FunctionDef>,
}

/// Import policy from a module manifest restricting what module members can import.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportPolicy {
    /// Whether URL-based imports are allowed. Default: `true`.
    #[serde(default = "default_true")]
    pub allow_network_imports: bool,
    /// Whether every import must specify a `version`. Default: `false`.
    #[serde(default)]
    pub require_version: bool,
    /// Whether every import must specify a `hash`. Default: `false`.
    #[serde(default)]
    pub require_hash: bool,
    /// Whether every import must specify a `signature`. Default: `false`.
    #[serde(default)]
    pub require_signature: bool,
    /// If non-empty, only these domains are allowed for URL imports.
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    /// Module names that cannot be imported by files in this module.
    #[serde(default)]
    pub blocked_modules: Vec<String>,
}

fn default_true() -> bool {
    true
}

impl Default for ImportPolicy {
    fn default() -> Self {
        Self {
            allow_network_imports: true,
            require_version: false,
            require_hash: false,
            require_signature: false,
            allowed_domains: Vec::new(),
            blocked_modules: Vec::new(),
        }
    }
}

/// Parsed `module.syaml` manifest that defines a module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleManifest {
    /// Module name (must match the registry entry in `syaml.toml`).
    pub name: String,
    /// Optional semver version string.
    #[serde(default)]
    pub version: Option<String>,
    /// Optional human-readable description.
    #[serde(default)]
    pub description: Option<String>,
    /// Key-value metadata merged into `meta.file` for all module members (file-level wins).
    #[serde(default)]
    pub metadata: BTreeMap<String, JsonValue>,
    /// Import restrictions applied to all member files.
    #[serde(default)]
    pub import_policy: ImportPolicy,
    /// Imports injected into all module member files (file-level can shadow).
    #[serde(default)]
    pub imports: BTreeMap<String, ImportBinding>,
}
