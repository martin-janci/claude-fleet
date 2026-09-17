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
#[derive(Clone, Debug)]
pub struct PairingRequest {
    pub code: String,
    pub name: String,
    pub mode: String,
    pub expires_at: Instant,
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
    /// [`ATTEMPT_INTERVAL`] in production; tests dial it down.
    pub attempt_interval: Duration,
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
    let peer = request
        .extensions()
        .get::<axum::extract::ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip().to_string())
        .unwrap_or_else(|| UNKNOWN_PEER.to_string());
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
            axum::Json(serde_json::json!({
                "token": token,
                "name": row.name,
                "mode": row.mode,
                "hub": state.base_url.as_str(),
            }))
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
