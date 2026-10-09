//! `runs { list }` in the store: the union, its mappings, its filters and
//! paging, the reach the service hands down, and the indexes it walks.

use super::*;
use crate::store::{NewAuxUsage, NewMission};

/// One of each source, at known times:
///
/// | t   | row                                                        |
/// |-----|------------------------------------------------------------|
/// | 100 | task 1: operator → worker `w` (on `a`), done                |
/// | 200 | task 2: `r` → `w`, failed "boom"                            |
/// | 300 | mission event: a step that started task 3                  |
/// | 300 | task 3: the mission's attempt, running, in `w2` (on `b`)    |
/// | 400 | Jev: a session-subject assist answer, not followed up      |
/// | 500 | aux: the mission's planner                                 |
/// | 600 | aux: a summary of `w`'s conversation                        |
/// | 700 | mission event: a budget brake                              |
/// | 800 | Jev: the offline benchmark (never listed)                  |
/// | 900 | mission event: `created` (audit, never listed)             |
struct Fx {
    s: Store,
    org_a: i64,
    org_b: i64,
    w: i64,
    r: i64,
    w2: i64,
    mission: i64,
}

fn fixture() -> Fx {
    let s = Store::open_in_memory().unwrap();
    s.insert_host("a", Some("a")).unwrap();
    s.insert_host("b", Some("b")).unwrap();
    let org_a = s.add_org("Acme", None, false).unwrap().id;
    let org_b = s.add_org("Beta", None, false).unwrap().id;
    s.set_host_org("a", Some(org_a)).unwrap();
    s.set_host_org("b", Some(org_b)).unwrap();
    let w = s
        .upsert_session("worker", "a", None, None, 1, 1, "running", None)
        .unwrap();
    let r = s
        .upsert_session("requester", "a", None, None, 1, 1, "running", None)
        .unwrap();
    let w2 = s
        .upsert_session("mission-run", "b", None, None, 1, 1, "running", None)
        .unwrap();
    s.conn
        .execute(
            "UPDATE sessions SET claude_session_id = 'conv-w' WHERE id = ?1",
            [w],
        )
        .unwrap();
    let item = s.create_local_work_item(None, "the item").unwrap().id;
    let m = s
        .create_mission(
            &NewMission {
                name: "Payments v2",
                goal: "ship it",
                org_id: Some(org_b),
                ..Default::default()
            },
            "person:1",
        )
        .unwrap();
    s.set_mission_item(m.id, item, true, "fleet").unwrap();
    let task = |id: i64, req: Option<i64>, wk: i64, state: &str, err: Option<&str>, at: i64| {
        s.conn
            .execute(
                "INSERT INTO tasks (id, requester_session_id, worker_session_id, prompt, state, \
                   error, created_at, started_at, finished_at, nonce, work_item_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8, 'n', ?9)",
                rusqlite::params![
                    id,
                    req,
                    wk,
                    format!("do thing {id}\nand more"),
                    state,
                    err,
                    at,
                    (state != "running").then_some(at + 5),
                    (id == 3).then_some(item),
                ],
            )
            .unwrap();
    };
    task(1, None, w, "done", None, 100);
    task(2, Some(r), w, "failed", Some("boom"), 200);
    task(3, None, w2, "running", None, 300);
    let event = |kind: &str, payload: &str, at: i64| {
        s.conn
            .execute(
                "INSERT INTO orchestration_events \
                   (orchestration_project_id, at, kind, actor, payload) \
                 VALUES (?1, ?2, ?3, 'loop', ?4)",
                rusqlite::params![m.id, at, kind, payload],
            )
            .unwrap();
    };
    // `create_mission` logged `created` at the wall clock: move it.
    s.conn
        .execute("UPDATE orchestration_events SET at = 900", [])
        .unwrap();
    event(
        "step",
        r#"{"step":"run","role":"implement","detail":"started","task_id":3}"#,
        300,
    );
    event("budget", r#"{"why":"over budget"}"#, 700);
    let decision = |at: i64, subject_kind: &str, subject: &str, org: Option<i64>| {
        s.conn
            .execute(
                "INSERT INTO decision_runs (at, feature, org_id, subject_kind, subject_id, mode, \
                   provider, model_version, question_version, answer, latency_ms, \
                   cost_microusd, called) \
                 VALUES (?1, 'status_map', ?2, ?3, ?4, 'assist', 'jev', 'jev-1', 'q1', \
                   'done', 1500, 21, 1)",
                rusqlite::params![at, org, subject_kind, subject],
            )
            .unwrap();
    };
    decision(400, "session", &w.to_string(), Some(org_a));
    decision(800, "bench", "x", None);
    s.insert_aux_usage(&NewAuxUsage {
        origin: crate::store::AUX_ORIGIN_PLANNER,
        host_alias: "b".into(),
        model: "sonnet".into(),
        mission_id: Some(m.id),
        org_id: Some(org_b),
        cost_micros: 900,
        input_tokens: Some(10),
        output_tokens: Some(2),
        at: 500,
        ..Default::default()
    })
    .unwrap();
    s.insert_aux_usage(&NewAuxUsage {
        origin: crate::store::AUX_ORIGIN_SUMMARY,
        host_alias: "a".into(),
        model: "haiku".into(),
        org_id: Some(org_a),
        claude_session_id: Some("conv-w".into()),
        cost_micros: 30,
        at: 600,
        ..Default::default()
    })
    .unwrap();
    Fx {
        s,
        org_a,
        org_b,
        w,
        r,
        w2,
        mission: m.id,
    }
}

fn list(fx: &Fx, f: RunsFilter) -> (Vec<RunRow>, i64) {
    fx.s.runs_list(&f).unwrap()
}

fn ids(rows: &[RunRow]) -> Vec<&str> {
    rows.iter().map(|r| r.id.as_str()).collect()
}

#[test]
fn the_union_lists_every_source_newest_first() {
    let fx = fixture();
    let (rows, total) = list(&fx, RunsFilter::default());
    assert_eq!(
        ids(&rows),
        [
            "orchestration:4",
            "aux:2",
            "aux:1",
            "jev:1",
            "orchestration:3",
            "task:3",
            "task:2",
            "task:1",
        ],
        "{rows:#?}"
    );
    assert_eq!(total, 8);
    let sources: std::collections::BTreeSet<_> = rows.iter().map(|r| r.source.as_str()).collect();
    // Every source but `routine`, whose fires have a fixture of their own
    // (`a_routines_fires_are_runs_linked_to_their_sessions`).
    let want: std::collections::BTreeSet<_> = RUN_SOURCES
        .iter()
        .copied()
        .filter(|s| *s != "routine")
        .collect();
    assert_eq!(sources, want);
    with_routine(&fx);
    let (rows, total) = list(&fx, RunsFilter::default());
    assert_eq!(total, 11);
    // Newest first across all five: the fires (t = 10, 20, 30) are the oldest.
    assert_eq!(
        ids(&rows[8..]),
        ["routine:3", "routine:2", "routine:1"],
        "{rows:#?}"
    );
}

#[test]
fn each_run_links_to_its_sessions() {
    let fx = fixture();
    let (rows, _) = list(&fx, RunsFilter::default());
    let by = |id: &str| rows.iter().find(|r| r.id == id).unwrap().clone();
    // A task: its worker, then its requester.
    assert_eq!(by("task:2").session_ids, [fx.w, fx.r]);
    assert_eq!(by("task:1").session_ids, [fx.w]);
    // A mission's step: the worker of the task it started.
    assert_eq!(by("orchestration:3").session_ids, [fx.w2]);
    // Jev about a session: that session.
    assert_eq!(by("jev:1").session_ids, [fx.w]);
    // A summary: the session holding the conversation it read.
    assert_eq!(by("aux:2").session_ids, [fx.w]);
    // A planner and a brake ran in no session.
    assert!(by("aux:1").session_ids.is_empty());
    assert!(by("orchestration:4").session_ids.is_empty());
}

#[test]
fn each_source_maps_owner_outcome_duration_and_cost() {
    let fx = fixture();
    let (rows, _) = list(&fx, RunsFilter::default());
    let by = |id: &str| rows.iter().find(|r| r.id == id).unwrap().clone();

    let t1 = by("task:1");
    assert_eq!(
        (t1.kind.as_str(), t1.owner.as_str()),
        ("operator", "operator")
    );
    assert_eq!(t1.outcome, "ok");
    assert_eq!((t1.ended_at, t1.duration_ms), (Some(105), Some(5000)));
    assert_eq!(t1.host.as_deref(), Some("a"));
    assert_eq!(t1.org_id, Some(fx.org_a));
    assert_eq!(t1.summary.as_deref(), Some("do thing 1"));
    assert_eq!(t1.cost_micros, None);

    let t2 = by("task:2");
    assert_eq!((t2.kind.as_str(), t2.owner.as_str()), ("task", "requester"));
    assert_eq!(
        (t2.outcome.as_str(), t2.error.as_deref()),
        ("failed", Some("boom"))
    );

    let t3 = by("task:3");
    assert_eq!(
        (t3.kind.as_str(), t3.owner.as_str()),
        ("mission", "Payments v2")
    );
    assert_eq!(t3.outcome, "running");
    assert_eq!(t3.mission_id, Some(fx.mission));
    assert_eq!(t3.org_id, Some(fx.org_b));
    assert_eq!(t3.duration_ms, None);

    let step = by("orchestration:3");
    assert_eq!(
        (step.kind.as_str(), step.outcome.as_str()),
        ("mission", "ok")
    );
    assert_eq!(step.summary.as_deref(), Some("loop: run started"));
    let brake = by("orchestration:4");
    assert_eq!(brake.outcome, "needs_person");
    assert_eq!(brake.summary.as_deref(), Some("loop: over budget"));
    assert_eq!(brake.org_id, Some(fx.org_b));

    let jev = by("jev:1");
    assert_eq!(
        (jev.kind.as_str(), jev.owner.as_str()),
        ("jev", "status_map")
    );
    // An assist answer nobody has followed up waits on a person.
    assert_eq!(jev.outcome, "needs_person");
    assert_eq!((jev.duration_ms, jev.cost_micros), (Some(1500), Some(21)));
    assert_eq!(jev.model.as_deref(), Some("jev-1"));

    let planner = by("aux:1");
    assert_eq!(
        (planner.kind.as_str(), planner.owner.as_str()),
        ("planner", "Payments v2")
    );
    assert_eq!(
        (planner.outcome.as_str(), planner.cost_micros),
        ("ok", Some(900))
    );
    assert_eq!(planner.model.as_deref(), Some("sonnet"));
    assert_eq!(planner.summary.as_deref(), Some("10 tokens in, 2 out"));
    let summary = by("aux:2");
    assert_eq!(
        (summary.kind.as_str(), summary.owner.as_str()),
        ("summary", "summary")
    );
}

#[test]
fn jev_outcomes_follow_the_fallback_answer_and_followup() {
    let fx = fixture();
    let ins = |fallback: Option<&str>, answer: Option<&str>, mode: &str, followup: Option<&str>| {
        fx.s.conn
            .execute(
                "INSERT INTO decision_runs (at, feature, subject_kind, subject_id, mode, \
                   provider, question_version, answer, fallback, followup, called) \
                 VALUES (50, 'start_project', 'work_start', 'item:1', ?1, 'jev', 'q', ?2, ?3, ?4, \
                   ?5)",
                rusqlite::params![
                    mode,
                    answer,
                    fallback,
                    followup,
                    fallback != Some("flag_off")
                ],
            )
            .unwrap();
        fx.s.conn.last_insert_rowid()
    };
    let cases = [
        (ins(Some("timeout"), None, "assist", None), "failed"),
        (
            ins(Some("low_confidence"), Some("7"), "assist", None),
            "nothing_to_do",
        ),
        (ins(None, Some("unsure"), "assist", None), "nothing_to_do"),
        (ins(None, Some("7"), "assist", Some("confirmed")), "ok"),
        (ins(None, Some("7"), "shadow", None), "ok"),
        // A proposal of nothing: no person is asked about it.
        (ins(None, Some("none"), "assist", None), "nothing_to_do"),
    ];
    // A run that never called the provider ran nothing: not listed.
    let not_asked = ins(Some("flag_off"), None, "off", None);
    let (rows, _) = list(&fx, RunsFilter::default());
    assert!(!rows.iter().any(|r| r.id == format!("jev:{not_asked}")));
    for (id, want) in cases {
        let row = rows.iter().find(|r| r.id == format!("jev:{id}")).unwrap();
        assert_eq!(row.outcome, want, "{row:?}");
        assert!(row.session_ids.is_empty(), "a work start ran in no session");
    }
    let timeout = rows
        .iter()
        .find(|r| r.id == format!("jev:{}", cases[0].0))
        .unwrap();
    assert_eq!(timeout.error.as_deref(), Some("timeout"));
}

#[test]
fn a_worker_that_reports_itself_blocked_needs_a_person() {
    let fx = fixture();
    fx.s.conn
        .execute(
            "UPDATE tasks SET result_json = '{\"outcome\":\"blocked\",\"summary\":\"s\"}' \
             WHERE id = 1",
            [],
        )
        .unwrap();
    let (rows, _) = list(&fx, RunsFilter::default());
    assert_eq!(
        rows.iter().find(|r| r.id == "task:1").unwrap().outcome,
        "needs_person"
    );
}

#[test]
fn filters_narrow_and_paging_counts_the_whole() {
    let fx = fixture();
    let f = |g: fn(&mut RunsFilter)| {
        let mut f = RunsFilter::default();
        g(&mut f);
        f
    };
    let (rows, total) = list(
        &fx,
        f(|f| {
            f.since = Some(300);
            f.until = Some(600);
        }),
    );
    assert_eq!(ids(&rows), ["aux:1", "jev:1", "orchestration:3", "task:3"]);
    assert_eq!(total, 4);

    let (rows, _) = list(&fx, f(|f| f.kind = Some("mission".into())));
    assert_eq!(ids(&rows), ["orchestration:4", "orchestration:3", "task:3"]);
    let (rows, _) = list(&fx, f(|f| f.kind = Some("planner".into())));
    assert_eq!(ids(&rows), ["aux:1"]);
    let (rows, _) = list(&fx, f(|f| f.outcome = Some("failed".into())));
    assert_eq!(ids(&rows), ["task:2"]);
    let (rows, _) = list(&fx, f(|f| f.outcome = Some("needs_person".into())));
    assert_eq!(ids(&rows), ["orchestration:4", "jev:1"]);

    let mut by_org = RunsFilter {
        org_id: Some(fx.org_b),
        ..Default::default()
    };
    let (rows, _) = list(&fx, by_org.clone());
    assert_eq!(
        ids(&rows),
        ["orchestration:4", "aux:1", "orchestration:3", "task:3"]
    );
    by_org.org_id = Some(fx.org_a);
    let (rows, _) = list(&fx, by_org);
    assert_eq!(ids(&rows), ["aux:2", "jev:1", "task:2", "task:1"]);

    let (rows, _) = list(
        &fx,
        RunsFilter {
            mission_id: Some(fx.mission),
            ..Default::default()
        },
    );
    assert_eq!(
        ids(&rows),
        ["orchestration:4", "aux:1", "orchestration:3", "task:3"]
    );

    let (rows, _) = list(
        &fx,
        RunsFilter {
            session_id: Some(fx.w),
            ..Default::default()
        },
    );
    assert_eq!(ids(&rows), ["aux:2", "jev:1", "task:2", "task:1"]);
    let (rows, _) = list(
        &fx,
        RunsFilter {
            session_id: Some(fx.w2),
            ..Default::default()
        },
    );
    assert_eq!(ids(&rows), ["orchestration:3", "task:3"]);

    // Paging: the page moves, the total does not.
    let (rows, total) = list(
        &fx,
        RunsFilter {
            limit: 3,
            offset: 3,
            ..Default::default()
        },
    );
    assert_eq!(ids(&rows), ["jev:1", "orchestration:3", "task:3"]);
    assert_eq!(total, 8);
    // The cap holds, and a nonsense page is clamped rather than refused.
    let (rows, _) = list(
        &fx,
        RunsFilter {
            limit: 10_000,
            offset: -4,
            ..Default::default()
        },
    );
    assert_eq!(rows.len(), 8);
    let (sql, params) = page_sql(&RunsFilter {
        limit: 10_000,
        ..Default::default()
    });
    assert!(sql.ends_with("LIMIT ? OFFSET ?"));
    assert_eq!(params[params.len() - 2], Value::Integer(RUNS_MAX_LIMIT));
}

#[test]
fn an_unknown_kind_or_outcome_is_refused() {
    let fx = fixture();
    for f in [
        RunsFilter {
            kind: Some("cron".into()),
            ..Default::default()
        },
        RunsFilter {
            outcome: Some("meh".into()),
            ..Default::default()
        },
    ] {
        assert_eq!(fx.s.runs_list(&f).unwrap_err().code, codes::E_INVALID);
    }
}

#[test]
fn a_scoped_reach_shows_only_what_it_names() {
    let fx = fixture();
    let scoped = |sessions: Vec<i64>, missions: Vec<i64>, spend: bool| RunsFilter {
        reach: RunsReach::Scoped {
            sessions,
            missions,
            routines: vec![],
            spend,
        },
        ..Default::default()
    };
    // Nothing named: nothing seen.
    let (rows, total) = list(&fx, scoped(vec![], vec![], false));
    assert!(rows.is_empty(), "{rows:#?}");
    assert_eq!(total, 0);

    // Org a's sessions only (what a client bound to org a sees): its tasks,
    // the Jev run about its session and the summary of its conversation —
    // nothing of org b's mission.
    let (rows, total) = list(&fx, scoped(vec![fx.w, fx.r], vec![], false));
    assert_eq!(ids(&rows), ["aux:2", "jev:1", "task:2", "task:1"]);
    assert_eq!(total, 4);

    // A task needs EVERY session it names: the worker alone is not enough
    // for a task the caller cannot see the requester of.
    let (rows, _) = list(&fx, scoped(vec![fx.w], vec![], false));
    assert_eq!(ids(&rows), ["aux:2", "jev:1", "task:1"]);

    // The mission: its actions, brakes and planner, and its task through
    // its session.
    let (rows, _) = list(&fx, scoped(vec![fx.w2], vec![fx.mission], false));
    assert_eq!(
        ids(&rows),
        ["orchestration:4", "aux:1", "orchestration:3", "task:3"]
    );

    // A detached task is the hub's alone.
    fx.s.conn
        .execute("UPDATE tasks SET detached_at = 1 WHERE id = 1", [])
        .unwrap();
    let (rows, _) = list(&fx, scoped(vec![fx.w, fx.r], vec![], false));
    assert_eq!(ids(&rows), ["aux:2", "jev:1", "task:2"]);

    // Whole-fleet spend adds the runs that belong to no session.
    fx.s.conn
        .execute(
            "UPDATE decision_runs SET subject_kind = 'tracker_section' \
             WHERE subject_kind = 'session'",
            [],
        )
        .unwrap();
    let (rows, _) = list(&fx, scoped(vec![fx.w, fx.r], vec![], false));
    assert_eq!(ids(&rows), ["aux:2", "task:2"]);
    let (rows, _) = list(&fx, scoped(vec![fx.w, fx.r], vec![], true));
    assert_eq!(ids(&rows), ["aux:2", "aux:1", "jev:1", "task:2"]);
}

/// The plan of the page query: each branch walks an index on its own time
/// column or filter column, never the whole table.
#[test]
fn the_union_walks_its_indexes() {
    let fx = fixture();
    let plan = |f: &RunsFilter| -> Vec<String> {
        let (sql, params) = page_sql(f);
        let mut stmt =
            fx.s.conn
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .unwrap();
        stmt.query_map(rusqlite::params_from_iter(params), |r| {
            r.get::<_, String>(3)
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
    };
    let has = |p: &[String], idx: &str| {
        assert!(
            p.iter().any(|d| d.contains(idx)),
            "expected {idx} in {p:#?}"
        );
    };
    let none_scans = |p: &[String], tables: &[&str]| {
        for t in tables {
            assert!(
                !p.iter()
                    .any(|d| d.starts_with(&format!("SCAN {t} ")) || d == &format!("SCAN {t}")),
                "a full scan of {t}: {p:#?}"
            );
        }
    };
    let window = RunsFilter {
        since: Some(100),
        until: Some(1000),
        ..Default::default()
    };
    let p = plan(&window);
    has(&p, "idx_tasks_created");
    has(&p, "idx_orch_events_kind_at");
    has(&p, "decision_runs_at");
    has(&p, "idx_aux_usage_at");
    has(&p, "routine_runs_started");
    none_scans(&p, &["t", "e", "d", "a", "rr"]);

    let by_org = RunsFilter {
        org_id: Some(fx.org_a),
        ..Default::default()
    };
    let p = plan(&by_org);
    has(&p, "decision_runs_org_at");
    has(&p, "idx_aux_usage_org_at");

    let by_mission = RunsFilter {
        mission_id: Some(fx.mission),
        ..Default::default()
    };
    let p = plan(&by_mission);
    has(&p, "idx_orch_events");
    has(&p, "idx_aux_usage_mission");
    has(&p, "idx_tasks_item");
    none_scans(&p, &["t", "e", "a"]);

    let by_session = RunsFilter {
        session_id: Some(fx.w),
        ..Default::default()
    };
    let p = plan(&by_session);
    has(&p, "decision_runs_subject");
    has(&p, "idx_aux_usage_conversation");
    has(&p, "idx_tasks_worker");
    has(&p, "idx_tasks_requester");
    has(&p, "routine_runs_session");
    none_scans(&p, &["t", "d", "a", "rr"]);

    // A summary's session is found through the conversation index.
    has(&plan(&RunsFilter::default()), "idx_sessions_claude_session");
}

/// A routine's fires (migration 131): a done one in `w`, a skipped one in
/// no session, a failed one.
fn with_routine(fx: &Fx) -> i64 {
    fx.s.conn
        .execute(
            "INSERT INTO routines (org_id, name, trigger, host_alias, project_id, prompt, \
               created_at, updated_at) \
             VALUES (?1, 'Nightly tidy', 'cron', 'a', 1, 'tidy up', 1, 1)",
            [fx.org_a],
        )
        .unwrap();
    let routine = fx.s.conn.last_insert_rowid();
    let fire = |state: &str, reason: Option<&str>, session: Option<i64>, at: i64| {
        fx.s.conn
            .execute(
                "INSERT INTO routine_runs (routine_id, trigger, state, reason, session_id, \
                   cost_micros, started_at, finished_at) \
                 VALUES (?1, 'cron', ?2, ?3, ?4, 77, ?5, ?6)",
                rusqlite::params![
                    routine,
                    state,
                    reason,
                    session,
                    at,
                    (state != "running").then_some(at + 60)
                ],
            )
            .unwrap();
    };
    fire("done", None, Some(fx.w), 10);
    fire("skipped", Some("a run is still open"), None, 20);
    fire("failed", Some("over budget"), Some(fx.w), 30);
    routine
}

#[test]
fn a_routines_fires_are_runs_linked_to_their_sessions() {
    let fx = fixture();
    let routine = with_routine(&fx);
    let (rows, total) = list(
        &fx,
        RunsFilter {
            kind: Some("routine".into()),
            ..Default::default()
        },
    );
    assert_eq!(ids(&rows), ["routine:3", "routine:2", "routine:1"]);
    assert_eq!(total, 3);
    let by = |id: &str| rows.iter().find(|r| r.id == id).unwrap().clone();
    let done = by("routine:1");
    assert_eq!(
        (done.owner.as_str(), done.outcome.as_str()),
        ("Nightly tidy", "ok")
    );
    assert_eq!(done.session_ids, [fx.w]);
    assert_eq!(done.routine_id, Some(routine));
    assert_eq!(
        (done.duration_ms, done.cost_micros),
        (Some(60_000), Some(77))
    );
    assert_eq!(
        (done.host.as_deref(), done.org_id),
        (Some("a"), Some(fx.org_a))
    );
    let skipped = by("routine:2");
    assert_eq!(skipped.outcome, "nothing_to_do");
    assert!(skipped.session_ids.is_empty());
    assert_eq!(
        skipped.summary.as_deref(),
        Some("cron: a run is still open")
    );
    let failed = by("routine:3");
    assert_eq!(
        (failed.outcome.as_str(), failed.error.as_deref()),
        ("failed", Some("over budget"))
    );

    // One routine's fires, and nothing else.
    let (_, total) = list(
        &fx,
        RunsFilter {
            routine_id: Some(routine),
            ..Default::default()
        },
    );
    assert_eq!(total, 3);

    // The session filter finds the fires that ran in it.
    let (rows, _) = list(
        &fx,
        RunsFilter {
            session_id: Some(fx.w),
            kind: Some("routine".into()),
            ..Default::default()
        },
    );
    assert_eq!(ids(&rows), ["routine:3", "routine:1"]);

    // A fire is its routine's: a scoped reader sees it only with the routine.
    let scoped = |routines: Vec<i64>| RunsFilter {
        kind: Some("routine".into()),
        reach: RunsReach::Scoped {
            sessions: vec![fx.w, fx.r],
            missions: vec![],
            routines,
            spend: true,
        },
        ..Default::default()
    };
    assert_eq!(list(&fx, scoped(vec![])).1, 0);
    assert_eq!(list(&fx, scoped(vec![routine])).1, 3);
}

#[test]
fn every_aux_origin_is_a_kind_a_filter_finds() {
    let fx = fixture();
    fx.s.insert_aux_usage(&NewAuxUsage {
        origin: crate::store::AUX_ORIGIN_TRIAGE,
        host_alias: "a".into(),
        model: "haiku".into(),
        cost_micros: 5,
        at: 700,
        ..Default::default()
    })
    .unwrap();
    let (rows, total) = list(
        &fx,
        RunsFilter {
            kind: Some("triage".into()),
            ..Default::default()
        },
    );
    assert_eq!(total, 1, "{rows:?}");
    assert_eq!(rows[0].kind, "triage");
    for origin in crate::store::AUX_ORIGINS {
        assert!(RUN_KINDS.contains(origin), "{origin} is not a run kind");
    }
}
