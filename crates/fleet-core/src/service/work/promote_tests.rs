use super::*;
use crate::ipc_error::codes;
use crate::net::https::{FakeTransport, Method, Response};
use crate::service::trackers::asana::Asana;
use crate::service::trackers::jira::JiraCloud;
use crate::service::trackers::write_back::{drain, DrainReport};
use crate::store::{
    NativeItem, TrackerConfig, TrackerCredential, TrackerItemWrite, TrackerSettings, WriteBack,
};
use serde_json::json;
use std::sync::Arc;

const WS: &str = "1200000000000001";
const P1: &str = "1200000000001001";

/// A Jira tracker owning ABC (creating on as `create`) with ABC-1 cached,
/// a local subtask under ABC-1 and a local task at the top.
struct Fx {
    store: Mutex<Store>,
    tracker: i64,
    ticket: i64,
    subtask: i64,
    top: i64,
}

fn jira_fx(create: bool) -> Fx {
    let s = Store::open_in_memory().unwrap();
    let t = s
        .add_tracker("jira", "Acme Jira", "https://acme.atlassian.net")
        .unwrap();
    s.set_tracker_probe(
        t.id,
        None,
        &TrackerConfig {
            key_prefixes: vec!["ABC".into(), "OPS".into()],
            ..Default::default()
        },
    )
    .unwrap();
    s.set_tracker_settings(t.id, &settings(create)).unwrap();
    let ticket = s
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
    let subtask = s
        .create_native_item(&NativeItem {
            title: "Write the migration",
            parent_id: Some(ticket),
            project_id: None,
            notes: Some("Backfill the old rows first."),
        })
        .unwrap()
        .id;
    let top = s
        .create_native_item(&NativeItem {
            title: "Audit the login flow",
            parent_id: None,
            project_id: None,
            notes: None,
        })
        .unwrap()
        .id;
    Fx {
        store: Mutex::new(s),
        tracker: t.id,
        ticket,
        subtask,
        top,
    }
}

fn settings(create: bool) -> TrackerSettings {
    TrackerSettings {
        write_back: WriteBack {
            create_tickets: create,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn args(item: i64) -> WorkLinkArgs {
    WorkLinkArgs {
        action: "promote".into(),
        item_id: Some(item),
        ..Default::default()
    }
}

fn host() -> OrgScope {
    OrgScope::Host {
        alias: "h".into(),
        org: None,
        isolated: Default::default(),
    }
}

fn cloud(fake: &FakeTransport) -> JiraCloud {
    JiraCloud::new(
        "https://acme.atlassian.net",
        TrackerConfig {
            key_prefixes: vec!["ABC".into(), "OPS".into()],
            ..Default::default()
        },
        Some(TrackerCredential {
            auth_kind: "basic".into(),
            username: Some("dev@example.com".into()),
            secret: crate::store::Secret::new("ATATT3xFfGF0-test-token-not-real-0000"),
        }),
        Arc::new(fake.clone()),
    )
}

fn issue_types(fake: &FakeTransport, project: &str) {
    fake.once(
        Method::Get,
        &format!("/rest/api/3/issue/createmeta/{project}/issuetypes"),
        Ok(Response::json(
            200,
            &json!({ "issueTypes": [
                { "id": "10001", "name": "Story", "subtask": false, "hierarchyLevel": 0 },
                { "id": "10002", "name": "Task", "subtask": false, "hierarchyLevel": 0 },
                { "id": "10003", "name": "Sub-task", "subtask": true, "hierarchyLevel": -1 },
            ] }),
        )),
    );
}

fn code(r: Result<WorkItemRow, IpcError>) -> String {
    r.expect_err("refused").code
}

#[test]
fn issue_types_are_picked_for_a_task_and_a_subtask() {
    use crate::service::trackers::jira_common::pick_issue_type;
    let cloud = json!({ "issueTypes": [
        { "id": "1", "name": "Epic", "subtask": false, "hierarchyLevel": 1 },
        { "id": "2", "name": "Bug", "subtask": false, "hierarchyLevel": 0 },
        { "id": "3", "name": "task", "subtask": false, "hierarchyLevel": 0 },
        { "id": "4", "name": "Subtask", "subtask": true, "hierarchyLevel": -1 },
    ] });
    assert_eq!(pick_issue_type(&cloud, false).as_deref(), Some("3"));
    assert_eq!(pick_issue_type(&cloud, true).as_deref(), Some("4"));
    // Data Center lists `values`; with no "Task", the first standard type.
    let dc = json!({ "values": [
        { "id": "7", "name": "Epic", "subtask": false, "hierarchyLevel": 1 },
        { "id": "8", "name": "Story", "subtask": false, "hierarchyLevel": 0 },
    ] });
    assert_eq!(pick_issue_type(&dc, false).as_deref(), Some("8"));
    assert_eq!(pick_issue_type(&dc, true), None);
    assert_eq!(pick_issue_type(&json!({}), false), None);
}

#[test]
fn a_subtask_of_a_ticket_targets_its_tracker_and_a_top_task_every_creating_one() {
    let f = jira_fx(true);
    let s = f.store.lock().unwrap();
    let trackers: std::collections::HashMap<_, _> = s
        .list_trackers()
        .unwrap()
        .into_iter()
        .map(|t| (t.id, t))
        .collect();
    let sub = s.get_work_item(f.subtask).unwrap().unwrap();
    let t = targets(&s, &OrgScope::All, &sub, &trackers).unwrap();
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].parent_key.as_deref(), Some("ABC-1"));
    assert!(t[0].containers.is_empty());
    let top = s.get_work_item(f.top).unwrap().unwrap();
    let t = targets(&s, &OrgScope::All, &top, &trackers).unwrap();
    assert_eq!(t.len(), 1);
    assert!(t[0].container_required);
    assert_eq!(
        t[0].containers,
        vec![("ABC".into(), "ABC".into()), ("OPS".into(), "OPS".into())]
    );
    // An agent, a tracker's ticket: nowhere.
    assert!(targets(&s, &host(), &top, &trackers).unwrap().is_empty());
    let ticket = s.get_work_item(f.ticket).unwrap().unwrap();
    assert!(targets(&s, &OrgScope::All, &ticket, &trackers)
        .unwrap()
        .is_empty());
    drop(s);
    // Creating off: nowhere either.
    let f = jira_fx(false);
    let s = f.store.lock().unwrap();
    let trackers: std::collections::HashMap<_, _> = s
        .list_trackers()
        .unwrap()
        .into_iter()
        .map(|t| (t.id, t))
        .collect();
    let top = s.get_work_item(f.top).unwrap().unwrap();
    assert!(targets(&s, &OrgScope::All, &top, &trackers)
        .unwrap()
        .is_empty());
}

#[test]
fn promoting_is_refused_with_nothing_queued_where_it_may_not_go() {
    let f = jira_fx(false);
    // The setting is off.
    assert_eq!(
        code(promote(&args(f.subtask), &f.store, &OrgScope::All, None)),
        codes::E_INVALID_STATE
    );
    let f = jira_fx(true);
    // An agent never creates a ticket; one whose host has no link to the
    // task does not even see it (the isolation matrix has the agent that
    // does: refused `E_FORBIDDEN`).
    assert_eq!(
        code(promote(&args(f.subtask), &f.store, &host(), None)),
        codes::E_NOTFOUND
    );
    // A tracker's ticket is one already.
    assert_eq!(
        code(promote(&args(f.ticket), &f.store, &OrgScope::All, None)),
        codes::E_INVALID
    );
    // A task at the top needs a tracker, and a project the site owns.
    assert_eq!(
        code(promote(&args(f.top), &f.store, &OrgScope::All, None)),
        codes::E_INVALID
    );
    let mut a = args(f.top);
    a.tracker_id = Some(f.tracker);
    assert_eq!(
        code(promote(&a, &f.store, &OrgScope::All, None)),
        codes::E_INVALID,
        "two projects: one must be picked"
    );
    a.container = Some("XYZ".into());
    assert_eq!(
        code(promote(&a, &f.store, &OrgScope::All, None)),
        codes::E_INVALID
    );
    // A subtask names no other tracker.
    let mut a = args(f.subtask);
    a.tracker_id = Some(f.tracker + 1);
    assert_eq!(
        code(promote(&a, &f.store, &OrgScope::All, None)),
        codes::E_INVALID
    );
    // A subtask of a task of fleet's own waits for its parent.
    let under_local = f
        .store
        .lock()
        .unwrap()
        .create_native_item(&NativeItem {
            title: "A step",
            parent_id: Some(f.top),
            project_id: None,
            notes: None,
        })
        .unwrap()
        .id;
    let e = promote(&args(under_local), &f.store, &OrgScope::All, None).unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
    assert!(e.message.contains("promote"), "{}", e.message);
    // An epic stays fleet's own.
    f.store.lock().unwrap().set_local_epic(f.top, true).unwrap();
    let mut a = args(f.top);
    a.tracker_id = Some(f.tracker);
    a.container = Some("abc".into());
    assert_eq!(
        code(promote(&a, &f.store, &OrgScope::All, None)),
        codes::E_INVALID
    );
    let s = f.store.lock().unwrap();
    let n: i64 = s
        .conn_ref()
        .query_row("SELECT COUNT(*) FROM tracker_writes", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0, "nothing queued for any refusal");
}

#[tokio::test]
async fn a_subtask_of_a_jira_ticket_becomes_its_sub_task_and_keeps_everything() {
    let f = jira_fx(true);
    let old_key = {
        let s = f.store.lock().unwrap();
        s.add_comment(f.subtask, "desktop", None, "Started on it")
            .unwrap();
        s.get_work_item(f.subtask).unwrap().unwrap().key.unwrap()
    };
    let row = promote(&args(f.subtask), &f.store, &OrgScope::All, None).unwrap();
    assert_eq!(row.source, "local", "fleet's own until the tracker answers");
    // Twice is refused while it is pending.
    assert_eq!(
        code(promote(&args(f.subtask), &f.store, &OrgScope::All, None)),
        codes::E_INVALID_STATE
    );
    {
        let s = f.store.lock().unwrap();
        let w = s.promotion_of(f.subtask).unwrap().unwrap();
        assert_eq!(
            (
                w.state.as_str(),
                w.item_key.as_str(),
                w.external_id.as_deref()
            ),
            ("pending", "ABC-1", Some("10001"))
        );
    }
    let t = f.store.lock().unwrap().require_tracker(f.tracker).unwrap();
    let fake = FakeTransport::new();
    issue_types(&fake, "ABC");
    fake.once(
        Method::Post,
        "/rest/api/3/issue",
        Ok(Response::json(
            201,
            &json!({ "id": "10050", "key": "ABC-7", "self": "https://acme.atlassian.net/rest/api/3/issue/10050" }),
        )),
    );
    let r = drain(&t, &cloud(&fake), &f.store, crate::store::now_unix() + 1).await;
    assert_eq!(r.sent, 1, "{r:?}");
    let sent = fake.requests();
    let create = sent
        .iter()
        .find(|r| r.method == Method::Post && r.url.ends_with("/rest/api/3/issue"))
        .expect("the create");
    let body = create.json_body().unwrap();
    let fields = &body["fields"];
    assert_eq!(fields["project"]["key"], "ABC");
    assert_eq!(fields["parent"]["key"], "ABC-1");
    assert_eq!(fields["issuetype"]["id"], "10003");
    assert_eq!(fields["summary"], "Write the migration");
    assert_eq!(fields["description"]["type"], "doc");
    assert_eq!(
        fields["description"]["content"][0]["content"][0]["text"],
        "Backfill the old rows first."
    );
    assert_eq!(
        fields["description"]["content"][1]["content"][0]["text"],
        "— via fleet"
    );
    {
        let s = f.store.lock().unwrap();
        let item = s.get_work_item(f.subtask).unwrap().unwrap();
        assert_eq!(item.source, "jira");
        assert_eq!(item.key.as_deref(), Some("ABC-7"));
        assert_eq!(item.external_id.as_deref(), Some("10050"));
        assert_eq!(item.tracker_id, Some(f.tracker));
        assert_eq!(item.parent_id, Some(f.ticket), "still under ABC-1");
        assert!(item.aliases.contains(&old_key), "{:?}", item.aliases);
        assert_eq!(item.display_key, None);
        assert_eq!(s.item_comments(f.subtask).unwrap().len(), 1);
        assert!(
            s.native_children(f.ticket)
                .unwrap()
                .iter()
                .any(|c| c.id == f.subtask),
            "still listed among the ticket's subtasks"
        );
        let w = s.promotion_of(f.subtask).unwrap().unwrap();
        assert_eq!(w.state, "done");
        assert_eq!(
            w.remote_url.as_deref(),
            Some("https://acme.atlassian.net/browse/ABC-7")
        );
    }
    // Sent once: a create is not idempotent at the tracker.
    let r = drain(&t, &cloud(&fake), &f.store, crate::store::now_unix() + 1).await;
    assert_eq!(r, DrainReport::default());
    // And it is a ticket now.
    assert_eq!(
        code(promote(&args(f.subtask), &f.store, &OrgScope::All, None)),
        codes::E_INVALID
    );
}

#[tokio::test]
async fn a_top_task_goes_into_the_project_picked_and_a_failed_one_may_try_again() {
    let f = jira_fx(true);
    let mut a = args(f.top);
    a.tracker_id = Some(f.tracker);
    a.container = Some("ops".into());
    promote(&a, &f.store, &OrgScope::All, None).unwrap();
    let t = f.store.lock().unwrap().require_tracker(f.tracker).unwrap();
    let fake = FakeTransport::new();
    issue_types(&fake, "OPS");
    fake.once(
        Method::Post,
        "/rest/api/3/issue",
        Ok(Response::json(403, &json!({}))),
    );
    let r = drain(&t, &cloud(&fake), &f.store, crate::store::now_unix() + 1).await;
    assert_eq!(r.given_up, 1);
    {
        let s = f.store.lock().unwrap();
        assert_eq!(s.get_work_item(f.top).unwrap().unwrap().source, "local");
        let trackers = s
            .list_trackers()
            .unwrap()
            .into_iter()
            .map(|t| (t.id, t))
            .collect();
        let v = promotion_view(&s, f.top, &trackers).unwrap().unwrap();
        assert_eq!(v.state, "failed");
        assert_eq!(v.tracker, "Acme Jira");
        assert!(v.error.is_some());
    }
    // A person tries again.
    promote(&a, &f.store, &OrgScope::All, None).unwrap();
    issue_types(&fake, "OPS");
    fake.once(
        Method::Post,
        "/rest/api/3/issue",
        Ok(Response::json(
            201,
            &json!({ "id": "20001", "key": "OPS-3" }),
        )),
    );
    let r = drain(&t, &cloud(&fake), &f.store, crate::store::now_unix() + 1).await;
    assert_eq!(r.sent, 1, "{r:?}");
    let create = fake
        .requests()
        .into_iter()
        .rfind(|r| r.method == Method::Post && r.url.ends_with("/rest/api/3/issue"))
        .unwrap();
    let body = create.json_body().unwrap();
    assert_eq!(body["fields"]["project"]["key"], "OPS");
    assert_eq!(body["fields"]["issuetype"]["id"], "10002");
    assert!(body["fields"].get("parent").is_none());
    let s = f.store.lock().unwrap();
    let item = s.get_work_item(f.top).unwrap().unwrap();
    assert_eq!(
        (item.source.as_str(), item.key.as_deref()),
        ("jira", Some("OPS-3"))
    );
    assert_eq!(item.parent_id, None);
}

#[tokio::test]
async fn the_setting_is_checked_again_before_the_create_is_sent() {
    let f = jira_fx(true);
    promote(&args(f.subtask), &f.store, &OrgScope::All, None).unwrap();
    let mut t = f.store.lock().unwrap().require_tracker(f.tracker).unwrap();
    t.settings.write_back.create_tickets = false;
    let fake = FakeTransport::new();
    let r = drain(&t, &cloud(&fake), &f.store, crate::store::now_unix() + 1).await;
    assert_eq!(r, DrainReport::default());
    assert!(fake.requests().is_empty());
    let s = f.store.lock().unwrap();
    assert_eq!(s.promotion_of(f.subtask).unwrap().unwrap().state, "pending");
}

#[tokio::test]
async fn an_asana_task_goes_into_a_project_and_draws_the_trackers_key() {
    let s = Store::open_in_memory().unwrap();
    let t = s
        .add_tracker("asana", "Acme Asana", "https://app.asana.com")
        .unwrap();
    let config = TrackerConfig {
        workspace: Some(WS.into()),
        projects: vec![(P1.into(), "Platform".into())],
        ..Default::default()
    };
    s.set_tracker_probe(t.id, None, &config).unwrap();
    s.set_tracker_settings(t.id, &settings(true)).unwrap();
    let top = s
        .create_native_item(&NativeItem {
            title: "Plan the offsite",
            parent_id: None,
            project_id: None,
            notes: Some("Two days."),
        })
        .unwrap()
        .id;
    let store = Mutex::new(s);
    let mut a = args(top);
    a.tracker_id = Some(t.id);
    a.container = Some("not-a-gid".into());
    assert_eq!(
        code(promote(&a, &store, &OrgScope::All, None)),
        codes::E_INVALID
    );
    a.container = Some(P1.into());
    promote(&a, &store, &OrgScope::All, None).unwrap();
    let fake = FakeTransport::new();
    fake.once(
        Method::Post,
        "/tasks",
        Ok(Response::json(
            201,
            &json!({ "data": { "gid": "1209000000000777",
                               "permalink_url": "https://app.asana.com/0/0/1209000000000777" } }),
        )),
    );
    let provider = Asana::new(
        "https://app.asana.com",
        config,
        TrackerSettings::default(),
        Some(TrackerCredential {
            auth_kind: "bearer".into(),
            username: None,
            secret: crate::store::Secret::new(
                "2/1200000000000100/1209999999999999:0123456789abcdef",
            ),
        }),
        Arc::new(fake.clone()),
    );
    let t = store.lock().unwrap().require_tracker(t.id).unwrap();
    let r = drain(&t, &provider, &store, crate::store::now_unix() + 1).await;
    assert_eq!(r.sent, 1, "{r:?}");
    let body = fake.requests()[0].json_body().unwrap();
    assert_eq!(body["data"]["name"], "Plan the offsite");
    assert_eq!(body["data"]["workspace"], WS);
    assert_eq!(body["data"]["projects"], json!([P1]));
    assert!(body["data"]["notes"]
        .as_str()
        .unwrap()
        .starts_with("Two days.\n\n— via fleet"));
    let s = store.lock().unwrap();
    let item = s.get_work_item(top).unwrap().unwrap();
    assert_eq!(item.source, "asana");
    assert_eq!(item.key.as_deref(), Some("asana:1209000000000777"));
    assert!(item.display_key.is_some(), "the tracker's numbering");
    assert_eq!(
        item.url.as_deref(),
        Some("https://app.asana.com/0/0/1209000000000777")
    );
}

#[tokio::test]
async fn an_asana_subtask_goes_under_its_parent_task() {
    let s = Store::open_in_memory().unwrap();
    let t = s
        .add_tracker("asana", "Acme Asana", "https://app.asana.com")
        .unwrap();
    s.set_tracker_settings(t.id, &settings(true)).unwrap();
    let parent = s
        .upsert_tracker_item(
            t.id,
            &TrackerItemWrite {
                external_id: "1209000000000001".into(),
                key: Some("asana:1209000000000001".into()),
                title: "Launch".into(),
                status_name: "To do".into(),
                status_category: "todo".into(),
                ..Default::default()
            },
        )
        .unwrap()
        .id;
    let sub = s
        .create_native_item(&NativeItem {
            title: "Book the venue",
            parent_id: Some(parent),
            project_id: None,
            notes: None,
        })
        .unwrap()
        .id;
    let store = Mutex::new(s);
    promote(&args(sub), &store, &OrgScope::All, None).unwrap();
    let fake = FakeTransport::new();
    fake.once(
        Method::Post,
        "/tasks/1209000000000001/subtasks",
        Ok(Response::json(
            201,
            &json!({ "data": { "gid": "1209000000000002" } }),
        )),
    );
    let provider = Asana::new(
        "https://app.asana.com",
        TrackerConfig::default(),
        TrackerSettings::default(),
        Some(TrackerCredential {
            auth_kind: "bearer".into(),
            username: None,
            secret: crate::store::Secret::new(
                "2/1200000000000100/1209999999999999:0123456789abcdef",
            ),
        }),
        Arc::new(fake.clone()),
    );
    let t = store.lock().unwrap().require_tracker(t.id).unwrap();
    let r = drain(&t, &provider, &store, crate::store::now_unix() + 1).await;
    assert_eq!(r.sent, 1, "{r:?}");
    let body = fake.requests()[0].json_body().unwrap();
    assert_eq!(body["data"]["name"], "Book the venue");
    assert!(body["data"].get("workspace").is_none());
    let s = store.lock().unwrap();
    let item = s.get_work_item(sub).unwrap().unwrap();
    assert_eq!(item.key.as_deref(), Some("asana:1209000000000002"));
    assert_eq!(item.parent_id, Some(parent));
}

#[test]
fn every_provider_takes_the_create_tickets_setting() {
    for provider in crate::store::TRACKER_PROVIDERS {
        let r = crate::store::validate_tracker_settings(provider, settings(true));
        assert_eq!(
            r.is_ok(),
            crate::service::trackers::creates_items(provider),
            "{provider}: {r:?}"
        );
    }
    for provider in ["jira", "jira_dc", "asana", "linear", "github"] {
        assert!(
            crate::service::trackers::creates_items(provider),
            "{provider}"
        );
    }
}

/// A GraphQL answer.
fn gql(data: serde_json::Value) -> Result<Response, crate::net::https::TransportError> {
    Ok(Response::json(200, &json!({ "data": data })))
}

/// A tracker of `provider` with creating on, `config` probed and `repos`
/// narrowed (GitHub).
fn tracker(s: &Store, provider: &str, site: &str, config: TrackerConfig, repos: &[&str]) -> i64 {
    let t = s.add_tracker(provider, provider, site).unwrap();
    s.set_tracker_probe(t.id, None, &config).unwrap();
    let mut st = settings(true);
    st.repos = repos.iter().map(|r| r.to_string()).collect();
    s.set_tracker_settings(t.id, &st).unwrap();
    t.id
}

fn local(s: &Store, title: &str, parent: Option<i64>) -> i64 {
    s.create_native_item(&NativeItem {
        title,
        parent_id: parent,
        project_id: None,
        notes: Some("Why it matters."),
    })
    .unwrap()
    .id
}

fn linear_provider(fake: &FakeTransport) -> crate::service::trackers::linear::Linear {
    crate::service::trackers::linear::Linear::new(
        "https://linear.app/acme",
        TrackerConfig {
            key_prefixes: vec!["ENG".into()],
            ..Default::default()
        },
        Some(TrackerCredential {
            auth_kind: "bearer".into(),
            username: None,
            secret: crate::store::Secret::new(format!("{}_{}", "lin_api", "n0tRea1".repeat(5))),
        }),
        Arc::new(fake.clone()),
    )
}

#[tokio::test]
async fn a_linear_task_goes_into_its_team_and_a_subtask_under_its_parent() {
    let s = Store::open_in_memory().unwrap();
    let t = tracker(
        &s,
        "linear",
        "https://linear.app/acme",
        TrackerConfig {
            key_prefixes: vec!["ENG".into()],
            ..Default::default()
        },
        &[],
    );
    let parent = s
        .upsert_tracker_item(
            t,
            &TrackerItemWrite {
                external_id: "issue-uuid-1".into(),
                key: Some("ENG-1".into()),
                title: "Launch".into(),
                status_name: "Todo".into(),
                status_category: "todo".into(),
                ..Default::default()
            },
        )
        .unwrap()
        .id;
    let top = local(&s, "Audit logins", None);
    let sub = local(&s, "Write the migration", Some(parent));
    let store = Mutex::new(s);
    // One team: picked without asking; a bad team key is refused.
    let mut a = args(top);
    a.tracker_id = Some(t);
    a.container = Some("not a key".into());
    assert_eq!(
        code(promote(&a, &store, &OrgScope::All, None)),
        codes::E_INVALID
    );
    a.container = None;
    promote(&a, &store, &OrgScope::All, None).unwrap();
    promote(&args(sub), &store, &OrgScope::All, None).unwrap();

    let fake = FakeTransport::new();
    for answer in [
        gql(json!({ "teams": { "nodes": [{ "id": "team-uuid" }] } })),
        gql(json!({ "issueCreate": { "success": true, "issue": {
            "id": "issue-uuid-2", "identifier": "ENG-2", "url": "https://linear.app/acme/issue/ENG-2" } } })),
        // The read-back of ENG-2 (an answer without it: read next pass).
        gql(json!({})),
        gql(json!({ "issue": { "id": "issue-uuid-1", "team": { "id": "team-uuid" } } })),
        gql(json!({ "issueCreate": { "success": true, "issue": {
            "id": "issue-uuid-3", "identifier": "ENG-3", "url": "https://linear.app/acme/issue/ENG-3" } } })),
        gql(json!({})),
    ] {
        fake.once(Method::Post, "api.linear.app/graphql", answer);
    }
    let row = store.lock().unwrap().require_tracker(t).unwrap();
    let r = drain(
        &row,
        &linear_provider(&fake),
        &store,
        crate::store::now_unix() + 1,
    )
    .await;
    assert_eq!(r.sent, 2, "{r:?}");
    let creates: Vec<serde_json::Value> = fake
        .requests()
        .iter()
        .filter_map(|r| r.json_body())
        .filter(|b| {
            b["query"]
                .as_str()
                .is_some_and(|q| q.contains("issueCreate"))
        })
        .collect();
    assert_eq!(creates.len(), 2);
    let top_input = &creates[0]["variables"]["input"];
    assert_eq!(top_input["teamId"], "team-uuid");
    assert_eq!(top_input["title"], "Audit logins");
    assert!(top_input.get("parentId").is_none());
    assert!(top_input["description"]
        .as_str()
        .unwrap()
        .starts_with("Why it matters.\n\n— via fleet"));
    assert_eq!(creates[1]["variables"]["input"]["parentId"], "issue-uuid-1");
    let s = store.lock().unwrap();
    let a = s.get_work_item(top).unwrap().unwrap();
    assert_eq!(
        (a.source.as_str(), a.key.as_deref()),
        ("linear", Some("ENG-2"))
    );
    let b = s.get_work_item(sub).unwrap().unwrap();
    assert_eq!(b.key.as_deref(), Some("ENG-3"));
    assert_eq!(b.external_id.as_deref(), Some("issue-uuid-3"));
    assert_eq!(b.parent_id, Some(parent));
}

fn github_provider(
    fake: &FakeTransport,
    repos: &[&str],
) -> crate::service::trackers::github::GitHub {
    crate::service::trackers::github::GitHub::new(
        "https://github.com/acme",
        TrackerConfig::default(),
        TrackerSettings {
            repos: repos.iter().map(|r| r.to_string()).collect(),
            ..Default::default()
        },
        Arc::new(fake.clone()),
    )
}

#[tokio::test]
async fn a_github_task_goes_into_a_repository_and_a_subtask_becomes_a_sub_issue() {
    let s = Store::open_in_memory().unwrap();
    let t = tracker(
        &s,
        "github",
        "https://github.com/acme",
        TrackerConfig::default(),
        &["acme/api"],
    );
    let parent = s
        .upsert_tracker_item(
            t,
            &TrackerItemWrite {
                external_id: "I_parent".into(),
                key: Some("acme/api#7".into()),
                title: "Launch".into(),
                status_name: "Open".into(),
                status_category: "todo".into(),
                ..Default::default()
            },
        )
        .unwrap()
        .id;
    let top = local(&s, "Audit logins", None);
    let sub = local(&s, "Write the migration", Some(parent));
    let store = Mutex::new(s);
    let mut a = args(top);
    a.tracker_id = Some(t);
    a.container = Some("acme/other".into());
    assert_eq!(
        code(promote(&a, &store, &OrgScope::All, None)),
        codes::E_INVALID,
        "not a repository the tracker is narrowed to"
    );
    a.container = Some("Acme/API".into());
    promote(&a, &store, &OrgScope::All, None).unwrap();
    promote(&args(sub), &store, &OrgScope::All, None).unwrap();

    let fake = FakeTransport::new();
    for answer in [
        gql(json!({ "repository": { "id": "R_api" } })),
        gql(
            json!({ "createIssue": { "issue": { "id": "I_new40", "number": 40,
            "url": "https://github.com/acme/api/issues/40",
            "repository": { "nameWithOwner": "acme/api" } } } }),
        ),
        // The read-back of #40 (not found: read next pass).
        gql(json!({ "nodes": [null] })),
        gql(json!({ "node": { "id": "I_parent", "repository": { "id": "R_api" } } })),
        gql(
            json!({ "createIssue": { "issue": { "id": "I_new41", "number": 41,
            "url": "https://github.com/acme/api/issues/41",
            "repository": { "nameWithOwner": "acme/api" } } } }),
        ),
        gql(json!({ "addSubIssue": { "issue": { "id": "I_parent" } } })),
        gql(json!({ "nodes": [null] })),
    ] {
        fake.once(Method::Post, "api.github.com/graphql", answer);
    }
    let row = store.lock().unwrap().require_tracker(t).unwrap();
    let r = drain(
        &row,
        &github_provider(&fake, &["acme/api"]),
        &store,
        crate::store::now_unix() + 1,
    )
    .await;
    assert_eq!(r.sent, 2, "{r:?}");
    let bodies: Vec<serde_json::Value> = fake
        .requests()
        .iter()
        .filter_map(|r| r.json_body())
        .collect();
    let create = bodies
        .iter()
        .find(|b| {
            b["query"]
                .as_str()
                .is_some_and(|q| q.contains("createIssue"))
        })
        .unwrap();
    assert_eq!(create["variables"]["repo"], "R_api");
    assert_eq!(create["variables"]["title"], "Audit logins");
    let link = bodies
        .iter()
        .find(|b| {
            b["query"]
                .as_str()
                .is_some_and(|q| q.contains("addSubIssue"))
        })
        .expect("the sub-issue link");
    assert_eq!(
        (
            link["variables"]["p"].as_str(),
            link["variables"]["c"].as_str()
        ),
        (Some("I_parent"), Some("I_new41"))
    );
    let s = store.lock().unwrap();
    let a = s.get_work_item(top).unwrap().unwrap();
    assert_eq!(
        (a.source.as_str(), a.key.as_deref()),
        ("github", Some("acme/api#40"))
    );
    assert_eq!(
        a.url.as_deref(),
        Some("https://github.com/acme/api/issues/40")
    );
    let b = s.get_work_item(sub).unwrap().unwrap();
    assert_eq!(b.key.as_deref(), Some("acme/api#41"));
    assert_eq!(b.parent_id, Some(parent));
}

#[tokio::test]
async fn a_github_repository_outside_the_trackers_scope_is_never_written_to() {
    // Narrowed to no repositories: the person types one, and the adapter
    // still holds it to the site's owner.
    let fake = FakeTransport::new();
    let g = github_provider(&fake, &[]);
    let e = crate::service::trackers::TrackerProvider::write(
        &g,
        &crate::service::trackers::WriteOp::CreateItem {
            container: Some("someone-else/repo".into()),
            parent: None,
            title: "x".into(),
            description: String::new(),
        },
    )
    .await
    .unwrap_err();
    assert!(
        matches!(e, crate::service::trackers::TrackerError::Invalid(_)),
        "{e:?}"
    );
    assert!(fake.requests().is_empty());
}
