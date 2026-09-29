//! `service::update` against real signed documents (fleet-update's testkit),
//! an in-memory store and a scripted channel.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use fleet_update::testkit::{manifest_json, TestKey};
use fleet_update::verify::{sha256_hex, verify_target};
use fleet_update::wire::{CheckRequest, Installed, Report, Speaks, Status};
use fleet_update::{
    Component, Fetch, Platform, TrustedKeys, UpdateError, UpdatePhase, Version, Window,
};

use super::*;
use crate::mcp::auth::{Caller, ClientRef, TokenMode};
use crate::store::Store;

const NOW: i64 = 1_790_763_120; // 2026-09-30T10:12:00Z
const BASE: &str = "https://raw.githubusercontent.com/martin-janci/claude-fleet/update-channels/";

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
    async fn get(&self, url: &str, _max: u64) -> Result<Vec<u8>, UpdateError> {
        self.0
            .lock()
            .unwrap()
            .get(url)
            .cloned()
            .ok_or_else(|| UpdateError::Http {
                status: 404,
                body: url.into(),
            })
    }
}

/// Publish 0.3.3 and 0.3.4 (stable, recommending 0.3.4, rollback 0.3.3)
/// the way CI will.
fn publish(fetch: &MapFetch, key: &TestKey, seq: u64) {
    let hub_contract = crate::wire_contract::CONTRACT_REVISION;
    let mut releases = Vec::new();
    for v in ["0.3.3", "0.3.4"] {
        let m = manifest_json(v, hub_contract, [hub_contract, hub_contract], 1, [1, 1]);
        let url = format!("https://github.com/martin-janci/claude-fleet/releases/download/v{v}/release-manifest.json");
        fetch.put(&url, &m);
        fetch.put(&format!("{url}.minisig"), &key.sign(m.as_bytes()));
        releases.push(serde_json::json!({"version": v, "manifest": url, "manifest_sha256": sha256_hex(m.as_bytes())}));
    }
    let ch = serde_json::json!({
        "schema": 1, "track": "stable", "sequence": seq,
        "generated_at": "2026-09-30T00:00:00Z", "expires_at": "2026-10-14T00:00:00Z",
        "current": "0.3.4", "recommended": "0.3.4", "rollback": "0.3.3",
        "minimum_supported": {"desktop": "0.3.0", "hub": "0.3.0"},
        "releases": releases
    })
    .to_string();
    fetch.put(&format!("{BASE}stable.json"), &ch);
    fetch.put(
        &format!("{BASE}stable.json.minisig"),
        &key.sign(ch.as_bytes()),
    );
}

fn keys(k: &TestKey) -> TrustedKeys {
    TrustedKeys::from_base64([k.public().as_str()]).unwrap()
}

fn client(id: i64, mode: TokenMode, org: Option<i64>) -> Caller {
    Caller {
        host_alias: None,
        client: Some(ClientRef {
            id,
            name: format!("c{id}"),
            trusted: false,
            org_id: org,
        }),
        mode,
    }
}

fn host(alias: &str) -> Caller {
    Caller {
        host_alias: Some(alias.into()),
        client: None,
        mode: TokenMode::Full,
    }
}

fn desktop_req(version: &str) -> CheckRequest {
    let c = crate::wire_contract::CONTRACT_REVISION;
    CheckRequest {
        update_proto: 1,
        component: Component::Desktop,
        platform: Platform::new("macos", "aarch64", "tauri"),
        installed: Installed::version(Version::parse(version).unwrap()),
        speaks: Speaks {
            contract_accepts: Some(Window::new(c, c)),
            agent_proto: None,
        },
        phase: UpdatePhase::Idle,
        attempt: None,
    }
}

fn hub_req(version: &str) -> CheckRequest {
    CheckRequest {
        update_proto: 1,
        component: Component::Hub,
        platform: Platform::new("linux", "x86_64", "oci"),
        installed: Installed::version(Version::parse(version).unwrap()),
        speaks: Speaks::default(),
        phase: UpdatePhase::Idle,
        attempt: None,
    }
}

async fn published_store(key: &TestKey) -> (Mutex<Store>, MapFetch) {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let fetch = MapFetch::default();
    publish(&fetch, key, 10);
    let o = refresh(&store, &fetch, BASE, &keys(key), NOW)
        .await
        .unwrap();
    assert_eq!((o.sequence, o.fresh, o.manifests), (10, true, 2));
    (store, fetch)
}

#[tokio::test]
async fn a_client_is_offered_a_decision_it_can_verify_itself() {
    let key = TestKey::new(9);
    let (store, _) = published_store(&key).await;
    let req = desktop_req("0.3.3");
    let d = check(
        &store,
        &client(1, TokenMode::Full, None),
        &req,
        &keys(&key),
        NOW,
    )
    .unwrap();
    assert_eq!(d.status, Status::UpdateAvailable, "{:?}", d.reason);
    assert_eq!(d.source, Source::Hub);
    // The client's own check, from the relayed evidence alone.
    let v = verify_target(&d, &keys(&key), &req.platform, 0, NOW)
        .unwrap()
        .unwrap();
    assert_eq!(v.version, Version::new(0, 3, 4));
    // And the hub recorded what the client runs.
    let o = lock(&store)
        .unwrap()
        .update_observed("client:1")
        .unwrap()
        .unwrap();
    assert_eq!(
        (o.version.as_str(), o.last_checked_at),
        ("0.3.3", Some(NOW))
    );
}

#[tokio::test]
async fn nothing_is_offered_without_a_trusted_key() {
    let key = TestKey::new(9);
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let fetch = MapFetch::default();
    publish(&fetch, &key, 10);
    let none = TrustedKeys::from_base64([]).unwrap();
    let e = refresh(&store, &fetch, BASE, &none, NOW).await.unwrap_err();
    assert_eq!(e.code, codes::E_UPDATE_UNVERIFIED);
    let d = check(
        &store,
        &client(1, TokenMode::Full, None),
        &desktop_req("0.3.3"),
        &none,
        NOW,
    )
    .unwrap();
    assert_eq!(d.status, Status::Unknown);
    let st = status(&store, &Caller::master(), &none, NOW).unwrap();
    assert!(st.channel.is_none());
    assert_eq!(st.last_refresh.unwrap()["ok"], false);
}

#[tokio::test]
async fn a_replayed_channel_is_refused_and_the_cache_kept() {
    let key = TestKey::new(9);
    let (store, fetch) = published_store(&key).await;
    publish(&fetch, &key, 9);
    let e = refresh(&store, &fetch, BASE, &keys(&key), NOW)
        .await
        .unwrap_err();
    assert_eq!(e.code, codes::E_UPDATE_UNVERIFIED);
    let st = status(&store, &Caller::master(), &keys(&key), NOW).unwrap();
    assert_eq!(st.channel.unwrap().sequence, 10);
}

#[tokio::test]
async fn a_tampered_cache_row_is_not_a_channel() {
    let key = TestKey::new(9);
    let (store, _) = published_store(&key).await;
    {
        let s = lock(&store).unwrap();
        let mut row = s.update_doc("channel", "stable").unwrap().unwrap();
        row.body = row
            .body
            .replace("\"recommended\":\"0.3.4\"", "\"recommended\":\"0.3.3\"");
        s.put_update_doc(&row).unwrap();
    }
    let d = check(
        &store,
        &client(1, TokenMode::Full, None),
        &desktop_req("0.3.3"),
        &keys(&key),
        NOW,
    )
    .unwrap();
    assert_eq!(d.status, Status::Unknown);
}

#[test]
fn identity_comes_from_the_token() {
    let upd = client(7, TokenMode::Updater, None);
    assert_eq!(identity(&upd).unwrap().target, "hub:self");
    assert_eq!(identity(&upd).unwrap().allowed, &[Component::Hub]);
    assert_eq!(identity(&host("box")).unwrap().target, "agent:box");
    assert_eq!(
        identity(&client(3, TokenMode::Readonly, None))
            .unwrap()
            .target,
        "client:3"
    );
    let m = identity(&Caller::master()).unwrap();
    assert!(!m.may_report && m.allowed.len() == 5);
    assert_eq!(
        identity(&client(4, TokenMode::Peer, None))
            .unwrap_err()
            .code,
        codes::E_FORBIDDEN
    );
}

#[tokio::test]
async fn a_caller_asks_only_for_its_own_component() {
    let key = TestKey::new(9);
    let (store, _) = published_store(&key).await;
    // A phone's token cannot speak for the hub; the updater's cannot speak
    // for a desktop.
    let e = check(
        &store,
        &client(1, TokenMode::Full, None),
        &hub_req("0.3.3"),
        &keys(&key),
        NOW,
    )
    .unwrap_err();
    assert_eq!(e.code, codes::E_FORBIDDEN);
    let e = check(
        &store,
        &client(2, TokenMode::Updater, None),
        &desktop_req("0.3.3"),
        &keys(&key),
        NOW,
    )
    .unwrap_err();
    assert_eq!(e.code, codes::E_FORBIDDEN);
    let d = check(
        &store,
        &client(2, TokenMode::Updater, None),
        &hub_req("0.3.3"),
        &keys(&key),
        NOW,
    )
    .unwrap();
    assert_eq!(d.status, Status::UpdateAvailable);
    // The master may look without being recorded.
    check(
        &store,
        &Caller::master(),
        &hub_req("0.3.3"),
        &keys(&key),
        NOW,
    )
    .unwrap();
    assert!(lock(&store)
        .unwrap()
        .update_observed("operator")
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn reports_are_recorded_once() {
    let key = TestKey::new(9);
    let (store, _) = published_store(&key).await;
    let r = Report {
        update_proto: 1,
        component: Component::Hub,
        installed: Installed::version(Version::new(0, 3, 4)),
        phase: UpdatePhase::Success,
        attempt: Some("01J9".into()),
        from: Some(Version::new(0, 3, 3)),
        to: Some(Version::new(0, 3, 4)),
        detail: serde_json::json!({"soak_secs": 120}),
        error: None,
    };
    let upd = client(2, TokenMode::Updater, None);
    assert!(report(&store, &upd, &r, NOW).unwrap());
    assert!(
        !report(&store, &upd, &r, NOW + 1).unwrap(),
        "a replayed report adds nothing"
    );
    let s = lock(&store).unwrap();
    assert_eq!(
        s.update_observed("hub:self").unwrap().unwrap().phase,
        "success"
    );
    assert_eq!(s.update_events("hub:self", 10).unwrap().len(), 1);
    drop(s);
    assert_eq!(
        report(&store, &Caller::master(), &r, NOW).unwrap_err().code,
        codes::E_FORBIDDEN
    );
}

#[tokio::test]
async fn a_pin_below_installed_rolls_the_hub_back() {
    let key = TestKey::new(9);
    let (store, _) = published_store(&key).await;
    pin(
        &store,
        "hub",
        "",
        "0.3.3",
        false,
        Some("0.3.4 regressed".into()),
        NOW,
    )
    .unwrap();
    let d = check(
        &store,
        &client(2, TokenMode::Updater, None),
        &hub_req("0.3.4"),
        &keys(&key),
        NOW,
    )
    .unwrap();
    assert_eq!(d.status, Status::Rollback);
    assert_eq!(d.target.as_ref().unwrap().version, Version::new(0, 3, 3));
    assert!(unpin(&store, "hub", "").unwrap());
    let d = check(
        &store,
        &client(2, TokenMode::Updater, None),
        &hub_req("0.3.4"),
        &keys(&key),
        NOW,
    )
    .unwrap();
    assert_eq!(d.status, Status::UpToDate);
}

#[test]
fn pins_are_validated() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    assert_eq!(
        pin(&store, "fridge", "", "0.3.3", false, None, NOW)
            .unwrap_err()
            .code,
        codes::E_INVALID
    );
    assert_eq!(
        pin(&store, "hub", "", "latest", false, None, NOW)
            .unwrap_err()
            .code,
        codes::E_INVALID
    );
    assert_eq!(
        pin(&store, "agent", "client:1", "0.3.3", false, None, NOW)
            .unwrap_err()
            .code,
        codes::E_INVALID
    );
    pin(&store, "agent", "agent:box", "0.3.3", true, None, NOW).unwrap();
    pin(&store, "desktop", "client:4", "0.3.3", false, None, NOW).unwrap();
}

#[tokio::test]
async fn status_rolls_up_and_a_scoped_caller_sees_only_itself() {
    let key = TestKey::new(9);
    let (store, _) = published_store(&key).await;
    let k = keys(&key);
    check(
        &store,
        &client(1, TokenMode::Full, None),
        &desktop_req("0.3.3"),
        &k,
        NOW,
    )
    .unwrap();
    check(
        &store,
        &client(2, TokenMode::Full, None),
        &desktop_req("0.3.4"),
        &k,
        NOW,
    )
    .unwrap();
    let agent_req = CheckRequest {
        component: Component::Agent,
        platform: Platform::new("linux", "x86_64", "tarball"),
        speaks: Speaks {
            contract_accepts: None,
            agent_proto: Some(1),
        },
        ..hub_req("0.3.3")
    };
    check(&store, &host("box"), &agent_req, &k, NOW).unwrap();

    let st = status(&store, &Caller::master(), &k, NOW).unwrap();
    assert_eq!(st.targets.len(), 3);
    let desktop = st
        .components
        .iter()
        .find(|c| c.component == "desktop")
        .unwrap();
    assert_eq!(
        (desktop.total, desktop.up_to_date, desktop.available),
        (2, 1, 1)
    );
    assert_eq!(st.channel.as_ref().unwrap().recommended, "0.3.4");

    let own = status(&store, &host("box"), &k, NOW).unwrap();
    assert_eq!(own.targets.len(), 1);
    assert_eq!(own.targets[0].target, "agent:box");
    assert_eq!(own.targets[0].status, Status::UpdateAvailable);
    let bound = status(&store, &client(1, TokenMode::Full, Some(3)), &k, NOW).unwrap();
    assert_eq!(bound.targets.len(), 1);
    assert!(bound.pins.is_empty());
}

#[tokio::test]
async fn refresh_fetches_only_what_changed() {
    let key = TestKey::new(9);
    let (store, fetch) = published_store(&key).await;
    publish(&fetch, &key, 11);
    let o = refresh(&store, &fetch, BASE, &keys(&key), NOW)
        .await
        .unwrap();
    assert_eq!((o.sequence, o.fetched, o.manifests), (11, 0, 2));
}

#[test]
fn a_newer_update_proto_is_refused() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let mut req = desktop_req("0.3.3");
    req.update_proto = 2;
    let none = TrustedKeys::from_base64([]).unwrap();
    assert_eq!(
        check(&store, &client(1, TokenMode::Full, None), &req, &none, NOW)
            .unwrap_err()
            .code,
        codes::E_UNSUPPORTED
    );
}

// ── Git mode (S3): a standalone hub reads the channel itself ──

fn git(seen: u64) -> GitCheck {
    GitCheck {
        base_url: BASE.into(),
        track: Track::Stable,
        policy: Policy::default(),
        seen,
    }
}

#[tokio::test]
async fn a_standalone_hub_finds_its_update_in_the_channel() {
    let key = TestKey::new(9);
    let fetch = MapFetch::default();
    publish(&fetch, &key, 10);
    let o = git_check(fetch.clone(), git(0), keys(&key), &hub_req("0.3.3"), NOW)
        .await
        .unwrap();
    assert_eq!(o.decision.status, Status::UpdateAvailable);
    assert_eq!(o.decision.source, Source::Git);
    let t = o.decision.target.as_ref().unwrap();
    assert_eq!(t.version, Version::new(0, 3, 4));
    assert!(
        t.evidence.is_some(),
        "a Git decision carries its own evidence"
    );
    let v = o
        .verified
        .expect("the target is proven against the signed documents");
    assert_eq!(v.version, Version::new(0, 3, 4));

    let o = git_check(fetch, git(0), keys(&key), &hub_req("0.3.4"), NOW)
        .await
        .unwrap();
    assert_eq!(o.decision.status, Status::UpToDate);
    assert!(o.verified.is_none());
}

#[tokio::test]
async fn a_standalone_check_trusts_only_the_release_key_and_newer_channels() {
    let key = TestKey::new(9);
    let fetch = MapFetch::default();
    publish(&fetch, &key, 10);
    let other = keys(&TestKey::new(3));
    let e = git_check(fetch.clone(), git(0), other, &hub_req("0.3.3"), NOW)
        .await
        .unwrap_err();
    assert_eq!(e.code, codes::E_UPDATE_UNVERIFIED);
    let e = git_check(fetch, git(11), keys(&key), &hub_req("0.3.3"), NOW)
        .await
        .unwrap_err();
    assert_eq!(
        e.code,
        codes::E_UPDATE_UNVERIFIED,
        "an older channel is a replay"
    );

    let e = git_check(
        MapFetch::default(),
        git(0),
        keys(&key),
        &hub_req("0.3.3"),
        NOW,
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, codes::E_HUB_UNAVAILABLE);
    assert!(e.message.contains("no stable channel"), "{}", e.message);
}

#[tokio::test]
async fn a_standalone_check_takes_the_hubs_own_settings_and_pin() {
    let key = TestKey::new(9);
    let (store, fetch) = published_store(&key).await;
    pin(&store, "hub", "", "0.3.3", false, None, NOW).unwrap();
    let setup = {
        let s = lock(&store).unwrap();
        GitCheck::from_store(Some(&s), Component::Hub, "hub:self", None).unwrap()
    };
    assert_eq!((setup.track, setup.seen), (Track::Stable, 10));
    let mut setup_at_base = setup.clone();
    setup_at_base.base_url = BASE.into();
    let o = git_check(fetch, setup_at_base, keys(&key), &hub_req("0.3.4"), NOW)
        .await
        .unwrap();
    assert_eq!(o.decision.status, Status::Rollback);

    let bare = GitCheck::from_store(None, Component::Hub, "hub:self", Some(Track::Beta)).unwrap();
    assert_eq!((bare.track, bare.seen), (Track::Beta, 0));
    assert_eq!(bare.policy.mode, Mode::Notify);
}

#[test]
fn the_hub_asks_for_itself() {
    let r = hub_self_request(Installed::version(Version::new(0, 4, 1)));
    assert_eq!(
        (r.component, r.update_proto),
        (Component::Hub, UPDATE_PROTO)
    );
    assert_eq!(r.platform.os, std::env::consts::OS);
}
