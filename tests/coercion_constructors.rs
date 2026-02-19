use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

use super_yaml::{compile_document, compile_document_from_path, MapEnvProvider};

fn env_provider(vars: &[(&str, &str)]) -> MapEnvProvider {
    let mut map = HashMap::new();
    for (k, v) in vars {
        map.insert((*k).to_string(), (*v).to_string());
    }
    MapEnvProvider::new(map)
}

fn compile(input: &str) -> serde_json::Value {
    compile_document(input, &env_provider(&[]))
        .expect("compile")
        .value
}

fn color_schema_prefix() -> &'static str {
    r#"
---!syaml/v0
---schema
Color:
  type: object
  properties:
    red: integer
    green: integer
    blue: integer
    alpha: number?
  constructors:
    rgb: { order: 1, regex: '^rgb\((?<red>\d+),\s*(?<green>\d+),\s*(?<blue>\d+)\)$', defaults: { alpha: 1 } }
    rgba: { order: 1, regex: '^rgba\((?<red>\d+),\s*(?<green>\d+),\s*(?<blue>\d+),\s*(?<alpha>0|1|0?\.\d+)\)$' }
    hex: { order: 2, regex: '^#(?<red_hex>[0-9A-Fa-f]{2})(?<green_hex>[0-9A-Fa-f]{2})(?<blue_hex>[0-9A-Fa-f]{2})(?<alpha_hex>[0-9A-Fa-f]{2})?$', map: { red: { group: red_hex, decode: hex_u8 }, green: { group: green_hex, decode: hex_u8 }, blue: { group: blue_hex, decode: hex_u8 }, alpha: { group: alpha_hex, decode: hex_alpha } }, defaults: { alpha: 1 } }
"#
}

#[test]
fn constructor_coerces_rgb_string_with_default_alpha() {
    let input = format!(
        "{}\n---data\naccent <Color>: \"rgb(10, 20, 30)\"\n",
        color_schema_prefix()
    );
    let compiled = compile(&input);
    assert_eq!(
        compiled["accent"],
        json!({"red": 10, "green": 20, "blue": 30, "alpha": 1})
    );
}

#[test]
fn constructor_coerces_rgba_string() {
    let input = format!(
        "{}\n---data\naccent <Color>: \"rgba(10, 20, 30, 0.5)\"\n",
        color_schema_prefix()
    );
    let compiled = compile(&input);
    assert_eq!(compiled["accent"]["red"], json!(10));
    assert_eq!(compiled["accent"]["green"], json!(20));
    assert_eq!(compiled["accent"]["blue"], json!(30));
    assert_eq!(compiled["accent"]["alpha"], json!(0.5));
}

#[test]
fn constructor_coerces_hex_string_with_default_alpha() {
    let input = format!(
        "{}\n---data\naccent <Color>: \"#0A141E\"\n",
        color_schema_prefix()
    );
    let compiled = compile(&input);
    assert_eq!(
        compiled["accent"],
        json!({"red": 10, "green": 20, "blue": 30, "alpha": 1})
    );
}

#[test]
fn constructor_coerces_hex_with_alpha_channel() {
    let input = format!(
        "{}\n---data\naccent <Color>: \"#0A141E80\"\n",
        color_schema_prefix()
    );
    let compiled = compile(&input);
    assert_eq!(compiled["accent"]["red"], json!(10));
    assert_eq!(compiled["accent"]["green"], json!(20));
    assert_eq!(compiled["accent"]["blue"], json!(30));
    let alpha = compiled["accent"]["alpha"].as_f64().expect("alpha number");
    assert!((alpha - (128.0 / 255.0)).abs() < 1e-12);
}

#[test]
fn constructor_reports_pattern_mismatch() {
    let input = format!(
        "{}\n---data\naccent <Color>: \"hsl(120, 50%, 25%)\"\n",
        color_schema_prefix()
    );
    let err = compile_document(&input, &env_provider(&[]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("constructor pattern mismatch"));
    assert!(err.contains("$.accent"));
    assert!(err.contains("Color"));
}

#[test]
fn constructor_reports_decode_failure_with_field_context() {
    let input = r##"
---!syaml/v0
---schema
Color:
  type: object
  properties:
    red: integer
  constructors:
    hex: { regex: '^#(?<raw>.{2})$', map: { red: { group: raw, decode: hex_u8 } } }
---data
accent <Color>: "#GG"
"##;

    let err = compile_document(input, &env_provider(&[]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("failed to decode capture"));
    assert!(err.contains("Color.red"));
}

#[test]
fn constructor_prefers_lowest_ordered_match() {
    let input = r#"
---!syaml/v0
---schema
Color:
  type: object
  properties:
    red: integer
    alpha: number?
  constructors:
    second: { order: 2, regex: '^dup(?<red>\d+)$', defaults: { alpha: 1 } }
    first: { order: 1, regex: '^dup(?<red>\d+)$', defaults: { alpha: 0 } }
---data
accent <Color>: "dup42"
"#;

    let compiled = compile(input);
    assert_eq!(compiled["accent"], json!({"red": 42, "alpha": 0}));
}

#[test]
fn constructor_requires_exactly_one_unordered_match() {
    let input = r#"
---!syaml/v0
---schema
Color:
  type: object
  properties:
    red: integer
  constructors:
    a: { regex: '^dup(?<red>\d+)$' }
    b: { regex: '^dup(?<red>\d+)$' }
---data
accent <Color>: "dup42"
"#;

    let err = compile_document(input, &env_provider(&[]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("ambiguous unordered constructor match"));
}

#[test]
fn constructor_prefers_ordered_matches_over_unordered() {
    let input = r#"
---!syaml/v0
---schema
Color:
  type: object
  properties:
    red: integer
    alpha: number?
  constructors:
    unordered: { regex: '^mix(?<red>\d+)$', defaults: { alpha: 9 } }
    ordered: { order: 1, regex: '^mix(?<red>\d+)$', defaults: { alpha: 1 } }
---data
accent <Color>: "mix7"
"#;

    let compiled = compile(input);
    assert_eq!(compiled["accent"], json!({"red": 7, "alpha": 1}));
}

#[test]
fn constructor_skips_non_string_values() {
    let input = format!(
        "{}\n---data\naccent <Color>:\n  red: 10\n  green: 20\n  blue: 30\n  alpha: 1\n",
        color_schema_prefix()
    );
    let compiled = compile(&input);
    assert_eq!(
        compiled["accent"],
        json!({"red": 10, "green": 20, "blue": 30, "alpha": 1})
    );
}

#[test]
fn constructor_does_not_run_without_type_hint() {
    let input = format!(
        "{}\n---data\naccent: \"rgb(10, 20, 30)\"\n",
        color_schema_prefix()
    );
    let compiled = compile(&input);
    assert_eq!(compiled["accent"], json!("rgb(10, 20, 30)"));
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "super_yaml_{}_{}_{}",
            prefix,
            std::process::id(),
            stamp
        ));
        fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    fn write(&self, file: &str, content: &str) {
        fs::write(self.path.join(file), content).expect("write temp file");
    }

    fn file_path(&self, file: &str) -> PathBuf {
        self.path.join(file)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if self.path.exists() {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[test]
fn constructor_works_for_namespaced_imported_type() {
    let dir = TempDir::new("imports_constructor_type");

    dir.write(
        "shared.syaml",
        r#"
---!syaml/v0
---schema
Color:
  type: object
  properties:
    red: integer
    green: integer
    blue: integer
    alpha: number?
  constructors:
    rgb: { regex: '^rgb\((?<red>\d+),\s*(?<green>\d+),\s*(?<blue>\d+)\)$', defaults: { alpha: 1 } }
---data
{}
"#,
    );

    dir.write(
        "root.syaml",
        r#"
---!syaml/v0
---meta
imports:
  shared: ./shared.syaml
---schema
{}
---data
accent <shared.Color>: "rgb(1, 2, 3)"
"#,
    );

    let compiled = compile_document_from_path(dir.file_path("root.syaml"), &env_provider(&[]))
        .expect("compile")
        .value;
    assert_eq!(
        compiled["accent"],
        json!({"red": 1, "green": 2, "blue": 3, "alpha": 1})
    );
}

#[test]
fn constructor_from_enum_maps_capture_to_string_enum_type() {
    let input = r#"
---!syaml/v0
---schema
MemoryUnit:
  - MiB
  - GiB
MemorySpec:
  type: object
  properties:
    amount: integer
    unit: MemoryUnit
  constructors:
    from_text: { regex: '^(?<amount>\d+)(?<raw_unit>[A-Za-z]+)$', map: { amount: { group: amount, decode: integer }, unit: { group: raw_unit, from_enum: MemoryUnit } } }
---data
memory <MemorySpec>: 16GiB
"#;
    let compiled = compile(input);
    assert_eq!(compiled["memory"], json!({"amount": 16, "unit": "GiB"}));
}

#[test]
fn constructor_from_enum_rejects_non_enum_value() {
    let input = r#"
---!syaml/v0
---schema
MemoryUnit:
  - MiB
  - GiB
MemorySpec:
  type: object
  properties:
    amount: integer
    unit: MemoryUnit
  constructors:
    from_text: { regex: '^(?<amount>\d+)(?<raw_unit>[A-Za-z]+)$', map: { amount: { group: amount, decode: integer }, unit: { group: raw_unit, from_enum: MemoryUnit } } }
---data
memory <MemorySpec>: 16TiB
"#;
    let err = compile_document(input, &env_provider(&[]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("is not in enum"));
    assert!(err.contains("MemoryUnit"));
}
