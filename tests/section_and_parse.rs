use std::collections::HashMap;

use super_yaml::parse_document;
use super_yaml::section_scanner::scan_sections;
use super_yaml::{validate_document, MapEnvProvider};

fn env_provider(vars: &[(&str, &str)]) -> MapEnvProvider {
    let mut map = HashMap::new();
    for (k, v) in vars {
        map.insert((*k).to_string(), (*v).to_string());
    }
    MapEnvProvider::new(map)
}

#[test]
fn scan_sections_accepts_meta_schema_data_order() {
    let input = r#"

---!syaml/v0
---meta
env: {}
---schema
{}
---data
name: test
"#;

    let (version, sections) = scan_sections(input).unwrap();
    assert_eq!(version, "v0");
    assert_eq!(sections.len(), 3);
    assert_eq!(sections[0].name, "meta");
    assert_eq!(sections[1].name, "schema");
    assert_eq!(sections[2].name, "data");
    assert!(sections[2].start_line > sections[1].start_line);
    assert!(sections[2].end_line >= sections[2].start_line);
}

#[test]
fn scan_sections_requires_marker_on_first_non_empty_line() {
    let input = "---schema\n{}\n---data\na: 1\n";
    let err = scan_sections(input).unwrap_err();
    assert!(err.to_string().contains("expected first non-empty line"));
}

#[test]
fn scan_sections_rejects_content_before_first_section() {
    let input = "---!syaml/v0\nhello\n---schema\n{}\n---data\na: 1\n";
    let err = scan_sections(input).unwrap_err();
    assert!(err
        .to_string()
        .contains("content before first section fence"));
}

#[test]
fn scan_sections_rejects_unknown_and_duplicate_and_accepts_any_order() {
    let unknown = "---!syaml/v0\n---schema\n{}\n---unknown\na: 1\n";
    let err = scan_sections(unknown).unwrap_err();
    assert!(err.to_string().contains("unknown section 'unknown'"));

    let duplicate = "---!syaml/v0\n---schema\n{}\n---schema\n{}\n---data\na: 1\n";
    let err = scan_sections(duplicate).unwrap_err();
    assert!(err.to_string().contains("duplicate section 'schema'"));

    let any_order = "---!syaml/v0\n---data\na: 1\n---schema\n{}\n";
    let (_version, sections) = scan_sections(any_order).unwrap();
    assert_eq!(sections.len(), 2);
    assert_eq!(sections[0].name, "data");
    assert_eq!(sections[1].name, "schema");
}

#[test]
fn scan_sections_allows_marker_only_document() {
    let input = "---!syaml/v0\n";
    let (_version, sections) = scan_sections(input).unwrap();
    assert!(sections.is_empty());
}

#[test]
fn parse_document_wraps_yaml_errors_with_section_name() {
    let input = r#"
---!syaml/v0
---schema
{}
---data
root:
  a: 1
 b: 2
"#;

    let err = parse_document(input).unwrap_err();
    assert!(err
        .to_string()
        .contains("yaml parse error in section 'data'"));
}

#[test]
fn parse_document_allows_missing_sections_with_defaults() {
    let input = r#"
---!syaml/v0
---schema
{}
"#;

    let parsed = parse_document(input).unwrap();
    assert!(parsed.meta.is_none());
    assert!(parsed.schema.types.is_empty());
    assert_eq!(parsed.data.value, serde_json::json!({}));
}

#[test]
fn parse_document_validates_meta_env_shape() {
    let input = r#"
---!syaml/v0
---meta
env: 123
---schema
{}
---data
name: test
"#;

    let err = parse_document(input).unwrap_err();
    assert!(err
        .to_string()
        .contains("meta.env must be a mapping/object"));
}

#[test]
fn parse_document_parses_meta_imports() {
    let input = r#"
---!syaml/v0
---meta
imports:
  shared: ./shared.syaml
---schema
{}
---data
name: test
"#;

    let parsed = parse_document(input).unwrap();
    let meta = parsed.meta.expect("meta");
    assert_eq!(meta.imports["shared"].path, "./shared.syaml");
}

#[test]
fn parse_document_parses_meta_file_details() {
    let input = r#"
---!syaml/v0
---meta
file:
  owner: platform
  revision: 3
---schema
{}
---data
name: test
"#;

    let parsed = parse_document(input).unwrap();
    let meta = parsed.meta.expect("meta");
    assert_eq!(meta.file["owner"], "platform");
    assert_eq!(meta.file["revision"], 3);
}

#[test]
fn parse_document_validates_meta_file_shape() {
    let input = r#"
---!syaml/v0
---meta
file: 123
---schema
{}
---data
name: test
"#;

    let err = parse_document(input).unwrap_err();
    assert!(err
        .to_string()
        .contains("meta.file must be a mapping/object"));
}

#[test]
fn parse_document_rejects_invalid_import_alias() {
    let input = r#"
---!syaml/v0
---meta
imports:
  bad-alias: ./shared.syaml
---schema
{}
---data
name: test
"#;

    let err = parse_document(input).unwrap_err();
    assert!(err.to_string().contains("invalid namespace alias"));
}

#[test]
fn parse_document_rejects_unsupported_env_binding_source() {
    let input = r#"
---!syaml/v0
---meta
env:
  TOKEN:
    from: file
    key: TOKEN_FILE
---schema
{}
---data
name: ok
"#;

    let err = parse_document(input).unwrap_err();
    assert!(err.to_string().contains("unsupported from='file'"));
}

#[test]
fn parse_document_requires_env_binding_key() {
    let input = r#"
---!syaml/v0
---meta
env:
  TOKEN:
    from: env
---schema
{}
---data
name: ok
"#;

    let err = parse_document(input).unwrap_err();
    assert!(err.to_string().contains("must define string key"));
}

#[test]
fn validate_document_reports_type_violations() {
    let input = r#"
---!syaml/v0
---schema
{}
---data
count <integer>: "abc"
"#;

    let err = validate_document(input, &env_provider(&[])).unwrap_err();
    assert!(err.to_string().contains("type mismatch"));
}

#[test]
fn validate_document_allows_optional_env_binding_without_value() {
    let input = r#"
---!syaml/v0
---meta
env:
  OPTIONAL:
    from: env
    key: OPTIONAL
    required: false
---schema
{}
---data
value <null>: "${env.OPTIONAL}"
"#;

    validate_document(input, &env_provider(&[])).unwrap();
}

#[test]
fn validate_document_accepts_schema_property_type_shorthand() {
    let input = r#"
---!syaml/v0
---schema
BoundsConfig:
  type: object
  properties:
    x_min: number
---data
bounds <BoundsConfig>:
  x_min: 1.5
"#;

    validate_document(input, &env_provider(&[])).unwrap();
}

#[test]
fn validate_document_accepts_optional_schema_property_type_shorthand() {
    let input = r#"
---!syaml/v0
---schema
Circle:
  type: object
  properties:
    radius: number?
    label: string
---data
circle <Circle>:
  label: unit
"#;

    validate_document(input, &env_provider(&[])).unwrap();
}

#[test]
fn validate_document_reports_unknown_schema_type_reference_even_when_unused() {
    let input = r#"
---!syaml/v0
---schema
Service:
  type: object
  properties:
    port:
      type: MissingType
---data
name: ok
"#;

    let err = validate_document(input, &env_provider(&[])).unwrap_err();
    assert!(err.to_string().contains("unknown type reference"));
    assert!(err.to_string().contains("MissingType"));
    assert!(err
        .to_string()
        .contains("schema.Service.properties.port.type"));
}

#[test]
fn parse_document_handles_trailing_newlines() {
    let input = "---!syaml/v0\n---schema\n{}\n---data\nname: x\n\n\n";
    let parsed = parse_document(input).unwrap();
    assert_eq!(parsed.version, "v0");
    assert_eq!(parsed.data.value["name"], "x");
}
