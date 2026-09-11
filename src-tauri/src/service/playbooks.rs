//! Stuck-session playbooks (PROD-2): per-`stuck_kind` remediation run by the
//! background tick right after a reconcile pass.
//!
//! | kind          | action                                   | gate                       |
//! |---------------|------------------------------------------|----------------------------|
//! | press_enter   | send Enter to the pane                   | `playbooks.press_enter`    |
//! | oom           | recreate (resume by claude_session_id)   | `playbooks.oom_recreate`, ≤1/h |
//! | auth_menu, trust_prompt, reconnect | notify only (row + timeline event) | always |
//!
//! Every action runs at most once per stuck *episode*: reconcile stamps
//! `stuck_since` when a `stuck_kind` appears (or changes), and a row whose
//! `last_playbook_at` is at/after that stamp is skipped. Both keystroke
//! playbooks default OFF so an upgrade changes nothing until the operator
//! opts in. The planner is pure; the executor is injectable for tests.

use crate::ipc_error::IpcError;
use crate::service::settings;
use crate::shell::quote;
use crate::ssh::SshClient;
use crate::store::{SessionRow, Store};
use std::sync::{Arc, Mutex};

/// Minimum spacing between two `oom` recreates of the same session.
pub const OOM_RECREATE_MIN_SPACING_SECS: i64 = 3600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlaybookConfig {
    pub press_enter: bool,
    pub oom_recreate: bool,
}

impl PlaybookConfig {
    pub fn from_store(s: &Store) -> Self {
        Self {
            press_enter: settings::get_bool(s, settings::PLAYBOOK_PRESS_ENTER),
            oom_recreate: settings::get_bool(s, settings::PLAYBOOK_OOM_RECREATE),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybookAction {
    PressEnter,
    Recreate,
    /// No keystrokes: only the `playbook_applied` timeline entry + row event.
    Notify,
}

impl PlaybookAction {
    fn as_str(self) -> &'static str {
        match self {
            PlaybookAction::PressEnter => "press_enter",
            PlaybookAction::Recreate => "recreate",
            PlaybookAction::Notify => "notify",
        }
    }
}

/// One planned remediation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub session_id: i64,
    pub host_alias: String,
    pub tmux_name: String,
    pub stuck_kind: String,
    pub action: PlaybookAction,
}

/// Pure: decide which rows get which playbook this tick.
///
/// A row qualifies when it is a live tmux session (`status == running`, not a
/// `bg` sentinel) with a `stuck_kind` AND a `stuck_since` stamp that is newer
/// than its `last_playbook_at`. Keystroke actions never target the registered
/// controller session (it would be steering itself); notify still applies.
pub fn plan(
    rows: &[SessionRow],
    cfg: &PlaybookConfig,
    controller: Option<&(String, String)>,
    now: i64,
) -> Vec<Planned> {
    let mut out = Vec::new();
    for r in rows {
        if r.status != "running" || r.kind == "bg" {
            continue;
        }
        let (Some(kind), Some(since)) = (r.stuck_kind.as_deref(), r.stuck_since) else {
            continue;
        };
        if r.last_playbook_at.map(|lp| lp >= since).unwrap_or(false) {
            continue; // already acted on this episode
        }
        let is_controller = controller
            .map(|(h, t)| h == &r.host_alias && t == &r.tmux_name)
            .unwrap_or(false);
        let action = match kind {
            "press_enter" if cfg.press_enter && !is_controller => PlaybookAction::PressEnter,
            "oom" if cfg.oom_recreate && !is_controller => {
                let recently = r
                    .last_playbook_at
                    .map(|lp| now - lp < OOM_RECREATE_MIN_SPACING_SECS)
                    .unwrap_or(false);
                if recently {
                    continue;
                }
                PlaybookAction::Recreate
            }
            "press_enter" | "oom" => continue, // gated off: leave the episode untouched
            "auth_menu" | "trust_prompt" | "reconnect" => PlaybookAction::Notify,
            _ => continue,
        };
        out.push(Planned {
            session_id: r.id,
            host_alias: r.host_alias.clone(),
            tmux_name: r.tmux_name.clone(),
            stuck_kind: kind.to_string(),
            action,
        });
    }
    out
}

/// What `press_enter` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressEnterOutcome {
    Sent,
    /// A human is attached to the tmux session: a synthetic keystroke would
    /// land in whatever they are typing, so nothing was sent.
    SkippedAttached,
}

/// Side effects a playbook can perform. Injected so the runner is testable
/// without tmux or ssh.
#[async_trait::async_trait]
pub trait PlaybookExec: Send + Sync {
    async fn press_enter(
        &self,
        host_alias: &str,
        tmux_name: &str,
    ) -> Result<PressEnterOutcome, IpcError>;
    async fn recreate(&self, session_id: i64) -> Result<(), IpcError>;
}

/// Production executor: `tmux send-keys … Enter` through the host shell, and
/// the regular `recreate_session` service path (resumes by claude_session_id).
pub struct RealPlaybookExec {
    pub store: Arc<Mutex<Store>>,
    pub ssh: Arc<SshClient>,
}

/// The exact script sent for `press_enter`. Pure so the quoting is testable.
pub fn press_enter_script(tmux_name: &str) -> String {
    format!("tmux send-keys -t {} Enter", quote(tmux_name))
}

/// Script that prints the number of clients attached to the session.
pub fn attached_probe_script(tmux_name: &str) -> String {
    format!(
        "tmux display -p -t {} '#{{session_attached}}'",
        quote(tmux_name)
    )
}

/// Pure: decide from `tmux display '#{session_attached}'` output whether a
/// keystroke may be sent. Anything but a clean `0` (an attached client, a
/// vanished session, garbage) means "do not type into this pane".
pub fn pane_is_attached(stdout: &str) -> bool {
    !matches!(stdout.trim().parse::<u32>(), Ok(0))
}

#[async_trait::async_trait]
impl PlaybookExec for RealPlaybookExec {
    async fn press_enter(
        &self,
        host_alias: &str,
        tmux_name: &str,
    ) -> Result<PressEnterOutcome, IpcError> {
        crate::validate::tmux_name_addressable(tmux_name)?;
        let attached = crate::service::sessions::run_host_script(
            &self.ssh,
            host_alias,
            &attached_probe_script(tmux_name),
            std::time::Duration::from_secs(10),
        )
        .await?;
        if !attached.status.success()
            || pane_is_attached(&String::from_utf8_lossy(&attached.stdout))
        {
            return Ok(PressEnterOutcome::SkippedAttached);
        }
        let out = crate::service::sessions::run_host_script(
            &self.ssh,
            host_alias,
            &press_enter_script(tmux_name),
            std::time::Duration::from_secs(10),
        )
        .await?;
        if !out.status.success() {
            return Err(IpcError::new(
                "E_TMUX",
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ));
        }
        Ok(PressEnterOutcome::Sent)
    }

    async fn recreate(&self, session_id: i64) -> Result<(), IpcError> {
        crate::service::sessions::recreate_session(
            crate::service::sessions::RecreateSessionArgs {
                session_id,
                force: false,
            },
            &self.store,
            &self.ssh,
        )
        .await
        .map(|_| ())
    }
}

/// Apply the plan. Returns the number of playbooks recorded. Each action is
/// stamped (`last_playbook_at` + `playbook_applied` event) whether it
/// succeeded or failed — a failing keystroke must not be retried every 20 s
/// for the same episode; the event detail carries the error for the timeline.
pub async fn run_with(
    store: &Mutex<Store>,
    exec: &dyn PlaybookExec,
    cfg: &PlaybookConfig,
    now: i64,
) -> usize {
    let (rows, controller) = {
        let Ok(s) = store.lock() else {
            return 0;
        };
        let rows = s.list_all_sessions().unwrap_or_default();
        let controller = s.get_controller().ok().flatten();
        (rows, controller)
    };
    let planned = plan(&rows, cfg, controller.as_ref(), now);
    let mut applied = 0;
    for p in planned {
        let result = match p.action {
            PlaybookAction::PressEnter => exec.press_enter(&p.host_alias, &p.tmux_name).await,
            PlaybookAction::Recreate => exec
                .recreate(p.session_id)
                .await
                .map(|()| PressEnterOutcome::Sent),
            PlaybookAction::Notify => Ok(PressEnterOutcome::Sent),
        };
        let detail = match &result {
            Ok(PressEnterOutcome::Sent) => format!("{}:{}", p.stuck_kind, p.action.as_str()),
            Ok(PressEnterOutcome::SkippedAttached) => {
                format!("{}:{}:skipped:attached", p.stuck_kind, p.action.as_str())
            }
            Err(e) => format!(
                "{}:{}:failed:{}",
                p.stuck_kind,
                p.action.as_str(),
                e.message
            ),
        };
        if let Err(e) = &result {
            eprintln!(
                "[playbook] {} on {}/{} failed: {e}",
                p.action.as_str(),
                p.host_alias,
                p.tmux_name
            );
        }
        if let Ok(s) = store.lock() {
            match s.mark_playbook_applied(p.session_id, now, &detail) {
                Ok(_) => applied += 1,
                Err(e) => eprintln!("[playbook] stamping session {} failed: {e}", p.session_id),
            }
        }
    }
    applied
}

/// Tick entry point: read the toggles, run the plan with the real executor.
pub async fn run(store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>) -> usize {
    let cfg = {
        let Ok(s) = store.lock() else {
            return 0;
        };
        PlaybookConfig::from_store(&s)
    };
    let exec = RealPlaybookExec {
        store: Arc::clone(store),
        ssh: Arc::clone(ssh),
    };
    run_with(store, &exec, &cfg, now_unix()).await
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn row(
        id: i64,
        name: &str,
        kind: Option<&str>,
        since: Option<i64>,
        last: Option<i64>,
    ) -> SessionRow {
        SessionRow {
            id,
            tmux_name: name.into(),
            host_alias: "local".into(),
            project_id: None,
            worktree_id: None,
            created_at: 0,
            last_activity_at: 0,
            status: "running".into(),
            notes: None,
            account_uuid: None,
            kind: "work".into(),
            reviews_session_id: None,
            worktree_key: None,
            lost_at: None,
            claude_session_id: None,
            claude_status: None,
            effort_level: None,
            pr_url: None,
            current_activity: None,
            context_pct: None,
            stuck_kind: kind.map(Into::into),
            friendly_name: None,
            safe_kill_state: None,
            safe_kill_nonce: None,
            safe_kill_detail: None,
            safe_kill_requested_at: None,
            idle_since: None,
            stuck_since: since,
            last_playbook_at: last,
            last_prompt: None,
            started_at: None,
            last_turn_at: None,
            ci_status: None,
            turn_seq: 0,
            last_stop_at: None,
            parent_session_id: None,
            tags: Vec::new(),
        }
    }

    const ALL_ON: PlaybookConfig = PlaybookConfig {
        press_enter: true,
        oom_recreate: true,
    };

    #[test]
    fn keystroke_playbooks_default_off_and_gate_per_kind() {
        let rows = vec![
            row(1, "a", Some("press_enter"), Some(100), None),
            row(2, "b", Some("oom"), Some(100), None),
            row(3, "c", Some("auth_menu"), Some(100), None),
        ];
        let off = PlaybookConfig::default();
        let planned = plan(&rows, &off, None, 200);
        // Only the notify-only kind survives with everything off.
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].session_id, 3);
        assert_eq!(planned[0].action, PlaybookAction::Notify);

        let planned = plan(&rows, &ALL_ON, None, 200);
        let actions: Vec<_> = planned.iter().map(|p| (p.session_id, p.action)).collect();
        assert_eq!(
            actions,
            vec![
                (1, PlaybookAction::PressEnter),
                (2, PlaybookAction::Recreate),
                (3, PlaybookAction::Notify)
            ]
        );
    }

    #[test]
    fn plan_skips_rows_already_handled_this_episode() {
        // last_playbook_at at/after stuck_since ⇒ same episode ⇒ skip.
        let rows = vec![
            row(1, "a", Some("press_enter"), Some(100), Some(100)),
            row(2, "b", Some("press_enter"), Some(100), Some(150)),
            // A NEW episode (stuck_since moved past the last playbook) runs again.
            row(3, "c", Some("press_enter"), Some(300), Some(150)),
        ];
        let ids: Vec<_> = plan(&rows, &ALL_ON, None, 400)
            .iter()
            .map(|p| p.session_id)
            .collect();
        assert_eq!(ids, vec![3]);
    }

    #[test]
    fn plan_requires_stuck_since_and_live_tmux_row() {
        let mut ghost = row(1, "g", Some("press_enter"), Some(1), None);
        ghost.status = "ghost".into();
        let mut bg = row(2, "bg:x", Some("press_enter"), Some(1), None);
        bg.kind = "bg".into();
        let no_stamp = row(3, "n", Some("press_enter"), None, None);
        let not_stuck = row(4, "ok", None, Some(1), None);
        assert!(plan(&[ghost, bg, no_stamp, not_stuck], &ALL_ON, None, 10).is_empty());
    }

    #[test]
    fn oom_recreate_is_rate_limited_to_once_per_hour() {
        // New episode 10 minutes after the last recreate ⇒ still throttled.
        let throttled = row(1, "a", Some("oom"), Some(5000), Some(4400));
        assert!(plan(&[throttled], &ALL_ON, None, 5000).is_empty());
        // Same shape but the last recreate was over an hour ago ⇒ runs.
        let due = row(2, "b", Some("oom"), Some(9000), Some(4400));
        let planned = plan(&[due], &ALL_ON, None, 9000);
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].action, PlaybookAction::Recreate);
    }

    #[test]
    fn keystrokes_never_target_the_controller_but_notify_does() {
        let ctl = ("local".to_string(), "ctl".to_string());
        let rows = vec![
            row(1, "ctl", Some("press_enter"), Some(1), None),
            row(2, "ctl", Some("oom"), Some(1), None),
            row(3, "ctl", Some("reconnect"), Some(1), None),
        ];
        let planned = plan(&rows, &ALL_ON, Some(&ctl), 10);
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].action, PlaybookAction::Notify);
    }

    #[test]
    fn pane_is_attached_only_trusts_a_clean_zero() {
        assert!(!pane_is_attached("0\n"));
        assert!(pane_is_attached("1\n"));
        assert!(pane_is_attached("2"));
        assert!(pane_is_attached(""));
        assert!(pane_is_attached("can't find session: x"));
        assert_eq!(
            attached_probe_script("dev-x"),
            "tmux display -p -t 'dev-x' '#{session_attached}'"
        );
    }

    #[tokio::test]
    async fn run_with_records_an_attached_skip_without_retrying() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let id = seed_stuck(&store, "dev-att", "press_enter");
        let exec = FakeExec {
            enters: AtomicUsize::new(0),
            recreates: AtomicUsize::new(0),
            fail: false,
            attached: true,
        };
        let now = now_unix() + 10;
        assert_eq!(run_with(&store, &exec, &ALL_ON, now).await, 1);
        assert_eq!(run_with(&store, &exec, &ALL_ON, now + 1).await, 0);
        let s = store.lock().unwrap();
        let events = s.list_session_events(id, 10).unwrap();
        assert!(events.iter().any(|e| e.kind == "playbook_applied"
            && e.detail.as_deref() == Some("press_enter:press_enter:skipped:attached")));
    }

    #[test]
    fn press_enter_script_quotes_the_session_name() {
        assert_eq!(
            press_enter_script("dev-x's"),
            "tmux send-keys -t 'dev-x'\\''s' Enter"
        );
    }

    struct FakeExec {
        enters: AtomicUsize,
        recreates: AtomicUsize,
        fail: bool,
        attached: bool,
    }

    #[async_trait::async_trait]
    impl PlaybookExec for FakeExec {
        async fn press_enter(&self, _h: &str, _t: &str) -> Result<PressEnterOutcome, IpcError> {
            self.enters.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                Err(IpcError::new("E_TMUX", "boom"))
            } else if self.attached {
                Ok(PressEnterOutcome::SkippedAttached)
            } else {
                Ok(PressEnterOutcome::Sent)
            }
        }
        async fn recreate(&self, _id: i64) -> Result<(), IpcError> {
            self.recreates.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    /// Seed a live, stuck work session through the real reconcile upsert so
    /// `stuck_since` is stamped the way production stamps it.
    fn seed_stuck(store: &Mutex<Store>, name: &str, kind: &str) -> i64 {
        let mut s = store.lock().unwrap();
        s.upsert_host("local").unwrap();
        s.apply_host_reconcile(crate::store::HostReconcile {
            alias: "local",
            reachable: true,
            claude_version: None,
            tmux_version: None,
            last_pinged_at: 1,
            probe_started_at: 0,
            sessions: &[crate::store::ReconcileSession {
                tmux_name: name,
                project_id: None,
                created_at: 1,
                last_activity_at: 1,
                account_uuid: None,
                worktree_key: None,
                claude_session_id: None,
                claude_status: Some("blocked".into()),
                effort_level: None,
                pr_url: None,
                current_activity: None,
                context_pct: None,
                stuck_kind: Some(kind.into()),
                intel_observed: true,
                ci_status: None,
                pr_observed: false,
            }],
            keep: &[name.to_string()],
        })
        .unwrap();
        s.get_session(name, "local").unwrap().unwrap().id
    }

    #[tokio::test]
    async fn run_with_applies_once_per_episode_and_records_the_event() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let id = seed_stuck(&store, "dev-a", "press_enter");
        let exec = FakeExec {
            enters: AtomicUsize::new(0),
            recreates: AtomicUsize::new(0),
            fail: false,
            attached: false,
        };
        let now = now_unix() + 10;
        assert_eq!(run_with(&store, &exec, &ALL_ON, now).await, 1);
        assert_eq!(exec.enters.load(Ordering::SeqCst), 1);
        // Second tick, same episode: nothing happens.
        assert_eq!(run_with(&store, &exec, &ALL_ON, now + 20).await, 0);
        assert_eq!(exec.enters.load(Ordering::SeqCst), 1);

        let s = store.lock().unwrap();
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.last_playbook_at, Some(now));
        let events = s.list_session_events(id, 10).unwrap();
        let applied: Vec<_> = events
            .iter()
            .filter(|e| e.kind == "playbook_applied")
            .map(|e| e.detail.clone().unwrap_or_default())
            .collect();
        assert_eq!(applied, vec!["press_enter:press_enter"]);
    }

    #[tokio::test]
    async fn run_with_stamps_a_failed_keystroke_so_it_is_not_retried() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let id = seed_stuck(&store, "dev-b", "press_enter");
        let exec = FakeExec {
            enters: AtomicUsize::new(0),
            recreates: AtomicUsize::new(0),
            fail: true,
            attached: false,
        };
        let now = now_unix() + 10;
        assert_eq!(run_with(&store, &exec, &ALL_ON, now).await, 1);
        assert_eq!(run_with(&store, &exec, &ALL_ON, now + 1).await, 0);
        assert_eq!(exec.enters.load(Ordering::SeqCst), 1);
        let s = store.lock().unwrap();
        let events = s.list_session_events(id, 10).unwrap();
        let detail = events
            .iter()
            .find(|e| e.kind == "playbook_applied")
            .and_then(|e| e.detail.clone())
            .unwrap();
        assert!(
            detail.starts_with("press_enter:press_enter:failed:"),
            "{detail}"
        );
    }

    #[tokio::test]
    async fn run_with_notify_only_kinds_touch_nothing_but_the_timeline() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let id = seed_stuck(&store, "dev-c", "auth_menu");
        let exec = FakeExec {
            enters: AtomicUsize::new(0),
            recreates: AtomicUsize::new(0),
            fail: false,
            attached: false,
        };
        assert_eq!(
            run_with(&store, &exec, &PlaybookConfig::default(), now_unix() + 5).await,
            1
        );
        assert_eq!(exec.enters.load(Ordering::SeqCst), 0);
        assert_eq!(exec.recreates.load(Ordering::SeqCst), 0);
        let s = store.lock().unwrap();
        let events = s.list_session_events(id, 10).unwrap();
        assert!(events.iter().any(
            |e| e.kind == "playbook_applied" && e.detail.as_deref() == Some("auth_menu:notify")
        ));
    }
}
