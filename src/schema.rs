//! Schema parsing and schema-based validation helpers.
//!
//! Supported keyword subset:
//! - Common: `type`, `enum`
//! - Numeric: `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum`
//! - String: `minLength`, `maxLength`, `pattern`
//! - Object: `properties`, `required`
//! - Array: `items`, `minItems`, `maxItems`

use std::collections::BTreeMap;

use regex::Regex;
use serde_json::Value as JsonValue;

use crate::ast::SchemaDoc;
use crate::error::SyamlError;

/// Parses a `schema` section JSON value into [`SchemaDoc`].
pub fn parse_schema(value: &JsonValue) -> Result<SchemaDoc, SyamlError> {
    let map = value
        .as_object()
        .ok_or_else(|| SyamlError::SchemaError("schema must be a mapping/object".to_string()))?;

    let mut types = BTreeMap::new();
    if let Some(types_json) = map.get("types") {
        let types_map = types_json
            .as_object()
            .ok_or_else(|| SyamlError::SchemaError("schema.types must be a mapping".to_string()))?;
        for (k, v) in types_map {
            types.insert(k.clone(), v.clone());
        }
    }

    let mut constraints = BTreeMap::new();
    if let Some(constraints_json) = map.get("constraints") {
        let constraints_map = constraints_json.as_object().ok_or_else(|| {
            SyamlError::SchemaError("schema.constraints must be a mapping".to_string())
        })?;
        for (path, value) in constraints_map {
            let expressions = match value {
                JsonValue::String(s) => vec![s.clone()],
                JsonValue::Array(items) => {
                    let mut out = Vec::with_capacity(items.len());
                    for item in items {
                        match item {
                            JsonValue::String(s) => out.push(s.clone()),
                            _ => {
                                return Err(SyamlError::SchemaError(format!(
                                    "constraint '{}' entries must be strings",
                                    path
                                )))
                            }
                        }
                    }
                    out
                }
                _ => {
                    return Err(SyamlError::SchemaError(format!(
                        "constraint '{}' must be string or list of strings",
                        path
                    )))
                }
            };
            constraints.insert(path.clone(), expressions);
        }
    }

    Ok(SchemaDoc { types, constraints })
}

/// Resolves a type name to a schema object.
///
/// If `type_name` exists in `schema.types`, that definition is returned.
/// Otherwise, built-in primitive names (`string`, `integer`, etc.) are mapped
/// to a schema object `{ "type": "<name>" }`.
pub fn resolve_type_schema(schema: &SchemaDoc, type_name: &str) -> Result<JsonValue, SyamlError> {
    if let Some(found) = schema.types.get(type_name) {
        return Ok(found.clone());
    }

    if matches!(
        type_name,
        "string" | "integer" | "number" | "boolean" | "object" | "array" | "null"
    ) {
        return Ok(serde_json::json!({ "type": type_name }));
    }

    Err(SyamlError::TypeHintError(format!(
        "unknown type '{}'; not found in schema.types",
        type_name
    )))
}

/// Validates a JSON value against a schema object at a logical path.
///
/// `path` is used only for error messages.
pub fn validate_json_against_schema(
    value: &JsonValue,
    schema: &JsonValue,
    path: &str,
) -> Result<(), SyamlError> {
    let schema_obj = schema.as_object().ok_or_else(|| {
        SyamlError::SchemaError(format!(
            "schema at {path} must be an object, found {schema:?}"
        ))
    })?;

    if let Some(type_value) = schema_obj.get("type") {
        let type_name = type_value.as_str().ok_or_else(|| {
            SyamlError::SchemaError(format!("schema 'type' at {path} must be a string"))
        })?;
        if !json_matches_type(value, type_name) {
            return Err(SyamlError::SchemaError(format!(
                "type mismatch at {path}: expected {type_name}, found {}",
                json_type_name(value)
            )));
        }
    }

    if let Some(enum_value) = schema_obj.get("enum") {
        let options = enum_value.as_array().ok_or_else(|| {
            SyamlError::SchemaError(format!("schema 'enum' at {path} must be an array"))
        })?;
        if !options.iter().any(|candidate| candidate == value) {
            return Err(SyamlError::SchemaError(format!(
                "enum mismatch at {path}: value {value} not in enum set"
            )));
        }
    }

    validate_numeric_keywords(value, schema_obj, path)?;
    validate_string_keywords(value, schema_obj, path)?;
    validate_object_keywords(value, schema_obj, path)?;
    validate_array_keywords(value, schema_obj, path)?;

    Ok(())
}

fn validate_numeric_keywords(
    value: &JsonValue,
    schema: &serde_json::Map<String, JsonValue>,
    path: &str,
) -> Result<(), SyamlError> {
    let val = match value.as_f64() {
        Some(v) => v,
        None => return Ok(()),
    };

    if let Some(minimum) = schema.get("minimum") {
        let min = minimum.as_f64().ok_or_else(|| {
            SyamlError::SchemaError(format!("minimum at {path} must be a number"))
        })?;
        if val < min {
            return Err(SyamlError::SchemaError(format!(
                "minimum violation at {path}: {val} < {min}"
            )));
        }
    }

    if let Some(maximum) = schema.get("maximum") {
        let max = maximum.as_f64().ok_or_else(|| {
            SyamlError::SchemaError(format!("maximum at {path} must be a number"))
        })?;
        if val > max {
            return Err(SyamlError::SchemaError(format!(
                "maximum violation at {path}: {val} > {max}"
            )));
        }
    }

    if let Some(exclusive_minimum) = schema.get("exclusiveMinimum") {
        let min = exclusive_minimum.as_f64().ok_or_else(|| {
            SyamlError::SchemaError(format!("exclusiveMinimum at {path} must be a number"))
        })?;
        if val <= min {
            return Err(SyamlError::SchemaError(format!(
                "exclusiveMinimum violation at {path}: {val} <= {min}"
            )));
        }
    }

    if let Some(exclusive_maximum) = schema.get("exclusiveMaximum") {
        let max = exclusive_maximum.as_f64().ok_or_else(|| {
            SyamlError::SchemaError(format!("exclusiveMaximum at {path} must be a number"))
        })?;
        if val >= max {
            return Err(SyamlError::SchemaError(format!(
                "exclusiveMaximum violation at {path}: {val} >= {max}"
            )));
        }
    }

    Ok(())
}

fn validate_string_keywords(
    value: &JsonValue,
    schema: &serde_json::Map<String, JsonValue>,
    path: &str,
) -> Result<(), SyamlError> {
    let s = match value.as_str() {
        Some(v) => v,
        None => return Ok(()),
    };

    if let Some(min_len) = schema.get("minLength") {
        let min = min_len.as_u64().ok_or_else(|| {
            SyamlError::SchemaError(format!("minLength at {path} must be an integer"))
        })?;
        if (s.chars().count() as u64) < min {
            return Err(SyamlError::SchemaError(format!(
                "minLength violation at {path}: {} < {min}",
                s.chars().count()
            )));
        }
    }

    if let Some(max_len) = schema.get("maxLength") {
        let max = max_len.as_u64().ok_or_else(|| {
            SyamlError::SchemaError(format!("maxLength at {path} must be an integer"))
        })?;
        if (s.chars().count() as u64) > max {
            return Err(SyamlError::SchemaError(format!(
                "maxLength violation at {path}: {} > {max}",
                s.chars().count()
            )));
        }
    }

    if let Some(pattern) = schema.get("pattern") {
        let pat = pattern.as_str().ok_or_else(|| {
            SyamlError::SchemaError(format!("pattern at {path} must be a string"))
        })?;
        let re = Regex::new(pat).map_err(|e| {
            SyamlError::SchemaError(format!("invalid pattern '{pat}' at {path}: {e}"))
        })?;
        if !re.is_match(s) {
            return Err(SyamlError::SchemaError(format!(
                "pattern violation at {path}: '{s}' does not match '{pat}'"
            )));
        }
    }

    Ok(())
}

fn validate_object_keywords(
    value: &JsonValue,
    schema: &serde_json::Map<String, JsonValue>,
    path: &str,
) -> Result<(), SyamlError> {
    let obj = match value.as_object() {
        Some(v) => v,
        None => return Ok(()),
    };

    if let Some(required) = schema.get("required") {
        let arr = required.as_array().ok_or_else(|| {
            SyamlError::SchemaError(format!("required at {path} must be an array"))
        })?;
        for req in arr {
            let key = req.as_str().ok_or_else(|| {
                SyamlError::SchemaError(format!("required entries at {path} must be strings"))
            })?;
            if !obj.contains_key(key) {
                return Err(SyamlError::SchemaError(format!(
                    "required property missing at {path}: '{key}'"
                )));
            }
        }
    }

    if let Some(props) = schema.get("properties") {
        let prop_map = props.as_object().ok_or_else(|| {
            SyamlError::SchemaError(format!("properties at {path} must be an object"))
        })?;
        for (k, child_schema) in prop_map {
            if let Some(child_value) = obj.get(k) {
                let child_path = format!("{}.{}", path, k);
                validate_json_against_schema(child_value, child_schema, &child_path)?;
            }
        }
    }

    Ok(())
}

fn validate_array_keywords(
    value: &JsonValue,
    schema: &serde_json::Map<String, JsonValue>,
    path: &str,
) -> Result<(), SyamlError> {
    let arr = match value.as_array() {
        Some(v) => v,
        None => return Ok(()),
    };

    if let Some(min_items) = schema.get("minItems") {
        let min = min_items.as_u64().ok_or_else(|| {
            SyamlError::SchemaError(format!("minItems at {path} must be an integer"))
        })?;
        if (arr.len() as u64) < min {
            return Err(SyamlError::SchemaError(format!(
                "minItems violation at {path}: {} < {min}",
                arr.len()
            )));
        }
    }

    if let Some(max_items) = schema.get("maxItems") {
        let max = max_items.as_u64().ok_or_else(|| {
            SyamlError::SchemaError(format!("maxItems at {path} must be an integer"))
        })?;
        if (arr.len() as u64) > max {
            return Err(SyamlError::SchemaError(format!(
                "maxItems violation at {path}: {} > {max}",
                arr.len()
            )));
        }
    }

    if let Some(items_schema) = schema.get("items") {
        for (idx, item) in arr.iter().enumerate() {
            let child_path = format!("{}[{}]", path, idx);
            validate_json_against_schema(item, items_schema, &child_path)?;
        }
    }

    Ok(())
}

fn json_matches_type(value: &JsonValue, type_name: &str) -> bool {
    match type_name {
        "string" => value.is_string(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        "null" => value.is_null(),
        _ => false,
    }
}

fn json_type_name(value: &JsonValue) -> &'static str {
    if value.is_null() {
        "null"
    } else if value.is_boolean() {
        "boolean"
    } else if value.as_i64().is_some() || value.as_u64().is_some() {
        "integer"
    } else if value.is_number() {
        "number"
    } else if value.is_string() {
        "string"
    } else if value.is_array() {
        "array"
    } else {
        "object"
    }
}
