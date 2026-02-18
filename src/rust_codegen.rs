//! Rust type generation from named schema definitions.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::ast::{ImportBinding, ParsedDocument};
use crate::{parse_document, SyamlError};

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
    cache: HashMap<PathBuf, BTreeMap<String, JsonValue>>,
    stack: Vec<PathBuf>,
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
) -> Result<BTreeMap<String, JsonValue>, SyamlError> {
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

    let parsed = parse_document(&input)?;
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
) -> Result<BTreeMap<String, JsonValue>, SyamlError> {
    let mut exported = parsed.schema.types;

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
    registry: &mut BTreeMap<String, JsonValue>,
    alias: &str,
    imported: &BTreeMap<String, JsonValue>,
) -> Result<(), SyamlError> {
    for (type_name, schema) in imported {
        let namespaced = format!("{alias}.{type_name}");
        if registry.contains_key(&namespaced) {
            return Err(SyamlError::ImportError(format!(
                "imported type '{}' conflicts with existing type",
                namespaced
            )));
        }
        registry.insert(namespaced, schema.clone());
    }
    Ok(())
}

struct RenderState {
    type_names: BTreeMap<String, String>,
    needs_serde_derives: bool,
    needs_serde_json_value: bool,
}

fn render_rust_types(types: &BTreeMap<String, JsonValue>) -> String {
    if types.is_empty() {
        return "// No schema definitions found.\n".to_string();
    }

    let type_names = build_type_name_map(types);
    let mut state = RenderState {
        type_names,
        needs_serde_derives: false,
        needs_serde_json_value: false,
    };

    let mut definitions = Vec::new();
    for (source_name, schema) in types {
        definitions.push(render_type_definition(source_name, schema, &mut state));
    }

    let mut out = String::new();
    out.push_str("// Generated by super-yaml (first-pass Rust codegen).\n");
    if state.needs_serde_derives {
        out.push_str("use serde::{Deserialize, Serialize};\n");
    }
    if state.needs_serde_json_value {
        out.push_str("use serde_json::Value;\n");
    }
    if state.needs_serde_derives || state.needs_serde_json_value {
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

    if let Some(variants) = collect_string_enum_variants(schema_obj) {
        state.needs_serde_derives = true;
        return render_string_enum(&rust_name, &variants);
    }

    if is_object_schema(schema_obj) {
        if let Some(properties) = schema_obj.get("properties").and_then(JsonValue::as_object) {
            state.needs_serde_derives = true;
            return render_object_struct(&rust_name, properties, schema_obj, state);
        }
        state.needs_serde_json_value = true;
        return format!("pub type {rust_name} = Value;\n");
    }

    let rust_type = rust_type_for_schema(schema, state);
    format!("pub type {rust_name} = {rust_type};\n")
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

fn is_object_schema(schema_obj: &JsonMap<String, JsonValue>) -> bool {
    match schema_obj.get("type").and_then(JsonValue::as_str) {
        Some("object") => true,
        Some(_) => false,
        None => schema_obj.contains_key("properties"),
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

#[cfg(test)]
mod tests {
    use super::{generate_rust_types, render_rust_types};
    use serde_json::json;
    use std::collections::BTreeMap;

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
        let rendered = render_rust_types(&BTreeMap::new());
        assert!(rendered.contains("No schema definitions found"));
    }

    #[test]
    fn falls_back_to_value_for_mixed_enum() {
        let mut types = BTreeMap::new();
        types.insert("Mixed".to_string(), json!({"enum": ["a", 1]}));
        let rendered = render_rust_types(&types);
        assert!(rendered.contains("pub type Mixed = Value;"));
    }
}
