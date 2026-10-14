//! Session access token: 128 random bits, hex encoded.

use anyhow::Result;

pub fn generate() -> Result<String> {
    let mut buf = [0u8; 16];
    getrandom::fill(&mut buf).map_err(|e| anyhow::anyhow!("reading OS randomness: {e}"))?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

/// Constant-time equality (length leaks, content does not).
pub fn verify(expected: &str, given: &str) -> bool {
    let (a, b) = (expected.as_bytes(), given.as_bytes());
    let mut diff = a.len() ^ b.len();
    for (i, x) in a.iter().enumerate() {
        diff |= (x ^ b.get(i).copied().unwrap_or(0)) as usize;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_are_32_hex_and_unique() {
        let (a, b) = (generate().expect("a"), generate().expect("b"));
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn verify_matches_only_equal() {
        assert!(verify("abc", "abc"));
        assert!(!verify("abc", "abd"));
        assert!(!verify("abc", "ab"));
        assert!(!verify("abc", "abcd"));
        assert!(!verify("abc", ""));
        assert!(!verify("", "x"));
        assert!(verify("", ""));
    }
}
