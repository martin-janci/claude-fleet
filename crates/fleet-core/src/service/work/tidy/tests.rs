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

/// A reachable `local` host and an idle work session with its own
/// worktree, linked to a work item done long ago (`done`) or still todo.
fn seed(store: &Mutex<Store>, name: &str, status: &str) -> i64 {
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
        .execute_batch(&format!(
            "UPDATE sessions SET idle_since = 0, worktree_key = '{name}' WHERE id = {id}; \
             UPDATE work_items SET status_category = '{status}', status_changed_at = 0 \
               WHERE id = {};",
            item.id
        ))
        .unwrap();
    id
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
        None,
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
    let r = work_tidy(&store, None, NOW).unwrap();
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
        None,
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

#[tokio::test]
async fn host_scope_and_bad_requests_are_per_item_or_refused() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let a = seed(&store, "a", "done");
    let exec = FakeExec::default();
    let r = tidy_apply(&store, &exec, &[item(a, "safe_kill")], Some("other"), NOW)
        .await
        .unwrap();
    assert!(!r.results[0].ok);
    let r = tidy_apply(&store, &exec, &[item(a, "explode")], None, NOW)
        .await
        .unwrap();
    assert!(r.results[0].error.as_deref().unwrap().contains("unknown"));
    let bad_days = TidyApplyItem {
        days: Some(0),
        ..item(a, "snooze")
    };
    let r = tidy_apply(&store, &exec, &[bad_days], None, NOW)
        .await
        .unwrap();
    assert!(!r.results[0].ok);
    assert_eq!(
        tidy_apply(&store, &exec, &[], None, NOW)
            .await
            .unwrap_err()
            .code,
        codes::E_INVALID
    );
    assert!(exec.calls().is_empty());
    assert_eq!(
        work_tidy(&store, Some("other"), NOW)
            .unwrap()
            .candidates
            .len(),
        0
    );
    assert_eq!(
        work_tidy(&store, Some("local"), NOW)
            .unwrap()
            .candidates
            .len(),
        1
    );
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
    let r = work_tidy(&store, None, NOW).unwrap();
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
