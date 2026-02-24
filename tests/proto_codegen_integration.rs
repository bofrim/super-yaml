use super_yaml::{generate_proto_types, generate_proto_types_from_path};

// ── Helper ──────────────────────────────────────────────────────────────────

fn proto(doc: &str) -> String {
    generate_proto_types(doc).expect("proto codegen should succeed")
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[test]
fn generate_proto_types_basic_message() {
    let doc = r#"---!syaml/v0
---schema
Point:
  type: object
  properties:
    x:
      type: number
      field_number: 1
    y:
      type: number
      field_number: 2
---data
"#;
    let out = proto(doc);
    assert!(
        out.contains("message Point {"),
        "missing message block:\n{out}"
    );
    assert!(out.contains("double x = 1;"), "missing x field:\n{out}");
    assert!(out.contains("double y = 2;"), "missing y field:\n{out}");
    assert!(
        out.starts_with("syntax = \"proto3\";"),
        "missing proto3 syntax:\n{out}"
    );
}

#[test]
fn generate_proto_types_enum() {
    let doc = r#"---!syaml/v0
---schema
Color:
  type: string
  enum:
    - red
    - green
    - blue
---data
"#;
    let out = proto(doc);
    assert!(out.contains("enum Color {"), "missing enum:\n{out}");
    assert!(
        out.contains("COLOR_UNSPECIFIED = 0;"),
        "missing unspecified zero:\n{out}"
    );
    assert!(out.contains("COLOR_RED = 1;"), "missing RED:\n{out}");
    assert!(out.contains("COLOR_GREEN = 2;"), "missing GREEN:\n{out}");
    assert!(out.contains("COLOR_BLUE = 3;"), "missing BLUE:\n{out}");
}

#[test]
fn generate_proto_types_repeated_field() {
    let doc = r#"---!syaml/v0
---schema
Container:
  type: object
  properties:
    tags:
      type: array
      items:
        type: string
      field_number: 1
---data
"#;
    let out = proto(doc);
    assert!(
        out.contains("repeated string tags = 1;"),
        "missing repeated field:\n{out}"
    );
}

#[test]
fn generate_proto_types_deprecated_field() {
    let doc = r#"---!syaml/v0
---schema
User:
  type: object
  properties:
    id:
      type: integer
      field_number: 1
    legacy_id:
      type: string
      field_number: 2
      deprecated: "1.0.0"
      optional: true
---data
"#;
    let out = proto(doc);
    assert!(
        out.contains("[deprecated = true]"),
        "missing deprecated option:\n{out}"
    );
    assert!(
        out.contains("legacy_id = 2 [deprecated = true];"),
        "deprecated field not rendered correctly:\n{out}"
    );
}

#[test]
fn generate_proto_types_reserved_for_removed() {
    let doc = r#"---!syaml/v0
---schema
Record:
  type: object
  properties:
    id:
      type: integer
      field_number: 1
    old_field:
      type: string
      field_number: 8
      since: "1.0.0"
      removed: "2.0.0"
      optional: true
---data
"#;
    let out = proto(doc);
    assert!(
        out.contains("reserved 8;"),
        "missing numeric reservation:\n{out}"
    );
    assert!(
        out.contains("reserved \"old_field\";"),
        "missing string reservation:\n{out}"
    );
    // The removed field itself should NOT be emitted as a normal field
    assert!(
        !out.contains("old_field = 8"),
        "removed field should not be a regular field:\n{out}"
    );
}

#[test]
fn generate_proto_types_errors_on_missing_field_number() {
    let doc = r#"---!syaml/v0
---schema
Broken:
  type: object
  properties:
    id:
      type: integer
    name:
      type: string
      field_number: 2
---data
"#;
    let err = generate_proto_types(doc).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("missing field_number") && msg.contains("id"),
        "expected error about missing field_number for 'id', got: {msg}"
    );
}

#[test]
fn generate_proto_types_optional_scalar_field() {
    let doc = r#"---!syaml/v0
---schema
Config:
  type: object
  properties:
    name:
      type: string
      field_number: 1
    description:
      type: string
      field_number: 2
      optional: true
---data
"#;
    let out = proto(doc);
    assert!(
        out.contains("optional string description = 2;"),
        "missing optional qualifier:\n{out}"
    );
    // Non-optional field should not have 'optional' qualifier
    assert!(
        out.contains("string name = 1;") && !out.contains("optional string name"),
        "non-optional field got 'optional' qualifier:\n{out}"
    );
}

#[test]
fn generate_proto_types_from_versioned_fields_example() {
    let out = generate_proto_types_from_path("examples/versioned_fields.syaml")
        .expect("versioned_fields example should generate valid proto");

    // Check all three message types are present
    assert!(
        out.contains("message Address {"),
        "missing Address message:\n{out}"
    );
    assert!(
        out.contains("message UserProfile {"),
        "missing UserProfile message:\n{out}"
    );
    assert!(
        out.contains("message BlogPost {"),
        "missing BlogPost message:\n{out}"
    );

    // old_id was removed in 2.0.0 — should be reserved
    assert!(
        out.contains("reserved 8;"),
        "old_id should be reserved at field 8:\n{out}"
    );
    assert!(
        out.contains("reserved \"old_id\";"),
        "old_id name should be reserved:\n{out}"
    );

    // deprecated fields get [deprecated = true]
    assert!(
        out.contains("[deprecated = true]"),
        "deprecated fields should have [deprecated = true]:\n{out}"
    );
}
