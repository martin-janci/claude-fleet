use super::*;
use crate::service::outcome::{CheckSummary, FailingCheck};
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};

const NOW: i64 = 1_000_000;
const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn evidence(merge_state: &str, failing: &[&str]) -> PrEvidence {
    PrEvidence {
        head_oid: Some(HEAD.into()),
        local_head: Some(HEAD.into()),
        ahead: Some(0),
        merge_state: Some(merge_state.into()),
        state: Some("OPEN".into()),
        checks: CheckSummary {
            total: 3,
            failing: failing
                .iter()
                .map(|n| FailingCheck {
                    name: (*n).into(),
                    url: None,
                })
                .collect(),
            failing_total: failing.len() as u32,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn cand(id: i64, ev: PrEvidence) -> Candidate {
    Candidate {
        session_id: id,
        host_alias: "local".into(),
        tmux_name: format!("s{id}"),
        project_id: 1,
        org_id: None,
        pr_url: Some(format!("https://github.com/o/r/pull/{id}")),
        evidence: ev,
        live: true,
        busy: false,
        idle_since: Some(NOW - 600),
        is_controller: false,
    }
}

fn rules(level: &str) -> HashMap<i64, ShepherdRuleRow> {
    HashMap::from([(
        1,
        ShepherdRuleRow {
            project_id: 1,
            level: level.into(),
            granted_by: "console".into(),
            granted_at: 0,
            expires_at: None,
            recipes: Some("REGEN_DOCS=1 cargo fleet-test -- reference_is_current".into()),
        },
    )])
}

#[test]
fn the_condition_is_the_first_of_conflict_behind_red() {
    assert_eq!(
        condition_of(&evidence("DIRTY", &["lint"])),
        Some(Condition::Conflict)
    );
    assert_eq!(
        condition_of(&evidence("BEHIND", &["lint"])),
        Some(Condition::Behind)
    );
    assert_eq!(
        condition_of(&evidence("UNSTABLE", &["lint"])),
        Some(Condition::CiRed)
    );
    assert_eq!(condition_of(&evidence("CLEAN", &[])), None);
    let mut merged = evidence("DIRTY", &["lint"]);
    merged.state = Some("MERGED".into());
    assert_eq!(condition_of(&merged), None);
}

#[test]
fn no_rule_means_nothing_happens() {
    let c = [cand(1, evidence("DIRTY", &[]))];
    assert!(plan(&c, &HashMap::new(), &Seen::default(), NOW).is_empty());
}

#[test]
fn watch_records_and_sends_nothing() {
    let c = [cand(1, evidence("DIRTY", &[]))];
    let p = plan(&c, &rules("watch"), &Seen::default(), NOW);
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].action, Action::Record("watched".into()));
}

#[test]
fn nudge_sends_the_conflict_prompt_with_the_recipes() {
    let c = [cand(1, evidence("DIRTY", &[]))];
    let p = plan(&c, &rules("nudge"), &Seen::default(), NOW);
    let Action::Nudge(prompt) = &p[0].action else {
        panic!("expected a nudge, got {:?}", p[0].action)
    };
    assert!(prompt.contains("merge conflict"));
    assert!(prompt.contains("REGEN_DOCS=1"));
    assert_eq!(p[0].condition, Condition::Conflict);
}

#[test]
fn a_seen_episode_is_not_acted_on_twice() {
    let c = [cand(1, evidence("DIRTY", &[]))];
    let mut seen = Seen::default();
    seen.episodes.insert((1, HEAD.into(), "conflict"));
    assert!(plan(&c, &rules("nudge"), &seen, NOW).is_empty());
    // A new push is a new episode.
    let mut ev = evidence("DIRTY", &[]);
    ev.head_oid = Some("b".repeat(40));
    ev.local_head = ev.head_oid.clone();
    assert_eq!(plan(&[cand(1, ev)], &rules("nudge"), &seen, NOW).len(), 1);
}

#[test]
fn a_busy_or_freshly_idle_session_is_left_for_a_later_tick() {
    let mut busy = cand(1, evidence("DIRTY", &[]));
    busy.busy = true;
    let mut fresh = cand(2, evidence("DIRTY", &[]));
    fresh.idle_since = Some(NOW - 5);
    let mut never_idle = cand(3, evidence("DIRTY", &[]));
    never_idle.idle_since = None;
    assert!(plan(
        &[busy, fresh, never_idle],
        &rules("nudge"),
        &Seen::default(),
        NOW
    )
    .is_empty());
}

#[test]
fn unpushed_local_work_waits() {
    let mut ahead = evidence("UNSTABLE", &["test"]);
    ahead.ahead = Some(2);
    let mut moved = evidence("UNSTABLE", &["test"]);
    moved.local_head = Some("c".repeat(40));
    assert!(plan(
        &[cand(1, ahead), cand(2, moved)],
        &rules("nudge"),
        &Seen::default(),
        NOW
    )
    .is_empty());
}

#[test]
fn the_budget_the_controller_and_a_dead_session_are_recorded_as_skips() {
    let mut seen = Seen::default();
    seen.nudges.insert(1, MAX_NUDGES_PER_DAY);
    let mut ctl = cand(2, evidence("DIRTY", &[]));
    ctl.is_controller = true;
    let mut dead = cand(3, evidence("DIRTY", &[]));
    dead.live = false;
    let p = plan(
        &[cand(1, evidence("DIRTY", &[])), ctl, dead],
        &rules("merge"),
        &seen,
        NOW,
    );
    let got: Vec<_> = p.iter().map(|p| p.action.clone()).collect();
    assert_eq!(
        got,
        vec![
            Action::Record("skipped:budget".into()),
            Action::Record("skipped:controller".into()),
            Action::Record("skipped:no_session".into()),
        ]
    );
}

#[test]
fn a_reading_without_a_head_is_ignored() {
    let mut ev = evidence("DIRTY", &[]);
    ev.head_oid = None;
    assert!(plan(&[cand(1, ev)], &rules("nudge"), &Seen::default(), NOW).is_empty());
}

#[test]
fn an_expired_rule_is_not_in_force() {
    let mut r = rules("nudge").remove(&1).unwrap();
    r.expires_at = Some(NOW);
    assert!(!r.active_at(NOW));
    r.expires_at = Some(NOW + 1);
    assert!(r.active_at(NOW));
}

// --- the runner against a real store ----------------------------------------

struct FakeExec {
    sent: AtomicUsize,
    attached: bool,
    fail: bool,
    last: Mutex<String>,
    merged: AtomicUsize,
    refuse: Option<&'static str>,
    merge_fails: bool,
    triaged: Mutex<Vec<(i64, Condition)>>,
}

impl FakeExec {
    fn new() -> Self {
        FakeExec {
            sent: AtomicUsize::new(0),
            attached: false,
            fail: false,
            last: Mutex::new(String::new()),
            merged: AtomicUsize::new(0),
            refuse: None,
            merge_fails: false,
            triaged: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl ShepherdExec for FakeExec {
    async fn nudge(&self, _h: &str, _t: &str, prompt: &str) -> Result<NudgeOutcome, IpcError> {
        if self.attached {
            return Ok(NudgeOutcome::Attached);
        }
        if self.fail {
            return Err(IpcError::new(crate::ipc_error::codes::E_TMUX, "no pane"));
        }
        self.sent.fetch_add(1, Ordering::SeqCst);
        *self.last.lock().unwrap() = prompt.to_string();
        Ok(NudgeOutcome::Sent)
    }

    async fn merge_if_green(
        &self,
        _h: &str,
        url: &str,
        sha: &str,
    ) -> Result<MergeOutcome, IpcError> {
        merge::merge_script(url, sha)?;
        if let Some(why) = self.refuse {
            return Ok(MergeOutcome::Refused(why.into()));
        }
        if self.merge_fails {
            return Err(IpcError::new(
                crate::ipc_error::codes::E_SHELL,
                "head moved",
            ));
        }
        self.merged.fetch_add(1, Ordering::SeqCst);
        Ok(MergeOutcome::Merged)
    }

    async fn triage(&self, id: i64, _org: Option<i64>, c: Condition, _ev: &PrEvidence) {
        self.triaged.lock().unwrap().push((id, c));
    }
}

/// A store with one running, idle session in project `o/r` whose PR reads
/// `ev`. Returns the store, the project id and the session id.
fn seeded(ev: &PrEvidence) -> (Mutex<Store>, i64, i64) {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("local").unwrap();
    let pid = s.upsert_project("o", "r", "/p/o/r").unwrap();
    let id = s
        .upsert_session("s1", "local", Some(pid), None, 1, 1, "running", None)
        .unwrap();
    s.conn_for_test()
        .execute(
            "UPDATE sessions SET idle_since = ?1, pr_evidence = ?2, \
             pr_url = 'https://github.com/o/r/pull/9' WHERE id = ?3",
            rusqlite::params![NOW - 600, serde_json::to_string(ev).unwrap(), id],
        )
        .unwrap();
    (Mutex::new(s), pid, id)
}

fn grant(store: &Mutex<Store>, pid: i64, level: &str) {
    store
        .lock()
        .unwrap()
        .grant_shepherd_rule(&ShepherdRuleRow {
            project_id: pid,
            level: level.into(),
            granted_by: "console".into(),
            granted_at: NOW,
            expires_at: None,
            recipes: None,
        })
        .unwrap();
}

#[tokio::test]
async fn the_runner_nudges_once_per_episode_and_records_it() {
    let (store, pid, id) = seeded(&evidence("UNSTABLE", &["clippy"]));
    let exec = FakeExec::new();
    assert_eq!(
        run_with(&store, &exec, NOW).await,
        0,
        "no rule, no shepherd"
    );
    grant(&store, pid, "nudge");
    assert_eq!(run_with(&store, &exec, NOW).await, 1);
    assert_eq!(run_with(&store, &exec, NOW + 20).await, 0);
    assert_eq!(exec.sent.load(Ordering::SeqCst), 1);
    assert!(exec.last.lock().unwrap().contains("clippy"));
    assert_eq!(
        *exec.triaged.lock().unwrap(),
        vec![(id, Condition::CiRed)],
        "Jev's triage is asked once per recorded episode"
    );
    let s = store.lock().unwrap();
    assert_eq!(
        s.shepherd_episode_outcome(id, HEAD, "ci_red").unwrap(),
        Some("nudged".into())
    );
    assert_eq!(s.count_shepherd_nudges_since(id, 0).unwrap(), 1);
    let events = s.list_session_events(id, 50).unwrap();
    assert!(events
        .iter()
        .any(|e| e.kind == "pr_shepherd" && e.detail.as_deref() == Some("ci_red:nudged")));
}

/// Redesign 8.1: Pause all stops the shepherd before it nudges or merges.
#[tokio::test]
async fn pause_all_stops_the_shepherd() {
    let (store, pid, id) = seeded(&evidence("UNSTABLE", &["clippy"]));
    grant(&store, pid, "nudge");
    crate::service::settings::set(
        &store.lock().unwrap(),
        crate::service::settings::AUTOMATION_PAUSED,
        "true",
    )
    .unwrap();
    let exec = FakeExec::new();
    assert_eq!(run_with(&store, &exec, NOW).await, 0);
    assert_eq!(exec.sent.load(Ordering::SeqCst), 0);
    let s = store.lock().unwrap();
    assert_eq!(
        s.shepherd_episode_outcome(id, HEAD, "ci_red").unwrap(),
        None
    );
}

#[tokio::test]
async fn an_attached_pane_leaves_the_episode_open() {
    let (store, pid, id) = seeded(&evidence("DIRTY", &[]));
    grant(&store, pid, "nudge");
    let mut exec = FakeExec::new();
    exec.attached = true;
    assert_eq!(run_with(&store, &exec, NOW).await, 0);
    assert_eq!(
        store
            .lock()
            .unwrap()
            .shepherd_episode_outcome(id, HEAD, "conflict")
            .unwrap(),
        None
    );
    exec.attached = false;
    assert_eq!(run_with(&store, &exec, NOW + 20).await, 1);
}

#[tokio::test]
async fn a_failed_send_is_recorded_and_counts_against_the_budget() {
    let (store, pid, id) = seeded(&evidence("DIRTY", &[]));
    grant(&store, pid, "nudge");
    let mut exec = FakeExec::new();
    exec.fail = true;
    assert_eq!(run_with(&store, &exec, NOW).await, 1);
    let s = store.lock().unwrap();
    assert_eq!(
        s.shepherd_episode_outcome(id, HEAD, "conflict").unwrap(),
        Some("failed:no pane".into())
    );
    assert_eq!(s.count_shepherd_nudges_since(id, 0).unwrap(), 1);
}

#[tokio::test]
async fn revoking_every_rule_stops_the_shepherd() {
    let (store, pid, _) = seeded(&evidence("DIRTY", &[]));
    grant(&store, pid, "nudge");
    assert_eq!(
        store.lock().unwrap().revoke_all_shepherd_rules().unwrap(),
        1
    );
    let exec = FakeExec::new();
    assert_eq!(run_with(&store, &exec, NOW).await, 0);
    assert_eq!(exec.sent.load(Ordering::SeqCst), 0);
}

#[test]
fn a_rule_refuses_an_unknown_level_and_long_recipes() {
    let s = Store::open_in_memory().unwrap();
    let pid = s.upsert_project("o", "r", "/p/o/r").unwrap();
    let mut r = ShepherdRuleRow {
        project_id: pid,
        level: "auto".into(),
        granted_by: "console".into(),
        granted_at: 0,
        expires_at: None,
        recipes: None,
    };
    assert!(s.grant_shepherd_rule(&r).is_err());
    r.level = "watch".into();
    r.recipes = Some("x".repeat(crate::store::SHEPHERD_RECIPES_MAX_CHARS + 1));
    assert!(s.grant_shepherd_rule(&r).is_err());
    r.recipes = None;
    s.grant_shepherd_rule(&r).unwrap();
    r.level = "merge".into();
    s.grant_shepherd_rule(&r).unwrap();
    assert_eq!(s.list_shepherd_rules().unwrap(), vec![r]);
    assert!(s.revoke_shepherd_rule(pid).unwrap());
    assert!(!s.revoke_shepherd_rule(pid).unwrap());
}

// --- the merge queue ----------------------------------------------------------

fn green(id: i64, pr: u64) -> Candidate {
    let mut c = cand(id, evidence("CLEAN", &[]));
    c.evidence.head_oid = Some(format!("{id:0>40}"));
    c.evidence.local_head = c.evidence.head_oid.clone();
    c.pr_url = Some(format!("https://github.com/o/r/pull/{pr}"));
    c
}

#[test]
fn only_a_merge_rule_merges_and_the_lowest_pr_goes_first() {
    let c = [green(1, 30), green(2, 12), green(3, 20)];
    let none = HashSet::new();
    let never = HashMap::new();
    assert!(merge::plan_merges(&c, &rules("nudge"), &none, &never, NOW).is_empty());
    let p = merge::plan_merges(&c, &rules("merge"), &none, &never, NOW);
    assert_eq!(p.len(), 1, "one per project");
    assert_eq!(p[0].pr_url, "https://github.com/o/r/pull/12");
    assert_eq!(p[0].session_id, 2);
}

#[test]
fn the_queue_waits_out_the_spacing_and_skips_tried_heads() {
    let c = [green(1, 5), green(2, 6)];
    let last = HashMap::from([(1, NOW - merge::MERGE_SPACING_SECS + 1)]);
    assert!(merge::plan_merges(&c, &rules("merge"), &HashSet::new(), &last, NOW).is_empty());
    let last = HashMap::from([(1, NOW - merge::MERGE_SPACING_SECS)]);
    let tried = HashSet::from([(1, format!("{:0>40}", 1))]);
    let p = merge::plan_merges(&c, &rules("merge"), &tried, &last, NOW);
    assert_eq!(p[0].session_id, 2);
}

#[test]
fn a_pr_that_is_not_fully_green_is_not_planned() {
    let mut pending = green(1, 1);
    pending.evidence.checks.pending = 1;
    let mut draft = green(2, 2);
    draft.evidence.draft = true;
    let mut changes = green(3, 3);
    changes.evidence.review_decision = Some("CHANGES_REQUESTED".into());
    let mut red = green(4, 4);
    red.evidence.checks.failing_total = 1;
    let mut blocked = green(5, 5);
    blocked.evidence.merge_state = Some("BLOCKED".into());
    let mut no_checks = green(6, 6);
    no_checks.evidence.checks.total = 0;
    let mut unpushed = green(7, 7);
    unpushed.evidence.ahead = Some(1);
    let mut bad_url = green(8, 8);
    bad_url.pr_url = Some("https://github.com/o/r/pull/8;x".into());
    let c = [
        pending, draft, changes, red, blocked, no_checks, unpushed, bad_url,
    ];
    assert!(
        merge::plan_merges(&c, &rules("merge"), &HashSet::new(), &HashMap::new(), NOW).is_empty()
    );
}

#[tokio::test]
async fn the_runner_merges_once_and_spaces_the_next() {
    let (store, pid, id) = seeded(&evidence("CLEAN", &[]));
    grant(&store, pid, "merge");
    let exec = FakeExec::new();
    assert_eq!(run_with(&store, &exec, NOW).await, 1);
    assert_eq!(run_with(&store, &exec, NOW + 20).await, 0);
    assert_eq!(exec.merged.load(Ordering::SeqCst), 1);
    let s = store.lock().unwrap();
    assert_eq!(s.last_shepherd_merge_at(pid).unwrap(), Some(NOW));
    assert!(s
        .list_session_events(id, 50)
        .unwrap()
        .iter()
        .any(|e| e.kind == "pr_shepherd" && e.detail.as_deref() == Some("merge:merged")));
}

#[tokio::test]
async fn a_refusal_is_retried_after_a_probe_period_and_a_failure_is_not() {
    let (store, pid, id) = seeded(&evidence("CLEAN", &[]));
    grant(&store, pid, "merge");
    let mut exec = FakeExec::new();
    exec.refuse = Some("checks");
    assert_eq!(run_with(&store, &exec, NOW).await, 1);
    assert_eq!(run_with(&store, &exec, NOW + 20).await, 0, "held");
    assert_eq!(
        store.lock().unwrap().last_shepherd_merge_at(pid).unwrap(),
        None,
        "a refusal is not a merge"
    );
    exec.refuse = None;
    exec.merge_fails = true;
    let later = NOW + merge::REFUSAL_RETRY_SECS + 1;
    assert_eq!(run_with(&store, &exec, later).await, 1);
    assert_eq!(
        run_with(&store, &exec, later + 10_000).await,
        0,
        "a failure is final"
    );
    let events = store.lock().unwrap().list_session_events(id, 50).unwrap();
    assert!(events
        .iter()
        .any(|e| e.detail.as_deref() == Some("merge:failed:head moved")));
}
