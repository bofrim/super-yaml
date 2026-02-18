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
types:
  MessageKind:
    enum: [join, leave]
  WsMessage:
    type: object
    required: [kind, room_id]
    properties:
      kind:
        type: MessageKind
      room_id:
        type: string
      payload:
        type: object
  Batch:
    type: array
    items:
      type: WsMessage
---data
example: 1
"#;

    let rendered = generate_rust_types(input).unwrap();

    assert!(rendered.contains("pub enum MessageKind"));
    assert!(rendered.contains("pub struct WsMessage"));
    assert!(rendered.contains("pub room_id: String"));
    assert!(rendered.contains("pub payload: Option<Value>"));
    assert!(rendered.contains("pub type Batch = Vec<WsMessage>;"));
}

#[test]
fn generate_rust_types_from_path_includes_imported_types() {
    let dir = TempDir::new("imports");

    dir.write(
        "shared.syaml",
        r#"
---!syaml/v0
---schema
types:
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
---front_matter
imports:
  shared: ./shared.syaml
---schema
types:
  Service:
    type: object
    required: [port]
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
