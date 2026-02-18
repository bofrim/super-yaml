//! Validation passes for type hints and schema constraints.

use std::collections::HashSet;

use serde_json::Value as JsonValue;
use std::collections::BTreeMap;

use crate::ast::SchemaDoc;
use crate::error::SyamlError;
use crate::expr::eval::{evaluate, EvalContext, EvalError};
use crate::expr::parse_expression;
use crate::resolve::get_json_path;
use crate::schema::{resolve_type_schema, validate_json_against_schema};

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
        let schema_for_type = resolve_type_schema(schema, type_name)?;
        let value = get_json_path(data, path).ok_or_else(|| {
            SyamlError::TypeHintError(format!("type hint references missing path '{}'", path))
        })?;
        validate_json_against_schema(value, &schema_for_type, path)?;
    }

    Ok(())
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
