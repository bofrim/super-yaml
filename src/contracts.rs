//! Parsing and validation for the `---contracts` section.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value as JsonValue;

use crate::ast::SchemaDoc;
use crate::ast::{
    ConditionSet, ContractsDoc, DataPermissions, FreezeMarkers, FunctionDef, ParameterDef,
    PermissionsDef, SpecificationDef,
};
use crate::error::SyamlError;

/// Parses a `---contracts` section value into a [`ContractsDoc`].
pub fn parse_contracts(value: &JsonValue) -> Result<ContractsDoc, SyamlError> {
    let map = value.as_object().ok_or_else(|| {
        SyamlError::ContractsError("contracts section must be a mapping/object".to_string())
    })?;

    let mut functions = BTreeMap::new();
    for (name, func_value) in map {
        let func_def = parse_function_def(name, func_value)?;
        functions.insert(name.clone(), func_def);
    }

    Ok(ContractsDoc { functions })
}

fn parse_function_def(name: &str, value: &JsonValue) -> Result<FunctionDef, SyamlError> {
    let map = value.as_object().ok_or_else(|| {
        SyamlError::ContractsError(format!("contracts.{} must be a mapping/object", name))
    })?;

    // inputs is required
    let inputs_raw = map.get("inputs").ok_or_else(|| {
        SyamlError::ContractsError(format!("contracts.{} must define 'inputs'", name))
    })?;
    let inputs_map = inputs_raw.as_object().ok_or_else(|| {
        SyamlError::ContractsError(format!(
            "contracts.{}.inputs must be a mapping/object",
            name
        ))
    })?;

    let mut inputs = BTreeMap::new();
    for (param_name, param_value) in inputs_map {
        let param_def = parse_parameter_def(name, param_name, param_value)?;
        inputs.insert(param_name.clone(), param_def);
    }

    let output = map.get("output").cloned();
    let errors = map.get("errors").cloned();

    let specification = if let Some(spec_val) = map.get("specification") {
        Some(parse_specification_def(name, spec_val)?)
    } else {
        None
    };

    let permissions = if let Some(perm_value) = map.get("permissions") {
        Some(parse_permissions_def(name, perm_value)?)
    } else {
        None
    };

    Ok(FunctionDef {
        inputs,
        output,
        errors,
        permissions,
        specification,
    })
}

fn parse_parameter_def(
    func_name: &str,
    param_name: &str,
    value: &JsonValue,
) -> Result<ParameterDef, SyamlError> {
    // Shorthand: "TypeName" -> {type_ref: {type: "TypeName"}, mutable: false}
    if let Some(type_str) = value.as_str() {
        return Ok(ParameterDef {
            type_ref: serde_json::json!({ "type": type_str }),
            mutable: false,
        });
    }

    // Expanded form: {type: "TypeName", mutable: true}
    let map = value.as_object().ok_or_else(|| {
        SyamlError::ContractsError(format!(
            "contracts.{}.inputs.{} must be a string or mapping",
            func_name, param_name
        ))
    })?;

    let type_ref = if let Some(t) = map.get("type") {
        serde_json::json!({ "type": t })
    } else {
        return Err(SyamlError::ContractsError(format!(
            "contracts.{}.inputs.{} must define 'type'",
            func_name, param_name
        )));
    };

    let mutable = map
        .get("mutable")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false);

    Ok(ParameterDef { type_ref, mutable })
}

fn parse_permissions_def(func_name: &str, value: &JsonValue) -> Result<PermissionsDef, SyamlError> {
    let map = value.as_object().ok_or_else(|| {
        SyamlError::ContractsError(format!(
            "contracts.{}.permissions must be a mapping/object",
            func_name
        ))
    })?;

    let file = map.get("file").cloned();
    let network = map.get("network").cloned();
    let env_perms = map.get("env").cloned();
    let process = map.get("process").cloned();

    let data = if let Some(data_val) = map.get("data") {
        let data_map = data_val.as_object().ok_or_else(|| {
            SyamlError::ContractsError(format!(
                "contracts.{}.permissions.data must be a mapping/object",
                func_name
            ))
        })?;

        let read = parse_string_list(
            data_map.get("read"),
            &format!("contracts.{}.permissions.data.read", func_name),
        )?;
        let write = parse_string_list(
            data_map.get("write"),
            &format!("contracts.{}.permissions.data.write", func_name),
        )?;

        Some(DataPermissions { read, write })
    } else {
        None
    };

    Ok(PermissionsDef {
        file,
        network,
        env_perms,
        process,
        data,
    })
}

fn parse_string_list(value: Option<&JsonValue>, path: &str) -> Result<Vec<String>, SyamlError> {
    let Some(val) = value else {
        return Ok(Vec::new());
    };
    let arr = val
        .as_array()
        .ok_or_else(|| SyamlError::ContractsError(format!("{} must be an array", path)))?;
    let mut out = Vec::new();
    for item in arr {
        let s = item.as_str().ok_or_else(|| {
            SyamlError::ContractsError(format!("{} entries must be strings", path))
        })?;
        out.push(s.to_string());
    }
    Ok(out)
}

fn parse_condition_set(value: &JsonValue, path: &str) -> Result<ConditionSet, SyamlError> {
    // Array form: treat entries as semantic conditions.
    if let Some(arr) = value.as_array() {
        let mut semantic = Vec::new();
        for item in arr {
            let s = item.as_str().ok_or_else(|| {
                SyamlError::ContractsError(format!("{} array entries must be strings", path))
            })?;
            semantic.push(s.to_string());
        }
        return Ok(ConditionSet {
            semantic,
            strict: Vec::new(),
        });
    }

    // Object form: {semantic: [...], strict: [...]}
    let map = value.as_object().ok_or_else(|| {
        SyamlError::ContractsError(format!("{} must be an array or mapping/object", path))
    })?;

    let semantic = parse_string_list(map.get("semantic"), &format!("{}.semantic", path))?;
    let strict = parse_string_list(map.get("strict"), &format!("{}.strict", path))?;

    Ok(ConditionSet { semantic, strict })
}

fn parse_specification_def(
    func_name: &str,
    value: &JsonValue,
) -> Result<SpecificationDef, SyamlError> {
    let map = value.as_object().ok_or_else(|| {
        SyamlError::ContractsError(format!(
            "contracts.{}.specification must be a mapping/object",
            func_name
        ))
    })?;

    let preconditions = if let Some(pre_val) = map.get("preconditions") {
        Some(parse_condition_set(
            pre_val,
            &format!("contracts.{}.specification.preconditions", func_name),
        )?)
    } else {
        None
    };

    let postconditions = if let Some(post_val) = map.get("postconditions") {
        Some(parse_condition_set(
            post_val,
            &format!("contracts.{}.specification.postconditions", func_name),
        )?)
    } else {
        None
    };

    // Collect remaining keys as extra pass-through.
    let mut extra = std::collections::BTreeMap::new();
    for (key, val) in map {
        if key != "preconditions" && key != "postconditions" {
            extra.insert(key.clone(), val.clone());
        }
    }

    Ok(SpecificationDef {
        preconditions,
        postconditions,
        extra,
    })
}

/// Validates strict conditions in specification blocks: syntax check + scope check.
///
/// For each function with a specification containing strict conditions:
/// - Parses each expression (syntax error → ContractsError)
/// - Walks the AST for variable references
/// - Verifies roots are in-scope (input/data for preconditions; input/data/output for postconditions)
/// - Verifies input params exist, data paths are covered by permissions.data.read, output is declared
pub fn validate_specification_strict_conditions(doc: &ContractsDoc) -> Result<(), SyamlError> {
    use crate::expr::parse_expression;
    use crate::schema::collect_var_paths;

    for (func_name, func_def) in &doc.functions {
        let Some(spec) = &func_def.specification else {
            continue;
        };

        let input_params: std::collections::BTreeSet<String> =
            func_def.inputs.keys().cloned().collect();

        let read_paths: Vec<String> = func_def
            .permissions
            .as_ref()
            .and_then(|p| p.data.as_ref())
            .map(|d| d.read.clone())
            .unwrap_or_default();

        let has_output = func_def.output.is_some();

        // Validate preconditions (strict)
        if let Some(cond_set) = &spec.preconditions {
            for expr_str in &cond_set.strict {
                let ast = parse_expression(expr_str).map_err(|e| {
                    SyamlError::ContractsError(format!(
                        "contracts.{}: invalid strict precondition expression '{}': {}",
                        func_name, expr_str, e
                    ))
                })?;

                let mut var_paths = Vec::new();
                collect_var_paths(&ast, &mut var_paths);

                for var_path in &var_paths {
                    let Some(root) = var_path.first() else {
                        continue;
                    };
                    match root.as_str() {
                        "input" => {
                            let param = var_path.get(1).ok_or_else(|| {
                                SyamlError::ContractsError(format!(
                                    "contracts.{}: strict precondition '{}': 'input' must be followed by a parameter name (e.g. input.x)",
                                    func_name, expr_str
                                ))
                            })?;
                            if !input_params.contains(param) {
                                return Err(SyamlError::ContractsError(format!(
                                    "contracts.{}: strict precondition '{}': unknown input parameter '{}'",
                                    func_name, expr_str, param
                                )));
                            }
                        }
                        "data" => {
                            let data_segs = &var_path[1..];
                            if !read_path_covers_segments(&read_paths, data_segs) {
                                return Err(SyamlError::ContractsError(format!(
                                    "contracts.{}: strict precondition '{}': data path '{}' is not covered by permissions.data.read",
                                    func_name, expr_str, var_path.join(".")
                                )));
                            }
                        }
                        "output" => {
                            return Err(SyamlError::ContractsError(format!(
                                "contracts.{}: strict precondition '{}': 'output' is not allowed in preconditions",
                                func_name, expr_str
                            )));
                        }
                        other => {
                            return Err(SyamlError::ContractsError(format!(
                                "contracts.{}: strict precondition '{}': unknown variable root '{}'; allowed roots are 'input' and 'data'",
                                func_name, expr_str, other
                            )));
                        }
                    }
                }
            }
        }

        // Validate postconditions (strict)
        if let Some(cond_set) = &spec.postconditions {
            for expr_str in &cond_set.strict {
                let ast = parse_expression(expr_str).map_err(|e| {
                    SyamlError::ContractsError(format!(
                        "contracts.{}: invalid strict postcondition expression '{}': {}",
                        func_name, expr_str, e
                    ))
                })?;

                let mut var_paths = Vec::new();
                collect_var_paths(&ast, &mut var_paths);

                for var_path in &var_paths {
                    let Some(root) = var_path.first() else {
                        continue;
                    };
                    match root.as_str() {
                        "input" => {
                            let param = var_path.get(1).ok_or_else(|| {
                                SyamlError::ContractsError(format!(
                                    "contracts.{}: strict postcondition '{}': 'input' must be followed by a parameter name (e.g. input.x)",
                                    func_name, expr_str
                                ))
                            })?;
                            if !input_params.contains(param) {
                                return Err(SyamlError::ContractsError(format!(
                                    "contracts.{}: strict postcondition '{}': unknown input parameter '{}'",
                                    func_name, expr_str, param
                                )));
                            }
                        }
                        "data" => {
                            let data_segs = &var_path[1..];
                            if !read_path_covers_segments(&read_paths, data_segs) {
                                return Err(SyamlError::ContractsError(format!(
                                    "contracts.{}: strict postcondition '{}': data path '{}' is not covered by permissions.data.read",
                                    func_name, expr_str, var_path.join(".")
                                )));
                            }
                        }
                        "output" => {
                            if !has_output {
                                return Err(SyamlError::ContractsError(format!(
                                    "contracts.{}: strict postcondition '{}': 'output' referenced but function declares no output type",
                                    func_name, expr_str
                                )));
                            }
                            // output.X path: valid when output is declared (no deeper schema check needed)
                        }
                        other => {
                            return Err(SyamlError::ContractsError(format!(
                                "contracts.{}: strict postcondition '{}': unknown variable root '{}'; allowed roots are 'input', 'data', and 'output'",
                                func_name, expr_str, other
                            )));
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// Returns true if any path in `read_paths` covers the given data segments.
///
/// A read path `$.a.b` covers `["a", "b"]` (exact) or `["a", "b", "c"]` (ancestor).
/// Wildcard paths (containing `*`) always cover.
fn read_path_covers_segments(read_paths: &[String], data_segs: &[String]) -> bool {
    for read_path in read_paths {
        if read_path.contains('*') {
            return true;
        }
        let read_segs = normalize_path(read_path);
        // read path is a prefix of or equal to data_segs
        if read_segs.len() <= data_segs.len() && read_segs == data_segs[..read_segs.len()] {
            return true;
        }
    }
    false
}

/// Validates that all type references in contracts definitions exist in the type registry.
pub fn validate_contracts_type_references(
    doc: &ContractsDoc,
    types: &BTreeMap<String, JsonValue>,
) -> Result<(), SyamlError> {
    for (func_name, func_def) in &doc.functions {
        for (param_name, param_def) in &func_def.inputs {
            let type_ref = &param_def.type_ref;
            if let Some(type_name) = type_ref.get("type").and_then(JsonValue::as_str) {
                if !is_builtin_type(type_name) && !types.contains_key(type_name) {
                    return Err(SyamlError::ContractsError(format!(
                        "contracts.{}.inputs.{}: unknown type '{}'",
                        func_name, param_name, type_name
                    )));
                }
            }
        }

        if let Some(output) = &func_def.output {
            if let Some(type_name) = output.get("type").and_then(JsonValue::as_str) {
                if !is_builtin_type(type_name) && !types.contains_key(type_name) {
                    return Err(SyamlError::ContractsError(format!(
                        "contracts.{}.output: unknown type '{}'",
                        func_name, type_name
                    )));
                }
            }
        }

        if let Some(errors) = &func_def.errors {
            if let Some(type_name) = errors.get("type").and_then(JsonValue::as_str) {
                if !is_builtin_type(type_name) && !types.contains_key(type_name) {
                    return Err(SyamlError::ContractsError(format!(
                        "contracts.{}.errors: unknown type '{}'",
                        func_name, type_name
                    )));
                }
            }
        }
    }
    Ok(())
}

fn is_builtin_type(name: &str) -> bool {
    matches!(
        name,
        "string" | "integer" | "number" | "boolean" | "object" | "array" | "null"
    )
}

/// Validates permission data paths against the actual data structure.
pub fn validate_permission_data_paths(
    doc: &ContractsDoc,
    data: &JsonValue,
    import_aliases: &BTreeSet<String>,
) -> Result<(), SyamlError> {
    for (func_name, func_def) in &doc.functions {
        let Some(perms) = &func_def.permissions else {
            continue;
        };
        let Some(data_perms) = &perms.data else {
            continue;
        };

        let all_paths: Vec<&str> = data_perms
            .read
            .iter()
            .chain(data_perms.write.iter())
            .map(|s| s.as_str())
            .collect();

        for path in all_paths {
            // Reject paths rooted at import aliases
            let root_segment = path.split('.').next().unwrap_or("");
            let root_stripped = root_segment.trim_start_matches('$').trim_start_matches('.');
            let first_key = root_stripped.split('.').next().unwrap_or(root_stripped);
            if import_aliases.contains(first_key) {
                return Err(SyamlError::ContractsError(format!(
                    "contracts.{}: permission path '{}' rooted at import alias '{}' is not allowed",
                    func_name, path, first_key
                )));
            }

            // Verify path exists in data (handle wildcard '*')
            if path.contains('*') {
                continue; // wildcard paths are accepted as-is
            }

            let normalized = normalize_path(path);
            if !path_exists_in_data(&normalized, data) {
                return Err(SyamlError::ContractsError(format!(
                    "contracts.{}: permission path '{}' does not exist in data",
                    func_name, path
                )));
            }
        }
    }
    Ok(())
}

fn normalize_path(path: &str) -> Vec<String> {
    // Strip leading `$.` or `$`
    let stripped = path.trim_start_matches('$').trim_start_matches('.');
    if stripped.is_empty() {
        return Vec::new();
    }
    stripped.split('.').map(String::from).collect()
}

fn path_exists_in_data(segments: &[String], data: &JsonValue) -> bool {
    let mut current = data;
    for seg in segments {
        match current.as_object() {
            Some(obj) => {
                if let Some(next) = obj.get(seg.as_str()) {
                    current = next;
                } else {
                    return false;
                }
            }
            None => return false,
        }
    }
    true
}

/// Validates that write paths don't target schema-frozen fields.
pub fn validate_permission_mutability_alignment(
    doc: &ContractsDoc,
    schema: &SchemaDoc,
    type_hints: &BTreeMap<String, String>,
) -> Result<(), SyamlError> {
    use crate::ast::MutabilityMode;
    use crate::schema::resolve_mutability_for_path;

    for (func_name, func_def) in &doc.functions {
        let Some(perms) = &func_def.permissions else {
            continue;
        };
        let Some(data_perms) = &perms.data else {
            continue;
        };

        for write_path in &data_perms.write {
            let normalized = if write_path.starts_with('$') {
                write_path.clone()
            } else {
                format!("$.{}", write_path)
            };

            let mode = resolve_mutability_for_path(&normalized, type_hints, schema)?;
            if mode == MutabilityMode::Frozen {
                return Err(SyamlError::ContractsError(format!(
                    "contracts.{}: write permission on '{}' conflicts with schema mutability 'frozen'",
                    func_name, write_path
                )));
            }
        }
    }
    Ok(())
}

/// Validates that write paths don't target instance-frozen keys.
pub fn validate_permission_instance_lock_conflicts(
    doc: &ContractsDoc,
    freeze_markers: &FreezeMarkers,
) -> Result<(), SyamlError> {
    for (func_name, func_def) in &doc.functions {
        let Some(perms) = &func_def.permissions else {
            continue;
        };
        let Some(data_perms) = &perms.data else {
            continue;
        };

        for write_path in &data_perms.write {
            let normalized = if write_path.starts_with('$') {
                write_path.clone()
            } else {
                format!("$.{}", write_path)
            };

            // Check if this exact path or any ancestor path is frozen
            if is_path_frozen(&normalized, freeze_markers) {
                return Err(SyamlError::ContractsError(format!(
                    "contracts.{}: write permission on '{}' conflicts with instance-level freeze marker",
                    func_name, write_path
                )));
            }
        }
    }
    Ok(())
}

fn is_path_frozen(path: &str, freeze_markers: &FreezeMarkers) -> bool {
    // Check exact path
    if freeze_markers.get(path).copied().unwrap_or(false) {
        return true;
    }

    // Check ancestor paths
    let segments: Vec<&str> = path.split('.').collect();
    for len in 1..segments.len() {
        let ancestor = segments[..len].join(".");
        if freeze_markers.get(&ancestor).copied().unwrap_or(false) {
            return true;
        }
    }

    false
}

/// Serializes a `ContractsDoc` to JSON text.
pub fn contracts_to_json(doc: &ContractsDoc, pretty: bool) -> Result<String, SyamlError> {
    if pretty {
        serde_json::to_string_pretty(doc).map_err(|e| SyamlError::SerializationError(e.to_string()))
    } else {
        serde_json::to_string(doc).map_err(|e| SyamlError::SerializationError(e.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Condition expression helpers
// ---------------------------------------------------------------------------

/// Collects unique `input.X` param names, `data.X` field names, and whether
/// `output` is referenced in a condition expression string.
fn collect_condition_refs(expr: &str) -> (BTreeSet<String>, BTreeSet<String>, bool) {
    let mut input_refs = BTreeSet::new();
    let mut data_refs = BTreeSet::new();
    let mut uses_output = false;

    let bytes = expr.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'.')
            {
                i += 1;
            }
            let token = &expr[start..i];
            if let Some(param) = token.strip_prefix("input.") {
                input_refs.insert(param.to_string());
            } else if let Some(field) = token.strip_prefix("data.") {
                data_refs.insert(field.to_string());
            } else if token == "output" {
                uses_output = true;
            }
        } else {
            i += 1;
        }
    }

    (input_refs, data_refs, uses_output)
}

/// Translates a syaml condition expression to target-language source by substituting
/// `input.X`, `data.X`, and `output` with the provided name functions.
/// All other tokens (operators, numbers, booleans, function names) pass through unchanged.
fn translate_condition_expr(
    expr: &str,
    input_fn: &dyn Fn(&str) -> String,
    data_fn: &dyn Fn(&str) -> String,
    output_var: &str,
) -> String {
    let mut result = String::new();
    let bytes = expr.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'.')
            {
                i += 1;
            }
            let token = &expr[start..i];
            if let Some(param) = token.strip_prefix("input.") {
                result.push_str(&input_fn(param));
            } else if let Some(field) = token.strip_prefix("data.") {
                result.push_str(&data_fn(field));
            } else if token == "output" {
                result.push_str(output_var);
            } else {
                result.push_str(token);
            }
        } else {
            result.push(bytes[i] as char);
            i += 1;
        }
    }

    result
}

/// Resolves a type_ref to its primitive base kind: "integer", "number", "string",
/// "boolean", "array", "object", or "unknown".  Follows named-type references one
/// level into the schema registry.
fn resolve_base_kind<'a>(
    type_ref: &'a JsonValue,
    types: &'a BTreeMap<String, JsonValue>,
) -> &'a str {
    if let Some(t) = type_ref.get("type").and_then(|v| v.as_str()) {
        match t {
            "integer" | "number" | "string" | "boolean" | "array" | "object" | "null" => t,
            named => {
                if let Some(schema) = types.get(named) {
                    // Follow one level — avoid infinite recursion by not recursing further
                    schema
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                } else {
                    "unknown"
                }
            }
        }
    } else {
        "unknown"
    }
}

/// Returns the dominant base kind for all input params referenced in a condition
/// expression (used to choose the matching data-field extraction method).
fn dominant_kind_for_expr(
    expr: &str,
    inputs: &BTreeMap<String, ParameterDef>,
    types: &BTreeMap<String, JsonValue>,
) -> &'static str {
    let (input_refs, _, _) = collect_condition_refs(expr);
    for param_name in &input_refs {
        if let Some(param_def) = inputs.get(param_name) {
            return match resolve_base_kind(&param_def.type_ref, types) {
                "integer" => "integer",
                "number" => "number",
                "string" => "string",
                "boolean" => "boolean",
                _ => "number",
            };
        }
    }
    "number"
}

fn has_any_strict_conditions(func_def: &FunctionDef) -> bool {
    let Some(spec) = &func_def.specification else {
        return false;
    };
    spec.preconditions
        .as_ref()
        .map(|c| !c.strict.is_empty())
        .unwrap_or(false)
        || spec
            .postconditions
            .as_ref()
            .map(|c| !c.strict.is_empty())
            .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Inline-type detection and generation
// ---------------------------------------------------------------------------

/// Returns true when a type_ref represents an inline (anonymous) schema that
/// needs a dedicated generated type rather than a simple primitive or named ref.
fn needs_generated_type(type_ref: &JsonValue) -> bool {
    let Some(t) = type_ref.get("type").and_then(|v| v.as_str()) else {
        return false;
    };
    match t {
        "integer" | "number" | "string" | "boolean" | "null" => false,
        "array" => {
            // Arrays with complex item schemas need a generated type
            type_ref
                .get("items")
                .map(|items| {
                    !matches!(
                        items.get("type").and_then(|v| v.as_str()),
                        Some("integer" | "number" | "string" | "boolean" | "null")
                    )
                })
                .unwrap_or(false)
        }
        "object" => type_ref.get("properties").is_some(),
        _ => false, // Named type reference — use as-is
    }
}

/// Collects any parameters that require a generated named type, returning
/// (type_name, Rust_struct_code) pairs.  Emitted before the function stubs.
fn collect_generated_types_rust(
    doc: &ContractsDoc,
    types: &BTreeMap<String, JsonValue>,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (func_name, func_def) in &doc.functions {
        for (param_name, param_def) in &func_def.inputs {
            if needs_generated_type(&param_def.type_ref) {
                let type_name = format!(
                    "{}{}Input",
                    to_pascal_case(func_name),
                    to_pascal_case(param_name)
                );
                let code = generate_rust_struct(&type_name, &param_def.type_ref, types);
                out.push((type_name, code));
            }
        }
        if let Some(output) = &func_def.output {
            if needs_generated_type(output) {
                let type_name = format!("{}Output", to_pascal_case(func_name));
                let code = generate_rust_struct(&type_name, output, types);
                out.push((type_name, code));
            }
        }
    }
    out
}

fn generate_rust_struct(
    type_name: &str,
    schema: &JsonValue,
    types: &BTreeMap<String, JsonValue>,
) -> String {
    let mut out = String::new();
    out.push_str("#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]\n");
    out.push_str(&format!("pub struct {} {{\n", type_name));
    if let Some(props) = schema.get("properties").and_then(|v| v.as_object()) {
        for (field_name, field_schema) in props {
            let rust_type = schema_to_rust_type(field_schema, types);
            out.push_str(&format!(
                "    pub {}: {},\n",
                to_snake_case(field_name),
                rust_type
            ));
        }
    }
    out.push_str("}\n");
    out
}

/// Same as above but for TypeScript interfaces.
fn collect_generated_types_ts(
    doc: &ContractsDoc,
    types: &BTreeMap<String, JsonValue>,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (func_name, func_def) in &doc.functions {
        for (param_name, param_def) in &func_def.inputs {
            if needs_generated_type(&param_def.type_ref) {
                let type_name = format!(
                    "{}{}Input",
                    to_pascal_case(func_name),
                    to_pascal_case(param_name)
                );
                let code = generate_ts_interface(&type_name, &param_def.type_ref, types);
                out.push((type_name, code));
            }
        }
        if let Some(output) = &func_def.output {
            if needs_generated_type(output) {
                let type_name = format!("{}Output", to_pascal_case(func_name));
                let code = generate_ts_interface(&type_name, output, types);
                out.push((type_name, code));
            }
        }
    }
    out
}

fn generate_ts_interface(
    type_name: &str,
    schema: &JsonValue,
    types: &BTreeMap<String, JsonValue>,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("export interface {} {{\n", type_name));
    if let Some(props) = schema.get("properties").and_then(|v| v.as_object()) {
        for (field_name, field_schema) in props {
            let ts_type = schema_to_ts_type(field_schema, types);
            out.push_str(&format!("  {}: {};\n", to_camel_case(field_name), ts_type));
        }
    }
    out.push_str("}\n");
    out
}

// ---------------------------------------------------------------------------
// Rust condition function builders
// ---------------------------------------------------------------------------

/// Returns the Rust type name to use for a parameter in the generated check
/// functions — respects named type aliases instead of resolving to primitives.
fn rust_check_param_type(param_def: &ParameterDef, types: &BTreeMap<String, JsonValue>) -> String {
    if needs_generated_type(&param_def.type_ref) {
        // Inline schema — the generated type name is handled by the caller; fall back
        schema_to_rust_type(&param_def.type_ref, types)
    } else {
        schema_to_rust_type(&param_def.type_ref, types)
    }
}

/// Generates a Rust data-field extraction line matched to the given primitive kind.
fn rust_data_extraction_for_kind(field: &str, kind: &str) -> String {
    let var = format!("data_{}", to_snake_case(field));
    match kind {
        "integer" => format!(
            "    let {var} = data.get(\"{field}\").and_then(|v| v.as_i64()).unwrap_or(0);\n"
        ),
        "string" => format!(
            "    let {var} = data.get(\"{field}\").and_then(|v| v.as_str()).unwrap_or(\"\").to_string();\n"
        ),
        "boolean" => format!(
            "    let {var} = data.get(\"{field}\").and_then(|v| v.as_bool()).unwrap_or(false);\n"
        ),
        _ => format!(
            "    let {var} = data.get(\"{field}\").and_then(|v| v.as_f64()).unwrap_or(0.0);\n"
        ),
    }
}

fn build_preconditions_fn_rust(
    func_name: &str,
    func_def: &FunctionDef,
    types: &BTreeMap<String, JsonValue>,
) -> Option<String> {
    let spec = func_def.specification.as_ref()?;
    let cond_set = spec.preconditions.as_ref()?;
    if cond_set.strict.is_empty() {
        return None;
    }

    // Collect all unique refs across strict preconditions
    let mut all_input_refs: BTreeSet<String> = BTreeSet::new();
    let mut all_data_refs: BTreeSet<String> = BTreeSet::new();
    for expr in &cond_set.strict {
        let (inp, dat, _) = collect_condition_refs(expr);
        all_input_refs.extend(inp);
        all_data_refs.extend(dat);
    }

    let fn_name = format!("{}_check_preconditions", to_snake_case(func_name));

    // Build parameter list: typed input params (in declaration order) + data if needed
    let needs_data = !all_data_refs.is_empty();
    let mut params: Vec<String> = func_def
        .inputs
        .iter()
        .filter(|(name, _)| all_input_refs.contains(*name))
        .map(|(name, def)| {
            format!(
                "{}: {}",
                to_snake_case(name),
                rust_check_param_type(def, types)
            )
        })
        .collect();
    if needs_data {
        params.push("data: &serde_json::Value".to_string());
    } else {
        params.push("_data: &serde_json::Value".to_string());
    }

    // Build body: data extractions then condition checks
    let mut body = String::new();
    for field in &all_data_refs {
        // Determine extraction kind from the expression(s) that reference this field
        let kind = cond_set
            .strict
            .iter()
            .find(|expr| {
                let (_, dat, _) = collect_condition_refs(expr);
                dat.contains(field)
            })
            .map(|expr| dominant_kind_for_expr(expr, &func_def.inputs, types))
            .unwrap_or("number");
        body.push_str(&rust_data_extraction_for_kind(field, kind));
    }
    if !all_data_refs.is_empty() {
        body.push('\n');
    }

    let input_fn = |p: &str| to_snake_case(p);
    let data_fn = |f: &str| format!("data_{}", to_snake_case(f));
    for expr in &cond_set.strict {
        let translated = translate_condition_expr(expr, &input_fn, &data_fn, "output");
        body.push_str(&format!(
            "    if !({translated}) {{\n        return Err(\"precondition violated: {expr}\".to_string());\n    }}\n"
        ));
    }
    body.push_str("    Ok(())\n");

    Some(format!(
        "fn {fn_name}(\n    {}\n) -> Result<(), String> {{\n{body}}}\n",
        params.join(",\n    ")
    ))
}

fn build_postconditions_fn_rust(
    func_name: &str,
    func_def: &FunctionDef,
    types: &BTreeMap<String, JsonValue>,
) -> Option<String> {
    let spec = func_def.specification.as_ref()?;
    let cond_set = spec.postconditions.as_ref()?;
    if cond_set.strict.is_empty() {
        return None;
    }

    let mut all_input_refs: BTreeSet<String> = BTreeSet::new();
    let mut all_data_refs: BTreeSet<String> = BTreeSet::new();
    let mut uses_output = false;
    for expr in &cond_set.strict {
        let (inp, dat, out) = collect_condition_refs(expr);
        all_input_refs.extend(inp);
        all_data_refs.extend(dat);
        uses_output = uses_output || out;
    }

    let fn_name = format!("{}_check_postconditions", to_snake_case(func_name));
    let needs_data = !all_data_refs.is_empty();

    let mut params: Vec<String> = func_def
        .inputs
        .iter()
        .filter(|(name, _)| all_input_refs.contains(*name))
        .map(|(name, def)| {
            format!(
                "{}: {}",
                to_snake_case(name),
                rust_check_param_type(def, types)
            )
        })
        .collect();
    if uses_output {
        let output_type = func_def
            .output
            .as_ref()
            .map(|o| schema_to_rust_type(o, types))
            .unwrap_or_else(|| "()".to_string());
        params.push(format!("output: {output_type}"));
    }
    if needs_data {
        params.push("data: &serde_json::Value".to_string());
    } else {
        params.push("_data: &serde_json::Value".to_string());
    }

    let mut body = String::new();
    for field in &all_data_refs {
        let kind = cond_set
            .strict
            .iter()
            .find(|expr| {
                let (_, dat, _) = collect_condition_refs(expr);
                dat.contains(field)
            })
            .map(|expr| dominant_kind_for_expr(expr, &func_def.inputs, types))
            .unwrap_or("number");
        body.push_str(&rust_data_extraction_for_kind(field, kind));
    }
    if !all_data_refs.is_empty() {
        body.push('\n');
    }

    let input_fn = |p: &str| to_snake_case(p);
    let data_fn = |f: &str| format!("data_{}", to_snake_case(f));
    for expr in &cond_set.strict {
        let translated = translate_condition_expr(expr, &input_fn, &data_fn, "output");
        body.push_str(&format!(
            "    if !({translated}) {{\n        return Err(\"postcondition violated: {expr}\".to_string());\n    }}\n"
        ));
    }
    body.push_str("    Ok(())\n");

    Some(format!(
        "fn {fn_name}(\n    {}\n) -> Result<(), String> {{\n{body}}}\n",
        params.join(",\n    ")
    ))
}

// ---------------------------------------------------------------------------
// TypeScript condition function builders
// ---------------------------------------------------------------------------

fn ts_check_param_type(param_def: &ParameterDef, types: &BTreeMap<String, JsonValue>) -> String {
    schema_to_ts_type(&param_def.type_ref, types)
}

fn ts_data_extraction_for_kind(field: &str, kind: &str) -> String {
    let var = format!("data{}", to_pascal_case(field));
    match kind {
        "string" => format!("  const {var} = String(data[\"{field}\"] ?? \"\");\n"),
        "boolean" => format!("  const {var} = Boolean(data[\"{field}\"] ?? false);\n"),
        _ => format!("  const {var} = Number(data[\"{field}\"] ?? 0);\n"),
    }
}

fn build_preconditions_fn_ts(
    func_name: &str,
    func_def: &FunctionDef,
    types: &BTreeMap<String, JsonValue>,
) -> Option<String> {
    let spec = func_def.specification.as_ref()?;
    let cond_set = spec.preconditions.as_ref()?;
    if cond_set.strict.is_empty() {
        return None;
    }

    let mut all_input_refs: BTreeSet<String> = BTreeSet::new();
    let mut all_data_refs: BTreeSet<String> = BTreeSet::new();
    for expr in &cond_set.strict {
        let (inp, dat, _) = collect_condition_refs(expr);
        all_input_refs.extend(inp);
        all_data_refs.extend(dat);
    }

    let fn_name = format!("{}CheckPreconditions", to_camel_case(func_name));

    let params: Vec<String> = func_def
        .inputs
        .iter()
        .filter(|(name, _)| all_input_refs.contains(*name))
        .map(|(name, def)| {
            format!(
                "{}: {}",
                to_camel_case(name),
                ts_check_param_type(def, types)
            )
        })
        .collect();

    let needs_data = !all_data_refs.is_empty();
    let mut all_params = params;
    if needs_data {
        all_params.push("data: Record<string, unknown>".to_string());
    }

    let mut body = String::new();
    for field in &all_data_refs {
        let kind = cond_set
            .strict
            .iter()
            .find(|expr| {
                let (_, dat, _) = collect_condition_refs(expr);
                dat.contains(field)
            })
            .map(|expr| dominant_kind_for_expr(expr, &func_def.inputs, types))
            .unwrap_or("number");
        body.push_str(&ts_data_extraction_for_kind(field, kind));
    }
    if !all_data_refs.is_empty() {
        body.push('\n');
    }

    let input_fn = |p: &str| to_camel_case(p);
    let data_fn = |f: &str| format!("data{}", to_pascal_case(f));
    for expr in &cond_set.strict {
        let translated = translate_condition_expr(expr, &input_fn, &data_fn, "output");
        body.push_str(&format!(
            "  if (!({translated})) throw new Error(\"precondition violated: {expr}\");\n"
        ));
    }

    Some(format!(
        "function {fn_name}({params}): void {{\n{body}}}\n",
        params = all_params.join(", ")
    ))
}

fn build_postconditions_fn_ts(
    func_name: &str,
    func_def: &FunctionDef,
    types: &BTreeMap<String, JsonValue>,
) -> Option<String> {
    let spec = func_def.specification.as_ref()?;
    let cond_set = spec.postconditions.as_ref()?;
    if cond_set.strict.is_empty() {
        return None;
    }

    let mut all_input_refs: BTreeSet<String> = BTreeSet::new();
    let mut all_data_refs: BTreeSet<String> = BTreeSet::new();
    let mut uses_output = false;
    for expr in &cond_set.strict {
        let (inp, dat, out) = collect_condition_refs(expr);
        all_input_refs.extend(inp);
        all_data_refs.extend(dat);
        uses_output = uses_output || out;
    }

    let fn_name = format!("{}CheckPostconditions", to_camel_case(func_name));

    let mut params: Vec<String> = func_def
        .inputs
        .iter()
        .filter(|(name, _)| all_input_refs.contains(*name))
        .map(|(name, def)| {
            format!(
                "{}: {}",
                to_camel_case(name),
                ts_check_param_type(def, types)
            )
        })
        .collect();
    if uses_output {
        let output_type = func_def
            .output
            .as_ref()
            .map(|o| schema_to_ts_type(o, types))
            .unwrap_or_else(|| "void".to_string());
        params.push(format!("output: {output_type}"));
    }
    let needs_data = !all_data_refs.is_empty();
    if needs_data {
        params.push("data: Record<string, unknown>".to_string());
    }

    let mut body = String::new();
    for field in &all_data_refs {
        let kind = cond_set
            .strict
            .iter()
            .find(|expr| {
                let (_, dat, _) = collect_condition_refs(expr);
                dat.contains(field)
            })
            .map(|expr| dominant_kind_for_expr(expr, &func_def.inputs, types))
            .unwrap_or("number");
        body.push_str(&ts_data_extraction_for_kind(field, kind));
    }
    if !all_data_refs.is_empty() {
        body.push('\n');
    }

    let input_fn = |p: &str| to_camel_case(p);
    let data_fn = |f: &str| format!("data{}", to_pascal_case(f));
    for expr in &cond_set.strict {
        let translated = translate_condition_expr(expr, &input_fn, &data_fn, "output");
        body.push_str(&format!(
            "  if (!({translated})) throw new Error(\"postcondition violated: {expr}\");\n"
        ));
    }

    Some(format!(
        "function {fn_name}({params}): void {{\n{body}}}\n",
        params = params.join(", ")
    ))
}

// ---------------------------------------------------------------------------
// Stub generators
// ---------------------------------------------------------------------------

/// Generates Rust function stubs from a contracts document.
///
/// For functions with strict pre/postconditions the output is split into four
/// pieces: a typed `_check_preconditions` function, a typed
/// `_check_postconditions` function, a private `_impl` stub, and a public
/// entry-point that chains them: validate-pre → call-impl → validate-post.
///
/// Every check function takes explicit typed parameters (no `serde_json::Value`
/// for inputs).  If any parameter has an inline object/array schema a
/// named Rust struct is generated before the stubs section.
///
/// Functions without strict conditions emit a single simple stub.
pub fn generate_rust_function_stubs(
    doc: &ContractsDoc,
    types: &BTreeMap<String, JsonValue>,
) -> String {
    if doc.functions.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    out.push_str("// --- Contracts stubs ---\n\n");

    // Emit any generated types for inline parameter schemas
    for (_, code) in collect_generated_types_rust(doc, types) {
        out.push_str(&code);
        out.push('\n');
    }

    for (func_name, func_def) in &doc.functions {
        let snake_name = to_snake_case(func_name);

        // Typed parameter list (shared by impl and public functions)
        let typed_params: Vec<String> = func_def
            .inputs
            .iter()
            .map(|(param_name, param_def)| {
                let rust_type = schema_to_rust_type(&param_def.type_ref, types);
                let mut_prefix = if param_def.mutable { "mut " } else { "" };
                format!("{}{}: {}", mut_prefix, to_snake_case(param_name), rust_type)
            })
            .collect();

        // Base return type (without Result wrapping from condition checks)
        let base_return_type = if let Some(output) = &func_def.output {
            schema_to_rust_type(output, types)
        } else {
            "()".to_string()
        };

        // Impl return type may wrap in Result when errors are declared
        let impl_return_type = if let Some(output) = &func_def.output {
            let base = schema_to_rust_type(output, types);
            if func_def.errors.is_some() {
                format!("Result<{}, Box<dyn std::error::Error>>", base)
            } else {
                base
            }
        } else if func_def.errors.is_some() {
            "Result<(), Box<dyn std::error::Error>>".to_string()
        } else {
            "()".to_string()
        };

        if has_any_strict_conditions(func_def) {
            // --- check_preconditions ---
            if let Some(pre_fn) = build_preconditions_fn_rust(func_name, func_def, types) {
                out.push_str(&pre_fn);
                out.push('\n');
            }

            // --- check_postconditions ---
            if let Some(post_fn) = build_postconditions_fn_rust(func_name, func_def, types) {
                out.push_str(&post_fn);
                out.push('\n');
            }

            // --- _impl ---
            if let Some(perms) = &func_def.permissions {
                out.push_str("/// permissions:");
                if let Some(dp) = &perms.data {
                    if !dp.read.is_empty() {
                        out.push_str(&format!(" read=[{}]", dp.read.join(", ")));
                    }
                    if !dp.write.is_empty() {
                        out.push_str(&format!(" write=[{}]", dp.write.join(", ")));
                    }
                }
                out.push('\n');
            }
            out.push_str(&format!(
                "fn {snake_name}_impl({}) -> {} {{\n    todo!()\n}}\n\n",
                typed_params.join(", "),
                impl_return_type,
            ));

            // --- public entry-point ---
            let has_pre = func_def
                .specification
                .as_ref()
                .and_then(|s| s.preconditions.as_ref())
                .map(|c| !c.strict.is_empty())
                .unwrap_or(false);
            let has_post = func_def
                .specification
                .as_ref()
                .and_then(|s| s.postconditions.as_ref())
                .map(|c| !c.strict.is_empty())
                .unwrap_or(false);

            // Collect which input params the precondition check needs
            let pre_input_refs: BTreeSet<String> = func_def
                .specification
                .as_ref()
                .and_then(|s| s.preconditions.as_ref())
                .map(|cs| {
                    cs.strict
                        .iter()
                        .flat_map(|e| {
                            let (inp, _, _) = collect_condition_refs(e);
                            inp
                        })
                        .collect()
                })
                .unwrap_or_default();
            let pre_needs_data = func_def
                .specification
                .as_ref()
                .and_then(|s| s.preconditions.as_ref())
                .map(|cs| {
                    cs.strict.iter().any(|e| {
                        let (_, dat, _) = collect_condition_refs(e);
                        !dat.is_empty()
                    })
                })
                .unwrap_or(false);

            // Same for postconditions
            let post_input_refs: BTreeSet<String> = func_def
                .specification
                .as_ref()
                .and_then(|s| s.postconditions.as_ref())
                .map(|cs| {
                    cs.strict
                        .iter()
                        .flat_map(|e| {
                            let (inp, _, _) = collect_condition_refs(e);
                            inp
                        })
                        .collect()
                })
                .unwrap_or_default();
            let post_uses_output = func_def
                .specification
                .as_ref()
                .and_then(|s| s.postconditions.as_ref())
                .map(|cs| {
                    cs.strict.iter().any(|e| {
                        let (_, _, out) = collect_condition_refs(e);
                        out
                    })
                })
                .unwrap_or(false);
            let post_needs_data = func_def
                .specification
                .as_ref()
                .and_then(|s| s.postconditions.as_ref())
                .map(|cs| {
                    cs.strict.iter().any(|e| {
                        let (_, dat, _) = collect_condition_refs(e);
                        !dat.is_empty()
                    })
                })
                .unwrap_or(false);

            let needs_data_param = pre_needs_data || post_needs_data;
            let mut pub_params = typed_params.clone();
            if needs_data_param {
                pub_params.push("data: &serde_json::Value".to_string());
            }

            let pub_return = format!("Result<{}, Box<dyn std::error::Error>>", base_return_type);

            let mut body = String::new();

            if has_pre {
                // Build pre-check call args: relevant input params in decl order + data
                // Non-Copy params (String, named-string types) are cloned to avoid move.
                let pre_args: Vec<String> = func_def
                    .inputs
                    .iter()
                    .filter(|(n, _)| pre_input_refs.contains(*n))
                    .map(|(n, def)| {
                        let var = to_snake_case(n);
                        if resolve_base_kind(&def.type_ref, types) == "string" {
                            format!("{var}.clone()")
                        } else {
                            var
                        }
                    })
                    .collect();
                let mut all_pre_args = pre_args;
                if pre_needs_data {
                    all_pre_args.push("data".to_string());
                } else {
                    all_pre_args.push("&serde_json::Value::Null".to_string());
                }
                body.push_str(&format!(
                    "    {snake_name}_check_preconditions({}).map_err(|e| Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e)) as Box<dyn std::error::Error>)?;\n",
                    all_pre_args.join(", ")
                ));
            }

            let impl_args: Vec<String> = func_def.inputs.keys().map(|p| to_snake_case(p)).collect();
            let impl_call = format!("{snake_name}_impl({})", impl_args.join(", "));
            if func_def.errors.is_some() {
                body.push_str(&format!("    let _result = {impl_call}?;\n"));
            } else if base_return_type == "()" {
                body.push_str(&format!("    {impl_call};\n"));
            } else {
                body.push_str(&format!("    let _result = {impl_call};\n"));
            }

            if has_post {
                let post_args: Vec<String> = func_def
                    .inputs
                    .iter()
                    .filter(|(n, _)| post_input_refs.contains(*n))
                    .map(|(n, def)| {
                        let var = to_snake_case(n);
                        if resolve_base_kind(&def.type_ref, types) == "string" {
                            format!("{var}.clone()")
                        } else {
                            var
                        }
                    })
                    .collect();
                let mut all_post_args = post_args;
                if post_uses_output {
                    all_post_args.push("_result".to_string());
                }
                if post_needs_data {
                    all_post_args.push("data".to_string());
                } else {
                    all_post_args.push("&serde_json::Value::Null".to_string());
                }
                body.push_str(&format!(
                    "    {snake_name}_check_postconditions({}).map_err(|e| Box::new(std::io::Error::new(std::io::ErrorKind::Other, e)) as Box<dyn std::error::Error>)?;\n",
                    all_post_args.join(", ")
                ));
            }

            if base_return_type == "()" {
                body.push_str("    Ok(())\n");
            } else {
                body.push_str("    Ok(_result)\n");
            }

            out.push_str(&format!(
                "pub fn {}({}) -> {} {{\n{}}}\n\n",
                snake_name,
                pub_params.join(", "),
                pub_return,
                body
            ));
        } else {
            // Simple stub — no strict conditions
            if let Some(perms) = &func_def.permissions {
                out.push_str("/// permissions:");
                if let Some(dp) = &perms.data {
                    if !dp.read.is_empty() {
                        out.push_str(&format!(" read=[{}]", dp.read.join(", ")));
                    }
                    if !dp.write.is_empty() {
                        out.push_str(&format!(" write=[{}]", dp.write.join(", ")));
                    }
                }
                out.push('\n');
            }
            out.push_str(&format!(
                "pub fn {}({}) -> {} {{\n    todo!()\n}}\n\n",
                snake_name,
                typed_params.join(", "),
                impl_return_type,
            ));
        }
    }

    out
}

/// Generates TypeScript function stubs from a contracts document.
///
/// For functions with strict pre/postconditions the output is split into four
/// pieces: a typed `CheckPreconditions` function, a typed
/// `CheckPostconditions` function, a private `Impl` stub, and a public
/// export that chains them: validate-pre → call-impl → validate-post.
///
/// Every check function takes explicit typed parameters.  If any parameter has
/// an inline object/array schema a TypeScript interface is generated first.
///
/// Functions without strict conditions emit a single simple stub.
pub fn generate_typescript_function_stubs(
    doc: &ContractsDoc,
    types: &BTreeMap<String, JsonValue>,
) -> String {
    if doc.functions.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    out.push_str("// --- Contracts stubs ---\n\n");

    // Emit any generated types for inline parameter schemas
    for (_, code) in collect_generated_types_ts(doc, types) {
        out.push_str(&code);
        out.push('\n');
    }

    for (func_name, func_def) in &doc.functions {
        let camel_name = to_camel_case(func_name);

        let typed_params: Vec<String> = func_def
            .inputs
            .iter()
            .map(|(param_name, param_def)| {
                let ts_type = schema_to_ts_type(&param_def.type_ref, types);
                format!("{}: {}", to_camel_case(param_name), ts_type)
            })
            .collect();

        let base_return_type = if let Some(output) = &func_def.output {
            schema_to_ts_type(output, types)
        } else {
            "void".to_string()
        };

        let impl_return_type = if let Some(output) = &func_def.output {
            let base = schema_to_ts_type(output, types);
            if func_def.errors.is_some() {
                format!("{} | Error", base)
            } else {
                base
            }
        } else {
            "void".to_string()
        };

        if has_any_strict_conditions(func_def) {
            // --- checkPreconditions ---
            if let Some(pre_fn) = build_preconditions_fn_ts(func_name, func_def, types) {
                out.push_str(&pre_fn);
                out.push('\n');
            }

            // --- checkPostconditions ---
            if let Some(post_fn) = build_postconditions_fn_ts(func_name, func_def, types) {
                out.push_str(&post_fn);
                out.push('\n');
            }

            // --- impl ---
            let impl_fn = format!("{camel_name}Impl");
            out.push_str(&format!(
                "function {}({}): {} {{\n  throw new Error(\'not implemented\');\n}}\n\n",
                impl_fn,
                typed_params.join(", "),
                impl_return_type,
            ));

            // --- public export with JSDoc ---
            out.push_str("/**\n");
            for (param_name, param_def) in &func_def.inputs {
                let ts_type = schema_to_ts_type(&param_def.type_ref, types);
                out.push_str(&format!(
                    " * @param {} - {}\n",
                    to_camel_case(param_name),
                    ts_type
                ));
            }
            let pre_needs_data = func_def
                .specification
                .as_ref()
                .and_then(|s| s.preconditions.as_ref())
                .map(|cs| {
                    cs.strict.iter().any(|e| {
                        let (_, dat, _) = collect_condition_refs(e);
                        !dat.is_empty()
                    })
                })
                .unwrap_or(false);
            let post_needs_data = func_def
                .specification
                .as_ref()
                .and_then(|s| s.postconditions.as_ref())
                .map(|cs| {
                    cs.strict.iter().any(|e| {
                        let (_, dat, _) = collect_condition_refs(e);
                        !dat.is_empty()
                    })
                })
                .unwrap_or(false);
            let needs_data_param = pre_needs_data || post_needs_data;
            if needs_data_param {
                out.push_str(" * @param data - runtime data snapshot for condition checks\n");
            }
            if let Some(output) = &func_def.output {
                let ts_type = schema_to_ts_type(output, types);
                out.push_str(&format!(" * @returns {}\n", ts_type));
            }
            if let Some(perms) = &func_def.permissions {
                if let Some(dp) = &perms.data {
                    if !dp.read.is_empty() || !dp.write.is_empty() {
                        out.push_str(" * @permissions");
                        if !dp.read.is_empty() {
                            out.push_str(&format!(" read=[{}]", dp.read.join(", ")));
                        }
                        if !dp.write.is_empty() {
                            out.push_str(&format!(" write=[{}]", dp.write.join(", ")));
                        }
                        out.push('\n');
                    }
                }
            }
            out.push_str(" */\n");

            let has_pre = func_def
                .specification
                .as_ref()
                .and_then(|s| s.preconditions.as_ref())
                .map(|c| !c.strict.is_empty())
                .unwrap_or(false);
            let has_post = func_def
                .specification
                .as_ref()
                .and_then(|s| s.postconditions.as_ref())
                .map(|c| !c.strict.is_empty())
                .unwrap_or(false);

            let pre_input_refs: BTreeSet<String> = func_def
                .specification
                .as_ref()
                .and_then(|s| s.preconditions.as_ref())
                .map(|cs| {
                    cs.strict
                        .iter()
                        .flat_map(|e| {
                            let (inp, _, _) = collect_condition_refs(e);
                            inp
                        })
                        .collect()
                })
                .unwrap_or_default();
            let pre_needs_data_val = func_def
                .specification
                .as_ref()
                .and_then(|s| s.preconditions.as_ref())
                .map(|cs| {
                    cs.strict.iter().any(|e| {
                        let (_, dat, _) = collect_condition_refs(e);
                        !dat.is_empty()
                    })
                })
                .unwrap_or(false);

            let post_input_refs: BTreeSet<String> = func_def
                .specification
                .as_ref()
                .and_then(|s| s.postconditions.as_ref())
                .map(|cs| {
                    cs.strict
                        .iter()
                        .flat_map(|e| {
                            let (inp, _, _) = collect_condition_refs(e);
                            inp
                        })
                        .collect()
                })
                .unwrap_or_default();
            let post_uses_output = func_def
                .specification
                .as_ref()
                .and_then(|s| s.postconditions.as_ref())
                .map(|cs| {
                    cs.strict.iter().any(|e| {
                        let (_, _, out) = collect_condition_refs(e);
                        out
                    })
                })
                .unwrap_or(false);
            let post_needs_data_val = func_def
                .specification
                .as_ref()
                .and_then(|s| s.postconditions.as_ref())
                .map(|cs| {
                    cs.strict.iter().any(|e| {
                        let (_, dat, _) = collect_condition_refs(e);
                        !dat.is_empty()
                    })
                })
                .unwrap_or(false);

            let mut pub_params = typed_params.clone();
            if needs_data_param {
                pub_params.push("data: Record<string, unknown>".to_string());
            }

            let mut body = String::new();

            if has_pre {
                let pre_args: Vec<String> = func_def
                    .inputs
                    .keys()
                    .filter(|n| pre_input_refs.contains(*n))
                    .map(|n| to_camel_case(n))
                    .collect();
                let mut all_pre_args = pre_args;
                if pre_needs_data_val {
                    all_pre_args.push("data".to_string());
                }
                out.push_str(&format!(
                    "export function {}({}): {} {{\n",
                    camel_name,
                    pub_params.join(", "),
                    base_return_type,
                ));
                body.push_str(&format!(
                    "  {camel_name}CheckPreconditions({});\n",
                    all_pre_args.join(", ")
                ));
            } else {
                out.push_str(&format!(
                    "export function {}({}): {} {{\n",
                    camel_name,
                    pub_params.join(", "),
                    base_return_type,
                ));
            }

            let impl_args: Vec<String> = func_def.inputs.keys().map(|p| to_camel_case(p)).collect();
            let impl_call = format!("{camel_name}Impl({})", impl_args.join(", "));
            if base_return_type == "void" {
                body.push_str(&format!("  {impl_call};\n"));
            } else {
                body.push_str(&format!("  const _result = {impl_call};\n"));
            }

            if has_post {
                let post_args: Vec<String> = func_def
                    .inputs
                    .keys()
                    .filter(|n| post_input_refs.contains(*n))
                    .map(|n| to_camel_case(n))
                    .collect();
                let mut all_post_args = post_args;
                if post_uses_output {
                    all_post_args.push("_result".to_string());
                }
                if post_needs_data_val {
                    all_post_args.push("data".to_string());
                }
                body.push_str(&format!(
                    "  {camel_name}CheckPostconditions({});\n",
                    all_post_args.join(", ")
                ));
            }

            if base_return_type != "void" {
                body.push_str("  return _result;\n");
            }

            out.push_str(&body);
            out.push_str("}\n\n");
        } else {
            // Simple stub — no strict conditions
            out.push_str("/**\n");
            for (param_name, param_def) in &func_def.inputs {
                let ts_type = schema_to_ts_type(&param_def.type_ref, types);
                out.push_str(&format!(
                    " * @param {} - {}\n",
                    to_camel_case(param_name),
                    ts_type
                ));
            }
            if let Some(output) = &func_def.output {
                let ts_type = schema_to_ts_type(output, types);
                out.push_str(&format!(" * @returns {}\n", ts_type));
            }
            if let Some(perms) = &func_def.permissions {
                if let Some(dp) = &perms.data {
                    if !dp.read.is_empty() || !dp.write.is_empty() {
                        out.push_str(" * @permissions");
                        if !dp.read.is_empty() {
                            out.push_str(&format!(" read=[{}]", dp.read.join(", ")));
                        }
                        if !dp.write.is_empty() {
                            out.push_str(&format!(" write=[{}]", dp.write.join(", ")));
                        }
                        out.push('\n');
                    }
                }
            }
            out.push_str(" */\n");

            out.push_str(&format!(
                "export function {}({}): {} {{\n  throw new Error(\'not implemented\');\n}}\n\n",
                camel_name,
                typed_params.join(", "),
                impl_return_type,
            ));
        }
    }

    out
}

fn schema_to_rust_type(schema: &JsonValue, _types: &BTreeMap<String, JsonValue>) -> String {
    if let Some(type_name) = schema.get("type").and_then(JsonValue::as_str) {
        return match type_name {
            "string" => "String".to_string(),
            "integer" => "i64".to_string(),
            "number" => "f64".to_string(),
            "boolean" => "bool".to_string(),
            "null" => "()".to_string(),
            "array" => "Vec<serde_json::Value>".to_string(),
            "object" => "std::collections::BTreeMap<String, serde_json::Value>".to_string(),
            other => {
                // Named type reference — convert to PascalCase
                to_pascal_case(other)
            }
        };
    }
    "serde_json::Value".to_string()
}

fn schema_to_ts_type(schema: &JsonValue, _types: &BTreeMap<String, JsonValue>) -> String {
    if let Some(type_name) = schema.get("type").and_then(JsonValue::as_str) {
        return match type_name {
            "string" => "string".to_string(),
            "integer" | "number" => "number".to_string(),
            "boolean" => "boolean".to_string(),
            "null" => "null".to_string(),
            "array" => "unknown[]".to_string(),
            "object" => "Record<string, unknown>".to_string(),
            other => to_pascal_case(other),
        };
    }
    "unknown".to_string()
}

fn to_snake_case(s: &str) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(ch.to_lowercase().next().unwrap());
    }
    out
}

fn to_pascal_case(s: &str) -> String {
    let mut out = String::new();
    let mut capitalize_next = true;
    for ch in s.chars() {
        if ch == '_' || ch == '-' {
            capitalize_next = true;
        } else if capitalize_next {
            out.extend(ch.to_uppercase());
            capitalize_next = false;
        } else {
            out.push(ch);
        }
    }
    out
}

fn to_camel_case(s: &str) -> String {
    let pascal = to_pascal_case(s);
    let mut chars = pascal.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),
    }
}
