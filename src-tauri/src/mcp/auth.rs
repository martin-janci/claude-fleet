//! Bearer-token authentication for the embedded MCP server.
//!
//! The server binds localhost only, but a token still guards against other
//! local processes and against a malicious web page issuing `fetch` calls
//! (which cannot read the token).

use axum::http::HeaderValue;

/// Constant-time byte comparison. Returns `false` immediately on a length
/// mismatch — the token length is fixed and not itself a secret — and runs
/// in time independent of *where* two equal-length inputs first differ.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Check an `Authorization` header value against the expected bearer token.
/// Accepts only the exact form `Bearer <token>`.
pub fn bearer_matches(header: Option<&HeaderValue>, expected: &str) -> bool {
    let Some(value) = header else {
        return false;
    };
    let Ok(text) = value.to_str() else {
        return false;
    };
    let Some(token) = text.strip_prefix("Bearer ") else {
        return false;
    };
    constant_time_eq(token.trim().as_bytes(), expected.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_matches_identical_and_rejects_others() {
        assert!(constant_time_eq(b"abc123", b"abc123"));
        assert!(!constant_time_eq(b"abc123", b"abc124"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn bearer_matches_accepts_correct_token() {
        let h = HeaderValue::from_static("Bearer s3cret");
        assert!(bearer_matches(Some(&h), "s3cret"));
    }

    #[test]
    fn bearer_matches_rejects_wrong_token() {
        let h = HeaderValue::from_static("Bearer wrong");
        assert!(!bearer_matches(Some(&h), "s3cret"));
    }

    #[test]
    fn bearer_matches_rejects_missing_or_malformed_header() {
        assert!(!bearer_matches(None, "s3cret"));
        let no_scheme = HeaderValue::from_static("s3cret");
        assert!(!bearer_matches(Some(&no_scheme), "s3cret"));
        let basic = HeaderValue::from_static("Basic s3cret");
        assert!(!bearer_matches(Some(&basic), "s3cret"));
    }
}
