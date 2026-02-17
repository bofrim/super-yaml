use serde_json::Value as JsonValue;

pub fn to_yaml_string(value: &JsonValue) -> String {
    let mut out = String::new();
    write_value(value, 0, &mut out);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn write_value(value: &JsonValue, indent: usize, out: &mut String) {
    match value {
        JsonValue::Object(map) => write_object(map, indent, out),
        JsonValue::Array(items) => write_array(items, indent, out),
        _ => {
            push_indent(indent, out);
            out.push_str(&render_scalar(value));
            out.push('\n');
        }
    }
}

fn write_object(map: &serde_json::Map<String, JsonValue>, indent: usize, out: &mut String) {
    if map.is_empty() {
        push_indent(indent, out);
        out.push_str("{}\n");
        return;
    }

    for (key, value) in map {
        push_indent(indent, out);
        out.push_str(&render_key(key));
        match value {
            JsonValue::Object(obj) => {
                if obj.is_empty() {
                    out.push_str(": {}\n");
                } else {
                    out.push_str(":\n");
                    write_object(obj, indent + 2, out);
                }
            }
            JsonValue::Array(arr) => {
                if arr.is_empty() {
                    out.push_str(": []\n");
                } else {
                    out.push_str(":\n");
                    write_array(arr, indent + 2, out);
                }
            }
            _ => {
                out.push_str(": ");
                out.push_str(&render_scalar(value));
                out.push('\n');
            }
        }
    }
}

fn write_array(items: &[JsonValue], indent: usize, out: &mut String) {
    if items.is_empty() {
        push_indent(indent, out);
        out.push_str("[]\n");
        return;
    }

    for item in items {
        match item {
            JsonValue::Object(map) if !map.is_empty() => {
                push_indent(indent, out);
                out.push_str("-\n");
                write_object(map, indent + 2, out);
            }
            JsonValue::Array(arr) if !arr.is_empty() => {
                push_indent(indent, out);
                out.push_str("-\n");
                write_array(arr, indent + 2, out);
            }
            _ => {
                push_indent(indent, out);
                out.push_str("- ");
                match item {
                    JsonValue::Object(map) if map.is_empty() => out.push_str("{}"),
                    JsonValue::Array(arr) if arr.is_empty() => out.push_str("[]"),
                    _ => out.push_str(&render_scalar(item)),
                }
                out.push('\n');
            }
        }
    }
}

fn render_key(input: &str) -> String {
    if is_plain_key(input) {
        input.to_string()
    } else {
        quote_string(input)
    }
}

fn render_scalar(value: &JsonValue) -> String {
    match value {
        JsonValue::Null => "null".to_string(),
        JsonValue::Bool(v) => v.to_string(),
        JsonValue::Number(v) => v.to_string(),
        JsonValue::String(v) => render_string(v),
        JsonValue::Array(_) | JsonValue::Object(_) => unreachable!("handled by callers"),
    }
}

fn render_string(input: &str) -> String {
    if is_plain_string(input) {
        input.to_string()
    } else {
        quote_string(input)
    }
}

fn quote_string(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 2);
    out.push('"');
    for ch in input.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn is_plain_key(input: &str) -> bool {
    if input.is_empty() {
        return false;
    }
    input
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

fn is_plain_string(input: &str) -> bool {
    if input.is_empty() {
        return false;
    }
    if input.trim() != input {
        return false;
    }

    let reserved = ["true", "false", "null", "~"];
    if reserved.contains(&input) {
        return false;
    }

    if input.parse::<i64>().is_ok() || input.parse::<f64>().is_ok() {
        return false;
    }

    for ch in input.chars() {
        if ch.is_control() {
            return false;
        }
        if matches!(ch, ':' | '#' | '"' | '\'' | '{' | '}' | '[' | ']' | ',') {
            return false;
        }
    }

    true
}

fn push_indent(indent: usize, out: &mut String) {
    for _ in 0..indent {
        out.push(' ');
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::to_yaml_string;

    #[test]
    fn renders_nested_values() {
        let value = json!({
            "app": "super",
            "ports": [8080, 8081],
            "meta": {"region": "us-east-1", "enabled": true}
        });

        let yaml = to_yaml_string(&value);
        assert!(yaml.contains("app: super"));
        assert!(yaml.contains("ports:"));
        assert!(yaml.contains("- 8080"));
        assert!(yaml.contains("meta:"));
        assert!(yaml.contains("enabled: true"));
    }

    #[test]
    fn quotes_strings_when_needed() {
        let value = json!({"note": "v1: stable"});
        let yaml = to_yaml_string(&value);
        assert!(yaml.contains("note: \"v1: stable\""));
    }
}
