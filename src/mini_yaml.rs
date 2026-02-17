use serde_json::{Map as JsonMap, Number as JsonNumber, Value as JsonValue};

use crate::error::SyamlError;

pub fn parse_document(input: &str) -> Result<JsonValue, SyamlError> {
    let lines: Vec<Line<'_>> = input
        .lines()
        .enumerate()
        .map(|(i, raw)| Line { number: i + 1, raw })
        .collect();

    let mut idx = 0usize;
    while idx < lines.len() && is_ignorable(lines[idx].raw) {
        idx += 1;
    }

    if idx >= lines.len() {
        return Ok(JsonValue::Object(JsonMap::new()));
    }

    let indent = leading_spaces(lines[idx].raw);
    parse_block(&lines, &mut idx, indent)
}

pub fn parse_scalar(input: &str) -> Result<JsonValue, SyamlError> {
    parse_inline_value(input.trim())
}

#[derive(Clone, Copy)]
struct Line<'a> {
    number: usize,
    raw: &'a str,
}

fn parse_block(
    lines: &[Line<'_>],
    idx: &mut usize,
    indent: usize,
) -> Result<JsonValue, SyamlError> {
    while *idx < lines.len() && is_ignorable(lines[*idx].raw) {
        *idx += 1;
    }

    if *idx >= lines.len() {
        return Ok(JsonValue::Object(JsonMap::new()));
    }

    let line = lines[*idx];
    let current_indent = leading_spaces(line.raw);
    if current_indent < indent {
        return Ok(JsonValue::Object(JsonMap::new()));
    }
    if current_indent > indent {
        return Err(SyamlError::YamlParseError {
            section: "unknown".to_string(),
            message: format!(
                "unexpected indentation at line {}: expected {}, found {}",
                line.number, indent, current_indent
            ),
        });
    }

    let trimmed = line.raw[indent..].trim_start();
    if trimmed.starts_with("- ") {
        parse_sequence(lines, idx, indent)
    } else if has_unquoted_colon(trimmed) {
        parse_mapping(lines, idx, indent)
    } else {
        let value = parse_inline_value(trimmed)?;
        *idx += 1;
        Ok(value)
    }
}

fn parse_mapping(
    lines: &[Line<'_>],
    idx: &mut usize,
    indent: usize,
) -> Result<JsonValue, SyamlError> {
    let mut map = JsonMap::new();

    while *idx < lines.len() {
        if is_ignorable(lines[*idx].raw) {
            *idx += 1;
            continue;
        }

        let line = lines[*idx];
        let current_indent = leading_spaces(line.raw);
        if current_indent < indent {
            break;
        }
        if current_indent > indent {
            return Err(SyamlError::YamlParseError {
                section: "unknown".to_string(),
                message: format!(
                    "unexpected indentation in mapping at line {}: expected {}",
                    line.number, indent
                ),
            });
        }

        let trimmed = line.raw[indent..].trim_start();
        if trimmed.starts_with("- ") {
            return Err(SyamlError::YamlParseError {
                section: "unknown".to_string(),
                message: format!("mixed sequence/mapping at line {}", line.number),
            });
        }

        let colon = find_unquoted_colon(trimmed).ok_or_else(|| SyamlError::YamlParseError {
            section: "unknown".to_string(),
            message: format!("expected key:value at line {}", line.number),
        })?;

        let key_raw = trimmed[..colon].trim();
        let key = parse_key(key_raw)?;
        if map.contains_key(&key) {
            return Err(SyamlError::YamlParseError {
                section: "unknown".to_string(),
                message: format!("duplicate key '{}' at line {}", key, line.number),
            });
        }

        let value_raw = trimmed[colon + 1..].trim_start();
        *idx += 1;

        let value = if value_raw.is_empty() {
            let mut lookahead = *idx;
            while lookahead < lines.len() && is_ignorable(lines[lookahead].raw) {
                lookahead += 1;
            }
            if lookahead >= lines.len() {
                JsonValue::Null
            } else {
                let next_indent = leading_spaces(lines[lookahead].raw);
                if next_indent <= indent {
                    JsonValue::Null
                } else {
                    parse_block(lines, idx, next_indent)?
                }
            }
        } else {
            parse_inline_value(value_raw)?
        };

        map.insert(key, value);
    }

    Ok(JsonValue::Object(map))
}

fn parse_sequence(
    lines: &[Line<'_>],
    idx: &mut usize,
    indent: usize,
) -> Result<JsonValue, SyamlError> {
    let mut items = Vec::new();

    while *idx < lines.len() {
        if is_ignorable(lines[*idx].raw) {
            *idx += 1;
            continue;
        }

        let line = lines[*idx];
        let current_indent = leading_spaces(line.raw);
        if current_indent < indent {
            break;
        }
        if current_indent > indent {
            return Err(SyamlError::YamlParseError {
                section: "unknown".to_string(),
                message: format!(
                    "unexpected indentation in sequence at line {}: expected {}",
                    line.number, indent
                ),
            });
        }

        let trimmed = line.raw[indent..].trim_start();
        if !trimmed.starts_with("- ") {
            break;
        }

        let rest = trimmed[2..].trim_start();
        *idx += 1;

        let value = if rest.is_empty() {
            let mut lookahead = *idx;
            while lookahead < lines.len() && is_ignorable(lines[lookahead].raw) {
                lookahead += 1;
            }
            if lookahead >= lines.len() {
                JsonValue::Null
            } else {
                let next_indent = leading_spaces(lines[lookahead].raw);
                if next_indent <= indent {
                    JsonValue::Null
                } else {
                    parse_block(lines, idx, next_indent)?
                }
            }
        } else {
            parse_inline_value(rest)?
        };

        items.push(value);
    }

    Ok(JsonValue::Array(items))
}

fn parse_inline_value(raw: &str) -> Result<JsonValue, SyamlError> {
    let s = raw.trim();
    if s.is_empty() {
        return Ok(JsonValue::Null);
    }

    if s.starts_with('"') || s.starts_with('\'') {
        return Ok(JsonValue::String(parse_quoted_string(s)?));
    }

    if s.starts_with('{') {
        return parse_inline_object(s);
    }

    if s.starts_with('[') {
        return parse_inline_array(s);
    }

    if s == "true" {
        return Ok(JsonValue::Bool(true));
    }
    if s == "false" {
        return Ok(JsonValue::Bool(false));
    }
    if s == "null" || s == "~" {
        return Ok(JsonValue::Null);
    }

    if let Ok(v) = s.parse::<i64>() {
        return Ok(JsonValue::Number(JsonNumber::from(v)));
    }
    if let Ok(v) = s.parse::<u64>() {
        return Ok(JsonValue::Number(JsonNumber::from(v)));
    }
    if let Ok(v) = s.parse::<f64>() {
        if let Some(n) = JsonNumber::from_f64(v) {
            return Ok(JsonValue::Number(n));
        }
    }

    Ok(JsonValue::String(strip_inline_comment(s).to_string()))
}

fn parse_inline_object(raw: &str) -> Result<JsonValue, SyamlError> {
    let Some(inner) = raw
        .trim()
        .strip_prefix('{')
        .and_then(|v| v.strip_suffix('}'))
    else {
        return Err(SyamlError::YamlParseError {
            section: "unknown".to_string(),
            message: format!("invalid inline object syntax '{}': missing braces", raw),
        });
    };

    let parts = split_top_level(inner, ',');
    let mut map = JsonMap::new();

    for part in parts {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        let colon = find_unquoted_colon(p).ok_or_else(|| SyamlError::YamlParseError {
            section: "unknown".to_string(),
            message: format!("invalid inline object entry '{}': expected ':'", p),
        })?;

        let key = parse_key(p[..colon].trim())?;
        let value = parse_inline_value(p[colon + 1..].trim())?;
        map.insert(key, value);
    }

    Ok(JsonValue::Object(map))
}

fn parse_inline_array(raw: &str) -> Result<JsonValue, SyamlError> {
    let Some(inner) = raw
        .trim()
        .strip_prefix('[')
        .and_then(|v| v.strip_suffix(']'))
    else {
        return Err(SyamlError::YamlParseError {
            section: "unknown".to_string(),
            message: format!("invalid inline array syntax '{}': missing brackets", raw),
        });
    };

    let parts = split_top_level(inner, ',');
    let mut items = Vec::new();
    for part in parts {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        items.push(parse_inline_value(p)?);
    }

    Ok(JsonValue::Array(items))
}

fn split_top_level(input: &str, delimiter: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut depth_brace = 0i32;
    let mut depth_bracket = 0i32;
    let mut in_single = false;
    let mut in_double = false;
    let mut escape = false;

    for ch in input.chars() {
        if in_double && escape {
            current.push(ch);
            escape = false;
            continue;
        }

        if in_double && ch == '\\' {
            current.push(ch);
            escape = true;
            continue;
        }

        match ch {
            '\'' if !in_double => {
                in_single = !in_single;
                current.push(ch);
            }
            '"' if !in_single => {
                in_double = !in_double;
                current.push(ch);
            }
            '{' if !in_single && !in_double => {
                depth_brace += 1;
                current.push(ch);
            }
            '}' if !in_single && !in_double => {
                depth_brace -= 1;
                current.push(ch);
            }
            '[' if !in_single && !in_double => {
                depth_bracket += 1;
                current.push(ch);
            }
            ']' if !in_single && !in_double => {
                depth_bracket -= 1;
                current.push(ch);
            }
            c if c == delimiter
                && !in_single
                && !in_double
                && depth_brace == 0
                && depth_bracket == 0 =>
            {
                out.push(current);
                current = String::new();
            }
            _ => current.push(ch),
        }
    }

    out.push(current);
    out
}

fn parse_key(raw: &str) -> Result<String, SyamlError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(SyamlError::YamlParseError {
            section: "unknown".to_string(),
            message: "empty mapping key".to_string(),
        });
    }

    if trimmed.starts_with('"') || trimmed.starts_with('\'') {
        return parse_quoted_string(trimmed);
    }

    Ok(trimmed.to_string())
}

fn parse_quoted_string(raw: &str) -> Result<String, SyamlError> {
    let mut chars = raw.chars();
    let quote = chars.next().ok_or_else(|| SyamlError::YamlParseError {
        section: "unknown".to_string(),
        message: "empty quoted string".to_string(),
    })?;

    if quote != '"' && quote != '\'' {
        return Err(SyamlError::YamlParseError {
            section: "unknown".to_string(),
            message: format!("invalid quoted string '{}': missing quote", raw),
        });
    }

    let mut out = String::new();
    let mut escaped = false;
    for ch in chars {
        if escaped {
            let actual = match ch {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                '\\' => '\\',
                '"' => '"',
                '\'' => '\'',
                other => other,
            };
            out.push(actual);
            escaped = false;
            continue;
        }

        if quote == '"' && ch == '\\' {
            escaped = true;
            continue;
        }

        if ch == quote {
            return Ok(out);
        }

        out.push(ch);
    }

    Err(SyamlError::YamlParseError {
        section: "unknown".to_string(),
        message: format!(
            "unterminated quoted string '{}': missing closing quote",
            raw
        ),
    })
}

fn has_unquoted_colon(input: &str) -> bool {
    find_unquoted_colon(input).is_some()
}

fn find_unquoted_colon(input: &str) -> Option<usize> {
    let mut in_single = false;
    let mut in_double = false;
    let mut depth_brace = 0i32;
    let mut depth_bracket = 0i32;
    let mut escape = false;

    for (i, ch) in input.char_indices() {
        if in_double && escape {
            escape = false;
            continue;
        }

        if in_double && ch == '\\' {
            escape = true;
            continue;
        }

        match ch {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '{' if !in_single && !in_double => depth_brace += 1,
            '}' if !in_single && !in_double => depth_brace -= 1,
            '[' if !in_single && !in_double => depth_bracket += 1,
            ']' if !in_single && !in_double => depth_bracket -= 1,
            ':' if !in_single && !in_double && depth_brace == 0 && depth_bracket == 0 => {
                return Some(i)
            }
            _ => {}
        }
    }

    None
}

fn strip_inline_comment(input: &str) -> &str {
    let mut in_single = false;
    let mut in_double = false;
    let mut escape = false;

    for (i, ch) in input.char_indices() {
        if in_double && escape {
            escape = false;
            continue;
        }

        if in_double && ch == '\\' {
            escape = true;
            continue;
        }

        match ch {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '#' if !in_single && !in_double => {
                if i == 0 {
                    return "";
                }
                let prev = input[..i].chars().last().unwrap_or(' ');
                if prev.is_whitespace() {
                    return input[..i].trim_end();
                }
            }
            _ => {}
        }
    }

    input
}

fn leading_spaces(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ').count()
}

fn is_ignorable(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.is_empty() || trimmed.starts_with('#')
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::parse_document;

    #[test]
    fn parses_simple_mapping_and_sequence() {
        let input = "name: test\nvalues:\n  - 1\n  - 2\n";
        let parsed = parse_document(input).unwrap();
        assert_eq!(parsed, json!({"name":"test", "values":[1,2]}));
    }

    #[test]
    fn parses_inline_map() {
        let input = "env: { from: env, key: DB_HOST, required: true }\n";
        let parsed = parse_document(input).unwrap();
        assert_eq!(
            parsed,
            json!({"env": {"from": "env", "key": "DB_HOST", "required": true}})
        );
    }
}
