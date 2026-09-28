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
#[cfg(test)]
mod tests;

pub use fetch::HttpsFetch;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use fleet_update::channel::{attach_evidence, RawDoc};
use fleet_update::decide::{decide, DecideInput, HubSpeaks, Pin, Policy};
use fleet_update::verify::verify_channel;
use fleet_update::wire::{CheckRequest, Decision, Report, Speaks, Status, UPDATE_PROTO};
use fleet_update::{
    fetch_channel, fetch_manifests, verify_listed_manifest, wanted_versions, Component, Fetch,
    Manifests, Mode, Platform, Source, Track, TrustedKeys, UpdateError, VerifiedChannel, Version,
    Window,
};
use serde::Serialize;

use crate::ipc_error::{codes, lock, IpcError};
use crate::mcp::auth::{Caller, TokenMode};
use crate::service::settings;
use crate::store::{Store, UpdateDesiredRow, UpdateDocRow, UpdateObservedRow};

/// Where CI publishes the channel documents (design U10).
pub const CHANNEL_BASE_URL: &str =
    "https://raw.githubusercontent.com/martin-janci/claude-fleet/update-channels/";
/// Overrides [`CHANNEL_BASE_URL`] (a mirror, a test). Only *where* the
/// documents come from: what is trusted is still the signature.
pub const CHANNEL_URL_ENV: &str = "FLEET_UPDATE_CHANNEL_URL";
/// How many of a track's newest releases the hub keeps manifests for.
pub const MANIFESTS_KEPT: usize = 10;
/// Internal (not a registry setting): the last refresh's outcome.
const LAST_REFRESH_KEY: &str = "update.last_refresh";

/// The release keys this hub verifies with: the compiled-in ones, plus — in
/// an `e2e` test build only — `FLEET_UPDATE_E2E_KEYS` (comma-separated), so
/// `scripts/hub-e2e.sh` can publish a channel signed by a throwaway key. A
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

pub fn track(store: &Store) -> Track {
    match setting(store, settings::UPDATE_TRACK).as_str() {
        "beta" => Track::Beta,
        "nightly" => Track::Nightly,
        _ => Track::Stable,
    }
}

fn setting(store: &Store, key: &str) -> String {
    let raw = store.get_setting(key).ok().flatten();
    settings::resolve(key, raw.as_deref()).to_string()
}

pub fn mode(store: &Store, c: Component) -> Mode {
    let key = match c {
        Component::Hub => settings::UPDATE_HUB_MODE,
        Component::Agent => settings::UPDATE_AGENT_MODE,
        Component::Desktop => settings::UPDATE_DESKTOP_MODE,
        Component::Android | Component::Ios => settings::UPDATE_MOBILE_MODE,
    };
    match setting(store, key).as_str() {
        "manual" => Mode::Manual,
        "automatic" => Mode::Automatic,
        _ => Mode::Notify,
    }
}

pub fn check_interval_secs(store: &Store) -> u64 {
    setting(store, settings::UPDATE_CHECK_INTERVAL_SECS)
        .parse()
        .unwrap_or(21_600)
}

/// The policy for one target: the fleet's mode for its component, and the
/// operator's pin (its own, else the component's).
fn policy(store: &Store, c: Component, target: &str) -> Result<Policy, IpcError> {
    let pin = store.update_desired_for(c.as_str(), target)?.and_then(|d| {
        Version::parse(&d.version).ok().map(|version| Pin {
            version,
            mandatory: d.mandatory,
        })
    });
    Ok(Policy {
        mode: mode(store, c),
        minimum: None,
        pin,
        outside_window: false,
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
        if let Some(verified) = verify_listed_manifest(&raw, listed, keys) {
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
    let policy = policy(store, component, target)?;
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
        rollout: None,
        target_id: target,
        source: Source::Hub,
        track: track(store),
        now,
    });
    if let Some(c) = cached {
        attach_evidence(&mut d, &c.raw, &c.manifests.raw);
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
    check_proto(req.update_proto)?;
    let id = identity(caller)?;
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
            phase: serde_json::to_value(req.phase)
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_else(|| "idle".into()),
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

/// `POST /update/report`: one state-machine transition and the observed
/// state that comes with it. Idempotent on `(target, attempt, phase)`.
pub fn report(
    store: &Mutex<Store>,
    caller: &Caller,
    r: &Report,
    now: i64,
) -> Result<bool, IpcError> {
    check_proto(r.update_proto)?;
    let id = identity(caller)?;
    if !id.may_report {
        return Err(IpcError::new(
            codes::E_FORBIDDEN,
            "the master token looks; only a target reports itself",
        ));
    }
    require_component(&id, r.component)?;
    let phase = serde_json::to_value(r.phase)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| "unknown".into());
    let detail = (!r.detail.is_null()).then(|| r.detail.to_string());
    let s = lock(store)?;
    let prev = s.update_observed(&id.target)?;
    s.upsert_update_observed(&UpdateObservedRow {
        target: id.target.clone(),
        component: r.component.as_str().into(),
        platform: prev.as_ref().and_then(|p| p.platform.clone()),
        version: r.installed.version.to_string(),
        commit_sha: r.installed.commit.clone(),
        build_id: r.installed.build_id.clone(),
        digest: r.installed.digest.clone(),
        speaks: prev.as_ref().and_then(|p| p.speaks.clone()),
        phase: phase.clone(),
        attempt: r.attempt.clone(),
        last_error: r.error.clone(),
        reported_at: now,
        last_checked_at: None,
    })?;
    s.insert_update_event(
        &id.target,
        r.attempt.as_deref(),
        &phase,
        r.from.as_ref().map(|v| v.to_string()).as_deref(),
        r.to.as_ref().map(|v| v.to_string()).as_deref(),
        detail.as_deref(),
        r.error.as_deref(),
        now,
    )
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
    let own = if caller.is_scoped() {
        Some(identity(caller)?.target)
    } else {
        None
    };
    let s = lock(store)?;
    let track = track(&s);
    let cached = load_cached(&s, track, keys, now);
    let mut targets = Vec::new();
    for o in s.update_observed_all()? {
        if own.as_ref().is_some_and(|t| *t != o.target) {
            continue;
        }
        let Ok(component) =
            serde_json::from_value::<Component>(serde_json::Value::String(o.component.clone()))
        else {
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
            &s,
            cached.as_ref(),
            component,
            &o.target,
            &platform,
            &installed,
            &speaks,
            now,
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
    })
}

// ── admin ──

fn parse_component(s: &str) -> Result<Component, IpcError> {
    serde_json::from_value(serde_json::Value::String(s.to_string())).map_err(|_| {
        IpcError::new(
            codes::E_INVALID,
            format!("component must be hub | agent | desktop | android | ios, got {s:?}"),
        )
    })
}

fn validate_target(component: Component, target: &str) -> Result<(), IpcError> {
    let ok = target.is_empty()
        || match component {
            Component::Hub => target == "hub:self",
            Component::Agent => target.strip_prefix("agent:").is_some_and(|a| !a.is_empty()),
            Component::Desktop | Component::Android | Component::Ios => target
                .strip_prefix("client:")
                .is_some_and(|id| id.parse::<i64>().is_ok()),
        };
    if ok {
        Ok(())
    } else {
        Err(IpcError::new(
            codes::E_INVALID,
            format!("target {target:?} is not a {} target", component.as_str()),
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
    validate_target(c, target)?;
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
    lock(store)?.set_update_desired(&row)?;
    Ok(row)
}

pub fn unpin(store: &Mutex<Store>, component: &str, target: &str) -> Result<bool, IpcError> {
    let c = parse_component(component)?;
    validate_target(c, target)?;
    lock(store)?.clear_update_desired(c.as_str(), target)
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
        Ok::<_, IpcError>((channel, raw, wanted, fetched))
    }
    .await;
    let s = lock(store)?;
    let (channel, raw, wanted, fetched) = match result {
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
    let keep: Vec<String> = wanted.iter().map(|v| v.to_string()).collect();
    s.prune_update_manifests(&keep)?;
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

fn record_hub_self(store: &Store, me: &HubSelf, now: i64) -> Result<(), IpcError> {
    let variant = if std::path::Path::new("/.dockerenv").exists() {
        "oci"
    } else {
        "tarball"
    };
    let prev = store.update_observed("hub:self")?;
    store.upsert_update_observed(&UpdateObservedRow {
        target: "hub:self".into(),
        component: "hub".into(),
        platform: json(&Platform::new(
            std::env::consts::OS,
            std::env::consts::ARCH,
            variant,
        )),
        version: me.version.clone(),
        commit_sha: Some(me.commit.clone()),
        build_id: Some(me.build_id.clone()),
        // The running image digest is the updater's to report.
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
        loop {
            let base = channel_base_url();
            let fetch = HttpsFetch::new(Some(&base));
            match refresh(&store, &fetch, &base, &keys, crate::store::now_unix()).await {
                Ok(o) => tracing::info!(
                    track = o.track.as_str(),
                    sequence = o.sequence,
                    fetched = o.fetched,
                    "update channel refreshed"
                ),
                // Debug, not warn: until the release key exists every refresh
                // fails verification, and that is the expected state.
                Err(e) => {
                    tracing::debug!(code = %e.code, error = %e.message, "update channel refresh failed")
                }
            }
            let interval = lock(&store)
                .map(|s| check_interval_secs(&s))
                .unwrap_or(21_600);
            tokio::select! {
                biased;
                _ = cancel.cancelled() => break,
                _ = tokio::time::sleep(std::time::Duration::from_secs(interval)) => {}
            }
        }
    })
}
