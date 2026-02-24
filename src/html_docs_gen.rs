//! HTML documentation generator for `.syaml` files.
//!
//! Produces a self-contained HTML page documenting the schema types, data
//! entries, and functional definitions found in a SYAML document.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value as JsonValue;

use crate::ast::{FunctionalDoc, Meta, SchemaDoc};
use crate::section_scanner::scan_sections;
use crate::{parse_document_or_manifest, SyamlError};

// ─── Public API ──────────────────────────────────────────────────────────────

/// Generates an HTML documentation page from an in-memory `.syaml` string.
///
/// No cross-file import links are produced; all type references within the
/// document are rendered as internal anchor links.
pub fn generate_html_docs(input: &str) -> Result<String, SyamlError> {
    let parsed = parse_document_or_manifest(input)?;
    let raw_data = extract_raw_data_section(input);
    let raw_schema = extract_raw_schema_section(input);
    let file_title = "SYAML Documentation";
    Ok(assemble_page(
        file_title,
        parsed.meta.as_ref(),
        &parsed.schema,
        parsed.functional.as_ref(),
        Some(&parsed.data.value),
        raw_data.as_deref(),
        raw_schema.as_deref(),
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
    let raw_data = extract_raw_data_section(&input);
    let raw_schema = extract_raw_schema_section(&input);
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
        raw_data.as_deref(),
        raw_schema.as_deref(),
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
    raw_data: Option<&str>,
    raw_schema: Option<&str>,
) -> String {
    assemble_page_with_paths(
        title,
        meta,
        schema,
        functional,
        data,
        raw_data,
        raw_schema,
        Path::new("."),
    )
}

fn assemble_page_with_paths(
    title: &str,
    meta: Option<&Meta>,
    schema: &SchemaDoc,
    functional: Option<&FunctionalDoc>,
    data: Option<&JsonValue>,
    raw_data: Option<&str>,
    raw_schema: Option<&str>,
    _base_dir: &Path,
) -> String {
    // Build import alias → relative html path map for cross-linking
    let import_html_paths: BTreeMap<String, String> = meta
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
                    let html_path = Path::new(raw)
                        .with_extension("html")
                        .to_string_lossy()
                        .into_owned();
                    Some((alias.clone(), html_path))
                })
                .collect()
        })
        .unwrap_or_default();

    let type_sources: BTreeMap<String, String> = raw_schema
        .map(extract_schema_type_blocks)
        .unwrap_or_default();

    let meta_html = meta
        .map(|m| render_meta_section(m, &import_html_paths))
        .unwrap_or_default();
    let schema_html = render_schema_section(schema, &import_html_paths, &type_sources);
    let functional_html = functional
        .map(|f| render_functional_section(f, &import_html_paths))
        .unwrap_or_default();

    // Only render data section if the value is non-null and non-empty.
    // For the single-file path there are no cross-file import links, so pass
    // the same import_html_paths map (which maps alias → sibling .html paths).
    let data_html = data
        .filter(|v| !is_empty_data(v))
        .map(|v| render_data_section(v, raw_data, &import_html_paths))
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
function toggleJsonView() {{
  var syaml = document.getElementById('syaml-content');
  var json  = document.getElementById('json-content');
  var btn   = document.getElementById('json-toggle');
  var showingJson = json.style.display !== 'none';
  if (showingJson) {{
    json.style.display  = 'none';
    syaml.style.display = 'block';
    btn.textContent = 'Show JSON';
  }} else {{
    syaml.style.display = 'none';
    json.style.display  = 'block';
    btn.textContent = 'Show SYAML';
  }}
}}
function toggleTypeSource(id) {{
  var rendered = document.getElementById('type-rendered-' + id);
  var source   = document.getElementById('type-source-'   + id);
  var btn      = document.getElementById('type-toggle-'   + id);
  var showingSource = source.style.display !== 'none';
  if (showingSource) {{
    source.style.display   = 'none';
    rendered.style.display = 'block';
    btn.textContent = 'Show source';
  }} else {{
    rendered.style.display = 'none';
    source.style.display   = 'block';
    btn.textContent = 'Show rendered';
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

fn build_nav_items(
    schema: &SchemaDoc,
    functional: Option<&FunctionalDoc>,
    data: Option<&JsonValue>,
) -> String {
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
        html.push_str(
            "<thead><tr><th>Alias</th><th>Path</th><th>Sections</th></tr></thead>\n<tbody>\n",
        );
        for (alias, binding) in &meta.imports {
            let path_cell = if let Some(html_path) = import_html_paths.get(alias) {
                format!(
                    "<a href=\"{}\">{}</a>",
                    html_escape(html_path),
                    html_escape(&binding.path)
                )
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

// ── SYAML syntax highlighter ─────────────────────────────────────────────────
//
// Processes raw SYAML source line-by-line, wrapping recognised tokens in
// <span class="hl-*"> elements.  The highlighter understands:
//   • full-line and inline comments  (#)
//   • list-item markers             (-)
//   • keys                          (identifier before < or :)
//   • type hints                    (<alias.TypeName>)
//   • the colon separator           (:)
//   • quoted strings                ("…" / '…')
//   • numbers with optional units   (42, 3.14, 1024bytes, 2.5mb)
//   • YAML keywords                 (true, false, null, ~)
//   • plain unquoted values         (enum members, bare identifiers)

fn hl_span(class: &str, content: &str) -> String {
    format!("<span class=\"{class}\">{content}</span>")
}

/// Find the position of the first `:` that acts as a YAML key-value separator:
/// i.e. `:` followed by a space/tab or at the very end of the string.
fn find_colon_sep(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    for i in 0..b.len() {
        if b[i] == b':' {
            match b.get(i + 1) {
                None | Some(b' ') | Some(b'\t') => return Some(i),
                _ => {}
            }
        }
    }
    None
}

/// Highlight a single SYAML value token (the right-hand side of `key: VALUE`).
fn hl_value(val: &str) -> String {
    if val.is_empty() {
        return String::new();
    }

    // Inline comment appended after value (e.g. `42 # count`)
    // Split on first ` #` sequence outside a quoted string.
    let (val_part, comment_part) = split_inline_comment(val);

    let highlighted = hl_value_token(val_part.trim_end());

    let comment_html = if comment_part.is_empty() {
        String::new()
    } else {
        format!(" {}", hl_span("hl-comment", &html_escape(comment_part)))
    };

    format!("{highlighted}{comment_html}")
}

fn hl_value_token(val: &str) -> String {
    if val.is_empty() {
        return String::new();
    }

    let first = val.as_bytes()[0];

    // Quoted string
    if first == b'"' || first == b'\'' {
        return hl_span("hl-string", &html_escape(val));
    }

    // YAML keywords
    if matches!(val, "true" | "false" | "null" | "~") {
        return hl_span("hl-keyword", val);
    }

    // Pure number
    if val.parse::<f64>().is_ok() {
        return hl_span("hl-number", val);
    }

    // Number with a unit suffix: digits (+ optional decimal) followed by non-digit letters
    // e.g. "1024bytes", "2.5mb", "50px"
    if first.is_ascii_digit() {
        let num_end = val
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .unwrap_or(val.len());
        if num_end > 0 && num_end < val.len() {
            let num_part = &val[..num_end];
            let unit_part = &val[num_end..];
            // Only treat as number+unit when the suffix is alphabetic
            if unit_part.chars().all(|c| c.is_alphabetic()) {
                return format!(
                    "{}{}",
                    hl_span("hl-number", num_part),
                    hl_span("hl-unit", &html_escape(unit_part))
                );
            }
        }
    }

    // URLs — render as strings
    if val.starts_with("http://") || val.starts_with("https://") {
        return hl_span("hl-string", &html_escape(val));
    }

    // Plain value (enum member, identifier, etc.)
    hl_span("hl-value", &html_escape(val))
}

/// Split `text` into (value_part, comment_part) where `comment_part` begins
/// at the first ` #` that is not inside a quoted string.
fn split_inline_comment(text: &str) -> (&str, &str) {
    let mut in_quote: Option<u8> = None;
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match in_quote {
            Some(q) if b[i] == q => in_quote = None,
            Some(_) => {}
            None => {
                if b[i] == b'"' || b[i] == b'\'' {
                    in_quote = Some(b[i]);
                } else if b[i] == b'#' && i > 0 && b[i - 1] == b' ' {
                    return (&text[..i - 1], &text[i..]);
                }
            }
        }
        i += 1;
    }
    (text, "")
}

/// Apply syntax highlighting to a raw SYAML data-section body.
/// Returns an HTML string with `<span>` tags; suitable for use inside `<pre><code>`.
/// Render a `<alias.TypeName>` or `<LocalType>` type hint as a linked token.
///
/// • `<alias.TypeName>` → links to `{import_html_paths[alias]}#type-TypeName`
/// • `<LocalType>`      → links to `#type-LocalType` (same-page anchor)
///
/// Falls back to a plain `hl-type` span when the alias is unknown.
fn hl_type_hint(inner: &str, import_html_paths: &BTreeMap<String, String>) -> String {
    let display = format!("&lt;{}&gt;", html_escape(inner));
    let href = if let Some(dot) = inner.find('.') {
        let alias = &inner[..dot];
        let type_name = &inner[dot + 1..];
        import_html_paths
            .get(alias)
            .map(|page| format!("{}#type-{}", page, html_escape(type_name)))
    } else {
        // Local type — anchor on the same page
        Some(format!("#type-{}", html_escape(inner)))
    };

    match href {
        Some(h) => format!(
            r#"<a href="{href}" class="hl-type">{display}</a>"#,
            href = h,
            display = display
        ),
        None => hl_span("hl-type", &display),
    }
}

fn highlight_syaml_source(source: &str, import_html_paths: &BTreeMap<String, String>) -> String {
    source
        .lines()
        .map(|line| highlight_syaml_line(line, import_html_paths))
        .collect::<Vec<_>>()
        .join("\n")
}

fn highlight_syaml_line(line: &str, import_html_paths: &BTreeMap<String, String>) -> String {
    // Preserve leading indentation verbatim.
    let indent_len = line.len() - line.trim_start().len();
    let indent = &line[..indent_len];
    let trimmed = &line[indent_len..];

    let mut out = html_escape(indent);

    if trimmed.is_empty() {
        return out;
    }

    // Full-line comment
    if trimmed.starts_with('#') {
        out.push_str(&hl_span("hl-comment", &html_escape(trimmed)));
        return out;
    }

    // List-item marker
    let content = if let Some(rest) = trimmed.strip_prefix("- ") {
        out.push_str(&hl_span("hl-op", "-"));
        out.push(' ');
        rest
    } else if trimmed == "-" {
        out.push_str(&hl_span("hl-op", "-"));
        return out;
    } else {
        trimmed
    };

    // If the content starts with a quote it's a bare scalar list item — no key.
    if content.starts_with('"') || content.starts_with('\'') {
        out.push_str(&hl_value(content));
        return out;
    }

    // Determine where the key ends: at the first `<` (type hint) or the first
    // colon-separator, whichever comes first.
    let lt_pos = content.find('<');
    let colon_pos = find_colon_sep(content);

    let key_end = match (lt_pos, colon_pos) {
        (Some(t), Some(c)) => t.min(c),
        (Some(t), None) => t,
        (None, Some(c)) => c,
        (None, None) => {
            // No separator at all: the whole thing is a bare key (sub-object on next lines)
            // or a plain scalar that didn't start with a quote.
            let is_identifier = content
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '-');
            if is_identifier {
                out.push_str(&hl_span("hl-key", &html_escape(content)));
            } else {
                out.push_str(&hl_value(content));
            }
            return out;
        }
    };

    // Emit key (trimming trailing space before type hint / colon)
    let key_raw = &content[..key_end];
    let key = key_raw.trim_end();
    if !key.is_empty() {
        out.push_str(&hl_span("hl-key", &html_escape(key)));
    }
    // Preserve whitespace between key and type hint / colon
    if key.len() < key_raw.len() {
        out.push_str(&html_escape(&key_raw[key.len()..]));
    }

    let rest = &content[key_end..];

    // Optional type hint  <alias.TypeName>  or  <LocalType>
    let rest = if rest.starts_with('<') {
        match rest.find('>') {
            Some(end) => {
                let inner = &rest[1..end]; // content between < and >
                out.push_str(&hl_type_hint(inner, import_html_paths));
                &rest[end + 1..]
            }
            None => {
                out.push_str(&html_escape(rest));
                ""
            }
        }
    } else {
        rest
    };

    // Colon separator and value
    if let Some(after_colon) = rest.strip_prefix(':') {
        out.push_str(&hl_span("hl-op", ":"));
        // One space between colon and value
        if let Some(value) = after_colon.strip_prefix(' ') {
            out.push(' ');
            if !value.is_empty() {
                out.push_str(&hl_value(value));
            }
        } else if !after_colon.is_empty() {
            // Colon immediately followed by something unexpected — pass through.
            out.push_str(&html_escape(after_colon));
        }
    } else if !rest.is_empty() {
        out.push_str(&html_escape(rest));
    }

    out
}

// ─────────────────────────────────────────────────────────────────────────────

/// Returns true when the data value is effectively empty (null or empty object).
fn is_empty_data(val: &JsonValue) -> bool {
    match val {
        JsonValue::Null => true,
        JsonValue::Object(map) => map.is_empty(),
        _ => false,
    }
}

/// Extracts the raw body of the `---data` section from a `.syaml` source string.
fn extract_raw_data_section(content: &str) -> Option<String> {
    let (_, sections) = scan_sections(content).ok()?;
    sections
        .into_iter()
        .find(|s| s.name == "data")
        .map(|s| s.body)
}

fn extract_raw_schema_section(content: &str) -> Option<String> {
    let (_, sections) = scan_sections(content).ok()?;
    sections
        .into_iter()
        .find(|s| s.name == "schema")
        .map(|s| s.body)
}

/// Split a raw `---schema` section body into per-type raw YAML blocks.
///
/// Each top-level key (starting at column 0, not a comment) begins a new type
/// block.  The block includes the `TypeName:` header line plus all indented
/// lines that follow.  Returns a map of type-name → raw block text.
fn extract_schema_type_blocks(schema_body: &str) -> BTreeMap<String, String> {
    let mut blocks: BTreeMap<String, String> = BTreeMap::new();
    let mut current_name: Option<String> = None;
    let mut current_lines: Vec<&str> = Vec::new();

    for line in schema_body.lines() {
        let is_top_level = !line.starts_with(' ')
            && !line.starts_with('\t')
            && !line.trim().is_empty()
            && !line.trim_start().starts_with('#');

        if is_top_level {
            if let Some(name) = current_name.take() {
                blocks.insert(name, current_lines.join("\n").trim_end().to_string());
            }
            // Strip trailing colon to get the bare type name.
            let type_name = line.trim_end().trim_end_matches(':').to_string();
            current_lines = vec![line];
            current_name = Some(type_name);
        } else if current_name.is_some() {
            current_lines.push(line);
        }
    }
    if let Some(name) = current_name.take() {
        blocks.insert(name, current_lines.join("\n").trim_end().to_string());
    }
    blocks
}

/// Renders the `---data` section.
///
/// The raw SYAML source is always shown. Clicking "Show JSON" opens the
/// compiled JSON panel to the right of the SYAML block in a side-by-side layout.
fn render_data_section(
    data: &JsonValue,
    raw_syaml: Option<&str>,
    import_html_paths: &BTreeMap<String, String>,
) -> String {
    let pretty_json = serde_json::to_string_pretty(data).unwrap_or_else(|_| "{}".to_string());

    let syaml_inner = match raw_syaml {
        Some(raw) if !raw.trim().is_empty() => {
            highlight_syaml_source(raw.trim_end(), import_html_paths)
        }
        _ => String::new(),
    };

    format!(
        r#"<section class="doc-section" id="data-section">
<h2>Data <button class="data-toggle-btn" onclick="toggleJsonView()" id="json-toggle">Show JSON</button></h2>
<div id="syaml-content">
  <pre class="syaml-source"><code>{syaml_code}</code></pre>
</div>
<div id="json-content" style="display:none">
  <pre><code>{json_code}</code></pre>
</div>
</section>
"#,
        syaml_code = syaml_inner,
        json_code = html_escape(&pretty_json),
    )
}

// ─── Schema section ──────────────────────────────────────────────────────────

fn render_schema_section(
    schema: &SchemaDoc,
    import_html_paths: &BTreeMap<String, String>,
    type_sources: &BTreeMap<String, String>,
) -> String {
    if schema.types.is_empty() {
        return String::new();
    }

    let mut html = String::new();
    html.push_str("<section class=\"doc-section\">\n<h2>Schema</h2>\n");

    for (name, type_val) in &schema.types {
        let extends_parent = schema.extends.get(name);
        let constraints = schema.type_constraints.get(name);
        let raw_source = type_sources.get(name.as_str()).map(|s| s.as_str());
        html.push_str(&render_type_card(
            name,
            type_val,
            extends_parent.map(|s| s.as_str()),
            constraints,
            import_html_paths,
            raw_source,
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
    raw_source: Option<&str>,
) -> String {
    let mut html = String::new();
    let card_id = html_escape(name);
    html.push_str(&format!(
        "<div class=\"type-card\" id=\"type-{id}\">\n",
        id = card_id
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
    // Kind badge
    let kind = detect_type_kind(type_val);
    html.push_str(&format!("<span class=\"kind-badge\">{kind}</span>\n"));
    // Toggle button (only when we have source)
    if raw_source.is_some() {
        html.push_str(&format!(
            "<button class=\"type-source-toggle\" id=\"type-toggle-{id}\" \
             onclick=\"toggleTypeSource('{id}')\">Show source</button>\n",
            id = card_id
        ));
    }
    html.push_str("</div>\n");

    // --- Rendered view ---
    html.push_str(&format!(
        "<div id=\"type-rendered-{id}\">\n",
        id = card_id
    ));

    // Description if present
    if let Some(desc) = type_val.get("description").and_then(|v| v.as_str()) {
        html.push_str(&format!(
            "<p class=\"type-desc\">{}</p>\n",
            html_escape(desc)
        ));
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

    html.push_str("</div>\n"); // end type-rendered

    // --- Source code view (hidden by default) ---
    if let Some(src) = raw_source {
        let highlighted = highlight_syaml_source(src, import_html_paths);
        html.push_str(&format!(
            "<div id=\"type-source-{id}\" class=\"type-source-view\" style=\"display:none\">\
             <pre class=\"syaml-source\"><code>{highlighted}</code></pre>\
             </div>\n",
            id = card_id,
            highlighted = highlighted,
        ));
    }

    html.push_str("</div>\n"); // end type-card
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
        notes.push(format!(
            "field #{}",
            html_escape(&json_value_display(fn_num))
        ));
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
        html.push_str(&format!(
            "<p>Min items: <code>{}</code></p>\n",
            html_escape(&json_value_display(min))
        ));
    }
    if let Some(max) = val.get("maxItems") {
        html.push_str(&format!(
            "<p>Max items: <code>{}</code></p>\n",
            html_escape(&json_value_display(max))
        ));
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
    for key in &[
        "minimum",
        "maximum",
        "exclusiveMinimum",
        "exclusiveMaximum",
        "minLength",
        "maxLength",
        "pattern",
    ] {
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
    let mut html =
        String::from("<div class=\"constraints\">\n<strong>Constraints:</strong>\n<ul>\n");
    for (path, exprs) in constraints {
        for expr in exprs {
            let label = if path == "$" || path.is_empty() {
                String::new()
            } else {
                format!(
                    "<code class=\"constraint-path\">{}</code>: ",
                    html_escape(path)
                )
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
        JsonValue::Array(arr) => arr
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
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

    let mut html =
        String::from("<div class=\"constraints\">\n<strong>Constraints:</strong>\n<ul>\n");
    for expr in &exprs {
        html.push_str(&format!("<li><code>{}</code></li>\n", html_escape(expr)));
    }
    html.push_str("</ul>\n</div>\n");
    html
}

// ─── Functional section ───────────────────────────────────────────────────────

fn render_functional_section(
    func: &FunctionalDoc,
    import_html_paths: &BTreeMap<String, String>,
) -> String {
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
        html.push_str(&format!(
            "<span class=\"type-name\">{}</span>\n",
            html_escape(name)
        ));
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
        return format!(
            "<code class=\"type-primitive\">{}</code>",
            html_escape(type_name)
        );
    }

    // Local type reference
    format!(
        "<a href=\"#type-{id}\" class=\"type-ref\">{name}</a>",
        id = html_escape(type_name),
        name = html_escape(type_name)
    )
}

fn render_type_ref_from_value(
    val: &JsonValue,
    import_html_paths: &BTreeMap<String, String>,
) -> String {
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
    matches!(
        name,
        "string" | "integer" | "number" | "boolean" | "object" | "array" | "null"
    )
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

/* Home / index link at the top of the sidebar */
.nav-home {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 8px 16px;
  font-size: 13px;
  font-weight: 600;
  color: #57606a;
  text-decoration: none;
  border-bottom: 1px solid #d0d7de;
  margin-bottom: 8px;
}
.nav-home:hover { color: #0969da; background: #f6f8fa; }
.nav-home svg { flex-shrink: 0; }

/* "Files" section in the sidebar of module pages */
.nav-files {
  list-style: none;
  margin-bottom: 4px;
}

.nav-files li a {
  display: block;
  padding: 3px 16px 3px 24px;
  color: #0969da;
  text-decoration: none;
  font-size: 13px;
  font-family: ui-monospace, "Cascadia Code", "Fira Code", monospace;
}

.nav-files li a:hover { background: #f6f8fa; }

/* Current page entry in the files list — shown as plain text, not a link */
li.nav-files-current {
  padding: 3px 16px 3px 24px;
  font-size: 13px;
  font-family: ui-monospace, "Cascadia Code", "Fira Code", monospace;
  font-weight: 700;
  color: #24292f;
}

/* Subtle page-title label above the in-page nav anchors */
.nav-title-sub {
  font-size: 11px;
  color: #57606a;
  padding: 8px 16px 2px;
  word-break: break-all;
  border-top: 1px solid #d0d7de;
  margin-top: 4px;
}

/* Directory group label in the sidebar (index page) */
#sidebar li.nav-group-label {
  font-size: 11px;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  color: #57606a;
  padding: 10px 16px 2px;
}

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

.type-source-toggle {
  margin-left: auto;
  font-size: 11px;
  font-weight: 500;
  padding: 2px 8px;
  border-radius: 4px;
  border: 1px solid #d0d7de;
  background: #f6f8fa;
  color: #24292f;
  cursor: pointer;
  font-family: inherit;
  transition: background 0.15s;
}
.type-source-toggle:hover {
  background: #eaeef2;
}

.type-source-view {
  margin-top: 8px;
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

/* ── Data section ── */

/* The section heading sits on one line with the toggle button inline */
#data-section h2 {
  display: flex;
  align-items: center;
  gap: 10px;
}

.data-toggle-btn {
  display: inline-block;
  font-size: 12px;
  font-weight: 600;
  padding: 3px 10px;
  border-radius: 4px;
  border: 1px solid #d0d7de;
  background: #f6f8fa;
  color: #24292f;
  cursor: pointer;
  font-family: ui-monospace, "Cascadia Code", "Fira Code", monospace;
  flex-shrink: 0;
}

.data-toggle-btn:hover {
  background: #e1e4e8;
  border-color: #b0b7be;
}

/* SYAML source — always visible, warm tint */
pre.syaml-source {
  background: #fff8f0;
  border: 1px solid #e6d7c3;
  border-radius: 6px;
  padding: 16px;
  overflow-x: auto;
  font-size: 12px;
  line-height: 1.5;
  margin: 0;
  height: 100%;
  box-sizing: border-box;
}

pre.syaml-source code {
  background: none;
  padding: 0;
  font-size: 12px;
}

/* ── SYAML syntax-highlighting token colours ── */
.hl-comment { color: #6e7781; font-style: italic; }
.hl-key     { color: #0550ae; }
.hl-type, a.hl-type { color: #8250df; text-decoration: none; }
a.hl-type:hover { text-decoration: underline; }
.hl-string  { color: #116329; }
.hl-number  { color: #953800; }
.hl-unit    { color: #953800; opacity: 0.75; }
.hl-keyword { color: #cf222e; font-weight: 600; }
.hl-op      { color: #57606a; }
.hl-value   { color: #24292f; }

/* Compiled JSON panel — toggled, cool grey */
#json-content pre {
  background: #f6f8fa;
  border: 1px solid #d0d7de;
  border-radius: 6px;
  padding: 16px;
  overflow-x: auto;
  font-size: 12px;
  line-height: 1.5;
}

#json-content code {
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
                if raw.starts_with("http://") || raw.starts_with("https://") || raw.starts_with('@')
                {
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
        SyamlError::ImportError(format!("failed to read directory '{}': {e}", dir.display()))
    })?;

    for entry in read_dir {
        let entry = entry
            .map_err(|e| SyamlError::ImportError(format!("failed to read directory entry: {e}")))?;
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

/// Return the longest common ancestor directory shared by all given paths and the base.
///
/// For example, given `base = /a/b/c` and paths `[/a/b/d/foo.syaml, /a/b/e/bar.syaml]`,
/// the common ancestor is `/a/b`.
fn common_ancestor(base: &Path, paths: &[PathBuf]) -> PathBuf {
    let mut ancestor: Vec<std::path::Component> = base.components().collect();
    for path in paths {
        let path_components: Vec<std::path::Component> = path.components().collect();
        let common_len = ancestor
            .iter()
            .zip(path_components.iter())
            .take_while(|(a, b)| a == b)
            .count();
        ancestor.truncate(common_len);
    }
    ancestor.iter().collect()
}

/// Generate a multi-file HTML documentation site.
/// Returns a map of relative output path (e.g. "files/module.html") → HTML content.
/// Always includes an "index.html" entry.
///
/// All paths in `roots` and `base_dir` should be canonical (absolute) so that
/// files from different modules can be correctly placed into namespace-aware
/// subdirectories in the output.  The effective base used for stripping prefixes
/// is the common ancestor of `base_dir` and all root paths, which means that
/// cross-module imports (e.g. `primitives/`, `time/`) land in their own
/// subdirectories instead of colliding with names from the primary module.
pub fn generate_html_docs_site(
    roots: &[PathBuf],
    base_dir: &Path,
) -> Result<BTreeMap<String, String>, SyamlError> {
    let mut site: BTreeMap<String, String> = BTreeMap::new();

    // Canonicalize base_dir so it matches the canonical roots even on systems
    // where the temp/working directory has symlinks (e.g. macOS /tmp → /private/tmp).
    let canonical_base = fs::canonicalize(base_dir).unwrap_or_else(|_| base_dir.to_path_buf());

    // Deduplicate roots by canonical path (callers should already pass canonical paths,
    // but guard here for safety).
    let unique_roots: Vec<PathBuf> = {
        let mut seen: BTreeMap<PathBuf, ()> = BTreeMap::new();
        let mut ordered: Vec<PathBuf> = Vec::new();
        for p in roots {
            let canonical = fs::canonicalize(p).unwrap_or_else(|_| p.clone());
            if seen.insert(canonical.clone(), ()).is_none() {
                ordered.push(canonical);
            }
        }
        ordered
    };

    // The effective base is the common ancestor of canonical_base and all root paths.
    // This naturally expands to cover sibling-module files pulled in via --follow-imports.
    let effective_base = common_ancestor(&canonical_base, &unique_roots);

    // Map: canonical file path → relative output HTML path (e.g. "files/module.html")
    let mut path_to_rel_html: BTreeMap<PathBuf, String> = BTreeMap::new();

    // Compute relative output paths for each root
    for canonical in &unique_roots {
        let rel = canonical
            .strip_prefix(&effective_base)
            .unwrap_or(canonical.as_path());
        let html_rel = rel.with_extension("html").to_string_lossy().into_owned();
        path_to_rel_html.insert(canonical.clone(), html_rel);
    }

    // Group html_rel paths by parent directory so each page can show a
    // "Files" sidebar section linking to its module siblings.
    let mut dir_to_pages: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for html_rel in path_to_rel_html.values() {
        let dir = Path::new(html_rel)
            .parent()
            .and_then(|p| if p == Path::new("") { None } else { p.to_str() })
            .unwrap_or("");
        if !dir.is_empty() {
            dir_to_pages
                .entry(dir.to_string())
                .or_default()
                .push(html_rel.clone());
        }
    }
    for pages in dir_to_pages.values_mut() {
        pages.sort();
    }

    // Generate HTML for each file
    let mut index_links: Vec<(String, String)> = Vec::new(); // (html_rel_path, title)

    // Iterate over unique_roots (already deduplicated canonical paths).
    for file_path in &unique_roots {
        let html_rel = match path_to_rel_html.get(file_path) {
            Some(r) => r.clone(),
            None => continue,
        };

        let content = fs::read_to_string(file_path).map_err(|e| {
            SyamlError::ImportError(format!("failed to read '{}': {e}", file_path.display()))
        })?;

        let parsed = parse_document_or_manifest(&content)?;

        // Build a title that includes the directory context so that pages from
        // different modules are clearly distinguishable (e.g. "files / module").
        let file_title_owned: String = {
            let stem = file_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("doc");
            let dir = Path::new(&html_rel).parent().and_then(|p| {
                if p == Path::new("") {
                    None
                } else {
                    p.to_str()
                }
            });
            match dir {
                Some(d) => format!("{d} / {stem}"),
                None => stem.to_string(),
            }
        };
        let file_title = file_title_owned.as_str();

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
                        // Resolve the import relative to this (canonical) file's directory.
                        let file_dir = file_path.parent().unwrap_or(Path::new("."));
                        let resolved = file_dir.join(raw);
                        let resolved_canonical = fs::canonicalize(&resolved).unwrap_or(resolved);

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
        let data_opt = if is_empty_data(data_val) {
            None
        } else {
            Some(data_val)
        };
        let raw_data = extract_raw_data_section(&content);
        let raw_schema = extract_raw_schema_section(&content);

        // Compute the relative path from this page back to index.html.
        let index_href = compute_relative_html_path(&current_html_dir, Path::new("index.html"));

        // Build the "Files" sidebar list: sibling pages in the same module directory.
        // Each entry is (relative_href, display_stem); href is empty for the current page.
        let page_dir = Path::new(&html_rel)
            .parent()
            .and_then(|p| if p == Path::new("") { None } else { p.to_str() })
            .unwrap_or("")
            .to_string();
        let module_files: Vec<(String, String)> = dir_to_pages
            .get(&page_dir)
            .map(|siblings| {
                siblings
                    .iter()
                    .map(|sibling_rel| {
                        let stem = Path::new(sibling_rel)
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or(sibling_rel.as_str())
                            .to_string();
                        let href = if sibling_rel == &html_rel {
                            String::new() // current page — no link
                        } else {
                            // Same directory: just the filename is the relative href.
                            Path::new(sibling_rel)
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or(sibling_rel.as_str())
                                .to_string()
                        };
                        (href, stem)
                    })
                    .collect()
            })
            .unwrap_or_default();

        let html = assemble_page_with_import_links(
            file_title,
            parsed.meta.as_ref(),
            &parsed.schema,
            parsed.functional.as_ref(),
            data_opt,
            &import_html_paths,
            Some(&index_href),
            raw_data.as_deref(),
            raw_schema.as_deref(),
            &module_files,
        );

        // Use the full relative path (without .html) as the index label so that
        // "files/module" and "primitives/module" are distinguishable.
        let index_label = Path::new(&html_rel)
            .with_extension("")
            .to_string_lossy()
            .into_owned();
        index_links.push((html_rel.clone(), index_label));
        site.insert(html_rel, html);
    }

    // Generate index.html
    let index_html = generate_index_page(&index_links);
    site.insert("index.html".to_string(), index_html);

    Ok(site)
}

/// Assemble a page with an explicit `import_html_paths` map (alias → href).
/// `index_href` is the relative path from this page back to index.html; when
/// `Some`, a home icon link is rendered at the top of the sidebar.
/// `raw_data` is the verbatim `---data` section body from the source file.
fn assemble_page_with_import_links(
    title: &str,
    meta: Option<&Meta>,
    schema: &SchemaDoc,
    functional: Option<&FunctionalDoc>,
    data: Option<&JsonValue>,
    import_html_paths: &BTreeMap<String, String>,
    index_href: Option<&str>,
    raw_data: Option<&str>,
    raw_schema: Option<&str>,
    module_files: &[(String, String)], // (relative_href, display_stem); href="" = current page
) -> String {
    let type_sources: BTreeMap<String, String> = raw_schema
        .map(extract_schema_type_blocks)
        .unwrap_or_default();

    let meta_html = meta
        .map(|m| render_meta_section(m, import_html_paths))
        .unwrap_or_default();
    let schema_html = render_schema_section(schema, import_html_paths, &type_sources);
    let functional_html = functional
        .map(|f| render_functional_section(f, import_html_paths))
        .unwrap_or_default();

    // Only render data section if the value is non-null and non-empty.
    // Pass import_html_paths so type hints in the SYAML source become links.
    let data_html = data
        .filter(|v| !is_empty_data(v))
        .map(|v| render_data_section(v, raw_data, import_html_paths))
        .unwrap_or_default();

    let nav_items = build_nav_items(schema, functional, data.filter(|v| !is_empty_data(v)));

    let home_link = match index_href {
        Some(href) => format!(
            r#"  <a href="{href}" class="nav-home" title="Documentation Index">
    <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3 9l9-7 9 7v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/><polyline points="9 22 9 12 15 12 15 22"/></svg>
    Index
  </a>
"#,
            href = html_escape(href)
        ),
        None => String::new(),
    };

    // "Files" sidebar section — links to sibling pages in the same module directory.
    let files_nav = if !module_files.is_empty() {
        let mut items = String::new();
        for (href, name) in module_files {
            if href.is_empty() {
                items.push_str(&format!(
                    "    <li class=\"nav-files-current\">{name}</li>\n",
                    name = html_escape(name)
                ));
            } else {
                items.push_str(&format!(
                    "    <li><a href=\"{href}\">{name}</a></li>\n",
                    href = html_escape(href),
                    name = html_escape(name)
                ));
            }
        }
        format!(
            "  <div class=\"nav-section\">Files</div>\n  <ul class=\"nav-files\">\n{items}  </ul>\n",
            items = items
        )
    } else {
        String::new()
    };

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
{home_link}{files_nav}  <div class="nav-section">On this page</div>
  <div class="nav-title-sub">{title}</div>
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
function toggleJsonView() {{
  var syaml = document.getElementById('syaml-content');
  var json  = document.getElementById('json-content');
  var btn   = document.getElementById('json-toggle');
  var showingJson = json.style.display !== 'none';
  if (showingJson) {{
    json.style.display  = 'none';
    syaml.style.display = 'block';
    btn.textContent = 'Show JSON';
  }} else {{
    syaml.style.display = 'none';
    json.style.display  = 'block';
    btn.textContent = 'Show SYAML';
  }}
}}
function toggleTypeSource(id) {{
  var rendered = document.getElementById('type-rendered-' + id);
  var source   = document.getElementById('type-source-'   + id);
  var btn      = document.getElementById('type-toggle-'   + id);
  var showingSource = source.style.display !== 'none';
  if (showingSource) {{
    source.style.display   = 'none';
    rendered.style.display = 'block';
    btn.textContent = 'Show source';
  }} else {{
    rendered.style.display = 'none';
    source.style.display   = 'block';
    btn.textContent = 'Show rendered';
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
    // Sort links so they appear in a stable, alphabetical order.
    let mut sorted_links: Vec<&(String, String)> = links.iter().collect();
    sorted_links.sort_by(|a, b| a.0.cmp(&b.0));

    // Group by parent directory (the module namespace).
    // Links at the root (no parent dir) go under an empty-string group.
    let mut groups: Vec<(String, Vec<&(String, String)>)> = Vec::new();
    for link in &sorted_links {
        let dir = Path::new(&link.0)
            .parent()
            .and_then(|p| if p == Path::new("") { None } else { p.to_str() })
            .unwrap_or("")
            .to_string();
        if let Some(last) = groups.last_mut() {
            if last.0 == dir {
                last.1.push(link);
                continue;
            }
        }
        groups.push((dir, vec![link]));
    }

    // Sidebar: flat list showing the leaf filename for brevity with the directory as a prefix label.
    let mut nav_links = String::new();
    let mut prev_dir = String::new();
    for link in &sorted_links {
        let dir = Path::new(&link.0)
            .parent()
            .and_then(|p| if p == Path::new("") { None } else { p.to_str() })
            .unwrap_or("")
            .to_string();
        let stem = Path::new(&link.0)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&link.1);
        if dir != prev_dir {
            if !dir.is_empty() {
                nav_links.push_str(&format!(
                    "    <li class=\"nav-group-label\">{}</li>\n",
                    html_escape(&dir)
                ));
            }
            prev_dir = dir;
        }
        nav_links.push_str(&format!(
            "    <li><a href=\"{href}\">{stem}</a></li>\n",
            href = html_escape(&link.0),
            stem = html_escape(stem),
        ));
    }

    // Main content: one section per module directory.
    let mut sections = String::new();
    for (dir, group_links) in &groups {
        let heading = if dir.is_empty() {
            "Root".to_string()
        } else {
            html_escape(dir)
        };
        let mut items = String::new();
        for (href, label) in group_links {
            let stem = Path::new(href)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(label);
            items.push_str(&format!(
                "      <li><a href=\"{href}\">{stem}</a></li>\n",
                href = html_escape(href),
                stem = html_escape(stem),
            ));
        }
        sections.push_str(&format!(
            "  <section class=\"doc-section\">\n    <h2>{heading}</h2>\n    <ul>\n{items}    </ul>\n  </section>\n"
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
{nav_links}  </ul>
</nav>
<main>
  <h1 class="page-title">Documentation Index</h1>
{sections}</main>
</body>
</html>
"#,
        css = inline_css(),
        nav_links = nav_links,
        sections = sections,
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
        assert!(
            html.contains("id=\"type-Money\""),
            "should contain Money anchor"
        );
        assert!(
            html.contains("id=\"type-Port\""),
            "should contain Port anchor"
        );
        assert!(
            html.contains("href=\"#type-Money\""),
            "nav should link to Money"
        );
        assert!(
            html.contains("<td><code>amount</code>"),
            "should list amount field"
        );
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
            html.contains("id=\"json-toggle\""),
            "should contain json-toggle button"
        );
        assert!(
            html.contains("Show JSON"),
            "button should initially say 'Show JSON'"
        );

        // Hidden JSON div is present and starts hidden
        assert!(
            html.contains("id=\"json-content\""),
            "should contain json-content div"
        );
        assert!(
            html.contains("style=\"display:none\""),
            "json-content should start hidden"
        );

        // Raw SYAML is visible by default; JSON panel is swapped in on toggle
        assert!(
            html.contains("id=\"syaml-content\""),
            "should contain syaml-content wrapper"
        );
        assert!(
            html.contains("class=\"syaml-source\""),
            "should contain syaml-source pre block"
        );

        // Data values appear (in SYAML block and/or JSON block)
        assert!(
            html.contains("localhost"),
            "should include data value 'localhost'"
        );
        assert!(html.contains("8080"), "should include data value '8080'");

        // JS toggle function is included
        assert!(
            html.contains("function toggleJsonView()"),
            "should include toggleJsonView JS function"
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
        assert!(
            site["index.html"].contains("types"),
            "index should link to types"
        );
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

        assert!(
            names.iter().any(|n| n == "invoice.syaml"),
            "should include invoice.syaml"
        );
        assert!(
            names.iter().any(|n| n == "refund.syaml"),
            "should include refund.syaml"
        );
        assert!(
            !names.iter().any(|n| n == "module.syaml"),
            "should exclude module.syaml"
        );
    }
}
