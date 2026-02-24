use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

use super_yaml::{compile_document_from_path, MapEnvProvider};

fn empty_env() -> MapEnvProvider {
    MapEnvProvider::new(std::collections::HashMap::new())
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
            "super_yaml_datarefs_{}_{}_{}",
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
    compile_document_from_path(path, &empty_env())
        .expect("compile")
        .value
}

fn compile_err(path: &Path) -> String {
    compile_document_from_path(path, &empty_env())
        .unwrap_err()
        .to_string()
}

#[test]
fn dollar_path_copies_scalar() {
    let td = TempDir::new("scalar");
    td.write("test.syaml", "---!syaml/v0\n---data\ndefaults:\n  timeout: 30\nservice:\n  timeout: $.defaults.timeout\n");
    let out = compile(&td.file_path("test.syaml"));
    assert_eq!(out["service"]["timeout"], json!(30));
}

#[test]
fn dollar_path_copies_entire_object() {
    let td = TempDir::new("obj");
    td.write("test.syaml", "---!syaml/v0\n---data\ndefaults:\n  timeout: 30\n  retries: 3\nservice_a:\n  config: $.defaults\n");
    let out = compile(&td.file_path("test.syaml"));
    assert_eq!(
        out["service_a"]["config"],
        json!({"timeout": 30, "retries": 3})
    );
}

#[test]
fn dot_sibling_reference() {
    let td = TempDir::new("sibling");
    td.write(
        "test.syaml",
        "---!syaml/v0\n---data\nserver:\n  host: localhost\n  addr: .host\n",
    );
    let out = compile(&td.file_path("test.syaml"));
    assert_eq!(out["server"]["addr"], json!("localhost"));
}

#[test]
fn chained_references_resolve_in_order() {
    let td = TempDir::new("chained");
    td.write("test.syaml", "---!syaml/v0\n---data\ndefaults:\n  port: 8080\nservice:\n  port: $.defaults.port\n  addr: .port\n");
    let out = compile(&td.file_path("test.syaml"));
    assert_eq!(out["service"]["port"], json!(8080));
    assert_eq!(out["service"]["addr"], json!(8080));
}

#[test]
fn dollar_path_copies_array() {
    let td = TempDir::new("array");
    td.write("test.syaml", "---!syaml/v0\n---data\nallowed_ips:\n  - 127.0.0.1\n  - 10.0.0.1\nservice:\n  whitelist: $.allowed_ips\n");
    let out = compile(&td.file_path("test.syaml"));
    assert_eq!(
        out["service"]["whitelist"],
        json!(["127.0.0.1", "10.0.0.1"])
    );
}

#[test]
fn unknown_dollar_path_gives_error() {
    let td = TempDir::new("unknown");
    td.write(
        "test.syaml",
        "---!syaml/v0\n---data\nservice:\n  timeout: $.nonexistent.value\n",
    );
    let err = compile_err(&td.file_path("test.syaml"));
    assert!(err.contains("not found"), "expected 'not found' in: {err}");
}

#[test]
fn circular_reference_gives_cycle_error() {
    let td = TempDir::new("cycle");
    td.write(
        "test.syaml",
        "---!syaml/v0\n---data\na:\n  x: $.b.y\nb:\n  y: $.a.x\n",
    );
    let err = compile_err(&td.file_path("test.syaml"));
    assert!(
        err.contains("cycle") || err.contains("dependency"),
        "expected cycle error in: {err}"
    );
}

#[test]
fn relative_reference_at_root_level_gives_error() {
    let td = TempDir::new("rootlevel");
    td.write(
        "test.syaml",
        "---!syaml/v0\n---data\nhost: localhost\naddr: .host\n",
    );
    let err = compile_err(&td.file_path("test.syaml"));
    assert!(
        err.contains("root level"),
        "expected 'root level' error in: {err}"
    );
}

#[test]
fn keyed_enum_member_reference_resolves_for_typed_value() {
    let td = TempDir::new("enum_member_typed");
    td.write(
        "test.syaml",
        "---!syaml/v0\n---schema\nSomeType:\n  type: object\n  properties:\n    field_a: integer\n    field_b: string\nSomeEnum:\n  type: SomeType\n  enum:\n    option_1:\n      field_a: 1\n      field_b: one\n    option_2:\n      field_a: 2\n      field_b: two\n---data\nsome_data <SomeType>: SomeEnum.option_1\n",
    );
    let out = compile(&td.file_path("test.syaml"));
    assert_eq!(out["some_data"], json!({"field_a": 1, "field_b": "one"}));
}

#[test]
fn keyed_enum_member_reference_rejects_invalid_member_in_typed_context() {
    let td = TempDir::new("enum_member_bad_typed");
    td.write(
        "test.syaml",
        "---!syaml/v0\n---schema\nSomeType:\n  type: object\n  properties:\n    field_a: integer\nSomeEnum:\n  type: SomeType\n  enum:\n    option_1:\n      field_a: 1\n---data\nsome_data <SomeType>: SomeEnum.missing\n",
    );
    let err = compile_err(&td.file_path("test.syaml"));
    assert!(err.contains("enum member"));
    assert!(err.contains("missing"));
}

#[test]
fn keyed_enum_member_reference_untyped_invalid_token_stays_string() {
    let td = TempDir::new("enum_member_untyped");
    td.write(
        "test.syaml",
        "---!syaml/v0\n---schema\nSomeType:\n  type: object\n  properties:\n    field_a: integer\nSomeEnum:\n  type: SomeType\n  enum:\n    option_1:\n      field_a: 1\n---data\nraw: SomeEnum.missing\n",
    );
    let out = compile(&td.file_path("test.syaml"));
    assert_eq!(out["raw"], json!("SomeEnum.missing"));
}

#[test]
fn keyed_enum_member_reference_resolves_for_namespaced_import_type() {
    let td = TempDir::new("enum_member_import");
    td.write(
        "shared.syaml",
        "---!syaml/v0\n---schema\nTimezone:\n  type: object\n  properties:\n    locale: string\n    offset: string\n  enum:\n    UTC:\n      locale: en-US\n      offset: '+00:00'\n---data\n{}\n",
    );
    td.write(
        "root.syaml",
        "---!syaml/v0\n---meta\nimports:\n  shared: ./shared.syaml\n---schema\n{}\n---data\ntz <shared.Timezone>: shared.Timezone.UTC\n",
    );
    let out = compile(&td.file_path("root.syaml"));
    assert_eq!(out["tz"], json!({"locale": "en-US", "offset": "+00:00"}));
}
