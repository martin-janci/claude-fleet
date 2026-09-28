//! `describe`: fetch-once-then-cache, an unsupported provider's honest
//! refusal, the host fence, and (the subtle point of the whole design) that
//! a warm cache never suppresses `lookup` / the start brief's own
//! truncation notice — those are about THEIR budget, not this cache.

use super::*;
use crate::net::https::{FakeTransport, Method, Response};
use crate::store::{TrackerConfig, TrackerItemWrite, WorkTarget};
use std::sync::Arc;

/// A store with one Jira item ("ABC-1") linked (as past work) to a session
/// on "hosta", and a fake transport scripted to answer the single-issue
/// `describe` endpoint once with `description`.
struct W {
    store: Arc<Mutex<Store>>,
    fake: FakeTransport,
}

impl W {
    fn net(&self) -> TrackerNet {
        TrackerNet::fake(Arc::new(self.fake.clone()))
    }

    /// How many requests the fake transport actually saw.
    fn fetches(&self) -> usize {
        self.fake.requests().len()
    }
}

/// Past work on `host`, like `tests_tickets.rs`'s `seeded_with_description`:
/// a session linked to `key`, then ended, so the host fence still sees the
/// key without a live session's other rules (start's duplicate guard, say)
/// getting in the way.
fn link_past_work(s: &Store, host: &str, key: &str) {
    s.upsert_host(host).unwrap();
    let sid = s
        .upsert_session("dev", host, None, None, 1, 1, "running", None)
        .unwrap();
    s.link_session_work(sid, WorkTarget::Key(key), "manual")
        .unwrap();
    s.conn_for_test()
        .execute(
            "UPDATE participants SET retired_at = 9 WHERE session_id = ?1",
            [sid],
        )
        .unwrap();
}

fn fake_jira_with_description(description: &str) -> W {
    let s = Store::open_in_memory().unwrap();
    let t = s
        .add_tracker("jira", "Acme", "https://acme.atlassian.net")
        .unwrap()
        .id;
    s.set_tracker_credential(
        t,
        "basic",
        Some("me@x.com"),
        Some("tok-0123456789abc"),
        None,
    )
    .unwrap();
    s.set_tracker_probe(
        t,
        None,
        &TrackerConfig {
            key_prefixes: vec!["ABC".into()],
            ..Default::default()
        },
    )
    .unwrap();
    s.set_tracker_state(t, "ok", None).unwrap();
    s.upsert_tracker_item(
        t,
        &TrackerItemWrite {
            external_id: "1".into(),
            key: Some("ABC-1".into()),
            title: "Refund".into(),
            status_name: "In Progress".into(),
            status_category: "in_progress".into(),
            ..Default::default()
        },
    )
    .unwrap();
    link_past_work(&s, "hosta", "ABC-1");

    let fake = FakeTransport::new();
    let body = serde_json::json!({
        "fields": { "description": {"type": "doc", "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": description}]}
        ]}}
    });
    fake.once(
        Method::Get,
        "/issue/ABC-1?fields=description",
        Ok(Response::json(200, &body)),
    );
    W {
        store: Arc::new(Mutex::new(s)),
        fake,
    }
}

/// An Asana item ("ASANA-1"), same host, `caps().describe == false` — no
/// credential and no scripted route, since a capability-gated refusal must
/// never reach the network to answer.
fn fake_asana_with_description(description: &str) -> W {
    let s = Store::open_in_memory().unwrap();
    let t = s
        .add_tracker("asana", "Asana", "https://app.asana.com")
        .unwrap()
        .id;
    s.set_tracker_probe(
        t,
        None,
        &TrackerConfig {
            key_prefixes: vec!["ASANA".into()],
            ..Default::default()
        },
    )
    .unwrap();
    s.set_tracker_state(t, "ok", None).unwrap();
    s.upsert_tracker_item(
        t,
        &TrackerItemWrite {
            external_id: "1".into(),
            key: Some("ASANA-1".into()),
            title: "Task".into(),
            status_name: "In Progress".into(),
            status_category: "in_progress".into(),
            description: Some(description.to_string()),
            ..Default::default()
        },
    )
    .unwrap();
    link_past_work(&s, "hosta", "ASANA-1");
    W {
        store: Arc::new(Mutex::new(s)),
        fake: FakeTransport::new(),
    }
}

#[tokio::test]
async fn describe_fetches_once_then_serves_the_cache() {
    let w = fake_jira_with_description(&"y".repeat(5000));
    let net = w.net();
    let first = describe(&w.store, &OrgScope::All, "ABC-1", &net)
        .await
        .unwrap();
    assert_eq!(first.chars, 5000);
    assert!(!first.from_cache);
    let second = describe(&w.store, &OrgScope::All, "ABC-1", &net)
        .await
        .unwrap();
    assert!(second.from_cache);
    assert_eq!(second.chars, 5000);
    assert_eq!(w.fetches(), 1);
}

#[tokio::test]
async fn a_provider_without_the_cap_answers_not_supported() {
    let w = fake_asana_with_description(&"y".repeat(5000));
    let net = w.net();
    let e = describe(&w.store, &OrgScope::All, "ASANA-1", &net)
        .await
        .unwrap_err();
    assert_eq!(e.code, codes::E_UNSUPPORTED);
    assert!(e.message.contains("open the ticket"), "{}", e.message);
    assert_eq!(w.fetches(), 0, "no network for an unsupported provider");
}

#[tokio::test]
async fn a_host_token_outside_the_org_gets_the_unknown_key_answer() {
    let w = fake_jira_with_description("anything");
    let net = w.net();
    let other = OrgScope::Host {
        alias: "other".into(),
        org: Some(2),
        isolated: Default::default(),
    };
    let e = describe(&w.store, &other, "ABC-1", &net).await.unwrap_err();
    // Exactly the fence `card` / `lookup` give a host token: a key its own
    // host does no work on reads as unknown, never revealing that another
    // host's session names it.
    assert_eq!(e.code, codes::E_FORBIDDEN);
    assert!(e.message.contains("per-host token"), "{}", e.message);
    assert_eq!(w.fetches(), 0, "the fence refuses before any network call");
}

/// The subtle point of the whole design: `describe` warming the cache must
/// not change what `lookup` or the start brief say about their OWN budget
/// (the 2000-char excerpt vs. the tracker's true length) — those notices are
/// about their own fence, not about what this cache happens to hold.
#[tokio::test]
async fn a_warm_describe_cache_does_not_suppress_the_lookup_or_brief_notice() {
    use crate::service::trackers::tickets::{self, StartArgs};

    let s = Store::open_in_memory().unwrap();
    let pid = s.upsert_project("acme", "app", "/p/acme/app").unwrap();
    let t = s
        .add_tracker("jira", "Acme", "https://acme.atlassian.net")
        .unwrap()
        .id;
    s.set_tracker_credential(
        t,
        "basic",
        Some("me@x.com"),
        Some("tok-0123456789abc"),
        None,
    )
    .unwrap();
    s.set_tracker_probe(
        t,
        None,
        &TrackerConfig {
            key_prefixes: vec!["ABC".into()],
            ..Default::default()
        },
    )
    .unwrap();
    s.set_tracker_state(t, "ok", None).unwrap();
    let item_id = s
        .upsert_tracker_item(
            t,
            &TrackerItemWrite {
                external_id: "1".into(),
                key: Some("ABC-1".into()),
                title: "Refund".into(),
                status_name: "In Progress".into(),
                status_category: "in_progress".into(),
                // The cache's excerpt (Task 1's 2k cap), and the tracker's
                // true length beside it.
                description: Some("x".repeat(trackers::DESCRIPTION_MAX_CHARS)),
                description_chars: Some(6812),
                ..Default::default()
            },
        )
        .unwrap()
        .id;
    link_past_work(&s, "hosta", "ABC-1");
    // Warm the describe cache directly: no network needed to prove the
    // point, and none is scripted below.
    s.put_description(item_id, &"y".repeat(6812), 6812).unwrap();

    let store = Arc::new(Mutex::new(s));
    let host = OrgScope::for_host(&store.lock().unwrap(), "hosta").unwrap();
    let net = TrackerNet::fake(Arc::new(FakeTransport::new()));

    let looked_up = tickets::lookup(&store, "ABC-1", &host, &net).await.unwrap();
    let d = looked_up.description.unwrap();
    assert!(d.contains("shown 2000 of 6812 chars"), "{d}");
    assert!(
        d.contains(r#"work { action: describe, key: "ABC-1" }"#),
        "{d}"
    );

    let plan = tickets::plan_start(
        &store,
        &StartArgs {
            reference: Some("ABC-1".into()),
            project_id: Some(pid),
            host_alias: Some("hosta".into()),
            ..Default::default()
        },
        &OrgScope::All,
        &net,
    )
    .await
    .unwrap();
    let brief = tickets::ticket_brief(&store, &plan).unwrap();
    assert!(
        brief.contains("shown ") && brief.contains(" of 6812 chars"),
        "{brief}"
    );
}

#[test]
fn describe_cache_retention_floor_is_the_documented_30_days() {
    assert_eq!(DESCRIBE_CACHE_RETENTION_FLOOR_DAYS, 30);
}
