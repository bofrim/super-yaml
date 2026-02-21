//! Import integrity verification: hash, signature, and version checks.

use std::fs;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::ast::{Meta, SignatureBinding};
use crate::error::SyamlError;

/// Verifies that `content` matches the expected hash string.
///
/// `expected` must be in `algorithm:hex_digest` format (e.g. `sha256:abcdef01...`).
/// Only `sha256` is currently supported.
pub fn verify_hash(content: &[u8], expected: &str) -> Result<(), SyamlError> {
    let (algo, expected_hex) = expected.split_once(':').ok_or_else(|| {
        SyamlError::HashError(format!(
            "invalid hash format '{}'; expected 'algorithm:hex_digest'",
            expected
        ))
    })?;

    match algo {
        "sha256" => {
            let mut hasher = Sha256::new();
            hasher.update(content);
            let actual_hex = hex::encode(hasher.finalize());
            if actual_hex != expected_hex {
                return Err(SyamlError::HashError(format!(
                    "sha256 mismatch: expected {expected_hex}, got {actual_hex}"
                )));
            }
            Ok(())
        }
        _ => Err(SyamlError::HashError(format!(
            "unsupported hash algorithm '{algo}'; supported: sha256"
        ))),
    }
}

/// Verifies an Ed25519 detached signature over `content`.
///
/// Reads the public key from `sig.public_key` (resolved relative to `base_dir`),
/// decodes the base64 signature from `sig.value`, and verifies.
pub fn verify_signature(
    content: &[u8],
    sig: &SignatureBinding,
    base_dir: &Path,
) -> Result<(), SyamlError> {
    use base64::Engine;
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};

    let key_path = if Path::new(&sig.public_key).is_absolute() {
        Path::new(&sig.public_key).to_path_buf()
    } else {
        base_dir.join(&sig.public_key)
    };

    let key_bytes = fs::read(&key_path).map_err(|e| {
        SyamlError::SignatureError(format!(
            "failed to read public key '{}': {e}",
            key_path.display()
        ))
    })?;

    let key_32: [u8; 32] = parse_ed25519_public_key(&key_bytes).map_err(|e| {
        SyamlError::SignatureError(format!(
            "invalid Ed25519 public key '{}': {e}",
            key_path.display()
        ))
    })?;

    let verifying_key = VerifyingKey::from_bytes(&key_32).map_err(|e| {
        SyamlError::SignatureError(format!("invalid Ed25519 public key bytes: {e}"))
    })?;

    let sig_bytes = base64::engine::general_purpose::STANDARD
        .decode(&sig.value)
        .map_err(|e| {
            SyamlError::SignatureError(format!("invalid base64 signature value: {e}"))
        })?;

    let signature = Signature::from_slice(&sig_bytes).map_err(|e| {
        SyamlError::SignatureError(format!("invalid Ed25519 signature ({} bytes): {e}", sig_bytes.len()))
    })?;

    verifying_key.verify(content, &signature).map_err(|_| {
        SyamlError::SignatureError("Ed25519 signature verification failed".to_string())
    })
}

/// Parses a raw Ed25519 public key from either raw 32-byte format or PEM-encoded PKCS#8/RFC 8032.
fn parse_ed25519_public_key(bytes: &[u8]) -> Result<[u8; 32], String> {
    if bytes.len() == 32 {
        let mut key = [0u8; 32];
        key.copy_from_slice(bytes);
        return Ok(key);
    }

    if let Ok(pem_str) = std::str::from_utf8(bytes) {
        if let Some(b64) = extract_pem_base64(pem_str) {
            use base64::Engine;
            if let Ok(der) = base64::engine::general_purpose::STANDARD.decode(b64) {
                return extract_ed25519_from_der(&der);
            }
        }
    }

    // Try as raw DER
    extract_ed25519_from_der(bytes)
}

fn extract_pem_base64(pem: &str) -> Option<String> {
    let mut in_block = false;
    let mut b64 = String::new();
    for line in pem.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("-----BEGIN") {
            in_block = true;
            continue;
        }
        if trimmed.starts_with("-----END") {
            break;
        }
        if in_block {
            b64.push_str(trimmed);
        }
    }
    if b64.is_empty() { None } else { Some(b64) }
}

/// Extracts the 32-byte Ed25519 public key from a DER-encoded SubjectPublicKeyInfo.
///
/// The standard ASN.1 prefix for Ed25519 SPKI is 12 bytes, followed by the 32-byte key.
fn extract_ed25519_from_der(der: &[u8]) -> Result<[u8; 32], String> {
    // Ed25519 SubjectPublicKeyInfo is exactly 44 bytes: 12-byte header + 32-byte key
    if der.len() == 44 && der.ends_with(&[0x00; 0]) {
        let mut key = [0u8; 32];
        key.copy_from_slice(&der[12..44]);
        return Ok(key);
    }
    // Fallback: look for 32-byte suffix after known OID prefix
    // OID 1.3.101.112 (Ed25519) = 06 03 2b 65 70
    let ed25519_oid: &[u8] = &[0x06, 0x03, 0x2b, 0x65, 0x70];
    if let Some(pos) = der
        .windows(ed25519_oid.len())
        .position(|w| w == ed25519_oid)
    {
        let after_oid = pos + ed25519_oid.len();
        if der.len() >= after_oid + 2 + 32 {
            // Skip BIT STRING tag (03) + length + unused-bits byte
            let key_start = der.len() - 32;
            let mut key = [0u8; 32];
            key.copy_from_slice(&der[key_start..]);
            return Ok(key);
        }
    }
    Err(format!(
        "could not extract 32-byte Ed25519 key from {} bytes of DER/PEM data",
        der.len()
    ))
}

/// Verifies that the imported document's `meta.file.version` satisfies `requirement`.
pub fn verify_version(meta: Option<&Meta>, requirement: &str) -> Result<(), SyamlError> {
    let req = semver::VersionReq::parse(requirement).map_err(|e| {
        SyamlError::VersionError(format!("invalid version requirement '{requirement}': {e}"))
    })?;

    let version_str = meta
        .and_then(|m| m.file.get("version"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            SyamlError::VersionError(format!(
                "imported file does not declare meta.file.version but requirement '{requirement}' was specified"
            ))
        })?;

    let version = semver::Version::parse(version_str).map_err(|e| {
        SyamlError::VersionError(format!(
            "imported file version '{version_str}' is not valid semver: {e}"
        ))
    })?;

    if !req.matches(&version) {
        return Err(SyamlError::VersionError(format!(
            "imported file version '{version}' does not satisfy requirement '{requirement}'"
        )));
    }

    Ok(())
}

/// Computes the SHA-256 hash of `content` and returns it in `sha256:hex` format.
pub fn compute_sha256(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn verify_hash_sha256_match() {
        let content = b"hello world";
        let hash = compute_sha256(content);
        assert!(verify_hash(content, &hash).is_ok());
    }

    #[test]
    fn verify_hash_sha256_mismatch() {
        let content = b"hello world";
        let err = verify_hash(content, "sha256:0000000000000000000000000000000000000000000000000000000000000000")
            .unwrap_err();
        assert!(err.to_string().contains("sha256 mismatch"));
    }

    #[test]
    fn verify_hash_unsupported_algorithm() {
        let err = verify_hash(b"x", "md5:abc").unwrap_err();
        assert!(err.to_string().contains("unsupported hash algorithm"));
    }

    #[test]
    fn verify_hash_invalid_format() {
        let err = verify_hash(b"x", "nocolon").unwrap_err();
        assert!(err.to_string().contains("invalid hash format"));
    }

    #[test]
    fn verify_version_satisfied() {
        let meta = Meta {
            file: BTreeMap::from([("version".into(), serde_json::json!("1.2.3"))]),
            env: BTreeMap::new(),
            imports: BTreeMap::new(),
        };
        assert!(verify_version(Some(&meta), "^1.0.0").is_ok());
    }

    #[test]
    fn verify_version_not_satisfied() {
        let meta = Meta {
            file: BTreeMap::from([("version".into(), serde_json::json!("1.2.3"))]),
            env: BTreeMap::new(),
            imports: BTreeMap::new(),
        };
        let err = verify_version(Some(&meta), ">=2.0.0").unwrap_err();
        assert!(err.to_string().contains("does not satisfy"));
    }

    #[test]
    fn verify_version_missing_version() {
        let meta = Meta {
            file: BTreeMap::new(),
            env: BTreeMap::new(),
            imports: BTreeMap::new(),
        };
        let err = verify_version(Some(&meta), "^1.0.0").unwrap_err();
        assert!(err.to_string().contains("does not declare"));
    }

    #[test]
    fn verify_version_invalid_requirement() {
        let err = verify_version(None, "not-a-version").unwrap_err();
        assert!(err.to_string().contains("invalid version requirement"));
    }

    #[test]
    fn verify_version_invalid_semver_in_file() {
        let meta = Meta {
            file: BTreeMap::from([("version".into(), serde_json::json!("banana"))]),
            env: BTreeMap::new(),
            imports: BTreeMap::new(),
        };
        let err = verify_version(Some(&meta), "^1.0.0").unwrap_err();
        assert!(err.to_string().contains("not valid semver"));
    }

    #[test]
    fn compute_sha256_deterministic() {
        let a = compute_sha256(b"test data");
        let b = compute_sha256(b"test data");
        assert_eq!(a, b);
        assert!(a.starts_with("sha256:"));
    }
}
