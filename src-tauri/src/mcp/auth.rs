//! Request authorization for the embedded MCP server.
//!
//! Two layers, checked in order:
//!
//! 1. **DNS-rebinding defense** — the server binds localhost, but a remote web
//!    page can still point its own domain at `127.0.0.1` and have the victim's
//!    browser issue requests. We reject any request whose `Origin` or `Host`
//!    header names a non-loopback address. The MCP HTTP-transport spec requires
//!    `Origin` validation for exactly this reason.
//! 2. **Bearer token** — the request must carry `Authorization: Bearer <token>`
//!    matching the configured secret. This guards against other local
//!    processes (and any browser request that *did* clear layer 1).

use axum::http::{header, HeaderMap, HeaderValue, StatusCode};

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

/// True if `value` (a `Host`-header authority — `host` or `host:port`, IPv6
/// in brackets) names the local machine.
pub fn is_loopback_host(value: &str) -> bool {
    let host = value.trim();
    // Bracketed IPv6: `[::1]` or `[::1]:port`.
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next() == Some("::1");
    }
    // `hostname[:port]` or `ipv4[:port]`.
    let name = host.split(':').next().unwrap_or(host);
    name.eq_ignore_ascii_case("localhost") || name == "127.0.0.1"
}

/// True if an `Origin` header value is a loopback `http(s)` origin. Anything
/// else — a remote origin, the opaque `null` origin, a non-http scheme — is
/// treated as cross-origin and rejected.
pub fn origin_is_loopback(origin: &str) -> bool {
    let after_scheme = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"));
    match after_scheme {
        Some(rest) => is_loopback_host(rest.split('/').next().unwrap_or(rest)),
        None => false,
    }
}

/// Authorize an incoming request. `Err` carries the status to return:
/// `403` for a cross-origin / DNS-rebinding attempt, `401` for a missing or
/// wrong bearer token.
pub fn check_request(headers: &HeaderMap, expected_token: &str) -> Result<(), StatusCode> {
    // Layer 1 — DNS-rebinding defense. An `Origin`/`Host` is validated only
    // when present; a non-browser MCP client legitimately omits `Origin`.
    if let Some(origin) = headers.get(header::ORIGIN) {
        if !origin.to_str().map(origin_is_loopback).unwrap_or(false) {
            return Err(StatusCode::FORBIDDEN);
        }
    }
    if let Some(host) = headers.get(header::HOST) {
        if !host.to_str().map(is_loopback_host).unwrap_or(false) {
            return Err(StatusCode::FORBIDDEN);
        }
    }
    // Layer 2 — bearer token.
    if bearer_matches(headers.get(header::AUTHORIZATION), expected_token) {
        Ok(())
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
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

    #[test]
    fn is_loopback_host_accepts_local_forms() {
        for h in [
            "127.0.0.1",
            "127.0.0.1:4180",
            "localhost",
            "localhost:4180",
            "LocalHost:4180",
            "[::1]",
            "[::1]:4180",
        ] {
            assert!(is_loopback_host(h), "should accept {h}");
        }
    }

    #[test]
    fn is_loopback_host_rejects_remote_forms() {
        for h in [
            "evil.com",
            "evil.com:4180",
            "127.0.0.1.evil.com",
            "10.0.0.5",
            "0.0.0.0",
        ] {
            assert!(!is_loopback_host(h), "should reject {h}");
        }
    }

    #[test]
    fn origin_is_loopback_accepts_local_and_rejects_remote() {
        assert!(origin_is_loopback("http://127.0.0.1:4180"));
        assert!(origin_is_loopback("http://localhost:4180"));
        assert!(origin_is_loopback("https://[::1]"));
        assert!(!origin_is_loopback("http://evil.com"));
        assert!(!origin_is_loopback("https://evil.com:4180"));
        assert!(!origin_is_loopback("null"));
        assert!(!origin_is_loopback("file://"));
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    #[test]
    fn check_request_allows_local_request_with_token() {
        let h = headers(&[
            ("host", "127.0.0.1:4180"),
            ("authorization", "Bearer s3cret"),
        ]);
        assert!(check_request(&h, "s3cret").is_ok());
    }

    #[test]
    fn check_request_allows_non_browser_client_without_origin() {
        // A CLI MCP client sends no Origin — only the token gates it.
        let h = headers(&[("authorization", "Bearer s3cret")]);
        assert!(check_request(&h, "s3cret").is_ok());
    }

    #[test]
    fn check_request_rejects_wrong_token_with_401() {
        let h = headers(&[("host", "127.0.0.1:4180"), ("authorization", "Bearer nope")]);
        assert_eq!(check_request(&h, "s3cret"), Err(StatusCode::UNAUTHORIZED));
    }

    #[test]
    fn check_request_rejects_remote_origin_with_403() {
        // DNS-rebinding attempt: a remote page's Origin, even with a token.
        let h = headers(&[
            ("host", "127.0.0.1:4180"),
            ("origin", "http://evil.com"),
            ("authorization", "Bearer s3cret"),
        ]);
        assert_eq!(check_request(&h, "s3cret"), Err(StatusCode::FORBIDDEN));
    }

    #[test]
    fn check_request_rejects_rebound_host_with_403() {
        // Host header carrying the attacker's domain (rebound to 127.0.0.1).
        let h = headers(&[("host", "evil.com"), ("authorization", "Bearer s3cret")]);
        assert_eq!(check_request(&h, "s3cret"), Err(StatusCode::FORBIDDEN));
    }
}
