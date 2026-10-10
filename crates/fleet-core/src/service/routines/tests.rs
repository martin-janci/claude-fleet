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
    let mut i = input(&f);
    i.trigger = "event".into();
    i.cron = None;
    i.event = Some("stuck".into());
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
    let mut i = input(&f);
    i.trigger = "event".into();
    i.cron = None;
    i.event = Some("stuck".into());
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
    let mut i = input(&f);
    i.trigger = "event".into();
    i.cron = None;
    i.event = Some("stuck".into());
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
        host_alias: Some("mac".into()),
        client: None,
        mode: TokenMode::Full,
        pane: None,
        is_personal_owner: false,
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

fn pr_routine(f: &Fx, event: &str, change: impl FnOnce(&mut RoutineInput)) -> RoutineRow {
    let mut i = input(f);
    i.trigger = "event".into();
    i.cron = None;
    i.event = Some(event.into());
    i.overlap = Some("parallel".into());
    change(&mut i);
    new_routine(f, i)
}

fn queued_prompt(f: &Fx, sid: i64) -> String {
    let s = lock(&f.store).unwrap();
    let rows = s.undelivered_handovers(sid).unwrap();
    rows[0].body.clone().unwrap_or_default()
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
    assert!(
        kinds(&f).is_empty(),
        "an open PR with nothing new is no event"
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
        ["pr_ci_failed", "pr_review", "pr_ci_passed", "pr_merged"],
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
    assert_eq!(kinds(&f).len(), 4);
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
    assert_eq!(
        runs[1].trigger_ref.as_deref(),
        Some(&*format!("retry:{}", failed.id))
    );
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
    let mut i = input(&f);
    i.retry_once = Some(true);
    let r = new_routine(&f, i);
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
    assert_eq!(
        runs[1].trigger_ref.as_deref(),
        Some(&*format!("retry:{}", first.id))
    );
    assert_eq!(runs[1].state, "failed");
    // Without retry_once a failure stays as it is.
    let plain = new_routine(&f, input(&f));
    run_now(&f.deps, &ana, plain.id, OCT8 + 100).await.unwrap();
    tick_once(&f.deps, OCT8 + 120).await;
    assert_eq!(runs_of(&f, plain.id).len(), 1);
    // A failure older than the retry window is not started again.
    let mut late = input(&f);
    late.retry_once = Some(true);
    let late = new_routine(&f, late);
    run_now(&f.deps, &ana, late.id, OCT8).await.unwrap();
    tick_once(&f.deps, OCT8 + tick::RETRY_WITHIN_SECS + 60).await;
    assert_eq!(runs_of(&f, late.id).len(), 1);
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
