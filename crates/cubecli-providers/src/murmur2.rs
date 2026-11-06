//! The CurseForge file fingerprint: 32-bit murmur2, seed 1, little-endian
//! u32 chunks — computed after removing whitespace bytes (`\t`, `\n`, `\r`,
//! space), matching the reference implementation the official launcher uses
//! for `POST /fingerprints` (whole-file in memory, like packwiz; the length
//! mixes into the seed, so this cannot be hashed incrementally).

use std::path::Path;

use crate::error::{ProviderError, Result};

const SEED: u32 = 1;
const M: u32 = 0x5bd1_e995;
const R: u32 = 24;

/// CurseForge murmur2 fingerprint of raw file bytes (seed 1, the CF constant).
pub fn digest(bytes: &[u8]) -> u32 {
    digest_with_seed(bytes, SEED)
}

/// murmur2 with an explicit seed (the CF seed is 1); exposed for tests so
/// published vectors from the reference implementation can pin every path.
fn digest_with_seed(bytes: &[u8], seed: u32) -> u32 {
    // Whitespace bytes are dropped before hashing — even for binary files;
    // that is what the reference implementations do.
    let data: Vec<u8> = bytes
        .iter()
        .copied()
        .filter(|b| !matches!(b, b'\t' | b'\n' | b'\r' | b' '))
        .collect();
    let len = data.len() as u32;

    if len <= 3 {
        return avalanche(tail(&data, seed ^ len));
    }

    let mut h = seed ^ len;
    let mut offset = 0usize;
    for _ in 0..len / 4 {
        let mut k = u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]);
        k = k.wrapping_mul(M);
        k ^= k >> R;
        k = k.wrapping_mul(M);
        h = h.wrapping_mul(M) ^ k;
        offset += 4;
    }
    avalanche(tail(&data[offset..], h))
}

/// Murmur2 tail: 0–3 leftover bytes mixed in, then the final avalanche.
fn tail(rest: &[u8], mut h: u32) -> u32 {
    if rest.len() >= 3 {
        h ^= u32::from(rest[2]) << 16;
    }
    if rest.len() >= 2 {
        h ^= u32::from(rest[1]) << 8;
    }
    if !rest.is_empty() {
        h ^= u32::from(rest[0]);
        h = h.wrapping_mul(M);
    }
    h
}

fn avalanche(mut h: u32) -> u32 {
    h ^= h >> 13;
    h = h.wrapping_mul(M);
    h ^= h >> 15;
    h
}

/// CurseForge fingerprint of a file (mod jars are read whole — the reference
/// implementations do the same).
pub fn digest_file(path: &Path) -> Result<u32> {
    let bytes = std::fs::read(path).map_err(|e| ProviderError::io(path, e))?;
    Ok(digest(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_bytes_are_skipped() {
        assert_eq!(digest(b"hello world"), digest(b"helloworld"));
        assert_eq!(digest(b"a\tb\nc\r d"), digest(b"abcd"));
        // whitespace-only content hashes like empty content (the reference
        // implementation has no empty special case: avalanche(seed))
        assert_eq!(digest(b" \t\n\r"), digest(b""));
    }

    #[test]
    fn deterministic_and_length_sensitive() {
        let a = digest(b"curseforge fingerprint input");
        assert_eq!(a, digest(b"curseforge fingerprint input"));
        assert_ne!(a, digest(b"curseforge fingerprint inpu"));
        // a trailing space is filtered, an extra letter is not
        assert_ne!(a, digest(b"curseforge fingerprint inputX"));
    }

    /// Published vectors from the reference implementation packwiz uses
    /// (aviddiviner/go-murmur `MurmurHash2`, murmur2_test.go) — they pin the
    /// tail-only (`foo`), single-block (`blam`), block+tail (`zztop`), and
    /// two-block (`foobarbaz`) paths.
    #[test]
    fn reference_vectors_from_go_murmur() {
        assert_eq!(digest_with_seed(b"foo", 123), 1_412_061_192);
        assert_eq!(digest_with_seed(b"zztop", 123), 1_878_194_508);
        assert_eq!(digest_with_seed(b"foobarbaz", 234), 1_777_016_281);
        assert_eq!(digest_with_seed(b"blam", 777), 1_668_928_339);
    }

    #[test]
    fn digest_file_matches_digest() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("mod.jar");
        std::fs::write(&path, b"hello world").unwrap();
        assert_eq!(digest_file(&path).unwrap(), digest(b"hello world"));
    }
}
