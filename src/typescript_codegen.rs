//! TypeScript type generation from named schema definitions.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::ast::{ImportBinding, ParsedDocument};
use crate::schema::parse_field_version_meta;
use crate::{parse_document, SyamlError};

const TYPESCRIPT_KEYWORDS: &[&str] = &[
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "new",
    "null",
    "return",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "as",
    "implements",
    "interface",
    "let",
    "package",
    "private",
    "protected",
    "public",
    "static",
    "yield",
    "any",
    "boolean",
    "constructor",
    "declare",
    "get",
    "module",
    "require",
    "number",
    "set",
    "string",
    "symbol",
    "type",
    "from",
    "of",
    "readonly",
    "keyof",
    "namespace",
    "abstract",
    "never",
    "object",
    "unknown",
    "bigint",
    "override",
];

/// Generates TypeScript types from an in-memory `.syaml` document.
///
/// Imported schemas are resolved relative to the current working directory.
pub fn generate_typescript_types(input: &str) -> Result<String, SyamlError> {
    let cwd = std::env::current_dir()?;
    let parsed = parse_document(input)?;

    let mut ctx = TypeCollectionContext::new();
    let types = collect_exported_types(parsed, &cwd, &mut ctx)?;
    Ok(render_typescript_types(&types))
}

/// Generates TypeScript types from a `.syaml` file path.
///
/// Imported schemas are resolved relative to each file's parent directory.
pub fn generate_typescript_types_from_path(path: impl AsRef<Path>) -> Result<String, SyamlError> {
    let mut ctx = TypeCollectionContext::new();
    let types = collect_types_from_file(path.as_ref(), &mut ctx)?;
    Ok(render_typescript_types(&types))
}

#[derive(Default)]
struct TypeCollectionContext {
    cache: HashMap<PathBuf, CollectedSchemas>,
    stack: Vec<PathBuf>,
}

#[derive(Clone, Default)]
struct CollectedSchemas {
    types: BTreeMap<String, JsonValue>,
    type_constraints: BTreeMap<String, BTreeMap<String, Vec<String>>>,
}

impl TypeCollectionContext {
    fn new() -> Self {
        Self {
            cache: HashMap::new(),
            stack: Vec::new(),
        }
    }
}

fn collect_types_from_file(
    path: &Path,
    ctx: &mut TypeCollectionContext,
) -> Result<CollectedSchemas, SyamlError> {
    let canonical_path = canonicalize_path(path)?;

    if let Some(cached) = ctx.cache.get(&canonical_path) {
        return Ok(cached.clone());
    }

    if let Some(index) = ctx.stack.iter().position(|p| p == &canonical_path) {
        let mut chain: Vec<String> = ctx.stack[index..]
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        chain.push(canonical_path.display().to_string());
        return Err(SyamlError::ImportError(format!(
            "cyclic import detected: {}",
            chain.join(" -> ")
        )));
    }

    let input = fs::read_to_string(&canonical_path).map_err(|e| {
        SyamlError::ImportError(format!(
            "failed to read import '{}': {e}",
            canonical_path.display()
        ))
    })?;

    ctx.stack.push(canonical_path.clone());
    let base_dir = canonical_path.parent().ok_or_else(|| {
        SyamlError::ImportError(format!(
            "failed to resolve parent directory for '{}'",
            canonical_path.display()
        ))
    })?;

    let parsed = parse_document(&input)?;
    let exported = collect_exported_types(parsed, base_dir, ctx);

    ctx.stack.pop();

    let exported = exported?;
    ctx.cache.insert(canonical_path, exported.clone());
    Ok(exported)
}

fn collect_exported_types(
    parsed: ParsedDocument,
    base_dir: &Path,
    ctx: &mut TypeCollectionContext,
) -> Result<CollectedSchemas, SyamlError> {
    let mut exported = CollectedSchemas {
        types: parsed.schema.types,
        type_constraints: parsed.schema.type_constraints,
    };

    if let Some(meta) = parsed.meta {
        for (alias, binding) in meta.imports {
            let import_path = resolve_import_path(base_dir, &binding)?;
            let imported = collect_types_from_file(&import_path, ctx).map_err(|e| {
                SyamlError::ImportError(format!(
                    "failed to compile import '{}' for namespace '{}': {e}",
                    import_path.display(),
                    alias
                ))
            })?;
            insert_imported_types(&mut exported, &alias, &imported)?;
        }
    }

    Ok(exported)
}

fn resolve_import_path(base_dir: &Path, binding: &ImportBinding) -> Result<PathBuf, SyamlError> {
    let trimmed = binding.path.trim();
    if trimmed.is_empty() {
        return Err(SyamlError::ImportError(
            "import path must be a non-empty string".to_string(),
        ));
    }

    let path = Path::new(trimmed);
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    };

    canonicalize_path(&resolved)
}

fn canonicalize_path(path: &Path) -> Result<PathBuf, SyamlError> {
    fs::canonicalize(path).map_err(|e| {
        SyamlError::ImportError(format!(
            "failed to resolve import path '{}': {e}",
            path.display()
        ))
    })
}

fn insert_imported_types(
    registry: &mut CollectedSchemas,
    alias: &str,
    imported: &CollectedSchemas,
) -> Result<(), SyamlError> {
    for (type_name, schema) in &imported.types {
        let namespaced = format!("{alias}.{type_name}");
        if registry.types.contains_key(&namespaced) {
            return Err(SyamlError::ImportError(format!(
                "imported type '{}' conflicts with existing type",
                namespaced
            )));
        }
        registry.types.insert(namespaced, schema.clone());
    }

    for (type_name, constraints) in &imported.type_constraints {
        let namespaced = format!("{alias}.{type_name}");
        if registry.type_constraints.contains_key(&namespaced) {
            return Err(SyamlError::ImportError(format!(
                "imported constraints for type '{}' conflict with existing type",
                namespaced
            )));
        }
        registry
            .type_constraints
            .insert(namespaced, constraints.clone());
    }
    Ok(())
}

struct RenderState {
    type_names: BTreeMap<String, String>,
    type_constraints: BTreeMap<String, BTreeMap<String, Vec<String>>>,
    needs_constraint_runtime: bool,
}

fn render_typescript_types(schemas: &CollectedSchemas) -> String {
    if schemas.types.is_empty() {
        return concat!(
            "// Generated by super-yaml (first-pass TypeScript codegen).\n",
            "// WARNING: This file is generated. Do not edit manually.\n",
            "// No schema definitions found.\n"
        )
        .to_string();
    }

    let type_names = build_type_name_map(&schemas.types);
    let mut state = RenderState {
        type_names,
        type_constraints: schemas.type_constraints.clone(),
        needs_constraint_runtime: false,
    };

    let mut definitions = Vec::new();
    for (source_name, schema) in &schemas.types {
        definitions.push(render_type_definition(source_name, schema, &mut state));
    }

    let mut out = String::new();
    out.push_str("// Generated by super-yaml (first-pass TypeScript codegen).\n");
    out.push_str("// WARNING: This file is generated. Do not edit manually.\n");
    if state.needs_constraint_runtime {
        out.push('\n');
        out.push_str(&render_constraint_runtime_helpers());
        out.push('\n');
    }
    out.push('\n');
    out.push_str(&definitions.join("\n\n"));
    out.push('\n');
    out
}

fn build_type_name_map(types: &BTreeMap<String, JsonValue>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut used = HashSet::new();

    for source_name in types.keys() {
        let base = sanitize_type_name(source_name, "Type");
        let unique = unique_identifier(&base, &mut used);
        out.insert(source_name.clone(), unique);
    }

    out
}

fn render_type_definition(
    source_name: &str,
    schema: &JsonValue,
    state: &mut RenderState,
) -> String {
    let ts_name = state
        .type_names
        .get(source_name)
        .cloned()
        .unwrap_or_else(|| sanitize_type_name(source_name, "Type"));

    let Some(schema_obj) = schema.as_object() else {
        return format!("export type {ts_name} = unknown;");
    };

    let mut out = if is_union_schema(schema_obj) {
        render_union_alias(&ts_name, schema_obj, state)
    } else if let Some(variants) = collect_string_enum_variants(schema_obj) {
        render_string_enum_alias(&ts_name, &variants)
    } else if is_object_schema(schema_obj) {
        if let Some(properties) = schema_obj.get("properties").and_then(JsonValue::as_object) {
            render_object_interface(&ts_name, properties, schema_obj, state)
        } else if let Some(values_schema) = schema_obj.get("values") {
            let value_type = ts_type_for_schema(values_schema, state);
            format!("export type {ts_name} = Record<string, {value_type}>;")
        } else {
            format!("export type {ts_name} = unknown;")
        }
    } else {
        let ts_type = ts_type_for_schema(schema, state);
        format!("export type {ts_name} = {ts_type};")
    };

    if let Some(constraints) = state.type_constraints.get(source_name) {
        if !constraints.is_empty() {
            state.needs_constraint_runtime = true;
            out.push('\n');
            out.push('\n');
            out.push_str(&render_constraint_functions(&ts_name, constraints));
        }
    }

    out
}

fn render_constraint_functions(
    type_name: &str,
    constraints: &BTreeMap<String, Vec<String>>,
) -> String {
    let mut out = String::new();
    let mut index = 1usize;
    let mut method_names = Vec::new();

    for (path, expressions) in constraints {
        for expression in expressions {
            let fn_name = format!("check{type_name}Constraint{index}");
            let escaped_path = escape_string(path);
            let escaped_expr = escape_string(expression);
            out.push_str(&format!(
                "export function {fn_name}(value: {type_name}): string | null {{\n  return runGeneratedConstraintCheck(value, \"{escaped_path}\", \"{escaped_expr}\");\n}}\n\n"
            ));
            method_names.push(fn_name);
            index += 1;
        }
    }

    let all_fn_name = format!("check{type_name}Constraints");
    out.push_str(&format!(
        "export function {all_fn_name}(value: {type_name}): string[] {{\n  const errors: string[] = [];\n"
    ));
    for fn_name in &method_names {
        out.push_str(&format!(
            "  const err{} = {}(value);\n  if (err{}) errors.push(err{});\n",
            fn_name, fn_name, fn_name, fn_name
        ));
    }
    out.push_str("  return errors;\n}\n");
    out
}

fn render_constraint_runtime_helpers() -> String {
    concat!(
        "type GeneratedConstraintContext = Record<string, unknown>;\n",
        "\n",
        "function normalizeConstraintPath(path: string): string {\n",
        "  const trimmed = path.trim();\n",
        "  if (trimmed === \"$\") return \"$\";\n",
        "  if (trimmed.startsWith(\"$.\")) return trimmed;\n",
        "  return `$.${trimmed}`;\n",
        "}\n",
        "\n",
        "function parentConstraintPath(path: string): string | null {\n",
        "  if (path === \"$\") return null;\n",
        "  const idx = path.lastIndexOf('.');\n",
        "  return idx <= 1 ? \"$\" : path.slice(0, idx);\n",
        "}\n",
        "\n",
        "function getConstraintPath(root: unknown, path: string): unknown {\n",
        "  const normalized = normalizeConstraintPath(path);\n",
        "  if (normalized === \"$\") return root;\n",
        "  const segments = normalized.slice(2).split('.').filter(Boolean);\n",
        "  let current: unknown = root;\n",
        "  for (const segment of segments) {\n",
        "    if (current === null || typeof current !== \"object\") return undefined;\n",
        "    const obj = current as Record<string, unknown>;\n",
        "    if (!(segment in obj)) return undefined;\n",
        "    current = obj[segment];\n",
        "  }\n",
        "  return current;\n",
        "}\n",
        "\n",
        "function len(value: unknown): number {\n",
        "  if (typeof value === \"string\") return [...value].length;\n",
        "  if (Array.isArray(value)) return value.length;\n",
        "  if (value !== null && typeof value === \"object\") return Object.keys(value).length;\n",
        "  throw new Error(\"len() expects string, array, or object\");\n",
        "}\n",
        "\n",
        "function coalesce(...values: unknown[]): unknown {\n",
        "  for (const value of values) {\n",
        "    if (value !== null && value !== undefined) return value;\n",
        "  }\n",
        "  return null;\n",
        "}\n",
        "\n",
        "function createConstraintContext(\n",
        "  data: unknown,\n",
        "  currentScope: unknown,\n",
        "  currentValue: unknown,\n",
        "  env: GeneratedConstraintContext,\n",
        "): GeneratedConstraintContext {\n",
        "  return new Proxy({} as GeneratedConstraintContext, {\n",
        "    has: () => true,\n",
        "    get: (_target, prop) => {\n",
        "      const key = String(prop);\n",
        "      if (key === \"env\") return env;\n",
        "      if (key === \"value\") return currentValue;\n",
        "      if (key === \"min\") return Math.min;\n",
        "      if (key === \"max\") return Math.max;\n",
        "      if (key === \"abs\") return Math.abs;\n",
        "      if (key === \"floor\") return Math.floor;\n",
        "      if (key === \"ceil\") return Math.ceil;\n",
        "      if (key === \"round\") return Math.round;\n",
        "      if (key === \"len\") return len;\n",
        "      if (key === \"coalesce\") return coalesce;\n",
        "\n",
        "      const rootObject = data as Record<string, unknown> | null;\n",
        "      if (rootObject && typeof rootObject === \"object\" && key in rootObject) {\n",
        "        return rootObject[key];\n",
        "      }\n",
        "      const scopeObject = currentScope as Record<string, unknown> | null;\n",
        "      if (scopeObject && typeof scopeObject === \"object\" && key in scopeObject) {\n",
        "        return scopeObject[key];\n",
        "      }\n",
        "      const valueObject = currentValue as Record<string, unknown> | null;\n",
        "      if (valueObject && typeof valueObject === \"object\" && key in valueObject) {\n",
        "        return valueObject[key];\n",
        "      }\n",
        "      throw new Error(`unknown reference '${key}'`);\n",
        "    },\n",
        "  });\n",
        "}\n",
        "\n",
        "function evaluateGeneratedConstraint(\n",
        "  expression: string,\n",
        "  data: unknown,\n",
        "  currentScope: unknown,\n",
        "  currentValue: unknown,\n",
        "): unknown {\n",
        "  const source = expression.trim().replace(/^=/, \"\").trim();\n",
        "  const ctx = createConstraintContext(data, currentScope, currentValue, {});\n",
        "  const runner = new Function(\n",
        "    \"ctx\",\n",
        "    \"with (ctx) { return (\" + source + \"); }\",\n",
        "  ) as (ctx: GeneratedConstraintContext) => unknown;\n",
        "  return runner(ctx);\n",
        "}\n",
        "\n",
        "function runGeneratedConstraintCheck(\n",
        "  value: unknown,\n",
        "  path: string,\n",
        "  expression: string,\n",
        "): string | null {\n",
        "  const normalizedPath = normalizeConstraintPath(path);\n",
        "  const currentValue = getConstraintPath(value, normalizedPath);\n",
        "  if (typeof currentValue === \"undefined\") {\n",
        "    return `constraint path '${normalizedPath}' not found`;\n",
        "  }\n",
        "  const parentPath = parentConstraintPath(normalizedPath);\n",
        "  const currentScope = parentPath ? getConstraintPath(value, parentPath) : undefined;\n",
        "  try {\n",
        "    const result = evaluateGeneratedConstraint(expression, value, currentScope, currentValue);\n",
        "    if (result === true) return null;\n",
        "    return `constraint failed at '${normalizedPath}': '${expression}' evaluated to ${String(result)}`;\n",
        "  } catch (error) {\n",
        "    const reason = error instanceof Error ? error.message : String(error);\n",
        "    return `constraint evaluation error at '${normalizedPath}': ${reason}`;\n",
        "  }\n",
        "}\n",
    )
    .to_string()
}

fn is_union_schema(schema_obj: &JsonMap<String, JsonValue>) -> bool {
    schema_obj.get("type").and_then(JsonValue::as_str) == Some("union")
}

fn render_union_alias(
    name: &str,
    schema_obj: &JsonMap<String, JsonValue>,
    state: &mut RenderState,
) -> String {
    let option_types: Vec<String> = match schema_obj.get("options") {
        Some(JsonValue::Array(items)) => items
            .iter()
            .map(|opt_schema| ts_type_for_schema(opt_schema, state))
            .collect(),
        Some(JsonValue::Object(opt_map)) => opt_map
            .values()
            .map(|opt_schema| ts_type_for_schema(opt_schema, state))
            .collect(),
        _ => vec!["unknown".to_string()],
    };

    format!("export type {name} = {};", option_types.join(" | "))
}

fn collect_string_enum_variants(schema_obj: &JsonMap<String, JsonValue>) -> Option<Vec<String>> {
    let values = schema_obj.get("enum")?.as_array()?;
    if values.is_empty() {
        return None;
    }

    let mut out = Vec::with_capacity(values.len());
    for value in values {
        let text = value.as_str()?;
        out.push(text.to_string());
    }
    Some(out)
}

fn is_object_schema(schema_obj: &JsonMap<String, JsonValue>) -> bool {
    match schema_obj.get("type").and_then(JsonValue::as_str) {
        Some("object") => true,
        Some(_) => false,
        None => schema_obj.contains_key("properties") || schema_obj.contains_key("values"),
    }
}

fn render_string_enum_alias(name: &str, variants: &[String]) -> String {
    let mut members = Vec::with_capacity(variants.len());
    for variant in variants {
        members.push(format!("\"{}\"", escape_string(variant)));
    }
    format!("export type {name} = {};", members.join(" | "))
}

fn render_object_interface(
    name: &str,
    properties: &JsonMap<String, JsonValue>,
    schema_obj: &JsonMap<String, JsonValue>,
    state: &mut RenderState,
) -> String {
    let required = required_property_set(schema_obj, properties);
    let mut keys: Vec<&String> = properties.keys().collect();
    keys.sort_unstable();

    let mut out = String::new();
    out.push_str(&format!("export interface {name} {{\n"));

    for key in keys {
        let schema = properties
            .get(key)
            .expect("property key present while iterating keys");
        let optional = !required.contains(key.as_str());
        let ts_type = ts_type_for_schema(schema, state);
        let field_name = render_property_name(key);
        let optional_suffix = if optional { "?" } else { "" };

        // Emit versioning JSDoc block if needed.
        if let Ok(Some(meta)) = parse_field_version_meta(schema) {
            let has_fn = meta.field_number.is_some();
            let has_dep = meta.deprecated.is_some();
            if has_fn || has_dep {
                out.push_str("  /**\n");
                if let Some(fn_num) = meta.field_number {
                    out.push_str(&format!("   * Field number: {fn_num}\n"));
                }
                if let Some(dep) = meta.deprecated {
                    let dep_msg = match dep.message {
                        Some(msg) => format!("since {} - {}", dep.version, msg),
                        None => format!("since {}", dep.version),
                    };
                    out.push_str(&format!("   * @deprecated {dep_msg}\n"));
                }
                out.push_str("   */\n");
            }
        }

        out.push_str(&format!("  {field_name}{optional_suffix}: {ts_type};\n"));
    }

    out.push('}');
    out
}

fn required_property_set(
    schema_obj: &JsonMap<String, JsonValue>,
    properties: &JsonMap<String, JsonValue>,
) -> HashSet<String> {
    // Backward-compatible behavior: explicit `required` list takes precedence.
    if let Some(required) = schema_obj.get("required").and_then(JsonValue::as_array) {
        let mut out = HashSet::new();
        for name in required {
            if let Some(name) = name.as_str() {
                out.insert(name.to_string());
            }
        }
        return out;
    }

    // New default: all properties are required unless `optional: true`.
    let mut out = HashSet::new();
    for (name, property_schema) in properties {
        let optional = property_schema
            .as_object()
            .and_then(|obj| obj.get("optional"))
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        if !optional {
            out.insert(name.clone());
        }
    }

    out
}

fn ts_type_for_schema(schema: &JsonValue, state: &mut RenderState) -> String {
    let Some(schema_obj) = schema.as_object() else {
        return "unknown".to_string();
    };

    if let Some(type_name) = schema_obj.get("type").and_then(JsonValue::as_str) {
        return ts_type_for_type_name(type_name, schema_obj, state);
    }

    if schema_obj.get("enum").is_some() {
        if let Some(variants) = collect_string_enum_variants(schema_obj) {
            let members: Vec<String> = variants
                .iter()
                .map(|variant| format!("\"{}\"", escape_string(variant)))
                .collect();
            return members.join(" | ");
        }
        return "unknown".to_string();
    }

    if schema_obj.contains_key("properties") {
        return "unknown".to_string();
    }
    if let Some(values_schema) = schema_obj.get("values") {
        let value_type = ts_type_for_schema(values_schema, state);
        return format!("Record<string, {value_type}>");
    }

    "unknown".to_string()
}

fn ts_type_for_type_name(
    type_name: &str,
    schema_obj: &JsonMap<String, JsonValue>,
    state: &mut RenderState,
) -> String {
    match type_name {
        "string" => "string".to_string(),
        "integer" => "number".to_string(),
        "number" => "number".to_string(),
        "boolean" => "boolean".to_string(),
        "null" => "null".to_string(),
        "array" => {
            if let Some(items) = schema_obj.get("items") {
                let item_type = ts_type_for_schema(items, state);
                format!("Array<{item_type}>")
            } else {
                "Array<unknown>".to_string()
            }
        }
        "object" => {
            if let Some(values_schema) = schema_obj.get("values") {
                let value_type = ts_type_for_schema(values_schema, state);
                format!("Record<string, {value_type}>")
            } else {
                "unknown".to_string()
            }
        }
        "union" => "unknown".to_string(),
        other => state
            .type_names
            .get(other)
            .cloned()
            .unwrap_or_else(|| "unknown".to_string()),
    }
}

fn sanitize_type_name(raw: &str, fallback: &str) -> String {
    let mut out = to_pascal_case(raw);
    if out.is_empty() {
        out = fallback.to_string();
    }

    if !starts_with_ident_char(&out) {
        out = format!("Type{out}");
    }

    if is_typescript_keyword(&out) {
        out.push_str("Type");
    }

    out
}

fn render_property_name(raw: &str) -> String {
    if is_valid_ts_identifier(raw) && !is_typescript_keyword(raw) {
        raw.to_string()
    } else {
        format!("\"{}\"", escape_string(raw))
    }
}

fn to_pascal_case(raw: &str) -> String {
    let mut out = String::new();
    for token in identifier_tokens(raw) {
        let mut chars = token.chars();
        if let Some(first) = chars.next() {
            out.push(first.to_ascii_uppercase());
            for ch in chars {
                out.push(ch.to_ascii_lowercase());
            }
        }
    }
    out
}

fn identifier_tokens(raw: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for chunk in raw.split(|c: char| !c.is_ascii_alphanumeric()) {
        if chunk.is_empty() {
            continue;
        }
        tokens.extend(split_camel_tokens(chunk));
    }
    tokens
}

fn split_camel_tokens(chunk: &str) -> Vec<String> {
    let chars: Vec<char> = chunk.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }

    let mut tokens = Vec::new();
    let mut start = 0usize;

    for i in 1..chars.len() {
        let prev = chars[i - 1];
        let curr = chars[i];
        let next = chars.get(i + 1).copied();

        let boundary = (prev.is_ascii_lowercase() && curr.is_ascii_uppercase())
            || (prev.is_ascii_alphabetic() && curr.is_ascii_digit())
            || (prev.is_ascii_digit() && curr.is_ascii_alphabetic())
            || (prev.is_ascii_uppercase()
                && curr.is_ascii_uppercase()
                && next.map(|n| n.is_ascii_lowercase()).unwrap_or(false));

        if boundary {
            let token: String = chars[start..i].iter().collect();
            if !token.is_empty() {
                tokens.push(token.to_ascii_lowercase());
            }
            start = i;
        }
    }

    let token: String = chars[start..].iter().collect();
    if !token.is_empty() {
        tokens.push(token.to_ascii_lowercase());
    }

    tokens
}

fn unique_identifier(base: &str, used: &mut HashSet<String>) -> String {
    if used.insert(base.to_string()) {
        return base.to_string();
    }

    let mut idx = 2usize;
    loop {
        let candidate = format!("{base}{idx}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        idx += 1;
    }
}

fn is_valid_ts_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    if !(first == '_' || first == '$' || first.is_ascii_alphabetic()) {
        return false;
    }

    chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
}

fn starts_with_ident_char(text: &str) -> bool {
    text.chars()
        .next()
        .map(|c| c == '_' || c == '$' || c.is_ascii_alphabetic())
        .unwrap_or(false)
}

fn is_typescript_keyword(text: &str) -> bool {
    TYPESCRIPT_KEYWORDS.iter().any(|kw| kw == &text)
}

fn escape_string(raw: &str) -> String {
    raw.replace('\\', "\\\\").replace('"', "\\\"")
}

// ─── Data constants generation ────────────────────────────────────────────────

/// Generates TypeScript types AND data constants from a `.syaml` file path.
///
/// The compiled (resolved) `---data` section is emitted as `export const` statements
/// after the type definitions, separated by a `// --- Data ---` comment.
pub fn generate_typescript_types_and_data_from_path(
    path: impl AsRef<Path>,
    env_provider: &dyn crate::EnvProvider,
) -> Result<String, SyamlError> {
    let path = path.as_ref();

    // Parse to get type hints (path → schema type name).
    let input = fs::read_to_string(path).map_err(|e| {
        SyamlError::ImportError(format!("failed to read '{}': {e}", path.display()))
    })?;
    let parsed = parse_document(&input)?;
    let type_hints = parsed.data.type_hints.clone();

    // Collect schema types for type rendering and name resolution.
    let mut ctx = TypeCollectionContext::new();
    let schemas = collect_types_from_file(path, &mut ctx)?;
    let type_names = build_type_name_map(&schemas.types);

    // Compile to get resolved data values.
    let compiled =
        crate::compile_document_from_path_with_fetch(path, env_provider, None, false)?;

    // Render types (existing logic).
    let types_output = render_typescript_types(&schemas);

    // Render data items.
    let data_output = render_typescript_data(&compiled.value, &type_hints, &type_names);

    if data_output.is_empty() {
        return Ok(types_output);
    }

    Ok(format!("{types_output}\n// --- Data ---\n\n{data_output}"))
}

fn render_typescript_data(
    data: &JsonValue,
    type_hints: &BTreeMap<String, String>,
    type_names: &BTreeMap<String, String>,
) -> String {
    let Some(obj) = data.as_object() else {
        return String::new();
    };
    if obj.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    for (key, value) in obj {
        let hint_path = format!("$.{key}");
        let type_hint = type_hints.get(&hint_path).map(String::as_str);
        let ts_type = data_ts_type(type_hint, value, type_names);
        let var_name = to_camel_case_ts(key);
        // JSON is valid TypeScript literal syntax.
        let json_str = serde_json::to_string_pretty(value)
            .unwrap_or_else(|_| "null".to_string());
        out.push_str(&format!(
            "export const {var_name}: {ts_type} = {json_str};\n\n"
        ));
    }
    out
}

fn data_ts_type(
    type_hint: Option<&str>,
    value: &JsonValue,
    type_names: &BTreeMap<String, String>,
) -> String {
    if let Some(hint) = type_hint {
        match hint {
            "string" => return "string".to_string(),
            "integer" | "number" => return "number".to_string(),
            "boolean" => return "boolean".to_string(),
            other => {
                if let Some(mapped) = type_names.get(other) {
                    return mapped.clone();
                }
            }
        }
    }
    // Infer from value when no type hint is available.
    match value {
        JsonValue::Bool(_) => "boolean".to_string(),
        JsonValue::Number(_) => "number".to_string(),
        JsonValue::String(_) => "string".to_string(),
        JsonValue::Array(_) => "unknown[]".to_string(),
        JsonValue::Object(_) => "Record<string, unknown>".to_string(),
        JsonValue::Null => "null".to_string(),
    }
}

/// Converts a raw key to camelCase for use as a TypeScript identifier.
fn to_camel_case_ts(raw: &str) -> String {
    let tokens = identifier_tokens(raw);
    if tokens.is_empty() {
        return "value".to_string();
    }
    let mut out = String::new();
    for (i, token) in tokens.iter().enumerate() {
        if i == 0 {
            out.push_str(token);
        } else {
            let mut chars = token.chars();
            if let Some(first) = chars.next() {
                out.push(first.to_ascii_uppercase());
                for ch in chars {
                    out.push(ch);
                }
            }
        }
    }
    if out.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
        out = format!("var{out}");
    }
    if is_typescript_keyword(&out) {
        out.push('_');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{generate_typescript_types, render_typescript_types, CollectedSchemas};
    use serde_json::json;

    #[test]
    fn renders_interfaces_aliases_and_string_unions() {
        let input = r#"
---!syaml/v0
---schema
MessageKind:
  enum: [join, leave]
WsMessage:
  type: object
  properties:
    kind:
      type: MessageKind
    room_id:
      type: string
    payload:
      type: object
      optional: true
Batch:
  type: array
  items:
    type: WsMessage
---data
example: 1
"#;

        let rendered = generate_typescript_types(input).unwrap();

        assert!(rendered.contains("export type MessageKind = \"join\" | \"leave\";"));
        assert!(rendered.contains("export interface WsMessage"));
        assert!(rendered.contains("room_id: string;"));
        assert!(rendered.contains("payload?: unknown;"));
        assert!(rendered.contains("export type Batch = Array<WsMessage>;"));
    }

    #[test]
    fn explicit_required_list_controls_requiredness() {
        let input = r#"
---!syaml/v0
---schema
Config:
  type: object
  required: [must]
  properties:
    must:
      type: string
      optional: true
    maybe:
      type: string
---data
x: 1
"#;

        let rendered = generate_typescript_types(input).unwrap();
        assert!(rendered.contains("must: string;"));
        assert!(rendered.contains("maybe?: string;"));
    }

    #[test]
    fn falls_back_to_unknown_for_mixed_enum_and_ambiguous_object() {
        let mut schemas = CollectedSchemas::default();
        schemas
            .types
            .insert("Mixed".to_string(), json!({"enum": ["a", 1]}));
        schemas
            .types
            .insert("LooseObject".to_string(), json!({"type": "object"}));

        let rendered = render_typescript_types(&schemas);
        assert!(rendered.contains("export type Mixed = unknown;"));
        assert!(rendered.contains("export type LooseObject = unknown;"));
    }

    #[test]
    fn sanitizes_identifiers_for_types_and_properties() {
        let input = r#"
---!syaml/v0
---schema
shared.http-server:
  type: object
  properties:
    kebab-case:
      type: string
    class:
      type: string
---data
x: 1
"#;

        let rendered = generate_typescript_types(input).unwrap();
        assert!(rendered.contains("export interface SharedHttpServer"));
        assert!(rendered.contains("\"kebab-case\": string;"));
        assert!(rendered.contains("\"class\": string;"));
    }

    #[test]
    fn renders_empty_type_set() {
        let rendered = render_typescript_types(&CollectedSchemas::default());
        assert!(rendered.contains("No schema definitions found"));
    }
}
