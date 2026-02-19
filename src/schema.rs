//! Schema parsing and schema-based validation helpers.
//!
//! Supported keyword subset:
//! - Common: `type`, `enum`
//! - Numeric: `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum`
//! - String: `minLength`, `maxLength`, `pattern`
//! - Object: `properties`, `values`, `required`, `optional` (on property schemas), `constructors`
//! - Array: `items`, `minItems`, `maxItems`

use std::collections::{BTreeMap, HashSet};

use regex::Regex;
use serde_json::Value as JsonValue;

use crate::ast::SchemaDoc;
use crate::error::SyamlError;
use crate::expr::{parse_expression, parser::Expr};

const MAX_SCHEMA_VALIDATION_DEPTH: usize = 64;

struct SchemaValidationContext<'a> {
    types: &'a BTreeMap<String, JsonValue>,
    type_stack: Vec<String>,
}

/// Parses a `schema` section JSON value into [`SchemaDoc`].
pub fn parse_schema(value: &JsonValue) -> Result<SchemaDoc, SyamlError> {
    let map = value
        .as_object()
        .ok_or_else(|| SyamlError::SchemaError("schema must be a mapping/object".to_string()))?;

    let type_map = if let Some(types_json) = map.get("types") {
        if map.len() == 1 {
            types_json.as_object().ok_or_else(|| {
                SyamlError::SchemaError("schema.types must be a mapping".to_string())
            })?
        } else {
            return Err(SyamlError::SchemaError(
                "schema cannot mix legacy 'types' wrapper with direct type definitions".to_string(),
            ));
        }
    } else {
        map
    };

    let mut types = BTreeMap::new();
    for (k, v) in type_map {
        types.insert(k.clone(), normalize_schema_node(v.clone()));
    }

    let mut type_constraints = BTreeMap::new();
    for (type_name, type_schema) in &types {
        let mut collected = BTreeMap::new();
        collect_type_constraints(type_schema, "$", type_name, &mut collected)?;
        validate_type_constraint_variable_scope(type_name, type_schema, &collected, &types)?;
        if !collected.is_empty() {
            type_constraints.insert(type_name.clone(), collected);
        }
    }

    Ok(SchemaDoc {
        types,
        type_constraints,
    })
}

/// Validates that every schema `type` keyword references either a builtin
/// primitive or a defined type in the provided registry.
///
/// The returned error path is rooted at `schema.<TypeName>...` so callers can
/// map diagnostics back to schema usage sites.
pub fn validate_schema_type_references(
    types: &BTreeMap<String, JsonValue>,
) -> Result<(), SyamlError> {
    for (type_name, schema) in types {
        let root_path = format!("schema.{}", type_name);
        validate_schema_type_references_inner(schema, types, &root_path)?;
    }
    Ok(())
}

fn validate_schema_type_references_inner(
    schema: &JsonValue,
    types: &BTreeMap<String, JsonValue>,
    path: &str,
) -> Result<(), SyamlError> {
    match schema {
        JsonValue::Object(map) => {
            validate_constructor_keywords(map, path, types)?;
            for (key, child) in map {
                let child_path = format!("{path}.{key}");
                if key == "type" {
                    let type_name = child.as_str().ok_or_else(|| {
                        SyamlError::SchemaError(format!(
                            "schema 'type' at {child_path} must be a string"
                        ))
                    })?;
                    if !is_builtin_type_name(type_name) && !types.contains_key(type_name) {
                        return Err(SyamlError::SchemaError(format!(
                            "unknown type reference at {child_path}: '{type_name}' not found in schema"
                        )));
                    }
                }
                validate_schema_type_references_inner(child, types, &child_path)?;
            }
        }
        JsonValue::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                let item_path = format!("{path}[{index}]");
                validate_schema_type_references_inner(item, types, &item_path)?;
            }
        }
        _ => {}
    }

    Ok(())
}

fn normalize_schema_node(schema: JsonValue) -> JsonValue {
    match schema {
        JsonValue::Array(values) if values.iter().all(JsonValue::is_string) => {
            let mut out = serde_json::Map::new();
            out.insert("type".to_string(), JsonValue::String("string".to_string()));
            out.insert("enum".to_string(), JsonValue::Array(values));
            JsonValue::Object(out)
        }
        JsonValue::String(type_name) => {
            let (normalized_type, optional) = parse_optional_type_marker(&type_name);
            let mut out = serde_json::Map::new();
            out.insert(
                "type".to_string(),
                JsonValue::String(normalized_type.to_string()),
            );
            if optional {
                out.insert("optional".to_string(), JsonValue::Bool(true));
            }
            JsonValue::Object(out)
        }
        JsonValue::Object(mut map) => {
            if let Some(type_name) = map.get("type").and_then(JsonValue::as_str) {
                let (normalized_type, optional) = parse_optional_type_marker(type_name);
                if normalized_type != type_name {
                    map.insert(
                        "type".to_string(),
                        JsonValue::String(normalized_type.to_string()),
                    );
                    if optional && !map.contains_key("optional") {
                        map.insert("optional".to_string(), JsonValue::Bool(true));
                    }
                }
            }

            if let Some(properties) = map.get_mut("properties") {
                if let Some(property_map) = properties.as_object_mut() {
                    for property_schema in property_map.values_mut() {
                        let normalized = normalize_schema_node(property_schema.clone());
                        *property_schema = normalized;
                    }
                }
            }

            if let Some(items) = map.get_mut("items") {
                let normalized = normalize_schema_node(items.clone());
                *items = normalized;
            }
            if let Some(values) = map.get_mut("values") {
                let normalized = normalize_schema_node(values.clone());
                *values = normalized;
            }

            JsonValue::Object(map)
        }
        other => other,
    }
}

fn parse_optional_type_marker(type_name: &str) -> (&str, bool) {
    match type_name.strip_suffix('?') {
        Some(base) if !base.is_empty() => (base, true),
        _ => (type_name, false),
    }
}

fn validate_constructor_keywords(
    schema: &serde_json::Map<String, JsonValue>,
    path: &str,
    types: &BTreeMap<String, JsonValue>,
) -> Result<(), SyamlError> {
    let Some(raw_constructors) = schema.get("constructors") else {
        return Ok(());
    };

    let declared_type = schema
        .get("type")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| {
            SyamlError::SchemaError(format!(
                "constructors at {path} require schema node with type: object"
            ))
        })?;
    if declared_type != "object" {
        return Err(SyamlError::SchemaError(format!(
            "constructors at {path} require type: object"
        )));
    }

    let constructors = raw_constructors.as_object().ok_or_else(|| {
        SyamlError::SchemaError(format!("constructors at {path} must be an object"))
    })?;
    if constructors.is_empty() {
        return Err(SyamlError::SchemaError(format!(
            "constructors at {path} must not be empty"
        )));
    }

    let property_map = schema.get("properties").and_then(JsonValue::as_object);

    for (constructor_name, raw_constructor) in constructors {
        let constructor_path = format!("{path}.constructors.{constructor_name}");
        let constructor = raw_constructor.as_object().ok_or_else(|| {
            SyamlError::SchemaError(format!("{constructor_path} must be an object"))
        })?;

        let regex_text = constructor
            .get("regex")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| {
                SyamlError::SchemaError(format!("{constructor_path}.regex must be a string"))
            })?;
        let regex = Regex::new(regex_text).map_err(|e| {
            SyamlError::SchemaError(format!(
                "invalid constructor regex '{}' at {}: {}",
                regex_text, constructor_path, e
            ))
        })?;
        if let Some(order) = constructor.get("order") {
            let Some(parsed) = order.as_i64() else {
                return Err(SyamlError::SchemaError(format!(
                    "{constructor_path}.order must be an integer >= 0"
                )));
            };
            if parsed < 0 {
                return Err(SyamlError::SchemaError(format!(
                    "{constructor_path}.order must be an integer >= 0"
                )));
            }
        }

        let mut capture_names = HashSet::new();
        for name in regex.capture_names().flatten() {
            capture_names.insert(name.to_string());
        }

        if let Some(raw_map) = constructor.get("map") {
            let map = raw_map.as_object().ok_or_else(|| {
                SyamlError::SchemaError(format!("{constructor_path}.map must be an object"))
            })?;
            for (dest, raw_rule) in map {
                if let Some(props) = property_map {
                    if !props.contains_key(dest) {
                        return Err(SyamlError::SchemaError(format!(
                            "{constructor_path}.map destination '{}' must be declared in properties",
                            dest
                        )));
                    }
                }
                let rule_path = format!("{constructor_path}.map.{dest}");
                let rule = raw_rule.as_object().ok_or_else(|| {
                    SyamlError::SchemaError(format!("{rule_path} must be an object"))
                })?;
                let group = rule
                    .get("group")
                    .and_then(JsonValue::as_str)
                    .ok_or_else(|| {
                        SyamlError::SchemaError(format!("{rule_path}.group must be a string"))
                    })?;
                if !capture_names.contains(group) {
                    return Err(SyamlError::SchemaError(format!(
                        "{rule_path}.group references unknown capture group '{}'",
                        group
                    )));
                }
                let raw_decode = rule.get("decode");
                let raw_from_enum = rule.get("from_enum");
                if raw_decode.is_some() && raw_from_enum.is_some() {
                    return Err(SyamlError::SchemaError(format!(
                        "{rule_path} cannot set both 'decode' and 'from_enum'"
                    )));
                }
                if let Some(raw_decode) = raw_decode {
                    let decode = raw_decode.as_str().ok_or_else(|| {
                        SyamlError::SchemaError(format!("{rule_path}.decode must be a string"))
                    })?;
                    validate_decode_name(decode, &format!("{rule_path}.decode"))?;
                }
                if let Some(raw_from_enum) = raw_from_enum {
                    let enum_type_name = raw_from_enum.as_str().ok_or_else(|| {
                        SyamlError::SchemaError(format!("{rule_path}.from_enum must be a string"))
                    })?;
                    validate_constructor_from_enum_reference(
                        types,
                        enum_type_name,
                        &format!("{rule_path}.from_enum"),
                    )?;
                }
            }
        }

        if let Some(raw_defaults) = constructor.get("defaults") {
            let defaults = raw_defaults.as_object().ok_or_else(|| {
                SyamlError::SchemaError(format!("{constructor_path}.defaults must be an object"))
            })?;
            if let Some(props) = property_map {
                for key in defaults.keys() {
                    if !props.contains_key(key) {
                        return Err(SyamlError::SchemaError(format!(
                            "{constructor_path}.defaults key '{}' must be declared in properties",
                            key
                        )));
                    }
                }
            }
        }
    }

    Ok(())
}

fn validate_constructor_from_enum_reference(
    types: &BTreeMap<String, JsonValue>,
    enum_type_name: &str,
    path: &str,
) -> Result<(), SyamlError> {
    let enum_schema = types.get(enum_type_name).ok_or_else(|| {
        SyamlError::SchemaError(format!(
            "unknown type reference at {}: '{}' not found in schema",
            path, enum_type_name
        ))
    })?;
    let enum_obj = enum_schema.as_object().ok_or_else(|| {
        SyamlError::SchemaError(format!(
            "referenced enum type '{}' at {} must be an object schema",
            enum_type_name, path
        ))
    })?;
    let enum_values = enum_obj
        .get("enum")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| {
            SyamlError::SchemaError(format!(
                "referenced type '{}' at {} must declare an enum array",
                enum_type_name, path
            ))
        })?;
    if enum_values.iter().any(|v| !v.is_string()) {
        return Err(SyamlError::SchemaError(format!(
            "referenced enum type '{}' at {} must contain only strings",
            enum_type_name, path
        )));
    }
    Ok(())
}

fn validate_decode_name(name: &str, path: &str) -> Result<(), SyamlError> {
    if matches!(
        name,
        "auto" | "string" | "integer" | "number" | "boolean" | "hex_u8" | "hex_alpha"
    ) {
        return Ok(());
    }
    Err(SyamlError::SchemaError(format!(
        "unsupported decode '{}' at {}",
        name, path
    )))
}

fn collect_type_constraints(
    schema: &JsonValue,
    current_path: &str,
    type_name: &str,
    out: &mut BTreeMap<String, Vec<String>>,
) -> Result<(), SyamlError> {
    let Some(schema_obj) = schema.as_object() else {
        return Ok(());
    };

    if let Some(raw_constraints) = schema_obj.get("constraints") {
        parse_type_local_constraints(raw_constraints, current_path, type_name, out)?;
    }

    if let Some(props_json) = schema_obj.get("properties") {
        if let Some(prop_map) = props_json.as_object() {
            for (key, child_schema) in prop_map {
                let child_path = if current_path == "$" {
                    format!("$.{}", key)
                } else {
                    format!("{}.{}", current_path, key)
                };
                collect_type_constraints(child_schema, &child_path, type_name, out)?;
            }
        }
    }

    Ok(())
}

fn parse_type_local_constraints(
    value: &JsonValue,
    current_path: &str,
    type_name: &str,
    out: &mut BTreeMap<String, Vec<String>>,
) -> Result<(), SyamlError> {
    match value {
        JsonValue::String(_) | JsonValue::Array(_) => {
            let location = format!("schema.{}.constraints", type_name);
            let expressions = parse_constraint_expressions(value, &location, current_path)?;
            append_constraints(out, current_path, expressions);
            Ok(())
        }
        JsonValue::Object(map) => {
            for (relative_path, raw_exprs) in map {
                let location = format!("schema.{}.constraints.{}", type_name, relative_path);
                let expressions =
                    parse_constraint_expressions(raw_exprs, &location, relative_path)?;
                let joined_path =
                    join_constraint_paths(current_path, &normalize_constraint_path(relative_path));
                append_constraints(out, &joined_path, expressions);
            }
            Ok(())
        }
        _ => Err(SyamlError::SchemaError(format!(
            "schema.{}.constraints must be string, list of strings, or mapping",
            type_name
        ))),
    }
}

fn parse_constraint_expressions(
    value: &JsonValue,
    location: &str,
    path_label: &str,
) -> Result<Vec<String>, SyamlError> {
    match value {
        JsonValue::String(s) => Ok(vec![s.clone()]),
        JsonValue::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    JsonValue::String(s) => out.push(s.clone()),
                    _ => {
                        return Err(SyamlError::SchemaError(format!(
                            "constraint '{}' entries must be strings",
                            path_label
                        )))
                    }
                }
            }
            Ok(out)
        }
        _ => Err(SyamlError::SchemaError(format!(
            "{location} must be string or list of strings"
        ))),
    }
}

fn validate_type_constraint_variable_scope(
    type_name: &str,
    type_schema: &JsonValue,
    constraints: &BTreeMap<String, Vec<String>>,
    types: &BTreeMap<String, JsonValue>,
) -> Result<(), SyamlError> {
    for (constraint_path, expressions) in constraints {
        let scope_schema =
            resolve_schema_scope(type_schema, constraint_path, types).ok_or_else(|| {
                SyamlError::SchemaError(format!(
                    "constraint path '{}' under schema.{} does not resolve to a schema node",
                    constraint_path, type_name
                ))
            })?;

        for expression in expressions {
            let source = expression.trim().trim_start_matches('=').trim();
            let ast = parse_expression(source).map_err(|e| {
                SyamlError::SchemaError(format!(
                    "invalid constraint expression '{}' at schema.{} path '{}': {}",
                    expression, type_name, constraint_path, e
                ))
            })?;
            let mut var_paths = Vec::new();
            collect_var_paths(&ast, &mut var_paths);

            for var_path in var_paths {
                let Some(first) = var_path.first() else {
                    continue;
                };
                if first == "value" || first == "env" {
                    continue;
                }
                if !is_schema_relative_path_valid(scope_schema, &var_path, types) {
                    return Err(SyamlError::SchemaError(format!(
                        "constraint '{}' at schema.{} path '{}' references '{}', which is outside the constrained type scope",
                        expression,
                        type_name,
                        constraint_path,
                        var_path.join(".")
                    )));
                }
            }
        }
    }

    Ok(())
}

fn collect_var_paths(expr: &Expr, out: &mut Vec<Vec<String>>) {
    match expr {
        Expr::Var(path) => out.push(path.clone()),
        Expr::Unary { expr, .. } => collect_var_paths(expr, out),
        Expr::Binary { left, right, .. } => {
            collect_var_paths(left, out);
            collect_var_paths(right, out);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_var_paths(arg, out);
            }
        }
        Expr::Number(_) | Expr::String(_) | Expr::Bool(_) | Expr::Null => {}
    }
}

fn resolve_schema_scope<'a>(
    type_schema: &'a JsonValue,
    constraint_path: &str,
    types: &'a BTreeMap<String, JsonValue>,
) -> Option<&'a JsonValue> {
    let mut current = dereference_named_schema(type_schema, types)?;
    for segment in parse_constraint_segments(constraint_path)? {
        current = dereference_named_schema(current, types)?;
        let obj = current.as_object()?;

        if let Some(properties) = obj.get("properties").and_then(JsonValue::as_object) {
            if let Some(next) = properties.get(&segment) {
                current = next;
                continue;
            }
        }

        if let Some(values_schema) = obj.get("values") {
            current = values_schema;
            continue;
        }

        return None;
    }

    Some(current)
}

fn is_schema_relative_path_valid(
    scope_schema: &JsonValue,
    path: &[String],
    types: &BTreeMap<String, JsonValue>,
) -> bool {
    let mut current = match dereference_named_schema(scope_schema, types) {
        Some(node) => node,
        None => return false,
    };

    for segment in path {
        current = match dereference_named_schema(current, types) {
            Some(node) => node,
            None => return false,
        };
        let Some(obj) = current.as_object() else {
            return false;
        };

        if let Some(properties) = obj.get("properties").and_then(JsonValue::as_object) {
            if let Some(next) = properties.get(segment) {
                current = next;
                continue;
            }
        }

        if let Some(values_schema) = obj.get("values") {
            current = values_schema;
            continue;
        }

        return false;
    }

    true
}

fn parse_constraint_segments(path: &str) -> Option<Vec<String>> {
    let normalized = normalize_constraint_path(path);
    if normalized == "$" {
        return Some(Vec::new());
    }
    let remainder = normalized.strip_prefix("$.")?;
    let mut out = Vec::new();
    for segment in remainder.split('.') {
        if segment.is_empty() {
            return None;
        }
        out.push(segment.to_string());
    }
    Some(out)
}

fn dereference_named_schema<'a>(
    mut node: &'a JsonValue,
    types: &'a BTreeMap<String, JsonValue>,
) -> Option<&'a JsonValue> {
    let mut depth = 0usize;
    loop {
        if depth > MAX_SCHEMA_VALIDATION_DEPTH {
            return None;
        }
        let Some(obj) = node.as_object() else {
            return Some(node);
        };
        let Some(type_name) = obj.get("type").and_then(JsonValue::as_str) else {
            return Some(node);
        };
        if is_builtin_type_name(type_name) {
            return Some(node);
        }
        let Some(next) = types.get(type_name) else {
            return None;
        };
        node = next;
        depth += 1;
    }
}

fn append_constraints(
    constraints: &mut BTreeMap<String, Vec<String>>,
    path: &str,
    expressions: Vec<String>,
) {
    constraints
        .entry(path.to_string())
        .or_default()
        .extend(expressions);
}

fn normalize_constraint_path(path: &str) -> String {
    if path == "$" || path.starts_with("$.") {
        path.to_string()
    } else {
        format!("$.{}", path)
    }
}

fn join_constraint_paths(base: &str, relative: &str) -> String {
    let base_norm = normalize_constraint_path(base);
    let rel_norm = normalize_constraint_path(relative);

    if rel_norm == "$" {
        base_norm
    } else if base_norm == "$" {
        rel_norm
    } else {
        format!("{}{}", base_norm, &rel_norm[1..])
    }
}

/// Resolves a type name to a schema object.
///
/// If `type_name` exists in the schema section, that definition is returned.
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
        "unknown type '{}'; not found in schema",
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
    let types = BTreeMap::new();
    validate_json_against_schema_with_types(value, schema, path, &types)
}

/// Validates a JSON value against a schema object and named type registry.
///
/// Named `type` references are resolved from `types`. Built-in primitive type
/// names (`string`, `integer`, etc.) are validated directly.
pub fn validate_json_against_schema_with_types(
    value: &JsonValue,
    schema: &JsonValue,
    path: &str,
    types: &BTreeMap<String, JsonValue>,
) -> Result<(), SyamlError> {
    let mut ctx = SchemaValidationContext {
        types,
        type_stack: Vec::new(),
    };
    validate_json_against_schema_inner(value, schema, path, 0, &mut ctx)
}

fn validate_json_against_schema_inner(
    value: &JsonValue,
    schema: &JsonValue,
    path: &str,
    depth: usize,
    ctx: &mut SchemaValidationContext<'_>,
) -> Result<(), SyamlError> {
    if depth > MAX_SCHEMA_VALIDATION_DEPTH {
        return Err(SyamlError::SchemaError(format!(
            "schema validation exceeded max depth ({MAX_SCHEMA_VALIDATION_DEPTH}) at {path}"
        )));
    }

    let schema_obj = schema.as_object().ok_or_else(|| {
        SyamlError::SchemaError(format!(
            "schema at {path} must be an object, found {schema:?}"
        ))
    })?;

    if let Some(type_value) = schema_obj.get("type") {
        let type_name = type_value.as_str().ok_or_else(|| {
            SyamlError::SchemaError(format!("schema 'type' at {path} must be a string"))
        })?;
        if is_builtin_type_name(type_name) {
            if !json_matches_type(value, type_name) {
                return Err(SyamlError::SchemaError(format!(
                    "type mismatch at {path}: expected {type_name}, found {}",
                    json_type_name(value)
                )));
            }
        } else {
            validate_named_type_reference(value, type_name, path, depth, ctx)?;
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
    validate_object_keywords(value, schema_obj, path, depth, ctx)?;
    validate_array_keywords(value, schema_obj, path, depth, ctx)?;

    Ok(())
}

fn validate_named_type_reference(
    value: &JsonValue,
    type_name: &str,
    path: &str,
    depth: usize,
    ctx: &mut SchemaValidationContext<'_>,
) -> Result<(), SyamlError> {
    let referenced_schema = ctx.types.get(type_name).ok_or_else(|| {
        SyamlError::SchemaError(format!(
            "unknown type reference at {path}: '{type_name}' not found in schema"
        ))
    })?;

    if let Some(cycle_start) = ctx.type_stack.iter().position(|t| t == type_name) {
        let mut cycle = ctx.type_stack[cycle_start..].to_vec();
        cycle.push(type_name.to_string());
        return Err(SyamlError::SchemaError(format!(
            "cyclic type reference at {path}: {}",
            cycle.join(" -> ")
        )));
    }

    ctx.type_stack.push(type_name.to_string());
    let result = validate_json_against_schema_inner(value, referenced_schema, path, depth + 1, ctx);
    ctx.type_stack.pop();
    result
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
    depth: usize,
    ctx: &mut SchemaValidationContext<'_>,
) -> Result<(), SyamlError> {
    let obj = match value.as_object() {
        Some(v) => v,
        None => return Ok(()),
    };

    let explicit_required = parse_required_property_set(schema, path)?;

    if let Some(required) = explicit_required.as_ref() {
        for key in required {
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
            let optional = property_is_optional(child_schema, path, k)?;
            let required = match explicit_required.as_ref() {
                // Legacy behavior: explicit `required` controls requiredness.
                Some(set) => set.contains(k.as_str()),
                // New default: all properties are required unless `optional: true`.
                None => !optional,
            };

            if required && !obj.contains_key(k) {
                return Err(SyamlError::SchemaError(format!(
                    "required property missing at {path}: '{k}'"
                )));
            }

            if let Some(child_value) = obj.get(k) {
                let child_path = format!("{}.{}", path, k);
                validate_json_against_schema_inner(
                    child_value,
                    child_schema,
                    &child_path,
                    depth + 1,
                    ctx,
                )?;
            }
        }
    }

    if let Some(values_schema) = schema.get("values") {
        if !values_schema.is_object() {
            return Err(SyamlError::SchemaError(format!(
                "values at {path} must be an object"
            )));
        }

        let prop_map = schema.get("properties").and_then(JsonValue::as_object);
        for (key, child_value) in obj {
            if prop_map.is_some_and(|props| props.contains_key(key)) {
                continue;
            }
            let child_path = format!("{}.{}", path, key);
            validate_json_against_schema_inner(
                child_value,
                values_schema,
                &child_path,
                depth + 1,
                ctx,
            )?;
        }
    }

    Ok(())
}

fn parse_required_property_set(
    schema: &serde_json::Map<String, JsonValue>,
    path: &str,
) -> Result<Option<HashSet<String>>, SyamlError> {
    let Some(required) = schema.get("required") else {
        return Ok(None);
    };

    let arr = required
        .as_array()
        .ok_or_else(|| SyamlError::SchemaError(format!("required at {path} must be an array")))?;

    let mut out = HashSet::new();
    for req in arr {
        let key = req.as_str().ok_or_else(|| {
            SyamlError::SchemaError(format!("required entries at {path} must be strings"))
        })?;
        out.insert(key.to_string());
    }

    Ok(Some(out))
}

fn property_is_optional(
    child_schema: &JsonValue,
    path: &str,
    key: &str,
) -> Result<bool, SyamlError> {
    let Some(child_obj) = child_schema.as_object() else {
        return Ok(false);
    };
    let Some(optional) = child_obj.get("optional") else {
        return Ok(false);
    };

    optional.as_bool().ok_or_else(|| {
        SyamlError::SchemaError(format!(
            "optional at {path}.properties.{key}.optional must be a boolean"
        ))
    })
}

fn validate_array_keywords(
    value: &JsonValue,
    schema: &serde_json::Map<String, JsonValue>,
    path: &str,
    depth: usize,
    ctx: &mut SchemaValidationContext<'_>,
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
            validate_json_against_schema_inner(item, items_schema, &child_path, depth + 1, ctx)?;
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

fn is_builtin_type_name(type_name: &str) -> bool {
    matches!(
        type_name,
        "string" | "integer" | "number" | "boolean" | "object" | "array" | "null"
    )
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
