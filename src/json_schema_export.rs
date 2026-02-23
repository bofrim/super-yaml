//! super_yaml schema → JSON Schema (draft 2020-12) export.
//!
//! Converts the named types in a SYAML `---schema` section into a standard
//! JSON Schema document with a `$defs` map.

use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::ast::SchemaDoc;
use crate::SyamlError;

/// SYAML-only keys that have no JSON Schema equivalent and must be stripped.
const STRIP_KEYS: &[&str] = &[
    "constructors",
    "constraints",
    "since",
    "deprecated",
    "removed",
    "field_number",
    "optional",
];

// ── Public API ─────────────────────────────────────────────────────────────

/// Converts a parsed SYAML [`SchemaDoc`] into a JSON Schema document string.
///
/// The output conforms to JSON Schema draft 2020-12. All named types appear
/// under the `$defs` key. Set `pretty` to `true` for indented output.
pub fn to_json_schema(schema: &SchemaDoc, pretty: bool) -> Result<String, SyamlError> {
    let mut defs = JsonMap::new();
    for (name, type_value) in &schema.types {
        defs.insert(name.clone(), convert_syaml_type(type_value));
    }

    let mut root = JsonMap::new();
    root.insert(
        "$schema".to_string(),
        JsonValue::String("https://json-schema.org/draft/2020-12/schema".to_string()),
    );
    root.insert("$defs".to_string(), JsonValue::Object(defs));

    let root_value = JsonValue::Object(root);
    if pretty {
        serde_json::to_string_pretty(&root_value)
            .map_err(|e| SyamlError::SerializationError(e.to_string()))
    } else {
        serde_json::to_string(&root_value)
            .map_err(|e| SyamlError::SerializationError(e.to_string()))
    }
}

// ── Conversion helpers ──────────────────────────────────────────────────────

fn convert_syaml_type(value: &JsonValue) -> JsonValue {
    match value {
        JsonValue::String(s) => convert_string_type(s),
        JsonValue::Object(map) => convert_object_type(map),
        // Arrays/other values are passed through (unlikely in schema position).
        other => other.clone(),
    }
}

/// Convert a bare string schema (e.g. `"string"`, `"MyType"`).
fn convert_string_type(s: &str) -> JsonValue {
    match s {
        "string" | "integer" | "number" | "boolean" | "null" => {
            let mut out = JsonMap::new();
            out.insert("type".to_string(), JsonValue::String(s.to_string()));
            JsonValue::Object(out)
        }
        // Named type reference → $ref
        _ => {
            let mut out = JsonMap::new();
            out.insert(
                "$ref".to_string(),
                JsonValue::String(format!("#/$defs/{s}")),
            );
            JsonValue::Object(out)
        }
    }
}

/// Convert an object-form schema node, dispatching on the `type` field.
fn convert_object_type(map: &JsonMap<String, JsonValue>) -> JsonValue {
    let type_str = map.get("type").and_then(|v| v.as_str());

    match type_str {
        Some("object") => {
            if map.contains_key("values") {
                // Map type: { type: object, values: <schema> }
                convert_map_type(map)
            } else {
                // Struct type: { type: object, properties: {...} }
                convert_struct_type(map)
            }
        }
        Some("array") => convert_array_type(map),
        Some("union") => convert_union_type(map),
        Some(primitive) if is_primitive(primitive) => convert_primitive_type(map, primitive),
        Some(named) => {
            // Named type reference expressed as { type: "MyType" }
            let mut out = JsonMap::new();
            out.insert(
                "$ref".to_string(),
                JsonValue::String(format!("#/$defs/{named}")),
            );
            JsonValue::Object(out)
        }
        None => {
            // Enum or pass-through with SYAML-specific keys stripped.
            if let Some(enum_values) = map.get("enum") {
                let mut out = JsonMap::new();
                out.insert("enum".to_string(), enum_values.clone());
                JsonValue::Object(out)
            } else {
                strip_syaml_keys(map)
            }
        }
    }
}

fn is_primitive(s: &str) -> bool {
    matches!(s, "string" | "integer" | "number" | "boolean" | "null")
}

/// Convert a primitive type node, copying over compatible JSON Schema keywords.
fn convert_primitive_type(map: &JsonMap<String, JsonValue>, primitive: &str) -> JsonValue {
    let mut out = JsonMap::new();
    out.insert("type".to_string(), JsonValue::String(primitive.to_string()));

    for key in &[
        "minimum",
        "maximum",
        "exclusiveMinimum",
        "exclusiveMaximum",
        "minLength",
        "maxLength",
        "pattern",
        "enum",
    ] {
        if let Some(v) = map.get(*key) {
            out.insert((*key).to_string(), v.clone());
        }
    }

    JsonValue::Object(out)
}

/// Convert an object struct type (with `properties`).
fn convert_struct_type(map: &JsonMap<String, JsonValue>) -> JsonValue {
    let mut out = JsonMap::new();
    out.insert("type".to_string(), JsonValue::String("object".to_string()));

    if let Some(props_value) = map.get("properties") {
        if let Some(props_map) = props_value.as_object() {
            let mut json_props = JsonMap::new();
            let mut required_fields: Vec<JsonValue> = Vec::new();

            for (field_name, field_schema) in props_map {
                let is_optional = field_schema
                    .as_object()
                    .and_then(|m| m.get("optional"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                if !is_optional {
                    required_fields.push(JsonValue::String(field_name.clone()));
                }

                json_props.insert(field_name.clone(), convert_syaml_type(field_schema));
            }

            out.insert("properties".to_string(), JsonValue::Object(json_props));
            if !required_fields.is_empty() {
                out.insert("required".to_string(), JsonValue::Array(required_fields));
            }
        }
    }

    if let Some(v) = map.get("additionalProperties") {
        out.insert("additionalProperties".to_string(), v.clone());
    }

    JsonValue::Object(out)
}

/// Convert a map type (`{ type: object, values: <schema> }`) to
/// `{ type: object, additionalProperties: <schema> }`.
fn convert_map_type(map: &JsonMap<String, JsonValue>) -> JsonValue {
    let mut out = JsonMap::new();
    out.insert("type".to_string(), JsonValue::String("object".to_string()));

    if let Some(values_schema) = map.get("values") {
        out.insert(
            "additionalProperties".to_string(),
            convert_syaml_type(values_schema),
        );
    }

    JsonValue::Object(out)
}

/// Convert an array type.
fn convert_array_type(map: &JsonMap<String, JsonValue>) -> JsonValue {
    let mut out = JsonMap::new();
    out.insert("type".to_string(), JsonValue::String("array".to_string()));

    if let Some(items_schema) = map.get("items") {
        out.insert("items".to_string(), convert_syaml_type(items_schema));
    }

    for key in &["minItems", "maxItems"] {
        if let Some(v) = map.get(*key) {
            out.insert((*key).to_string(), v.clone());
        }
    }

    JsonValue::Object(out)
}

/// Convert a union type to `oneOf`.
fn convert_union_type(map: &JsonMap<String, JsonValue>) -> JsonValue {
    let mut out = JsonMap::new();

    if let Some(variants_value) = map.get("variants") {
        if let Some(variants_array) = variants_value.as_array() {
            let one_of: Vec<JsonValue> = variants_array.iter().map(convert_syaml_type).collect();
            out.insert("oneOf".to_string(), JsonValue::Array(one_of));
        }
    }

    JsonValue::Object(out)
}

/// Return a copy of `map` with SYAML-only keys removed.
fn strip_syaml_keys(map: &JsonMap<String, JsonValue>) -> JsonValue {
    let mut out = JsonMap::new();
    for (key, value) in map {
        if !STRIP_KEYS.contains(&key.as_str()) {
            out.insert(key.clone(), value.clone());
        }
    }
    JsonValue::Object(out)
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::schema::parse_schema;

    use super::*;

    fn schema_from_json(value: serde_json::Value) -> SchemaDoc {
        parse_schema(&value).unwrap()
    }

    #[test]
    fn empty_schema_produces_empty_defs() {
        let schema = schema_from_json(json!({}));
        let out = to_json_schema(&schema, false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            parsed["$schema"],
            "https://json-schema.org/draft/2020-12/schema"
        );
        assert_eq!(parsed["$defs"], json!({}));
    }

    #[test]
    fn primitive_types_convert() {
        let schema = schema_from_json(json!({
            "Port": { "type": "integer", "minimum": 1, "maximum": 65535 },
            "Name": { "type": "string", "minLength": 1 }
        }));
        let out = to_json_schema(&schema, false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["$defs"]["Port"]["type"], "integer");
        assert_eq!(parsed["$defs"]["Port"]["minimum"], 1);
        assert_eq!(parsed["$defs"]["Port"]["maximum"], 65535);
        assert_eq!(parsed["$defs"]["Name"]["type"], "string");
        assert_eq!(parsed["$defs"]["Name"]["minLength"], 1);
    }

    #[test]
    fn object_type_with_required_and_optional() {
        let schema = schema_from_json(json!({
            "Config": {
                "type": "object",
                "properties": {
                    "host": { "type": "string" },
                    "port": { "type": "integer", "optional": true }
                }
            }
        }));
        let out = to_json_schema(&schema, false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        let defs = &parsed["$defs"]["Config"];
        assert_eq!(defs["type"], "object");
        // Only "host" should be required (port is optional)
        let required = defs["required"].as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert_eq!(required[0], "host");
        // optional key stripped from property schema
        assert!(defs["properties"]["port"]["optional"].is_null());
    }

    #[test]
    fn union_type_converts_to_one_of() {
        let schema = schema_from_json(json!({
            "StringOrInt": {
                "type": "union",
                "variants": ["string", "integer"]
            }
        }));
        let out = to_json_schema(&schema, false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        let one_of = parsed["$defs"]["StringOrInt"]["oneOf"].as_array().unwrap();
        assert_eq!(one_of.len(), 2);
        assert_eq!(one_of[0]["type"], "string");
        assert_eq!(one_of[1]["type"], "integer");
    }

    #[test]
    fn map_type_converts_to_additional_properties() {
        let schema = schema_from_json(json!({
            "Tags": {
                "type": "object",
                "values": { "type": "string" }
            }
        }));
        let out = to_json_schema(&schema, false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        let defs = &parsed["$defs"]["Tags"];
        assert_eq!(defs["type"], "object");
        assert_eq!(defs["additionalProperties"]["type"], "string");
    }

    #[test]
    fn array_type_converts() {
        let schema = schema_from_json(json!({
            "Names": {
                "type": "array",
                "items": { "type": "string" },
                "minItems": 1
            }
        }));
        let out = to_json_schema(&schema, false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        let defs = &parsed["$defs"]["Names"];
        assert_eq!(defs["type"], "array");
        assert_eq!(defs["items"]["type"], "string");
        assert_eq!(defs["minItems"], 1);
    }

    #[test]
    fn named_type_ref_produces_dollar_ref() {
        let schema = schema_from_json(json!({
            "Port": { "type": "integer" },
            "Config": {
                "type": "object",
                "properties": {
                    "port": { "type": "Port" }
                }
            }
        }));
        let out = to_json_schema(&schema, false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            parsed["$defs"]["Config"]["properties"]["port"]["$ref"],
            "#/$defs/Port"
        );
    }

    #[test]
    fn syaml_only_keys_are_stripped() {
        let schema = schema_from_json(json!({
            "Port": {
                "type": "integer",
                "minimum": 1,
                "constraints": "value >= 1",
                "since": "1.0.0",
                "deprecated": "2.0.0",
                "field_number": 1
            }
        }));
        let out = to_json_schema(&schema, false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        let port = &parsed["$defs"]["Port"];
        assert!(port["constraints"].is_null());
        assert!(port["since"].is_null());
        assert!(port["deprecated"].is_null());
        assert!(port["field_number"].is_null());
        assert_eq!(port["minimum"], 1);
    }

    #[test]
    fn pretty_flag_produces_indented_output() {
        let schema = schema_from_json(json!({}));
        let compact = to_json_schema(&schema, false).unwrap();
        let pretty = to_json_schema(&schema, true).unwrap();
        assert!(!compact.contains('\n'));
        assert!(pretty.contains('\n'));
    }
}
