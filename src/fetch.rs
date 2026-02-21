//! URL-based import fetching, disk caching, and lockfile management.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::SyamlError;
use crate::verify;

/// Distinguishes local filesystem imports from remote URL imports.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum ImportSource {
    /// A local file path (already canonicalized).
    Local(PathBuf),
    /// A remote URL with its local cache path.
    Remote {
        url: String,
        cache_path: PathBuf,
    },
}

impl ImportSource {
    /// Returns the canonical path used for caching and cycle detection.
    pub fn canonical_path(&self) -> &Path {
        match self {
            ImportSource::Local(p) => p,
            ImportSource::Remote { cache_path, .. } => cache_path,
        }
    }

    /// Returns a display-friendly identifier (path or URL).
    pub fn display_id(&self) -> String {
        match self {
            ImportSource::Local(p) => p.display().to_string(),
            ImportSource::Remote { url, .. } => url.clone(),
        }
    }
}

/// Determines whether a raw import path is a URL or a local filesystem path,
/// and resolves it to an `ImportSource`.
pub fn resolve_import_source(
    base_dir: &Path,
    raw_path: &str,
    ctx: &FetchContext,
) -> Result<ImportSource, SyamlError> {
    let trimmed = raw_path.trim();
    if trimmed.is_empty() {
        return Err(SyamlError::ImportError(
            "import path must be a non-empty string".to_string(),
        ));
    }

    if is_url(trimmed) {
        let cache_path = url_cache_path(trimmed, &ctx.cache_dir);
        Ok(ImportSource::Remote {
            url: trimmed.to_string(),
            cache_path,
        })
    } else {
        let path = Path::new(trimmed);
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            base_dir.join(path)
        };
        let canonical = fs::canonicalize(&resolved).map_err(|e| {
            SyamlError::ImportError(format!(
                "failed to resolve import path '{}': {e}",
                resolved.display()
            ))
        })?;
        Ok(ImportSource::Local(canonical))
    }
}

/// Resolves a relative import path from a parent source.
///
/// If the parent is a remote URL, relative paths are resolved as relative URLs.
/// If the parent is local, relative paths resolve against the filesystem.
pub fn resolve_import_source_from_parent(
    parent: &ImportSource,
    raw_path: &str,
    ctx: &FetchContext,
) -> Result<ImportSource, SyamlError> {
    let trimmed = raw_path.trim();
    if is_url(trimmed) {
        let cache_path = url_cache_path(trimmed, &ctx.cache_dir);
        return Ok(ImportSource::Remote {
            url: trimmed.to_string(),
            cache_path,
        });
    }

    match parent {
        ImportSource::Local(parent_path) => {
            let base_dir = parent_path.parent().ok_or_else(|| {
                SyamlError::ImportError(format!(
                    "failed to resolve parent directory for '{}'",
                    parent_path.display()
                ))
            })?;
            resolve_import_source(base_dir, trimmed, ctx)
        }
        ImportSource::Remote { url, .. } => {
            let base_url = url_directory(url);
            let resolved_url = resolve_relative_url(&base_url, trimmed);
            let cache_path = url_cache_path(&resolved_url, &ctx.cache_dir);
            Ok(ImportSource::Remote {
                url: resolved_url,
                cache_path,
            })
        }
    }
}

/// Reads the content of an import source, fetching from network if needed.
///
/// For remote sources, checks the lockfile cache first. On cache miss, performs
/// an HTTP GET and writes the result to the disk cache and lockfile.
pub fn read_import_source(
    source: &ImportSource,
    ctx: &mut FetchContext,
) -> Result<String, SyamlError> {
    match source {
        ImportSource::Local(path) => fs::read_to_string(path).map_err(|e| {
            SyamlError::ImportError(format!(
                "failed to read import '{}': {e}",
                path.display()
            ))
        }),
        ImportSource::Remote { url, cache_path } => {
            if !ctx.force_update {
                if let Some(content) = try_read_cached(cache_path, url, &ctx.lockfile) {
                    return Ok(content);
                }
            }

            let content = fetch_url(url)?;

            fs::create_dir_all(cache_path.parent().unwrap_or(Path::new("."))).map_err(|e| {
                SyamlError::FetchError(format!(
                    "failed to create cache directory: {e}"
                ))
            })?;
            fs::write(cache_path, &content).map_err(|e| {
                SyamlError::FetchError(format!(
                    "failed to write cache file '{}': {e}",
                    cache_path.display()
                ))
            })?;

            let hash = verify::compute_sha256(content.as_bytes());
            ctx.lockfile.entries.insert(
                url.clone(),
                LockEntry {
                    hash,
                    file_version: None,
                    cached_path: cache_path.display().to_string(),
                    fetched_at: now_iso8601(),
                },
            );
            ctx.lockfile_dirty = true;

            Ok(content)
        }
    }
}

/// Updates the lockfile entry with the resolved version from the imported document.
pub fn update_lockfile_version(
    source: &ImportSource,
    version: Option<&str>,
    ctx: &mut FetchContext,
) {
    if let ImportSource::Remote { url, .. } = source {
        if let Some(entry) = ctx.lockfile.entries.get_mut(url) {
            entry.file_version = version.map(String::from);
            ctx.lockfile_dirty = true;
        }
    }
}

/// Flushes the lockfile to disk if it has been modified.
pub fn flush_lockfile(ctx: &FetchContext) -> Result<(), SyamlError> {
    if !ctx.lockfile_dirty {
        return Ok(());
    }
    if let Some(ref path) = ctx.lockfile_path {
        let json = serde_json::to_string_pretty(&ctx.lockfile).map_err(|e| {
            SyamlError::FetchError(format!("failed to serialize lockfile: {e}"))
        })?;
        fs::write(path, json).map_err(|e| {
            SyamlError::FetchError(format!(
                "failed to write lockfile '{}': {e}",
                path.display()
            ))
        })?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// FetchContext
// ---------------------------------------------------------------------------

/// Shared context for URL fetching, caching, and lockfile management.
pub struct FetchContext {
    pub cache_dir: PathBuf,
    pub lockfile_path: Option<PathBuf>,
    pub lockfile: Lockfile,
    pub lockfile_dirty: bool,
    pub force_update: bool,
}

impl FetchContext {
    pub fn new(root_dir: &Path, cache_dir: Option<PathBuf>, force_update: bool) -> Self {
        let cache_dir = cache_dir
            .or_else(|| std::env::var("SYAML_CACHE_DIR").ok().map(PathBuf::from))
            .unwrap_or_else(default_cache_dir);

        let lockfile_path = root_dir.join("syaml.lock");
        let lockfile = read_lockfile(&lockfile_path);

        Self {
            cache_dir,
            lockfile_path: Some(lockfile_path),
            lockfile,
            lockfile_dirty: false,
            force_update,
        }
    }

    /// Creates a no-op context that disables lockfile and uses a temp cache dir.
    pub fn disabled() -> Self {
        Self {
            cache_dir: std::env::temp_dir().join("super_yaml_cache"),
            lockfile_path: None,
            lockfile: Lockfile::default(),
            lockfile_dirty: false,
            force_update: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Lockfile
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Lockfile {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub entries: BTreeMap<String, LockEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockEntry {
    pub hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_version: Option<String>,
    pub cached_path: String,
    pub fetched_at: String,
}

pub fn read_lockfile(path: &Path) -> Lockfile {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Lockfile {
            version: 1,
            ..Default::default()
        })
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn is_url(path: &str) -> bool {
    path.starts_with("http://") || path.starts_with("https://")
}

fn default_cache_dir() -> PathBuf {
    dirs_fallback().join("super_yaml")
}

fn dirs_fallback() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".cache")
    } else {
        std::env::temp_dir().join(".cache")
    }
}

fn url_cache_path(url: &str, cache_dir: &Path) -> PathBuf {
    let hash = verify::compute_sha256(url.as_bytes());
    let hex = hash.strip_prefix("sha256:").unwrap_or(&hash);
    let short = &hex[..16.min(hex.len())];
    let filename = format!("{short}.syaml");
    cache_dir.join(filename)
}

fn url_directory(url: &str) -> String {
    if let Some(pos) = url.rfind('/') {
        url[..=pos].to_string()
    } else {
        url.to_string()
    }
}

fn resolve_relative_url(base: &str, relative: &str) -> String {
    if relative.starts_with('/') {
        if let Some(scheme_end) = base.find("://") {
            let after_scheme = &base[scheme_end + 3..];
            if let Some(slash) = after_scheme.find('/') {
                return format!("{}{}", &base[..scheme_end + 3 + slash], relative);
            }
        }
        return format!("{}{}", base.trim_end_matches('/'), relative);
    }

    let mut result = base.to_string();
    for segment in relative.split('/') {
        match segment {
            ".." => {
                if result.ends_with('/') {
                    result.pop();
                }
                if let Some(pos) = result.rfind('/') {
                    result.truncate(pos + 1);
                }
            }
            "." => {}
            other => {
                if !result.ends_with('/') {
                    result.push('/');
                }
                result.push_str(other);
            }
        }
    }
    result
}

fn try_read_cached(cache_path: &Path, url: &str, lockfile: &Lockfile) -> Option<String> {
    let entry = lockfile.entries.get(url)?;
    if !cache_path.exists() {
        return None;
    }
    let content = fs::read_to_string(cache_path).ok()?;
    let actual_hash = verify::compute_sha256(content.as_bytes());
    if actual_hash == entry.hash {
        Some(content)
    } else {
        None
    }
}

fn fetch_url(url: &str) -> Result<String, SyamlError> {
    let body = ureq::get(url)
        .call()
        .map_err(|e| SyamlError::FetchError(format!("HTTP request to '{}' failed: {e}", url)))?
        .into_body()
        .read_to_string()
        .map_err(|e| {
            SyamlError::FetchError(format!(
                "failed to read response body from '{}': {e}",
                url
            ))
        })?;
    Ok(body)
}

fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs();
    let days = secs / 86400;
    let time_secs = secs % 86400;
    let hours = time_secs / 3600;
    let mins = (time_secs % 3600) / 60;
    let s = time_secs % 60;

    // Approximate date from epoch days (good enough for timestamps)
    let (y, m, d) = epoch_days_to_date(days);
    format!("{y:04}-{m:02}-{d:02}T{hours:02}:{mins:02}:{s:02}Z")
}

fn epoch_days_to_date(days: u64) -> (u64, u64, u64) {
    // Civil days algorithm (Howard Hinnant)
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_url_detects_http_and_https() {
        assert!(is_url("https://example.com/file.syaml"));
        assert!(is_url("http://localhost:8080/test"));
        assert!(!is_url("./local.syaml"));
        assert!(!is_url("/absolute/path.syaml"));
    }

    #[test]
    fn resolve_relative_url_handles_simple_cases() {
        assert_eq!(
            resolve_relative_url("https://example.com/configs/", "shared.syaml"),
            "https://example.com/configs/shared.syaml"
        );
        assert_eq!(
            resolve_relative_url("https://example.com/configs/", "../other/file.syaml"),
            "https://example.com/other/file.syaml"
        );
        assert_eq!(
            resolve_relative_url("https://example.com/configs/", "./file.syaml"),
            "https://example.com/configs/file.syaml"
        );
    }

    #[test]
    fn resolve_relative_url_handles_absolute_path() {
        assert_eq!(
            resolve_relative_url("https://example.com/configs/", "/root.syaml"),
            "https://example.com/root.syaml"
        );
    }

    #[test]
    fn url_directory_strips_filename() {
        assert_eq!(
            url_directory("https://example.com/configs/file.syaml"),
            "https://example.com/configs/"
        );
        assert_eq!(
            url_directory("https://example.com/file.syaml"),
            "https://example.com/"
        );
    }

    #[test]
    fn lockfile_round_trip() {
        let lock = Lockfile {
            version: 1,
            entries: BTreeMap::from([(
                "https://example.com/shared.syaml".to_string(),
                LockEntry {
                    hash: "sha256:abcdef".to_string(),
                    file_version: Some("1.2.3".to_string()),
                    cached_path: "/tmp/cache/abc.syaml".to_string(),
                    fetched_at: "2026-02-20T12:00:00Z".to_string(),
                },
            )]),
        };
        let json = serde_json::to_string_pretty(&lock).unwrap();
        let parsed: Lockfile = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.version, 1);
        assert!(parsed.entries.contains_key("https://example.com/shared.syaml"));
        let entry = &parsed.entries["https://example.com/shared.syaml"];
        assert_eq!(entry.file_version.as_deref(), Some("1.2.3"));
    }

    #[test]
    fn resolve_import_source_local() {
        let ctx = FetchContext::disabled();
        let dir = std::env::temp_dir();
        let tmp = dir.join("syaml_test_resolve_local.syaml");
        fs::write(&tmp, "placeholder").unwrap();
        let source = resolve_import_source(&dir, tmp.to_str().unwrap(), &ctx).unwrap();
        assert!(matches!(source, ImportSource::Local(_)));
        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn resolve_import_source_url() {
        let ctx = FetchContext::disabled();
        let dir = std::env::temp_dir();
        let source = resolve_import_source(&dir, "https://example.com/file.syaml", &ctx).unwrap();
        assert!(matches!(source, ImportSource::Remote { .. }));
    }
}
