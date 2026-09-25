//! The sync tick over `FakeTransport` and an in-memory store: views and
//! watermarks, overlap dedupe, linked refresh, unavailable, retro-binding,
//! silence on an unchanged pass, and the failure states.

use super::*;
use crate::events::RecordingEventBus;
use crate::net::https::{FakeTransport, Method, Response};
use crate::service::trackers::jira::VIEW_MINE;
use crate::service::trackers::TrackerNet;
use crate::store::{TrackerConfig, WorkTarget};
use serde_json::{json, Value};

const T0: i64 = 1_790_000_000;

fn fixture(name: &str) -> Value {
    let p = format!(
        "{}/src/service/trackers/testdata/jira/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn ok(name: &str) -> Result<Response, crate::net::https::TransportError> {
    Ok(Response::json(200, &fixture(name)))
}

struct Fx {
    store: Mutex<Store>,
    bus: Arc<RecordingEventBus>,
    fake: FakeTransport,
    tracker: i64,
}

impl Fx {
    fn new() -> Fx {
        let bus = Arc::new(RecordingEventBus::new());
        let s = Store::open_with_bus_in_memory(bus.clone()).unwrap();
        let t = s
            .add_tracker("jira", "Acme", "https://acme.atlassian.net")
            .unwrap()
            .id;
        s.set_tracker_credential(
            t,
            "basic",
            Some("dev@example.com"),
            Some("tok-0123456789abc"),
            None,
        )
        .unwrap();
        s.set_tracker_probe(
            t,
            Some("cloud"),
            &TrackerConfig {
                account_id: Some("557058:00000000-aaaa-bbbb-cccc-000000000001".into()),
                key_prefixes: vec!["ABC".into(), "TEAM".into()],
                sprint_projects: vec!["ABC".into()],
                sprint_field: Some("customfield_10020".into()),
                ..Default::default()
            },
        )
        .unwrap();
        s.sync_tracker_views(t, &[("mine".into(), "My work".into(), VIEW_MINE.into())])
            .unwrap();
        s.set_tracker_state(t, "ok", None).unwrap();
        s.upsert_host("h").unwrap();
        bus.take();
        Fx {
            store: Mutex::new(s),
            bus,
            fake: FakeTransport::new(),
            tracker: t,
        }
    }

    fn sync(&self, clock: fn() -> i64) -> TrackerSync {
        TrackerSync::new(TrackerNet::fake(Arc::new(self.fake.clone()))).with_clock(clock)
    }

    fn row(&self) -> TrackerRow {
        self.store
            .lock()
            .unwrap()
            .require_tracker(self.tracker)
            .unwrap()
    }

    fn session(&self, name: &str) -> i64 {
        self.store
            .lock()
            .unwrap()
            .upsert_session(name, "h", None, None, 1, 1, "running", None)
            .unwrap()
    }

    fn item(&self, key: &str) -> crate::store::WorkItemRow {
        self.store
            .lock()
            .unwrap()
            .tracker_item_for_key(key)
            .unwrap()
            .unwrap_or_else(|| panic!("{key} not cached"))
    }
}

#[tokio::test]
async fn a_first_pass_lists_views_whole_and_sets_the_watermark() {
    let fx = Fx::new();
    fx.fake
        .once(Method::Post, "/search/jql", ok("search_mine_p1.json"))
        .once(Method::Post, "/search/jql", ok("search_mine_p2.json"));
    let passes = fx.sync(|| T0).run_pass(&fx.store).await.unwrap();
    let p = &passes[0];
    assert_eq!(
        (p.seen, p.changed, p.error.as_deref()),
        (7, 7, None),
        "{p:?}"
    );
    let jql = fx.fake.requests()[0].json_body().unwrap()["jql"].clone();
    assert_eq!(jql, format!("({VIEW_MINE}) ORDER BY updated DESC"), "whole");
    let row = fx.row();
    assert_eq!((row.state.as_str(), row.last_sync_at), ("ok", Some(T0)));
    let wm = fx
        .store
        .lock()
        .unwrap()
        .list_tracker_views(fx.tracker)
        .unwrap()[0]
        .watermark;
    assert_eq!(wm, Some(1_789_892_130), "the newest `updated` seen");
    // Parent resolved within the pass (the epic came first).
    let story = fx.item("ABC-101");
    assert_eq!(story.parent_id, Some(fx.item("ABC-100").id));
    assert_eq!(story.iteration.as_deref(), Some("ABC Sprint 7"));
    assert_eq!(
        fx.bus.names().iter().filter(|n| **n == "work:item").count(),
        7
    );
    // The first sync is announced once (the UI's retro-link reveal).
    assert_eq!(
        fx.bus
            .names()
            .iter()
            .filter(|n| **n == "work:tracker")
            .count(),
        1
    );
}

#[tokio::test]
async fn an_unchanged_pass_is_incremental_with_overlap_and_emits_nothing() {
    let fx = Fx::new();
    let sync = fx.sync(|| T0);
    fx.fake
        .once(Method::Post, "/search/jql", ok("search_mine_p2.json"))
        .once(Method::Post, "/search/jql", ok("search_mine_p2.json"));
    sync.run_pass(&fx.store).await.unwrap();
    fx.bus.take();
    let passes = sync.run_pass(&fx.store).await.unwrap();
    assert_eq!(passes[0].changed, 0, "{:?}", passes[0]);
    assert!(
        fx.bus.names().is_empty(),
        "no event on an unchanged pass: {:?}",
        fx.bus.names()
    );
    let second = fx.fake.requests()[1].json_body().unwrap()["jql"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        second.starts_with(&format!("({VIEW_MINE}) AND updated >= -")),
        "incremental from the watermark: {second}"
    );
}

#[tokio::test]
async fn linked_items_refresh_by_id_status_moves_are_journaled_and_missing_ones_go_unavailable() {
    let fx = Fx::new();
    let sync = fx.sync(|| T0);
    fx.fake
        .once(Method::Post, "/search/jql", ok("search_mine_p1.json"));
    // p1 ends with a token; its next page is empty.
    fx.fake.once(
        Method::Post,
        "/search/jql",
        Ok(Response::json(200, &json!({"issues": [], "isLast": true}))),
    );
    sync.run_pass(&fx.store).await.unwrap();
    let sid = fx.session("dev");
    let other = fx.session("other");
    {
        let s = fx.store.lock().unwrap();
        s.conn_for_test()
            .execute(
                "UPDATE sessions SET claude_session_id = 'conv-1' WHERE id = ?1",
                [sid],
            )
            .unwrap();
        s.link_session_work(sid, WorkTarget::Key("ABC-101"), "manual")
            .unwrap();
        // A second linked item the tracker will not return.
        s.upsert_tracker_item(
            fx.tracker,
            &crate::store::TrackerItemWrite {
                external_id: "10999".into(),
                key: Some("ABC-999".into()),
                title: "gone".into(),
                status_name: "To Do".into(),
                status_category: "todo".into(),
                ..Default::default()
            },
        )
        .unwrap();
        s.link_session_work(other, WorkTarget::Key("ABC-999"), "manual")
            .unwrap();
    }
    assert_eq!(fx.row_session_status(sid).as_deref(), Some("in_progress"));
    fx.bus.take();
    fx.fake.clear_routes();
    fx.fake
        .once(
            Method::Post,
            "/search/jql",
            Ok(Response::json(200, &json!({"issues": [], "isLast": true}))),
        )
        .once(Method::Post, "/issue/bulkfetch", ok("bulkfetch.json"));
    let p = sync.run_pass(&fx.store).await.unwrap().remove(0);
    assert_eq!(p.unavailable, 1, "{p:?}");
    let body = fx
        .fake
        .requests()
        .into_iter()
        .rev()
        .find(|r| r.url.ends_with("/issue/bulkfetch"))
        .unwrap()
        .json_body()
        .unwrap();
    let mut asked: Vec<String> = body["issueIdsOrKeys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    asked.sort();
    assert_eq!(asked, vec!["10101", "10999"], "by id, never by key");
    // The chip follows: done now.
    assert_eq!(fx.row_session_status(sid).as_deref(), Some("done"));
    let gone = fx.item("ABC-999");
    assert_eq!(
        gone.unavailable_reason.as_deref(),
        Some("not_found_or_no_permission"),
        "missing is not gone"
    );
    let journal: String = fx
        .store
        .lock()
        .unwrap()
        .conn_for_test()
        .query_row(
            "SELECT body FROM work_journal WHERE kind = 'status_change'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(journal, "ABC-101: In Review → Done");
}

impl Fx {
    fn row_session_status(&self, sid: i64) -> Option<String> {
        self.store
            .lock()
            .unwrap()
            .get_session_by_id(sid)
            .unwrap()
            .unwrap()
            .work
            .and_then(|w| w.status_category)
    }
}

#[tokio::test]
async fn keys_typed_before_connecting_are_fetched_by_key_and_bound() {
    let fx = Fx::new();
    let sid = fx.session("abc-5-fix");
    fx.store
        .lock()
        .unwrap()
        .link_session_work(sid, WorkTarget::Key("abc-5"), "manual")
        .unwrap();
    // A key of a prefix no tracker owns stays a bare key and is never asked.
    let other = fx.session("zed");
    fx.store
        .lock()
        .unwrap()
        .link_session_work(other, WorkTarget::Key("ZED-1"), "manual")
        .unwrap();
    fx.fake
        .once(
            Method::Post,
            "/search/jql",
            Ok(Response::json(200, &json!({"issues": [], "isLast": true}))),
        )
        .once(
            Method::Post,
            "/issue/bulkfetch",
            Ok(Response::json(
                200,
                &json!({"issues": [{
                    "id": "10005", "key": "ABC-5",
                    "fields": {"summary": "Fix the thing",
                               "status": {"name": "To Do", "statusCategory": {"key": "new"}},
                               "issuetype": {"name": "Task", "hierarchyLevel": 0},
                               "updated": "2026-09-21T12:00:00.000Z", "project": {"key": "ABC"}}
                }]}),
            )),
        );
    let p = fx.sync(|| T0).run_pass(&fx.store).await.unwrap().remove(0);
    assert_eq!(p.bound_sessions, 1, "{p:?}");
    let body = fx.fake.requests().last().unwrap().json_body().unwrap();
    assert_eq!(body["issueIdsOrKeys"], json!(["ABC-5"]));
    let s = fx.store.lock().unwrap();
    let w = s.get_session_by_id(sid).unwrap().unwrap().work.unwrap();
    assert_eq!(w.title, "Fix the thing");
    assert_eq!(w.status_category.as_deref(), Some("todo"));
    assert_eq!(
        s.get_session_by_id(other)
            .unwrap()
            .unwrap()
            .work
            .unwrap()
            .item_id,
        None
    );
}

#[tokio::test]
async fn a_key_the_tracker_does_not_know_is_not_asked_again_soon() {
    let fx = Fx::new();
    let sid = fx.session("dev");
    fx.store
        .lock()
        .unwrap()
        .link_session_work(sid, WorkTarget::Key("ABC-404"), "manual")
        .unwrap();
    fx.fake
        .always(
            Method::Post,
            "/search/jql",
            Ok(Response::json(200, &json!({"issues": [], "isLast": true}))),
        )
        .always(
            Method::Post,
            "/issue/bulkfetch",
            Ok(Response::json(
                200,
                &json!({"issues": [], "issueErrors": []}),
            )),
        );
    let sync = fx.sync(|| T0);
    sync.run_pass(&fx.store).await.unwrap();
    sync.run_pass(&fx.store).await.unwrap();
    assert_eq!(fx.fake.count("/issue/bulkfetch"), 1);
}

#[tokio::test]
async fn a_403_on_a_view_disables_that_view_and_the_tracker_stays_ok() {
    let fx = Fx::new();
    fx.store
        .lock()
        .unwrap()
        .sync_tracker_views(
            fx.tracker,
            &[
                ("mine".into(), "My work".into(), VIEW_MINE.into()),
                ("filter:9".into(), "Secret".into(), "filter = 9".into()),
            ],
        )
        .unwrap();
    fx.fake
        .once(
            Method::Post,
            "/search/jql",
            Ok(Response::json(200, &json!({"issues": [], "isLast": true}))),
        )
        .once(Method::Post, "/search/jql", Ok(Response::new(403, "")));
    let p = fx.sync(|| T0).run_pass(&fx.store).await.unwrap().remove(0);
    assert_eq!(p.disabled_views, vec!["filter:9"]);
    assert_eq!(fx.row().state, "ok");
    let views = fx
        .store
        .lock()
        .unwrap()
        .list_tracker_views(fx.tracker)
        .unwrap();
    assert!(views
        .iter()
        .find(|v| v.view_id == "filter:9")
        .is_some_and(|v| !v.enabled));
    assert!(views
        .iter()
        .find(|v| v.view_id == "mine")
        .is_some_and(|v| v.enabled));
}

#[tokio::test]
async fn a_401_stops_polling_until_a_person_acts() {
    let fx = Fx::new();
    fx.fake
        .always(Method::Post, "/search/jql", Ok(Response::new(401, "")));
    let sync = fx.sync(|| T0);
    let p = sync.run_pass(&fx.store).await.unwrap().remove(0);
    assert!(
        p.error.as_deref().is_some_and(|e| e.contains("expire")),
        "{p:?}"
    );
    let row = fx.row();
    assert_eq!(row.state, "auth_failed");
    assert!(row.last_error.is_some());
    let again = sync.run_pass(&fx.store).await.unwrap().remove(0);
    assert!(again.skipped);
    assert_eq!(fx.fake.count("/search/jql"), 1, "polling stopped");
}

/// One 429 answered by `once`, then success; the ONE sync's clock is the
/// test's, advanced by `wait` seconds after the 429. Asserts the wait is
/// kept in the same instance: skipped (and the tracker not asked) right up
/// to `secs`, run once `secs` plus the largest jitter has passed, and the
/// wait cleared after that.
async fn a_429_is_waited_out(retry_after: Option<&str>, secs: u64) {
    use std::sync::atomic::{AtomicI64, Ordering};
    let fx = Fx::new();
    let mut limited = Response::new(429, "");
    if let Some(v) = retry_after {
        limited = limited.with_header("Retry-After", v);
    }
    fx.fake
        .once(Method::Post, "/search/jql", Ok(limited))
        .always(
            Method::Post,
            "/search/jql",
            Ok(Response::json(200, &json!({"issues": [], "isLast": true}))),
        );
    let clock = Arc::new(AtomicI64::new(T0));
    let sync = {
        let clock = Arc::clone(&clock);
        TrackerSync::new(TrackerNet::fake(Arc::new(fx.fake.clone())))
            .with_clock(move || clock.load(Ordering::SeqCst))
    };
    let at = |after: u64| clock.store(T0 + after as i64, Ordering::SeqCst);
    let p = sync.run_pass(&fx.store).await.unwrap().remove(0);
    assert!(!p.skipped && p.error.is_some(), "{p:?}");
    assert_eq!(fx.row().state, "rate_limited");
    // Inside the wait: skipped, and the tracker is not asked.
    for after in [1, secs - 1] {
        at(after);
        let p = sync.run_pass(&fx.store).await.unwrap().remove(0);
        assert!(p.skipped, "{after} s after the 429: {p:?}");
        assert_eq!(fx.row().state, "rate_limited");
    }
    assert_eq!(
        fx.fake.count("/search/jql"),
        1,
        "a skipped pass asks nothing"
    );
    // Past the wait and the most jitter it can carry (a quarter, at least
    // 5 s): the SAME sync runs, and the tracker is ok again.
    at(secs + (secs / 4).max(5) + 1);
    let p = sync.run_pass(&fx.store).await.unwrap().remove(0);
    assert!(!p.skipped && p.error.is_none(), "{p:?}");
    assert_eq!(fx.row().state, "ok");
    assert_eq!(fx.fake.count("/search/jql"), 2);
    // The wait is cleared, not merely elapsed: the pass after runs too.
    let p = sync.run_pass(&fx.store).await.unwrap().remove(0);
    assert!(!p.skipped, "{p:?}");
}

#[tokio::test]
async fn a_429_waits_out_retry_after_then_resumes() {
    a_429_is_waited_out(Some("30"), 30).await;
}

#[tokio::test]
async fn a_429_without_retry_after_waits_the_default() {
    a_429_is_waited_out(None, DEFAULT_RETRY_SECS).await;
}

/// A tracker's strings are bounded before they are stored: a 10k-character
/// summary becomes a `TITLE_MAX_CHARS` title, a long status name a
/// `FIELD_MAX_CHARS` one, and a list keeps at most `LIST_MAX` entries.
#[tokio::test]
async fn oversized_tracker_fields_are_capped_before_they_are_stored() {
    let fx = Fx::new();
    fx.fake.once(
        Method::Post,
        "/search/jql",
        Ok(Response::json(
            200,
            &json!({"issues": [{"id": "9", "key": "ABC-9", "fields": {
                "summary": "t".repeat(10_000),
                "status": {"name": "s".repeat(500), "statusCategory": {"key": "new"}},
                "issuetype": {"name": "Task", "hierarchyLevel": 0},
                "project": {"key": "ABC"},
                "assignee": {"accountId": "a1", "displayName": "n".repeat(400)}}}],
                "isLast": true}),
        )),
    );
    let p = fx.sync(|| T0).run_pass(&fx.store).await.unwrap().remove(0);
    assert_eq!((p.seen, p.error.as_deref()), (1, None), "{p:?}");
    let item = fx.item("ABC-9");
    assert_eq!(item.title.chars().count(), TITLE_MAX_CHARS);
    assert_eq!(
        item.status_name.as_deref().map(str::len),
        Some(FIELD_MAX_CHARS)
    );
    assert!(item
        .assignees
        .iter()
        .all(|a| a.chars().count() <= FIELD_MAX_CHARS));
    // The lists, at the write shape.
    let many = (0..40)
        .map(|i| format!("{i}-{}", "x".repeat(300)))
        .collect();
    let w = to_write(WorkItemSnapshot {
        external_id: "1".into(),
        assignees: many,
        ..Default::default()
    });
    assert_eq!(w.assignees.len(), LIST_MAX);
    assert!(w
        .assignees
        .iter()
        .all(|a| a.chars().count() == FIELD_MAX_CHARS));
}

/// A key over `KEY_MAX_CHARS` is dropped, not cut: `owner/repo#123` cut in
/// its number is issue #12's own key. The row keeps its identity (the id)
/// and everything else; an over-long alias goes the same way, the rest of
/// the aliases stay.
#[test]
fn an_over_long_key_is_dropped_not_truncated() {
    let repo = format!("some-org/{}", "r".repeat(57));
    let key = format!("{repo}#123");
    assert_eq!(key.chars().count(), 70);
    let w = to_write(WorkItemSnapshot {
        external_id: "I_1".into(),
        key: Some(key.clone()),
        aliases: vec![format!("{repo}#12"), "ABC-12".into()],
        url: Some(format!("https://github.com/{repo}/issues/123")),
        title: "long repo".into(),
        ..Default::default()
    });
    assert_eq!(w.key, None, "no `…#12` minted from #123");
    assert_eq!(w.aliases, vec!["ABC-12"]);
    assert_eq!(w.external_id, "I_1");
    assert!(w.url.is_some());
    let w = to_write(WorkItemSnapshot {
        external_id: "1".into(),
        key: Some("x".repeat(KEY_MAX_CHARS)),
        ..Default::default()
    });
    assert_eq!(w.key.map(|k| k.chars().count()), Some(KEY_MAX_CHARS));
}

/// The 429 deadline is on the row too: a sync with no memory of it (a
/// restart) still waits it out, and a pass after it clears the row.
#[tokio::test]
async fn a_429_deadline_is_kept_on_the_row_and_cleared_by_a_good_pass() {
    let fx = Fx::new();
    fx.fake
        .once(
            Method::Post,
            "/search/jql",
            Ok(Response::new(429, "").with_header("Retry-After", "30")),
        )
        .always(
            Method::Post,
            "/search/jql",
            Ok(Response::json(200, &json!({"issues": [], "isLast": true}))),
        );
    fx.sync(|| T0).run_pass(&fx.store).await.unwrap();
    let nb = fx
        .store
        .lock()
        .unwrap()
        .tracker_not_before(fx.tracker)
        .unwrap()
        .expect("the deadline is on the row");
    assert!((T0 + 30..=T0 + 40).contains(&nb), "{nb}");
    // A fresh sync (nothing in memory), still inside the window: skipped.
    assert!(fx.sync(|| T0 + 10).run_pass(&fx.store).await.unwrap()[0].skipped);
    assert_eq!(
        fx.fake.count("/search/jql"),
        1,
        "no request inside the window"
    );
    // Past it: runs, and the row's deadline is gone.
    let p = fx
        .sync(|| T0 + 3600)
        .run_pass(&fx.store)
        .await
        .unwrap()
        .remove(0);
    assert!(!p.skipped && p.error.is_none(), "{p:?}");
    assert_eq!(
        fx.store
            .lock()
            .unwrap()
            .tracker_not_before(fx.tracker)
            .unwrap(),
        None
    );
}

/// The row is the one deadline the tick reads: a successful `test_tracker`
/// clears it, and the SAME sync — whose own memory of the 429 would have
/// parked the tracker for up to `MAX_RETRY_SECS` — runs on its next pass,
/// while a lookup was already allowed again.
#[tokio::test]
async fn a_good_test_ends_the_wait_for_the_sync_that_saw_the_429() {
    let fx = Fx::new();
    fx.fake
        .once(
            Method::Post,
            "/search/jql",
            Ok(Response::new(429, "").with_header("Retry-After", "3600")),
        )
        .always(
            Method::Post,
            "/search/jql",
            Ok(Response::json(200, &json!({"issues": [], "isLast": true}))),
        );
    let sync = fx.sync(|| T0);
    let p = sync.run_pass(&fx.store).await.unwrap().remove(0);
    assert!(!p.skipped && p.error.is_some(), "{p:?}");
    assert_eq!(fx.row().state, "rate_limited");
    assert!(
        sync.run_pass(&fx.store).await.unwrap()[0].skipped,
        "inside the wait"
    );
    // The operator presses Test (the quota is back): the probe answers.
    fx.fake
        .once(
            Method::Get,
            "/myself",
            Ok(Response::json(200, &fixture("myself.json"))),
        )
        .once(
            Method::Get,
            "/_edge/tenant_info",
            Ok(Response::json(200, &fixture("tenant_info.json"))),
        )
        .once(
            Method::Get,
            "startAt=0",
            Ok(Response::json(200, &fixture("project_search_p2.json"))),
        )
        .once(
            Method::Get,
            "/rest/api/3/field",
            Ok(Response::json(200, &fixture("fields.json"))),
        )
        .once(
            Method::Get,
            "/filter/favourite",
            Ok(Response::json(200, &fixture("filter_favourite.json"))),
        );
    let r = crate::service::trackers::admin::test_tracker(
        fx.tracker,
        &fx.store,
        &TrackerNet::fake(Arc::new(fx.fake.clone())),
    )
    .await
    .unwrap();
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(
        fx.store
            .lock()
            .unwrap()
            .tracker_not_before(fx.tracker)
            .unwrap(),
        None
    );
    // Still at T0, an hour inside what this sync remembers: it runs.
    let p = sync.run_pass(&fx.store).await.unwrap().remove(0);
    assert!(!p.skipped && p.error.is_none(), "{p:?}");
    assert_eq!(fx.row().state, "ok");
}

/// An absurd `Retry-After` neither overflows nor parks the tracker past
/// `MAX_RETRY_SECS` (plus jitter).
#[tokio::test]
async fn a_huge_retry_after_is_clamped() {
    let fx = Fx::new();
    fx.fake.once(
        Method::Post,
        "/search/jql",
        Ok(Response::new(429, "").with_header("Retry-After", "4000000000000000000")),
    );
    let sync = fx.sync(|| T0);
    sync.run_pass(&fx.store).await.unwrap();
    assert_eq!(fx.row().state, "rate_limited");
    let nb = sync.not_before.lock().unwrap()[&fx.tracker];
    assert!(nb > T0, "{nb}");
    assert!(
        nb <= T0 + (MAX_RETRY_SECS + MAX_RETRY_SECS / 4) as i64,
        "{nb}"
    );
}

#[tokio::test]
async fn offline_marks_the_tracker_unreachable_and_the_cache_still_answers() {
    let fx = Fx::new();
    fx.fake
        .once(Method::Post, "/search/jql", ok("search_mine_p2.json"));
    let sync = fx.sync(|| T0);
    sync.run_pass(&fx.store).await.unwrap();
    fx.fake.clear_routes(); // the network goes away
    let p = sync.run_pass(&fx.store).await.unwrap().remove(0);
    assert!(p.error.is_some());
    assert_eq!(fx.row().state, "unreachable");
    assert_eq!(
        fx.item("TEAM-7").title,
        "Onboarding checklist",
        "the cache stays"
    );
    // Unreachable is transient: the next pass tries again.
    assert!(!sync.run_pass(&fx.store).await.unwrap()[0].skipped);
}

#[tokio::test]
async fn trackers_needing_a_person_or_a_test_are_not_polled() {
    let fx = Fx::new();
    for state in ["auth_failed", "captcha", "unconfigured"] {
        fx.store
            .lock()
            .unwrap()
            .set_tracker_state(fx.tracker, state, None)
            .unwrap();
        assert!(
            fx.sync(|| T0).run_pass(&fx.store).await.unwrap()[0].skipped,
            "{state}"
        );
    }
    assert!(fx.fake.requests().is_empty());
}

#[tokio::test]
async fn passes_are_single_flight() {
    let fx = Fx::new();
    let sync = fx.sync(|| T0);
    sync.running.store(true, Ordering::Release);
    assert!(sync.run_pass(&fx.store).await.is_none());
}

#[test]
fn the_interval_setting_defaults_turns_off_and_has_a_floor() {
    let st = Mutex::new(Store::open_in_memory().unwrap());
    assert_eq!(
        interval(&st),
        Some(Duration::from_secs(DEFAULT_INTERVAL_SECS))
    );
    st.lock()
        .unwrap()
        .set_setting(crate::service::settings::WORK_SYNC_INTERVAL_SECS, "0")
        .unwrap();
    assert_eq!(interval(&st), None);
    st.lock()
        .unwrap()
        .set_setting(crate::service::settings::WORK_SYNC_INTERVAL_SECS, "5")
        .unwrap();
    assert_eq!(interval(&st), Some(Duration::from_secs(60)));
}

/// A provider whose views are read by sync token (M6.0's opaque mark).
/// `bad`: its changes carry an item the store refuses (no id).
struct TokenProvider {
    expired: bool,
    bad: bool,
}

#[async_trait::async_trait]
impl TrackerProvider for TokenProvider {
    fn caps(&self) -> crate::service::trackers::Caps {
        crate::service::trackers::Caps {
            incremental: Incremental::SyncToken,
            ..Default::default()
        }
    }
    async fn probe(&self) -> Result<crate::service::trackers::TrackerInfo, TrackerError> {
        unreachable!()
    }
    async fn views(&self, _: &TrackerConfig) -> Result<Vec<ViewDef>, TrackerError> {
        unreachable!()
    }
    async fn list(
        &self,
        _: &ViewDef,
        since: Option<i64>,
        _: Option<String>,
    ) -> Result<crate::service::trackers::Page, TrackerError> {
        assert_eq!(since, None, "a token provider lists whole");
        Ok(crate::service::trackers::Page {
            items: vec![snap("whole")],
            next: None,
        })
    }
    async fn fetch(&self, _: &[ItemRef]) -> Result<Vec<Fetched>, TrackerError> {
        unreachable!()
    }
    fn recognize(&self, _: &str, _: crate::service::trackers::RefCtx<'_>) -> Vec<ItemRef> {
        vec![]
    }
    async fn changes(
        &self,
        _: &ViewDef,
        mark: Option<&str>,
    ) -> Result<crate::service::trackers::Changes, TrackerError> {
        Ok(crate::service::trackers::Changes {
            items: if mark.is_some() && !self.expired {
                vec![snap(if self.bad { "" } else { "changed" })]
            } else {
                vec![]
            },
            mark: Some("tok-2".into()),
            expired: mark.is_none() || self.expired,
        })
    }
}

fn snap(id: &str) -> WorkItemSnapshot {
    WorkItemSnapshot {
        external_id: id.into(),
        updated: Some(1),
        ..Default::default()
    }
}

#[tokio::test]
async fn a_sync_token_view_reads_changes_and_an_expired_token_lists_whole() {
    let def = ViewDef {
        id: "project:1".into(),
        label: "P".into(),
        query: "1".into(),
    };
    let ids = |v: &[WorkItemSnapshot]| v.iter().map(|i| i.external_id.clone()).collect::<Vec<_>>();
    let p = TokenProvider {
        expired: false,
        bad: false,
    };
    // No token yet: a whole listing, then a first token.
    let (items, full, mark) = read_view(&p, &def, Incremental::SyncToken, false, None, None)
        .await
        .unwrap();
    assert_eq!(
        (ids(&items), full, mark.as_deref()),
        (vec!["whole".into()], true, Some("tok-2"))
    );
    // A live token: only the changes, and the next token.
    let (items, full, mark) =
        read_view(&p, &def, Incremental::SyncToken, false, None, Some("tok-1"))
            .await
            .unwrap();
    assert_eq!(
        (ids(&items), full, mark.as_deref()),
        (vec!["changed".into()], false, Some("tok-2"))
    );
    // An expired token: one whole listing, and the fresh token.
    let p = TokenProvider {
        expired: true,
        bad: false,
    };
    let (items, full, mark) = read_view(&p, &def, Incremental::SyncToken, false, None, Some("old"))
        .await
        .unwrap();
    assert_eq!(
        (ids(&items), full, mark.as_deref()),
        (vec!["whole".into()], true, Some("tok-2"))
    );
}

/// The token moves only once the items it stands for are stored: a store
/// write that fails leaves the token where it was, so the next pass reads
/// the same changes again instead of skipping them.
#[tokio::test]
async fn a_sync_mark_is_written_only_after_the_items_are_stored() {
    let fx = Fx::new();
    let views = {
        let s = fx.store.lock().unwrap();
        s.sync_tracker_views(fx.tracker, &[("project:1".into(), "P".into(), "1".into())])
            .unwrap();
        s.set_tracker_view_mark(fx.tracker, "project:1", Some("tok-1"))
            .unwrap();
        s.set_tracker_view_watermark(fx.tracker, "project:1", T0)
            .unwrap();
        s.list_tracker_views(fx.tracker).unwrap()
    };
    let mark = |fx: &Fx| -> Option<String> {
        fx.store
            .lock()
            .unwrap()
            .list_tracker_views(fx.tracker)
            .unwrap()
            .remove(0)
            .sync_mark
    };
    let sync = fx.sync(|| T0);
    // Inside FULL_EVERY_SECS of a whole listing, so the view reads changes.
    sync.last_full
        .lock()
        .unwrap()
        .insert((fx.tracker, "project:1".into()), T0);
    let row = fx.row();
    let mut pass = TrackerPass::default();
    let bad = TokenProvider {
        expired: false,
        bad: true,
    };
    let r = sync
        .run_provider(&row, &bad, views.clone(), &fx.store, T0, &mut pass)
        .await;
    assert!(r.is_err(), "the store refused the item");
    assert_eq!(
        mark(&fx).as_deref(),
        Some("tok-1"),
        "the token did not move"
    );
    let good = TokenProvider {
        expired: false,
        bad: false,
    };
    sync.run_provider(&row, &good, views, &fx.store, T0, &mut pass)
        .await
        .unwrap();
    assert_eq!(mark(&fx).as_deref(), Some("tok-2"));
    assert_eq!(pass.changed, 1);
}
