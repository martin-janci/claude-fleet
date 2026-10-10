//! Typed done_when and Verified / Unverified (orchestration O3).

use super::*;
use crate::mcp::auth::{Caller, ClientRef, TokenMode};
use crate::service::work::missions::{self, MissionInput};
use crate::store::TaskReport;

fn person(store: &Mutex<Store>, id: i64) -> ViewScope {
    Caller {
        api: None,
        host_alias: None,
        client: Some(ClientRef {
            id: 7,
            name: "phone".into(),
            trusted: false,
            org_id: None,
            person_id: Some(id),
        }),
        mode: TokenMode::Full,
        pane: None,
        is_personal_owner: false,
    }
    .view_scope(&lock(store).unwrap())
    .unwrap()
}

fn fixture() -> (Mutex<Store>, ViewScope, i64) {
    let st = Mutex::new(Store::open_in_memory().unwrap());
    let item = lock(&st)
        .unwrap()
        .create_local_work_item(None, "queue")
        .unwrap()
        .id;
    (st, ViewScope::internal(), item)
}

fn set(st: &Mutex<Store>, me: &ViewScope, item: i64, lines: &[&str]) -> VerifyOutcome {
    set_done_when(
        &WorkLinkArgs {
            action: "done_when".into(),
            item_id: Some(item),
            done_when: Some(lines.iter().map(|l| l.to_string()).collect()),
            ..Default::default()
        },
        st,
        me,
    )
    .unwrap()
}

fn check_of<'a>(v: &'a Verification, line: &str) -> &'a CondCheck {
    v.checks.iter().find(|c| c.line == line).unwrap()
}

/// A finished run of `item` in `role` that reported `outcome`.
fn run(st: &Mutex<Store>, item: i64, role: &str, outcome: &str, tests: &[&str]) {
    let s = lock(st).unwrap();
    let t = s
        .insert_task(None, None, "go", &format!("n-{role}-{outcome}"))
        .unwrap();
    s.set_task_run(t.id, item, 1, role).unwrap();
    let t = s.get_task(t.id).unwrap().unwrap();
    let report = TaskReport {
        summary: format!("{role} {outcome}"),
        outcome: outcome.into(),
        tests_run: tests.iter().map(|x| x.to_string()).collect(),
        ..Default::default()
    };
    crate::service::tasks::complete_task_reported(&s, &t, "", Some(&report)).unwrap();
}

#[test]
fn lines_are_typed_by_prefix_and_free_text_is_a_persons() {
    assert_eq!(parse_cond("ci"), Cond::Ci(None));
    assert_eq!(parse_cond("CI: test "), Cond::Ci(Some("test".into())));
    assert_eq!(parse_cond("review"), Cond::Review);
    assert_eq!(
        parse_cond("test:cargo test"),
        Cond::Test(Some("cargo test".into()))
    );
    assert_eq!(parse_cond("person"), Cond::Person);
    assert_eq!(parse_cond("the demo works on a phone"), Cond::Person);
}

#[test]
fn an_item_is_verified_only_when_every_line_passed() {
    let (st, me, item) = fixture();
    let out = set(&st, &me, item, &["review", "test:cargo test", "person"]);
    let v = out.verification.unwrap();
    assert_eq!(v.state, "unverified");
    assert!(v.checks.iter().all(|c| c.state == "pending"));
    // The worker's own claim is no review.
    run(&st, item, "implement", "done", &["cargo test"]);
    let v = now_check(&st, item, 0);
    assert_eq!(check_of(&v, "review").state, "pending");
    run(&st, item, "review", "done", &[]);
    run(&st, item, "test", "done", &["cargo test --workspace"]);
    let v = now_check(&st, item, 0);
    assert_eq!(check_of(&v, "review").state, "pass");
    assert_eq!(check_of(&v, "test:cargo test").state, "pass");
    assert_eq!(v.state, "unverified", "the person has not checked theirs");
    let out = verify(
        &WorkLinkArgs {
            action: "verify".into(),
            item_id: Some(item),
            line: Some("person".into()),
            ok: Some(true),
            ..Default::default()
        },
        &st,
        &me,
    )
    .unwrap();
    assert_eq!(out.verification.unwrap().state, "verified");
}

/// The item's verification at `now`, under one lock (the store's mutex is
/// not reentrant).
fn now_check(st: &Mutex<Store>, id: i64, now: i64) -> Verification {
    let s = lock(st).unwrap();
    let item = s.get_work_item(id).unwrap().unwrap();
    verification(&s, &item, now).unwrap().unwrap()
}

#[test]
fn a_reviewer_who_asks_for_changes_fails_the_item() {
    let (st, me, item) = fixture();
    set(&st, &me, item, &["review", "test:npm test"]);
    run(&st, item, "review", "partial", &[]);
    run(&st, item, "test", "done", &["cargo test"]);
    let v = now_check(&st, item, 0);
    assert_eq!(check_of(&v, "review").state, "fail");
    assert_eq!(
        check_of(&v, "test:npm test").state,
        "pending",
        "it ran something else"
    );
    assert_eq!(v.state, "failed");
}

#[test]
fn ci_reads_a_fresh_reading_on_the_checkouts_own_commit() {
    let (st, me, item) = fixture();
    set(&st, &me, item, &["ci", "ci:lint"]);
    let w = {
        let s = lock(&st).unwrap();
        s.upsert_host("h").unwrap();
        let w = s
            .upsert_session("w", "h", None, None, 1, 1, "running", None)
            .unwrap();
        let t = s.insert_task(None, Some(w), "go", "nci").unwrap();
        s.set_task_run(t.id, item, 1, "implement").unwrap();
        w
    };
    let reading = |json: &str, at: i64| {
        lock(&st)
            .unwrap()
            .conn_ref()
            .execute(
                "UPDATE sessions SET pr_evidence = ?1, pr_checked_at = ?2 WHERE id = ?3",
                rusqlite::params![json, at, w],
            )
            .unwrap();
    };
    let now = 10_000;
    let states = || {
        let v = now_check(&st, item, now);
        (
            check_of(&v, "ci").state.clone(),
            check_of(&v, "ci:lint").state.clone(),
        )
    };
    assert_eq!(states(), ("pending".into(), "pending".into()));
    reading(
        r#"{"head_oid":"a","local_head":"a","checks":{"total":3}}"#,
        now,
    );
    assert_eq!(states(), ("pass".into(), "pass".into()));
    reading(
        r#"{"head_oid":"a","local_head":"a","checks":{"total":3,"failing":[{"name":"lint"}],"failing_total":1}}"#,
        now,
    );
    assert_eq!(states(), ("fail".into(), "fail".into()));
    reading(
        r#"{"head_oid":"a","local_head":"a","checks":{"total":3,"failing":[{"name":"e2e"}],"failing_total":1}}"#,
        now,
    );
    assert_eq!(states(), ("fail".into(), "pass".into()));
    reading(
        r#"{"head_oid":"a","local_head":"b","checks":{"total":3}}"#,
        now,
    );
    assert_eq!(
        states(),
        ("pending".into(), "pending".into()),
        "another commit"
    );
    reading(
        r#"{"head_oid":"a","local_head":"a","checks":{"total":3}}"#,
        1,
    );
    assert_eq!(
        states(),
        ("pending".into(), "pending".into()),
        "an old reading"
    );
}

#[test]
fn only_a_person_records_a_check_and_only_of_a_real_line() {
    let (st, me, item) = fixture();
    set(&st, &me, item, &["person"]);
    let args = |line: &str| WorkLinkArgs {
        action: "verify".into(),
        item_id: Some(item),
        line: Some(line.into()),
        ok: Some(true),
        ..Default::default()
    };
    assert_eq!(
        verify(&args("ci"), &st, &me).unwrap_err().code,
        codes::E_INVALID
    );
    let host = Caller {
        api: None,
        host_alias: Some("h".into()),
        client: None,
        mode: TokenMode::Full,
        pane: None,
        is_personal_owner: false,
    }
    .view_scope(&lock(&st).unwrap())
    .unwrap();
    assert_eq!(
        verify(&args("person"), &st, &host).unwrap_err().code,
        codes::E_FORBIDDEN
    );
}

#[test]
fn a_missions_lines_decide_its_root_and_show_on_the_graph() {
    let st = Mutex::new(Store::open_in_memory().unwrap());
    let ana = lock(&st).unwrap().create_person("ana", None).unwrap().id;
    let me = person(&st, ana);
    let m = missions::save(
        &WorkLinkArgs {
            action: "mission_save".into(),
            mission: Some(MissionInput {
                name: Some("m".into()),
                goal: Some("g".into()),
                done_when: Some(vec!["person".into()]),
                ..Default::default()
            }),
            ..Default::default()
        },
        &st,
        &me,
    )
    .unwrap();
    let root = m.root_item_id.unwrap();
    let d = missions::mission(&st, &me, m.id, None).unwrap();
    let node = d.graph.nodes.iter().find(|n| n.item_id == root).unwrap();
    assert_eq!(node.verification.as_ref().unwrap().state, "unverified");
    // Another person cannot set lines on an item they cannot see.
    let bo = lock(&st).unwrap().create_person("bo", None).unwrap().id;
    let bo = person(&st, bo);
    let e = set_done_when(
        &WorkLinkArgs {
            action: "done_when".into(),
            item_id: Some(root),
            done_when: Some(vec!["review".into()]),
            ..Default::default()
        },
        &st,
        &bo,
    )
    .unwrap_err();
    assert_eq!(e.code, codes::E_NOTFOUND);
}

#[test]
fn a_review_or_test_before_the_latest_implementation_is_pending() {
    let (st, me, item) = fixture();
    set(&st, &me, item, &["review", "test"]);
    run(&st, item, "implement", "done", &[]);
    run(&st, item, "review", "changes_requested", &[]);
    run(&st, item, "test", "done", &[]);
    let v = now_check(&st, item, 0);
    assert_eq!(check_of(&v, "review").state, "fail");
    assert_eq!(check_of(&v, "test").state, "pass");
    // The retry rewrote the work: neither old verdict holds for it.
    run(&st, item, "implement", "done", &[]);
    let v = now_check(&st, item, 0);
    assert_eq!(check_of(&v, "review").state, "pending");
    assert_eq!(check_of(&v, "test").state, "pending");
    assert_eq!(v.state, "unverified");
    run(&st, item, "review", "done", &[]);
    let v = now_check(&st, item, 0);
    assert_eq!(check_of(&v, "review").state, "pass");
}
