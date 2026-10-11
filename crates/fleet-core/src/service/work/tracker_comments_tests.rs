//! `tracker_comments`: one live read fenced like `describe`, a body fenced
//! for an agent, fleet's own posts left out; and `comment`'s
//! `post_to_tracker`, queued with the comment or refused with nothing
//! written.

use super::*;
use crate::ipc_error::codes;
use crate::net::https::{FakeTransport, Method, Response};
use crate::service::work::WorkLinkArgs;
use crate::store::{
    NativeItem, TrackerConfig, TrackerItemWrite, TrackerSettings, WorkTarget, WriteBack,
};
use std::sync::Arc;

struct W {
    store: Arc<Mutex<Store>>,
    fake: FakeTransport,
    tracker: i64,
    item: i64,
}

impl W {
    fn net(&self) -> TrackerNet {
        TrackerNet::fake(Arc::new(self.fake.clone()))
    }
}

/// A Jira tracker with ABC-1, linked as past work on `hosta`; comments
/// write-back on when `post`.
fn jira(post: bool) -> W {
    let s = Store::open_in_memory().unwrap();
    let t = s
        .add_tracker("jira", "Acme Jira", "https://acme.atlassian.net")
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
    if post {
        s.set_tracker_settings(
            t,
            &TrackerSettings {
                write_back: WriteBack {
                    comments: true,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
    }
    let item = s
        .upsert_tracker_item(
            t,
            &TrackerItemWrite {
                external_id: "10001".into(),
                key: Some("ABC-1".into()),
                title: "Refund".into(),
                status_name: "In Progress".into(),
                status_category: "in_progress".into(),
                ..Default::default()
            },
        )
        .unwrap()
        .id;
    s.upsert_host("hosta").unwrap();
    let sid = s
        .upsert_session("dev", "hosta", None, None, 1, 1, "running", None)
        .unwrap();
    s.link_session_work(sid, WorkTarget::Key("ABC-1"), "manual")
        .unwrap();
    s.conn_for_test()
        .execute(
            "UPDATE participants SET retired_at = 9 WHERE session_id = ?1",
            [sid],
        )
        .unwrap();
    W {
        store: Arc::new(Mutex::new(s)),
        fake: FakeTransport::new(),
        tracker: t,
        item,
    }
}

fn adf(text: &str) -> serde_json::Value {
    serde_json::json!({ "type": "doc", "version": 1, "content": [
        { "type": "paragraph", "content": [{ "type": "text", "text": text }] }
    ]})
}

/// Jira's answer: newest first, as `orderBy=-created` asks.
fn answer(w: &W) {
    w.fake.always(
        Method::Get,
        "/rest/api/3/issue/ABC-1/comment",
        Ok(Response::json(
            200,
            &serde_json::json!({ "comments": [
                { "id": "20", "author": { "displayName": "Bob" }, "body": adf("second"),
                  "created": "2026-10-02T10:00:00.000+0000" },
                { "id": "10", "author": { "displayName": "Ann" }, "body": adf("first"),
                  "created": "2026-10-01T10:00:00.000+0000" },
            ]}),
        )),
    );
}

fn comment_args(item: i64, post: Option<bool>) -> WorkLinkArgs {
    WorkLinkArgs {
        action: "comment".into(),
        item_id: Some(item),
        notes: Some("Shipped in 1.2".into()),
        post_to_tracker: post,
        ..Default::default()
    }
}

fn host(org: Option<i64>, alias: &str) -> OrgScope {
    OrgScope::Host {
        alias: alias.into(),
        org,
        isolated: Default::default(),
    }
}

fn outbox(w: &W) -> Vec<crate::store::TrackerWriteRow> {
    let s = w.store.lock().unwrap();
    let ids: Vec<i64> = s
        .conn_for_test()
        .prepare("SELECT id FROM tracker_writes ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    ids.into_iter()
        .map(|id| s.tracker_write(id).unwrap().unwrap())
        .collect()
}

#[tokio::test]
async fn a_tickets_comments_come_oldest_first_with_the_trackers_name() {
    let w = jira(false);
    answer(&w);
    let got = tracker_comments(&w.store, &OrgScope::All, "abc-1", &w.net())
        .await
        .unwrap();
    assert_eq!(got.key, "ABC-1");
    assert_eq!(got.tracker, "Acme Jira");
    assert_eq!(got.provider, "jira");
    assert!(!got.can_post, "write-back is off");
    let bodies: Vec<_> = got.comments.iter().map(|c| c.body.as_str()).collect();
    assert_eq!(bodies, ["first", "second"]);
    assert_eq!(got.comments[0].author, "Ann");
    assert_eq!(
        got.comments[0].url.as_deref(),
        Some("https://acme.atlassian.net/browse/ABC-1?focusedCommentId=10")
    );
    let sent = w.fake.requests();
    assert_eq!(sent.len(), 1);
    assert!(
        sent[0].url.contains("orderBy=-created&maxResults=50"),
        "{}",
        sent[0].url
    );
}

#[tokio::test]
async fn a_host_token_reads_bodies_fenced_and_outside_its_org_nothing() {
    let w = jira(false);
    answer(&w);
    let got = tracker_comments(&w.store, &host(None, "hosta"), "ABC-1", &w.net())
        .await
        .unwrap();
    assert!(
        got.comments[0]
            .body
            .contains(crate::mcp::guard::UNTRUSTED_END),
        "{:?}",
        got.comments[0].body
    );
    assert!(!got.can_post, "an agent never posts");
    let before = w.fake.requests().len();
    let e = tracker_comments(&w.store, &host(Some(2), "other"), "ABC-1", &w.net())
        .await
        .unwrap_err();
    assert_eq!(e.code, codes::E_FORBIDDEN);
    assert_eq!(
        w.fake.requests().len(),
        before,
        "refused before the network"
    );
}

#[tokio::test]
async fn fleets_own_work_has_no_tracker_comments() {
    let w = jira(false);
    let key = {
        let s = w.store.lock().unwrap();
        s.create_native_item(&NativeItem {
            title: "Local",
            parent_id: None,
            project_id: None,
            notes: None,
        })
        .unwrap()
        .key
        .unwrap()
    };
    let e = tracker_comments(&w.store, &OrgScope::All, &key, &w.net())
        .await
        .unwrap_err();
    assert_eq!(e.code, codes::E_UNSUPPORTED);
    assert!(w.fake.requests().is_empty());
}

#[test]
fn posting_is_refused_with_nothing_written_unless_the_tracker_allows_it() {
    // Off: refused, and the comment is not written either.
    let w = jira(false);
    let e = crate::service::work::local::comment(
        &comment_args(w.item, Some(true)),
        &w.store,
        &OrgScope::All,
        "desktop",
        None,
    )
    .unwrap_err();
    assert_eq!(e.code, codes::E_INVALID_STATE);
    assert!(e.message.contains("Acme Jira"), "{}", e.message);
    assert!(w
        .store
        .lock()
        .unwrap()
        .item_comments(w.item)
        .unwrap()
        .is_empty());
    assert!(outbox(&w).is_empty());

    // On, but an agent asks: refused.
    let w = jira(true);
    let e = crate::service::work::local::comment(
        &comment_args(w.item, Some(true)),
        &w.store,
        &host(None, "hosta"),
        "host:hosta",
        None,
    )
    .unwrap_err();
    assert_eq!(e.code, codes::E_FORBIDDEN);
    assert!(outbox(&w).is_empty());

    // Fleet's own work: no tracker to post to.
    let native = w
        .store
        .lock()
        .unwrap()
        .create_native_item(&NativeItem {
            title: "Local",
            parent_id: None,
            project_id: None,
            notes: None,
        })
        .unwrap()
        .id;
    let e = crate::service::work::local::comment(
        &comment_args(native, Some(true)),
        &w.store,
        &OrgScope::All,
        "desktop",
        None,
    )
    .unwrap_err();
    assert_eq!(e.code, codes::E_UNSUPPORTED);
    assert!(outbox(&w).is_empty());
}

#[test]
fn a_post_is_queued_with_the_comment_signed_by_its_person() {
    let w = jira(true);
    let person = w
        .store
        .lock()
        .unwrap()
        .create_person("ann", Some("Ann Lee"))
        .unwrap()
        .id;
    let c = crate::service::work::local::comment(
        &comment_args(w.item, Some(true)),
        &w.store,
        &OrgScope::All,
        "client:phone",
        Some(person),
    )
    .unwrap();
    assert_eq!(c.tracker_post.as_ref().unwrap().state, "pending");
    let rows = outbox(&w);
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert_eq!(r.op, "comment");
    assert_eq!(r.tracker_id, w.tracker);
    assert_eq!(r.item_key, "ABC-1");
    assert_eq!(r.url, format!("fleet-comment:{}", c.id));
    assert_eq!(r.external_id.as_deref(), Some("10001"));
    assert_eq!(
        r.body.as_deref(),
        Some("Shipped in 1.2\n\n— Ann Lee via fleet")
    );
    // Without a person, no device name reaches the tracker.
    let c2 = crate::service::work::local::comment(
        &comment_args(w.item, Some(true)),
        &w.store,
        &OrgScope::All,
        "client:phone",
        None,
    )
    .unwrap();
    let rows = outbox(&w);
    assert_eq!(rows[1].url, format!("fleet-comment:{}", c2.id));
    assert_eq!(
        rows[1].body.as_deref(),
        Some("Shipped in 1.2\n\n— via fleet")
    );
    // Not asked: a comment alone, as before.
    crate::service::work::local::comment(
        &comment_args(w.item, None),
        &w.store,
        &OrgScope::All,
        "desktop",
        None,
    )
    .unwrap();
    assert_eq!(outbox(&w).len(), 2);
}

#[tokio::test]
async fn a_comment_fleet_posted_is_shown_once_until_it_is_deleted_in_fleet() {
    let w = jira(true);
    answer(&w);
    let c = crate::service::work::local::comment(
        &comment_args(w.item, Some(true)),
        &w.store,
        &OrgScope::All,
        "desktop",
        None,
    )
    .unwrap();
    {
        let id = outbox(&w)[0].id;
        let s = w.store.lock().unwrap();
        s.finish_tracker_write_as(
            id,
            Some("20"),
            Some("https://acme.atlassian.net/browse/ABC-1?focusedCommentId=20"),
        )
        .unwrap();
        let row = s.get_comment(c.id).unwrap().unwrap();
        let post = row.tracker_post.unwrap();
        assert_eq!(post.state, "done");
        assert!(post.url.unwrap().ends_with("focusedCommentId=20"));
    }
    let got = tracker_comments(&w.store, &OrgScope::All, "ABC-1", &w.net())
        .await
        .unwrap();
    assert!(got.can_post);
    assert_eq!(
        got.comments
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>(),
        ["10"],
        "fleet shows its own post as its own comment"
    );
    // Deleted in fleet: the tracker keeps it, so it shows from there.
    w.store
        .lock()
        .unwrap()
        .delete_comment(c.id, None, "desktop")
        .unwrap();
    assert_eq!(outbox(&w).len(), 1, "a sent post stays");
    let got = tracker_comments(&w.store, &OrgScope::All, "ABC-1", &w.net())
        .await
        .unwrap();
    assert_eq!(got.comments.len(), 2);
}

#[test]
fn deleting_a_comment_cancels_its_pending_post() {
    let w = jira(true);
    let c = crate::service::work::local::comment(
        &comment_args(w.item, Some(true)),
        &w.store,
        &OrgScope::All,
        "desktop",
        None,
    )
    .unwrap();
    assert_eq!(outbox(&w).len(), 1);
    let gone = w
        .store
        .lock()
        .unwrap()
        .delete_comment(c.id, None, "desktop")
        .unwrap()
        .unwrap();
    assert_eq!(gone.tracker_post.unwrap().state, "pending");
    assert!(outbox(&w).is_empty(), "the pending post is cancelled");
}

fn plan_for(w: &W) -> crate::service::trackers::tickets::StartPlan {
    serde_json::from_value(serde_json::json!({
        "key": "ABC-1", "title": "Refund", "item_id": w.item, "project_id": 1,
        "host_alias": "hosta", "branch": "abc-1-refund", "name": "ABC-1 Refund",
    }))
    .unwrap()
}

/// A start's brief carries the ticket's comments, read live by the hub:
/// the session's Claude needs no tracker access of its own.
#[tokio::test]
async fn a_start_brief_carries_the_tickets_latest_comments_fenced() {
    let w = jira(false);
    answer(&w);
    let tail = brief_extras(&w.store, &plan_for(&w), &w.net()).await;
    let open = tail.find("[claude-fleet:").expect("fenced");
    let first = tail.find("Ann (2026-10-01): first").expect(&tail);
    let second = tail.find("Bob (2026-10-02): second").expect(&tail);
    let end = tail.find(crate::mcp::guard::UNTRUSTED_END).unwrap();
    assert!(open < first && first < second && second < end, "{tail}");
    assert!(
        w.fake.requests()[0].url.contains("maxResults=10"),
        "{}",
        w.fake.requests()[0].url
    );
}

/// A tracker that fails costs the brief its comments, never the start.
#[tokio::test]
async fn a_failing_tracker_leaves_the_brief_without_them() {
    let w = jira(false);
    w.fake.always(
        Method::Get,
        "/rest/api/3/issue/ABC-1/comment",
        Ok(Response::json(500, &serde_json::json!({}))),
    );
    assert_eq!(brief_extras(&w.store, &plan_for(&w), &w.net()).await, "");
    let mut plan = plan_for(&w);
    plan.item_id = None;
    assert_eq!(brief_extras(&w.store, &plan, &w.net()).await, "");
}

/// The tail stays inside its budget, keeping the newest comments, and
/// never loses its fence's end.
#[test]
fn the_brief_tail_keeps_the_newest_within_its_budget() {
    let comments: Vec<TrackerComment> = (0..30)
        .map(|i| TrackerComment {
            id: format!("c{i}"),
            author: "Ann".into(),
            body: format!("comment {i} {}", "x".repeat(500)),
            created_at: 1_790_000_000 + i,
            url: None,
        })
        .collect();
    let attachments = vec![TrackerAttachment {
        id: "a1".into(),
        name: "spec.pdf".into(),
        url: Some("https://app.asana.com/x".into()),
        size: None,
        created_at: None,
    }];
    let tail = brief_tail(&attachments, &comments);
    assert!(
        tail.chars().count() <= BRIEF_EXTRAS_MAX_CHARS,
        "{}",
        tail.len()
    );
    assert!(tail.contains("- spec.pdf: https://app.asana.com/x"));
    assert!(tail.contains("comment 29 "), "the newest is kept");
    assert!(!tail.contains("comment 0 "), "the oldest gives way");
    assert!(tail.contains("older on the ticket"));
    assert!(tail.trim_end().ends_with(crate::mcp::guard::UNTRUSTED_END));
    assert_eq!(brief_tail(&[], &[]), "");
}
