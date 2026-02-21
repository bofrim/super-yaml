use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

use ed25519_dalek::{Signer, SigningKey};

use super_yaml::{compile_document_from_path, verify, MapEnvProvider};

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

#[test]
fn template_with_sibling_keys_merges_and_validates() {
    let dir = TempDir::new("template_siblings");

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
  port <integer>: 9090
  tls: true
"#,
    );

    let compiled = compile(&dir.file_path("root.syaml"));
    assert_eq!(compiled["service"]["name"], json!("api-service"));
    assert_eq!(compiled["service"]["host"], json!("api.internal"));
    assert_eq!(compiled["service"]["port"], json!(9090));
    assert_eq!(compiled["service"]["tls"], json!(true));
    assert_eq!(compiled["service"]["env"], json!("prod"));
}

#[test]
fn template_with_sibling_expression_resolves() {
    let dir = TempDir::new("template_sibling_expr");

    dir.write(
        "root.syaml",
        r#"
---!syaml/v0
---schema
{}
---data
_templates:
  base:
    host: "{{HOST}}"
    port: "{{PORT:8080}}"

service:
  "{{_templates.base}}":
    HOST: api.internal
  url: "https://${service.host}:${service.port}"
"#,
    );

    let compiled = compile(&dir.file_path("root.syaml"));
    assert_eq!(compiled["service"]["host"], json!("api.internal"));
    assert_eq!(compiled["service"]["port"], json!(8080));
    assert_eq!(compiled["service"]["url"], json!("https://api.internal:8080"));
}

#[test]
fn locked_template_field_allows_unlocked_override_and_blocks_locked() {
    let dir = TempDir::new("template_locked_fields");

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
    name!: "{{NAME}}"
    host!: "{{HOST}}"
    port: "{{PORT:8080}}"
    tls: "{{TLS:false}}"
    env!: "{{ENV}}"
"#,
    );

    dir.write(
        "ok.syaml",
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
  port: 9090
  tls: true
"#,
    );

    let compiled = compile(&dir.file_path("ok.syaml"));
    assert_eq!(compiled["service"]["name"], json!("api-service"));
    assert_eq!(compiled["service"]["host"], json!("api.internal"));
    assert_eq!(compiled["service"]["port"], json!(9090));
    assert_eq!(compiled["service"]["tls"], json!(true));
    assert_eq!(compiled["service"]["env"], json!("prod"));

    dir.write(
        "bad.syaml",
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
  name: override-attempt
"#,
    );

    let err = compile_document_from_path(dir.file_path("bad.syaml"), &env_provider(&[]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("conflicts with locked template field"));
    assert!(err.contains("'name'"));
}

// ---------------------------------------------------------------------------
// Hash verification
// ---------------------------------------------------------------------------

#[test]
fn import_with_correct_hash_succeeds() {
    let dir = TempDir::new("hash_ok");
    let shared_content = r#"
---!syaml/v0
---schema
Port:
  type: integer
  minimum: 1
  maximum: 65535
---data
port <Port>: 8080
"#;
    dir.write("shared.syaml", shared_content);
    let hash = verify::compute_sha256(shared_content.as_bytes());

    dir.write(
        "root.syaml",
        &format!(
            r#"
---!syaml/v0
---meta
imports:
  shared:
    path: ./shared.syaml
    hash: {hash}
---schema
{{}}
---data
p <shared.Port>: "${{shared.port}}"
"#
        ),
    );

    let compiled = compile(&dir.file_path("root.syaml"));
    assert_eq!(compiled["p"], json!(8080));
}

#[test]
fn import_with_wrong_hash_fails() {
    let dir = TempDir::new("hash_bad");
    dir.write(
        "shared.syaml",
        r#"
---!syaml/v0
---schema
{}
---data
x: 1
"#,
    );

    dir.write(
        "root.syaml",
        r#"
---!syaml/v0
---meta
imports:
  shared:
    path: ./shared.syaml
    hash: sha256:0000000000000000000000000000000000000000000000000000000000000000
---schema
{}
---data
y: 1
"#,
    );

    let err = compile_document_from_path(dir.file_path("root.syaml"), &env_provider(&[]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("hash verification failed"));
    assert!(err.contains("sha256 mismatch"));
}

// ---------------------------------------------------------------------------
// Version pinning
// ---------------------------------------------------------------------------

#[test]
fn import_with_satisfied_version_succeeds() {
    let dir = TempDir::new("version_ok");
    dir.write(
        "shared.syaml",
        r#"
---!syaml/v0
---meta
file:
  version: "1.2.3"
---schema
{}
---data
val: 42
"#,
    );

    dir.write(
        "root.syaml",
        r#"
---!syaml/v0
---meta
imports:
  shared:
    path: ./shared.syaml
    version: "^1.0.0"
---schema
{}
---data
v: "${shared.val}"
"#,
    );

    let compiled = compile(&dir.file_path("root.syaml"));
    assert_eq!(compiled["v"], json!(42));
}

#[test]
fn import_with_unsatisfied_version_fails() {
    let dir = TempDir::new("version_bad");
    dir.write(
        "shared.syaml",
        r#"
---!syaml/v0
---meta
file:
  version: "1.2.3"
---schema
{}
---data
val: 42
"#,
    );

    dir.write(
        "root.syaml",
        r#"
---!syaml/v0
---meta
imports:
  shared:
    path: ./shared.syaml
    version: ">=2.0.0"
---schema
{}
---data
v: "${shared.val}"
"#,
    );

    let err = compile_document_from_path(dir.file_path("root.syaml"), &env_provider(&[]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("version requirement not satisfied"));
    assert!(err.contains("does not satisfy"));
}

#[test]
fn import_version_required_but_file_has_none_fails() {
    let dir = TempDir::new("version_missing");
    dir.write(
        "shared.syaml",
        r#"
---!syaml/v0
---schema
{}
---data
val: 1
"#,
    );

    dir.write(
        "root.syaml",
        r#"
---!syaml/v0
---meta
imports:
  shared:
    path: ./shared.syaml
    version: "^1.0.0"
---schema
{}
---data
v: "${shared.val}"
"#,
    );

    let err = compile_document_from_path(dir.file_path("root.syaml"), &env_provider(&[]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("version requirement not satisfied"));
    assert!(err.contains("does not declare"));
}

#[test]
fn import_with_hash_and_version_both_pass() {
    let dir = TempDir::new("hash_version_ok");
    let shared_content = r#"
---!syaml/v0
---meta
file:
  version: "2.1.0"
---schema
{}
---data
x: 99
"#;
    dir.write("shared.syaml", shared_content);
    let hash = verify::compute_sha256(shared_content.as_bytes());

    dir.write(
        "root.syaml",
        &format!(
            r#"
---!syaml/v0
---meta
imports:
  shared:
    path: ./shared.syaml
    hash: {hash}
    version: ">=2.0.0, <3.0.0"
---schema
{{}}
---data
v: "${{shared.x}}"
"#
        ),
    );

    let compiled = compile(&dir.file_path("root.syaml"));
    assert_eq!(compiled["v"], json!(99));
}

// ---------------------------------------------------------------------------
// Signature verification
// ---------------------------------------------------------------------------

fn generate_test_keypair() -> (SigningKey, ed25519_dalek::VerifyingKey) {
    let signing_key = SigningKey::from_bytes(&[42u8; 32]);
    let verifying_key = signing_key.verifying_key();
    (signing_key, verifying_key)
}

#[test]
fn import_with_valid_signature_succeeds() {
    use base64::Engine;

    let dir = TempDir::new("sig_ok");
    let (signing_key, verifying_key) = generate_test_keypair();

    let shared_content = r#"
---!syaml/v0
---schema
{}
---data
value: signed
"#;
    dir.write("shared.syaml", shared_content);

    let signature = signing_key.sign(shared_content.as_bytes());
    let sig_b64 = base64::engine::general_purpose::STANDARD.encode(signature.to_bytes());

    // Write raw 32-byte public key
    fs::write(dir.file_path("pub.key"), verifying_key.as_bytes()).unwrap();

    dir.write(
        "root.syaml",
        &format!(
            r#"
---!syaml/v0
---meta
imports:
  shared:
    path: ./shared.syaml
    signature:
      public_key: ./pub.key
      value: "{sig_b64}"
---schema
{{}}
---data
v: "${{shared.value}}"
"#
        ),
    );

    let compiled = compile(&dir.file_path("root.syaml"));
    assert_eq!(compiled["v"], json!("signed"));
}

#[test]
fn import_with_invalid_signature_fails() {
    use base64::Engine;

    let dir = TempDir::new("sig_bad");
    let (_signing_key, verifying_key) = generate_test_keypair();

    let shared_content = r#"
---!syaml/v0
---schema
{}
---data
value: original
"#;
    dir.write("shared.syaml", shared_content);

    // Sign different content
    let other_key = SigningKey::from_bytes(&[99u8; 32]);
    let bad_signature = other_key.sign(b"different content");
    let sig_b64 = base64::engine::general_purpose::STANDARD.encode(bad_signature.to_bytes());

    fs::write(dir.file_path("pub.key"), verifying_key.as_bytes()).unwrap();

    dir.write(
        "root.syaml",
        &format!(
            r#"
---!syaml/v0
---meta
imports:
  shared:
    path: ./shared.syaml
    signature:
      public_key: ./pub.key
      value: "{sig_b64}"
---schema
{{}}
---data
v: "${{shared.value}}"
"#
        ),
    );

    let err = compile_document_from_path(dir.file_path("root.syaml"), &env_provider(&[]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("signature verification failed"));
}

#[test]
fn import_with_hash_signature_and_version_all_pass() {
    use base64::Engine;

    let dir = TempDir::new("all_verify");
    let (signing_key, verifying_key) = generate_test_keypair();

    let shared_content = r#"
---!syaml/v0
---meta
file:
  version: "3.0.1"
---schema
{}
---data
val: 777
"#;
    dir.write("shared.syaml", shared_content);

    let hash = verify::compute_sha256(shared_content.as_bytes());
    let signature = signing_key.sign(shared_content.as_bytes());
    let sig_b64 = base64::engine::general_purpose::STANDARD.encode(signature.to_bytes());

    fs::write(dir.file_path("pub.key"), verifying_key.as_bytes()).unwrap();

    dir.write(
        "root.syaml",
        &format!(
            r#"
---!syaml/v0
---meta
imports:
  shared:
    path: ./shared.syaml
    hash: {hash}
    signature:
      public_key: ./pub.key
      value: "{sig_b64}"
    version: "^3.0.0"
---schema
{{}}
---data
v: "${{shared.val}}"
"#
        ),
    );

    let compiled = compile(&dir.file_path("root.syaml"));
    assert_eq!(compiled["v"], json!(777));
}
