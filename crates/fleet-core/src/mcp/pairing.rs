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

/// `POST /pair` attempts allowed across the whole hub, whatever address they
/// come from, in any rolling [`GLOBAL_WINDOW`]. The per-address budget alone
/// is only as good as the address: a peer behind a believed front end can
/// name a fresh `X-Forwarded-For` address per request, and an IPv6 host owns
/// a /64 of them. A window rather than a fixed spacing because pairing comes
/// in bursts (an operator pairing a phone, a tablet and a laptop in a row)
/// that a one-a-second spacing would refuse; thirty a minute caps guessing
/// however the addresses are minted and costs no real pairing anything.
pub const GLOBAL_ATTEMPTS: usize = 30;

/// The window [`GLOBAL_ATTEMPTS`] is counted over.
pub const GLOBAL_WINDOW: Duration = Duration::from_secs(60);

/// The hub-wide attempt budget: the times of the attempts it let through in
/// the current window. Holds at most the limit's worth of instants.
#[derive(Default)]
pub struct GlobalBudget {
    recent: Mutex<std::collections::VecDeque<Instant>>,
}

impl GlobalBudget {
    fn lock(&self) -> std::sync::MutexGuard<'_, std::collections::VecDeque<Instant>> {
        self.recent
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// `Err(time until a slot frees)` when `limit` attempts already landed in
    /// the `window` ending at `now`. Records nothing: [`Self::record`] does,
    /// once the per-address budget has let the attempt through too, so one
    /// address hammering its own bucket cannot spend the hub's.
    fn peek(&self, now: Instant, limit: usize, window: Duration) -> Result<(), Duration> {
        let mut recent = self.lock();
        while recent
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) >= window)
        {
            recent.pop_front();
        }
        match recent.front() {
            Some(oldest) if recent.len() >= limit => {
                Err(window.saturating_sub(now.saturating_duration_since(*oldest)))
            }
            _ => Ok(()),
        }
    }

    fn record(&self, now: Instant, limit: usize) {
        let mut recent = self.lock();
        recent.push_back(now);
        while recent.len() > limit {
            recent.pop_front();
        }
    }
}

/// Largest `POST /pair` body accepted. The real one is a few dozen bytes.
const MAX_BODY: usize = 4 * 1024;

/// Rate-limit key for a request whose peer address is unknown (no
/// `ConnectInfo` in the extensions). One shared bucket, so an unknown peer is
/// throttled at least as hard as a known one — never less.
const UNKNOWN_PEER: &str = "unknown";

/// What one `pair_client` call asks for. A struct rather than six
/// positionals: the list has grown once per feature (trust, then the org,
/// then the person) and a sixth `Option<i64>` in a row of them is how a
/// caller puts the org where the person goes.
#[derive(Clone, Copy)]
pub struct MintRequest<'a> {
    pub name: &'a str,
    pub mode: &'a str,
    /// Pair as a device the operator vouches for
    /// (`client_tokens.trusted_at`): its prompts are delivered unmarked.
    pub trusted: bool,
    /// Bind to this org the moment it pairs (work graph M14).
    pub org_id: Option<i64>,
    /// WHOSE device this is (multi-user M1), by NAME.
    ///
    /// A name and not an id because the `people` row is created at
    /// **redemption**: [`PendingPairings`] is in-process memory, so a code
    /// minted and never walked to the phone must not leave an orphan person
    /// behind. The name's shape is validated at mint all the same, so the
    /// operator reads the error at their own terminal rather than the phone
    /// reading it minutes later.
    ///
    /// `None` only for a `peer` or `updater` code: a linked hub and
    /// `fleet-updater` are not anybody's device, and
    /// `Store::set_client_person` refuses both. Every ordinary pairing
    /// carries a person — `pair_client` defaults it to the hub's personal
    /// owner rather than minting a person-less token.
    pub person: Option<&'a str>,
    /// How long the code stays valid; it also dies on first use.
    pub ttl: Duration,
}

/// One minted, not-yet-used pairing code.
#[derive(Clone)]
pub struct PairingRequest {
    pub code: String,
    pub name: String,
    pub mode: String,
    /// Whether the client is paired as one the operator vouches for
    /// (`client_tokens.trusted_at`): its prompts are delivered unmarked.
    pub trusted: bool,
    /// The org the client is bound to once paired (work graph M14,
    /// `fleet-hub pair --org`); `None`: unbound.
    pub org_id: Option<i64>,
    /// The person the client is bound to once paired (multi-user M1,
    /// `fleet-hub pair --person`), by name — see [`MintRequest::person`].
    pub person: Option<String>,
    pub expires_at: Instant,
}

/// The code is a credential: a `{:?}` in a log line or an error message must
/// not spell it out. Everything else about a pairing is safe to print — a
/// person's name included: it is what the operator typed at their own
/// terminal, and seeing it in a log line is how they notice a typo made a
/// second colleague.
impl std::fmt::Debug for PairingRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairingRequest")
            .field("code", &"<redacted>")
            .field("name", &self.name)
            .field("mode", &self.mode)
            .field("trusted", &self.trusted)
            .field("person", &self.person)
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
        return bucket_of(peer);
    }
    forwarded_last_hop(headers).map_or_else(|| bucket_of(peer), bucket_of)
}

/// The budget an address draws on: itself, or for a routable IPv6 address
/// its /64 — one host is routinely handed a whole /64, so per-address
/// buckets would let it mint a fresh one per request. An IPv4-mapped IPv6
/// address is its IPv4 address; loopback stays itself.
fn bucket_of(ip: std::net::IpAddr) -> String {
    use std::net::IpAddr;
    match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => v4.to_string(),
            None if v6.is_loopback() => v6.to_string(),
            None => {
                let s = v6.segments();
                format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
            }
        },
        v4 => v4.to_string(),
    }
}

/// The per-address bucket for a [`limiter_key`]: an IPv6 address counts as
/// its /64, the block one host or one subscriber line is handed, so cycling
/// addresses inside it does not mint fresh buckets. IPv4 (mapped or not) and
/// a non-address key are their own bucket.
fn address_bucket(key: &str) -> String {
    match key.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V6(v6)) if v6.to_ipv4_mapped().is_none() => {
            let s = v6.segments();
            format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
        }
        _ => key.to_string(),
    }
}

/// The last `X-Forwarded-For` hop that parses as an IP address. Several
/// header instances are read as one chain, so the chain is scanned
/// right-to-left — the rightmost hop is the one the nearest proxy appended,
/// and everything left of it is whatever the client claimed.
///
/// The scan does not stop at the rightmost element: some proxies append a
/// non-address token of their own (`unknown` is the classic, and a bracketed
/// `[::1]:443` or a `host:port` form shows up too). Testing only the last
/// element would then find nothing, fall back to the peer, and put every
/// client behind that proxy into the proxy's single bucket — exactly the
/// shared bucket this function exists to avoid. So it keeps walking left
/// until something parses; only a chain with no parseable hop at all falls
/// back to the peer.
fn forwarded_last_hop(headers: &axum::http::HeaderMap) -> Option<std::net::IpAddr> {
    let chain: Vec<&str> = headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .collect();
    chain
        .iter()
        .rev()
        .find_map(|h| h.parse::<std::net::IpAddr>().ok())
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
    trusted: bool,
    org_id: Option<i64>,
    person: Option<String>,
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

    /// Mint a fresh code for `req`, valid for its `ttl`. Sweeps expired
    /// entries first, so the map cannot grow without bound on a hub where
    /// codes are minted and never redeemed.
    ///
    /// Everything the pairing is to become rides on the code — trust, the
    /// org, the person — so there is no window in which the new client is
    /// live but unbound.
    pub fn mint(&self, req: MintRequest<'_>) -> PairingRequest {
        let now = Instant::now();
        self.sweep(now);
        let expires_at = now + req.ttl;
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
                name: req.name.to_string(),
                mode: req.mode.to_string(),
                trusted: req.trusted,
                org_id: req.org_id,
                person: req.person.map(str::to_string),
                expires_at,
            },
        );
        PairingRequest {
            code,
            name: req.name.to_string(),
            mode: req.mode.to_string(),
            trusted: req.trusted,
            org_id: req.org_id,
            person: req.person.map(str::to_string),
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
            trusted: entry.trusted,
            org_id: entry.org_id,
            person: entry.person,
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
    /// The hub-wide attempt budget, shared by every clone.
    pub global: Arc<GlobalBudget>,
    /// Attempts allowed hub-wide per `global_window`: [`GLOBAL_ATTEMPTS`]
    /// and [`GLOBAL_WINDOW`] in production; `pub(crate)` for the same reason
    /// as `attempt_interval`.
    pub(crate) global_limit: usize,
    pub(crate) global_window: Duration,
    /// Single sign-on (`mcp::oidc`): the provider a sign-in at
    /// `/auth/oidc/start` goes to, or `None` when the hub has none — the
    /// default, under which both `/auth/oidc` routes answer 404.
    pub oidc: Option<Arc<super::oidc::OidcProvider>>,
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
            global: Arc::new(GlobalBudget::default()),
            global_limit: GLOBAL_ATTEMPTS,
            global_window: GLOBAL_WINDOW,
            oidc: None,
        }
    }

    /// The same state with single sign-on through `oidc`.
    pub fn with_oidc(mut self, oidc: Option<Arc<super::oidc::OidcProvider>>) -> Self {
        self.oidc = oidc;
        self
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
<title>Orbit Fleet pairing</title>
<style>
 :root { color-scheme: light dark }
 body { margin: 0; padding: 2rem 1.25rem; font: 16px/1.55 system-ui, sans-serif; max-width: 34rem }
 h1 { font-size: 1.25rem; margin: 0 0 1rem }
 p { margin: 0 0 1rem }
 .muted { opacity: .7; font-size: .875rem }
</style></head><body>
<h1>Orbit Fleet pairing</h1>
<p>This link pairs a device with an Orbit Fleet hub.</p>
<p><strong>Open the Orbit Fleet app on this device</strong> and scan the code
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
    // must cost the hub a hash-map probe, not a body read. The hub-wide
    // budget is asked first, so a flood of minted addresses is cut off
    // before each one gets a bucket of its own, and charged last, so an
    // attempt its own address's budget refuses costs the hub nothing.
    let now = Instant::now();
    let budget = state
        .global
        .peek(now, state.global_limit, state.global_window)
        .and_then(|()| {
            state.rate.check(
                &format!("pair:{}", address_bucket(&peer)),
                state.attempt_interval,
            )
        })
        .map(|()| state.global.record(now, state.global_limit));
    if let Err(left) = budget {
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
        // One transaction: a bind that fails after the insert (the org was
        // removed while the code waited) must not leave a live, unbound
        // row behind that nobody holds the token for and that keeps the
        // name taken. The code is spent either way.
        s.atomically(|s| {
            // The trust grant rides on the same code the operator minted: a
            // `--trusted` pairing lands trusted, nothing else does.
            s.insert_client_token(&req.name, &hash, &req.mode)
                .and_then(|row| {
                    if req.trusted {
                        s.set_client_trust(&row.name, true)
                    } else {
                        Ok(row)
                    }
                })
                .and_then(|row| match req.org_id {
                    // The org binding rides on the code too (work graph M14).
                    Some(org) => s.set_client_org(&row.name, Some(org)),
                    None => Ok(row),
                })
                .and_then(|row| match req.person.as_deref() {
                    // And WHOSE device it is (multi-user M1). The `people` row
                    // is created HERE, at redemption, not at mint: a code minted
                    // and never walked to the phone must not leave an orphan
                    // person behind. An existing live person of that name is
                    // reused, which is what makes "pair my second phone" and
                    // "pair a colleague's laptop" the same command.
                    Some(person) => {
                        let row_id = match s.get_person_by_name(person)? {
                            Some(p) => p.id,
                            None => s.create_person(person, None)?.id,
                        };
                        s.set_client_person(&row.name, Some(row_id))
                    }
                    // A `peer` link and an `updater` token are nobody's device;
                    // `pair_client` refuses `person` for both.
                    None => Ok(row),
                })
        })
    };
    match inserted {
        Ok(row) => {
            tracing::info!(
                client = %row.name,
                mode = %row.mode,
                trusted = row.trusted_at.is_some(),
                person = ?row.person_id,
                "[mcp] paired a client"
            );
            // The ONE response that ever carries the plaintext token. Tell
            // every cache between here and the phone to keep no copy of it.
            (
                [(axum::http::header::CACHE_CONTROL, "no-store")],
                axum::Json(serde_json::json!({
                    "token": token,
                    "name": row.name,
                    "mode": row.mode,
                    "trusted": row.trusted_at.is_some(),
                    "org_id": row.org_id,
                    "person_id": row.person_id,
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

    /// The plain mint these tests want: a device of the hub's own owner,
    /// bound to no org. Spelled once here so a test reads as "a code for
    /// `phone`, `full`, not trusted" rather than as a six-field literal.
    fn mint(
        p: &PendingPairings,
        name: &str,
        mode: &str,
        trusted: bool,
        ttl: Duration,
    ) -> PairingRequest {
        p.mint(MintRequest {
            name,
            mode,
            trusted,
            org_id: None,
            person: Some("owner"),
            ttl,
        })
    }

    #[test]
    fn a_code_is_eight_crockford_chars_and_unique() {
        let p = PendingPairings::new();
        let a = mint(&p, "phone", "full", false, Duration::from_secs(600));
        let b = mint(&p, "tablet", "full", false, Duration::from_secs(600));
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
        let req = mint(&p, "phone", "readonly", false, Duration::from_secs(600));
        let got = p.consume(&req.code).expect("first use");
        assert_eq!(got.name, "phone");
        assert_eq!(got.mode, "readonly");
        assert!(p.consume(&req.code).is_none(), "second use must fail");
    }

    #[test]
    fn an_expired_code_is_refused_and_swept() {
        let p = PendingPairings::new();
        let req = mint(&p, "phone", "full", false, Duration::from_millis(1));
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
        let live = mint(&p, "kept", "full", false, Duration::from_secs(600));
        mint(&p, "stale", "full", false, Duration::from_millis(1));
        std::thread::sleep(Duration::from_millis(5));
        mint(&p, "fresh", "full", false, Duration::from_secs(600));
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
        let req = mint(&p, "phone", "full", false, Duration::from_secs(600));
        let rendered = format!("{req:?}");
        assert!(
            !rendered.contains(&req.code),
            "the code must not appear in Debug output: {rendered}"
        );
        assert!(rendered.contains("phone"), "{rendered}");
        assert!(rendered.contains("<redacted>"), "{rendered}");
        // A person's name is not a secret and is deliberately printable: a
        // log line naming it is how the operator notices a typo made a
        // second colleague instead of pairing their own second device.
        assert!(rendered.contains("owner"), "{rendered}");
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
        // A routable IPv6 address is budgeted by its /64, forwarded or not:
        // one host holds the whole prefix. Mapped IPv4 is its IPv4 address.
        assert_eq!(
            limiter_key(ip("2001:db8:1:2:aaaa::1"), &hdrs("")),
            "2001:db8:1:2::/64"
        );
        assert_eq!(
            limiter_key(ip("127.0.0.1"), &hdrs("2001:db8:1:2:bbbb::9")),
            "2001:db8:1:2::/64"
        );
        assert_eq!(
            limiter_key(ip("::ffff:198.51.100.4"), &hdrs("")),
            "198.51.100.4"
        );
        assert_eq!(limiter_key(ip("::1"), &hdrs("")), "::1");
    }

    /// Some proxies append a token that is not an address — `unknown` is the
    /// classic, and a `host:port` or bracketed IPv6 form shows up too.
    /// Testing only the rightmost element would find nothing, fall back to
    /// the peer, and collapse every client behind that proxy into the proxy's
    /// own bucket. The scan walks left until something parses.
    #[test]
    fn an_ipv6_address_is_bucketed_by_its_64() {
        assert_eq!(
            address_bucket("2001:db8:1:2:aaaa::1"),
            address_bucket("2001:db8:1:2:ffff:ffff:ffff:ffff")
        );
        assert_eq!(address_bucket("2001:db8:1:2::9"), "2001:db8:1:2::/64");
        assert_ne!(
            address_bucket("2001:db8:1:2::1"),
            address_bucket("2001:db8:1:3::1")
        );
        assert_eq!(address_bucket("203.0.113.7"), "203.0.113.7");
        assert_eq!(address_bucket("::ffff:203.0.113.7"), "::ffff:203.0.113.7");
        assert_eq!(address_bucket(UNKNOWN_PEER), UNKNOWN_PEER);
    }

    #[test]
    fn a_non_address_last_hop_does_not_collapse_everyone_into_the_proxy() {
        let hdrs = |vals: &[&str]| {
            let mut h = axum::http::HeaderMap::new();
            for v in vals {
                h.append("x-forwarded-for", v.parse().unwrap());
            }
            h
        };
        let ip = |s: &str| Some(s.parse::<std::net::IpAddr>().unwrap());
        for chain in [
            "203.0.113.7, unknown",
            "203.0.113.7, [2001:db8::1]:443",
            "203.0.113.7, 10.0.0.1:8080",
            "203.0.113.7, unknown, _hidden",
        ] {
            assert_eq!(
                limiter_key(ip("127.0.0.1"), &hdrs(&[chain])),
                "203.0.113.7",
                "{chain:?}"
            );
        }
        // Several header instances are one chain, scanned right-to-left.
        assert_eq!(
            limiter_key(ip("127.0.0.1"), &hdrs(&["9.9.9.9", "203.0.113.7, unknown"])),
            "203.0.113.7"
        );
        // Still nothing parseable anywhere: the peer, never a chosen bucket.
        assert_eq!(
            limiter_key(ip("127.0.0.1"), &hdrs(&["unknown, _hidden"])),
            "127.0.0.1"
        );
    }

    /// The trust grant rides on the code, per mint.
    #[test]
    fn a_code_carries_its_trust_grant() {
        let p = PendingPairings::new();
        let vouched = mint(&p, "desk", "full", true, Duration::from_secs(600));
        let plain = mint(&p, "phone", "full", false, Duration::from_secs(600));
        assert!(vouched.trusted && !plain.trusted);
        assert!(p.consume(&vouched.code).unwrap().trusted);
        assert!(!p.consume(&plain.code).unwrap().trusted);
        // The Debug form names the grant, never the code.
        let dbg = format!("{vouched:?}");
        assert!(
            dbg.contains("trusted: true") && !dbg.contains(&vouched.code),
            "{dbg}"
        );
    }

    /// The person rides on the code exactly as the trust grant does
    /// (multi-user M1): the device is bound the moment it pairs, so there is
    /// no window in which it is live and belongs to nobody. The row itself
    /// is created at redemption — see [`MintRequest::person`] — so an
    /// abandoned code leaves nothing behind.
    #[test]
    fn a_code_carries_the_person_it_will_be_paired_for() {
        let p = PendingPairings::new();
        let colleague = p.mint(MintRequest {
            name: "ada-laptop",
            mode: "full",
            trusted: false,
            org_id: None,
            person: Some("ada"),
            ttl: Duration::from_secs(600),
        });
        // A hub link is nobody's device, and carries no person.
        let link = p.mint(MintRequest {
            name: "hub-b",
            mode: "peer",
            trusted: false,
            org_id: None,
            person: None,
            ttl: Duration::from_secs(600),
        });
        assert_eq!(colleague.person.as_deref(), Some("ada"));
        assert_eq!(link.person, None);
        assert_eq!(
            p.consume(&colleague.code).unwrap().person.as_deref(),
            Some("ada")
        );
        assert_eq!(p.consume(&link.code).unwrap().person, None);
    }

    /// Codes are minted per request, so two clients never share one.
    #[test]
    fn each_code_carries_its_own_name_and_mode() {
        let p = PendingPairings::new();
        let a = mint(&p, "phone", "full", false, Duration::from_secs(600));
        let b = mint(&p, "kiosk", "readonly", false, Duration::from_secs(600));
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
        // …and its person, which the helper above sets for both.
        assert_eq!(got_a.person.as_deref(), Some("owner"));
        assert_eq!(got_b.person.as_deref(), Some("owner"));
    }
}
