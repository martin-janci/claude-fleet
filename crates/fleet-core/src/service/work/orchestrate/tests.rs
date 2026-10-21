//! The mission loop: autonomy, grants, cards, a person's steps and one
//! tick (orchestration O4–O8).

use super::*;
use crate::mcp::auth::{Caller, ClientRef, TokenMode};
use crate::net::https::FakeTransport;
use crate::service::work::missions::{self, MissionInput};
use crate::store::{TaskReport, TreeEntry};

struct Fx {
    deps: Deps,
    me: ViewScope,
    m: MissionRow,
    root: i64,
}

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
        is_fleet_admin: false,
    }
    .view_scope(&lock(store).unwrap())
    .unwrap()
}

fn host(store: &Mutex<Store>) -> ViewScope {
    Caller {
        api: None,
        host_alias: Some("h".into()),
        client: None,
        mode: TokenMode::Full,
        pane: None,
        is_personal_owner: false,
        is_fleet_admin: false,
    }
    .view_scope(&lock(store).unwrap())
    .unwrap()
}

fn by_id(action: &str, id: i64) -> WorkLinkArgs {
    WorkLinkArgs {
        action: action.into(),
        mission_id: Some(id),
        ..Default::default()
    }
}

/// An active level-2 mission of ana's, with its root.
fn fixture() -> Fx {
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    let ana = lock(&store).unwrap().create_person("ana", None).unwrap().id;
    let me = person(&store, ana);
    let m = missions::save(
        &WorkLinkArgs {
            action: "mission_save".into(),
            mission: Some(MissionInput {
                name: Some("m".into()),
                goal: Some("g".into()),
                level: Some(2),
                ..Default::default()
            }),
            ..Default::default()
        },
        &store,
        &me,
    )
    .unwrap();
    let m = missions::set_state(
        &WorkLinkArgs {
            status: Some("active".into()),
            ..by_id("mission_state", m.id)
        },
        &store,
        &me,
    )
    .unwrap();
    let root = m.root_item_id.unwrap();
    let deps = Deps {
        store,
        ssh: Arc::new(crate::ssh::SshClient::new()),
        reg: CancellationRegistry::new(),
        net: TrackerNet::fake(Arc::new(FakeTransport::new())),
    };
    Fx { deps, me, m, root }
}

fn member(fx: &Fx, title: &str) -> i64 {
    let s = lock(&fx.deps.store).unwrap();
    let made = s
        .propose_tree(
            fx.root,
            &[TreeEntry {
                title: title.into(),
                ..Default::default()
            }],
            "t",
        )
        .unwrap();
    s.accept_proposals(&[made[0].id]).unwrap();
    made[0].id
}

fn mission_now(fx: &Fx) -> MissionRow {
    lock(&fx.deps.store)
        .unwrap()
        .get_mission(fx.m.id)
        .unwrap()
        .unwrap()
}

#[test]
fn autonomy_is_the_least_of_ceiling_level_and_grant() {
    let fx = fixture();
    let s = lock(&fx.deps.store).unwrap();
    let a = autonomy(&s, &fx.m, 0).unwrap();
    assert_eq!((a.asked, a.ceiling, a.effective), (2, 1, 1));
    s.set_setting(settings::ORCHESTRATOR_MAX_LEVEL, "3")
        .unwrap();
    let a = autonomy(&s, &fx.m, 0).unwrap();
    assert_eq!(a.effective, 1);
    assert!(a.why.contains("without a grant"), "{}", a.why);
    drop(s);
    grant(
        &WorkLinkArgs {
            level: Some(3),
            hours: Some(2),
            ..by_id("mission_grant", fx.m.id)
        },
        &fx.deps.store,
        &fx.me,
    )
    .unwrap();
    let s = lock(&fx.deps.store).unwrap();
    assert_eq!(
        autonomy(&s, &fx.m, now_unix()).unwrap().effective,
        2,
        "the row read before the grant asks for 2"
    );
    drop(s);
    let m = mission_now(&fx);
    let s = lock(&fx.deps.store).unwrap();
    assert_eq!(
        (m.level, autonomy(&s, &m, now_unix()).unwrap().effective),
        (3, 3),
        "signing L3 raised the ask to it"
    );
    s.set_setting(settings::ORCHESTRATOR_ENABLED, "false")
        .unwrap();
    let a = autonomy(&s, &fx.m, now_unix()).unwrap();
    assert_eq!((a.effective, a.enabled), (0, false));
}

#[test]
fn only_a_person_signs_a_grant_and_revoking_ends_it() {
    let fx = fixture();
    let args = |level: i64, hours: u32| WorkLinkArgs {
        level: Some(level),
        hours: Some(hours),
        ..by_id("mission_grant", fx.m.id)
    };
    let h = host(&fx.deps.store);
    assert_eq!(
        grant(&args(2, 1), &fx.deps.store, &h).unwrap_err().code,
        codes::E_FORBIDDEN
    );
    assert_eq!(
        grant(&args(4, 1), &fx.deps.store, &fx.me).unwrap_err().code,
        codes::E_INVALID
    );
    assert_eq!(
        grant(&args(2, 24 * 8), &fx.deps.store, &fx.me)
            .unwrap_err()
            .code,
        codes::E_INVALID
    );
    let g = grant(&args(2, 1), &fx.deps.store, &fx.me).unwrap();
    assert_eq!(g.level, 2);
    assert_eq!(
        revoke(&by_id("mission_revoke", fx.m.id), &fx.deps.store, &fx.me).unwrap(),
        1
    );
    let s = lock(&fx.deps.store).unwrap();
    assert!(s.live_mission_grant(fx.m.id, now_unix()).unwrap().is_none());
}

/// TASK-293: a mission made in the room asks L0 and the loop runs at the
/// least of the ask, the ceiling and the grant, so an L2 grant on it never
/// started a run. Signing raises the ask to the grant's level.
#[test]
fn a_grant_raises_a_lower_ask_so_the_loop_runs_under_it() {
    let fx = fixture();
    let s = lock(&fx.deps.store).unwrap();
    s.set_setting(settings::ORCHESTRATOR_MAX_LEVEL, "2")
        .unwrap();
    s.update_mission(
        fx.m.id,
        None,
        &crate::store::MissionPatch {
            level: Some(0),
            ..Default::default()
        },
        "ana",
    )
    .unwrap();
    drop(s);
    let m = mission_now(&fx);
    assert_eq!(
        autonomy(&lock(&fx.deps.store).unwrap(), &m, now_unix())
            .unwrap()
            .effective,
        0
    );
    grant(
        &WorkLinkArgs {
            level: Some(2),
            ..by_id("mission_grant", fx.m.id)
        },
        &fx.deps.store,
        &fx.me,
    )
    .unwrap();
    let m = mission_now(&fx);
    let s = lock(&fx.deps.store).unwrap();
    assert_eq!(m.level, 2);
    let a = autonomy(&s, &m, now_unix()).unwrap();
    assert!(a.effective >= AUTO_LEVEL, "{a:?}");
    let e = &s.mission_events(fx.m.id, None, 1).unwrap()[0];
    assert_eq!(e.kind, "grant");
    assert_eq!(e.payload.as_ref().unwrap()["raised_from"], 0);
    drop(s);
    // A lower grant never lowers what the mission asks.
    grant(
        &WorkLinkArgs {
            level: Some(1),
            ..by_id("mission_grant", fx.m.id)
        },
        &fx.deps.store,
        &fx.me,
    )
    .unwrap();
    assert_eq!(mission_now(&fx).level, 2);
}

/// TASK-293: a finite mission's grant may last until it completes, bounded
/// by its budget alone.
#[test]
fn a_grant_until_done_needs_a_budget_and_ends_with_the_finite_mission() {
    let fx = fixture();
    let args = |hours: Option<u32>, cents: Option<i64>| WorkLinkArgs {
        level: Some(2),
        hours,
        budget_cents: cents,
        until_done: Some(true),
        ..by_id("mission_grant", fx.m.id)
    };
    for bad in [args(None, None), args(Some(2), Some(500))] {
        assert_eq!(
            grant(&bad, &fx.deps.store, &fx.me).unwrap_err().code,
            codes::E_INVALID
        );
    }
    let g = grant(&args(None, Some(500)), &fx.deps.store, &fx.me).unwrap();
    assert!(g.until_done);
    assert_eq!(
        (g.expires_at, g.budget_micros),
        (GRANT_UNTIL_DONE_AT, Some(5_000_000))
    );
    let s = lock(&fx.deps.store).unwrap();
    let in_a_month = now_unix() + 30 * 86_400;
    assert!(s.live_mission_grant(fx.m.id, in_a_month).unwrap().is_some());
    let e = &s.mission_events(fx.m.id, None, 1).unwrap()[0];
    assert_eq!(e.payload.as_ref().unwrap()["until_done"], true);
    // Made continuous, it never completes: the grant stops holding.
    s.update_mission(
        fx.m.id,
        None,
        &crate::store::MissionPatch {
            mode: Some("continuous".into()),
            ..Default::default()
        },
        "ana",
    )
    .unwrap();
    assert!(s.live_mission_grant(fx.m.id, now_unix()).unwrap().is_none());
    drop(s);
    assert_eq!(
        grant(&args(None, Some(500)), &fx.deps.store, &fx.me)
            .unwrap_err()
            .code,
        codes::E_INVALID,
        "a continuous mission's grant runs some hours"
    );
    let s = lock(&fx.deps.store).unwrap();
    s.update_mission(
        fx.m.id,
        None,
        &crate::store::MissionPatch {
            mode: Some("finite".into()),
            ..Default::default()
        },
        "ana",
    )
    .unwrap();
    assert!(s.live_mission_grant(fx.m.id, now_unix()).unwrap().is_some());
    // It ends with the mission: reopened, it has none.
    s.set_mission_state(fx.m.id, None, "completed", "ana")
        .unwrap();
    s.set_mission_state(fx.m.id, None, "paused", "ana").unwrap();
    assert!(s.live_mission_grant(fx.m.id, now_unix()).unwrap().is_none());
}

#[test]
fn pause_all_pauses_what_the_caller_may_change_and_revokes_its_grants() {
    let fx = fixture();
    grant(
        &WorkLinkArgs {
            level: Some(2),
            ..by_id("mission_grant", fx.m.id)
        },
        &fx.deps.store,
        &fx.me,
    )
    .unwrap();
    let bo = lock(&fx.deps.store)
        .unwrap()
        .create_person("bo", None)
        .unwrap()
        .id;
    let bo = person(&fx.deps.store, bo);
    assert!(pause_all(&fx.deps.store, &bo).unwrap().is_empty());
    assert_eq!(pause_all(&fx.deps.store, &fx.me).unwrap(), vec![fx.m.id]);
    assert_eq!(mission_now(&fx).state, "paused");
    let s = lock(&fx.deps.store).unwrap();
    assert!(s.live_mission_grant(fx.m.id, now_unix()).unwrap().is_none());
}

#[tokio::test]
async fn a_person_closes_an_implemented_item_and_the_mission_completes() {
    let fx = fixture();
    let item = member(&fx, "one");
    {
        let s = lock(&fx.deps.store).unwrap();
        let t = s.insert_task(None, None, "go", "n-1").unwrap();
        s.set_task_run(t.id, item, 1, "implement").unwrap();
        let t = s.get_task(t.id).unwrap().unwrap();
        let report = TaskReport {
            summary: "did it".into(),
            outcome: "done".into(),
            ..Default::default()
        };
        crate::service::tasks::complete_task_reported(&s, &t, "", Some(&report)).unwrap();
        s.set_task_evidence(t.id, &evidence(2, 3)).unwrap();
    }
    let plan = {
        let m = mission_now(&fx);
        let s = lock(&fx.deps.store).unwrap();
        plan_for(&s, &m).unwrap().unwrap()
    };
    let key = format!("close:{item}");
    assert!(
        plan.steps.iter().any(|s| s.key() == key),
        "{:?}",
        plan.steps
    );
    // A step that is not next is refused.
    let e = start(
        &WorkLinkArgs {
            step: Some("run:999".into()),
            ..by_id("mission_start", fx.m.id)
        },
        &fx.deps,
        &fx.me,
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, codes::E_INVALID_STATE);
    let out = start(
        &WorkLinkArgs {
            step: Some(key),
            ..by_id("mission_start", fx.m.id)
        },
        &fx.deps,
        &fx.me,
    )
    .await
    .unwrap();
    assert!(out.results[0].ok, "{:?}", out.results);
    let steps = {
        let m = mission_now(&fx);
        let s = lock(&fx.deps.store).unwrap();
        plan_for(&s, &m).unwrap().unwrap().steps
    };
    assert!(steps.iter().any(|s| s.kind == "complete"), "{steps:?}");
    let out = start(&by_id("mission_start", fx.m.id), &fx.deps, &fx.me)
        .await
        .unwrap();
    assert!(out.results.iter().all(|r| r.ok), "{:?}", out.results);
    assert_eq!(mission_now(&fx).state, "completed");
}

fn evidence(commits: u32, files: u32) -> crate::store::TaskEvidence {
    crate::store::TaskEvidence {
        at: 1,
        commits_total: commits,
        files_total: files,
        ..Default::default()
    }
}

/// The completion gate's run evidence (mission #4): an implement run that
/// reports done but committed nothing and changed no file is not closed by
/// the loop. It asks a person, naming what is missing, until a person
/// overrides it with a reason.
#[tokio::test]
async fn the_loop_does_not_close_an_implement_item_with_no_run_evidence() {
    let fx = granted();
    let item = member(&fx, "one");
    {
        let s = lock(&fx.deps.store).unwrap();
        let t = s.insert_task(None, None, "go", "n-1").unwrap();
        s.set_task_run(t.id, item, 1, "implement").unwrap();
        let t = s.get_task(t.id).unwrap().unwrap();
        let report = TaskReport {
            summary: "did it".into(),
            outcome: "done".into(),
            ..Default::default()
        };
        crate::service::tasks::complete_task_reported(&s, &t, "", Some(&report)).unwrap();
        s.set_task_evidence(t.id, &evidence(0, 0)).unwrap();
    }
    let steps = {
        let m = mission_now(&fx);
        let s = lock(&fx.deps.store).unwrap();
        plan_for(&s, &m).unwrap().unwrap().steps
    };
    assert!(!steps.iter().any(|st| st.kind == "close"), "{steps:?}");
    let ask = steps
        .iter()
        .find(|st| st.kind == "ask" && st.item_id == Some(item))
        .unwrap_or_else(|| panic!("{steps:?}"));
    assert!(ask.reason.contains("no run evidence"), "{}", ask.reason);
    assert!(
        ask.context
            .as_deref()
            .is_some_and(|c| c.contains("committed nothing")),
        "{ask:?}"
    );
    // A stale plan's close is refused for the loop.
    let close = Step {
        kind: "close".into(),
        item_id: Some(item),
        role: None,
        reason: "stale".into(),
        context: None,
        auto: true,
    };
    let r = apply_step(
        &fx.deps,
        &fx.m,
        &close,
        &Actor::Loop,
        &ViewScope::internal(),
    )
    .await;
    assert!(!r.ok);
    assert!(r.detail.contains("no run evidence"), "{}", r.detail);
    assert_ne!(
        lock(&fx.deps.store)
            .unwrap()
            .get_work_item(item)
            .unwrap()
            .unwrap()
            .status_category,
        "done"
    );
    // A person's override, with a reason, lets the close through.
    crate::service::work::verify::verify(
        &WorkLinkArgs {
            action: "verify".into(),
            item_id: Some(item),
            line: Some(crate::service::work::gate::EVIDENCE_LINE.into()),
            ok: Some(true),
            note: Some("the fix was a host config change".into()),
            ..Default::default()
        },
        &fx.deps.store,
        &fx.me,
    )
    .unwrap();
    let steps = {
        let m = mission_now(&fx);
        let s = lock(&fx.deps.store).unwrap();
        plan_for(&s, &m).unwrap().unwrap().steps
    };
    assert!(
        steps
            .iter()
            .any(|st| st.kind == "close" && st.item_id == Some(item)),
        "{steps:?}"
    );
    let r = apply_step(
        &fx.deps,
        &fx.m,
        &close,
        &Actor::Loop,
        &ViewScope::internal(),
    )
    .await;
    assert!(r.ok, "{}", r.detail);
}

/// The loop applies the planner's cards one by one, each perhaps after an
/// SSH start; a card a person dismissed meanwhile stays dismissed and is
/// never applied, and one being decided by a person is left to them.
#[tokio::test]
async fn the_loop_never_applies_a_card_a_person_decided_meanwhile() {
    let fx = fixture();
    let (one, two, three) = (member(&fx, "one"), member(&fx, "two"), member(&fx, "three"));
    let hold = |n: &str, item: i64| {
        lock(&fx.deps.store)
            .unwrap()
            .add_card(
                fx.m.id,
                &NewCard {
                    decision_id: n,
                    source: "planner",
                    kind: "hold",
                    work_item_id: Some(item),
                    payload: None,
                },
            )
            .unwrap()
            .unwrap()
    };
    let cards = vec![hold("t:1", one), hold("t:2", two), hold("t:3", three)];
    let dismiss = WorkLinkArgs {
        action: "card_decide".into(),
        card_id: Some(cards[1].id),
        ok: Some(false),
        ..Default::default()
    };
    decide_card(&dismiss, &fx.deps, &fx.me).await.unwrap();
    // A person is mid-decision on the third.
    let _theirs = CardClaim::take(cards[2].id).unwrap();
    let m = mission_now(&fx);
    let shown = loop_apply_cards(&fx.deps, &m, cards, AUTO_LEVEL, true)
        .await
        .unwrap();
    let states: Vec<&str> = shown.iter().map(|c| c.state.as_str()).collect();
    assert_eq!(states, ["applied", "dismissed", "open"]);
    let held = |item: i64| {
        lock(&fx.deps.store)
            .unwrap()
            .get_work_item(item)
            .unwrap()
            .unwrap()
            .held_at
            .is_some()
    };
    assert!(held(one));
    assert!(!held(two), "a dismissed card is never applied");
    assert!(!held(three), "a card a person is deciding is theirs");
}

#[tokio::test]
async fn a_card_is_decided_once_and_never_by_an_agent() {
    let fx = fixture();
    let item = member(&fx, "one");
    let card = lock(&fx.deps.store)
        .unwrap()
        .add_card(
            fx.m.id,
            &NewCard {
                decision_id: "t:1",
                source: "planner",
                kind: "hold",
                work_item_id: Some(item),
                payload: None,
            },
        )
        .unwrap()
        .unwrap();
    let args = |ok: bool| WorkLinkArgs {
        action: "card_decide".into(),
        card_id: Some(card.id),
        ok: Some(ok),
        ..Default::default()
    };
    let h = host(&fx.deps.store);
    assert_eq!(
        decide_card(&args(true), &fx.deps, &h)
            .await
            .unwrap_err()
            .code,
        codes::E_FORBIDDEN
    );
    let c = decide_card(&args(true), &fx.deps, &fx.me).await.unwrap();
    assert_eq!(c.state, "applied");
    assert!(lock(&fx.deps.store)
        .unwrap()
        .get_work_item(item)
        .unwrap()
        .unwrap()
        .held_at
        .is_some());
    assert_eq!(
        decide_card(&args(false), &fx.deps, &fx.me)
            .await
            .unwrap_err()
            .code,
        codes::E_INVALID_STATE
    );
}

#[tokio::test]
async fn a_tick_takes_the_lease_and_sets_the_next_wake() {
    let fx = fixture();
    let now = now_unix();
    {
        let s = lock(&fx.deps.store).unwrap();
        assert_eq!(s.missions_due(now).unwrap(), vec![fx.m.id]);
    }
    // No host for the planner: the tick says so in the log and goes on.
    tick_once(&fx.deps, now).await;
    let m = mission_now(&fx);
    assert!(
        m.next_wake_at.is_some_and(|w| w > now),
        "{:?}",
        m.next_wake_at
    );
    let s = lock(&fx.deps.store).unwrap();
    assert!(s.missions_due(now).unwrap().is_empty());
    // A finished task wakes it again.
    s.wake_mission(fx.m.id).unwrap();
    assert_eq!(s.missions_due(now_unix() + 1).unwrap(), vec![fx.m.id]);
}

/// Redesign 8.2: a planner run adds a cost row with its origin, and the
/// budget brake counts it with the workers' spend.
#[test]
fn a_planner_run_adds_a_cost_row_the_budget_brake_counts() {
    let fx = fixture();
    let raw = r#"{"type":"result","subtype":"success","is_error":false,"result":"[]","total_cost_usd":0.0125,"usage":{"input_tokens":900,"output_tokens":40}}"#;
    let (answer, usage) = planner::planner_answer(raw.to_string());
    assert_eq!(answer, "[]", "the model's text, out of the envelope");
    let s = lock(&fx.deps.store).unwrap();
    assert_eq!(s.mission_cost_micros(fx.m.id).unwrap(), 0);
    book_planner_run(&s, &fx.m, "h", "sonnet", usage.as_ref(), 100);
    let rows = s.mission_aux_usage(fx.m.id).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (
            rows[0].origin.as_str(),
            rows[0].host_alias.as_str(),
            rows[0].model.as_str()
        ),
        ("planner", "h", "sonnet")
    );
    assert_eq!(
        (rows[0].input_tokens, rows[0].output_tokens),
        (Some(900), Some(40))
    );
    assert_eq!(rows[0].cost_micros, 12_500);
    assert_eq!(s.mission_cost_micros(fx.m.id).unwrap(), 12_500);
    // A reply with no envelope is still a run: booked at 0.
    let (answer, usage) = planner::planner_answer("[]".to_string());
    assert_eq!((answer.as_str(), usage.is_none()), ("[]", true));
    book_planner_run(&s, &fx.m, "h", "sonnet", None, 101);
    assert_eq!(s.mission_aux_usage(fx.m.id).unwrap().len(), 2);
    assert_eq!(s.mission_cost_micros(fx.m.id).unwrap(), 12_500);
}

/// Redesign 8.1: Pause all stops the mission loop before it takes a lease.
#[tokio::test]
async fn pause_all_stops_the_mission_tick() {
    let fx = fixture();
    let now = now_unix();
    settings::set(
        &lock(&fx.deps.store).unwrap(),
        settings::AUTOMATION_PAUSED,
        "true",
    )
    .unwrap();
    tick_once(&fx.deps, now).await;
    let m = mission_now(&fx);
    assert_eq!(m.next_wake_at, fx.m.next_wake_at, "the tick did nothing");
    let s = lock(&fx.deps.store).unwrap();
    assert_eq!(s.missions_due(now).unwrap(), vec![fx.m.id], "still due");
}

/// Review r06 F5: a step the loop planned before Pause all was pressed is
/// refused when its turn comes, before any run starts.
#[tokio::test]
async fn a_loop_step_planned_before_pause_all_does_not_start() {
    let fx = fixture();
    let item = member(&fx, "one");
    settings::set(
        &lock(&fx.deps.store).unwrap(),
        settings::AUTOMATION_PAUSED,
        "true",
    )
    .unwrap();
    let step = Step {
        kind: "run".into(),
        item_id: Some(item),
        role: Some("implement".into()),
        reason: "ready".into(),
        context: None,
        auto: true,
    };
    let r = apply_step(&fx.deps, &fx.m, &step, &Actor::Loop, &ViewScope::internal()).await;
    assert!(!r.ok);
    assert!(r.detail.contains("stood down"), "{}", r.detail);
    let s = lock(&fx.deps.store).unwrap();
    assert_eq!(s.mission_task_counts(fx.m.id).unwrap().open, 0);
}

/// [`fixture`] with the loop allowed to act: a level-2 grant under a
/// level-3 ceiling.
fn granted() -> Fx {
    let fx = fixture();
    lock(&fx.deps.store)
        .unwrap()
        .set_setting(settings::ORCHESTRATOR_MAX_LEVEL, "3")
        .unwrap();
    grant(
        &WorkLinkArgs {
            level: Some(2),
            hours: Some(2),
            ..by_id("mission_grant", fx.m.id)
        },
        &fx.deps.store,
        &fx.me,
    )
    .unwrap();
    fx
}

fn loop_run(item: i64) -> Step {
    Step {
        kind: "run".into(),
        item_id: Some(item),
        role: Some("implement".into()),
        reason: "the planner said so".into(),
        context: None,
        auto: true,
    }
}

/// A planner card the loop applies holds to the parallel limit, as the
/// steps `plan_steps` counts slots for do.
#[tokio::test]
async fn a_loop_run_past_the_parallel_limit_is_refused() {
    let fx = granted();
    let (a, b, c) = (member(&fx, "a"), member(&fx, "b"), member(&fx, "c"));
    {
        let s = lock(&fx.deps.store).unwrap();
        for item in [a, b] {
            crate::service::work::run::record_headless_run(&s, "x", item, 1, "implement").unwrap();
        }
        assert_eq!(s.mission_task_counts(fx.m.id).unwrap().open, 2);
    }
    let r = apply_step(
        &fx.deps,
        &fx.m,
        &loop_run(c),
        &Actor::Loop,
        &ViewScope::internal(),
    )
    .await;
    assert!(!r.ok);
    assert!(r.detail.contains("2 at once"), "{}", r.detail);
    let s = lock(&fx.deps.store).unwrap();
    assert_eq!(s.mission_task_counts(fx.m.id).unwrap().open, 2);
}

/// A hold stops the loop: a held item whose attempt failed is neither
/// retried nor run by a planner card.
#[tokio::test]
async fn a_held_item_is_not_retried_or_run_by_the_loop() {
    let fx = granted();
    let item = member(&fx, "one");
    {
        let s = lock(&fx.deps.store).unwrap();
        let t =
            crate::service::work::run::record_headless_run(&s, "x", item, 1, "implement").unwrap();
        crate::service::tasks::fail_task(&s, t.id, "boom").unwrap();
        s.set_item_hold(item, true, "ana").unwrap();
        let m = s.get_mission(fx.m.id).unwrap().unwrap();
        let a = autonomy(&s, &m, now_unix()).unwrap();
        let steps = steps_for(&s, &m, &a).unwrap();
        assert!(
            !steps.iter().any(|st| st.item_id == Some(item)
                && matches!(st.kind.as_str(), "run" | "retry" | "review" | "close")),
            "{steps:?}"
        );
    }
    let r = apply_step(
        &fx.deps,
        &fx.m,
        &loop_run(item),
        &Actor::Loop,
        &ViewScope::internal(),
    )
    .await;
    assert!(!r.ok);
    assert!(r.detail.contains("holds"), "{}", r.detail);
}

#[test]
fn a_continuous_mission_wakes_on_its_timer() {
    let mut m = fixture().m;
    let idle = MissionTaskCounts::default();
    assert_eq!(next_wake(&m, &idle, 100), 100 + IDLE_RECHECK_SECS);
    m.mode = "continuous".into();
    m.policy.wake_every_secs = Some(600);
    assert_eq!(next_wake(&m, &idle, 100), 700);
    let busy = MissionTaskCounts {
        open: 1,
        ..Default::default()
    };
    assert_eq!(next_wake(&m, &busy, 100), 100 + RUNNING_RECHECK_SECS);
}

/// Redesign 2.2 (migration 124): a run's session is the mission's, whoever
/// took the step.
#[test]
fn a_runs_start_is_recorded_as_the_missions() {
    let fx = fixture();
    let item = member(&fx, "a");
    let s = lock(&fx.deps.store).unwrap();
    for actor in [Actor::Loop, Actor::Person("fleet".into())] {
        let start = start_for(&s, &fx.m, item, &actor, None).unwrap();
        assert_eq!(
            start.origin,
            Some(crate::store::SessionOrigin::mission(fx.m.id)),
            "{actor:?}"
        );
    }
}

/// Redesign 8.7: a grant names its login, and the loop holds a run whose
/// account is past `accounts.pause_at` (the plan's check: the loop skips an
/// account over its threshold), saying so once.
#[test]
fn the_loop_holds_runs_on_an_account_over_the_line() {
    let fx = fixture();
    let item = member(&fx, "a");
    let g = grant(
        &WorkLinkArgs {
            level: Some(2),
            hosts: Some(vec!["mac".into()]),
            profile: Some(" work ".into()),
            ..by_id("mission_grant", fx.m.id)
        },
        &fx.deps.store,
        &fx.me,
    )
    .unwrap();
    assert_eq!(g.profile.as_deref(), Some("work"));
    assert_eq!(
        grant(
            &WorkLinkArgs {
                level: Some(2),
                profile: Some("../x".into()),
                ..by_id("mission_grant", fx.m.id)
            },
            &fx.deps.store,
            &fx.me,
        )
        .unwrap_err()
        .code,
        codes::E_INVALID
    );
    let s = lock(&fx.deps.store).unwrap();
    let g = s.live_mission_grant(fx.m.id, now_unix()).unwrap().unwrap();
    assert_eq!(g.profile.as_deref(), Some("work"), "read back");
    let start = start_for(&s, &fx.m, item, &Actor::Loop, Some(&g)).unwrap();
    assert_eq!(start.profile.as_deref(), Some("work"), "the run bills it");
    let now = 5_000;
    crate::service::account_limits::seed_usage(&s, "mac", Some("work"), "work", 91.0, now);
    let run = Step {
        kind: "run".into(),
        item_id: Some(item),
        role: Some("implement".into()),
        reason: String::new(),
        context: None,
        auto: true,
    };
    let ask = Step {
        kind: "ask".into(),
        auto: false,
        ..run.clone()
    };
    let limit_events = |s: &Store| {
        s.mission_events(fx.m.id, None, 50)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "account_limit")
            .count()
    };
    for _ in 0..2 {
        let mut steps = vec![run.clone(), ask.clone()];
        hold_runs_over_limit(&s, &fx.m, Some(&g), &mut steps, now).unwrap();
        assert_eq!(steps, vec![ask.clone()], "the run waits, the ask stays");
    }
    assert_eq!(limit_events(&s), 1, "said once");
    let e = &s.mission_events(fx.m.id, None, 1).unwrap()[0];
    let why = e.payload.as_ref().unwrap()["why"].as_str().unwrap();
    assert!(
        why.contains("profile work on mac") && why.contains("91%"),
        "{why}"
    );
    // Review r01: another event in between does not start a new episode.
    event(&s, fx.m.id, "card", "me", None, serde_json::json!({}));
    let mut steps = vec![run.clone()];
    hold_runs_over_limit(&s, &fx.m, Some(&g), &mut steps, now).unwrap();
    assert_eq!(limit_events(&s), 1, "one held episode, said once");
    // A run that went ends the episode: the next hold is said again.
    event(
        &s,
        fx.m.id,
        "step",
        "loop",
        Some(item),
        serde_json::json!({ "step": "run" }),
    );
    let mut steps = vec![run.clone()];
    hold_runs_over_limit(&s, &fx.m, Some(&g), &mut steps, now).unwrap();
    assert_eq!(limit_events(&s), 2, "a new episode");
    // Under the line again: the run goes.
    crate::service::account_limits::seed_usage(&s, "mac", Some("work"), "work", 30.0, now);
    let mut steps = vec![run.clone()];
    hold_runs_over_limit(&s, &fx.m, Some(&g), &mut steps, now).unwrap();
    assert_eq!(steps, vec![run]);
}

/// Review r05 F5: the over-limit hold checks the host the start will land
/// on. A mission with no repos and an item with no project starts where a
/// start rule says (`tickets::seen_place`), not where nothing is known: the
/// rule's host at 95% holds the run.
#[test]
fn the_hold_checks_the_host_a_start_rule_picks() {
    let fx = fixture();
    let item = member(&fx, "a");
    let s = lock(&fx.deps.store).unwrap();
    let key = s.get_work_item(item).unwrap().unwrap().key.unwrap();
    let pid = s.upsert_project("o", "r", "/repo").unwrap();
    let now = 5_000;
    s.insert_start_rule(None, None, &key, pid, Some("mac"), "active", 0, now)
        .unwrap();
    crate::service::account_limits::seed_usage(&s, "mac", None, "acc-mac", 95.0, now);
    let start = start_for(&s, &fx.m, item, &Actor::Loop, None).unwrap();
    assert_eq!((start.project_id, start.host_alias), (None, None));
    let run = Step {
        kind: "run".into(),
        item_id: Some(item),
        role: Some("implement".into()),
        reason: String::new(),
        context: None,
        auto: true,
    };
    let mut steps = vec![run];
    hold_runs_over_limit(&s, &fx.m, None, &mut steps, now).unwrap();
    assert!(steps.is_empty(), "held: {steps:?}");
    let e = &s.mission_events(fx.m.id, None, 1).unwrap()[0];
    let why = e.payload.as_ref().unwrap()["why"].as_str().unwrap();
    assert!(why.contains("mac's own login"), "{why}");
}

/// Review round 15, F19: what makes a mission due rests partly on agents'
/// own reports, so completing it is a person's act at every level. The
/// loop's step is not its own to take, the planner's card is not applied by
/// the loop, and the loop is refused if it tries; a person still can.
#[tokio::test]
async fn the_loop_never_completes_a_mission() {
    for level in 0..=3 {
        assert!(
            !loop_applies_card("complete", level),
            "the planner's complete card waits for a person at L{level}"
        );
        assert!(!loop_applies_card("ask", level));
    }
    assert!(loop_applies_card("run", AUTO_LEVEL));
    let fx = fixture();
    let step = Step {
        kind: "complete".into(),
        item_id: Some(fx.root),
        role: None,
        reason: "every task is done".into(),
        context: None,
        auto: true,
    };
    let m = mission_now(&fx);
    let r = apply_step(&fx.deps, &m, &step, &Actor::Loop, &ViewScope::internal()).await;
    assert!(!r.ok, "{}", r.detail);
    assert!(r.detail.contains("a person completes"), "{}", r.detail);
    assert_eq!(mission_now(&fx).state, "active");
    let r = apply_step(
        &fx.deps,
        &m,
        &step,
        &Actor::Person("person:1".into()),
        &fx.me,
    )
    .await;
    assert!(r.ok, "{}", r.detail);
    assert_eq!(mission_now(&fx).state, "completed");
}

/// Contract 14: the missions list carries each mission's spend and its
/// grant's budget, and the plan an average run's cost for the run cards.
#[test]
fn a_mission_lists_its_spend_and_estimates_a_run_from_finished_ones() {
    let fx = fixture();
    let item = member(&fx, "a");
    {
        let s = lock(&fx.deps.store).unwrap();
        assert_eq!(run_estimate(&s, fx.m.id).unwrap(), None, "no history yet");
        s.upsert_host("h").unwrap();
        for (n, cost, finished) in [
            ("w1", 2_000_000, Some(10)),
            ("w2", 4_000_000, Some(20)),
            ("w3", 9_000_000, None),
        ] {
            let sid = s
                .upsert_session(n, "h", None, None, 0, 0, "running", None)
                .unwrap();
            s.conn_ref()
                .execute(
                    "UPDATE sessions SET usage_cost_micros = ?2 WHERE id = ?1",
                    rusqlite::params![sid, cost],
                )
                .unwrap();
            s.conn_ref()
                .execute(
                    "INSERT INTO tasks (worker_session_id, state, created_at, finished_at, nonce, \
                                        work_item_id) VALUES (?1, 'done', 1, ?2, ?3, ?4)",
                    rusqlite::params![sid, finished, n, item],
                )
                .unwrap();
        }
        let e = run_estimate(&s, fx.m.id).unwrap().unwrap();
        assert_eq!(
            (e.micros, e.runs, e.basis.as_str()),
            (3_000_000, 2, "mission"),
            "the two finished runs; the open one is not counted"
        );
        s.add_grant(
            fx.m.id,
            &crate::store::NewGrant {
                level: 2,
                granted_by: "me",
                hosts: None,
                budget_micros: Some(40_000_000),
                max_parallel: None,
                profile: None,
                expires_at: now_unix() + 3600,
            },
        )
        .unwrap();
    }
    let listed = missions::missions(&fx.deps.store, &fx.me).unwrap();
    let m = listed.iter().find(|m| m.id == fx.m.id).unwrap();
    assert_eq!(m.cost_micros, Some(15_000_000), "every worker's spend");
    assert_eq!(m.budget_micros, Some(40_000_000));
    let json = serde_json::to_value(m).unwrap();
    assert_eq!(json["cost_micros"], 15_000_000);
    // A plain store read leaves both out of the wire.
    let raw = serde_json::to_value(mission_now(&fx)).unwrap();
    assert!(raw.get("cost_micros").is_none(), "{raw}");
}

/// Review r01: two planner runs in one second card their answers apart (the
/// ids do not collide), and a mission has one planner call at a time.
#[test]
fn two_planner_runs_in_one_second_keep_both_answers_and_run_one_at_a_time() {
    let fx = fixture();
    let item = member(&fx, "a");
    let cmds = vec![Command::Run {
        item_id: item,
        role: None,
    }];
    let first = card_commands(&fx.deps, &fx.m, &cmds, 1_000).unwrap();
    let second = card_commands(&fx.deps, &fx.m, &cmds, 1_000).unwrap();
    assert_eq!(
        (first.len(), second.len()),
        (1, 1),
        "neither run's card is dropped"
    );
    assert_ne!(first[0].decision_id, second[0].decision_id);

    let slot = PlannerSlot::take(&fx.deps, &fx.m).unwrap();
    assert_eq!(
        PlannerSlot::take(&fx.deps, &fx.m).err().map(|e| e.code),
        Some(codes::E_EXISTS.to_string()),
        "a second call waits for the first"
    );
    drop(slot);
    assert!(
        PlannerSlot::take(&fx.deps, &fx.m).is_ok(),
        "the slot is given back"
    );
}

/// Two decisions on one card at once: the second hears it is being decided
/// instead of applying the card a second time, and the claim is released
/// with the first.
#[test]
fn a_card_is_claimed_while_it_is_decided() {
    let first = CardClaim::take(987_654).expect("free");
    let second = CardClaim::take(987_654).expect_err("held");
    assert_eq!(second.code, codes::E_INVALID_STATE);
    drop(first);
    assert!(CardClaim::take(987_654).is_ok(), "released with the first");
}

/// Control review F5: a run no longer takes the grant's first host; it goes
/// where placement finds the most headroom inside the grant, on the login
/// with it, and keeps the grant's first host when nothing there qualifies.
#[test]
fn a_run_goes_to_the_best_host_inside_its_grant() {
    let fx = fixture();
    let item = member(&fx, "a");
    let g = grant(
        &WorkLinkArgs {
            level: Some(2),
            hosts: Some(vec!["mac".into(), "htz".into()]),
            ..by_id("mission_grant", fx.m.id)
        },
        &fx.deps.store,
        &fx.me,
    )
    .unwrap();
    let s = lock(&fx.deps.store).unwrap();
    let now = now_unix();
    let seed = crate::service::account_limits::seed_usage;
    seed(&s, "mac", None, "acct-mac", 80.0, now);
    seed(&s, "htz", None, "acct-htz", 70.0, now);
    seed(&s, "htz", Some("spare"), "acct-spare", 5.0, now);
    crate::service::account_limits::ready_host(&s, "mac");
    crate::service::account_limits::ready_host(&s, "htz");
    let start = start_for(&s, &fx.m, item, &Actor::Loop, Some(&g)).unwrap();
    assert_eq!(start.host_alias.as_deref(), Some("htz"));
    assert_eq!(
        start.profile.as_deref(),
        Some("spare"),
        "the login with headroom"
    );
    // Every login in the grant over the line: the first host, as before.
    seed(&s, "mac", None, "acct-mac", 95.0, now);
    seed(&s, "htz", None, "acct-htz", 95.0, now);
    seed(&s, "htz", Some("spare"), "acct-spare", 99.0, now);
    let start = start_for(&s, &fx.m, item, &Actor::Loop, Some(&g)).unwrap();
    assert_eq!(start.host_alias.as_deref(), Some("mac"));
    assert_eq!(start.profile, None);
}

/// Mission #6 (2026-10-20): a run stuck open for over an hour braked the
/// mission, and every resume braked it again on the same stale run before
/// the tick tried a step. A person's resume restarts the window.
#[tokio::test]
async fn a_persons_resume_restarts_the_no_progress_window() {
    let fx = fixture();
    let item = member(&fx, "a");
    let now = now_unix();
    let stale = now - 2 * fx.m.policy.no_progress_secs as i64;
    {
        let s = lock(&fx.deps.store).unwrap();
        crate::service::work::run::record_headless_run(&s, "x", item, 1, "implement").unwrap();
        let c = s.conn_for_test();
        c.execute("UPDATE tasks SET created_at = ?1, started_at = ?1", [stale])
            .unwrap();
        c.execute(
            "UPDATE orchestration_events SET at = ?1 WHERE kind = 'state'",
            [stale],
        )
        .unwrap();
    }
    let braked = || {
        lock(&fx.deps.store)
            .unwrap()
            .conn_for_test()
            .query_row(
                "SELECT COUNT(*) FROM orchestration_events \
                 WHERE orchestration_project_id = ?1 AND kind = 'no_progress'",
                [fx.m.id],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
    };
    tick_mission(&fx.deps, fx.m.id, now).await.unwrap();
    assert_eq!(mission_now(&fx).state, "paused", "the stale run brakes it");
    assert_eq!(braked(), 1);

    missions::set_state(
        &WorkLinkArgs {
            status: Some("active".into()),
            ..by_id("mission_state", fx.m.id)
        },
        &fx.deps.store,
        &fx.me,
    )
    .unwrap();
    tick_mission(&fx.deps, fx.m.id, now_unix()).await.unwrap();
    assert_eq!(mission_now(&fx).state, "active", "the resume gets a window");
    assert_eq!(braked(), 1);

    // The window still closes: an hour on with nothing moving brakes again.
    let later = now_unix() + fx.m.policy.no_progress_secs as i64 + 1;
    tick_mission(&fx.deps, fx.m.id, later).await.unwrap();
    assert_eq!(mission_now(&fx).state, "paused");
    assert_eq!(braked(), 2);
}
