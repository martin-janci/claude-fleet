//! Pairing: how a client (a phone, a browser on a laptop) gets its first
//! credential.
//!
//! A pairing is a two-step exchange rather than a token printed in a QR code.
//! `pair_client` (the MCP tool the `fleet-hub pair` CLI drives) mints a short
//! **code** — 8 Crockford base32 characters from the CSPRNG, kept in memory
//! only — and the terminal shows [`pair_url`] as a QR. The client then posts
//! the code to [`handle_pair`], which consumes it once and answers with a
//! freshly generated client token.
//!
//! Why the extra hop: the QR is displayed on a terminal that may be shared,
//! screen-shotted, scrolled back or logged. A code that dies on first use and
//! expires in minutes is a far smaller thing to leak than a bearer token that
//! is good until it is revoked.
//!
//! Pending codes live in this process only: a hub restart invalidates every
//! outstanding code, which is the desired failure mode.

use super::auth;
use super::guard::RateLimiter;
use crate::store::Store;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Crockford base32: the digits plus the letters, minus `I`, `L`, `O` and
/// `U` — so a code read off a screen cannot be mistyped as a 1/0 or read as
/// an unfortunate word.
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Characters in a pairing code: 8 × 5 bits = 40 bits of entropy, which is
/// plenty for a code that expires in minutes, dies on first use and is
/// guessed at a rate the [`RateLimiter`] pins to ten attempts a minute.
const CODE_LEN: usize = 8;

/// Default lifetime of a pairing code when the caller names none.
pub const DEFAULT_TTL: Duration = Duration::from_secs(10 * 60);

/// Minimum spacing between two `POST /pair` attempts from one address — one
/// every 6 s, i.e. the ten-a-minute budget the design asks for.
pub const ATTEMPT_INTERVAL: Duration = Duration::from_secs(6);

/// Largest `POST /pair` body accepted. The real one is a few dozen bytes.
const MAX_BODY: usize = 4 * 1024;

/// Rate-limit key for a request whose peer address is unknown (no
/// `ConnectInfo` in the extensions). One shared bucket, so an unknown peer is
/// throttled at least as hard as a known one — never less.
const UNKNOWN_PEER: &str = "unknown";

/// One minted, not-yet-used pairing code.
#[derive(Clone)]
pub struct PairingRequest {
    pub code: String,
    pub name: String,
    pub mode: String,
    pub expires_at: Instant,
}

/// The code is a credential: a `{:?}` in a log line or an error message must
/// not spell it out. Everything else about a pairing is safe to print.
impl std::fmt::Debug for PairingRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairingRequest")
            .field("code", &"<redacted>")
            .field("name", &self.name)
            .field("mode", &self.mode)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// The rate-limit bucket for one `POST /pair` attempt.
///
/// The budget is per source address, so the address has to be the real one.
/// The shipped compose topology puts Caddy in front of the hub, which makes
/// every request's TCP peer the proxy: one bucket for the whole internet, and
/// anyone could hold the operator's phone at 429 by guessing codes. So when —
/// and only when — the peer is a loopback or private address (i.e. plausibly
/// that front end) the `X-Forwarded-For` chain is believed, and then only its
/// LAST hop: that is the one the trusted proxy appended itself, while every
/// earlier hop is whatever the client claimed. From a routable peer the
/// header is attacker-chosen — honouring it would hand a flooder a fresh
/// bucket per request — so the peer itself is the key.
pub(crate) fn limiter_key(
    peer: Option<std::net::IpAddr>,
    headers: &axum::http::HeaderMap,
) -> String {
    let Some(peer) = peer else {
        return UNKNOWN_PEER.to_string();
    };
    if !is_trusted_front_end(peer) {
        return peer.to_string();
    }
    forwarded_last_hop(headers).unwrap_or_else(|| peer.to_string())
}

/// The last `X-Forwarded-For` hop, when it parses as an IP address. Several
/// header instances are read as one chain, so the last value of the last
/// header wins — that is the hop the nearest proxy appended.
fn forwarded_last_hop(headers: &axum::http::HeaderMap) -> Option<String> {
    let last = headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .rfind(|h| !h.is_empty())?;
    // A bracketed IPv6 form (`[::1]:443`) and a `host:port` v4 form both show
    // up behind some proxies; anything that is not a bare address is refused
    // rather than guessed at, and the peer is used instead.
    last.parse::<std::net::IpAddr>()
        .ok()
        .map(|ip| ip.to_string())
}

/// Whether `ip` may be believed when it forwards an address: loopback, or a
/// private / link-local / unique-local address. That is where the shipped
/// compose topology puts the reverse proxy. Deliberately NOT a configurable
/// allowlist: the only thing this decides is which bucket an attempt is
/// counted against, and a wrong answer costs a shared bucket, not access.
fn is_trusted_front_end(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    // `::ffff:10.0.0.1` is the same machine as `10.0.0.1`.
    let ip = match ip {
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(v6)),
        v4 => v4,
    };
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            // fc00::/7 (unique local) and fe80::/10 (link local); `is_unique_local`
            // is still unstable, so the prefixes are spelled out.
            v6.is_loopback() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
    }
}

/// What the registry stores per code (the code itself is the map key).
struct Pending {
    name: String,
    mode: String,
    expires_at: Instant,
}

/// The in-memory registry of outstanding pairing codes, shared by the MCP
/// tool that mints them and the `/pair` route that consumes them.
#[derive(Default)]
pub struct PendingPairings {
    pending: Mutex<HashMap<String, Pending>>,
}

impl PendingPairings {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Pending>> {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Mint a fresh code for `name`/`mode`, valid for `ttl`. Sweeps expired
    /// entries first, so the map cannot grow without bound on a hub where
    /// codes are minted and never redeemed.
    pub fn mint(&self, name: &str, mode: &str, ttl: Duration) -> PairingRequest {
        let now = Instant::now();
        self.sweep(now);
        let expires_at = now + ttl;
        let mut pending = self.lock();
        // A 40-bit collision is not going to happen; redrawing costs nothing
        // and keeps `insert` from silently stealing another client's code.
        let code = loop {
            let candidate = random_code();
            if !pending.contains_key(&candidate) {
                break candidate;
            }
        };
        pending.insert(
            code.clone(),
            Pending {
                name: name.to_string(),
                mode: mode.to_string(),
                expires_at,
            },
        );
        PairingRequest {
            code,
            name: name.to_string(),
            mode: mode.to_string(),
            expires_at,
        }
    }

    /// Redeem `code`: `None` when it is unknown, already used or expired —
    /// the three failures the caller must not be able to tell apart.
    ///
    /// The lookup is a full scan with a constant-time comparison rather than
    /// a hash lookup: it never returns early, so how long the answer takes
    /// does not depend on how much of a guessed code was right. An expired
    /// entry is removed on the way out, so a late guess of a code that has
    /// timed out still finds nothing.
    pub fn consume(&self, code: &str) -> Option<PairingRequest> {
        let now = Instant::now();
        let mut pending = self.lock();
        let mut hit: Option<String> = None;
        for key in pending.keys() {
            if auth::constant_time_eq(key.as_bytes(), code.as_bytes()) {
                hit = Some(key.clone());
            }
        }
        let key = hit?;
        let entry = pending.remove(&key)?;
        if entry.expires_at <= now {
            return None;
        }
        Some(PairingRequest {
            code: key,
            name: entry.name,
            mode: entry.mode,
            expires_at: entry.expires_at,
        })
    }

    /// Drop every entry that has expired as of `now`.
    pub fn sweep(&self, now: Instant) {
        self.lock().retain(|_, p| p.expires_at > now);
    }

    /// How many codes are outstanding (after no sweep) — for tests and for
    /// the pairing tool's own bookkeeping.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Eight Crockford base32 characters drawn from 5 CSPRNG bytes (40 bits, so
/// every bit drawn is used and no character is biased).
fn random_code() -> String {
    use rand::Rng;
    let mut raw = [0u8; 5];
    rand::rng().fill_bytes(&mut raw);
    let bits = raw.iter().fold(0u64, |acc, b| (acc << 8) | u64::from(*b));
    let mut code = String::with_capacity(CODE_LEN);
    for i in (0..CODE_LEN).rev() {
        let idx = ((bits >> (i * 5)) & 0x1f) as usize;
        code.push(CROCKFORD[idx] as char);
    }
    code
}

/// The URL a pairing QR encodes. The code travels in the **fragment**, which
/// a browser never puts on the wire and no reverse proxy or access log ever
/// sees; the page at `/pair` reads it and posts it back itself.
pub fn pair_url(base: &str, code: &str) -> String {
    format!("{}/pair#{code}", base.trim_end_matches('/'))
}

/// What `POST /pair` needs: the store to write the new client row into, the
/// registry the code was minted in, a limiter for the per-address attempt
/// budget, and the base URL to hand the client so it knows where to come
/// back. Cheap to clone.
#[derive(Clone)]
pub struct PairState {
    pub store: Arc<Mutex<Store>>,
    pub pairings: Arc<PendingPairings>,
    pub rate: Arc<RateLimiter>,
    /// The hub's public URL, or its loopback base — echoed as `hub`.
    pub base_url: Arc<String>,
    /// Minimum spacing between two attempts from one address.
    /// [`ATTEMPT_INTERVAL`] in production; tests dial it down. `pub(crate)`
    /// so only this crate's tests can weaken the budget — an embedder must
    /// not be able to switch it off.
    pub(crate) attempt_interval: Duration,
}

impl PairState {
    pub fn new(
        store: Arc<Mutex<Store>>,
        pairings: Arc<PendingPairings>,
        rate: Arc<RateLimiter>,
        base_url: String,
    ) -> Self {
        Self {
            store,
            pairings,
            rate,
            base_url: Arc::new(base_url),
            attempt_interval: ATTEMPT_INTERVAL,
        }
    }
}

/// The `POST /pair` request body.
#[derive(serde::Deserialize)]
pub struct PairBody {
    pub code: String,
}

/// The page a camera scan lands on. Deliberately inert: no JavaScript, no
/// auto-redeem, no secret. The code is in the URL **fragment**, which the
/// browser never sends, so this page could not redeem it even if it wanted
/// to — and the claude-fleet client on the device, which does read the
/// fragment, is what the reader is pointed at. Without it a scan would get a
/// bare `405 Method Not Allowed` and look broken.
const PAIR_PAGE: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex, nofollow">
<title>claude-fleet pairing</title>
<style>
 :root { color-scheme: light dark }
 body { margin: 0; padding: 2rem 1.25rem; font: 16px/1.55 system-ui, sans-serif; max-width: 34rem }
 h1 { font-size: 1.25rem; margin: 0 0 1rem }
 p { margin: 0 0 1rem }
 .muted { opacity: .7; font-size: .875rem }
</style></head><body>
<h1>claude-fleet pairing</h1>
<p>This link pairs a device with a claude-fleet hub.</p>
<p><strong>Open the claude-fleet app on this device</strong> and scan the code
again from inside it. The app reads the pairing code out of this link and
exchanges it for a credential of its own.</p>
<p class="muted">The code is in the part of the address after the
<code>#</code>, which your browser never sends to the hub, so this page cannot
pair anything by itself. A pairing code can be used once and expires within
minutes.</p>
</body></html>
"#;

/// `GET /pair` — the static page above. Unauthenticated like the POST, and
/// for the same reason: whoever scans the QR has no credential yet.
pub async fn handle_pair_page() -> Response {
    (
        [
            (axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (axum::http::header::CACHE_CONTROL, "no-store"),
        ],
        PAIR_PAGE,
    )
        .into_response()
}

/// The one answer every bad code gets: unknown, already used and expired are
/// indistinguishable, and nothing in the body or the log names the code.
fn invalid_code() -> Response {
    (
        StatusCode::NOT_FOUND,
        axum::Json(serde_json::json!({ "error": "invalid code" })),
    )
        .into_response()
}

/// `POST /pair` — redeem a pairing code for a client token. **Unauthenticated
/// by design**: this is how a client obtains its first credential, so it
/// cannot be asked for one. See `mcp::build_app` for where it is mounted
/// relative to the `authorize` layer, and why.
///
/// Nothing token-shaped is ever logged: not the presented code, not the
/// minted token, not its hash. The only thing a failure records is that an
/// attempt came from an address.
pub async fn handle_pair(
    axum::extract::State(state): axum::extract::State<PairState>,
    request: axum::extract::Request,
) -> Response {
    let peer = limiter_key(
        request
            .extensions()
            .get::<axum::extract::ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip()),
        request.headers(),
    );
    // Spend the attempt budget before parsing anything: a flood of guesses
    // must cost the hub a hash-map probe, not a body read.
    if let Err(left) = state
        .rate
        .check(&format!("pair:{peer}"), state.attempt_interval)
    {
        let retry = left.as_secs().max(1).to_string();
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(axum::http::header::RETRY_AFTER, retry)],
            axum::Json(serde_json::json!({ "error": "too many attempts" })),
        )
            .into_response();
    }
    let bytes = match axum::body::to_bytes(request.into_body(), MAX_BODY).await {
        Ok(b) => b,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let Ok(body) = serde_json::from_slice::<PairBody>(&bytes) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Some(req) = state.pairings.consume(&body.code) else {
        tracing::warn!(peer = %peer, "[mcp] refused a pairing attempt");
        return invalid_code();
    };
    let token = super::generate_token();
    // Only the hash is stored; the plaintext leaves in this one response and
    // is never recoverable afterwards.
    let hash = auth::sha256_hex(&token);
    // Sync lock in its own scope — never held across an `.await`.
    let inserted = {
        let Ok(s) = state.store.lock() else {
            tracing::error!("[mcp] store lock poisoned while pairing");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        s.insert_client_token(&req.name, &hash, &req.mode)
    };
    match inserted {
        Ok(row) => {
            tracing::info!(client = %row.name, mode = %row.mode, "[mcp] paired a client");
            // The ONE response that ever carries the plaintext token. Tell
            // every cache between here and the phone to keep no copy of it.
            (
                [(axum::http::header::CACHE_CONTROL, "no-store")],
                axum::Json(serde_json::json!({
                    "token": token,
                    "name": row.name,
                    "mode": row.mode,
                    "hub": state.base_url.as_str(),
                })),
            )
                .into_response()
        }
        Err(e) => {
            // The code is spent either way — mint a new one. The usual cause
            // is a live client already holding this name.
            tracing::warn!(error = %e.message, "[mcp] could not store a paired client");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(serde_json::json!({ "error": "pairing failed" })),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_code_is_eight_crockford_chars_and_unique() {
        let p = PendingPairings::new();
        let a = p.mint("phone", "full", Duration::from_secs(600));
        let b = p.mint("tablet", "full", Duration::from_secs(600));
        assert_eq!(a.code.len(), 8);
        assert!(
            a.code
                .chars()
                .all(|c| "0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(c)),
            "{}",
            a.code
        );
        assert_ne!(a.code, b.code);
    }

    #[test]
    fn a_code_works_once() {
        let p = PendingPairings::new();
        let req = p.mint("phone", "readonly", Duration::from_secs(600));
        let got = p.consume(&req.code).expect("first use");
        assert_eq!(got.name, "phone");
        assert_eq!(got.mode, "readonly");
        assert!(p.consume(&req.code).is_none(), "second use must fail");
    }

    #[test]
    fn an_expired_code_is_refused_and_swept() {
        let p = PendingPairings::new();
        let req = p.mint("phone", "full", Duration::from_millis(1));
        std::thread::sleep(Duration::from_millis(5));
        assert!(p.consume(&req.code).is_none());
    }

    #[test]
    fn an_unknown_code_is_refused() {
        let p = PendingPairings::new();
        assert!(p.consume("ZZZZZZZZ").is_none());
    }

    #[test]
    fn pair_url_puts_the_code_in_the_fragment() {
        assert_eq!(
            pair_url("https://fleet.example.com", "ABCD1234"),
            "https://fleet.example.com/pair#ABCD1234"
        );
    }

    #[test]
    fn pair_url_does_not_double_the_slash() {
        assert_eq!(
            pair_url("http://127.0.0.1:4180/", "ABCD1234"),
            "http://127.0.0.1:4180/pair#ABCD1234"
        );
    }

    /// A consumed code frees its slot, and `mint` sweeps what timed out, so
    /// the registry does not grow on a hub whose codes are never redeemed.
    #[test]
    fn minting_sweeps_expired_codes() {
        let p = PendingPairings::new();
        let live = p.mint("kept", "full", Duration::from_secs(600));
        p.mint("stale", "full", Duration::from_millis(1));
        std::thread::sleep(Duration::from_millis(5));
        p.mint("fresh", "full", Duration::from_secs(600));
        assert_eq!(p.len(), 2, "the expired code must have been swept");
        assert!(p.consume(&live.code).is_some(), "a live code still works");
        assert_eq!(p.len(), 1, "consuming frees the slot");
    }

    /// A `PairingRequest` travels through the tool layer and could land in a
    /// `{:?}` log line or an error message; the code is a credential, so its
    /// `Debug` shows a placeholder.
    #[test]
    fn debug_never_prints_the_code() {
        let p = PendingPairings::new();
        let req = p.mint("phone", "full", Duration::from_secs(600));
        let rendered = format!("{req:?}");
        assert!(
            !rendered.contains(&req.code),
            "the code must not appear in Debug output: {rendered}"
        );
        assert!(rendered.contains("phone"), "{rendered}");
        assert!(rendered.contains("<redacted>"), "{rendered}");
    }

    /// The attempt budget is keyed on the source address. Behind the shipped
    /// compose topology the TCP peer is Caddy, so every phone on the internet
    /// would share one bucket and anyone could hold the operator's phone at
    /// 429. `X-Forwarded-For` fixes that — but only when the peer really is a
    /// front end: from a routable peer the header is attacker-chosen and
    /// would hand a flooder a fresh bucket per request.
    #[test]
    fn the_forwarded_key_is_trusted_only_from_a_loopback_or_private_peer() {
        let hdrs = |v: &str| {
            let mut h = axum::http::HeaderMap::new();
            h.insert("x-forwarded-for", v.parse().unwrap());
            h
        };
        let ip = |s: &str| Some(s.parse::<std::net::IpAddr>().unwrap());
        // Loopback and private peers are the proxy: the LAST hop is the one
        // it appended itself; earlier hops are the client's own claim.
        assert_eq!(
            limiter_key(ip("127.0.0.1"), &hdrs("9.9.9.9, 203.0.113.7")),
            "203.0.113.7"
        );
        assert_eq!(
            limiter_key(ip("172.18.0.4"), &hdrs("203.0.113.8")),
            "203.0.113.8"
        );
        assert_eq!(limiter_key(ip("::1"), &hdrs("203.0.113.9")), "203.0.113.9");
        // A routable peer's header is ignored — the peer is the key.
        assert_eq!(
            limiter_key(ip("198.51.100.4"), &hdrs("203.0.113.7")),
            "198.51.100.4"
        );
        // Garbage from a trusted peer falls back to the peer, never to a
        // bucket an attacker chose.
        assert_eq!(
            limiter_key(ip("127.0.0.1"), &hdrs("not-an-ip")),
            "127.0.0.1"
        );
        assert_eq!(limiter_key(ip("127.0.0.1"), &hdrs("")), "127.0.0.1");
        // No header, and no peer at all.
        assert_eq!(
            limiter_key(ip("127.0.0.1"), &axum::http::HeaderMap::new()),
            "127.0.0.1"
        );
        assert_eq!(limiter_key(None, &hdrs("203.0.113.7")), UNKNOWN_PEER);
    }

    /// Codes are minted per request, so two clients never share one.
    #[test]
    fn each_code_carries_its_own_name_and_mode() {
        let p = PendingPairings::new();
        let a = p.mint("phone", "full", Duration::from_secs(600));
        let b = p.mint("kiosk", "readonly", Duration::from_secs(600));
        let got_b = p.consume(&b.code).expect("kiosk");
        assert_eq!(
            (got_b.name.as_str(), got_b.mode.as_str()),
            ("kiosk", "readonly")
        );
        let got_a = p.consume(&a.code).expect("phone");
        assert_eq!(
            (got_a.name.as_str(), got_a.mode.as_str()),
            ("phone", "full")
        );
    }
}
