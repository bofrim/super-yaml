use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use super_yaml::{generate_rust_types, generate_rust_types_from_path};

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
            "super_yaml_rust_codegen_{}_{}_{}",
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
fn generate_rust_types_from_inline_document() {
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

    let rendered = generate_rust_types(input).unwrap();

    assert!(rendered.contains("pub enum MessageKind"));
    assert!(rendered.contains("pub struct WsMessage"));
    // Inline enum `operator: [ema, derivative, rolling_mean]` is promoted to a named type.
    assert!(rendered.contains("pub enum WsMessageOperator"));
    assert!(rendered.contains("pub operator: WsMessageOperator"));
    assert!(rendered.contains("pub room_id: String"));
    assert!(rendered.contains("pub payload: Option<Value>"));
    assert!(rendered.contains("pub type Batch = Vec<WsMessage>;"));
    assert!(rendered
        .contains("pub type ServicesByName = std::collections::BTreeMap<String, WsMessage>;"));
    assert!(rendered.contains("pub fn check_ws_message_constraint_1(value: &WsMessage)"));
    assert!(rendered.contains("pub fn check_ws_message_constraints(value: &WsMessage)"));
}

#[test]
fn generate_rust_types_from_path_includes_imported_types() {
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

    let rendered = generate_rust_types_from_path(dir.file_path("root.syaml")).unwrap();

    assert!(rendered.contains("pub type SharedPort = i64;"));
    assert!(rendered.contains("pub struct Service"));
    assert!(rendered.contains("pub port: SharedPort,"));
}

#[test]
fn generate_rust_types_includes_deprecated_and_field_number() {
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

    let rendered = generate_rust_types(input).unwrap();
    // field_number doc comment
    assert!(rendered.contains("/// Field number: 1"), "missing field_number comment for id");
    assert!(rendered.contains("/// Field number: 6"), "missing field_number comment for legacy_name");
    // deprecated attribute
    assert!(rendered.contains("#[deprecated"), "missing #[deprecated] attribute");
    assert!(rendered.contains("Use 'name' instead"), "missing deprecation message");
}
