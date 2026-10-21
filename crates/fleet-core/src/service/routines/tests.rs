//! Routines on a fake clock with a fake start path: the schedule, Skip
//! next, the overlap rule, both budgets, Pause all, event triggers, how a
//! run settles, and the fences (owner, org member and admin, org
//! boundary, the unknown-id answer).

use super::*;
use crate::mcp::auth::{Caller, ClientRef, TokenMode};
use crate::store::{SessionRow, UsageDelta, UsageTotals};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

/// 2026-10-08 00:00 UTC, a Thursday.
const OCT8: i64 = 1_791_417_600;
const H: i64 = 3600;

/// What a run asked the start path for.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Started {
    host: String,
    project: i64,
    profile: Option<String>,
    owner: Option<i64>,
    origin: Option<String>,
    origin_ref: Option<String>,
    friendly: Option<String>,
}

/// A start path that makes the row the real one would, origin and owner
/// included, without a host.
struct FakeSpawn {
    store: Arc<Mutex<Store>>,
    started: Mutex<Vec<Started>>,
    n: AtomicUsize,
    fail: AtomicBool,
    /// Only a start on this host fails (G3.8's fallback).
    fail_on: Mutex<Option<String>>,
    /// The turns a time cap stopped: `(host, tmux name)`.
    stopped: Mutex<Vec<(String, String)>>,
    /// The prompts typed into a run's session: `(session id, prompt)`.
    typed: Mutex<Vec<(i64, String)>>,
    /// Typing a prompt fails, saying this.
    untypable: Mutex<Option<String>>,
}

#[async_trait::async_trait]
impl tick::Spawn for FakeSpawn {
    async fn spawn(
        &self,
        args: crate::service::sessions::NewSessionArgs,
    ) -> Result<SessionRow, IpcError> {
        if self.fail.load(Ordering::SeqCst)
            || self.fail_on.lock().unwrap().as_deref() == Some(args.host_alias.as_str())
        {
            return Err(IpcError::new(codes::E_SSH, "host unreachable"));
        }
        let origin = args.origin.clone().expect("a routine names its origin");
        self.started.lock().unwrap().push(Started {
            host: args.host_alias.clone(),
            project: args.project_id,
            profile: args.profile.clone(),
            owner: args.owner_person_id,
            origin: Some(origin.origin.to_string()),
            origin_ref: origin.origin_ref.clone(),
            friendly: args.friendly_name.clone(),
        });
        let n = self.n.fetch_add(1, Ordering::SeqCst);
        let s = lock(&self.store)?;
        let id = s.upsert_session(
            &format!("rt-{n}"),
            &args.host_alias,
            Some(args.project_id),
            None,
            1,
            1,
            "running",
            None,
        )?;
        s.claim_if_unclaimed(id, args.owner_person_id)?;
        s.set_session_origin(id, &origin)?;
        Ok(s.get_session_by_id(id)?.unwrap())
    }
    async fn stop_turn(&self, host_alias: &str, tmux_name: &str) -> Result<(), IpcError> {
        self.stopped
            .lock()
            .unwrap()
            .push((host_alias.to_string(), tmux_name.to_string()));
        Ok(())
    }
    async fn deliver(&self, row: &SessionRow, prompt: &str, handover: i64) -> Result<(), String> {
        if let Some(why) = self.untypable.lock().unwrap().clone() {
            return Err(why);
        }
        // As `seed_routine` does once the prompt is in.
        lock(&self.store)
            .unwrap()
            .mark_handovers_delivered(&[handover], None)
            .unwrap();
        self.typed
            .lock()
            .unwrap()
            .push((row.id, prompt.to_string()));
        Ok(())
    }
}

struct Fx {
    store: Arc<Mutex<Store>>,
    fake: Arc<FakeSpawn>,
    deps: Deps,
    ana: i64,
    bo: i64,
    project: i64,
}

fn fx() -> Fx {
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    let (ana, bo, project) = {
        let s = lock(&store).unwrap();
        s.insert_host("mac", Some("mac")).unwrap();
        s.insert_host("nas", Some("nas")).unwrap();
        (
            s.create_person("ana", None).unwrap().id,
            s.create_person("bo", None).unwrap().id,
            s.upsert_project("acme", "web", "/src").unwrap(),
        )
    };
    let fake = Arc::new(FakeSpawn {
        store: Arc::clone(&store),
        started: Mutex::new(Vec::new()),
        n: AtomicUsize::new(0),
        fail: AtomicBool::new(false),
        fail_on: Mutex::new(None),
        stopped: Mutex::new(Vec::new()),
        typed: Mutex::new(Vec::new()),
        untypable: Mutex::new(None),
    });
    let deps = Deps {
        store: Arc::clone(&store),
        spawn: fake.clone(),
    };
    Fx {
        store,
        fake,
        deps,
        ana,
        bo,
        project,
    }
}

/// A person's device, bound to `org` or to none, scoped the way a request is.
fn person(store: &Mutex<Store>, org: Option<i64>, id: i64) -> ViewScope {
    Caller {
        api: None,
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
        is_fleet_admin: false,
    }
    .view_scope(&lock(store).unwrap())
    .unwrap()
}

fn input(f: &Fx) -> RoutineInput {
    RoutineInput {
        name: "Morning PR sweep".into(),
        trigger: "cron".into(),
        cron: Some("0 9 * * 1-5".into()),
        host_alias: "mac".into(),
        project_id: f.project,
        profile: Some("work".into()),
        prompt: "Review my open PRs and list what needs me.".into(),
        ..Default::default()
    }
}

fn new_routine(f: &Fx, input: RoutineInput) -> RoutineRow {
    save(&f.store, &person(&f.store, None, f.ana), None, &input).unwrap()
}

/// Put the routine's next fire at `at`, as the scheduler would have.
fn due_at(f: &Fx, id: i64, at: i64) {
    lock(&f.store)
        .unwrap()
        .advance_routine(id, Some(at), true)
        .unwrap();
}

fn routine(f: &Fx, id: i64) -> RoutineRow {
    lock(&f.store).unwrap().get_routine(id).unwrap().unwrap()
}

/// A routine that retries a failed run once.
fn retrying_routine(f: &Fx) -> RoutineRow {
    let mut i = input(f);
    i.retry_once = Some(true);
    new_routine(f, i)
}

/// `run` is the retry of run `failed`.
fn assert_retry_of(run: &RoutineRunRow, failed: i64) {
    assert_eq!(run.trigger_ref, Some(format!("retry:{failed}")), "{run:?}");
}

fn runs_of(f: &Fx, id: i64) -> Vec<RoutineRunRow> {
    let mut r = lock(&f.store).unwrap().routine_runs(id, 50).unwrap();
    r.reverse();
    r
}

fn spend(f: &Fx, session: i64, cost: i64) {
    lock(&f.store)
        .unwrap()
        .apply_usage(
            session,
            "mac",
            &UsageDelta {
                reset: true,
                totals: UsageTotals {
                    cost_micros: cost,
                    ..Default::default()
                },
                model: None,
                offset: cost,
                source: "s.jsonl".into(),
                last_msg_id: None,
                last_msg_usage: None,
                now: OCT8,
                by_day: Vec::new(),
                backfill_until: None,
            },
        )
        .unwrap();
}

fn event(f: &Fx, session: i64, kind: &str) {
    lock(&f.store)
        .unwrap()
        .insert_session_event(session, kind, None)
        .unwrap();
}

#[tokio::test]
async fn a_due_cron_routine_starts_its_session_and_queues_its_prompt() {
    let f = fx();
    let r = new_routine(&f, input(&f));
    assert_eq!(r.owner_person_id, Some(f.ana));
    assert!(r.next_run_at.is_some(), "a new cron routine is scheduled");
    // Thursday 09:00 UTC.
    let nine = OCT8 + 9 * H;
    due_at(&f, r.id, nine);
    // Not due a minute early.
    tick_once(&f.deps, nine - 60).await;
    assert!(runs_of(&f, r.id).is_empty());
    tick_once(&f.deps, nine + 5).await;
    let runs = runs_of(&f, r.id);
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].state, "running");
    assert_eq!(runs[0].trigger, "cron");
    assert_eq!(runs[0].scheduled_for, Some(nine));
    let started = f.fake.started.lock().unwrap().clone();
    assert_eq!(
        started,
        vec![Started {
            host: "mac".into(),
            project: f.project,
            profile: Some("work".into()),
            owner: Some(f.ana),
            origin: Some("routine".into()),
            origin_ref: Some(r.id.to_string()),
            friendly: Some("Morning PR sweep".into()),
        }]
    );
    let sid = runs[0].session_id.unwrap();
    let s = lock(&f.store).unwrap();
    let row = s.get_session_by_id(sid).unwrap().unwrap();
    assert_eq!(row.origin.as_deref(), Some("routine"));
    assert_eq!(row.owner_person_id, Some(f.ana));
    // The next fire is Friday 09:00, counted from the fire.
    assert_eq!(
        s.get_routine(r.id).unwrap().unwrap().next_run_at,
        Some(nine + 24 * H)
    );
}

/// Let the background typing of the runs just fired finish.
async fn typing_done() {
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
}

/// The run's prompt is typed into its session: queued alone, it sat in a
/// fresh session that took no turn, and the run stayed `running` with
/// nothing done.
#[tokio::test]
async fn a_run_types_its_prompt_into_its_session() {
    let f = fx();
    let r = new_routine(&f, input(&f));
    let run = tick::fire(&f.deps, &r, "run_now", None, None, OCT8)
        .await
        .unwrap();
    typing_done().await;
    let sid = run.session_id.unwrap();
    let typed = f.fake.typed.lock().unwrap().clone();
    assert_eq!(typed, vec![(sid, queued_prompt(&f, sid))]);
    assert!(typed[0].1.starts_with("Review my open PRs"));
    assert_eq!(runs_of(&f, r.id)[0].state, "running");
}

/// A prompt that could not be typed fails the run with a fix that opens
/// its session (where the dialog or the dead REPL is), so it shows and
/// `retry_once` can take it.
#[tokio::test]
async fn a_run_whose_prompt_was_not_typed_fails() {
    let f = fx();
    let r = new_routine(&f, input(&f));
    *f.fake.untypable.lock().unwrap() = Some("a dialog is up in its session (trust_prompt)".into());
    tick::fire(&f.deps, &r, "run_now", None, None, OCT8)
        .await
        .unwrap();
    typing_done().await;
    let run = runs_of(&f, r.id).pop().unwrap();
    assert_eq!(run.state, "failed");
    assert_eq!(run.error_code.as_deref(), Some(fix::E_PROMPT_NOT_DELIVERED));
    assert_eq!(
        run.reason.as_deref(),
        Some("its prompt was not delivered: a dialog is up in its session (trust_prompt)")
    );
    let named = fix::fix(&routine(&f, r.id), &run).unwrap();
    assert_eq!(
        (named.label.as_str(), named.action.as_str()),
        ("Open its session", "session")
    );
    // Its queued prompt is retired: a person typing into the session later
    // must not hand Claude the routine's work a second time.
    let left = lock(&f.store)
        .unwrap()
        .undelivered_handovers(run.session_id.unwrap())
        .unwrap();
    assert!(left.is_empty(), "{left:?}");
}

/// The hub restarted while a run's prompt was being typed: nothing will
/// ever type it, so past `PROMPT_DELIVERY_SECS` the run fails as
/// undelivered (and its queued prompt is retired) instead of holding the
/// routine for six hours as "still going".
#[tokio::test]
async fn a_run_whose_typing_was_lost_fails_as_undelivered() {
    let f = fx();
    let r = new_routine(&f, input(&f));
    // Fired without the background typing ever running (a restart).
    let run = tick::fire(&f.deps, &r, "run_now", None, None, OCT8)
        .await
        .unwrap();
    let sid = run.session_id.unwrap();
    // Undo the fake's typing: the restart lost it.
    lock(&f.store)
        .unwrap()
        .conn_ref()
        .execute(
            "UPDATE work_journal SET delivered_at = NULL WHERE kind = 'handover'",
            [],
        )
        .unwrap();
    tick::settle(&lock(&f.store).unwrap(), OCT8 + 60).unwrap();
    assert_eq!(
        runs_of(&f, r.id)[0].state,
        "running",
        "still inside the budget"
    );
    tick::settle(
        &lock(&f.store).unwrap(),
        OCT8 + tick::PROMPT_DELIVERY_SECS + 1,
    )
    .unwrap();
    let run = runs_of(&f, r.id).pop().unwrap();
    assert_eq!(run.state, "failed");
    assert_eq!(run.error_code.as_deref(), Some(fix::E_PROMPT_NOT_DELIVERED));
    assert!(lock(&f.store)
        .unwrap()
        .undelivered_handovers(sid)
        .unwrap()
        .is_empty());
}

/// A run that closed before its typing gave up keeps how it closed.
#[tokio::test]
async fn a_late_typing_failure_leaves_a_closed_run_alone() {
    let f = fx();
    let r = new_routine(&f, input(&f));
    let run = tick::fire(&f.deps, &r, "run_now", None, None, OCT8)
        .await
        .unwrap();
    event(&f, run.session_id.unwrap(), "turn_done");
    tick::settle(&lock(&f.store).unwrap(), OCT8 + 60).unwrap();
    tick::undelivered(&f.store, run.id, "too late", OCT8 + 120, None);
    let run = runs_of(&f, r.id).pop().unwrap();
    assert_eq!(run.state, "done");
    assert_eq!(run.error_code, None);
}

#[tokio::test]
async fn a_fire_missed_while_down_runs_once() {
    let f = fx();
    let r = new_routine(&f, input(&f));
    let nine = OCT8 + 9 * H;
    due_at(&f, r.id, nine);
    // Back on Monday 10:00: one run, and the next fire is Tuesday's.
    let monday = OCT8 + 4 * 24 * H + 10 * H;
    tick_once(&f.deps, monday).await;
    tick_once(&f.deps, monday + 20).await;
    assert_eq!(runs_of(&f, r.id).len(), 1);
    assert_eq!(
        routine(&f, r.id).next_run_at,
        Some(OCT8 + 5 * 24 * H + 9 * H)
    );
}

#[tokio::test]
async fn skip_next_records_the_fire_as_skipped_once() {
    let f = fx();
    let r = new_routine(&f, input(&f));
    let ana = person(&f.store, None, f.ana);
    let nine = OCT8 + 9 * H;
    due_at(&f, r.id, nine);
    assert!(skip_next(&f.store, &ana, r.id, true).unwrap().skip_next);
    tick_once(&f.deps, nine).await;
    let runs = runs_of(&f, r.id);
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].state, "skipped");
    assert_eq!(runs[0].reason.as_deref(), Some("a person skipped it"));
    assert!(f.fake.started.lock().unwrap().is_empty());
    assert!(!routine(&f, r.id).skip_next, "a skip is spent");
    // The next one runs.
    let next = routine(&f, r.id).next_run_at.unwrap();
    tick_once(&f.deps, next).await;
    assert_eq!(runs_of(&f, r.id).last().unwrap().state, "running");
}

/// A start path that waits for the test to let it finish, so a second fire
/// can land while the first one's session is being made.
struct GatedSpawn {
    inner: Arc<FakeSpawn>,
    entered: tokio::sync::Notify,
    gate: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl tick::Spawn for GatedSpawn {
    async fn spawn(
        &self,
        args: crate::service::sessions::NewSessionArgs,
    ) -> Result<SessionRow, IpcError> {
        self.entered.notify_one();
        self.gate.notified().await;
        tick::Spawn::spawn(&*self.inner, args).await
    }
}

/// Review r06 F1: a second Run now while the first one's session is still
/// being made sees the first one starting and skips, rather than starting a
/// second session for an `overlap: skip` routine.
#[tokio::test]
async fn a_run_now_during_a_starting_run_is_skipped() {
    let f = fx();
    let r = new_routine(&f, input(&f));
    let ana = person(&f.store, None, f.ana);
    let gated = Arc::new(GatedSpawn {
        inner: f.fake.clone(),
        entered: tokio::sync::Notify::new(),
        gate: tokio::sync::Notify::new(),
    });
    let deps = Deps {
        store: Arc::clone(&f.store),
        spawn: gated.clone(),
    };
    let first = {
        let (deps, ana) = (deps.clone(), ana.clone());
        tokio::spawn(async move { run_now(&deps, &ana, r.id, OCT8).await })
    };
    if tokio::time::timeout(std::time::Duration::from_secs(5), gated.entered.notified())
        .await
        .is_err()
    {
        panic!("the first run never started: {:?}", first.await);
    }
    let second = run_now(&f.deps, &ana, r.id, OCT8 + 1).await.unwrap();
    assert_eq!(second.state, "skipped");
    assert_eq!(
        second.reason.as_deref(),
        Some("its last run is still starting")
    );
    gated.gate.notify_one();
    assert_eq!(first.await.unwrap().unwrap().state, "running");
    assert_eq!(f.fake.started.lock().unwrap().len(), 1, "one session");
    // The claim is gone once the run is recorded; the open run answers now.
    let third = run_now(&f.deps, &ana, r.id, OCT8 + 2).await.unwrap();
    assert_eq!(third.reason.as_deref(), Some("its last run is still going"));
}

/// Review r06 F3: a Skip next a person sets while a scheduled fire is
/// starting its session is kept for the next fire, not cleared by it.
#[tokio::test]
async fn a_skip_next_set_during_a_fire_survives_it() {
    let f = fx();
    let r = new_routine(&f, input(&f));
    let ana = person(&f.store, None, f.ana);
    due_at(&f, r.id, OCT8);
    let gated = Arc::new(GatedSpawn {
        inner: f.fake.clone(),
        entered: tokio::sync::Notify::new(),
        gate: tokio::sync::Notify::new(),
    });
    let deps = Deps {
        store: Arc::clone(&f.store),
        spawn: gated.clone(),
    };
    let pass = tokio::spawn(async move { tick::tick_once(&deps, OCT8).await });
    if tokio::time::timeout(std::time::Duration::from_secs(5), gated.entered.notified())
        .await
        .is_err()
    {
        panic!("the fire never started: {:?}", pass.await);
    }
    assert!(skip_next(&f.store, &ana, r.id, true).unwrap().skip_next);
    gated.gate.notify_one();
    pass.await.unwrap();
    assert_eq!(runs_of(&f, r.id)[0].state, "running");
    assert!(
        routine(&f, r.id).skip_next,
        "the person's skip is still to come"
    );
}

#[tokio::test]
async fn overlap_skip_waits_for_the_open_run_and_parallel_does_not() {
    let f = fx();
    let r = new_routine(&f, input(&f));
    let ana = person(&f.store, None, f.ana);
    let first = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    assert_eq!(first.state, "running");
    assert_eq!(first.trigger, "run_now");
    let second = run_now(&f.deps, &ana, r.id, OCT8 + 60).await.unwrap();
    assert_eq!(second.state, "skipped");
    assert_eq!(
        second.reason.as_deref(),
        Some("its last run is still going")
    );
    // Parallel starts beside it.
    let mut i = input(&f);
    i.overlap = Some("parallel".into());
    save(&f.store, &ana, Some(r.id), &i).unwrap();
    let third = run_now(&f.deps, &ana, r.id, OCT8 + 120).await.unwrap();
    assert_eq!(third.state, "running");
    assert_eq!(f.fake.started.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn a_run_settles_on_its_sessions_turn() {
    let f = fx();
    let r = new_routine(&f, input(&f));
    let ana = person(&f.store, None, f.ana);
    let mut i = input(&f);
    i.overlap = Some("parallel".into());
    save(&f.store, &ana, Some(r.id), &i).unwrap();
    let done = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    let errored = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    let lost = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    let removed = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    let quiet = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    let going = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    typing_done().await;
    spend(&f, done.session_id.unwrap(), 420_000);
    event(&f, done.session_id.unwrap(), "turn_done");
    event(&f, errored.session_id.unwrap(), "stop_failure");
    {
        let s = lock(&f.store).unwrap();
        let lost_name = s
            .get_session_by_id(lost.session_id.unwrap())
            .unwrap()
            .unwrap()
            .tmux_name;
        let keep: Vec<String> = s
            .list_sessions_for_host("mac")
            .unwrap()
            .into_iter()
            .map(|row| row.tmux_name)
            .filter(|n| *n != lost_name)
            .collect();
        s.mark_host_sessions_lost("mac", "gone", &keep, OCT8, OCT8)
            .unwrap();
        s.delete_session(removed.session_id.unwrap()).unwrap();
    }
    let later = OCT8 + 60;
    lock(&f.store)
        .unwrap()
        .set_routine_run_cost(quiet.id, 0)
        .unwrap();
    tick_once(&f.deps, later).await;
    let by_id = |id: i64| {
        lock(&f.store)
            .unwrap()
            .get_routine_run(id)
            .unwrap()
            .unwrap()
    };
    let d = by_id(done.id);
    assert_eq!((d.state.as_str(), d.cost_micros), ("done", 420_000));
    assert_eq!(d.finished_at, Some(later));
    assert_eq!(by_id(errored.id).state, "failed");
    assert_eq!(
        by_id(lost.id).reason.as_deref(),
        Some("its session was lost")
    );
    assert_eq!(by_id(removed.id).state, "failed");
    assert_eq!(by_id(going.id).state, "running");
    // Six hours with no turn.
    tick_once(&f.deps, OCT8 + RUN_STALE_SECS + 1).await;
    assert_eq!(
        by_id(quiet.id).reason.as_deref(),
        Some("no turn finished in 6 hours")
    );
}

use super::tick::RUN_STALE_SECS;

#[tokio::test]
async fn a_run_over_its_budget_fails_and_pauses_the_routine() {
    let f = fx();
    let mut i = input(&f);
    i.budget_run_micros = Some(1_000_000);
    let r = new_routine(&f, i);
    let ana = person(&f.store, None, f.ana);
    let run = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    spend(&f, run.session_id.unwrap(), 400_000);
    tick_once(&f.deps, OCT8 + 20).await;
    let still = lock(&f.store)
        .unwrap()
        .get_routine_run(run.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        (still.state.as_str(), still.cost_micros),
        ("running", 400_000)
    );
    spend(&f, run.session_id.unwrap(), 1_200_000);
    tick_once(&f.deps, OCT8 + 40).await;
    let s = lock(&f.store).unwrap();
    let over = s.get_routine_run(run.id).unwrap().unwrap();
    assert_eq!(over.state, "failed");
    assert!(
        over.reason
            .as_deref()
            .unwrap()
            .contains("$1.20 of the run's $1.00"),
        "{over:?}"
    );
    let r = s.get_routine(r.id).unwrap().unwrap();
    assert!(!r.enabled);
    assert_eq!(r.next_run_at, None);
    assert!(r.paused_reason.unwrap().contains("over its budget"));
}

#[tokio::test]
async fn the_day_budget_stops_new_runs_until_tomorrow() {
    let f = fx();
    let mut i = input(&f);
    i.budget_day_micros = Some(2_000_000);
    let r = new_routine(&f, i);
    let ana = person(&f.store, None, f.ana);
    let first = run_now(&f.deps, &ana, r.id, OCT8 + 8 * H).await.unwrap();
    spend(&f, first.session_id.unwrap(), 2_500_000);
    event(&f, first.session_id.unwrap(), "turn_done");
    tick_once(&f.deps, OCT8 + 9 * H).await;
    let refused = run_now(&f.deps, &ana, r.id, OCT8 + 10 * H).await.unwrap();
    assert_eq!(refused.state, "skipped");
    assert_eq!(
        refused.reason.as_deref(),
        Some("its runs spent $2.50 of today's $2.00")
    );
    // A new day.
    let next = run_now(&f.deps, &ana, r.id, OCT8 + 24 * H + 60)
        .await
        .unwrap();
    assert_eq!(next.state, "running");
}

/// The plan's check for 8.7: the loop skips an account over its threshold,
/// and fires again once the account is back under it.
#[tokio::test]
async fn a_fire_on_an_account_over_the_line_is_skipped() {
    let f = fx();
    let r = new_routine(&f, input(&f));
    {
        let s = lock(&f.store).unwrap();
        crate::service::account_limits::seed_usage(
            &s,
            "mac",
            Some("work"),
            "work",
            93.0,
            OCT8 + 9 * H,
        );
        // The host's own login is not the routine's: its reading is no reason.
        crate::service::account_limits::seed_usage(&s, "mac", None, "own", 10.0, OCT8 + 9 * H);
    }
    due_at(&f, r.id, OCT8 + 9 * H);
    tick_once(&f.deps, OCT8 + 9 * H).await;
    let runs = runs_of(&f, r.id);
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].state, "skipped");
    let why = runs[0].reason.as_deref().unwrap();
    assert!(
        why.contains("profile work on mac") && why.contains("93%"),
        "{why}"
    );
    assert!(f.fake.started.lock().unwrap().is_empty());
    assert!(routine(&f, r.id).next_run_at.unwrap() > OCT8 + 9 * H);
    // A newer reading under the line: the next fire runs.
    {
        let s = lock(&f.store).unwrap();
        crate::service::account_limits::seed_usage(
            &s,
            "mac",
            Some("work"),
            "work",
            20.0,
            OCT8 + 24 * H,
        );
    }
    due_at(&f, r.id, OCT8 + 33 * H);
    tick_once(&f.deps, OCT8 + 33 * H).await;
    assert_eq!(runs_of(&f, r.id)[1].state, "running");
}

/// Review w02-6: a login fleet knows is signed out is treated like one past
/// the line. The fallback takes the run; without one, the fire skips and
/// says how to sign in again, instead of starting a session at `/login`.
#[tokio::test]
async fn a_fire_on_a_signed_out_login_goes_to_the_fallback_or_skips() {
    let f = fx();
    let r = new_routine(&f, input(&f));
    reachable(&f, "mac", true);
    reachable(&f, "nas", true);
    lock(&f.store)
        .unwrap()
        .set_host_signed_out_logins(
            "mac",
            &[crate::store::SignedOutLogin {
                profile: Some("work".into()),
                since: OCT8 - 24 * H,
                checked_at: OCT8,
            }],
        )
        .unwrap();
    due_at(&f, r.id, OCT8 + 9 * H);
    tick_once(&f.deps, OCT8 + 9 * H).await;
    let runs = runs_of(&f, r.id);
    assert_eq!(runs[0].state, "skipped");
    let why = runs[0].reason.as_deref().unwrap();
    assert!(
        why.contains("profile work on mac is signed out") && why.contains("/login"),
        "{why}"
    );
    assert!(f.fake.started.lock().unwrap().is_empty());
    let mut i = input(&f);
    i.fallback_host = Some("nas".into());
    let fb = new_routine(&f, i);
    let ana = person(&f.store, None, f.ana);
    let moved = run_now(&f.deps, &ana, fb.id, OCT8 + 10 * H).await.unwrap();
    assert_eq!(moved.host_alias.as_deref(), Some("nas"));
    assert!(
        moved
            .reason
            .as_deref()
            .unwrap()
            .starts_with("on nas: profile work on mac is signed out"),
        "{moved:?}"
    );
}

/// Redesign 8.7: a routine names the account it runs as.
#[test]
fn a_routine_names_the_account_it_bills() {
    let f = fx();
    let r = new_routine(&f, input(&f));
    let ana = person(&f.store, None, f.ana);
    assert_eq!(
        get(&f.store, &ana, r.id).unwrap().account,
        None,
        "no login yet"
    );
    {
        let s = lock(&f.store).unwrap();
        let now = crate::store::now_unix();
        crate::service::account_limits::seed_usage(&s, "mac", Some("work"), "work", 95.0, now);
    }
    let a = get(&f.store, &ana, r.id).unwrap().account.unwrap();
    assert_eq!(
        (
            a.host_alias.as_str(),
            a.login.profile.as_deref(),
            a.login.account_uuid.as_str()
        ),
        ("mac", Some("work"), "work")
    );
    assert!(a.over);
}

#[tokio::test]
async fn pause_all_stops_the_schedule_but_not_a_person() {
    let f = fx();
    let r = new_routine(&f, input(&f));
    lock(&f.store)
        .unwrap()
        .set_setting(crate::service::settings::AUTOMATION_PAUSED, "true")
        .unwrap();
    let nine = OCT8 + 9 * H;
    due_at(&f, r.id, nine);
    tick_once(&f.deps, nine).await;
    assert!(runs_of(&f, r.id).is_empty());
    assert_eq!(routine(&f, r.id).next_run_at, Some(nine), "the fire waits");
    let ana = person(&f.store, None, f.ana);
    assert_eq!(
        run_now(&f.deps, &ana, r.id, nine).await.unwrap().state,
        "running"
    );
}

#[tokio::test]
async fn a_failed_start_is_a_failed_run() {
    let f = fx();
    let r = new_routine(&f, input(&f));
    f.fake.fail.store(true, Ordering::SeqCst);
    let ana = person(&f.store, None, f.ana);
    let run = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    assert_eq!(run.state, "failed");
    assert_eq!(run.reason.as_deref(), Some("host unreachable"));
    assert_eq!(run.session_id, None);
}

#[tokio::test]
async fn an_event_routine_fires_on_its_owners_sessions_only() {
    let f = fx();
    // History before the routine never fires it.
    let old = {
        let s = lock(&f.store).unwrap();
        let id = s
            .upsert_session("old", "mac", None, None, 1, 1, "running", None)
            .unwrap();
        s.claim_if_unclaimed(id, Some(f.ana)).unwrap();
        id
    };
    event(&f, old, "stuck");
    let mut i = event_input(&f, "stuck");
    i.overlap = Some("parallel".into());
    let r = new_routine(&f, i);
    assert_eq!(r.next_run_at, None);
    tick_once(&f.deps, OCT8).await;
    assert!(runs_of(&f, r.id).is_empty(), "history does not fire");
    let (mine, theirs) = {
        let s = lock(&f.store).unwrap();
        let mine = s
            .upsert_session("mine", "mac", None, None, 1, 1, "running", None)
            .unwrap();
        s.claim_if_unclaimed(mine, Some(f.ana)).unwrap();
        let theirs = s
            .upsert_session("theirs", "mac", None, None, 1, 1, "running", None)
            .unwrap();
        s.claim_if_unclaimed(theirs, Some(f.bo)).unwrap();
        (mine, theirs)
    };
    event(&f, theirs, "stuck");
    event(&f, mine, "turn_done");
    event(&f, mine, "stuck");
    tick_once(&f.deps, OCT8 + 20).await;
    let runs = runs_of(&f, r.id);
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!(runs[0].trigger, "event");
    assert!(runs[0]
        .trigger_ref
        .as_deref()
        .unwrap()
        .starts_with(&format!("session:{mine}:")));
    // The run's own session getting stuck does not fire it again.
    event(&f, runs[0].session_id.unwrap(), "stuck");
    tick_once(&f.deps, OCT8 + 40).await;
    assert_eq!(runs_of(&f, r.id).len(), 1);
    // Nor does the same event twice.
    tick_once(&f.deps, OCT8 + 60).await;
    assert_eq!(runs_of(&f, r.id).len(), 1);
}

/// Review r16: a pass that finds nothing of its kind still moves the cursor
/// past the events it skipped, so a rare kind does not rescan the whole
/// event history on every pass, and a later event of the kind still fires.
#[tokio::test]
async fn an_event_routine_with_nothing_to_fire_moves_its_cursor_on() {
    let f = fx();
    let i = event_input(&f, "stuck");
    let r = new_routine(&f, i);
    let mine = {
        let s = lock(&f.store).unwrap();
        let mine = s
            .upsert_session("mine", "mac", None, None, 1, 1, "running", None)
            .unwrap();
        s.claim_if_unclaimed(mine, Some(f.ana)).unwrap();
        mine
    };
    event(&f, mine, "turn_done");
    event(&f, mine, "turn_done");
    let newest = lock(&f.store).unwrap().latest_session_event_id().unwrap();
    tick_once(&f.deps, OCT8).await;
    assert!(runs_of(&f, r.id).is_empty());
    assert_eq!(routine(&f, r.id).event_cursor, newest);
    event(&f, mine, "stuck");
    tick_once(&f.deps, OCT8 + 20).await;
    assert_eq!(runs_of(&f, r.id).len(), 1);
}

/// A pass reads the event routines before it fires anything; one turned
/// off while earlier fires ran does not fire on that stale read.
#[tokio::test]
async fn an_event_routine_turned_off_during_a_pass_does_not_fire() {
    let f = fx();
    let i = event_input(&f, "stuck");
    let r = new_routine(&f, i);
    let mine = {
        let s = lock(&f.store).unwrap();
        let mine = s
            .upsert_session("mine", "mac", None, None, 1, 1, "running", None)
            .unwrap();
        s.claim_if_unclaimed(mine, Some(f.ana)).unwrap();
        mine
    };
    event(&f, mine, "stuck");
    let read_by_the_pass = routine(&f, r.id);
    set_enabled(&f.store, &person(&f.store, None, f.ana), r.id, false).unwrap();
    tick::tick_event(&f.deps, &read_by_the_pass, OCT8)
        .await
        .unwrap();
    assert!(runs_of(&f, r.id).is_empty());
    // …and gave its lease back.
    assert!(lock(&f.store)
        .unwrap()
        .take_routine_lease(r.id, OCT8)
        .unwrap());
}

#[test]
fn bad_routines_are_refused_with_the_reason() {
    let f = fx();
    let ana = person(&f.store, None, f.ana);
    type Change = Box<dyn Fn(&mut RoutineInput)>;
    let cases: Vec<(Change, &str)> = vec![
        (Box::new(|i| i.name = " ".into()), "name"),
        (Box::new(|i| i.prompt = String::new()), "prompt"),
        (Box::new(|i| i.trigger = "hourly".into()), "trigger"),
        (Box::new(|i| i.cron = None), "needs cron"),
        (Box::new(|i| i.cron = Some("0 25 * * *".into())), "hour"),
        (
            Box::new(|i| i.cron = Some("0 0 31 2 *".into())),
            "never fires",
        ),
        (
            Box::new(|i| i.utc_offset_min = Some(15 * 60)),
            "utc_offset_min",
        ),
        (
            Box::new(|i| {
                i.trigger = "event".into();
                i.event = Some("anything".into())
            }),
            "event must be",
        ),
        (Box::new(|i| i.host_alias = "pi".into()), "host pi"),
        (Box::new(|i| i.project_id = 999), "project 999"),
        (Box::new(|i| i.profile = Some("../x".into())), "profile"),
        (
            Box::new(|i| i.budget_day_micros = Some(0)),
            "budget_day_micros",
        ),
        (Box::new(|i| i.overlap = Some("queue".into())), "overlap"),
    ];
    for (change, says) in cases {
        let mut i = input(&f);
        change(&mut i);
        let e = save(&f.store, &ana, None, &i).unwrap_err();
        assert!(e.message.contains(says), "{says}: {}", e.message);
    }
    assert!(lock(&f.store).unwrap().list_routines().unwrap().is_empty());
}

#[test]
fn another_person_cannot_see_or_change_a_routine() {
    let f = fx();
    let r = new_routine(&f, input(&f));
    let bo = person(&f.store, None, f.bo);
    assert!(list(&f.store, &bo).unwrap().is_empty());
    let unknown = get(&f.store, &bo, 999_999).unwrap_err();
    let hidden = get(&f.store, &bo, r.id).unwrap_err();
    assert_eq!(hidden.code, unknown.code);
    assert_eq!(
        hidden.message.replace(&r.id.to_string(), "N"),
        unknown.message.replace("999999", "N"),
        "a hidden routine answers as an unknown one"
    );
    for e in [
        save(&f.store, &bo, Some(r.id), &input(&f)).unwrap_err(),
        delete(&f.store, &bo, r.id).unwrap_err(),
        set_enabled(&f.store, &bo, r.id, false).unwrap_err(),
        skip_next(&f.store, &bo, r.id, true).unwrap_err(),
        runs(&f.store, &bo, r.id, None).unwrap_err(),
    ] {
        assert_eq!(e.code, codes::E_NOTFOUND);
    }
}

#[tokio::test]
async fn another_person_cannot_run_a_routine() {
    let f = fx();
    let r = new_routine(&f, input(&f));
    let bo = person(&f.store, None, f.bo);
    let e = run_now(&f.deps, &bo, r.id, OCT8).await.unwrap_err();
    assert_eq!(e.code, codes::E_NOTFOUND);
    assert!(f.fake.started.lock().unwrap().is_empty());
}

#[test]
fn an_org_member_reads_and_only_an_admin_changes() {
    let f = fx();
    let org = {
        let s = lock(&f.store).unwrap();
        let org = s.add_org("Acme", None, false).unwrap().id;
        s.set_host_org("mac", Some(org)).unwrap();
        s.set_org_member(org, f.bo, "member", None).unwrap();
        org
    };
    let r = new_routine(&f, input(&f));
    assert_eq!(r.org_id, Some(org), "a routine is its host's org's");
    let bo = person(&f.store, None, f.bo);
    let d = get(&f.store, &bo, r.id).unwrap();
    assert!(!d.may_change);
    assert_eq!(
        set_enabled(&f.store, &bo, r.id, false).unwrap_err().code,
        codes::E_FORBIDDEN
    );
    lock(&f.store)
        .unwrap()
        .set_org_member(org, f.bo, "admin", None)
        .unwrap();
    assert!(get(&f.store, &bo, r.id).unwrap().may_change);
    assert!(!set_enabled(&f.store, &bo, r.id, false).unwrap().enabled);
}

#[test]
fn the_org_boundary_comes_before_the_owner() {
    let f = fx();
    let (acme, other) = {
        let s = lock(&f.store).unwrap();
        let acme = s.add_org("Acme", None, false).unwrap().id;
        let other = s.add_org("Other", None, false).unwrap().id;
        s.set_host_org("mac", Some(acme)).unwrap();
        s.set_host_org("nas", Some(other)).unwrap();
        (acme, other)
    };
    let r = new_routine(&f, input(&f));
    let bound = person(&f.store, Some(other), f.ana);
    assert!(list(&f.store, &bound).unwrap().is_empty());
    assert_eq!(
        get(&f.store, &bound, r.id).unwrap_err().code,
        codes::E_NOTFOUND
    );
    // Nor can it name a host outside its org.
    let e = save(&f.store, &bound, None, &input(&f)).unwrap_err();
    assert_eq!(e.code, codes::E_NOTFOUND);
    // And a routine does not move between orgs.
    let ana = person(&f.store, None, f.ana);
    let mut moved = input(&f);
    moved.host_alias = "nas".into();
    let e = save(&f.store, &ana, Some(r.id), &moved).unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
    assert_eq!(routine(&f, r.id).org_id, Some(acme));
}

#[test]
fn a_caller_without_a_person_owns_no_routine() {
    let f = fx();
    let host = Caller {
        api: None,
        host_alias: Some("mac".into()),
        client: None,
        mode: TokenMode::Full,
        pane: None,
        is_personal_owner: false,
        is_fleet_admin: false,
    }
    .view_scope(&lock(&f.store).unwrap())
    .unwrap();
    assert_eq!(
        save(&f.store, &host, None, &input(&f)).unwrap_err().code,
        codes::E_FORBIDDEN
    );
}

#[test]
fn turning_a_routine_back_on_restarts_its_schedule_and_cursor() {
    let f = fx();
    let r = new_routine(&f, input(&f));
    let ana = person(&f.store, None, f.ana);
    let off = set_enabled(&f.store, &ana, r.id, false).unwrap();
    assert_eq!(off.next_run_at, None);
    let on = set_enabled(&f.store, &ana, r.id, true).unwrap();
    assert!(on.next_run_at.is_some());
    assert!(delete(&f.store, &ana, r.id).unwrap());
    assert!(list(&f.store, &ana).unwrap().is_empty());
    // Skip next is a schedule's.
    let mut manual = input(&f);
    manual.trigger = "manual".into();
    let m = new_routine(&f, manual);
    assert_eq!(
        skip_next(&f.store, &ana, m.id, true).unwrap_err().code,
        codes::E_INVALID_STATE
    );
}

/// A fire that is due but not yet ticked survives an edit that keeps the
/// schedule, and a "turn on" of a routine that is already on.
#[test]
fn a_due_fire_survives_a_save_or_turn_on_that_keeps_the_schedule() {
    let f = fx();
    let r = new_routine(&f, input(&f));
    let ana = person(&f.store, None, f.ana);
    due_at(&f, r.id, 1000);
    assert_eq!(
        set_enabled(&f.store, &ana, r.id, true).unwrap().next_run_at,
        Some(1000)
    );
    let mut edited = input(&f);
    edited.prompt = "Review my open PRs, oldest first.".into();
    let saved = save(&f.store, &ana, Some(r.id), &edited).unwrap();
    assert_eq!(saved.next_run_at, Some(1000));
    // A new schedule starts from now.
    edited.cron = Some("0 10 * * 1-5".into());
    let moved = save(&f.store, &ana, Some(r.id), &edited).unwrap();
    assert!(moved.next_run_at.unwrap() > 1000);
}

// Step 8.10: what a run came to.

fn run_row(f: &Fx, id: i64) -> RoutineRunRow {
    lock(&f.store)
        .unwrap()
        .get_routine_run(id)
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn a_failed_run_is_failed_from_its_exit_and_nothing_overrides_it() {
    let f = fx();
    let r = new_routine(&f, input(&f));
    let ana = person(&f.store, None, f.ana);
    let mut i = input(&f);
    i.overlap = Some("parallel".into());
    save(&f.store, &ana, Some(r.id), &i).unwrap();
    let errored = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    event(&f, errored.session_id.unwrap(), "stop_failure");
    f.fake.fail.store(true, Ordering::SeqCst);
    let unstarted = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    tick_once(&f.deps, OCT8 + 60).await;
    for id in [errored.id, unstarted.id] {
        let run = run_row(&f, id);
        assert_eq!(run.state, "failed");
        assert_eq!(run.outcome.as_deref(), Some("failed"));
        assert_eq!(run.outcome_source.as_deref(), Some("exit"));
        let s = lock(&f.store).unwrap();
        assert!(!outcome::record(
            &s,
            &run,
            outcome::RunOutcome::Nothing,
            outcome::OutcomeSource::Jev,
            OCT8
        )
        .unwrap());
        assert!(!outcome::record(
            &s,
            &run,
            outcome::RunOutcome::DidWork,
            outcome::OutcomeSource::Rule,
            OCT8
        )
        .unwrap());
    }
    assert_eq!(run_row(&f, errored.id).outcome.as_deref(), Some("failed"));
}

#[tokio::test]
async fn a_done_run_no_rule_can_read_waits_for_jev_and_nothing_to_do_stays_quiet() {
    let f = fx();
    let r = new_routine(&f, input(&f));
    let ana = person(&f.store, None, f.ana);
    let run = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    let sid = run.session_id.unwrap();
    event(&f, sid, "turn_done");
    tick_once(&f.deps, OCT8 + 60).await;
    let done = run_row(&f, run.id);
    assert_eq!(done.state, "done");
    assert_eq!(done.outcome, None, "no rule reads a plain finished turn");
    {
        let s = lock(&f.store).unwrap();
        let waiting: Vec<i64> = s
            .routine_runs_without_outcome(10)
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(waiting, vec![run.id]);
        assert_eq!(
            s.get_session_by_id(sid).unwrap().unwrap().last_viewed_at,
            None
        );
        // Jev's answer: nothing to do. The session is marked seen, so its
        // finished turn is not an unread one in the Inbox.
        assert!(outcome::record(
            &s,
            &done,
            outcome::RunOutcome::Nothing,
            outcome::OutcomeSource::Jev,
            OCT8 + 90
        )
        .unwrap());
        assert_eq!(
            s.get_session_by_id(sid).unwrap().unwrap().last_viewed_at,
            Some(OCT8 + 90)
        );
        assert!(s.routine_runs_without_outcome(10).unwrap().is_empty());
        // A rule that learns more later still wins over Jev.
        assert!(outcome::record(
            &s,
            &done,
            outcome::RunOutcome::DidWork,
            outcome::OutcomeSource::Rule,
            OCT8 + 95
        )
        .unwrap());
        assert!(!outcome::record(
            &s,
            &done,
            outcome::RunOutcome::Nothing,
            outcome::OutcomeSource::Jev,
            OCT8 + 99
        )
        .unwrap());
    }
    let after = run_row(&f, run.id);
    assert_eq!(after.outcome.as_deref(), Some("did_work"));
    assert_eq!(after.outcome_source.as_deref(), Some("rule"));
}

#[tokio::test]
async fn the_rules_read_an_open_question_and_a_pull_request() {
    let f = fx();
    let r = new_routine(&f, input(&f));
    let ana = person(&f.store, None, f.ana);
    let run = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    let mut done = run.clone();
    done.state = "done".into();
    let row = lock(&f.store)
        .unwrap()
        .get_session_by_id(run.session_id.unwrap())
        .unwrap()
        .unwrap();
    use outcome::{rule_outcome, OutcomeSource::*, RunOutcome::*};
    assert_eq!(
        rule_outcome(&run, Some(&row)),
        None,
        "a running run has no outcome"
    );
    assert_eq!(rule_outcome(&done, Some(&row)), None);
    assert_eq!(
        rule_outcome(&done, None),
        None,
        "a vanished session is the exit's to say"
    );

    let mut asked = row.clone();
    asked.claude_status = Some("blocked".into());
    assert_eq!(rule_outcome(&done, Some(&asked)), Some((NeedsPerson, Rule)));
    let mut j2 = row.clone();
    j2.turn_outcome = Some("asked".into());
    // J2 is Jev's answer: the run is never labelled as a rule's.
    assert_eq!(rule_outcome(&done, Some(&j2)), Some((NeedsPerson, Jev)));
    j2.turn_outcome = Some("finished".into());
    assert_eq!(
        rule_outcome(&done, Some(&j2)),
        None,
        "finished is Jev's to split"
    );

    let mut pr = row.clone();
    pr.pr_url = Some("https://github.com/acme/web/pull/7".into());
    assert_eq!(rule_outcome(&done, Some(&pr)), Some((DidWork, Rule)));
    // A question beats a PR: the person is wanted either way.
    pr.claude_status = Some("blocked".into());
    assert_eq!(rule_outcome(&done, Some(&pr)), Some((NeedsPerson, Rule)));

    let mut failed = done.clone();
    failed.state = "failed".into();
    assert_eq!(rule_outcome(&failed, Some(&pr)), Some((Failed, Exit)));
    let mut skipped = done;
    skipped.state = "skipped".into();
    assert_eq!(rule_outcome(&skipped, None), None);
}

#[test]
fn an_outcome_outside_the_list_is_refused() {
    let f = fx();
    let s = lock(&f.store).unwrap();
    assert_eq!(
        s.set_routine_run_outcome(1, "busy", "jev")
            .unwrap_err()
            .code,
        codes::E_INVALID
    );
    assert_eq!(
        s.set_routine_run_outcome(1, "nothing", "llm")
            .unwrap_err()
            .code,
        codes::E_INVALID
    );
}

#[tokio::test]
async fn a_failed_run_is_in_the_inbox_until_retried_or_paused() {
    let f = fx();
    let r = new_routine(&f, input(&f));
    let ana = person(&f.store, None, f.ana);
    let bo = person(&f.store, None, f.bo);
    assert!(failing(&f.store, &ana).unwrap().is_empty());

    f.fake.fail.store(true, Ordering::SeqCst);
    let run = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    let inbox = failing(&f.store, &ana).unwrap();
    assert_eq!(inbox.len(), 1);
    assert_eq!((inbox[0].routine.id, inbox[0].run.id), (r.id, run.id));
    assert!(inbox[0].may_change);
    // Another person's routine is not their Inbox's.
    assert!(failing(&f.store, &bo).unwrap().is_empty());

    // Retry: a newer run that starts takes it out.
    f.fake.fail.store(false, Ordering::SeqCst);
    run_now(&f.deps, &ana, r.id, OCT8 + 60).await.unwrap();
    assert!(failing(&f.store, &ana).unwrap().is_empty());

    // Pause: a person switching it off after a failure takes it out.
    let other = new_routine(&f, input(&f));
    f.fake.fail.store(true, Ordering::SeqCst);
    run_now(&f.deps, &ana, other.id, OCT8).await.unwrap();
    assert_eq!(failing(&f.store, &ana).unwrap().len(), 1);
    set_enabled(&f.store, &ana, other.id, false).unwrap();
    assert!(failing(&f.store, &ana).unwrap().is_empty());
}

#[tokio::test]
async fn a_routine_the_scheduler_paused_over_budget_stays_in_the_inbox() {
    let f = fx();
    let mut i = input(&f);
    i.budget_run_micros = Some(1_000_000);
    let r = new_routine(&f, i);
    let ana = person(&f.store, None, f.ana);
    let run = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    spend(&f, run.session_id.unwrap(), 1_200_000);
    tick_once(&f.deps, OCT8 + 40).await;
    assert!(!routine(&f, r.id).enabled);
    let inbox = failing(&f.store, &ana).unwrap();
    assert_eq!(inbox.len(), 1);
    assert!(inbox[0].routine.paused_reason.is_some());
}

// ---- Pull request triggers (M15 step G2.4) --------------------------------

const PR: &str = "https://github.com/acme/web/pull/42";

/// A session of `owner` on mac, as reconcile would have recorded it.
fn pr_session(f: &Fx, name: &str, owner: i64) -> i64 {
    let s = lock(&f.store).unwrap();
    let id = s
        .upsert_session(name, "mac", Some(f.project), None, 1, 1, "running", None)
        .unwrap();
    s.claim_if_unclaimed(id, Some(owner)).unwrap();
    id
}

/// One reconcile pass seeing `url` on session `sid`: the `pull_requests`
/// upsert reconcile runs, with gh's state, review decision and checks.
fn see_pr(f: &Fx, sid: i64, url: &str, state: &str, review: Option<&str>, ci: Option<&str>) {
    let s = lock(&f.store).unwrap();
    let ev = crate::service::outcome::PrEvidence {
        state: Some(state.into()),
        review_decision: review.map(str::to_string),
        title: Some("Fix login".into()),
        ..Default::default()
    };
    crate::store::Store::upsert_pull_request_in_tx(
        s.conn_ref(),
        url,
        ci,
        Some(&ev),
        crate::store::PrSeenBy {
            session_id: sid,
            session_name: "api",
            host_alias: "mac",
            project_id: Some(f.project),
        },
        OCT8,
    )
    .unwrap();
}

/// `input` turned into an event routine on `event`.
fn event_input(f: &Fx, event: &str) -> RoutineInput {
    let mut i = input(f);
    i.trigger = "event".into();
    i.cron = None;
    i.event = Some(event.into());
    i
}

fn pr_routine(f: &Fx, event: &str, change: impl FnOnce(&mut RoutineInput)) -> RoutineRow {
    let mut i = event_input(f, event);
    i.overlap = Some("parallel".into());
    change(&mut i);
    new_routine(f, i)
}

/// The prompt a run queued for its session, typed or not.
fn queued_prompt(f: &Fx, sid: i64) -> String {
    let s = lock(&f.store).unwrap();
    s.conn_ref()
        .query_row(
            "SELECT body FROM work_journal WHERE kind = 'handover' AND participant_id IN \
               (SELECT id FROM participants WHERE session_id = ?1) ORDER BY id LIMIT 1",
            [sid],
            |r| r.get::<_, Option<String>>(0),
        )
        .unwrap()
        .unwrap_or_default()
}

#[test]
fn a_pull_request_change_writes_review_ci_and_merge_events() {
    let f = fx();
    let sid = pr_session(&f, "api", f.ana);
    let kinds = |f: &Fx| -> Vec<String> {
        let s = lock(&f.store).unwrap();
        s.session_events_after(sid, 0, 50)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind.starts_with("pr_"))
            .map(|e| {
                assert_eq!(e.detail.as_deref(), Some(PR), "the detail is the PR's URL");
                e.kind
            })
            .collect()
    };
    see_pr(
        &f,
        sid,
        PR,
        "OPEN",
        Some("REVIEW_REQUIRED"),
        Some("pending"),
    );
    assert_eq!(
        kinds(&f),
        ["pr_opened"],
        "an open PR seen for the first time is opened, and nothing else"
    );
    see_pr(
        &f,
        sid,
        PR,
        "OPEN",
        Some("REVIEW_REQUIRED"),
        Some("failing"),
    );
    see_pr(
        &f,
        sid,
        PR,
        "OPEN",
        Some("REVIEW_REQUIRED"),
        Some("failing"),
    );
    see_pr(
        &f,
        sid,
        PR,
        "OPEN",
        Some("CHANGES_REQUESTED"),
        Some("passing"),
    );
    see_pr(
        &f,
        sid,
        PR,
        "MERGED",
        Some("CHANGES_REQUESTED"),
        Some("passing"),
    );
    assert_eq!(
        kinds(&f),
        [
            "pr_opened",
            "pr_ci_failed",
            "pr_review",
            "pr_ci_passed",
            "pr_merged"
        ],
        "one event per change, none for the same reading twice"
    );
    // A PR first seen merged is history.
    see_pr(
        &f,
        sid,
        "https://github.com/acme/web/pull/7",
        "MERGED",
        None,
        Some("failing"),
    );
    assert_eq!(kinds(&f).len(), 5);
}

#[tokio::test]
async fn a_routine_fires_when_its_owners_pr_gets_a_review() {
    let f = fx();
    let r = pr_routine(&f, "pr_review", |_| {});
    let mine = pr_session(&f, "mine", f.ana);
    let theirs = pr_session(&f, "theirs", f.bo);
    see_pr(&f, mine, PR, "OPEN", None, None);
    see_pr(
        &f,
        theirs,
        "https://github.com/acme/web/pull/43",
        "OPEN",
        None,
        None,
    );
    tick_once(&f.deps, OCT8).await;
    assert!(runs_of(&f, r.id).is_empty());
    see_pr(
        &f,
        theirs,
        "https://github.com/acme/web/pull/43",
        "OPEN",
        Some("APPROVED"),
        None,
    );
    see_pr(&f, mine, PR, "OPEN", Some("CHANGES_REQUESTED"), None);
    tick_once(&f.deps, OCT8 + 20).await;
    let runs = runs_of(&f, r.id);
    assert_eq!(runs.len(), 1, "only the owner's PR: {runs:?}");
    assert!(runs[0].trigger_ref.as_deref().unwrap().starts_with("pr:"));
    let prompt = queued_prompt(&f, runs[0].session_id.unwrap());
    assert!(prompt.starts_with("Review my open PRs"), "{prompt}");
    assert!(
        prompt.ends_with(&format!(
            "Started because pull request {PR} got a review asking for changes."
        )),
        "{prompt}"
    );
}

#[tokio::test]
async fn a_repo_filter_and_anyone_pick_the_prs_that_fire() {
    let f = fx();
    let org = {
        let s = lock(&f.store).unwrap();
        let org = s.add_org("Acme", None, false).unwrap().id;
        s.set_host_org("mac", Some(org)).unwrap();
        s.set_org_member(org, f.ana, "member", None).unwrap();
        s.set_org_member(org, f.bo, "member", None).unwrap();
        org
    };
    let r = pr_routine(&f, "pr_ci_failed", |i| {
        i.event_repo = Some(" WEB ".into());
        i.event_author = Some("anyone".into());
    });
    assert_eq!(r.org_id, Some(org));
    assert_eq!(r.event_repo.as_deref(), Some("WEB"));
    let theirs = pr_session(&f, "theirs", f.bo);
    // Another repo of the same owner does not pass the filter.
    see_pr(
        &f,
        theirs,
        "https://github.com/acme/api/pull/1",
        "OPEN",
        None,
        Some("failing"),
    );
    // Bo's PR in acme/web does, with anyone.
    see_pr(&f, theirs, PR, "OPEN", None, Some("failing"));
    tick_once(&f.deps, OCT8).await;
    let runs = runs_of(&f, r.id);
    assert_eq!(runs.len(), 1, "{runs:?}");
    let pr_id = lock(&f.store)
        .unwrap()
        .pull_request_by_url(PR)
        .unwrap()
        .unwrap()
        .id;
    assert!(runs[0]
        .trigger_ref
        .as_deref()
        .unwrap()
        .starts_with(&format!("pr:{pr_id}:")));
    // A PR a routine's own session opened fires nothing, anyone or not.
    let run_session = runs[0].session_id.unwrap();
    see_pr(
        &f,
        run_session,
        "https://github.com/acme/web/pull/44",
        "OPEN",
        None,
        Some("failing"),
    );
    tick_once(&f.deps, OCT8 + 20).await;
    assert_eq!(runs_of(&f, r.id).len(), 1);
}

#[tokio::test]
async fn the_rate_holds_one_run_per_pr_in_its_window() {
    let f = fx();
    let r = pr_routine(&f, "pr_ci_failed", |i| i.event_rate_secs = Some(H));
    let mine = pr_session(&f, "mine", f.ana);
    let flap = |f: &Fx, url: &str| {
        see_pr(f, mine, url, "OPEN", None, Some("pending"));
        see_pr(f, mine, url, "OPEN", None, Some("failing"));
    };
    // Two failures of one PR in one pass: one run.
    flap(&f, PR);
    flap(&f, PR);
    tick_once(&f.deps, OCT8).await;
    assert_eq!(runs_of(&f, r.id).len(), 1);
    // Inside the hour: dropped, not recorded. Another PR still fires.
    flap(&f, PR);
    flap(&f, "https://github.com/acme/web/pull/43");
    tick_once(&f.deps, OCT8 + 30 * 60).await;
    assert_eq!(runs_of(&f, r.id).len(), 2);
    // Past the hour, the first PR fires again.
    flap(&f, PR);
    tick_once(&f.deps, OCT8 + H + 1).await;
    let runs = runs_of(&f, r.id);
    assert_eq!(runs.len(), 3);
    assert!(runs.iter().all(|r| r.state != "skipped"), "{runs:?}");
}

#[test]
fn pull_request_filters_are_checked() {
    let f = fx();
    let ana = person(&f.store, None, f.ana);
    type Change = Box<dyn Fn(&mut RoutineInput)>;
    let event = |i: &mut RoutineInput, e: &str| {
        i.trigger = "event".into();
        i.event = Some(e.into());
    };
    let cases: Vec<(Change, &str)> = vec![
        (
            Box::new(move |i| {
                event(i, "stuck");
                i.event_repo = Some("acme/web".into());
            }),
            "repo filter is for a pull request event",
        ),
        (
            Box::new(move |i| {
                event(i, "stuck");
                i.event_author = Some("anyone".into());
            }),
            "its owner's",
        ),
        (
            Box::new(move |i| {
                event(i, "pr_merged");
                i.event_repo = Some("a/b/c".into());
            }),
            "owner/name or name",
        ),
        (
            Box::new(move |i| {
                event(i, "pr_merged");
                i.event_author = Some("bo".into());
            }),
            "event_author must be",
        ),
        (
            Box::new(move |i| {
                event(i, "pr_merged");
                i.event_rate_secs = Some(0);
            }),
            "event_rate_secs",
        ),
    ];
    for (change, says) in cases {
        let mut i = input(&f);
        change(&mut i);
        let e = save(&f.store, &ana, None, &i).unwrap_err();
        assert_eq!(e.code, codes::E_INVALID);
        assert!(e.message.contains(says), "{says}: {}", e.message);
    }
    // A cron routine drops the event filters it was sent.
    let mut i = input(&f);
    i.event_repo = Some("acme/web".into());
    i.event_rate_secs = Some(60);
    let r = save(&f.store, &ana, None, &i).unwrap();
    assert_eq!((r.event_repo, r.event_rate_secs), (None, None));
    // `me` is stored as absent.
    let mut i = input(&f);
    event(&mut i, "pr_merged");
    i.event_author = Some("me".into());
    assert_eq!(save(&f.store, &ana, None, &i).unwrap().event_author, None);
}

#[test]
fn a_repo_filter_matches_owner_and_name_or_name() {
    assert!(repo_matches("acme/web", Some("Acme/Web")));
    assert!(repo_matches("web", Some("acme/web")));
    assert!(!repo_matches("acme/web", Some("other/web")));
    assert!(!repo_matches("web", Some("acme/webapp")));
    assert!(!repo_matches("web", None));
}

/// Gap plan G2.3: the editor's next runs across a daylight-saving change.
/// Europe/Bratislava leaves CEST (+02:00) for CET (+01:00) on Sunday 25
/// October 2026. A weekday 08:30 line saved at +120 keeps that offset, so
/// past the change it fires at 06:30 UTC, 07:30 on the wall: the preview
/// lists the instants the scheduler will use, and the editor shows them in
/// the device's zone, the moved hour included.
#[test]
fn preview_lists_the_next_runs_the_scheduler_uses_across_a_dst_change() {
    let f = fx();
    let ana = person(&f.store, None, f.ana);
    let fri_oct23_noon = OCT8 + 15 * 86_400 + 12 * H;
    let line = RoutineInput {
        cron: Some("30 8 * * 1-5".into()),
        utc_offset_min: Some(120),
        ..input(&f)
    };
    let p = preview(&f.store, &ana, None, &line, fri_oct23_noon).unwrap();
    let mon_oct26 = OCT8 + 18 * 86_400;
    assert_eq!(
        p.next_runs,
        (0..5)
            .map(|d| mon_oct26 + d * 86_400 + 6 * H + 30 * 60)
            .collect::<Vec<_>>(),
        "Mon–Fri at 08:30 +02:00, which is 07:30 once CET begins"
    );
    assert_eq!(p.utc_offset_min, 120);
    // Saved again after the change, at +60, it is back on 08:30 local.
    let after = RoutineInput {
        utc_offset_min: Some(60),
        ..line.clone()
    };
    let p = preview(&f.store, &ana, None, &after, fri_oct23_noon).unwrap();
    assert_eq!(p.next_runs[0], mon_oct26 + 7 * H + 30 * 60);
    // And it is the very fire save schedules.
    let saved = save(&f.store, &ana, None, &line).unwrap();
    let now = crate::store::now_unix();
    assert_eq!(
        saved.next_run_at,
        preview(&f.store, &ana, None, &line, now)
            .unwrap()
            .next_runs
            .first()
            .copied()
    );
}

/// Gap plan G2.3: a dry run writes nothing, says what save would refuse,
/// and names the account the chosen login bills among the host's logins.
#[test]
fn preview_writes_nothing_and_says_what_save_would_refuse() {
    let f = fx();
    let ana = person(&f.store, None, f.ana);
    {
        let s = lock(&f.store).unwrap();
        crate::service::account_limits::seed_usage(&s, "mac", None, "own", 10.0, OCT8);
        crate::service::account_limits::seed_usage(&s, "mac", Some("work"), "work", 95.0, OCT8);
    }
    let blank = RoutineInput {
        prompt: "  ".into(),
        ..input(&f)
    };
    let p = preview(&f.store, &ana, None, &blank, OCT8).unwrap();
    assert!(
        p.problem.as_deref().is_some_and(|m| m.contains("prompt")),
        "{p:?}"
    );
    assert!(list(&f.store, &ana).unwrap().is_empty(), "nothing saved");
    assert_eq!(
        p.logins
            .iter()
            .map(|l| l.login.profile.as_deref())
            .collect::<Vec<_>>(),
        vec![None, Some("work")],
        "the host's own login first"
    );
    let a = p.account.expect("profile work is on a known account");
    assert_eq!(a.login.account_uuid, "work");
    let p = preview(&f.store, &ana, None, &input(&f), OCT8).unwrap();
    assert_eq!(p.problem, None);
    assert_eq!(p.next_runs.len(), PREVIEW_RUNS);
    // An event routine has no next runs; an unknown host has no logins.
    let ev = RoutineInput {
        trigger: "event".into(),
        event: Some("stuck".into()),
        host_alias: "nowhere".into(),
        ..input(&f)
    };
    let p = preview(&f.store, &ana, None, &ev, OCT8).unwrap();
    assert!(p.next_runs.is_empty() && p.logins.is_empty());
    assert!(p.problem.unwrap().contains("nowhere"));
}

/// A preview of a change is fenced like the change: another person's
/// routine is unknown to them.
#[test]
fn preview_of_another_persons_routine_is_not_found() {
    let f = fx();
    let r = new_routine(&f, input(&f));
    let bo = person(&f.store, None, f.bo);
    let e = preview(&f.store, &bo, Some(r.id), &input(&f), OCT8).unwrap_err();
    assert_eq!(e.code, codes::E_NOTFOUND);
}

// ---- automation guards (M15 step G3.8) ---------------------------------------

/// The plan's check: the fleet's daily budget stops every routine's new
/// runs, Run now included, until the next UTC day.
#[tokio::test]
async fn the_fleet_budget_stops_every_routines_runs_until_tomorrow() {
    let f = fx();
    let a = new_routine(&f, input(&f));
    let mut other = input(&f);
    other.name = "Evening sweep".into();
    let b = new_routine(&f, other);
    lock(&f.store)
        .unwrap()
        .set_setting(crate::service::settings::AUTOMATION_DAILY_BUDGET, "3")
        .unwrap();
    let ana = person(&f.store, None, f.ana);
    let first = run_now(&f.deps, &ana, a.id, OCT8 + 8 * H).await.unwrap();
    spend(&f, first.session_id.unwrap(), 3_100_000);
    event(&f, first.session_id.unwrap(), "turn_done");
    tick_once(&f.deps, OCT8 + 8 * H + 20).await;
    let b_now = budget(&f.store, OCT8 + 9 * H).unwrap();
    assert_eq!(
        b_now,
        FleetBudget {
            spent_micros: 3_100_000,
            budget_micros: Some(3_000_000),
            since: OCT8,
        }
    );
    // The other routine's schedule and a person's Run now both stop.
    due_at(&f, b.id, OCT8 + 9 * H);
    tick_once(&f.deps, OCT8 + 9 * H).await;
    let skipped = runs_of(&f, b.id);
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].state, "skipped");
    assert_eq!(
        skipped[0].reason.as_deref(),
        Some("the fleet's routines spent $3.10 of today's $3.00 (automation.daily_budget)")
    );
    let refused = run_now(&f.deps, &ana, a.id, OCT8 + 10 * H).await.unwrap();
    assert_eq!(refused.state, "skipped");
    // A new UTC day.
    let next = run_now(&f.deps, &ana, a.id, OCT8 + 24 * H + 60)
        .await
        .unwrap();
    assert_eq!(next.state, "running");
    // No budget (0) stops nothing.
    lock(&f.store)
        .unwrap()
        .set_setting(crate::service::settings::AUTOMATION_DAILY_BUDGET, "0")
        .unwrap();
    assert_eq!(budget(&f.store, OCT8).unwrap().budget_micros, None);
}

/// The plan's check: Pause all still stops all, a retry included; the time
/// cap's stop still runs, since it only brakes.
#[tokio::test]
async fn pause_all_still_stops_retries_and_fires() {
    let f = fx();
    let mut i = input(&f);
    i.retry_once = Some(true);
    i.run_max_secs = Some(600);
    let r = new_routine(&f, i);
    let ana = person(&f.store, None, f.ana);
    f.fake.fail.store(true, Ordering::SeqCst);
    let failed = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    assert_eq!(failed.state, "failed");
    f.fake.fail.store(false, Ordering::SeqCst);
    lock(&f.store)
        .unwrap()
        .set_setting(crate::service::settings::AUTOMATION_PAUSED, "true")
        .unwrap();
    due_at(&f, r.id, OCT8 + 20);
    tick_once(&f.deps, OCT8 + 20).await;
    assert_eq!(
        runs_of(&f, r.id).len(),
        1,
        "no retry and no fire while paused"
    );
    // A person's run goes on, and its time cap still stops it.
    lock(&f.store)
        .unwrap()
        .set_setting(crate::service::settings::AUTOMATION_PAUSED, "false")
        .unwrap();
    tick_once(&f.deps, OCT8 + 40).await;
    let runs = runs_of(&f, r.id);
    assert_retry_of(&runs[1], failed.id);
    lock(&f.store)
        .unwrap()
        .set_setting(crate::service::settings::AUTOMATION_PAUSED, "true")
        .unwrap();
    tick_once(&f.deps, OCT8 + 40 + 601).await;
    let capped = run_row(&f, runs[1].id);
    assert_eq!(capped.state, "failed");
    assert_eq!(capped.error_code.as_deref(), Some(fix::E_RUN_TIME_CAP));
    assert_eq!(f.fake.stopped.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_failed_run_is_retried_once_and_never_again() {
    let f = fx();
    let r = retrying_routine(&f);
    let ana = person(&f.store, None, f.ana);
    f.fake.fail.store(true, Ordering::SeqCst);
    let first = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    assert_eq!(first.error_code.as_deref(), Some(codes::E_SSH));
    tick_once(&f.deps, OCT8 + 20).await;
    tick_once(&f.deps, OCT8 + 40).await;
    tick_once(&f.deps, OCT8 + 60).await;
    let runs = runs_of(&f, r.id);
    assert_eq!(runs.len(), 2, "one retry, which failed too: {runs:?}");
    assert_eq!(runs[1].trigger, "run_now");
    assert_retry_of(&runs[1], first.id);
    assert_eq!(runs[1].state, "failed");
    // Without retry_once a failure stays as it is.
    let plain = new_routine(&f, input(&f));
    run_now(&f.deps, &ana, plain.id, OCT8 + 100).await.unwrap();
    tick_once(&f.deps, OCT8 + 120).await;
    assert_eq!(runs_of(&f, plain.id).len(), 1);
    // A failure older than the retry window is not started again.
    let late = retrying_routine(&f);
    run_now(&f.deps, &ana, late.id, OCT8).await.unwrap();
    tick_once(&f.deps, OCT8 + tick::RETRY_WITHIN_SECS + 60).await;
    assert_eq!(runs_of(&f, late.id).len(), 1);
}

/// A run that ended at a rate limit waits for the limit to lift (F16); a
/// limit that lasts past the plain one-hour window must not age the retry
/// out, so it is still retried once the window allows.
#[tokio::test]
async fn a_rate_limited_run_is_still_retried_after_the_hour() {
    let f = fx();
    let r = retrying_routine(&f);
    let ana = person(&f.store, None, f.ana);
    f.fake.fail.store(true, Ordering::SeqCst);
    let first = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    lock(&f.store)
        .unwrap()
        .set_routine_run_error_code(first.id, super::fix::E_TURN_RATE_LIMIT)
        .unwrap();
    f.fake.fail.store(false, Ordering::SeqCst);
    tick_once(&f.deps, OCT8 + 3 * 3600).await;
    let runs = runs_of(&f, r.id);
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert_retry_of(&runs[1], first.id);
}

#[tokio::test]
async fn a_run_past_its_time_cap_is_stopped_and_failed() {
    let f = fx();
    let mut i = input(&f);
    i.run_max_secs = Some(20 * 60);
    let r = new_routine(&f, i);
    let ana = person(&f.store, None, f.ana);
    let run = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    tick_once(&f.deps, OCT8 + 19 * 60).await;
    assert_eq!(run_row(&f, run.id).state, "running");
    tick_once(&f.deps, OCT8 + 21 * 60).await;
    let capped = run_row(&f, run.id);
    assert_eq!(capped.state, "failed");
    assert_eq!(
        capped.reason.as_deref(),
        Some("it ran past its 20 min time cap; its turn was stopped")
    );
    let name = lock(&f.store)
        .unwrap()
        .get_session_by_id(run.session_id.unwrap())
        .unwrap()
        .unwrap()
        .tmux_name;
    assert_eq!(
        *f.fake.stopped.lock().unwrap(),
        vec![("mac".to_string(), name)]
    );
    let d = get(&f.store, &ana, r.id).unwrap();
    assert_eq!(d.fixes.len(), 1);
    assert_eq!(
        (d.fixes[0].label.as_str(), d.fixes[0].action.as_str()),
        ("Raise its time cap", "edit")
    );
}

fn reachable(f: &Fx, host: &str, up: bool) {
    lock(&f.store)
        .unwrap()
        .update_host_probe(host, up, None, None, OCT8)
        .unwrap();
}

#[tokio::test]
async fn a_fallback_host_takes_a_run_its_host_cannot() {
    let f = fx();
    let mut i = input(&f);
    i.fallback_host = Some("nas".into());
    i.overlap = Some("parallel".into());
    let r = new_routine(&f, i);
    let ana = person(&f.store, None, f.ana);
    reachable(&f, "mac", true);
    reachable(&f, "nas", true);
    // Its own host, while it can.
    let own = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    assert_eq!(own.host_alias.as_deref(), Some("mac"));
    assert_eq!(own.reason, None);
    // Unreachable: the fallback, saying why.
    reachable(&f, "mac", false);
    let moved = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    assert_eq!(moved.state, "running");
    assert_eq!(moved.host_alias.as_deref(), Some("nas"));
    assert_eq!(moved.reason.as_deref(), Some("on nas: mac was unreachable"));
    // Its login past the line: the fallback.
    reachable(&f, "mac", true);
    {
        let s = lock(&f.store).unwrap();
        crate::service::account_limits::seed_usage(&s, "mac", Some("work"), "work", 95.0, OCT8);
    }
    let over = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    assert_eq!(over.host_alias.as_deref(), Some("nas"));
    assert!(
        over.reason.as_deref().unwrap().starts_with("on nas: "),
        "{over:?}"
    );
    {
        let s = lock(&f.store).unwrap();
        crate::service::account_limits::seed_usage(&s, "mac", Some("work"), "work", 10.0, OCT8 + 1);
    }
    // A start on its host that fails: the fallback, once.
    *f.fake.fail_on.lock().unwrap() = Some("mac".into());
    let retried = run_now(&f.deps, &ana, r.id, OCT8 + 2).await.unwrap();
    assert_eq!(retried.state, "running");
    assert_eq!(retried.host_alias.as_deref(), Some("nas"));
    assert!(
        retried
            .reason
            .as_deref()
            .unwrap()
            .contains("the start on mac failed"),
        "{retried:?}"
    );
    let hosts: Vec<String> = f
        .fake
        .started
        .lock()
        .unwrap()
        .iter()
        .map(|s| s.host.clone())
        .collect();
    assert_eq!(hosts, ["mac", "nas", "nas", "nas"]);
    // Both down: no fallback can take it, the run fails on its own host.
    *f.fake.fail_on.lock().unwrap() = None;
    f.fake.fail.store(true, Ordering::SeqCst);
    let failed = run_now(&f.deps, &ana, r.id, OCT8 + 3).await.unwrap();
    assert_eq!(failed.state, "failed");
    assert_eq!(failed.host_alias.as_deref(), Some("nas"));
}

#[tokio::test]
async fn autonomy_adds_its_line_to_the_runs_prompt() {
    let f = fx();
    let ana = person(&f.store, None, f.ana);
    for (level, line) in [
        (Some(0), Some("Autonomy L0, report only")),
        (Some(1), Some("Autonomy L1, ask before push")),
        (Some(2), None),
        (None, None),
    ] {
        let mut i = input(&f);
        i.autonomy = level;
        let r = new_routine(&f, i);
        assert_eq!(r.autonomy, level.filter(|l| *l < 2), "2 is stored as none");
        let run = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
        let prompt = queued_prompt(&f, run.session_id.unwrap());
        assert!(prompt.starts_with("Review my open PRs"), "{prompt}");
        match line {
            Some(l) => assert!(prompt.contains(l), "{prompt}"),
            None => assert!(!prompt.contains("Autonomy"), "{prompt}"),
        }
    }
}

#[test]
fn guards_are_checked() {
    let f = fx();
    let ana = person(&f.store, None, f.ana);
    lock(&f.store)
        .unwrap()
        .insert_host("far", Some("far"))
        .unwrap();
    let org = lock(&f.store)
        .unwrap()
        .add_org("Acme", None, false)
        .unwrap()
        .id;
    lock(&f.store)
        .unwrap()
        .set_host_org("far", Some(org))
        .unwrap();
    let refused = |change: &dyn Fn(&mut RoutineInput), says: &str| {
        let mut i = input(&f);
        change(&mut i);
        let e = save(&f.store, &ana, None, &i).unwrap_err();
        assert!(e.message.contains(says), "{says}: {}", e.message);
    };
    refused(
        &|i| i.time_zone = Some("Europe/Bratislava; rm".into()),
        "time_zone",
    );
    refused(&|i| i.run_max_secs = Some(30), "run_max_secs");
    refused(
        &|i| i.run_max_secs = Some(RUN_STALE_SECS + 1),
        "run_max_secs",
    );
    refused(&|i| i.fallback_host = Some("mac".into()), "another host");
    refused(&|i| i.fallback_host = Some("nowhere".into()), "not found");
    refused(
        &|i| i.fallback_host = Some("far".into()),
        "another organisation",
    );
    refused(&|i| i.autonomy = Some(3), "autonomy");
    let mut ok = input(&f);
    ok.time_zone = Some("Europe/Bratislava".into());
    ok.run_max_secs = Some(1200);
    ok.fallback_host = Some(" nas ".into());
    ok.retry_once = Some(true);
    ok.autonomy = Some(1);
    let r = save(&f.store, &ana, None, &ok).unwrap();
    assert_eq!(
        (
            r.time_zone.as_deref(),
            r.run_max_secs,
            r.fallback_host.as_deref(),
            r.retry_once,
            r.autonomy
        ),
        (
            Some("Europe/Bratislava"),
            Some(1200),
            Some("nas"),
            true,
            Some(1)
        )
    );
}

#[tokio::test]
async fn a_failed_run_names_its_fix() {
    let f = fx();
    let ana = person(&f.store, None, f.ana);
    let mut i = input(&f);
    i.overlap = Some("parallel".into());
    let r = new_routine(&f, i);
    let auth = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    lock(&f.store)
        .unwrap()
        .insert_session_event(
            auth.session_id.unwrap(),
            "stop_failure",
            Some("auth (authentication_error): HTTP 401"),
        )
        .unwrap();
    tick_once(&f.deps, OCT8 + 20).await;
    let run = run_row(&f, auth.id);
    assert_eq!(run.error_code.as_deref(), Some(fix::E_TURN_AUTH));
    assert_eq!(
        run.reason.as_deref(),
        Some("its turn ended in an error: auth (authentication_error): HTTP 401")
    );
    let failing = failing(&f.store, &ana).unwrap();
    let fx_ = failing[0].fix.clone().unwrap();
    assert_eq!(
        (
            fx_.code.as_str(),
            fx_.label.as_str(),
            fx_.action.as_str(),
            fx_.host.as_deref()
        ),
        (fix::E_TURN_AUTH, "Log in again on mac", "host", Some("mac"))
    );
    // A start that failed names its host, and a fallback when there is none.
    f.fake.fail.store(true, Ordering::SeqCst);
    let down = run_now(&f.deps, &ana, r.id, OCT8 + 30).await.unwrap();
    let d = get(&f.store, &ana, r.id).unwrap();
    let named = d.fixes.iter().find(|x| x.run_id == down.id).unwrap();
    assert_eq!(named.label, "Check mac, or give it a fallback host");
    assert_eq!(named.code, codes::E_SSH);
    // A run that did not fail has none.
    assert!(d.fixes.iter().all(|x| x.run_id != auth.id + 1000));
    assert_eq!(
        fix::turn_code(Some("rate_limit (429)")),
        fix::E_TURN_RATE_LIMIT
    );
    assert_eq!(fix::turn_code(None), fix::E_TURN_FAILED);
    assert_eq!(tick::cap_words(90 * 60), "1 h 30 min");
    assert_eq!(tick::cap_words(2 * H), "2 h");
}

// G7.9: a person answers a run Jev read as "nothing to do" (Change, Send to
// Inbox on the Automation board).
#[tokio::test]
async fn a_person_sends_a_nothing_run_to_the_inbox_and_jev_is_corrected() {
    let f = fx();
    let r = new_routine(&f, input(&f));
    let ana = person(&f.store, None, f.ana);
    let bo = person(&f.store, None, f.bo);
    let run = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    let sid = run.session_id.unwrap();
    event(&f, sid, "turn_done");
    tick_once(&f.deps, OCT8 + 60).await;
    let done = run_row(&f, run.id);
    {
        let s = lock(&f.store).unwrap();
        assert!(outcome::record(
            &s,
            &done,
            outcome::RunOutcome::Nothing,
            outcome::OutcomeSource::Jev,
            OCT8 + 90
        )
        .unwrap());
        assert_eq!(
            s.get_session_by_id(sid).unwrap().unwrap().last_viewed_at,
            Some(OCT8 + 90)
        );
    }

    // Another person may not answer it, and a bad value is refused.
    assert_eq!(
        set_run_outcome(&f.store, &bo, r.id, run.id, "needs_person", OCT8 + 120)
            .unwrap_err()
            .code,
        codes::E_NOTFOUND
    );
    assert_eq!(
        set_run_outcome(&f.store, &ana, r.id, run.id, "failed", OCT8 + 120)
            .unwrap_err()
            .code,
        codes::E_INVALID
    );
    assert_eq!(
        set_run_outcome(&f.store, &ana, r.id, run.id + 99, "nothing", OCT8 + 120)
            .unwrap_err()
            .code,
        codes::E_NOTFOUND
    );

    // Send to Inbox: the person's answer outranks Jev's, and the session's
    // finished turn reads as unread again.
    let sent = set_run_outcome(&f.store, &ana, r.id, run.id, "needs_person", OCT8 + 120).unwrap();
    assert_eq!(sent.outcome.as_deref(), Some("needs_person"));
    assert_eq!(sent.outcome_source.as_deref(), Some("rule"));
    assert_eq!(
        lock(&f.store)
            .unwrap()
            .get_session_by_id(sid)
            .unwrap()
            .unwrap()
            .last_viewed_at,
        None
    );

    // Change back: nothing to do marks it seen again.
    let back = set_run_outcome(&f.store, &ana, r.id, run.id, "nothing", OCT8 + 150).unwrap();
    assert_eq!(back.outcome.as_deref(), Some("nothing"));
    assert_eq!(
        lock(&f.store)
            .unwrap()
            .get_session_by_id(sid)
            .unwrap()
            .unwrap()
            .last_viewed_at,
        Some(OCT8 + 150)
    );
}

#[tokio::test]
async fn a_failed_run_stays_failed_whatever_a_person_says() {
    let f = fx();
    let r = new_routine(&f, input(&f));
    let ana = person(&f.store, None, f.ana);
    f.fake.fail.store(true, Ordering::SeqCst);
    let run = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    tick_once(&f.deps, OCT8 + 60).await;
    assert_eq!(
        set_run_outcome(&f.store, &ana, r.id, run.id, "did_work", OCT8 + 90)
            .unwrap_err()
            .code,
        codes::E_INVALID_STATE
    );
    assert_eq!(run_row(&f, run.id).outcome.as_deref(), Some("failed"));
}

// ---- Debugging (migration 186): the trace, explain, dry_run ----

fn verdicts(f: &Fx, id: i64) -> Vec<(String, String)> {
    let mut rows = lock(&f.store).unwrap().routine_decisions(id, 200).unwrap();
    rows.reverse();
    rows.into_iter()
        .map(|d| (d.verdict, d.reason.unwrap_or_default()))
        .collect()
}

fn check<'a>(e: &'a RoutineExplain, name: &str) -> &'a debug::RoutineCheck {
    e.checks
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("no {name} check: {:?}", e.checks))
}

#[tokio::test]
async fn the_trace_says_why_an_event_did_not_fire_and_what_did() {
    let f = fx();
    let r = pr_routine(&f, "pr_ci_failed", |i| i.event_rate_secs = Some(H));
    let mine = pr_session(&f, "mine", f.ana);
    let theirs = pr_session(&f, "theirs", f.bo);
    let fail = |f: &Fx, sid: i64, url: &str| {
        see_pr(f, sid, url, "OPEN", None, Some("pending"));
        see_pr(f, sid, url, "OPEN", None, Some("failing"));
    };
    fail(&f, mine, PR);
    fail(&f, theirs, "https://github.com/acme/web/pull/43");
    tick_once(&f.deps, OCT8).await;
    // Inside the rate: dropped, and the trace says so.
    fail(&f, mine, PR);
    tick_once(&f.deps, OCT8 + 60).await;
    assert_eq!(runs_of(&f, r.id).len(), 1);
    let v = verdicts(&f, r.id);
    let kinds: Vec<&str> = v.iter().map(|(k, _)| k.as_str()).collect();
    // A pass judges its batch first, then fires: bo's PR is filtered as
    // it is read, before ana's fires.
    assert_eq!(kinds, ["filtered", "fired", "dropped"], "{v:?}");
    assert!(v[0].1.contains("someone else's"), "{v:?}");
    assert!(v[1].1.starts_with("started on mac"), "{v:?}");
    assert!(v[2].1.contains("already ran for pr:"), "{v:?}");
    // A skip is a decision too, with its run.
    let ana = person(&f.store, None, f.ana);
    let mut i = input(&f);
    i.budget_day_micros = Some(1_000_000);
    let c = new_routine(&f, i);
    let first = run_now(&f.deps, &ana, c.id, OCT8 + H).await.unwrap();
    spend(&f, first.session_id.unwrap(), 2_000_000);
    event(&f, first.session_id.unwrap(), "turn_done");
    tick_once(&f.deps, OCT8 + H + 20).await;
    let skipped = run_now(&f.deps, &ana, c.id, OCT8 + 2 * H).await.unwrap();
    assert_eq!(skipped.state, "skipped");
    let d = lock(&f.store).unwrap().routine_decisions(c.id, 1).unwrap();
    assert_eq!(d[0].verdict, "skipped");
    assert_eq!(d[0].run_id, Some(skipped.id));
}

#[tokio::test]
async fn explain_names_the_check_that_stops_a_routine() {
    let f = fx();
    let r = new_routine(&f, input(&f));
    let ana = person(&f.store, None, f.ana);
    let e = explain(&f.store, &ana, r.id, None, OCT8).unwrap();
    assert_eq!(check(&e, "enabled").state, "ok");
    assert!(e.summary.starts_with("Fires at its next"), "{}", e.summary);
    assert_eq!(e.next_run_at, r.next_run_at);
    lock(&f.store)
        .unwrap()
        .set_setting(crate::service::settings::AUTOMATION_PAUSED, "true")
        .unwrap();
    let e = explain(&f.store, &ana, r.id, None, OCT8).unwrap();
    assert_eq!(e.state, "block");
    assert_eq!(check(&e, "automation").state, "block");
    assert!(e.summary.contains("Pause all is on"), "{}", e.summary);
    lock(&f.store)
        .unwrap()
        .set_setting(crate::service::settings::AUTOMATION_PAUSED, "false")
        .unwrap();
    skip_next(&f.store, &ana, r.id, true).unwrap();
    let e = explain(&f.store, &ana, r.id, None, OCT8).unwrap();
    assert_eq!(check(&e, "schedule").state, "warn");
    assert!(e.summary.contains("Skip next"), "{}", e.summary);
    set_enabled(&f.store, &ana, r.id, false).unwrap();
    let e = explain(&f.store, &ana, r.id, None, OCT8).unwrap();
    assert!(
        e.summary.starts_with("Does not fire: it is off"),
        "{}",
        e.summary
    );
    // Someone else's routine is not there at all.
    let bo = person(&f.store, None, f.bo);
    assert_eq!(
        explain(&f.store, &bo, r.id, None, OCT8).unwrap_err().code,
        codes::E_NOTFOUND
    );
    assert_eq!(
        dry_run(&f.store, &bo, r.id, None, OCT8).unwrap_err().code,
        codes::E_NOTFOUND
    );
}

#[tokio::test]
async fn explain_judges_the_events_its_owner_may_read_only() {
    let f = fx();
    let r = pr_routine(&f, "pr_review", |i| i.event_repo = Some("acme/web".into()));
    let mine = pr_session(&f, "mine", f.ana);
    let theirs = pr_session(&f, "theirs", f.bo);
    see_pr(&f, mine, PR, "OPEN", None, None);
    see_pr(&f, mine, PR, "OPEN", Some("APPROVED"), None);
    let other = "https://github.com/acme/api/pull/7";
    see_pr(&f, mine, other, "OPEN", None, None);
    see_pr(&f, mine, other, "OPEN", Some("APPROVED"), None);
    let hidden = "https://github.com/acme/web/pull/99";
    see_pr(&f, theirs, hidden, "OPEN", None, None);
    see_pr(&f, theirs, hidden, "OPEN", Some("APPROVED"), None);
    let ana = person(&f.store, None, f.ana);
    let e = explain(&f.store, &ana, r.id, None, OCT8).unwrap();
    let ev = e.event.expect("an event routine explains its events");
    assert_eq!(ev.kind, "pr_review");
    assert_eq!(ev.unread, None, "a person is not told every person's count");
    assert_eq!(
        ev.recent.len(),
        2,
        "bo's session is left out: {:?}",
        ev.recent
    );
    assert!(ev
        .recent
        .iter()
        .all(|v| v.session_id == Some(mine) && !v.read));
    let (fires, filtered): (Vec<_>, Vec<_>) = ev.recent.iter().partition(|v| v.fires);
    assert_eq!(fires.len(), 1);
    assert!(
        filtered[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("acme/api is not acme/web"),
        "{filtered:?}"
    );
    // The internal reader counts the backlog.
    let all = explain(&f.store, &ViewScope::internal(), r.id, None, OCT8).unwrap();
    assert_eq!(all.event.unwrap().unread, Some(3));
    // The pass's filtered decision about bo's session is not shown to ana.
    tick_once(&f.deps, OCT8 + 20).await;
    let d = decisions(&f.store, &ana, r.id, None).unwrap();
    assert!(d.iter().all(|d| d.session_id != Some(theirs)), "{d:?}");
    assert!(d.iter().any(|d| d.verdict == "fired"), "{d:?}");
    let all = decisions(&f.store, &ViewScope::internal(), r.id, None).unwrap();
    assert!(all.iter().any(|d| d.session_id == Some(theirs)), "{all:?}");
}

#[tokio::test]
async fn dry_run_says_what_a_fire_would_start_and_writes_nothing() {
    let f = fx();
    let r = pr_routine(&f, "pr_review", |i| i.autonomy = Some(0));
    let mine = pr_session(&f, "mine", f.ana);
    let theirs = pr_session(&f, "theirs", f.bo);
    see_pr(&f, mine, PR, "OPEN", None, None);
    see_pr(&f, mine, PR, "OPEN", Some("CHANGES_REQUESTED"), None);
    see_pr(
        &f,
        theirs,
        "https://github.com/acme/web/pull/43",
        "OPEN",
        None,
        None,
    );
    see_pr(
        &f,
        theirs,
        "https://github.com/acme/web/pull/43",
        "OPEN",
        Some("APPROVED"),
        None,
    );
    let ana = person(&f.store, None, f.ana);
    let d = dry_run(&f.store, &ana, r.id, None, OCT8).unwrap();
    assert!(d.starts, "{d:?}");
    assert_eq!(d.session.host_alias, "mac");
    assert_eq!(d.session.owner_person_id, Some(f.ana));
    let ev = d
        .event
        .clone()
        .expect("its kind's newest event ana may read");
    assert_eq!(ev.session_id, Some(mine));
    assert!(ev.fires);
    assert!(d.prompt.starts_with("Review my open PRs"), "{}", d.prompt);
    assert!(
        d.prompt.ends_with(&format!(
            "Started because pull request {PR} got a review asking for changes."
        )),
        "{}",
        d.prompt
    );
    assert!(d.scheduled_only.iter().any(|c| c.name == "event"));
    assert!(runs_of(&f, r.id).is_empty(), "a dry run starts nothing");
    assert!(f.fake.started.lock().unwrap().is_empty());
    assert!(lock(&f.store)
        .unwrap()
        .routine_decisions(r.id, 10)
        .unwrap()
        .is_empty());
    // An event on bo's session answers like one that does not exist.
    let bo_event = lock(&f.store)
        .unwrap()
        .recent_events_of_kind("pr_review", 10)
        .unwrap()
        .into_iter()
        .find(|e| e.1 == theirs)
        .unwrap()
        .0;
    let err = dry_run(&f.store, &ana, r.id, Some(bo_event), OCT8).unwrap_err();
    assert_eq!(err.code, codes::E_NOTFOUND);
    assert_eq!(
        dry_run(&f.store, &ana, r.id, Some(9_999_999), OCT8)
            .unwrap_err()
            .message,
        err.message.replace(&bo_event.to_string(), "9999999")
    );
    // A cron routine takes no event.
    let c = new_routine(&f, input(&f));
    assert_eq!(
        dry_run(&f.store, &ana, c.id, Some(bo_event), OCT8)
            .unwrap_err()
            .code,
        codes::E_INVALID
    );
    let d = dry_run(&f.store, &ana, c.id, None, OCT8).unwrap();
    assert!(d.event.is_none());
    assert_eq!(d.prompt, c.prompt);
}

/// A run's trace, as `(phase, kind, summary)`.
fn trace_of(f: &Fx, run_id: i64) -> Vec<(String, String, String)> {
    lock(&f.store)
        .unwrap()
        .run_trace(&crate::service::run_trace::routine_ref(run_id), 0, 100)
        .unwrap()
        .into_iter()
        .map(|t| (t.phase, t.kind, t.summary))
        .collect()
}

fn phases(t: &[(String, String, String)]) -> Vec<&str> {
    t.iter().map(|(p, _, _)| p.as_str()).collect()
}

#[tokio::test]
async fn a_run_traces_each_step_from_its_trigger_to_its_settle() {
    let f = fx();
    let r = new_routine(&f, input(&f));
    let ana = person(&f.store, None, f.ana);
    let run = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    typing_done().await;
    let t = trace_of(&f, run.id);
    assert_eq!(
        phases(&t),
        [
            "trigger",
            "checks",
            "placement",
            "spawn",
            "prompt",
            "prompt"
        ]
    );
    assert_eq!(t[0].2, "Started by Run now");
    assert!(t[1].2.contains("checks passed"), "{t:?}");
    assert_eq!(t[2].2, "Placed on mac");
    assert_eq!(t[3].2, "Session started on mac");
    assert_eq!(t[5].2, "Prompt typed into the session");
    event(&f, run.session_id.unwrap(), "turn_done");
    tick_once(&f.deps, OCT8 + 60).await;
    let t = trace_of(&f, run.id);
    assert_eq!(
        t[6],
        ("settle".into(), "settle".into(), "Its turn finished".into())
    );
}

#[tokio::test]
async fn a_failed_run_traces_its_error_and_who_judged_it() {
    let f = fx();
    let r = new_routine(&f, input(&f));
    let ana = person(&f.store, None, f.ana);
    let run = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    typing_done().await;
    event(&f, run.session_id.unwrap(), "stop_failure");
    tick_once(&f.deps, OCT8 + 60).await;
    let t = trace_of(&f, run.id);
    let n = t.len();
    assert_eq!(&t[n - 2].0, "settle");
    assert_eq!(&t[n - 2].1, "error");
    assert!(
        t[n - 2].2.starts_with("Its turn ended in an error"),
        "{t:?}"
    );
    assert_eq!(
        t[n - 1],
        (
            "outcome".into(),
            "decision".into(),
            "It failed, judged by how it ended".into()
        )
    );
}

#[tokio::test]
async fn a_skipped_fire_traces_why_and_a_failed_start_traces_the_error() {
    let f = fx();
    let r = new_routine(&f, input(&f));
    let ana = person(&f.store, None, f.ana);
    run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    let skipped = run_now(&f.deps, &ana, r.id, OCT8 + 60).await.unwrap();
    let t = trace_of(&f, skipped.id);
    assert_eq!(phases(&t), ["trigger", "checks", "settle"]);
    assert_eq!(t[1].2, "Skipped: its last run is still going");
    // A start that fails, with no fallback.
    let mut i = input(&f);
    i.overlap = Some("parallel".into());
    save(&f.store, &ana, Some(r.id), &i).unwrap();
    f.fake.fail.store(true, Ordering::SeqCst);
    let failed = run_now(&f.deps, &ana, r.id, OCT8 + 120).await.unwrap();
    let t = trace_of(&f, failed.id);
    assert_eq!(phases(&t), ["trigger", "checks", "placement", "spawn"]);
    assert_eq!(t[3].1, "error");
    assert_eq!(t[3].2, "The start on mac failed: host unreachable");
}

#[tokio::test]
async fn a_fallback_start_traces_both_hosts() {
    let f = fx();
    let mut i = input(&f);
    i.fallback_host = Some("nas".into());
    let r = new_routine(&f, i);
    let ana = person(&f.store, None, f.ana);
    reachable(&f, "mac", true);
    reachable(&f, "nas", true);
    *f.fake.fail_on.lock().unwrap() = Some("mac".into());
    let run = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    let t = trace_of(&f, run.id);
    assert_eq!(
        &phases(&t)[2..6],
        ["placement", "spawn", "placement", "spawn"]
    );
    assert_eq!(t[3].1, "error");
    assert!(
        t[4].2.starts_with("Placed on nas: the start on mac failed"),
        "{t:?}"
    );
    assert_eq!(t[5].2, "Session started on nas");
}

#[tokio::test]
async fn only_who_sees_the_routine_reads_its_runs_trace() {
    use crate::service::run_trace::{read, routine_ref, TraceArgs};
    let f = fx();
    let r = new_routine(&f, input(&f));
    let ana = person(&f.store, None, f.ana);
    let run = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    let args = TraceArgs {
        run_id: routine_ref(run.id),
        ..Default::default()
    };
    let mine = read(&f.store, &ana, &args).unwrap();
    assert_eq!(mine.steps[0].seq, 1);
    assert!(!mine.more);
    let bo = person(&f.store, None, f.bo);
    assert_eq!(
        read(&f.store, &bo, &args).unwrap_err().code,
        codes::E_NOTFOUND
    );
    // Reading on from a step, and in pages.
    let on = read(
        &f.store,
        &ana,
        &TraceArgs {
            after_seq: Some(1),
            limit: Some(1),
            ..args.clone()
        },
    )
    .unwrap();
    assert_eq!(on.steps.len(), 1);
    assert_eq!(on.steps[0].seq, 2);
    assert!(on.more);
    // A source with no trace, and nonsense, are not found.
    for id in ["task:1", "routine:x", "nothing"] {
        let a = TraceArgs {
            run_id: id.into(),
            ..Default::default()
        };
        assert_eq!(
            read(&f.store, &ana, &a).unwrap_err().code,
            codes::E_NOTFOUND
        );
    }
}

// ── A run's work in its trace (`work_steps`) ──────────────────────────────

fn transcript(lines: &[serde_json::Value]) -> crate::service::transcript::Conversation {
    let jsonl = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    crate::service::transcript::Conversation {
        turns: crate::service::transcript::parse_conversation(&jsonl),
        truncated: false,
        context: None,
        events: Vec::new(),
    }
}

#[test]
fn a_transcript_yields_its_finished_tools_with_their_times() {
    use serde_json::json;
    let conv = transcript(&[
        json!({"type":"user","timestamp":"2026-10-08T10:00:00Z","message":{"role":"user","content":"go"}}),
        json!({"type":"assistant","timestamp":"2026-10-08T10:00:05Z","message":{"content":[
            {"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo test"}}]}}),
        json!({"type":"user","timestamp":"2026-10-08T10:00:47Z","message":{"content":[
            {"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}),
        json!({"type":"assistant","timestamp":"2026-10-08T10:00:50Z","message":{"content":[
            {"type":"tool_use","id":"t2","name":"Read","input":{"file_path":"/a/b.rs"}}]}}),
        json!({"type":"user","timestamp":"2026-10-08T10:00:51Z","message":{"content":[
            {"type":"tool_result","tool_use_id":"t2","content":"no such file","is_error":true}]}}),
        json!({"type":"assistant","timestamp":"2026-10-08T10:00:52Z","message":{"content":[
            {"type":"tool_use","id":"t3","name":"Edit","input":{"file_path":"/a/c.rs"}}]}}),
    ]);
    let calls = work_steps::tool_calls(&conv);
    // t3 has no result yet: it is the session's current activity, not a step.
    assert_eq!(
        calls.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        ["t1", "t2"]
    );
    assert_eq!(calls[0].name, "Bash");
    assert!(!calls[0].error && calls[1].error);
    let step = work_steps::tool_step(&calls[0], 0);
    assert_eq!(
        (step.phase, step.kind, step.actor),
        ("working", "tool_call", "claude")
    );
    assert_eq!(step.duration_ms, Some(42_000));
    assert_eq!(step.at, calls[0].ended_at.unwrap());
    let failed = work_steps::tool_step(&calls[1], 0);
    assert_eq!(failed.kind, "error");
    assert!(failed.summary.ends_with("failed"), "{}", failed.summary);
}

struct FakeTools(Vec<work_steps::ToolCall>);

#[async_trait::async_trait]
impl work_steps::ToolReader for FakeTools {
    async fn tools(&self, _row: &SessionRow) -> Result<Vec<work_steps::ToolCall>, IpcError> {
        Ok(self.0.clone())
    }
}

fn call(id: &str, at: i64) -> work_steps::ToolCall {
    work_steps::ToolCall {
        id: id.into(),
        name: "Bash".into(),
        summary: format!("Bash({id})"),
        target: None,
        error: false,
        at: Some(at),
        ended_at: Some(at + 3),
    }
}

#[tokio::test]
async fn an_open_runs_tools_land_in_its_trace_once_each() {
    let f = fx();
    let r = new_routine(&f, input(&f));
    let ana = person(&f.store, None, f.ana);
    let run = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    typing_done().await;
    // A call from before the run started is not the run's.
    let reader = FakeTools(vec![call("old", OCT8 - 100), call("a", OCT8 + 5)]);
    work_steps::read_open(&f.store, &reader, OCT8 + 20).await;
    let reader = FakeTools(vec![
        call("old", OCT8 - 100),
        call("a", OCT8 + 5),
        call("b", OCT8 + 30),
    ]);
    work_steps::read_open(&f.store, &reader, OCT8 + 40).await;
    let work: Vec<_> = trace_of(&f, run.id)
        .into_iter()
        .filter(|(p, _, _)| p == "working")
        .map(|(_, _, s)| s)
        .collect();
    assert_eq!(work, ["Bash(a)", "Bash(b)"]);
    // A settled run is read no more.
    event(&f, run.session_id.unwrap(), "turn_done");
    tick_once(&f.deps, OCT8 + 60).await;
    let reader = FakeTools(vec![call("c", OCT8 + 70)]);
    work_steps::read_open(&f.store, &reader, OCT8 + 80).await;
    assert!(!trace_of(&f, run.id).iter().any(|(_, _, s)| s == "Bash(c)"));
}

// ---- Control review F16: back off on no change, wait out a limit ----

#[tokio::test]
async fn a_fire_at_a_limit_holds_the_next_until_the_reset() {
    let f = fx();
    let mut i = input(&f);
    i.cron = Some("0 * * * *".into());
    let r = new_routine(&f, i);
    // 9:30, the account at 93%; its window resets at 10:30.
    let t = OCT8 + 9 * H + 1800;
    crate::service::account_limits::seed_usage(
        &lock(&f.store).unwrap(),
        "mac",
        Some("work"),
        "work",
        93.0,
        t,
    );
    due_at(&f, r.id, t);
    tick_once(&f.deps, t).await;
    assert_eq!(runs_of(&f, r.id)[0].state, "skipped");
    // Not 10:00, when it would only skip again: the first hour past 10:30.
    assert_eq!(routine(&f, r.id).next_run_at, Some(OCT8 + 11 * H));
    let v = verdicts(&f, r.id);
    assert!(
        v.iter()
            .any(|(_, why)| why.contains("held until") && why.contains("usage limit")),
        "{v:?}"
    );
    // Off: the schedule as written.
    crate::service::settings::set(
        &lock(&f.store).unwrap(),
        crate::service::settings::ROUTINES_PAUSE_AT_LIMIT,
        "false",
    )
    .unwrap();
    due_at(&f, r.id, t + 20);
    tick_once(&f.deps, t + 20).await;
    assert_eq!(routine(&f, r.id).next_run_at, Some(OCT8 + 10 * H));
}

#[tokio::test]
async fn a_retry_held_at_a_limit_outlasts_the_retry_window() {
    let f = fx();
    let r = retrying_routine(&f);
    let ana = person(&f.store, None, f.ana);
    let run = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    let seed = |pct: f64, at: i64| {
        crate::service::account_limits::seed_usage(
            &lock(&f.store).unwrap(),
            "mac",
            Some("work"),
            "work",
            pct,
            at,
        )
    };
    seed(93.0, OCT8);
    lock(&f.store)
        .unwrap()
        .insert_session_event(
            run.session_id.unwrap(),
            "stop_failure",
            Some("rate_limit (429)"),
        )
        .unwrap();
    tick_once(&f.deps, OCT8 + 20).await;
    assert_eq!(
        run_row(&f, run.id).error_code.as_deref(),
        Some(fix::E_TURN_RATE_LIMIT)
    );
    assert_eq!(runs_of(&f, r.id).len(), 1, "held at the limit");
    // Still over the line two hours on: still held, not dropped.
    seed(93.0, OCT8 + 2 * H);
    tick_once(&f.deps, OCT8 + 2 * H).await;
    assert_eq!(runs_of(&f, r.id).len(), 1);
    // The limit lifts past the usual retry window: the retry runs.
    seed(10.0, OCT8 + 3 * H);
    tick_once(&f.deps, OCT8 + 3 * H).await;
    let runs = runs_of(&f, r.id);
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert_retry_of(&runs[1], run.id);
}

/// Review w02-10: a rate-limited retry is held only when a fire now would
/// be skipped too. With a fallback host that can take it, the retry goes
/// there instead of waiting for the primary's reset.
#[tokio::test]
async fn a_rate_limited_retry_goes_to_the_fallback_a_fire_would_use() {
    let f = fx();
    let mut i = input(&f);
    i.retry_once = Some(true);
    i.fallback_host = Some("nas".into());
    let r = new_routine(&f, i);
    reachable(&f, "mac", true);
    reachable(&f, "nas", true);
    let ana = person(&f.store, None, f.ana);
    let run = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    assert_eq!(run.host_alias.as_deref(), Some("mac"));
    crate::service::account_limits::seed_usage(
        &lock(&f.store).unwrap(),
        "mac",
        Some("work"),
        "work",
        93.0,
        OCT8,
    );
    lock(&f.store)
        .unwrap()
        .insert_session_event(
            run.session_id.unwrap(),
            "stop_failure",
            Some("rate_limit (429)"),
        )
        .unwrap();
    tick_once(&f.deps, OCT8 + 20).await;
    let runs = runs_of(&f, r.id);
    assert_eq!(runs.len(), 2, "not held: {runs:?}");
    assert_eq!(runs[1].host_alias.as_deref(), Some("nas"));
}

#[tokio::test]
async fn a_run_that_found_nothing_doubles_the_gap_when_backoff_is_on() {
    let f = fx();
    let mut i = input(&f);
    i.cron = Some("0 * * * *".into());
    let r = new_routine(&f, i);
    crate::service::settings::set(
        &lock(&f.store).unwrap(),
        crate::service::settings::ROUTINES_BACKOFF_ON_NO_CHANGE,
        "true",
    )
    .unwrap();
    let nine = OCT8 + 9 * H;
    due_at(&f, r.id, nine);
    tick_once(&f.deps, nine).await;
    // No streak yet: the next hour.
    assert_eq!(routine(&f, r.id).next_run_at, Some(nine + H));
    let run = runs_of(&f, r.id).remove(0);
    event(&f, run.session_id.unwrap(), "turn_done");
    tick_once(&f.deps, nine + 60).await;
    {
        let s = lock(&f.store).unwrap();
        let done = s.get_routine_run(run.id).unwrap().unwrap();
        assert_eq!(done.state, "done");
        outcome::record(
            &s,
            &done,
            outcome::RunOutcome::Nothing,
            outcome::OutcomeSource::Jev,
            nine + 90,
        )
        .unwrap();
    }
    tick_once(&f.deps, nine + H).await;
    assert_eq!(runs_of(&f, r.id).len(), 2);
    // One run with nothing new: two hours, on the schedule.
    assert_eq!(routine(&f, r.id).next_run_at, Some(nine + 3 * H));
    let v = verdicts(&f, r.id);
    assert!(
        v.iter().any(|(_, why)| why.contains("found nothing new")),
        "{v:?}"
    );
}

fn event_routine(f: &Fx, event: &str, change: impl FnOnce(&mut RoutineInput)) -> RoutineRow {
    pr_routine(f, event, change)
}

fn fleet_event(f: &Fx, e: crate::store::NewFleetEvent<'_>) -> i64 {
    lock(&f.store).unwrap().insert_fleet_event(&e).unwrap()
}

fn host_down(host: &str) -> (String, String) {
    (
        format!("host:{host}"),
        format!("host {host} is unreachable"),
    )
}

#[tokio::test]
async fn a_routine_fires_on_a_host_going_offline_once_per_host_in_its_rate() {
    let f = fx();
    let (old_subject, old_words) = host_down("mac");
    // History before the routine never fires it.
    fleet_event(
        &f,
        crate::store::NewFleetEvent {
            kind: "host_offline",
            subject: &old_subject,
            detail: Some(&old_words),
            host_alias: Some("mac"),
            ..Default::default()
        },
    );
    let r = event_routine(&f, "host_offline", |i| i.event_rate_secs = Some(H));
    tick_once(&f.deps, OCT8).await;
    assert!(runs_of(&f, r.id).is_empty(), "history does not fire");
    for host in ["mac", "mac", "nas"] {
        let (subject, words) = host_down(host);
        fleet_event(
            &f,
            crate::store::NewFleetEvent {
                kind: "host_offline",
                subject: &subject,
                detail: Some(&words),
                host_alias: Some(host),
                ..Default::default()
            },
        );
    }
    tick_once(&f.deps, OCT8 + 20).await;
    let runs = runs_of(&f, r.id);
    assert_eq!(runs.len(), 2, "one per host in the rate: {runs:?}");
    assert!(runs[0]
        .trigger_ref
        .as_deref()
        .unwrap()
        .starts_with("fleet:host:mac:"));
    let sid = runs[0].session_id.unwrap();
    assert!(
        queued_prompt(&f, sid).contains("Started because host mac is unreachable."),
        "{}",
        queued_prompt(&f, sid)
    );
    assert_eq!(
        trace::trigger_words("event", runs[0].trigger_ref.as_deref()),
        "Fired by a fleet event on host mac"
    );
}

#[tokio::test]
async fn a_host_filter_narrows_fleet_and_session_events() {
    let f = fx();
    let r = event_routine(&f, "host_online", |i| i.event_host = Some("nas".into()));
    assert_eq!(r.event_host.as_deref(), Some("nas"));
    for host in ["mac", "nas"] {
        let subject = format!("host:{host}");
        fleet_event(
            &f,
            crate::store::NewFleetEvent {
                kind: "host_online",
                subject: &subject,
                host_alias: Some(host),
                ..Default::default()
            },
        );
    }
    tick_once(&f.deps, OCT8).await;
    let runs = runs_of(&f, r.id);
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert!(runs[0]
        .trigger_ref
        .as_deref()
        .unwrap()
        .starts_with("fleet:host:nas:"));
    let filtered = lock(&f.store).unwrap().routine_decisions(r.id, 10).unwrap();
    assert!(
        filtered
            .iter()
            .any(|d| d.verdict == "filtered"
                && d.reason.as_deref().unwrap_or("").contains("not nas")),
        "{filtered:?}"
    );
    // A session event on another host does not fire a host-filtered routine.
    let stuck = event_routine(&f, "stuck", |i| i.event_host = Some("nas".into()));
    let on_mac = pr_session(&f, "on-mac", f.ana);
    event(&f, on_mac, "stuck");
    tick_once(&f.deps, OCT8 + 20).await;
    assert!(runs_of(&f, stuck.id).is_empty());
    // An account event takes no host filter.
    let mut i = event_input(&f, "account_limited");
    i.event_host = Some("mac".into());
    let err = save(&f.store, &person(&f.store, None, f.ana), None, &i).unwrap_err();
    assert_eq!(err.code, codes::E_INVALID);
    i.event_host = Some("nope".into());
    i.event = Some("host_low".into());
    let err = save(&f.store, &person(&f.store, None, f.ana), None, &i).unwrap_err();
    assert_eq!(err.code, codes::E_NOTFOUND);
}

#[tokio::test]
async fn a_fleet_event_host_filter_stays_in_the_routines_org() {
    let f = fx();
    let other = {
        let s = lock(&f.store).unwrap();
        let o = s.add_org("other", None, false).unwrap();
        s.set_host_org("nas", Some(o.id)).unwrap();
        o.id
    };
    let mut i = event_input(&f, "host_offline");
    i.event_host = Some("nas".into());
    // The routine runs on mac (no org); nas's events are of `other` and
    // would be filtered on every fire.
    let ana = person(&f.store, None, f.ana);
    let err = save(&f.store, &ana, None, &i).unwrap_err();
    assert_eq!(err.code, codes::E_INVALID, "{err:?}");
    assert!(err.message.contains("another organisation"), "{err:?}");
    // In the same org it saves.
    lock(&f.store)
        .unwrap()
        .set_host_org("mac", Some(other))
        .unwrap();
    assert!(save(&f.store, &ana, None, &i).is_ok());
    // A session event fires on its owner, not its org: any host passes.
    lock(&f.store).unwrap().set_host_org("mac", None).unwrap();
    i.event = Some("stuck".into());
    assert!(save(&f.store, &ana, None, &i).is_ok());
}

#[tokio::test]
async fn a_host_filter_takes_any_valid_alias() {
    let f = fx();
    let long = format!("{}.example.com", "a".repeat(70));
    lock(&f.store)
        .unwrap()
        .insert_host(&long, Some(&long))
        .unwrap();
    let r = event_routine(&f, "host_online", |i| i.event_host = Some(long.clone()));
    assert_eq!(r.event_host.as_deref(), Some(&*long));
}

#[tokio::test]
async fn a_mission_event_fires_its_owners_routine_only() {
    let f = fx();
    let r = event_routine(&f, "mission_done", |_| {});
    for owner in [f.bo, f.ana] {
        let subject = format!("mission:{owner}");
        fleet_event(
            &f,
            crate::store::NewFleetEvent {
                kind: "mission_done",
                subject: &subject,
                detail: Some("mission #1 \"Ship it\" completed"),
                person_id: Some(owner),
                ..Default::default()
            },
        );
    }
    tick_once(&f.deps, OCT8).await;
    let runs = runs_of(&f, r.id);
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert!(runs[0]
        .trigger_ref
        .as_deref()
        .unwrap()
        .starts_with(&format!("fleet:mission:{}:", f.ana)));
}

#[tokio::test]
async fn needs_you_fires_on_a_blocking_notification_only() {
    let f = fx();
    let r = event_routine(&f, NEEDS_YOU, |_| {});
    let sid = pr_session(&f, "asks", f.ana);
    {
        let s = lock(&f.store).unwrap();
        s.insert_session_event(sid, "notification", Some("idle_prompt"))
            .unwrap();
    }
    tick_once(&f.deps, OCT8).await;
    assert!(
        runs_of(&f, r.id).is_empty(),
        "an idle notice waits on nobody"
    );
    assert!(
        lock(&f.store)
            .unwrap()
            .routine_decisions(r.id, 10)
            .unwrap()
            .is_empty(),
        "and it is not the routine's event: no trace row"
    );
    {
        let s = lock(&f.store).unwrap();
        s.insert_session_event(sid, "notification", Some("permission_prompt"))
            .unwrap();
    }
    tick_once(&f.deps, OCT8 + 20).await;
    assert_eq!(runs_of(&f, r.id).len(), 1);
}

#[tokio::test]
async fn a_routine_switched_to_a_fleet_event_starts_after_the_newest_one() {
    let f = fx();
    let r = event_routine(&f, "stuck", |_| {});
    let id = fleet_event(
        &f,
        crate::store::NewFleetEvent {
            kind: "account_limited",
            subject: "account:u-1",
            ..Default::default()
        },
    );
    let i = event_input(&f, "account_limited");
    let saved = save(&f.store, &person(&f.store, None, f.ana), Some(r.id), &i).unwrap();
    assert_eq!(routine(&f, saved.id).event_cursor, id);
    tick_once(&f.deps, OCT8).await;
    assert!(runs_of(&f, r.id).is_empty());
}

#[test]
fn a_mission_that_completes_writes_a_fleet_event() {
    let f = fx();
    let s = lock(&f.store).unwrap();
    let m = s
        .create_mission(
            &crate::store::NewMission {
                org_id: None,
                owner_person_id: Some(f.ana),
                root_item_id: None,
                name: "Ship it",
                goal: "Ship the release.",
                non_goals: None,
                done_when: &[],
                mode: None,
                level: None,
            },
            "test",
        )
        .unwrap();
    s.set_mission_state(m.id, None, "active", "test").unwrap();
    s.set_mission_state(m.id, None, "completed", "test")
        .unwrap();
    let rows = s.fleet_events_of_kind_after(0, "mission_done", 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].subject, format!("mission:{}", m.id));
    assert_eq!(rows[0].person_id, Some(f.ana));
}

// ---- Review w06: what an event run is told, and what its trace keeps ----

fn owned_session(f: &Fx, name: &str, host: &str) -> i64 {
    let s = lock(&f.store).unwrap();
    let id = s
        .upsert_session(name, host, None, None, 1, 1, "running", None)
        .unwrap();
    s.claim_if_unclaimed(id, Some(f.ana)).unwrap();
    id
}

#[tokio::test]
async fn a_session_event_run_is_told_which_session_started_it() {
    let f = fx();
    let r = event_routine(&f, "stuck", |_| {});
    let mine = owned_session(&f, "api", "nas");
    lock(&f.store)
        .unwrap()
        .insert_session_event(mine, "stuck", Some("no output for 30 min"))
        .unwrap();
    tick_once(&f.deps, OCT8).await;
    let runs = runs_of(&f, r.id);
    assert_eq!(runs.len(), 1, "{runs:?}");
    let prompt = queued_prompt(&f, runs[0].session_id.unwrap());
    assert!(
        prompt.ends_with(&format!(
            "Started because session #{mine} \"api\" on nas is stuck (no output for 30 min)."
        )),
        "{prompt}"
    );
}

#[tokio::test]
async fn the_retry_of_an_event_run_is_told_what_started_it() {
    let f = fx();
    let r = event_routine(&f, "stuck", |i| i.retry_once = Some(true));
    let mine = owned_session(&f, "api", "mac");
    f.fake.fail.store(true, Ordering::SeqCst);
    event(&f, mine, "stuck");
    tick_once(&f.deps, OCT8).await;
    let runs = runs_of(&f, r.id);
    assert_eq!(runs[0].state, "failed", "{runs:?}");
    f.fake.fail.store(false, Ordering::SeqCst);
    tick_once(&f.deps, OCT8 + 20).await;
    let runs = runs_of(&f, r.id);
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert_eq!(
        runs[1].trigger_ref.as_deref(),
        Some(&*format!("retry:{}", runs[0].id))
    );
    let prompt = queued_prompt(&f, runs[1].session_id.unwrap());
    assert!(
        prompt.ends_with(&format!(
            "Started because session #{mine} \"api\" on mac is stuck."
        )),
        "{prompt}"
    );
}

#[test]
fn a_retry_rebuilds_the_line_of_a_pr_or_fleet_event() {
    let f = fx();
    let sid = pr_session(&f, "api", f.ana);
    see_pr(&f, sid, PR, "OPEN", None, None);
    let s = lock(&f.store).unwrap();
    let eid = s.latest_session_event_id().unwrap();
    let pr = s.pull_request_by_url(PR).unwrap().unwrap();
    assert_eq!(
        tick::context_of_ref(&s, Some(&format!("pr:{}:{eid}", pr.id))).as_deref(),
        Some(&*format!("Started because pull request {PR} was opened."))
    );
    let id = s
        .insert_fleet_event(&crate::store::NewFleetEvent {
            kind: "host_offline",
            subject: "host:mac",
            detail: Some("host mac is unreachable"),
            host_alias: Some("mac"),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        tick::context_of_ref(&s, Some(&format!("fleet:host:mac:{id}"))).as_deref(),
        Some("Started because host mac is unreachable.")
    );
    assert_eq!(tick::context_of_ref(&s, Some("retry:4")), None);
    assert_eq!(tick::context_of_ref(&s, None), None);
}

#[tokio::test]
async fn the_events_a_filter_keeps_out_are_one_trace_row_per_pass() {
    let f = fx();
    let r = event_routine(&f, "turn_done", |i| i.event_host = Some("nas".into()));
    let on_mac = owned_session(&f, "web", "mac");
    for _ in 0..5 {
        event(&f, on_mac, "turn_done");
    }
    tick_once(&f.deps, OCT8).await;
    let v = verdicts(&f, r.id);
    let filtered: Vec<_> = v.iter().filter(|(k, _)| k == "filtered").collect();
    assert_eq!(filtered.len(), 1, "{v:?}");
    assert!(
        filtered[0].1.starts_with("5 turn_done events filtered: #"),
        "{v:?}"
    );
    assert!(filtered[0].1.ends_with("; and 2 more"), "{v:?}");
}

#[tokio::test]
async fn a_move_fires_once_on_the_row_it_moved_to() {
    let f = fx();
    let r = event_routine(&f, "session_moved", |i| i.event_host = Some("nas".into()));
    let from = owned_session(&f, "api", "mac");
    let to = owned_session(&f, "api", "nas");
    let detail = serde_json::json!({
        "from_host": "mac",
        "to_host": "nas",
        "from_session_id": from,
        "to_session_id": to,
    })
    .to_string();
    {
        let s = lock(&f.store).unwrap();
        for sid in [from, to] {
            s.insert_session_event(sid, "session_moved", Some(&detail))
                .unwrap();
        }
    }
    tick_once(&f.deps, OCT8).await;
    let runs = runs_of(&f, r.id);
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert!(runs[0]
        .trigger_ref
        .as_deref()
        .unwrap()
        .starts_with(&format!("session:{to}:")));
    let prompt = queued_prompt(&f, runs[0].session_id.unwrap());
    assert!(
        prompt.ends_with("on nas was moved from mac to nas."),
        "{prompt}"
    );
    let v = verdicts(&f, r.id);
    assert!(!v.iter().any(|(k, _)| k == "filtered"), "{v:?}");
}

#[tokio::test]
async fn a_retry_held_at_a_limit_says_so_once_and_when_a_newer_run_drops_it() {
    let f = fx();
    let mut i = input(&f);
    i.retry_once = Some(true);
    let r = new_routine(&f, i);
    let ana = person(&f.store, None, f.ana);
    let run = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    crate::service::account_limits::seed_usage(
        &lock(&f.store).unwrap(),
        "mac",
        Some("work"),
        "work",
        93.0,
        OCT8,
    );
    lock(&f.store)
        .unwrap()
        .insert_session_event(
            run.session_id.unwrap(),
            "stop_failure",
            Some("rate_limit (429)"),
        )
        .unwrap();
    tick_once(&f.deps, OCT8 + 20).await;
    tick_once(&f.deps, OCT8 + 40).await;
    let waits = |f: &Fx| {
        verdicts(f, r.id)
            .into_iter()
            .filter(|(k, why)| k == "skipped" && why.contains("waits"))
            .count()
    };
    assert_eq!(waits(&f), 1, "{:?}", verdicts(&f, r.id));
    let next = run_now(&f.deps, &ana, r.id, OCT8 + 60).await.unwrap();
    let v = verdicts(&f, r.id);
    let dropped = format!(
        "retry of run #{} dropped: run #{} ran since",
        run.id, next.id
    );
    assert!(
        v.iter().any(|(k, why)| k == "dropped" && *why == dropped),
        "{v:?}"
    );
}

/// Review w30-5: every event a routine may fire on has a label on the
/// desktop (`src/lib/routines.ts`), so a new kind cannot show as its raw key.
#[test]
fn every_event_has_a_desktop_label() {
    let ts = crate::repo_files::read("src/lib/routines.ts");
    let table = |name: &str| -> String {
        let head = format!("const {name}: Record<string, string> = {{");
        let start = ts
            .find(&head)
            .unwrap_or_else(|| panic!("{name} in routines.ts"));
        let body = &ts[start..];
        body[..body.find("\n};").unwrap()].to_string()
    };
    let tables = [
        table("EVENT_WORDS"),
        table("FLEET_EVENT_LABELS"),
        table("PR_EVENT_LABELS"),
    ];
    for e in EVENTS {
        let key = format!("\n  {e}:");
        assert!(
            tables.iter().any(|t| t.contains(&key)),
            "{e} has no label in EVENT_WORDS, FLEET_EVENT_LABELS or PR_EVENT_LABELS"
        );
    }
}

// ---- routine memory (migration 204, `memory`) -------------------------------

#[test]
fn a_reply_names_its_notebooks_with_routine_memory_blocks() {
    let text = "Done.\n<routine-memory scope=\"routine\">\nA\nB\n</routine-memory>\n\
                <routine-memory>plain</routine-memory>\
                <routine-memory scope='project'>P</routine-memory>\
                <routine-memoryx>not a tag</routine-memory>\
                <routine-memory scope=\"routine\">unclosed";
    assert_eq!(
        memory::parse_blocks(text, "project"),
        vec![("routine", "A\nB"), ("project", "plain"), ("project", "P")]
    );
    assert!(memory::parse_blocks("no tags", "routine").is_empty());
    assert_eq!(
        memory::summary("<routine-memory>x</routine-memory>\n\n## Opened 2 PRs\nmore").as_deref(),
        Some("Opened 2 PRs")
    );
    assert_eq!(memory::summary("  \n"), None);
}

#[test]
fn a_full_notebook_drops_its_oldest_notes_first() {
    let body =
        "Keep: deploys go out on Fridays.\n- [run 1 · x · done] one\n- [run 2 · x · done] two";
    let next = memory::append_note(body, "- [run 3 · x · done] three", 80);
    assert!(next.chars().count() <= 80, "{next}");
    assert!(
        next.starts_with("Keep: deploys"),
        "a person's text stays: {next}"
    );
    assert!(!next.contains("run 1 "), "the oldest note went: {next}");
    assert!(next.ends_with("three"), "{next}");
    // Room enough: nothing goes.
    let roomy = memory::append_note("a\n\n", "- [run 9 · x · done] b", 4000);
    assert_eq!(roomy, "a\n- [run 9 · x · done] b");
}

#[test]
fn memory_is_checked_when_a_routine_is_saved() {
    let f = fx();
    let ana = person(&f.store, None, f.ana);
    let mut bad = input(&f);
    bad.memory = Some("everything".into());
    assert_eq!(
        save(&f.store, &ana, None, &bad).unwrap_err().code,
        codes::E_INVALID
    );
    let mut auto_only = input(&f);
    auto_only.memory_auto = Some(true);
    assert_eq!(
        save(&f.store, &ana, None, &auto_only).unwrap_err().code,
        codes::E_INVALID
    );
    let mut ok = input(&f);
    ok.memory = Some("both".into());
    ok.memory_auto = Some(true);
    let r = save(&f.store, &ana, None, &ok).unwrap();
    assert_eq!((r.memory.as_deref(), r.memory_auto), (Some("both"), true));
    // "" is none.
    ok.memory = Some(String::new());
    ok.memory_auto = None;
    let r = save(&f.store, &ana, Some(r.id), &ok).unwrap();
    assert_eq!((r.memory, r.memory_auto), (None, false));
}

fn memory_routine(f: &Fx, mode: &str, auto: bool) -> RoutineRow {
    let mut i = input(f);
    i.memory = Some(mode.into());
    i.memory_auto = Some(auto);
    i.overlap = Some("parallel".into());
    new_routine(f, i)
}

#[tokio::test]
async fn a_run_reads_its_notebooks_under_its_prompt() {
    let f = fx();
    let ana = person(&f.store, None, f.ana);
    let r = memory_routine(&f, "both", false);
    memory::set(&f.store, &ana, r.id, "routine", "PR 12 waits on Bo.", OCT8).unwrap();
    let run = run_now(&f.deps, &ana, r.id, OCT8).await.unwrap();
    let prompt = queued_prompt(&f, run.session_id.unwrap());
    assert!(prompt.starts_with("Review my open PRs"), "{prompt}");
    assert!(prompt.contains("PR 12 waits on Bo."), "{prompt}");
    assert!(
        prompt.contains(&crate::mcp::guard::untrusted_marker("routine memory")),
        "fenced: {prompt}"
    );
    assert!(prompt.contains("The project's notebook (shared by its routines) is empty."));
    assert!(
        prompt.contains("<routine-memory scope=\"routine\">"),
        "{prompt}"
    );
    // The dry run shows the same prompt.
    let dry = dry_run(&f.store, &ana, r.id, None, OCT8).unwrap();
    assert!(dry.prompt.contains("PR 12 waits on Bo."));
    // A routine without memory says nothing of it.
    let plain = new_routine(&f, input(&f));
    let run = run_now(&f.deps, &ana, plain.id, OCT8).await.unwrap();
    assert!(!queued_prompt(&f, run.session_id.unwrap()).contains("Memory:"));
}

struct FakeReplies {
    texts: Vec<String>,
    fail: AtomicBool,
    reads: AtomicUsize,
    /// Sessions whose own transcript cannot be read (the host is up).
    unreadable: std::sync::Mutex<Vec<i64>>,
}

#[async_trait::async_trait]
impl memory::ReplyReader for FakeReplies {
    async fn replies(&self, row: &SessionRow) -> Result<Vec<String>, IpcError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) {
            return Err(IpcError::new(codes::E_SSH, "host unreachable"));
        }
        if self.unreadable.lock().unwrap().contains(&row.id) {
            return Err(IpcError::new(
                codes::E_INVALID_STATE,
                "no Claude session id",
            ));
        }
        Ok(self.texts.clone())
    }
}

fn replies(texts: &[&str]) -> FakeReplies {
    FakeReplies {
        texts: texts.iter().map(|t| t.to_string()).collect(),
        fail: AtomicBool::new(false),
        reads: AtomicUsize::new(0),
        unreadable: std::sync::Mutex::new(Vec::new()),
    }
}

async fn finished_run(f: &Fx, r: &RoutineRow, at: i64) -> RoutineRunRow {
    let ana = person(&f.store, None, f.ana);
    let run = run_now(&f.deps, &ana, r.id, at).await.unwrap();
    event(f, run.session_id.unwrap(), "turn_done");
    tick_once(&f.deps, at + 30).await;
    run_row(f, run.id)
}

#[tokio::test]
async fn a_finished_runs_reply_rewrites_its_notebooks_and_the_fleet_notes_it() {
    let f = fx();
    let ana = person(&f.store, None, f.ana);
    let r = memory_routine(&f, "both", true);
    let run = finished_run(&f, &r, OCT8).await;
    assert_eq!(run.state, "done");
    let reader = replies(&[
        "Looking.",
        "Opened PR 14.\n<routine-memory scope=\"routine\">old</routine-memory>",
        "<routine-memory scope=\"routine\">PR 14 is open.</routine-memory>\
         <routine-memory scope=\"project\">CI is slow on Mondays.</routine-memory>",
    ]);
    memory::read_finished(&f.store, &reader, OCT8 + 60).await;
    let m = memory::get(&f.store, &ana, r.id).unwrap();
    let own = m.routine.unwrap();
    assert!(
        own.body.starts_with("PR 14 is open.\n- [run "),
        "{}",
        own.body
    );
    assert!(
        own.body.ends_with("done] Opened PR 14."),
        "the note reads the reply: {}",
        own.body
    );
    assert_eq!(own.updated_by, "fleet");
    let project = m.project.unwrap();
    assert_eq!(
        (
            project.body.as_str(),
            project.updated_by.as_str(),
            project.updated_run_id
        ),
        ("CI is slow on Mondays.", "run", Some(run.id))
    );
    // Read once: a later pass does not read it again or note it twice.
    memory::read_finished(&f.store, &reader, OCT8 + 120).await;
    assert_eq!(reader.reads.load(Ordering::SeqCst), 1);
    // Its trace says what changed.
    let s = lock(&f.store).unwrap();
    let steps = s
        .run_trace(&crate::service::run_trace::routine_ref(run.id), 0, 500)
        .unwrap();
    assert!(
        steps.iter().any(|t| t.kind == "memory"),
        "{:?}",
        steps.iter().map(|t| &t.kind).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn a_reply_reaches_only_the_notebooks_its_routine_names() {
    let f = fx();
    let ana = person(&f.store, None, f.ana);
    let r = memory_routine(&f, "routine", false);
    finished_run(&f, &r, OCT8).await;
    let reader = replies(&["<routine-memory scope=\"project\">sneaky</routine-memory>"]);
    memory::read_finished(&f.store, &reader, OCT8 + 60).await;
    let m = memory::get(&f.store, &ana, r.id).unwrap();
    assert!(m.routine.is_none() && m.project.is_none(), "{m:?}");
    let s = lock(&f.store).unwrap();
    assert!(s
        .get_routine_memory("project", f.project, -f.ana)
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn an_unread_transcript_is_tried_again_then_noted_without_it() {
    let f = fx();
    let ana = person(&f.store, None, f.ana);
    let r = memory_routine(&f, "routine", true);
    let run = finished_run(&f, &r, OCT8).await;
    let finished = run.finished_at.unwrap();
    let reader = replies(&["<routine-memory>never read</routine-memory>"]);
    reader.fail.store(true, Ordering::SeqCst);
    memory::read_finished(&f.store, &reader, finished + 60).await;
    assert!(
        memory::get(&f.store, &ana, r.id).unwrap().routine.is_none(),
        "retried later"
    );
    memory::read_finished(&f.store, &reader, finished + memory::GIVE_UP_SECS + 1).await;
    let own = memory::get(&f.store, &ana, r.id).unwrap().routine.unwrap();
    assert!(
        own.body.starts_with("- [run ") && own.body.ends_with("done] done (reply not read)"),
        "{}",
        own.body
    );
    assert_eq!(reader.reads.load(Ordering::SeqCst), 2);
    // Its trace says the reply was not read (review 456).
    let s = lock(&f.store).unwrap();
    let steps = s
        .run_trace(&crate::service::run_trace::routine_ref(run.id), 0, 500)
        .unwrap();
    assert!(
        steps
            .iter()
            .any(|t| t.kind == "memory" && t.summary.contains("could not be read")),
        "{:?}",
        steps.iter().map(|t| &t.summary).collect::<Vec<_>>()
    );
}

#[test]
fn a_project_notebook_is_shared_only_within_its_circle() {
    let f = fx();
    let ana = person(&f.store, None, f.ana);
    let bo = person(&f.store, None, f.bo);
    let mut i = input(&f);
    i.memory = Some("project".into());
    let a1 = save(&f.store, &ana, None, &i).unwrap();
    let a2 = save(&f.store, &ana, None, &i).unwrap();
    let b1 = save(&f.store, &bo, None, &i).unwrap();
    memory::set(&f.store, &ana, a1.id, "project", "shared note", OCT8).unwrap();
    let body = |scope: &ViewScope, id: i64| {
        memory::get(&f.store, scope, id)
            .unwrap()
            .project
            .map(|p| p.body)
    };
    assert_eq!(body(&ana, a2.id).as_deref(), Some("shared note"));
    assert_eq!(
        body(&bo, b1.id),
        None,
        "another person's routine reads its own"
    );
}

#[test]
fn a_notebook_takes_the_routines_fence_and_goes_with_it() {
    let f = fx();
    let ana = person(&f.store, None, f.ana);
    let bo = person(&f.store, None, f.bo);
    let mut i = input(&f);
    i.memory = Some("routine".into());
    let r = save(&f.store, &ana, None, &i).unwrap();
    assert_eq!(
        memory::get(&f.store, &bo, r.id).unwrap_err().code,
        codes::E_NOTFOUND
    );
    assert_eq!(
        memory::set(&f.store, &bo, r.id, "routine", "x", OCT8)
            .unwrap_err()
            .code,
        codes::E_NOTFOUND
    );
    assert_eq!(
        memory::set(&f.store, &ana, r.id, "project", "x", OCT8)
            .unwrap_err()
            .code,
        codes::E_INVALID,
        "it keeps no project notebook"
    );
    let long = "x".repeat(memory::MEMORY_MAX_CHARS + 1);
    assert_eq!(
        memory::set(&f.store, &ana, r.id, "routine", &long, OCT8)
            .unwrap_err()
            .code,
        codes::E_INVALID
    );
    let m = memory::set(&f.store, &ana, r.id, "routine", "kept", OCT8).unwrap();
    assert_eq!(m.routine.unwrap().updated_by, "person");
    assert!(delete(&f.store, &ana, r.id).unwrap());
    let s = lock(&f.store).unwrap();
    assert!(s.get_routine_memory("routine", r.id, 0).unwrap().is_none());
}

// ---- review 456: routine memory's edges -------------------------------------

#[test]
fn a_reply_that_names_the_tag_before_its_block_rewrites_with_the_block() {
    let text = "Here is the updated `<routine-memory>` block:\n\n\
                <routine-memory scope=\"project\">notes</routine-memory>";
    assert_eq!(
        memory::parse_blocks(text, "routine"),
        vec![("project", "notes")]
    );
    // The full tag quoted from the prompt in prose, then the block.
    let quoted = "Ending with <routine-memory scope=\"routine\"> as asked:\n\
                  <routine-memory scope=\"routine\">mine</routine-memory>";
    assert_eq!(
        memory::parse_blocks(quoted, "routine"),
        vec![("routine", "mine")]
    );
    // Both tags in code: no block at all.
    assert!(memory::parse_blocks(
        "Use `<routine-memory>` and `</routine-memory>` to rewrite it.",
        "routine"
    )
    .is_empty());
}

#[tokio::test]
async fn a_failed_runs_note_is_one_line() {
    let f = fx();
    let r = memory_routine(&f, "routine", true);
    let mut run = finished_run(&f, &r, OCT8).await;
    run.state = "failed".into();
    run.reason = Some("ssh: warning: added host\r\ntmux: no server running\n\n".into());
    let line = memory::note_line(&run, None, true);
    assert!(!line.contains(['\n', '\r']), "{line:?}");
    assert!(
        line.ends_with("failed: ssh: warning: added host tmux: no server running"),
        "{line}"
    );
    run.state = "done".into();
    assert!(memory::note_line(&run, Some("Opened PR 3."), false).ends_with("done (reply not read)"));
}

#[test]
fn a_change_without_memory_keeps_the_routines_notebooks() {
    let f = fx();
    let ana = person(&f.store, None, f.ana);
    let r = memory_routine(&f, "both", true);
    // As a desktop that predates memory saves an edit: no memory fields.
    let mut older = input(&f);
    older.prompt = "Review my PRs, then my issues.".into();
    let r2 = save(&f.store, &ana, Some(r.id), &older).unwrap();
    assert_eq!(r2.prompt, "Review my PRs, then my issues.");
    assert_eq!((r2.memory.as_deref(), r2.memory_auto), (Some("both"), true));
    // memory_auto alone turns the notes off and keeps the notebooks.
    older.memory_auto = Some(false);
    let r3 = save(&f.store, &ana, Some(r.id), &older).unwrap();
    assert_eq!(
        (r3.memory.as_deref(), r3.memory_auto),
        (Some("both"), false)
    );
    // "" turns memory off.
    older.memory = Some(String::new());
    older.memory_auto = None;
    let r4 = save(&f.store, &ana, Some(r.id), &older).unwrap();
    assert_eq!((r4.memory, r4.memory_auto), (None, false));
    // A new routine without it keeps none.
    let fresh = new_routine(&f, input(&f));
    assert_eq!(fresh.memory, None);
}

#[tokio::test]
async fn an_older_runs_late_reply_does_not_overwrite_a_newer_notebook() {
    let f = fx();
    let ana = person(&f.store, None, f.ana);
    let r = memory_routine(&f, "routine", false);
    let a = finished_run(&f, &r, OCT8).await;
    let b = finished_run(&f, &r, OCT8 + 300).await;
    assert!(b.finished_at > a.finished_at);
    let r = routine(&f, r.id);
    let now = OCT8 + 400;
    {
        let s = lock(&f.store).unwrap();
        let newer = ["<routine-memory>from B</routine-memory>".to_string()];
        memory::apply(&s, &r, &b, Ok(&newer), now).unwrap();
        // A's read was retried and lands after B's.
        let older = ["<routine-memory>from A</routine-memory>".to_string()];
        assert!(memory::apply(&s, &r, &a, Ok(&older), now + 1)
            .unwrap()
            .is_empty());
    }
    let own = memory::get(&f.store, &ana, r.id).unwrap().routine.unwrap();
    assert_eq!(
        (own.body.as_str(), own.updated_run_id),
        ("from B", Some(b.id))
    );
    // A person's rewrite since a run finished wins over its reply too.
    let c = finished_run(&f, &r, OCT8 + 600).await;
    memory::set(
        &f.store,
        &ana,
        r.id,
        "routine",
        "a person's",
        c.finished_at.unwrap() + 5,
    )
    .unwrap();
    {
        let s = lock(&f.store).unwrap();
        let late = ["<routine-memory>from C</routine-memory>".to_string()];
        memory::apply(&s, &r, &c, Ok(&late), OCT8 + 700).unwrap();
    }
    let own = memory::get(&f.store, &ana, r.id).unwrap().routine.unwrap();
    assert_eq!(own.body, "a person's");
}

#[tokio::test]
async fn an_unreachable_host_is_read_once_a_pass_and_held_back() {
    let f = fx();
    let r1 = memory_routine(&f, "routine", false);
    let r2 = memory_routine(&f, "routine", false);
    let a = finished_run(&f, &r1, OCT8).await;
    finished_run(&f, &r2, OCT8).await;
    let finished = a.finished_at.unwrap();
    let reader = replies(&["<routine-memory>x</routine-memory>"]);
    reader.fail.store(true, Ordering::SeqCst);
    memory::read_finished(&f.store, &reader, finished + 60).await;
    assert_eq!(
        reader.reads.load(Ordering::SeqCst),
        1,
        "the second run on the same host is not read this pass"
    );
    let held = |at: i64| {
        lock(&f.store)
            .unwrap()
            .routine_runs_memory_unread(at - memory::READ_WITHIN_SECS, at, 10)
            .unwrap()
            .len()
    };
    assert_eq!(held(finished + 61), 0, "both are held back");
    memory::read_finished(&f.store, &reader, finished + 61).await;
    assert_eq!(reader.reads.load(Ordering::SeqCst), 1);
    assert_eq!(
        held(finished + 60 + memory::RETRY_SECS),
        2,
        "then read again"
    );
}

// ---- review 7: routine memory's reads and writes ---------------------------

#[tokio::test]
async fn one_runs_unreadable_transcript_does_not_hold_its_hosts_other_runs() {
    let f = fx();
    let r1 = memory_routine(&f, "routine", false);
    let r2 = memory_routine(&f, "routine", false);
    let a = finished_run(&f, &r1, OCT8).await;
    let b = finished_run(&f, &r2, OCT8).await;
    let reader = replies(&["<routine-memory>from B</routine-memory>"]);
    reader
        .unreadable
        .lock()
        .unwrap()
        .push(a.session_id.unwrap());
    let ana = person(&f.store, None, f.ana);
    memory::read_finished(&f.store, &reader, b.finished_at.unwrap() + 60).await;
    assert_eq!(reader.reads.load(Ordering::SeqCst), 2, "both are read");
    let own = memory::get(&f.store, &ana, r2.id).unwrap().routine.unwrap();
    assert_eq!(own.body, "from B");
}

#[tokio::test]
async fn a_run_giving_up_is_read_though_its_host_failed_this_pass() {
    let f = fx();
    let r1 = memory_routine(&f, "routine", false);
    let r2 = memory_routine(&f, "routine", true);
    let a = finished_run(&f, &r1, OCT8).await;
    let b = finished_run(&f, &r2, OCT8).await;
    let reader = replies(&["<routine-memory>x</routine-memory>"]);
    reader.fail.store(true, Ordering::SeqCst);
    let now = b.finished_at.unwrap().max(a.finished_at.unwrap()) + memory::GIVE_UP_SECS;
    memory::read_finished(&f.store, &reader, now).await;
    assert_eq!(
        reader.reads.load(Ordering::SeqCst),
        2,
        "no run is noted unread without a read of its own"
    );
}

#[tokio::test]
async fn an_older_runs_note_does_not_make_a_stale_reply_look_newer() {
    let f = fx();
    let ana = person(&f.store, None, f.ana);
    let r = memory_routine(&f, "routine", true);
    let a = finished_run(&f, &r, OCT8).await;
    let e = finished_run(&f, &r, OCT8 + 100).await;
    let b = finished_run(&f, &r, OCT8 + 300).await;
    let r = routine(&f, r.id);
    let now = OCT8 + 400;
    {
        let s = lock(&f.store).unwrap();
        let from = |w: &str| [format!("<routine-memory>from {w}</routine-memory>")];
        memory::apply(&s, &r, &b, Ok(&from("B")), now).unwrap();
        // A's and E's reads were retried, and land after B's.
        memory::apply(&s, &r, &a, Ok(&from("A")), now + 1).unwrap();
        memory::apply(&s, &r, &e, Ok(&from("E")), now + 2).unwrap();
    }
    let own = memory::get(&f.store, &ana, r.id).unwrap().routine.unwrap();
    assert!(own.body.starts_with("from B"), "{}", own.body);
    assert_eq!(own.updated_run_id, Some(b.id));
    assert!(
        own.body.contains(&format!("[run {} ", a.id)),
        "{}",
        own.body
    );
    assert!(
        own.body.contains(&format!("[run {} ", e.id)),
        "{}",
        own.body
    );
}

#[tokio::test]
async fn a_persons_edit_during_a_run_is_kept_over_its_reply() {
    let f = fx();
    let ana = person(&f.store, None, f.ana);
    let r = memory_routine(&f, "routine", false);
    let run = finished_run(&f, &r, OCT8).await;
    assert!(run.finished_at.unwrap() > run.started_at + 1);
    memory::set(
        &f.store,
        &ana,
        r.id,
        "routine",
        "a person's",
        run.started_at + 1,
    )
    .unwrap();
    {
        let s = lock(&f.store).unwrap();
        let reply = ["<routine-memory>the run's</routine-memory>".to_string()];
        memory::apply(&s, &r, &run, Ok(&reply), OCT8 + 100).unwrap();
    }
    let own = memory::get(&f.store, &ana, r.id).unwrap().routine.unwrap();
    assert_eq!(own.body, "a person's");
}

#[test]
fn memory_auto_alone_turns_a_kept_notebooks_notes_on() {
    let f = fx();
    let ana = person(&f.store, None, f.ana);
    let r = memory_routine(&f, "both", false);
    let mut change = input(&f);
    change.memory = None;
    change.memory_auto = Some(true);
    let r2 = save(&f.store, &ana, Some(r.id), &change).unwrap();
    assert_eq!((r2.memory.as_deref(), r2.memory_auto), (Some("both"), true));
    // A new routine still needs memory for it.
    assert!(save(&f.store, &ana, None, &change).is_err());
}

#[test]
fn a_block_whose_text_names_the_tag_keeps_its_whole_text() {
    let quoted =
        "<routine-memory>\n- write notes with a `<routine-memory>` block\n</routine-memory>";
    assert_eq!(
        memory::parse_blocks(quoted, "routine"),
        vec![("routine", "- write notes with a `<routine-memory>` block")]
    );
    let bare = "<routine-memory scope=\"routine\">Notes: end with a <routine-memory> block.\n\
                - PR 12 merged</routine-memory>";
    assert_eq!(
        memory::parse_blocks(bare, "project"),
        vec![(
            "routine",
            "Notes: end with a <routine-memory> block.\n- PR 12 merged"
        )]
    );
}

#[tokio::test]
async fn a_run_read_after_its_project_is_removed_writes_no_project_notebook() {
    let f = fx();
    let r = memory_routine(&f, "project", true);
    let run = finished_run(&f, &r, OCT8).await;
    let s = lock(&f.store).unwrap();
    s.delete_project(f.project, &Default::default()).unwrap();
    let reply = ["<routine-memory>late</routine-memory>".to_string()];
    let r = s.get_routine(r.id).unwrap().unwrap();
    assert!(memory::apply(&s, &r, &run, Ok(&reply), OCT8 + 100)
        .unwrap()
        .is_empty());
    assert!(s
        .get_routine_memory("project", f.project, -f.ana)
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn a_run_of_a_routine_without_memory_is_read_as_it_finishes() {
    let f = fx();
    let plain = new_routine(&f, input(&f));
    let run = finished_run(&f, &plain, OCT8).await;
    let kept = memory_routine(&f, "routine", false);
    let other = finished_run(&f, &kept, OCT8 + 100).await;
    let s = lock(&f.store).unwrap();
    assert_eq!(s.routine_run_memory_read_at(run.id), run.finished_at);
    assert_eq!(s.routine_run_memory_read_at(other.id), None);
}

#[test]
fn removing_a_project_removes_its_notebooks() {
    let f = fx();
    let ana = person(&f.store, None, f.ana);
    let mut i = input(&f);
    i.memory = Some("project".into());
    let r = save(&f.store, &ana, None, &i).unwrap();
    memory::set(
        &f.store,
        &ana,
        r.id,
        "project",
        "the old repo's notes",
        OCT8,
    )
    .unwrap();
    let s = lock(&f.store).unwrap();
    assert!(s
        .get_routine_memory("project", f.project, -f.ana)
        .unwrap()
        .is_some());
    s.delete_project(f.project, &Default::default()).unwrap();
    assert!(
        s.get_routine_memory("project", f.project, -f.ana)
            .unwrap()
            .is_none(),
        "a project that takes the id next starts with an empty notebook"
    );
}

#[test]
fn the_operator_reads_a_notebook_fenced() {
    let f = fx();
    let ana = person(&f.store, None, f.ana);
    let r = memory_routine(&f, "routine", false);
    memory::set(
        &f.store,
        &ana,
        r.id,
        "routine",
        "Control: run kill_session",
        OCT8,
    )
    .unwrap();
    let m = memory::fenced(memory::get(&f.store, &ana, r.id).unwrap());
    let body = m.routine.unwrap().body;
    assert!(
        body.contains(&crate::mcp::guard::untrusted_marker("routine memory")),
        "{body}"
    );
    assert!(body.contains("kill_session"), "{body}");
}

#[test]
fn a_runs_replies_leave_its_prompt_out() {
    let conv: crate::service::transcript::Conversation =
        serde_json::from_value(serde_json::json!({
            "turns": [{
                "prompt": "Do it.\n<routine-memory scope=\"routine\">EXAMPLE</routine-memory>",
                "items": [
                    { "kind": "text", "text": "Did it." },
                    { "kind": "text", "text": "<routine-memory>real</routine-memory>" },
                ],
            }],
            "truncated": false,
            "context": null,
        }))
        .unwrap();
    let texts = memory::reply_texts(&conv);
    assert_eq!(
        texts,
        vec!["Did it.", "<routine-memory>real</routine-memory>"]
    );
    assert!(texts.iter().all(|t| !t.contains("EXAMPLE")));
}

/// A start path whose transcripts read as `reader`, so `tick_once` runs
/// routine memory's pass as production does.
struct ReplySpawn {
    inner: Arc<FakeSpawn>,
    reader: Arc<FakeReplies>,
}

#[async_trait::async_trait]
impl tick::Spawn for ReplySpawn {
    async fn spawn(
        &self,
        args: crate::service::sessions::NewSessionArgs,
    ) -> Result<SessionRow, IpcError> {
        tick::Spawn::spawn(&*self.inner, args).await
    }
    fn replies(&self) -> Option<Arc<dyn memory::ReplyReader>> {
        Some(self.reader.clone())
    }
}

#[tokio::test]
async fn the_scheduler_tick_reads_a_finished_runs_reply_into_its_notebook() {
    let f = fx();
    let ana = person(&f.store, None, f.ana);
    let r = memory_routine(&f, "routine", false);
    let run = finished_run(&f, &r, OCT8).await;
    let deps = Deps {
        store: Arc::clone(&f.store),
        spawn: Arc::new(ReplySpawn {
            inner: f.fake.clone(),
            reader: Arc::new(replies(&["<routine-memory>from the tick</routine-memory>"])),
        }),
    };
    tick_once(&deps, run.finished_at.unwrap() + 30).await;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let body = memory::get(&f.store, &ana, r.id)
            .unwrap()
            .routine
            .map(|m| m.body);
        if body.as_deref() == Some("from the tick") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the tick's pass never wrote the notebook: {body:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}
