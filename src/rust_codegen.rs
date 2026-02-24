//! Rust type generation from named schema definitions.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::ast::{ImportBinding, ParsedDocument};
use crate::schema::parse_field_version_meta;
use crate::{parse_document, parse_document_or_manifest, SyamlError};

const RUST_KEYWORDS: &[&str] = &[
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for",
    "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
    "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where",
    "while", "async", "await", "dyn", "abstract", "become", "box", "do", "final", "macro",
    "override", "priv", "try", "typeof", "unsized", "virtual", "yield",
];

/// Generates Rust types from an in-memory `.syaml` document.
///
/// Imported schemas are resolved relative to the current working directory.
pub fn generate_rust_types(input: &str) -> Result<String, SyamlError> {
    let cwd = std::env::current_dir()?;
    let parsed = parse_document(input)?;

    let mut ctx = TypeCollectionContext::new();
    let types = collect_exported_types(parsed, &cwd, &mut ctx)?;
    Ok(render_rust_types(&types))
}

/// Generates Rust types from a `.syaml` file path.
///
/// Imported schemas are resolved relative to each file's parent directory.
pub fn generate_rust_types_from_path(path: impl AsRef<Path>) -> Result<String, SyamlError> {
    let mut ctx = TypeCollectionContext::new();
    let types = collect_types_from_file(path.as_ref(), &mut ctx)?;
    Ok(render_rust_types(&types))
}

#[derive(Default)]
struct TypeCollectionContext {
    cache: HashMap<PathBuf, CollectedSchemas>,
    stack: Vec<PathBuf>,
}

#[derive(Clone, Default)]
struct CollectedSchemas {
    types: BTreeMap<String, JsonValue>,
    type_constraints: BTreeMap<String, BTreeMap<String, Vec<String>>>,
}

impl TypeCollectionContext {
    fn new() -> Self {
        Self {
            cache: HashMap::new(),
            stack: Vec::new(),
        }
    }
}

fn collect_types_from_file(
    path: &Path,
    ctx: &mut TypeCollectionContext,
) -> Result<CollectedSchemas, SyamlError> {
    let canonical_path = canonicalize_path(path)?;

    if let Some(cached) = ctx.cache.get(&canonical_path) {
        return Ok(cached.clone());
    }

    if let Some(index) = ctx.stack.iter().position(|p| p == &canonical_path) {
        let mut chain: Vec<String> = ctx.stack[index..]
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

    ctx.stack.push(canonical_path.clone());
    let base_dir = canonical_path.parent().ok_or_else(|| {
        SyamlError::ImportError(format!(
            "failed to resolve parent directory for '{}'",
            canonical_path.display()
        ))
    })?;

    let parsed = parse_document_or_manifest(&input)?;
    let exported = collect_exported_types(parsed, base_dir, ctx);

    ctx.stack.pop();

    let exported = exported?;
    ctx.cache.insert(canonical_path, exported.clone());
    Ok(exported)
}

fn collect_exported_types(
    parsed: ParsedDocument,
    base_dir: &Path,
    ctx: &mut TypeCollectionContext,
) -> Result<CollectedSchemas, SyamlError> {
    let mut exported = CollectedSchemas {
        types: parsed.schema.types,
        type_constraints: parsed.schema.type_constraints,
    };

    if let Some(meta) = parsed.meta {
        for (alias, binding) in meta.imports {
            let import_path = resolve_import_path(base_dir, &binding)?;
            let imported = collect_types_from_file(&import_path, ctx).map_err(|e| {
                SyamlError::ImportError(format!(
                    "failed to compile import '{}' for namespace '{}': {e}",
                    import_path.display(),
                    alias
                ))
            })?;
            insert_imported_types(&mut exported, &alias, &imported)?;
        }
    }

    Ok(exported)
}

fn resolve_import_path(base_dir: &Path, binding: &ImportBinding) -> Result<PathBuf, SyamlError> {
    let trimmed = binding.path.trim();
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

fn insert_imported_types(
    registry: &mut CollectedSchemas,
    alias: &str,
    imported: &CollectedSchemas,
) -> Result<(), SyamlError> {
    for (type_name, schema) in &imported.types {
        let namespaced = format!("{alias}.{type_name}");
        if registry.types.contains_key(&namespaced) {
            return Err(SyamlError::ImportError(format!(
                "imported type '{}' conflicts with existing type",
                namespaced
            )));
        }
        registry.types.insert(namespaced, schema.clone());
    }

    for (type_name, constraints) in &imported.type_constraints {
        let namespaced = format!("{alias}.{type_name}");
        if registry.type_constraints.contains_key(&namespaced) {
            return Err(SyamlError::ImportError(format!(
                "imported constraints for type '{}' conflict with existing type",
                namespaced
            )));
        }
        registry
            .type_constraints
            .insert(namespaced, constraints.clone());
    }
    Ok(())
}

struct RenderState {
    type_names: BTreeMap<String, String>,
    all_types: BTreeMap<String, JsonValue>,
    type_constraints: BTreeMap<String, BTreeMap<String, Vec<String>>>,
    needs_serde_derives: bool,
    needs_serde_json_value: bool,
    needs_constraint_runtime: bool,
}

fn render_rust_types(schemas: &CollectedSchemas) -> String {
    let expanded = expand_inline_enum_schemas(schemas);
    render_rust_types_expanded(&expanded)
}

/// Promotes inline string-enum property schemas to standalone named enum types so that
/// struct fields use a proper Rust enum instead of `String`.
///
/// For a property like `env: [dev, staging, prod]` inside `ServiceConfig`, this generates
/// a new type `ServiceConfigEnv: {enum: ["dev","staging","prod"]}` and rewrites the
/// property to `env: {type: "ServiceConfigEnv"}`.
fn expand_inline_enum_schemas(schemas: &CollectedSchemas) -> CollectedSchemas {
    // Build the set of Rust identifiers already in use to avoid name collisions.
    let mut used_rust_names: HashSet<String> = schemas
        .types
        .keys()
        .map(|k| sanitize_type_name(k, "Type"))
        .collect();
    // Track source-name keys we will insert (same PascalCase as the Rust name for
    // auto-generated types).
    let mut used_source_names: HashSet<String> = schemas.types.keys().cloned().collect();

    struct Promotion {
        parent_source: String,
        field_name: String,
        generated_source: String,
        enum_schema: JsonValue,
    }

    let mut promotions: Vec<Promotion> = Vec::new();

    for (type_name, schema) in &schemas.types {
        let Some(schema_obj) = schema.as_object() else {
            continue;
        };
        let Some(props) = schema_obj.get("properties").and_then(|p| p.as_object()) else {
            continue;
        };
        let parent_rust = sanitize_type_name(type_name, "Type");

        for (field_name, field_schema) in props {
            let Some(field_obj) = field_schema.as_object() else {
                continue;
            };
            // Promote inline string-enum schemas.  After schema normalization,
            // `env: [dev, staging, prod]` becomes `{type: "string", enum: [...]}`.
            // We promote any schema that has "enum" and whose "type" (if present) is
            // exactly "string" — not a named type reference like {type: "Environment"}.
            let type_val = field_obj.get("type").and_then(|t| t.as_str());
            if !field_obj.contains_key("enum") || !matches!(type_val, None | Some("string")) {
                continue;
            }
            if collect_string_enum_variants(field_obj).is_none() {
                continue; // non-string enum values — leave as-is
            }

            let field_pascal = to_pascal_case(field_name);
            let base_rust = format!("{parent_rust}{field_pascal}");
            let generated_rust = unique_identifier(&base_rust, &mut used_rust_names);

            // Use the Rust name as the source-name key; ensure it's also unique there.
            let mut generated_source = generated_rust.clone();
            while used_source_names.contains(&generated_source) {
                generated_source.push('_');
            }
            used_source_names.insert(generated_source.clone());

            let enum_values = field_obj["enum"].clone();
            promotions.push(Promotion {
                parent_source: type_name.clone(),
                field_name: field_name.clone(),
                generated_source,
                enum_schema: serde_json::json!({ "enum": enum_values }),
            });
        }
    }

    if promotions.is_empty() {
        return schemas.clone();
    }

    let mut types = schemas.types.clone();

    for p in &promotions {
        // Register the new standalone enum type.
        types.insert(p.generated_source.clone(), p.enum_schema.clone());

        // Rewrite the parent property to a type reference, preserving `optional`.
        if let Some(parent_schema) = types.get_mut(&p.parent_source) {
            if let Some(props) = parent_schema
                .as_object_mut()
                .and_then(|o| o.get_mut("properties"))
                .and_then(|pv| pv.as_object_mut())
            {
                if let Some(field_val) = props.get_mut(&p.field_name) {
                    let optional = field_val
                        .as_object()
                        .and_then(|o| o.get("optional"))
                        .cloned();
                    let mut new_field = serde_json::json!({ "type": p.generated_source.as_str() });
                    if let Some(opt) = optional {
                        new_field["optional"] = opt;
                    }
                    *field_val = new_field;
                }
            }
        }
    }

    CollectedSchemas {
        types,
        type_constraints: schemas.type_constraints.clone(),
    }
}

fn render_rust_types_expanded(schemas: &CollectedSchemas) -> String {
    if schemas.types.is_empty() {
        return concat!(
            "// Generated by super-yaml (first-pass Rust codegen).\n",
            "// WARNING: This file is generated. Do not edit manually.\n",
            "// No schema definitions found.\n"
        )
        .to_string();
    }

    let type_names = build_type_name_map(&schemas.types);
    let mut state = RenderState {
        type_names,
        all_types: schemas.types.clone(),
        type_constraints: schemas.type_constraints.clone(),
        needs_serde_derives: false,
        needs_serde_json_value: false,
        needs_constraint_runtime: false,
    };

    let mut definitions = Vec::new();
    for (source_name, schema) in &schemas.types {
        definitions.push(render_type_definition(source_name, schema, &mut state));
    }

    let mut out = String::new();
    out.push_str("// Generated by super-yaml (first-pass Rust codegen).\n");
    out.push_str("// WARNING: This file is generated. Do not edit manually.\n");
    if state.needs_serde_derives {
        out.push_str("use serde::{Deserialize, Serialize};\n");
    }
    if state.needs_serde_json_value {
        out.push_str("use serde_json::Value;\n");
    }
    if state.needs_constraint_runtime {
        out.push_str("use std::collections::BTreeMap;\n");
    }
    if state.needs_serde_derives || state.needs_serde_json_value || state.needs_constraint_runtime {
        out.push('\n');
    }
    if state.needs_constraint_runtime {
        out.push_str(&render_constraint_runtime_helpers());
        out.push('\n');
    }

    for def in definitions {
        out.push_str(&def);
        out.push('\n');
    }

    out
}

fn build_type_name_map(types: &BTreeMap<String, JsonValue>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut used = HashSet::new();

    for source_name in types.keys() {
        let base = sanitize_type_name(source_name, "Type");
        let unique = unique_identifier(&base, &mut used);
        out.insert(source_name.clone(), unique);
    }

    out
}

fn render_type_definition(
    source_name: &str,
    schema: &JsonValue,
    state: &mut RenderState,
) -> String {
    let rust_name = state
        .type_names
        .get(source_name)
        .cloned()
        .unwrap_or_else(|| sanitize_type_name(source_name, "Type"));

    let Some(schema_obj) = schema.as_object() else {
        state.needs_serde_json_value = true;
        return format!("pub type {rust_name} = Value;\n");
    };

    let mut out = if is_union_schema(schema_obj) {
        state.needs_serde_derives = true;
        render_union_enum(&rust_name, schema_obj, state)
    } else if let Some(members) = collect_keyed_enum_members(schema_obj) {
        state.needs_serde_derives = true;
        render_keyed_enum_helpers(source_name, &rust_name, schema_obj, &members, state)
    } else if let Some(variants) = collect_string_enum_variants(schema_obj) {
        state.needs_serde_derives = true;
        render_string_enum(&rust_name, &variants)
    } else if is_object_schema(schema_obj) {
        if let Some(properties) = schema_obj.get("properties").and_then(JsonValue::as_object) {
            state.needs_serde_derives = true;
            render_object_struct(&rust_name, properties, schema_obj, state)
        } else if let Some(values_schema) = schema_obj.get("values") {
            let value_type = rust_type_for_schema(values_schema, state);
            format!("pub type {rust_name} = std::collections::BTreeMap<String, {value_type}>;\n")
        } else {
            state.needs_serde_json_value = true;
            format!("pub type {rust_name} = Value;\n")
        }
    } else {
        let rust_type = rust_type_for_schema(schema, state);
        format!("pub type {rust_name} = {rust_type};\n")
    };

    if let Some(constraints) = state.type_constraints.get(source_name) {
        if !constraints.is_empty() {
            state.needs_constraint_runtime = true;
            out.push('\n');
            out.push_str(&render_constraint_functions(&rust_name, constraints));
        }
    }

    out
}

fn render_constraint_functions(
    type_name: &str,
    constraints: &BTreeMap<String, Vec<String>>,
) -> String {
    let mut out = String::new();
    let mut index = 1usize;
    let mut all_pairs = Vec::new();

    for (path, expressions) in constraints {
        for expression in expressions {
            let fn_name = format!(
                "check_{}_constraint_{}",
                sanitize_field_name(type_name).trim_end_matches('_'),
                index
            );
            let escaped_path = escape_string(path);
            let escaped_expr = escape_string(expression);
            out.push_str(&format!(
                "pub fn {fn_name}(value: &{type_name}) -> Result<(), String> {{\n    run_generated_constraint_check(value, &[\"{escaped_path}\"], &[\"{escaped_expr}\"])\n}}\n\n"
            ));
            all_pairs.push((escaped_path, escaped_expr));
            index += 1;
        }
    }

    let all_fn_name = format!(
        "check_{}_constraints",
        sanitize_field_name(type_name).trim_end_matches('_')
    );
    out.push_str(&format!(
        "pub fn {all_fn_name}(value: &{type_name}) -> Result<(), String> {{\n    run_generated_constraint_check(\n        value,\n        &[{}],\n        &[{}],\n    )\n}}\n",
        all_pairs
            .iter()
            .map(|(path, _)| format!("\"{path}\""))
            .collect::<Vec<_>>()
            .join(", "),
        all_pairs
            .iter()
            .map(|(_, expr)| format!("\"{expr}\""))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    out
}

fn render_constraint_runtime_helpers() -> String {
    concat!(
        "fn run_generated_constraint_check<T: serde::Serialize>(\n",
        "    value: &T,\n",
        "    paths: &[&str],\n",
        "    expressions: &[&str],\n",
        ") -> Result<(), String> {\n",
        "    if paths.len() != expressions.len() {\n",
        "        return Err(\"invalid generated constraints: path/expression length mismatch\".to_string());\n",
        "    }\n",
        "    let data = serde_json::to_value(value).map_err(|e| e.to_string())?;\n",
        "    let mut constraints: BTreeMap<String, Vec<String>> = BTreeMap::new();\n",
        "    for (path, expression) in paths.iter().zip(expressions.iter()) {\n",
        "        constraints\n",
        "            .entry((*path).to_string())\n",
        "            .or_default()\n",
        "            .push((*expression).to_string());\n",
        "    }\n",
        "    let env = BTreeMap::new();\n",
        "    super_yaml::validate::validate_constraints(&data, &env, &constraints)\n",
        "        .map_err(|e| e.to_string())\n",
        "}\n",
    )
    .to_string()
}

fn is_union_schema(schema_obj: &JsonMap<String, JsonValue>) -> bool {
    schema_obj.get("type").and_then(JsonValue::as_str) == Some("union")
}

fn render_union_enum(
    name: &str,
    schema_obj: &JsonMap<String, JsonValue>,
    state: &mut RenderState,
) -> String {
    let mut out = String::new();
    out.push_str("#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]\n");
    out.push_str("#[serde(untagged)]\n");
    out.push_str(&format!("pub enum {name} {{\n"));

    let mut used = HashSet::new();

    match schema_obj.get("options") {
        Some(JsonValue::Object(opt_map)) => {
            for (key, opt_schema) in opt_map {
                let variant_name = sanitize_variant_name(key, "Variant");
                let unique_variant = unique_identifier(&variant_name, &mut used);
                let rust_type = rust_type_for_schema(opt_schema, state);
                out.push_str(&format!("    {unique_variant}({rust_type}),\n"));
            }
        }
        Some(JsonValue::Array(items)) => {
            for (idx, opt_schema) in items.iter().enumerate() {
                let variant_name = infer_union_variant_name(opt_schema, idx, state);
                let unique_variant = unique_identifier(&variant_name, &mut used);
                let rust_type = rust_type_for_schema(opt_schema, state);
                out.push_str(&format!("    {unique_variant}({rust_type}),\n"));
            }
        }
        _ => {
            state.needs_serde_json_value = true;
            out.push_str("    Unknown(Value),\n");
        }
    }

    out.push_str("}\n");
    out
}

fn infer_union_variant_name(schema: &JsonValue, index: usize, state: &RenderState) -> String {
    if let Some(type_name) = schema
        .as_object()
        .and_then(|obj| obj.get("type"))
        .and_then(JsonValue::as_str)
    {
        if let Some(mapped) = state.type_names.get(type_name) {
            return mapped.clone();
        }
        let pascal = sanitize_type_name(type_name, "Variant");
        if !pascal.is_empty() {
            return pascal;
        }
    }
    format!("Variant{}", index + 1)
}

fn collect_string_enum_variants(schema_obj: &JsonMap<String, JsonValue>) -> Option<Vec<String>> {
    let values = schema_obj.get("enum")?.as_array()?;
    if values.is_empty() {
        return None;
    }

    let mut out = Vec::with_capacity(values.len());
    for value in values {
        let text = value.as_str()?;
        out.push(text.to_string());
    }
    Some(out)
}

fn collect_keyed_enum_members(
    schema_obj: &JsonMap<String, JsonValue>,
) -> Option<Vec<(String, JsonValue)>> {
    let enum_map = schema_obj.get("enum")?.as_object()?;
    if enum_map.is_empty() {
        return None;
    }
    if schema_obj.get("type").and_then(JsonValue::as_str).is_none() {
        return None;
    }
    let mut members: Vec<(String, JsonValue)> = enum_map
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    members.sort_by(|a, b| a.0.cmp(&b.0));
    Some(members)
}

fn is_object_schema(schema_obj: &JsonMap<String, JsonValue>) -> bool {
    match schema_obj.get("type").and_then(JsonValue::as_str) {
        Some("object") => true,
        Some(_) => false,
        None => schema_obj.contains_key("properties") || schema_obj.contains_key("values"),
    }
}

fn render_string_enum(name: &str, variants: &[String]) -> String {
    let mut out = String::new();
    out.push_str("#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]\n");
    out.push_str(&format!("pub enum {name} {{\n"));

    let mut used = HashSet::new();
    for raw in variants {
        let base = sanitize_variant_name(raw, "Variant");
        let variant_name = unique_identifier(&base, &mut used);

        if variant_name != raw.as_str() {
            out.push_str(&format!(
                "    #[serde(rename = \"{}\")]\n",
                escape_string(raw)
            ));
        }
        out.push_str(&format!("    {variant_name},\n"));
    }

    out.push_str("}\n");
    out
}

fn render_keyed_enum_helpers(
    source_name: &str,
    rust_name: &str,
    schema_obj: &JsonMap<String, JsonValue>,
    members: &[(String, JsonValue)],
    state: &mut RenderState,
) -> String {
    let base_type = schema_obj
        .get("type")
        .and_then(JsonValue::as_str)
        .map(|type_name| rust_type_for_type_name(type_name, schema_obj, state))
        .unwrap_or_else(|| {
            state.needs_serde_json_value = true;
            "Value".to_string()
        });
    let key_enum_name = format!("{rust_name}Key");
    let helper_fn = format!(
        "{}_value",
        sanitize_field_name(source_name).trim_end_matches('_')
    );

    let mut out = String::new();
    out.push_str(&format!("pub type {rust_name} = {base_type};\n\n"));
    out.push_str("#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]\n");
    out.push_str(&format!("pub enum {key_enum_name} {{\n"));
    let mut used_variants = HashSet::new();
    let mut variants = Vec::with_capacity(members.len());
    for (raw_key, _) in members {
        let base = sanitize_variant_name(raw_key, "Variant");
        let variant_name = unique_identifier(&base, &mut used_variants);
        variants.push((raw_key.clone(), variant_name.clone()));
        if variant_name != raw_key.as_str() {
            out.push_str(&format!(
                "    #[serde(rename = \"{}\")]\n",
                escape_string(raw_key)
            ));
        }
        out.push_str(&format!("    {variant_name},\n"));
    }
    out.push_str("}\n\n");

    out.push_str(&format!("impl {key_enum_name} {{\n"));
    out.push_str(&format!("    pub fn value(&self) -> {rust_name} {{\n"));
    out.push_str("        match self {\n");
    let declared_type_name = schema_obj
        .get("type")
        .and_then(JsonValue::as_str)
        .unwrap_or(source_name);
    for ((_, value), (_, variant_name)) in members.iter().zip(variants.iter()) {
        let literal = rust_value_literal(
            value,
            Some(&serde_json::json!({ "type": declared_type_name })),
            &state.all_types,
            &state.type_names,
        );
        out.push_str(&format!("            Self::{variant_name} => {literal},\n"));
    }
    out.push_str("        }\n");
    out.push_str("    }\n");
    out.push_str("}\n\n");

    out.push_str(&format!(
        "pub fn {helper_fn}(key: {key_enum_name}) -> {rust_name} {{\n    key.value()\n}}\n"
    ));
    out
}

fn render_object_struct(
    name: &str,
    properties: &JsonMap<String, JsonValue>,
    schema_obj: &JsonMap<String, JsonValue>,
    state: &mut RenderState,
) -> String {
    let mut out = String::new();
    out.push_str("#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]\n");
    out.push_str(&format!("pub struct {name} {{\n"));

    let required = required_property_set(schema_obj, properties);
    let mut used_fields = HashSet::new();

    let mut keys: Vec<&String> = properties.keys().collect();
    keys.sort_unstable();

    for key in keys {
        let schema = properties
            .get(key)
            .expect("property key present while iterating keys");
        let base_field = sanitize_field_name(key);
        let field_name = unique_identifier(&base_field, &mut used_fields);
        let optional = !required.contains(key.as_str());

        let mut rust_type = rust_type_for_schema(schema, state);
        if optional {
            rust_type = format!("Option<{rust_type}>");
        }

        // Emit versioning doc comments and attributes.
        if let Ok(Some(meta)) = parse_field_version_meta(schema) {
            if let Some(fn_num) = meta.field_number {
                out.push_str(&format!("    /// Field number: {fn_num}\n"));
            }
            if let Some(dep) = meta.deprecated {
                let note = match dep.message {
                    Some(msg) => format!("since {}: {}", dep.version, msg),
                    None => format!("since {}", dep.version),
                };
                out.push_str(&format!(
                    "    #[deprecated(note = \"{}\")]\n",
                    escape_string(&note)
                ));
            }
        }

        if field_name != key.as_str() {
            out.push_str(&format!(
                "    #[serde(rename = \"{}\")]\n",
                escape_string(key)
            ));
        }
        if optional {
            out.push_str("    #[serde(skip_serializing_if = \"Option::is_none\")]\n");
        }
        out.push_str(&format!("    pub {field_name}: {rust_type},\n"));
    }

    out.push_str("}\n");

    if let Some(template) = schema_obj.get("as_string").and_then(JsonValue::as_str) {
        out.push('\n');
        out.push_str(&render_display_impl(name, template));
    }

    out
}

/// Parses an `as_string` template into a `Display` implementation for `name`.
///
/// Template syntax: literal text interspersed with `{{property_name}}` placeholders.
fn render_display_impl(name: &str, template: &str) -> String {
    // Collect alternating segments: (format_str_piece, field_accessor).
    let mut format_str = String::new();
    let mut args: Vec<String> = Vec::new();
    let mut remaining = template;

    while let Some(open) = remaining.find("{{") {
        // Literal text before the placeholder.
        let literal = &remaining[..open];
        if !literal.is_empty() {
            format_str.push_str(&literal.replace('{', "{{").replace('}', "}}"));
        }
        remaining = &remaining[open + 2..];
        if let Some(close) = remaining.find("}}") {
            let placeholder = remaining[..close].trim();
            format_str.push_str("{}");
            args.push(format!("self.{}", sanitize_field_name(placeholder)));
            remaining = &remaining[close + 2..];
        }
    }
    // Trailing literal after the last placeholder.
    if !remaining.is_empty() {
        format_str.push_str(&remaining.replace('{', "{{").replace('}', "}}"));
    }

    let mut out = String::new();
    out.push_str(&format!("impl std::fmt::Display for {name} {{\n"));
    out.push_str("    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {\n");
    if args.is_empty() {
        out.push_str(&format!("        write!(f, \"{}\")\n", format_str));
    } else {
        out.push_str(&format!(
            "        write!(f, \"{}\", {})\n",
            format_str,
            args.join(", ")
        ));
    }
    out.push_str("    }\n");
    out.push_str("}\n");
    out
}

fn required_property_set(
    schema_obj: &JsonMap<String, JsonValue>,
    properties: &JsonMap<String, JsonValue>,
) -> HashSet<String> {
    // Backward-compatible behavior: explicit `required` list takes precedence.
    if let Some(required) = schema_obj.get("required").and_then(JsonValue::as_array) {
        let mut out = HashSet::new();
        for name in required {
            if let Some(name) = name.as_str() {
                out.insert(name.to_string());
            }
        }
        return out;
    }

    // New default: all properties are required unless `optional: true`.
    let mut out = HashSet::new();
    for (name, property_schema) in properties {
        let optional = property_schema
            .as_object()
            .and_then(|obj| obj.get("optional"))
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        if !optional {
            out.insert(name.clone());
        }
    }

    out
}

fn rust_type_for_schema(schema: &JsonValue, state: &mut RenderState) -> String {
    let Some(schema_obj) = schema.as_object() else {
        state.needs_serde_json_value = true;
        return "Value".to_string();
    };

    if let Some(type_name) = schema_obj.get("type").and_then(JsonValue::as_str) {
        return rust_type_for_type_name(type_name, schema_obj, state);
    }

    if let Some(enum_values) = schema_obj.get("enum").and_then(JsonValue::as_array) {
        return inferred_enum_backing_type(enum_values, state);
    }

    if schema_obj.contains_key("properties") {
        state.needs_serde_json_value = true;
        return "Value".to_string();
    }
    if let Some(values_schema) = schema_obj.get("values") {
        let value_type = rust_type_for_schema(values_schema, state);
        return format!("std::collections::BTreeMap<String, {value_type}>");
    }

    state.needs_serde_json_value = true;
    "Value".to_string()
}

fn rust_type_for_type_name(
    type_name: &str,
    schema_obj: &JsonMap<String, JsonValue>,
    state: &mut RenderState,
) -> String {
    match type_name {
        "string" => "String".to_string(),
        "integer" => "i64".to_string(),
        "number" => "f64".to_string(),
        "boolean" => "bool".to_string(),
        "null" => "()".to_string(),
        "array" => {
            if let Some(items) = schema_obj.get("items") {
                let item_type = rust_type_for_schema(items, state);
                format!("Vec<{item_type}>")
            } else {
                state.needs_serde_json_value = true;
                "Vec<Value>".to_string()
            }
        }
        "object" => {
            if let Some(values_schema) = schema_obj.get("values") {
                let value_type = rust_type_for_schema(values_schema, state);
                format!("std::collections::BTreeMap<String, {value_type}>")
            } else {
                state.needs_serde_json_value = true;
                "Value".to_string()
            }
        }
        "union" => {
            state.needs_serde_json_value = true;
            "Value".to_string()
        }
        other => {
            if let Some(mapped) = state.type_names.get(other) {
                mapped.clone()
            } else {
                state.needs_serde_json_value = true;
                "Value".to_string()
            }
        }
    }
}

fn inferred_enum_backing_type(enum_values: &[JsonValue], state: &mut RenderState) -> String {
    if enum_values.is_empty() {
        state.needs_serde_json_value = true;
        return "Value".to_string();
    }

    if enum_values.iter().all(JsonValue::is_string) {
        return "String".to_string();
    }
    if enum_values.iter().all(|v| v.as_i64().is_some()) {
        return "i64".to_string();
    }
    if enum_values.iter().all(JsonValue::is_number) {
        return "f64".to_string();
    }
    if enum_values.iter().all(JsonValue::is_boolean) {
        return "bool".to_string();
    }
    if enum_values.iter().all(JsonValue::is_null) {
        return "()".to_string();
    }

    state.needs_serde_json_value = true;
    "Value".to_string()
}

fn sanitize_type_name(raw: &str, fallback: &str) -> String {
    let mut out = to_pascal_case(raw);
    if out.is_empty() {
        out = fallback.to_string();
    }

    if !starts_with_ident_char(&out) {
        out = format!("Type{out}");
    }

    if is_rust_keyword(&out) {
        out.push_str("Type");
    }

    out
}

fn sanitize_variant_name(raw: &str, fallback: &str) -> String {
    let mut out = to_pascal_case(raw);
    if out.is_empty() {
        out = fallback.to_string();
    }

    if !starts_with_ident_char(&out) {
        out = format!("V{out}");
    }

    if is_rust_keyword(&out) {
        out.push_str("Value");
    }

    out
}

fn sanitize_field_name(raw: &str) -> String {
    let tokens = identifier_tokens(raw);
    let mut out = if tokens.is_empty() {
        "field".to_string()
    } else {
        tokens.join("_")
    };

    if out
        .chars()
        .next()
        .map(|c| c.is_ascii_digit())
        .unwrap_or(false)
    {
        out = format!("field_{out}");
    }

    if is_rust_keyword(&out) {
        out.push('_');
    }

    out
}

fn to_pascal_case(raw: &str) -> String {
    let mut out = String::new();
    for token in identifier_tokens(raw) {
        let mut chars = token.chars();
        if let Some(first) = chars.next() {
            out.push(first.to_ascii_uppercase());
            for ch in chars {
                out.push(ch.to_ascii_lowercase());
            }
        }
    }
    out
}

fn identifier_tokens(raw: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for chunk in raw.split(|c: char| !c.is_ascii_alphanumeric()) {
        if chunk.is_empty() {
            continue;
        }
        tokens.extend(split_camel_tokens(chunk));
    }
    tokens
}

fn split_camel_tokens(chunk: &str) -> Vec<String> {
    let chars: Vec<char> = chunk.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }

    let mut tokens = Vec::new();
    let mut start = 0usize;

    for i in 1..chars.len() {
        let prev = chars[i - 1];
        let curr = chars[i];
        let next = chars.get(i + 1).copied();

        let boundary = (prev.is_ascii_lowercase() && curr.is_ascii_uppercase())
            || (prev.is_ascii_alphabetic() && curr.is_ascii_digit())
            || (prev.is_ascii_digit() && curr.is_ascii_alphabetic())
            || (prev.is_ascii_uppercase()
                && curr.is_ascii_uppercase()
                && next.map(|n| n.is_ascii_lowercase()).unwrap_or(false));

        if boundary {
            let token: String = chars[start..i].iter().collect();
            if !token.is_empty() {
                tokens.push(token.to_ascii_lowercase());
            }
            start = i;
        }
    }

    let token: String = chars[start..].iter().collect();
    if !token.is_empty() {
        tokens.push(token.to_ascii_lowercase());
    }

    tokens
}

fn unique_identifier(base: &str, used: &mut HashSet<String>) -> String {
    if used.insert(base.to_string()) {
        return base.to_string();
    }

    let mut idx = 2usize;
    loop {
        let candidate = format!("{base}{idx}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        idx += 1;
    }
}

fn starts_with_ident_char(text: &str) -> bool {
    text.chars()
        .next()
        .map(|c| c == '_' || c.is_ascii_alphabetic())
        .unwrap_or(false)
}

fn is_rust_keyword(text: &str) -> bool {
    RUST_KEYWORDS.iter().any(|kw| kw == &text)
}

fn escape_string(raw: &str) -> String {
    raw.replace('\\', "\\\\").replace('"', "\\\"")
}

// ─── Data constants / fns generation ──────────────────────────────────────────

/// Generates Rust types AND data constants/fns from a `.syaml` file path.
///
/// The compiled (resolved) `---data` section is emitted as `pub const` or `pub fn`
/// items after the type definitions, separated by a `// --- Data ---` comment.
pub fn generate_rust_types_and_data_from_path(
    path: impl AsRef<Path>,
    env_provider: &dyn crate::EnvProvider,
) -> Result<String, SyamlError> {
    let path = path.as_ref();

    // Parse to get type hints (path → schema type name).
    let input = fs::read_to_string(path).map_err(|e| {
        SyamlError::ImportError(format!("failed to read '{}': {e}", path.display()))
    })?;
    let parsed = parse_document(&input)?;
    let type_hints = parsed.data.type_hints.clone();

    // Collect schema types for type rendering and name resolution.
    let mut ctx = TypeCollectionContext::new();
    let schemas = collect_types_from_file(path, &mut ctx)?;

    // Expand inline enums to named types — both the type output and the data
    // renderer must operate on the same expanded map so their names agree.
    let expanded = expand_inline_enum_schemas(&schemas);
    let type_names = build_type_name_map(&expanded.types);

    // Compile to get resolved data values.
    let compiled = crate::compile_document_from_path_with_fetch(path, env_provider, None, false)?;

    // Render types using the expanded schema set.
    let types_output = render_rust_types_expanded(&expanded);

    // Render data items using the same expanded types + type_names.
    let data_output = render_rust_data(&compiled.value, &type_hints, &expanded.types, &type_names);

    let functional_output = if let Some(ref func_doc) = parsed.functional {
        crate::functional::generate_rust_function_stubs(func_doc, &expanded.types)
    } else {
        String::new()
    };

    if data_output.is_empty() && functional_output.is_empty() {
        return Ok(types_output);
    }

    let mut result = types_output;
    if !data_output.is_empty() {
        result = format!("{result}\n// --- Data ---\n\n{data_output}");
    }
    if !functional_output.is_empty() {
        result = format!("{result}\n{functional_output}");
    }
    Ok(result)
}

fn render_rust_data(
    data: &JsonValue,
    type_hints: &BTreeMap<String, String>,
    types: &BTreeMap<String, JsonValue>,
    type_names: &BTreeMap<String, String>,
) -> String {
    let Some(obj) = data.as_object() else {
        return String::new();
    };
    if obj.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    for (key, value) in obj {
        let hint_path = format!("$.{key}");
        let type_hint = type_hints.get(&hint_path).map(String::as_str);
        // Resolve effective schema: if hint is a named type, look it up directly.
        let schema = type_hint.and_then(|hint| types.get(hint));
        let var_name = sanitize_field_name(key);
        let item = render_rust_data_item(&var_name, type_hint, value, schema, types, type_names);
        out.push_str(&item);
        out.push('\n');
    }
    out
}

/// Returns the Rust type name for a top-level data value given its type hint.
fn data_rust_type(
    type_hint: Option<&str>,
    _schema: Option<&JsonValue>,
    value: &JsonValue,
    type_names: &BTreeMap<String, String>,
) -> String {
    if let Some(hint) = type_hint {
        match hint {
            "string" => return "String".to_string(),
            "integer" => return "i64".to_string(),
            "number" => return "f64".to_string(),
            "boolean" => return "bool".to_string(),
            other => {
                if let Some(mapped) = type_names.get(other) {
                    return mapped.clone();
                }
            }
        }
    }
    // No hint: infer from value.
    match value {
        JsonValue::Bool(_) => "bool".to_string(),
        JsonValue::Number(n) if n.as_i64().is_some() => "i64".to_string(),
        JsonValue::Number(_) => "f64".to_string(),
        JsonValue::String(_) => "String".to_string(),
        JsonValue::Array(_) => "Vec<serde_json::Value>".to_string(),
        JsonValue::Object(_) => "serde_json::Value".to_string(),
        JsonValue::Null => "()".to_string(),
    }
}

fn render_rust_data_item(
    var_name: &str,
    type_hint: Option<&str>,
    value: &JsonValue,
    schema: Option<&JsonValue>,
    types: &BTreeMap<String, JsonValue>,
    type_names: &BTreeMap<String, String>,
) -> String {
    let rust_type = data_rust_type(type_hint, schema, value, type_names);
    // SCREAMING_SNAKE_CASE for const identifiers.
    let const_name = var_name.to_uppercase();

    match value {
        JsonValue::Null => {
            format!("pub const {const_name}: () = ();\n")
        }
        JsonValue::Bool(b) => {
            format!("pub const {const_name}: {rust_type} = {b};\n")
        }
        JsonValue::Number(n) => {
            // Determine int vs float from schema type or value itself.
            let schema_type = schema
                .and_then(|s| s.as_object())
                .and_then(|o| o.get("type"))
                .and_then(|t| t.as_str());
            let is_float = schema_type == Some("number") || n.as_i64().is_none();

            if is_float {
                let f = n.as_f64().unwrap_or(0.0);
                let lit = if f.fract() == 0.0 {
                    format!("{:.1}", f)
                } else {
                    format!("{}", f)
                };
                format!("pub const {const_name}: {rust_type} = {lit};\n")
            } else {
                let i = n.as_i64().unwrap();
                format!("pub const {const_name}: {rust_type} = {i};\n")
            }
        }
        JsonValue::String(s) => {
            // Check if the schema represents a string enum.
            let is_enum = schema
                .and_then(|s| s.as_object())
                .and_then(|o| o.get("enum"))
                .is_some();
            if is_enum {
                let variant = sanitize_variant_name(s, "Variant");
                return format!("pub const {const_name}: {rust_type} = {rust_type}::{variant};\n");
            }
            let escaped = escape_string(s);
            format!("pub fn {var_name}() -> String {{\n    \"{escaped}\".to_string()\n}}\n")
        }
        JsonValue::Array(arr) => {
            let item_schema = schema
                .and_then(|s| s.as_object())
                .and_then(|o| o.get("items"));
            let item_type = item_schema
                .map(|is| rust_schema_to_type(Some(is), types, type_names))
                .unwrap_or_else(|| "serde_json::Value".to_string());
            let items: Vec<String> = arr
                .iter()
                .map(|item| rust_value_literal(item, item_schema, types, type_names))
                .collect();
            format!(
                "pub fn {var_name}() -> Vec<{item_type}> {{\n    vec![{}]\n}}\n",
                items.join(", ")
            )
        }
        JsonValue::Object(obj_map) => {
            if let Some(schema_val) = schema {
                if let Some(schema_obj) = schema_val.as_object() {
                    // Struct with named properties.
                    if let Some(props) = schema_obj.get("properties").and_then(|p| p.as_object()) {
                        let required = required_property_set(schema_obj, props);
                        let mut field_lines = String::new();
                        let mut sorted_keys: Vec<_> = props.keys().collect();
                        sorted_keys.sort_unstable();
                        for prop_key in sorted_keys {
                            let prop_schema = props.get(prop_key).unwrap();
                            let field_name = sanitize_field_name(prop_key);
                            let is_optional = !required.contains(prop_key.as_str());
                            if is_optional {
                                if let Some(field_val) = obj_map.get(prop_key) {
                                    let lit = rust_value_literal(
                                        field_val,
                                        Some(prop_schema),
                                        types,
                                        type_names,
                                    );
                                    field_lines
                                        .push_str(&format!("        {field_name}: Some({lit}),\n"));
                                } else {
                                    field_lines.push_str(&format!("        {field_name}: None,\n"));
                                }
                            } else if let Some(field_val) = obj_map.get(prop_key) {
                                let lit = rust_value_literal(
                                    field_val,
                                    Some(prop_schema),
                                    types,
                                    type_names,
                                );
                                field_lines.push_str(&format!("        {field_name}: {lit},\n"));
                            }
                        }
                        return format!(
                            "pub fn {var_name}() -> {rust_type} {{\n    {rust_type} {{\n{field_lines}    }}\n}}\n"
                        );
                    }

                    // Typed dict.
                    if let Some(values_schema) = schema_obj.get("values") {
                        let val_type = rust_schema_to_type(Some(values_schema), types, type_names);
                        let mut entries: Vec<String> = obj_map
                            .iter()
                            .map(|(k, v)| {
                                let lit =
                                    rust_value_literal(v, Some(values_schema), types, type_names);
                                format!("        (\"{}\".to_string(), {})", escape_string(k), lit)
                            })
                            .collect();
                        entries.sort();
                        let entries_str = if entries.is_empty() {
                            String::new()
                        } else {
                            format!("\n{}\n    ", entries.join(",\n"))
                        };
                        return format!(
                            "pub fn {var_name}() -> std::collections::BTreeMap<String, {val_type}> {{\n    [{entries_str}].into_iter().collect()\n}}\n"
                        );
                    }
                }
            }

            // Fallback: serde_json::json! macro.
            let json_str = serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_string());
            format!(
                "pub fn {var_name}() -> serde_json::Value {{\n    serde_json::json!({json_str})\n}}\n"
            )
        }
    }
}

/// Generates a Rust literal expression for `value` given its `schema`.
///
/// Used recursively for struct fields, array items, and dict values.
fn rust_value_literal(
    value: &JsonValue,
    schema: Option<&JsonValue>,
    types: &BTreeMap<String, JsonValue>,
    type_names: &BTreeMap<String, String>,
) -> String {
    // Resolve a schema type-reference ({type: "SomeName"}) to the named schema + Rust name.
    let (effective_schema, effective_rust_name) =
        resolve_schema_type_ref(schema, types, type_names);

    match value {
        JsonValue::Null => "()".to_string(),
        JsonValue::Bool(b) => b.to_string(),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.to_string()
            } else if let Some(f) = n.as_f64() {
                if f.fract() == 0.0 {
                    format!("{:.1}", f)
                } else {
                    format!("{}", f)
                }
            } else {
                "0".to_string()
            }
        }
        JsonValue::String(s) => {
            let is_enum = effective_schema
                .and_then(|s| s.as_object())
                .and_then(|o| o.get("enum"))
                .is_some();
            if is_enum {
                if let Some(type_name) = effective_rust_name.as_deref() {
                    let variant = sanitize_variant_name(s, "Variant");
                    return format!("{type_name}::{variant}");
                }
            }
            format!("\"{}\".to_string()", escape_string(s))
        }
        JsonValue::Array(arr) => {
            let item_schema = effective_schema
                .and_then(|s| s.as_object())
                .and_then(|o| o.get("items"));
            let items: Vec<String> = arr
                .iter()
                .map(|item| rust_value_literal(item, item_schema, types, type_names))
                .collect();
            format!("vec![{}]", items.join(", "))
        }
        JsonValue::Object(obj_map) => {
            if let Some(schema_val) = effective_schema {
                if let Some(schema_obj) = schema_val.as_object() {
                    // Struct with properties — requires a known type name.
                    if let Some(props) = schema_obj.get("properties").and_then(|p| p.as_object()) {
                        if let Some(struct_name) = effective_rust_name.as_deref() {
                            let required = required_property_set(schema_obj, props);
                            let mut field_parts: Vec<String> = Vec::new();
                            let mut sorted_keys: Vec<_> = props.keys().collect();
                            sorted_keys.sort_unstable();
                            for prop_key in sorted_keys {
                                let prop_schema = props.get(prop_key).unwrap();
                                let field_name = sanitize_field_name(prop_key);
                                let is_optional = !required.contains(prop_key.as_str());
                                if is_optional {
                                    if let Some(field_val) = obj_map.get(prop_key) {
                                        let lit = rust_value_literal(
                                            field_val,
                                            Some(prop_schema),
                                            types,
                                            type_names,
                                        );
                                        field_parts.push(format!("{field_name}: Some({lit})"));
                                    } else {
                                        field_parts.push(format!("{field_name}: None"));
                                    }
                                } else if let Some(field_val) = obj_map.get(prop_key) {
                                    let lit = rust_value_literal(
                                        field_val,
                                        Some(prop_schema),
                                        types,
                                        type_names,
                                    );
                                    field_parts.push(format!("{field_name}: {lit}"));
                                }
                            }
                            return format!("{struct_name} {{ {} }}", field_parts.join(", "));
                        }
                    }

                    // Typed dict.
                    if let Some(values_schema) = schema_obj.get("values") {
                        let entries: Vec<String> = obj_map
                            .iter()
                            .map(|(k, v)| {
                                let lit =
                                    rust_value_literal(v, Some(values_schema), types, type_names);
                                format!("(\"{}\".to_string(), {})", escape_string(k), lit)
                            })
                            .collect();
                        return format!("[{}].into_iter().collect()", entries.join(", "));
                    }
                }
            }

            // Fallback: serde_json::json! macro.
            let json_str = serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string());
            format!("serde_json::json!({})", json_str)
        }
    }
}

/// Resolves a schema type-reference `{type: "SomeName"}` to the named schema and its
/// Rust identifier. Returns `(effective_schema, rust_type_name)`.
fn resolve_schema_type_ref<'a>(
    schema: Option<&'a JsonValue>,
    types: &'a BTreeMap<String, JsonValue>,
    type_names: &BTreeMap<String, String>,
) -> (Option<&'a JsonValue>, Option<String>) {
    let Some(s) = schema else {
        return (None, None);
    };
    if let Some(type_ref) = s
        .as_object()
        .and_then(|o| o.get("type"))
        .and_then(|t| t.as_str())
    {
        if let Some(named_schema) = types.get(type_ref) {
            let rust_name = type_names.get(type_ref).cloned();
            return (Some(named_schema), rust_name);
        }
    }
    (Some(s), None)
}

/// Returns the Rust type string for a schema, without needing mutable RenderState.
fn rust_schema_to_type(
    schema: Option<&JsonValue>,
    types: &BTreeMap<String, JsonValue>,
    type_names: &BTreeMap<String, String>,
) -> String {
    let Some(schema_obj) = schema.and_then(|s| s.as_object()) else {
        return "serde_json::Value".to_string();
    };

    if let Some(type_name) = schema_obj.get("type").and_then(|t| t.as_str()) {
        return match type_name {
            "string" => "String".to_string(),
            "integer" => "i64".to_string(),
            "number" => "f64".to_string(),
            "boolean" => "bool".to_string(),
            "null" => "()".to_string(),
            "array" => {
                let item_type = schema_obj
                    .get("items")
                    .map(|is| rust_schema_to_type(Some(is), types, type_names))
                    .unwrap_or_else(|| "serde_json::Value".to_string());
                format!("Vec<{item_type}>")
            }
            "object" => {
                if let Some(values) = schema_obj.get("values") {
                    let val_type = rust_schema_to_type(Some(values), types, type_names);
                    format!("std::collections::BTreeMap<String, {val_type}>")
                } else {
                    "serde_json::Value".to_string()
                }
            }
            other => type_names
                .get(other)
                .cloned()
                .unwrap_or_else(|| "serde_json::Value".to_string()),
        };
    }

    if schema_obj.contains_key("enum") {
        return "String".to_string();
    }
    if let Some(values) = schema_obj.get("values") {
        let val_type = rust_schema_to_type(Some(values), types, type_names);
        return format!("std::collections::BTreeMap<String, {val_type}>");
    }
    "serde_json::Value".to_string()
}

#[cfg(test)]
mod tests {
    use super::{generate_rust_types, render_rust_types, CollectedSchemas};
    use serde_json::json;

    #[test]
    fn renders_structs_enums_and_aliases() {
        let input = r#"
---!syaml/v0
---schema
MessageKind:
  enum: [join, leave]
WsMessage:
  type: object
  properties:
    kind:
      type: MessageKind
    room_id:
      type: string
    payload:
      type: object
      optional: true
Batch:
  type: array
  items:
    type: WsMessage
---data
example: 1
"#;

        let rendered = generate_rust_types(input).unwrap();

        assert!(rendered.contains("pub enum MessageKind"));
        assert!(rendered.contains("#[serde(rename = \"join\")]"));
        assert!(rendered.contains("pub struct WsMessage"));
        assert!(rendered.contains("pub room_id: String"));
        assert!(rendered.contains("pub payload: Option<Value>"));
        assert!(rendered.contains("pub type Batch = Vec<WsMessage>;"));
    }

    #[test]
    fn renders_empty_type_set() {
        let rendered = render_rust_types(&CollectedSchemas::default());
        assert!(rendered.contains("No schema definitions found"));
    }

    #[test]
    fn falls_back_to_value_for_mixed_enum() {
        let mut schemas = CollectedSchemas::default();
        schemas
            .types
            .insert("Mixed".to_string(), json!({"enum": ["a", 1]}));
        let rendered = render_rust_types(&schemas);
        assert!(rendered.contains("pub type Mixed = Value;"));
    }

    #[test]
    fn generates_constraint_functions_for_inline_numeric_keywords() {
        let input = r#"
---!syaml/v0
---schema
Port:
  type: integer
  minimum: 1
  maximum: 65535
---data
port <Port>: 8080
"#;
        let rendered = generate_rust_types(input).unwrap();
        assert!(
            rendered.contains("check_port_constraints"),
            "expected constraint function, got:\n{}",
            rendered
        );
        assert!(
            rendered.contains("value >= 1"),
            "expected minimum expression, got:\n{}",
            rendered
        );
        assert!(
            rendered.contains("value <= 65535"),
            "expected maximum expression, got:\n{}",
            rendered
        );
    }

    #[test]
    fn generates_constraint_functions_for_inline_string_length_keywords() {
        let input = r#"
---!syaml/v0
---schema
Tag:
  type: string
  minLength: 1
  maxLength: 64
---data
tag <Tag>: hello
"#;
        let rendered = generate_rust_types(input).unwrap();
        assert!(
            rendered.contains("check_tag_constraints"),
            "expected constraint function, got:\n{}",
            rendered
        );
        assert!(
            rendered.contains("len(value) >= 1"),
            "expected minLength expression, got:\n{}",
            rendered
        );
        assert!(
            rendered.contains("len(value) <= 64"),
            "expected maxLength expression, got:\n{}",
            rendered
        );
    }

    #[test]
    fn generates_constraint_functions_for_inline_array_item_count_keywords() {
        let input = r#"
---!syaml/v0
---schema
Tags:
  type: array
  items:
    type: string
  minItems: 1
  maxItems: 10
---data
tags <Tags>:
  - hello
"#;
        let rendered = generate_rust_types(input).unwrap();
        assert!(
            rendered.contains("check_tags_constraints"),
            "expected constraint function, got:\n{}",
            rendered
        );
        assert!(
            rendered.contains("len(value) >= 1"),
            "expected minItems expression, got:\n{}",
            rendered
        );
        assert!(
            rendered.contains("len(value) <= 10"),
            "expected maxItems expression, got:\n{}",
            rendered
        );
    }

    #[test]
    fn inline_keywords_and_constraints_section_both_appear_in_generated_code() {
        let input = r#"
---!syaml/v0
---schema
Count:
  type: integer
  minimum: 0
  constraints: "value != 42"
---data
count <Count>: 7
"#;
        let rendered = generate_rust_types(input).unwrap();
        assert!(
            rendered.contains("value >= 0"),
            "missing minimum expr:\n{}",
            rendered
        );
        assert!(
            rendered.contains("value != 42"),
            "missing explicit constraint:\n{}",
            rendered
        );
    }
}
