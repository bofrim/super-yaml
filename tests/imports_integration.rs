use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

use super_yaml::{compile_document_from_path, MapEnvProvider};

fn env_provider(vars: &[(&str, &str)]) -> MapEnvProvider {
    let mut map = HashMap::new();
    for (k, v) in vars {
        map.insert((*k).to_string(), (*v).to_string());
    }
    MapEnvProvider::new(map)
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

fn compile(path: &Path) -> serde_json::Value {
    compile_document_from_path(path, &env_provider(&[]))
        .expect("compile")
        .value
}

#[test]
fn imports_expose_data_and_types_under_namespace_alias() {
    let dir = TempDir::new("imports_namespace");

    dir.write(
        "shared.syaml",
        r#"
---!syaml/v0
---schema
Port:
  type: integer
  minimum: 1
  maximum: 65535
Service:
  type: object
  properties:
    port:
      type: Port
---data
defaults:
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
RootService:
  type: object
  properties:
    port:
      type: shared.Port
      constraints: "value == shared.defaults.port"
---data
port <shared.Port>: "${shared.defaults.port}"
service <RootService>:
  port: "${port}"
"#,
    );

    let compiled = compile(&dir.file_path("root.syaml"));
    assert_eq!(compiled["port"], json!(8080));
    assert_eq!(compiled["service"]["port"], json!(8080));
    assert_eq!(compiled["shared"]["defaults"]["port"], json!(8080));
}

#[test]
fn import_alias_must_not_conflict_with_existing_data_key() {
    let dir = TempDir::new("imports_data_conflict");

    dir.write(
        "shared.syaml",
        r#"
---!syaml/v0
---schema
{}
---data
value: 1
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
shared: already_here
"#,
    );

    let err = compile_document_from_path(dir.file_path("root.syaml"), &env_provider(&[]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("conflicts with existing data key"));
}

#[test]
fn imports_detect_cycles() {
    let dir = TempDir::new("imports_cycle");

    dir.write(
        "a.syaml",
        r#"
---!syaml/v0
---meta
imports:
  b: ./b.syaml
---schema
{}
---data
name: a
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
{}
---data
name: b
"#,
    );

    let err = compile_document_from_path(dir.file_path("a.syaml"), &env_provider(&[]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("cyclic import detected"));
}

#[test]
fn imports_report_missing_namespaced_schema_type_reference() {
    let dir = TempDir::new("imports_missing_type");

    dir.write(
        "shared.syaml",
        r#"
---!syaml/v0
---schema
Port:
  type: integer
---data
defaults:
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
RootService:
  type: object
  properties:
    port:
      type: shared.Missing
---data
service <RootService>:
  port: 8080
"#,
    );

    let err = compile_document_from_path(dir.file_path("root.syaml"), &env_provider(&[]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("unknown type reference"));
    assert!(err.contains("shared.Missing"));
    assert!(err.contains("schema.RootService.properties.port.type"));
}
