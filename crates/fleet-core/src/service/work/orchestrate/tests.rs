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
        "the mission asks for 2"
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
