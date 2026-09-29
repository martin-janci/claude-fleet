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
    // A well-formed target that does not exist is refused too.
    assert_eq!(
        pin(&store, "agent", "agent:box", "0.3.3", true, None, NOW)
            .unwrap_err()
            .code,
        codes::E_NOTFOUND
    );
    assert_eq!(
        pin(&store, "desktop", "client:999", "0.3.3", false, None, NOW)
            .unwrap_err()
            .code,
        codes::E_NOTFOUND
    );
    let (client_id, revoked_id) = {
        let s = lock(&store).unwrap();
        s.insert_host("box", None).unwrap();
        let live = s.insert_client_token("phone", "sha-a", "full").unwrap().id;
        let gone = s.insert_client_token("old", "sha-b", "full").unwrap().id;
        s.revoke_client_token("old").unwrap();
        (live, gone)
    };
    pin(&store, "agent", "agent:box", "0.3.3", true, None, NOW).unwrap();
    pin(
        &store,
        "desktop",
        &format!("client:{client_id}"),
        "0.3.3",
        false,
        None,
        NOW,
    )
    .unwrap();
    assert_eq!(
        pin(
            &store,
            "desktop",
            &format!("client:{revoked_id}"),
            "0.3.3",
            false,
            None,
            NOW
        )
        .unwrap_err()
        .code,
        codes::E_NOTFOUND
    );
    // Unpinning a target that is gone stays possible.
    assert!(!unpin(&store, "desktop", "client:999").unwrap());
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

#[test]
fn a_hub_link_is_refused_before_its_update_proto_is_read() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let peer = client(4, TokenMode::Peer, None);
    let none = TrustedKeys::from_base64([]).unwrap();
    for proto in [0, 99] {
        let mut req = desktop_req("0.3.3");
        req.update_proto = proto;
        assert_eq!(
            check(&store, &peer, &req, &none, NOW).unwrap_err().code,
            codes::E_FORBIDDEN
        );
        let mut r = desktop_report("a1", UpdatePhase::Downloading, "0.3.3");
        r.update_proto = proto;
        assert_eq!(
            report(&store, &peer, &r, NOW).unwrap_err().code,
            codes::E_FORBIDDEN
        );
    }
}

fn desktop_report(attempt: &str, phase: UpdatePhase, version: &str) -> Report {
    Report {
        update_proto: 1,
        component: Component::Desktop,
        installed: Installed::version(Version::parse(version).unwrap()),
        phase,
        attempt: Some(attempt.into()),
        from: None,
        to: None,
        detail: serde_json::Value::Null,
        error: None,
    }
}

#[test]
fn a_late_or_replayed_report_never_overwrites_a_newer_attempt() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let c = client(1, TokenMode::Full, None);
    let observed = || {
        let o = lock(&store)
            .unwrap()
            .update_observed("client:1")
            .unwrap()
            .unwrap();
        (o.attempt.unwrap(), o.phase, o.version)
    };
    let a1 = desktop_report("a1", UpdatePhase::Installing, "0.3.3");
    assert!(report(&store, &c, &a1, NOW).unwrap());
    for (phase, at) in [
        (UpdatePhase::Downloading, NOW + 9),
        (UpdatePhase::Success, NOW + 10),
    ] {
        assert!(report(&store, &c, &desktop_report("a2", phase, "0.3.4"), at).unwrap());
    }
    let a2 = || ("a2".to_string(), "success".to_string(), "0.3.4".to_string());
    // A replay of a1's report: nothing recorded, nothing overwritten.
    assert!(!report(&store, &c, &a1, NOW + 20).unwrap());
    assert_eq!(observed(), a2());
    // A plain duplicate of the newest report.
    assert!(!report(
        &store,
        &c,
        &desktop_report("a2", UpdatePhase::Success, "0.3.4"),
        NOW + 21
    )
    .unwrap());
    assert_eq!(observed(), a2());
    // A late, never-seen phase of the older attempt is logged, not state.
    assert!(report(
        &store,
        &c,
        &desktop_report("a1", UpdatePhase::Failed, "0.3.3"),
        NOW + 22
    )
    .unwrap());
    assert_eq!(observed(), a2());
    assert_eq!(
        lock(&store)
            .unwrap()
            .update_events("client:1", 10)
            .unwrap()
            .len(),
        4
    );
    // A new attempt, opened at `downloading`, is state again.
    assert!(report(
        &store,
        &c,
        &desktop_report("a3", UpdatePhase::Downloading, "0.3.4"),
        NOW + 30
    )
    .unwrap());
    assert_eq!(observed().0, "a3");
}

#[test]
fn a_late_older_attempt_stays_history_after_an_attempt_less_report() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let c = client(1, TokenMode::Full, None);
    let observed = || {
        let o = lock(&store)
            .unwrap()
            .update_observed("client:1")
            .unwrap()
            .unwrap();
        (o.attempt, o.phase, o.version)
    };
    // a2 opens at `downloading`, so it replaces a1 as state and its
    // success is the observed row before the attempt-less report arrives.
    let steps = [
        (desktop_report("a1", UpdatePhase::Installing, "0.3.3"), NOW),
        (
            desktop_report("a2", UpdatePhase::Downloading, "0.3.4"),
            NOW + 9,
        ),
        (
            desktop_report("a2", UpdatePhase::Success, "0.3.4"),
            NOW + 10,
        ),
    ];
    for (r, at) in &steps {
        assert!(report(&store, &c, r, *at).unwrap());
    }
    assert_eq!(
        observed(),
        (Some("a2".into()), "success".into(), "0.3.4".into())
    );
    assert!(report(
        &store,
        &c,
        &Report {
            attempt: None,
            ..desktop_report("", UpdatePhase::Checking, "0.3.4")
        },
        NOW + 20
    )
    .unwrap());
    assert_eq!(observed(), (None, "checking".into(), "0.3.4".into()));
    // The observed row no longer names a2, but the log still orders a1
    // before it: the late failure is logged, not state.
    assert!(report(
        &store,
        &c,
        &desktop_report("a1", UpdatePhase::Failed, "0.3.3"),
        NOW + 30
    )
    .unwrap());
    let (_, phase, version) = observed();
    assert_eq!(version, "0.3.4");
    assert_ne!(phase, "failed");
    assert_eq!(observed(), (None, "checking".into(), "0.3.4".into()));
    // A new attempt is still state.
    assert!(report(
        &store,
        &c,
        &desktop_report("a3", UpdatePhase::Downloading, "0.3.4"),
        NOW + 40
    )
    .unwrap());
    assert_eq!(observed().0.as_deref(), Some("a3"));
}

#[test]
fn a_pruned_older_attempt_reported_late_stays_history() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let c = client(1, TokenMode::Full, None);
    for (phase, at) in [
        (UpdatePhase::Downloading, NOW),
        (UpdatePhase::Installing, NOW + 1),
    ] {
        assert!(report(&store, &c, &desktop_report("a1", phase, "0.3.3"), at).unwrap());
    }
    // Fill the log past the per-target cap so that a1's events are dropped.
    let cap = crate::store::UPDATE_EVENTS_PER_TARGET;
    {
        let s = lock(&store).unwrap();
        for i in 0..cap {
            assert!(s
                .insert_update_event(
                    "client:1",
                    Some(&format!("filler{i}")),
                    "checking",
                    None,
                    None,
                    None,
                    None,
                    NOW + 10 + i64::from(i),
                )
                .unwrap());
        }
    }
    let after = NOW + 10 + i64::from(cap);
    for (phase, at) in [
        (UpdatePhase::Downloading, after),
        (UpdatePhase::Success, after + 1),
    ] {
        assert!(report(&store, &c, &desktop_report("a2", phase, "0.3.4"), at).unwrap());
    }
    {
        let s = lock(&store).unwrap();
        assert!(!s.update_attempt_kept("client:1", "a1").unwrap());
        assert!(s.update_attempt_kept("client:1", "a2").unwrap());
    }
    // A late or retried failure of the forgotten a1 is logged, not state.
    assert!(report(
        &store,
        &c,
        &desktop_report("a1", UpdatePhase::Failed, "0.3.3"),
        after + 10
    )
    .unwrap());
    let s = lock(&store).unwrap();
    assert!(s.update_attempt_kept("client:1", "a1").unwrap());
    let o = s.update_observed("client:1").unwrap().unwrap();
    assert_eq!(o.attempt.as_deref(), Some("a2"));
    assert_eq!(o.version, "0.3.4");
    assert_ne!(o.phase, "failed");
}

#[test]
fn a_report_without_an_attempt_is_always_the_observed_state() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let c = client(1, TokenMode::Full, None);
    let observed = || {
        let o = lock(&store)
            .unwrap()
            .update_observed("client:1")
            .unwrap()
            .unwrap();
        (o.attempt, o.phase, o.version)
    };
    let checking = |version: &str| Report {
        attempt: None,
        ..desktop_report("", UpdatePhase::Checking, version)
    };
    assert!(report(&store, &c, &checking("0.3.3"), NOW).unwrap());
    assert_eq!(observed(), (None, "checking".into(), "0.3.3".into()));
    for (phase, version, at) in [
        (UpdatePhase::Downloading, "0.3.3", NOW + 10),
        (UpdatePhase::Success, "0.3.4", NOW + 20),
    ] {
        assert!(report(&store, &c, &desktop_report("a1", phase, version), at).unwrap());
    }
    assert_eq!(
        observed(),
        (Some("a1".into()), "success".into(), "0.3.4".into())
    );
    // An attempt-less report is never a replay (migration 087): the second
    // `checking` is logged too, and it is the latest state.
    assert!(report(&store, &c, &checking("0.3.4"), NOW + 30).unwrap());
    assert_eq!(observed(), (None, "checking".into(), "0.3.4".into()));
    assert_eq!(
        lock(&store)
            .unwrap()
            .update_events("client:1", 10)
            .unwrap()
            .len(),
        4
    );
}

#[tokio::test]
async fn a_new_track_or_interval_wakes_the_refresh_tick() {
    let s = Store::open_in_memory().unwrap();
    settings::set(&s, settings::UPDATE_TRACK, "beta").unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), REFRESH_WAKE.notified())
        .await
        .expect("a track change wakes the tick");
    assert_eq!(track(&s), Track::Beta);
    settings::set(&s, settings::UPDATE_CHECK_INTERVAL_SECS, "7200").unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), REFRESH_WAKE.notified())
        .await
        .expect("an interval change wakes the tick");
    assert_eq!(check_interval_secs(&s), 7200);
}

#[test]
fn nightly_is_not_offered_until_it_is_published() {
    assert_eq!(
        settings::validate(settings::UPDATE_TRACK, "nightly")
            .unwrap_err()
            .code,
        codes::E_INVALID
    );
    let s = Store::open_in_memory().unwrap();
    // A value stored before nightly was withdrawn reads as the default.
    s.set_setting(settings::UPDATE_TRACK, "nightly").unwrap();
    assert_eq!(track(&s), Track::Stable);
}

#[test]
fn the_hub_records_itself_and_a_new_version_clears_the_old_attempt() {
    let s = Store::open_in_memory().unwrap();
    let me = |v: &str| HubSelf {
        version: v.into(),
        commit: "abc123".into(),
        build_id: "b42".into(),
    };
    // First start: no row yet.
    record_hub_self(&s, &me("0.4.1"), NOW).unwrap();
    let o = s.update_observed("hub:self").unwrap().unwrap();
    assert_eq!(
        (o.component.as_str(), o.version.as_str(), o.phase.as_str()),
        ("hub", "0.4.1", "idle")
    );
    assert_eq!(o.commit_sha.as_deref(), Some("abc123"));
    assert_eq!(o.build_id.as_deref(), Some("b42"));
    let platform: Platform = serde_json::from_str(o.platform.as_deref().unwrap()).unwrap();
    assert_eq!(platform, hub_platform());
    assert!(o.attempt.is_none() && o.last_error.is_none());

    // The updater reports a failed attempt on this version.
    s.upsert_update_observed(&UpdateObservedRow {
        phase: "failed".into(),
        attempt: Some("a1".into()),
        last_error: Some("ready timeout".into()),
        digest: Some("sha256:d".into()),
        ..o
    })
    .unwrap();
    // A restart on the same version keeps what the updater said.
    record_hub_self(&s, &me("0.4.1"), NOW + 60).unwrap();
    let o = s.update_observed("hub:self").unwrap().unwrap();
    assert_eq!(
        (
            o.phase.as_str(),
            o.attempt.as_deref(),
            o.last_error.as_deref()
        ),
        ("failed", Some("a1"), Some("ready timeout"))
    );
    assert_eq!(o.digest.as_deref(), Some("sha256:d"));
    assert_eq!(o.reported_at, NOW + 60);

    // A new binary: the old attempt no longer describes it.
    record_hub_self(&s, &me("0.4.2"), NOW + 120).unwrap();
    let o = s.update_observed("hub:self").unwrap().unwrap();
    assert_eq!((o.version.as_str(), o.phase.as_str()), ("0.4.2", "idle"));
    assert!(o.attempt.is_none() && o.last_error.is_none() && o.digest.is_none());
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

// ── update:changed (S4b) ──

#[tokio::test]
async fn the_update_picture_emits_ids_only_when_it_moves() {
    let bus = Arc::new(crate::events::RecordingEventBus::new());
    let store = Mutex::new(Store::open_with_bus_in_memory(bus.clone()).unwrap());
    let key = TestKey::new(9);
    let fetch = MapFetch::default();
    publish(&fetch, &key, 10);
    refresh(&store, &fetch, BASE, &keys(&key), NOW)
        .await
        .unwrap();
    assert_eq!(bus.take(), vec!["update:changed:channel:".to_string()]);
    refresh(&store, &fetch, BASE, &keys(&key), NOW)
        .await
        .unwrap();
    assert!(bus.take().is_empty(), "an unchanged channel is silent");

    // A pin names a live client (the pin hardening), so pair one first.
    let id = lock(&store)
        .unwrap()
        .insert_client_token("desk", "sha-desk", "full")
        .unwrap()
        .id;
    bus.take();
    let observed = format!("update:changed:observed:client:{id}");
    let c = client(id, TokenMode::Full, None);
    check(&store, &c, &desktop_req("0.3.3"), &keys(&key), NOW).unwrap();
    assert_eq!(bus.take(), vec![observed.clone()]);
    check(&store, &c, &desktop_req("0.3.3"), &keys(&key), NOW + 60).unwrap();
    assert!(bus.take().is_empty(), "a routine re-check is silent");
    check(&store, &c, &desktop_req("0.3.4"), &keys(&key), NOW + 120).unwrap();
    assert_eq!(bus.take(), vec![observed]);

    let target = format!("client:{id}");
    pin(&store, "desktop", &target, "0.3.3", false, None, NOW).unwrap();
    pin(&store, "hub", "", "0.3.3", false, None, NOW).unwrap();
    assert!(unpin(&store, "hub", "").unwrap());
    assert!(!unpin(&store, "hub", "").unwrap());
    assert_eq!(
        bus.take(),
        vec![
            format!("update:changed:pin:{target}"),
            "update:changed:pin:".to_string(),
            "update:changed:pin:".to_string(),
        ]
    );
}

// ── fleet_health.updates (S4b) ──

#[tokio::test]
async fn fleet_health_names_the_targets_that_need_a_person() {
    let key = TestKey::new(9);
    let (store, _) = published_store(&key).await;
    let k = keys(&key);
    // A desktop below the signed minimum (0.3.0) is blocked.
    check(
        &store,
        &client(1, TokenMode::Full, None),
        &desktop_req("0.2.9"),
        &k,
        NOW,
    )
    .unwrap();
    // One that is merely behind is not a person's problem.
    check(
        &store,
        &client(2, TokenMode::Full, None),
        &desktop_req("0.3.3"),
        &k,
        NOW,
    )
    .unwrap();
    // The hub's own install failed.
    let failed = Report {
        update_proto: 1,
        component: Component::Hub,
        installed: Installed::version(Version::new(0, 3, 3)),
        phase: UpdatePhase::Failed,
        attempt: Some("01JA".into()),
        from: Some(Version::new(0, 3, 3)),
        to: Some(Version::new(0, 3, 4)),
        detail: serde_json::Value::Null,
        error: Some("not ready after 120 s".into()),
    };
    report(&store, &client(9, TokenMode::Updater, None), &failed, NOW).unwrap();

    let s = lock(&store).unwrap();
    let h = health(&s, &Caller::master(), &k, NOW).unwrap();
    assert_eq!(h.channel, "fresh");
    let got: Vec<(&str, &str)> = h
        .attention
        .iter()
        .map(|a| (a.reason.as_str(), a.target.as_str()))
        .collect();
    assert_eq!(
        got,
        vec![
            (ATTENTION_UPDATE_REQUIRED, "client:1"),
            (ATTENTION_UPDATE_FAILED, "hub:self"),
        ]
    );
    let hub = h.attention.iter().find(|a| a.target == "hub:self").unwrap();
    assert_eq!(hub.detail.as_deref(), Some("not ready after 120 s"));

    // An org-bound client sees itself only, never the hub's failure.
    let bound = health(&s, &client(1, TokenMode::Full, Some(3)), &k, NOW).unwrap();
    assert_eq!(bound.attention.len(), 1);
    assert_eq!(bound.attention[0].target, "client:1");

    // Past the channel's signed expiry nothing new is offered: say so.
    let later = NOW + 30 * 24 * 60 * 60;
    let stale = health(&s, &Caller::master(), &k, later).unwrap();
    assert_eq!(stale.channel, "stale");
    assert_eq!(stale.attention[0].reason, ATTENTION_CHANNEL_STALE);
    assert_eq!(stale.attention[0].target, "channel:stable");

    // Before any channel: nothing to say, and no attention for it.
    let empty = Store::open_in_memory().unwrap();
    let none = health(&empty, &Caller::master(), &k, NOW).unwrap();
    assert_eq!((none.channel.as_str(), none.attention.len()), ("none", 0));
}

#[tokio::test]
async fn why_names_one_targets_whole_decision() {
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

    let d = check_for(&store, &Caller::master(), "client:1", &k, NOW).unwrap();
    assert_eq!(d.status, Status::UpdateAvailable);
    let t = d.target.as_ref().unwrap();
    assert_eq!(t.version, Version::new(0, 3, 4));
    assert!(
        t.evidence.is_none(),
        "the dashboard's why carries no documents"
    );

    // A scoped client asks about itself, never about another.
    let me = client(1, TokenMode::Full, Some(3));
    assert_eq!(
        check_for(&store, &me, "client:1", &k, NOW).unwrap().status,
        Status::UpdateAvailable
    );
    assert_eq!(
        check_for(&store, &me, "client:2", &k, NOW)
            .unwrap_err()
            .code,
        codes::E_FORBIDDEN
    );
    assert_eq!(
        check_for(&store, &Caller::master(), "client:77", &k, NOW)
            .unwrap_err()
            .code,
        codes::E_INVALID
    );
}

// ── X-Fleet-Client (S4b) ──

#[tokio::test]
async fn a_clients_header_records_its_build_and_keeps_its_own_reports() {
    use fleet_update::client_header::ClientHeader;
    let key = TestKey::new(9);
    let (store, _) = published_store(&key).await;
    let k = keys(&key);
    let c = client(4, TokenMode::Full, None);
    // Its own check first: the variant, and a phase of its own.
    check(&store, &c, &desktop_req("0.3.3"), &k, NOW).unwrap();
    let s = lock(&store).unwrap();
    let h =
        ClientHeader::parse("desktop/0.3.4 (macos-aarch64; build 1a2b3c4; contract 6-6)").unwrap();
    record_client_header(&s, &c, &h, NOW + 60).unwrap();
    let o = s.update_observed("client:4").unwrap().unwrap();
    assert_eq!(o.version, "0.3.4");
    assert_eq!(o.commit_sha.as_deref(), Some("1a2b3c4"));
    let p: Platform = serde_json::from_str(o.platform.as_deref().unwrap()).unwrap();
    assert_eq!(
        p,
        Platform::new("macos", "aarch64", "tauri"),
        "the reported variant stays"
    );
    assert_eq!(o.last_checked_at, Some(NOW), "a header is not a check");

    // Only a client token, and only for a client component.
    let agent = ClientHeader::parse("agent/0.3.4").unwrap();
    record_client_header(&s, &c, &agent, NOW).unwrap();
    assert_eq!(
        s.update_observed("client:4").unwrap().unwrap().component,
        "desktop"
    );
    record_client_header(&s, &host("h1"), &h, NOW).unwrap();
    record_client_header(&s, &Caller::master(), &h, NOW).unwrap();
    assert!(s.update_observed("agent:h1").unwrap().is_none());
    assert!(s.update_observed("operator").unwrap().is_none());
}
