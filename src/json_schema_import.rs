//! JSON Schema to super_yaml schema conversion.
//!
//! Reads a JSON Schema document and produces a `.syaml` file with a `---schema`
//! section that mirrors the JSON Schema structure using super_yaml's type syntax.

use std::fs;
use std::path::Path;

use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::SyamlError;

// ── Public API ─────────────────────────────────────────────────────────────

/// Converts a JSON Schema string to a `.syaml` document string.
pub fn from_json_schema(input: &str) -> Result<String, SyamlError> {
    let root: JsonValue = serde_json::from_str(input)
        .map_err(|e| SyamlError::SchemaError(format!("invalid JSON: {e}")))?;
    convert_json_schema_document(&root)
}

/// Converts a JSON Schema file to a `.syaml` document string.
pub fn from_json_schema_path(path: impl AsRef<Path>) -> Result<String, SyamlError> {
    let input = fs::read_to_string(path.as_ref()).map_err(|e| {
        SyamlError::SchemaError(format!(
            "failed to read '{}': {e}",
            path.as_ref().display()
        ))
    })?;
    from_json_schema(&input)
}

// ── Conversion context ──────────────────────────────────────────────────────

struct ConversionContext {
    /// All $defs / definitions from the root schema.
    defs: JsonMap<String, JsonValue>,
    /// Ordered list of named types to emit (preserves insertion order).
    named_types: Vec<(String, SchemaEntry)>,
    /// Counter for anonymous inline-promoted types (reserved for future use).
    #[allow(dead_code)]
    anon_counter: usize,
    /// Stack of currently-resolving $ref names (cycle detection).
    ref_stack: Vec<String>,
}

impl ConversionContext {
    fn new(defs: JsonMap<String, JsonValue>) -> Self {
        Self {
            defs,
            named_types: Vec::new(),
            anon_counter: 0,
            ref_stack: Vec::new(),
        }
    }

    fn push_named_type(&mut self, name: String, entry: SchemaEntry) {
        // Don't add duplicates (e.g. from $defs already processed).
        if !self.named_types.iter().any(|(n, _)| n == &name) {
            self.named_types.push((name, entry));
        }
    }

    #[allow(dead_code)]
    fn fresh_anon_name(&mut self, hint: &str) -> String {
        self.anon_counter += 1;
        format!("{hint}_{}", self.anon_counter)
    }
}

struct SchemaEntry {
    /// The syaml-representation of this schema (JsonValue).
    schema: JsonValue,
    /// Optional description for YAML comment rendering.
    description: Option<String>,
}

// ── Top-level conversion ────────────────────────────────────────────────────

fn convert_json_schema_document(root: &JsonValue) -> Result<String, SyamlError> {
    let obj = root.as_object().ok_or_else(|| {
        SyamlError::SchemaError("JSON Schema root must be an object".to_string())
    })?;

    // Collect $defs and definitions into one map.
    let mut defs = JsonMap::new();
    if let Some(JsonValue::Object(d)) = obj.get("$defs") {
        for (k, v) in d {
            defs.insert(k.clone(), v.clone());
        }
    }
    if let Some(JsonValue::Object(d)) = obj.get("definitions") {
        for (k, v) in d {
            defs.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }

    let mut ctx = ConversionContext::new(defs.clone());

    // Process each def as a named type.
    let def_keys: Vec<String> = defs.keys().cloned().collect();
    for name in def_keys {
        if ctx.defs.contains_key(&name) {
            let schema = ctx.defs[&name].clone();
            let entry = convert_schema_to_syaml(&schema, &name, &mut ctx)?;
            ctx.push_named_type(name, entry);
        }
    }

    // Process the root schema itself.
    let root_name = obj
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("Root")
        .to_string();

    // Only emit root if it has meaningful content beyond $defs/definitions.
    let has_root_content = obj.keys().any(|k| {
        !matches!(
            k.as_str(),
            "$schema" | "$defs" | "definitions" | "title" | "$id"
        )
    });

    if has_root_content {
        let entry = convert_schema_to_syaml(root, &root_name, &mut ctx)?;
        ctx.push_named_type(root_name, entry);
    }

    let doc = render_syaml_document(&ctx.named_types);
    Ok(doc)
}

// ── Schema conversion ───────────────────────────────────────────────────────

fn convert_schema_to_syaml(
    schema: &JsonValue,
    hint_name: &str,
    ctx: &mut ConversionContext,
) -> Result<SchemaEntry, SyamlError> {
    let description = schema
        .get("description")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let syaml_schema = convert_schema_inner(schema, hint_name, ctx)?;

    Ok(SchemaEntry {
        schema: syaml_schema,
        description,
    })
}

fn convert_schema_inner(
    schema: &JsonValue,
    hint_name: &str,
    ctx: &mut ConversionContext,
) -> Result<JsonValue, SyamlError> {
    // Handle boolean schema
    if let Some(b) = schema.as_bool() {
        if b {
            return Ok(json_str("object"));
        } else {
            return Ok(json_str("null"));
        }
    }

    let obj = match schema.as_object() {
        Some(o) => o,
        None => return Ok(json_str("object")),
    };

    // $ref
    if let Some(ref_val) = obj.get("$ref") {
        if let Some(ref_str) = ref_val.as_str() {
            let name = resolve_ref_name(ref_str);
            // Ensure the ref target is converted.
            ensure_ref_converted(&name, ctx)?;
            return Ok(json_str(&name));
        }
    }

    // enum
    if let Some(enum_val) = obj.get("enum") {
        return convert_enum(enum_val);
    }

    // const
    if let Some(const_val) = obj.get("const") {
        return convert_const(const_val);
    }

    // anyOf / oneOf
    if let Some(any_of) = obj.get("anyOf").or_else(|| obj.get("oneOf")) {
        return convert_union(any_of, hint_name, ctx);
    }

    // allOf
    if let Some(all_of) = obj.get("allOf") {
        return convert_all_of(all_of, hint_name, ctx);
    }

    // type field
    if let Some(type_val) = obj.get("type") {
        match type_val {
            JsonValue::String(t) => {
                return convert_typed_schema(t, obj, hint_name, ctx);
            }
            JsonValue::Array(types) => {
                return convert_type_array(types, obj, hint_name, ctx);
            }
            _ => {}
        }
    }

    // No explicit type — try to infer from shape
    if obj.contains_key("properties") || obj.contains_key("additionalProperties") {
        return convert_typed_schema("object", obj, hint_name, ctx);
    }

    // Fallback: emit as-is with type: object
    Ok(json_str("object"))
}

fn convert_typed_schema(
    type_str: &str,
    obj: &JsonMap<String, JsonValue>,
    hint_name: &str,
    ctx: &mut ConversionContext,
) -> Result<JsonValue, SyamlError> {
    match type_str {
        "string" => convert_string_schema(obj),
        "integer" | "number" => convert_numeric_schema(type_str, obj),
        "boolean" => Ok(json_str("boolean")),
        "null" => Ok(json_str("null")),
        "object" => convert_object_schema(obj, hint_name, ctx),
        "array" => convert_array_schema(obj, hint_name, ctx),
        other => Ok(json_str(other)),
    }
}

fn convert_string_schema(obj: &JsonMap<String, JsonValue>) -> Result<JsonValue, SyamlError> {
    let mut out = JsonMap::new();

    // format → pattern + comment
    let (format_comment, format_pattern) = if let Some(fmt) = obj.get("format").and_then(|v| v.as_str()) {
        match fmt {
            "email" => (
                Some("JSON Schema format: email".to_string()),
                Some(r"^[^@]+@[^@]+\.[^@]+$".to_string()),
            ),
            "uri" | "url" => (
                Some(format!("JSON Schema format: {fmt}")),
                Some(r"^https?://".to_string()),
            ),
            "date-time" => (
                Some("JSON Schema format: date-time".to_string()),
                Some(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}".to_string()),
            ),
            "date" => (
                Some("JSON Schema format: date".to_string()),
                Some(r"^\d{4}-\d{2}-\d{2}$".to_string()),
            ),
            "time" => (
                Some("JSON Schema format: time".to_string()),
                Some(r"^\d{2}:\d{2}:\d{2}".to_string()),
            ),
            other => (Some(format!("JSON Schema format: {other}")), None),
        }
    } else {
        (None, None)
    };

    let has_constraints = obj.contains_key("minLength")
        || obj.contains_key("maxLength")
        || obj.contains_key("pattern")
        || format_pattern.is_some()
        || format_comment.is_some();

    if !has_constraints {
        return Ok(json_str("string"));
    }

    out.insert("type".to_string(), json_str("string"));

    if let Some(v) = obj.get("minLength") {
        out.insert("minLength".to_string(), v.clone());
    }
    if let Some(v) = obj.get("maxLength") {
        out.insert("maxLength".to_string(), v.clone());
    }
    if let Some(v) = obj.get("pattern") {
        out.insert("pattern".to_string(), v.clone());
    } else if let Some(p) = format_pattern {
        out.insert("pattern".to_string(), JsonValue::String(p));
    }

    if let Some(comment) = format_comment {
        out.insert("_format_comment".to_string(), JsonValue::String(comment));
    }

    Ok(JsonValue::Object(out))
}

fn convert_numeric_schema(
    type_str: &str,
    obj: &JsonMap<String, JsonValue>,
) -> Result<JsonValue, SyamlError> {
    let has_constraints = obj.contains_key("minimum")
        || obj.contains_key("maximum")
        || obj.contains_key("exclusiveMinimum")
        || obj.contains_key("exclusiveMaximum")
        || obj.contains_key("multipleOf");

    if !has_constraints {
        return Ok(json_str(type_str));
    }

    let mut out = JsonMap::new();
    out.insert("type".to_string(), json_str(type_str));
    for key in &["minimum", "maximum", "exclusiveMinimum", "exclusiveMaximum", "multipleOf"] {
        if let Some(v) = obj.get(*key) {
            out.insert(key.to_string(), v.clone());
        }
    }
    Ok(JsonValue::Object(out))
}

fn convert_object_schema(
    obj: &JsonMap<String, JsonValue>,
    hint_name: &str,
    ctx: &mut ConversionContext,
) -> Result<JsonValue, SyamlError> {
    let required: Vec<String> = obj
        .get("required")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    // additionalProperties as map type (no properties key)
    if !obj.contains_key("properties") {
        if let Some(ap) = obj.get("additionalProperties") {
            if let Ok(val_type) = convert_schema_inner(ap, &format!("{hint_name}Value"), ctx) {
                let mut out = JsonMap::new();
                out.insert("type".to_string(), json_str("object"));
                out.insert("values".to_string(), val_type);
                return Ok(JsonValue::Object(out));
            }
        }
        // Empty object
        let mut out = JsonMap::new();
        out.insert("type".to_string(), json_str("object"));
        return Ok(JsonValue::Object(out));
    }

    let props = obj["properties"].as_object().map(|m| m.clone()).unwrap_or_default();

    let mut out = JsonMap::new();
    out.insert("type".to_string(), json_str("object"));

    let mut props_out = JsonMap::new();
    for (field_name, field_schema) in &props {
        let is_required = required.contains(field_name);
        let prop_hint = format!("{hint_name}_{field_name}");
        let prop_entry = convert_property(field_schema, &prop_hint, is_required, ctx)?;
        props_out.insert(field_name.clone(), prop_entry);
    }

    out.insert("properties".to_string(), JsonValue::Object(props_out));
    Ok(JsonValue::Object(out))
}

fn convert_property(
    schema: &JsonValue,
    hint_name: &str,
    is_required: bool,
    ctx: &mut ConversionContext,
) -> Result<JsonValue, SyamlError> {
    // Check for nullable type array like ["string","null"]
    let (inner_schema, nullable) = extract_nullable(schema);
    let optional = !is_required || nullable;

    let obj = inner_schema.as_object();
    let description = inner_schema
        .get("description")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    // Try to resolve to an inline value first.
    let syaml = convert_schema_inner(&inner_schema, hint_name, ctx)?;

    // If the property is a complex object with many fields, promote to named type.
    let should_promote = should_promote_to_named_type(obj, &syaml);

    let syaml = if should_promote {
        // Already converted inline - now register it as a named type.
        let type_name = hint_name.to_string();
        let entry = SchemaEntry {
            schema: syaml.clone(),
            description: description.clone(),
        };
        ctx.push_named_type(type_name.clone(), entry);
        json_str(&type_name)
    } else {
        syaml
    };

    // Wrap with optional and description if needed.
    if optional || description.is_some() {
        if let Some(inline) = can_inline(&syaml) {
            // Can use shorthand: TypeName? or include description separately
            if optional && description.is_none() {
                return Ok(JsonValue::String(format!("{inline}?")));
            }
            // Need block form for description + optional
            if optional || description.is_some() {
                let mut out = JsonMap::new();
                // Extract type info from inline
                out.insert("type".to_string(), json_str(&inline));
                if optional {
                    out.insert("optional".to_string(), JsonValue::Bool(true));
                }
                return Ok(JsonValue::Object(out));
            }
        }
        // Block form
        if let JsonValue::Object(mut m) = syaml {
            if optional {
                m.insert("optional".to_string(), JsonValue::Bool(true));
            }
            return Ok(JsonValue::Object(m));
        }
        // Fallback: wrap in object
        let mut out = JsonMap::new();
        out.insert("type".to_string(), syaml);
        if optional {
            out.insert("optional".to_string(), JsonValue::Bool(true));
        }
        return Ok(JsonValue::Object(out));
    }

    Ok(syaml)
}

fn should_promote_to_named_type(
    original_obj: Option<&JsonMap<String, JsonValue>>,
    _syaml: &JsonValue,
) -> bool {
    let Some(obj) = original_obj else {
        return false;
    };
    let typ = obj.get("type").and_then(|v| v.as_str());
    if typ != Some("object") {
        return false;
    }
    let props = obj.get("properties").and_then(|v| v.as_object());
    let Some(props) = props else {
        return false;
    };

    // Promote if > 3 properties
    if props.len() > 3 {
        return true;
    }

    // Promote if any property is itself an object
    for prop_schema in props.values() {
        let inner_type = prop_schema.get("type").and_then(|v| v.as_str());
        if inner_type == Some("object") {
            return true;
        }
        // Also check if the syaml block contains nested objects
        if prop_schema.get("properties").is_some() {
            return true;
        }
    }

    false
}

fn extract_nullable(schema: &JsonValue) -> (JsonValue, bool) {
    let obj = match schema.as_object() {
        Some(o) => o,
        None => return (schema.clone(), false),
    };

    // Check type array like ["string", "null"]
    if let Some(JsonValue::Array(types)) = obj.get("type") {
        let non_null: Vec<&JsonValue> = types.iter().filter(|v| v.as_str() != Some("null")).collect();
        let has_null = types.iter().any(|v| v.as_str() == Some("null"));
        if has_null && non_null.len() == 1 {
            let mut new_obj = obj.clone();
            if let Some(t) = non_null[0].as_str() {
                new_obj.insert("type".to_string(), JsonValue::String(t.to_string()));
            } else {
                new_obj.insert("type".to_string(), non_null[0].clone());
            }
            return (JsonValue::Object(new_obj), true);
        }
    }

    // anyOf with null
    if let Some(JsonValue::Array(any_of)) = obj.get("anyOf") {
        let non_null: Vec<&JsonValue> = any_of.iter().filter(|v| {
            v.get("type").and_then(|t| t.as_str()) != Some("null")
        }).collect();
        let has_null = any_of.iter().any(|v| {
            v.get("type").and_then(|t| t.as_str()) == Some("null")
        });
        if has_null && non_null.len() == 1 {
            return ((*non_null[0]).clone(), true);
        }
    }

    (schema.clone(), false)
}

fn convert_array_schema(
    obj: &JsonMap<String, JsonValue>,
    hint_name: &str,
    ctx: &mut ConversionContext,
) -> Result<JsonValue, SyamlError> {
    let mut out = JsonMap::new();
    out.insert("type".to_string(), json_str("array"));

    if let Some(items) = obj.get("items") {
        let items_hint = format!("{hint_name}Item");
        let items_syaml = convert_schema_inner(items, &items_hint, ctx)?;
        out.insert("items".to_string(), items_syaml);
    }

    if let Some(v) = obj.get("minItems") {
        out.insert("minItems".to_string(), v.clone());
    }
    if let Some(v) = obj.get("maxItems") {
        out.insert("maxItems".to_string(), v.clone());
    }

    Ok(JsonValue::Object(out))
}

fn convert_enum(enum_val: &JsonValue) -> Result<JsonValue, SyamlError> {
    let arr = enum_val
        .as_array()
        .ok_or_else(|| SyamlError::SchemaError("enum must be an array".to_string()))?;

    // All-string shorthand: emit as JsonValue::Array of strings.
    let all_strings = arr.iter().all(|v| v.is_string());
    if all_strings {
        let strs: Vec<JsonValue> = arr
            .iter()
            .map(|v| JsonValue::String(v.as_str().unwrap().to_string()))
            .collect();
        return Ok(JsonValue::Array(strs));
    }

    // Mixed enum: emit as object with enum key
    let mut out = JsonMap::new();
    out.insert("enum".to_string(), enum_val.clone());
    Ok(JsonValue::Object(out))
}

fn convert_const(const_val: &JsonValue) -> Result<JsonValue, SyamlError> {
    // A const is effectively a single-value enum.
    let arr = JsonValue::Array(vec![const_val.clone()]);
    convert_enum(&arr)
}

fn convert_union(
    options: &JsonValue,
    hint_name: &str,
    ctx: &mut ConversionContext,
) -> Result<JsonValue, SyamlError> {
    let arr = options
        .as_array()
        .ok_or_else(|| SyamlError::SchemaError("anyOf/oneOf must be an array".to_string()))?;

    // Check if this is actually a nullable pattern (T | null)
    let non_null: Vec<&JsonValue> = arr.iter().filter(|v| {
        v.get("type").and_then(|t| t.as_str()) != Some("null")
    }).collect();
    let has_null = arr.iter().any(|v| {
        v.get("type").and_then(|t| t.as_str()) == Some("null")
    });

    if has_null && non_null.len() == 1 {
        let mut entry = convert_schema_inner(non_null[0], hint_name, ctx)?;
        // Mark optional
        if let JsonValue::Object(ref mut m) = entry {
            m.insert("optional".to_string(), JsonValue::Bool(true));
        } else {
            let mut out = JsonMap::new();
            out.insert("type".to_string(), entry);
            out.insert("optional".to_string(), JsonValue::Bool(true));
            entry = JsonValue::Object(out);
        }
        return Ok(entry);
    }

    let mut options_out = Vec::new();
    for (i, option) in arr.iter().enumerate() {
        let opt_hint = format!("{hint_name}_option{i}");
        let opt_syaml = convert_schema_inner(option, &opt_hint, ctx)?;
        options_out.push(opt_syaml);
    }

    let mut out = JsonMap::new();
    out.insert("type".to_string(), json_str("union"));
    out.insert("options".to_string(), JsonValue::Array(options_out));
    Ok(JsonValue::Object(out))
}

fn convert_all_of(
    all_of: &JsonValue,
    hint_name: &str,
    ctx: &mut ConversionContext,
) -> Result<JsonValue, SyamlError> {
    let arr = all_of
        .as_array()
        .ok_or_else(|| SyamlError::SchemaError("allOf must be an array".to_string()))?;

    // Try to merge all object schemas.
    let mut merged_props = JsonMap::new();
    let mut merged_required = Vec::new();
    let mut can_merge = true;

    for member in arr {
        let resolved = if let Some(ref_str) = member.get("$ref").and_then(|v| v.as_str()) {
            let name = resolve_ref_name(ref_str);
            ctx.defs.get(&name).cloned().unwrap_or_else(|| member.clone())
        } else {
            member.clone()
        };

        let resolved_obj = match resolved.as_object() {
            Some(o) => o.clone(),
            None => { can_merge = false; break; }
        };

        // Must be an object type or have properties.
        let is_obj = resolved_obj.get("type").and_then(|v| v.as_str()) == Some("object")
            || resolved_obj.contains_key("properties");
        if !is_obj {
            can_merge = false;
            break;
        }

        if let Some(JsonValue::Object(props)) = resolved_obj.get("properties") {
            for (k, v) in props {
                merged_props.insert(k.clone(), v.clone());
            }
        }
        if let Some(JsonValue::Array(req)) = resolved_obj.get("required") {
            for r in req {
                if let Some(s) = r.as_str() {
                    if !merged_required.contains(&s.to_string()) {
                        merged_required.push(s.to_string());
                    }
                }
            }
        }
    }

    if can_merge {
        let mut merged = JsonMap::new();
        merged.insert("type".to_string(), json_str("object"));
        if !merged_required.is_empty() {
            merged.insert(
                "required".to_string(),
                JsonValue::Array(merged_required.into_iter().map(JsonValue::String).collect()),
            );
        }
        merged.insert("properties".to_string(), JsonValue::Object(merged_props));
        return convert_object_schema(&merged, hint_name, ctx);
    }

    // Fallback: emit object with comment.
    let mut out = JsonMap::new();
    out.insert("type".to_string(), json_str("object"));
    out.insert(
        "_allof_comment".to_string(),
        JsonValue::String("allOf merge not fully supported".to_string()),
    );
    Ok(JsonValue::Object(out))
}

fn convert_type_array(
    types: &[JsonValue],
    obj: &JsonMap<String, JsonValue>,
    hint_name: &str,
    ctx: &mut ConversionContext,
) -> Result<JsonValue, SyamlError> {
    let non_null: Vec<&str> = types
        .iter()
        .filter_map(|v| v.as_str())
        .filter(|&s| s != "null")
        .collect();
    let has_null = types.iter().any(|v| v.as_str() == Some("null"));

    if has_null && non_null.len() == 1 {
        // T | null → optional
        let t = non_null[0];
        let mut new_obj = obj.clone();
        new_obj.insert("type".to_string(), json_str(t));
        let inner = convert_typed_schema(t, &new_obj, hint_name, ctx)?;
        if let Some(inline) = can_inline(&inner) {
            return Ok(JsonValue::String(format!("{inline}?")));
        }
        if let JsonValue::Object(mut m) = inner {
            m.insert("optional".to_string(), JsonValue::Bool(true));
            return Ok(JsonValue::Object(m));
        }
        return Ok(inner);
    }

    // Multiple real types → union
    let mut options = Vec::new();
    for t in &non_null {
        options.push(JsonValue::Object({
            let mut m = JsonMap::new();
            m.insert("type".to_string(), json_str(t));
            m
        }));
    }
    let mut out = JsonMap::new();
    out.insert("type".to_string(), json_str("union"));
    out.insert("options".to_string(), JsonValue::Array(options));
    Ok(JsonValue::Object(out))
}

fn ensure_ref_converted(name: &str, ctx: &mut ConversionContext) -> Result<(), SyamlError> {
    // Already converted?
    if ctx.named_types.iter().any(|(n, _)| n == name) {
        return Ok(());
    }

    // Cycle check
    if ctx.ref_stack.contains(&name.to_string()) {
        return Err(SyamlError::SchemaError(format!(
            "circular $ref detected for '{name}'"
        )));
    }

    let schema = match ctx.defs.get(name).cloned() {
        Some(s) => s,
        None => return Ok(()), // Unknown ref - skip silently
    };

    ctx.ref_stack.push(name.to_string());
    let entry = convert_schema_to_syaml(&schema, name, ctx)?;
    ctx.ref_stack.pop();
    ctx.push_named_type(name.to_string(), entry);
    Ok(())
}

fn resolve_ref_name(ref_str: &str) -> String {
    // "#/$defs/Foo" → "Foo"
    // "#/definitions/Foo" → "Foo"
    if let Some(last) = ref_str.split('/').last() {
        last.to_string()
    } else {
        ref_str.to_string()
    }
}

// ── Inline helpers ──────────────────────────────────────────────────────────

/// Returns Some("typename") if the schema can be expressed as a simple inline string.
fn can_inline(schema: &JsonValue) -> Option<String> {
    match schema {
        JsonValue::String(s) => Some(s.clone()),
        JsonValue::Object(m) => {
            if m.len() == 1 {
                if let Some(JsonValue::String(t)) = m.get("type") {
                    return Some(t.clone());
                }
            }
            None
        }
        _ => None,
    }
}

fn json_str(s: &str) -> JsonValue {
    JsonValue::String(s.to_string())
}

// ── YAML rendering ──────────────────────────────────────────────────────────

fn render_syaml_document(named_types: &[(String, SchemaEntry)]) -> String {
    let mut out = String::new();
    out.push_str("---!syaml/v0\n---schema\n");

    for (i, (name, entry)) in named_types.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        render_type_entry(name, entry, &mut out);
    }

    out
}

fn render_type_entry(name: &str, entry: &SchemaEntry, out: &mut String) {
    // Description as YAML comment.
    if let Some(desc) = &entry.description {
        for line in desc.lines() {
            out.push_str(&format!("# {line}\n"));
        }
    }

    // Top-level enum shorthand: TypeName: [val1, val2]
    if let JsonValue::Array(values) = &entry.schema {
        let items: Vec<String> = values
            .iter()
            .map(|v| match v {
                JsonValue::String(s) => s.clone(),
                _ => v.to_string(),
            })
            .collect();
        out.push_str(&format!("{name}: [{}]\n", items.join(", ")));
        return;
    }

    // Simple inline type.
    if let Some(inline) = can_inline(&entry.schema) {
        out.push_str(&format!("{name}: {inline}\n"));
        return;
    }

    // Block form.
    out.push_str(&format!("{name}:\n"));
    render_schema_block(&entry.schema, 2, out);
}

fn render_schema_block(schema: &JsonValue, indent: usize, out: &mut String) {
    let pad = " ".repeat(indent);

    match schema {
        JsonValue::String(s) => {
            out.push_str(&format!("{pad}type: {s}\n"));
        }
        JsonValue::Object(m) => {
            render_object_block(m, indent, out);
        }
        JsonValue::Array(items) => {
            let strs: Vec<String> = items
                .iter()
                .map(|v| match v {
                    JsonValue::String(s) => s.clone(),
                    _ => v.to_string(),
                })
                .collect();
            out.push_str(&format!("{pad}enum: [{}]\n", strs.join(", ")));
        }
        _ => {
            out.push_str(&format!("{pad}type: object\n"));
        }
    }
}

fn render_object_block(m: &JsonMap<String, JsonValue>, indent: usize, out: &mut String) {
    let pad = " ".repeat(indent);

    // Emit comments for special metadata keys.
    if let Some(JsonValue::String(comment)) = m.get("_format_comment") {
        out.push_str(&format!("{pad}# {comment}\n"));
    }
    if let Some(JsonValue::String(comment)) = m.get("_allof_comment") {
        out.push_str(&format!("{pad}# {comment}\n"));
    }

    // type
    if let Some(t) = m.get("type") {
        if let Some(s) = t.as_str() {
            out.push_str(&format!("{pad}type: {s}\n"));
        }
    }

    // optional
    if let Some(JsonValue::Bool(true)) = m.get("optional") {
        out.push_str(&format!("{pad}optional: true\n"));
    }

    // items (array)
    if let Some(items) = m.get("items") {
        if let Some(inline) = can_inline(items) {
            out.push_str(&format!("{pad}items: {inline}\n"));
        } else {
            out.push_str(&format!("{pad}items:\n"));
            render_schema_block(items, indent + 2, out);
        }
    }

    // values (map type)
    if let Some(values) = m.get("values") {
        if let Some(inline) = can_inline(values) {
            out.push_str(&format!("{pad}values: {inline}\n"));
        } else {
            out.push_str(&format!("{pad}values:\n"));
            render_schema_block(values, indent + 2, out);
        }
    }

    // enum
    if let Some(JsonValue::Array(items)) = m.get("enum") {
        let strs: Vec<String> = items
            .iter()
            .map(|v| match v {
                JsonValue::String(s) => s.clone(),
                _ => v.to_string(),
            })
            .collect();
        out.push_str(&format!("{pad}enum: [{}]\n", strs.join(", ")));
    }

    // union options
    if let Some(JsonValue::Array(options)) = m.get("options") {
        out.push_str(&format!("{pad}options:\n"));
        for opt in options {
            if let Some(inline) = can_inline(opt) {
                out.push_str(&format!("{pad}  - {inline}\n"));
            } else {
                out.push_str(&format!("{pad}  -\n"));
                render_schema_block(opt, indent + 4, out);
            }
        }
    }

    // properties
    if let Some(JsonValue::Object(props)) = m.get("properties") {
        out.push_str(&format!("{pad}properties:\n"));
        for (field_name, field_schema) in props {
            render_property_entry(field_name, field_schema, indent + 2, out);
        }
    }

    // Numeric/string/array constraints
    for key in &[
        "minimum", "maximum", "exclusiveMinimum", "exclusiveMaximum", "multipleOf",
        "minLength", "maxLength", "pattern",
        "minItems", "maxItems",
    ] {
        if let Some(v) = m.get(*key) {
            match v {
                JsonValue::String(s) => out.push_str(&format!("{pad}{key}: \"{s}\"\n")),
                JsonValue::Number(n) => out.push_str(&format!("{pad}{key}: {n}\n")),
                _ => out.push_str(&format!("{pad}{key}: {v}\n")),
            }
        }
    }
}

fn render_property_entry(
    name: &str,
    schema: &JsonValue,
    indent: usize,
    out: &mut String,
) {
    let pad = " ".repeat(indent);

    // Inline shorthand (e.g. "string", "Foo", "Foo?")
    match schema {
        JsonValue::String(s) => {
            out.push_str(&format!("{pad}{name}: {s}\n"));
            return;
        }
        JsonValue::Object(m) => {
            // Check for optional inline shorthand
            let type_val = m.get("type");
            let is_optional = m.get("optional").and_then(|v| v.as_bool()).unwrap_or(false);
            let only_type_and_optional = m.len() <= 2
                && m.keys().all(|k| k == "type" || k == "optional");

            if only_type_and_optional {
                if let Some(JsonValue::String(t)) = type_val {
                    if is_optional {
                        out.push_str(&format!("{pad}{name}: {t}?\n"));
                    } else {
                        out.push_str(&format!("{pad}{name}: {t}\n"));
                    }
                    return;
                }
            }

            // Block form
            out.push_str(&format!("{pad}{name}:\n"));
            render_object_block(m, indent + 2, out);
        }
        JsonValue::Array(items) => {
            let strs: Vec<String> = items
                .iter()
                .map(|v| match v {
                    JsonValue::String(s) => s.clone(),
                    _ => v.to_string(),
                })
                .collect();
            out.push_str(&format!("{pad}{name}: [{}]\n", strs.join(", ")));
        }
        _ => {
            out.push_str(&format!("{pad}{name}: object\n"));
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn convert(input: &str) -> String {
        from_json_schema(input).expect("conversion failed")
    }

    #[test]
    fn primitive_string_type() {
        let out = convert(r#"{"title":"MyStr","type":"string"}"#);
        assert!(out.contains("MyStr: string"), "got: {out}");
    }

    #[test]
    fn primitive_integer_type() {
        let out = convert(r#"{"title":"MyInt","type":"integer"}"#);
        assert!(out.contains("MyInt: integer"), "got: {out}");
    }

    #[test]
    fn primitive_boolean_type() {
        let out = convert(r#"{"title":"MyBool","type":"boolean"}"#);
        assert!(out.contains("MyBool: boolean"), "got: {out}");
    }

    #[test]
    fn numeric_constraints_passthrough() {
        let out = convert(r#"{"$defs":{"Port":{"type":"integer","minimum":1,"maximum":65535}}}"#);
        assert!(out.contains("Port:"), "got: {out}");
        assert!(out.contains("minimum: 1"), "got: {out}");
        assert!(out.contains("maximum: 65535"), "got: {out}");
    }

    #[test]
    fn string_constraints_passthrough() {
        let out = convert(
            r#"{"$defs":{"Name":{"type":"string","minLength":1,"maxLength":100,"pattern":"^[a-z]+"}}}"#,
        );
        assert!(out.contains("Name:"), "got: {out}");
        assert!(out.contains("minLength: 1"), "got: {out}");
        assert!(out.contains("maxLength: 100"), "got: {out}");
        assert!(out.contains("pattern:"), "got: {out}");
    }

    #[test]
    fn all_string_enum_shorthand() {
        let out = convert(r#"{"$defs":{"Color":{"enum":["red","green","blue"]}}}"#);
        assert!(out.contains("Color: [red, green, blue]"), "got: {out}");
    }

    #[test]
    fn object_with_required_marks_optional() {
        let out = convert(r#"{
            "$defs": {
                "User": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "integer"},
                        "name": {"type": "string"},
                        "email": {"type": "string"}
                    },
                    "required": ["id", "name"]
                }
            }
        }"#);
        // email is not required → should be marked optional
        assert!(out.contains("email: string?") || out.contains("optional: true"), "got: {out}");
        // id and name should not be optional
        assert!(out.contains("id: integer"), "got: {out}");
        assert!(out.contains("name: string"), "got: {out}");
    }

    #[test]
    fn ref_resolution() {
        let out = convert(r##"{
            "$defs": {
                "Tag": {"type": "string"},
                "Item": {
                    "type": "object",
                    "properties": {
                        "tag": {"$ref": "#/$defs/Tag"}
                    },
                    "required": ["tag"]
                }
            }
        }"##);
        assert!(out.contains("tag: Tag"), "got: {out}");
    }

    #[test]
    fn any_of_union() {
        let out = convert(r#"{
            "$defs": {
                "Val": {
                    "anyOf": [
                        {"type": "string"},
                        {"type": "integer"}
                    ]
                }
            }
        }"#);
        assert!(out.contains("type: union"), "got: {out}");
    }

    #[test]
    fn nullable_type_array() {
        let out = convert(r#"{"$defs":{"MaybeStr":{"type":["string","null"]}}}"#);
        // Should render as optional
        assert!(
            out.contains("string?") || out.contains("optional: true"),
            "got: {out}"
        );
    }

    #[test]
    fn description_becomes_comment() {
        let out = convert(
            r#"{"$defs":{"Port":{"type":"integer","description":"The TCP port number"}}}"#,
        );
        assert!(out.contains("# The TCP port number"), "got: {out}");
    }

    #[test]
    fn array_schema_with_items() {
        let out = convert(r#"{
            "$defs": {
                "Tags": {
                    "type": "array",
                    "items": {"type": "string"},
                    "minItems": 1,
                    "maxItems": 10
                }
            }
        }"#);
        assert!(out.contains("type: array"), "got: {out}");
        assert!(out.contains("items: string"), "got: {out}");
        assert!(out.contains("minItems: 1"), "got: {out}");
        assert!(out.contains("maxItems: 10"), "got: {out}");
    }

    #[test]
    fn format_email_adds_pattern() {
        let out = convert(r#"{"$defs":{"Email":{"type":"string","format":"email"}}}"#);
        assert!(out.contains("pattern:"), "got: {out}");
        assert!(out.contains("# JSON Schema format: email"), "got: {out}");
    }

    #[test]
    fn document_has_header() {
        let out = convert(r#"{"$defs":{}}"#);
        assert!(out.starts_with("---!syaml/v0\n---schema\n"), "got: {out}");
    }
}
