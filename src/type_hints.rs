//! Type-hint extraction and data key normalization.
//!
//! Keys may declare a hint using `key <TypeName>`. During normalization,
//! the key is rewritten to `key` and a hint entry is emitted for that path.

use std::collections::BTreeMap;

use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::error::SyamlError;

const MAX_NORMALIZE_DEPTH: usize = 64;
const MAX_TYPE_HINTS: usize = 10_000;

/// Normalizes a data value and extracts type hints.
///
/// Returns `(normalized_data, hints_by_path)`.
pub fn normalize_data_with_hints(
    value: &JsonValue,
) -> Result<(JsonValue, BTreeMap<String, String>), SyamlError> {
    let mut hints = BTreeMap::new();
    let normalized = normalize_value(value, "$", &mut hints, 0)?;
    Ok((normalized, hints))
}

fn normalize_value(
    value: &JsonValue,
    path: &str,
    hints: &mut BTreeMap<String, String>,
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
                let child_path = format!("{}.{}", path, canonical_key);
                if out.contains_key(&canonical_key) {
                    return Err(SyamlError::TypeHintError(format!(
                        "duplicate canonical key '{}' at {path}",
                        canonical_key
                    )));
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
                    normalize_value(v, &child_path, hints, depth + 1)?,
                );
            }
            Ok(JsonValue::Object(out))
        }
        JsonValue::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                let child_path = format!("{}[{}]", path, i);
                out.push(normalize_value(item, &child_path, hints, depth + 1)?);
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

    if !hint
        .chars()
        .next()
        .map(|c| c.is_ascii_alphabetic() || c == '_')
        .unwrap_or(false)
        || !hint.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Ok((trimmed.to_string(), None));
    }

    Ok((base.to_string(), Some(hint.to_string())))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::normalize_data_with_hints;

    #[test]
    fn extracts_type_hints() {
        let json = json!({"name <string>": "a", "count <integer>": 2});
        let (_data, hints) = normalize_data_with_hints(&json).unwrap();
        assert_eq!(hints.get("$.name").unwrap(), "string");
        assert_eq!(hints.get("$.count").unwrap(), "integer");
    }
}
