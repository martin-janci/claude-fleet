use super::*;
use crate::net::https::{FakeTransport, Method, Response};
use crate::service::trackers::jira::JiraCloud;
use crate::service::trackers::jira_dc::JiraDc;
use crate::store::{
    TrackerConfig, TrackerCredential, TrackerItemWrite, TrackerSettings, WorkTarget, WriteBack,
};
use std::sync::Arc;

const PR: &str = "https://github.com/acme/api/pull/42";
const CID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";

fn cred() -> TrackerCredential {
    TrackerCredential {
        auth_kind: "basic".into(),
        username: Some("dev@example.com".into()),
        secret: crate::store::Secret::new("ATATT3xFfGF0-test-token-not-real-0000"),
    }
}

/// A Jira tracker with ABC-1 in it, and a session on host `h` linked to it
/// by `source`. `write_back` turns the PR link on.
struct Fx {
    s: Store,
    tracker: i64,
    session: i64,
}

fn fx(source: &str, write_back: bool) -> Fx {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h").unwrap();
    let t = s
        .add_tracker("jira", "Acme Jira", "https://acme.atlassian.net")
        .unwrap();
    if write_back {
        s.set_tracker_settings(
            t.id,
            &TrackerSettings {
                write_back: WriteBack {
                    pr_remote_link: true,
                },
                ..Default::default()
            },
        )
        .unwrap();
    }
    let item = s
        .upsert_tracker_item(
            t.id,
            &TrackerItemWrite {
                external_id: "10001".into(),
                key: Some("ABC-1".into()),
                title: "Fix login".into(),
                status_name: "In Progress".into(),
                status_category: "in_progress".into(),
                ..Default::default()
            },
        )
        .unwrap()
        .id;
    let session = s
        .upsert_session("dev-abc-1", "h", None, None, 1, 1, "running", None)
        .unwrap();
    s.set_claude_session_id(session, CID).unwrap();
    s.link_session_work(session, WorkTarget::Item(item), source)
        .unwrap();
    Fx {
        s,
        tracker: t.id,
        session,
    }
}

fn outbox(s: &Store) -> Vec<crate::store::TrackerWriteRow> {
    s.conn_ref()
        .prepare("SELECT id FROM tracker_writes ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get::<_, i64>(0))
        .unwrap()
        .map(|id| s.tracker_write(id.unwrap()).unwrap().unwrap())
        .collect()
}

// ── the PR URL ──────────────────────────────────────────────────────────

#[test]
fn only_a_pull_request_url_gets_a_title() {
    assert_eq!(pr_title(PR).as_deref(), Some("PR: acme/api#42"));
    assert_eq!(
        pr_title("https://ghe.corp.example:8443/o/r.js/pull/7/").as_deref(),
        Some("PR: o/r.js#7")
    );
    for bad in [
        "http://github.com/acme/api/pull/42",
        "https://github.com/acme/api/issues/42",
        "https://github.com/acme/api/pull/42/files",
        "https://github.com/acme/api/pull/42?x=1",
        "https://github.com/acme/api/pull/4x",
        "https://github.com/../api/pull/42",
        "https://github.com/acme/api/pull/42\n",
        "https://github.com/ac me/api/pull/42",
        "javascript:alert(1)",
        "",
    ] {
        assert_eq!(pr_title(bad), None, "{bad:?}");
    }
}

#[test]
fn backoff_doubles_from_a_minute_to_six_hours() {
    assert_eq!(backoff_secs(0), 60);
    assert_eq!(backoff_secs(1), 120);
    assert_eq!(backoff_secs(4), 960);
    assert_eq!(backoff_secs(40), 6 * 3_600);
}

// ── what gets queued ────────────────────────────────────────────────────

#[test]
fn a_person_linked_session_queues_one_write_however_often_the_pr_is_seen() {
    for source in ["manual", "started"] {
        let f = fx(source, true);
        assert_eq!(on_pr(&f.s, f.session, PR).unwrap(), 1, "{source}");
        assert_eq!(on_pr(&f.s, f.session, PR).unwrap(), 0, "{source}");
        let rows = outbox(&f.s);
        assert_eq!(rows.len(), 1);
        let w = &rows[0];
        assert_eq!(
            (
                w.tracker_id,
                w.item_key.as_str(),
                w.url.as_str(),
                w.title.as_str()
            ),
            (f.tracker, "ABC-1", PR, "PR: acme/api#42")
        );
        assert_eq!(w.claude_session_id.as_deref(), Some(CID));
    }
}

#[test]
fn nothing_is_queued_without_the_setting_for_a_guess_or_for_a_non_pr_url() {
    let off = fx("manual", false);
    assert_eq!(on_pr(&off.s, off.session, PR).unwrap(), 0);
    let f = fx("manual", true);
    assert_eq!(
        on_pr(&f.s, f.session, "https://github.com/acme/api/issues/1").unwrap(),
        0
    );
    // A detection guess (suggested) and an agent's inference never write.
    for source in ["agent", "agent_inferred"] {
        let g = fx("manual", true);
        g.s.conn_ref()
            .execute(
                "UPDATE work_links SET source = ?1, state = 'suggested'",
                [source],
            )
            .unwrap();
        assert_eq!(on_pr(&g.s, g.session, PR).unwrap(), 0, "{source}");
    }
    let r = fx("manual", true);
    r.s.conn_ref()
        .execute("UPDATE work_links SET state = 'rejected'", [])
        .unwrap();
    assert_eq!(on_pr(&r.s, r.session, PR).unwrap(), 0);
    assert!(outbox(&off.s).is_empty() && outbox(&f.s).is_empty());
}

#[test]
fn a_session_of_another_org_never_queues_a_write() {
    let f = fx("manual", true);
    let a = f.s.add_org("Company A", Some("#f00"), false).unwrap();
    let b = f.s.add_org("Company B", Some("#00f"), false).unwrap();
    f.s.set_tracker_org(f.tracker, Some(a.id)).unwrap();
    f.s.set_host_org("h", Some(b.id)).unwrap();
    assert_eq!(on_pr(&f.s, f.session, PR).unwrap(), 0);
    // Same org: queued.
    f.s.set_host_org("h", Some(a.id)).unwrap();
    assert_eq!(on_pr(&f.s, f.session, PR).unwrap(), 1);
}

#[test]
fn write_back_is_a_jira_setting() {
    let wb = TrackerSettings {
        write_back: WriteBack {
            pr_remote_link: true,
        },
        ..Default::default()
    };
    for p in ["jira", "jira_dc"] {
        assert!(
            crate::store::validate_tracker_settings(p, wb.clone()).is_ok(),
            "{p}"
        );
    }
    for p in ["github", "asana", "linear"] {
        let e = crate::store::validate_tracker_settings(p, wb.clone()).unwrap_err();
        assert_eq!(e.code, crate::ipc_error::codes::E_INVALID, "{p}");
    }
    // Off serialises to nothing: an older reader sees the same JSON.
    let off = serde_json::to_string(&TrackerSettings::default()).unwrap();
    assert_eq!(off, "{}");
}

// ── the drain ───────────────────────────────────────────────────────────

fn cloud(fake: &FakeTransport) -> JiraCloud {
    JiraCloud::new(
        "https://acme.atlassian.net",
        TrackerConfig {
            key_prefixes: vec!["ABC".into()],
            ..Default::default()
        },
        Some(cred()),
        Arc::new(fake.clone()),
    )
}

#[tokio::test]
async fn a_drain_posts_one_idempotent_remote_link_and_journals_it() {
    let f = fx("manual", true);
    on_pr(&f.s, f.session, PR).unwrap();
    let t = f.s.require_tracker(f.tracker).unwrap();
    let store = Mutex::new(f.s);
    let fake = FakeTransport::new();
    fake.once(
        Method::Post,
        "/rest/api/3/issue/ABC-1/remotelink",
        Ok(Response::json(
            201,
            &serde_json::json!({ "id": 1, "self": "x" }),
        )),
    );
    let r = drain(&t, &cloud(&fake), &store, crate::store::now_unix() + 1).await;
    assert_eq!(
        r,
        DrainReport {
            sent: 1,
            ..Default::default()
        }
    );
    let sent = fake.requests();
    assert_eq!(sent.len(), 1);
    let body = sent[0].json_body().expect("a JSON body");
    assert_eq!(body["globalId"], format!("fleet:pr:{PR}"));
    assert_eq!(body["object"]["url"], PR);
    assert_eq!(body["object"]["title"], "PR: acme/api#42");
    assert!(sent[0]
        .headers
        .iter()
        .any(|(k, v)| k == "Authorization" && v.starts_with("Basic ")));
    {
        let s = store.lock().unwrap();
        assert_eq!(outbox(&s)[0].state, "done");
        let journal: i64 = s
            .conn_ref()
            .query_row(
                "SELECT COUNT(*) FROM work_journal WHERE kind = 'write_back' AND claude_session_id = ?1",
                [CID],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(journal, 1);
    }
    // Done: a second drain sends nothing.
    let r = drain(&t, &cloud(&fake), &store, crate::store::now_unix() + 1).await;
    assert_eq!(r, DrainReport::default());
    assert_eq!(fake.requests().len(), 1);
}

#[tokio::test]
async fn data_center_writes_through_api_v2() {
    let f = fx("started", true);
    on_pr(&f.s, f.session, PR).unwrap();
    let mut t = f.s.require_tracker(f.tracker).unwrap();
    t.provider = "jira_dc".into();
    let store = Mutex::new(f.s);
    let fake = FakeTransport::new();
    fake.once(
        Method::Post,
        "/rest/api/2/issue/ABC-1/remotelink",
        Ok(Response::json(200, &serde_json::json!({ "id": 1 }))),
    );
    let dc = JiraDc::new(
        "https://jira.corp.example",
        TrackerConfig::default(),
        Some(TrackerCredential {
            auth_kind: "bearer".into(),
            username: None,
            secret: crate::store::Secret::new("pat-not-real"),
        }),
        Arc::new(fake.clone()),
    );
    let r = drain(&t, &dc, &store, crate::store::now_unix() + 1).await;
    assert_eq!(r.sent, 1);
    assert!(fake.requests()[0]
        .headers
        .iter()
        .any(|(k, v)| k == "Authorization" && v.starts_with("Bearer ")));
}

#[tokio::test]
async fn a_rate_limit_waits_without_spending_an_attempt_and_a_403_gives_up() {
    let f = fx("manual", true);
    on_pr(&f.s, f.session, PR).unwrap();
    on_pr(&f.s, f.session, "https://github.com/acme/api/pull/43").unwrap();
    let t = f.s.require_tracker(f.tracker).unwrap();
    let store = Mutex::new(f.s);
    let fake = FakeTransport::new();
    fake.always(
        Method::Post,
        "/remotelink",
        Ok(Response::new(429, "").with_header("Retry-After", "30")),
    );
    let now = crate::store::now_unix() + 1;
    let r = drain(&t, &cloud(&fake), &store, now).await;
    // The first is rate-limited and the drain stops: the second is not sent.
    assert_eq!((r.sent, r.retrying), (0, 1));
    assert_eq!(fake.requests().len(), 1);
    {
        let s = store.lock().unwrap();
        let rows = outbox(&s);
        assert_eq!((rows[0].state.as_str(), rows[0].attempts), ("pending", 0));
        assert_eq!(rows[0].next_at, now + 30);
    }
    fake.clear_routes();
    fake.always(Method::Post, "/remotelink", Ok(Response::new(403, "")));
    let r = drain(&t, &cloud(&fake), &store, now + 60).await;
    assert_eq!(r.given_up, 2);
    let s = store.lock().unwrap();
    assert!(outbox(&s).iter().all(|w| w.state == "failed"));
    assert_eq!(s.tracker_write_failures(t.id).unwrap(), 2);
}

#[tokio::test]
async fn the_setting_and_the_org_are_checked_again_before_sending() {
    // Turned off after queueing: nothing is sent, the row waits.
    let f = fx("manual", true);
    on_pr(&f.s, f.session, PR).unwrap();
    let mut t = f.s.require_tracker(f.tracker).unwrap();
    t.settings.write_back.pr_remote_link = false;
    let store = Mutex::new(f.s);
    let fake = FakeTransport::new();
    let r = drain(&t, &cloud(&fake), &store, crate::store::now_unix() + 1).await;
    assert_eq!(r, DrainReport::default());
    assert!(fake.requests().is_empty());
    assert_eq!(outbox(&store.lock().unwrap())[0].state, "pending");

    // The tracker moved to another org: given up, never sent.
    let g = fx("manual", true);
    let a = g.s.add_org("Company A", Some("#f00"), false).unwrap();
    let b = g.s.add_org("Company B", Some("#00f"), false).unwrap();
    g.s.set_host_org("h", Some(a.id)).unwrap();
    on_pr(&g.s, g.session, PR).unwrap();
    g.s.set_tracker_org(g.tracker, Some(b.id)).unwrap();
    let t = g.s.require_tracker(g.tracker).unwrap();
    let store = Mutex::new(g.s);
    let r = drain(&t, &cloud(&fake), &store, crate::store::now_unix() + 1).await;
    assert_eq!(r.given_up, 1);
    assert!(fake.requests().is_empty());
}

#[tokio::test]
async fn a_read_only_provider_refuses_every_write() {
    struct ReadOnly;
    #[async_trait::async_trait]
    impl TrackerProvider for ReadOnly {
        fn caps(&self) -> super::super::Caps {
            super::super::Caps::default()
        }
        async fn probe(&self) -> Result<super::super::TrackerInfo, TrackerError> {
            unreachable!()
        }
        async fn views(
            &self,
            _: &TrackerConfig,
        ) -> Result<Vec<super::super::ViewDef>, TrackerError> {
            unreachable!()
        }
        async fn list(
            &self,
            _: &super::super::ViewDef,
            _: Option<i64>,
            _: Option<String>,
        ) -> Result<super::super::Page, TrackerError> {
            unreachable!()
        }
        async fn fetch(
            &self,
            _: &[super::super::ItemRef],
        ) -> Result<Vec<super::super::Fetched>, TrackerError> {
            unreachable!()
        }
        fn recognize(&self, _: &str, _: super::super::RefCtx<'_>) -> Vec<super::super::ItemRef> {
            Vec::new()
        }
    }
    let op = WriteOp::PrRemoteLink {
        key: "ABC-1".into(),
        url: PR.into(),
        title: "PR".into(),
    };
    assert!(matches!(
        ReadOnly.write(&op).await,
        Err(TrackerError::Refused(_))
    ));
}

#[tokio::test]
async fn a_key_that_is_not_a_jira_key_is_never_put_in_the_path() {
    let fake = FakeTransport::new();
    let op = WriteOp::PrRemoteLink {
        key: "../../admin".into(),
        url: PR.into(),
        title: "PR".into(),
    };
    assert!(matches!(
        cloud(&fake).write(&op).await,
        Err(TrackerError::Invalid(_))
    ));
    assert!(fake.requests().is_empty());
}
