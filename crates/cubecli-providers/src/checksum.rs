//! Hash computation and verification (sha1 / sha256 / md5).

use std::io::Read;
use std::path::Path;

use sha1::Digest as _;

use cubecli_core::model::Checksum;

use crate::error::{ProviderError, Result};

/// Stream a file through the matching hasher, returning lowercase hex.
pub fn digest_file(algorithm: &str, path: &Path) -> Result<String> {
    let file = std::fs::File::open(path).map_err(|e| ProviderError::io(path, e))?;
    let mut reader = std::io::BufReader::new(file);
    match algorithm {
        "sha1" => hex_digest(path, sha1::Sha1::new(), &mut reader),
        "sha256" => hex_digest(path, sha2::Sha256::new(), &mut reader),
        "md5" => hex_digest(path, md5::Md5::new(), &mut reader),
        other => Err(ProviderError::Other(format!(
            "unknown hash algorithm `{other}`"
        ))),
    }
}

fn hex_digest<D: sha2::Digest>(
    path: &Path,
    mut hasher: D,
    reader: &mut impl Read,
) -> Result<String> {
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| ProviderError::io(path, e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Verify a downloaded file against an expected checksum.
pub fn verify(path: &Path, expected: &Checksum) -> Result<()> {
    let actual = digest_file(expected.algorithm(), path)?;
    if expected.matches(&actual) {
        Ok(())
    } else {
        Err(ProviderError::ChecksumMismatch {
            file: path.display().to_string(),
            algorithm: expected.algorithm(),
            expected: expected.expected_hex().to_string(),
            actual,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_digests() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f");
        std::fs::write(&path, b"hello").unwrap();
        assert_eq!(
            digest_file("sha1", &path).unwrap(),
            "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d"
        );
        assert_eq!(
            digest_file("sha256", &path).unwrap(),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert_eq!(
            digest_file("md5", &path).unwrap(),
            "5d41402abc4b2a76b9719d911017c592"
        );
    }

    #[test]
    fn verify_detects_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f");
        std::fs::write(&path, b"hello").unwrap();
        let good = Checksum::Sha1("aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d".into());
        assert!(verify(&path, &good).is_ok());
        let bad = Checksum::Sha1("0000000000000000000000000000000000000000".into());
        let err = verify(&path, &bad).unwrap_err();
        assert!(matches!(err, ProviderError::ChecksumMismatch { .. }));
    }
}
