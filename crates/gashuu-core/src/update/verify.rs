//! SHA-256 verification of a downloaded asset against the release `SHA256SUMS`.

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt::Write;

/// Parse a `SHA256SUMS` file into a `filename -> lowercase-hex` map. Accepts the
/// coreutils format `"<hex>  <name>"` and the binary-marker `"<hex> *<name>"`.
pub fn parse_sha256sums(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(2, char::is_whitespace);
        if let (Some(hash), Some(rest)) = (parts.next(), parts.next()) {
            let name = rest.trim_start_matches([' ', '*']).trim();
            map.insert(name.to_string(), hash.to_ascii_lowercase());
        }
    }
    map
}

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut s = String::with_capacity(digest.len() * 2);
    for b in digest {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// True iff `bytes` hashes to `expected_hex` (case-insensitive).
pub fn is_verified(bytes: &[u8], expected_hex: &str) -> bool {
    sha256_hex(bytes).eq_ignore_ascii_case(expected_hex.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Known vector: SHA-256("abc").
    const ABC_HASH: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn sha256_of_abc_matches_known_vector() {
        assert_eq!(sha256_hex(b"abc"), ABC_HASH);
    }

    #[test]
    fn is_verified_matches_and_rejects() {
        assert!(is_verified(b"abc", ABC_HASH));
        assert!(is_verified(b"abc", &ABC_HASH.to_uppercase()));
        assert!(!is_verified(b"abcd", ABC_HASH));
    }

    #[test]
    fn parse_sums_both_formats() {
        let text = format!(
            "{ABC_HASH}  gashuu-v1.0.0-x86_64.AppImage\n{ABC_HASH} *gashuu-v1.0.0-windows-x64.zip\n"
        );
        let map = parse_sha256sums(&text);
        assert_eq!(map.get("gashuu-v1.0.0-x86_64.AppImage").unwrap(), ABC_HASH);
        assert_eq!(map.get("gashuu-v1.0.0-windows-x64.zip").unwrap(), ABC_HASH);
    }

    #[test]
    fn parse_sums_skips_blank_lines() {
        assert!(parse_sha256sums("\n\n   \n").is_empty());
    }
}
