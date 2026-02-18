//! `super_yaml` compiles sectioned `.syaml` documents into resolved JSON or YAML.
//!
//! A document goes through these stages:
//! 1. Section scanning and shape validation (`---!syaml/v0`, section order, required sections).
//! 2. Parsing section bodies with the built-in YAML subset parser.
//! 3. Schema extraction and type-hint normalization.
//! 4. Environment binding resolution.
//! 5. Derived expression/interpolation resolution.
//! 6. Type-hint and constraint validation.
//!
//! Use [`compile_document`] for full compilation, [`validate_document`] for validation-only
//! workflows, or [`compile_document_to_json`] / [`compile_document_to_yaml`] for serialized output.

/// Abstract syntax tree and compiled document container types.
pub mod ast;
/// Error types used throughout parsing, compilation, and validation.
pub mod error;
/// Expression lexer/parser/evaluator used by derived values and constraints.
pub mod expr;
/// Minimal YAML subset parser used for section bodies.
pub mod mini_yaml;
/// Environment and expression resolution over parsed data.
pub mod resolve;
/// Schema parsing and schema-based validation helpers.
pub mod schema;
/// Top-level section marker scanner and order validator.
pub mod section_scanner;
/// Type-hint extraction (`key <Type>`) and normalization.
pub mod type_hints;
/// Constraint and type-hint validation routines.
pub mod validate;
/// JSON-to-YAML renderer used by compiled YAML output.
pub mod yaml_writer;

use std::collections::BTreeMap;

use serde_json::Value as JsonValue;

use ast::{CompiledDocument, DataDoc, EnvBinding, FrontMatter, ParsedDocument};
pub use error::SyamlError;
use resolve::{resolve_env_bindings, resolve_expressions};
pub use resolve::{EnvProvider, MapEnvProvider, ProcessEnvProvider};
use schema::parse_schema;
use section_scanner::scan_sections;
use type_hints::normalize_data_with_hints;
use validate::{build_effective_constraints, validate_constraints, validate_type_hints};

/// Parses a `.syaml` document into its structured representation.
///
/// This performs marker and section validation, section body parsing, schema parsing,
/// and data type-hint normalization. It does not resolve expressions or environment
/// bindings and does not run constraints.
pub fn parse_document(input: &str) -> Result<ParsedDocument, SyamlError> {
    let (version, sections) = scan_sections(input)?;

    let mut front_matter: Option<FrontMatter> = None;
    let mut schema = None;
    let mut data = None;

    for section in sections {
        let section_value = parse_section_value(&section.name, &section.body)?;
        match section.name.as_str() {
            "front_matter" => {
                front_matter = Some(parse_front_matter(&section_value)?);
            }
            "schema" => {
                schema = Some(parse_schema(&section_value)?);
            }
            "data" => {
                let (value, type_hints) = normalize_data_with_hints(&section_value)?;
                data = Some(DataDoc { value, type_hints });
            }
            _ => unreachable!("validated by section scanner"),
        }
    }

    Ok(ParsedDocument {
        version,
        front_matter,
        schema: schema.ok_or_else(|| {
            SyamlError::SectionError("missing required section 'schema'".to_string())
        })?,
        data: data.ok_or_else(|| {
            SyamlError::SectionError("missing required section 'data'".to_string())
        })?,
    })
}

/// Compiles a `.syaml` document into resolved data.
///
/// Compilation includes expression resolution, environment substitution, type-hint
/// checks, and constraint evaluation.
pub fn compile_document(
    input: &str,
    env_provider: &dyn EnvProvider,
) -> Result<CompiledDocument, SyamlError> {
    let parsed = parse_document(input)?;

    let env_values = resolve_env_bindings(parsed.front_matter.as_ref(), env_provider)?;
    let mut resolved_data = parsed.data.value.clone();
    resolve_expressions(&mut resolved_data, &env_values)?;

    validate_type_hints(&resolved_data, &parsed.data.type_hints, &parsed.schema)?;
    let constraints = build_effective_constraints(&parsed.data.type_hints, &parsed.schema);
    validate_constraints(&resolved_data, &env_values, &constraints)?;

    Ok(CompiledDocument {
        value: resolved_data,
    })
}

/// Validates a `.syaml` document without returning compiled output.
///
/// This runs the full compilation pipeline and discards the result.
pub fn validate_document(input: &str, env_provider: &dyn EnvProvider) -> Result<(), SyamlError> {
    compile_document(input, env_provider).map(|_| ())
}

/// Compiles a `.syaml` document and returns JSON text.
///
/// Set `pretty` to `true` to emit pretty-printed JSON.
pub fn compile_document_to_json(
    input: &str,
    env_provider: &dyn EnvProvider,
    pretty: bool,
) -> Result<String, SyamlError> {
    let compiled = compile_document(input, env_provider)?;
    compiled.to_json_string(pretty)
}

/// Compiles a `.syaml` document and returns YAML text.
pub fn compile_document_to_yaml(
    input: &str,
    env_provider: &dyn EnvProvider,
) -> Result<String, SyamlError> {
    let compiled = compile_document(input, env_provider)?;
    Ok(compiled.to_yaml_string())
}

fn parse_section_value(section: &str, body: &str) -> Result<JsonValue, SyamlError> {
    mini_yaml::parse_document(body).map_err(|e| match e {
        SyamlError::YamlParseError { message, .. } => SyamlError::YamlParseError {
            section: section.to_string(),
            message,
        },
        other => other,
    })
}

fn parse_front_matter(value: &JsonValue) -> Result<FrontMatter, SyamlError> {
    let map = value.as_object().ok_or_else(|| {
        SyamlError::SchemaError("front_matter section must be a mapping/object".to_string())
    })?;

    let mut env = BTreeMap::new();

    if let Some(env_value) = map.get("env") {
        let env_map = env_value.as_object().ok_or_else(|| {
            SyamlError::SchemaError("front_matter.env must be a mapping/object".to_string())
        })?;

        for (symbol, binding_value) in env_map {
            let binding = parse_env_binding(symbol, binding_value)?;
            env.insert(symbol.clone(), binding);
        }
    }

    Ok(FrontMatter { env })
}

fn parse_env_binding(symbol: &str, value: &JsonValue) -> Result<EnvBinding, SyamlError> {
    let map = value.as_object().ok_or_else(|| {
        SyamlError::SchemaError(format!(
            "front_matter.env.{} must be a mapping/object",
            symbol
        ))
    })?;

    let from = map.get("from").and_then(|v| v.as_str()).unwrap_or("env");

    if from != "env" {
        return Err(SyamlError::SchemaError(format!(
            "front_matter.env.{} has unsupported from='{}'; only 'env' is supported",
            symbol, from
        )));
    }

    let key = map.get("key").and_then(|v| v.as_str()).ok_or_else(|| {
        SyamlError::SchemaError(format!(
            "front_matter.env.{} must define string key",
            symbol
        ))
    })?;

    let required = map
        .get("required")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let default = map.get("default").cloned();

    Ok(EnvBinding {
        key: key.to_string(),
        required,
        default,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use crate::{
        compile_document, compile_document_to_yaml, parse_document, validate_document,
        MapEnvProvider,
    };

    fn env_provider(vars: &[(&str, &str)]) -> MapEnvProvider {
        let mut map = HashMap::new();
        for (k, v) in vars {
            map.insert((*k).to_string(), (*v).to_string());
        }
        MapEnvProvider::new(map)
    }

    #[test]
    fn parses_minimal_document() {
        let input = "---!syaml/v0\n---schema\ntypes: {}\n---data\nname: x\n";
        let parsed = parse_document(input).unwrap();
        assert_eq!(parsed.version, "v0");
        assert!(parsed.front_matter.is_none());
    }

    #[test]
    fn missing_marker_fails() {
        let input = "---schema\ntypes: {}\n---data\nname: x\n";
        let err = parse_document(input).unwrap_err();
        assert!(err.to_string().contains("marker error"));
    }

    #[test]
    fn compiles_with_expressions_and_constraints() {
        let input = r#"
---!syaml/v0
---front_matter
env:
  CPU_CORES:
    from: env
    key: CPU_CORES
    default: 4
---schema
types:
  Port:
    type: integer
    minimum: 1
    maximum: 65535
constraints:
  replicas:
    - "value >= 1"
  max_connections:
    - "value % replicas == 0"
---data
replicas <integer>: 3
worker_threads <integer>: "=max(2, env.CPU_CORES * 2)"
max_connections <integer>: "=replicas * worker_threads * 25"
port <Port>: 5432
"#;

        let compiled = compile_document(input, &env_provider(&[("CPU_CORES", "8")])).unwrap();
        assert_eq!(compiled.value["worker_threads"], json!(16));
        assert_eq!(compiled.value["max_connections"], json!(1200));
    }

    #[test]
    fn compiles_with_type_local_constraints() {
        let input = r#"
---!syaml/v0
---schema
types:
  EpisodeConfig:
    type: object
    required: [initial_population_size, max_agents]
    properties:
      initial_population_size:
        type: integer
        constraints:
          - "value >= 1"
          - "value <= max_agents"
      max_agents:
        type: integer
        constraints: "value >= 1"
    constraints:
      - "initial_population_size <= max_agents"
---data
episode <EpisodeConfig>:
  initial_population_size: 3
  max_agents: 5
"#;

        let compiled = compile_document(input, &env_provider(&[])).unwrap();
        assert_eq!(
            compiled.value["episode"]["initial_population_size"],
            json!(3)
        );
        assert_eq!(compiled.value["episode"]["max_agents"], json!(5));
    }

    #[test]
    fn compiles_with_type_local_constraint_path_map() {
        let input = r#"
---!syaml/v0
---schema
types:
  EpisodeConfig:
    type: object
    required: [initial_population_size, max_agents]
    constraints:
      initial_population_size:
        - "value >= 1"
        - "value <= max_agents"
      max_agents:
        - "value >= 1"
---data
episode <EpisodeConfig>:
  initial_population_size: 3
  max_agents: 5
"#;

        let compiled = compile_document(input, &env_provider(&[])).unwrap();
        assert_eq!(
            compiled.value["episode"]["initial_population_size"],
            json!(3)
        );
        assert_eq!(compiled.value["episode"]["max_agents"], json!(5));
    }

    #[test]
    fn type_local_constraint_failure_is_reported() {
        let input = r#"
---!syaml/v0
---schema
types:
  EpisodeConfig:
    type: object
    required: [initial_population_size, max_agents]
    properties:
      initial_population_size:
        type: integer
      max_agents:
        type: integer
    constraints:
      - "initial_population_size <= max_agents"
---data
episode <EpisodeConfig>:
  initial_population_size: 6
  max_agents: 5
"#;

        let err = compile_document(input, &env_provider(&[])).unwrap_err();
        assert!(err.to_string().contains("constraint failed"));
    }

    #[test]
    fn missing_required_env_errors() {
        let input = r#"
---!syaml/v0
---front_matter
env:
  DB_HOST:
    from: env
    key: DB_HOST
    required: true
---schema
types: {}
---data
host <string>: "${env.DB_HOST}"
"#;

        let err = validate_document(input, &env_provider(&[])).unwrap_err();
        assert!(err
            .to_string()
            .contains("missing required environment variable"));
    }

    #[test]
    fn cycle_detection_errors() {
        let input = r#"
---!syaml/v0
---schema
types: {}
---data
a <integer>: "=b + 1"
b <integer>: "=a + 1"
"#;

        let err = compile_document(input, &env_provider(&[])).unwrap_err();
        assert!(err.to_string().contains("cycle error"));
    }

    #[test]
    fn constraint_failure_errors() {
        let input = r#"
---!syaml/v0
---schema
types: {}
constraints:
  replicas:
    - "value >= 5"
---data
replicas <integer>: 3
"#;

        let err = validate_document(input, &env_provider(&[])).unwrap_err();
        assert!(err.to_string().contains("constraint failed"));
    }

    #[test]
    fn multiple_interpolations_in_one_string_resolve() {
        let input = r#"
---!syaml/v0
---schema
types: {}
---data
a <string>: hello
b <string>: world
msg <string>: "${a} ${b}"
"#;

        let compiled = compile_document(input, &env_provider(&[])).unwrap();
        assert_eq!(compiled.value["msg"], json!("hello world"));
    }

    #[test]
    fn compile_to_yaml_outputs_plain_yaml() {
        let input = r#"
---!syaml/v0
---schema
types: {}
---data
name <string>: super_yaml
count <integer>: 3
"#;

        let yaml = compile_document_to_yaml(input, &env_provider(&[])).unwrap();
        assert!(yaml.contains("name: super_yaml"));
        assert!(yaml.contains("count: 3"));
    }
}
