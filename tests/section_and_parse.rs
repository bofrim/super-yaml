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
fn scan_sections_accepts_front_matter_schema_data_order() {
    let input = r#"

---!syaml/v0
---front_matter
env: {}
---schema
types: {}
---data
name: test
"#;

    let (version, sections) = scan_sections(input).unwrap();
    assert_eq!(version, "v0");
    assert_eq!(sections.len(), 3);
    assert_eq!(sections[0].name, "front_matter");
    assert_eq!(sections[1].name, "schema");
    assert_eq!(sections[2].name, "data");
    assert!(sections[2].start_line > sections[1].start_line);
    assert!(sections[2].end_line >= sections[2].start_line);
}

#[test]
fn scan_sections_requires_marker_on_first_non_empty_line() {
    let input = "---schema\ntypes: {}\n---data\na: 1\n";
    let err = scan_sections(input).unwrap_err();
    assert!(err.to_string().contains("expected first non-empty line"));
}

#[test]
fn scan_sections_rejects_content_before_first_section() {
    let input = "---!syaml/v0\nhello\n---schema\ntypes: {}\n---data\na: 1\n";
    let err = scan_sections(input).unwrap_err();
    assert!(err.to_string().contains("content before first section fence"));
}

#[test]
fn scan_sections_rejects_unknown_duplicate_and_wrong_order() {
    let unknown = "---!syaml/v0\n---schema\ntypes: {}\n---unknown\na: 1\n";
    let err = scan_sections(unknown).unwrap_err();
    assert!(err.to_string().contains("unknown section 'unknown'"));

    let duplicate =
        "---!syaml/v0\n---schema\ntypes: {}\n---schema\ntypes: {}\n---data\na: 1\n";
    let err = scan_sections(duplicate).unwrap_err();
    assert!(err.to_string().contains("duplicate section 'schema'"));

    let wrong_order = "---!syaml/v0\n---data\na: 1\n---schema\ntypes: {}\n";
    let err = scan_sections(wrong_order).unwrap_err();
    assert!(err.to_string().contains("invalid section order"));
}

#[test]
fn scan_sections_rejects_missing_sections() {
    let input = "---!syaml/v0\n";
    let err = scan_sections(input).unwrap_err();
    assert!(err.to_string().contains("no sections found"));
}

#[test]
fn parse_document_wraps_yaml_errors_with_section_name() {
    let input = r#"
---!syaml/v0
---schema
types: {}
---data
root:
  a: 1
 b: 2
"#;

    let err = parse_document(input).unwrap_err();
    assert!(err.to_string().contains("yaml parse error in section 'data'"));
}

#[test]
fn parse_document_requires_valid_section_set_and_order() {
    let input = r#"
---!syaml/v0
---schema
types: {}
"#;

    let err = parse_document(input).unwrap_err();
    assert!(err.to_string().contains("invalid section order"));
}

#[test]
fn parse_document_validates_front_matter_env_shape() {
    let input = r#"
---!syaml/v0
---front_matter
env: 123
---schema
types: {}
---data
name: test
"#;

    let err = parse_document(input).unwrap_err();
    assert!(err
        .to_string()
        .contains("front_matter.env must be a mapping/object"));
}

#[test]
fn parse_document_rejects_unsupported_env_binding_source() {
    let input = r#"
---!syaml/v0
---front_matter
env:
  TOKEN:
    from: file
    key: TOKEN_FILE
---schema
types: {}
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
---front_matter
env:
  TOKEN:
    from: env
---schema
types: {}
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
types: {}
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
---front_matter
env:
  OPTIONAL:
    from: env
    key: OPTIONAL
    required: false
---schema
types: {}
---data
value <null>: "${env.OPTIONAL}"
"#;

    validate_document(input, &env_provider(&[])).unwrap();
}

#[test]
fn parse_document_handles_trailing_newlines() {
    let input = "---!syaml/v0\n---schema\ntypes: {}\n---data\nname: x\n\n\n";
    let parsed = parse_document(input).unwrap();
    assert_eq!(parsed.version, "v0");
    assert_eq!(parsed.data.value["name"], "x");
}
