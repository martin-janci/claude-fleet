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
//!    refused every mutating tool. A third kind of token identifies a paired
//!    *client* (a phone — migration `032_client_tokens.sql`): the DB keeps
//!    only its SHA-256, and it resolves to a caller that is deliberately
//!    NEITHER the master NOR a host, so the fleet-admin tools stay out of
//!    its reach.

use crate::store::{ClientTokenRow, HostTokenRow};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};

/// What a token is allowed to do. Unknown mode strings in the DB fall back
/// to `Readonly` — fail closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenMode {
    /// Every tool.
    Full,
    /// Only tools that observe the fleet; mutating tools get `E_FORBIDDEN`.
    Readonly,
    /// Another fleet's hub (federation): `peer_exchange` and nothing else.
    /// Only a paired client row can hold it (see `parse_client`).
    Peer,
}

/// The one tool a `Peer` token may call, and that only a `Peer` token may
/// call.
pub(crate) const PEER_TOOL: &str = "peer_exchange";

impl TokenMode {
    /// A host token row's mode. `peer` is NOT recognised here: a host's
    /// token can never become a hub link, whatever string its row holds.
    pub fn parse(s: &str) -> TokenMode {
        match s {
            "full" => TokenMode::Full,
            _ => TokenMode::Readonly,
        }
    }

    /// A paired client row's mode: `full`, `peer`, else `readonly`.
    fn parse_client(s: &str) -> TokenMode {
        match s {
            "peer" => TokenMode::Peer,
            other => TokenMode::parse(other),
        }
    }
}

/// The paired client behind a request: the `client_tokens` row that matched.
/// Only the id, the name and the trust flag travel — never the token or its
/// hash. `trusted` is `trusted_at IS NOT NULL` on the row: the operator has
/// vouched for this device, so what it sends is delivered without the
/// untrusted-content marker (`mcp::tools::apply_marker`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientRef {
    pub id: i64,
    pub name: String,
    pub trusted: bool,
}

/// The authenticated identity behind a request, derived from the bearer
/// token that matched. Inserted into the request extensions by the auth
/// middleware so tools and the `/hook` handler can read it.
///
/// Exactly one of the three shapes: master (`host_alias` and `client` both
/// `None`), a host (`host_alias` set), a paired client (`client` set).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caller {
    /// `None` for the master token (desktop / local agent use — unrestricted)
    /// AND for a paired client; `Some(alias)` for a per-host token, which
    /// scopes identity-bearing tools (`register_self`, `send_message`,
    /// `inbox`) to that host.
    pub host_alias: Option<String>,
    /// `Some(_)` only for a paired client (a phone). A client is never the
    /// master: [`Caller::is_master`] — the fleet-admin gate — checks this
    /// field too, so provisioning, `add_host`/`remove_host`, `apply_sync` and
    /// `set_secret` stay unreachable from a paired device.
    pub client: Option<ClientRef>,
    pub mode: TokenMode,
}

impl Caller {
    /// The master-token caller: no host binding, no client, full mode.
    pub fn master() -> Self {
        Caller {
            host_alias: None,
            client: None,
            mode: TokenMode::Full,
        }
    }

    /// True only for the master token. A paired client carries no host alias
    /// either, so the client field must be checked as well — this is the one
    /// gate that keeps the fleet-admin tools master-only.
    pub fn is_master(&self) -> bool {
        self.host_alias.is_none() && self.client.is_none()
    }

    /// True for a paired client (a phone), whatever its mode.
    pub fn is_client(&self) -> bool {
        self.client.is_some()
    }

    /// True for the UX agent's operator session: the paired client token
    /// `ensure_operator` mints under [`OPERATOR_CLIENT_NAME`]. Its session
    /// starts and kills always need a person's approval (work graph M9.7,
    /// decision D12).
    ///
    /// [`OPERATOR_CLIENT_NAME`]: crate::service::operator::OPERATOR_CLIENT_NAME
    pub fn is_operator(&self) -> bool {
        self.host_alias.is_none()
            && self
                .client
                .as_ref()
                .is_some_and(|c| c.name == crate::service::operator::OPERATOR_CLIENT_NAME)
    }

    /// True for a paired client the operator has vouched for
    /// (`client_tokens.trusted_at` set): its text is the operator's own, so
    /// the untrusted-content marker is left off. Never true for the master
    /// (which has `raw` for that) or a per-host token.
    pub fn is_trusted_client(&self) -> bool {
        self.client.as_ref().is_some_and(|c| c.trusted)
    }

    /// The work graph's org scope for this caller (M5) — the ONE place a
    /// caller becomes a scope. The master and a paired client read every
    /// org (the org is a view there); a per-host token is bounded by its
    /// host's org, read from the store now, so a host moved by the master
    /// is fenced from its next call on.
    pub fn org_scope(
        &self,
        store: &crate::store::Store,
    ) -> Result<crate::service::orgs::OrgScope, crate::ipc_error::IpcError> {
        match &self.host_alias {
            None => Ok(crate::service::orgs::OrgScope::All),
            Some(h) => crate::service::orgs::OrgScope::for_host(store, h),
        }
    }

    /// Short identity label for audit rows and rate-limit buckets.
    pub fn label(&self) -> String {
        match (&self.host_alias, &self.client) {
            (Some(h), _) => format!("host:{h}"),
            (None, Some(c)) => format!("client:{}", c.name),
            (None, None) => "master".to_string(),
        }
    }
}

/// Lowercase-hex SHA-256 of a token. `client_tokens` stores only this, so a
/// stolen database hands out no usable bearer token.
pub fn sha256_hex(s: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(s.as_bytes()))
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
/// a per-host token → that host with its stored mode; a paired client's token
/// (matched against the stored SHA-256) → that client; anything else → `None`.
/// Every candidate is compared in constant time and the scan never
/// short-circuits, so timing does not reveal which (if any) token matched.
///
/// `client_tokens` must be the *live* rows — `Store::active_client_tokens`,
/// which drops revoked ones — so a revoked pairing can never resolve.
pub fn resolve_token(
    presented: &str,
    master: &str,
    host_tokens: &[HostTokenRow],
    client_tokens: &[ClientTokenRow],
) -> Option<Caller> {
    let mut found: Option<Caller> = None;
    if !master.is_empty() && constant_time_eq(presented.as_bytes(), master.as_bytes()) {
        found = Some(Caller::master());
    }
    for row in host_tokens {
        if !row.token.is_empty() && constant_time_eq(presented.as_bytes(), row.token.as_bytes()) {
            found = Some(Caller {
                host_alias: Some(row.host_alias.clone()),
                client: None,
                mode: TokenMode::parse(&row.mode),
            });
        }
    }
    // Hash once, then compare every stored digest — same no-short-circuit
    // shape as above.
    let presented_sha = sha256_hex(presented);
    for row in client_tokens {
        if !row.token_sha256.is_empty()
            && constant_time_eq(presented_sha.as_bytes(), row.token_sha256.as_bytes())
        {
            found = Some(Caller {
                host_alias: None,
                client: Some(ClientRef {
                    id: row.id,
                    name: row.name.clone(),
                    trusted: row.trusted_at.is_some(),
                }),
                mode: TokenMode::parse_client(&row.mode),
            });
        }
    }
    found
}

/// True if `value` (a `Host`-header authority — `host` or `host:port`, IPv6
/// in brackets) names the local machine.
///
/// Splitting the authority is this function's job; deciding what counts as
/// the local machine is [`fleet_proto::net::is_loopback`]'s, shared with the
/// agent, the hub's bind check and the desktop.
pub fn is_loopback_host(value: &str) -> bool {
    fleet_proto::net::is_loopback(&authority_host(value))
}

/// True if an `Origin` header value is a loopback `http(s)` origin. Anything
/// else — a remote origin, the opaque `null` origin, a non-http scheme — is
/// treated as cross-origin and rejected.
// Kept as a public, independently-tested special case of `origin_allowed`
// (empty allowlist) even though production code now calls `check_origin`
// directly; not currently called outside its own test.
#[allow(dead_code)]
pub fn origin_is_loopback(origin: &str) -> bool {
    origin_allowed(origin, &[])
}

/// Lower-cased, trimmed allowlist entries (`host` or `host:port`), empties
/// dropped. Built once at server start from the hub's configuration.
pub fn normalize_allowed_hosts(list: &[String]) -> Vec<String> {
    list.iter()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

/// The host part of a `Host`-header authority: brackets and port stripped,
/// lower-cased.
fn authority_host(value: &str) -> String {
    let v = value.trim();
    if let Some(rest) = v.strip_prefix('[') {
        return rest.split(']').next().unwrap_or("").to_ascii_lowercase();
    }
    v.split(':').next().unwrap_or(v).to_ascii_lowercase()
}

/// True when `value` is loopback or names an allowlisted host, matched as
/// the full authority (`host:port`) or as the bare host.
fn host_allowed(value: &str, allowed: &[String]) -> bool {
    if is_loopback_host(value) {
        return true;
    }
    let full = value.trim().to_ascii_lowercase();
    let host = authority_host(value);
    allowed.iter().any(|a| *a == full || *a == host)
}

/// True when an `Origin` is a loopback `http(s)` origin or one whose
/// authority is allowlisted.
fn origin_allowed(origin: &str, allowed: &[String]) -> bool {
    let after_scheme = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"));
    match after_scheme {
        Some(rest) => host_allowed(rest.split('/').next().unwrap_or(rest), allowed),
        None => false,
    }
}

/// Layer 1 — DNS-rebinding defense. An `Origin`/`Host` is validated only when
/// present; a non-browser MCP client legitimately omits `Origin`. Loopback is
/// always accepted; a hub exposed at a public URL adds that URL's host to
/// `allowed`. `Err(403)` on anything else.
///
/// `allowed` must ALREADY be normalized through [`normalize_allowed_hosts`],
/// as `mcp::start` does once before handing it to the `AuthState`: the
/// comparison here is exact, so an un-normalized entry (a scheme, a trailing
/// slash, mixed case) simply never matches.
pub fn check_origin(headers: &HeaderMap, allowed: &[String]) -> Result<(), StatusCode> {
    if let Some(origin) = headers.get(header::ORIGIN) {
        if !origin
            .to_str()
            .map(|o| origin_allowed(o, allowed))
            .unwrap_or(false)
        {
            return Err(StatusCode::FORBIDDEN);
        }
    }
    if let Some(host) = headers.get(header::HOST) {
        if !host
            .to_str()
            .map(|h| host_allowed(h, allowed))
            .unwrap_or(false)
        {
            return Err(StatusCode::FORBIDDEN);
        }
    }
    Ok(())
}

/// Authorize an incoming request and identify its caller. `Err` carries the
/// status to return: `403` for a cross-origin / DNS-rebinding attempt, `401`
/// for a missing or unknown bearer token. `allowed` must already be normalized
/// ([`normalize_allowed_hosts`]) — see [`check_origin`].
pub fn check_request(
    headers: &HeaderMap,
    master_token: &str,
    host_tokens: &[HostTokenRow],
    client_tokens: &[ClientTokenRow],
    allowed: &[String],
) -> Result<Caller, StatusCode> {
    check_origin(headers, allowed)?;
    let presented =
        bearer_token(headers.get(header::AUTHORIZATION)).ok_or(StatusCode::UNAUTHORIZED)?;
    resolve_token(presented, master_token, host_tokens, client_tokens)
        .ok_or(StatusCode::UNAUTHORIZED)
}

/// The `Peer` gate shared by `/events` and `/report`: neither route is
/// `peer_exchange`, so a hub link's token must be refused before either does
/// any work. `/mcp`'s own gate is `enforce_mode` in `tools/support.rs`; this
/// is the same rule for the two routes that sit outside the tool router but
/// still take a `Caller` from the request extensions. A shared fn rather than
/// the check inlined twice, since the two routes disagreeing about who is a
/// peer is exactly the kind of drift this exists to prevent.
///
/// Returns the `403` to send when the caller must be refused, or `None` when
/// it may proceed.
pub(crate) fn refuses_peer(caller: &Caller) -> Option<axum::response::Response> {
    use axum::response::IntoResponse;
    if caller.mode != TokenMode::Peer {
        return None;
    }
    Some(
        (
            StatusCode::FORBIDDEN,
            "a hub link may call peer_exchange only\n",
        )
            .into_response(),
    )
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
    fn a_trusted_row_resolves_to_a_trusted_client_ref() {
        let mut row = client_row(3, "mac-desktop", "tok", "full");
        row.trusted_at = Some(1_700_000_000);
        let c = resolve_token("tok", "master", &[], &[row]).expect("resolves");
        assert!(c.is_client() && !c.is_master());
        assert!(c.is_trusted_client());
        assert!(c.client.as_ref().unwrap().trusted);
        let plain = resolve_token(
            "tok",
            "master",
            &[],
            &[client_row(3, "phone", "tok", "full")],
        )
        .unwrap();
        assert!(!plain.is_trusted_client());
        assert!(!Caller::master().is_trusted_client());
    }

    fn client_row(id: i64, name: &str, token: &str, mode: &str) -> ClientTokenRow {
        ClientTokenRow {
            id,
            name: name.into(),
            token_sha256: sha256_hex(token),
            mode: mode.into(),
            created_at: 0,
            last_seen_at: None,
            revoked_at: None,
            trusted_at: None,
        }
    }

    #[test]
    fn a_client_token_resolves_to_a_client_caller_that_is_not_master() {
        let rows = vec![client_row(7, "phone", "tok-phone", "full")];
        let c = resolve_token("tok-phone", "s3cret", &[], &rows).unwrap();
        assert!(!c.is_master(), "a client must never count as the master");
        assert!(c.is_client());
        assert_eq!(c.label(), "client:phone");
        assert_eq!(c.mode, TokenMode::Full);
        assert_eq!(c.client.as_ref().unwrap().id, 7);
        assert!(c.host_alias.is_none());
    }

    #[test]
    fn a_readonly_client_keeps_its_mode_and_an_unknown_token_resolves_to_nothing() {
        let rows = vec![client_row(1, "tablet", "tok-t", "readonly")];
        assert_eq!(
            resolve_token("tok-t", "s3cret", &[], &rows).unwrap().mode,
            TokenMode::Readonly
        );
        assert!(resolve_token("nope", "s3cret", &[], &rows).is_none());
    }

    #[test]
    fn the_master_and_host_tokens_still_resolve_with_clients_present() {
        let clients = vec![client_row(1, "phone", "tok-phone", "full")];
        let hosts = vec![host_row("mefistos", "tok-mef", "full")];
        assert_eq!(
            resolve_token("s3cret", "s3cret", &hosts, &clients).unwrap(),
            Caller::master()
        );
        let h = resolve_token("tok-mef", "s3cret", &hosts, &clients).unwrap();
        assert_eq!(h.host_alias.as_deref(), Some("mefistos"));
        assert!(!h.is_client());
    }

    #[test]
    fn sha256_hex_is_lowercase_hex_of_the_token() {
        // Known vector: SHA-256 of "abc".
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn a_client_token_never_authorizes_as_the_master_through_check_request() {
        let clients = vec![client_row(3, "phone", "tok-phone", "full")];
        let h = headers(&[
            ("host", "127.0.0.1:4180"),
            ("authorization", "Bearer tok-phone"),
        ]);
        let c = check_request(&h, "s3cret", &[], &clients, &[]).unwrap();
        assert!(!c.is_master());
        assert!(c.is_client());
        assert_eq!(c.label(), "client:phone");
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
            resolve_token("master-tok", "master-tok", &hosts, &[]),
            Some(Caller::master())
        );
        assert_eq!(
            resolve_token("tok-mef", "master-tok", &hosts, &[]),
            Some(Caller {
                host_alias: Some("mefistos".into()),
                client: None,
                mode: TokenMode::Full
            })
        );
        assert_eq!(
            resolve_token("tok-tur", "master-tok", &hosts, &[]),
            Some(Caller {
                host_alias: Some("turanga".into()),
                client: None,
                mode: TokenMode::Readonly
            })
        );
        // Unknown mode strings fail closed to readonly.
        assert_eq!(
            resolve_token("tok-weird", "master-tok", &hosts, &[])
                .unwrap()
                .mode,
            TokenMode::Readonly
        );
        assert_eq!(resolve_token("nope", "master-tok", &hosts, &[]), None);
        // An empty configured token never matches an empty presented one.
        assert_eq!(
            resolve_token("", "", &[host_row("h", "", "full")], &[]),
            None
        );
        // …nor an empty hash on a client row.
        assert_eq!(
            resolve_token(
                "",
                "",
                &[],
                &[ClientTokenRow {
                    id: 1,
                    name: "c".into(),
                    token_sha256: String::new(),
                    mode: "full".into(),
                    created_at: 0,
                    last_seen_at: None,
                    revoked_at: None,
                    trusted_at: None,
                }]
            ),
            None
        );
    }

    #[test]
    fn only_a_client_row_can_be_a_peer() {
        assert_eq!(TokenMode::parse_client("peer"), TokenMode::Peer);
        assert_eq!(TokenMode::parse_client("full"), TokenMode::Full);
        assert_eq!(TokenMode::parse_client("readonly"), TokenMode::Readonly);
        assert_eq!(
            TokenMode::parse_client("anything-else"),
            TokenMode::Readonly
        );
        // A host token row is parsed with `parse`: `peer` there is unknown and
        // fails closed, so an agent's token can never reach `peer_exchange`.
        assert_eq!(TokenMode::parse("peer"), TokenMode::Readonly);
    }

    #[test]
    fn a_peer_mode_row_resolves_only_through_a_client_row() {
        // A host token row whose `mode` column somehow holds "peer" still
        // resolves as `Readonly` — `resolve_token`'s host loop uses `parse`,
        // not `parse_client`.
        let hosts = [host_row("hub-a", "tok-hub", "peer")];
        assert_eq!(
            resolve_token("tok-hub", "master-tok", &hosts, &[])
                .unwrap()
                .mode,
            TokenMode::Readonly
        );
        // A client row with mode "peer" resolves to `TokenMode::Peer`.
        let clients = vec![client_row(9, "hub-b", "tok-client", "peer")];
        let c = resolve_token("tok-client", "master-tok", &[], &clients).unwrap();
        assert_eq!(c.mode, TokenMode::Peer);
        assert!(c.is_client());
        assert!(!c.is_master());
    }

    #[test]
    fn caller_labels_and_master_flag() {
        assert!(Caller::master().is_master());
        assert_eq!(Caller::master().label(), "master");
        assert!(!Caller::master().is_client());
        let c = Caller {
            host_alias: Some("mefistos".into()),
            client: None,
            mode: TokenMode::Full,
        };
        assert!(!c.is_master());
        assert!(!c.is_client());
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
            // The whole of 127.0.0.0/8, since the rule moved to
            // `fleet_proto::net`. A `Host` header that is an IP literal
            // cannot be an attacker's DNS-rebinding name, so widening from
            // the single address costs nothing: these really are this
            // machine.
            "127.0.0.53",
            "127.0.0.53:4180",
            "127.255.255.255",
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
            // A name under `localhost` is a DNS lookup on a resolver that
            // does not honour RFC 6761, so it is not this machine.
            "evil.localhost",
            "evil.localhost:4180",
            // An IPv4-mapped v6 address is routable, not `::1`.
            "[::ffff:127.0.0.1]",
            "[::ffff:127.0.0.1]:4180",
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
        assert_eq!(
            check_request(&h, "s3cret", &[], &[], &[]),
            Ok(Caller::master())
        );
    }

    #[test]
    fn check_request_identifies_host_token_callers() {
        let h = headers(&[("authorization", "Bearer tok-mef")]);
        let caller = check_request(
            &h,
            "s3cret",
            &[host_row("mefistos", "tok-mef", "readonly")],
            &[],
            &[],
        )
        .unwrap();
        assert_eq!(caller.host_alias.as_deref(), Some("mefistos"));
        assert_eq!(caller.mode, TokenMode::Readonly);
    }

    #[test]
    fn check_request_allows_non_browser_client_without_origin() {
        // A CLI MCP client sends no Origin — only the token gates it.
        let h = headers(&[("authorization", "Bearer s3cret")]);
        assert!(check_request(&h, "s3cret", &[], &[], &[]).is_ok());
    }

    #[test]
    fn check_request_rejects_wrong_token_with_401() {
        let h = headers(&[("host", "127.0.0.1:4180"), ("authorization", "Bearer nope")]);
        assert_eq!(
            check_request(&h, "s3cret", &[], &[], &[]),
            Err(StatusCode::UNAUTHORIZED)
        );
        let none = headers(&[("host", "127.0.0.1:4180")]);
        assert_eq!(
            check_request(&none, "s3cret", &[], &[], &[]),
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
        assert_eq!(
            check_request(&h, "s3cret", &[], &[], &[]),
            Err(StatusCode::FORBIDDEN)
        );
        assert_eq!(check_origin(&h, &[]), Err(StatusCode::FORBIDDEN));
    }

    #[test]
    fn check_request_rejects_rebound_host_with_403() {
        // Host header carrying the attacker's domain (rebound to 127.0.0.1).
        let h = headers(&[("host", "evil.com"), ("authorization", "Bearer s3cret")]);
        assert_eq!(
            check_request(&h, "s3cret", &[], &[], &[]),
            Err(StatusCode::FORBIDDEN)
        );
    }

    fn allow_headers(host: &str, origin: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, host.parse().unwrap());
        if let Some(o) = origin {
            h.insert(header::ORIGIN, o.parse().unwrap());
        }
        h.insert(header::AUTHORIZATION, "Bearer s3cret".parse().unwrap());
        h
    }

    #[test]
    fn allowlisted_host_and_origin_pass_others_still_403() {
        let allowed = normalize_allowed_hosts(&["Fleet.Example.com".into()]);
        // Bare host and port-qualified authority both match, case-insensitively.
        assert!(check_request(
            &allow_headers("fleet.example.com", None),
            "s3cret",
            &[],
            &[],
            &allowed
        )
        .is_ok());
        assert!(check_request(
            &allow_headers("FLEET.example.com:443", None),
            "s3cret",
            &[],
            &[],
            &allowed
        )
        .is_ok());
        assert!(check_request(
            &allow_headers("fleet.example.com", Some("https://fleet.example.com")),
            "s3cret",
            &[],
            &[],
            &allowed
        )
        .is_ok());
        // Loopback keeps working with a non-empty list.
        assert!(check_request(
            &allow_headers("127.0.0.1:4180", None),
            "s3cret",
            &[],
            &[],
            &allowed
        )
        .is_ok());
        // Not listed → 403 before the token is looked at.
        assert_eq!(
            check_request(
                &allow_headers("evil.example.com", None),
                "s3cret",
                &[],
                &[],
                &allowed
            ),
            Err(StatusCode::FORBIDDEN)
        );
        assert_eq!(
            check_request(
                &allow_headers("fleet.example.com", Some("https://evil.example.com")),
                "s3cret",
                &[],
                &[],
                &allowed
            ),
            Err(StatusCode::FORBIDDEN)
        );
        // An empty list is today's behaviour: loopback only.
        assert_eq!(
            check_request(
                &allow_headers("fleet.example.com", None),
                "s3cret",
                &[],
                &[],
                &[]
            ),
            Err(StatusCode::FORBIDDEN)
        );
    }

    #[test]
    fn normalize_allowed_hosts_trims_lowercases_and_drops_empties() {
        let out = normalize_allowed_hosts(&[" A.Example.com ".into(), "".into(), "b:8443".into()]);
        assert_eq!(out, vec!["a.example.com".to_string(), "b:8443".to_string()]);
    }
}
