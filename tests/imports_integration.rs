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
fn imports_expose_types_and_allow_namespaced_data_references_without_emitting_namespace() {
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
---data
port <shared.Port>: "${shared.defaults.port}"
service <RootService>:
  port: "${port}"
"#,
    );

    let compiled = compile(&dir.file_path("root.syaml"));
    assert_eq!(compiled["port"], json!(8080));
    assert_eq!(compiled["service"]["port"], json!(8080));
    assert!(compiled.get("shared").is_none());
}

#[test]
fn import_alias_can_be_explicitly_extracted_into_output() {
    let dir = TempDir::new("imports_data_extract");

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
shared_copy: shared
"#,
    );

    let compiled = compile(&dir.file_path("root.syaml"));
    assert_eq!(compiled["shared_copy"]["value"], json!(1));
}

#[test]
fn import_path_can_be_explicitly_extracted_into_output_value() {
    let dir = TempDir::new("imports_path_extract");

    dir.write(
        "shared.syaml",
        r#"
---!syaml/v0
---schema
{}
---data
templates:
  service:
    host: "{{HOST}}"
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
some_key: shared.templates.service
"#,
    );

    let compiled = compile(&dir.file_path("root.syaml"));
    assert_eq!(compiled["some_key"]["host"], json!("{{HOST}}"));
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

#[test]
fn imports_allow_descendant_hint_with_direct_source_namespace_type() {
    let dir = TempDir::new("imports_source_namespace_hint");

    dir.write(
        "a.syaml",
        r#"
---!syaml/v0
---schema
SchemaA:
  type: integer
  minimum: 1
---data
{}
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
SchemaB:
  type: object
  properties:
    value:
      type: a.SchemaA
---data
{}
"#,
    );

    dir.write(
        "c.syaml",
        r#"
---!syaml/v0
---meta
imports:
  a: ./a.syaml
  b: ./b.syaml
---schema
{}
---data
item <b.SchemaB>:
  value <a.SchemaA>: 7
"#,
    );

    let compiled = compile(&dir.file_path("c.syaml"));
    assert_eq!(compiled["item"]["value"], json!(7));
}

#[test]
fn private_top_level_data_keys_are_local_only_and_not_emitted() {
    let dir = TempDir::new("private_data_local");

    dir.write(
        "root.syaml",
        r#"
---!syaml/v0
---schema
{}
---data
_defaults:
  port: 8080
_templates:
  service:
    host: "{{HOST:localhost}}"
    port: "{{PORT}}"
service:
  "{{_templates.service}}":
    PORT: "${_defaults.port}"
public_port: "${_defaults.port}"
"#,
    );

    let compiled = compile(&dir.file_path("root.syaml"));
    assert_eq!(compiled["service"]["host"], json!("localhost"));
    assert_eq!(compiled["service"]["port"], json!(8080));
    assert_eq!(compiled["public_port"], json!(8080));
    assert!(compiled.get("_defaults").is_none());
    assert!(compiled.get("_templates").is_none());
}

#[test]
fn imported_documents_do_not_expose_private_top_level_data_keys() {
    let dir = TempDir::new("private_data_imports");

    dir.write(
        "shared.syaml",
        r#"
---!syaml/v0
---schema
{}
---data
_secret:
  base: 41
public_answer: "=_secret.base + 1"
"#,
    );

    dir.write(
        "ok.syaml",
        r#"
---!syaml/v0
---meta
imports:
  shared: ./shared.syaml
---schema
{}
---data
answer: "${shared.public_answer}"
"#,
    );

    let compiled = compile(&dir.file_path("ok.syaml"));
    assert_eq!(compiled["answer"], json!(42));

    dir.write(
        "leak.syaml",
        r#"
---!syaml/v0
---meta
imports:
  shared: ./shared.syaml
---schema
{}
---data
attempt: "${shared._secret.base}"
"#,
    );

    let err = compile_document_from_path(dir.file_path("leak.syaml"), &env_provider(&[]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("unknown reference 'shared._secret.base'"));
}

#[test]
fn imports_allow_descendant_hint_with_direct_source_namespace_for_nested_custom_type() {
    let dir = TempDir::new("imports_source_namespace_nested_type");

    dir.write(
        "system_manifest.syaml",
        r#"
---!syaml/v0
---schema
ManifestParams:
  type: object
  properties:
    mode: string
SystemManifestEntry:
  type: object
  properties:
    enabled: boolean
    params: ManifestParams
SystemManifestConfig:
  type: object
  values:
    type: SystemManifestEntry
---data
{}
"#,
    );

    dir.write(
        "sim.syaml",
        r#"
---!syaml/v0
---meta
imports:
  system_manifest: ./system_manifest.syaml
---schema
WorldSystemsConfig:
  type: object
  properties:
    manifest: system_manifest.SystemManifestConfig
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
  sim: ./sim.syaml
  system_manifest: ./system_manifest.syaml
---schema
{}
---data
systems <sim.WorldSystemsConfig>:
  manifest <system_manifest.SystemManifestConfig>:
    topology <system_manifest.SystemManifestEntry>:
      enabled <boolean>: true
      params <system_manifest.ManifestParams>:
        mode <string>: active
"#,
    );

    let compiled = compile(&dir.file_path("root.syaml"));
    assert_eq!(
        compiled["systems"]["manifest"]["topology"]["enabled"],
        json!(true)
    );
    assert_eq!(
        compiled["systems"]["manifest"]["topology"]["params"]["mode"],
        json!("active")
    );
}

#[test]
fn imports_expand_template_invocation_with_defaults() {
    let dir = TempDir::new("imports_template_success");

    dir.write(
        "tpl.syaml",
        r#"
---!syaml/v0
---schema
Service:
  type: object
  properties:
    name: string
    host: string
    port: integer
    tls: boolean
    env: [prod, staging, dev]
---data
templates:
  service:
    name: "{{NAME}}"
    host: "{{HOST}}"
    port: "{{PORT:8080}}"
    tls: "{{TLS:false}}"
    env: "{{ENV}}"
"#,
    );

    dir.write(
        "root.syaml",
        r#"
---!syaml/v0
---meta
imports:
  tpl: ./tpl.syaml
---schema
{}
---data
service <tpl.Service>:
  "{{tpl.templates.service}}":
    NAME: api-service
    HOST: api.internal
    ENV: prod
"#,
    );

    let compiled = compile(&dir.file_path("root.syaml"));
    assert_eq!(compiled["service"]["name"], json!("api-service"));
    assert_eq!(compiled["service"]["host"], json!("api.internal"));
    assert_eq!(compiled["service"]["port"], json!(8080));
    assert_eq!(compiled["service"]["tls"], json!(false));
    assert_eq!(compiled["service"]["env"], json!("prod"));
}

#[test]
fn imports_template_invocation_rejects_missing_required_variable() {
    let dir = TempDir::new("imports_template_missing_var");

    dir.write(
        "tpl.syaml",
        r#"
---!syaml/v0
---schema
Service:
  type: object
  properties:
    host: string
    env: [prod, staging, dev]
---data
templates:
  service:
    host: "{{HOST}}"
    env: "{{ENV}}"
"#,
    );

    dir.write(
        "root.syaml",
        r#"
---!syaml/v0
---meta
imports:
  tpl: ./tpl.syaml
---schema
{}
---data
service <tpl.Service>:
  "{{tpl.templates.service}}":
    HOST: api.internal
"#,
    );

    let err = compile_document_from_path(dir.file_path("root.syaml"), &env_provider(&[]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("missing required template variable 'ENV'"));
}

#[test]
fn imports_template_invocation_rejects_unexpected_variable() {
    let dir = TempDir::new("imports_template_unexpected_var");

    dir.write(
        "tpl.syaml",
        r#"
---!syaml/v0
---schema
Service:
  type: object
  properties:
    host: string
---data
templates:
  service:
    host: "{{HOST}}"
"#,
    );

    dir.write(
        "root.syaml",
        r#"
---!syaml/v0
---meta
imports:
  tpl: ./tpl.syaml
---schema
{}
---data
service <tpl.Service>:
  "{{tpl.templates.service}}":
    HOST: api.internal
    EXTRA: nope
"#,
    );

    let err = compile_document_from_path(dir.file_path("root.syaml"), &env_provider(&[]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("unexpected template variable 'EXTRA'"));
}

#[test]
fn imports_template_invocation_values_are_validated_against_schema() {
    let dir = TempDir::new("imports_template_type_validation");

    dir.write(
        "tpl.syaml",
        r#"
---!syaml/v0
---schema
Service:
  type: object
  properties:
    host: string
    port: integer
---data
templates:
  service:
    host: "{{HOST}}"
    port: "{{PORT:8080}}"
"#,
    );

    dir.write(
        "root.syaml",
        r#"
---!syaml/v0
---meta
imports:
  tpl: ./tpl.syaml
---schema
{}
---data
service <tpl.Service>:
  "{{tpl.templates.service}}":
    HOST: api.internal
    PORT: "443"
"#,
    );

    let err = compile_document_from_path(dir.file_path("root.syaml"), &env_provider(&[]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("expected integer"));
    assert!(err.contains("$.service.port"));
}
