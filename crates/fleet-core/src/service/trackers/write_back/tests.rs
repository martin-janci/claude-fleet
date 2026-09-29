use super::*;
use crate::net::https::{FakeTransport, Method, Response, TransportError};
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
    item: i64,
    session: i64,
}

fn fx(source: &str, write_back: bool) -> Fx {
    fx_with(Some(source), write_back)
}

/// [`fx`], or with the session not linked at all when `source` is `None`.
fn fx_with(source: Option<&str>, write_back: bool) -> Fx {
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
    if let Some(source) = source {
        s.link_session_work(session, WorkTarget::Item(item), source)
            .unwrap();
    }
    Fx {
        s,
        tracker: t.id,
        item,
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

/// D34: a suggestion an AGENT confirmed (a per-host token, the operator) is
/// recorded as `agent`, so it never writes to the tracker; the same
/// suggestion confirmed by a person does. Through the one decision entry
/// the MCP tool and the desktop share, with the caller's decider.
#[test]
fn a_suggestion_an_agent_confirmed_never_queues_a_write() {
    use crate::service::orgs::OrgScope;
    use crate::service::work::{work_link_as, WorkLinkArgs};
    use crate::store::Decider;
    for (decider, source, queued) in [(Decider::Agent, "agent", 0), (Decider::Person, "manual", 1)]
    {
        let f = fx("manual", true);
        f.s.conn_ref()
            .execute(
                "UPDATE work_links SET source = 'branch', state = 'suggested', \
                   is_primary = 0, rule = 'R3b'",
                [],
            )
            .unwrap();
        let link_id = f.s.session_work_links(f.session).unwrap()[0].id;
        let session = f.session;
        let store = std::sync::Mutex::new(f.s);
        let args = WorkLinkArgs {
            session_id: Some(session),
            action: "confirm".into(),
            link_id: Some(link_id),
            // What a caller claims does not matter.
            source: Some("manual".into()),
            ..Default::default()
        };
        work_link_as(&args, &store, &OrgScope::All, decider).unwrap();
        let s = store.into_inner().unwrap();
        let l = s.get_work_link(link_id).unwrap().unwrap();
        assert_eq!(
            (l.state.as_str(), l.source.as_str()),
            ("confirmed", source),
            "{decider:?}"
        );
        assert_eq!(on_pr(&s, session, PR).unwrap(), queued, "{decider:?}");
    }
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

// ── a PR that was there first ───────────────────────────────────────────

/// The PR probe already saw the PR (signals stamped): it will not queue
/// again until they change.
fn with_pr(s: &Store, session: i64) {
    s.conn_ref()
        .execute(
            "UPDATE sessions SET pr_url = ?1, pr_signals = '{}', pr_signals_at = 1 WHERE id = ?2",
            rusqlite::params![PR, session],
        )
        .unwrap();
}

fn link_through_service(f: Fx, decider: crate::store::Decider, source: &str) -> (Store, i64) {
    use crate::service::orgs::OrgScope;
    use crate::service::work::{work_link_as, WorkLinkArgs};
    let store = std::sync::Mutex::new(f.s);
    let args = WorkLinkArgs {
        session_id: Some(f.session),
        action: "link".into(),
        item_id: Some(f.item),
        source: Some(source.into()),
        ..Default::default()
    };
    work_link_as(&args, &store, &OrgScope::All, decider).unwrap();
    (store.into_inner().unwrap(), f.tracker)
}

#[test]
fn a_person_linking_a_session_that_already_has_a_pr_queues_its_write() {
    use crate::store::Decider;
    let f = fx_with(None, true);
    with_pr(&f.s, f.session);
    let (s, tracker) = link_through_service(f, Decider::Person, "manual");
    let rows = outbox(&s);
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (
            rows[0].tracker_id,
            rows[0].item_key.as_str(),
            rows[0].url.as_str()
        ),
        (tracker, "ABC-1", PR)
    );
    assert_eq!(rows[0].state, "pending");
    // An agent's link (whatever source it claims) and a person recording
    // an agent's source never write.
    for (decider, source) in [(Decider::Agent, "manual"), (Decider::Person, "agent")] {
        let g = fx_with(None, true);
        with_pr(&g.s, g.session);
        let (s, _) = link_through_service(g, decider, source);
        assert!(outbox(&s).is_empty(), "{decider:?} {source}");
    }
}

#[test]
fn a_person_confirming_a_suggestion_after_the_pr_queues_its_write() {
    use crate::service::orgs::OrgScope;
    use crate::service::work::{work_link_as, WorkLinkArgs};
    use crate::store::Decider;
    let f = fx("manual", true);
    f.s.conn_ref()
        .execute(
            "UPDATE work_links SET source = 'branch', state = 'suggested', \
               is_primary = 0, rule = 'R3b'",
            [],
        )
        .unwrap();
    with_pr(&f.s, f.session);
    let link_id = f.s.session_work_links(f.session).unwrap()[0].id;
    let session = f.session;
    let store = std::sync::Mutex::new(f.s);
    let args = WorkLinkArgs {
        session_id: Some(session),
        action: "confirm".into(),
        link_id: Some(link_id),
        ..Default::default()
    };
    work_link_as(&args, &store, &OrgScope::All, Decider::Person).unwrap();
    let s = store.into_inner().unwrap();
    let rows = outbox(&s);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].link_id, Some(link_id));
}

fn turn_write_back_on(s: Store, tracker: i64) -> Store {
    use crate::service::trackers::admin::{admin_sync, WorkAdminArgs};
    let store = std::sync::Mutex::new(s);
    admin_sync(
        &WorkAdminArgs {
            action: "update".into(),
            tracker_id: Some(tracker),
            settings: Some(serde_json::json!({ "write_back": { "pr_remote_link": true } })),
            ..Default::default()
        },
        &store,
    )
    .unwrap();
    store.into_inner().unwrap()
}

#[test]
fn turning_write_back_on_queues_the_prs_already_linked_once() {
    let f = fx("manual", false);
    with_pr(&f.s, f.session);
    assert!(outbox(&f.s).is_empty());
    let s = turn_write_back_on(f.s, f.tracker);
    let rows = outbox(&s);
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].item_key.as_str(), rows[0].url.as_str()),
        ("ABC-1", PR)
    );
    // Already on: a second update queues nothing more.
    let s = turn_write_back_on(s, f.tracker);
    assert_eq!(outbox(&s).len(), 1);
}

#[test]
fn turning_write_back_on_skips_an_agent_link_and_another_org() {
    let agent = fx("agent", false);
    with_pr(&agent.s, agent.session);
    let s = turn_write_back_on(agent.s, agent.tracker);
    assert!(outbox(&s).is_empty());

    let f = fx("manual", false);
    let a = f.s.add_org("Company A", Some("#f00"), false).unwrap();
    let b = f.s.add_org("Company B", Some("#00f"), false).unwrap();
    f.s.set_tracker_org(f.tracker, Some(a.id)).unwrap();
    f.s.set_host_org("h", Some(b.id)).unwrap();
    with_pr(&f.s, f.session);
    let s = turn_write_back_on(f.s, f.tracker);
    assert!(outbox(&s).is_empty());
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

// ── ported from the parallel M13.4e build ───────────────────────────────

/// Only a link a person made is written: a confirmed `agent` link (an
/// agent's own `work_link`) and a `manual` suggestion never queue.
#[test]
fn a_link_no_person_made_or_decided_never_queues() {
    let f = fx("agent", true);
    let state: String =
        f.s.conn_ref()
            .query_row("SELECT state FROM work_links", [], |r| r.get(0))
            .unwrap();
    assert_eq!(state, "confirmed");
    assert_eq!(on_pr(&f.s, f.session, PR).unwrap(), 0);
    let g = fx("manual", true);
    g.s.conn_ref()
        .execute("UPDATE work_links SET state = 'suggested'", [])
        .unwrap();
    assert_eq!(on_pr(&g.s, g.session, PR).unwrap(), 0);
    assert!(outbox(&f.s).is_empty() && outbox(&g.s).is_empty());
}

/// The same PR on the same item from two sessions is one write.
#[test]
fn two_sessions_with_the_same_pr_queue_one_write() {
    let f = fx("manual", true);
    let item: i64 =
        f.s.conn_ref()
            .query_row("SELECT item_id FROM work_links", [], |r| r.get(0))
            .unwrap();
    let other =
        f.s.upsert_session("dev-abc-1-b", "h", None, None, 1, 1, "running", None)
            .unwrap();
    f.s.link_session_work(other, WorkTarget::Item(item), "started")
        .unwrap();
    assert_eq!(on_pr(&f.s, f.session, PR).unwrap(), 1);
    assert_eq!(on_pr(&f.s, other, PR).unwrap(), 0);
    assert_eq!(outbox(&f.s).len(), 1);
}

/// A write-back flag on a tracker that is not Jira (hand-edited, or the
/// provider changed under it) queues nothing and sends nothing.
#[tokio::test]
async fn a_write_back_flag_on_a_non_jira_tracker_writes_nothing() {
    let f = fx("manual", true);
    on_pr(&f.s, f.session, PR).unwrap();
    f.s.conn_ref()
        .execute("UPDATE trackers SET provider = 'linear'", [])
        .unwrap();
    assert_eq!(
        on_pr(&f.s, f.session, "https://github.com/acme/api/pull/43").unwrap(),
        0
    );
    let t = f.s.require_tracker(f.tracker).unwrap();
    assert!(t.settings.write_back.pr_remote_link);
    let store = Mutex::new(f.s);
    let fake = FakeTransport::new();
    let r = drain(&t, &cloud(&fake), &store, crate::store::now_unix() + 1).await;
    assert_eq!(r, DrainReport::default());
    assert!(fake.requests().is_empty());
    assert_eq!(outbox(&store.lock().unwrap()).len(), 1);
}

/// A failure that may heal (the network) backs off, a minute after the
/// first, and is sent again once due.
#[tokio::test]
async fn a_network_error_backs_off_and_the_write_lands_when_due() {
    let f = fx("manual", true);
    on_pr(&f.s, f.session, PR).unwrap();
    let t = f.s.require_tracker(f.tracker).unwrap();
    let store = Mutex::new(f.s);
    let fake = FakeTransport::new();
    fake.once(
        Method::Post,
        "/remotelink",
        Err(TransportError::Connect("connection reset".into())),
    );
    let now = crate::store::now_unix() + 1;
    let r = drain(&t, &cloud(&fake), &store, now).await;
    assert_eq!((r.sent, r.retrying, r.given_up), (0, 1, 0));
    {
        let s = store.lock().unwrap();
        let w = &outbox(&s)[0];
        assert_eq!((w.state.as_str(), w.attempts), ("pending", 1));
        assert_eq!(w.next_at, now + backoff_secs(0));
        assert!(w.last_error.is_some());
    }
    // Not due yet: nothing is sent.
    let r = drain(&t, &cloud(&fake), &store, now + backoff_secs(0) - 1).await;
    assert_eq!(r, DrainReport::default());
    assert_eq!(fake.requests().len(), 1);
    fake.once(
        Method::Post,
        "/remotelink",
        Ok(Response::json(201, &serde_json::json!({ "id": 1 }))),
    );
    let r = drain(&t, &cloud(&fake), &store, now + backoff_secs(0)).await;
    assert_eq!(r.sent, 1);
    let s = store.lock().unwrap();
    let w = &outbox(&s)[0];
    assert_eq!((w.state.as_str(), w.last_error.as_deref()), ("done", None));
}

/// A write that keeps failing gives up at the last allowed attempt, counts
/// as a failure, and is never sent again.
#[tokio::test]
async fn a_write_that_keeps_failing_gives_up_after_the_last_attempt() {
    let f = fx("manual", true);
    on_pr(&f.s, f.session, PR).unwrap();
    let t = f.s.require_tracker(f.tracker).unwrap();
    let store = Mutex::new(f.s);
    let fake = FakeTransport::new();
    fake.always(
        Method::Post,
        "/remotelink",
        Err(TransportError::Connect("no route to host".into())),
    );
    let mut now = crate::store::now_unix() + 1;
    let mut last = DrainReport::default();
    for _ in 0..crate::store::WRITE_MAX_ATTEMPTS {
        last = drain(&t, &cloud(&fake), &store, now).await;
        now += MAX_BACKOFF_SECS;
    }
    assert_eq!(last.given_up, 1, "{last:?}");
    let n = crate::store::WRITE_MAX_ATTEMPTS as usize;
    assert_eq!(fake.requests().len(), n);
    {
        let s = store.lock().unwrap();
        let w = &outbox(&s)[0];
        assert_eq!(
            (w.state.as_str(), w.attempts),
            ("failed", crate::store::WRITE_MAX_ATTEMPTS)
        );
        assert_eq!(s.tracker_write_failures(t.id).unwrap(), 1);
    }
    let r = drain(&t, &cloud(&fake), &store, now + MAX_BACKOFF_SECS).await;
    assert_eq!(r, DrainReport::default());
    assert_eq!(fake.requests().len(), n);
}

/// What a failed write stores as its error never carries the credential,
/// whatever the transport or the tracker echoed.
#[tokio::test]
async fn a_stored_error_never_carries_the_credential() {
    let secret = cred().secret.expose().to_string();
    let f = fx("manual", true);
    on_pr(&f.s, f.session, PR).unwrap();
    on_pr(&f.s, f.session, "https://github.com/acme/api/pull/43").unwrap();
    let t = f.s.require_tracker(f.tracker).unwrap();
    let store = Mutex::new(f.s);
    let fake = FakeTransport::new();
    fake.once(
        Method::Post,
        "/remotelink",
        Err(TransportError::Connect(format!("proxy said: bad {secret}"))),
    );
    fake.once(
        Method::Post,
        "/remotelink",
        Ok(Response::new(500, format!("echo: {secret}"))),
    );
    let r = drain(&t, &cloud(&fake), &store, crate::store::now_unix() + 1).await;
    assert_eq!(r.retrying + r.given_up, 2, "{r:?}");
    let s = store.lock().unwrap();
    for w in outbox(&s) {
        let e = w.last_error.unwrap_or_default();
        assert!(!e.is_empty());
        assert!(!e.contains(&secret), "{e}");
    }
}

/// Settled writes go once they are older than the journal's retention
/// window; a pending one stays, and a window of `0` keeps them all. The GC
/// retention sweep (M12.3) does it, not the drain, so a failing tracker's
/// outbox shrinks too.
#[tokio::test]
async fn the_gc_sweep_drops_settled_writes_past_the_journal_window() {
    let f = fx("manual", true);
    on_pr(&f.s, f.session, PR).unwrap();
    on_pr(&f.s, f.session, "https://github.com/acme/api/pull/43").unwrap();
    let ids: Vec<i64> = outbox(&f.s).iter().map(|w| w.id).collect();
    f.s.finish_tracker_write(ids[0]).unwrap();
    let long_ago = crate::store::now_unix() - 5 * 365 * 86_400;
    f.s.conn_ref()
        .execute(
            "UPDATE tracker_writes SET updated_at = ?1, next_at = ?2",
            rusqlite::params![long_ago, i64::MAX],
        )
        .unwrap();
    crate::service::settings::set(
        &f.s,
        crate::service::settings::WORK_RETENTION_JOURNAL_DAYS,
        "0",
    )
    .unwrap();
    let t = f.s.require_tracker(f.tracker).unwrap();
    let store = Mutex::new(f.s);
    let fake = FakeTransport::new();
    let now = crate::store::now_unix();
    let sweep = |store: &Mutex<Store>| crate::service::work::retention::sweep(store, now);
    assert_eq!(sweep(&store).tracker_writes, 0);
    assert_eq!(outbox(&store.lock().unwrap()).len(), 2, "0 keeps forever");
    crate::service::settings::set(
        &store.lock().unwrap(),
        crate::service::settings::WORK_RETENTION_JOURNAL_DAYS,
        "365",
    )
    .unwrap();
    // The drain sends nothing here (the pending one is not due) and sweeps
    // nothing: retention is the GC's.
    drain(&t, &cloud(&fake), &store, now).await;
    assert_eq!(
        outbox(&store.lock().unwrap()).len(),
        2,
        "the drain never sweeps"
    );
    assert_eq!(sweep(&store).tracker_writes, 1);
    let left = outbox(&store.lock().unwrap());
    assert_eq!(left.len(), 1, "{left:?}");
    assert_eq!((left[0].id, left[0].state.as_str()), (ids[1], "pending"));
    assert!(fake.requests().is_empty());
}
