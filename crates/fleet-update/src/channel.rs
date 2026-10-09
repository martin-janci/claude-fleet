//! `UpdateChannel` and its two implementations (F3, design §7).
//!
//! - [`GitUpdateChannel`] reads the signed channel document CI publishes
//!   (`update-channels:<track>.json`) and runs [`decide`] itself under a local
//!   policy: a standalone desktop, a hub reading its own release feed, a
//!   `fleet-updater --standalone`.
//! - [`HubUpdateChannel`] asks a hub (`POST /update/check`) and obeys it —
//!   within the publisher's signed bounds (U6).
//!
//! Both end in [`verify_target`], so what gets installed is proven against
//! the signed documents the same way whichever source decided.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Mutex;

use async_trait::async_trait;

use crate::channel_doc::ReleaseRef;
use crate::decide::{decide, DecideInput, Policy};
use crate::manifest::ReleaseManifest;
use crate::model::{Source, Track};
use crate::verify::{
    sha256_hex, verify_channel, verify_manifest, verify_target, TrustedKeys, VerifiedChannel,
    VerifiedTarget, VerifyError,
};
use crate::wire::{CheckRequest, Decision, Evidence, Report, UPDATE_PROTO};
use crate::Version;

/// Documents are small; anything larger is not one of ours.
pub const MAX_DOC_BYTES: u64 = 1 << 20;
/// How many releases newer than the installed one a Git check fetches
/// manifests for once they install here. The newest permitted one wins, so a
/// handful is plenty.
pub const MAX_MANIFESTS: usize = 5;
/// How many it looks at, at most, to find those: on `nightly` most releases
/// are per-push builds with no desktop bundle (S2b), and they are skipped
/// past rather than counted.
pub const MAX_MANIFESTS_SCANNED: usize = 30;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateError {
    /// The request never got an answer (DNS, TCP, TLS, timeout).
    Transport(String),
    /// An answer that was not a success.
    Http {
        status: u16,
        body: String,
    },
    /// A body that does not parse as what was expected.
    Decode(String),
    TooLarge,
    /// The signed documents do not support the decision (`E_UPDATE_UNVERIFIED`).
    Unverified(VerifyError),
    /// The hub answered for a different component or a protocol it should not.
    Protocol(String),
}

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UpdateError::Transport(m) => write!(f, "unreachable: {m}"),
            UpdateError::Http { status, body } => {
                write!(f, "HTTP {status}: {body}")
            }
            UpdateError::Decode(m) => write!(f, "unreadable update response: {m}"),
            UpdateError::TooLarge => write!(f, "update document too large"),
            UpdateError::Unverified(e) => write!(f, "update not verified: {e}"),
            UpdateError::Protocol(m) => write!(f, "update protocol error: {m}"),
        }
    }
}

impl std::error::Error for UpdateError {}

impl From<VerifyError> for UpdateError {
    fn from(e: VerifyError) -> Self {
        UpdateError::Unverified(e)
    }
}

/// HTTP GET, as the embedding process does it (fleet-core's HTTP/1 client in
/// the hub and desktop, a tiny one in fleet-updater).
#[async_trait]
pub trait Fetch: Send + Sync {
    /// The body of a 200, at most `max_bytes` long.
    async fn get(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>, UpdateError>;
}

/// A POST to the paired hub with the caller's own credential.
#[async_trait]
pub trait HubTransport: Send + Sync {
    /// `path` is `/update/check` or `/update/report`; the body is JSON.
    async fn post(&self, path: &str, body: Vec<u8>) -> Result<Vec<u8>, UpdateError>;
}

/// The highest channel `sequence` seen per track (the replay guard). The
/// embedding process persists it; a lost store only weakens replay protection
/// back to "no older than the compiled-in floor".
pub trait SequenceStore: Send + Sync {
    fn seen(&self, track: Track) -> u64;
    fn record(&self, track: Track, sequence: u64);
}

#[derive(Default)]
pub struct MemorySequenceStore(Mutex<BTreeMap<Track, u64>>);

impl SequenceStore for MemorySequenceStore {
    fn seen(&self, track: Track) -> u64 {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&track)
            .copied()
            .unwrap_or(0)
    }

    fn record(&self, track: Track, sequence: u64) {
        let mut m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let e = m.entry(track).or_insert(0);
        *e = (*e).max(sequence);
    }
}

/// A check's result: the decision, and — when it names a target — that
/// target proven against the signed documents.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckOutcome {
    pub decision: Decision,
    pub verified: Option<VerifiedTarget>,
}

#[async_trait]
pub trait UpdateChannel: Send + Sync {
    fn source(&self) -> Source;
    async fn check(&self, req: &CheckRequest) -> Result<CheckOutcome, UpdateError>;
    async fn report(&self, report: &Report) -> Result<(), UpdateError>;
}

/// A document and its detached signature, exactly as fetched.
#[derive(Debug, Clone, PartialEq)]
pub struct RawDoc {
    pub body: String,
    pub sig: String,
}

/// Put the signed documents a decision rests on into it (design §6.2), so
/// whoever receives it can verify it alone. The hub does this for every
/// decision it serves; [`GitUpdateChannel`] does it for its own.
pub fn attach_evidence(
    decision: &mut Decision,
    channel: &RawDoc,
    manifests: &BTreeMap<Version, RawDoc>,
) {
    if let Some(t) = decision.target.as_mut() {
        if let Some(m) = manifests.get(&t.version) {
            t.evidence = Some(Evidence {
                channel: channel.body.clone(),
                channel_sig: channel.sig.clone(),
                manifest: m.body.clone(),
                manifest_sig: m.sig.clone(),
            });
        }
    }
}

type Clock = Box<dyn Fn() -> i64 + Send + Sync>;

fn system_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Git mode (F1): the source of truth is what CI published from `main` and
/// the tags; the policy is local.
pub struct GitUpdateChannel<F: Fetch> {
    fetch: F,
    /// Where `<track>.json` lives, e.g.
    /// `https://raw.githubusercontent.com/martin-janci/claude-fleet/update-channels/`.
    base_url: String,
    track: Track,
    policy: Policy,
    keys: TrustedKeys,
    seen: Box<dyn SequenceStore>,
    now: Clock,
}

impl<F: Fetch> GitUpdateChannel<F> {
    pub fn new(
        fetch: F,
        base_url: impl Into<String>,
        track: Track,
        policy: Policy,
        keys: TrustedKeys,
        seen: Box<dyn SequenceStore>,
    ) -> Self {
        GitUpdateChannel {
            fetch,
            base_url: base_url.into(),
            track,
            policy,
            keys,
            seen,
            now: Box::new(system_now),
        }
    }

    /// Replace the clock (tests).
    pub fn with_clock(mut self, now: impl Fn() -> i64 + Send + Sync + 'static) -> Self {
        self.now = Box::new(now);
        self
    }
}

/// A document and its `.minisig`, fetched verbatim.
pub async fn fetch_raw<F: Fetch + ?Sized>(fetch: &F, url: &str) -> Result<RawDoc, UpdateError> {
    let body = fetch.get(url, MAX_DOC_BYTES).await?;
    let sig = fetch.get(&format!("{url}.minisig"), 4096).await?;
    let text = |b: Vec<u8>| String::from_utf8(b).map_err(|e| UpdateError::Decode(e.to_string()));
    Ok(RawDoc {
        body: text(body)?,
        sig: text(sig)?,
    })
}

/// `<base>/<track>.json`.
pub fn channel_url(base_url: &str, track: Track) -> String {
    let sep = if base_url.ends_with('/') { "" } else { "/" };
    format!("{base_url}{sep}{}.json", track.as_str())
}

/// Fetch and verify a track's channel document. The caller records
/// `doc.sequence` in its [`SequenceStore`] afterwards.
pub async fn fetch_channel<F: Fetch + ?Sized>(
    fetch: &F,
    base_url: &str,
    track: Track,
    keys: &TrustedKeys,
    seen: u64,
    now: i64,
) -> Result<(VerifiedChannel, RawDoc), UpdateError> {
    let raw = fetch_raw(fetch, &channel_url(base_url, track)).await?;
    let channel = verify_channel(raw.body.as_bytes(), &raw.sig, keys, track, seen, now)?;
    Ok((channel, raw))
}

/// Verify one release's manifest against the channel's listing: the exact
/// bytes the channel names (sha256), a trusted signature, and the version it
/// is listed under. `None` for anything else: it simply cannot be a target.
pub fn verify_listed_manifest(
    raw: &RawDoc,
    listed: &ReleaseRef,
    keys: &TrustedKeys,
) -> Option<ReleaseManifest> {
    if !sha256_hex(raw.body.as_bytes()).eq_ignore_ascii_case(&listed.manifest_sha256) {
        return None;
    }
    let m = verify_manifest(raw.body.as_bytes(), &raw.sig, keys).ok()?;
    (m.release.version == listed.version).then_some(m)
}

/// Verified manifests, with the raw documents they came from (the evidence a
/// decision relays).
#[derive(Debug, Clone, Default)]
pub struct Manifests {
    pub verified: BTreeMap<Version, ReleaseManifest>,
    pub raw: BTreeMap<Version, RawDoc>,
}

/// The amendments the channel lists for `version` (design §4), fetched and
/// verified: one that fails either is left out (its component is then simply
/// not offered).
pub async fn fetch_amendments<F: Fetch + ?Sized>(
    fetch: &F,
    channel: &VerifiedChannel,
    version: &Version,
    keys: &TrustedKeys,
) -> Vec<(
    crate::channel_doc::AmendmentRef,
    RawDoc,
    crate::manifest::Amendment,
)> {
    let mut out = Vec::new();
    let Some(listed) = channel.doc.release(version) else {
        return out;
    };
    for a in &listed.amendments {
        let Ok(raw) = fetch_raw(fetch, &a.manifest).await else {
            continue;
        };
        if let Some(am) =
            crate::verify::verify_amendment(raw.body.as_bytes(), &raw.sig, keys, a, version)
        {
            out.push((a.clone(), raw, am));
        }
    }
    out
}

/// Fetch and verify the manifests of `versions` the channel lists, with
/// their amendments folded in. One that fails to fetch or verify is left out.
pub async fn fetch_manifests<F: Fetch + ?Sized>(
    fetch: &F,
    channel: &VerifiedChannel,
    versions: impl IntoIterator<Item = Version>,
    keys: &TrustedKeys,
) -> Manifests {
    let mut out = Manifests::default();
    for v in versions {
        let Some(listed) = channel.doc.release(&v) else {
            continue;
        };
        let Ok(raw) = fetch_raw(fetch, &listed.manifest).await else {
            continue;
        };
        if let Some(mut m) = verify_listed_manifest(&raw, listed, keys) {
            for (_, _, a) in fetch_amendments(fetch, channel, &v, keys).await {
                m.amend(&a);
            }
            out.verified.insert(v.clone(), m);
            out.raw.insert(v, raw);
        }
    }
    out
}

/// The versions worth a manifest fetch: the `newest` releases above `above`
/// (all of them when `above` is `None`), plus the signed rollback target.
pub fn wanted_versions(
    channel: &VerifiedChannel,
    above: Option<&Version>,
    newest: usize,
) -> Vec<Version> {
    let mut vs: Vec<Version> = channel
        .doc
        .releases
        .iter()
        .map(|r| r.version.clone())
        .filter(|v| above.is_none_or(|a| v > a))
        .collect();
    vs.sort_by(|a: &Version, b: &Version| b.cmp(a));
    vs.truncate(newest);
    if let Some(rb) = &channel.doc.rollback {
        if !vs.contains(rb) && channel.doc.release(rb).is_some() {
            vs.push(rb.clone());
        }
    }
    vs
}

#[async_trait]
impl<F: Fetch> UpdateChannel for GitUpdateChannel<F> {
    fn source(&self) -> Source {
        Source::Git
    }

    async fn check(&self, req: &CheckRequest) -> Result<CheckOutcome, UpdateError> {
        let now = (self.now)();
        let seen = self.seen.seen(self.track);
        let (channel, channel_raw) = fetch_channel(
            &self.fetch,
            &self.base_url,
            self.track,
            &self.keys,
            seen,
            now,
        )
        .await?;
        self.seen.record(self.track, channel.doc.sequence);

        // Manifests of the newest releases above what is installed, newest
        // first, until MAX_MANIFESTS of them carry an artifact for this caller.
        let wanted = wanted_versions(
            &channel,
            Some(&req.installed.version),
            MAX_MANIFESTS_SCANNED,
        );
        let mut found = Manifests::default();
        let mut installable = 0;
        for v in wanted {
            let one = fetch_manifests(&self.fetch, &channel, [v], &self.keys).await;
            for (v, m) in one.verified {
                if m.artifact_for(req.component, &req.platform).is_some() {
                    installable += 1;
                }
                found.verified.insert(v, m);
            }
            found.raw.extend(one.raw);
            if installable >= MAX_MANIFESTS {
                break;
            }
        }
        let Manifests {
            verified: manifests,
            raw: raws,
        } = found;

        let mut decision = decide(&DecideInput {
            component: req.component,
            platform: &req.platform,
            installed: &req.installed.version,
            speaks: &req.speaks,
            hub: None,
            channel: Some(&channel),
            manifests: &manifests,
            policy: &self.policy,
            rollout: None,
            target_id: "local",
            source: Source::Git,
            track: self.track,
            now,
        });
        attach_evidence(&mut decision, &channel_raw, &raws);
        let verified = verify_target(&decision, &self.keys, &req.platform, seen, now)?;
        Ok(CheckOutcome { decision, verified })
    }

    /// Nobody to report to in Git mode.
    async fn report(&self, _report: &Report) -> Result<(), UpdateError> {
        Ok(())
    }
}

/// Hub mode (F2): the hub decides; this checks the decision against the
/// signed documents and nothing more.
pub struct HubUpdateChannel<T: HubTransport> {
    transport: T,
    keys: TrustedKeys,
    seen: Box<dyn SequenceStore>,
    now: Clock,
}

impl<T: HubTransport> HubUpdateChannel<T> {
    pub fn new(transport: T, keys: TrustedKeys, seen: Box<dyn SequenceStore>) -> Self {
        HubUpdateChannel {
            transport,
            keys,
            seen,
            now: Box::new(system_now),
        }
    }

    pub fn with_clock(mut self, now: impl Fn() -> i64 + Send + Sync + 'static) -> Self {
        self.now = Box::new(now);
        self
    }
}

#[async_trait]
impl<T: HubTransport> UpdateChannel for HubUpdateChannel<T> {
    fn source(&self) -> Source {
        Source::Hub
    }

    async fn check(&self, req: &CheckRequest) -> Result<CheckOutcome, UpdateError> {
        let body = serde_json::to_vec(req).map_err(|e| UpdateError::Decode(e.to_string()))?;
        let resp = self.transport.post("/update/check", body).await?;
        let decision: Decision =
            serde_json::from_slice(&resp).map_err(|e| UpdateError::Decode(e.to_string()))?;
        if decision.update_proto < UPDATE_PROTO {
            return Err(UpdateError::Protocol(format!(
                "hub answered update_proto {}",
                decision.update_proto
            )));
        }
        if decision.component != req.component {
            return Err(UpdateError::Protocol(format!(
                "asked about {}, the hub answered for {}",
                req.component.as_str(),
                decision.component.as_str()
            )));
        }
        let now = (self.now)();
        let verified = verify_target(
            &decision,
            &self.keys,
            &req.platform,
            self.seen.seen(decision.track),
            now,
        )?;
        if let Some(v) = &verified {
            self.seen.record(decision.track, v.channel_sequence);
        }
        Ok(CheckOutcome { decision, verified })
    }

    async fn report(&self, report: &Report) -> Result<(), UpdateError> {
        let body = serde_json::to_vec(report).map_err(|e| UpdateError::Decode(e.to_string()))?;
        self.transport
            .post("/update/report", body)
            .await
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Component, Mode, Platform};
    use crate::phase::UpdatePhase;
    use crate::testkit::{manifest_json, TestKey};
    use crate::wire::{Installed, ReasonCode, Speaks, Status};
    use std::collections::HashMap;
    use std::sync::Arc;

    const NOW: i64 = 1_790_763_120;
    const BASE: &str = "https://raw.test/update-channels/";

    #[derive(Clone, Default)]
    struct MapFetch(Arc<Mutex<HashMap<String, Vec<u8>>>>);

    impl MapFetch {
        fn put(&self, url: &str, body: &str) {
            self.0
                .lock()
                .unwrap()
                .insert(url.into(), body.as_bytes().to_vec());
        }
    }

    #[async_trait]
    impl Fetch for MapFetch {
        async fn get(&self, url: &str, max: u64) -> Result<Vec<u8>, UpdateError> {
            let b = self
                .0
                .lock()
                .unwrap()
                .get(url)
                .cloned()
                .ok_or_else(|| UpdateError::Http {
                    status: 404,
                    body: url.into(),
                })?;
            if b.len() as u64 > max {
                return Err(UpdateError::TooLarge);
            }
            Ok(b)
        }
    }

    /// Publishes 0.3.3 and 0.3.4 (+ a channel recommending 0.3.4) the way
    /// CI will, signed with `key`.
    fn publish(fetch: &MapFetch, key: &TestKey, seq: u64) -> String {
        let mut releases = Vec::new();
        for v in ["0.3.3", "0.3.4"] {
            let m = manifest_json(v, 5, [5, 5], 1, [1, 1]);
            let url = format!("https://example.test/releases/download/v{v}/release-manifest.json");
            fetch.put(&url, &m);
            fetch.put(&format!("{url}.minisig"), &key.sign(m.as_bytes()));
            releases.push(serde_json::json!({"version": v, "manifest": url, "manifest_sha256": sha256_hex(m.as_bytes())}));
        }
        let ch = serde_json::json!({
            "schema": 1, "track": "stable", "sequence": seq,
            "generated_at": "2026-09-30T00:00:00Z", "expires_at": "2026-10-14T00:00:00Z",
            "current": "0.3.4", "recommended": "0.3.4",
            "minimum_supported": {"desktop": "0.3.0"},
            "releases": releases
        })
        .to_string();
        fetch.put(&format!("{BASE}stable.json"), &ch);
        fetch.put(
            &format!("{BASE}stable.json.minisig"),
            &key.sign(ch.as_bytes()),
        );
        ch
    }

    fn req(version: &str) -> CheckRequest {
        CheckRequest {
            update_proto: 1,
            component: Component::Desktop,
            platform: Platform::new("macos", "aarch64", "tauri"),
            installed: Installed::version(Version::parse(version).unwrap()),
            speaks: Speaks::default(),
            phase: UpdatePhase::Idle,
            attempt: None,
        }
    }

    fn git(fetch: MapFetch, key: &TestKey) -> GitUpdateChannel<MapFetch> {
        GitUpdateChannel::new(
            fetch,
            BASE,
            Track::Stable,
            Policy::default(),
            TrustedKeys::from_base64([key.public().as_str()]).unwrap(),
            Box::new(MemorySequenceStore::default()),
        )
        .with_clock(|| NOW)
    }

    #[tokio::test]
    async fn git_mode_offers_a_verified_update() {
        let (fetch, key) = (MapFetch::default(), TestKey::new(7));
        publish(&fetch, &key, 10);
        let out = git(fetch, &key).check(&req("0.3.3")).await.unwrap();
        assert_eq!(out.decision.status, Status::UpdateAvailable);
        assert_eq!(out.decision.source, Source::Git);
        let v = out.verified.expect("verified target");
        assert_eq!(v.version, Version::new(0, 3, 4));
        assert_eq!(
            v.url.as_deref(),
            Some("https://example.test/releases/download/v0.3.4/claude-fleet_0.3.4_aarch64.app.tar.gz")
        );
    }

    /// nightly (S2b): a desktop bundle once a day, the hub on every push. The
    /// day's desktop build is found past a dozen newer hub-only ones.
    #[tokio::test]
    async fn git_mode_finds_the_newest_release_it_can_install_past_ones_it_cannot() {
        let (fetch, key) = (MapFetch::default(), TestKey::new(7));
        let mut releases = Vec::new();
        let mut vs = vec!["0.3.5-dev.1.desktop.gaaaaaaa".to_string()];
        vs.extend((2..14).map(|n| format!("0.3.5-dev.{n}.gbbbbbbb")));
        for v in &vs {
            let mut m: serde_json::Value =
                serde_json::from_str(&manifest_json(v, 5, [5, 5], 1, [1, 1])).unwrap();
            m["release"]["track"] = "nightly".into();
            if !v.contains(".desktop.") {
                m["components"].as_object_mut().unwrap().remove("desktop");
            }
            let m = m.to_string();
            let url = format!("https://example.test/v{v}/release-manifest.json");
            fetch.put(&url, &m);
            fetch.put(&format!("{url}.minisig"), &key.sign(m.as_bytes()));
            releases.push(serde_json::json!({"version": v, "manifest": url, "manifest_sha256": sha256_hex(m.as_bytes())}));
        }
        let newest = vs.last().unwrap();
        let ch = serde_json::json!({
            "schema": 1, "track": "nightly", "sequence": 3,
            "generated_at": "2026-09-30T00:00:00Z", "expires_at": "2026-10-14T00:00:00Z",
            "current": newest, "recommended": newest, "releases": releases
        })
        .to_string();
        fetch.put(&format!("{BASE}nightly.json"), &ch);
        fetch.put(
            &format!("{BASE}nightly.json.minisig"),
            &key.sign(ch.as_bytes()),
        );
        let ch = GitUpdateChannel::new(
            fetch,
            BASE,
            Track::Nightly,
            Policy::default(),
            TrustedKeys::from_base64([key.public().as_str()]).unwrap(),
            Box::new(MemorySequenceStore::default()),
        )
        .with_clock(|| NOW);
        let out = ch.check(&req("0.3.4")).await.unwrap();
        assert_eq!(
            out.decision.status,
            Status::UpdateAvailable,
            "{:?}",
            out.decision.reason
        );
        assert_eq!(
            out.verified.unwrap().version,
            Version::parse("0.3.5-dev.1.desktop.gaaaaaaa").unwrap()
        );
    }

    #[tokio::test]
    async fn git_mode_never_offers_a_tampered_manifest() {
        let (fetch, key) = (MapFetch::default(), TestKey::new(7));
        publish(&fetch, &key, 10);
        let url = "https://example.test/releases/download/v0.3.4/release-manifest.json";
        let evil = manifest_json("0.3.4", 5, [5, 5], 1, [1, 1]).replace("mac-0.3.4", "evil");
        fetch.put(url, &evil);
        let out = git(fetch, &key).check(&req("0.3.3")).await.unwrap();
        assert_eq!(out.decision.status, Status::Unknown);
        assert_eq!(out.decision.reason.code, ReasonCode::NoArtifact);
        assert!(out.verified.is_none());
    }

    #[tokio::test]
    async fn git_mode_refuses_a_replayed_channel() {
        let (fetch, key) = (MapFetch::default(), TestKey::new(7));
        publish(&fetch, &key, 10);
        let ch = git(fetch.clone(), &key);
        ch.check(&req("0.3.3")).await.unwrap();
        publish(&fetch, &key, 9);
        assert!(matches!(
            ch.check(&req("0.3.3")).await,
            Err(UpdateError::Unverified(VerifyError::Replayed {
                seen: 10,
                got: 9
            }))
        ));
    }

    /// A hub that answers with whatever decision the test hands it.
    struct FakeHub(Mutex<Option<Decision>>, Arc<Mutex<Vec<String>>>);

    #[async_trait]
    impl HubTransport for FakeHub {
        async fn post(&self, path: &str, _body: Vec<u8>) -> Result<Vec<u8>, UpdateError> {
            self.1.lock().unwrap().push(path.into());
            let d = self.0.lock().unwrap().clone();
            Ok(d.map(|d| serde_json::to_vec(&d).unwrap())
                .unwrap_or_else(|| b"{}".to_vec()))
        }
    }

    /// What an honest hub would answer: the Git channel's own decision,
    /// evidence attached.
    async fn honest_decision(key: &TestKey) -> Decision {
        let fetch = MapFetch::default();
        publish(&fetch, key, 10);
        let mut d = git(fetch, key).check(&req("0.3.3")).await.unwrap().decision;
        d.source = Source::Hub;
        d.mode = Mode::Automatic;
        d
    }

    fn hub(d: Decision, key: &TestKey) -> HubUpdateChannel<FakeHub> {
        HubUpdateChannel::new(
            FakeHub(Mutex::new(Some(d)), Arc::default()),
            TrustedKeys::from_base64([key.public().as_str()]).unwrap(),
            Box::new(MemorySequenceStore::default()),
        )
        .with_clock(|| NOW)
    }

    #[tokio::test]
    async fn hub_mode_obeys_a_verified_decision() {
        let key = TestKey::new(7);
        let d = honest_decision(&key).await;
        let out = hub(d, &key).check(&req("0.3.3")).await.unwrap();
        assert_eq!(out.decision.mode, Mode::Automatic);
        assert_eq!(out.verified.unwrap().version, Version::new(0, 3, 4));
    }

    #[tokio::test]
    async fn hub_mode_refuses_a_swapped_artifact_or_missing_evidence() {
        let key = TestKey::new(7);
        let mut d = honest_decision(&key).await;
        if let Some(t) = d.target.as_mut() {
            t.artifact = crate::manifest::Artifact::Tauri {
                platform: "macos-aarch64".into(),
                variant: None,
                name: "claude-fleet_0.3.4_aarch64.app.tar.gz".into(),
                sha256: "evil".into(),
                size: 1,
                tauri_signature: "sig".into(),
            };
        }
        assert_eq!(
            hub(d.clone(), &key).check(&req("0.3.3")).await,
            Err(UpdateError::Unverified(VerifyError::ArtifactMismatch))
        );

        let mut bare = honest_decision(&key).await;
        bare.target.as_mut().unwrap().evidence = None;
        assert_eq!(
            hub(bare, &key).check(&req("0.3.3")).await,
            Err(UpdateError::Unverified(VerifyError::MissingEvidence))
        );
    }

    #[tokio::test]
    async fn hub_mode_refuses_a_target_below_the_signed_floor() {
        // A compromised hub pointing a desktop at a release the channel's
        // signed minimum rules out (U6).
        let key = TestKey::new(7);
        let mut d = honest_decision(&key).await;
        let ev = d.target.as_mut().unwrap().evidence.as_mut().unwrap();
        let raised = ev
            .channel
            .replace("\"desktop\":\"0.3.0\"", "\"desktop\":\"0.3.5\"");
        ev.channel_sig = key.sign(raised.as_bytes());
        ev.channel = raised;
        assert_eq!(
            hub(d, &key).check(&req("0.3.3")).await,
            Err(UpdateError::Unverified(VerifyError::NotPermitted(
                Version::new(0, 3, 4)
            )))
        );
    }

    #[tokio::test]
    async fn hub_mode_refuses_a_target_from_an_expired_channel() {
        // A compromised hub freezing the channel: an old, correctly signed
        // document whose `expires_at` has passed still names a target, so a
        // later withdrawal or raised floor would never reach this client.
        let key = TestKey::new(7);
        let d = honest_decision(&key).await;
        let expired = crate::time::parse_rfc3339("2026-10-14T00:00:01Z").unwrap();
        assert_eq!(
            hub(d.clone(), &key)
                .with_clock(move || expired)
                .check(&req("0.3.3"))
                .await,
            Err(UpdateError::Unverified(VerifyError::Stale))
        );
        assert!(hub(d, &key).check(&req("0.3.3")).await.is_ok());
    }

    #[tokio::test]
    async fn hub_mode_refuses_an_answer_for_another_component() {
        let key = TestKey::new(7);
        let mut d = honest_decision(&key).await;
        d.component = Component::Hub;
        assert!(matches!(
            hub(d, &key).check(&req("0.3.3")).await,
            Err(UpdateError::Protocol(_))
        ));
    }

    #[tokio::test]
    async fn hub_mode_reports_to_the_report_route() {
        let key = TestKey::new(7);
        let paths = Arc::new(Mutex::new(Vec::new()));
        let ch = HubUpdateChannel::new(
            FakeHub(Mutex::new(None), paths.clone()),
            TrustedKeys::from_base64([key.public().as_str()]).unwrap(),
            Box::new(MemorySequenceStore::default()),
        );
        ch.report(&Report {
            update_proto: 1,
            component: Component::Desktop,
            installed: Installed::version(Version::new(0, 3, 4)),
            phase: UpdatePhase::Success,
            attempt: Some("01J9".into()),
            from: Some(Version::new(0, 3, 3)),
            to: Some(Version::new(0, 3, 4)),
            detail: serde_json::Value::Null,
            error: None,
        })
        .await
        .unwrap();
        assert_eq!(*paths.lock().unwrap(), vec!["/update/report".to_string()]);
    }
}
