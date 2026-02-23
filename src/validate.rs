//! Validation passes for type hints and schema constraints.

use std::collections::{BTreeMap, HashSet};

use serde_json::json;
use serde_json::Value as JsonValue;

use crate::ast::SchemaDoc;
use crate::error::SyamlError;
use crate::expr::eval::{evaluate, EvalContext, EvalError};
use crate::expr::parse_expression;
use crate::expr::parser::{BinaryOp, Expr};
use crate::resolve::get_json_path;
use crate::schema::{parse_field_version_meta, resolve_type_schema, validate_json_against_schema_with_types};

const MAX_CONSTRAINT_PATHS: usize = 2048;
const MAX_CONSTRAINTS_PER_PATH: usize = 128;
const MAX_CONSTRAINT_EXPR_LEN: usize = 4096;
const REL_LT: u8 = 1;
const REL_EQ: u8 = 2;
const REL_GT: u8 = 4;
const REL_ANY: u8 = REL_LT | REL_EQ | REL_GT;

/// Validates normalized data values against extracted type hints.
///
/// Each hint path must exist in `data`, and each referenced type must resolve
/// either to a named type in `schema` or to a built-in primitive type.
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
    if expected_type != type_name && !are_equivalent_type_names(schema, &expected_type, type_name)?
    {
        return Err(SyamlError::TypeHintError(format!(
            "type hint mismatch at '{}': hint '{}' does not match schema-defined type '{}' under '{}'",
            path, type_name, expected_type, ancestor_path
        )));
    }

    Ok(())
}

fn are_equivalent_type_names(
    schema: &SchemaDoc,
    expected_type: &str,
    hinted_type: &str,
) -> Result<bool, SyamlError> {
    if expected_type == hinted_type {
        return Ok(true);
    }

    // IS-A check: hinted_type is a descendant of expected_type via extends.
    if is_descendant_of(hinted_type, expected_type, &schema.extends) {
        return Ok(true);
    }

    if !is_namespaced_suffix_match(expected_type, hinted_type) {
        return Ok(false);
    }

    let expected_schema = resolve_type_schema(schema, expected_type)?;
    let hinted_schema = resolve_type_schema(schema, hinted_type)?;
    Ok(
        normalize_schema_type_references_for_comparison(schema, &expected_schema)
            == normalize_schema_type_references_for_comparison(schema, &hinted_schema),
    )
}

/// Returns true if `child` is a direct or transitive descendant of `ancestor`
/// in the extends chain.
fn is_descendant_of(
    child: &str,
    ancestor: &str,
    extends_map: &BTreeMap<String, String>,
) -> bool {
    let mut current = child;
    let mut seen = std::collections::HashSet::new();
    loop {
        if !seen.insert(current) {
            return false;
        }
        match extends_map.get(current) {
            Some(parent) => {
                if parent == ancestor {
                    return true;
                }
                current = parent.as_str();
            }
            None => return false,
        }
    }
}

fn is_namespaced_suffix_match(left: &str, right: &str) -> bool {
    let left_segments: Vec<&str> = left.split('.').collect();
    let right_segments: Vec<&str> = right.split('.').collect();

    if left_segments.len() == right_segments.len() {
        return false;
    }

    let (longer, shorter) = if left_segments.len() > right_segments.len() {
        (left_segments, right_segments)
    } else {
        (right_segments, left_segments)
    };
    longer[longer.len() - shorter.len()..] == shorter
}

fn normalize_schema_type_references_for_comparison(
    schema: &SchemaDoc,
    value: &JsonValue,
) -> JsonValue {
    match value {
        JsonValue::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, child) in map {
                if key == "type" {
                    if let Some(type_name) = child.as_str() {
                        if !is_builtin_type_name(type_name) {
                            let canonical = canonicalize_imported_type_name(schema, type_name);
                            out.insert(key.clone(), JsonValue::String(canonical));
                            continue;
                        }
                    }
                }
                out.insert(
                    key.clone(),
                    normalize_schema_type_references_for_comparison(schema, child),
                );
            }
            JsonValue::Object(out)
        }
        JsonValue::Array(items) => JsonValue::Array(
            items
                .iter()
                .map(|item| normalize_schema_type_references_for_comparison(schema, item))
                .collect(),
        ),
        _ => value.clone(),
    }
}

fn canonicalize_imported_type_name(schema: &SchemaDoc, type_name: &str) -> String {
    if is_builtin_type_name(type_name) {
        return type_name.to_string();
    }

    let mut canonical = type_name.to_string();
    while let Some((_, rest)) = canonical.split_once('.') {
        if schema.types.contains_key(rest) {
            canonical = rest.to_string();
        } else {
            break;
        }
    }

    canonical
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

    // Union types cannot be statically descended — we don't know which option applies.
    if schema_obj.get("type").and_then(JsonValue::as_str) == Some("union") {
        return Ok(SegmentLookup::Unspecified);
    }

    let local_result = match segment {
        HintPathSegment::Key(key) => {
            if let Some(props_json) = schema_obj.get("properties") {
                let props = props_json.as_object().ok_or_else(|| {
                    SyamlError::SchemaError(format!("properties at {path} must be an object"))
                })?;
                if let Some(found) = props.get(key) {
                    SegmentLookup::Found(found.clone())
                } else if let Some(values_schema) = schema_obj.get("values") {
                    SegmentLookup::Found(values_schema.clone())
                } else {
                    SegmentLookup::ExplicitlyMissing
                }
            } else if let Some(values_schema) = schema_obj.get("values") {
                SegmentLookup::Found(values_schema.clone())
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
            "unknown type reference at {path}: '{type_name}' not found in schema"
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

/// Builds the full constraint set for a document from type-local constraints
/// expanded through hinted paths.
pub fn build_effective_constraints(
    hints: &BTreeMap<String, String>,
    schema: &SchemaDoc,
) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for (hint_path, type_name) in hints {
        let type_local = schema.type_constraints.get(type_name).or_else(|| {
            // Handle namespaced imports like "t.TimeRange" -> "TimeRange"
            if let Some(dot_pos) = type_name.rfind('.') {
                let base_name = &type_name[dot_pos + 1..];
                schema.type_constraints.get(base_name)
            } else {
                None
            }
        });

        let Some(type_local) = type_local else {
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
    let imports = BTreeMap::new();
    validate_constraints_with_imports(data, env, constraints, &imports)
}

/// Evaluates schema constraints against data, environment, and imported namespaces.
pub fn validate_constraints_with_imports(
    data: &JsonValue,
    env: &BTreeMap<String, JsonValue>,
    constraints: &BTreeMap<String, Vec<String>>,
    imports: &BTreeMap<String, JsonValue>,
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
        let mut parsed_expressions = Vec::with_capacity(expressions.len());

        for expression in expressions {
            let source = expression.trim().trim_start_matches('=').trim();
            if source.len() > MAX_CONSTRAINT_EXPR_LEN {
                return Err(SyamlError::ConstraintError(format!(
                    "constraint expression at '{}' exceeds max length ({MAX_CONSTRAINT_EXPR_LEN})",
                    normalized_path
                )));
            }
            let ast = parse_expression(source)?;
            parsed_expressions.push((expression.clone(), ast));
        }

        detect_impossible_constraints(&normalized_path, &parsed_expressions)?;

        for (expression, ast) in &parsed_expressions {
            let unresolved = HashSet::new();
            let ctx = EvalContext {
                data,
                imports,
                env,
                unresolved_paths: &unresolved,
                current_value: Some(value),
                current_scope,
                named_scopes: std::collections::BTreeMap::new(),
            };

            let result = evaluate(ast, &ctx).map_err(|eval_err| {
                match eval_err {
                    EvalError::Fatal(SyamlError::ExpressionError(msg)) => {
                        SyamlError::ConstraintError(format!(
                            "constraint evaluation failed at '{}': {} (in expression '{}')",
                            normalized_path, msg, expression
                        ))
                    }
                    other => map_eval_error(other),
                }
            })?;
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

fn detect_impossible_constraints(
    path: &str,
    expressions: &[(String, Expr)],
) -> Result<(), SyamlError> {
    let mut pair_states: BTreeMap<(String, String), PairConstraintState> = BTreeMap::new();
    let mut numeric_states: BTreeMap<String, NumericConstraintState> = BTreeMap::new();

    for (source, ast) in expressions {
        let mut comparisons = Vec::new();
        collect_var_comparisons(ast, &mut comparisons);

        for comparison in comparisons {
            let ((left, right), relation_mask) = canonicalize_var_comparison(comparison);
            let key = (left, right);
            let initial_mask = if key.0 == key.1 { REL_EQ } else { REL_ANY };
            let state = pair_states
                .entry(key)
                .or_insert_with(|| PairConstraintState {
                    allowed_mask: initial_mask,
                    source: source.clone(),
                });

            let merged = state.allowed_mask & relation_mask;
            if merged == 0 {
                return Err(SyamlError::ConstraintError(format!(
                    "impossible constraints at '{}': '{}' conflicts with '{}'",
                    path, state.source, source
                )));
            }

            state.allowed_mask = merged;
            state.source = source.clone();
        }

        let mut numeric_comparisons = Vec::new();
        collect_numeric_var_comparisons(ast, &mut numeric_comparisons);
        for comparison in numeric_comparisons {
            apply_numeric_comparison(path, source, comparison, &mut numeric_states)?;
        }
    }

    Ok(())
}

#[derive(Clone)]
struct PairConstraintState {
    allowed_mask: u8,
    source: String,
}

struct VarComparison {
    left: String,
    right: String,
    relation_mask: u8,
}

#[derive(Default)]
struct NumericConstraintState {
    lower: Option<NumericBound>,
    upper: Option<NumericBound>,
    not_equals: Vec<NumericNotEqual>,
}

struct NumericBound {
    value: f64,
    inclusive: bool,
    source: String,
}

struct NumericNotEqual {
    value: f64,
    source: String,
}

struct NumericComparison {
    var: String,
    op: BinaryOp,
    value: f64,
}

fn collect_var_comparisons(expr: &Expr, out: &mut Vec<VarComparison>) {
    match expr {
        Expr::Binary {
            op: BinaryOp::And,
            left,
            right,
        } => {
            collect_var_comparisons(left, out);
            collect_var_comparisons(right, out);
        }
        Expr::Binary { op, left, right } => {
            let Some(mask) = relation_mask_for_op(*op) else {
                return;
            };
            let (Expr::Var(left_path), Expr::Var(right_path)) = (&**left, &**right) else {
                return;
            };
            out.push(VarComparison {
                left: left_path.join("."),
                right: right_path.join("."),
                relation_mask: mask,
            });
        }
        _ => {}
    }
}

fn relation_mask_for_op(op: BinaryOp) -> Option<u8> {
    match op {
        BinaryOp::Lt => Some(REL_LT),
        BinaryOp::Lte => Some(REL_LT | REL_EQ),
        BinaryOp::Gt => Some(REL_GT),
        BinaryOp::Gte => Some(REL_GT | REL_EQ),
        BinaryOp::Eq => Some(REL_EQ),
        BinaryOp::NotEq => Some(REL_LT | REL_GT),
        _ => None,
    }
}

fn canonicalize_var_comparison(cmp: VarComparison) -> ((String, String), u8) {
    if cmp.left <= cmp.right {
        ((cmp.left, cmp.right), cmp.relation_mask)
    } else {
        (
            (cmp.right, cmp.left),
            invert_relation_mask(cmp.relation_mask),
        )
    }
}

fn invert_relation_mask(mask: u8) -> u8 {
    let mut out = mask & REL_EQ;
    if (mask & REL_LT) != 0 {
        out |= REL_GT;
    }
    if (mask & REL_GT) != 0 {
        out |= REL_LT;
    }
    out
}

fn collect_numeric_var_comparisons(expr: &Expr, out: &mut Vec<NumericComparison>) {
    match expr {
        Expr::Binary {
            op: BinaryOp::And,
            left,
            right,
        } => {
            collect_numeric_var_comparisons(left, out);
            collect_numeric_var_comparisons(right, out);
        }
        Expr::Binary { op, left, right } => {
            let Some(cmp) = normalize_numeric_comparison(*op, left, right) else {
                return;
            };
            out.push(cmp);
        }
        _ => {}
    }
}

fn normalize_numeric_comparison(
    op: BinaryOp,
    left: &Expr,
    right: &Expr,
) -> Option<NumericComparison> {
    match (left, right) {
        (Expr::Var(var_path), Expr::Number(value)) => {
            if relation_mask_for_op(op).is_none() {
                return None;
            }
            Some(NumericComparison {
                var: var_path.join("."),
                op,
                value: *value,
            })
        }
        (Expr::Number(value), Expr::Var(var_path)) => {
            let flipped = flip_comparison_op(op)?;
            Some(NumericComparison {
                var: var_path.join("."),
                op: flipped,
                value: *value,
            })
        }
        _ => None,
    }
}

fn flip_comparison_op(op: BinaryOp) -> Option<BinaryOp> {
    match op {
        BinaryOp::Lt => Some(BinaryOp::Gt),
        BinaryOp::Lte => Some(BinaryOp::Gte),
        BinaryOp::Gt => Some(BinaryOp::Lt),
        BinaryOp::Gte => Some(BinaryOp::Lte),
        BinaryOp::Eq => Some(BinaryOp::Eq),
        BinaryOp::NotEq => Some(BinaryOp::NotEq),
        _ => None,
    }
}

fn apply_numeric_comparison(
    path: &str,
    source: &str,
    cmp: NumericComparison,
    numeric_states: &mut BTreeMap<String, NumericConstraintState>,
) -> Result<(), SyamlError> {
    let state = numeric_states.entry(cmp.var).or_default();
    match cmp.op {
        BinaryOp::Lt => {
            update_upper_bound(
                path,
                state,
                NumericBound {
                    value: cmp.value,
                    inclusive: false,
                    source: source.to_string(),
                },
            )?;
        }
        BinaryOp::Lte => {
            update_upper_bound(
                path,
                state,
                NumericBound {
                    value: cmp.value,
                    inclusive: true,
                    source: source.to_string(),
                },
            )?;
        }
        BinaryOp::Gt => {
            update_lower_bound(
                path,
                state,
                NumericBound {
                    value: cmp.value,
                    inclusive: false,
                    source: source.to_string(),
                },
            )?;
        }
        BinaryOp::Gte => {
            update_lower_bound(
                path,
                state,
                NumericBound {
                    value: cmp.value,
                    inclusive: true,
                    source: source.to_string(),
                },
            )?;
        }
        BinaryOp::Eq => {
            update_lower_bound(
                path,
                state,
                NumericBound {
                    value: cmp.value,
                    inclusive: true,
                    source: source.to_string(),
                },
            )?;
            update_upper_bound(
                path,
                state,
                NumericBound {
                    value: cmp.value,
                    inclusive: true,
                    source: source.to_string(),
                },
            )?;
        }
        BinaryOp::NotEq => {
            state.not_equals.push(NumericNotEqual {
                value: cmp.value,
                source: source.to_string(),
            });
        }
        _ => unreachable!("comparison op filtering handled in collection"),
    }

    ensure_numeric_state_consistent(path, state)
}

fn update_lower_bound(
    path: &str,
    state: &mut NumericConstraintState,
    candidate: NumericBound,
) -> Result<(), SyamlError> {
    let replace = match state.lower.as_ref() {
        None => true,
        Some(existing) => is_stricter_lower_bound(&candidate, existing),
    };
    if replace {
        state.lower = Some(candidate);
    }
    ensure_numeric_bounds_consistent(path, state)
}

fn update_upper_bound(
    path: &str,
    state: &mut NumericConstraintState,
    candidate: NumericBound,
) -> Result<(), SyamlError> {
    let replace = match state.upper.as_ref() {
        None => true,
        Some(existing) => is_stricter_upper_bound(&candidate, existing),
    };
    if replace {
        state.upper = Some(candidate);
    }
    ensure_numeric_bounds_consistent(path, state)
}

fn is_stricter_lower_bound(candidate: &NumericBound, existing: &NumericBound) -> bool {
    if candidate.value > existing.value {
        return true;
    }
    if candidate.value < existing.value {
        return false;
    }
    !candidate.inclusive && existing.inclusive
}

fn is_stricter_upper_bound(candidate: &NumericBound, existing: &NumericBound) -> bool {
    if candidate.value < existing.value {
        return true;
    }
    if candidate.value > existing.value {
        return false;
    }
    !candidate.inclusive && existing.inclusive
}

fn ensure_numeric_bounds_consistent(
    path: &str,
    state: &NumericConstraintState,
) -> Result<(), SyamlError> {
    let (Some(lower), Some(upper)) = (state.lower.as_ref(), state.upper.as_ref()) else {
        return Ok(());
    };

    if lower.value > upper.value
        || (lower.value == upper.value && (!lower.inclusive || !upper.inclusive))
    {
        return Err(SyamlError::ConstraintError(format!(
            "impossible constraints at '{}': '{}' conflicts with '{}'",
            path, lower.source, upper.source
        )));
    }

    Ok(())
}

fn ensure_numeric_state_consistent(
    path: &str,
    state: &NumericConstraintState,
) -> Result<(), SyamlError> {
    ensure_numeric_bounds_consistent(path, state)?;

    let Some((exact_value, exact_source)) = exact_numeric_value(state) else {
        return Ok(());
    };

    for neq in &state.not_equals {
        if neq.value == exact_value {
            return Err(SyamlError::ConstraintError(format!(
                "impossible constraints at '{}': '{}' conflicts with '{}'",
                path, exact_source, neq.source
            )));
        }
    }

    Ok(())
}

fn exact_numeric_value(state: &NumericConstraintState) -> Option<(f64, &str)> {
    let (Some(lower), Some(upper)) = (state.lower.as_ref(), state.upper.as_ref()) else {
        return None;
    };
    if lower.value == upper.value && lower.inclusive && upper.inclusive {
        Some((lower.value, lower.source.as_str()))
    } else {
        None
    }
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

/// Validates data fields against schema versioning annotations.
///
/// When `target_version` is `None`, all checks are skipped and no warnings are produced.
/// Returns collected warnings (deprecation notices). Hard errors are returned as `SyamlError`.
pub fn validate_versioned_fields(
    data: &JsonValue,
    hints: &BTreeMap<String, String>,
    schema: &SchemaDoc,
    target_version: Option<&semver::Version>,
) -> Result<Vec<String>, SyamlError> {
    let Some(target) = target_version else {
        return Ok(Vec::new());
    };

    let mut warnings = Vec::new();

    for (hint_path, type_name) in hints {
        let type_schema = match schema.types.get(type_name) {
            Some(s) => s,
            None => continue,
        };
        let Some(type_obj) = type_schema.as_object() else {
            continue;
        };
        let Some(props) = type_obj.get("properties").and_then(JsonValue::as_object) else {
            continue;
        };

        let data_value = match get_json_path(data, hint_path) {
            Some(v) => v,
            None => continue,
        };
        let Some(data_obj) = data_value.as_object() else {
            continue;
        };

        for (prop_name, prop_schema) in props {
            let field_present = data_obj.contains_key(prop_name);

            let meta = match parse_field_version_meta(prop_schema) {
                Ok(m) => m,
                Err(_) => continue,
            };
            let Some(meta) = meta else {
                continue;
            };

            // `since` check: field is present but target < since → error
            if let Some(ref since) = meta.since {
                if field_present && target < since {
                    return Err(SyamlError::VersionFieldError(format!(
                        "field '{}' at '{}' is not available until version {} (target: {})",
                        prop_name, hint_path, since, target
                    )));
                }
            }

            // `removed` check: field is present and target >= removed → error
            if let Some(ref removed) = meta.removed {
                if field_present && target >= removed {
                    return Err(SyamlError::VersionFieldError(format!(
                        "field '{}' at '{}' was removed in version {} (target: {})",
                        prop_name, hint_path, removed, target
                    )));
                }
            }

            // `deprecated` check: field is present and target >= deprecated.version
            if let Some(ref dep_info) = meta.deprecated {
                if field_present && target >= &dep_info.version {
                    use crate::schema::DeprecationSeverity;
                    let msg = match &dep_info.message {
                        Some(m) => format!(
                            "field '{}' at '{}' is deprecated since version {}: {}",
                            prop_name, hint_path, dep_info.version, m
                        ),
                        None => format!(
                            "field '{}' at '{}' is deprecated since version {}",
                            prop_name, hint_path, dep_info.version
                        ),
                    };
                    match dep_info.severity {
                        DeprecationSeverity::Error => {
                            return Err(SyamlError::VersionFieldError(msg));
                        }
                        DeprecationSeverity::Warning => {
                            warnings.push(msg);
                        }
                    }
                }
            }
        }
    }

    Ok(warnings)
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
