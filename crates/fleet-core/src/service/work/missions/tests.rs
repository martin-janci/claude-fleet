//! Missions behind their fences (orchestration O1): the owner, the org's
//! members and admins, the org boundary, and the unknown-id answer for
//! everyone else.

use super::*;
use crate::mcp::auth::{Caller, ClientRef, TokenMode};
use std::sync::Mutex;

fn store() -> Mutex<Store> {
    Mutex::new(Store::open_in_memory().unwrap())
}

/// A person's device, bound to `org` or to none, scoped the way a request is.
fn person(store: &Mutex<Store>, org: Option<i64>, id: i64) -> ViewScope {
    Caller {
        host_alias: None,
        client: Some(ClientRef {
            id: 7,
            name: "phone".into(),
            trusted: false,
            org_id: org,
            person_id: Some(id),
        }),
        mode: TokenMode::Full,
        pane: None,
        is_personal_owner: false,
    }
    .view_scope(&lock(store).unwrap())
    .unwrap()
}

fn people(store: &Mutex<Store>) -> (i64, i64) {
    let s = lock(store).unwrap();
    (
        s.create_person("ana", None).unwrap().id,
        s.create_person("bo", None).unwrap().id,
    )
}

fn save_args(input: MissionInput) -> WorkLinkArgs {
    WorkLinkArgs {
        action: "mission_save".into(),
        mission: Some(input),
        ..Default::default()
    }
}

fn new_mission(store: &Mutex<Store>, scope: &ViewScope, org: Option<i64>) -> MissionRow {
    save(
        &save_args(MissionInput {
            name: Some("Payments v2".into()),
            goal: Some("cards and refunds".into()),
            org_id: org,
            ..Default::default()
        }),
        store,
        scope,
    )
    .unwrap()
}

fn by_id(action: &str, id: i64) -> WorkLinkArgs {
    WorkLinkArgs {
        action: action.into(),
        mission_id: Some(id),
        ..Default::default()
    }
}

#[test]
fn a_new_mission_belongs_to_its_creator_and_roots_a_new_task() {
    let st = store();
    let (ana, _) = people(&st);
    let m = new_mission(&st, &person(&st, None, ana), None);
    assert_eq!(m.owner_person_id, Some(ana));
    assert_eq!(m.state, "draft");
    let root = m.root_item_id.expect("a root task");
    let s = lock(&st).unwrap();
    let item = s.get_work_item(root).unwrap().unwrap();
    assert_eq!(item.title, "Payments v2");
    assert_eq!(item.notes.as_deref(), Some("cards and refunds"));
}

#[test]
fn a_refused_create_leaves_no_root_task_behind() {
    let st = store();
    let (ana, _) = people(&st);
    let before = lock(&st).unwrap().local_work_items().unwrap().len();
    let e = save(
        &save_args(MissionInput {
            name: Some("m".into()),
            goal: Some("g".into()),
            level: Some(9),
            ..Default::default()
        }),
        &st,
        &person(&st, None, ana),
    )
    .unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
    assert_eq!(lock(&st).unwrap().local_work_items().unwrap().len(), before);
}

#[test]
fn another_person_cannot_see_or_change_a_mission() {
    let st = store();
    let (ana, bo) = people(&st);
    let m = new_mission(&st, &person(&st, None, ana), None);
    let bo = person(&st, None, bo);
    assert!(missions(&st, &bo).unwrap().is_empty());
    let unknown = mission(&st, &bo, 999_999, None).unwrap_err();
    let hidden = mission(&st, &bo, m.id, None).unwrap_err();
    assert_eq!(hidden.code, unknown.code);
    assert_eq!(
        hidden.message.replace(&m.id.to_string(), "N"),
        unknown.message.replace("999999", "N"),
        "a hidden mission answers as an unknown one"
    );
    let mut st_args = by_id("mission_state", m.id);
    st_args.status = Some("active".into());
    assert_eq!(set_state(&st_args, &st, &bo).unwrap_err().code, hidden.code);
    assert_eq!(
        delete(&by_id("mission_delete", m.id), &st, &bo)
            .unwrap_err()
            .code,
        hidden.code
    );
}

#[test]
fn an_org_member_reads_and_only_an_admin_changes() {
    let st = store();
    let (ana, bo) = people(&st);
    let org = {
        let s = lock(&st).unwrap();
        let org = s.add_org("Acme", None, false).unwrap().id;
        s.set_org_member(org, bo, "member", None).unwrap();
        org
    };
    let m = new_mission(&st, &person(&st, None, ana), Some(org));
    let bo_scope = person(&st, None, bo);
    let d = mission(&st, &bo_scope, m.id, None).unwrap();
    assert!(!d.may_change);
    let mut args = by_id("mission_state", m.id);
    args.status = Some("active".into());
    assert_eq!(
        set_state(&args, &st, &bo_scope).unwrap_err().code,
        codes::E_FORBIDDEN
    );
    lock(&st)
        .unwrap()
        .set_org_member(org, bo, "admin", None)
        .unwrap();
    assert!(mission(&st, &bo_scope, m.id, None).unwrap().may_change);
    assert_eq!(set_state(&args, &st, &bo_scope).unwrap().state, "active");
}

#[test]
fn the_org_boundary_comes_before_the_owner() {
    let st = store();
    let (ana, _) = people(&st);
    let (acme, other) = {
        let s = lock(&st).unwrap();
        (
            s.add_org("Acme", None, false).unwrap().id,
            s.add_org("Other", None, false).unwrap().id,
        )
    };
    let m = new_mission(&st, &person(&st, None, ana), Some(acme));
    let bound = person(&st, Some(other), ana);
    assert!(missions(&st, &bound).unwrap().is_empty());
    assert_eq!(
        mission(&st, &bound, m.id, None).unwrap_err().code,
        codes::E_NOTFOUND
    );
}

#[test]
fn a_bound_caller_roots_its_mission_at_a_task_it_can_see() {
    let st = store();
    let (ana, _) = people(&st);
    let org = lock(&st).unwrap().add_org("Acme", None, false).unwrap().id;
    let bound = person(&st, Some(org), ana);
    let e = save(
        &save_args(MissionInput {
            name: Some("m".into()),
            goal: Some("g".into()),
            org_id: Some(org),
            ..Default::default()
        }),
        &st,
        &bound,
    )
    .unwrap_err();
    assert_eq!(e.code, codes::E_FORBIDDEN);
}

#[test]
fn a_caller_without_a_person_owns_no_mission() {
    let st = store();
    let host = Caller {
        host_alias: Some("h".into()),
        client: None,
        mode: TokenMode::Full,
        pane: None,
        is_personal_owner: false,
    }
    .view_scope(&lock(&st).unwrap())
    .unwrap();
    let e = save(
        &save_args(MissionInput {
            name: Some("m".into()),
            goal: Some("g".into()),
            ..Default::default()
        }),
        &st,
        &host,
    )
    .unwrap_err();
    assert_eq!(e.code, codes::E_FORBIDDEN);
}

#[test]
fn items_repos_and_the_phase() {
    let st = store();
    let (ana, _) = people(&st);
    let me = person(&st, None, ana);
    let m = new_mission(&st, &me, None);
    let (task, project) = {
        let s = lock(&st).unwrap();
        (
            s.create_local_work_item(None, "cards").unwrap().id,
            s.upsert_project("acme", "pay", "/src/pay").unwrap(),
        )
    };
    let mut args = by_id("mission_item", m.id);
    args.item_id = Some(task);
    assert_eq!(item(&args, &st, &me).unwrap().total, 2);
    let mut args = by_id("mission_repo", m.id);
    args.project_id = Some(project);
    args.role = Some("backend".into());
    assert_eq!(repo(&args, &st, &me).unwrap().repos.len(), 1);
    let d = mission(&st, &me, m.id, None).unwrap();
    assert_eq!(d.phase, None, "a draft has no loop phase");
    assert_eq!(d.items.len(), 2);
    assert_eq!(d.items[0].id, m.root_item_id.unwrap(), "root first");
    let mut args = by_id("mission_state", m.id);
    args.status = Some("active".into());
    set_state(&args, &st, &me).unwrap();
    assert_eq!(
        mission(&st, &me, m.id, None).unwrap().phase.as_deref(),
        Some("waiting")
    );
    let kinds: Vec<String> = mission(&st, &me, m.id, None)
        .unwrap()
        .events
        .into_iter()
        .map(|e| e.kind)
        .collect();
    assert_eq!(kinds, ["state", "repo_added", "item_added", "created"]);
}

#[test]
fn a_change_keeps_the_org_and_honours_the_version() {
    let st = store();
    let (ana, _) = people(&st);
    let me = person(&st, None, ana);
    let m = new_mission(&st, &me, None);
    let mut args = save_args(MissionInput {
        org_id: Some(1),
        ..Default::default()
    });
    args.mission_id = Some(m.id);
    assert_eq!(save(&args, &st, &me).unwrap_err().code, codes::E_INVALID);
    let mut args = save_args(MissionInput {
        level: Some(1),
        ..Default::default()
    });
    args.mission_id = Some(m.id);
    args.expected_version = Some(m.version);
    assert_eq!(save(&args, &st, &me).unwrap().level, 1);
    assert_eq!(save(&args, &st, &me).unwrap_err().code, codes::E_CONFLICT);
}

/// A live, running work session on `item` with its own worktree on `host`.
fn live_session(s: &Store, host: &str, name: &str, item: i64) -> i64 {
    s.upsert_host(host).unwrap();
    let pid = s.upsert_project("acme", "pay", "/src/pay").unwrap();
    let wt = s
        .upsert_worktree(
            pid,
            name,
            &format!("/src/pay/.worktrees/{name}"),
            Some(name),
        )
        .unwrap();
    let id = s
        .upsert_session(name, host, Some(pid), Some(wt), 1, 1, "running", None)
        .unwrap();
    s.link_session_work(id, crate::store::WorkTarget::Item(item), "manual")
        .unwrap();
    id
}

#[test]
fn a_finished_mission_lists_its_live_sessions_and_its_prs_and_reopens() {
    // Its own host alias: the per-worktree sizes are process-wide.
    const HOST: &str = "finish-test-host";
    let st = store();
    let (ana, bo) = people(&st);
    let me = person(&st, None, ana);
    let m = new_mission(&st, &me, None);
    let (task, mine, theirs) = {
        let s = lock(&st).unwrap();
        let task = s.create_local_work_item(None, "cards").unwrap().id;
        let mine = live_session(&s, HOST, "pay-cards", task);
        let theirs = live_session(&s, HOST, "pay-refunds", task);
        s.conn_ref()
            .execute(
                "UPDATE sessions SET owner_person_id = ?1 WHERE id = ?2",
                rusqlite::params![ana, mine],
            )
            .unwrap();
        s.conn_ref()
            .execute(
                "UPDATE sessions SET owner_person_id = ?1, \
                   pr_url = 'https://github.com/acme/pay/pull/476' WHERE id = ?2",
                rusqlite::params![bo, theirs],
            )
            .unwrap();
        Store::upsert_pull_request_in_tx(
            s.conn_ref(),
            "https://github.com/acme/pay/pull/476",
            Some("passing"),
            Some(&crate::service::outcome::PrEvidence {
                state: Some("MERGED".into()),
                ..Default::default()
            }),
            crate::store::PrSeenBy {
                session_id: theirs,
                session_name: "pay-refunds",
                host_alias: HOST,
                project_id: None,
            },
            100,
        )
        .unwrap();
        (task, mine, theirs)
    };
    crate::service::sessions::worktree_sizes::record(
        HOST,
        100,
        [("/src/pay/.worktrees/pay-cards".to_string(), 2_048)],
    );
    let mut args = by_id("mission_item", m.id);
    args.item_id = Some(task);
    item(&args, &st, &me).unwrap();

    // Going: no finish block.
    let mut st_args = by_id("mission_state", m.id);
    st_args.status = Some("active".into());
    set_state(&st_args, &st, &me).unwrap();
    assert_eq!(mission(&st, &me, m.id, None).unwrap().finish, None);

    st_args.status = Some("completed".into());
    set_state(&st_args, &st, &me).unwrap();
    let all = mission(&st, &ViewScope::internal(), m.id, None).unwrap();
    let f = all.finish.expect("a finished mission says what it leaves");
    assert_eq!(
        f.sessions.iter().map(|x| x.session_id).collect::<Vec<_>>(),
        [mine, theirs]
    );
    assert_eq!(f.sessions[0].item_id, task);
    assert_eq!(f.sessions[0].worktree_kb, Some(2_048), "measured");
    assert_eq!(f.sessions[1].worktree_kb, None, "not measured, never 0");
    assert_eq!(f.prs.len(), 1);
    assert_eq!(f.prs[0].number, Some(476));
    assert_eq!(f.prs[0].state, "MERGED");

    // Ana sees her own session only, and not the PR bo's session opened.
    let f = mission(&st, &me, m.id, None).unwrap().finish.unwrap();
    assert_eq!(
        f.sessions.iter().map(|x| x.session_id).collect::<Vec<_>>(),
        [mine]
    );
    assert!(f.prs.is_empty(), "{:?}", f.prs);

    // Archiving ends a session: it leaves the list, its PR stays (the
    // ended link keeps the URL).
    lock(&st).unwrap().delete_session(theirs).unwrap();
    let f = mission(&st, &ViewScope::internal(), m.id, None)
        .unwrap()
        .finish
        .unwrap();
    assert_eq!(
        f.sessions.iter().map(|x| x.session_id).collect::<Vec<_>>(),
        [mine]
    );
    assert_eq!(f.prs.len(), 1, "the PR outlives its session");

    // Reopen: paused, and no finish block.
    st_args.status = Some("paused".into());
    let r = set_state(&st_args, &st, &me).unwrap();
    assert_eq!(r.state, "paused");
    assert_eq!(r.finished_at, None);
    assert_eq!(mission(&st, &me, m.id, None).unwrap().finish, None);
}
