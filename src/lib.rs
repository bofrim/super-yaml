//! `super_yaml` compiles sectioned `.syaml` documents into resolved JSON or YAML,
//! and can generate first-pass Rust types from `schema.types`.
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
//! workflows, [`compile_document_to_json`] / [`compile_document_to_yaml`] for serialized output,
//! or [`generate_rust_types`] / [`generate_rust_types_from_path`] for Rust code generation.

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
/// Rust type generation from `schema.types`.
pub mod rust_codegen;
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

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value as JsonValue;

use ast::{CompiledDocument, DataDoc, EnvBinding, FrontMatter, ImportBinding, ParsedDocument};
pub use error::SyamlError;
use resolve::{resolve_env_bindings, resolve_expressions};
pub use resolve::{EnvProvider, MapEnvProvider, ProcessEnvProvider};
pub use rust_codegen::{generate_rust_types, generate_rust_types_from_path};
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
    let cwd = std::env::current_dir()?;
    let mut ctx = CompileContext::new(env_provider);
    let compiled = compile_document_internal(input, &cwd, &mut ctx)?;
    Ok(CompiledDocument {
        value: compiled.value,
    })
}

/// Compiles a `.syaml` file into resolved data.
///
/// Import paths are resolved relative to the source file's parent directory.
pub fn compile_document_from_path(
    path: impl AsRef<Path>,
    env_provider: &dyn EnvProvider,
) -> Result<CompiledDocument, SyamlError> {
    let mut ctx = CompileContext::new(env_provider);
    let compiled = compile_document_from_file(path.as_ref(), &mut ctx)?;
    Ok(CompiledDocument {
        value: compiled.value,
    })
}

/// Validates a `.syaml` document without returning compiled output.
///
/// This runs the full compilation pipeline and discards the result.
pub fn validate_document(input: &str, env_provider: &dyn EnvProvider) -> Result<(), SyamlError> {
    compile_document(input, env_provider).map(|_| ())
}

/// Validates a `.syaml` file without returning compiled output.
pub fn validate_document_from_path(
    path: impl AsRef<Path>,
    env_provider: &dyn EnvProvider,
) -> Result<(), SyamlError> {
    compile_document_from_path(path, env_provider).map(|_| ())
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

#[derive(Clone)]
struct CompiledWithTypes {
    value: JsonValue,
    exported_types: BTreeMap<String, JsonValue>,
}

struct CompileContext<'a> {
    env_provider: &'a dyn EnvProvider,
    import_cache: HashMap<PathBuf, CompiledWithTypes>,
    import_stack: Vec<PathBuf>,
}

impl<'a> CompileContext<'a> {
    fn new(env_provider: &'a dyn EnvProvider) -> Self {
        Self {
            env_provider,
            import_cache: HashMap::new(),
            import_stack: Vec::new(),
        }
    }
}

fn compile_document_internal(
    input: &str,
    base_dir: &Path,
    ctx: &mut CompileContext<'_>,
) -> Result<CompiledWithTypes, SyamlError> {
    let parsed = parse_document(input)?;
    compile_parsed_document(parsed, base_dir, ctx)
}

fn compile_document_from_file(
    path: &Path,
    ctx: &mut CompileContext<'_>,
) -> Result<CompiledWithTypes, SyamlError> {
    let canonical_path = canonicalize_path(path)?;

    if let Some(cached) = ctx.import_cache.get(&canonical_path) {
        return Ok(cached.clone());
    }

    if let Some(index) = ctx.import_stack.iter().position(|p| p == &canonical_path) {
        let mut chain: Vec<String> = ctx.import_stack[index..]
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        chain.push(canonical_path.display().to_string());
        return Err(SyamlError::ImportError(format!(
            "cyclic import detected: {}",
            chain.join(" -> ")
        )));
    }

    let input = fs::read_to_string(&canonical_path).map_err(|e| {
        SyamlError::ImportError(format!(
            "failed to read import '{}': {e}",
            canonical_path.display()
        ))
    })?;

    ctx.import_stack.push(canonical_path.clone());
    let base_dir = canonical_path.parent().ok_or_else(|| {
        SyamlError::ImportError(format!(
            "failed to resolve parent directory for '{}'",
            canonical_path.display()
        ))
    })?;
    let compiled = compile_document_internal(&input, base_dir, ctx);
    ctx.import_stack.pop();

    let compiled = compiled?;
    ctx.import_cache
        .insert(canonical_path.clone(), compiled.clone());
    Ok(compiled)
}

fn compile_parsed_document(
    parsed: ParsedDocument,
    base_dir: &Path,
    ctx: &mut CompileContext<'_>,
) -> Result<CompiledWithTypes, SyamlError> {
    let mut schema = parsed.schema;
    let mut data = parsed.data.value.clone();

    if let Some(front_matter) = parsed.front_matter.as_ref() {
        merge_imports(front_matter, base_dir, &mut schema.types, &mut data, ctx)?;
    }

    let env_values = resolve_env_bindings(parsed.front_matter.as_ref(), ctx.env_provider)?;
    resolve_expressions(&mut data, &env_values)?;

    validate_type_hints(&data, &parsed.data.type_hints, &schema)?;
    let constraints = build_effective_constraints(&parsed.data.type_hints, &schema);
    validate_constraints(&data, &env_values, &constraints)?;

    Ok(CompiledWithTypes {
        value: data,
        exported_types: schema.types,
    })
}

fn merge_imports(
    front_matter: &FrontMatter,
    base_dir: &Path,
    type_registry: &mut BTreeMap<String, JsonValue>,
    data: &mut JsonValue,
    ctx: &mut CompileContext<'_>,
) -> Result<(), SyamlError> {
    for (alias, binding) in &front_matter.imports {
        let import_path = resolve_import_path(base_dir, &binding.path)?;
        let imported = compile_document_from_file(&import_path, ctx)?;
        insert_imported_data_namespace(data, alias, imported.value.clone())?;
        insert_imported_types(type_registry, alias, &imported.exported_types)?;
    }
    Ok(())
}

fn resolve_import_path(base_dir: &Path, raw_path: &str) -> Result<PathBuf, SyamlError> {
    let trimmed = raw_path.trim();
    if trimmed.is_empty() {
        return Err(SyamlError::ImportError(
            "import path must be a non-empty string".to_string(),
        ));
    }

    let path = Path::new(trimmed);
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    };
    canonicalize_path(&resolved)
}

fn canonicalize_path(path: &Path) -> Result<PathBuf, SyamlError> {
    fs::canonicalize(path).map_err(|e| {
        SyamlError::ImportError(format!(
            "failed to resolve import path '{}': {e}",
            path.display()
        ))
    })
}

fn insert_imported_data_namespace(
    root_data: &mut JsonValue,
    alias: &str,
    imported_value: JsonValue,
) -> Result<(), SyamlError> {
    let root_object = root_data.as_object_mut().ok_or_else(|| {
        SyamlError::ImportError(format!(
            "data section must be an object when using imports (missing namespace '{alias}')"
        ))
    })?;

    if root_object.contains_key(alias) {
        return Err(SyamlError::ImportError(format!(
            "import namespace '{alias}' conflicts with existing data key"
        )));
    }

    root_object.insert(alias.to_string(), imported_value);
    Ok(())
}

fn insert_imported_types(
    target_types: &mut BTreeMap<String, JsonValue>,
    alias: &str,
    imported_types: &BTreeMap<String, JsonValue>,
) -> Result<(), SyamlError> {
    if imported_types.is_empty() {
        return Ok(());
    }

    let rename_map: BTreeMap<String, String> = imported_types
        .keys()
        .map(|name| (name.clone(), format!("{alias}.{name}")))
        .collect();
    let known_type_names: HashSet<String> = rename_map.keys().cloned().collect();

    for (name, schema) in imported_types {
        let prefixed_name = rename_map.get(name).expect("present");
        if target_types.contains_key(prefixed_name) {
            return Err(SyamlError::ImportError(format!(
                "imported type '{}' conflicts with existing schema type '{}'",
                name, prefixed_name
            )));
        }

        let rewritten = rewrite_schema_type_references(schema, &known_type_names, &rename_map);
        target_types.insert(prefixed_name.clone(), rewritten);
    }

    Ok(())
}

fn rewrite_schema_type_references(
    value: &JsonValue,
    known_type_names: &HashSet<String>,
    rename_map: &BTreeMap<String, String>,
) -> JsonValue {
    match value {
        JsonValue::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, child) in map {
                if key == "type" {
                    if let JsonValue::String(type_name) = child {
                        if known_type_names.contains(type_name)
                            && !is_builtin_primitive_type(type_name)
                        {
                            let renamed = rename_map.get(type_name).expect("present").clone();
                            out.insert(key.clone(), JsonValue::String(renamed));
                            continue;
                        }
                    }
                }
                out.insert(
                    key.clone(),
                    rewrite_schema_type_references(child, known_type_names, rename_map),
                );
            }
            JsonValue::Object(out)
        }
        JsonValue::Array(items) => JsonValue::Array(
            items
                .iter()
                .map(|item| rewrite_schema_type_references(item, known_type_names, rename_map))
                .collect(),
        ),
        _ => value.clone(),
    }
}

fn is_builtin_primitive_type(type_name: &str) -> bool {
    matches!(
        type_name,
        "string" | "integer" | "number" | "boolean" | "object" | "array" | "null"
    )
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
    let mut imports = BTreeMap::new();

    if let Some(env_value) = map.get("env") {
        let env_map = env_value.as_object().ok_or_else(|| {
            SyamlError::SchemaError("front_matter.env must be a mapping/object".to_string())
        })?;

        for (symbol, binding_value) in env_map {
            let binding = parse_env_binding(symbol, binding_value)?;
            env.insert(symbol.clone(), binding);
        }
    }

    if let Some(imports_value) = map.get("imports") {
        let imports_map = imports_value.as_object().ok_or_else(|| {
            SyamlError::SchemaError("front_matter.imports must be a mapping/object".to_string())
        })?;
        for (alias, import_value) in imports_map {
            let binding = parse_import_binding(alias, import_value)?;
            imports.insert(alias.clone(), binding);
        }
    }

    Ok(FrontMatter { env, imports })
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

fn parse_import_binding(alias: &str, value: &JsonValue) -> Result<ImportBinding, SyamlError> {
    if !is_valid_namespace_segment(alias) {
        return Err(SyamlError::SchemaError(format!(
            "front_matter.imports.{} has invalid namespace alias; expected [A-Za-z_][A-Za-z0-9_]*",
            alias
        )));
    }

    let path = match value {
        JsonValue::String(path) => path.clone(),
        JsonValue::Object(map) => map
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                SyamlError::SchemaError(format!(
                    "front_matter.imports.{} must define string path",
                    alias
                ))
            })?
            .to_string(),
        _ => {
            return Err(SyamlError::SchemaError(format!(
                "front_matter.imports.{} must be string or mapping/object",
                alias
            )))
        }
    };

    if path.trim().is_empty() {
        return Err(SyamlError::SchemaError(format!(
            "front_matter.imports.{} path must be non-empty",
            alias
        )));
    }

    Ok(ImportBinding { path })
}

fn is_valid_namespace_segment(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
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
