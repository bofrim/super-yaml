//! Runtime evaluator for expression AST values.

use std::collections::HashSet;

use serde_json::{Number as JsonNumber, Value as JsonValue};
use std::collections::BTreeMap;

use crate::error::SyamlError;

use super::parser::{BinaryOp, Expr, UnaryOp};

#[derive(Debug)]
/// Evaluation-time error classification.
pub enum EvalError {
    /// Dependency is not resolved yet; caller should retry later.
    Unresolved(String),
    /// Non-recoverable expression error.
    Fatal(SyamlError),
}

impl From<SyamlError> for EvalError {
    fn from(value: SyamlError) -> Self {
        Self::Fatal(value)
    }
}

/// Context object passed to expression evaluation.
pub struct EvalContext<'a> {
    /// Root resolved/partially-resolved data tree.
    pub data: &'a JsonValue,
    /// Resolved environment symbol map.
    pub env: &'a BTreeMap<String, JsonValue>,
    /// Paths still pending resolution (used for cycle/dependency handling).
    pub unresolved_paths: &'a HashSet<String>,
    /// Current `value` target for constraint expressions.
    pub current_value: Option<&'a JsonValue>,
    /// Current object scope used for local lookups in constraint expressions.
    pub current_scope: Option<&'a JsonValue>,
}

/// Evaluates an expression AST node into a JSON value.
pub fn evaluate(expr: &Expr, ctx: &EvalContext<'_>) -> Result<JsonValue, EvalError> {
    match expr {
        Expr::Number(n) => number(*n),
        Expr::String(s) => Ok(JsonValue::String(s.clone())),
        Expr::Bool(b) => Ok(JsonValue::Bool(*b)),
        Expr::Null => Ok(JsonValue::Null),
        Expr::Var(path) => resolve_var(path, ctx),
        Expr::Unary { op, expr } => {
            let value = evaluate(expr, ctx)?;
            match op {
                UnaryOp::Neg => Ok(number(-as_f64(&value)?)?),
                UnaryOp::Not => Ok(JsonValue::Bool(!as_bool(&value)?)),
            }
        }
        Expr::Binary { op, left, right } => {
            let l = evaluate(left, ctx)?;
            let r = evaluate(right, ctx)?;
            eval_binary(*op, l, r)
        }
        Expr::Call { name, args } => eval_call(name, args, ctx),
    }
}

fn resolve_var(path: &[String], ctx: &EvalContext<'_>) -> Result<JsonValue, EvalError> {
    if path.is_empty() {
        return Err(SyamlError::ExpressionError("empty variable path".to_string()).into());
    }

    if path[0] == "env" {
        if path.len() != 2 {
            return Err(SyamlError::ExpressionError(format!(
                "env reference must be env.NAME, got {}",
                path.join(".")
            ))
            .into());
        }
        return ctx.env.get(&path[1]).cloned().ok_or_else(|| {
            EvalError::Fatal(SyamlError::ExpressionError(format!(
                "unknown env binding '{}'",
                path[1]
            )))
        });
    }

    if path[0] == "value" {
        let base = ctx.current_value.ok_or_else(|| {
            EvalError::Fatal(SyamlError::ExpressionError(
                "'value' is only available in constraint expressions".to_string(),
            ))
        })?;
        return if path.len() == 1 {
            Ok(base.clone())
        } else {
            lookup_path(base, &path[1..]).cloned().ok_or_else(|| {
                EvalError::Fatal(SyamlError::ExpressionError(format!(
                    "path '{}' not found under value",
                    path.join(".")
                )))
            })
        };
    }

    let full_path = format!("$.{}", path.join("."));
    if ctx.unresolved_paths.contains(&full_path) {
        return Err(EvalError::Unresolved(full_path));
    }

    if let Some(found) = lookup_path(ctx.data, path) {
        return Ok(found.clone());
    }

    if let Some(scope) = ctx.current_scope {
        if let Some(found) = lookup_path(scope, path) {
            return Ok(found.clone());
        }
    }

    if let Some(current) = ctx.current_value {
        if let Some(found) = lookup_path(current, path) {
            return Ok(found.clone());
        }
    }

    Err(EvalError::Fatal(SyamlError::ExpressionError(format!(
        "unknown reference '{}'",
        path.join(".")
    ))))
}

fn lookup_path<'a>(root: &'a JsonValue, path: &[String]) -> Option<&'a JsonValue> {
    let mut cur = root;
    for segment in path {
        cur = cur.as_object()?.get(segment)?;
    }
    Some(cur)
}

fn eval_binary(op: BinaryOp, left: JsonValue, right: JsonValue) -> Result<JsonValue, EvalError> {
    match op {
        BinaryOp::Add => {
            if left.is_string() || right.is_string() {
                Ok(JsonValue::String(format!(
                    "{}{}",
                    json_to_string(&left),
                    json_to_string(&right)
                )))
            } else {
                Ok(number(as_f64(&left)? + as_f64(&right)?)?)
            }
        }
        BinaryOp::Sub => Ok(number(as_f64(&left)? - as_f64(&right)?)?),
        BinaryOp::Mul => Ok(number(as_f64(&left)? * as_f64(&right)?)?),
        BinaryOp::Div => {
            let rhs = as_f64(&right)?;
            if rhs == 0.0 {
                return Err(SyamlError::ExpressionError("division by zero".to_string()).into());
            }
            Ok(number(as_f64(&left)? / rhs)?)
        }
        BinaryOp::Mod => {
            let l = as_i64(&left)?;
            let r = as_i64(&right)?;
            if r == 0 {
                return Err(SyamlError::ExpressionError("modulo by zero".to_string()).into());
            }
            Ok(JsonValue::Number(JsonNumber::from(l % r)))
        }
        BinaryOp::Eq => Ok(JsonValue::Bool(left == right)),
        BinaryOp::NotEq => Ok(JsonValue::Bool(left != right)),
        BinaryOp::Lt => compare(left, right, |a, b| a < b),
        BinaryOp::Lte => compare(left, right, |a, b| a <= b),
        BinaryOp::Gt => compare(left, right, |a, b| a > b),
        BinaryOp::Gte => compare(left, right, |a, b| a >= b),
        BinaryOp::And => Ok(JsonValue::Bool(as_bool(&left)? && as_bool(&right)?)),
        BinaryOp::Or => Ok(JsonValue::Bool(as_bool(&left)? || as_bool(&right)?)),
    }
}

fn compare<F>(left: JsonValue, right: JsonValue, cmp: F) -> Result<JsonValue, EvalError>
where
    F: Fn(f64, f64) -> bool,
{
    Ok(JsonValue::Bool(cmp(as_f64(&left)?, as_f64(&right)?)))
}

fn eval_call(name: &str, args: &[Expr], ctx: &EvalContext<'_>) -> Result<JsonValue, EvalError> {
    let mut evaluated = Vec::with_capacity(args.len());
    for arg in args {
        evaluated.push(evaluate(arg, ctx)?);
    }

    match name {
        "min" => {
            require_arity_at_least(name, &evaluated, 1)?;
            let mut m = f64::INFINITY;
            for value in &evaluated {
                m = m.min(as_f64(value)?);
            }
            number(m)
        }
        "max" => {
            require_arity_at_least(name, &evaluated, 1)?;
            let mut m = f64::NEG_INFINITY;
            for value in &evaluated {
                m = m.max(as_f64(value)?);
            }
            number(m)
        }
        "abs" => {
            require_arity(name, &evaluated, 1)?;
            number(as_f64(&evaluated[0])?.abs())
        }
        "floor" => {
            require_arity(name, &evaluated, 1)?;
            number(as_f64(&evaluated[0])?.floor())
        }
        "ceil" => {
            require_arity(name, &evaluated, 1)?;
            number(as_f64(&evaluated[0])?.ceil())
        }
        "round" => {
            require_arity(name, &evaluated, 1)?;
            number(as_f64(&evaluated[0])?.round())
        }
        "len" => {
            require_arity(name, &evaluated, 1)?;
            let n = match &evaluated[0] {
                JsonValue::String(v) => v.chars().count() as i64,
                JsonValue::Array(v) => v.len() as i64,
                JsonValue::Object(v) => v.len() as i64,
                _ => {
                    return Err(SyamlError::ExpressionError(
                        "len() expects string, array, or object".to_string(),
                    )
                    .into())
                }
            };
            Ok(JsonValue::Number(JsonNumber::from(n)))
        }
        "coalesce" => {
            require_arity_at_least(name, &evaluated, 1)?;
            for value in evaluated {
                if !value.is_null() {
                    return Ok(value);
                }
            }
            Ok(JsonValue::Null)
        }
        _ => Err(SyamlError::ExpressionError(format!("unknown function '{name}'")).into()),
    }
}

fn require_arity(name: &str, args: &[JsonValue], expected: usize) -> Result<(), EvalError> {
    if args.len() != expected {
        return Err(SyamlError::ExpressionError(format!(
            "{name} expects {expected} arguments, got {}",
            args.len()
        ))
        .into());
    }
    Ok(())
}

fn require_arity_at_least(name: &str, args: &[JsonValue], min: usize) -> Result<(), EvalError> {
    if args.len() < min {
        return Err(SyamlError::ExpressionError(format!(
            "{name} expects at least {min} arguments, got {}",
            args.len()
        ))
        .into());
    }
    Ok(())
}

fn as_f64(value: &JsonValue) -> Result<f64, EvalError> {
    value.as_f64().ok_or_else(|| {
        EvalError::Fatal(SyamlError::ExpressionError(format!(
            "expected number, got {}",
            json_type_name(value)
        )))
    })
}

fn as_i64(value: &JsonValue) -> Result<i64, EvalError> {
    if let Some(v) = value.as_i64() {
        Ok(v)
    } else if let Some(v) = value.as_u64() {
        i64::try_from(v).map_err(|_| {
            EvalError::Fatal(SyamlError::ExpressionError(format!(
                "integer value out of range: {v}"
            )))
        })
    } else {
        Err(EvalError::Fatal(SyamlError::ExpressionError(format!(
            "expected integer, got {}",
            json_type_name(value)
        ))))
    }
}

fn as_bool(value: &JsonValue) -> Result<bool, EvalError> {
    value.as_bool().ok_or_else(|| {
        EvalError::Fatal(SyamlError::ExpressionError(format!(
            "expected boolean, got {}",
            json_type_name(value)
        )))
    })
}

fn number(value: f64) -> Result<JsonValue, EvalError> {
    let num = JsonNumber::from_f64(value).ok_or_else(|| {
        EvalError::Fatal(SyamlError::ExpressionError(format!(
            "invalid numeric result {value}"
        )))
    })?;

    if value.fract() == 0.0 {
        if value >= i64::MIN as f64 && value <= i64::MAX as f64 {
            return Ok(JsonValue::Number(JsonNumber::from(value as i64)));
        }
    }

    Ok(JsonValue::Number(num))
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

fn json_to_string(value: &JsonValue) -> String {
    match value {
        JsonValue::String(v) => v.clone(),
        _ => value.to_string(),
    }
}
