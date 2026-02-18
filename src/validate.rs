//! Validation passes for type hints and schema constraints.

use std::collections::HashSet;

use serde_json::json;
use serde_json::Value as JsonValue;
use std::collections::BTreeMap;

use crate::ast::SchemaDoc;
use crate::error::SyamlError;
use crate::expr::eval::{evaluate, EvalContext, EvalError};
use crate::expr::parse_expression;
use crate::resolve::get_json_path;
use crate::schema::{resolve_type_schema, validate_json_against_schema_with_types};

const MAX_CONSTRAINT_PATHS: usize = 2048;
const MAX_CONSTRAINTS_PER_PATH: usize = 128;
const MAX_CONSTRAINT_EXPR_LEN: usize = 4096;

/// Validates normalized data values against extracted type hints.
///
/// Each hint path must exist in `data`, and each referenced type must resolve
/// either to a named type in `schema.types` or to a built-in primitive type.
pub fn validate_type_hints(
    data: &JsonValue,
    hints: &BTreeMap<String, String>,
    schema: &SchemaDoc,
) -> Result<(), SyamlError> {
    for (path, type_name) in hints {
        let _ = resolve_type_schema(schema, type_name)?;
        let value = get_json_path(data, path).ok_or_else(|| {
            SyamlError::TypeHintError(format!("type hint references missing path '{}'", path))
        })?;
        let hinted_schema = json!({ "type": type_name });
        validate_json_against_schema_with_types(value, &hinted_schema, path, &schema.types)?;
    }

    for (path, type_name) in hints {
        validate_nested_hint_matches_parent_schema(path, type_name, hints, schema)?;
    }

    Ok(())
}

fn validate_nested_hint_matches_parent_schema(
    path: &str,
    type_name: &str,
    hints: &BTreeMap<String, String>,
    schema: &SchemaDoc,
) -> Result<(), SyamlError> {
    let Some((ancestor_path, ancestor_type)) = nearest_ancestor_hint(path, hints) else {
        return Ok(());
    };

    let Some(expected_schema) =
        resolve_expected_schema_for_descendant(schema, &ancestor_path, ancestor_type, path)?
    else {
        return Ok(());
    };
    let expected_type = schema_declared_type_name(&expected_schema, path)?;
    if expected_type != type_name {
        return Err(SyamlError::TypeHintError(format!(
            "type hint mismatch at '{}': hint '{}' does not match schema-defined type '{}' under '{}'",
            path, type_name, expected_type, ancestor_path
        )));
    }

    Ok(())
}

fn nearest_ancestor_hint<'a>(
    path: &str,
    hints: &'a BTreeMap<String, String>,
) -> Option<(String, &'a str)> {
    let mut candidate = parent_path(path);
    while let Some(parent) = candidate {
        if let Some(type_name) = hints.get(&parent) {
            return Some((parent, type_name.as_str()));
        }
        candidate = parent_path(&parent);
    }
    None
}

fn resolve_expected_schema_for_descendant(
    schema: &SchemaDoc,
    ancestor_path: &str,
    ancestor_type: &str,
    descendant_path: &str,
) -> Result<Option<JsonValue>, SyamlError> {
    let ancestor_segments = parse_hint_path(ancestor_path)?;
    let descendant_segments = parse_hint_path(descendant_path)?;
    if !descendant_segments.starts_with(&ancestor_segments) {
        return Err(SyamlError::TypeHintError(format!(
            "internal type-hint path mismatch: '{}' is not under '{}'",
            descendant_path, ancestor_path
        )));
    }

    let mut current_schema = resolve_type_schema(schema, ancestor_type)?;
    for segment in descendant_segments.iter().skip(ancestor_segments.len()) {
        let mut visited_types = Vec::new();
        let lookup = descend_schema_for_segment(
            schema,
            &current_schema,
            segment,
            descendant_path,
            &mut visited_types,
        )?;
        match lookup {
            SegmentLookup::Found(next_schema) => current_schema = next_schema,
            SegmentLookup::ExplicitlyMissing => {
                return Err(SyamlError::TypeHintError(format!(
                    "type hint '{}' is not declared in schema under ancestor hint '{}' ({})",
                    descendant_path, ancestor_path, ancestor_type
                )));
            }
            SegmentLookup::Unspecified => return Ok(None),
        };
    }

    Ok(Some(current_schema))
}

fn descend_schema_for_segment(
    schema: &SchemaDoc,
    current_schema: &JsonValue,
    segment: &HintPathSegment,
    path: &str,
    visited_types: &mut Vec<String>,
) -> Result<SegmentLookup, SyamlError> {
    let schema_obj = current_schema.as_object().ok_or_else(|| {
        SyamlError::SchemaError(format!(
            "schema at {path} must be an object, found {current_schema:?}"
        ))
    })?;

    let local_result = match segment {
        HintPathSegment::Key(key) => {
            if let Some(props_json) = schema_obj.get("properties") {
                let props = props_json.as_object().ok_or_else(|| {
                    SyamlError::SchemaError(format!("properties at {path} must be an object"))
                })?;
                if let Some(found) = props.get(key) {
                    SegmentLookup::Found(found.clone())
                } else {
                    SegmentLookup::ExplicitlyMissing
                }
            } else {
                SegmentLookup::Unspecified
            }
        }
        HintPathSegment::Index(_) => {
            if let Some(items) = schema_obj.get("items") {
                SegmentLookup::Found(items.clone())
            } else {
                SegmentLookup::Unspecified
            }
        }
    };
    if let SegmentLookup::Found(_) = local_result {
        return Ok(local_result);
    }

    match segment {
        HintPathSegment::Key(_) => {
            if let Some(raw_type) = schema_obj.get("type") {
                let type_name = raw_type.as_str().ok_or_else(|| {
                    SyamlError::SchemaError(format!("schema 'type' at {path} must be a string"))
                })?;
                if is_builtin_type_name(type_name) && type_name != "object" {
                    return Ok(SegmentLookup::ExplicitlyMissing);
                }
            }
        }
        HintPathSegment::Index(_) => {
            if let Some(raw_type) = schema_obj.get("type") {
                let type_name = raw_type.as_str().ok_or_else(|| {
                    SyamlError::SchemaError(format!("schema 'type' at {path} must be a string"))
                })?;
                if is_builtin_type_name(type_name) && type_name != "array" {
                    return Ok(SegmentLookup::ExplicitlyMissing);
                }
            }
        }
    }

    let Some(raw_type) = schema_obj.get("type") else {
        return Ok(local_result);
    };
    let type_name = raw_type.as_str().ok_or_else(|| {
        SyamlError::SchemaError(format!("schema 'type' at {path} must be a string"))
    })?;
    if is_builtin_type_name(type_name) {
        return Ok(local_result);
    }

    let referenced = schema.types.get(type_name).ok_or_else(|| {
        SyamlError::TypeHintError(format!(
            "unknown type reference at {path}: '{type_name}' not found in schema.types"
        ))
    })?;
    if let Some(cycle_start) = visited_types.iter().position(|t| t == type_name) {
        let mut cycle = visited_types[cycle_start..].to_vec();
        cycle.push(type_name.to_string());
        return Err(SyamlError::TypeHintError(format!(
            "cyclic type reference while resolving nested type hints at {path}: {}",
            cycle.join(" -> ")
        )));
    }

    visited_types.push(type_name.to_string());
    let referenced_result =
        descend_schema_for_segment(schema, referenced, segment, path, visited_types);
    visited_types.pop();
    let referenced_result = referenced_result?;
    if let SegmentLookup::Found(_) = referenced_result {
        return Ok(referenced_result);
    }

    match local_result {
        SegmentLookup::ExplicitlyMissing => Ok(SegmentLookup::ExplicitlyMissing),
        SegmentLookup::Unspecified => Ok(referenced_result),
        SegmentLookup::Found(_) => unreachable!("found handled above"),
    }
}

fn schema_declared_type_name(schema: &JsonValue, path: &str) -> Result<String, SyamlError> {
    let schema_obj = schema.as_object().ok_or_else(|| {
        SyamlError::SchemaError(format!(
            "schema at {path} must be an object, found {schema:?}"
        ))
    })?;
    let raw_type = schema_obj.get("type").ok_or_else(|| {
        SyamlError::TypeHintError(format!(
            "cannot validate type hint at '{}': schema definition does not declare a 'type'",
            path
        ))
    })?;
    let type_name = raw_type.as_str().ok_or_else(|| {
        SyamlError::SchemaError(format!("schema 'type' at {path} must be a string"))
    })?;
    Ok(type_name.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HintPathSegment {
    Key(String),
    Index(usize),
}

enum SegmentLookup {
    Found(JsonValue),
    ExplicitlyMissing,
    Unspecified,
}

fn parse_hint_path(path: &str) -> Result<Vec<HintPathSegment>, SyamlError> {
    if path == "$" {
        return Ok(Vec::new());
    }
    if !path.starts_with("$.") {
        return Err(SyamlError::TypeHintError(format!(
            "invalid type hint path '{}'; expected '$' or '$.' prefix",
            path
        )));
    }

    let mut out = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = path[2..].chars().collect();
    let mut i = 0usize;

    while i < chars.len() {
        let ch = chars[i];
        if ch == '.' {
            if !current.is_empty() {
                out.push(HintPathSegment::Key(current.clone()));
                current.clear();
            }
            i += 1;
            continue;
        }

        if ch == '[' {
            if !current.is_empty() {
                out.push(HintPathSegment::Key(current.clone()));
                current.clear();
            }
            i += 1;
            let mut num = String::new();
            while i < chars.len() && chars[i] != ']' {
                num.push(chars[i]);
                i += 1;
            }
            if i >= chars.len() || chars[i] != ']' {
                return Err(SyamlError::TypeHintError(format!(
                    "invalid array path segment in '{}'",
                    path
                )));
            }
            i += 1;
            let idx: usize = num.parse().map_err(|_| {
                SyamlError::TypeHintError(format!("invalid array index '{}' in '{}'", num, path))
            })?;
            out.push(HintPathSegment::Index(idx));
            continue;
        }

        current.push(ch);
        i += 1;
    }

    if !current.is_empty() {
        out.push(HintPathSegment::Key(current));
    }

    Ok(out)
}

fn is_builtin_type_name(type_name: &str) -> bool {
    matches!(
        type_name,
        "string" | "integer" | "number" | "boolean" | "object" | "array" | "null"
    )
}

/// Builds the full constraint set for a document by combining top-level
/// `schema.constraints` with type-local constraints from hinted paths.
pub fn build_effective_constraints(
    hints: &BTreeMap<String, String>,
    schema: &SchemaDoc,
) -> BTreeMap<String, Vec<String>> {
    let mut out = schema.constraints.clone();

    for (hint_path, type_name) in hints {
        let Some(type_local) = schema.type_constraints.get(type_name) else {
            continue;
        };

        for (relative_path, expressions) in type_local {
            let absolute_path = join_paths(hint_path, relative_path);
            out.entry(absolute_path)
                .or_default()
                .extend(expressions.iter().cloned());
        }
    }

    out
}

/// Evaluates schema constraints against data and environment context.
///
/// Constraints are keyed by path and must evaluate to boolean values.
/// Paths may be absolute (`$.a.b`) or shorthand (`a.b`).
pub fn validate_constraints(
    data: &JsonValue,
    env: &BTreeMap<String, JsonValue>,
    constraints: &BTreeMap<String, Vec<String>>,
) -> Result<(), SyamlError> {
    if constraints.len() > MAX_CONSTRAINT_PATHS {
        return Err(SyamlError::ConstraintError(format!(
            "too many constraint paths: {} (max {MAX_CONSTRAINT_PATHS})",
            constraints.len()
        )));
    }

    for (path, expressions) in constraints {
        if expressions.len() > MAX_CONSTRAINTS_PER_PATH {
            return Err(SyamlError::ConstraintError(format!(
                "too many constraints for path '{}': {} (max {MAX_CONSTRAINTS_PER_PATH})",
                path,
                expressions.len()
            )));
        }

        let normalized_path = normalize_path(path);
        let value = get_json_path(data, &normalized_path).ok_or_else(|| {
            SyamlError::ConstraintError(format!(
                "constraint path '{}' not found (normalized '{}')",
                path, normalized_path
            ))
        })?;
        let current_scope = parent_path(&normalized_path).and_then(|p| get_json_path(data, &p));

        for expression in expressions {
            let source = expression.trim().trim_start_matches('=').trim();
            if source.len() > MAX_CONSTRAINT_EXPR_LEN {
                return Err(SyamlError::ConstraintError(format!(
                    "constraint expression at '{}' exceeds max length ({MAX_CONSTRAINT_EXPR_LEN})",
                    normalized_path
                )));
            }
            let ast = parse_expression(source)?;
            let unresolved = HashSet::new();
            let ctx = EvalContext {
                data,
                env,
                unresolved_paths: &unresolved,
                current_value: Some(value),
                current_scope,
            };

            let result = evaluate(&ast, &ctx).map_err(map_eval_error)?;
            match result {
                JsonValue::Bool(true) => {}
                JsonValue::Bool(false) => {
                    return Err(SyamlError::ConstraintError(format!(
                        "constraint failed at '{}': '{}' evaluated to false",
                        normalized_path, expression
                    )));
                }
                other => {
                    return Err(SyamlError::ConstraintError(format!(
                        "constraint '{}' at '{}' must evaluate to boolean, got {}",
                        expression,
                        normalized_path,
                        json_type_name(&other)
                    )));
                }
            }
        }
    }

    Ok(())
}

fn normalize_path(path: &str) -> String {
    if path == "$" || path.starts_with("$.") {
        path.to_string()
    } else {
        format!("$.{}", path)
    }
}

fn join_paths(base: &str, relative: &str) -> String {
    let base_norm = normalize_path(base);
    let rel_norm = normalize_path(relative);

    if rel_norm == "$" {
        base_norm
    } else if base_norm == "$" {
        rel_norm
    } else {
        format!("{}{}", base_norm, &rel_norm[1..])
    }
}

fn parent_path(path: &str) -> Option<String> {
    if path == "$" {
        return None;
    }

    let mut last_sep = None;
    for (idx, ch) in path.char_indices() {
        if ch == '.' && idx > 1 {
            last_sep = Some(idx);
        } else if ch == '[' {
            last_sep = Some(idx);
        }
    }

    match last_sep {
        Some(1) => Some("$".to_string()),
        Some(idx) => Some(path[..idx].to_string()),
        None => None,
    }
}

fn map_eval_error(err: EvalError) -> SyamlError {
    match err {
        EvalError::Unresolved(dep) => SyamlError::ConstraintError(format!(
            "unresolved dependency while evaluating constraint: {dep}"
        )),
        EvalError::Fatal(e) => e,
    }
}

fn json_type_name(value: &JsonValue) -> &'static str {
    if value.is_null() {
        "null"
    } else if value.is_boolean() {
        "boolean"
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
