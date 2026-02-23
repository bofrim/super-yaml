//! `super_yaml` compiles sectioned `.syaml` documents into resolved JSON or YAML,
//! and can generate first-pass Rust types from named schema definitions.
//!
//! A document goes through these stages:
//! 1. Section scanning and shape validation (`---!syaml/v0`, section order, required sections).
//! 2. Parsing section bodies with the built-in YAML subset parser.
//! 3. Schema extraction and type-hint normalization.
//! 4. Explicit import-value extraction.
//! 5. Template expansion.
//! 6. Environment binding resolution.
//! 7. Derived expression/interpolation resolution.
//! 8. String constructor coercion for hinted object types.
//! 9. Type-hint and constraint validation.
//!
//! Use [`compile_document`] for full compilation, [`validate_document`] for validation-only
//! workflows, [`compile_document_to_json`] / [`compile_document_to_yaml`] for serialized output,
//! or [`generate_rust_types`] / [`generate_rust_types_from_path`] and
//! [`generate_typescript_types`] / [`generate_typescript_types_from_path`] for code generation.

/// Abstract syntax tree and compiled document container types.
pub mod ast;
/// Regex-based string constructor coercion for hinted object types.
pub mod coerce;
/// Error types used throughout parsing, compilation, and validation.
pub mod error;
/// URL-based import fetching, disk caching, and lockfile management.
pub mod fetch;
/// Parsing and validation for the `---functional` section.
pub mod functional;
/// Expression lexer/parser/evaluator used by derived values and constraints.
pub mod expr;
/// Minimal YAML subset parser used for section bodies.
pub mod mini_yaml;
/// Environment and expression resolution over parsed data.
pub mod resolve;
/// Rust type generation from named schema definitions.
pub mod rust_codegen;
/// Schema parsing and schema-based validation helpers.
pub mod schema;
/// Top-level section marker scanner and order validator.
pub mod section_scanner;
/// Data-template expansion (`{{template.path}}` keys + `{{VAR}}` placeholders).
pub mod template;
/// Type-hint extraction (`key <Type>`) and normalization.
pub mod type_hints;
/// TypeScript type generation from named schema definitions.
pub mod typescript_codegen;
/// Proto3 file generation from named schema definitions.
pub mod proto_codegen;
/// JSON Schema to super_yaml schema conversion.
pub mod json_schema_import;
/// super_yaml schema to JSON Schema export.
pub mod json_schema_export;
/// Constraint and type-hint validation routines.
pub mod validate;
/// Import integrity verification: hash, signature, and version checks.
pub mod verify;
/// JSON-to-YAML renderer used by compiled YAML output.
pub mod yaml_writer;
/// Module manifest parsing, discovery, and import policy enforcement.
pub mod module;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value as JsonValue;

use ast::{
    CompiledDocument, DataDoc, EnvBinding, ImportBinding, Meta, ModuleManifest, ParsedDocument,
    SignatureBinding,
};
use coerce::coerce_string_constructors_for_type_hints;
pub use error::SyamlError;
use fetch::FetchContext;
use resolve::{resolve_env_bindings, resolve_expressions_with_imports};
pub use resolve::{EnvProvider, MapEnvProvider, ProcessEnvProvider};
pub use rust_codegen::{
    generate_rust_types, generate_rust_types_and_data_from_path, generate_rust_types_from_path,
};
use schema::{parse_schema, validate_schema_type_references, validate_strict_field_numbers};
use section_scanner::scan_sections;
use template::expand_data_templates;
use type_hints::normalize_data_with_hints;
pub use typescript_codegen::{
    generate_typescript_types, generate_typescript_types_and_data_from_path,
    generate_typescript_types_from_path,
};
pub use proto_codegen::{generate_proto_types, generate_proto_types_from_path};
pub use json_schema_import::{from_json_schema, from_json_schema_path};
pub use json_schema_export::to_json_schema;
use validate::{
    build_effective_constraints, validate_constraints_with_imports, validate_type_hints,
    validate_versioned_fields,
};

/// Parses a `.syaml` document into its structured representation.
///
/// This performs marker and section validation, section body parsing, schema parsing,
/// and data type-hint normalization. It does not resolve expressions or environment
/// bindings and does not run constraints.
pub fn parse_document(input: &str) -> Result<ParsedDocument, SyamlError> {
    let (version, sections) = scan_sections(input)?;

    let mut meta: Option<Meta> = None;
    let mut schema = parse_schema(&JsonValue::Object(serde_json::Map::new()))?;
    let mut data = DataDoc {
        value: JsonValue::Object(serde_json::Map::new()),
        type_hints: BTreeMap::new(),
        freeze_markers: BTreeMap::new(),
    };
    let mut functional: Option<crate::ast::FunctionalDoc> = None;

    for section in sections {
        let section_value = parse_section_value(&section.name, &section.body)?;
        match section.name.as_str() {
            "meta" => {
                meta = Some(parse_meta(&section_value)?);
            }
            "schema" => {
                schema = parse_schema(&section_value)?;
            }
            "data" => {
                let (value, type_hints, freeze_markers) = normalize_data_with_hints(&section_value)?;
                data = DataDoc { value, type_hints, freeze_markers };
            }
            "functional" => {
                functional = Some(functional::parse_functional(&section_value)?);
            }
            "module" => {
                // Module sections are only valid in module.syaml; handled by module::parse_module_manifest.
                // Reject them in regular document parsing.
                return Err(SyamlError::SectionError(
                    "'---module' section is only allowed in module.syaml manifest files".to_string(),
                ));
            }
            _ => unreachable!("validated by section scanner"),
        }
    }

    Ok(ParsedDocument {
        version,
        meta,
        schema,
        data,
        functional,
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
        warnings: compiled.warnings,
    })
}

/// Compiles a `.syaml` file into resolved data.
///
/// Import paths are resolved relative to the source file's parent directory.
/// URL imports are cached to disk and recorded in a lockfile alongside the source file.
pub fn compile_document_from_path(
    path: impl AsRef<Path>,
    env_provider: &dyn EnvProvider,
) -> Result<CompiledDocument, SyamlError> {
    compile_document_from_path_with_fetch(path, env_provider, None, false)
}

/// Compiles a `.syaml` file with explicit fetch options for URL imports.
///
/// - `cache_dir`: overrides the default `$SYAML_CACHE_DIR` / `~/.cache/super_yaml/` location.
/// - `force_update`: when `true`, bypasses the lockfile cache and re-fetches all URL imports.
pub fn compile_document_from_path_with_fetch(
    path: impl AsRef<Path>,
    env_provider: &dyn EnvProvider,
    cache_dir: Option<PathBuf>,
    force_update: bool,
) -> Result<CompiledDocument, SyamlError> {
    let path = path.as_ref();
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let root_dir = canonical
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let mut ctx = CompileContext {
        env_provider,
        import_cache: HashMap::new(),
        import_stack: Vec::new(),
        fetch_ctx: FetchContext::new(&root_dir, cache_dir, force_update),
    };
    let compiled = compile_document_from_file(path, &mut ctx)?;
    fetch::flush_lockfile(&ctx.fetch_ctx)?;
    Ok(CompiledDocument {
        value: compiled.value,
        warnings: compiled.warnings,
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

/// Parses a `.syaml` document and returns a JSON Schema document derived from
/// its `schema` section.
///
/// The `env_provider` parameter is accepted for API consistency but is not used,
/// since schema export does not require expression or environment resolution.
/// Set `pretty` to `true` for indented output.
pub fn compile_document_to_json_schema(
    input: &str,
    _env_provider: &dyn EnvProvider,
    pretty: bool,
) -> Result<String, SyamlError> {
    let parsed = parse_document(input)?;
    json_schema_export::to_json_schema(&parsed.schema, pretty)
}

#[derive(Clone)]
struct CompiledWithTypes {
    value: JsonValue,
    exported_types: BTreeMap<String, JsonValue>,
    warnings: Vec<String>,
}

struct CompileContext<'a> {
    env_provider: &'a dyn EnvProvider,
    import_cache: HashMap<PathBuf, CompiledWithTypes>,
    import_stack: Vec<PathBuf>,
    fetch_ctx: FetchContext,
}

impl<'a> CompileContext<'a> {
    fn new(env_provider: &'a dyn EnvProvider) -> Self {
        Self {
            env_provider,
            import_cache: HashMap::new(),
            import_stack: Vec::new(),
            fetch_ctx: FetchContext::disabled(),
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
    let input = fs::read_to_string(&canonical_path).map_err(|e| {
        SyamlError::ImportError(format!(
            "failed to read import '{}': {e}",
            canonical_path.display()
        ))
    })?;
    compile_document_from_content(&input, &canonical_path, ctx)
}

fn compile_document_from_content(
    input: &str,
    canonical_path: &Path,
    ctx: &mut CompileContext<'_>,
) -> Result<CompiledWithTypes, SyamlError> {
    if let Some(cached) = ctx.import_cache.get(canonical_path) {
        return Ok(cached.clone());
    }

    if let Some(index) = ctx.import_stack.iter().position(|p| p == canonical_path) {
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

    ctx.import_stack.push(canonical_path.to_path_buf());
    let base_dir = canonical_path.parent().ok_or_else(|| {
        SyamlError::ImportError(format!(
            "failed to resolve parent directory for '{}'",
            canonical_path.display()
        ))
    })?;

    let is_manifest = canonical_path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n == module::MANIFEST_FILENAME)
        .unwrap_or(false);

    let compiled = if is_manifest {
        // Compile a module manifest: validate structure, export schema types if present.
        compile_module_manifest(input, base_dir, ctx)
    } else {
        // Regular document: parse, apply module context, then compile.
        compile_document_with_module_context(input, canonical_path, base_dir, ctx)
    };

    ctx.import_stack.pop();

    let compiled = compiled?;
    ctx.import_cache
        .insert(canonical_path.to_path_buf(), compiled.clone());
    Ok(compiled)
}

/// Compiles a `module.syaml` manifest: validates structure, returns schema types but empty data.
fn compile_module_manifest(
    input: &str,
    base_dir: &Path,
    ctx: &mut CompileContext<'_>,
) -> Result<CompiledWithTypes, SyamlError> {
    let manifest = module::parse_module_manifest(input)?;

    // Parse schema section if present (for shared types)
    let (_, sections) = section_scanner::scan_sections(input)?;
    let schema_sec = sections.iter().find(|s| s.name == "schema");

    let mut exported_types = BTreeMap::new();
    if let Some(sec) = schema_sec {
        let schema_val = parse_section_value("schema", &sec.body)?;
        let schema_doc = parse_schema(&schema_val)?;
        // Resolve imports declared in the manifest's meta (for schema cross-refs)
        if !manifest.imports.is_empty() {
            let manifest_meta = Meta {
                file: BTreeMap::new(),
                env: BTreeMap::new(),
                imports: manifest.imports.clone(),
            };
            let mut dummy_types = schema_doc.types.clone();
            let mut dummy_data = HashMap::new();
            merge_imports(&manifest_meta, base_dir, &mut dummy_types, &mut dummy_data, ctx)?;
            exported_types = dummy_types;
        } else {
            exported_types = schema_doc.types;
        }
    }

    Ok(CompiledWithTypes {
        value: JsonValue::Object(serde_json::Map::new()),
        exported_types,
        warnings: Vec::new(),
    })
}

/// Parses a regular (non-manifest) document, applies module context, then compiles it.
fn compile_document_with_module_context(
    input: &str,
    canonical_path: &Path,
    base_dir: &Path,
    ctx: &mut CompileContext<'_>,
) -> Result<CompiledWithTypes, SyamlError> {
    let mut parsed = parse_document(input)?;

    // Step 1: find module manifest (if any) and enforce policy on file's own imports
    let manifest_result = find_and_load_module_manifest(canonical_path)?;

    if let Some((ref manifest, ref manifest_path)) = manifest_result {
        // Enforce policy on the file's declared imports BEFORE injecting module imports
        if let Some(ref meta) = parsed.meta {
            module::enforce_import_policy(
                &meta.imports,
                &manifest.import_policy,
                &canonical_path.display().to_string(),
            )?;
        }
        // Inject module metadata and imports, but skip any that would import this file itself.
        let manifest_dir = manifest_path.parent().unwrap_or(Path::new("."));
        let filtered_manifest =
            filter_self_referential_imports(manifest, manifest_dir, canonical_path);
        module::apply_module_to_meta(&mut parsed.meta, &filtered_manifest);
    }

    // Step 2: resolve @module import paths to real filesystem paths
    resolve_module_imports_in_meta(&mut parsed.meta, canonical_path)?;

    compile_parsed_document(parsed, base_dir, ctx)
}

/// Returns a copy of `manifest` with any imports that resolve to `self_path` removed.
/// This prevents module-injected imports from creating self-import cycles.
fn filter_self_referential_imports(
    manifest: &ModuleManifest,
    manifest_dir: &Path,
    self_path: &Path,
) -> ModuleManifest {
    let mut filtered = manifest.clone();
    filtered.imports.retain(|_alias, binding| {
        if binding.path.starts_with('@') || is_url(&binding.path) {
            return true; // can't cheaply resolve these here; keep them
        }
        let resolved = manifest_dir.join(&binding.path);
        // Canonicalize for comparison if possible
        let resolved_canonical = fs::canonicalize(&resolved).unwrap_or(resolved);
        resolved_canonical != self_path
    });
    filtered
}

fn is_url(path: &str) -> bool {
    path.starts_with("http://") || path.starts_with("https://")
}

/// Walks up from `file_path` to find and parse the nearest `module.syaml`.
/// Returns `(manifest, manifest_path)` or `None`.
fn find_and_load_module_manifest(
    file_path: &Path,
) -> Result<Option<(ModuleManifest, PathBuf)>, SyamlError> {
    let Some(manifest_path) = module::find_module_manifest(file_path) else {
        return Ok(None);
    };
    let content = fs::read_to_string(&manifest_path).map_err(|e| {
        SyamlError::ModuleManifestError(format!(
            "failed to read module manifest '{}': {e}",
            manifest_path.display()
        ))
    })?;
    let manifest = module::parse_module_manifest(&content)?;
    Ok(Some((manifest, manifest_path)))
}

/// Resolves any `@module` or `@module/file` import paths in meta to real filesystem paths.
fn resolve_module_imports_in_meta(
    meta: &mut Option<Meta>,
    file_path: &Path,
) -> Result<(), SyamlError> {
    let Some(ref mut meta) = meta else {
        return Ok(());
    };

    let has_module_imports = meta.imports.values().any(|b| b.path.starts_with('@'));
    if !has_module_imports {
        return Ok(());
    }

    let start = file_path.parent().unwrap_or(file_path);
    let project_root = module::find_project_root(start).ok_or(SyamlError::NoProjectRegistry)?;
    let registry = module::load_module_registry(&project_root)?;

    for binding in meta.imports.values_mut() {
        if binding.path.starts_with('@') {
            let resolved = module::resolve_module_import(&binding.path, &registry)?;
            binding.path = resolved.display().to_string();
        }
    }

    Ok(())
}

fn compile_parsed_document(
    parsed: ParsedDocument,
    base_dir: &Path,
    ctx: &mut CompileContext<'_>,
) -> Result<CompiledWithTypes, SyamlError> {
    let mut schema = parsed.schema;
    let mut data = parsed.data.value.clone();
    let mut imported_data = HashMap::new();

    if let Some(meta) = parsed.meta.as_ref() {
        merge_imports(meta, base_dir, &mut schema.types, &mut imported_data, ctx)?;
    }
    validate_schema_type_references(&schema.types)?;

    let strict_field_numbers = parsed
        .meta
        .as_ref()
        .and_then(|m| m.file.get("strict_field_numbers"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if strict_field_numbers {
        validate_strict_field_numbers(&schema.types)?;
    }

    let target_schema_version: Option<semver::Version> = parsed
        .meta
        .as_ref()
        .and_then(|m| m.file.get("schema_version"))
        .and_then(|v| v.as_str())
        .map(|s| semver::Version::parse(s))
        .transpose()
        .map_err(|e| SyamlError::VersionError(format!("invalid meta.file.schema_version: {e}")))?;

    extract_explicit_import_values(&mut data, &imported_data)?;
    expand_data_templates(&mut data, &imported_data)?;

    let env_values = resolve_env_bindings(parsed.meta.as_ref(), ctx.env_provider)?;
    let imports_for_eval: BTreeMap<String, JsonValue> = imported_data
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    resolve_expressions_with_imports(&mut data, &env_values, &imports_for_eval)?;
    coerce_string_constructors_for_type_hints(&mut data, &parsed.data.type_hints, &schema.types)?;

    validate_type_hints(&data, &parsed.data.type_hints, &schema)?;
    let constraints = build_effective_constraints(&parsed.data.type_hints, &schema);
    validate_constraints_with_imports(&data, &env_values, &constraints, &imports_for_eval)?;

    let warnings =
        validate_versioned_fields(&data, &parsed.data.type_hints, &schema, target_schema_version.as_ref())?;

    if let Some(ref func_doc) = parsed.functional {
        let import_aliases: std::collections::BTreeSet<String> = parsed.meta.iter()
            .flat_map(|m| m.imports.keys().cloned())
            .collect();
        let all_types = schema.types.clone();
        functional::validate_functional_type_references(func_doc, &all_types)?;
        functional::validate_permission_data_paths(func_doc, &data, &import_aliases)?;
        functional::validate_permission_mutability_alignment(func_doc, &schema, &parsed.data.type_hints)?;
        functional::validate_permission_instance_lock_conflicts(func_doc, &parsed.data.freeze_markers)?;
        functional::validate_specification_strict_conditions(func_doc)?;
    }

    strip_private_top_level_data_keys(&mut data);

    Ok(CompiledWithTypes {
        value: data,
        exported_types: schema.types,
        warnings,
    })
}

fn strip_private_top_level_data_keys(data: &mut JsonValue) {
    let Some(root) = data.as_object_mut() else {
        return;
    };
    root.retain(|key, _| !is_private_top_level_key(key));
}

fn is_private_top_level_key(key: &str) -> bool {
    key.starts_with('_')
}

fn merge_imports(
    meta: &Meta,
    base_dir: &Path,
    type_registry: &mut BTreeMap<String, JsonValue>,
    imported_data: &mut HashMap<String, JsonValue>,
    ctx: &mut CompileContext<'_>,
) -> Result<(), SyamlError> {
    for (alias, binding) in &meta.imports {
        let source =
            fetch::resolve_import_source(base_dir, &binding.path, &ctx.fetch_ctx)?;
        let display_id = source.display_id();

        let content = fetch::read_import_source(&source, &mut ctx.fetch_ctx).map_err(|e| {
            SyamlError::ImportError(format!(
                "failed to read import '{}' for namespace '{}': {e}",
                display_id, alias
            ))
        })?;

        if let Some(ref expected_hash) = binding.hash {
            verify::verify_hash(content.as_bytes(), expected_hash).map_err(|e| {
                SyamlError::HashError(format!(
                    "import '{}' for namespace '{}': {e}",
                    display_id, alias
                ))
            })?;
        }

        if let Some(ref sig) = binding.signature {
            verify::verify_signature(content.as_bytes(), sig, base_dir).map_err(|e| {
                SyamlError::SignatureError(format!(
                    "import '{}' for namespace '{}': {e}",
                    display_id, alias
                ))
            })?;
        }

        let canonical = source.canonical_path().to_path_buf();
        let imported =
            compile_document_from_content(&content, &canonical, ctx).map_err(|e| {
                SyamlError::ImportError(format!(
                    "failed to compile import '{}' for namespace '{}': {e}",
                    display_id, alias
                ))
            })?;

        if let Some(ref version_req) = binding.version {
            let imported_parsed = parse_document(&content)?;
            let imported_version = imported_parsed
                .meta
                .as_ref()
                .and_then(|m| m.file.get("version"))
                .and_then(|v| v.as_str());

            verify::verify_version(imported_parsed.meta.as_ref(), version_req).map_err(|e| {
                SyamlError::VersionError(format!(
                    "import '{}' for namespace '{}': {e}",
                    display_id, alias
                ))
            })?;

            fetch::update_lockfile_version(&source, imported_version, &mut ctx.fetch_ctx);
        }

        imported_data.insert(alias.clone(), imported.value.clone());
        insert_imported_types(type_registry, alias, &imported.exported_types)?;
    }
    Ok(())
}

fn canonicalize_path(path: &Path) -> Result<PathBuf, SyamlError> {
    fs::canonicalize(path).map_err(|e| {
        SyamlError::ImportError(format!(
            "failed to resolve import path '{}': {e}",
            path.display()
        ))
    })
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

fn extract_explicit_import_values(
    data: &mut JsonValue,
    imports: &HashMap<String, JsonValue>,
) -> Result<(), SyamlError> {
    let root = data.as_object().ok_or_else(|| {
        SyamlError::ImportError(
            "data section must be a mapping/object when using imports".to_string(),
        )
    })?;
    if root.is_empty() || imports.is_empty() {
        return Ok(());
    }
    extract_import_values_inner(data, imports);
    Ok(())
}

fn extract_import_values_inner(value: &mut JsonValue, imports: &HashMap<String, JsonValue>) {
    match value {
        JsonValue::Object(map) => {
            for child in map.values_mut() {
                extract_import_values_inner(child, imports);
            }
        }
        JsonValue::Array(items) => {
            for item in items.iter_mut() {
                extract_import_values_inner(item, imports);
            }
        }
        JsonValue::String(raw) => {
            if let Some(resolved) = try_resolve_import_reference(raw, imports) {
                *value = resolved;
            }
        }
        _ => {}
    }
}

fn try_resolve_import_reference(
    raw: &str,
    imports: &HashMap<String, JsonValue>,
) -> Option<JsonValue> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut segments = trimmed.split('.');
    let first = segments.next()?;
    if !is_valid_namespace_segment(first) {
        return None;
    }
    let mut current = imports.get(first)?;
    for segment in segments {
        if !is_valid_namespace_segment(segment) {
            return None;
        }
        current = current.as_object()?.get(segment)?;
    }
    Some(current.clone())
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

fn parse_meta(value: &JsonValue) -> Result<Meta, SyamlError> {
    let map = value.as_object().ok_or_else(|| {
        SyamlError::SchemaError("meta section must be a mapping/object".to_string())
    })?;

    let file = match map.get("file") {
        Some(file_value) => {
            let file_map = file_value.as_object().ok_or_else(|| {
                SyamlError::SchemaError("meta.file must be a mapping/object".to_string())
            })?;
            file_map
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        }
        None => BTreeMap::new(),
    };

    let mut env = BTreeMap::new();
    let mut imports = BTreeMap::new();

    if let Some(env_value) = map.get("env") {
        let env_map = env_value.as_object().ok_or_else(|| {
            SyamlError::SchemaError("meta.env must be a mapping/object".to_string())
        })?;

        for (symbol, binding_value) in env_map {
            let binding = parse_env_binding(symbol, binding_value)?;
            env.insert(symbol.clone(), binding);
        }
    }

    if let Some(imports_value) = map.get("imports") {
        let imports_map = imports_value.as_object().ok_or_else(|| {
            SyamlError::SchemaError("meta.imports must be a mapping/object".to_string())
        })?;
        for (alias, import_value) in imports_map {
            let binding = parse_import_binding(alias, import_value)?;
            imports.insert(alias.clone(), binding);
        }
    }

    Ok(Meta { file, env, imports })
}

fn parse_env_binding(symbol: &str, value: &JsonValue) -> Result<EnvBinding, SyamlError> {
    let map = value.as_object().ok_or_else(|| {
        SyamlError::SchemaError(format!("meta.env.{} must be a mapping/object", symbol))
    })?;

    let from = map.get("from").and_then(|v| v.as_str()).unwrap_or("env");

    if from != "env" {
        return Err(SyamlError::SchemaError(format!(
            "meta.env.{} has unsupported from='{}'; only 'env' is supported",
            symbol, from
        )));
    }

    let key = map.get("key").and_then(|v| v.as_str()).ok_or_else(|| {
        SyamlError::SchemaError(format!("meta.env.{} must define string key", symbol))
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
            "meta.imports.{} has invalid namespace alias; expected [A-Za-z_][A-Za-z0-9_]*",
            alias
        )));
    }

    let (path, hash, signature, version) = match value {
        JsonValue::String(path) => (path.clone(), None, None, None),
        JsonValue::Object(map) => {
            let path = map
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    SyamlError::SchemaError(format!(
                        "meta.imports.{} must define string path",
                        alias
                    ))
                })?
                .to_string();

            let hash = map.get("hash").and_then(|v| v.as_str()).map(String::from);

            let signature = if let Some(sig_val) = map.get("signature") {
                let sig_map = sig_val.as_object().ok_or_else(|| {
                    SyamlError::SchemaError(format!(
                        "meta.imports.{}.signature must be a mapping/object",
                        alias
                    ))
                })?;
                let public_key = sig_map
                    .get("public_key")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        SyamlError::SchemaError(format!(
                            "meta.imports.{}.signature must define string public_key",
                            alias
                        ))
                    })?
                    .to_string();
                let sig_value = sig_map
                    .get("value")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        SyamlError::SchemaError(format!(
                            "meta.imports.{}.signature must define string value",
                            alias
                        ))
                    })?
                    .to_string();
                Some(SignatureBinding {
                    public_key,
                    value: sig_value,
                })
            } else {
                None
            };

            let version = map
                .get("version")
                .and_then(|v| v.as_str())
                .map(String::from);

            (path, hash, signature, version)
        }
        _ => {
            return Err(SyamlError::SchemaError(format!(
                "meta.imports.{} must be string or mapping/object",
                alias
            )))
        }
    };

    if path.trim().is_empty() {
        return Err(SyamlError::SchemaError(format!(
            "meta.imports.{} path must be non-empty",
            alias
        )));
    }

    Ok(ImportBinding {
        path,
        hash,
        signature,
        version,
    })
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
        let input = "---!syaml/v0\n---schema\n{}\n---data\nname: x\n";
        let parsed = parse_document(input).unwrap();
        assert_eq!(parsed.version, "v0");
        assert!(parsed.meta.is_none());
    }

    #[test]
    fn missing_marker_fails() {
        let input = "---schema\n{}\n---data\nname: x\n";
        let err = parse_document(input).unwrap_err();
        assert!(err.to_string().contains("marker error"));
    }

    #[test]
    fn compiles_with_expressions_and_constraints() {
        let input = r#"
---!syaml/v0
---meta
env:
  CPU_CORES:
    from: env
    key: CPU_CORES
    default: 4
---schema
Port:
  type: integer
  minimum: 1
  maximum: 65535
ReplicaCount:
  type: integer
  constraints: "value >= 1"
MaxConnections:
  type: integer
  constraints: "value >= 1"
---data
replicas <ReplicaCount>: 3
worker_threads <integer>: "=max(2, env.CPU_CORES * 2)"
max_connections <MaxConnections>: "=replicas * worker_threads * 25"
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
SessionConfig:
  type: object
  properties:
    min_attendees:
      type: integer
      constraints:
        - "value >= 1"
        - "value <= 1000000"
    max_attendees:
      type: integer
      constraints: "value >= 1"
  constraints:
    - "min_attendees <= max_attendees"
---data
session <SessionConfig>:
  min_attendees: 3
  max_attendees: 5
"#;

        let compiled = compile_document(input, &env_provider(&[])).unwrap();
        assert_eq!(
            compiled.value["session"]["min_attendees"],
            json!(3)
        );
        assert_eq!(compiled.value["session"]["max_attendees"], json!(5));
    }

    #[test]
    fn compiles_with_type_local_constraint_path_map() {
        let input = r#"
---!syaml/v0
---schema
SessionConfig:
  type: object
  properties:
    min_attendees:
      type: integer
    max_attendees:
      type: integer
  constraints:
    min_attendees:
      - "value >= 1"
      - "value <= 1000000"
    max_attendees:
      - "value >= 1"
    $:
      - "min_attendees <= max_attendees"
---data
session <SessionConfig>:
  min_attendees: 3
  max_attendees: 5
"#;

        let compiled = compile_document(input, &env_provider(&[])).unwrap();
        assert_eq!(
            compiled.value["session"]["min_attendees"],
            json!(3)
        );
        assert_eq!(compiled.value["session"]["max_attendees"], json!(5));
    }

    #[test]
    fn type_local_constraint_failure_is_reported() {
        let input = r#"
---!syaml/v0
---schema
SessionConfig:
  type: object
  properties:
    min_attendees:
      type: integer
    max_attendees:
      type: integer
  constraints:
    - "min_attendees <= max_attendees"
---data
session <SessionConfig>:
  min_attendees: 6
  max_attendees: 5
"#;

        let err = compile_document(input, &env_provider(&[])).unwrap_err();
        assert!(err.to_string().contains("constraint failed"));
    }

    #[test]
    fn missing_required_env_errors() {
        let input = r#"
---!syaml/v0
---meta
env:
  DB_HOST:
    from: env
    key: DB_HOST
    required: true
---schema
{}
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
{}
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
MinReplicas:
  type: integer
  constraints: "value >= 5"
---data
replicas <MinReplicas>: 3
"#;

        let err = validate_document(input, &env_provider(&[])).unwrap_err();
        assert!(err.to_string().contains("constraint failed"));
    }

    #[test]
    fn multiple_interpolations_in_one_string_resolve() {
        let input = r#"
---!syaml/v0
---schema
{}
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
{}
---data
name <string>: super_yaml
count <integer>: 3
"#;

        let yaml = compile_document_to_yaml(input, &env_provider(&[])).unwrap();
        assert!(yaml.contains("name: super_yaml"));
        assert!(yaml.contains("count: 3"));
    }
}
