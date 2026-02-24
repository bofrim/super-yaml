use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use super_yaml::{generate_typescript_types, generate_typescript_types_from_path};

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
            "super_yaml_ts_codegen_{}_{}_{}",
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
fn generate_typescript_types_from_inline_document() {
    let input = r#"
---!syaml/v0
---schema
MessageKind:
  enum: [join, leave]
WsMessage:
  type: object
  constraints: "value.room_id != \"\""
  properties:
    kind:
      type: MessageKind
    operator: [ema, derivative, rolling_mean]
    room_id:
      type: string
    payload:
      type: object
      optional: true
Batch:
  type: array
  items:
    type: WsMessage
ServicesByName:
  type: object
  values:
    type: WsMessage
---data
example: 1
"#;

    let rendered = generate_typescript_types(input).unwrap();

    assert!(rendered.contains("export type MessageKind = \"join\" | \"leave\";"));
    assert!(rendered.contains("export interface WsMessage"));
    assert!(rendered.contains("operator: string;"));
    assert!(rendered.contains("room_id: string;"));
    assert!(rendered.contains("payload?: unknown;"));
    assert!(rendered.contains("export type Batch = Array<WsMessage>;"));
    assert!(rendered.contains("export type ServicesByName = Record<string, WsMessage>;"));
    assert!(rendered.contains("export function checkWsMessageConstraint1(value: WsMessage)"));
    assert!(rendered.contains("export function checkWsMessageConstraints(value: WsMessage)"));
}

#[test]
fn generate_typescript_types_from_path_includes_imported_types() {
    let dir = TempDir::new("imports");

    dir.write(
        "shared.syaml",
        r#"
---!syaml/v0
---schema
Port:
  type: integer
  minimum: 1
  maximum: 65535
---data
port <Port>: 8080
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
Service:
  type: object
  properties:
    port:
      type: shared.Port
---data
service <Service>:
  port: 8080
"#,
    );

    let rendered = generate_typescript_types_from_path(dir.file_path("root.syaml")).unwrap();

    assert!(rendered.contains("export type SharedPort = number;"));
    assert!(rendered.contains("export interface Service"));
    assert!(rendered.contains("port: SharedPort;"));
}

#[test]
fn generate_typescript_types_from_path_reports_import_cycles() {
    let dir = TempDir::new("cycles");

    dir.write(
        "a.syaml",
        r#"
---!syaml/v0
---meta
imports:
  b: ./b.syaml
---schema
AType:
  type: b.BType
---data
a: 1
"#,
    );

    dir.write(
        "b.syaml",
        r#"
---!syaml/v0
---meta
imports:
  a: ./a.syaml
---schema
BType:
  type: a.AType
---data
b: 1
"#,
    );

    let err = generate_typescript_types_from_path(dir.file_path("a.syaml")).unwrap_err();
    assert!(err.to_string().contains("cyclic import detected"));
}

#[test]
fn generate_typescript_types_includes_deprecated_jsdoc() {
    let input = r#"
---!syaml/v0
---schema
User:
  type: object
  properties:
    id:
      type: integer
      field_number: 1
      since: "1.0.0"
    legacy_name:
      type: string
      field_number: 6
      since: "1.0.0"
      deprecated:
        version: "2.0.0"
        severity: warning
        message: "Use 'name' instead"
      optional: true
---data
x: 1
"#;

    let rendered = generate_typescript_types(input).unwrap();
    // field_number comment
    assert!(rendered.contains("Field number: 1"), "missing field_number for id");
    assert!(rendered.contains("Field number: 6"), "missing field_number for legacy_name");
    // @deprecated JSDoc
    assert!(rendered.contains("@deprecated"), "missing @deprecated JSDoc");
    assert!(rendered.contains("Use 'name' instead"), "missing deprecation message");
}

#[test]
fn generates_to_string_fn_for_as_string_type() {
    let input = r#"
---!syaml/v0
---schema
Semver:
  type: object
  as_string: "{{major}}.{{minor}}.{{patch}}"
  properties:
    major:
      type: integer
      minimum: 0
    minor:
      type: integer
      minimum: 0
    patch:
      type: integer
      minimum: 0
---data
ver: 1
"#;
    let rendered = generate_typescript_types(input).unwrap();
    assert!(rendered.contains("export function semverToString("), "missing tostring fn");
    assert!(rendered.contains(": Semver): string {"), "missing return type");
    assert!(rendered.contains("return `"), "missing template literal");
    assert!(rendered.contains("${semver.major}"), "missing major accessor");
    assert!(rendered.contains("${semver.minor}"), "missing minor accessor");
    assert!(rendered.contains("${semver.patch}"), "missing patch accessor");
}
