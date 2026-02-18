//! Environment and expression resolution for parsed data.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value as JsonValue;

use crate::ast::{EnvBinding, FrontMatter};
use crate::error::SyamlError;
use crate::expr::eval::{evaluate, EvalContext, EvalError};
use crate::expr::parse_expression;
use crate::mini_yaml;

const MAX_DERIVED_EXPRESSIONS: usize = 1024;
const MAX_INTERPOLATIONS_PER_STRING: usize = 128;
const MAX_EXPRESSION_SOURCE_LEN: usize = 4096;

/// Environment lookup abstraction used during compilation.
pub trait EnvProvider {
    /// Returns the environment value for `key`, if available.
    fn get(&self, key: &str) -> Option<String>;
}

/// [`EnvProvider`] implementation backed by process environment variables.
pub struct ProcessEnvProvider;

impl EnvProvider for ProcessEnvProvider {
    fn get(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }
}

#[derive(Debug, Clone)]
/// [`EnvProvider`] implementation backed by a caller-provided map.
pub struct MapEnvProvider {
    values: HashMap<String, String>,
}

impl MapEnvProvider {
    /// Creates a new map-backed provider.
    pub fn new(values: HashMap<String, String>) -> Self {
        Self { values }
    }
}

impl EnvProvider for MapEnvProvider {
    fn get(&self, key: &str) -> Option<String> {
        self.values.get(key).cloned()
    }
}

/// Resolves all `front_matter.env` bindings into concrete JSON values.
pub fn resolve_env_bindings(
    front_matter: Option<&FrontMatter>,
    env_provider: &dyn EnvProvider,
) -> Result<BTreeMap<String, JsonValue>, SyamlError> {
    let mut out = BTreeMap::new();
    let Some(front_matter) = front_matter else {
        return Ok(out);
    };

    for (symbol, binding) in &front_matter.env {
        let value = resolve_one_binding(symbol, binding, env_provider)?;
        out.insert(symbol.clone(), value);
    }

    Ok(out)
}

fn resolve_one_binding(
    symbol: &str,
    binding: &EnvBinding,
    env_provider: &dyn EnvProvider,
) -> Result<JsonValue, SyamlError> {
    if let Some(raw) = env_provider.get(&binding.key) {
        parse_env_scalar(&raw).map_err(|_e| {
            SyamlError::EnvError(format!(
                "failed to parse env '{}': invalid scalar value",
                binding.key
            ))
        })
    } else if let Some(default) = &binding.default {
        Ok(default.clone())
    } else if binding.required {
        Err(SyamlError::EnvError(format!(
            "missing required environment variable '{}' for symbol '{}'",
            binding.key, symbol
        )))
    } else {
        Ok(JsonValue::Null)
    }
}

fn parse_env_scalar(raw: &str) -> Result<JsonValue, SyamlError> {
    mini_yaml::parse_scalar(raw)
}

/// Resolves derived expressions and interpolations within a JSON data tree in-place.
///
/// Strings that start with `=` are evaluated as full expressions.
/// Strings containing `${...}` are treated as interpolations.
pub fn resolve_expressions(
    data: &mut JsonValue,
    env: &BTreeMap<String, JsonValue>,
) -> Result<(), SyamlError> {
    let mut expr_nodes = Vec::new();
    collect_expression_nodes(data, "$", &mut expr_nodes);

    if expr_nodes.is_empty() {
        return Ok(());
    }

    if expr_nodes.len() > MAX_DERIVED_EXPRESSIONS {
        return Err(SyamlError::ExpressionError(format!(
            "too many derived expressions/interpolations: {} (max {MAX_DERIVED_EXPRESSIONS})",
            expr_nodes.len()
        )));
    }

    let mut unresolved: HashSet<String> = expr_nodes.iter().map(|n| n.path.clone()).collect();
    let max_passes = expr_nodes.len() + 1;

    for _ in 0..max_passes {
        if unresolved.is_empty() {
            return Ok(());
        }

        let mut progress = false;
        for node in &expr_nodes {
            if !unresolved.contains(&node.path) {
                continue;
            }

            match eval_node(node, data, env, &unresolved) {
                Ok(value) => {
                    set_json_path(data, &node.path, value)?;
                    unresolved.remove(&node.path);
                    progress = true;
                }
                Err(EvalError::Unresolved(_dep)) => {}
                Err(EvalError::Fatal(err)) => return Err(err),
            }
        }

        if !progress {
            let mut paths: Vec<String> = unresolved.into_iter().collect();
            paths.sort();
            return Err(SyamlError::CycleError(format!(
                "could not resolve derived values; possible dependency cycle among: {}",
                paths.join(", ")
            )));
        }
    }

    Err(SyamlError::CycleError(
        "expression resolution exceeded max passes".to_string(),
    ))
}

#[derive(Debug, Clone)]
struct ExpressionNode {
    path: String,
    raw: String,
}

fn collect_expression_nodes(value: &JsonValue, path: &str, out: &mut Vec<ExpressionNode>) {
    match value {
        JsonValue::Object(map) => {
            for (k, v) in map {
                let child = format!("{}.{}", path, k);
                collect_expression_nodes(v, &child, out);
            }
        }
        JsonValue::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                let child = format!("{}[{}]", path, i);
                collect_expression_nodes(v, &child, out);
            }
        }
        JsonValue::String(s) => {
            let trimmed = s.trim();
            if trimmed.starts_with('=') || trimmed.contains("${") {
                out.push(ExpressionNode {
                    path: path.to_string(),
                    raw: s.clone(),
                });
            }
        }
        _ => {}
    }
}

fn eval_node(
    node: &ExpressionNode,
    data: &JsonValue,
    env: &BTreeMap<String, JsonValue>,
    unresolved: &HashSet<String>,
) -> Result<JsonValue, EvalError> {
    let raw = node.raw.trim();
    if let Some(expr_source) = raw.strip_prefix('=') {
        let source = expr_source.trim();
        ensure_expression_source_len(source)?;
        let parsed = parse_expression(source)?;
        let ctx = EvalContext {
            data,
            env,
            unresolved_paths: unresolved,
            current_value: None,
            current_scope: None,
        };
        return evaluate(&parsed, &ctx);
    }

    evaluate_interpolation(raw, data, env, unresolved)
}

fn evaluate_interpolation(
    raw: &str,
    data: &JsonValue,
    env: &BTreeMap<String, JsonValue>,
    unresolved: &HashSet<String>,
) -> Result<JsonValue, EvalError> {
    let all_re = interpolation_regex();
    let mut matches: Vec<(usize, usize, String)> = Vec::new();
    for caps in all_re.captures_iter(raw) {
        if matches.len() >= MAX_INTERPOLATIONS_PER_STRING {
            return Err(SyamlError::ExpressionError(format!(
                "too many interpolation segments in one string (max {MAX_INTERPOLATIONS_PER_STRING})"
            ))
            .into());
        }
        let whole = caps.get(0).ok_or_else(|| {
            SyamlError::ExpressionError("invalid interpolation capture".to_string())
        })?;
        let expr = caps
            .get(1)
            .ok_or_else(|| {
                SyamlError::ExpressionError("invalid interpolation capture".to_string())
            })?
            .as_str()
            .to_string();
        matches.push((whole.start(), whole.end(), expr));
    }

    if matches.len() == 1 && matches[0].0 == 0 && matches[0].1 == raw.len() {
        let source = matches[0].2.trim();
        ensure_expression_source_len(source)?;
        let parsed = parse_expression(source)?;
        let ctx = EvalContext {
            data,
            env,
            unresolved_paths: unresolved,
            current_value: None,
            current_scope: None,
        };
        return evaluate(&parsed, &ctx);
    }

    let mut out = String::new();
    let mut last = 0usize;

    for (start, end, expr_source) in matches {
        out.push_str(&raw[last..start]);
        let source = expr_source.trim();
        ensure_expression_source_len(source)?;
        let parsed = parse_expression(source)?;
        let ctx = EvalContext {
            data,
            env,
            unresolved_paths: unresolved,
            current_value: None,
            current_scope: None,
        };
        let eval = evaluate(&parsed, &ctx)?;
        out.push_str(&json_to_string(&eval));
        last = end;
    }

    out.push_str(&raw[last..]);
    Ok(JsonValue::String(out))
}

fn interpolation_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\$\{([^}]+)\}").expect("valid regex"))
}

fn ensure_expression_source_len(source: &str) -> Result<(), EvalError> {
    if source.len() > MAX_EXPRESSION_SOURCE_LEN {
        return Err(SyamlError::ExpressionError(format!(
            "expression exceeds max length ({MAX_EXPRESSION_SOURCE_LEN})"
        ))
        .into());
    }
    Ok(())
}

fn json_to_string(value: &JsonValue) -> String {
    match value {
        JsonValue::String(v) => v.clone(),
        other => other.to_string(),
    }
}

fn set_json_path(root: &mut JsonValue, path: &str, value: JsonValue) -> Result<(), SyamlError> {
    let segments = parse_path(path)?;
    if segments.is_empty() {
        *root = value;
        return Ok(());
    }

    let mut current = root;
    for segment in &segments[..segments.len() - 1] {
        match segment {
            PathSegment::Key(key) => {
                current = current
                    .as_object_mut()
                    .and_then(|map| map.get_mut(key))
                    .ok_or_else(|| {
                        SyamlError::ExpressionError(format!(
                            "path '{}' not found while setting value",
                            path
                        ))
                    })?;
            }
            PathSegment::Index(i) => {
                current = current
                    .as_array_mut()
                    .and_then(|arr| arr.get_mut(*i))
                    .ok_or_else(|| {
                        SyamlError::ExpressionError(format!(
                            "path '{}' not found while setting array index",
                            path
                        ))
                    })?;
            }
        }
    }

    match segments.last().expect("non-empty") {
        PathSegment::Key(key) => {
            let map = current.as_object_mut().ok_or_else(|| {
                SyamlError::ExpressionError(format!("path '{}' does not point to object", path))
            })?;
            map.insert(key.clone(), value);
        }
        PathSegment::Index(i) => {
            let arr = current.as_array_mut().ok_or_else(|| {
                SyamlError::ExpressionError(format!("path '{}' does not point to array", path))
            })?;
            if *i >= arr.len() {
                return Err(SyamlError::ExpressionError(format!(
                    "array index out of bounds in path '{}'",
                    path
                )));
            }
            arr[*i] = value;
        }
    }

    Ok(())
}

#[derive(Debug)]
enum PathSegment {
    Key(String),
    Index(usize),
}

fn parse_path(path: &str) -> Result<Vec<PathSegment>, SyamlError> {
    if path == "$" {
        return Ok(Vec::new());
    }

    if !path.starts_with("$.") {
        return Err(SyamlError::ExpressionError(format!(
            "invalid path '{}'; expected to start with '$.'",
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
                out.push(PathSegment::Key(current.clone()));
                current.clear();
            }
            i += 1;
            continue;
        }

        if ch == '[' {
            if !current.is_empty() {
                out.push(PathSegment::Key(current.clone()));
                current.clear();
            }
            i += 1;
            let mut num = String::new();
            while i < chars.len() && chars[i] != ']' {
                num.push(chars[i]);
                i += 1;
            }
            if i >= chars.len() || chars[i] != ']' {
                return Err(SyamlError::ExpressionError(format!(
                    "invalid array path segment in '{}'",
                    path
                )));
            }
            i += 1;
            let idx: usize = num.parse().map_err(|_| {
                SyamlError::ExpressionError(format!("invalid array index '{}' in '{}'", num, path))
            })?;
            out.push(PathSegment::Index(idx));
            continue;
        }

        current.push(ch);
        i += 1;
    }

    if !current.is_empty() {
        out.push(PathSegment::Key(current));
    }

    Ok(out)
}

/// Gets a value by normalized JSON path (`$`, `$.a.b`, `$.items[0]`).
pub fn get_json_path<'a>(root: &'a JsonValue, path: &str) -> Option<&'a JsonValue> {
    let segments = parse_path(path).ok()?;
    let mut current = root;
    for segment in segments {
        match segment {
            PathSegment::Key(key) => {
                current = current.as_object()?.get(&key)?;
            }
            PathSegment::Index(i) => {
                current = current.as_array()?.get(i)?;
            }
        }
    }
    Some(current)
}
