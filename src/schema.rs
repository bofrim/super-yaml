//! Schema parsing and schema-based validation helpers.
//!
//! Supported keyword subset:
//! - Common: `type`, `enum`
//! - Numeric: `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum`
//! - String: `minLength`, `maxLength`, `pattern`
//! - Object: `properties`, `values`, `required`, `optional` (on property schemas), `constructors`
//! - Array: `items`, `minItems`, `maxItems`
//! - Version: `since`, `deprecated`, `removed`, `field_number`

use std::collections::{BTreeMap, HashSet, VecDeque};

use regex::Regex;
use serde_json::Value as JsonValue;

use crate::ast::SchemaDoc;
use crate::error::SyamlError;
use crate::expr::{parse_expression, parser::{Expr, BinaryOp}};

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

    // Pass 1: scan keys, build extends_map, insert types under base names.
    let mut types = BTreeMap::new();
    let mut extends_map: BTreeMap<String, String> = BTreeMap::new();
    for (k, v) in type_map {
        let (base_name, parent) = split_schema_key_and_parent(k)?;
        if let Some(p) = parent {
            extends_map.insert(base_name.clone(), p);
        }
        types.insert(base_name, normalize_schema_node(v.clone()));
    }

    // Pass 2: expand extends before collecting constraints.
    expand_extends_types(&mut types, &extends_map)?;

    let mut type_constraints = BTreeMap::new();
    for (type_name, type_schema) in &types {
        let mut collected = BTreeMap::new();
        collect_type_constraints(type_schema, "$", type_name, &mut collected)?;
        validate_type_constraint_variable_scope(type_name, type_schema, &collected, &types)?;
        if !collected.is_empty() {
            type_constraints.insert(type_name.clone(), collected);
        }
    }

    normalize_inline_schema_constraints(&types, &mut type_constraints);

    validate_versioned_field_annotations(&types)?;

    for (type_name, type_schema) in &types {
        validate_mutability_keywords(type_schema, &format!("schema.{}", type_name))?;
    }

    Ok(SchemaDoc {
        types,
        type_constraints,
        extends: extends_map,
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
    validate_schema_type_references_with_extends(types, &BTreeMap::new())
}

/// Like [`validate_schema_type_references`] but also accepts an `extends_map`
/// so that child types are treated as valid substitutes for their parent types
/// (IS-A relationship).
pub fn validate_schema_type_references_with_extends(
    types: &BTreeMap<String, JsonValue>,
    extends_map: &BTreeMap<String, String>,
) -> Result<(), SyamlError> {
    for (type_name, schema) in types {
        let root_path = format!("schema.{}", type_name);
        validate_schema_type_references_inner(schema, types, extends_map, &root_path)?;
    }
    Ok(())
}

fn validate_schema_type_references_inner(
    schema: &JsonValue,
    types: &BTreeMap<String, JsonValue>,
    extends_map: &BTreeMap<String, String>,
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
                    if type_name != "union"
                        && !is_builtin_type_name(type_name)
                        && !types.contains_key(type_name)
                        && !is_descendant_of_any(type_name, types, extends_map)
                    {
                        return Err(SyamlError::SchemaError(format!(
                            "unknown type reference at {child_path}: '{type_name}' not found in schema"
                        )));
                    }
                }
                validate_schema_type_references_inner(child, types, extends_map, &child_path)?;
            }
        }
        JsonValue::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                let item_path = format!("{path}[{index}]");
                validate_schema_type_references_inner(item, types, extends_map, &item_path)?;
            }
        }
        _ => {}
    }

    Ok(())
}

/// Returns true if `type_name` is a defined type or a descendant of any defined type.
fn is_descendant_of_any(
    type_name: &str,
    types: &BTreeMap<String, JsonValue>,
    extends_map: &BTreeMap<String, String>,
) -> bool {
    // Walk the extends chain upward; if we reach a known type, it's valid.
    let mut current = type_name;
    let mut seen = HashSet::new();
    loop {
        if !seen.insert(current) {
            return false; // cycle guard
        }
        if types.contains_key(current) {
            return true;
        }
        match extends_map.get(current) {
            Some(parent) => current = parent.as_str(),
            None => return false,
        }
    }
}

/// Splits a schema key like `ChildType <ParentType>` into `("ChildType", Some("ParentType"))`.
/// Keys without angle brackets return `(key, None)`.
fn split_schema_key_and_parent(key: &str) -> Result<(String, Option<String>), SyamlError> {
    let trimmed = key.trim();
    if !trimmed.ends_with('>') {
        return Ok((trimmed.to_string(), None));
    }

    let lt = match trimmed.rfind('<') {
        Some(i) => i,
        None => return Ok((trimmed.to_string(), None)),
    };

    if lt == 0 || lt + 1 >= trimmed.len() {
        return Ok((trimmed.to_string(), None));
    }

    let base = trimmed[..lt].trim_end();
    let parent = trimmed[lt + 1..trimmed.len() - 1].trim();

    if base.is_empty() {
        return Err(SyamlError::SchemaError(format!(
            "invalid extends syntax '{}': missing type name",
            key
        )));
    }
    if parent.is_empty() {
        return Err(SyamlError::SchemaError(format!(
            "invalid extends syntax '{}': missing parent type name",
            key
        )));
    }

    Ok((base.to_string(), Some(parent.to_string())))
}

/// Expands all child types in `types` by merging parent properties into them.
///
/// Expansion is done in topological order so that multi-level chains (A → B → C)
/// are handled correctly. After expansion each child is a standalone flat object
/// with all ancestor properties included.
fn expand_extends_types(
    types: &mut BTreeMap<String, JsonValue>,
    extends_map: &BTreeMap<String, String>,
) -> Result<(), SyamlError> {
    if extends_map.is_empty() {
        return Ok(());
    }

    // Validate that every parent exists.
    for (child, parent) in extends_map {
        if !types.contains_key(parent) {
            return Err(SyamlError::SchemaError(format!(
                "type '{}' extends unknown type '{}'",
                child, parent
            )));
        }
    }

    // Topological sort (Kahn's algorithm).
    // `degree[child]` = number of ancestors of `child` that are themselves children.
    let mut degree: BTreeMap<String, usize> = extends_map.keys().map(|k| (k.clone(), 0)).collect();
    for (child, parent) in extends_map {
        if degree.contains_key(parent.as_str()) {
            *degree.get_mut(child.as_str()).unwrap() += 1;
        }
    }

    let mut queue: VecDeque<String> = degree
        .iter()
        .filter(|(_, &d)| d == 0)
        .map(|(k, _)| k.clone())
        .collect();
    let mut order: Vec<String> = Vec::new();

    while let Some(node) = queue.pop_front() {
        order.push(node.clone());
        // Find all nodes whose parent is `node`.
        for (child, parent) in extends_map {
            if parent == &node {
                let d = degree.get_mut(child.as_str()).unwrap();
                *d -= 1;
                if *d == 0 {
                    queue.push_back(child.clone());
                }
            }
        }
    }

    if order.len() != degree.len() {
        // Cycle detected — collect involved type names.
        let cycle_nodes: Vec<String> = degree
            .into_keys()
            .filter(|k| !order.contains(k))
            .collect();
        let mut sorted = cycle_nodes;
        sorted.sort();
        return Err(SyamlError::SchemaError(format!(
            "circular type extension involving: {}",
            sorted.join(", ")
        )));
    }

    // Expand in topological order.
    for child_name in &order {
        let parent_name = &extends_map[child_name];

        // Clone parent schema before borrowing child mutably.
        let parent_schema = types[parent_name].clone();
        let child_schema = types[child_name].clone();

        let parent_obj = parent_schema.as_object().ok_or_else(|| {
            SyamlError::SchemaError(format!(
                "type '{}' extends '{}' which is not an object type",
                child_name, parent_name
            ))
        })?;
        if parent_obj.get("type").and_then(JsonValue::as_str) != Some("object") {
            return Err(SyamlError::SchemaError(format!(
                "type '{}' extends '{}' which is not an object type",
                child_name, parent_name
            )));
        }

        let child_obj = child_schema.as_object().ok_or_else(|| {
            SyamlError::SchemaError(format!(
                "only object types can use extends, but '{}' is not an object type",
                child_name
            ))
        })?;
        if child_obj.get("type").and_then(JsonValue::as_str) != Some("object") {
            return Err(SyamlError::SchemaError(format!(
                "only object types can use extends, but '{}' is not an object type",
                child_name
            )));
        }

        let parent_props = parent_obj
            .get("properties")
            .and_then(JsonValue::as_object)
            .cloned()
            .unwrap_or_default();

        let child_props = child_obj
            .get("properties")
            .and_then(JsonValue::as_object)
            .cloned()
            .unwrap_or_default();

        // Forbid redeclaration of parent fields.
        for field in child_props.keys() {
            if parent_props.contains_key(field.as_str()) {
                return Err(SyamlError::SchemaError(format!(
                    "type '{}' cannot redeclare field '{}' already defined in '{}'",
                    child_name, field, parent_name
                )));
            }
        }

        // Merge: parent properties first, then child properties.
        let mut merged_props = parent_props;
        merged_props.extend(child_props);

        // Merge `required` arrays (union, deduplicated).
        let parent_required: Vec<String> = parent_obj
            .get("required")
            .and_then(JsonValue::as_array)
            .map(|a| a.iter().filter_map(JsonValue::as_str).map(String::from).collect())
            .unwrap_or_default();
        let child_required: Vec<String> = child_obj
            .get("required")
            .and_then(JsonValue::as_array)
            .map(|a| a.iter().filter_map(JsonValue::as_str).map(String::from).collect())
            .unwrap_or_default();
        let mut merged_required: Vec<String> = parent_required;
        for r in child_required {
            if !merged_required.contains(&r) {
                merged_required.push(r);
            }
        }

        // Merge `constraints` (parent first, then child).
        let parent_constraints: Vec<JsonValue> = parent_obj
            .get("constraints")
            .and_then(JsonValue::as_array)
            .cloned()
            .unwrap_or_default();
        let child_constraints: Vec<JsonValue> = child_obj
            .get("constraints")
            .and_then(JsonValue::as_array)
            .cloned()
            .unwrap_or_default();
        let merged_constraints: Vec<JsonValue> = parent_constraints
            .into_iter()
            .chain(child_constraints)
            .collect();

        // Build the expanded child schema.
        let mut expanded = serde_json::Map::new();
        expanded.insert("type".to_string(), JsonValue::String("object".to_string()));
        if !merged_props.is_empty() {
            expanded.insert("properties".to_string(), JsonValue::Object(merged_props));
        }
        if !merged_required.is_empty() {
            expanded.insert(
                "required".to_string(),
                JsonValue::Array(merged_required.into_iter().map(JsonValue::String).collect()),
            );
        }
        if !merged_constraints.is_empty() {
            expanded.insert("constraints".to_string(), JsonValue::Array(merged_constraints));
        }
        // Copy over any other child-level keys (mutability, constructors, etc.) excluding
        // keys we've already handled.
        for (k, v) in child_obj {
            if !matches!(k.as_str(), "type" | "properties" | "required" | "constraints") {
                expanded.insert(k.clone(), v.clone());
            }
        }

        types.insert(child_name.clone(), JsonValue::Object(expanded));
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
            // Pipe shorthand: "TypeA | TypeB | TypeC" expands to a union.
            if type_name.contains(" | ") {
                let options: Vec<JsonValue> = type_name
                    .split(" | ")
                    .map(|part| {
                        let trimmed = part.trim();
                        normalize_schema_node(JsonValue::String(trimmed.to_string()))
                    })
                    .collect();
                let mut out = serde_json::Map::new();
                out.insert("type".to_string(), JsonValue::String("union".to_string()));
                out.insert("options".to_string(), JsonValue::Array(options));
                return JsonValue::Object(out);
            }

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

            // Normalize union options (array or map values).
            if map.get("type").and_then(JsonValue::as_str) == Some("union") {
                if let Some(options) = map.get_mut("options") {
                    match options {
                        JsonValue::Array(items) => {
                            for item in items.iter_mut() {
                                let normalized = normalize_schema_node(item.clone());
                                *item = normalized;
                            }
                        }
                        JsonValue::Object(opt_map) => {
                            for opt_schema in opt_map.values_mut() {
                                let normalized = normalize_schema_node(opt_schema.clone());
                                *opt_schema = normalized;
                            }
                        }
                        _ => {}
                    }
                }
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
                let raw_eq = rule.get("eq");
                let set_count = [raw_decode, raw_from_enum, raw_eq]
                    .iter()
                    .filter(|v| v.is_some())
                    .count();
                if set_count > 1 {
                    return Err(SyamlError::SchemaError(format!(
                        "{rule_path} can only set one of 'decode', 'from_enum', or 'eq'"
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
                if let Some(raw_eq) = raw_eq {
                    raw_eq.as_str().ok_or_else(|| {
                        SyamlError::SchemaError(format!("{rule_path}.eq must be a string"))
                    })?;
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

    // Recurse into union options.
    if let Some(options) = schema_obj.get("options") {
        match options {
            JsonValue::Array(items) => {
                for item in items {
                    collect_type_constraints(item, current_path, type_name, out)?;
                }
            }
            JsonValue::Object(opt_map) => {
                for opt_schema in opt_map.values() {
                    collect_type_constraints(opt_schema, current_path, type_name, out)?;
                }
            }
            _ => {}
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

/// Converts inline JSON Schema keywords (`minimum`, `maxLength`, `minItems`,
/// etc.) into expression strings and injects them into `type_constraints`.
/// This allows inline schema constraint keywords to feed through the same
/// constraint code generation pipeline as the explicit `constraints` section.
fn normalize_inline_schema_constraints(
    types: &BTreeMap<String, JsonValue>,
    type_constraints: &mut BTreeMap<String, BTreeMap<String, Vec<String>>>,
) {
    for (type_name, schema) in types {
        let mut injected: BTreeMap<String, Vec<String>> = BTreeMap::new();
        collect_inline_schema_exprs(schema, "$", &mut injected);
        if !injected.is_empty() {
            let entry = type_constraints.entry(type_name.clone()).or_default();
            for (path, exprs) in injected {
                entry.entry(path).or_default().extend(exprs);
            }
        }
    }
}

/// Recursively walks a schema node and collects constraint expressions derived
/// from inline JSON Schema keywords. Results are stored at the JSON path where
/// each keyword was found, mirroring the path-mapped format used by the
/// explicit `constraints` section.
fn collect_inline_schema_exprs(
    schema: &JsonValue,
    path: &str,
    out: &mut BTreeMap<String, Vec<String>>,
) {
    let Some(obj) = schema.as_object() else {
        return;
    };

    let mut exprs = Vec::new();

    if let Some(n) = obj.get("minimum").and_then(JsonValue::as_f64) {
        exprs.push(format!("value >= {}", format_schema_number(n)));
    }
    if let Some(n) = obj.get("maximum").and_then(JsonValue::as_f64) {
        exprs.push(format!("value <= {}", format_schema_number(n)));
    }
    if let Some(n) = obj.get("exclusiveMinimum").and_then(JsonValue::as_f64) {
        exprs.push(format!("value > {}", format_schema_number(n)));
    }
    if let Some(n) = obj.get("exclusiveMaximum").and_then(JsonValue::as_f64) {
        exprs.push(format!("value < {}", format_schema_number(n)));
    }
    // multipleOf uses integer modulo in the expression engine; skip non-integer divisors
    if let Some(n) = obj.get("multipleOf").and_then(JsonValue::as_i64) {
        if n > 0 {
            exprs.push(format!("value % {} == 0", n));
        }
    }

    if let Some(n) = obj.get("minLength").and_then(JsonValue::as_u64) {
        exprs.push(format!("len(value) >= {}", n));
    }
    if let Some(n) = obj.get("maxLength").and_then(JsonValue::as_u64) {
        exprs.push(format!("len(value) <= {}", n));
    }
    if let Some(n) = obj.get("minItems").and_then(JsonValue::as_u64) {
        exprs.push(format!("len(value) >= {}", n));
    }
    if let Some(n) = obj.get("maxItems").and_then(JsonValue::as_u64) {
        exprs.push(format!("len(value) <= {}", n));
    }

    if !exprs.is_empty() {
        append_constraints(out, path, exprs);
    }

    // Recurse into object properties to handle constraints on nested fields
    if let Some(props) = obj.get("properties").and_then(JsonValue::as_object) {
        for (key, child_schema) in props {
            let child_path = if path == "$" {
                format!("$.{}", key)
            } else {
                format!("{}.{}", path, key)
            };
            collect_inline_schema_exprs(child_schema, &child_path, out);
        }
    }
}

/// Formats a float from a JSON Schema numeric keyword as an integer string when
/// the value is whole, to produce clean expressions like `value >= 1` rather
/// than `value >= 1`.
fn format_schema_number(n: f64) -> String {
    if n.fract() == 0.0 && n >= i64::MIN as f64 && n <= i64::MAX as f64 {
        format!("{}", n as i64)
    } else {
        format!("{}", n)
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

            // Validate constraint operations against schema types
            validate_constraint_operations(&ast, scope_schema, expression, type_name, constraint_path, types)?;
        }
    }

    Ok(())
}

fn validate_constraint_operations(
    expr: &Expr,
    scope_schema: &JsonValue,
    expression: &str,
    type_name: &str,
    constraint_path: &str,
    types: &BTreeMap<String, JsonValue>,
) -> Result<(), SyamlError> {
    use crate::expr::parser::BinaryOp;

    match expr {
        Expr::Binary { op, left, right } => {
            match op {
                BinaryOp::Lt | BinaryOp::Lte | BinaryOp::Gt | BinaryOp::Gte => {
                    // Comparison operators require numeric operands
                    // String comparisons are not allowed as they are unreliable and error-prone
                    let left_type = infer_expression_type(left, scope_schema, types);
                    let right_type = infer_expression_type(right, scope_schema, types);

                    if !is_numeric_type(&left_type) || !is_numeric_type(&right_type) {
                        return Err(SyamlError::SchemaError(format!(
                            "constraint '{}' at schema.{} path '{}' uses '{}' operator with non-numeric types (left: {}, right: {})",
                            expression, type_name, constraint_path, op_symbol(*op),
                            left_type.as_deref().unwrap_or("unknown"), right_type.as_deref().unwrap_or("unknown")
                        )));
                    }
                }
                BinaryOp::Eq | BinaryOp::NotEq => {
                    // Equality operators are generally allowed for any types
                }
                BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => {
                    // Arithmetic operators require numeric operands
                    let left_type = infer_expression_type(left, scope_schema, types);

                    let right_type = infer_expression_type(right, scope_schema, types);

                    if !is_numeric_type(&left_type) || !is_numeric_type(&right_type) {
                        return Err(SyamlError::SchemaError(format!(
                            "constraint '{}' at schema.{} path '{}' uses '{}' operator with non-numeric types (left: {}, right: {})",
                            expression, type_name, constraint_path, op_symbol(*op),
                            left_type.as_deref().unwrap_or("unknown"), right_type.as_deref().unwrap_or("unknown")
                        )));
                    }
                }
                _ => {
                    // Other operators are allowed
                }
            }

            // Recursively validate sub-expressions
            validate_constraint_operations(left, scope_schema, expression, type_name, constraint_path, types)?;
            validate_constraint_operations(right, scope_schema, expression, type_name, constraint_path, types)?;
        }
        Expr::Unary { expr: inner, .. } => {
            // Recursively validate inner expression
            validate_constraint_operations(inner, scope_schema, expression, type_name, constraint_path, types)?;
        }
        Expr::Call { name, args } => {
            // Handle known functions that return specific types
            match name.as_str() {
                "len" => {
                    // len() returns a number - this is valid for comparisons
                }
                _ => {
                    // Unknown function - could return any type
                }
            }

            // Validate arguments
            for arg in args {
                validate_constraint_operations(arg, scope_schema, expression, type_name, constraint_path, types)?;
            }
        }
        _ => {
            // Other expression types don't need validation
        }
    }

    Ok(())
}

fn infer_expression_type(
    expr: &Expr,
    scope_schema: &JsonValue,
    types: &BTreeMap<String, JsonValue>,
) -> Option<String> {

    match expr {
        Expr::Var(path) => {
            if path.is_empty() {
                return None;
            }
            let first = &path[0];
            if first == "value" {
                // This is the current value being constrained
                return infer_schema_type(scope_schema, types);
            } else {
                // This is a property access
                let mut current_schema = scope_schema;
                for segment in path {
                    if let Some(obj) = current_schema.as_object() {
                        if let Some(props) = obj.get("properties") {
                            if let Some(prop_schema) = props.as_object().and_then(|p| p.get(segment)) {
                                current_schema = prop_schema;
                                continue;
                            }
                        }
                    }
                    return None; // Path not found
                }
                return infer_schema_type(current_schema, types);
            }
        }
        Expr::Binary { op, left, right } => {
            let left_type = infer_expression_type(left, scope_schema, types);
            let right_type = infer_expression_type(right, scope_schema, types);

            match op {
                BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => {
                    // Arithmetic operations result in numbers if both operands are numbers
                    if is_numeric_type(&left_type) && is_numeric_type(&right_type) {
                        Some("number".to_string())
                    } else {
                        None
                    }
                }
                BinaryOp::Lt | BinaryOp::Lte | BinaryOp::Gt | BinaryOp::Gte => {
                    // Comparison operations result in booleans
                    Some("boolean".to_string())
                }
                BinaryOp::Eq | BinaryOp::NotEq => {
                    // Equality operations result in booleans
                    Some("boolean".to_string())
                }
                BinaryOp::And | BinaryOp::Or => {
                    // Logical operations result in booleans
                    Some("boolean".to_string())
                }
            }
        }
        Expr::Call { name, .. } => {
            // Handle known functions that return specific types
            match name.as_str() {
                "len" => Some("number".to_string()), // len() returns a number
                "min" | "max" => Some("number".to_string()), // min/max return numbers
                _ => None, // Unknown function return type
            }
        }
        Expr::Number(_) => Some("number".to_string()),
        Expr::String(_) => Some("string".to_string()),
        Expr::Bool(_) => Some("boolean".to_string()),
        _ => None,
    }
}

fn infer_schema_type(schema: &JsonValue, types: &BTreeMap<String, JsonValue>) -> Option<String> {
    if let Some(type_val) = schema.as_object().and_then(|o| o.get("type")) {
        if let Some(type_str) = type_val.as_str() {
            // Check if this is a reference to a named type
            if let Some(named_type) = types.get(type_str) {
                return infer_schema_type(named_type, types);
            } else if type_str.contains('.') {
                // Handle imported types like "prim.NonNegativeInteger"
                // For now, infer based on common patterns
                if type_str.contains("Integer") {
                    return Some("integer".to_string());
                } else if type_str.contains("Number") {
                    return Some("number".to_string());
                } else if type_str.contains("String") {
                    return Some("string".to_string());
                }
                // Unknown imported type
                return None;
            } else {
                // It's a built-in type like "string", "number", etc.
                return Some(type_str.to_string());
            }
        }
    }
    if let Some(ref_val) = schema.as_object().and_then(|o| o.get("$ref")) {
        if let Some(ref_str) = ref_val.as_str() {
            // Handle explicit type references
            if let Some(referenced) = types.get(ref_str) {
                return infer_schema_type(referenced, types);
            }
        }
    }
    None
}

fn is_numeric_type(type_name: &Option<String>) -> bool {
    matches!(type_name.as_deref(), Some("number") | Some("integer"))
}


fn op_symbol(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Mod => "%",
        BinaryOp::Eq => "==",
        BinaryOp::NotEq => "!=",
        BinaryOp::Lt => "<",
        BinaryOp::Lte => "<=",
        BinaryOp::Gt => ">",
        BinaryOp::Gte => ">=",
        BinaryOp::And => "&&",
        BinaryOp::Or => "||",
    }
}

pub(crate) fn collect_var_paths(expr: &Expr, out: &mut Vec<Vec<String>>) {
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
        if is_builtin_type_name(type_name) || type_name == "union" {
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
        if type_name == "union" {
            return validate_union_type(value, schema_obj, path, depth, ctx);
        }
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

fn validate_union_type(
    value: &JsonValue,
    schema_obj: &serde_json::Map<String, JsonValue>,
    path: &str,
    depth: usize,
    ctx: &mut SchemaValidationContext<'_>,
) -> Result<(), SyamlError> {
    let options = schema_obj.get("options").ok_or_else(|| {
        SyamlError::SchemaError(format!(
            "union type at {path} requires 'options' (array or object)"
        ))
    })?;

    let tag_key = schema_obj.get("tag").and_then(JsonValue::as_str);
    let tag_required = schema_obj
        .get("tag_required")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false);

    // If tag dispatch is configured, try tag-based lookup first.
    if let Some(tag) = tag_key {
        if let Some(data_obj) = value.as_object() {
            if let Some(tag_value) = data_obj.get(tag).and_then(JsonValue::as_str) {
                // Map-based options: look up by key.
                if let Some(opt_map) = options.as_object() {
                    if let Some(matched_schema) = opt_map.get(tag_value) {
                        return validate_json_against_schema_inner(
                            value,
                            matched_schema,
                            path,
                            depth + 1,
                            ctx,
                        );
                    }
                }
            } else if tag_required {
                return Err(SyamlError::SchemaError(format!(
                    "union tag field '{}' is required but missing or not a string at {path}",
                    tag
                )));
            }
        } else if tag_required {
            return Err(SyamlError::SchemaError(format!(
                "union tag field '{}' is required but value at {path} is not an object",
                tag
            )));
        }
    }

    // Ordered matching: try each option, first success wins.
    let option_schemas: Vec<&JsonValue> = match options {
        JsonValue::Array(items) => items.iter().collect(),
        JsonValue::Object(map) => map.values().collect(),
        _ => {
            return Err(SyamlError::SchemaError(format!(
                "union 'options' at {path} must be an array or object"
            )));
        }
    };

    let mut errors = Vec::new();
    for option_schema in &option_schemas {
        match validate_json_against_schema_inner(value, option_schema, path, depth + 1, ctx) {
            Ok(()) => return Ok(()),
            Err(e) => errors.push(e.to_string()),
        }
    }

    Err(SyamlError::SchemaError(format!(
        "union mismatch at {path}: value did not match any option. Errors: [{}]",
        errors.join("; ")
    )))
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

// ── Field-level versioning metadata ──────────────────────────────────────────

/// Lifecycle metadata that can be attached to a property schema.
#[derive(Debug, Clone)]
pub struct FieldVersionMeta {
    /// Version in which the field was introduced.
    pub since: Option<semver::Version>,
    /// Deprecation information, if the field has been deprecated.
    pub deprecated: Option<DeprecationInfo>,
    /// Version in which the field was removed.
    pub removed: Option<semver::Version>,
    /// Stable numeric field identity (protobuf-style).
    pub field_number: Option<u64>,
}

/// Information about a field's deprecation.
#[derive(Debug, Clone)]
pub struct DeprecationInfo {
    /// Version at which the field was deprecated.
    pub version: semver::Version,
    /// How violations should be surfaced.
    pub severity: DeprecationSeverity,
    /// Optional human-readable message.
    pub message: Option<String>,
}

/// Controls whether a deprecation produces a warning or a hard error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeprecationSeverity {
    Warning,
    Error,
}

/// Extracts versioning metadata from a property schema object.
///
/// Returns `Ok(None)` for fields with no version annotations.
pub fn parse_field_version_meta(
    property_schema: &JsonValue,
) -> Result<Option<FieldVersionMeta>, SyamlError> {
    let Some(obj) = property_schema.as_object() else {
        return Ok(None);
    };

    let has_since = obj.contains_key("since");
    let has_deprecated = obj.contains_key("deprecated");
    let has_removed = obj.contains_key("removed");
    let has_field_number = obj.contains_key("field_number");

    if !has_since && !has_deprecated && !has_removed && !has_field_number {
        return Ok(None);
    }

    let since = if let Some(v) = obj.get("since") {
        let s = v.as_str().ok_or_else(|| {
            SyamlError::SchemaError("'since' must be a semver string".to_string())
        })?;
        Some(
            semver::Version::parse(s)
                .map_err(|e| SyamlError::SchemaError(format!("invalid 'since' version: {e}")))?,
        )
    } else {
        None
    };

    let removed = if let Some(v) = obj.get("removed") {
        let s = v.as_str().ok_or_else(|| {
            SyamlError::SchemaError("'removed' must be a semver string".to_string())
        })?;
        Some(
            semver::Version::parse(s)
                .map_err(|e| SyamlError::SchemaError(format!("invalid 'removed' version: {e}")))?,
        )
    } else {
        None
    };

    let deprecated = if let Some(dep_val) = obj.get("deprecated") {
        Some(parse_deprecation_info(dep_val)?)
    } else {
        None
    };

    let field_number = if let Some(fn_val) = obj.get("field_number") {
        let n = fn_val.as_u64().ok_or_else(|| {
            SyamlError::SchemaError(
                "'field_number' must be a positive integer".to_string(),
            )
        })?;
        if n == 0 {
            return Err(SyamlError::SchemaError(
                "'field_number' must be a positive integer (> 0)".to_string(),
            ));
        }
        Some(n)
    } else {
        None
    };

    // Validate ordering: since <= deprecated.version <= removed
    if let (Some(ref s), Some(ref d)) = (&since, &deprecated) {
        if s > &d.version {
            return Err(SyamlError::SchemaError(format!(
                "version ordering violation: 'since' ({}) must be <= 'deprecated' ({})",
                s, d.version
            )));
        }
    }
    if let (Some(ref d), Some(ref r)) = (&deprecated, &removed) {
        if &d.version > r {
            return Err(SyamlError::SchemaError(format!(
                "version ordering violation: 'deprecated' ({}) must be <= 'removed' ({})",
                d.version, r
            )));
        }
    }
    if let (Some(ref s), Some(ref r)) = (&since, &removed) {
        if s > r {
            return Err(SyamlError::SchemaError(format!(
                "version ordering violation: 'since' ({}) must be <= 'removed' ({})",
                s, r
            )));
        }
    }

    // Removed fields must be optional
    if removed.is_some() {
        let is_optional = obj
            .get("optional")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        if !is_optional {
            return Err(SyamlError::SchemaError(
                "a field with 'removed' must also have 'optional: true'".to_string(),
            ));
        }
    }

    Ok(Some(FieldVersionMeta {
        since,
        deprecated,
        removed,
        field_number,
    }))
}

fn parse_deprecation_info(value: &JsonValue) -> Result<DeprecationInfo, SyamlError> {
    match value {
        JsonValue::String(s) => {
            let version = semver::Version::parse(s).map_err(|e| {
                SyamlError::SchemaError(format!("invalid 'deprecated' version: {e}"))
            })?;
            Ok(DeprecationInfo {
                version,
                severity: DeprecationSeverity::Warning,
                message: None,
            })
        }
        JsonValue::Object(map) => {
            let version_str = map
                .get("version")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| {
                    SyamlError::SchemaError(
                        "'deprecated' object must have a 'version' string".to_string(),
                    )
                })?;
            let version = semver::Version::parse(version_str).map_err(|e| {
                SyamlError::SchemaError(format!("invalid 'deprecated.version': {e}"))
            })?;

            let severity = if let Some(sev_val) = map.get("severity") {
                let sev_str = sev_val.as_str().ok_or_else(|| {
                    SyamlError::SchemaError(
                        "'deprecated.severity' must be a string".to_string(),
                    )
                })?;
                match sev_str {
                    "warning" => DeprecationSeverity::Warning,
                    "error" => DeprecationSeverity::Error,
                    other => {
                        return Err(SyamlError::SchemaError(format!(
                            "invalid 'deprecated.severity' value '{}'; expected 'warning' or 'error'",
                            other
                        )));
                    }
                }
            } else {
                DeprecationSeverity::Warning
            };

            let message = map
                .get("message")
                .and_then(JsonValue::as_str)
                .map(String::from);

            Ok(DeprecationInfo {
                version,
                severity,
                message,
            })
        }
        _ => Err(SyamlError::SchemaError(
            "'deprecated' must be a semver string or an object with 'version'".to_string(),
        )),
    }
}

/// Validates versioned field annotations for all types in the schema.
///
/// Checks that:
/// - `since`, `deprecated`, `removed` are valid semver strings
/// - Version ordering holds: `since <= deprecated.version <= removed`
/// - Fields with `removed` have `optional: true`
/// - `field_number` values are unique within each type
pub fn validate_versioned_field_annotations(
    types: &BTreeMap<String, JsonValue>,
) -> Result<(), SyamlError> {
    for (type_name, type_schema) in types {
        let Some(obj) = type_schema.as_object() else {
            continue;
        };
        let Some(props) = obj.get("properties").and_then(JsonValue::as_object) else {
            continue;
        };

        let mut seen_field_numbers: BTreeMap<u64, String> = BTreeMap::new();
        for (prop_name, prop_schema) in props {
            let meta = parse_field_version_meta(prop_schema).map_err(|e| {
                SyamlError::SchemaError(format!(
                    "schema.{}.properties.{}: {}",
                    type_name,
                    prop_name,
                    e
                ))
            })?;
            if let Some(meta) = meta {
                if let Some(fn_num) = meta.field_number {
                    if let Some(existing) = seen_field_numbers.get(&fn_num) {
                        return Err(SyamlError::SchemaError(format!(
                            "schema.{}: duplicate field_number {} on properties '{}' and '{}'",
                            type_name, fn_num, existing, prop_name
                        )));
                    }
                    seen_field_numbers.insert(fn_num, prop_name.clone());
                }
            }
        }
    }
    Ok(())
}

/// Validates that every property of every object type in the registry has a `field_number`.
///
/// This is called when `meta.file.strict_field_numbers: true` is set. It covers all types
/// including those merged from imported documents.
pub fn validate_strict_field_numbers(types: &BTreeMap<String, JsonValue>) -> Result<(), SyamlError> {
    for (type_name, type_schema) in types {
        let Some(obj) = type_schema.as_object() else {
            continue;
        };
        let Some(props) = obj.get("properties").and_then(JsonValue::as_object) else {
            continue;
        };
        for (prop_name, prop_schema) in props {
            let has_field_number = prop_schema
                .as_object()
                .and_then(|o| o.get("field_number"))
                .is_some();
            if !has_field_number {
                return Err(SyamlError::SchemaError(format!(
                    "strict_field_numbers: type '{}' property '{}' is missing a field_number",
                    type_name, prop_name
                )));
            }
        }
    }
    Ok(())
}

use crate::ast::MutabilityMode;

/// Parses the `mutability` keyword from a schema node.
pub fn parse_mutability_mode(schema_node: &JsonValue) -> Result<MutabilityMode, SyamlError> {
    let obj = schema_node.as_object().ok_or_else(|| {
        SyamlError::MutabilityError("schema node must be an object to have mutability".to_string())
    })?;
    let raw = obj.get("mutability").ok_or_else(|| {
        SyamlError::MutabilityError("no mutability keyword found on schema node".to_string())
    })?;
    let s = raw.as_str().ok_or_else(|| {
        SyamlError::MutabilityError("mutability must be a string".to_string())
    })?;
    match s {
        "frozen" => Ok(MutabilityMode::Frozen),
        "replace" => Ok(MutabilityMode::Replace),
        "append_only" => Ok(MutabilityMode::AppendOnly),
        "map_put_only" => Ok(MutabilityMode::MapPutOnly),
        "monotone_increase" => Ok(MutabilityMode::MonotoneIncrease),
        other => Err(SyamlError::MutabilityError(format!(
            "unknown mutability mode '{}'; expected frozen, replace, append_only, map_put_only, or monotone_increase",
            other
        ))),
    }
}

/// Validates mutability keyword semantic rules across all schema types.
pub fn validate_mutability_keywords(schema: &JsonValue, path: &str) -> Result<(), SyamlError> {
    let Some(obj) = schema.as_object() else {
        return Ok(());
    };
    if let Some(raw) = obj.get("mutability") {
        let mode = parse_mutability_mode(schema)?;
        let type_name = obj.get("type").and_then(JsonValue::as_str);
        match mode {
            MutabilityMode::AppendOnly => {
                if type_name != Some("array") {
                    return Err(SyamlError::MutabilityError(format!(
                        "mutability 'append_only' at {} requires type: array",
                        path
                    )));
                }
            }
            MutabilityMode::MapPutOnly => {
                if type_name != Some("object") {
                    return Err(SyamlError::MutabilityError(format!(
                        "mutability 'map_put_only' at {} requires type: object",
                        path
                    )));
                }
            }
            MutabilityMode::MonotoneIncrease => {
                if !matches!(type_name, Some("integer") | Some("number")) {
                    return Err(SyamlError::MutabilityError(format!(
                        "mutability 'monotone_increase' at {} requires type: integer or number",
                        path
                    )));
                }
            }
            _ => {}
        }
        let _ = raw; // suppress unused warning
    }
    // Recurse into properties, items, values
    if let Some(props) = obj.get("properties").and_then(JsonValue::as_object) {
        for (k, v) in props {
            validate_mutability_keywords(v, &format!("{}.properties.{}", path, k))?;
        }
    }
    if let Some(items) = obj.get("items") {
        validate_mutability_keywords(items, &format!("{}.items", path))?;
    }
    if let Some(values) = obj.get("values") {
        validate_mutability_keywords(values, &format!("{}.values", path))?;
    }
    Ok(())
}

/// Walks a dot-separated data path through type hints + schema to find the effective mutability mode.
pub fn resolve_mutability_for_path(
    path: &str,
    type_hints: &std::collections::BTreeMap<String, String>,
    schema: &crate::ast::SchemaDoc,
) -> Result<MutabilityMode, SyamlError> {
    // First look for direct type hint on this path
    if let Some(type_name) = type_hints.get(path) {
        if let Some(schema_node) = schema.types.get(type_name.as_str()) {
            if schema_node.as_object().and_then(|o| o.get("mutability")).is_some() {
                return parse_mutability_mode(schema_node);
            }
        }
    }
    // Walk parent paths: try $.a.b -> $.a -> $
    let segments: Vec<&str> = path.split('.').collect();
    for len in (1..segments.len()).rev() {
        let parent_path: String = segments[..len].join(".");
        if let Some(type_name) = type_hints.get(&parent_path) {
            if let Some(schema_node) = schema.types.get(type_name.as_str()) {
                if schema_node.as_object().and_then(|o| o.get("mutability")).is_some() {
                    return parse_mutability_mode(schema_node);
                }
            }
        }
    }
    Ok(MutabilityMode::Replace) // default: mutable
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(schema_json: serde_json::Value) -> SchemaDoc {
        parse_schema(&schema_json).unwrap()
    }

    #[test]
    fn numeric_keywords_produce_constraint_expressions() {
        let schema = json!({
            "Price": {
                "type": "number",
                "minimum": 0,
                "maximum": 9999,
                "exclusiveMinimum": -1,
                "exclusiveMaximum": 10000
            }
        });
        let doc = parse(schema);
        let exprs = doc.type_constraints["Price"]["$"].clone();
        assert!(exprs.contains(&"value >= 0".to_string()), "{:?}", exprs);
        assert!(exprs.contains(&"value <= 9999".to_string()), "{:?}", exprs);
        assert!(exprs.contains(&"value > -1".to_string()), "{:?}", exprs);
        assert!(exprs.contains(&"value < 10000".to_string()), "{:?}", exprs);
    }

    #[test]
    fn multiple_of_produces_modulo_expression() {
        let schema = json!({
            "EvenNum": {
                "type": "integer",
                "multipleOf": 2
            }
        });
        let doc = parse(schema);
        let exprs = &doc.type_constraints["EvenNum"]["$"];
        assert!(
            exprs.contains(&"value % 2 == 0".to_string()),
            "{:?}",
            exprs
        );
    }

    #[test]
    fn string_length_keywords_produce_len_expressions() {
        let schema = json!({
            "Tag": {
                "type": "string",
                "minLength": 1,
                "maxLength": 64
            }
        });
        let doc = parse(schema);
        let exprs = &doc.type_constraints["Tag"]["$"];
        assert!(
            exprs.contains(&"len(value) >= 1".to_string()),
            "{:?}",
            exprs
        );
        assert!(
            exprs.contains(&"len(value) <= 64".to_string()),
            "{:?}",
            exprs
        );
    }

    #[test]
    fn array_item_count_keywords_produce_len_expressions() {
        let schema = json!({
            "Tags": {
                "type": "array",
                "items": { "type": "string" },
                "minItems": 1,
                "maxItems": 10
            }
        });
        let doc = parse(schema);
        let exprs = &doc.type_constraints["Tags"]["$"];
        assert!(
            exprs.contains(&"len(value) >= 1".to_string()),
            "{:?}",
            exprs
        );
        assert!(
            exprs.contains(&"len(value) <= 10".to_string()),
            "{:?}",
            exprs
        );
    }

    #[test]
    fn nested_field_keywords_produce_path_mapped_expressions() {
        let schema = json!({
            "Config": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "minLength": 1, "maxLength": 100 },
                    "count": { "type": "integer", "minimum": 0, "maximum": 255 }
                }
            }
        });
        let doc = parse(schema);
        let constraints = &doc.type_constraints["Config"];
        let name_exprs = &constraints["$.name"];
        assert!(
            name_exprs.contains(&"len(value) >= 1".to_string()),
            "{:?}",
            name_exprs
        );
        assert!(
            name_exprs.contains(&"len(value) <= 100".to_string()),
            "{:?}",
            name_exprs
        );
        let count_exprs = &constraints["$.count"];
        assert!(
            count_exprs.contains(&"value >= 0".to_string()),
            "{:?}",
            count_exprs
        );
        assert!(
            count_exprs.contains(&"value <= 255".to_string()),
            "{:?}",
            count_exprs
        );
    }

    #[test]
    fn inline_keywords_coexist_with_explicit_constraints_section() {
        let schema = json!({
            "Port": {
                "type": "integer",
                "minimum": 1,
                "maximum": 65535,
                "constraints": "value != 0"
            }
        });
        let doc = parse(schema);
        let exprs = &doc.type_constraints["Port"]["$"];
        assert!(exprs.contains(&"value >= 1".to_string()), "{:?}", exprs);
        assert!(exprs.contains(&"value <= 65535".to_string()), "{:?}", exprs);
        assert!(exprs.contains(&"value != 0".to_string()), "{:?}", exprs);
    }

    #[test]
    fn float_boundary_is_formatted_cleanly() {
        let schema = json!({
            "Rate": {
                "type": "number",
                "minimum": 0.5,
                "maximum": 1.0
            }
        });
        let doc = parse(schema);
        let exprs = &doc.type_constraints["Rate"]["$"];
        assert!(
            exprs.contains(&"value >= 0.5".to_string()),
            "{:?}",
            exprs
        );
        assert!(
            exprs.contains(&"value <= 1".to_string()),
            "{:?}",
            exprs
        );
    }

    #[test]
    fn type_with_no_inline_keywords_has_no_injected_constraints() {
        let schema = json!({
            "Label": {
                "type": "string"
            }
        });
        let doc = parse(schema);
        assert!(
            !doc.type_constraints.contains_key("Label"),
            "expected no constraints, got {:?}",
            doc.type_constraints
        );
    }

    // ── extends tests ─────────────────────────────────────────────────────────

    fn parse_err(schema_json: serde_json::Value) -> String {
        parse_schema(&schema_json).unwrap_err().to_string()
    }

    #[test]
    fn extends_basic_child_gets_parent_fields() {
        let schema = json!({
            "Animal": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "age": { "type": "integer" }
                }
            },
            "Dog <Animal>": {
                "type": "object",
                "properties": {
                    "breed": { "type": "string" }
                }
            }
        });
        let doc = parse(schema);
        let dog = doc.types.get("Dog").unwrap().as_object().unwrap().clone();
        let props = dog["properties"].as_object().unwrap();
        assert!(props.contains_key("name"), "should inherit 'name'");
        assert!(props.contains_key("age"), "should inherit 'age'");
        assert!(props.contains_key("breed"), "should have own 'breed'");
        assert!(doc.extends.contains_key("Dog"));
        assert_eq!(doc.extends["Dog"], "Animal");
    }

    #[test]
    fn extends_child_adds_new_field_no_error() {
        let schema = json!({
            "Base": {
                "type": "object",
                "properties": { "id": { "type": "string" } }
            },
            "Extended <Base>": {
                "type": "object",
                "properties": { "extra": { "type": "integer" } }
            }
        });
        let doc = parse(schema);
        let ext = doc.types.get("Extended").unwrap().as_object().unwrap().clone();
        let props = ext["properties"].as_object().unwrap();
        assert!(props.contains_key("id"));
        assert!(props.contains_key("extra"));
    }

    #[test]
    fn extends_child_redeclares_parent_field_errors() {
        let schema = json!({
            "Base": {
                "type": "object",
                "properties": { "id": { "type": "string" } }
            },
            "Child <Base>": {
                "type": "object",
                "properties": { "id": { "type": "integer" } }
            }
        });
        let err = parse_err(schema);
        assert!(
            err.contains("cannot redeclare field 'id'"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn extends_unknown_parent_errors() {
        let schema = json!({
            "Child <NoSuchType>": {
                "type": "object",
                "properties": { "x": { "type": "string" } }
            }
        });
        let err = parse_err(schema);
        assert!(
            err.contains("extends unknown type 'NoSuchType'"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn extends_non_object_parent_errors() {
        let schema = json!({
            "MyStr": { "type": "string" },
            "Child <MyStr>": {
                "type": "object",
                "properties": { "x": { "type": "string" } }
            }
        });
        let err = parse_err(schema);
        assert!(
            err.contains("not an object type"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn extends_non_object_child_errors() {
        let schema = json!({
            "Base": {
                "type": "object",
                "properties": { "id": { "type": "string" } }
            },
            "Child <Base>": { "type": "string" }
        });
        let err = parse_err(schema);
        assert!(
            err.contains("not an object type"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn extends_circular_errors() {
        let schema = json!({
            "A <B>": {
                "type": "object",
                "properties": { "a": { "type": "string" } }
            },
            "B <A>": {
                "type": "object",
                "properties": { "b": { "type": "string" } }
            }
        });
        let err = parse_err(schema);
        assert!(
            err.contains("circular type extension"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn extends_multilevel_chain() {
        let schema = json!({
            "A": {
                "type": "object",
                "properties": { "a_field": { "type": "string" } }
            },
            "B <A>": {
                "type": "object",
                "properties": { "b_field": { "type": "integer" } }
            },
            "C <B>": {
                "type": "object",
                "properties": { "c_field": { "type": "boolean" } }
            }
        });
        let doc = parse(schema);
        let c = doc.types.get("C").unwrap().as_object().unwrap().clone();
        let props = c["properties"].as_object().unwrap();
        assert!(props.contains_key("a_field"), "C should inherit a_field from A");
        assert!(props.contains_key("b_field"), "C should inherit b_field from B");
        assert!(props.contains_key("c_field"), "C should have own c_field");
    }

    #[test]
    fn extends_required_merged() {
        let schema = json!({
            "Base": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" }
                },
                "required": ["id"]
            },
            "Child <Base>": {
                "type": "object",
                "properties": {
                    "extra": { "type": "string" }
                },
                "required": ["extra"]
            }
        });
        let doc = parse(schema);
        let child = doc.types.get("Child").unwrap().as_object().unwrap().clone();
        let required: Vec<&str> = child["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(required.contains(&"id"), "should inherit required 'id'");
        assert!(required.contains(&"extra"), "should keep required 'extra'");
    }
}
