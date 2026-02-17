use std::collections::HashSet;

use serde_json::Value as JsonValue;
use std::collections::BTreeMap;

use crate::ast::SchemaDoc;
use crate::error::SyamlError;
use crate::expr::eval::{evaluate, EvalContext, EvalError};
use crate::expr::parse_expression;
use crate::resolve::get_json_path;
use crate::schema::{resolve_type_schema, validate_json_against_schema};

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

pub fn validate_constraints(
    data: &JsonValue,
    env: &BTreeMap<String, JsonValue>,
    constraints: &BTreeMap<String, Vec<String>>,
) -> Result<(), SyamlError> {
    for (path, expressions) in constraints {
        let normalized_path = normalize_path(path);
        let value = get_json_path(data, &normalized_path).ok_or_else(|| {
            SyamlError::ConstraintError(format!(
                "constraint path '{}' not found (normalized '{}')",
                path, normalized_path
            ))
        })?;

        for expression in expressions {
            let source = expression.trim().trim_start_matches('=').trim();
            let ast = parse_expression(source)?;
            let unresolved = HashSet::new();
            let ctx = EvalContext {
                data,
                env,
                unresolved_paths: &unresolved,
                current_value: Some(value),
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
