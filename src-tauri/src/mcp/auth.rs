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
//!    matching either the master token (desktop / local clients) or one of the
//!    per-host tokens minted at provisioning (migration 018). The token that
//!    matched becomes the request's [`Caller`]: a per-host token identifies —
//!    and scopes the caller to — that host, so a token lifted from one
//!    machine cannot impersonate another, and a `readonly` host token is
//!    refused every mutating tool.

use crate::store::HostTokenRow;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};

/// What a token is allowed to do. Unknown mode strings in the DB fall back
/// to `Readonly` — fail closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenMode {
    /// Every tool.
    Full,
    /// Only tools that observe the fleet; mutating tools get `E_FORBIDDEN`.
    Readonly,
}

impl TokenMode {
    pub fn parse(s: &str) -> TokenMode {
        match s {
            "full" => TokenMode::Full,
            _ => TokenMode::Readonly,
        }
    }
}

/// The authenticated identity behind a request, derived from the bearer
/// token that matched. Inserted into the request extensions by the auth
/// middleware so tools and the `/hook` handler can read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caller {
    /// `None` for the master token (desktop / local agent use — unrestricted);
    /// `Some(alias)` for a per-host token, which scopes identity-bearing
    /// tools (`register_self`, `send_message`, `inbox`) to that host.
    pub host_alias: Option<String>,
    pub mode: TokenMode,
}

impl Caller {
    /// The master-token caller: no host binding, full mode.
    pub fn master() -> Self {
        Caller {
            host_alias: None,
            mode: TokenMode::Full,
        }
    }

    pub fn is_master(&self) -> bool {
        self.host_alias.is_none()
    }

    /// Short identity label for audit rows and rate-limit buckets.
    pub fn label(&self) -> String {
        match &self.host_alias {
            Some(h) => format!("host:{h}"),
            None => "master".to_string(),
        }
    }
}

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

/// Extract the token from an `Authorization` header value. Accepts only the
/// exact form `Bearer <token>`.
pub fn bearer_token(header: Option<&HeaderValue>) -> Option<&str> {
    let text = header?.to_str().ok()?;
    let token = text.strip_prefix("Bearer ")?.trim();
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

/// Map a presented token to its [`Caller`]: the master token → unrestricted;
/// a per-host token → that host with its stored mode; anything else → `None`.
/// Every candidate is compared in constant time and the scan never
/// short-circuits, so timing does not reveal which (if any) token matched.
pub fn resolve_token(
    presented: &str,
    master: &str,
    host_tokens: &[HostTokenRow],
) -> Option<Caller> {
    let mut found: Option<Caller> = None;
    if !master.is_empty() && constant_time_eq(presented.as_bytes(), master.as_bytes()) {
        found = Some(Caller::master());
    }
    for row in host_tokens {
        if !row.token.is_empty() && constant_time_eq(presented.as_bytes(), row.token.as_bytes()) {
            found = Some(Caller {
                host_alias: Some(row.host_alias.clone()),
                mode: TokenMode::parse(&row.mode),
            });
        }
    }
    found
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

/// Layer 1 — DNS-rebinding defense. An `Origin`/`Host` is validated only when
/// present; a non-browser MCP client legitimately omits `Origin`. `Err(403)`
/// on a cross-origin / rebound request.
pub fn check_origin(headers: &HeaderMap) -> Result<(), StatusCode> {
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
    Ok(())
}

/// Authorize an incoming request and identify its caller. `Err` carries the
/// status to return: `403` for a cross-origin / DNS-rebinding attempt, `401`
/// for a missing or unknown bearer token.
pub fn check_request(
    headers: &HeaderMap,
    master_token: &str,
    host_tokens: &[HostTokenRow],
) -> Result<Caller, StatusCode> {
    check_origin(headers)?;
    let presented =
        bearer_token(headers.get(header::AUTHORIZATION)).ok_or(StatusCode::UNAUTHORIZED)?;
    resolve_token(presented, master_token, host_tokens).ok_or(StatusCode::UNAUTHORIZED)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host_row(alias: &str, token: &str, mode: &str) -> HostTokenRow {
        HostTokenRow {
            host_alias: alias.into(),
            token: token.into(),
            created_at: 0,
            mode: mode.into(),
        }
    }

    #[test]
    fn constant_time_eq_matches_identical_and_rejects_others() {
        assert!(constant_time_eq(b"abc123", b"abc123"));
        assert!(!constant_time_eq(b"abc123", b"abc124"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn bearer_token_extracts_exact_bearer_form_only() {
        let h = HeaderValue::from_static("Bearer s3cret");
        assert_eq!(bearer_token(Some(&h)), Some("s3cret"));
        let padded = HeaderValue::from_static("Bearer  s3cret ");
        assert_eq!(bearer_token(Some(&padded)), Some("s3cret"));
        assert!(bearer_token(None).is_none());
        let no_scheme = HeaderValue::from_static("s3cret");
        assert!(bearer_token(Some(&no_scheme)).is_none());
        let basic = HeaderValue::from_static("Basic s3cret");
        assert!(bearer_token(Some(&basic)).is_none());
        let empty = HeaderValue::from_static("Bearer ");
        assert!(bearer_token(Some(&empty)).is_none());
    }

    #[test]
    fn resolve_token_maps_master_and_host_tokens_to_callers() {
        let hosts = [
            host_row("mefistos", "tok-mef", "full"),
            host_row("turanga", "tok-tur", "readonly"),
            host_row("weird", "tok-weird", "not-a-mode"),
        ];
        assert_eq!(
            resolve_token("master-tok", "master-tok", &hosts),
            Some(Caller::master())
        );
        assert_eq!(
            resolve_token("tok-mef", "master-tok", &hosts),
            Some(Caller {
                host_alias: Some("mefistos".into()),
                mode: TokenMode::Full
            })
        );
        assert_eq!(
            resolve_token("tok-tur", "master-tok", &hosts),
            Some(Caller {
                host_alias: Some("turanga".into()),
                mode: TokenMode::Readonly
            })
        );
        // Unknown mode strings fail closed to readonly.
        assert_eq!(
            resolve_token("tok-weird", "master-tok", &hosts)
                .unwrap()
                .mode,
            TokenMode::Readonly
        );
        assert_eq!(resolve_token("nope", "master-tok", &hosts), None);
        // An empty configured token never matches an empty presented one.
        assert_eq!(resolve_token("", "", &[host_row("h", "", "full")]), None);
    }

    #[test]
    fn caller_labels_and_master_flag() {
        assert!(Caller::master().is_master());
        assert_eq!(Caller::master().label(), "master");
        let c = Caller {
            host_alias: Some("mefistos".into()),
            mode: TokenMode::Full,
        };
        assert!(!c.is_master());
        assert_eq!(c.label(), "host:mefistos");
        assert_eq!(TokenMode::parse("full"), TokenMode::Full);
        assert_eq!(TokenMode::parse("readonly"), TokenMode::Readonly);
        assert_eq!(TokenMode::parse("anything-else"), TokenMode::Readonly);
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
        assert_eq!(check_request(&h, "s3cret", &[]), Ok(Caller::master()));
    }

    #[test]
    fn check_request_identifies_host_token_callers() {
        let h = headers(&[("authorization", "Bearer tok-mef")]);
        let caller =
            check_request(&h, "s3cret", &[host_row("mefistos", "tok-mef", "readonly")]).unwrap();
        assert_eq!(caller.host_alias.as_deref(), Some("mefistos"));
        assert_eq!(caller.mode, TokenMode::Readonly);
    }

    #[test]
    fn check_request_allows_non_browser_client_without_origin() {
        // A CLI MCP client sends no Origin — only the token gates it.
        let h = headers(&[("authorization", "Bearer s3cret")]);
        assert!(check_request(&h, "s3cret", &[]).is_ok());
    }

    #[test]
    fn check_request_rejects_wrong_token_with_401() {
        let h = headers(&[("host", "127.0.0.1:4180"), ("authorization", "Bearer nope")]);
        assert_eq!(
            check_request(&h, "s3cret", &[]),
            Err(StatusCode::UNAUTHORIZED)
        );
        let none = headers(&[("host", "127.0.0.1:4180")]);
        assert_eq!(
            check_request(&none, "s3cret", &[]),
            Err(StatusCode::UNAUTHORIZED)
        );
    }

    #[test]
    fn check_request_rejects_remote_origin_with_403() {
        // DNS-rebinding attempt: a remote page's Origin, even with a token.
        let h = headers(&[
            ("host", "127.0.0.1:4180"),
            ("origin", "http://evil.com"),
            ("authorization", "Bearer s3cret"),
        ]);
        assert_eq!(check_request(&h, "s3cret", &[]), Err(StatusCode::FORBIDDEN));
        assert_eq!(check_origin(&h), Err(StatusCode::FORBIDDEN));
    }

    #[test]
    fn check_request_rejects_rebound_host_with_403() {
        // Host header carrying the attacker's domain (rebound to 127.0.0.1).
        let h = headers(&[("host", "evil.com"), ("authorization", "Bearer s3cret")]);
        assert_eq!(check_request(&h, "s3cret", &[]), Err(StatusCode::FORBIDDEN));
    }
}
