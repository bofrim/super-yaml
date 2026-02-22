//! Proto3 file generation from named schema definitions.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::ast::ImportBinding;
use crate::schema::parse_field_version_meta;
use crate::{parse_document, SyamlError};

/// Generates a proto3 file from an in-memory `.syaml` document.
///
/// Imported schemas are resolved relative to the current working directory.
/// Returns an error if any `message` field is missing a `field_number`.
pub fn generate_proto_types(input: &str) -> Result<String, SyamlError> {
    let cwd = std::env::current_dir()?;
    let parsed = parse_document(input)?;
    let mut ctx = TypeCollectionContext::new();
    let types = collect_exported_types(parsed, &cwd, &mut ctx)?;
    render_proto_types(&types)
}

/// Generates a proto3 file from a `.syaml` file path.
///
/// Imported schemas are resolved relative to each file's parent directory.
/// Returns an error if any `message` field is missing a `field_number`.
pub fn generate_proto_types_from_path(path: impl AsRef<Path>) -> Result<String, SyamlError> {
    let mut ctx = TypeCollectionContext::new();
    let types = collect_types_from_file(path.as_ref(), &mut ctx)?;
    render_proto_types(&types)
}

// ── Import resolution (same pattern as rust_codegen / typescript_codegen) ──

#[derive(Default)]
struct TypeCollectionContext {
    cache: HashMap<PathBuf, BTreeMap<String, JsonValue>>,
    stack: Vec<PathBuf>,
}

impl TypeCollectionContext {
    fn new() -> Self {
        Self::default()
    }
}

fn collect_types_from_file(
    path: &Path,
    ctx: &mut TypeCollectionContext,
) -> Result<BTreeMap<String, JsonValue>, SyamlError> {
    let canonical = canonicalize_path(path)?;

    if let Some(cached) = ctx.cache.get(&canonical) {
        return Ok(cached.clone());
    }

    if let Some(index) = ctx.stack.iter().position(|p| p == &canonical) {
        let mut chain: Vec<String> = ctx.stack[index..]
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        chain.push(canonical.display().to_string());
        return Err(SyamlError::ImportError(format!(
            "cyclic import detected: {}",
            chain.join(" -> ")
        )));
    }

    let input = fs::read_to_string(&canonical).map_err(|e| {
        SyamlError::ImportError(format!(
            "failed to read import '{}': {e}",
            canonical.display()
        ))
    })?;

    ctx.stack.push(canonical.clone());
    let base_dir = canonical.parent().ok_or_else(|| {
        SyamlError::ImportError(format!(
            "failed to resolve parent directory for '{}'",
            canonical.display()
        ))
    })?;

    let parsed = parse_document(&input)?;
    let exported = collect_exported_types(parsed, base_dir, ctx);

    ctx.stack.pop();

    let exported = exported?;
    ctx.cache.insert(canonical, exported.clone());
    Ok(exported)
}

fn collect_exported_types(
    parsed: crate::ast::ParsedDocument,
    base_dir: &Path,
    ctx: &mut TypeCollectionContext,
) -> Result<BTreeMap<String, JsonValue>, SyamlError> {
    let mut types = parsed.schema.types;

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
            for (type_name, schema) in imported {
                let namespaced = format!("{alias}.{type_name}");
                if types.contains_key(&namespaced) {
                    return Err(SyamlError::ImportError(format!(
                        "imported type '{namespaced}' conflicts with existing type"
                    )));
                }
                types.insert(namespaced, schema);
            }
        }
    }

    Ok(types)
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

// ── Proto3 rendering ────────────────────────────────────────────────────────

fn render_proto_types(types: &BTreeMap<String, JsonValue>) -> Result<String, SyamlError> {
    let mut enums = Vec::new();
    let mut messages = Vec::new();
    let mut skipped = Vec::new();

    for (name, schema) in types {
        let Some(obj) = schema.as_object() else {
            skipped.push(format!(
                "// {name}: not representable as a proto message or enum\n"
            ));
            continue;
        };

        if let Some(variants) = collect_string_enum_variants(obj) {
            enums.push(render_proto_enum(name, &variants));
        } else if is_object_schema(obj) {
            if obj.get("properties").and_then(JsonValue::as_object).is_some() {
                messages.push(render_proto_message(name, obj)?);
            } else if let Some(value_schema) = obj.get("values") {
                // map<string, T> — emit as a message alias note; not a standalone message
                let value_type = proto_scalar_type(value_schema, types);
                skipped.push(format!(
                    "// {name}: map<string, {value_type}> — not directly representable as a proto message\n"
                ));
            } else {
                skipped.push(format!(
                    "// {name}: untyped object — not representable as a proto message\n"
                ));
            }
        } else {
            skipped.push(format!(
                "// {name}: primitive/array type — not representable as a proto message\n"
            ));
        }
    }

    let mut out = String::new();
    out.push_str("syntax = \"proto3\";\n");
    out.push('\n');
    out.push_str("// Generated by super-yaml (first-pass proto codegen).\n");
    out.push_str("// WARNING: This file is generated. Do not edit manually.\n");

    if !skipped.is_empty() {
        out.push('\n');
        for s in &skipped {
            out.push_str(s);
        }
    }

    if !enums.is_empty() {
        out.push('\n');
        out.push_str("// ── Enums ──────────────────────────────────────────────────────────────\n");
        out.push('\n');
        for e in &enums {
            out.push_str(e);
            out.push('\n');
        }
    }

    if !messages.is_empty() {
        out.push('\n');
        out.push_str(
            "// ── Messages ───────────────────────────────────────────────────────────\n",
        );
        out.push('\n');
        for m in &messages {
            out.push_str(m);
            out.push('\n');
        }
    }

    Ok(out)
}

// ── Enum rendering ──────────────────────────────────────────────────────────

fn collect_string_enum_variants(schema_obj: &JsonMap<String, JsonValue>) -> Option<Vec<String>> {
    let values = schema_obj.get("enum")?.as_array()?;
    if values.is_empty() {
        return None;
    }
    // All values must be strings for a proto enum
    values
        .iter()
        .map(|v| v.as_str().map(str::to_owned))
        .collect()
}

fn render_proto_enum(name: &str, variants: &[String]) -> String {
    let prefix = screaming_snake_case(name);
    let unspecified = format!("{prefix}_UNSPECIFIED");

    let mut out = String::new();
    out.push_str(&format!("enum {name} {{\n"));
    out.push_str(&format!("  {unspecified} = 0;\n"));
    for (i, variant) in variants.iter().enumerate() {
        let value_name = format!("{prefix}_{}", screaming_snake_case(variant));
        out.push_str(&format!("  {value_name} = {};\n", i + 1));
    }
    out.push_str("}\n");
    out
}

// ── Message rendering ───────────────────────────────────────────────────────

fn is_object_schema(schema_obj: &JsonMap<String, JsonValue>) -> bool {
    matches!(
        schema_obj.get("type").and_then(JsonValue::as_str),
        Some("object") | None
    )
}

fn render_proto_message(
    name: &str,
    schema_obj: &JsonMap<String, JsonValue>,
) -> Result<String, SyamlError> {
    let properties = schema_obj
        .get("properties")
        .and_then(JsonValue::as_object)
        .unwrap(); // caller guarantees this exists

    let mut fields: Vec<(u64, String)> = Vec::new();
    let mut reserved_lines: Vec<String> = Vec::new();
    let mut unsupported: Vec<String> = Vec::new();

    for (prop_name, prop_schema) in properties {
        let Some(prop_obj) = prop_schema.as_object() else {
            unsupported.push(format!("  // {prop_name}: unsupported (non-object schema)\n"));
            continue;
        };

        let meta = parse_field_version_meta(prop_schema).map_err(|e| {
            SyamlError::SchemaError(format!(
                "proto codegen: type '{name}' property '{prop_name}': {e}"
            ))
        })?;

        // Handle removed fields: emit reserved, skip field body
        if let Some(meta) = &meta {
            if meta.removed.is_some() {
                if let Some(fn_num) = meta.field_number {
                    reserved_lines.push(format!("  reserved {fn_num};\n"));
                    reserved_lines.push(format!("  reserved \"{prop_name}\";\n"));
                }
                // If no field_number, just note it as removed without a reservation
                continue;
            }
        }

        // Require field_number for non-removed fields
        let field_number = meta
            .as_ref()
            .and_then(|m| m.field_number)
            .ok_or_else(|| {
                SyamlError::SchemaError(format!(
                    "proto codegen: type '{name}' property '{prop_name}' is missing field_number"
                ))
            })?;

        let is_optional = prop_obj
            .get("optional")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);

        let is_deprecated = meta
            .as_ref()
            .map(|m| m.deprecated.is_some())
            .unwrap_or(false);

        let field_line =
            render_proto_field(prop_name, prop_schema, prop_obj, field_number, is_optional, is_deprecated);

        match field_line {
            Ok(line) => fields.push((field_number, line)),
            Err(msg) => unsupported.push(format!("  // {prop_name}: {msg}\n")),
        }
    }

    // Sort fields by field number
    fields.sort_by_key(|(n, _)| *n);

    let mut out = String::new();
    out.push_str(&format!("message {name} {{\n"));

    for s in &unsupported {
        out.push_str(s);
    }

    for (_, line) in &fields {
        out.push_str(line);
    }

    for r in &reserved_lines {
        out.push_str(r);
    }

    out.push_str("}\n");
    Ok(out)
}

fn render_proto_field(
    name: &str,
    prop_schema: &JsonValue,
    prop_obj: &JsonMap<String, JsonValue>,
    field_number: u64,
    is_optional: bool,
    is_deprecated: bool,
) -> Result<String, &'static str> {
    let type_str = prop_obj.get("type").and_then(JsonValue::as_str);

    let (qualifier, proto_type) = match type_str {
        Some("array") => {
            let items = prop_obj.get("items").unwrap_or(&JsonValue::Null);
            let inner = proto_type_for_schema(items, prop_obj);
            let inner = inner.ok_or("unsupported array item type")?;
            ("repeated".to_string(), inner)
        }
        Some("object") if prop_obj.get("values").is_some() => {
            let value_schema = prop_obj.get("values").unwrap();
            let value_type = proto_type_for_schema(value_schema, prop_obj)
                .ok_or("unsupported map value type")?;
            let map_type = format!("map<string, {value_type}>");
            let deprecated_opt = if is_deprecated { " [deprecated = true]" } else { "" };
            return Ok(format!(
                "  {map_type} {name} = {field_number}{deprecated_opt};\n"
            ));
        }
        Some("union") | Some("null") => return Err("unsupported type (union/null)"),
        _ => {
            // Check for inline object (has "properties" but no named type)
            if prop_obj.get("properties").is_some() {
                return Err("unsupported inline object type");
            }
            let proto_t = proto_type_for_schema(prop_schema, prop_obj)
                .ok_or("unsupported type")?;
            let q = if is_optional && is_scalar_proto_type(&proto_t) {
                "optional".to_string()
            } else {
                String::new()
            };
            (q, proto_t)
        }
    };

    let deprecated_opt = if is_deprecated { " [deprecated = true]" } else { "" };

    let line = if qualifier.is_empty() {
        format!("  {proto_type} {name} = {field_number}{deprecated_opt};\n")
    } else {
        format!("  {qualifier} {proto_type} {name} = {field_number}{deprecated_opt};\n")
    };

    Ok(line)
}

fn proto_type_for_schema(
    schema: &JsonValue,
    _prop_obj: &JsonMap<String, JsonValue>,
) -> Option<String> {
    let obj = schema.as_object()?;
    let type_str = obj.get("type").and_then(JsonValue::as_str);

    match type_str {
        Some("string") => Some("string".to_string()),
        Some("integer") => Some("int64".to_string()),
        Some("number") => Some("double".to_string()),
        Some("boolean") => Some("bool".to_string()),
        Some("array") => {
            // nested array — not standard proto3, skip
            None
        }
        Some(named_type) => {
            // Named type reference
            Some(proto_message_name(named_type))
        }
        None => {
            // Could be a named type reference via the "type" key not present
            // but "properties" exists → inline object → unsupported
            if obj.get("properties").is_some() {
                None
            } else {
                None
            }
        }
    }
}

/// Same as proto_type_for_schema but works on a raw JsonValue for scalar lookups.
fn proto_scalar_type(schema: &JsonValue, _types: &BTreeMap<String, JsonValue>) -> String {
    let Some(obj) = schema.as_object() else {
        return "bytes".to_string();
    };
    match obj.get("type").and_then(JsonValue::as_str) {
        Some("string") => "string".to_string(),
        Some("integer") => "int64".to_string(),
        Some("number") => "double".to_string(),
        Some("boolean") => "bool".to_string(),
        Some(t) => proto_message_name(t),
        None => "bytes".to_string(),
    }
}

fn is_scalar_proto_type(t: &str) -> bool {
    matches!(t, "string" | "int64" | "double" | "bool" | "bytes" | "int32" | "uint64" | "float")
}

fn proto_message_name(name: &str) -> String {
    // Strip namespace prefix (e.g. "lib.Foo" → "Foo" for now)
    let base = name.rsplit('.').next().unwrap_or(name);
    base.to_string()
}

// ── Naming helpers ──────────────────────────────────────────────────────────

fn screaming_snake_case(s: &str) -> String {
    let mut out = String::new();
    let mut prev_lower = false;
    for ch in s.chars() {
        if ch == '-' || ch == '_' || ch == '.' {
            if !out.is_empty() {
                out.push('_');
            }
            prev_lower = false;
        } else if ch.is_uppercase() && prev_lower {
            out.push('_');
            out.extend(ch.to_uppercase());
            prev_lower = false;
        } else {
            out.extend(ch.to_uppercase());
            prev_lower = ch.is_lowercase();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::screaming_snake_case;

    #[test]
    fn screaming_snake_cases_camel() {
        assert_eq!(screaming_snake_case("MessageKind"), "MESSAGE_KIND");
        assert_eq!(screaming_snake_case("blogPost"), "BLOG_POST");
        assert_eq!(screaming_snake_case("join"), "JOIN");
        assert_eq!(screaming_snake_case("draft_title"), "DRAFT_TITLE");
    }
}
