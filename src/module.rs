//! Module manifest parsing and module-level metadata/import injection.
//!
//! A *module* is a directory containing a `module.syaml` manifest file.
//! The manifest defines module-level metadata, import policy, and shared imports
//! that are inherited by all `.syaml` files within that directory tree.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value as JsonValue;

use crate::ast::{ImportBinding, ImportPolicy, Meta, ModuleManifest};
use crate::error::SyamlError;
use crate::section_scanner::scan_sections;

/// Name of the module manifest file.
pub const MANIFEST_FILENAME: &str = "module.syaml";

/// Name of the project registry file.
pub const REGISTRY_FILENAME: &str = "syaml.syaml";

// ---------------------------------------------------------------------------
// Manifest parsing
// ---------------------------------------------------------------------------

/// Parses a `module.syaml` manifest from its raw source text.
///
/// Validates that:
/// - The document has a `---module` section.
/// - `---data` and `---functional` sections are absent.
pub fn parse_module_manifest(input: &str) -> Result<ModuleManifest, SyamlError> {
    let (_version, sections) = scan_sections(input)?;

    // Enforce manifest-only constraints
    for sec in &sections {
        if matches!(sec.name.as_str(), "data" | "functional") {
            return Err(SyamlError::ModuleManifestError(format!(
                "'---{}' section is not allowed in module.syaml",
                sec.name
            )));
        }
    }

    let module_sec = sections
        .iter()
        .find(|s| s.name == "module")
        .ok_or_else(|| {
            SyamlError::ModuleManifestError(
                "module.syaml must contain a '---module' section".to_string(),
            )
        })?;

    let meta_sec = sections.iter().find(|s| s.name == "meta");

    parse_manifest_from_sections(&module_sec.body, meta_sec.map(|s| s.body.as_str()))
}

fn parse_manifest_from_sections(
    module_body: &str,
    meta_body: Option<&str>,
) -> Result<ModuleManifest, SyamlError> {
    let module_val = crate::mini_yaml::parse_document(module_body).map_err(|e| match e {
        SyamlError::YamlParseError { message, .. } => SyamlError::ModuleManifestError(format!(
            "---module section: {message}"
        )),
        other => other,
    })?;

    let module_map = module_val.as_object().ok_or_else(|| {
        SyamlError::ModuleManifestError("---module section must be a mapping".to_string())
    })?;

    let name = module_map
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            SyamlError::ModuleManifestError("---module must define a string 'name'".to_string())
        })?
        .to_string();

    let version = module_map
        .get("version")
        .and_then(|v| v.as_str())
        .map(String::from);

    let description = module_map
        .get("description")
        .and_then(|v| v.as_str())
        .map(String::from);

    let metadata: BTreeMap<String, JsonValue> = module_map
        .get("metadata")
        .and_then(|v| v.as_object())
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();

    let import_policy = module_map
        .get("import_policy")
        .map(parse_import_policy)
        .transpose()?
        .unwrap_or_default();

    // Parse meta section imports if present
    let imports = if let Some(body) = meta_body {
        let meta_val = crate::mini_yaml::parse_document(body).map_err(|e| match e {
            SyamlError::YamlParseError { message, .. } => {
                SyamlError::ModuleManifestError(format!("---meta section: {message}"))
            }
            other => other,
        })?;

        let meta_map = meta_val.as_object().ok_or_else(|| {
            SyamlError::ModuleManifestError("---meta section must be a mapping".to_string())
        })?;

        if let Some(imports_val) = meta_map.get("imports") {
            let imports_map = imports_val.as_object().ok_or_else(|| {
                SyamlError::ModuleManifestError(
                    "meta.imports must be a mapping".to_string(),
                )
            })?;
            let mut result = BTreeMap::new();
            for (alias, import_val) in imports_map {
                let binding = parse_manifest_import_binding(alias, import_val)?;
                result.insert(alias.clone(), binding);
            }
            result
        } else {
            BTreeMap::new()
        }
    } else {
        BTreeMap::new()
    };

    Ok(ModuleManifest {
        name,
        version,
        description,
        metadata,
        import_policy,
        imports,
    })
}

fn parse_import_policy(value: &JsonValue) -> Result<ImportPolicy, SyamlError> {
    let map = value.as_object().ok_or_else(|| {
        SyamlError::ModuleManifestError("import_policy must be a mapping".to_string())
    })?;

    let allow_network_imports = map
        .get("allow_network_imports")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let require_version = map
        .get("require_version")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let require_hash = map
        .get("require_hash")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let require_signature = map
        .get("require_signature")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let allowed_domains = map
        .get("allowed_domains")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let blocked_modules = map
        .get("blocked_modules")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    Ok(ImportPolicy {
        allow_network_imports,
        require_version,
        require_hash,
        require_signature,
        allowed_domains,
        blocked_modules,
    })
}

fn parse_manifest_import_binding(
    alias: &str,
    value: &JsonValue,
) -> Result<ImportBinding, SyamlError> {
    match value {
        JsonValue::String(path) => Ok(ImportBinding {
            path: path.clone(),
            hash: None,
            signature: None,
            version: None,
        }),
        JsonValue::Object(map) => {
            let path = map
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    SyamlError::ModuleManifestError(format!(
                        "meta.imports.{alias} must define string path"
                    ))
                })?
                .to_string();
            let hash = map.get("hash").and_then(|v| v.as_str()).map(String::from);
            let version = map
                .get("version")
                .and_then(|v| v.as_str())
                .map(String::from);
            Ok(ImportBinding {
                path,
                hash,
                signature: None,
                version,
            })
        }
        _ => Err(SyamlError::ModuleManifestError(format!(
            "meta.imports.{alias} must be string or mapping"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Module discovery
// ---------------------------------------------------------------------------

/// Walks up the directory tree from `file_path`'s parent, returning the path
/// to the nearest `module.syaml` found, or `None` if not found.
///
/// Stops at the project root (directory containing `syaml.syaml` or `.git`).
pub fn find_module_manifest(file_path: &Path) -> Option<PathBuf> {
    let start = file_path.parent()?;
    let project_root = find_project_root(start);

    let mut current = start;
    loop {
        let candidate = current.join(MANIFEST_FILENAME);
        if candidate.exists() {
            return Some(candidate);
        }

        // Stop if we've reached the project root
        if let Some(ref root) = project_root {
            if current == root {
                break;
            }
        }

        match current.parent() {
            Some(parent) => current = parent,
            None => break,
        }
    }
    None
}

/// Finds the project root: the nearest ancestor directory containing `syaml.syaml`
/// or `.git`.
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut current = start;
    loop {
        if current.join(REGISTRY_FILENAME).exists() || current.join(".git").exists() {
            return Some(current.to_path_buf());
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => return None,
        }
    }
}

// ---------------------------------------------------------------------------
// Project registry
// ---------------------------------------------------------------------------

/// Parses `syaml.syaml` and returns the `modules` data key as a map from module
/// name to directory path (relative to the registry file's directory).
pub fn load_module_registry(root: &Path) -> Result<BTreeMap<String, PathBuf>, SyamlError> {
    let registry_path = root.join(REGISTRY_FILENAME);
    let content = std::fs::read_to_string(&registry_path).map_err(|e| {
        SyamlError::ModuleManifestError(format!(
            "failed to read {}: {e}",
            registry_path.display()
        ))
    })?;
    parse_syaml_registry(&content, root)
}

/// Parses the `---data` section of a `syaml.syaml` registry file and extracts
/// the `modules` mapping of `name -> directory path`.
///
/// The registry format is:
/// ```yaml
/// ---!syaml/v0
/// ---data
/// modules:
///   payments: "services/payments/"
///   core: "shared/core/"
/// ```
fn parse_syaml_registry(content: &str, root: &Path) -> Result<BTreeMap<String, PathBuf>, SyamlError> {
    let (_version, sections) = scan_sections(content).map_err(|e| {
        SyamlError::ModuleManifestError(format!("invalid syaml.syaml: {e}"))
    })?;

    let data_sec = sections
        .iter()
        .find(|s| s.name == "data")
        .ok_or_else(|| {
            SyamlError::ModuleManifestError(
                "syaml.syaml must contain a '---data' section".to_string(),
            )
        })?;

    let data_val = crate::mini_yaml::parse_document(&data_sec.body).map_err(|e| match e {
        SyamlError::YamlParseError { message, .. } => {
            SyamlError::ModuleManifestError(format!("syaml.syaml ---data section: {message}"))
        }
        other => other,
    })?;

    let modules_val = data_val
        .as_object()
        .and_then(|m| m.get("modules"))
        .ok_or_else(|| {
            SyamlError::ModuleManifestError(
                "syaml.syaml must define a 'modules' mapping in ---data".to_string(),
            )
        })?;

    let modules_map = modules_val.as_object().ok_or_else(|| {
        SyamlError::ModuleManifestError("syaml.syaml 'modules' must be a mapping".to_string())
    })?;

    let mut result = BTreeMap::new();
    for (name, path_val) in modules_map {
        let path_str = path_val.as_str().ok_or_else(|| {
            SyamlError::ModuleManifestError(format!(
                "syaml.syaml modules.{name} must be a string path"
            ))
        })?;
        result.insert(name.clone(), root.join(path_str));
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// @module import resolution
// ---------------------------------------------------------------------------

/// Resolves a `@module_name` or `@module_name/file` import path to a filesystem
/// path, using the registry loaded from `syaml.syaml`.
pub fn resolve_module_import(
    at_path: &str,
    registry: &BTreeMap<String, PathBuf>,
) -> Result<PathBuf, SyamlError> {
    // at_path is like "@payments" or "@payments/invoice"
    let without_at = at_path.trim_start_matches('@');
    let (module_name, sub_path) = match without_at.split_once('/') {
        Some((name, rest)) => (name, Some(rest)),
        None => (without_at, None),
    };

    let module_dir = registry.get(module_name).ok_or_else(|| {
        SyamlError::ModuleNotFound(module_name.to_string())
    })?;

    let resolved = match sub_path {
        Some(file) => {
            let filename = if file.ends_with(".syaml") {
                file.to_string()
            } else {
                format!("{file}.syaml")
            };
            module_dir.join(filename)
        }
        None => module_dir.join(MANIFEST_FILENAME),
    };

    Ok(resolved)
}

// ---------------------------------------------------------------------------
// Module metadata injection
// ---------------------------------------------------------------------------

/// Merges module manifest metadata into the file's `meta.file` map (file-level wins),
/// and injects module-level imports into the file's `meta.imports` map (file-level shadows).
pub fn apply_module_to_meta(meta: &mut Option<Meta>, manifest: &ModuleManifest) {
    let meta = meta.get_or_insert_with(|| Meta {
        file: BTreeMap::new(),
        env: BTreeMap::new(),
        imports: BTreeMap::new(),
    });

    // Merge metadata: module values are the base, file-level overrides
    for (key, value) in &manifest.metadata {
        meta.file.entry(key.clone()).or_insert_with(|| value.clone());
    }

    // Inject module-level imports: file-level shadows module-level
    for (alias, binding) in &manifest.imports {
        meta.imports
            .entry(alias.clone())
            .or_insert_with(|| binding.clone());
    }
}

// ---------------------------------------------------------------------------
// Import policy enforcement
// ---------------------------------------------------------------------------

/// Validates all imports declared in a file against the enclosing module's policy.
///
/// `file_display` is used in error messages (e.g. the file path).
pub fn enforce_import_policy(
    imports: &BTreeMap<String, ImportBinding>,
    policy: &ImportPolicy,
    file_display: &str,
) -> Result<(), SyamlError> {
    for (alias, binding) in imports {
        let path = &binding.path;

        // Network import check
        if !policy.allow_network_imports && is_url(path) {
            return Err(SyamlError::ImportPolicyViolation {
                file: file_display.to_string(),
                reason: format!(
                    "import '{alias}' uses a network URL but allow_network_imports is false"
                ),
            });
        }

        // Allowed domains check
        if !policy.allowed_domains.is_empty() && is_url(path) {
            let domain = extract_url_domain(path);
            if !policy.allowed_domains.iter().any(|d| d == &domain) {
                return Err(SyamlError::ImportPolicyViolation {
                    file: file_display.to_string(),
                    reason: format!(
                        "import '{alias}' uses domain '{}' which is not in allowed_domains",
                        domain
                    ),
                });
            }
        }

        // Blocked modules check
        if path.starts_with('@') {
            let module_name = path
                .trim_start_matches('@')
                .split('/')
                .next()
                .unwrap_or("");
            if policy.blocked_modules.iter().any(|b| b == module_name) {
                return Err(SyamlError::ImportPolicyViolation {
                    file: file_display.to_string(),
                    reason: format!(
                        "import '{alias}' references blocked module '{module_name}'"
                    ),
                });
            }
        }

        // require_version check
        if policy.require_version && binding.version.is_none() {
            return Err(SyamlError::ImportPolicyViolation {
                file: file_display.to_string(),
                reason: format!(
                    "import '{alias}' must specify a version (require_version is true)"
                ),
            });
        }

        // require_hash check
        if policy.require_hash && binding.hash.is_none() {
            return Err(SyamlError::ImportPolicyViolation {
                file: file_display.to_string(),
                reason: format!(
                    "import '{alias}' must specify a hash (require_hash is true)"
                ),
            });
        }

        // require_signature check
        if policy.require_signature && binding.signature.is_none() {
            return Err(SyamlError::ImportPolicyViolation {
                file: file_display.to_string(),
                reason: format!(
                    "import '{alias}' must specify a signature (require_signature is true)"
                ),
            });
        }
    }

    Ok(())
}

fn is_url(path: &str) -> bool {
    path.starts_with("http://") || path.starts_with("https://")
}

fn extract_url_domain(url: &str) -> String {
    // Strip scheme
    let after_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    // Take up to first slash
    after_scheme
        .split('/')
        .next()
        .unwrap_or(after_scheme)
        .to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_binding(path: &str) -> ImportBinding {
        ImportBinding {
            path: path.to_string(),
            hash: None,
            signature: None,
            version: None,
        }
    }

    fn make_binding_with_version(path: &str) -> ImportBinding {
        ImportBinding {
            path: path.to_string(),
            hash: None,
            signature: None,
            version: Some("^1.0".to_string()),
        }
    }

    #[test]
    fn parse_valid_manifest() {
        let input = r#"---!syaml/v0

---module
name: payments
version: "1.0.0"
description: "Payment processing schemas"
metadata:
  owner: platform-team
  strict_field_numbers: true
import_policy:
  allow_network_imports: false

---meta
imports:
  infra: ./infra_common.syaml
"#;
        let manifest = parse_module_manifest(input).unwrap();
        assert_eq!(manifest.name, "payments");
        assert_eq!(manifest.version.as_deref(), Some("1.0.0"));
        assert_eq!(manifest.description.as_deref(), Some("Payment processing schemas"));
        assert_eq!(
            manifest.metadata.get("owner").and_then(|v| v.as_str()),
            Some("platform-team")
        );
        assert!(!manifest.import_policy.allow_network_imports);
        assert!(manifest.imports.contains_key("infra"));
    }

    #[test]
    fn manifest_missing_module_section_errors() {
        let input = r#"---!syaml/v0
---schema
{}
"#;
        let err = parse_module_manifest(input).unwrap_err();
        assert!(err.to_string().contains("---module"), "{err}");
    }

    #[test]
    fn manifest_with_data_section_errors() {
        let input = r#"---!syaml/v0
---module
name: foo
---data
x: 1
"#;
        let err = parse_module_manifest(input).unwrap_err();
        assert!(err.to_string().contains("---data"), "{err}");
    }

    #[test]
    fn manifest_with_functional_section_errors() {
        let input = r#"---!syaml/v0
---module
name: foo
---functional
{}
"#;
        let err = parse_module_manifest(input).unwrap_err();
        assert!(err.to_string().contains("---functional"), "{err}");
    }

    #[test]
    fn apply_module_metadata_merges_with_file_wins() {
        let manifest = ModuleManifest {
            name: "mod".to_string(),
            version: None,
            description: None,
            metadata: {
                let mut m = BTreeMap::new();
                m.insert("owner".to_string(), JsonValue::String("module-team".to_string()));
                m.insert("base_key".to_string(), JsonValue::String("from-module".to_string()));
                m
            },
            import_policy: ImportPolicy::default(),
            imports: BTreeMap::new(),
        };

        let mut meta = Some(Meta {
            file: {
                let mut f = BTreeMap::new();
                f.insert("owner".to_string(), JsonValue::String("file-team".to_string()));
                f
            },
            env: BTreeMap::new(),
            imports: BTreeMap::new(),
        });

        apply_module_to_meta(&mut meta, &manifest);

        let file = &meta.unwrap().file;
        // File-level wins
        assert_eq!(file["owner"].as_str(), Some("file-team"));
        // Module key injected when not already set
        assert_eq!(file["base_key"].as_str(), Some("from-module"));
    }

    #[test]
    fn apply_module_imports_file_shadows() {
        let manifest = ModuleManifest {
            name: "mod".to_string(),
            version: None,
            description: None,
            metadata: BTreeMap::new(),
            import_policy: ImportPolicy::default(),
            imports: {
                let mut m = BTreeMap::new();
                m.insert("shared".to_string(), make_binding("./module_shared.syaml"));
                m.insert("extra".to_string(), make_binding("./extra.syaml"));
                m
            },
        };

        let mut meta = Some(Meta {
            file: BTreeMap::new(),
            env: BTreeMap::new(),
            imports: {
                let mut i = BTreeMap::new();
                // File declares its own "shared" — should not be overwritten
                i.insert("shared".to_string(), make_binding("./file_shared.syaml"));
                i
            },
        });

        apply_module_to_meta(&mut meta, &manifest);

        let imports = &meta.unwrap().imports;
        // File-level import wins
        assert_eq!(imports["shared"].path, "./file_shared.syaml");
        // Module import injected
        assert_eq!(imports["extra"].path, "./extra.syaml");
    }

    #[test]
    fn policy_rejects_network_import_when_disabled() {
        let policy = ImportPolicy {
            allow_network_imports: false,
            ..Default::default()
        };
        let mut imports = BTreeMap::new();
        imports.insert("remote".to_string(), make_binding("https://example.com/schema.syaml"));

        let err = enforce_import_policy(&imports, &policy, "test.syaml").unwrap_err();
        assert!(err.to_string().contains("allow_network_imports"), "{err}");
    }

    #[test]
    fn policy_allows_network_import_when_enabled() {
        let policy = ImportPolicy::default(); // allow_network_imports: true
        let mut imports = BTreeMap::new();
        imports.insert("remote".to_string(), make_binding("https://example.com/schema.syaml"));
        enforce_import_policy(&imports, &policy, "test.syaml").unwrap();
    }

    #[test]
    fn policy_rejects_missing_version_when_required() {
        let policy = ImportPolicy {
            require_version: true,
            ..Default::default()
        };
        let mut imports = BTreeMap::new();
        imports.insert("dep".to_string(), make_binding("./dep.syaml"));

        let err = enforce_import_policy(&imports, &policy, "test.syaml").unwrap_err();
        assert!(err.to_string().contains("require_version"), "{err}");
    }

    #[test]
    fn policy_accepts_import_with_version_when_required() {
        let policy = ImportPolicy {
            require_version: true,
            ..Default::default()
        };
        let mut imports = BTreeMap::new();
        imports.insert("dep".to_string(), make_binding_with_version("./dep.syaml"));
        enforce_import_policy(&imports, &policy, "test.syaml").unwrap();
    }

    #[test]
    fn policy_rejects_missing_hash_when_required() {
        let policy = ImportPolicy {
            require_hash: true,
            ..Default::default()
        };
        let mut imports = BTreeMap::new();
        imports.insert("dep".to_string(), make_binding("./dep.syaml"));

        let err = enforce_import_policy(&imports, &policy, "test.syaml").unwrap_err();
        assert!(err.to_string().contains("require_hash"), "{err}");
    }

    #[test]
    fn policy_rejects_blocked_module() {
        let policy = ImportPolicy {
            blocked_modules: vec!["evil".to_string()],
            ..Default::default()
        };
        let mut imports = BTreeMap::new();
        imports.insert("e".to_string(), make_binding("@evil/schema"));

        let err = enforce_import_policy(&imports, &policy, "test.syaml").unwrap_err();
        assert!(err.to_string().contains("blocked module"), "{err}");
    }

    #[test]
    fn policy_rejects_disallowed_domain() {
        let policy = ImportPolicy {
            allowed_domains: vec!["trusted.com".to_string()],
            ..Default::default()
        };
        let mut imports = BTreeMap::new();
        imports.insert(
            "bad".to_string(),
            make_binding("https://untrusted.com/schema.syaml"),
        );

        let err = enforce_import_policy(&imports, &policy, "test.syaml").unwrap_err();
        assert!(err.to_string().contains("allowed_domains"), "{err}");
    }

    #[test]
    fn policy_accepts_allowed_domain() {
        let policy = ImportPolicy {
            allowed_domains: vec!["trusted.com".to_string()],
            ..Default::default()
        };
        let mut imports = BTreeMap::new();
        imports.insert(
            "ok".to_string(),
            make_binding("https://trusted.com/schema.syaml"),
        );
        enforce_import_policy(&imports, &policy, "test.syaml").unwrap();
    }

    #[test]
    fn resolve_module_import_simple() {
        let mut registry = BTreeMap::new();
        registry.insert(
            "payments".to_string(),
            PathBuf::from("/project/services/payments"),
        );

        let path = resolve_module_import("@payments", &registry).unwrap();
        assert_eq!(path, PathBuf::from("/project/services/payments/module.syaml"));
    }

    #[test]
    fn resolve_module_import_with_file() {
        let mut registry = BTreeMap::new();
        registry.insert(
            "payments".to_string(),
            PathBuf::from("/project/services/payments"),
        );

        let path = resolve_module_import("@payments/invoice", &registry).unwrap();
        assert_eq!(path, PathBuf::from("/project/services/payments/invoice.syaml"));
    }

    #[test]
    fn resolve_module_import_with_syaml_extension() {
        let mut registry = BTreeMap::new();
        registry.insert(
            "payments".to_string(),
            PathBuf::from("/project/services/payments"),
        );

        let path = resolve_module_import("@payments/invoice.syaml", &registry).unwrap();
        assert_eq!(path, PathBuf::from("/project/services/payments/invoice.syaml"));
    }

    #[test]
    fn resolve_module_import_unknown_module_errors() {
        let registry: BTreeMap<String, PathBuf> = BTreeMap::new();
        let err = resolve_module_import("@unknown", &registry).unwrap_err();
        assert!(err.to_string().contains("module not found"), "{err}");
    }

    #[test]
    fn parse_syaml_registry_basic() {
        let content = r#"---!syaml/v0
---data
modules:
  payments: "services/payments/"
  core: "shared/core/"
"#;
        let result = parse_syaml_registry(content, Path::new("/project")).unwrap();
        assert_eq!(result["payments"], PathBuf::from("/project/services/payments/"));
        assert_eq!(result["core"], PathBuf::from("/project/shared/core/"));
    }

    #[test]
    fn parse_syaml_registry_missing_data_section_errors() {
        let content = "---!syaml/v0\n---schema\n{}\n";
        let err = parse_syaml_registry(content, Path::new("/project")).unwrap_err();
        assert!(err.to_string().contains("---data"), "{err}");
    }

    #[test]
    fn parse_syaml_registry_missing_modules_key_errors() {
        let content = "---!syaml/v0\n---data\nother: value\n";
        let err = parse_syaml_registry(content, Path::new("/project")).unwrap_err();
        assert!(err.to_string().contains("modules"), "{err}");
    }
}
