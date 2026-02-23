//! HTML documentation generator for `.syaml` files.
//!
//! Produces a self-contained HTML page documenting the schema types, data
//! entries, and functional definitions found in a SYAML document.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value as JsonValue;

use crate::ast::{FunctionalDoc, Meta, SchemaDoc};
use crate::{parse_document_or_manifest, SyamlError};

// ─── Public API ──────────────────────────────────────────────────────────────

/// Generates an HTML documentation page from an in-memory `.syaml` string.
///
/// No cross-file import links are produced; all type references within the
/// document are rendered as internal anchor links.
pub fn generate_html_docs(input: &str) -> Result<String, SyamlError> {
    let parsed = parse_document_or_manifest(input)?;
    let file_title = "SYAML Documentation";
    Ok(assemble_page(
        file_title,
        parsed.meta.as_ref(),
        &parsed.schema,
        parsed.functional.as_ref(),
        Some(&parsed.data.value),
    ))
}

/// Generates an HTML documentation page from a `.syaml` file path.
///
/// The file stem is used as the page title. Import paths in `meta.imports` are
/// used to generate relative cross-file `href` links to sibling `.html` files.
pub fn generate_html_docs_from_path(path: impl AsRef<Path>) -> Result<String, SyamlError> {
    let path = path.as_ref();
    let input = fs::read_to_string(path).map_err(|e| {
        SyamlError::ImportError(format!("failed to read '{}': {e}", path.display()))
    })?;
    let parsed = parse_document_or_manifest(&input)?;
    let file_title = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("SYAML Documentation");
    let base_dir = path.parent().unwrap_or(Path::new("."));
    Ok(assemble_page_with_paths(
        file_title,
        parsed.meta.as_ref(),
        &parsed.schema,
        parsed.functional.as_ref(),
        Some(&parsed.data.value),
        base_dir,
    ))
}

// ─── Page assembly ───────────────────────────────────────────────────────────

fn assemble_page(
    title: &str,
    meta: Option<&Meta>,
    schema: &SchemaDoc,
    functional: Option<&FunctionalDoc>,
    data: Option<&JsonValue>,
) -> String {
    assemble_page_with_paths(title, meta, schema, functional, data, Path::new("."))
}

fn assemble_page_with_paths(
    title: &str,
    meta: Option<&Meta>,
    schema: &SchemaDoc,
    functional: Option<&FunctionalDoc>,
    data: Option<&JsonValue>,
    _base_dir: &Path,
) -> String {
    // Build import alias → relative html path map for cross-linking
    let import_html_paths: BTreeMap<String, String> = meta
        .map(|m| {
            m.imports
                .iter()
                .filter_map(|(alias, binding)| {
                    let raw = &binding.path;
                    if raw.starts_with("http://") || raw.starts_with("https://") || raw.starts_with('@') {
                        return None;
                    }
                    let html_path = Path::new(raw)
                        .with_extension("html")
                        .to_string_lossy()
                        .into_owned();
                    Some((alias.clone(), html_path))
                })
                .collect()
        })
        .unwrap_or_default();

    let meta_html = meta
        .map(|m| render_meta_section(m, &import_html_paths))
        .unwrap_or_default();
    let schema_html = render_schema_section(schema, &import_html_paths);
    let functional_html = functional
        .map(|f| render_functional_section(f, &import_html_paths))
        .unwrap_or_default();

    // Only render data section if the value is non-null and non-empty
    let data_html = data
        .filter(|v| !is_empty_data(v))
        .map(render_data_section)
        .unwrap_or_default();

    let nav_items = build_nav_items(schema, functional, data.filter(|v| !is_empty_data(v)));

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>{title}</title>
<style>
{css}
</style>
</head>
<body>
<nav id="sidebar">
  <div class="nav-title">{title}</div>
  <ul>
{nav_items}
  </ul>
</nav>
<main>
  <h1 class="page-title">{title}</h1>
{meta_html}
{schema_html}
{functional_html}
{data_html}
</main>
<script>
function toggleData() {{
  var el = document.getElementById('data-content');
  var btn = document.getElementById('data-toggle');
  if (el.style.display === 'none') {{
    el.style.display = 'block';
    btn.textContent = 'Hide data';
  }} else {{
    el.style.display = 'none';
    btn.textContent = 'Show data';
  }}
}}
</script>
</body>
</html>
"#,
        title = html_escape(title),
        css = inline_css(),
        nav_items = nav_items,
        meta_html = meta_html,
        schema_html = schema_html,
        functional_html = functional_html,
        data_html = data_html,
    )
}

// ─── Nav sidebar ─────────────────────────────────────────────────────────────

fn build_nav_items(schema: &SchemaDoc, functional: Option<&FunctionalDoc>, data: Option<&JsonValue>) -> String {
    let mut items = String::new();

    if !schema.types.is_empty() {
        items.push_str(r#"    <li class="nav-section">Schema</li>"#);
        items.push('\n');
        for name in schema.types.keys() {
            items.push_str(&format!(
                "    <li><a href=\"#type-{id}\">{name}</a></li>\n",
                id = html_escape(name),
                name = html_escape(name),
            ));
        }
    }

    if let Some(func) = functional {
        if !func.functions.is_empty() {
            items.push_str(r#"    <li class="nav-section">Functions</li>"#);
            items.push('\n');
            for name in func.functions.keys() {
                items.push_str(&format!(
                    "    <li><a href=\"#fn-{id}\">{name}</a></li>\n",
                    id = html_escape(name),
                    name = html_escape(name),
                ));
            }
        }
    }

    if data.is_some() {
        items.push_str(r#"    <li class="nav-section">Data</li>"#);
        items.push('\n');
        items.push_str("    <li><a href=\"#data-section\">Data</a></li>\n");
    }

    items
}

// ─── Meta section ────────────────────────────────────────────────────────────

fn render_meta_section(meta: &Meta, import_html_paths: &BTreeMap<String, String>) -> String {
    let mut html = String::new();
    html.push_str("<section class=\"doc-section\">\n<h2>Meta</h2>\n");

    // File metadata
    if !meta.file.is_empty() {
        html.push_str("<h3>File Metadata</h3>\n<table class=\"prop-table\">\n");
        html.push_str("<thead><tr><th>Key</th><th>Value</th></tr></thead>\n<tbody>\n");
        for (key, val) in &meta.file {
            html.push_str(&format!(
                "<tr><td><code>{}</code></td><td><code>{}</code></td></tr>\n",
                html_escape(key),
                html_escape(&json_value_display(val)),
            ));
        }
        html.push_str("</tbody></table>\n");
    }

    // Imports
    if !meta.imports.is_empty() {
        html.push_str("<h3>Imports</h3>\n<table class=\"prop-table\">\n");
        html.push_str("<thead><tr><th>Alias</th><th>Path</th><th>Sections</th></tr></thead>\n<tbody>\n");
        for (alias, binding) in &meta.imports {
            let path_cell = if let Some(html_path) = import_html_paths.get(alias) {
                format!("<a href=\"{}\">{}</a>", html_escape(html_path), html_escape(&binding.path))
            } else {
                html_escape(&binding.path)
            };
            let sections = binding
                .sections
                .as_deref()
                .map(|s| s.join(", "))
                .unwrap_or_else(|| "all".to_string());
            html.push_str(&format!(
                "<tr><td><code>{}</code></td><td>{}</td><td>{}</td></tr>\n",
                html_escape(alias),
                path_cell,
                html_escape(&sections),
            ));
        }
        html.push_str("</tbody></table>\n");
    }

    // Env bindings
    if !meta.env.is_empty() {
        html.push_str("<h3>Environment Bindings</h3>\n<table class=\"prop-table\">\n");
        html.push_str("<thead><tr><th>Symbol</th><th>Key</th><th>Required</th><th>Default</th></tr></thead>\n<tbody>\n");
        for (symbol, binding) in &meta.env {
            let default_val = binding
                .default
                .as_ref()
                .map(|v| html_escape(&json_value_display(v)))
                .unwrap_or_else(|| "—".to_string());
            html.push_str(&format!(
                "<tr><td><code>{}</code></td><td><code>{}</code></td><td>{}</td><td>{}</td></tr>\n",
                html_escape(symbol),
                html_escape(&binding.key),
                if binding.required { "yes" } else { "no" },
                default_val,
            ));
        }
        html.push_str("</tbody></table>\n");
    }

    html.push_str("</section>\n");
    html
}

// ─── Data section ─────────────────────────────────────────────────────────────

/// Returns true when the data value is effectively empty (null or empty object).
fn is_empty_data(val: &JsonValue) -> bool {
    match val {
        JsonValue::Null => true,
        JsonValue::Object(map) => map.is_empty(),
        _ => false,
    }
}

/// Renders the `---data` section with a show/hide toggle button.
fn render_data_section(data: &JsonValue) -> String {
    let pretty = serde_json::to_string_pretty(data).unwrap_or_else(|_| "{}".to_string());
    format!(
        r#"<section class="doc-section" id="data-section">
<h2>Data</h2>
<button class="data-toggle-btn" onclick="toggleData()" id="data-toggle">Show data</button>
<div id="data-content" style="display:none">
<pre><code>{code}</code></pre>
</div>
</section>
"#,
        code = html_escape(&pretty),
    )
}

// ─── Schema section ──────────────────────────────────────────────────────────

fn render_schema_section(schema: &SchemaDoc, import_html_paths: &BTreeMap<String, String>) -> String {
    if schema.types.is_empty() {
        return String::new();
    }

    let mut html = String::new();
    html.push_str("<section class=\"doc-section\">\n<h2>Schema</h2>\n");

    for (name, type_val) in &schema.types {
        let extends_parent = schema.extends.get(name);
        let constraints = schema.type_constraints.get(name);
        html.push_str(&render_type_card(
            name,
            type_val,
            extends_parent.map(|s| s.as_str()),
            constraints,
            import_html_paths,
        ));
    }

    html.push_str("</section>\n");
    html
}

fn render_type_card(
    name: &str,
    type_val: &JsonValue,
    extends_parent: Option<&str>,
    constraints: Option<&BTreeMap<String, Vec<String>>>,
    import_html_paths: &BTreeMap<String, String>,
) -> String {
    let mut html = String::new();
    html.push_str(&format!(
        "<div class=\"type-card\" id=\"type-{id}\">\n",
        id = html_escape(name)
    ));

    // Card header
    html.push_str("<div class=\"type-header\">\n");
    html.push_str(&format!(
        "<span class=\"type-name\">{}</span>\n",
        html_escape(name)
    ));
    if let Some(parent) = extends_parent {
        html.push_str(&format!(
            " <span class=\"extends-badge\">extends {}</span>\n",
            render_type_ref(parent, import_html_paths)
        ));
    }
    // Show the kind badge
    let kind = detect_type_kind(type_val);
    html.push_str(&format!("<span class=\"kind-badge\">{kind}</span>\n"));
    html.push_str("</div>\n");

    // Description if present
    if let Some(desc) = type_val.get("description").and_then(|v| v.as_str()) {
        html.push_str(&format!("<p class=\"type-desc\">{}</p>\n", html_escape(desc)));
    }

    // Render body based on kind
    match kind {
        "object" => html.push_str(&render_object_type(type_val, import_html_paths)),
        "enum" => html.push_str(&render_enum_type(type_val)),
        "array" => html.push_str(&render_array_type(type_val, import_html_paths)),
        "union" => html.push_str(&render_union_type(type_val, import_html_paths)),
        _ => html.push_str(&render_scalar_type(type_val)),
    }

    // Constraints
    if let Some(c) = constraints {
        html.push_str(&render_constraints(c));
    } else if let Some(c_val) = type_val.get("constraints") {
        html.push_str(&render_inline_constraints(c_val));
    }

    html.push_str("</div>\n");
    html
}

fn detect_type_kind(val: &JsonValue) -> &'static str {
    if val.get("enum").is_some() {
        return "enum";
    }
    let type_str = val.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match type_str {
        "object" => "object",
        "array" => "array",
        "union" => "union",
        "integer" => "integer",
        "number" => "number",
        "boolean" => "boolean",
        "string" => "string",
        "null" => "null",
        _ => {
            // If it has a `properties` key it's likely an object even without explicit type
            if val.get("properties").is_some() {
                "object"
            } else {
                "type"
            }
        }
    }
}

fn render_object_type(val: &JsonValue, import_html_paths: &BTreeMap<String, String>) -> String {
    let Some(props) = val.get("properties").and_then(|p| p.as_object()) else {
        return String::new();
    };

    let required_list: Vec<&str> = val
        .get("required")
        .and_then(|r| r.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    let mut html = String::from("<table class=\"prop-table\">\n");
    html.push_str("<thead><tr><th>Field</th><th>Type</th><th>Required</th><th>Notes</th></tr></thead>\n<tbody>\n");

    for (field_name, field_val) in props {
        let type_display = render_type_ref_from_value(field_val, import_html_paths);
        let required = required_list.contains(&field_name.as_str());
        let notes = collect_field_notes(field_val);

        html.push_str(&format!(
            "<tr><td><code>{field}</code>{dep}</td><td>{type_display}</td><td>{req}</td><td>{notes}</td></tr>\n",
            field = html_escape(field_name),
            dep = if is_deprecated(field_val) { " <span class=\"deprecated-badge\">deprecated</span>" } else { "" },
            type_display = type_display,
            req = if required { "✓" } else { "" },
            notes = notes,
        ));
    }

    html.push_str("</tbody></table>\n");
    html
}

fn collect_field_notes(val: &JsonValue) -> String {
    let mut notes = Vec::new();
    if let Some(desc) = val.get("description").and_then(|v| v.as_str()) {
        notes.push(html_escape(desc));
    }
    if let Some(since) = val.get("since").and_then(|v| v.as_str()) {
        notes.push(format!("since {}", html_escape(since)));
    }
    if let Some(fn_num) = val.get("field_number") {
        notes.push(format!("field #{}", html_escape(&json_value_display(fn_num))));
    }
    notes.join("; ")
}

fn is_deprecated(val: &JsonValue) -> bool {
    val.get("deprecated")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

fn render_enum_type(val: &JsonValue) -> String {
    let Some(variants) = val.get("enum").and_then(|e| e.as_array()) else {
        return String::new();
    };
    let mut html = String::from("<ul class=\"enum-list\">\n");
    for v in variants {
        html.push_str(&format!(
            "<li><code>{}</code></li>\n",
            html_escape(&json_value_display(v))
        ));
    }
    html.push_str("</ul>\n");
    html
}

fn render_array_type(val: &JsonValue, import_html_paths: &BTreeMap<String, String>) -> String {
    let mut html = String::new();
    if let Some(items) = val.get("items") {
        let item_type = render_type_ref_from_value(items, import_html_paths);
        html.push_str(&format!("<p>Items: {item_type}</p>\n"));
    }
    if let Some(min) = val.get("minItems") {
        html.push_str(&format!("<p>Min items: <code>{}</code></p>\n", html_escape(&json_value_display(min))));
    }
    if let Some(max) = val.get("maxItems") {
        html.push_str(&format!("<p>Max items: <code>{}</code></p>\n", html_escape(&json_value_display(max))));
    }
    html
}

fn render_union_type(val: &JsonValue, import_html_paths: &BTreeMap<String, String>) -> String {
    let options = val
        .get("options")
        .or_else(|| val.get("one_of"))
        .or_else(|| val.get("anyOf"))
        .and_then(|o| o.as_array());

    let Some(opts) = options else {
        return String::new();
    };

    let mut html = String::from("<ul class=\"union-list\">\n");
    for opt in opts {
        let t = render_type_ref_from_value(opt, import_html_paths);
        html.push_str(&format!("<li>{t}</li>\n"));
    }
    html.push_str("</ul>\n");
    html
}

fn render_scalar_type(val: &JsonValue) -> String {
    let mut parts = Vec::new();
    for key in &["minimum", "maximum", "exclusiveMinimum", "exclusiveMaximum", "minLength", "maxLength", "pattern"] {
        if let Some(v) = val.get(*key) {
            parts.push(format!(
                "<code>{}: {}</code>",
                html_escape(key),
                html_escape(&json_value_display(v))
            ));
        }
    }
    if parts.is_empty() {
        return String::new();
    }
    format!("<p class=\"scalar-constraints\">{}</p>\n", parts.join(" "))
}

fn render_constraints(constraints: &BTreeMap<String, Vec<String>>) -> String {
    let mut html = String::from("<div class=\"constraints\">\n<strong>Constraints:</strong>\n<ul>\n");
    for (path, exprs) in constraints {
        for expr in exprs {
            let label = if path == "$" || path.is_empty() {
                String::new()
            } else {
                format!("<code class=\"constraint-path\">{}</code>: ", html_escape(path))
            };
            html.push_str(&format!(
                "<li>{label}<code>{}</code></li>\n",
                html_escape(expr)
            ));
        }
    }
    html.push_str("</ul>\n</div>\n");
    html
}

fn render_inline_constraints(val: &JsonValue) -> String {
    let exprs: Vec<String> = match val {
        JsonValue::String(s) => vec![s.clone()],
        JsonValue::Array(arr) => arr.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
        JsonValue::Object(map) => {
            let mut all = Vec::new();
            for (path, v) in map {
                match v {
                    JsonValue::String(s) => all.push(format!("{path}: {s}")),
                    JsonValue::Array(arr) => {
                        for expr in arr {
                            if let Some(s) = expr.as_str() {
                                all.push(format!("{path}: {s}"));
                            }
                        }
                    }
                    _ => {}
                }
            }
            all
        }
        _ => return String::new(),
    };

    if exprs.is_empty() {
        return String::new();
    }

    let mut html = String::from("<div class=\"constraints\">\n<strong>Constraints:</strong>\n<ul>\n");
    for expr in &exprs {
        html.push_str(&format!("<li><code>{}</code></li>\n", html_escape(expr)));
    }
    html.push_str("</ul>\n</div>\n");
    html
}

// ─── Functional section ───────────────────────────────────────────────────────

fn render_functional_section(func: &FunctionalDoc, import_html_paths: &BTreeMap<String, String>) -> String {
    if func.functions.is_empty() {
        return String::new();
    }

    let mut html = String::new();
    html.push_str("<section class=\"doc-section\">\n<h2>Functions</h2>\n");

    for (name, def) in &func.functions {
        html.push_str(&format!(
            "<div class=\"type-card\" id=\"fn-{id}\">\n",
            id = html_escape(name)
        ));
        html.push_str("<div class=\"type-header\">\n");
        html.push_str(&format!("<span class=\"type-name\">{}</span>\n", html_escape(name)));
        html.push_str("<span class=\"kind-badge\">fn</span>\n");
        html.push_str("</div>\n");

        // Inputs table
        if !def.inputs.is_empty() {
            html.push_str("<h4>Inputs</h4>\n<table class=\"prop-table\">\n");
            html.push_str("<thead><tr><th>Parameter</th><th>Type</th><th>Mutable</th></tr></thead>\n<tbody>\n");
            for (param, pdef) in &def.inputs {
                let type_display = render_type_ref_from_value(&pdef.type_ref, import_html_paths);
                html.push_str(&format!(
                    "<tr><td><code>{}</code></td><td>{}</td><td>{}</td></tr>\n",
                    html_escape(param),
                    type_display,
                    if pdef.mutable { "✓" } else { "" },
                ));
            }
            html.push_str("</tbody></table>\n");
        }

        // Output
        if let Some(output) = &def.output {
            let out_type = render_type_ref_from_value(output, import_html_paths);
            html.push_str(&format!("<p><strong>Returns:</strong> {out_type}</p>\n"));
        }

        // Errors
        if let Some(errors) = &def.errors {
            let err_type = render_type_ref_from_value(errors, import_html_paths);
            html.push_str(&format!("<p><strong>Errors:</strong> {err_type}</p>\n"));
        }

        html.push_str("</div>\n");
    }

    html.push_str("</section>\n");
    html
}

// ─── Type reference helpers ──────────────────────────────────────────────────

/// Render a type name as a local anchor link or cross-file link.
fn render_type_ref(type_name: &str, import_html_paths: &BTreeMap<String, String>) -> String {
    // Check if it's an aliased import reference like "alias.TypeName"
    if let Some(dot_pos) = type_name.find('.') {
        let alias = &type_name[..dot_pos];
        let local_type = &type_name[dot_pos + 1..];
        if let Some(html_path) = import_html_paths.get(alias) {
            return format!(
                "<a href=\"{}#type-{}\">{}</a>",
                html_escape(html_path),
                html_escape(local_type),
                html_escape(type_name)
            );
        }
    }

    // Check if it's a known primitive
    if is_builtin(type_name) {
        return format!("<code class=\"type-primitive\">{}</code>", html_escape(type_name));
    }

    // Local type reference
    format!(
        "<a href=\"#type-{id}\" class=\"type-ref\">{name}</a>",
        id = html_escape(type_name),
        name = html_escape(type_name)
    )
}

fn render_type_ref_from_value(val: &JsonValue, import_html_paths: &BTreeMap<String, String>) -> String {
    match val {
        JsonValue::String(s) => render_type_ref(s, import_html_paths),
        JsonValue::Object(map) => {
            if let Some(t) = map.get("type").and_then(|v| v.as_str()) {
                return render_type_ref(t, import_html_paths);
            }
            if let Some(r) = map.get("$ref").and_then(|v| v.as_str()) {
                return render_type_ref(r, import_html_paths);
            }
            "<em>object</em>".to_string()
        }
        JsonValue::Array(arr) => {
            let parts: Vec<String> = arr
                .iter()
                .map(|v| render_type_ref_from_value(v, import_html_paths))
                .collect();
            parts.join(" | ")
        }
        _ => html_escape(&json_value_display(val)),
    }
}

fn is_builtin(name: &str) -> bool {
    matches!(name, "string" | "integer" | "number" | "boolean" | "object" | "array" | "null")
}

// ─── Utility helpers ─────────────────────────────────────────────────────────

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

fn json_value_display(val: &JsonValue) -> String {
    match val {
        JsonValue::String(s) => s.clone(),
        JsonValue::Number(n) => n.to_string(),
        JsonValue::Bool(b) => b.to_string(),
        JsonValue::Null => "null".to_string(),
        JsonValue::Array(arr) => {
            let parts: Vec<String> = arr.iter().map(json_value_display).collect();
            format!("[{}]", parts.join(", "))
        }
        JsonValue::Object(_) => serde_json::to_string(val).unwrap_or_else(|_| "{}".to_string()),
    }
}

// ─── CSS ─────────────────────────────────────────────────────────────────────

fn inline_css() -> &'static str {
    r#"
*, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }

body {
  display: flex;
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial, sans-serif;
  font-size: 14px;
  line-height: 1.6;
  color: #24292f;
  background: #f6f8fa;
  min-height: 100vh;
}

/* ── Sidebar ── */
#sidebar {
  position: sticky;
  top: 0;
  height: 100vh;
  width: 220px;
  min-width: 220px;
  overflow-y: auto;
  background: #fff;
  border-right: 1px solid #d0d7de;
  padding: 16px 0;
}

.nav-title {
  font-weight: 600;
  font-size: 13px;
  color: #57606a;
  padding: 0 16px 8px;
  border-bottom: 1px solid #d0d7de;
  margin-bottom: 8px;
  word-break: break-all;
}

#sidebar ul {
  list-style: none;
}

#sidebar li a {
  display: block;
  padding: 3px 16px;
  color: #0969da;
  text-decoration: none;
  font-size: 13px;
  font-family: ui-monospace, "Cascadia Code", "Fira Code", monospace;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

#sidebar li a:hover { background: #f6f8fa; }

.nav-section {
  font-size: 11px;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  color: #57606a;
  padding: 8px 16px 2px;
}

/* ── Main content ── */
main {
  flex: 1;
  padding: 32px 40px;
  max-width: 900px;
  min-width: 0;
}

.page-title {
  font-size: 24px;
  font-weight: 700;
  color: #24292f;
  margin-bottom: 24px;
  padding-bottom: 12px;
  border-bottom: 1px solid #d0d7de;
}

/* ── Sections ── */
.doc-section {
  margin-bottom: 40px;
}

.doc-section > h2 {
  font-size: 18px;
  font-weight: 600;
  color: #24292f;
  margin-bottom: 16px;
  padding-bottom: 8px;
  border-bottom: 1px solid #d0d7de;
}

.doc-section h3 {
  font-size: 14px;
  font-weight: 600;
  color: #57606a;
  margin: 16px 0 8px;
}

/* ── Type cards ── */
.type-card {
  background: #fff;
  border: 1px solid #d0d7de;
  border-radius: 6px;
  margin-bottom: 16px;
  padding: 16px;
}

.type-header {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 8px;
  flex-wrap: wrap;
}

.type-name {
  font-family: ui-monospace, "Cascadia Code", "Fira Code", monospace;
  font-size: 15px;
  font-weight: 600;
  color: #0969da;
}

.kind-badge {
  font-size: 11px;
  font-weight: 600;
  text-transform: uppercase;
  padding: 2px 6px;
  border-radius: 4px;
  background: #ddf4ff;
  color: #0550ae;
  font-family: ui-monospace, monospace;
}

.extends-badge {
  font-size: 12px;
  color: #57606a;
}

.extends-badge a {
  color: #0969da;
}

.deprecated-badge {
  font-size: 11px;
  font-weight: 600;
  padding: 1px 5px;
  border-radius: 4px;
  background: #ffebe9;
  color: #d73a49;
}

.type-desc {
  color: #57606a;
  margin-bottom: 8px;
  font-size: 13px;
}

/* ── Property tables ── */
.prop-table {
  width: 100%;
  border-collapse: collapse;
  margin-bottom: 8px;
  font-size: 13px;
}

.prop-table th {
  text-align: left;
  padding: 6px 10px;
  background: #f6f8fa;
  border: 1px solid #d0d7de;
  font-weight: 600;
  color: #57606a;
}

.prop-table td {
  padding: 6px 10px;
  border: 1px solid #d0d7de;
  vertical-align: top;
}

.prop-table tr:nth-child(even) td { background: #f6f8fa; }

code {
  font-family: ui-monospace, "Cascadia Code", "Fira Code", monospace;
  font-size: 12px;
  background: #f6f8fa;
  padding: 1px 4px;
  border-radius: 3px;
}

a { color: #0969da; text-decoration: none; }
a:hover { text-decoration: underline; }

.type-ref { color: #0969da; font-family: ui-monospace, monospace; font-size: 12px; }
.type-primitive { background: #fff8c5; color: #9a6700; }

/* ── Enum / union lists ── */
.enum-list, .union-list {
  list-style: none;
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-bottom: 8px;
}

.enum-list li code {
  background: #f6f8fa;
  border: 1px solid #d0d7de;
  padding: 2px 8px;
  border-radius: 12px;
  font-size: 12px;
}

/* ── Constraints ── */
.constraints {
  margin-top: 10px;
  padding: 10px 12px;
  background: #f6f8fa;
  border-left: 3px solid #0969da;
  border-radius: 0 4px 4px 0;
  font-size: 13px;
}

.constraints ul { margin-top: 4px; padding-left: 20px; }
.constraints li { margin: 2px 0; }
.constraint-path { color: #57606a; }

.scalar-constraints { font-size: 13px; color: #57606a; }

h4 {
  font-size: 13px;
  font-weight: 600;
  color: #57606a;
  margin: 12px 0 6px;
}

/* ── Data toggle button ── */
.data-toggle-btn {
  display: inline-block;
  font-size: 12px;
  font-weight: 600;
  padding: 4px 10px;
  border-radius: 4px;
  border: 1px solid #d0d7de;
  background: #f6f8fa;
  color: #24292f;
  cursor: pointer;
  margin-bottom: 12px;
  font-family: ui-monospace, "Cascadia Code", "Fira Code", monospace;
}

.data-toggle-btn:hover {
  background: #e1e4e8;
  border-color: #b0b7be;
}

#data-content pre {
  background: #f6f8fa;
  border: 1px solid #d0d7de;
  border-radius: 6px;
  padding: 16px;
  overflow-x: auto;
  font-size: 12px;
  line-height: 1.5;
}

#data-content code {
  background: none;
  padding: 0;
  font-size: 12px;
}
"#
}


// ─── Multi-file site generation ──────────────────────────────────────────────

/// Walk local imports recursively from `root`, returning a map of canonical path → raw file content.
/// Skips URL imports (http/https) and @module imports.
/// Deduplicates by canonical path (via fs::canonicalize, fallback to absolute path).
pub fn collect_import_graph(root: &Path) -> Result<BTreeMap<PathBuf, String>, SyamlError> {
    let mut result: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut queue: Vec<PathBuf> = Vec::new();

    // Canonicalize the root path
    let root_canonical = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    queue.push(root_canonical);

    while let Some(current_path) = queue.pop() {
        if result.contains_key(&current_path) {
            continue;
        }

        let content = match fs::read_to_string(&current_path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!(
                    "super_yaml: warning: skipping '{}': {e}",
                    current_path.display()
                );
                continue;
            }
        };

        let parsed = match parse_document_or_manifest(&content) {
            Ok(p) => p,
            Err(e) => {
                eprintln!(
                    "super_yaml: warning: failed to parse '{}': {e}",
                    current_path.display()
                );
                result.insert(current_path, content);
                continue;
            }
        };

        result.insert(current_path.clone(), content);

        // Enqueue local imports
        if let Some(meta) = &parsed.meta {
            let base = current_path.parent().unwrap_or(Path::new("."));
            for (_alias, binding) in &meta.imports {
                let raw = &binding.path;
                if raw.starts_with("http://") || raw.starts_with("https://") || raw.starts_with('@') {
                    continue;
                }
                let resolved = base.join(raw);
                if !resolved.exists() {
                    eprintln!(
                        "super_yaml: warning: import path not found: '{}'",
                        resolved.display()
                    );
                    continue;
                }
                let canonical = fs::canonicalize(&resolved).unwrap_or(resolved);
                if !result.contains_key(&canonical) {
                    queue.push(canonical);
                }
            }
        }
    }

    Ok(result)
}

/// Given a path to a module.syaml file, return all .syaml files in the same directory
/// (excluding module.syaml itself), sorted alphabetically.
pub fn discover_module_members(module_syaml: &Path) -> Result<Vec<PathBuf>, SyamlError> {
    let dir = module_syaml.parent().unwrap_or(Path::new("."));
    let mut members: Vec<PathBuf> = Vec::new();

    let read_dir = fs::read_dir(dir).map_err(|e| {
        SyamlError::ImportError(format!(
            "failed to read directory '{}': {e}",
            dir.display()
        ))
    })?;

    for entry in read_dir {
        let entry = entry.map_err(|e| {
            SyamlError::ImportError(format!("failed to read directory entry: {e}"))
        })?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("syaml") {
            continue;
        }
        if path.file_name().and_then(|n| n.to_str()) == Some("module.syaml") {
            continue;
        }
        members.push(path);
    }

    members.sort();
    Ok(members)
}

/// Generate a multi-file HTML documentation site.
/// Returns a map of relative output path (e.g. "invoice.html") → HTML content.
/// Always includes an "index.html" entry.
pub fn generate_html_docs_site(
    roots: &[PathBuf],
    base_dir: &Path,
) -> Result<BTreeMap<String, String>, SyamlError> {
    let mut site: BTreeMap<String, String> = BTreeMap::new();

    // Map: canonical file path → relative output HTML path (e.g. "payments/invoice.html")
    let mut path_to_rel_html: BTreeMap<PathBuf, String> = BTreeMap::new();

    // Compute relative output paths for each root
    for file_path in roots {
        let canonical = fs::canonicalize(file_path).unwrap_or_else(|_| file_path.clone());
        let rel = file_path
            .strip_prefix(base_dir)
            .unwrap_or(file_path.as_path());
        let html_rel = rel.with_extension("html").to_string_lossy().into_owned();
        path_to_rel_html.insert(canonical, html_rel);
    }

    // Generate HTML for each file
    let mut index_links: Vec<(String, String)> = Vec::new(); // (html_rel_path, title)

    for file_path in roots {
        let canonical = fs::canonicalize(file_path).unwrap_or_else(|_| file_path.clone());
        let html_rel = match path_to_rel_html.get(&canonical) {
            Some(r) => r.clone(),
            None => continue,
        };

        let content = fs::read_to_string(file_path).map_err(|e| {
            SyamlError::ImportError(format!(
                "failed to read '{}': {e}",
                file_path.display()
            ))
        })?;

        let parsed = parse_document_or_manifest(&content)?;

        let file_title = file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("SYAML Documentation");

        // Build import_html_paths for cross-linking from this page's location
        let current_html_dir = Path::new(&html_rel)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from(""));

        let import_html_paths: BTreeMap<String, String> = parsed
            .meta
            .as_ref()
            .map(|m| {
                m.imports
                    .iter()
                    .filter_map(|(alias, binding)| {
                        let raw = &binding.path;
                        if raw.starts_with("http://")
                            || raw.starts_with("https://")
                            || raw.starts_with('@')
                        {
                            return None;
                        }
                        // Resolve the import to a canonical path
                        let file_dir = file_path.parent().unwrap_or(Path::new("."));
                        let resolved = file_dir.join(raw);
                        let resolved_canonical =
                            fs::canonicalize(&resolved).unwrap_or(resolved);

                        // Find the html_rel for that canonical path
                        let target_html_rel = path_to_rel_html.get(&resolved_canonical)?;

                        // Compute relative path from current_html_dir to target_html_rel
                        let rel_link = compute_relative_html_path(
                            &current_html_dir,
                            Path::new(target_html_rel),
                        );
                        Some((alias.clone(), rel_link))
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Pass data to the assembler so data section is rendered
        let data_val = &parsed.data.value;
        let data_opt = if is_empty_data(data_val) { None } else { Some(data_val) };

        let html = assemble_page_with_import_links(
            file_title,
            parsed.meta.as_ref(),
            &parsed.schema,
            parsed.functional.as_ref(),
            data_opt,
            &import_html_paths,
        );

        index_links.push((html_rel.clone(), file_title.to_string()));
        site.insert(html_rel, html);
    }

    // Generate index.html
    let index_html = generate_index_page(&index_links);
    site.insert("index.html".to_string(), index_html);

    Ok(site)
}

/// Assemble a page with an explicit `import_html_paths` map (alias → href).
fn assemble_page_with_import_links(
    title: &str,
    meta: Option<&Meta>,
    schema: &SchemaDoc,
    functional: Option<&FunctionalDoc>,
    data: Option<&JsonValue>,
    import_html_paths: &BTreeMap<String, String>,
) -> String {
    let meta_html = meta
        .map(|m| render_meta_section(m, import_html_paths))
        .unwrap_or_default();
    let schema_html = render_schema_section(schema, import_html_paths);
    let functional_html = functional
        .map(|f| render_functional_section(f, import_html_paths))
        .unwrap_or_default();

    // Only render data section if the value is non-null and non-empty
    let data_html = data
        .filter(|v| !is_empty_data(v))
        .map(render_data_section)
        .unwrap_or_default();

    let nav_items = build_nav_items(schema, functional, data.filter(|v| !is_empty_data(v)));

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>{title}</title>
<style>
{css}
</style>
</head>
<body>
<nav id="sidebar">
  <div class="nav-title">{title}</div>
  <ul>
{nav_items}
  </ul>
</nav>
<main>
  <h1 class="page-title">{title}</h1>
{meta_html}
{schema_html}
{functional_html}
{data_html}
</main>
<script>
function toggleData() {{
  var el = document.getElementById('data-content');
  var btn = document.getElementById('data-toggle');
  if (el.style.display === 'none') {{
    el.style.display = 'block';
    btn.textContent = 'Hide data';
  }} else {{
    el.style.display = 'none';
    btn.textContent = 'Show data';
  }}
}}
</script>
</body>
</html>
"#,
        title = html_escape(title),
        css = inline_css(),
        nav_items = nav_items,
        meta_html = meta_html,
        schema_html = schema_html,
        functional_html = functional_html,
        data_html = data_html,
    )
}

/// Compute a relative path from `from_dir` to `to_file`.
/// Both paths are relative to the site root.
fn compute_relative_html_path(from_dir: &Path, to_file: &Path) -> String {
    // Count how many levels to go up from from_dir
    let from_components: Vec<_> = from_dir.components().collect();
    let to_components: Vec<_> = to_file.components().collect();

    // Find common prefix length
    let common_len = from_components
        .iter()
        .zip(to_components.iter())
        .take_while(|(a, b)| a == b)
        .count();

    let up_count = from_components.len() - common_len;
    let down_parts: Vec<_> = to_components[common_len..].to_vec();

    let mut parts: Vec<String> = Vec::new();
    for _ in 0..up_count {
        parts.push("..".to_string());
    }
    for part in &down_parts {
        parts.push(part.as_os_str().to_string_lossy().into_owned());
    }

    if parts.is_empty() {
        to_file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| to_file.to_string_lossy().into_owned())
    } else {
        parts.join("/")
    }
}

/// Generate a simple index page listing all documentation pages.
fn generate_index_page(links: &[(String, String)]) -> String {
    let mut list_items = String::new();
    for (href, title) in links {
        list_items.push_str(&format!(
            "    <li><a href=\"{href}\">{title}</a></li>\n",
            href = html_escape(href),
            title = html_escape(title),
        ));
    }

    let mut nav_links = String::new();
    for (href, title) in links {
        nav_links.push_str(&format!(
            "    <li><a href=\"{href}\">{title}</a></li>\n",
            href = html_escape(href),
            title = html_escape(title),
        ));
    }

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Documentation Index</title>
<style>
{css}
</style>
</head>
<body>
<nav id="sidebar">
  <div class="nav-title">Documentation Index</div>
  <ul>
{nav_links}
  </ul>
</nav>
<main>
  <h1 class="page-title">Documentation Index</h1>
  <section class="doc-section">
    <h2>Pages</h2>
    <ul>
{list_items}
    </ul>
  </section>
</main>
</body>
</html>
"#,
        css = inline_css(),
        nav_links = nav_links,
        list_items = list_items,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_html_with_type_anchor_ids() {
        let input = r#"---!syaml/v0
---schema
Money:
  type: object
  properties:
    amount:
      type: integer
    currency:
      type: string
  required:
    - amount
    - currency
Port:
  type: integer
  minimum: 1
  maximum: 65535
---data
price <Money>:
  amount: 100
  currency: USD
"#;
        let html = generate_html_docs(input).unwrap();
        assert!(html.contains("id=\"type-Money\""), "should contain Money anchor");
        assert!(html.contains("id=\"type-Port\""), "should contain Port anchor");
        assert!(html.contains("href=\"#type-Money\""), "nav should link to Money");
        assert!(html.contains("<td><code>amount</code>"), "should list amount field");
        assert!(html.contains("minimum"), "should show minimum constraint");
    }

    #[test]
    fn generates_html_with_enum_type() {
        let input = r#"---!syaml/v0
---schema
Status:
  enum:
    - active
    - inactive
    - pending
---data
{}
"#;
        let html = generate_html_docs(input).unwrap();
        assert!(html.contains("id=\"type-Status\""));
        assert!(html.contains("active"));
        assert!(html.contains("enum"));
    }

    #[test]
    fn generates_html_with_meta_imports() {
        let input = r#"---!syaml/v0
---meta
file:
  version: "1.0.0"
imports:
  payments: ../payments/invoice.syaml
---schema
{}
---data
{}
"#;
        let html = generate_html_docs(input).unwrap();
        assert!(html.contains("payments"));
        assert!(html.contains("1.0.0"));
    }

    #[test]
    fn html_escape_prevents_xss() {
        assert_eq!(html_escape("<script>"), "&lt;script&gt;");
        assert_eq!(html_escape("a & b"), "a &amp; b");
    }

    #[test]
    fn generates_html_with_data_section_toggle() {
        let input = r#"---!syaml/v0
---schema
Config:
  type: object
  properties:
    host:
      type: string
    port:
      type: integer
---data
host: localhost
port: 8080
"#;
        let html = generate_html_docs(input).unwrap();

        // Data section wrapper with correct id
        assert!(
            html.contains("id=\"data-section\""),
            "should contain data-section anchor"
        );

        // Toggle button is present with correct id and initial label
        assert!(
            html.contains("id=\"data-toggle\""),
            "should contain data-toggle button"
        );
        assert!(
            html.contains("Show data"),
            "button should initially say 'Show data'"
        );

        // Hidden div is present and starts hidden
        assert!(
            html.contains("id=\"data-content\""),
            "should contain data-content div"
        );
        assert!(
            html.contains("style=\"display:none\""),
            "data-content should start hidden"
        );

        // Actual data values are rendered inside a pre/code block
        assert!(html.contains("<pre><code>"), "should contain pre/code block");
        assert!(html.contains("localhost"), "should include data value 'localhost'");
        assert!(html.contains("8080"), "should include data value '8080'");

        // JS toggle function is included
        assert!(
            html.contains("function toggleData()"),
            "should include toggleData JS function"
        );

        // Nav sidebar item for Data section
        assert!(
            html.contains("href=\"#data-section\""),
            "nav should link to data-section"
        );
    }

    #[test]
    fn does_not_render_data_section_when_empty() {
        let input = r#"---!syaml/v0
---schema
Foo:
  type: string
---data
{}
"#;
        let html = generate_html_docs(input).unwrap();
        assert!(
            !html.contains("id=\"data-section\""),
            "should not render data section for empty data"
        );
        assert!(
            !html.contains("id=\"data-toggle\""),
            "should not render data toggle for empty data"
        );
    }

    #[test]
    fn generate_html_docs_site_returns_index() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("syaml_html_site_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join("types.syaml");
        {
            let mut f = std::fs::File::create(&file_path).unwrap();
            writeln!(f, "---!syaml/v0").unwrap();
            writeln!(f, "---schema").unwrap();
            writeln!(f, "Foo:").unwrap();
            writeln!(f, "  type: string").unwrap();
            writeln!(f, "---data").unwrap();
            writeln!(f, "{{}}").unwrap();
        }

        let site = generate_html_docs_site(&[file_path], &dir).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(site.contains_key("index.html"), "site must have index.html");
        assert!(site.contains_key("types.html"), "site must have types.html");
        assert!(site["index.html"].contains("types"), "index should link to types");
    }

    #[test]
    fn discover_module_members_excludes_module_syaml() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("syaml_discover_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        for name in &["module.syaml", "invoice.syaml", "refund.syaml"] {
            let p = dir.join(name);
            std::fs::File::create(&p).unwrap().write_all(b"").unwrap();
        }

        let module_path = dir.join("module.syaml");
        let mut members = discover_module_members(&module_path).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        members.sort();

        let names: Vec<_> = members
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap().to_string())
            .collect();

        assert!(names.iter().any(|n| n == "invoice.syaml"), "should include invoice.syaml");
        assert!(names.iter().any(|n| n == "refund.syaml"), "should include refund.syaml");
        assert!(!names.iter().any(|n| n == "module.syaml"), "should exclude module.syaml");
    }
}
