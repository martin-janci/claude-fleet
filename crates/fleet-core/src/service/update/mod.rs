//! Application updates, the hub's half (update-channel design §6–§7, slice S4).
//!
//! The hub is the fleet's policy authority: it reads the signed release
//! channel (`refresh`), keeps what every target reports about itself
//! (`report`, and every `check`), and answers `/update/check` with the same
//! pure `fleet_update::decide` a standalone client runs, under the fleet's
//! `update.*` policy and the operator's pins. It never authors content: every
//! decision relays the signed documents it rests on, and the caller verifies
//! them itself (`fleet_update::verify::verify_target`).
//!
//! The cache (`update_docs`) is re-verified on every read, so a row written
//! by anything but `refresh` is simply not a channel.

mod fetch;
pub mod mirror;
pub mod rollout;
#[cfg(test)]
mod tests;

pub use fetch::HttpsFetch;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use fleet_update::channel::{attach_evidence, CheckOutcome, RawDoc};
use fleet_update::decide::{decide, DecideInput, HubSpeaks, Pin, Policy};
use fleet_update::verify::verify_channel;
use fleet_update::wire::{CheckRequest, Decision, Installed, Report, Speaks, Status, UPDATE_PROTO};
use fleet_update::{
    fetch_channel, fetch_manifests, verify_listed_manifest, wanted_versions, Component, Fetch,
    GitUpdateChannel, Manifests, MemorySequenceStore, Mode, Platform, SequenceStore, Source, Track,
    TrustedKeys, UpdateChannel, UpdateError, UpdatePhase, VerifiedChannel, Version, Window,
};
use serde::Serialize;

use crate::ipc_error::{codes, lock, IpcError};
use crate::mcp::auth::{Caller, TokenMode};
use crate::service::settings;
use crate::store::{
    Store, UpdateDesiredRow, UpdateDocRow, UpdateObservedRow, UpdateOrgPolicyRow, UpdateRolloutRow,
};

/// Where CI publishes the channel documents (design U10).
pub const CHANNEL_BASE_URL: &str =
    "https://raw.githubusercontent.com/martin-janci/claude-fleet/update-channels/";
/// Overrides [`CHANNEL_BASE_URL`] (a mirror, a test). Only *where* the
/// documents come from: what is trusted is still the signature.
pub const CHANNEL_URL_ENV: &str = "FLEET_UPDATE_CHANNEL_URL";
/// How many of a track's newest releases the hub keeps manifests for.
/// Twenty, not ten: on `nightly` most releases are per-push builds with no
/// desktop bundle, and the newest one a desktop can install must still be
/// among them (nightly.yml cuts at most one every two hours, S2b).
pub const MANIFESTS_KEPT: usize = 20;
/// Internal (not a registry setting): the last refresh's outcome.
const LAST_REFRESH_KEY: &str = "update.last_refresh";

/// The release keys this hub verifies with: the compiled-in ones, plus — in
/// an `e2e` test build only — `FLEET_UPDATE_E2E_KEYS` (comma-separated),
/// reserved for S4b's planned `scripts/hub-e2e.sh` section U, which will
/// publish a channel signed by a throwaway key (nothing sets it yet). A
/// release build never reads that variable.
pub fn trusted_keys() -> TrustedKeys {
    #[allow(unused_mut)]
    let mut keys: Vec<String> = fleet_update::keys::RELEASE_KEYS
        .iter()
        .map(|k| k.to_string())
        .collect();
    #[cfg(feature = "e2e")]
    if let Ok(extra) = std::env::var("FLEET_UPDATE_E2E_KEYS") {
        keys.extend(
            extra
                .split(',')
                .map(str::trim)
                .filter(|k| !k.is_empty())
                .map(String::from),
        );
    }
    TrustedKeys::from_base64(keys.iter().map(String::as_str)).unwrap_or_else(|e| {
        tracing::warn!(error = %e, "an update key does not decode; trusting the compiled-in keys only");
        fleet_update::keys::release_keys()
    })
}

pub fn channel_base_url() -> String {
    std::env::var(CHANNEL_URL_ENV)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| CHANNEL_BASE_URL.to_string())
}

// ── policy ──

/// The track the hub follows.
pub fn track(store: &Store) -> Track {
    match settings::get_string(store, settings::UPDATE_TRACK).as_str() {
        "beta" => Track::Beta,
        "nightly" => Track::Nightly,
        "dev" => Track::Dev,
        _ => Track::Stable,
    }
}

pub fn mode(store: &Store, c: Component) -> Mode {
    let key = match c {
        Component::Hub => settings::UPDATE_HUB_MODE,
        Component::Agent => settings::UPDATE_AGENT_MODE,
        Component::Desktop => settings::UPDATE_DESKTOP_MODE,
        Component::Android | Component::Ios => settings::UPDATE_MOBILE_MODE,
    };
    parse_mode(&settings::get_string(store, key))
}

pub fn check_interval_secs(store: &Store) -> u64 {
    // Resolved against its spec, so never below the minimum and never 0.
    settings::get_secs(store, settings::UPDATE_CHECK_INTERVAL_SECS)
}

/// Wakes the refresh tick early: a new track has no cached channel (every
/// target would read `unknown` until the next tick), and a new interval
/// should not wait out the old one's sleep.
static REFRESH_WAKE: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// `settings::set` calls this for every `update.*` key it stores.
pub fn settings_changed(key: &str) {
    if key == settings::UPDATE_TRACK || key == settings::UPDATE_CHECK_INTERVAL_SECS {
        REFRESH_WAKE.notify_one();
    }
    // A mode, a floor or a track moves decisions without a new channel.
    decisions_may_have_changed();
}

/// The decision pusher's beat without a wake: rollout waves and the
/// maintenance window move decisions by the clock alone.
const DECIDE_BEAT: std::time::Duration = std::time::Duration::from_secs(300);

/// Wakes the decision pusher ([`push_decisions`]) in `fleet-hub serve`.
static DECIDE_WAKE: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// A pin, a policy setting or a refreshed channel: what some target would be
/// told may have changed.
pub fn decisions_may_have_changed() {
    DECIDE_WAKE.notify_one();
}

/// The status and version `/events` last carried per target.
static PUSHED: Mutex<BTreeMap<String, (String, Option<String>)>> = Mutex::new(BTreeMap::new());

/// `update:decision` (design §6.4) for every target whose decision differs
/// from the last one this hub computed for it, so a client checks again now
/// rather than on its next interval. A target seen for the first time is
/// recorded, not pushed: it has only just been told by its own check.
/// Returns the targets pushed.
pub fn push_decisions(store: &Mutex<Store>, keys: &TrustedKeys, now: i64) -> Vec<String> {
    let rows = match lock(store).and_then(|s| s.update_observed_all()) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let mut pushed = Vec::new();
    for o in rows {
        let Ok(d) = check_for(store, &Caller::master(), &o.target, keys, now) else {
            continue;
        };
        let status = serde_json::to_value(d.status)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        let version = d.target.as_ref().map(|t| t.version.to_string());
        let before = PUSHED
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(o.target.clone(), (status.clone(), version.clone()));
        if before.is_some_and(|b| b != (status.clone(), version.clone())) {
            if let Ok(s) = lock(store) {
                s.emit_update_decision(&o.target, &status, version.as_deref());
            }
            pushed.push(o.target);
        }
    }
    pushed
}

/// The org a target belongs to: a paired client's binding, an agent host's
/// `hosts.org_id`. The hub itself and an unbound client have none.
pub fn target_org(store: &Store, target: &str) -> Result<Option<i64>, IpcError> {
    if let Some(id) = target
        .strip_prefix("client:")
        .and_then(|s| s.parse::<i64>().ok())
    {
        return Ok(store.client_token_binding(id)?.and_then(|b| b.org_id));
    }
    if let Some(alias) = target.strip_prefix("agent:") {
        return store.host_org(alias);
    }
    Ok(None)
}

fn parse_mode(s: &str) -> Mode {
    match s {
        "manual" => Mode::Manual,
        "automatic" => Mode::Automatic,
        _ => Mode::Notify,
    }
}

/// The policy for one target: the fleet's `update.*` settings for its
/// component, overridden by its org's row (S9) where it has one; the pin
/// is the target's own, else its org's, else the component's.
fn policy(store: &Store, c: Component, target: &str, now: i64) -> Result<Policy, IpcError> {
    let as_pin = |version: &str, mandatory: bool| {
        Version::parse(version)
            .ok()
            .map(|version| Pin { version, mandatory })
    };
    let desired = store.update_desired_for(c.as_str(), target)?;
    let own = desired
        .as_ref()
        .filter(|d| !d.target.is_empty())
        .and_then(|d| as_pin(&d.version, d.mandatory));
    let fleet_pin = desired
        .as_ref()
        .filter(|d| d.target.is_empty())
        .and_then(|d| as_pin(&d.version, d.mandatory));
    let org = match target_org(store, target)? {
        Some(id) => store.update_org_policy(id, c.as_str())?,
        None => None,
    };
    let org_pin = org.as_ref().and_then(|o| {
        o.pin_version
            .as_deref()
            .and_then(|v| as_pin(v, o.pin_mandatory))
    });
    let mode = org
        .as_ref()
        .and_then(|o| o.mode.as_deref())
        .map(parse_mode)
        .unwrap_or_else(|| mode(store, c));
    let outside_window = match org.as_ref().and_then(|o| o.window.as_deref()) {
        Some(w) => rollout::outside(w, mode, now),
        None => rollout::outside_window(store, mode, now),
    };
    Ok(Policy {
        mode,
        minimum: org
            .as_ref()
            .and_then(|o| o.minimum.as_deref())
            .and_then(|v| Version::parse(v).ok()),
        pin: own.or(org_pin).or(fleet_pin),
        outside_window,
        check_interval_secs: check_interval_secs(store),
    })
}

/// What this running hub serves: every target's compatibility is judged
/// against it (decide rule 2).
pub fn hub_speaks() -> HubSpeaks {
    HubSpeaks {
        contract_serves: crate::wire_contract::CONTRACT_REVISION,
        agent_proto_accepts: Window::new(
            fleet_proto::MIN_SUPPORTED_PROTO,
            fleet_proto::PROTO_VERSION,
        ),
    }
}

// ── who is asking ──

/// The target a credential answers for (design §6.1). Derived from the
/// token, never from the request body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub target: String,
    pub allowed: &'static [Component],
    /// Only a target reports itself; the master only looks.
    pub may_report: bool,
}

const CLIENT_COMPONENTS: &[Component] = &[Component::Desktop, Component::Android, Component::Ios];
const ALL_COMPONENTS: &[Component] = &[
    Component::Hub,
    Component::Agent,
    Component::Desktop,
    Component::Android,
    Component::Ios,
];

pub fn identity(caller: &Caller) -> Result<Identity, IpcError> {
    if caller.mode == TokenMode::Peer {
        return Err(IpcError::new(
            codes::E_FORBIDDEN,
            "a hub link may call peer_exchange only",
        ));
    }
    if caller.mode == TokenMode::Updater {
        return Ok(Identity {
            target: "hub:self".into(),
            allowed: &[Component::Hub],
            may_report: true,
        });
    }
    if let Some(host) = &caller.host_alias {
        return Ok(Identity {
            target: format!("agent:{host}"),
            allowed: &[Component::Agent],
            may_report: true,
        });
    }
    if let Some(c) = &caller.client {
        return Ok(Identity {
            target: format!("client:{}", c.id),
            allowed: CLIENT_COMPONENTS,
            may_report: true,
        });
    }
    Ok(Identity {
        target: "operator".into(),
        allowed: ALL_COMPONENTS,
        may_report: false,
    })
}

fn require_component(id: &Identity, c: Component) -> Result<(), IpcError> {
    if id.allowed.contains(&c) {
        Ok(())
    } else {
        Err(IpcError::new(
            codes::E_FORBIDDEN,
            format!(
                "{} cannot ask or report for component {}",
                id.target,
                c.as_str()
            ),
        ))
    }
}

// ── the cached, verified channel ──

/// A track's channel and manifests as the hub last fetched them, verified
/// again now.
#[derive(Debug, Clone)]
pub struct Cached {
    pub channel: VerifiedChannel,
    pub raw: RawDoc,
    pub manifests: Manifests,
    pub fetched_at: i64,
}

/// `None` when nothing verifies: an empty cache, or rows no trusted key
/// signed. Freshness is judged at `now`.
pub fn load_cached(store: &Store, track: Track, keys: &TrustedKeys, now: i64) -> Option<Cached> {
    let row = store.update_doc("channel", track.as_str()).ok().flatten()?;
    let channel = verify_channel(row.body.as_bytes(), &row.sig, keys, track, 0, now).ok()?;
    let mut manifests = Manifests::default();
    for m in store.update_docs("manifest").ok().unwrap_or_default() {
        let Ok(v) = Version::parse(&m.key) else {
            continue;
        };
        let Some(listed) = channel.doc.release(&v) else {
            continue;
        };
        let raw = RawDoc {
            body: m.body,
            sig: m.sig,
        };
        if let Some(mut verified) = verify_listed_manifest(&raw, listed, keys) {
            // Its amendments (the phone's APK, design §4), verified again on
            // every read like everything else in the cache.
            for a in &listed.amendments {
                let key = amendment_key(&v, &a.component);
                if let Some(row) = store.update_doc("amendment", &key).ok().flatten() {
                    if let Some(am) = fleet_update::verify::verify_amendment(
                        row.body.as_bytes(),
                        &row.sig,
                        keys,
                        a,
                        &v,
                    ) {
                        verified.amend(&am);
                    }
                }
            }
            manifests.verified.insert(v.clone(), verified);
            manifests.raw.insert(v, raw);
        }
    }
    Some(Cached {
        raw: RawDoc {
            body: row.body,
            sig: row.sig,
        },
        channel,
        manifests,
        fetched_at: row.fetched_at,
    })
}

/// The `update_docs` key of one release's amendment for one component.
fn amendment_key(v: &Version, component: &str) -> String {
    format!("{v}/{component}")
}

/// One decision, as the hub makes it for `target`.
#[allow(clippy::too_many_arguments)]
fn decide_for(
    store: &Store,
    cached: Option<&Cached>,
    component: Component,
    target: &str,
    platform: &Platform,
    installed: &Version,
    speaks: &Speaks,
    now: i64,
) -> Result<Decision, IpcError> {
    let policy = policy(store, component, target, now)?;
    let rollout = rollout::for_decide(store, component)?;
    let empty = BTreeMap::new();
    let mut d = decide(&DecideInput {
        component,
        platform,
        installed,
        speaks,
        hub: Some(hub_speaks()),
        channel: cached.map(|c| &c.channel),
        manifests: cached.map(|c| &c.manifests.verified).unwrap_or(&empty),
        policy: &policy,
        rollout: rollout.as_ref(),
        target_id: target,
        source: Source::Hub,
        track: track(store),
        now,
    });
    if let Some(c) = cached {
        attach_evidence(&mut d, &c.raw, &c.manifests.raw);
    }
    if mirror::enabled(store) {
        if let Some(t) = d.target.as_mut() {
            t.mirror = t.artifact.content().map(|(sha, _)| mirror::path_for(sha));
        }
    }
    // An operator's pin the target has not reached yet (`update_now`
    // above all): ask again within minutes, not hours.
    if d.reason.code == fleet_update::ReasonCode::Pinned && d.target.is_some() {
        d.next_check_secs = d.next_check_secs.min(PINNED_RECHECK_SECS);
    }
    Ok(d)
}

fn check_proto(update_proto: u32) -> Result<(), IpcError> {
    if update_proto == 0 || update_proto > UPDATE_PROTO {
        return Err(IpcError::new(
            codes::E_UNSUPPORTED,
            format!("update_proto {update_proto}; this hub speaks {UPDATE_PROTO}"),
        ));
    }
    Ok(())
}

fn json(v: &impl Serialize) -> Option<String> {
    serde_json::to_string(v).ok()
}

/// `POST /update/check`: record what the caller says it runs, then decide.
pub fn check(
    store: &Mutex<Store>,
    caller: &Caller,
    req: &CheckRequest,
    keys: &TrustedKeys,
    now: i64,
) -> Result<Decision, IpcError> {
    // The token before the body: a hub link is refused whatever it sends.
    let id = identity(caller)?;
    check_proto(req.update_proto)?;
    require_component(&id, req.component)?;
    let s = lock(store)?;
    if id.may_report {
        let prev = s.update_observed(&id.target)?;
        s.upsert_update_observed(&UpdateObservedRow {
            target: id.target.clone(),
            component: req.component.as_str().into(),
            platform: json(&req.platform),
            version: req.installed.version.to_string(),
            commit_sha: req.installed.commit.clone(),
            build_id: req.installed.build_id.clone(),
            digest: req.installed.digest.clone(),
            speaks: json(&req.speaks),
            phase: req.phase.as_str().into(),
            attempt: req.attempt.clone(),
            last_error: prev.and_then(|p| p.last_error),
            reported_at: now,
            last_checked_at: Some(now),
        })?;
    }
    let cached = load_cached(&s, track(&s), keys, now);
    decide_for(
        &s,
        cached.as_ref(),
        req.component,
        &id.target,
        &req.platform,
        &req.installed.version,
        &req.speaks,
        now,
    )
}

/// Record what a paired client's `X-Fleet-Client` header says it runs
/// (design §6.3), so the dashboard knows its version without it calling
/// `/update`. Only a client token, only a client component; the phase, the
/// last check and the last error are the target's own reports and stay.
/// `authorize` calls it at most once a minute per client.
pub fn record_client_header(
    s: &Store,
    caller: &Caller,
    h: &fleet_update::client_header::ClientHeader,
    now: i64,
) -> Result<(), IpcError> {
    if caller.client.is_none() || caller.mode == TokenMode::Updater {
        return Ok(());
    }
    let id = identity(caller)?;
    if !id.allowed.contains(&h.component) {
        return Ok(());
    }
    let prev = s.update_observed(&id.target)?;
    // The header carries os and arch only: keep a reported variant while
    // they match.
    let platform = h.platform.as_ref().map(|p| {
        let variant = prev
            .as_ref()
            .and_then(|o| o.platform.as_deref())
            .and_then(|j| serde_json::from_str::<Platform>(j).ok())
            .filter(|old| old.os == p.os && old.arch == p.arch)
            .map(|old| old.variant)
            .unwrap_or_default();
        Platform::new(&p.os, &p.arch, &variant)
    });
    let mut speaks: Speaks = prev
        .as_ref()
        .and_then(|o| o.speaks.as_deref())
        .and_then(|j| serde_json::from_str(j).ok())
        .unwrap_or_default();
    if h.contract_accepts.is_some() {
        speaks.contract_accepts = h.contract_accepts;
    }
    let same_build = prev
        .as_ref()
        .is_some_and(|o| o.version == h.version.to_string());
    s.upsert_update_observed(&UpdateObservedRow {
        target: id.target,
        component: h.component.as_str().into(),
        platform: platform
            .as_ref()
            .and_then(json)
            .or_else(|| prev.as_ref().and_then(|o| o.platform.clone())),
        version: h.version.to_string(),
        commit_sha: h.build.clone().or_else(|| {
            prev.as_ref()
                .filter(|_| same_build)
                .and_then(|o| o.commit_sha.clone())
        }),
        build_id: prev
            .as_ref()
            .filter(|_| same_build)
            .and_then(|o| o.build_id.clone()),
        digest: None,
        speaks: json(&speaks),
        phase: prev
            .as_ref()
            .map(|o| o.phase.clone())
            .unwrap_or_else(|| "idle".into()),
        attempt: prev.as_ref().and_then(|o| o.attempt.clone()),
        last_error: prev.as_ref().and_then(|o| o.last_error.clone()),
        reported_at: now,
        last_checked_at: None,
    })
}

/// `POST /update/report`: one state-machine transition and the observed
/// state that comes with it. Idempotent on `(target, attempt, phase)` for a
/// report that carries an attempt: a replayed report adds nothing and returns
/// `false`, and a late report of an older attempt is logged but never
/// overwrites the newer attempt's observed state. A report without an attempt
/// always becomes the observed state; the return says whether its event was
/// newly recorded.
pub fn report(
    store: &Mutex<Store>,
    caller: &Caller,
    r: &Report,
    now: i64,
) -> Result<bool, IpcError> {
    // The token before the body: a hub link is refused whatever it sends.
    let id = identity(caller)?;
    check_proto(r.update_proto)?;
    if !id.may_report {
        return Err(IpcError::new(
            codes::E_FORBIDDEN,
            "the master token looks; only a target reports itself",
        ));
    }
    require_component(&id, r.component)?;
    let phase = r.phase.as_str();
    let detail = (!r.detail.is_null()).then(|| r.detail.to_string());
    let s = lock(store)?;
    s.atomically(|s| {
        let recorded = s.insert_update_event(
            &id.target,
            r.attempt.as_deref(),
            phase,
            r.from.as_ref().map(|v| v.to_string()).as_deref(),
            r.to.as_ref().map(|v| v.to_string()).as_deref(),
            detail.as_deref(),
            r.error.as_deref(),
            now,
        )?;
        // Only a report that carries an attempt is de-duplicated and
        // ordered: an attempt is made at `downloading`, so an attempt-less
        // report (checking / available / idle) is always the latest state.
        let this_attempt = r.attempt.as_deref().unwrap_or("");
        if !recorded && !this_attempt.is_empty() {
            return Ok(false);
        }
        let prev = s.update_observed(&id.target)?;
        if let Some(p) = &prev {
            let prev_attempt = p.attempt.as_deref().unwrap_or("");
            if prev_attempt != this_attempt && !prev_attempt.is_empty() && !this_attempt.is_empty()
            {
                // Attempts are ordered by when the hub first heard of them:
                // a late report of an older attempt is history, not state.
                let first = |a: &str| s.update_attempt_first_seen(&id.target, a);
                if let (Some(this), Some(newer)) = (first(this_attempt)?, first(prev_attempt)?) {
                    if this < newer {
                        return Ok(true);
                    }
                }
            } else if !this_attempt.is_empty() && prev_attempt == this_attempt {
                // Within one attempt a delayed report of an earlier phase
                // (a `downloading` retried after `installed`) is history too
                // (r18-U6).
                if let (Some(this), Some(seen)) = (attempt_rank(phase), attempt_rank(&p.phase)) {
                    if this < seen {
                        return Ok(true);
                    }
                }
            }
        }
        s.upsert_update_observed(&UpdateObservedRow {
            target: id.target.clone(),
            component: r.component.as_str().into(),
            platform: prev.as_ref().and_then(|p| p.platform.clone()),
            version: r.installed.version.to_string(),
            commit_sha: r.installed.commit.clone(),
            build_id: r.installed.build_id.clone(),
            digest: r.installed.digest.clone(),
            speaks: prev.as_ref().and_then(|p| p.speaks.clone()),
            phase: phase.into(),
            attempt: r.attempt.clone(),
            last_error: r.error.clone(),
            reported_at: now,
            last_checked_at: None,
        })?;
        Ok(recorded)
    })
}

/// Where `phase` falls in one attempt's run, for ordering its reports;
/// `None` for a phase outside an attempt (idle, checking, available).
fn attempt_rank(phase: &str) -> Option<u8> {
    Some(match phase {
        "downloading" => 1,
        "verifying" => 2,
        "ready" => 3,
        "installing" => 4,
        "validating" => 5,
        "success" | "failed" => 6,
        "rolling_back" => 7,
        "recovered" | "rollback_failed" => 8,
        _ => return None,
    })
}

// ── status ──

#[derive(Debug, Clone, Serialize)]
pub struct ChannelStatus {
    pub track: Track,
    pub sequence: u64,
    pub fresh: bool,
    pub current: String,
    pub recommended: String,
    pub fetched_at: i64,
    pub manifests: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct TargetStatus {
    pub target: String,
    pub component: String,
    pub version: String,
    pub phase: String,
    pub reported_at: i64,
    pub last_checked_at: Option<i64>,
    pub last_error: Option<String>,
    /// What the hub would tell it now.
    pub status: Status,
    pub target_version: Option<String>,
    pub mandatory: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ComponentSummary {
    pub component: String,
    pub total: usize,
    pub up_to_date: usize,
    pub available: usize,
    pub required: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateStatus {
    pub track: Track,
    /// `None`: no channel that verifies yet (no release key, never fetched,
    /// or a fetch that failed).
    pub channel: Option<ChannelStatus>,
    pub last_refresh: Option<serde_json::Value>,
    pub components: Vec<ComponentSummary>,
    pub targets: Vec<TargetStatus>,
    pub pins: Vec<UpdateDesiredRow>,
    /// Active rollouts, then the five that ended last (S9); empty for a
    /// scoped caller.
    #[serde(default)]
    pub rollouts: Vec<RolloutStatus>,
    /// Per-org overrides of the fleet's policy (S9); empty for a scoped caller.
    #[serde(default)]
    pub policies: Vec<UpdateOrgPolicyRow>,
}

/// One rollout and its open wave's tally so far.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RolloutStatus {
    #[serde(flatten)]
    pub rollout: UpdateRolloutRow,
    pub tally: rollout::WaveTally,
}

/// Every observed target (or only `own`), with what the hub would tell it
/// now.
fn target_rows(
    s: &Store,
    cached: Option<&Cached>,
    own: Option<&str>,
    now: i64,
) -> Result<Vec<TargetStatus>, IpcError> {
    let mut targets = Vec::new();
    for o in s.update_observed_all()? {
        if own.is_some_and(|t| t != o.target) {
            continue;
        }
        let Ok(component) = o.component.parse::<Component>() else {
            continue;
        };
        let platform: Platform = o
            .platform
            .as_deref()
            .and_then(|p| serde_json::from_str(p).ok())
            .unwrap_or_else(|| Platform::new("", "", ""));
        let speaks: Speaks = o
            .speaks
            .as_deref()
            .and_then(|p| serde_json::from_str(p).ok())
            .unwrap_or_default();
        let Ok(installed) = Version::parse(&o.version) else {
            continue;
        };
        let d = decide_for(
            s, cached, component, &o.target, &platform, &installed, &speaks, now,
        )?;
        targets.push(TargetStatus {
            target: o.target,
            component: o.component,
            version: o.version,
            phase: o.phase,
            reported_at: o.reported_at,
            last_checked_at: o.last_checked_at,
            last_error: o.last_error,
            status: d.status,
            target_version: d.target.as_ref().map(|t| t.version.to_string()),
            mandatory: d.target.as_ref().is_some_and(|t| t.mandatory),
            reason: d.reason.text,
        });
    }
    Ok(targets)
}

/// The fleet's update picture (`update_status`). A caller behind an org
/// boundary (a per-host token, an org-bound client) sees its own row only and
/// no pins; the master and an unbound client see every target.
pub fn status(
    store: &Mutex<Store>,
    caller: &Caller,
    keys: &TrustedKeys,
    now: i64,
) -> Result<UpdateStatus, IpcError> {
    // This is the org boundary, not a privacy fence: a caller behind an org boundary sees its
    // own update target only, as the doc above says.
    //
    // **The narrower question is an OWNER DECISION, and it is the page the
    // `check_for` and `health` guards below follow** (multi-user M1, T9e).
    // `update_status` is `Access::Client`, and an UNBOUND paired client — a
    // person's own phone — has `is_scoped() == false`, so `own` is `None`,
    // `target_rows` skips nothing, and that caller is handed every observed
    // target: `client:<id>` / `agent:<host>` / `hub:self`, each with its
    // `version`, `platform`, `phase` and `last_error`. That is an inventory
    // of every other person's DEVICES. It is the same question
    // `work::view::task` carries for one `Placement.updated_by` string, in
    // the strictly larger case, and the eight rules do not cover device
    // identity — so it is recorded as open here and in `OPEN_QUESTIONS`
    // rather than settled by silence. The fence if the answer is no is this
    // same line with a person predicate instead of `is_scoped()`.
    let own = if caller.is_scoped() {
        Some(identity(caller)?.target)
    } else {
        None
    };
    let s = lock(store)?;
    let track = track(&s);
    let cached = load_cached(&s, track, keys, now);
    let targets = target_rows(&s, cached.as_ref(), own.as_deref(), now)?;
    let mut components: BTreeMap<String, ComponentSummary> = BTreeMap::new();
    for t in &targets {
        let e = components
            .entry(t.component.clone())
            .or_insert_with(|| ComponentSummary {
                component: t.component.clone(),
                total: 0,
                up_to_date: 0,
                available: 0,
                required: 0,
                failed: 0,
            });
        e.total += 1;
        match t.status {
            Status::UpToDate => e.up_to_date += 1,
            Status::UpdateAvailable | Status::Rollback => e.available += 1,
            Status::UpdateRequired | Status::ClientTooNew => e.required += 1,
            Status::Hold | Status::Unknown => {}
        }
        if matches!(t.phase.as_str(), "failed" | "rollback_failed") {
            e.failed += 1;
        }
    }
    let last_refresh = s
        .get_setting(LAST_REFRESH_KEY)?
        .and_then(|v| serde_json::from_str(&v).ok());
    Ok(UpdateStatus {
        track,
        channel: cached.as_ref().map(|c| ChannelStatus {
            track,
            sequence: c.channel.doc.sequence,
            fresh: c.channel.fresh,
            current: c.channel.doc.current.to_string(),
            recommended: c.channel.doc.recommended.to_string(),
            fetched_at: c.fetched_at,
            manifests: c.manifests.verified.len(),
        }),
        last_refresh,
        components: components.into_values().collect(),
        targets,
        pins: if own.is_some() {
            Vec::new()
        } else {
            s.update_desired_all()?
        },
        rollouts: if own.is_some() {
            Vec::new()
        } else {
            s.update_rollouts(5)?
                .into_iter()
                .map(|r| {
                    let tally = rollout::tally(&s, &r)?;
                    Ok(RolloutStatus { rollout: r, tally })
                })
                .collect::<Result<_, IpcError>>()?
        },
        policies: if own.is_some() {
            Vec::new()
        } else {
            s.update_org_policies()?
        },
    })
}

/// What the hub would tell one `target` now, and why (`update_status {
/// target }`, the dashboard's "why"): the full decision for its last
/// reported build, without the relayed documents. A scoped caller may ask
/// about itself only.
pub fn check_for(
    store: &Mutex<Store>,
    caller: &Caller,
    target: &str,
    keys: &TrustedKeys,
    now: i64,
) -> Result<Decision, IpcError> {
    // This is the org boundary, not a privacy fence: the same rule, asked of one named target.
    if caller.is_scoped() && identity(caller)?.target != target {
        return Err(IpcError::new(
            codes::E_FORBIDDEN,
            "a scoped token may ask about its own target only",
        ));
    }
    let s = lock(store)?;
    let o = s.update_observed(target)?.ok_or_else(|| {
        IpcError::new(
            codes::E_INVALID,
            format!("{target:?} has not reported to this hub (see update_status)"),
        )
    })?;
    let component =
        serde_json::from_value::<Component>(serde_json::Value::String(o.component.clone()))
            .map_err(|_| {
                IpcError::new(
                    codes::E_INVALID,
                    format!("unknown component {:?}", o.component),
                )
            })?;
    let platform: Platform = o
        .platform
        .as_deref()
        .and_then(|p| serde_json::from_str(p).ok())
        .unwrap_or_else(|| Platform::new("", "", ""));
    let speaks: Speaks = o
        .speaks
        .as_deref()
        .and_then(|p| serde_json::from_str(p).ok())
        .unwrap_or_default();
    let installed = Version::parse(&o.version).map_err(|e| {
        IpcError::new(
            codes::E_INVALID,
            format!("reported version {:?}: {e}", o.version),
        )
    })?;
    let cached = load_cached(&s, track(&s), keys, now);
    let mut d = decide_for(
        &s,
        cached.as_ref(),
        component,
        target,
        &platform,
        &installed,
        &speaks,
        now,
    )?;
    if let Some(t) = d.target.as_mut() {
        t.evidence = None;
    }
    Ok(d)
}

// ── fleet_health ──

/// `fleet_health.updates[].reason`: a target the hub would answer
/// `update_required` (blocked until it updates).
pub const ATTENTION_UPDATE_REQUIRED: &str = "update_required";
/// An install that failed (phase `failed`).
pub const ATTENTION_UPDATE_FAILED: &str = "update_failed";
/// An install that was rolled back to the previous build (phase `recovered`).
pub const ATTENTION_UPDATE_ROLLED_BACK: &str = "update_rolled_back";
/// A rollback that did not come back either: terminal until an operator
/// clears it (phase `rollback_failed`).
pub const ATTENTION_ROLLBACK_FAILED: &str = "rollback_failed";
/// The verified channel is past its signed `expires_at`: nothing new is
/// offered until the publisher re-signs or the hub can fetch it again.
pub const ATTENTION_CHANNEL_STALE: &str = "channel_stale";
/// A rollout paused itself (its wave's failure ratio reached the halt ratio)
/// or an operator paused it: the rest of the fleet waits (S9).
pub const ATTENTION_ROLLOUT_PAUSED: &str = "rollout_paused";

/// One thing about the fleet's updates a person should look at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct UpdateAttention {
    /// One of the `ATTENTION_*` codes.
    pub reason: String,
    /// `client:3`, `agent:<host>`, `hub:self`; `channel:<track>` for
    /// `channel_stale`.
    pub target: String,
    #[serde(default)]
    pub component: String,
    /// The target's reported version (empty for the channel).
    #[serde(default)]
    pub version: String,
    /// The hub's reason text or the target's last error.
    #[serde(default)]
    pub detail: Option<String>,
}

/// `fleet_health.updates` (update design §9): the channel's state and what
/// needs a person.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct UpdatesHealth {
    /// `fresh`, `stale`, or `none` (no channel verifies yet).
    #[serde(default)]
    pub channel: String,
    #[serde(default)]
    pub attention: Vec<UpdateAttention>,
}

/// The update roll-up for `fleet_health`. A scoped caller (a per-host token,
/// an org-bound client) sees its own target only, as in `update_status`.
pub fn health(
    s: &Store,
    caller: &Caller,
    keys: &TrustedKeys,
    now: i64,
) -> Result<UpdatesHealth, IpcError> {
    // This is the org boundary, not a privacy fence: the same rule for the `fleet_health` roll-
    // up.
    let own = if caller.is_scoped() {
        Some(identity(caller)?.target)
    } else {
        None
    };
    let track = track(s);
    let cached = load_cached(s, track, keys, now);
    let mut attention = Vec::new();
    let channel = match &cached {
        None => "none",
        Some(c) if c.channel.fresh => "fresh",
        Some(_) => {
            attention.push(UpdateAttention {
                reason: ATTENTION_CHANNEL_STALE.into(),
                target: format!("channel:{}", track.as_str()),
                component: String::new(),
                version: String::new(),
                detail: None,
            });
            "stale"
        }
    };
    if own.is_none() {
        for r in s.update_rollouts(0)? {
            if let Some(why) = r.paused_reason.clone().filter(|_| r.paused_at.is_some()) {
                attention.push(UpdateAttention {
                    reason: ATTENTION_ROLLOUT_PAUSED.into(),
                    target: format!("rollout:{}", r.id),
                    component: r.component,
                    version: r.version,
                    detail: Some(why),
                });
            }
        }
    }
    for t in target_rows(s, cached.as_ref(), own.as_deref(), now)? {
        let (reason, detail) = match t.phase.as_str() {
            "rollback_failed" => (ATTENTION_ROLLBACK_FAILED, t.last_error.clone()),
            "failed" => (ATTENTION_UPDATE_FAILED, t.last_error.clone()),
            "recovered" => (ATTENTION_UPDATE_ROLLED_BACK, t.last_error.clone()),
            _ if t.status == Status::UpdateRequired => {
                (ATTENTION_UPDATE_REQUIRED, Some(t.reason.clone()))
            }
            _ => continue,
        };
        attention.push(UpdateAttention {
            reason: reason.into(),
            target: t.target,
            component: t.component,
            version: t.version,
            detail,
        });
    }
    Ok(UpdatesHealth {
        channel: channel.into(),
        attention,
    })
}

// ── admin ──

fn parse_component(s: &str) -> Result<Component, IpcError> {
    s.parse().map_err(|_| {
        IpcError::new(
            codes::E_INVALID,
            format!("component must be hub | agent | desktop | android | ios, got {s:?}"),
        )
    })
}

/// What a pin's target names: `Any` is every target of the component, or
/// `hub:self`.
enum TargetRef<'a> {
    Any,
    Agent(&'a str),
    Client(i64),
}

fn validate_target(component: Component, target: &str) -> Result<TargetRef<'_>, IpcError> {
    let parsed = if target.is_empty() {
        Some(TargetRef::Any)
    } else {
        match component {
            Component::Hub => (target == "hub:self").then_some(TargetRef::Any),
            Component::Agent => target
                .strip_prefix("agent:")
                .filter(|a| !a.is_empty())
                .map(TargetRef::Agent),
            Component::Desktop | Component::Android | Component::Ios => target
                .strip_prefix("client:")
                .and_then(|id| id.parse::<i64>().ok())
                .map(TargetRef::Client),
        }
    };
    parsed.ok_or_else(|| {
        IpcError::new(
            codes::E_INVALID,
            format!("target {target:?} is not a {} target", component.as_str()),
        )
    })
}

/// A pin names a target that exists: an unrevoked client, a known host.
/// (Unpinning stays lenient, so a pin on a target since removed can go.)
fn require_target_exists(store: &Store, t: &TargetRef, target: &str) -> Result<(), IpcError> {
    let exists = match t {
        TargetRef::Any => true,
        TargetRef::Agent(alias) => store.get_host_row(alias)?.is_some(),
        TargetRef::Client(id) => store.client_token_is_live(*id)?,
    };
    if exists {
        Ok(())
    } else {
        Err(IpcError::new(
            codes::E_NOTFOUND,
            format!("no such target {target:?}"),
        ))
    }
}

/// Pin `component` (every target, or `target`) to `version`.
pub fn pin(
    store: &Mutex<Store>,
    component: &str,
    target: &str,
    version: &str,
    mandatory: bool,
    reason: Option<String>,
    now: i64,
) -> Result<UpdateDesiredRow, IpcError> {
    let c = parse_component(component)?;
    let t = validate_target(c, target)?;
    let v = Version::parse(version)
        .map_err(|e| IpcError::new(codes::E_INVALID, format!("version {version:?}: {e}")))?;
    let row = UpdateDesiredRow {
        component: c.as_str().into(),
        target: target.into(),
        version: v.to_string(),
        mandatory,
        reason,
        set_by: "operator".into(),
        set_at: now,
    };
    let s = lock(store)?;
    require_target_exists(&s, &t, target)?;
    s.set_update_desired(&row)?;
    decisions_may_have_changed();
    Ok(row)
}

pub fn unpin(store: &Mutex<Store>, component: &str, target: &str) -> Result<bool, IpcError> {
    let c = parse_component(component)?;
    validate_target(c, target)?;
    let removed = lock(store)?.clear_update_desired(c.as_str(), target)?;
    decisions_may_have_changed();
    Ok(removed)
}

/// What `update_admin { action: set_policy }` may set for one org.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OrgPolicyInput {
    pub mode: Option<String>,
    pub minimum: Option<String>,
    pub window: Option<String>,
    pub pin_version: Option<String>,
    pub pin_mandatory: bool,
    pub reason: Option<String>,
}

/// `update_admin { action: set_policy, org_id, component, … }`: one org's
/// override of the fleet's update policy for one component (S9). Fields left
/// out keep the fleet's value; the row replaces any earlier one whole.
pub fn set_org_policy(
    store: &Mutex<Store>,
    org_id: i64,
    component: &str,
    input: OrgPolicyInput,
    set_by: &str,
    now: i64,
) -> Result<UpdateOrgPolicyRow, IpcError> {
    let c = parse_component(component)?;
    let invalid = |m: String| IpcError::new(codes::E_INVALID, m);
    if let Some(m) = input.mode.as_deref() {
        let allowed: &[&str] = if matches!(c, Component::Android | Component::Ios) {
            settings::UPDATE_MOBILE_MODES
        } else {
            settings::UPDATE_MODES
        };
        if !allowed.contains(&m) {
            return Err(invalid(format!(
                "mode for {} must be one of {}, got {m:?}",
                c.as_str(),
                allowed.join(" | ")
            )));
        }
    }
    let version = |what: &str, v: &Option<String>| -> Result<Option<String>, IpcError> {
        v.as_deref()
            .map(|v| {
                Version::parse(v)
                    .map(|v| v.to_string())
                    .map_err(|e| invalid(format!("{what} {v:?}: {e}")))
            })
            .transpose()
    };
    let minimum = version("minimum", &input.minimum)?;
    let pin_version = version("pin", &input.pin_version)?;
    if let Some(w) = input.window.as_deref() {
        if settings::parse_time_range(w).is_none() {
            return Err(invalid(format!(
                "window {w:?}: a daily HH:MM-HH:MM in UTC, or \"\" for any time"
            )));
        }
    }
    if input.mode.is_none() && minimum.is_none() && input.window.is_none() && pin_version.is_none()
    {
        return Err(invalid(
            "set_policy needs at least one of mode, minimum, window, version".into(),
        ));
    }
    let s = lock(store)?;
    if s.get_org(org_id)?.is_none() {
        return Err(IpcError::new(
            codes::E_NOTFOUND,
            format!("no such org {org_id}"),
        ));
    }
    let row = UpdateOrgPolicyRow {
        org_id,
        component: c.as_str().into(),
        mode: input.mode,
        minimum,
        window: input.window.map(|w| w.trim().to_string()),
        pin_version,
        pin_mandatory: input.pin_mandatory,
        reason: input.reason,
        set_by: set_by.into(),
        set_at: now,
    };
    s.set_update_org_policy(&row)?;
    decisions_may_have_changed();
    Ok(row)
}

/// `update_admin { action: clear_policy, org_id, component }`.
pub fn clear_org_policy(
    store: &Mutex<Store>,
    org_id: i64,
    component: &str,
) -> Result<bool, IpcError> {
    let c = parse_component(component)?;
    let removed = lock(store)?.clear_update_org_policy(org_id, c.as_str())?;
    decisions_may_have_changed();
    Ok(removed)
}

/// How soon a target with an operator's pin to install asks again.
pub const PINNED_RECHECK_SECS: u64 = 120;

/// The file a hub's own updater watches (`<data_dir>/update-now`): written by
/// `update_now` for the hub, it wakes `fleet-updater` (which polls it) and
/// `fleet-hub-update.path` (which starts one `fleet-hub update apply`).
pub const UPDATE_NOW_FILE: &str = "update-now";

/// The shell line that pokes an agent host's updater: the agent's own unit
/// has `RuntimeDirectory=fleet-agent`, and `fleet-agent-update.path` starts a
/// pass when `update-now` appears there. Prints `poked` or why not.
pub const AGENT_POKE_SCRIPT: &str = "d=\"${RUNTIME_DIRECTORY:-}\"; \
    if [ -n \"$d\" ] && [ -d \"$d\" ]; then : > \"$d/update-now\" && echo poked; \
    else echo 'no update trigger: re-run fleet-agent install --auto-update'; fi";

/// What `update_now` set, and whom the caller should wake.
#[derive(Debug, Clone, Serialize)]
pub struct UpdateNow {
    pub pin: UpdateDesiredRow,
    /// Agent hosts to poke (`AGENT_POKE_SCRIPT`); empty for other components.
    pub agents: Vec<String>,
    /// The hub's own updater was woken (`<data_dir>/update-now`).
    pub hub_woken: bool,
}

/// `update_admin { action: update_now, component, target?, version? }`: install
/// now, whatever the mode. A mandatory pin to `version` (default: the
/// channel's recommended release) for the component or one target — a person
/// is still asked on a desktop or a phone, which show it as required — then
/// every updater that can be woken is: the hub's own through its trigger
/// file, and the agents, whose hosts the caller pokes (they need the SSH
/// layer, which this module does not hold).
pub fn update_now(
    store: &Mutex<Store>,
    component: &str,
    target: &str,
    version: Option<&str>,
    keys: &TrustedKeys,
    now: i64,
) -> Result<UpdateNow, IpcError> {
    let c = parse_component(component)?;
    let v = {
        let s = lock(store)?;
        let cached = load_cached(&s, track(&s), keys, now).ok_or_else(|| {
            IpcError::new(
                codes::E_UPDATE_UNVERIFIED,
                "no verified release channel yet: refresh it first",
            )
        })?;
        let v = match version {
            Some(v) => Version::parse(v)
                .map_err(|e| IpcError::new(codes::E_INVALID, format!("version {v:?}: {e}")))?,
            None => cached.channel.doc.recommended.clone(),
        };
        if !(cached.channel.doc.permits(c, &v) && cached.manifests.verified.contains_key(&v)) {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!(
                    "{v} is not a release the verified channel offers {}",
                    c.as_str()
                ),
            ));
        }
        v
    };
    let pin = pin(
        store,
        c.as_str(),
        target,
        &v.to_string(),
        true,
        Some("update now".into()),
        now,
    )?;
    let agents = if c == Component::Agent {
        match target.strip_prefix("agent:") {
            Some(alias) => vec![alias.to_string()],
            None => lock(store)?.agent_host_aliases()?,
        }
    } else {
        Vec::new()
    };
    let hub_woken = c == Component::Hub && wake_hub_updater();
    Ok(UpdateNow {
        pin,
        agents,
        hub_woken,
    })
}

/// Write `<data_dir>/update-now`; false when this process has no data dir.
pub fn wake_hub_updater() -> bool {
    match mirror::data_dir() {
        Some(d) => std::fs::write(d.join(UPDATE_NOW_FILE), b"").is_ok(),
        None => false,
    }
}

/// `update_admin { action: rollout_start }`: `version` must be a release the
/// verified channel lists, permits for the component, and has a manifest for.
pub fn rollout_start(
    store: &Mutex<Store>,
    component: &str,
    version: &str,
    waves: Option<Vec<u8>>,
    halt_failure_ratio: Option<f64>,
    keys: &TrustedKeys,
    now: i64,
) -> Result<UpdateRolloutRow, IpcError> {
    let c = parse_component(component)?;
    let v = Version::parse(version)
        .map_err(|e| IpcError::new(codes::E_INVALID, format!("version {version:?}: {e}")))?;
    let listed = {
        let s = lock(store)?;
        load_cached(&s, track(&s), keys, now).is_some_and(|cached| {
            cached.channel.doc.permits(c, &v) && cached.manifests.verified.contains_key(&v)
        })
    };
    rollout::start(store, c, &v, listed, waves, halt_failure_ratio, now)
}

pub fn rollout_pause(
    store: &Mutex<Store>,
    component: &str,
    reason: Option<&str>,
    now: i64,
) -> Result<UpdateRolloutRow, IpcError> {
    rollout::pause(store, parse_component(component)?, reason, now)
}

pub fn rollout_resume(
    store: &Mutex<Store>,
    component: &str,
    now: i64,
) -> Result<UpdateRolloutRow, IpcError> {
    rollout::resume(store, parse_component(component)?, now)
}

pub fn rollout_abort(
    store: &Mutex<Store>,
    component: &str,
    now: i64,
) -> Result<UpdateRolloutRow, IpcError> {
    rollout::abort(store, parse_component(component)?, now)
}

// ── Git mode: a standalone check ──

/// A standalone check (Git mode, design §7 F1): no hub above, so the
/// published channel is read directly and decided under a local policy.
/// What `fleet-hub update check` runs for the hub itself, and what a
/// standalone desktop will run for its own build (S7).
#[derive(Debug, Clone)]
pub struct GitCheck {
    /// Where `<track>.json` lives ([`channel_base_url`]).
    pub base_url: String,
    pub track: Track,
    pub policy: Policy,
    /// The highest channel sequence already trusted (the replay guard).
    pub seen: u64,
}

impl GitCheck {
    /// The settings, pin and last-seen sequence of `store` when there is
    /// one, the defaults otherwise; `track` overrides `update.track`.
    pub fn from_store(
        store: Option<&Store>,
        component: Component,
        target: &str,
        track_override: Option<Track>,
    ) -> Result<GitCheck, IpcError> {
        let track = track_override.unwrap_or_else(|| store.map_or(Track::Stable, track));
        let (policy, seen) = match store {
            Some(s) => (
                policy(s, component, target, crate::store::now_unix())?,
                s.update_doc("channel", track.as_str())?
                    .and_then(|d| d.sequence)
                    .map_or(0, |q| q.max(0) as u64),
            ),
            None => (Policy::default(), 0),
        };
        Ok(GitCheck {
            base_url: channel_base_url(),
            track,
            policy,
            seen,
        })
    }
}

/// The check request for this running hub build.
pub fn hub_self_request(installed: Installed) -> CheckRequest {
    CheckRequest {
        update_proto: UPDATE_PROTO,
        component: Component::Hub,
        platform: hub_platform(),
        installed,
        speaks: Speaks::default(),
        phase: UpdatePhase::Idle,
        attempt: None,
    }
}

/// Decide `req` from the published channel ([`GitUpdateChannel`]), and
/// prove a target it names against the signed documents. A channel that
/// is not published yet is `E_HUB_UNAVAILABLE`, saying so; one no trusted
/// key signed, or an older one than `check.seen`, is `E_UPDATE_UNVERIFIED`.
pub async fn git_check<F: Fetch + 'static>(
    fetch: F,
    check: GitCheck,
    keys: TrustedKeys,
    req: &CheckRequest,
    now: i64,
) -> Result<CheckOutcome, IpcError> {
    let url = fleet_update::channel::channel_url(&check.base_url, check.track);
    let seen = MemorySequenceStore::default();
    seen.record(check.track, check.seen);
    let channel = GitUpdateChannel::new(
        fetch,
        check.base_url,
        check.track,
        check.policy,
        keys,
        Box::new(seen),
    )
    .with_clock(move || now);
    channel.check(req).await.map_err(|e| match e {
        UpdateError::Http { status: 404, .. } => IpcError::new(
            codes::E_HUB_UNAVAILABLE,
            format!(
                "no {} channel is published yet (nothing at {url})",
                check.track.as_str()
            ),
        ),
        other => update_err(other),
    })
}

// ── refresh ──

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RefreshOutcome {
    pub track: Track,
    pub sequence: u64,
    pub fresh: bool,
    pub manifests: usize,
    pub fetched: usize,
}

fn update_err(e: UpdateError) -> IpcError {
    match e {
        UpdateError::Unverified(v) => IpcError::new(codes::E_UPDATE_UNVERIFIED, v.to_string()),
        UpdateError::Transport(m) => IpcError::new(codes::E_HUB_UNREACHABLE, m),
        other => IpcError::new(codes::E_HUB_UNAVAILABLE, other.to_string()),
    }
}

/// Fetch the track's channel, verify it, fetch and verify the manifests of
/// its newest releases (plus the rollback target and every pinned version),
/// and replace the cache. The store lock is never held across a fetch.
pub async fn refresh(
    store: &Mutex<Store>,
    fetch: &dyn Fetch,
    base_url: &str,
    keys: &TrustedKeys,
    now: i64,
) -> Result<RefreshOutcome, IpcError> {
    let (track, seen, pinned, cached_sha) = {
        let s = lock(store)?;
        let track = track(&s);
        let seen = s
            .update_doc("channel", track.as_str())?
            .and_then(|d| d.sequence)
            .map(|q| q.max(0) as u64)
            .unwrap_or(0);
        let pinned: Vec<Version> = s
            .update_desired_all()?
            .iter()
            .filter_map(|d| Version::parse(&d.version).ok())
            .collect();
        let cached_sha: BTreeMap<String, String> = s
            .update_docs("manifest")?
            .into_iter()
            .map(|d| (d.key, fleet_update::verify::sha256_hex(d.body.as_bytes())))
            .collect();
        (track, seen, pinned, cached_sha)
    };
    let cached_amendments: BTreeMap<String, String> = lock(store)?
        .update_docs("amendment")?
        .into_iter()
        .map(|d| (d.key, fleet_update::verify::sha256_hex(d.body.as_bytes())))
        .collect();
    let result = async {
        let (channel, raw) = fetch_channel(fetch, base_url, track, keys, seen, now)
            .await
            .map_err(update_err)?;
        let mut wanted = wanted_versions(&channel, None, MANIFESTS_KEPT);
        for v in pinned {
            if !wanted.contains(&v) && channel.doc.release(&v).is_some() {
                wanted.push(v);
            }
        }
        // Only what changed: a manifest is immutable, so a cached one whose
        // bytes the channel still lists is kept as is.
        let to_fetch: Vec<Version> = wanted
            .iter()
            .filter(|v| {
                let listed = channel
                    .doc
                    .release(v)
                    .map(|r| r.manifest_sha256.to_ascii_lowercase());
                cached_sha
                    .get(&v.to_string())
                    .map(|s| s.to_ascii_lowercase())
                    != listed
            })
            .cloned()
            .collect();
        let fetched = fetch_manifests(fetch, &channel, to_fetch, keys).await;
        // Amendments arrive after their release (the phone's APK, design §4),
        // so they are fetched on their own: whatever the channel now lists
        // for a kept release that the cache does not hold byte for byte.
        let mut amendments: Vec<(String, RawDoc)> = Vec::new();
        for v in &wanted {
            let Some(listed) = channel.doc.release(v) else {
                continue;
            };
            let stale = listed.amendments.iter().any(|a| {
                cached_amendments
                    .get(&amendment_key(v, &a.component))
                    .map(|s| s.eq_ignore_ascii_case(&a.manifest_sha256))
                    != Some(true)
            });
            if stale {
                for (a, raw, _) in
                    fleet_update::channel::fetch_amendments(fetch, &channel, v, keys).await
                {
                    amendments.push((amendment_key(v, &a.component), raw));
                }
            }
        }
        Ok::<_, IpcError>((channel, raw, wanted, fetched, amendments))
    }
    .await;
    let s = lock(store)?;
    let (channel, raw, wanted, fetched, amendments) = match result {
        Ok(r) => r,
        Err(e) => {
            let _ = s.set_setting(
                LAST_REFRESH_KEY,
                &serde_json::json!({"at": now, "ok": false, "code": e.code, "error": e.message})
                    .to_string(),
            );
            return Err(e);
        }
    };
    s.put_update_doc(&UpdateDocRow {
        kind: "channel".into(),
        key: track.as_str().into(),
        body: raw.body,
        sig: raw.sig,
        sequence: Some(channel.doc.sequence as i64),
        fetched_at: now,
    })?;
    for (v, m) in &fetched.raw {
        s.put_update_doc(&UpdateDocRow {
            kind: "manifest".into(),
            key: v.to_string(),
            body: m.body.clone(),
            sig: m.sig.clone(),
            sequence: None,
            fetched_at: now,
        })?;
    }
    for (key, a) in &amendments {
        s.put_update_doc(&UpdateDocRow {
            kind: "amendment".into(),
            key: key.clone(),
            body: a.body.clone(),
            sig: a.sig.clone(),
            sequence: None,
            fetched_at: now,
        })?;
    }
    let keep: Vec<String> = wanted.iter().map(|v| v.to_string()).collect();
    let pruned = s.prune_update_manifests(&keep)?;
    let dropped = mirror::prune(&s, keys, now);
    if dropped > 0 {
        tracing::info!(
            files = dropped,
            "[update] dropped mirrored artifacts no release lists"
        );
    }
    if channel.doc.sequence != seen
        || !fetched.raw.is_empty()
        || !amendments.is_empty()
        || pruned > 0
    {
        s.emit_update_changed("channel", None);
        decisions_may_have_changed();
    }
    let manifests = s.update_docs("manifest")?.len();
    let outcome = RefreshOutcome {
        track,
        sequence: channel.doc.sequence,
        fresh: channel.fresh,
        manifests,
        fetched: fetched.raw.len(),
    };
    let _ = s.set_setting(
        LAST_REFRESH_KEY,
        &serde_json::json!({"at": now, "ok": true, "sequence": outcome.sequence, "fetched": outcome.fetched})
            .to_string(),
    );
    Ok(outcome)
}

/// What the hub knows about its own build, recorded as `hub:self` when the
/// refresh tick starts so the dashboard shows the hub before any
/// `fleet-updater` reports for it.
#[derive(Debug, Clone)]
pub struct HubSelf {
    pub version: String,
    pub commit: String,
    pub build_id: String,
}

/// This hub's platform: in a container it is updated as an image, else as
/// the release tarball.
pub fn hub_platform() -> Platform {
    let variant = if std::path::Path::new("/.dockerenv").exists() {
        "oci"
    } else {
        "tarball"
    };
    Platform::new(std::env::consts::OS, std::env::consts::ARCH, variant)
}

/// Record `hub:self` at start. The same version keeps the phase, attempt
/// and error the updater last reported; a new version clears them — a new
/// binary is running, and the old attempt's outcome no longer describes it
/// (the updater's own queued reports, if any, follow and set them again).
pub(crate) fn record_hub_self(store: &Store, me: &HubSelf, now: i64) -> Result<(), IpcError> {
    let prev = store
        .update_observed("hub:self")?
        .filter(|p| p.version == me.version);
    store.upsert_update_observed(&UpdateObservedRow {
        target: "hub:self".into(),
        component: "hub".into(),
        platform: json(&hub_platform()),
        version: me.version.clone(),
        commit_sha: Some(me.commit.clone()),
        build_id: Some(me.build_id.clone()),
        // The running image digest is the updater's to report (and a new
        // version's is not the old one's).
        digest: prev.as_ref().and_then(|p| p.digest.clone()),
        speaks: None,
        phase: prev
            .as_ref()
            .map(|p| p.phase.clone())
            .unwrap_or_else(|| "idle".into()),
        attempt: prev.as_ref().and_then(|p| p.attempt.clone()),
        last_error: prev.and_then(|p| p.last_error),
        reported_at: now,
        last_checked_at: None,
    })
}

/// The hub's channel refresh, every `update.check_interval_secs`, until
/// `cancel`. A failed refresh keeps the last good cache.
pub fn spawn_refresh_tick(
    store: Arc<Mutex<Store>>,
    me: HubSelf,
    cancel: tokio_util::sync::CancellationToken,
) -> tokio::task::JoinHandle<()> {
    crate::rt::spawn(async move {
        if let Ok(s) = lock(&store) {
            if let Err(e) = record_hub_self(&s, &me, crate::store::now_unix()) {
                tracing::warn!(error = %e.message, "could not record the hub's own version");
            }
        }
        let keys = trusted_keys();
        // The decision pusher: woken by a pin, a setting or a new channel,
        // and every few minutes for the rollouts' waves and the maintenance
        // window's edges, which move decisions with nothing else changing.
        {
            let (store, keys, cancel) = (store.clone(), keys.clone(), cancel.clone());
            crate::rt::spawn(async move {
                let _ = push_decisions(&store, &keys, crate::store::now_unix());
                loop {
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => break,
                        _ = DECIDE_WAKE.notified() => {}
                        _ = tokio::time::sleep(DECIDE_BEAT) => {}
                    }
                    match rollout::advance(&store, crate::store::now_unix()) {
                        Ok(moved) => {
                            for m in moved {
                                tracing::info!(component = %m.component, version = %m.version, what = m.what, wave = m.wave + 1, "update rollout moved");
                            }
                        }
                        Err(e) => {
                            tracing::warn!(error = %e.message, "could not advance the update rollouts")
                        }
                    }
                    let pushed = push_decisions(&store, &keys, crate::store::now_unix());
                    if !pushed.is_empty() {
                        tracing::info!(targets = ?pushed, "update decisions changed");
                    }
                }
            });
        }
        // The last failure's code: a failure warns once, and again only when
        // the reason changes, so a hub offline for a week does not warn every
        // tick.
        let mut last_code: Option<String> = None;
        loop {
            let base = channel_base_url();
            let fetch = HttpsFetch::new(Some(&base));
            let refreshed = refresh(&store, &fetch, &base, &keys, crate::store::now_unix()).await;
            let outcome = refreshed
                .as_ref()
                .map(|_| ())
                .map_err(|e| e.message.clone());
            match refreshed {
                Ok(o) => {
                    last_code = None;
                    tracing::info!(
                        track = o.track.as_str(),
                        sequence = o.sequence,
                        fetched = o.fetched,
                        "update channel refreshed"
                    )
                }
                // A signature, rollback or transport failure is the operator's
                // to see; repeats of the same one are not. (Until a release
                // publishes the track's channel every refresh fails, so the
                // repeats below stay at debug.)
                Err(e) if last_code.as_deref() != Some(e.code.as_str()) => {
                    tracing::warn!(code = %e.code, error = %e.message, "update channel refresh failed");
                    last_code = Some(e.code.clone());
                }
                Err(e) => {
                    tracing::debug!(code = %e.code, error = %e.message, "update channel refresh failed again")
                }
            }
            let interval = lock(&store)
                .map(|s| check_interval_secs(&s))
                .unwrap_or(21_600);
            crate::service::loops::report(
                "updates",
                outcome,
                Some(std::time::Duration::from_secs(interval)),
            );
            tokio::select! {
                biased;
                _ = cancel.cancelled() => break,
                // A new track or interval: refresh now, then re-arm.
                _ = REFRESH_WAKE.notified() => {}
                _ = tokio::time::sleep(std::time::Duration::from_secs(interval)) => {}
            }
        }
    })
}
