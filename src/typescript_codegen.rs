//! TypeScript type generation from named schema definitions.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::ast::{ImportBinding, ParsedDocument};
use crate::{parse_document, SyamlError};

const TYPESCRIPT_KEYWORDS: &[&str] = &[
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "new",
    "null",
    "return",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "as",
    "implements",
    "interface",
    "let",
    "package",
    "private",
    "protected",
    "public",
    "static",
    "yield",
    "any",
    "boolean",
    "constructor",
    "declare",
    "get",
    "module",
    "require",
    "number",
    "set",
    "string",
    "symbol",
    "type",
    "from",
    "of",
    "readonly",
    "keyof",
    "namespace",
    "abstract",
    "never",
    "object",
    "unknown",
    "bigint",
    "override",
];

/// Generates TypeScript types from an in-memory `.syaml` document.
///
/// Imported schemas are resolved relative to the current working directory.
pub fn generate_typescript_types(input: &str) -> Result<String, SyamlError> {
    let cwd = std::env::current_dir()?;
    let parsed = parse_document(input)?;

    let mut ctx = TypeCollectionContext::new();
    let types = collect_exported_types(parsed, &cwd, &mut ctx)?;
    Ok(render_typescript_types(&types))
}

/// Generates TypeScript types from a `.syaml` file path.
///
/// Imported schemas are resolved relative to each file's parent directory.
pub fn generate_typescript_types_from_path(path: impl AsRef<Path>) -> Result<String, SyamlError> {
    let mut ctx = TypeCollectionContext::new();
    let types = collect_types_from_file(path.as_ref(), &mut ctx)?;
    Ok(render_typescript_types(&types))
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
}

fn render_typescript_types(types: &BTreeMap<String, JsonValue>) -> String {
    if types.is_empty() {
        return "// No schema definitions found.\n".to_string();
    }

    let type_names = build_type_name_map(types);
    let mut state = RenderState { type_names };

    let mut definitions = Vec::new();
    for (source_name, schema) in types {
        definitions.push(render_type_definition(source_name, schema, &mut state));
    }

    let mut out = String::new();
    out.push_str("// Generated by super-yaml (first-pass TypeScript codegen).\n\n");
    out.push_str(&definitions.join("\n\n"));
    out.push('\n');
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
    let ts_name = state
        .type_names
        .get(source_name)
        .cloned()
        .unwrap_or_else(|| sanitize_type_name(source_name, "Type"));

    let Some(schema_obj) = schema.as_object() else {
        return format!("export type {ts_name} = unknown;");
    };

    if let Some(variants) = collect_string_enum_variants(schema_obj) {
        return render_string_enum_alias(&ts_name, &variants);
    }

    if is_object_schema(schema_obj) {
        if let Some(properties) = schema_obj.get("properties").and_then(JsonValue::as_object) {
            return render_object_interface(&ts_name, properties, schema_obj, state);
        }
        return format!("export type {ts_name} = unknown;");
    }

    let ts_type = ts_type_for_schema(schema, state);
    format!("export type {ts_name} = {ts_type};")
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

fn render_string_enum_alias(name: &str, variants: &[String]) -> String {
    let mut members = Vec::with_capacity(variants.len());
    for variant in variants {
        members.push(format!("\"{}\"", escape_string(variant)));
    }
    format!("export type {name} = {};", members.join(" | "))
}

fn render_object_interface(
    name: &str,
    properties: &JsonMap<String, JsonValue>,
    schema_obj: &JsonMap<String, JsonValue>,
    state: &mut RenderState,
) -> String {
    let required = required_property_set(schema_obj, properties);
    let mut keys: Vec<&String> = properties.keys().collect();
    keys.sort_unstable();

    let mut out = String::new();
    out.push_str(&format!("export interface {name} {{\n"));

    for key in keys {
        let schema = properties
            .get(key)
            .expect("property key present while iterating keys");
        let optional = !required.contains(key.as_str());
        let ts_type = ts_type_for_schema(schema, state);
        let field_name = render_property_name(key);
        let optional_suffix = if optional { "?" } else { "" };
        out.push_str(&format!("  {field_name}{optional_suffix}: {ts_type};\n"));
    }

    out.push('}');
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

fn ts_type_for_schema(schema: &JsonValue, state: &mut RenderState) -> String {
    let Some(schema_obj) = schema.as_object() else {
        return "unknown".to_string();
    };

    if let Some(type_name) = schema_obj.get("type").and_then(JsonValue::as_str) {
        return ts_type_for_type_name(type_name, schema_obj, state);
    }

    if schema_obj.get("enum").is_some() {
        if let Some(variants) = collect_string_enum_variants(schema_obj) {
            let members: Vec<String> = variants
                .iter()
                .map(|variant| format!("\"{}\"", escape_string(variant)))
                .collect();
            return members.join(" | ");
        }
        return "unknown".to_string();
    }

    if schema_obj.contains_key("properties") {
        return "unknown".to_string();
    }

    "unknown".to_string()
}

fn ts_type_for_type_name(
    type_name: &str,
    schema_obj: &JsonMap<String, JsonValue>,
    state: &mut RenderState,
) -> String {
    match type_name {
        "string" => "string".to_string(),
        "integer" => "number".to_string(),
        "number" => "number".to_string(),
        "boolean" => "boolean".to_string(),
        "null" => "null".to_string(),
        "array" => {
            if let Some(items) = schema_obj.get("items") {
                let item_type = ts_type_for_schema(items, state);
                format!("Array<{item_type}>")
            } else {
                "Array<unknown>".to_string()
            }
        }
        "object" => "unknown".to_string(),
        other => state
            .type_names
            .get(other)
            .cloned()
            .unwrap_or_else(|| "unknown".to_string()),
    }
}

fn sanitize_type_name(raw: &str, fallback: &str) -> String {
    let mut out = to_pascal_case(raw);
    if out.is_empty() {
        out = fallback.to_string();
    }

    if !starts_with_ident_char(&out) {
        out = format!("Type{out}");
    }

    if is_typescript_keyword(&out) {
        out.push_str("Type");
    }

    out
}

fn render_property_name(raw: &str) -> String {
    if is_valid_ts_identifier(raw) && !is_typescript_keyword(raw) {
        raw.to_string()
    } else {
        format!("\"{}\"", escape_string(raw))
    }
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

fn is_valid_ts_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    if !(first == '_' || first == '$' || first.is_ascii_alphabetic()) {
        return false;
    }

    chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
}

fn starts_with_ident_char(text: &str) -> bool {
    text.chars()
        .next()
        .map(|c| c == '_' || c == '$' || c.is_ascii_alphabetic())
        .unwrap_or(false)
}

fn is_typescript_keyword(text: &str) -> bool {
    TYPESCRIPT_KEYWORDS.iter().any(|kw| kw == &text)
}

fn escape_string(raw: &str) -> String {
    raw.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::{generate_typescript_types, render_typescript_types};
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn renders_interfaces_aliases_and_string_unions() {
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

        let rendered = generate_typescript_types(input).unwrap();

        assert!(rendered.contains("export type MessageKind = \"join\" | \"leave\";"));
        assert!(rendered.contains("export interface WsMessage"));
        assert!(rendered.contains("room_id: string;"));
        assert!(rendered.contains("payload?: unknown;"));
        assert!(rendered.contains("export type Batch = Array<WsMessage>;"));
    }

    #[test]
    fn explicit_required_list_controls_requiredness() {
        let input = r#"
---!syaml/v0
---schema
Config:
  type: object
  required: [must]
  properties:
    must:
      type: string
      optional: true
    maybe:
      type: string
---data
x: 1
"#;

        let rendered = generate_typescript_types(input).unwrap();
        assert!(rendered.contains("must: string;"));
        assert!(rendered.contains("maybe?: string;"));
    }

    #[test]
    fn falls_back_to_unknown_for_mixed_enum_and_ambiguous_object() {
        let mut types = BTreeMap::new();
        types.insert("Mixed".to_string(), json!({"enum": ["a", 1]}));
        types.insert("LooseObject".to_string(), json!({"type": "object"}));

        let rendered = render_typescript_types(&types);
        assert!(rendered.contains("export type Mixed = unknown;"));
        assert!(rendered.contains("export type LooseObject = unknown;"));
    }

    #[test]
    fn sanitizes_identifiers_for_types_and_properties() {
        let input = r#"
---!syaml/v0
---schema
shared.http-server:
  type: object
  properties:
    kebab-case:
      type: string
    class:
      type: string
---data
x: 1
"#;

        let rendered = generate_typescript_types(input).unwrap();
        assert!(rendered.contains("export interface SharedHttpServer"));
        assert!(rendered.contains("\"kebab-case\": string;"));
        assert!(rendered.contains("\"class\": string;"));
    }

    #[test]
    fn renders_empty_type_set() {
        let rendered = render_typescript_types(&BTreeMap::new());
        assert!(rendered.contains("No schema definitions found"));
    }
}
