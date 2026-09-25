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
    let r = work_tidy(&store, &OrgScope::All, NOW).unwrap();
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
        work_tidy(&store, &host("other"), NOW)
            .unwrap()
            .candidates
            .len(),
        0
    );
    assert_eq!(
        work_tidy(&store, &host("local"), NOW)
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
    let r = work_tidy(&store, &OrgScope::All, NOW).unwrap();
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
    let all = work_tidy(&o.store, &OrgScope::All, NOW).unwrap();
    let ids: Vec<i64> = all.candidates.iter().map(|c| c.session_id).collect();
    assert_eq!(ids, vec![o.a_sess, o.b_sess, o.x_sess]);
    assert_eq!(all.candidates[2].key.as_deref(), Some("BB-1"));
    // Host A (in Company A): its org's sessions only, and B's ticket on
    // its own session is not named.
    let mine = work_tidy(&o.store, &scope, NOW).unwrap();
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
    let c = work_tidy(&o.store, &OrgScope::All, NOW).unwrap().candidates;
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
    work_tidy(store, &OrgScope::All, NOW)
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
    let report = work_tidy(&store, &OrgScope::All, NOW).unwrap();
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
    assert!(work_tidy(&store, &OrgScope::All, later)
        .unwrap()
        .candidates
        .iter()
        .any(|c| c.session_id == id));
    // The latest keep wins, shorter or not.
    tidy_apply(&store, &exec, &[keep(Some(1))], &OrgScope::All, NOW)
        .await
        .unwrap();
    assert!(work_tidy(&store, &OrgScope::All, NOW + 86_400 + 1)
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
    let err = apply_one(
        &store,
        &exec,
        &snap,
        &plan,
        &item(id, "safe_kill"),
        &OrgScope::All,
        "auto:idle_unlinked",
        NOW,
    )
    .await
    .unwrap_err();
    assert!(err.message.contains("auto-tidy never"), "{}", err.message);
    assert!(exec.calls().is_empty());
}
