//! A status move, per adapter, over [`FakeTransport`] (CF-6): which status
//! each picks, the exact requests each sends, and what each refuses without
//! sending. No test here reaches a tracker.

use super::asana::Asana;
use super::github::GitHub;
use super::jira::JiraCloud;
use super::jira_common::pick_transition;
use super::jira_dc::JiraDc;
use super::linear::{pick_state, Linear};
use super::*;
use crate::net::https::{FakeTransport, Method, Response};
use crate::store::{TrackerConfig, TrackerCredential, TrackerSettings};
use serde_json::{json, Value};

/// A credential assembled at run time, never a literal token.
fn cred(kind: &str) -> TrackerCredential {
    TrackerCredential {
        auth_kind: kind.into(),
        username: (kind == "basic").then(|| "dev@example.com".into()),
        secret: crate::store::Secret::new(format!("tok{}", "0Ab9".repeat(6))),
    }
}

fn keys(prefixes: &[&str]) -> TrackerConfig {
    TrackerConfig {
        key_prefixes: prefixes.iter().map(|p| p.to_string()).collect(),
        ..Default::default()
    }
}

fn mv(key: &str, external_id: Option<&str>, category: &str, name: Option<&str>) -> WriteOp {
    WriteOp::SetStatus {
        key: key.into(),
        external_id: external_id.map(str::to_string),
        category: category.into(),
        name: name.map(str::to_string),
    }
}

/// The requests sent, as `METHOD path-and-query`.
fn sent(f: &FakeTransport) -> Vec<String> {
    f.requests()
        .iter()
        .map(|r| {
            let path = r.url.splitn(4, '/').nth(3).unwrap_or_default();
            format!("{} /{path}", r.method.as_str())
        })
        .collect()
}

// ── the shared pieces ───────────────────────────────────────────────────

#[test]
fn status_names_are_one_whatever_their_case_and_spacing() {
    assert!(same_status_name("In  Review ", "in review"));
    assert!(!same_status_name("In Review", "Review"));
    assert_eq!(category_label("in_progress"), "In progress");
    assert_eq!(category_label("done"), "Done");
    assert_eq!(category_label("todo"), "To do");
}

#[test]
fn a_move_nobody_can_make_is_unsupported_and_leaves_the_tracker_ok() {
    let e = TrackerError::Unsupported("ABC-1's workflow has no move to Done".into());
    assert_eq!(e.state(), None, "the tracker itself is fine");
    assert_eq!(e.explain(), "ABC-1's workflow has no move to Done");
    assert_eq!(e.to_ipc().code, crate::ipc_error::codes::E_UNSUPPORTED);
}

// ── Jira ────────────────────────────────────────────────────────────────

fn transitions() -> Value {
    json!({ "transitions": [
        { "id": "11", "name": "Start", "to": { "name": "In Progress",
            "statusCategory": { "key": "indeterminate" } } },
        { "id": "21", "name": "Review", "to": { "name": "In Review",
            "statusCategory": { "key": "indeterminate" } } },
        { "id": "31", "name": "Resolve", "isAvailable": false, "to": { "name": "Done",
            "statusCategory": { "key": "done" } } },
        { "id": "../9", "name": "Bad", "to": { "name": "Closed",
            "statusCategory": { "key": "done" } } },
    ]})
}

#[test]
fn jira_takes_the_named_transition_else_the_first_of_the_category() {
    let t = transitions();
    assert_eq!(
        pick_transition(&t, "ABC-1", "in_progress", Some("in review")).unwrap(),
        "21"
    );
    assert_eq!(
        pick_transition(&t, "ABC-1", "in_progress", None).unwrap(),
        "11"
    );
    // A column this workflow has no status for: the category decides.
    assert_eq!(
        pick_transition(&t, "ABC-1", "in_progress", Some("Doing")).unwrap(),
        "11"
    );
    // The only ways to done are unavailable, or carry an id that is no id.
    let e = pick_transition(&t, "ABC-1", "done", None).unwrap_err();
    let TrackerError::Unsupported(m) = e else {
        panic!("{e:?}")
    };
    assert!(m.contains("ABC-1's workflow has no move to Done"), "{m}");
    assert!(m.contains("In Progress, In Review"), "{m}");
    let e = pick_transition(&json!({ "transitions": [] }), "ABC-1", "done", None).unwrap_err();
    assert!(matches!(e, TrackerError::Unsupported(m) if m.contains("allows no move")));
}

fn cloud(f: &FakeTransport) -> JiraCloud {
    JiraCloud::new(
        "https://acme.atlassian.net",
        keys(&["ABC"]),
        Some(cred("basic")),
        Arc::new(f.clone()),
    )
}

fn dc(f: &FakeTransport) -> JiraDc {
    JiraDc::new(
        "https://jira.corp.example/jira",
        keys(&["OPS"]),
        Some(cred("bearer")),
        Arc::new(f.clone()),
    )
}

#[tokio::test]
async fn jira_cloud_asks_the_workflow_then_takes_one_transition() {
    let f = FakeTransport::new();
    f.once(
        Method::Get,
        "/rest/api/3/issue/ABC-1/transitions",
        Ok(Response::json(200, &transitions())),
    );
    f.once(
        Method::Post,
        "/rest/api/3/issue/ABC-1/transitions",
        Ok(Response::new(204, "")),
    );
    cloud(&f)
        .write(&mv(
            "abc-1",
            Some("10001"),
            "in_progress",
            Some("In Review"),
        ))
        .await
        .unwrap();
    assert_eq!(
        sent(&f),
        [
            "GET /rest/api/3/issue/ABC-1/transitions",
            "POST /rest/api/3/issue/ABC-1/transitions"
        ]
    );
    assert_eq!(
        f.requests()[1].json_body().unwrap(),
        json!({ "transition": { "id": "21" } })
    );
}

#[tokio::test]
async fn jira_with_no_such_move_sends_nothing_and_a_screen_is_said_in_words() {
    let f = FakeTransport::new();
    f.always(
        Method::Get,
        "/transitions",
        Ok(Response::json(200, &transitions())),
    );
    let e = cloud(&f)
        .write(&mv("ABC-1", None, "done", None))
        .await
        .unwrap_err();
    assert!(matches!(e, TrackerError::Unsupported(_)), "{e:?}");
    assert_eq!(sent(&f).len(), 1, "only the question was asked");

    // A transition whose screen wants a field: Jira's 400.
    f.always(Method::Post, "/transitions", Ok(Response::new(400, "{}")));
    let e = cloud(&f)
        .write(&mv("ABC-1", None, "in_progress", None))
        .await
        .unwrap_err();
    assert!(
        matches!(&e, TrackerError::Unsupported(m) if m.contains("needs a field")),
        "{e:?}"
    );

    // A key that is no key never reaches a path.
    let f = FakeTransport::new();
    assert!(cloud(&f)
        .write(&mv("../../myself", None, "done", None))
        .await
        .is_err());
    assert!(f.requests().is_empty());
}

#[tokio::test]
async fn jira_dc_moves_through_api_v2() {
    let f = FakeTransport::new();
    f.once(
        Method::Get,
        "/jira/rest/api/2/issue/OPS-7/transitions",
        Ok(Response::json(200, &transitions())),
    );
    f.once(
        Method::Post,
        "/jira/rest/api/2/issue/OPS-7/transitions",
        Ok(Response::new(204, "")),
    );
    dc(&f)
        .write(&mv("OPS-7", None, "in_progress", None))
        .await
        .unwrap();
    assert_eq!(
        f.requests()[1].json_body().unwrap(),
        json!({ "transition": { "id": "11" } })
    );
}

// ── Linear ──────────────────────────────────────────────────────────────

fn states() -> Value {
    json!([
        { "id": "s-done", "name": "Done", "type": "completed", "position": 5.0 },
        { "id": "s-cancel", "name": "Canceled", "type": "canceled", "position": 6.0 },
        { "id": "s-review", "name": "In Review", "type": "started", "position": 4.0 },
        { "id": "s-prog", "name": "In Progress", "type": "started", "position": 3.0 },
        { "id": "s-todo", "name": "Todo", "type": "unstarted", "position": 2.0 },
        { "id": "s-back", "name": "Backlog", "type": "backlog", "position": 1.0 },
    ])
}

#[test]
fn linear_takes_the_named_state_else_the_teams_first_of_that_type() {
    let s = states();
    assert_eq!(
        pick_state(&s, "in_progress", Some("in review")).as_deref(),
        Some("s-review")
    );
    assert_eq!(
        pick_state(&s, "in_progress", None).as_deref(),
        Some("s-prog")
    );
    // To do is unstarted before backlog, whatever the positions say.
    assert_eq!(pick_state(&s, "todo", None).as_deref(), Some("s-todo"));
    // Done is completed, never canceled.
    assert_eq!(pick_state(&s, "done", None).as_deref(), Some("s-done"));
    let only_canceled = json!([{ "id": "c", "name": "Canceled", "type": "canceled" }]);
    assert_eq!(pick_state(&only_canceled, "done", None), None);
    assert_eq!(pick_state(&json!(null), "done", None), None);
}

fn linear(f: &FakeTransport) -> Linear {
    Linear::new(
        "https://linear.app/acme",
        keys(&["ENG"]),
        Some(cred("bearer")),
        Arc::new(f.clone()),
    )
}

#[tokio::test]
async fn linear_reads_the_teams_states_then_sets_one() {
    let f = FakeTransport::new();
    f.once(
        Method::Post,
        "/graphql",
        Ok(Response::json(
            200,
            &json!({ "data": { "issue": { "team": { "states": { "nodes": states() } } } } }),
        )),
    );
    f.once(
        Method::Post,
        "/graphql",
        Ok(Response::json(
            200,
            &json!({ "data": { "issueUpdate": { "success": true } } }),
        )),
    );
    linear(&f)
        .write(&mv("ENG-7", Some("uuid-7"), "done", None))
        .await
        .unwrap();
    let reqs = f.requests();
    assert_eq!(reqs.len(), 2);
    let update = reqs[1].json_body().unwrap();
    assert!(update["query"].as_str().unwrap().contains("issueUpdate"));
    // Everything a caller chose travels as a variable, never in the query.
    assert_eq!(
        update["variables"],
        json!({ "id": "uuid-7", "state": "s-done" })
    );
}

#[tokio::test]
async fn linear_with_no_state_for_it_sends_no_update() {
    let f = FakeTransport::new();
    f.once(
        Method::Post,
        "/graphql",
        Ok(Response::json(
            200,
            &json!({ "data": { "issue": { "team": { "states": { "nodes": [
                { "id": "s-todo", "name": "Todo", "type": "unstarted", "position": 1.0 }
            ] } } } } }),
        )),
    );
    let e = linear(&f)
        .write(&mv("ENG-7", Some("uuid-7"), "done", None))
        .await
        .unwrap_err();
    assert!(
        matches!(&e, TrackerError::Unsupported(m) if m.contains("ENG-7") && m.contains("Done")),
        "{e:?}"
    );
    assert_eq!(f.requests().len(), 1);
}

// ── GitHub ──────────────────────────────────────────────────────────────

fn github(f: &FakeTransport) -> GitHub {
    GitHub::new(
        "https://github.com/acme",
        TrackerConfig::default(),
        TrackerSettings::default(),
        Arc::new(f.clone()),
    )
}

fn issue_state(f: &FakeTransport, state: &str) {
    f.once(
        Method::Post,
        "/graphql",
        Ok(Response::json(
            200,
            &json!({ "data": { "node": { "state": state } } }),
        )),
    );
}

#[tokio::test]
async fn github_closes_for_done_and_reopens_for_anything_else() {
    let f = FakeTransport::new();
    issue_state(&f, "OPEN");
    f.once(
        Method::Post,
        "/graphql",
        Ok(Response::json(
            200,
            &json!({ "data": { "closeIssue": { "issue": { "id": "I_abc" } } } }),
        )),
    );
    github(&f)
        .write(&mv(
            "acme/api#7",
            Some("I_abc"),
            "done",
            Some("Closed as not planned"),
        ))
        .await
        .unwrap();
    let close = f.requests()[1].json_body().unwrap();
    assert!(close["query"].as_str().unwrap().contains("closeIssue"));
    assert_eq!(
        close["variables"],
        json!({ "id": "I_abc", "reason": "NOT_PLANNED" })
    );

    let f = FakeTransport::new();
    issue_state(&f, "CLOSED");
    f.once(
        Method::Post,
        "/graphql",
        Ok(Response::json(
            200,
            &json!({ "data": { "reopenIssue": { "issue": { "id": "I_abc" } } } }),
        )),
    );
    github(&f)
        .write(&mv("acme/api#7", Some("I_abc"), "todo", None))
        .await
        .unwrap();
    let reopen = f.requests()[1].json_body().unwrap();
    assert!(reopen["query"].as_str().unwrap().contains("reopenIssue"));
    assert_eq!(reopen["variables"], json!({ "id": "I_abc" }));
}

#[tokio::test]
async fn github_has_no_move_between_the_two_open_statuses() {
    let f = FakeTransport::new();
    issue_state(&f, "OPEN");
    let e = github(&f)
        .write(&mv("acme/api#7", Some("I_abc"), "in_progress", None))
        .await
        .unwrap_err();
    assert!(
        matches!(&e, TrackerError::Unsupported(m) if m.contains("pull request")),
        "{e:?}"
    );
    assert_eq!(f.requests().len(), 1, "only the state was read");
}

// ── Asana ───────────────────────────────────────────────────────────────

const TASK: &str = "1200000000000777";
const PROJECT: &str = "1200000000001001";

fn asana_with(f: &FakeTransport, settings: TrackerSettings) -> Asana {
    Asana::new(
        "https://app.asana.com",
        TrackerConfig::default(),
        settings,
        Some(cred("bearer")),
        Arc::new(f.clone()),
    )
}

/// The task's own answer and its board's sections: Backlog, Doing (mapped in
/// progress by a person), Review (unmapped: to do), Shipped (mapped done).
fn board(f: &FakeTransport, completed: bool, section: Option<&str>) -> Asana {
    let memberships = match section {
        Some(s) => json!([{ "project": { "gid": PROJECT },
            "section": { "gid": "1200000000002000", "name": s } }]),
        None => json!([]),
    };
    f.always(
        Method::Get,
        &format!("/tasks/{TASK}?"),
        Ok(Response::json(
            200,
            &json!({ "data": { "gid": TASK, "completed": completed, "memberships": memberships } }),
        )),
    );
    f.always(
        Method::Get,
        &format!("/projects/{PROJECT}/sections"),
        Ok(Response::json(
            200,
            &json!({ "data": [
                { "gid": "1200000000002001", "name": "Backlog" },
                { "gid": "1200000000002002", "name": "Doing" },
                { "gid": "1200000000002003", "name": "Review" },
                { "gid": "1200000000002004", "name": "Shipped" },
            ]}),
        )),
    );
    f.always(
        Method::Put,
        &format!("/tasks/{TASK}"),
        Ok(Response::json(200, &json!({ "data": { "gid": TASK } }))),
    );
    f.always(
        Method::Post,
        "/addTask",
        Ok(Response::json(200, &json!({ "data": {} }))),
    );
    let mut settings = TrackerSettings::default();
    settings
        .section_map
        .insert("doing".into(), "in_progress".into());
    settings.section_map.insert("shipped".into(), "done".into());
    settings.section_map_confirmed = true;
    asana_with(f, settings)
}

/// The writes among the requests sent.
fn writes(f: &FakeTransport) -> Vec<(String, Value)> {
    f.requests()
        .iter()
        .filter(|r| r.method != Method::Get)
        .map(|r| {
            let path = r.url.split("/api/1.0").nth(1).unwrap_or_default();
            (
                format!("{} {path}", r.method.as_str()),
                r.json_body().unwrap_or_default(),
            )
        })
        .collect()
}

#[tokio::test]
async fn asana_moves_to_the_section_the_card_was_dropped_on() {
    let f = FakeTransport::new();
    board(&f, false, Some("Backlog"))
        .write(&mv(
            &format!("asana:{TASK}"),
            Some(TASK),
            "todo",
            Some("review"),
        ))
        .await
        .unwrap();
    assert_eq!(
        writes(&f),
        [(
            "POST /sections/1200000000002003/addTask".to_string(),
            json!({ "data": { "task": TASK } })
        )]
    );
}

#[tokio::test]
async fn asana_on_a_plain_column_takes_the_boards_first_section_of_that_status() {
    // The owner's choice (CF-6): the first mapped section, in board order.
    let f = FakeTransport::new();
    board(&f, false, Some("Backlog"))
        .write(&mv(&format!("asana:{TASK}"), None, "in_progress", None))
        .await
        .unwrap();
    assert_eq!(
        writes(&f),
        [(
            "POST /sections/1200000000002002/addTask".to_string(),
            json!({ "data": { "task": TASK } })
        )]
    );

    // Already in a section of that status: nothing to move.
    let f = FakeTransport::new();
    board(&f, false, Some("Review"))
        .write(&mv(&format!("asana:{TASK}"), None, "todo", None))
        .await
        .unwrap();
    assert!(writes(&f).is_empty());
}

#[tokio::test]
async fn asana_done_completes_and_a_move_out_of_done_reopens() {
    // Plain Done: completed, where it is.
    let f = FakeTransport::new();
    board(&f, false, Some("Doing"))
        .write(&mv(
            &format!("asana:{TASK}"),
            Some(TASK),
            "done",
            Some("Completed"),
        ))
        .await
        .unwrap();
    assert_eq!(
        writes(&f),
        [(
            format!("PUT /tasks/{TASK}"),
            json!({ "data": { "completed": true } })
        )]
    );

    // A section that counts as done is a section: moved there, not completed.
    let f = FakeTransport::new();
    board(&f, false, Some("Doing"))
        .write(&mv(&format!("asana:{TASK}"), None, "done", Some("Shipped")))
        .await
        .unwrap();
    assert_eq!(
        writes(&f)
            .iter()
            .map(|(w, _)| w.as_str())
            .collect::<Vec<_>>(),
        ["POST /sections/1200000000002004/addTask"]
    );

    // Out of Done: reopened, then put in the first section of that status.
    let f = FakeTransport::new();
    board(&f, true, Some("Shipped"))
        .write(&mv(&format!("asana:{TASK}"), None, "in_progress", None))
        .await
        .unwrap();
    assert_eq!(
        writes(&f),
        [
            (
                format!("PUT /tasks/{TASK}"),
                json!({ "data": { "completed": false } })
            ),
            (
                "POST /sections/1200000000002002/addTask".to_string(),
                json!({ "data": { "task": TASK } })
            )
        ]
    );
}

#[tokio::test]
async fn asana_with_no_section_for_it_writes_nothing() {
    // On no board at all: only completing works.
    let f = FakeTransport::new();
    let e = board(&f, false, None)
        .write(&mv(&format!("asana:{TASK}"), None, "in_progress", None))
        .await
        .unwrap_err();
    assert!(
        matches!(&e, TrackerError::Unsupported(m) if m.contains("on no Asana board")),
        "{e:?}"
    );
    assert!(writes(&f).is_empty());
    board(&f, false, None)
        .write(&mv(&format!("asana:{TASK}"), None, "done", None))
        .await
        .unwrap();
    assert_eq!(writes(&f).len(), 1);

    // A gid that is no gid never reaches a path.
    let f = FakeTransport::new();
    assert!(asana_with(&f, TrackerSettings::default())
        .write(&mv("asana:../x", Some("../x"), "done", None))
        .await
        .is_err());
    assert!(f.requests().is_empty());
}

// ── every provider takes the op ─────────────────────────────────────────

#[test]
fn every_provider_that_writes_is_one_this_file_moves() {
    for p in crate::store::TRACKER_PROVIDERS {
        assert!(
            matches!(*p, "jira" | "jira_dc" | "asana" | "linear" | "github"),
            "{p}: a new provider needs a status move (WriteOp::SetStatus) and a test here"
        );
    }
}
