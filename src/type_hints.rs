//! Type-hint extraction and data key normalization.
//!
//! Keys may declare a hint using `key <TypeName>`. During normalization,
//! the key is rewritten to `key` and a hint entry is emitted for that path.
//! Keys may also be frozen using a `^` suffix, e.g. `key^` or `key <Type>^`.

use std::collections::BTreeMap;

use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::error::SyamlError;

const MAX_NORMALIZE_DEPTH: usize = 64;
const MAX_TYPE_HINTS: usize = 10_000;

/// Normalizes a data value and extracts type hints and freeze markers.
///
/// Returns `(normalized_data, hints_by_path, freeze_markers_by_path)`.
pub fn normalize_data_with_hints(
    value: &JsonValue,
) -> Result<(JsonValue, BTreeMap<String, String>, BTreeMap<String, bool>), SyamlError> {
    let mut hints = BTreeMap::new();
    let mut freeze_markers = BTreeMap::new();
    let normalized = normalize_value(value, "$", &mut hints, &mut freeze_markers, 0)?;
    Ok((normalized, hints, freeze_markers))
}

fn normalize_value(
    value: &JsonValue,
    path: &str,
    hints: &mut BTreeMap<String, String>,
    freeze_markers: &mut BTreeMap<String, bool>,
    depth: usize,
) -> Result<JsonValue, SyamlError> {
    if depth > MAX_NORMALIZE_DEPTH {
        return Err(SyamlError::TypeHintError(format!(
            "data nesting depth exceeds maximum ({MAX_NORMALIZE_DEPTH}) at {path}"
        )));
    }

    match value {
        JsonValue::Object(map) => {
            let mut out = JsonMap::new();
            for (key_str, v) in map {
                let (canonical_key, hint) = split_key_and_hint(key_str)?;
                // Check for freeze marker (^ suffix) on the canonical key
                let (canonical_key, frozen) = split_key_and_freeze(&canonical_key);
                let child_path = format!("{}.{}", path, canonical_key);
                if out.contains_key(&canonical_key) {
                    return Err(SyamlError::TypeHintError(format!(
                        "duplicate canonical key '{}' at {path}",
                        canonical_key
                    )));
                }

                if frozen {
                    freeze_markers.insert(child_path.clone(), true);
                }

                if let Some(t) = hint {
                    if hints.len() >= MAX_TYPE_HINTS {
                        return Err(SyamlError::TypeHintError(format!(
                            "type hint count exceeds maximum ({MAX_TYPE_HINTS})"
                        )));
                    }
                    hints.insert(child_path.clone(), t);
                }

                out.insert(
                    canonical_key,
                    normalize_value(v, &child_path, hints, freeze_markers, depth + 1)?,
                );
            }
            Ok(JsonValue::Object(out))
        }
        JsonValue::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                let child_path = format!("{}[{}]", path, i);
                out.push(normalize_value(item, &child_path, hints, freeze_markers, depth + 1)?);
            }
            Ok(JsonValue::Array(out))
        }
        _ => Ok(value.clone()),
    }
}

fn split_key_and_hint(raw: &str) -> Result<(String, Option<String>), SyamlError> {
    let trimmed = raw.trim();
    if !trimmed.ends_with('>') {
        return Ok((trimmed.to_string(), None));
    }

    let lt = match trimmed.rfind('<') {
        Some(i) => i,
        None => return Ok((trimmed.to_string(), None)),
    };

    if lt == 0 || lt + 1 >= trimmed.len() {
        return Ok((trimmed.to_string(), None));
    }

    let base = trimmed[..lt].trim_end();
    let hint = &trimmed[lt + 1..trimmed.len() - 1];
    if base.is_empty() {
        return Err(SyamlError::TypeHintError(format!(
            "invalid type hint key '{}': missing key name",
            raw
        )));
    }

    if hint.is_empty() {
        return Err(SyamlError::TypeHintError(format!(
            "invalid type hint key '{}': missing type name",
            raw
        )));
    }

    if !is_valid_type_name(hint) {
        return Ok((trimmed.to_string(), None));
    }

    Ok((base.to_string(), Some(hint.to_string())))
}

fn split_key_and_freeze(canonical: &str) -> (String, bool) {
    match canonical.strip_suffix('^') {
        Some(base) if !base.is_empty() => (base.to_string(), true),
        _ => (canonical.to_string(), false),
    }
}

fn is_valid_type_name(hint: &str) -> bool {
    hint.split('.').all(is_valid_type_segment)
}

fn is_valid_type_segment(segment: &str) -> bool {
    let mut chars = segment.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::normalize_data_with_hints;

    #[test]
    fn extracts_type_hints() {
        let json = json!({"name <string>": "a", "count <integer>": 2});
        let (_data, hints, _freeze) = normalize_data_with_hints(&json).unwrap();
        assert_eq!(hints.get("$.name").unwrap(), "string");
        assert_eq!(hints.get("$.count").unwrap(), "integer");
    }

    #[test]
    fn extracts_namespaced_type_hints() {
        let json = json!({"endpoint <shared.Port>": 8080});
        let (_data, hints, _freeze) = normalize_data_with_hints(&json).unwrap();
        assert_eq!(hints.get("$.endpoint").unwrap(), "shared.Port");
    }

    #[test]
    fn extracts_freeze_markers() {
        let json = json!({"name^": "locked"});
        let (data, _hints, freeze) = normalize_data_with_hints(&json).unwrap();
        assert!(freeze.get("$.name").copied().unwrap_or(false));
        // Key should be normalized to "name" without the ^
        assert!(data.as_object().unwrap().contains_key("name"));
    }
}
