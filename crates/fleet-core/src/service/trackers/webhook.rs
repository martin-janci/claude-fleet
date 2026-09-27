//! Webhook nudges (work graph M13.4f, decisions D13 / D28).
//!
//! On a hub with a public URL, a tracker whose admin minted a secret
//! (`fleet-hub tracker webhook <id>`) may call `POST /hooks/tracker/<id>`
//! when one of its items changes. The delivery only **nudges**: fleet reads
//! one item reference out of it and refreshes that item from the tracker's
//! own API ([`super::sync::fetch_one`]), exactly as a poll would. Polling
//! stays the source of truth; a webhook only makes it sooner.
//!
//! * **Verified before it is read.** The signature is an HMAC-SHA256 of the
//!   raw body with the tracker's secret, in each provider's own header
//!   ([`verify`]): GitHub `X-Hub-Signature-256: sha256=<hex>`, Jira Cloud
//!   `X-Hub-Signature: sha256=<hex>`, Linear `Linear-Signature: <hex>` plus
//!   a `webhookTimestamp` within [`LINEAR_WINDOW_MS`]. Compared in constant
//!   time. A missing or wrong signature is 401 and counts in
//!   `fleet_health` as `webhook_rejected`.
//! * **The payload is never trusted.** Only an item key is taken from it
//!   ([`item_ref`]), and only an item fleet already has for *that* tracker
//!   is refreshed: a delivery cannot add an item, change one, or reach
//!   another tracker. New items still arrive by polling.
//! * **Bursts coalesce.** Deliveries for one item within [`COALESCE`]
//!   become one fetch, at the end of the window ([`schedule`]); at most
//!   [`PENDING_MAX`] items wait per tracker. The route (`mcp/tracker_hook.rs`)
//!   also rate-limits per tracker and per address and caps the body at
//!   [`BODY_MAX`].
//! * **A tracker that cannot be asked is not asked.** A delivery for a
//!   tracker whose sync is stopped (a refused credential) or rate-limited
//!   is accepted and dropped.

use super::{sync, ItemRef};
use crate::ipc_error::lock;
use crate::store::{Secret, Store};
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// The largest body a delivery may have.
pub const BODY_MAX: usize = 64 * 1024;

/// How far a Linear delivery's `webhookTimestamp` may be from now.
pub const LINEAR_WINDOW_MS: i64 = 60_000;

/// Deliveries for one item within this window make one fetch.
pub const COALESCE: Duration = Duration::from_secs(5);

/// Items waiting for their fetch, per tracker, at most.
pub const PENDING_MAX: usize = 50;

/// The providers that take a webhook (D28).
pub const WEBHOOK_PROVIDERS: &[&str] = &["jira", "github", "linear"];

/// PURE: `HMAC-SHA256(secret, body)`, lower-case hex.
pub fn hmac_sha256_hex(secret: &[u8], body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC takes a key of any length");
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

/// Why a delivery was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reject {
    /// No signature header, or not in the provider's format.
    Unsigned,
    /// The signature does not match the body.
    BadSignature,
    /// Linear: the timestamp is missing or outside the window (a replay).
    Stale,
}

/// PURE: check a delivery's signature for `provider`. `header` looks a
/// header up by its lower-case name. `now_ms` is unix milliseconds.
pub fn verify(
    provider: &str,
    header: &dyn Fn(&str) -> Option<String>,
    body: &[u8],
    secret: &Secret,
    now_ms: i64,
) -> Result<(), Reject> {
    let (name, prefix) = match provider {
        "github" => ("x-hub-signature-256", "sha256="),
        "jira" => ("x-hub-signature", "sha256="),
        "linear" => ("linear-signature", ""),
        _ => return Err(Reject::Unsigned),
    };
    let given = header(name).ok_or(Reject::Unsigned)?;
    let given = given.trim();
    let hex_sig = if prefix.is_empty() {
        given
    } else {
        given
            .get(..prefix.len())
            .filter(|p| p.eq_ignore_ascii_case(prefix))
            .map(|_| &given[prefix.len()..])
            .ok_or(Reject::Unsigned)?
    };
    let given = hex_sig.to_ascii_lowercase();
    let want = hmac_sha256_hex(secret.expose().as_bytes(), body);
    if !crate::mcp::auth::constant_time_eq(given.as_bytes(), want.as_bytes()) {
        return Err(Reject::BadSignature);
    }
    if provider == "linear" {
        // Signed, so the timestamp is Linear's own; it bounds a replay.
        let at = serde_json::from_slice::<Value>(body)
            .ok()
            .and_then(|v| v.get("webhookTimestamp").and_then(Value::as_i64))
            .ok_or(Reject::Stale)?;
        if (now_ms - at).abs() > LINEAR_WINDOW_MS {
            return Err(Reject::Stale);
        }
    }
    Ok(())
}

/// The one thing fleet reads out of a delivery: which item, as the fetch
/// reference and the key fleet stores it under. `None` for a delivery that
/// is not about an issue (a GitHub `ping`, a comment-only event, a project
/// event): accepted and dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookItem {
    pub reference: ItemRef,
    /// The keys the item may be stored under, most likely first.
    pub keys: Vec<String>,
}

/// PURE: the item a verified delivery is about. `event` is GitHub's
/// `X-GitHub-Event`. `ghes_host` is a GitHub Enterprise tracker's host,
/// whose keys carry it.
pub fn item_ref(
    provider: &str,
    event: Option<&str>,
    body: &Value,
    ghes_host: Option<&str>,
) -> Option<HookItem> {
    match provider {
        "jira" => {
            let key = body.pointer("/issue/key").and_then(Value::as_str)?.trim();
            super::jira_common::is_key(key).then(|| HookItem {
                reference: ItemRef::Key(key.to_string()),
                keys: vec![key.to_string()],
            })
        }
        "github" => {
            if event != Some("issues") {
                return None;
            }
            let repo = body
                .pointer("/repository/full_name")
                .and_then(Value::as_str)?
                .trim();
            let n = body.pointer("/issue/number").and_then(Value::as_u64)?;
            let (owner, name) = repo.split_once('/')?;
            let ok = |p: &str| {
                !p.is_empty()
                    && p.len() <= 100
                    && p.chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
            };
            if !ok(owner) || !ok(name) || n == 0 {
                return None;
            }
            let repo = repo.to_ascii_lowercase();
            let mut keys = vec![format!("{repo}#{n}")];
            if let Some(h) = ghes_host {
                keys.insert(0, format!("{}/{repo}#{n}", h.to_ascii_lowercase()));
            }
            Some(HookItem {
                reference: ItemRef::RepoNumber { repo, n },
                keys,
            })
        }
        "linear" => {
            if body.get("type").and_then(Value::as_str) != Some("Issue") {
                return None;
            }
            let key = body
                .pointer("/data/identifier")
                .and_then(Value::as_str)?
                .trim();
            let (team, n) = key.split_once('-')?;
            let valid = !team.is_empty()
                && team.len() <= 10
                && team.chars().all(|c| c.is_ascii_alphanumeric())
                && !n.is_empty()
                && n.len() <= 12
                && n.chars().all(|c| c.is_ascii_digit());
            let key = key.to_ascii_uppercase();
            valid.then(|| HookItem {
                reference: ItemRef::Key(key.clone()),
                keys: vec![key],
            })
        }
        _ => None,
    }
}

/// Per-tracker delivery counters (`fleet_health`), in memory like the
/// sync's metrics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HookMetrics {
    /// The last delivery that verified (unix seconds).
    pub last_delivery_at: Option<i64>,
    /// Deliveries refused for their signature since this process started.
    pub rejected: u64,
}

fn metrics() -> &'static Mutex<HashMap<i64, HookMetrics>> {
    static M: OnceLock<Mutex<HashMap<i64, HookMetrics>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Tracker `id`'s counters.
pub fn hook_metrics(id: i64) -> HookMetrics {
    metrics()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&id)
        .copied()
        .unwrap_or_default()
}

fn record(id: i64, f: impl FnOnce(&mut HookMetrics)) {
    let mut m = metrics().lock().unwrap_or_else(|e| e.into_inner());
    f(m.entry(id).or_default());
}

/// The items waiting for their fetch, per tracker. One per server: the
/// route's state owns it.
#[derive(Debug, Default)]
pub struct Coalescer {
    pending: Mutex<HashMap<i64, HashSet<String>>>,
}

impl Coalescer {
    pub fn new() -> Self {
        Self::default()
    }
}

/// What [`schedule`] did with an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheduled {
    /// A fetch will run at the end of the window.
    Fetch,
    /// A fetch for the same item is already waiting.
    Coalesced,
    /// Too many items wait for this tracker; polling will catch it.
    Full,
}

/// Queue a fetch of `item` of tracker `tracker_id` after `window`, unless
/// one is already waiting. The fetch re-reads the tracker and skips it when
/// its sync is stopped or rate-limited.
pub fn schedule(
    coalescer: &Arc<Coalescer>,
    store: Arc<Mutex<Store>>,
    tracker_id: i64,
    item: HookItem,
    window: Duration,
) -> Scheduled {
    let key = item.keys[0].clone();
    {
        let mut p = coalescer.pending.lock().unwrap_or_else(|e| e.into_inner());
        let set = p.entry(tracker_id).or_default();
        if set.contains(&key) {
            return Scheduled::Coalesced;
        }
        if set.len() >= PENDING_MAX {
            return Scheduled::Full;
        }
        set.insert(key.clone());
    }
    let coalescer = Arc::clone(coalescer);
    tokio::spawn(async move {
        tokio::time::sleep(window).await;
        if let Some(set) = coalescer
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(&tracker_id)
        {
            set.remove(&key);
        }
        let t = match lock(&store).and_then(|s| {
            let t = s.get_tracker(tracker_id)?;
            let not_before = s.tracker_not_before(tracker_id)?;
            Ok((t, not_before))
        }) {
            Ok((Some(t), nb))
                if sync::runnable(&t.state) && nb.is_none_or(|n| n <= crate::store::now_unix()) =>
            {
                t
            }
            _ => return,
        };
        let net = super::default_net();
        match sync::fetch_one(&t, item.reference, &store, &net).await {
            Ok(_) => tracing::debug!(tracker = tracker_id, key = %key, "[webhook] item refreshed"),
            Err(e) => {
                tracing::debug!(tracker = tracker_id, key = %key, error = %e.explain(), "[webhook] fetch failed")
            }
        }
    });
    Scheduled::Fetch
}

/// What a delivery got back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// No such tracker, not a webhook provider, no secret: the route is not
    /// there (404).
    NotHere,
    /// The signature failed (401).
    Rejected(Reject),
    /// A verified body that is not JSON (400).
    Malformed,
    /// Verified and understood; `Some` when a fetch was queued or merged.
    Accepted(Option<Scheduled>),
}

/// Handle one delivery for tracker `tracker_id`: the whole path after the
/// route's rate limit and body cap. `header` looks a header up by its
/// lower-case name.
pub fn deliver(
    coalescer: &Arc<Coalescer>,
    store: &Arc<Mutex<Store>>,
    tracker_id: i64,
    header: &dyn Fn(&str) -> Option<String>,
    body: &[u8],
    now_ms: i64,
    window: Duration,
) -> Outcome {
    let found = lock(store).and_then(|s| {
        let public = s
            .get_setting(crate::service::hub::SETTING_PUBLIC_URL)
            .ok()
            .flatten()
            .is_some_and(|u| !u.trim().is_empty());
        let t = s.get_tracker(tracker_id)?;
        let secret = s.resolve_tracker_webhook_secret(tracker_id)?;
        Ok((public, t, secret))
    });
    let (t, secret) = match found {
        Ok((true, Some(t), Some(secret))) if WEBHOOK_PROVIDERS.contains(&t.provider.as_str()) => {
            (t, secret)
        }
        _ => return Outcome::NotHere,
    };
    if let Err(r) = verify(&t.provider, header, body, &secret, now_ms) {
        record(tracker_id, |m| m.rejected += 1);
        return Outcome::Rejected(r);
    }
    record(tracker_id, |m| m.last_delivery_at = Some(now_ms / 1_000));
    let Ok(json) = serde_json::from_slice::<Value>(body) else {
        return Outcome::Malformed;
    };
    let ghes = t.settings.hostname.as_deref();
    let Some(item) = item_ref(
        &t.provider,
        header("x-github-event").as_deref(),
        &json,
        ghes,
    ) else {
        return Outcome::Accepted(None);
    };
    // Only an item fleet already has for this tracker.
    let known = lock(store).map(|s| {
        item.keys.iter().any(|k| {
            s.tracker_item_for_key_in(tracker_id, k)
                .ok()
                .flatten()
                .is_some()
        })
    });
    if !matches!(known, Ok(true)) {
        return Outcome::Accepted(None);
    }
    Outcome::Accepted(Some(schedule(
        coalescer,
        Arc::clone(store),
        tracker_id,
        item,
        window,
    )))
}

#[cfg(test)]
mod tests;
