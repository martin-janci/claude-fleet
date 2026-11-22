//! Stuck-session playbooks (PROD-2): per-`stuck_kind` remediation run by the
//! background tick right after a reconcile pass.
//!
//! | kind          | action                                   | gate                       |
//! |---------------|------------------------------------------|----------------------------|
//! | press_enter   | send Enter to the pane                   | `playbooks.press_enter`    |
//! | oom           | recreate (resume by claude_session_id)   | `playbooks.oom_recreate`, ≤1/h, ≤`playbooks.oom_max_attempts` per 24 h, never while working |
//! | auth_menu, trust_prompt, reconnect | notify only (row + timeline event) | always |
//!
//! Every action runs at most once per stuck *episode*: reconcile stamps
//! `stuck_since` when a `stuck_kind` appears (or changes), and a row whose
//! `last_playbook_at` is at/after that stamp is skipped. Both keystroke
//! playbooks default OFF so an upgrade changes nothing until the operator
//! opts in. The planner is pure; the executor is injectable for tests.

use crate::ipc_error::{codes, IpcError};
use crate::service::settings;
use crate::shell::quote;
use crate::ssh::SshClient;
use crate::store::{SessionRow, Store};
use std::sync::{Arc, Mutex};

/// Minimum spacing between two `oom` recreates of the same session.
pub const OOM_RECREATE_MIN_SPACING_SECS: i64 = 3600;

/// Recreates one session may get per [`OOM_ATTEMPT_WINDOW_SECS`]
/// (`playbooks.oom_max_attempts`): the F1 loop was one session recreated
/// twice in 83 minutes on a word it was reading, and a spacing is not a
/// budget.
pub const OOM_ATTEMPT_WINDOW_SECS: i64 = 86_400;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlaybookConfig {
    pub press_enter: bool,
    pub oom_recreate: bool,
    /// Recreates one session may get per [`OOM_ATTEMPT_WINDOW_SECS`];
    /// `0` refuses every recreate (the toggle stays on so refusals are logged).
    pub oom_max_attempts: u32,
}

impl PlaybookConfig {
    pub fn from_store(s: &Store) -> Self {
        Self {
            press_enter: settings::get_bool(s, settings::PLAYBOOK_PRESS_ENTER),
            oom_recreate: settings::get_bool(s, settings::PLAYBOOK_OOM_RECREATE),
            oom_max_attempts: settings::get_string(s, settings::PLAYBOOK_OOM_MAX_ATTEMPTS)
                .parse()
                .unwrap_or(2),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybookAction {
    PressEnter,
    Recreate,
    /// No keystrokes: only the `playbook_applied` timeline entry + row event.
    Notify,
    /// A keystroke playbook that would have run but was refused. The reason
    /// goes on the timeline as `<kind>:<action>:skipped:<why>` and the
    /// episode is stamped, so the refusal is written once, not every tick.
    Skipped {
        action: &'static str,
        why: &'static str,
    },
}

impl PlaybookAction {
    fn as_str(self) -> &'static str {
        match self {
            PlaybookAction::PressEnter => "press_enter",
            PlaybookAction::Recreate => "recreate",
            PlaybookAction::Notify => "notify",
            PlaybookAction::Skipped { action, .. } => action,
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

/// Pure: decide which rows get which playbook this tick. See
/// [`plan_with_attempts`]; this shape (no attempt counts) is what every
/// caller that has no timeline in hand uses.
pub fn plan(
    rows: &[SessionRow],
    cfg: &PlaybookConfig,
    controller: Option<&(String, String)>,
    now: i64,
) -> Vec<Planned> {
    plan_with_attempts(
        rows,
        cfg,
        controller,
        now,
        &std::collections::HashMap::new(),
    )
}

/// Why an `oom` recreate must NOT run on this row right now, or `None` when
/// it may. `since` is the row's `stuck_since`; `attempts` how many recreates
/// the session already got in the window.
///
/// * `working`: the UserPromptSubmit hook says a turn is in flight — the
///   OOM text is scrollback, and a recreate destroys the turn (F1).
/// * `turn_after_stuck`: a Stop hook landed at/after the episode began, so
///   the REPL outlived whatever printed the text.
/// * `attempts`: the budget is spent.
pub fn oom_recreate_refusal(
    row: &SessionRow,
    since: i64,
    attempts: u32,
    max_attempts: u32,
) -> Option<&'static str> {
    if row.claude_status.as_deref() == Some("working") {
        return Some("working");
    }
    if row.last_turn_at.is_some_and(|t| t >= since) {
        return Some("turn_after_stuck");
    }
    if attempts >= max_attempts {
        return Some("attempts");
    }
    None
}

/// [`plan`] with each session's recreate count over
/// [`OOM_ATTEMPT_WINDOW_SECS`] (`oom_attempts`, by session id; absent = 0).
///
/// A row qualifies when it is a live tmux session (`status == running`, not a
/// pane-less `bg` / `external` sentinel) with a `stuck_kind` AND a `stuck_since` stamp that is newer
/// than its `last_playbook_at`. Keystroke actions never target the registered
/// controller session (it would be steering itself); notify still applies.
pub fn plan_with_attempts(
    rows: &[SessionRow],
    cfg: &PlaybookConfig,
    controller: Option<&(String, String)>,
    now: i64,
    oom_attempts: &std::collections::HashMap<i64, u32>,
) -> Vec<Planned> {
    let mut out = Vec::new();
    for r in rows {
        if r.status != "running" || crate::store::has_no_pane(&r.kind) {
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
                let attempts = oom_attempts.get(&r.id).copied().unwrap_or(0);
                match oom_recreate_refusal(r, since, attempts, cfg.oom_max_attempts) {
                    Some(why) => PlaybookAction::Skipped {
                        action: "recreate",
                        why,
                    },
                    None => PlaybookAction::Recreate,
                }
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
    format!(
        "tmux send-keys -t {} Enter",
        quote(&crate::tmux::exact_pane(tmux_name))
    )
}

/// Script that prints the number of clients attached to the session.
pub fn attached_probe_script(tmux_name: &str) -> String {
    format!(
        "tmux display -p -t {} '#{{session_attached}}'",
        quote(&crate::tmux::exact_pane(tmux_name))
    )
}

/// Pure: decide from `tmux display '#{session_attached}'` output whether a
/// keystroke may be sent. Anything but a clean `0` (an attached client, a
/// vanished session, garbage) means "do not type into this pane".
pub fn pane_is_attached(stdout: &str) -> bool {
    !matches!(stdout.trim().parse::<u32>(), Ok(0))
}

/// `press_enter` on any executor: skip the pane when a person is attached
/// to it (or the probe cannot tell), else send Enter. Autofix's stuck
/// fixes (`service::autofix::stuck`) press through this too.
pub async fn press_enter_on(
    ssh: &dyn crate::ssh::SshExec,
    host_alias: &str,
    tmux_name: &str,
) -> Result<PressEnterOutcome, IpcError> {
    crate::validate::host_alias(host_alias)?;
    crate::validate::tmux_name_addressable(tmux_name)?;
    let timeout = std::time::Duration::from_secs(10);
    let attached =
        crate::ssh::run_shell(ssh, host_alias, &attached_probe_script(tmux_name), timeout).await?;
    if !attached.status.success() || pane_is_attached(&String::from_utf8_lossy(&attached.stdout)) {
        return Ok(PressEnterOutcome::SkippedAttached);
    }
    let out =
        crate::ssh::run_shell(ssh, host_alias, &press_enter_script(tmux_name), timeout).await?;
    if !out.status.success() {
        return Err(IpcError::new(
            codes::E_TMUX,
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(PressEnterOutcome::Sent)
}

/// Script that prints the session's visible pane.
pub fn capture_pane_script(tmux_name: &str) -> String {
    format!(
        "tmux capture-pane -p -t {}",
        quote(&crate::tmux::exact_pane(tmux_name))
    )
}

/// Where the row that says yes sits in a folder-trust dialog, seen from the
/// focused row ([`trust_yes_move`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustMove {
    /// The focused row is the yes: Enter confirms it.
    Stay,
    /// The yes is the row below the focused one.
    Down,
    /// The yes is the row above the focused one.
    Up,
}

/// PURE: read a captured pane as a folder-trust dialog and say how to reach
/// its yes row. `None` for a layout this does not recognise, which sends no
/// key at all.
///
/// The layouts it reads (fixtures in the tests and in `pane_intel`):
/// Claude Code's numbered one with the yes first and focused, its current
/// unnumbered one with `No, exit` first and focused (2.1.322 on), and
/// Codex's `Trust and continue`. The rule is the same for all: exactly one
/// focused row, exactly one row that says yes, and the two the same row or
/// neighbours. The dialog is recognised by its own question above the
/// choices ([`TRUST_QUESTIONS`]), not by the word "trust" anywhere: a
/// permission prompt for a command that says `--trust-server-cert` has a
/// focused `Yes` too, and Enter there approves the command. The yes row is
/// also one of the last rows of the pane, where a dialog draws its choices.
pub fn trust_yes_move(pane: &str) -> Option<TrustMove> {
    let rows: Vec<(bool, &str)> = pane
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(choice_row)
        .collect();
    let mut focused = rows.iter().enumerate().filter(|(_, r)| r.0).map(|(i, _)| i);
    let (Some(at), None) = (focused.next(), focused.next()) else {
        return None;
    };
    let mut yes = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| says_yes(r.1))
        .map(|(i, _)| i);
    let (Some(yes), None) = (yes.next(), yes.next()) else {
        return None;
    };
    if yes + TRUST_TAIL_ROWS < rows.len() {
        return None;
    }
    let asked = rows[..at.min(yes)]
        .iter()
        .map(|r| r.1.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ");
    if !TRUST_QUESTIONS.iter().any(|q| asked.contains(q)) {
        return None;
    }
    if yes == at {
        Some(TrustMove::Stay)
    } else if yes == at + 1 {
        Some(TrustMove::Down)
    } else if yes + 1 == at {
        Some(TrustMove::Up)
    } else {
        None
    }
}

/// Words only a folder-trust dialog's question uses: Claude Code's current
/// ("one you trust"), its numbered ("trust this folder") and oldest ("trust
/// the files") layouts, and Codex's ("Trust this folder?").
const TRUST_QUESTIONS: [&str; 3] = ["one you trust", "trust this folder", "trust the files"];

/// How many non-blank rows may follow the yes row: the other choice and the
/// key hint line, with one to spare.
const TRUST_TAIL_ROWS: usize = 4;

/// The cursor a select dialog draws on its focused row: Claude Code's, and
/// Codex's.
const DIALOG_CURSORS: [char; 2] = ['❯', '›'];

/// PURE: one pane row as a dialog choice: whether the cursor is on it, and
/// its text without the cursor and the `1. ` numbering.
fn choice_row(line: &str) -> (bool, &str) {
    let text = line.trim();
    let (focused, text) = match text.strip_prefix(DIALOG_CURSORS) {
        Some(rest) => (true, rest.trim_start()),
        None => (false, text),
    };
    let unnumbered = text.trim_start_matches(|c: char| c.is_ascii_digit());
    let text = match unnumbered.strip_prefix('.') {
        Some(rest) if unnumbered.len() < text.len() => rest.trim_start(),
        _ => text,
    };
    (focused, text)
}

/// PURE: whether a choice's text is the one that trusts the folder.
fn says_yes(text: &str) -> bool {
    let lower = text.to_lowercase();
    ["yes", "trust and continue"].iter().any(|word| {
        lower
            .strip_prefix(word)
            .is_some_and(|rest| !rest.starts_with(|c: char| c.is_alphanumeric()))
    })
}

/// How long the dialog gets to redraw after an arrow key before the pane
/// is read again.
const TRUST_MOVE_SETTLE: std::time::Duration = std::time::Duration::from_millis(400);

/// Answer the folder-trust dialog in a pane with its yes row.
///
/// Enter alone answers whichever row is focused, and since Claude Code
/// 2.1.322 that row is `No, exit`: the Enter autofix used to press ended
/// the session it was meant to free (2026-11-17). So the pane is read
/// first ([`trust_yes_move`]); an arrow key moves the cursor when the yes
/// is not the focused row, and Enter follows only once a second read shows
/// the cursor on it. A layout it does not recognise gets no key at all
/// (`E_INVALID`), and neither does a pane a person is attached to.
pub async fn accept_trust_on(
    ssh: &dyn crate::ssh::SshExec,
    host_alias: &str,
    tmux_name: &str,
) -> Result<PressEnterOutcome, IpcError> {
    use crate::tmux::NamedKey;
    crate::validate::host_alias(host_alias)?;
    crate::validate::tmux_name_addressable(tmux_name)?;
    let timeout = std::time::Duration::from_secs(10);
    let run = |script: String| async move {
        let out = crate::ssh::run_shell(ssh, host_alias, &script, timeout).await?;
        if !out.status.success() {
            return Err(IpcError::new(
                codes::E_TMUX,
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let attached =
        crate::ssh::run_shell(ssh, host_alias, &attached_probe_script(tmux_name), timeout).await?;
    if !attached.status.success() || pane_is_attached(&String::from_utf8_lossy(&attached.stdout)) {
        return Ok(PressEnterOutcome::SkippedAttached);
    }
    let arrow = match trust_yes_move(&run(capture_pane_script(tmux_name)).await?) {
        None => {
            return Err(IpcError::new(
                codes::E_INVALID,
                "fleet does not recognise the trust dialog that session shows, so nothing was \
                 typed into it; answer it in the session",
            ))
        }
        Some(TrustMove::Stay) => None,
        Some(TrustMove::Down) => Some(NamedKey::Down),
        Some(TrustMove::Up) => Some(NamedKey::Up),
    };
    if let Some(key) = arrow {
        run(crate::tmux::send_named_key(tmux_name, key)).await?;
        tokio::time::sleep(TRUST_MOVE_SETTLE).await;
        let now = trust_yes_move(&run(capture_pane_script(tmux_name)).await?);
        if now != Some(TrustMove::Stay) {
            return Err(IpcError::new(
                codes::E_INVALID,
                "the cursor did not reach the trust dialog's yes row, so nothing was confirmed; \
                 answer it in the session",
            ));
        }
    }
    run(press_enter_script(tmux_name)).await?;
    Ok(PressEnterOutcome::Sent)
}

/// A person's Enter into a session stopped at the folder-trust dialog,
/// answered with its yes row ([`accept_trust_on`]) instead of a bare Enter.
///
/// Every client offers Enter as the trust answer: the phone's "Trust this
/// folder · Enter" (`send_prompt { keys: "Enter" }`), an empty prompt, and
/// Control told to press it. Since Claude Code 2.1.322 the focused row is
/// `No, exit`, so the bare key ended the session it was meant to free. Done
/// here, on the hub, a phone that predates the fix is fixed too.
///
/// `None` when the row is not at `stuck_kind: trust_prompt`: the caller
/// presses its Enter as before. A pane someone is attached to gets no key
/// (`E_INVALID_STATE`); a dialog fleet cannot read gets none either
/// (`E_INVALID`, from [`accept_trust_on`]).
pub async fn enter_answers_trust(
    store: &Mutex<Store>,
    ssh: &dyn crate::ssh::SshExec,
    host_alias: &str,
    tmux_name: &str,
) -> Option<Result<(), IpcError>> {
    let at_trust = {
        let s = store.lock().ok()?;
        s.get_session(tmux_name, host_alias)
            .ok()
            .flatten()
            .is_some_and(|r| r.stuck_kind.as_deref() == Some("trust_prompt"))
    };
    if !at_trust {
        return None;
    }
    Some(match accept_trust_on(ssh, host_alias, tmux_name).await {
        Ok(PressEnterOutcome::Sent) => Ok(()),
        Ok(PressEnterOutcome::SkippedAttached) => Err(IpcError::new(
            codes::E_INVALID_STATE,
            "someone is attached to that session's terminal, so fleet typed nothing into its \
             folder-trust dialog; answer it there",
        )),
        Err(e) => Err(e),
    })
}

#[async_trait::async_trait]
impl PlaybookExec for RealPlaybookExec {
    async fn press_enter(
        &self,
        host_alias: &str,
        tmux_name: &str,
    ) -> Result<PressEnterOutcome, IpcError> {
        press_enter_on(self.ssh.as_ref(), host_alias, tmux_name).await
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
    let (rows, controller, attempts) = {
        let Ok(s) = store.lock() else {
            return 0;
        };
        // Pause all (redesign 8.1): no keystroke, no recreate. The tick's
        // gate records the pause; this stops any other caller too.
        if crate::service::loops::paused(&s) {
            return 0;
        }
        let rows = s.list_all_sessions().unwrap_or_default();
        let controller = s.get_controller().ok().flatten();
        let attempts: std::collections::HashMap<i64, u32> = rows
            .iter()
            .filter(|r| r.stuck_kind.as_deref() == Some("oom"))
            .map(|r| {
                (
                    r.id,
                    s.count_oom_recreates_since(r.id, now - OOM_ATTEMPT_WINDOW_SECS)
                        .unwrap_or(0),
                )
            })
            .collect();
        (rows, controller, attempts)
    };
    let planned = plan_with_attempts(&rows, cfg, controller.as_ref(), now, &attempts);
    let mut applied = 0;
    for p in planned {
        let result = match p.action {
            PlaybookAction::PressEnter => exec.press_enter(&p.host_alias, &p.tmux_name).await,
            PlaybookAction::Recreate => exec
                .recreate(p.session_id)
                .await
                .map(|()| PressEnterOutcome::Sent),
            PlaybookAction::Notify | PlaybookAction::Skipped { .. } => Ok(PressEnterOutcome::Sent),
        };
        let detail = match (p.action, &result) {
            (PlaybookAction::Skipped { action, why }, _) => {
                format!("{}:{action}:skipped:{why}", p.stuck_kind)
            }
            (_, Ok(PressEnterOutcome::Sent)) => format!("{}:{}", p.stuck_kind, p.action.as_str()),
            (_, Ok(PressEnterOutcome::SkippedAttached)) => {
                format!("{}:{}:skipped:attached", p.stuck_kind, p.action.as_str())
            }
            (_, Err(e)) => format!(
                "{}:{}:failed:{}",
                p.stuck_kind,
                p.action.as_str(),
                e.message
            ),
        };
        if let Err(e) = &result {
            tracing::warn!(
                action = p.action.as_str(),
                host = %p.host_alias,
                session = %p.tmux_name,
                error = %e,
                "[playbook] action failed"
            );
        }
        if let Ok(outcome) = &result {
            tracing::info!(
                host = %p.host_alias,
                session = %p.tmux_name,
                kind = %p.stuck_kind,
                action = %p.action.as_str(),
                outcome = ?outcome,
                "[playbook] applied"
            );
        }
        if let Ok(s) = store.lock() {
            match s.mark_playbook_applied(p.session_id, now, &detail) {
                Ok(_) => applied += 1,
                Err(e) => tracing::warn!(
                    session_id = p.session_id,
                    error = %e,
                    "[playbook] stamping the session failed"
                ),
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

use crate::store::now_unix;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
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
            row_version: 0,
            prompt_submit_seq: 0,
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
            lost_reason: None,
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
            stale_working_at: None,
            stale_demoted_at: None,
            work_rev: 0,
            pr_evidence: None,
            pr_checked_at: None,
            owner_person_id: None,
            visibility: crate::store::VISIBILITY_UNCLAIMED.into(),
            claude_profile: None,
            agent: crate::store::AGENT_CLAUDE.into(),
            origin: None,
            origin_ref: None,
            last_viewed_at: None,
            turn_outcome: None,
            pinned_at: None,
            prompt_suggestion: None,
            proposals: Vec::new(),
            pending_form: None,
            form_draft: None,
            parent_session_id: None,
            tags: Vec::new(),
            usage: Default::default(),
            context: Default::default(),
            pending_input: None,
            work: None,
            work_rejected: vec![],
            work_suggested: None,
            org_id: None,
        }
    }

    const ALL_ON: PlaybookConfig = PlaybookConfig {
        press_enter: true,
        oom_recreate: true,
        oom_max_attempts: 2,
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
    fn plan_never_targets_external_rows() {
        let mut ext = row(1, "bg:x", Some("press_enter"), Some(1), None);
        ext.kind = "external".into();
        assert!(plan(&[ext], &ALL_ON, None, 10).is_empty());
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

    fn oom_row(id: i64, since: i64) -> SessionRow {
        row(id, "dev-oom", Some("oom"), Some(since), None)
    }

    #[test]
    fn oom_recreate_is_refused_while_the_row_is_working_and_says_so() {
        let mut r = oom_row(1, 100);
        r.claude_status = Some("working".into());
        let planned = plan(&[r], &ALL_ON, None, 200);
        assert_eq!(
            planned.len(),
            1,
            "the refusal is planned so it is written once"
        );
        assert_eq!(
            planned[0].action,
            PlaybookAction::Skipped {
                action: "recreate",
                why: "working"
            }
        );
    }

    #[test]
    fn oom_recreate_is_refused_when_a_turn_ended_after_the_episode_began() {
        let mut r = oom_row(1, 100);
        r.claude_status = Some("idle".into());
        r.last_turn_at = Some(150);
        assert_eq!(
            plan(&[r], &ALL_ON, None, 200)[0].action,
            PlaybookAction::Skipped {
                action: "recreate",
                why: "turn_after_stuck"
            }
        );
        // A turn that ended BEFORE the flag is no evidence of life.
        let mut r = oom_row(2, 100);
        r.last_turn_at = Some(50);
        assert_eq!(
            plan(&[r], &ALL_ON, None, 200)[0].action,
            PlaybookAction::Recreate
        );
    }

    #[test]
    fn oom_recreate_stops_at_the_attempt_budget() {
        let r = oom_row(1, 100);
        let mut spent = HashMap::new();
        spent.insert(1, 2);
        assert_eq!(
            plan_with_attempts(std::slice::from_ref(&r), &ALL_ON, None, 200, &spent)[0].action,
            PlaybookAction::Skipped {
                action: "recreate",
                why: "attempts"
            }
        );
        spent.insert(1, 1);
        assert_eq!(
            plan_with_attempts(&[r], &ALL_ON, None, 200, &spent)[0].action,
            PlaybookAction::Recreate
        );
    }

    #[tokio::test]
    async fn run_with_counts_recreates_over_the_window_and_records_the_refusal() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let id = seed_stuck(&store, "dev-oom", "oom");
        let exec = FakeExec {
            enters: AtomicUsize::new(0),
            recreates: AtomicUsize::new(0),
            fail: false,
            attached: false,
        };
        let now = now_unix() + 10;
        // Two recreates already inside the 24 h window, the last of them
        // past the 1 h spacing — the spacing alone would let a third run.
        {
            let s = store.lock().unwrap();
            s.mark_playbook_applied(id, now - 7200, "oom:recreate")
                .unwrap();
            s.mark_playbook_applied(id, now - 3601, "oom:recreate")
                .unwrap();
        }
        assert_eq!(run_with(&store, &exec, &ALL_ON, now).await, 1);
        assert_eq!(
            exec.recreates.load(Ordering::SeqCst),
            0,
            "the budget is spent"
        );
        assert_eq!(
            run_with(&store, &exec, &ALL_ON, now + 20).await,
            0,
            "written once per episode"
        );
        let s = store.lock().unwrap();
        let events = s.list_session_events(id, 10).unwrap();
        assert!(events.iter().any(|e| e.kind == "playbook_applied"
            && e.detail.as_deref() == Some("oom:recreate:skipped:attempts")));
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
            "tmux display -p -t '=dev-x:' '#{session_attached}'"
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

    /// Claude Code 2.1.326 (and 2.1.322, `pane_intel`'s fixture): unnumbered
    /// rows, `No, exit` first and focused.
    const TRUST_NO_FIRST: &str = "\
 Accessing workspace:

 /mnt/sda4/projects/papayapos-backend

 Quick safety check: Is this a project you created or one you trust? (Like your own code, a well-known open source project, or work from your team).

 Claude Code'll be able to read, edit, and execute files here.

 Security guide

 ❯ No, exit
   Yes, I trust this folder

 Enter to confirm · Esc to cancel
";
    /// The same dialog once Down moved the cursor.
    const TRUST_NO_FIRST_ON_YES: &str = "\
 Quick safety check: Is this a project you created or one you trust?

   No, exit
 ❯ Yes, I trust this folder

 Enter to confirm · Esc to cancel
";
    /// The numbered layout, yes first and focused.
    const TRUST_YES_FIRST: &str =
        "Quick safety check: Is this a project you created or one you trust?\n\
         ❯ 1. Yes, I trust this folder\n  2. No, exit\n\nEnter to confirm · Esc to cancel\n";
    /// The oldest layout.
    const TRUST_OLD: &str =
        "Do you trust the files in this folder?\n ❯ 1. Yes, proceed\n   2. No\nEnter to select";
    const TRUST_CODEX: &str = include_str!("../agent_adapter/testdata/codex/trust_folder.txt");

    #[test]
    fn the_yes_row_is_found_in_every_layout_fleet_knows() {
        assert_eq!(trust_yes_move(TRUST_NO_FIRST), Some(TrustMove::Down));
        assert_eq!(trust_yes_move(TRUST_NO_FIRST_ON_YES), Some(TrustMove::Stay));
        assert_eq!(trust_yes_move(TRUST_YES_FIRST), Some(TrustMove::Stay));
        assert_eq!(trust_yes_move(TRUST_OLD), Some(TrustMove::Stay));
        assert_eq!(trust_yes_move(TRUST_CODEX), Some(TrustMove::Stay));
        // A yes above the focused row.
        assert_eq!(
            trust_yes_move("Do you trust the files in this folder?\n   Yes, proceed\n ❯ No\n"),
            Some(TrustMove::Up)
        );
        // Blank rows between the choices do not hide them.
        assert_eq!(
            trust_yes_move("one you trust?\n ❯ No, exit\n\n   Yes, I trust this folder\n"),
            Some(TrustMove::Down)
        );
    }

    #[test]
    fn a_layout_fleet_does_not_recognise_gets_no_move() {
        for (why, pane) in [
            ("nothing", ""),
            (
                "no cursor",
                "one you trust?\n   No, exit\n   Yes, I trust this folder\n",
            ),
            (
                "two cursors",
                "one you trust?\n ❯ No, exit\n ❯ Yes, I trust this folder\n",
            ),
            (
                "no yes row",
                "one you trust?\n ❯ No, exit\n   Maybe later\n",
            ),
            (
                "two yes rows",
                "one you trust?\n ❯ No, exit\n   Yes, I trust this folder\n   Yes, always\n",
            ),
            (
                "the yes is not next to the cursor",
                "one you trust?\n ❯ No, exit\n   Ask me later\n   Yes, I trust this folder\n",
            ),
            (
                "not a trust dialog",
                "Do you want to proceed?\n ❯ 1. Yes\n   2. No\n",
            ),
            (
                "a permission prompt for a command that mentions trust",
                "Bash command\n  psql --trust-server-cert -c 'select 1'\n  Update the truststore\n\
                 Do you want to proceed?\n ❯ 1. Yes\n   2. No, and tell Claude what to do differently\n\
                 Esc to cancel\n",
            ),
            (
                "the trust question only below the choices",
                "Do you want to proceed?\n ❯ 1. Yes\n   2. No\n  (do you trust the files?)\n",
            ),
            (
                "the yes row far above the end of the pane",
                "one you trust?\n ❯ Yes, I trust this folder\n   No, exit\n a\n b\n c\n d\n",
            ),
        ] {
            assert_eq!(trust_yes_move(pane), None, "{why}");
        }
    }

    /// A host whose `work-1` pane shows `first`, then `later` on every
    /// capture after it; nobody attached.
    fn trust_host(first: &str, later: &str) -> crate::ssh_fake::FakeSsh {
        use crate::ssh_fake::{FakeSsh, Match, Reply};
        let fake = FakeSsh::new();
        fake.on(
            Match::script(&attached_probe_script("work-1")),
            Reply::ok("0\n"),
        )
        .on(
            Match::script(&capture_pane_script("work-1")),
            Reply::ok(later),
        )
        .on_host_once(
            "h1",
            Match::script(&capture_pane_script("work-1")),
            Reply::ok(first),
        );
        fake
    }

    /// The keys sent to the pane, in order.
    fn keys_sent(fake: &crate::ssh_fake::FakeSsh) -> Vec<String> {
        fake.calls()
            .iter()
            .filter_map(|c| c.script())
            .filter_map(|s| {
                s.strip_prefix("tmux send-keys -t '=work-1:' ")
                    .map(str::to_string)
            })
            .collect()
    }

    /// 2026-11-17: the fix pressed Enter in a dialog whose focused row was
    /// `No, exit`, so approving it ended the session. The cursor goes to
    /// the yes first, and Enter follows only once the pane shows it there.
    #[tokio::test(start_paused = true)]
    async fn a_dialog_with_no_focused_is_answered_yes_not_exited() {
        let fake = trust_host(TRUST_NO_FIRST, TRUST_NO_FIRST_ON_YES);
        let out = accept_trust_on(&fake, "h1", "work-1").await.unwrap();
        assert_eq!(out, PressEnterOutcome::Sent);
        assert_eq!(keys_sent(&fake), ["Down", "Enter"]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_dialog_with_yes_focused_gets_enter_alone() {
        for pane in [TRUST_YES_FIRST, TRUST_OLD] {
            let fake = trust_host(pane, pane);
            let out = accept_trust_on(&fake, "h1", "work-1").await.unwrap();
            assert_eq!(out, PressEnterOutcome::Sent);
            assert_eq!(keys_sent(&fake), ["Enter"]);
        }
    }

    /// Enter is the key that can end the session, so it is never sent on a
    /// guess: not when the cursor did not land on the yes, not for a layout
    /// fleet cannot read, not into a pane someone is attached to.
    #[tokio::test(start_paused = true)]
    async fn enter_is_never_sent_unless_the_pane_shows_the_yes_focused() {
        let stayed = trust_host(TRUST_NO_FIRST, TRUST_NO_FIRST);
        let err = accept_trust_on(&stayed, "h1", "work-1").await.unwrap_err();
        assert_eq!(err.code, codes::E_INVALID, "{err:?}");
        assert_eq!(keys_sent(&stayed), ["Down"], "{}", err.message);

        let gone = trust_host(TRUST_NO_FIRST, "❯ \n");
        let err = accept_trust_on(&gone, "h1", "work-1").await.unwrap_err();
        assert_eq!(err.code, codes::E_INVALID, "{err:?}");
        assert_eq!(keys_sent(&gone), ["Down"]);

        let odd = "one you trust?\n   No, exit\n   Yes, I trust this folder\n";
        let unknown = trust_host(odd, odd);
        let err = accept_trust_on(&unknown, "h1", "work-1").await.unwrap_err();
        assert_eq!(err.code, codes::E_INVALID, "{err:?}");
        assert!(err.message.contains("nothing was typed"), "{}", err.message);
        assert!(keys_sent(&unknown).is_empty());

        let attached = trust_host(TRUST_NO_FIRST, TRUST_NO_FIRST_ON_YES);
        attached.on(
            crate::ssh_fake::Match::script(&attached_probe_script("work-1")),
            crate::ssh_fake::Reply::ok("1\n"),
        );
        let out = accept_trust_on(&attached, "h1", "work-1").await.unwrap();
        assert_eq!(out, PressEnterOutcome::SkippedAttached);
        assert!(keys_sent(&attached).is_empty());
    }

    /// w06-1: the phone's "Trust this folder · Enter" (and any other bare
    /// Enter) into a session at the trust dialog picks the yes row, not the
    /// focused `No, exit`; a session at any other state is left to the
    /// caller's own Enter.
    #[tokio::test(start_paused = true)]
    async fn an_enter_into_a_trust_prompt_answers_yes() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let id = {
            let s = store.lock().unwrap();
            s.upsert_host("h1").unwrap();
            s.upsert_session("work-1", "h1", None, None, 0, 0, "running", None)
                .unwrap()
        };
        let fake = trust_host(TRUST_NO_FIRST, TRUST_NO_FIRST_ON_YES);
        assert!(enter_answers_trust(&store, &fake, "h1", "work-1")
            .await
            .is_none());
        assert!(keys_sent(&fake).is_empty());

        store
            .lock()
            .unwrap()
            .conn_ref()
            .execute(
                "UPDATE sessions SET claude_status = 'blocked', stuck_kind = 'trust_prompt' WHERE id = ?1",
                rusqlite::params![id],
            )
            .unwrap();
        enter_answers_trust(&store, &fake, "h1", "work-1")
            .await
            .expect("routed")
            .unwrap();
        assert_eq!(keys_sent(&fake), ["Down", "Enter"]);

        let attached = trust_host(TRUST_NO_FIRST, TRUST_NO_FIRST_ON_YES);
        attached.on(
            crate::ssh_fake::Match::script(&attached_probe_script("work-1")),
            crate::ssh_fake::Reply::ok("1\n"),
        );
        let err = enter_answers_trust(&store, &attached, "h1", "work-1")
            .await
            .expect("routed")
            .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID_STATE, "{err:?}");
        assert!(keys_sent(&attached).is_empty());
    }

    #[test]
    fn press_enter_script_quotes_the_session_name() {
        assert_eq!(
            press_enter_script("dev-x's"),
            "tmux send-keys -t '=dev-x'\\''s:' Enter"
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
                Err(IpcError::new(codes::E_TMUX, "boom"))
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
                tmux_pane_id: None,
                pending_input: None,
                pane_working: false,
                pr_evidence: None,
            }],
            keep: &[name.to_string()],
            lost_ttl_cutoff: None,
            min_ghost_age_cutoff: None,
            skip_prune: false,
            reconciled_at: None,
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

    /// Redesign 8.1: Pause all stops the playbooks before they press a key.
    #[tokio::test]
    async fn pause_all_stops_the_playbooks() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let id = seed_stuck(&store, "dev-a", "press_enter");
        crate::service::settings::set(
            &store.lock().unwrap(),
            crate::service::settings::AUTOMATION_PAUSED,
            "true",
        )
        .unwrap();
        let exec = FakeExec {
            enters: AtomicUsize::new(0),
            recreates: AtomicUsize::new(0),
            fail: false,
            attached: false,
        };
        let now = now_unix() + 10;
        assert_eq!(run_with(&store, &exec, &ALL_ON, now).await, 0);
        assert_eq!(exec.enters.load(Ordering::SeqCst), 0);
        let row = store
            .lock()
            .unwrap()
            .get_session_by_id(id)
            .unwrap()
            .unwrap();
        assert_eq!(row.last_playbook_at, None);
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

    /// perf-logs §5: "applied 1 stuck playbook(s)" named no host, session or
    /// kind — the oom loop on 21480 was invisible in the log.
    #[tokio::test]
    async fn an_applied_playbook_logs_host_session_kind_and_action_at_info() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        seed_stuck(&store, "dev-log", "press_enter");
        let exec = FakeExec {
            enters: AtomicUsize::new(0),
            recreates: AtomicUsize::new(0),
            fail: false,
            attached: false,
        };
        let log = crate::logging::capture::start();
        assert_eq!(run_with(&store, &exec, &ALL_ON, now_unix() + 10).await, 1);
        let text = log.text();
        assert!(
            text.contains("INFO") && text.contains("[playbook] applied"),
            "{text}"
        );
        for field in [
            "host=local",
            "session=dev-log",
            "kind=press_enter",
            "action=press_enter",
        ] {
            assert!(text.contains(field), "missing {field} in:\n{text}");
        }
    }
}
