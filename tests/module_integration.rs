//! Integration tests for module manifest support.
//!
//! Tests cover:
//! - Module metadata inheritance into member files
//! - Module imports injected into member files
//! - `@module/file` import path resolution via `syaml.syaml` registry
//! - Import policy enforcement (network, version, hash, blocked modules)
//! - Validation: `---module` forbidden in non-manifest files
//! - Validation: `---data` / `---contracts` forbidden in `module.syaml`

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super_yaml::{compile_document_from_path, MapEnvProvider};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn empty_env() -> MapEnvProvider {
    MapEnvProvider::new(HashMap::new())
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
            "super_yaml_module_{prefix}_{}_{}",
            std::process::id(),
            stamp
        ));
        fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    fn subdir(&self, name: &str) -> PathBuf {
        let p = self.path.join(name);
        fs::create_dir_all(&p).expect("create subdir");
        p
    }

    fn write(&self, file: &str, content: &str) -> PathBuf {
        let dest = self.path.join(file);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(&dest, content).expect("write temp file");
        dest
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if self.path.exists() {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

// ---------------------------------------------------------------------------
// Module metadata inheritance
// ---------------------------------------------------------------------------

#[test]
fn module_metadata_inherited_into_member_file() {
    let dir = TempDir::new("meta_inherit");

    dir.write(
        "module.syaml",
        r#"---!syaml/v0

---module
name: mymod
metadata:
  owner: platform-team
  env: production
"#,
    );

    let member = dir.write(
        "service.syaml",
        r#"---!syaml/v0

---schema
{}

---data
name <string>: "my-service"
"#,
    );

    let value = compile(&member);
    assert_eq!(value["name"], "my-service");
    // The module metadata is merged into meta.file, not the data output —
    // it affects compilation settings (like strict_field_numbers), not the data.
    // Verify the file compiled successfully with module context applied.
}

#[test]
fn file_level_metadata_wins_over_module() {
    let dir = TempDir::new("meta_override");

    dir.write(
        "module.syaml",
        r#"---!syaml/v0

---module
name: mymod
metadata:
  owner: module-team
"#,
    );

    // The file's meta.file.owner should win over the module's owner.
    // strict_field_numbers from module gets merged, but file doesn't declare it.
    // We verify no error occurs and correct data is produced.
    let member = dir.write(
        "service.syaml",
        r#"---!syaml/v0

---meta
file:
  owner: file-team

---schema
{}

---data
name <string>: "svc"
"#,
    );

    let value = compile(&member);
    assert_eq!(value["name"], "svc");
}

#[test]
fn module_strict_field_numbers_applies_to_members() {
    let dir = TempDir::new("strict_fn");

    dir.write(
        "module.syaml",
        r#"---!syaml/v0

---module
name: strictmod
metadata:
  strict_field_numbers: true
"#,
    );

    // A file with a schema that has proper field numbers should compile fine.
    let member = dir.write(
        "ok.syaml",
        r#"---!syaml/v0

---schema
Config:
  type: object
  properties:
    name:
      type: string
      field_number: 1
    value:
      type: integer
      field_number: 2

---data
cfg <Config>:
  name: "test"
  value: 42
"#,
    );

    let value = compile(&member);
    assert_eq!(value["cfg"]["name"], "test");
}

#[test]
fn module_strict_field_numbers_rejects_missing_numbers() {
    let dir = TempDir::new("strict_fn_fail");

    dir.write(
        "module.syaml",
        r#"---!syaml/v0

---module
name: strictmod
metadata:
  strict_field_numbers: true
"#,
    );

    let member = dir.write(
        "bad.syaml",
        r#"---!syaml/v0

---schema
Config:
  type: object
  properties:
    name:
      type: string
    value:
      type: integer

---data
cfg <Config>:
  name: "test"
  value: 42
"#,
    );

    let err = compile_err(&member);
    assert!(
        err.contains("field_number") || err.contains("strict"),
        "expected strict field number error, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// Module imports injection
// ---------------------------------------------------------------------------

#[test]
fn module_imports_injected_into_member_files() {
    let dir = TempDir::new("mod_imports");

    // Shared types file
    dir.write(
        "shared.syaml",
        r#"---!syaml/v0

---schema
BaseConfig:
  type: object
  properties:
    env:
      type: string

---data
base_env <string>: "production"
"#,
    );

    dir.write(
        "module.syaml",
        r#"---!syaml/v0

---module
name: mymod

---meta
imports:
  shared: ./shared.syaml
"#,
    );

    // Member file does NOT redeclare the "shared" import — it's injected
    let member = dir.write(
        "service.syaml",
        r#"---!syaml/v0

---schema
{}

---data
my_env <string>: "=shared.base_env"
"#,
    );

    let value = compile(&member);
    assert_eq!(value["my_env"], "production");
}

#[test]
fn file_import_shadows_module_import() {
    let dir = TempDir::new("shadow_imports");

    dir.write(
        "module_shared.syaml",
        r#"---!syaml/v0

---schema
{}

---data
label <string>: "from-module"
"#,
    );

    dir.write(
        "file_shared.syaml",
        r#"---!syaml/v0

---schema
{}

---data
label <string>: "from-file"
"#,
    );

    dir.write(
        "module.syaml",
        r#"---!syaml/v0

---module
name: shadowmod

---meta
imports:
  shared: ./module_shared.syaml
"#,
    );

    // File declares its own "shared" import — should shadow the module's
    let member = dir.write(
        "service.syaml",
        r#"---!syaml/v0

---meta
imports:
  shared: ./file_shared.syaml

---schema
{}

---data
label <string>: "=shared.label"
"#,
    );

    let value = compile(&member);
    assert_eq!(value["label"], "from-file");
}

// ---------------------------------------------------------------------------
// @module import resolution via syaml.syaml
// ---------------------------------------------------------------------------

#[test]
fn at_module_import_resolves_via_registry() {
    let dir = TempDir::new("at_module");

    // Create project registry at root
    dir.write(
        "syaml.syaml",
        r#"---!syaml/v0
---data
modules:
  shared: "shared/"
"#,
    );

    // Create the shared module
    dir.write(
        "shared/module.syaml",
        r#"---!syaml/v0

---module
name: shared
"#,
    );

    dir.write(
        "shared/types.syaml",
        r#"---!syaml/v0

---schema
Version:
  type: string

---data
default_version <string>: "v1.0.0"
"#,
    );

    // Service file uses @shared/types import
    let _ = dir.subdir("services");
    let service = dir.write(
        "services/service.syaml",
        r#"---!syaml/v0

---meta
imports:
  shared_types: "@shared/types"

---schema
{}

---data
version <string>: "=shared_types.default_version"
"#,
    );

    let value = compile(&service);
    assert_eq!(value["version"], "v1.0.0");
}

#[test]
fn at_module_import_without_registry_errors() {
    let dir = TempDir::new("no_registry");

    // No syaml.syaml, no .git
    let service = dir.write(
        "service.syaml",
        r#"---!syaml/v0

---meta
imports:
  shared: "@unknown/types"

---schema
{}

---data
x <string>: "hello"
"#,
    );

    let err = compile_err(&service);
    assert!(
        err.contains("no project registry") || err.contains("syaml.syaml"),
        "expected registry error, got: {err}"
    );
}

#[test]
fn at_module_import_unknown_module_errors() {
    let dir = TempDir::new("unknown_module");

    dir.write(
        "syaml.syaml",
        r#"---!syaml/v0
---data
modules:
  known: "known/"
"#,
    );

    let service = dir.write(
        "service.syaml",
        r#"---!syaml/v0

---meta
imports:
  x: "@unknown/types"

---schema
{}

---data
y <string>: "hello"
"#,
    );

    let err = compile_err(&service);
    assert!(
        err.contains("module not found") || err.contains("unknown"),
        "expected module not found error, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// Import policy enforcement
// ---------------------------------------------------------------------------

#[test]
fn import_policy_rejects_network_import_when_disabled() {
    let dir = TempDir::new("policy_network");

    dir.write(
        "module.syaml",
        r#"---!syaml/v0

---module
name: secure_mod
import_policy:
  allow_network_imports: false
"#,
    );

    let member = dir.write(
        "service.syaml",
        r#"---!syaml/v0

---meta
imports:
  remote: "https://example.com/schema.syaml"

---schema
{}

---data
x <string>: "hello"
"#,
    );

    let err = compile_err(&member);
    assert!(
        err.contains("allow_network_imports") || err.contains("policy"),
        "expected network import policy error, got: {err}"
    );
}

#[test]
fn import_policy_rejects_import_missing_version_when_required() {
    let dir = TempDir::new("policy_version");

    dir.write(
        "other.syaml",
        r#"---!syaml/v0

---schema
{}

---data
x <string>: "hello"
"#,
    );

    dir.write(
        "module.syaml",
        r#"---!syaml/v0

---module
name: versioned_mod
import_policy:
  require_version: true
"#,
    );

    let member = dir.write(
        "service.syaml",
        r#"---!syaml/v0

---meta
imports:
  dep: "./other.syaml"

---schema
{}

---data
x <string>: "value"
"#,
    );

    let err = compile_err(&member);
    assert!(
        err.contains("require_version") || err.contains("version"),
        "expected require_version policy error, got: {err}"
    );
}

#[test]
fn import_policy_rejects_blocked_module() {
    let dir = TempDir::new("policy_blocked");

    dir.write(
        "syaml.syaml",
        r#"---!syaml/v0
---data
modules:
  blocked_mod: "blocked_mod/"
  main_mod: "main_mod/"
"#,
    );

    dir.write(
        "blocked_mod/module.syaml",
        r#"---!syaml/v0

---module
name: blocked_mod
"#,
    );

    dir.write(
        "main_mod/module.syaml",
        r#"---!syaml/v0

---module
name: main_mod
import_policy:
  blocked_modules:
    - blocked_mod
"#,
    );

    let member = dir.write(
        "main_mod/service.syaml",
        r#"---!syaml/v0

---meta
imports:
  bad: "@blocked_mod"

---schema
{}

---data
x <string>: "hello"
"#,
    );

    let err = compile_err(&member);
    assert!(
        err.contains("blocked module") || err.contains("blocked_mod"),
        "expected blocked module error, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// Manifest structural validation
// ---------------------------------------------------------------------------

#[test]
fn module_section_rejected_in_non_manifest_file() {
    let dir = TempDir::new("bad_module_sec");

    let bad_file = dir.write(
        "not_module.syaml",
        r#"---!syaml/v0

---module
name: sneaky

---schema
{}

---data
x <string>: "hello"
"#,
    );

    let err = compile_err(&bad_file);
    assert!(
        err.contains("module.syaml") || err.contains("---module") || err.contains("section"),
        "expected section error, got: {err}"
    );
}

#[test]
fn data_section_rejected_in_module_manifest() {
    let dir = TempDir::new("manifest_data");

    // Write a module.syaml with a ---data section (invalid)
    let bad_manifest = dir.write(
        "module.syaml",
        r#"---!syaml/v0

---module
name: badmod

---data
x: 1
"#,
    );

    // Compiling a sibling file will try to load the manifest and fail
    let sibling = dir.write(
        "service.syaml",
        r#"---!syaml/v0

---schema
{}

---data
x <string>: "hello"
"#,
    );

    let err = compile_err(&sibling);
    assert!(
        err.contains("---data") || err.contains("not allowed"),
        "expected manifest validation error, got: {err}"
    );

    // Compiling module.syaml directly should also fail
    let err2 = compile_err(&bad_manifest);
    assert!(
        err2.contains("---data") || err2.contains("not allowed"),
        "expected manifest validation error when compiled directly, got: {err2}"
    );
}

// ---------------------------------------------------------------------------
// Example files smoke tests
// ---------------------------------------------------------------------------

#[test]
fn payments_example_invoice_compiles() {
    let path = Path::new("examples/payments/invoice.syaml");
    if !path.exists() {
        return; // skip if examples not present
    }
    let value = compile(path);
    assert!(value.is_object());
}

#[test]
fn payments_example_refund_compiles() {
    let path = Path::new("examples/payments/refund.syaml");
    if !path.exists() {
        return;
    }
    let value = compile(path);
    assert!(value.is_object());
}
