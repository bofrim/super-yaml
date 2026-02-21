//! Template expansion for data trees.
//!
//! Supports:
//! - Template invocation keys: `{{namespace.path.to.template}}`
//! - Placeholder values inside template definitions: `{{VAR}}` or `{{VAR:default}}`

use std::collections::{BTreeSet, HashMap};
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::error::SyamlError;

/// Expands template invocations in-place within a data tree.
pub fn expand_data_templates(
    data: &mut JsonValue,
    imports: &HashMap<String, JsonValue>,
) -> Result<(), SyamlError> {
    let root_snapshot = data.clone();
    expand_value(data, &root_snapshot, imports, "$")
}

fn expand_value(
    value: &mut JsonValue,
    root: &JsonValue,
    imports: &HashMap<String, JsonValue>,
    path: &str,
) -> Result<(), SyamlError> {
    match value {
        JsonValue::Object(map) => {
            if let Some((template_ref, vars)) = parse_template_invocation(map, path)? {
                let template_source = resolve_template_path(root, imports, &template_ref, path)?;
                let mut allowed_vars = BTreeSet::new();
                collect_placeholders(template_source, &mut allowed_vars);

                for key in vars.keys() {
                    if !allowed_vars.contains(key) {
                        return Err(SyamlError::TemplateError(format!(
                            "unexpected template variable '{}' at {} for template '{}'; allowed: {}",
                            key,
                            path,
                            template_ref,
                            format_allowed_vars(&allowed_vars)
                        )));
                    }
                }

                let siblings: Vec<(String, JsonValue)> = map
                    .iter()
                    .filter(|(k, _)| parse_template_key(k).is_none())
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();

                let filled = substitute_placeholders(template_source, &vars, path)?;

                if siblings.is_empty() {
                    *value = filled;
                } else {
                    let mut merged = match filled {
                        JsonValue::Object(obj) => obj,
                        _ => {
                            return Err(SyamlError::TemplateError(format!(
                                "template '{}' at {} must expand to an object when used alongside sibling keys",
                                template_ref, path
                            )));
                        }
                    };
                    for (k, v) in siblings {
                        merged.insert(k, v);
                    }
                    *value = JsonValue::Object(merged);
                }

                return expand_value(value, root, imports, path);
            }

            for (key, child) in map.iter_mut() {
                let child_path = format!("{}.{}", path, key);
                expand_value(child, root, imports, &child_path)?;
            }
            Ok(())
        }
        JsonValue::Array(items) => {
            for (idx, child) in items.iter_mut().enumerate() {
                let child_path = format!("{}[{}]", path, idx);
                expand_value(child, root, imports, &child_path)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn parse_template_invocation(
    map: &JsonMap<String, JsonValue>,
    path: &str,
) -> Result<Option<(String, HashMap<String, JsonValue>)>, SyamlError> {
    let mut invocation_key: Option<String> = None;
    let mut raw_key: Option<String> = None;

    for key in map.keys() {
        if let Some(template_ref) = parse_template_key(key) {
            if invocation_key.is_some() {
                return Err(SyamlError::TemplateError(format!(
                    "multiple template invocation keys found at {}; only one is allowed",
                    path
                )));
            }
            invocation_key = Some(template_ref);
            raw_key = Some(key.clone());
        }
    }

    let Some(template_ref) = invocation_key else {
        return Ok(None);
    };

    let raw_vars = map
        .get(raw_key.as_ref().unwrap())
        .expect("template key exists in map")
        .as_object()
        .ok_or_else(|| {
            SyamlError::TemplateError(format!(
                "template invocation at {} must map to an object of variable values",
                path
            ))
        })?;

    let vars = raw_vars
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect::<HashMap<_, _>>();

    Ok(Some((template_ref, vars)))
}

fn resolve_template_path<'a>(
    root: &'a JsonValue,
    imports: &'a HashMap<String, JsonValue>,
    template_ref: &str,
    usage_path: &str,
) -> Result<&'a JsonValue, SyamlError> {
    let mut segments = template_ref.split('.');
    let Some(first) = segments.next() else {
        return Err(SyamlError::TemplateError(format!(
            "invalid template reference '{}' at {}",
            template_ref, usage_path
        )));
    };
    if first.is_empty() {
        return Err(SyamlError::TemplateError(format!(
            "invalid template reference '{}' at {}",
            template_ref, usage_path
        )));
    }

    let mut current = if let Some(import_root) = imports.get(first) {
        import_root
    } else {
        root.as_object()
            .and_then(|obj| obj.get(first))
            .ok_or_else(|| {
                SyamlError::TemplateError(format!(
                    "template '{}' referenced at {} was not found",
                    template_ref, usage_path
                ))
            })?
    };

    for segment in segments {
        if segment.is_empty() {
            return Err(SyamlError::TemplateError(format!(
                "invalid template reference '{}' at {}",
                template_ref, usage_path
            )));
        }
        current = current
            .as_object()
            .and_then(|obj| obj.get(segment))
            .ok_or_else(|| {
                SyamlError::TemplateError(format!(
                    "template '{}' referenced at {} was not found",
                    template_ref, usage_path
                ))
            })?;
    }
    Ok(current)
}

fn collect_placeholders(value: &JsonValue, out: &mut BTreeSet<String>) {
    match value {
        JsonValue::Object(map) => {
            for child in map.values() {
                collect_placeholders(child, out);
            }
        }
        JsonValue::Array(items) => {
            for child in items {
                collect_placeholders(child, out);
            }
        }
        JsonValue::String(text) => {
            for placeholder in parse_placeholders_in_text(text) {
                out.insert(placeholder.name);
            }
        }
        _ => {}
    }
}

fn substitute_placeholders(
    value: &JsonValue,
    vars: &HashMap<String, JsonValue>,
    path: &str,
) -> Result<JsonValue, SyamlError> {
    match value {
        JsonValue::Object(map) => {
            let mut out = JsonMap::new();
            for (key, child) in map {
                let child_path = format!("{}.{}", path, key);
                out.insert(
                    key.clone(),
                    substitute_placeholders(child, vars, &child_path)?,
                );
            }
            Ok(JsonValue::Object(out))
        }
        JsonValue::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for (idx, child) in items.iter().enumerate() {
                let child_path = format!("{}[{}]", path, idx);
                out.push(substitute_placeholders(child, vars, &child_path)?);
            }
            Ok(JsonValue::Array(out))
        }
        JsonValue::String(text) => {
            let Some(placeholder) = parse_placeholder(text) else {
                return substitute_placeholders_in_string(text, vars, path);
            };

            if let Some(found) = vars.get(&placeholder.name) {
                return Ok(found.clone());
            }

            if let Some(default_raw) = placeholder.default_raw {
                let default = crate::mini_yaml::parse_scalar(default_raw.trim()).map_err(|_| {
                    SyamlError::TemplateError(format!(
                        "invalid default value '{}' for template variable '{}' at {}",
                        default_raw, placeholder.name, path
                    ))
                })?;
                return Ok(default);
            }

            Err(SyamlError::TemplateError(format!(
                "missing required template variable '{}' at {}",
                placeholder.name, path
            )))
        }
        _ => Ok(value.clone()),
    }
}

fn substitute_placeholders_in_string(
    text: &str,
    vars: &HashMap<String, JsonValue>,
    path: &str,
) -> Result<JsonValue, SyamlError> {
    let mut rendered = String::with_capacity(text.len());
    let mut cursor = 0usize;
    let mut found_any = false;

    for captures in placeholder_scan_regex().captures_iter(text) {
        let Some(full) = captures.get(0) else {
            continue;
        };
        let Some(name_match) = captures.get(1) else {
            continue;
        };
        found_any = true;

        rendered.push_str(&text[cursor..full.start()]);

        let name = name_match.as_str();
        let default_raw = captures.get(2).map(|m| m.as_str().to_string());
        let replacement = resolve_placeholder_value(name, default_raw, vars, path)?;
        rendered.push_str(&replacement);

        cursor = full.end();
    }

    if !found_any {
        return Ok(JsonValue::String(text.to_string()));
    }

    rendered.push_str(&text[cursor..]);
    Ok(JsonValue::String(rendered))
}

fn resolve_placeholder_value(
    name: &str,
    default_raw: Option<String>,
    vars: &HashMap<String, JsonValue>,
    path: &str,
) -> Result<String, SyamlError> {
    if let Some(found) = vars.get(name) {
        return json_value_to_template_string(found, name, path);
    }

    if let Some(default_raw) = default_raw {
        let default = crate::mini_yaml::parse_scalar(default_raw.trim()).map_err(|_| {
            SyamlError::TemplateError(format!(
                "invalid default value '{}' for template variable '{}' at {}",
                default_raw, name, path
            ))
        })?;
        return json_value_to_template_string(&default, name, path);
    }

    Err(SyamlError::TemplateError(format!(
        "missing required template variable '{}' at {}",
        name, path
    )))
}

fn json_value_to_template_string(
    value: &JsonValue,
    placeholder_name: &str,
    path: &str,
) -> Result<String, SyamlError> {
    match value {
        JsonValue::Null => Ok("null".to_string()),
        JsonValue::Bool(v) => Ok(v.to_string()),
        JsonValue::Number(v) => Ok(v.to_string()),
        JsonValue::String(v) => Ok(v.clone()),
        JsonValue::Array(_) | JsonValue::Object(_) => Err(SyamlError::TemplateError(format!(
            "template variable '{}' at {} must be scalar when used inside a larger string",
            placeholder_name, path
        ))),
    }
}

fn format_allowed_vars(vars: &BTreeSet<String>) -> String {
    if vars.is_empty() {
        "(none)".to_string()
    } else {
        vars.iter().cloned().collect::<Vec<_>>().join(", ")
    }
}

fn parse_template_key(key: &str) -> Option<String> {
    let captures = template_key_regex().captures(key.trim())?;
    captures.get(1).map(|m| m.as_str().trim().to_string())
}

#[derive(Debug)]
struct Placeholder {
    name: String,
    default_raw: Option<String>,
}

fn parse_placeholder(text: &str) -> Option<Placeholder> {
    let captures = placeholder_regex().captures(text.trim())?;
    let name = captures.get(1)?.as_str().to_string();
    let default_raw = captures.get(2).map(|m| m.as_str().to_string());
    Some(Placeholder { name, default_raw })
}

fn parse_placeholders_in_text(text: &str) -> Vec<Placeholder> {
    placeholder_scan_regex()
        .captures_iter(text)
        .filter_map(|captures| {
            let name = captures.get(1)?.as_str().to_string();
            let default_raw = captures.get(2).map(|m| m.as_str().to_string());
            Some(Placeholder { name, default_raw })
        })
        .collect()
}

fn template_key_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^\{\{\s*([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*)\s*\}\}$")
            .expect("valid template key regex")
    })
}

fn placeholder_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^\{\{\s*([A-Za-z_][A-Za-z0-9_]*)(?::(.*))?\s*\}\}$")
            .expect("valid placeholder regex")
    })
}

fn placeholder_scan_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\{\{\s*([A-Za-z_][A-Za-z0-9_]*)(?::(.*?))?\s*\}\}")
            .expect("valid placeholder scan regex")
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use super::expand_data_templates;

    #[test]
    fn expands_invocation_and_defaults() {
        let mut data = json!({
            "tpl": {
                "templates": {
                    "service": {
                        "host": "{{HOST}}",
                        "port": "{{PORT:8080}}",
                        "tls": "{{TLS:false}}"
                    }
                }
            },
            "service": {
                "{{tpl.templates.service}}": {
                    "HOST": "api.internal"
                }
            }
        });

        let imports = HashMap::new();
        expand_data_templates(&mut data, &imports).unwrap();
        assert_eq!(data["service"]["host"], json!("api.internal"));
        assert_eq!(data["service"]["port"], json!(8080));
        assert_eq!(data["service"]["tls"], json!(false));
    }

    #[test]
    fn rejects_unknown_var() {
        let mut data = json!({
            "tpl": { "templates": { "x": { "a": "{{A}}" } } },
            "item": { "{{tpl.templates.x}}": { "A": 1, "B": 2 } }
        });

        let imports = HashMap::new();
        let err = expand_data_templates(&mut data, &imports)
            .unwrap_err()
            .to_string();
        assert!(err.contains("unexpected template variable 'B'"));
    }

    #[test]
    fn merges_sibling_keys_with_template_expansion() {
        let mut data = json!({
            "tpl": {
                "templates": {
                    "service": {
                        "host": "{{HOST}}",
                        "port": "{{PORT:8080}}",
                        "tls": "{{TLS:false}}"
                    }
                }
            },
            "service": {
                "{{tpl.templates.service}}": {
                    "HOST": "api.internal"
                },
                "replicas": 3,
                "debug": false
            }
        });

        let imports = HashMap::new();
        expand_data_templates(&mut data, &imports).unwrap();
        assert_eq!(data["service"]["host"], json!("api.internal"));
        assert_eq!(data["service"]["port"], json!(8080));
        assert_eq!(data["service"]["tls"], json!(false));
        assert_eq!(data["service"]["replicas"], json!(3));
        assert_eq!(data["service"]["debug"], json!(false));
    }

    #[test]
    fn sibling_keys_override_template_values() {
        let mut data = json!({
            "_templates": {
                "base": {
                    "host": "{{HOST}}",
                    "port": "{{PORT:8080}}",
                    "tls": "{{TLS:false}}"
                }
            },
            "service": {
                "{{_templates.base}}": {
                    "HOST": "default.internal"
                },
                "port": 9090
            }
        });

        let imports = HashMap::new();
        expand_data_templates(&mut data, &imports).unwrap();
        assert_eq!(data["service"]["host"], json!("default.internal"));
        assert_eq!(data["service"]["port"], json!(9090));
        assert_eq!(data["service"]["tls"], json!(false));
    }

    #[test]
    fn rejects_non_object_template_with_siblings() {
        let mut data = json!({
            "_templates": {
                "items": ["a", "b", "c"]
            },
            "result": {
                "{{_templates.items}}": {},
                "extra": true
            }
        });

        let imports = HashMap::new();
        let err = expand_data_templates(&mut data, &imports)
            .unwrap_err()
            .to_string();
        assert!(err.contains("must expand to an object when used alongside sibling keys"));
    }

    #[test]
    fn interpolates_placeholders_inside_larger_string_values() {
        let mut data = json!({
            "_templates": {
                "deploy": {
                    "path": "{{DEPLOY_ROOT}}/{{APP_NAME}}",
                    "unit": "templates/{{APP_NAME}}.service.j2",
                    "url": "https://{{HOST}}:{{PORT:443}}/health"
                }
            },
            "service": {
                "{{_templates.deploy}}": {
                    "DEPLOY_ROOT": "/srv/apps",
                    "APP_NAME": "api",
                    "HOST": "api.internal"
                }
            }
        });

        let imports = HashMap::new();
        expand_data_templates(&mut data, &imports).unwrap();
        assert_eq!(data["service"]["path"], json!("/srv/apps/api"));
        assert_eq!(data["service"]["unit"], json!("templates/api.service.j2"));
        assert_eq!(data["service"]["url"], json!("https://api.internal:443/health"));
    }
}
