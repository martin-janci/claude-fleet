//! Write-back (M13.4e) over a fake Jira that upserts remote links by
//! `globalId`, the way Jira does: the opt-in, the link-source filter, the
//! org fence, per-host links, idempotency, retry, give-up and 429.

use super::*;
use crate::net::https::{HttpTransport, Method, Request, Response, TransportError};
use crate::service::trackers::{provider_for, TrackerNet};
use crate::store::{StartSource, TrackerItemWrite, TrackerSettings, WorkTarget};
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

const TOKEN: &str = "ATATT3xFfGF0-write-back-token-not-real";
const PR: &str = "https://github.com/acme/api/pull/12";
const NOW: i64 = 1_790_000_000;

/// Jira's remote-link endpoint: upsert by `globalId` (201 new, 200
/// updated), with scripted failures served first.
#[derive(Default)]
struct FakeJira {
    /// issue path → globalId → url.
    links: Mutex<BTreeMap<String, BTreeMap<String, String>>>,
    fail: Mutex<VecDeque<Result<Response, TransportError>>>,
    sent: Mutex<Vec<Request>>,
}

impl FakeJira {
    fn fail_next(&self, r: Result<Response, TransportError>) {
        self.fail.lock().unwrap().push_back(r);
    }
    fn sent(&self) -> Vec<Request> {
        self.sent.lock().unwrap().clone()
    }
    fn links_on(&self, issue: &str) -> BTreeMap<String, String> {
        self.links
            .lock()
            .unwrap()
            .get(issue)
            .cloned()
            .unwrap_or_default()
    }
}

#[async_trait::async_trait]
impl HttpTransport for FakeJira {
    async fn send(&self, req: Request) -> Result<Response, TransportError> {
        self.sent.lock().unwrap().push(req.clone());
        if let Some(r) = self.fail.lock().unwrap().pop_front() {
            return r;
        }
        assert_eq!(req.method, Method::Post, "{}", req.url);
        let path = req.url.split("/issue/").nth(1).unwrap_or_default();
        let issue = path.trim_end_matches("/remotelink").to_string();
        assert!(req.url.ends_with("/remotelink"), "{}", req.url);
        let body: Value = serde_json::from_slice(req.body.as_deref().unwrap()).unwrap();
        let gid = body["globalId"].as_str().unwrap().to_string();
        let url = body["object"]["url"].as_str().unwrap().to_string();
        let mut links = self.links.lock().unwrap();
        let on = links.entry(issue).or_default();
        let status = if on.insert(gid, url).is_some() {
            200
        } else {
            201
        };
        Ok(Response::new(status, r#"{"id":10000,"self":"x"}"#))
    }
}

struct Fx {
    store: Mutex<Store>,
    jira: Arc<FakeJira>,
    tracker: i64,
    item: i64,
}

impl Fx {
    fn new(provider: &str, site: &str) -> Fx {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        let t = s.add_tracker(provider, "Acme", site).unwrap().id;
        s.set_tracker_credential(t, "basic", Some("dev@example.com"), Some(TOKEN), None)
            .unwrap();
        let item = s
            .upsert_tracker_item(
                t,
                &TrackerItemWrite {
                    external_id: "10001".into(),
                    key: Some("ABC-1".into()),
                    title: "Third-party <title>".into(),
                    status_name: "To Do".into(),
                    status_category: "todo".into(),
                    ..Default::default()
                },
            )
            .unwrap()
            .id;
        Fx {
            store: Mutex::new(s),
            jira: Arc::new(FakeJira::default()),
            tracker: t,
            item,
        }
    }

    fn jira() -> Fx {
        Fx::new("jira", "https://acme.atlassian.net")
    }

    fn opt_in(&self, on: bool) {
        self.store
            .lock()
            .unwrap()
            .set_tracker_settings(
                self.tracker,
                &TrackerSettings {
                    pr_remote_link: on,
                    ..Default::default()
                },
            )
            .unwrap();
    }

    /// A live session linked by `source`, with a conversation and `pr`.
    fn session(&self, name: &str, source: &str, pr: &str) -> (i64, i64) {
        let s = self.store.lock().unwrap();
        let sid = s
            .upsert_session(name, "h", None, None, 1, 1, "running", None)
            .unwrap();
        s.rebind_conversation(
            sid,
            &format!("conv-{name}"),
            StartSource::Startup,
            None,
            None,
        )
        .unwrap();
        s.conn_for_test()
            .execute(
                "UPDATE sessions SET pr_url = ?2 WHERE id = ?1",
                rusqlite::params![sid, pr],
            )
            .unwrap();
        let l = s
            .link_session_work(sid, WorkTarget::Item(self.item), "manual")
            .unwrap()
            .id;
        s.conn_for_test()
            .execute(
                "UPDATE work_links SET source = ?2 WHERE id = ?1",
                rusqlite::params![l, source],
            )
            .unwrap();
        (sid, l)
    }

    async fn pass(&self, now: i64) -> WriteBackPass {
        let (row, cred) = {
            let s = self.store.lock().unwrap();
            (
                s.require_tracker(self.tracker).unwrap(),
                s.resolve_tracker_credential(self.tracker).unwrap(),
            )
        };
        let net = TrackerNet::fake(self.jira.clone());
        let p = provider_for(&row, cred, &net).unwrap();
        run(&row, p.as_ref(), &self.store, now).await
    }

    fn rows(&self) -> Vec<crate::store::WriteOutboxRow> {
        self.store
            .lock()
            .unwrap()
            .outbox_rows(self.tracker)
            .unwrap()
    }

    fn journal(&self, conv: &str) -> Vec<String> {
        self.store
            .lock()
            .unwrap()
            .journal_for_conversations(&[conv.to_string()])
            .unwrap()
            .into_iter()
            .filter(|j| j.kind == "write_back")
            .filter_map(|j| j.body)
            .collect()
    }
}

#[tokio::test]
async fn with_the_opt_in_off_nothing_is_queued_or_sent() {
    let f = Fx::jira();
    f.session("a", "manual", PR);
    assert_eq!(f.pass(NOW).await, WriteBackPass::default());
    assert!(f.rows().is_empty());
    assert!(f.jira.sent().is_empty());
}

#[tokio::test]
async fn a_persons_link_with_a_pr_writes_one_remote_link_and_journals_it() {
    let f = Fx::jira();
    f.opt_in(true);
    f.session("a", "manual", PR);
    let p = f.pass(NOW).await;
    assert_eq!((p.queued, p.written), (1, 1), "{p:?}");
    let sent = f.jira.sent();
    assert_eq!(sent.len(), 1);
    let r = &sent[0];
    assert_eq!(
        r.url,
        "https://acme.atlassian.net/rest/api/3/issue/10001/remotelink"
    );
    let body: Value = serde_json::from_slice(r.body.as_deref().unwrap()).unwrap();
    assert_eq!(
        body,
        serde_json::json!({
            "globalId": format!("fleet:pr:{PR}"),
            "object": { "url": PR, "title": "Pull request acme/api#12" },
        }),
        "only fleet's own URL and title; nothing else"
    );
    assert!(
        !String::from_utf8_lossy(r.body.as_deref().unwrap()).contains(TOKEN),
        "no secret in the body"
    );
    assert_eq!(f.rows()[0].state, "done");
    assert_eq!(
        f.journal("conv-a"),
        vec![format!("PR linked on ABC-1 (remote link): {PR}")]
    );
    // The next pass: nothing new to send.
    assert_eq!(f.pass(NOW + 300).await, WriteBackPass::default());
    assert_eq!(f.jira.sent().len(), 1);
}

/// Idempotency, both ways: a repeated trigger is one outbox row (one
/// request), and even a repeated request is one link on the ticket, since
/// the tracker upserts by `globalId`.
#[tokio::test]
async fn the_same_global_id_twice_is_one_link() {
    let f = Fx::jira();
    f.opt_in(true);
    f.session("a", "manual", PR);
    f.session("b", "started", PR);
    let p = f.pass(NOW).await;
    assert_eq!((p.queued, p.written), (1, 1), "{p:?}");
    f.pass(NOW + 300).await;
    assert_eq!(f.jira.sent().len(), 1);
    // Sent again anyway (a row swept and re-queued): still one link.
    f.store
        .lock()
        .unwrap()
        .conn_for_test()
        .execute("DELETE FROM tracker_write_outbox", [])
        .unwrap();
    f.pass(NOW + 600).await;
    assert_eq!(f.jira.sent().len(), 2);
    let on = f.jira.links_on("10001");
    assert_eq!(on.len(), 1, "{on:?}");
    assert_eq!(
        on.get(&format!("fleet:pr:{PR}")).map(String::as_str),
        Some(PR)
    );
}

#[tokio::test]
async fn only_manual_and_started_links_write() {
    let f = Fx::jira();
    f.opt_in(true);
    for (i, source) in ["agent", "resumed", "forked", "inherited", "branch", "pr"]
        .iter()
        .enumerate()
    {
        f.session(&format!("s{i}"), source, PR);
    }
    let (_, suggested) = f.session("sug", "manual", "https://github.com/acme/api/pull/77");
    f.store
        .lock()
        .unwrap()
        .conn_for_test()
        .execute(
            "UPDATE work_links SET state = 'suggested' WHERE id = ?1",
            [suggested],
        )
        .unwrap();
    assert_eq!(f.pass(NOW).await, WriteBackPass::default());
    assert!(f.jira.sent().is_empty());
}

#[tokio::test]
async fn never_to_another_orgs_tracker() {
    let f = Fx::jira();
    f.opt_in(true);
    let (a, b) = {
        let s = f.store.lock().unwrap();
        let a = s.add_org("A", None, false).unwrap().id;
        let b = s.add_org("B", None, false).unwrap().id;
        s.set_tracker_org(f.tracker, Some(a)).unwrap();
        s.set_host_org("h", Some(b)).unwrap();
        (a, b)
    };
    // B's session on A's ticket (a forced cross-org link): never written.
    f.session("x", "manual", PR);
    assert_eq!(f.pass(NOW).await, WriteBackPass::default());
    assert!(f.jira.sent().is_empty());
    // The host moves into A: now it is the tracker's own org.
    f.store.lock().unwrap().set_host_org("h", Some(a)).unwrap();
    assert_eq!(f.pass(NOW + 1).await.written, 1);
    let _ = b;
}

#[tokio::test]
async fn a_per_host_tokens_link_never_writes_and_a_queued_write_is_cancelled() {
    let f = Fx::jira();
    f.opt_in(true);
    let (_, l) = f.session("a", "manual", PR);
    // Queued while a person's decision stood, then re-decided by a per-host
    // token before it was sent: cancelled, never sent.
    {
        let s = f.store.lock().unwrap();
        let c = s
            .pr_remote_link_candidates(f.tracker, None, NOW)
            .unwrap()
            .remove(0);
        s.enqueue_pr_remote_link(f.tracker, &c, NOW).unwrap();
        s.mark_link_host_decided(l).unwrap();
    }
    let p = f.pass(NOW).await;
    assert_eq!((p.cancelled, p.written), (1, 0), "{p:?}");
    assert!(f.jira.sent().is_empty());
    assert_eq!(f.rows()[0].state, "cancelled");
}

#[tokio::test]
async fn a_failed_write_retries_with_a_backoff_and_then_succeeds() {
    let f = Fx::jira();
    f.opt_in(true);
    f.session("a", "manual", PR);
    f.jira.fail_next(Ok(Response::new(500, "")));
    let p = f.pass(NOW).await;
    assert_eq!((p.retried, p.written), (1, 0), "{p:?}");
    let r = &f.rows()[0];
    assert_eq!((r.state.as_str(), r.attempts), ("pending", 1));
    assert_eq!(r.next_attempt_at, NOW + RETRY_BASE_SECS);
    assert_eq!(
        r.last_error.as_deref(),
        Some("unexpected answer from the tracker: HTTP 500")
    );
    // Not due yet: nothing sent.
    f.pass(NOW + RETRY_BASE_SECS - 1).await;
    assert_eq!(f.jira.sent().len(), 1);
    assert_eq!(f.pass(NOW + RETRY_BASE_SECS).await.written, 1);
    assert_eq!(f.rows()[0].state, "done");
    assert_eq!(backoff_secs(2), 120);
    assert_eq!(backoff_secs(MAX_ATTEMPTS), RETRY_MAX_SECS);
}

#[tokio::test]
async fn a_write_that_keeps_failing_gives_up_and_says_so() {
    let f = Fx::jira();
    f.opt_in(true);
    f.session("a", "manual", PR);
    let mut now = NOW;
    for _ in 0..MAX_ATTEMPTS {
        f.jira.fail_next(Ok(Response::new(403, "")));
        f.pass(now).await;
        now += RETRY_MAX_SECS;
    }
    let r = &f.rows()[0];
    assert_eq!((r.state.as_str(), r.attempts), ("failed", MAX_ATTEMPTS));
    assert_eq!(f.jira.sent().len(), MAX_ATTEMPTS as usize);
    assert_eq!(
        f.journal("conv-a"),
        vec!["PR not linked on ABC-1; gave up: not permitted: 403".to_string()]
    );
    // Failed is final: nothing more is sent.
    f.pass(now + RETRY_MAX_SECS).await;
    assert_eq!(f.jira.sent().len(), MAX_ATTEMPTS as usize);
    let c = f.store.lock().unwrap().outbox_counts(f.tracker).unwrap();
    assert_eq!(
        (c.failed, c.last_error.as_deref()),
        (1, Some("not permitted: 403"))
    );
}

#[tokio::test]
async fn a_429_honours_retry_after_is_not_an_attempt_and_ends_the_pass() {
    let f = Fx::jira();
    f.opt_in(true);
    f.session("a", "manual", PR);
    f.session("b", "manual", "https://github.com/acme/api/pull/13");
    f.jira
        .fail_next(Ok(Response::new(429, "").with_header("Retry-After", "30")));
    let p = f.pass(NOW).await;
    assert_eq!((p.queued, p.retried, p.written), (2, 1, 0), "{p:?}");
    assert_eq!(f.jira.sent().len(), 1, "the pass stopped at the 429");
    let r = &f.rows()[0];
    assert_eq!((r.attempts, r.next_attempt_at), (0, NOW + 30));
    assert_eq!(f.pass(NOW + 30).await.written, 2);
}

#[tokio::test]
async fn a_stored_error_never_carries_the_token() {
    let f = Fx::jira();
    f.opt_in(true);
    f.session("a", "manual", PR);
    f.jira.fail_next(Err(TransportError::Connect(format!(
        "proxy said: bad {TOKEN}"
    ))));
    f.pass(NOW).await;
    let e = f.rows()[0].last_error.clone().unwrap();
    assert!(!e.contains(TOKEN), "{e}");
}

#[tokio::test]
async fn data_center_writes_through_api_v2() {
    let f = Fx::new("jira_dc", "https://jira.corp.example");
    f.opt_in(true);
    f.session("a", "manual", PR);
    assert_eq!(f.pass(NOW).await.written, 1);
    assert_eq!(
        f.jira.sent()[0].url,
        "https://jira.corp.example/rest/api/2/issue/10001/remotelink"
    );
}

/// Only Jira takes the PR remote link: every other provider's opt-in is
/// refused, and even a stored one (hand-edited) writes nothing.
#[tokio::test]
async fn other_providers_are_not_supported() {
    for p in ["github", "asana", "linear"] {
        let e = crate::store::validate_tracker_settings(
            p,
            TrackerSettings {
                pr_remote_link: true,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(e.message.contains("not supported"), "{p}: {}", e.message);
    }
    for p in ["jira", "jira_dc"] {
        assert!(crate::store::validate_tracker_settings(
            p,
            TrackerSettings {
                pr_remote_link: true,
                ..Default::default()
            },
        )
        .is_ok());
    }
    let f = Fx::new("linear", "https://linear.app/acme");
    f.opt_in(true);
    f.session("a", "manual", PR);
    assert_eq!(f.pass(NOW).await, WriteBackPass::default());
    assert!(f.jira.sent().is_empty());
}
