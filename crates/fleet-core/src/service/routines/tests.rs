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
}

#[async_trait::async_trait]
impl tick::Spawn for FakeSpawn {
    async fn spawn(
        &self,
        args: crate::service::sessions::NewSessionArgs,
    ) -> Result<SessionRow, IpcError> {
        if self.fail.load(Ordering::SeqCst) {
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
        .advance_routine(id, Some(at))
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
