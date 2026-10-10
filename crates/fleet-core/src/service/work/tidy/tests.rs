//! `tidy_apply` as a batch against a fake executor, and the GC sweep's
//! auto-tidy pass (only the allowed reasons; nothing when it is off).

use super::*;
use crate::service::gc::{sweep_with, GcConfig, GcReport};
use crate::service::safe_kill::{DirtyFile, SafeKillInspection};
use std::sync::atomic::{AtomicUsize, Ordering};

const NOW: i64 = 10_000_000;

/// Records every call; `dirty` names get a dirty inspection, `failing`
/// names fail their kill (an unreachable host, a vanished pane).
#[derive(Default)]
struct FakeExec {
    dirty: Vec<String>,
    failing: Vec<String>,
    calls: Mutex<Vec<String>>,
    inspects: AtomicUsize,
}

#[async_trait::async_trait]
impl GcExec for FakeExec {
    async fn inspect(&self, _h: &str, t: &str) -> Result<SafeKillInspection, IpcError> {
        self.inspects.fetch_add(1, Ordering::SeqCst);
        let dirty = self.dirty.iter().any(|d| d == t);
        Ok(SafeKillInspection {
            has_worktree: true,
            worktree_path: Some("/w".into()),
            branch: Some("b".into()),
            upstream: Some("origin/b".into()),
            dirty_files: if dirty {
                vec![DirtyFile {
                    status: " M".into(),
                    path: "f".into(),
                }]
            } else {
                vec![]
            },
            unpushed_commits: 0,
            safe_to_remove: !dirty,
            error: None,
        })
    }
    async fn safe_kill(&self, _h: &str, t: &str) -> Result<(), IpcError> {
        self.calls.lock().unwrap().push(format!("safe_kill {t}"));
        Ok(())
    }
    async fn kill(&self, _h: &str, t: &str) -> Result<(), IpcError> {
        self.calls.lock().unwrap().push(format!("kill {t}"));
        if self.failing.iter().any(|f| f == t) {
            return Err(IpcError::new(codes::E_SSH, "ssh: connect to host failed"));
        }
        Ok(())
    }
}

impl FakeExec {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

/// The common shape every `seed*` helper needs: a reachable `local` host, an
/// idle `running` work session with its own worktree, and a fresh local item
/// linked to it as the primary confirmed work. Status is left exactly as
/// `create_local_work_item` leaves it — `'todo'`, `status_set_by` `NULL` —
/// so a caller that wants no person status (the merged-PR path) need not
/// reach around anything to get it. Returns `(session_id, item_id)`.
fn seed_session_and_item(store: &Mutex<Store>, name: &str) -> (i64, i64) {
    let s = store.lock().unwrap();
    s.upsert_host("local").unwrap();
    s.update_host_probe("local", true, None, None, 1).unwrap();
    let pid = s.upsert_project("o", "r", "/p/o/r").unwrap();
    let wid = s
        .upsert_worktree(pid, name, &format!("/p/o/r/.worktrees/{name}"), Some(name))
        .unwrap();
    let id = s
        .upsert_session(name, "local", Some(pid), Some(wid), 1, 1, "running", None)
        .unwrap();
    s.set_claude_session_id(id, &format!("c-{name}")).unwrap();
    s.set_claude_status_by_session_id(&format!("c-{name}"), "idle")
        .unwrap();
    let key = format!("{}-1", name.to_ascii_uppercase());
    let item = s.create_local_work_item(Some(&key), name).unwrap();
    s.link_session_work(id, crate::store::WorkTarget::Item(item.id), "manual")
        .unwrap();
    s.conn_ref()
        .execute(
            &format!("UPDATE sessions SET idle_since = 0, worktree_key = '{name}' WHERE id = {id}"),
            [],
        )
        .unwrap();
    (id, item.id)
}

/// A reachable `local` host and an idle work session with its own
/// worktree, linked to a work item a person set to `status` long ago.
fn seed(store: &Mutex<Store>, name: &str, status: &str) -> i64 {
    let (id, item_id) = seed_session_and_item(store, name);
    let s = store.lock().unwrap();
    // Through the real API, not a raw UPDATE to `status_category`: the only
    // writers of a local item's status are creation ('todo'), this
    // (`set_item_status`, 'person') and the merged-PR stamp ('derived'). A
    // raw UPDATE leaves `status_set_by` NULL — a stored status with no
    // owner that no production path can ever produce — which once
    // manufactured an unreachable "in_progress, unowned" fixture and masked
    // a real classification bug (task 3 fix round 2). Keep this going
    // through the API even if it looks like it could be simplified back.
    s.set_item_status(item_id, status).unwrap();
    // `set_item_status` now also stamps `status_changed_at` (task 3 fix
    // round 3) — but to the real wall clock (`now_unix()`), not this file's
    // small synthetic `NOW`. Push it into that synthetic frame so the
    // `done_idle` / `pr_merged_idle` age thresholds are met without waiting
    // real time; this is a clock fixture, not a second status write.
    s.conn_ref()
        .execute(
            "UPDATE work_items SET status_changed_at = 0 WHERE id = ?1",
            [item_id],
        )
        .unwrap();
    id
}

/// A per-host token's scope on `alias`, in no org.
fn host(alias: &str) -> OrgScope {
    OrgScope::Host {
        alias: alias.into(),
        org: None,
        isolated: Default::default(),
    }
}

fn item(session_id: i64, action: &str) -> TidyApplyItem {
    TidyApplyItem {
        session_id,
        action: action.into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn tidy_apply_is_a_batch_that_reports_every_item() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let dirty = seed(&store, "dirty", "done");
    let clean = seed(&store, "clean", "done");
    let failing = seed(&store, "failing", "done");
    let busy = seed(&store, "busy", "done");
    let snoozed = seed(&store, "snoozed", "done");
    store
        .lock()
        .unwrap()
        .set_claude_status_by_session_id("c-busy", "working")
        .unwrap();
    let exec = FakeExec {
        dirty: vec!["dirty".into()],
        failing: vec!["failing".into()],
        ..Default::default()
    };
    let report = tidy_apply(
        &store,
        &exec,
        &[
            item(dirty, "safe_kill"),
            // A person's plain "kill" of their own dirty tree still goes
            // through the safe path; a clean one is killed.
            item(clean, "kill"),
            item(failing, "safe_kill"),
            item(busy, "safe_kill"),
            item(snoozed, "snooze"),
            item(9_999, "archive"),
        ],
        &OrgScope::All,
        NOW,
    )
    .await
    .unwrap();
    let got: Vec<(i64, bool, Option<&str>)> = report
        .results
        .iter()
        .map(|r| (r.session_id, r.ok, r.outcome.as_deref()))
        .collect();
    assert_eq!(
        got,
        vec![
            (dirty, true, Some("safe_kill_requested")),
            (clean, true, Some("killed")),
            (failing, false, None),
            (busy, false, None),
            (snoozed, true, Some("snoozed")),
            (9_999, false, None),
        ]
    );
    assert!(report.results[2].error.as_deref().unwrap().contains("ssh"));
    assert!(report.results[3]
        .error
        .as_deref()
        .unwrap()
        .contains("protected (active)"));
    assert_eq!(
        exec.calls(),
        vec!["safe_kill dirty", "kill clean", "kill failing"],
        "the protected session is never touched; the failure did not stop the batch"
    );
    let s = store.lock().unwrap();
    let ev = s.list_session_events(dirty, 10).unwrap();
    assert!(ev.iter().any(|e| e.kind == "gc_tidied"
        && e.detail.as_deref() == Some("manual:safe_kill:safe_kill_requested")));
    let ev = s.list_session_events(failing, 10).unwrap();
    assert!(ev.iter().any(|e| e.kind == "gc_failed"));
    let journal = s
        .journal_for_conversations(&["c-clean".to_string()])
        .unwrap();
    assert!(journal.iter().any(|j| j.kind == "tidy"));
    drop(s);
    // The snooze took: nothing is suggested for it now.
    let r = work_tidy(
        &store,
        &crate::service::view_scope::ViewScope::internal(),
        NOW,
    )
    .unwrap();
    assert!(!r.candidates.iter().any(|c| c.session_id == snoozed));
}

#[tokio::test]
async fn a_shared_worktree_is_plain_killed_and_archive_keeps_tmux() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let a = seed(&store, "a", "done");
    let b = seed(&store, "b", "todo");
    store
        .lock()
        .unwrap()
        .conn_ref()
        .execute_batch("UPDATE sessions SET worktree_key = 'shared';")
        .unwrap();
    let exec = FakeExec {
        dirty: vec!["a".into()],
        ..Default::default()
    };
    let report = tidy_apply(
        &store,
        &exec,
        &[item(a, "safe_kill"), item(b, "archive")],
        &OrgScope::All,
        NOW,
    )
    .await
    .unwrap();
    assert!(report.results.iter().all(|r| r.ok), "{report:?}");
    assert_eq!(
        exec.calls(),
        vec!["kill a"],
        "never a safe-remove of a shared tree"
    );
    assert_eq!(exec.inspects.load(Ordering::SeqCst), 0);
    let s = store.lock().unwrap();
    let row = s.get_session_by_id(b).unwrap().unwrap();
    assert_eq!(row.status, "running");
    assert!(row.work.unwrap().archived_at.is_some());
}

/// A review session runs in its source's worktree: the source is only ever
/// plain-killed while the review lives, never safe-killed (whose completion
/// removes the tree the review is using).
#[tokio::test]
async fn a_tree_a_live_review_uses_is_only_plain_killed() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let a = seed(&store, "a", "done");
    {
        let s = store.lock().unwrap();
        let pid = s.upsert_project("o", "r", "/p/o/r").unwrap();
        let review = s
            .upsert_session("a-review", "local", Some(pid), None, 1, 1, "running", None)
            .unwrap();
        s.conn_ref()
            .execute(
                "UPDATE sessions SET kind = 'review', worktree_key = 'a' WHERE id = ?1",
                [review],
            )
            .unwrap();
    }
    let exec = FakeExec {
        dirty: vec!["a".into()],
        ..Default::default()
    };
    let plan = work_tidy(
        &store,
        &crate::service::view_scope::ViewScope::internal(),
        NOW,
    )
    .unwrap();
    let c = plan
        .candidates
        .iter()
        .find(|c| c.session_id == a)
        .expect("the done source is still a candidate");
    assert_eq!(c.action, TidyAction::Kill);
    let report = tidy_apply(&store, &exec, &[item(a, "safe_kill")], &OrgScope::All, NOW)
        .await
        .unwrap();
    assert!(report.results[0].ok, "{report:?}");
    assert_eq!(exec.calls(), vec!["kill a"]);
    assert_eq!(exec.inspects.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn host_scope_and_bad_requests_are_per_item_or_refused() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let a = seed(&store, "a", "done");
    let exec = FakeExec::default();
    let r = tidy_apply(&store, &exec, &[item(a, "safe_kill")], &host("other"), NOW)
        .await
        .unwrap();
    assert!(!r.results[0].ok);
    let r = tidy_apply(&store, &exec, &[item(a, "explode")], &OrgScope::All, NOW)
        .await
        .unwrap();
    assert!(r.results[0].error.as_deref().unwrap().contains("unknown"));
    let bad_days = TidyApplyItem {
        days: Some(0),
        ..item(a, "snooze")
    };
    let r = tidy_apply(&store, &exec, &[bad_days], &OrgScope::All, NOW)
        .await
        .unwrap();
    assert!(!r.results[0].ok);
    assert_eq!(
        tidy_apply(&store, &exec, &[], &OrgScope::All, NOW)
            .await
            .unwrap_err()
            .code,
        codes::E_INVALID
    );
    assert!(exec.calls().is_empty());
    assert_eq!(
        work_tidy(
            &store,
            &crate::service::view_scope::ViewScope::internal().with_org(host("other")),
            NOW
        )
        .unwrap()
        .candidates
        .len(),
        0
    );
    assert_eq!(
        work_tidy(
            &store,
            &crate::service::view_scope::ViewScope::internal().with_org(host("local")),
            NOW
        )
        .unwrap()
        .candidates
        .len(),
        1
    );
}

/// A refused item never writes to the session it named: a per-host token
/// could otherwise flood (and so evict, at the timeline's cap) the history
/// of any session on any host, or of an id that does not exist at all.
#[tokio::test]
async fn a_refused_item_writes_no_event_to_the_foreign_session() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let a = seed(&store, "a", "done");
    let count = |kind: &str| -> i64 {
        store
            .lock()
            .unwrap()
            .conn_ref()
            .query_row(
                "SELECT count(*) FROM session_events WHERE kind = ?1",
                [kind],
                |r| r.get(0),
            )
            .unwrap()
    };
    let exec = FakeExec::default();
    let items: Vec<TidyApplyItem> = (0..5)
        .map(|_| item(a, "snooze"))
        .chain(std::iter::once(item(999_999, "kill")))
        .collect();
    let r = tidy_apply(&store, &exec, &items, &host("other"), NOW)
        .await
        .unwrap();
    assert!(r
        .results
        .iter()
        .all(|r| !r.ok && r.error.as_deref().unwrap().contains("not found")));
    assert_eq!(count("gc_failed"), 0, "a refused id gets no timeline row");
    assert!(store
        .lock()
        .unwrap()
        .list_session_events(999_999, 10)
        .unwrap()
        .is_empty());
    // A failure on a session the caller does see is still recorded.
    let r = tidy_apply(&store, &exec, &[item(a, "explode")], &host("local"), NOW)
        .await
        .unwrap();
    assert!(!r.results[0].ok);
    assert_eq!(count("gc_failed"), 1);
    assert!(exec.calls().is_empty());
}

const GC_OFF: GcConfig = GcConfig {
    enabled: false,
    bg_idle_secs: 0,
    shell_idle_secs: 0,
    work_idle_secs: 0,
    sweep_interval_secs: 1,
};

#[tokio::test]
async fn auto_tidy_off_leaves_the_sweep_exactly_as_before() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    seed(&store, "done", "done");
    let exec = FakeExec::default();
    assert_eq!(
        sweep_with(&store, &exec, &GC_OFF, NOW).await,
        GcReport::default()
    );
    // The idle killer on, auto-tidy off: the killer alone acts, as before.
    let killer = GcConfig {
        enabled: true,
        work_idle_secs: 50,
        ..GC_OFF
    };
    let report = sweep_with(&store, &exec, &killer, NOW).await;
    assert_eq!((report.killed, report.tidied), (1, 0));
    assert_eq!(exec.calls(), vec!["kill done"]);
}

#[tokio::test]
async fn auto_tidy_acts_only_on_the_allowed_reasons() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let done = seed(&store, "done", "done");
    let merged = seed(&store, "merged", "todo");
    let wip = seed(&store, "wip", "in_progress");
    {
        let s = store.lock().unwrap();
        s.conn_ref()
            .execute_batch(&format!(
                "UPDATE sessions SET pr_signals = '{{\"state\":\"MERGED\"}}' \
                 WHERE id IN ({merged}, {wip});"
            ))
            .unwrap();
        settings::set(&s, settings::WORK_AUTO_TIDY, "true").unwrap();
        settings::set(&s, settings::WORK_AUTO_TIDY_REASONS, "done_idle").unwrap();
    }
    let exec = FakeExec {
        dirty: vec!["done".into()],
        ..Default::default()
    };
    let report = sweep_with(&store, &exec, &GC_OFF, NOW).await;
    assert_eq!(report.tidied, 1);
    assert_eq!(exec.calls(), vec!["safe_kill done"]);
    let s = store.lock().unwrap();
    assert!(s
        .list_session_events(done, 10)
        .unwrap()
        .iter()
        .any(|e| e.kind == "gc_tidied"
            && e.detail.as_deref() == Some("auto:done_idle:safe_kill:safe_kill_requested")));
    drop(s);
    // pr_merged_idle is still only suggested; the in-progress one never is.
    let r = work_tidy(
        &store,
        &crate::service::view_scope::ViewScope::internal(),
        NOW,
    )
    .unwrap();
    let merged_c = r
        .candidates
        .iter()
        .find(|c| c.session_id == merged)
        .expect("still suggested");
    assert!(!merged_c.auto);
    assert!(!r.candidates.iter().any(|c| c.session_id == wip));
    assert!(r.auto_tidy);
    assert_eq!(r.auto_reasons, vec![TidyReason::DoneIdle]);
}

/// End to end, through the real read path — no hand-stamping and no
/// hand-built `TidyLink`: a merged PR reaches `Store::stamp_derived_done`
/// via `tidy_sessions()` (inside `work_tidy`), and the planner classifies
/// the result as `pr_merged_idle`. Proves the two halves — the stamp write
/// and the reason classification — actually meet (task 3 fix round 3).
#[test]
fn a_merged_prs_stamp_is_offered_as_pr_merged_idle_end_to_end() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let (sid, item_id) = seed_session_and_item(&store, "just-merged");
    store
        .lock()
        .unwrap()
        .conn_ref()
        .execute(
            "UPDATE sessions SET pr_signals = '{\"state\":\"MERGED\"}' WHERE id = ?1",
            [sid],
        )
        .unwrap();
    let r = work_tidy(
        &store,
        &crate::service::view_scope::ViewScope::internal(),
        NOW,
    )
    .unwrap();
    let c = r
        .candidates
        .iter()
        .find(|c| c.session_id == sid)
        .expect("the merged session is a candidate");
    assert_eq!(c.reason, TidyReason::PrMergedIdle);
    let row = store
        .lock()
        .unwrap()
        .get_work_item(item_id)
        .unwrap()
        .unwrap();
    assert_eq!(row.status_category, "done");
    assert_eq!(row.status_set_by.as_deref(), Some("derived"));
    // Prove `stamp_derived_done` itself wrote `status_changed_at` — nothing
    // else in this test overwrites it, so reverting that write would show
    // up here as `None`.
    assert!(row.status_changed_at.is_some());
}

/// End to end for the other half (final review round 2): the merged signal
/// is gone — `sessions.pr_signals` is deleted with its session row, and a
/// probe that can no longer find the PR writes `NULL` — but the stamp it
/// left behind still offers the session as `pr_merged_idle`. Before this it
/// matched no reason at all and lingered in tidy-up forever.
#[test]
fn a_stamped_done_is_still_offered_once_its_signal_is_gone_end_to_end() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let (sid, item_id) = seed_session_and_item(&store, "signal-gone");
    {
        let s = store.lock().unwrap();
        let merged = serde_json::json!({ "state": "MERGED" }).to_string();
        s.set_pr_signals("local", "signal-gone", Some(&merged))
            .unwrap();
        assert_eq!(
            s.get_work_item(item_id).unwrap().unwrap().status_category,
            "done"
        );
        // The signal goes the way it really goes: the probe no longer finds
        // a PR for this session.
        s.set_pr_signals("local", "signal-gone", None).unwrap();
        assert!(s.tidy_sessions().unwrap().iter().all(|t| !t.pr_merged));
        // The stamp's own clock, into this file's synthetic frame.
        s.conn_ref()
            .execute(
                "UPDATE work_items SET status_changed_at = 0 WHERE id = ?1",
                [item_id],
            )
            .unwrap();
    }
    let r = work_tidy(
        &store,
        &crate::service::view_scope::ViewScope::internal(),
        NOW,
    )
    .unwrap();
    let c = r
        .candidates
        .iter()
        .find(|c| c.session_id == sid)
        .expect("a stamped done must still be offered with no live signal");
    assert_eq!(c.reason, TidyReason::PrMergedIdle);
}

/// Finding 1 of the final whole-branch review, and the one that made half
/// the feature not work as shipped: the derived `done` must be stamped by
/// the path that RECORDS the merged PR, not only by the one tidy happens to
/// look through.
///
/// Two halves, in order, and note what this test does NOT do — it never
/// calls `Store::tidy_sessions()` (nor `work_tidy`, `Snapshot::take`,
/// `tidy_apply`), so nothing here can be satisfied by the tidy-path stamp:
///
/// 1. `work.auto_tidy` is unset — the default, off (D2) — so the REAL
///    background sweep (`sweep_with`, the GC tick the app runs) reaches
///    `auto_tidy`, which returns before `Snapshot::take`. The item is still
///    `todo` afterwards: that is exactly the hole, and it is why the second
///    half has to exist. Were the stamp moved back behind auto-tidy, the
///    test would stop at the assertion after `set_pr_signals`.
/// 2. `Store::set_pr_signals` — what the reconcile pass calls when the PR
///    probe reads `MERGED` — stamps it, so a `kill_session` that deletes
///    `pr_signals` with the session row can no longer lose the `done`.
#[tokio::test]
async fn a_merged_pr_stamps_done_in_the_background_with_auto_tidy_off() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let (sid, item_id) = seed_session_and_item(&store, "merged-bg");
    let merged = serde_json::json!({ "head": "feat/x", "state": "MERGED" }).to_string();
    {
        let s = store.lock().unwrap();
        // The merged fact as a session that has already been probed carries
        // it, so the sweep below sees the same input the stamp reads.
        s.conn_ref()
            .execute(
                "UPDATE sessions SET pr_signals = ?2 WHERE id = ?1",
                rusqlite::params![sid, merged],
            )
            .unwrap();
        assert!(
            !tidy_config(&s).auto_anywhere(),
            "the default must be auto-tidy off, or this test proves nothing"
        );
    }
    // The real background tick, auto-tidy off: it stamps nothing.
    let exec = FakeExec::default();
    assert_eq!(
        sweep_with(&store, &exec, &GC_OFF, NOW).await,
        GcReport::default()
    );
    let after_sweep = store
        .lock()
        .unwrap()
        .get_work_item(item_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        after_sweep.status_category, "todo",
        "the tidy path is unreachable in the background with auto_tidy off — \
         which is why the stamp cannot live only there"
    );

    // The write that records the merged PR: this is what must stamp.
    store
        .lock()
        .unwrap()
        .set_pr_signals("local", "merged-bg", Some(&merged))
        .unwrap();
    let row = store
        .lock()
        .unwrap()
        .get_work_item(item_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        row.status_category, "done",
        "a merged PR must stamp its local work done where the fact is written"
    );
    assert_eq!(row.status_set_by.as_deref(), Some("derived"));
}

/// Once `status_changed_at` is old enough, a person's `done` on a local item
/// is offered as `done_idle` — the reason that, before `set_item_status`
/// stamped `status_changed_at` (task 3 fix round 3), could never fire for
/// native work at all: the column stayed `NULL` forever, so `done_long`
/// (`service/gc/tidy.rs`) was always false for a local item, whoever set it.
#[test]
fn a_persons_done_ages_into_done_idle() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let (sid, item_id) = seed_session_and_item(&store, "shipped-by-hand");
    {
        let s = store.lock().unwrap();
        let row = s.set_item_status(item_id, "done").unwrap().unwrap();
        // Prove the API itself wrote the column, before the clock push below
        // overwrites it — otherwise this test would still pass even if
        // `set_item_status` never stamped `status_changed_at` at all.
        assert!(row.status_changed_at.is_some());
        // Push the real wall-clock stamp `set_item_status` just wrote into
        // this file's synthetic `NOW` frame (see `seed`'s own comment).
        s.conn_ref()
            .execute(
                "UPDATE work_items SET status_changed_at = 0 WHERE id = ?1",
                [item_id],
            )
            .unwrap();
    }
    let r = work_tidy(
        &store,
        &crate::service::view_scope::ViewScope::internal(),
        NOW,
    )
    .unwrap();
    let c = r
        .candidates
        .iter()
        .find(|c| c.session_id == sid)
        .expect("a candidate");
    assert_eq!(c.reason, TidyReason::DoneIdle);
}

/// Orgs (work graph M5 merged into M7): `local` in Company A, sessions of
/// A's and B's projects on it, and one A session carrying B's ticket.
struct Orgs {
    store: Mutex<Store>,
    a: i64,
    b_sess: i64,
    a_sess: i64,
    x_sess: i64,
    org_a: i64,
}

fn orgs() -> Orgs {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("local").unwrap();
    s.update_host_probe("local", true, None, None, 1).unwrap();
    let a = s.add_org("Company A", None, false).unwrap().id;
    let b = s.add_org("Company B", None, false).unwrap().id;
    for (org, owner) in [(a, "acme"), (b, "beta")] {
        s.add_org_rule(crate::store::OrgRuleRow {
            org_id: org,
            owner: Some(owner.into()),
            ..Default::default()
        })
        .unwrap();
    }
    s.set_host_org("local", Some(a)).unwrap();
    let tb = s
        .add_tracker("jira", "B", "https://bravo.atlassian.net")
        .unwrap()
        .id;
    s.set_tracker_org(tb, Some(b)).unwrap();
    let b_item = s
        .upsert_tracker_item(
            tb,
            &crate::store::TrackerItemWrite {
                external_id: "1".into(),
                key: Some("BB-1".into()),
                title: "Bravo".into(),
                status_name: "Done".into(),
                status_category: "done".into(),
                ..Default::default()
            },
        )
        .unwrap()
        .id;
    let sess = |name: &str, owner: &str| {
        let pid = s
            .upsert_project(owner, "r", &format!("/p/{owner}/r"))
            .unwrap();
        let wid = s
            .upsert_worktree(
                pid,
                name,
                &format!("/p/{owner}/r/.worktrees/{name}"),
                Some(name),
            )
            .unwrap();
        let id = s
            .upsert_session(name, "local", Some(pid), Some(wid), 1, 1, "running", None)
            .unwrap();
        s.set_claude_session_id(id, &format!("c-{name}")).unwrap();
        s.set_claude_status_by_session_id(&format!("c-{name}"), "idle")
            .unwrap();
        s.conn_ref()
            .execute(
                "UPDATE sessions SET idle_since = 0, worktree_key = ?2 WHERE id = ?1",
                rusqlite::params![id, name],
            )
            .unwrap();
        id
    };
    let a_sess = sess("a1", "acme");
    let b_sess = sess("b1", "beta");
    let x_sess = sess("x1", "acme");
    for (id, key) in [(a_sess, "AA-1"), (b_sess, "BB-9")] {
        let item = s.create_local_work_item(Some(key), key).unwrap();
        s.link_session_work(id, crate::store::WorkTarget::Item(item.id), "manual")
            .unwrap();
    }
    s.link_session_work(x_sess, crate::store::WorkTarget::Item(b_item), "manual")
        .unwrap();
    s.conn_ref()
        .execute_batch("UPDATE work_items SET status_category = 'done', status_changed_at = 0;")
        .unwrap();
    Orgs {
        store: Mutex::new(s),
        a,
        b_sess,
        a_sess,
        x_sess,
        org_a: a,
    }
}

#[tokio::test]
async fn a_per_host_token_never_sees_or_touches_another_orgs_candidates() {
    let o = orgs();
    let scope = {
        let s = o.store.lock().unwrap();
        OrgScope::for_host(&s, "local").unwrap()
    };
    // The master sees all three.
    let all = work_tidy(
        &o.store,
        &crate::service::view_scope::ViewScope::internal(),
        NOW,
    )
    .unwrap();
    let ids: Vec<i64> = all.candidates.iter().map(|c| c.session_id).collect();
    assert_eq!(ids, vec![o.a_sess, o.b_sess, o.x_sess]);
    assert_eq!(all.candidates[2].key.as_deref(), Some("BB-1"));
    // Host A (in Company A): its org's sessions only, and B's ticket on
    // its own session is not named.
    let mine = work_tidy(
        &o.store,
        &crate::service::view_scope::ViewScope::internal().with_org(scope.clone()),
        NOW,
    )
    .unwrap();
    let ids: Vec<i64> = mine.candidates.iter().map(|c| c.session_id).collect();
    assert_eq!(ids, vec![o.a_sess, o.x_sess]);
    let x = &mine.candidates[1];
    assert_eq!(
        (x.key.as_deref(), x.link_id, x.item_status.as_deref()),
        (None, None, None)
    );
    assert_eq!(mine.candidates[0].key.as_deref(), Some("AA-1"));
    assert!(!serde_json::to_string(&mine).unwrap().contains("BB-"));

    // Acting on B's session reads as an unknown one; B's link cannot be
    // snoozed through the A session either. Nothing is executed.
    let exec = FakeExec::default();
    let r = tidy_apply(
        &o.store,
        &exec,
        &[
            item(o.b_sess, "safe_kill"),
            item(o.x_sess, "snooze"),
            item(o.a_sess, "never"),
        ],
        &scope,
        NOW,
    )
    .await
    .unwrap();
    let got: Vec<(bool, Option<&str>)> = r
        .results
        .iter()
        .map(|r| (r.ok, r.error.as_deref()))
        .collect();
    assert_eq!(
        got[0],
        (false, Some(&*format!("session {} not found", o.b_sess)))
    );
    assert!(!got[1].0 && got[1].1.unwrap().contains("no such live work link"));
    assert_eq!(got[2], (true, None));
    assert!(exec.calls().is_empty());
}

/// `work_link { snooze | never | archive }` without `link_id` resolves the
/// primary first and checks it: B's forced link on A's own session is never
/// flagged by A's token, with or without the id, and archive stamps only
/// what the scope sees.
#[test]
fn lifecycle_flags_without_link_id_never_reach_another_orgs_primary() {
    use crate::service::work::{work_link, WorkLinkArgs};
    let o = orgs();
    let (scope, b_link) = {
        let s = o.store.lock().unwrap();
        (
            OrgScope::for_host(&s, "local").unwrap(),
            s.session_work_links(o.x_sess).unwrap()[0].id,
        )
    };
    let args = |sid: i64, action: &str, link_id: Option<i64>| WorkLinkArgs {
        session_id: Some(sid),
        action: action.into(),
        link_id,
        ..Default::default()
    };
    for (action, link_id) in [
        ("never", None),
        ("snooze", None),
        ("never", Some(b_link)),
        ("snooze", Some(b_link)),
    ] {
        let err = work_link(&args(o.x_sess, action, link_id), &o.store, &scope).unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND, "{action} {link_id:?}");
        assert!(!err.message.contains("BB-"));
    }
    assert_eq!(
        work_link(&args(o.x_sess, "archive", None), &o.store, &scope)
            .unwrap_err()
            .code,
        codes::E_INVALID,
        "no visible linked work to archive under"
    );
    let flags = |sid: i64| -> (i64, Option<i64>, Option<i64>) {
        o.store
            .lock()
            .unwrap()
            .conn_ref()
            .query_row(
                "SELECT tidy_never, tidy_snoozed_until, archived_at FROM work_links \
                 WHERE ended_at IS NULL AND participant_id = \
                   (SELECT id FROM participants WHERE session_id = ?1 AND retired_at IS NULL)",
                [sid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap()
    };
    assert_eq!(flags(o.x_sess), (0, None, None), "B's link is untouched");
    // The same calls on A's own link work, and the master flags B's.
    work_link(&args(o.a_sess, "never", None), &o.store, &scope).unwrap();
    work_link(&args(o.a_sess, "archive", None), &o.store, &scope).unwrap();
    let (never, _, archived) = flags(o.a_sess);
    assert!(never == 1 && archived.is_some());
    work_link(&args(o.x_sess, "never", None), &o.store, &OrgScope::All).unwrap();
    assert_eq!(flags(o.x_sess).0, 1);
}

#[tokio::test]
async fn auto_tidy_follows_the_org_override() {
    let o = orgs();
    {
        let s = o.store.lock().unwrap();
        // Global off, Company A on: only A's sessions are acted on.
        s.set_org_auto_tidy(o.org_a, Some(true)).unwrap();
        let cfg = tidy_config(&s);
        assert!(!cfg.auto && cfg.auto_anywhere());
    }
    let exec = FakeExec::default();
    let report = sweep_with(&o.store, &exec, &GC_OFF, NOW).await;
    assert_eq!(report.tidied, 2);
    assert_eq!(exec.calls(), vec!["kill a1", "kill x1"]);
    // Global on, Company A off: only the others.
    {
        let s = o.store.lock().unwrap();
        settings::set(&s, settings::WORK_AUTO_TIDY, "true").unwrap();
        s.set_org_auto_tidy(o.org_a, Some(false)).unwrap();
    }
    let c = work_tidy(
        &o.store,
        &crate::service::view_scope::ViewScope::internal(),
        NOW,
    )
    .unwrap()
    .candidates;
    let auto: Vec<i64> = c.iter().filter(|c| c.auto).map(|c| c.session_id).collect();
    assert_eq!(auto, vec![o.b_sess]);
    // Inherit clears the override.
    let s = o.store.lock().unwrap();
    assert_eq!(s.set_org_auto_tidy(o.org_a, None).unwrap().auto_tidy, None);
    let _ = o.a;
}

// ── idle_unlinked and keep (work graph M11.3) ─────────────────────────

/// An idle work session on `local` with its own worktree and NO work
/// linked, created (and last used) long before `NOW`.
fn seed_unlinked(store: &Mutex<Store>, name: &str) -> i64 {
    let s = store.lock().unwrap();
    s.upsert_host("local").unwrap();
    s.update_host_probe("local", true, None, None, 1).unwrap();
    let pid = s.upsert_project("o", "r", "/p/o/r").unwrap();
    let wid = s
        .upsert_worktree(pid, name, &format!("/p/o/r/.worktrees/{name}"), Some(name))
        .unwrap();
    let id = s
        .upsert_session(name, "local", Some(pid), Some(wid), 1, 1, "running", None)
        .unwrap();
    s.set_claude_session_id(id, &format!("c-{name}")).unwrap();
    s.set_claude_status_by_session_id(&format!("c-{name}"), "idle")
        .unwrap();
    s.conn_ref()
        .execute_batch(&format!(
            "UPDATE sessions SET idle_since = 0, worktree_key = '{name}', created_at = 0, \
               started_at = NULL, last_turn_at = NULL, last_stop_at = NULL, \
               last_touch_at = NULL WHERE id = {id};"
        ))
        .unwrap();
    id
}

fn reason_of(store: &Mutex<Store>, id: i64) -> Option<TidyReason> {
    work_tidy(
        store,
        &crate::service::view_scope::ViewScope::internal(),
        NOW,
    )
    .unwrap()
    .candidates
    .iter()
    .find(|c| c.session_id == id)
    .map(|c| c.reason)
}

#[tokio::test]
async fn idle_unlinked_is_suggested_and_killed_only_when_clean() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let clean = seed_unlinked(&store, "clean");
    let dirty = seed_unlinked(&store, "dirty");
    assert_eq!(reason_of(&store, clean), Some(TidyReason::IdleUnlinked));
    assert_eq!(reason_of(&store, dirty), Some(TidyReason::IdleUnlinked));
    let report = work_tidy(
        &store,
        &crate::service::view_scope::ViewScope::internal(),
        NOW,
    )
    .unwrap();
    assert_eq!(report.idle_unlinked_days, 7);
    let exec = FakeExec {
        dirty: vec!["dirty".into()],
        ..Default::default()
    };
    let r = tidy_apply(
        &store,
        &exec,
        &[item(clean, "safe_kill"), item(dirty, "safe_kill")],
        &OrgScope::All,
        NOW,
    )
    .await
    .unwrap();
    assert!(r.results[0].ok, "{r:?}");
    assert_eq!(r.results[0].outcome.as_deref(), Some("killed"));
    assert!(!r.results[1].ok);
    assert!(
        r.results[1]
            .error
            .as_deref()
            .unwrap()
            .contains("uncommitted or unpushed"),
        "{r:?}"
    );
    assert_eq!(
        exec.calls(),
        vec!["kill clean"],
        "dirty unlinked work is refused, never safe-killed"
    );
}

#[tokio::test]
async fn an_unlinked_session_that_is_no_longer_a_candidate_is_not_killed() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let used = seed_unlinked(&store, "used");
    // Prompted two days ago: out of the window (and past the 1 h touch).
    store
        .lock()
        .unwrap()
        .conn_ref()
        .execute_batch(&format!(
            "UPDATE sessions SET last_touch_at = {} WHERE id = {used};",
            NOW - 2 * 86_400
        ))
        .unwrap();
    assert_eq!(reason_of(&store, used), None);
    let exec = FakeExec::default();
    let r = tidy_apply(&store, &exec, &[item(used, "kill")], &OrgScope::All, NOW)
        .await
        .unwrap();
    assert!(!r.results[0].ok);
    assert!(r.results[0]
        .error
        .as_deref()
        .unwrap()
        .contains("no longer a tidy-up candidate"));
    assert!(exec.calls().is_empty());
    assert_eq!(exec.inspects.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_suggested_link_keeps_a_session_out_of_idle_unlinked() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let id = seed_unlinked(&store, "guessed");
    assert_eq!(reason_of(&store, id), Some(TidyReason::IdleUnlinked));
    store
        .lock()
        .unwrap()
        .conn_ref()
        .execute_batch(&format!(
            "INSERT INTO work_links (ref_key, participant_id, state, source, created_at) \
             SELECT 'ABC-9', p.id, 'suggested', 'branch', 1 FROM participants p \
             WHERE p.session_id = {id} AND p.retired_at IS NULL;"
        ))
        .unwrap();
    assert_eq!(reason_of(&store, id), None, "a guess is still work");
}

#[tokio::test]
async fn keep_holds_a_session_out_for_n_days_per_session() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let id = seed_unlinked(&store, "keepme");
    let other = seed_unlinked(&store, "other");
    let exec = FakeExec::default();
    let keep = |days: Option<u32>| TidyApplyItem {
        days,
        ..item(id, "keep")
    };
    // Bounds first: nothing written.
    for bad in [Some(0), Some(91)] {
        let r = tidy_apply(&store, &exec, &[keep(bad)], &OrgScope::All, NOW)
            .await
            .unwrap();
        assert!(!r.results[0].ok, "{bad:?}");
    }
    assert_eq!(reason_of(&store, id), Some(TidyReason::IdleUnlinked));
    // Another host's token: the session does not exist for it.
    let r = tidy_apply(&store, &exec, &[keep(None)], &host("elsewhere"), NOW)
        .await
        .unwrap();
    assert!(r.results[0].error.as_deref().unwrap().contains("not found"));
    assert_eq!(reason_of(&store, id), Some(TidyReason::IdleUnlinked));
    // Its own host's token keeps it (no link needed), for 3 days.
    let r = tidy_apply(&store, &exec, &[keep(Some(3))], &host("local"), NOW)
        .await
        .unwrap();
    assert_eq!(r.results[0].outcome.as_deref(), Some("kept"), "{r:?}");
    assert_eq!(reason_of(&store, id), None);
    assert_eq!(
        reason_of(&store, other),
        Some(TidyReason::IdleUnlinked),
        "per session"
    );
    // It is on the timeline, and back once the keep ends.
    let ev = store.lock().unwrap().list_session_events(id, 10).unwrap();
    assert!(ev
        .iter()
        .any(|e| e.kind == "tidy_kept"
            && e.detail.as_deref() == Some(&*(NOW + 3 * 86_400).to_string())));
    let later = NOW + 3 * 86_400 + 1;
    assert!(work_tidy(
        &store,
        &crate::service::view_scope::ViewScope::internal(),
        later
    )
    .unwrap()
    .candidates
    .iter()
    .any(|c| c.session_id == id));
    // The latest keep wins, shorter or not.
    tidy_apply(&store, &exec, &[keep(Some(1))], &OrgScope::All, NOW)
        .await
        .unwrap();
    assert!(work_tidy(
        &store,
        &crate::service::view_scope::ViewScope::internal(),
        NOW + 86_400 + 1
    )
    .unwrap()
    .candidates
    .iter()
    .any(|c| c.session_id == id));
    assert!(exec.calls().is_empty(), "keep never kills");
}

#[tokio::test]
async fn a_keep_does_not_survive_its_session_row() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let id = seed_unlinked(&store, "gone");
    {
        let s = store.lock().unwrap();
        s.keep_tidy(id, NOW + 30 * 86_400).unwrap();
        // A keep written before the row was (re)created does not apply.
        s.conn_ref()
            .execute_batch(&format!(
                "UPDATE session_events SET at = 0 WHERE session_id = {id}; \
                 UPDATE sessions SET created_at = 5 WHERE id = {id};"
            ))
            .unwrap();
    }
    assert_eq!(reason_of(&store, id), Some(TidyReason::IdleUnlinked));
}

#[tokio::test]
async fn auto_tidy_never_kills_an_idle_unlinked_session() {
    // D19: every switch on, every reason allowed in the planner — still
    // nothing. (The setting cannot even name idle_unlinked.)
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let id = seed_unlinked(&store, "lonely");
    {
        let s = store.lock().unwrap();
        settings::set(&s, settings::WORK_AUTO_TIDY, "true").unwrap();
        settings::set(
            &s,
            settings::WORK_AUTO_TIDY_REASONS,
            "done_idle,pr_merged_idle,not_planned",
        )
        .unwrap();
        assert!(settings::set(&s, settings::WORK_AUTO_TIDY_REASONS, "idle_unlinked").is_err());
    }
    let exec = FakeExec::default();
    let report = sweep_with(&store, &exec, &GC_OFF, NOW).await;
    assert_eq!(report.tidied, 0);
    assert!(exec.calls().is_empty());
    assert_eq!(reason_of(&store, id), Some(TidyReason::IdleUnlinked));
    // And the executor itself refuses an automatic idle_unlinked kill, were
    // a candidate ever to reach it.
    let snap = Snapshot::take(&store).unwrap();
    let plan = snap.plan(NOW);
    let s = resolve(&snap, &OrgScope::All, id).unwrap();
    let ctx = ApplyCtx {
        scope: &OrgScope::All,
        source: "auto:idle_unlinked",
        now: NOW,
        plan: &plan,
    };
    let err = apply_one(&store, &exec, &snap, s, &item(id, "safe_kill"), ctx)
        .await
        .unwrap_err();
    assert!(err.message.contains("auto-tidy never"), "{}", err.message);
    assert!(exec.calls().is_empty());
}

/// G1.9: a `safe_kill` candidate carries its own worktree's size as the host
/// probe last measured it, which the sheet totals as "frees about …"; a
/// tree the probe has not measured says nothing rather than 0.
#[test]
fn a_safe_kill_candidate_carries_its_worktrees_measured_size() {
    // Its own host alias: the per-worktree sizes are process-wide.
    const HOST: &str = "tidy-size-host";
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let measured = seed(&store, "measured", "done");
    let unmeasured = seed(&store, "unmeasured", "done");
    {
        let s = store.lock().unwrap();
        s.upsert_host(HOST).unwrap();
        s.update_host_probe(HOST, true, None, None, 1).unwrap();
        s.conn_ref()
            .execute(
                "UPDATE sessions SET host_alias = ?1 WHERE id IN (?2, ?3)",
                rusqlite::params![HOST, measured, unmeasured],
            )
            .unwrap();
    }
    crate::service::sessions::worktree_sizes::record(
        HOST,
        NOW,
        [("/p/o/r/.worktrees/measured".to_string(), 2_200_000)],
    );
    let r = work_tidy(
        &store,
        &crate::service::view_scope::ViewScope::internal(),
        NOW,
    )
    .unwrap();
    let of = |id: i64| {
        r.candidates
            .iter()
            .find(|c| c.session_id == id)
            .expect("a done, idle session is a candidate")
    };
    assert_eq!(of(measured).action, TidyAction::SafeKill);
    assert_eq!(of(measured).worktree_kb, Some(2_200_000));
    assert_eq!(of(unmeasured).worktree_kb, None);
    // G3.12: the row detail names the tree a clean up removes, measured or not.
    assert_eq!(
        of(measured).worktree_path.as_deref(),
        Some("/p/o/r/.worktrees/measured")
    );
    assert_eq!(
        of(unmeasured).worktree_path.as_deref(),
        Some("/p/o/r/.worktrees/unmeasured")
    );
    // On the wire only when known.
    let wire = serde_json::to_value(of(unmeasured)).unwrap();
    assert!(wire.get("worktree_kb").is_none(), "{wire}");
}
