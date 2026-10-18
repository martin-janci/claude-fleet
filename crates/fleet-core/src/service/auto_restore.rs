//! Automatic recovery: a session the host killed comes back by itself.
//!
//! **Why.** A pane has no `remain-on-exit`, so when the host kills its agent
//! — the kernel's OOM killer, systemd-oomd (which since the per-pane scopes
//! of `fleet-agent` kills ONE pane rather than the whole tmux server), a
//! reboot — the tmux session ends with it. Reconcile then records the row as
//! lost: `process_died` for one pane whose hooks were alive and which never
//! said SessionEnd (`store::reconcile::PROCESS_DIED_CASE`), `tmux_server_gone`
//! or `host_reboot` for a mass loss. Before this module those rows waited for
//! a person to press Restore, and a single OOM-killed session simply sat in
//! the sidebar as a ghost. Now the background tick resumes them on its own.
//!
//! **Not silent.** The old reason for leaving them alone was that a session
//! may have stopped mid-sentence. That still holds, so every attempt leaves
//! exactly one timeline event — [`EVENT_RESTORED`], [`EVENT_FAILED`] or
//! [`EVENT_SKIPPED`] with why — which the timeline, the Inbox and Control's
//! event feed (`service::operator_feed`) read as "Recovered automatically
//! after its process stopped"; and the operator itself, when it is the one
//! that died, is told in its own conversation (a `[fleet]` note queued for
//! its next idle moment) so it can say what happened and carry on.
//!
//! **Control off a dead host.** When Control's own host stops answering, its
//! session cannot be restarted where it is. Once that host has been silent
//! for [`OPERATOR_MOVE_AFTER_SECS`] (or the grace, if longer), the tick runs
//! the same `ensure_operator` the desktop's panel runs on open, which starts
//! Control on a host that answers, within the same attempt budget. The move
//! is an [`EVENT_OPERATOR_MOVED`] on the stranded row, and the new Control
//! is told. Before this a fleet whose person had only the phone kept a dead
//! Control until someone opened the desktop (2026-10-15).
//!
//! **What qualifies.** A lost tmux row with a conversation to resume
//! (`claude_session_id`), a reason in [`RESUMABLE_LOST_REASONS`], lost at
//! least `recovery.grace_secs` ago (so a reboot or a lost tmux server has a
//! pass to be classified as one, and the memory that ran out has a moment to
//! settle), on an active, reachable host, with no safe-kill in flight. The
//! registered fleet controller is never touched (the batch restore refuses
//! it without `force`, as it always has). Pane-less (`bg` / `external`) and
//! `shell` rows have nothing to resume.
//!
//! **Bounds.** At most `recovery.max_attempts` attempts per session per
//! [`ATTEMPT_WINDOW_SECS`], failures included (counted from the timeline, so
//! the budget survives a restart of the hub), and at most
//! [`MAX_RESTORES_PER_TICK`] restores per tick, [`MAX_RESTORES_PER_HOST_PER_TICK`]
//! of them on any one host. A spent budget is refused once per loss episode
//! ([`EVENT_SKIPPED`]). A mass loss within [`MASS_LOSS_REFUND_SECS`] of the
//! session's last automatic restore gives that restore back: the host lost
//! everything, so the restore did not fail, and one storm must not spend
//! the budget a later loss needs (2026-10-16: `budget_spent` across a whole
//! host after two storms). Only the latest restore is given back, so a
//! host that keeps dropping its tmux server still runs out.
//!
//! **A mass loss waits for evidence.** `tmux_server_gone` and
//! `host_reboot` rows wait at least [`MASS_LOSS_GRACE_SECS`] and, on a host
//! that reports health, for a sample taken after the loss: whatever took
//! the server down has to show in a reading before a dozen sessions start
//! into it. Control goes first.
//!
//! **Load-aware, not a hard skip.** A host under load — memory at or under
//! the low-memory line (`health::mem_low`), memory pressure at or over
//! `recovery.max_mem_pressure` (what systemd-oomd acts on), too little
//! available memory for the restores this tick has launched there lately
//! plus one ([`RESTORE_MEM_KB`] each, over [`RESTORE_SETTLE_SECS`]) — those
//! three are the hard floor — or its CPU saturated
//! (`health::cpu_saturated`) — DEFERS the restore: no attempt is
//! spent, the row stays resumable, and the tick retries every pass until
//! the load drops (restoring into the same shortage only loops). One
//! [`EVENT_DEFERRED`] per episode says why, and the row's lost TTL is
//! extended once from that moment (`Store::ghost_and_clean`'s Phase 2 reads
//! the event), so a long overload cannot reap a session it was waiting to
//! bring back — bounded: one extension per episode.
//!
//! **Jev.** When a restore is due on a loaded host, or a recovery of the
//! session already failed once, the decision layer is asked
//! (`decide::restore_timing`: `recover_now`, `defer` or a move to a named
//! healthy host). Its answer is advisory and bounded: followed in `assist`
//! only, a `recover_now` never resumes past the low-memory floor, and a
//! move names only an active, reachable host with healthy memory in the
//! session's org — and since a lost session has no live source to carry a
//! worktree from, a move is honoured as a deferral that records the target
//! for Control and the person. Off, shadow or unanswered: the rule above.

use crate::ipc_error::{codes, IpcError};
use crate::service::decide::restore_timing::{self, Advice, Advised, HostFacts, RestoreQuestion};
use crate::service::settings;
use crate::ssh::SshClient;
use crate::store::{HostRow, SessionRow, Store, RESUMABLE_LOST_REASONS};
use async_trait::async_trait;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// The window `recovery.max_attempts` is counted over.
pub const ATTEMPT_WINDOW_SECS: i64 = 86_400;

/// At most this many restores per tick, across all hosts. A host that lost
/// twenty sessions to a reboot gets them back over a few ticks rather than
/// all at once (each tick is ~20 s; the batch restore staggers within).
pub const MAX_RESTORES_PER_TICK: usize = 4;

/// At most this many of a tick's restores on one host. Four at once into a
/// host that just lost its tmux server to memory pressure is how
/// 2026-10-16 relaunched ~25 sessions on one host in three minutes; one per
/// tick lets each start show in the next health sample first.
pub const MAX_RESTORES_PER_HOST_PER_TICK: usize = 1;

/// A `tmux_server_gone` / `host_reboot` row waits at least this long
/// (or `recovery.grace_secs`, if longer) before it is restored.
pub const MASS_LOSS_GRACE_SECS: i64 = 180;

/// The available memory (KiB) one resumed Claude session is assumed to
/// take while it starts. A host must hold this much per restore it got in
/// the last [`RESTORE_SETTLE_SECS`], plus one, before the next.
pub const RESTORE_MEM_KB: i64 = 600 * 1024;

/// How long a restore counts as still starting: its memory may not show
/// in the host's health sample yet.
pub const RESTORE_SETTLE_SECS: i64 = 120;

/// A mass loss this soon after a session's last automatic restore gives
/// that restore back to the budget (see the module doc's Bounds).
pub const MASS_LOSS_REFUND_SECS: i64 = 900;

/// The `lost_reason`s of a whole-host loss.
pub const MASS_LOSS_REASONS: &[&str] = &["tmux_server_gone", "host_reboot"];

/// Timeline event: the session was brought back. Detail: its `lost_reason`.
pub const EVENT_RESTORED: &str = "auto_restored";
/// Timeline event: an attempt failed. Detail: `<lost_reason>: <error>`.
pub const EVENT_FAILED: &str = "auto_restore_failed";
/// Timeline event: an attempt was refused. Detail: [`SKIP_BUDGET`].
pub const EVENT_SKIPPED: &str = "auto_restore_skipped";
/// Timeline event: the restore waits, once per episode. Detail: one of the
/// `DEFER_*` words, or `move:<alias>`; Jev's one-line reason follows
/// `: ` when it decided.
pub const EVENT_DEFERRED: &str = "auto_restore_deferred";

/// Timeline event on the stranded operator row: Control was started again
/// on another host because its own stopped answering. Detail: `<from>-><to>`.
pub const EVENT_OPERATOR_MOVED: &str = "operator_moved";
/// Timeline event on the stranded operator row: that move failed.
/// Detail: `<from>: <error>`.
pub const EVENT_OPERATOR_MOVE_FAILED: &str = "operator_move_failed";

/// Control is moved off a host only once that host has been silent this
/// long (or `recovery.grace_secs`, whichever is longer): a probe that fails
/// once on a flaky link must not cost Control its conversation.
pub const OPERATOR_MOVE_AFTER_SECS: i64 = 300;

/// [`EVENT_SKIPPED`] detail: `recovery.max_attempts` is spent.
pub const SKIP_BUDGET: &str = "budget_spent";
/// [`EVENT_DEFERRED`] detail: the host is short of memory.
pub const DEFER_MEMORY_LOW: &str = "memory_low";
/// [`EVENT_DEFERRED`] detail: the host's memory pressure is at or over
/// `recovery.max_mem_pressure`.
pub const DEFER_MEM_PRESSURE: &str = "memory_pressure";
/// [`EVENT_DEFERRED`] detail: the host lacks [`RESTORE_MEM_KB`] for each
/// restore still starting there, plus this one.
pub const DEFER_MEM_HEADROOM: &str = "memory_headroom";
/// [`EVENT_DEFERRED`] detail: the host's CPU is saturated.
pub const DEFER_CPU: &str = "cpu_saturated";
/// [`EVENT_DEFERRED`] detail: Jev chose to wait.
pub const DEFER_JEV: &str = "jev_defer";

/// The `recovery.*` settings in force.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutoRestoreConfig {
    pub enabled: bool,
    pub grace_secs: i64,
    pub max_attempts: u32,
    /// Available memory, percent of physical, at or under which a host is
    /// too short to restore onto (`health.mem_low_pct`, `health::mem_low`):
    /// the same line `fleet_health` judges `mem_low` against.
    pub mem_low_pct: f64,
    /// Memory pressure at or over which a host defers
    /// (`recovery.max_mem_pressure`).
    pub max_mem_pressure: f64,
}

impl AutoRestoreConfig {
    pub fn from_store(s: &Store) -> Self {
        Self {
            enabled: settings::get_bool(s, settings::RECOVERY_AUTO_RESTORE),
            grace_secs: settings::get_secs(s, settings::RECOVERY_GRACE_SECS) as i64,
            max_attempts: settings::get_string(s, settings::RECOVERY_MAX_ATTEMPTS)
                .parse()
                .unwrap_or(2),
            mem_low_pct: crate::service::health::load_limits(s).mem_low_pct,
            max_mem_pressure: settings::get_string(s, settings::RECOVERY_MAX_MEM_PRESSURE)
                .parse()
                .unwrap_or(20.0),
        }
    }
}

/// One lost row the tick may act on, with the facts the planner needs read
/// off the store beforehand.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub session_id: i64,
    pub host_alias: String,
    pub tmux_name: String,
    pub lost_reason: String,
    pub lost_at: i64,
    /// The UX agent's own session: restarted, not batch-restored, and told.
    pub operator: bool,
    /// Attempts in the last [`ATTEMPT_WINDOW_SECS`].
    pub attempts: u32,
    /// Of those, the failed ones.
    pub failed_attempts: u32,
    /// The [`EVENT_SKIPPED`] details already written this loss episode.
    pub skipped: Vec<String>,
    /// An [`EVENT_DEFERRED`] was already written this loss episode.
    pub deferred_noted: bool,
    /// The host is active and reachable.
    pub host_ready: bool,
    /// The host's health sample was taken after `lost_at`, or the host
    /// never reports one (nothing to wait for).
    pub sampled_since_loss: bool,
    /// Why the host reads as loaded (a `DEFER_*` word), `None` when calm.
    pub load: Option<&'static str>,
    /// The host is under the hard low-memory floor: nothing resumes there,
    /// whatever Jev says.
    pub floor: bool,
    /// What Jev would be asked (its `baseline` set by the planner).
    pub question: RestoreQuestion,
}

/// What the tick does with one candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Resume it through the batch restore.
    Restore,
    /// The operator: `restart_session`, then a note.
    RestartOperator,
    /// Wait; the detail an [`EVENT_DEFERRED`] would carry.
    Defer(String),
    /// Write one [`EVENT_SKIPPED`] with this reason.
    Skip(&'static str),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Planned {
    pub candidate: Candidate,
    pub action: Action,
    /// Jev has a doubt to settle here: loaded host, or a failed recovery.
    pub ask: bool,
}

/// PURE: how long after `lost_at` a row with this reason waits.
pub fn grace_for(reason: &str, cfg: &AutoRestoreConfig) -> i64 {
    if MASS_LOSS_REASONS.contains(&reason) {
        cfg.grace_secs.max(MASS_LOSS_GRACE_SECS)
    } else {
        cfg.grace_secs
    }
}

/// PURE: the rule. A candidate still inside its grace ([`grace_for`]), a
/// mass loss whose host has not been sampled since, one on a host that is
/// not ready (it comes back with the host), or refused for a reason
/// already written this episode produces nothing. A loaded host defers
/// (planned every tick so Jev may weigh in, written once per episode).
/// Restores are capped at [`MAX_RESTORES_PER_TICK`], and at
/// [`MAX_RESTORES_PER_HOST_PER_TICK`] per host, Control first and then in
/// `lost_at` order (oldest first); refusals and deferrals are not capped.
pub fn plan(cands: &[Candidate], cfg: &AutoRestoreConfig, now: i64) -> Vec<Planned> {
    if !cfg.enabled {
        return Vec::new();
    }
    let mut ordered: Vec<&Candidate> = cands.iter().collect();
    ordered.sort_by_key(|c| (!c.operator, c.lost_at, c.session_id));
    let mut out = Vec::new();
    let mut restores = 0usize;
    let mut per_host: HashMap<&str, usize> = HashMap::new();
    for c in ordered {
        if !RESUMABLE_LOST_REASONS.contains(&c.lost_reason.as_str())
            || now - c.lost_at < grace_for(&c.lost_reason, cfg)
            || (MASS_LOSS_REASONS.contains(&c.lost_reason.as_str()) && !c.sampled_since_loss)
            || !c.host_ready
        {
            continue;
        }
        let host_restores = per_host.entry(c.host_alias.as_str()).or_default();
        let (action, ask) = if c.attempts >= cfg.max_attempts {
            if c.skipped.iter().any(|d| d == SKIP_BUDGET) {
                continue;
            }
            (Action::Skip(SKIP_BUDGET), false)
        } else if let Some(why) = c.load {
            (Action::Defer(why.to_string()), true)
        } else if restores >= MAX_RESTORES_PER_TICK
            || *host_restores >= MAX_RESTORES_PER_HOST_PER_TICK
        {
            continue;
        } else {
            restores += 1;
            *host_restores += 1;
            let a = if c.operator {
                Action::RestartOperator
            } else {
                Action::Restore
            };
            (a, c.failed_attempts > 0)
        };
        let mut candidate = c.clone();
        candidate.question.baseline = if c.load.is_some() {
            restore_timing::DEFER
        } else {
            restore_timing::RECOVER_NOW
        };
        out.push(Planned {
            candidate,
            action,
            ask,
        });
    }
    out
}

/// PURE: what Jev's advice turns a planned action into. Only a followed
/// (`assist`) answer changes anything, always through
/// [`restore_timing::bounded`]; `room` says whether this tick's restore cap
/// still has space for a deferral Jev wants resumed now.
pub fn apply_advice(p: &Planned, advised: Option<&Advised>, room: bool) -> Action {
    let Some(a) = advised.filter(|a| a.follow) else {
        return p.action.clone();
    };
    let c = &p.candidate;
    let resume = if c.operator {
        Action::RestartOperator
    } else {
        Action::Restore
    };
    match restore_timing::bounded(a.advice.clone(), c.floor, &c.question) {
        None => p.action.clone(),
        Some(Advice::RecoverNow) => match &p.action {
            Action::Defer(_) if !room => p.action.clone(),
            Action::Defer(_) => resume,
            other => other.clone(),
        },
        Some(Advice::Defer) => Action::Defer(format!(
            "{DEFER_JEV}: {}",
            restore_timing::reason_line(&Advice::Defer, a.confidence)
        )),
        // The operator moves with its own machinery (`ensure_operator`'s
        // fallback host); a lost session has no live source to move.
        Some(Advice::Move(_)) if c.operator => p.action.clone(),
        Some(Advice::Move(h)) => Action::Defer(format!(
            "move:{h}: {}",
            restore_timing::reason_line(&Advice::Move(h.clone()), a.confidence)
        )),
    }
}

/// PURE: the words a note or a card uses for a `lost_reason`.
pub fn reason_words(reason: &str) -> &'static str {
    match reason {
        "process_died" => "its process was killed, likely out of memory",
        "tmux_server_gone" => "the tmux server stopped",
        "host_reboot" => "the host rebooted",
        _ => "it stopped unexpectedly",
    }
}

/// PURE: the note queued for the operator after it was restarted. Opens
/// with `[fleet]` (`prompt_origin::FLEET_PROMPT_MARKS`), so it is typed as
/// fleet's and never read as the person talking.
pub fn operator_note(host: &str, reason: &str, lost_at: i64) -> String {
    format!(
        "[fleet] Your session was restarted automatically after its process stopped on \
         {host} ({}) at {}. Your last turn may have been cut off: read the end of your \
         conversation, tell the person in one line what happened, then carry on.",
        reason_words(reason),
        crate::service::agents::review::rfc3339(lost_at)
            .replace('T', " ")
            .replace('Z', " UTC"),
    )
}

/// Side effects of a run, injected for tests.
#[async_trait]
pub trait AutoRestoreExec: Send + Sync {
    /// The batch restore of `ids` on `host` (`restore_host_sessions`).
    async fn restore(
        &self,
        host: &str,
        ids: Vec<i64>,
    ) -> Result<crate::service::sessions::RestoreReport, IpcError>;
    /// Restart the operator's session; answers its row id.
    async fn restart_operator(&self, host: &str, tmux_name: &str) -> Result<i64, IpcError>;
    /// Queue `body` for the session's next idle moment.
    async fn queue_note(&self, session_id: i64, body: String) -> Result<(), IpcError>;
    /// Start Control again where it can run (`ensure_operator`, which moves
    /// it off a host that does not answer); answers the new row.
    async fn move_operator(&self) -> Result<SessionRow, IpcError>;
}

/// Production executor over the real restore, restart and deferred queue.
pub struct RealAutoRestoreExec {
    pub store: Arc<Mutex<Store>>,
    pub ssh: Arc<SshClient>,
}

#[async_trait]
impl AutoRestoreExec for RealAutoRestoreExec {
    async fn restore(
        &self,
        host: &str,
        ids: Vec<i64>,
    ) -> Result<crate::service::sessions::RestoreReport, IpcError> {
        crate::service::sessions::restore_host_sessions(
            crate::service::sessions::RestoreHostSessionsArgs {
                host_alias: host.to_string(),
                dry_run: false,
                session_ids: Some(ids),
            },
            &self.store,
            &self.ssh,
        )
        .await
    }

    async fn restart_operator(&self, host: &str, tmux_name: &str) -> Result<i64, IpcError> {
        crate::service::sessions::restart_session(
            crate::service::sessions::RestartSessionArgs {
                host_alias: host.to_string(),
                name: tmux_name.to_string(),
                force: false,
                profile: None,
                model: None,
                effort: None,
            },
            &self.store,
            &self.ssh,
        )
        .await
        .map(|row| row.id)
    }

    async fn queue_note(&self, session_id: i64, body: String) -> Result<(), IpcError> {
        crate::service::sessions::queue_prompt(
            crate::service::sessions::QueuePromptArgs {
                session_id,
                prompt: body,
                ..Default::default()
            },
            &self.store,
            &self.ssh,
        )
        .await
        .map(|_| ())
    }

    async fn move_operator(&self) -> Result<SessionRow, IpcError> {
        crate::service::operator::ensure_operator(
            &self.store,
            &self.ssh,
            &crate::cancel::CancellationRegistry::new(),
        )
        .await
    }
}

/// What a run did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunReport {
    pub restored: usize,
    pub failed: usize,
    pub skipped: usize,
    /// Planned deferrals, written or not (one event per episode).
    pub deferred: usize,
    /// Control was started on another host this run.
    pub operator_moved: bool,
}

/// Control stranded on a host that stopped answering, as the tick sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct Stranded {
    /// The stranded operator row (it stays where it is).
    pub session_id: i64,
    /// The host it is stranded on.
    pub host: String,
    /// Where `ensure_operator` would start it instead.
    pub fallback: String,
    /// Seconds since that host last answered; `None` when it never did, or
    /// it was hidden or removed (nothing to wait for).
    pub silent_for: Option<i64>,
    /// Moves tried in the last [`ATTEMPT_WINDOW_SECS`], failed ones included.
    pub attempts: u32,
}

/// PURE: whether the tick moves Control now. It moves only a live operator
/// whose host has been silent past the grace, within the restore budget, and
/// only when a host can take it — the same move the desktop's panel makes on
/// open, so a fleet whose person is away from the desktop gets its Control
/// back too (2026-10-15: Control sat on a dead host while the phone had no
/// way to move it).
pub fn move_due(st: &Stranded, cfg: &AutoRestoreConfig) -> bool {
    let wait = cfg.grace_secs.max(OPERATOR_MOVE_AFTER_SECS);
    st.attempts < cfg.max_attempts && st.silent_for.is_none_or(|s| s >= wait)
}

/// PURE: the note queued for Control after it was started on another host.
pub fn operator_moved_note(from: &str, to: &str) -> String {
    format!(
        "[fleet] Your host {from} stopped answering, so you were started again on {to}. \
         Your earlier conversation stays on {from} and comes back with it. Check what \
         you were following (fleet_status, your missions) and carry on; tell the person \
         what moved."
    )
}

/// Read the stranded operator: `None` unless Control lives on a host that
/// does not answer and another host can take it.
fn stranded(store: &Mutex<Store>, now: i64) -> Option<Stranded> {
    let st = crate::service::operator::operator_status(store).ok()?;
    if st.blocked.as_deref() != Some("host_down") {
        return None;
    }
    let (row, fallback) = (st.session?, st.fallback?);
    let s = store.lock().ok()?;
    let host = s.get_host_row(&row.host_alias).ok().flatten();
    let silent_for = host
        .filter(|h| !h.hidden)
        .and_then(|h| h.last_reachable_at)
        .map(|at| now - at);
    let attempts = s
        .count_session_events_since(
            row.id,
            &[EVENT_OPERATOR_MOVED, EVENT_OPERATOR_MOVE_FAILED],
            now - ATTEMPT_WINDOW_SECS,
        )
        .unwrap_or(u32::MAX);
    Some(Stranded {
        session_id: row.id,
        host: row.host_alias,
        fallback,
        silent_for,
        attempts,
    })
}

/// Move Control off a host that stopped answering, when [`move_due`].
async fn move_stranded_operator(
    store: &Mutex<Store>,
    exec: &dyn AutoRestoreExec,
    cfg: &AutoRestoreConfig,
    now: i64,
    report: &mut RunReport,
) {
    let Some(st) = stranded(store, now) else {
        return;
    };
    if !move_due(&st, cfg) {
        return;
    }
    match exec.move_operator().await {
        Ok(row) => {
            tracing::warn!(from = %st.host, to = %row.host_alias, "[auto-restore] operator moved off a host that does not answer");
            record(
                store,
                st.session_id,
                EVENT_OPERATOR_MOVED,
                &format!("{}->{}", st.host, row.host_alias),
            );
            report.operator_moved = true;
            if let Err(e) = exec
                .queue_note(row.id, operator_moved_note(&st.host, &row.host_alias))
                .await
            {
                tracing::warn!(session_id = row.id, error = %e.message, "[auto-restore] operator note not queued");
            }
        }
        Err(e) => {
            tracing::warn!(from = %st.host, to = %st.fallback, error = %e.message, "[auto-restore] operator move failed");
            record(
                store,
                st.session_id,
                EVENT_OPERATOR_MOVE_FAILED,
                &format!("{}: {}", st.host, e.message),
            );
            report.failed += 1;
        }
    }
}

/// Whether a lost row is one the tick looks at: a resumable tmux agent row,
/// not the controller, with no safe-kill in flight. PURE.
fn eligible(row: &SessionRow, controller: Option<&(String, String)>) -> bool {
    row.lost_at.is_some()
        && row
            .lost_reason
            .as_deref()
            .is_some_and(|r| RESUMABLE_LOST_REASONS.contains(&r))
        && row.claude_session_id.is_some()
        && !crate::store::has_no_pane(&row.kind)
        && row.kind != "shell"
        && row.agent != crate::store::AGENT_SHELL
        && row.safe_kill_state.is_none()
        && !controller.is_some_and(|(h, n)| *h == row.host_alias && *n == row.tmux_name)
}

/// Asks the decision layer about one planned restore. Injected for tests.
#[async_trait]
pub trait RestoreAdvisor: Send + Sync {
    async fn advise(&self, q: &RestoreQuestion) -> Option<Advised>;
}

/// The production advisor: `decide::restore_timing` over the envelope.
pub struct JevAdvisor(pub crate::service::decide::DecideCtx);

#[async_trait]
impl RestoreAdvisor for JevAdvisor {
    async fn advise(&self, q: &RestoreQuestion) -> Option<Advised> {
        restore_timing::ask(&self.0, q).await
    }
}

/// An advisor that never answers: the rule alone.
pub struct NoAdvice;

#[async_trait]
impl RestoreAdvisor for NoAdvice {
    async fn advise(&self, _q: &RestoreQuestion) -> Option<Advised> {
        None
    }
}

/// Why `h` reads as loaded, as a `DEFER_*` word: memory first (it is the
/// floor, [`memory_floor`]), then CPU. `None` when calm or unknown.
/// `starting` is how many restores the tick launched on `h` in the last
/// [`RESTORE_SETTLE_SECS`].
///
/// The low-memory line and CPU are `fleet_health`'s own judgments
/// (`health::mem_low`, `health::cpu_saturated`: `health.cpu_busy_pct` on
/// two samples in a row), so a host the Hosts view shows as loaded is the
/// host this defers on.
pub fn host_load(
    h: &HostRow,
    cfg: &AutoRestoreConfig,
    starting: u32,
    now: i64,
) -> Option<&'static str> {
    memory_floor(h, cfg, starting, now)
        .or_else(|| crate::service::health::cpu_saturated(h, now).then_some(DEFER_CPU))
}

/// Why nothing may resume on `h` whatever Jev says, as a `DEFER_*` word:
/// available memory at or under the low line, memory pressure at or over
/// `recovery.max_mem_pressure`, or less available memory than
/// [`RESTORE_MEM_KB`] for each of the `starting` restores plus this one.
/// Judged on a fresh sample only; `None` when calm or unknown.
pub fn memory_floor(
    h: &HostRow,
    cfg: &AutoRestoreConfig,
    starting: u32,
    now: i64,
) -> Option<&'static str> {
    use crate::service::health;
    if health::mem_low(h, cfg.mem_low_pct, now) {
        return Some(DEFER_MEMORY_LOW);
    }
    if !health::sample_fresh(h, now) {
        return None;
    }
    if h.mem_pressure.is_some_and(|p| p >= cfg.max_mem_pressure) {
        return Some(DEFER_MEM_PRESSURE);
    }
    if h.mem_avail_kb
        .is_some_and(|kb| kb < (i64::from(starting) + 1) * RESTORE_MEM_KB)
    {
        return Some(DEFER_MEM_HEADROOM);
    }
    None
}

/// The numbers Jev reads about one host, bucketed.
fn host_facts(h: &HostRow, live: i64, now: i64) -> HostFacts {
    HostFacts {
        alias: h.alias.clone(),
        cpu_busy_pct: crate::service::health::cpu_busy_pct(h, now).map(restore_timing::tenth),
        mem_avail_pct: crate::service::health::mem_avail_pct(h, now).map(restore_timing::tenth),
        disk_free_pct: match (h.disk_home_free_kb, h.disk_home_total_kb) {
            (Some(free), Some(total)) if total > 0 => {
                Some(restore_timing::tenth(free as f64 * 100.0 / total as f64))
            }
            _ => None,
        },
        live_sessions: live,
    }
}

/// Read the candidates under one lock. `None` while paused.
fn candidates(s: &Store, cfg: &AutoRestoreConfig, now: i64) -> Option<Vec<Candidate>> {
    // Pause all (redesign 8.1): the tick's gate records the pause; this
    // stops any other caller too.
    if crate::service::loops::paused(s) {
        return None;
    }
    let rows = s.list_all_sessions().unwrap_or_default();
    let controller = s.get_controller().ok().flatten();
    let hosts: Vec<HostRow> = crate::service::hosts::active_hosts(
        s.list_hosts().unwrap_or_default(),
        crate::service::hub::local_host_enabled(),
    );
    let mut live: HashMap<&str, i64> = HashMap::new();
    for r in rows.iter().filter(|r| r.status == "running") {
        *live.entry(r.host_alias.as_str()).or_default() += 1;
    }
    let live_on = |alias: &str| live.get(alias).copied().unwrap_or(0);
    // Restores launched lately, per host: their memory may not be in the
    // sample yet (`memory_floor`).
    let starting: HashMap<&str, u32> = hosts
        .iter()
        .map(|h| {
            let n = s
                .count_host_session_events_since(
                    &h.alias,
                    EVENT_RESTORED,
                    now - RESTORE_SETTLE_SECS,
                )
                .unwrap_or(0);
            (h.alias.as_str(), n)
        })
        .collect();
    let starting_on = |alias: &str| starting.get(alias).copied().unwrap_or(0);
    let operator = crate::service::operator::operator_ref(s);
    Some(
        rows.iter()
            .filter(|r| eligible(r, controller.as_ref()))
            .map(|r| {
                let lost_at = r.lost_at.unwrap_or(now);
                let host = hosts.iter().find(|h| h.alias == r.host_alias);
                let since = now - ATTEMPT_WINDOW_SECS;
                let failed_attempts = s
                    .count_session_events_since(r.id, &[EVENT_FAILED], since)
                    .unwrap_or(0);
                let reason = r.lost_reason.as_deref().unwrap_or_default();
                let refund = MASS_LOSS_REASONS.contains(&reason)
                    && s.last_session_event_at(r.id, EVENT_RESTORED, lost_at)
                        .ok()
                        .flatten()
                        .is_some_and(|at| at >= since && lost_at - at <= MASS_LOSS_REFUND_SECS);
                let attempts = s
                    .count_session_events_since(r.id, &[EVENT_RESTORED, EVENT_FAILED], since)
                    .unwrap_or(u32::MAX)
                    .saturating_sub(u32::from(refund));
                let load = host.and_then(|h| host_load(h, cfg, starting_on(&h.alias), now));
                // A move may name only a healthy, reachable, active host in
                // the same org as the one the session ran on.
                let move_to = host
                    .map(|own| {
                        hosts
                            .iter()
                            .filter(|h| {
                                h.alias != own.alias
                                    && h.reachable
                                    && h.org_id == own.org_id
                                    && host_load(h, cfg, starting_on(&h.alias), now).is_none()
                                    && crate::service::health::mem_avail_pct(h, now).is_some()
                            })
                            .map(|h| host_facts(h, live_on(&h.alias), now))
                            .collect()
                    })
                    .unwrap_or_default();
                let question = RestoreQuestion {
                    session_id: r.id,
                    org_id: r.org_id,
                    lost_at,
                    lost_reason: r.lost_reason.clone().unwrap_or_default(),
                    idle_minutes: restore_timing::minutes_bucket(now - r.last_activity_at),
                    turn_outcome: r.turn_outcome.clone(),
                    // A prompt clears `idle_since`; only a Stop sets it again,
                    // and loss does not touch it: unset means a turn was
                    // running when the process died.
                    mid_turn: r.idle_since.is_none(),
                    failed_attempts,
                    host: host
                        .map(|h| host_facts(h, live_on(&h.alias), now))
                        .unwrap_or_default(),
                    move_to,
                    baseline: restore_timing::RECOVER_NOW,
                };
                Candidate {
                    session_id: r.id,
                    host_alias: r.host_alias.clone(),
                    tmux_name: r.tmux_name.clone(),
                    lost_reason: r.lost_reason.clone().unwrap_or_default(),
                    lost_at,
                    operator: operator.as_ref().is_some_and(|o| {
                        o.host_alias == r.host_alias && o.tmux_name == r.tmux_name
                    }),
                    attempts,
                    failed_attempts,
                    skipped: s
                        .session_event_details_since(r.id, EVENT_SKIPPED, lost_at)
                        .unwrap_or_default(),
                    deferred_noted: s
                        .count_session_events_since(r.id, &[EVENT_DEFERRED], lost_at)
                        .unwrap_or(0)
                        > 0,
                    host_ready: host.is_some_and(|h| h.reachable),
                    sampled_since_loss: host
                        .is_some_and(|h| h.health_at.is_none_or(|at| at > lost_at)),
                    load,
                    floor: host.is_some_and(|h| {
                        memory_floor(h, cfg, starting_on(&h.alias), now).is_some()
                    }),
                    question,
                }
            })
            .collect(),
    )
}

/// Write one attempt's event. Best-effort, like every timeline write.
fn record(store: &Mutex<Store>, session_id: i64, kind: &str, detail: &str) {
    match store.lock() {
        Ok(s) => {
            if let Err(e) = s.insert_session_event(session_id, kind, Some(detail)) {
                tracing::warn!(session_id, kind, error = %e, "[auto-restore] event not recorded");
            }
        }
        Err(e) => tracing::warn!(session_id, error = %e, "[auto-restore] store lock failed"),
    }
}

/// An error that says "not now" rather than "this attempt failed": another
/// restore of the host holds its claim, or the host dropped off between the
/// plan and the call. Neither spends the session's budget; the next tick
/// tries again.
fn not_now(e: &IpcError) -> bool {
    e.code == codes::E_INVALID_STATE || e.code == codes::E_HOST_OFFLINE
}

/// Plan, consult the advisor where the rule has a doubt, and execute one
/// run. Returns what it did.
pub async fn run_with(
    store: &Mutex<Store>,
    exec: &dyn AutoRestoreExec,
    advisor: &dyn RestoreAdvisor,
    cfg: &AutoRestoreConfig,
    now: i64,
) -> RunReport {
    let mut report = RunReport::default();
    if !cfg.enabled {
        return report;
    }
    let cands = {
        let Ok(s) = store.lock() else {
            return report;
        };
        match candidates(&s, cfg, now) {
            Some(c) => c,
            None => return report,
        }
    };
    let planned = plan(&cands, cfg, now);
    let resumes = |a: &Action| matches!(a, Action::Restore | Action::RestartOperator);
    let mut restores = planned.iter().filter(|p| resumes(&p.action)).count();
    let mut per_host: HashMap<String, usize> = HashMap::new();
    for p in planned.iter().filter(|p| resumes(&p.action)) {
        *per_host.entry(p.candidate.host_alias.clone()).or_default() += 1;
    }
    let mut by_host: BTreeMap<String, Vec<Candidate>> = BTreeMap::new();
    for p in planned {
        let advised = if p.ask {
            advisor.advise(&p.candidate.question).await
        } else {
            None
        };
        let host_room = per_host
            .get(&p.candidate.host_alias)
            .is_none_or(|n| *n < MAX_RESTORES_PER_HOST_PER_TICK);
        let action = apply_advice(
            &p,
            advised.as_ref(),
            restores < MAX_RESTORES_PER_TICK && host_room,
        );
        if matches!(p.action, Action::Defer(_)) && resumes(&action) {
            restores += 1;
            *per_host.entry(p.candidate.host_alias.clone()).or_default() += 1;
        }
        let c = p.candidate;
        match action {
            Action::Skip(why) => {
                tracing::info!(
                    host = %c.host_alias,
                    session = %c.tmux_name,
                    reason = %c.lost_reason,
                    why,
                    "[auto-restore] skipped"
                );
                record(store, c.session_id, EVENT_SKIPPED, why);
                report.skipped += 1;
            }
            Action::Defer(detail) => {
                report.deferred += 1;
                if !c.deferred_noted {
                    tracing::info!(
                        host = %c.host_alias,
                        session = %c.tmux_name,
                        reason = %c.lost_reason,
                        why = %detail,
                        "[auto-restore] deferred"
                    );
                    record(store, c.session_id, EVENT_DEFERRED, &detail);
                }
            }
            Action::RestartOperator => {
                match exec.restart_operator(&c.host_alias, &c.tmux_name).await {
                    Ok(id) => {
                        tracing::info!(host = %c.host_alias, session = %c.tmux_name, "[auto-restore] operator restarted");
                        record(store, c.session_id, EVENT_RESTORED, &c.lost_reason);
                        report.restored += 1;
                        let note = operator_note(&c.host_alias, &c.lost_reason, c.lost_at);
                        if let Err(e) = exec.queue_note(id, note).await {
                            tracing::warn!(session_id = id, error = %e.message, "[auto-restore] operator note not queued");
                        }
                    }
                    Err(e) if not_now(&e) => {}
                    Err(e) => {
                        tracing::warn!(host = %c.host_alias, session = %c.tmux_name, error = %e.message, "[auto-restore] operator restart failed");
                        record(
                            store,
                            c.session_id,
                            EVENT_FAILED,
                            &format!("{}: {}", c.lost_reason, e.message),
                        );
                        report.failed += 1;
                    }
                }
            }
            Action::Restore => by_host.entry(c.host_alias.clone()).or_default().push(c),
        }
    }
    for (host, cands) in by_host {
        let ids: Vec<i64> = cands.iter().map(|c| c.session_id).collect();
        let reason_of = |id: i64| {
            cands
                .iter()
                .find(|c| c.session_id == id)
                .map(|c| c.lost_reason.clone())
                .unwrap_or_default()
        };
        match exec.restore(&host, ids.clone()).await {
            Ok(r) => {
                for o in r.results {
                    let reason = reason_of(o.session_id);
                    if o.ok {
                        record(store, o.session_id, EVENT_RESTORED, &reason);
                        report.restored += 1;
                    } else if o.error.as_deref()
                        == Some(crate::service::sessions::RESTORED_ELSEWHERE)
                    {
                        // A person (or another restore) got there first:
                        // not an attempt of ours.
                    } else {
                        let err = o.error.unwrap_or_else(|| "unknown error".into());
                        record(
                            store,
                            o.session_id,
                            EVENT_FAILED,
                            &format!("{reason}: {err}"),
                        );
                        report.failed += 1;
                    }
                }
            }
            Err(e) if not_now(&e) => {
                tracing::debug!(host = %host, error = %e.message, "[auto-restore] host busy or offline; next tick");
            }
            Err(e) => {
                tracing::warn!(host = %host, error = %e.message, "[auto-restore] restore failed");
                for id in ids {
                    let reason = reason_of(id);
                    record(store, id, EVENT_FAILED, &format!("{reason}: {}", e.message));
                    report.failed += 1;
                }
            }
        }
    }
    move_stranded_operator(store, exec, cfg, now, &mut report).await;
    report
}

static RUNNING: AtomicBool = AtomicBool::new(false);

/// Tick entry point: gate, then run detached and single-flight. A run that
/// is still restoring when the next tick fires makes that tick a no-op.
pub fn maybe_run(store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>) {
    let cfg = {
        let Ok(s) = store.lock() else {
            return;
        };
        let cfg = AutoRestoreConfig::from_store(&s);
        if !crate::service::loops::gate_in("auto_restore", &s, None) {
            return;
        }
        cfg
    };
    if !cfg.enabled {
        crate::service::loops::report("auto_restore", Ok::<_, String>(()), None);
        return;
    }
    if RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    let store = Arc::clone(store);
    let ssh = Arc::clone(ssh);
    crate::rt::spawn(async move {
        let _flight = crate::rt::ClearOnDrop::of_static(&RUNNING);
        let exec = RealAutoRestoreExec {
            store: Arc::clone(&store),
            ssh,
        };
        let advisor = JevAdvisor(crate::service::decide::DecideCtx::jev(Arc::clone(&store)));
        let r = run_with(&store, &exec, &advisor, &cfg, crate::store::now_unix()).await;
        crate::service::loops::report("auto_restore", Ok::<_, String>(()), None);
        if r.restored + r.failed + r.skipped > 0 || r.operator_moved {
            tracing::info!(
                "[auto-restore] restored={} failed={} skipped={} deferred={} operator_moved={}",
                r.restored,
                r.failed,
                r.skipped,
                r.deferred,
                r.operator_moved
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::sessions::{RestoreOutcome, RestoreReport};

    const CFG: AutoRestoreConfig = AutoRestoreConfig {
        enabled: true,
        grace_secs: 60,
        max_attempts: 2,
        mem_low_pct: 10.0,
        max_mem_pressure: 20.0,
    };

    fn question(id: i64) -> RestoreQuestion {
        RestoreQuestion {
            session_id: id,
            org_id: None,
            lost_at: 0,
            lost_reason: "process_died".into(),
            idle_minutes: 5,
            turn_outcome: None,
            mid_turn: false,
            failed_attempts: 0,
            host: HostFacts {
                alias: "h".into(),
                ..Default::default()
            },
            move_to: vec![HostFacts {
                alias: "calm".into(),
                ..Default::default()
            }],
            baseline: restore_timing::RECOVER_NOW,
        }
    }

    fn cand(id: i64, reason: &str, lost_at: i64) -> Candidate {
        Candidate {
            session_id: id,
            host_alias: "h".into(),
            tmux_name: format!("s{id}"),
            lost_reason: reason.into(),
            lost_at,
            operator: false,
            attempts: 0,
            failed_attempts: 0,
            skipped: vec![],
            deferred_noted: false,
            host_ready: true,
            sampled_since_loss: true,
            load: None,
            floor: false,
            question: question(id),
        }
    }

    fn actions(p: &[Planned]) -> Vec<(i64, Action)> {
        p.iter()
            .map(|p| (p.candidate.session_id, p.action.clone()))
            .collect()
    }

    /// `cand` on its own host, so the per-host cap stays out of the way.
    fn cand_on(id: i64, reason: &str, lost_at: i64) -> Candidate {
        Candidate {
            host_alias: format!("h{id}"),
            ..cand(id, reason, lost_at)
        }
    }

    #[test]
    fn restores_resumable_rows_past_the_grace_only() {
        let c = vec![
            cand_on(1, "process_died", 100),
            cand_on(2, "tmux_server_gone", 100),
            cand_on(3, "host_reboot", 100),
            cand_on(4, "missing", 100),
            cand_on(5, "killed", 100),
            cand_on(6, "process_died", 150),
        ];
        assert_eq!(
            actions(&plan(&c, &CFG, 200)),
            vec![(1, Action::Restore)],
            "6 is still inside its 60 s grace, a mass loss inside its {MASS_LOSS_GRACE_SECS} s; \
             missing and killed are never restored"
        );
        assert_eq!(
            actions(&plan(&c, &CFG, 100 + MASS_LOSS_GRACE_SECS)),
            vec![
                (1, Action::Restore),
                (2, Action::Restore),
                (3, Action::Restore),
                (6, Action::Restore)
            ],
        );
        let off = AutoRestoreConfig {
            enabled: false,
            ..CFG
        };
        assert!(plan(&c, &off, 200).is_empty());
    }

    #[test]
    fn a_host_that_is_not_ready_waits_without_an_event() {
        let mut c = cand(1, "process_died", 0);
        c.host_ready = false;
        assert!(plan(&[c], &CFG, 1_000).is_empty());
    }

    #[test]
    fn a_loaded_host_defers_and_asks_and_the_budget_refuses_once() {
        let mut spent = cand(1, "process_died", 0);
        spent.attempts = 2;
        let mut loaded = cand(2, "process_died", 0);
        loaded.load = Some(DEFER_MEMORY_LOW);
        loaded.floor = true;
        let p = plan(&[spent.clone(), loaded.clone()], &CFG, 1_000);
        assert_eq!(
            actions(&p),
            vec![
                (1, Action::Skip(SKIP_BUDGET)),
                (2, Action::Defer(DEFER_MEMORY_LOW.into()))
            ]
        );
        assert!(!p[0].ask);
        assert!(p[1].ask, "a loaded host is Jev's to weigh");
        assert_eq!(p[1].candidate.question.baseline, restore_timing::DEFER);
        spent.skipped = vec![SKIP_BUDGET.into()];
        assert!(
            plan(&[spent], &CFG, 1_000).is_empty(),
            "already written this episode"
        );
        // The load dropped: the same episode restores after all.
        loaded.load = None;
        loaded.floor = false;
        assert_eq!(
            actions(&plan(&[loaded], &CFG, 1_000)),
            vec![(2, Action::Restore)]
        );
    }

    #[test]
    fn a_failed_recovery_asks_jev_before_the_next_attempt() {
        let mut c = cand(1, "process_died", 0);
        c.attempts = 1;
        c.failed_attempts = 1;
        let p = plan(&[c], &CFG, 1_000);
        assert_eq!(p[0].action, Action::Restore);
        assert!(p[0].ask);
    }

    #[test]
    fn restores_are_capped_per_tick_oldest_first_and_the_operator_restarts() {
        // Six hosts, one loss each: the global cap, oldest first, except
        // that Control goes first even though it was lost last.
        let mut c: Vec<Candidate> = (1..=6).map(|i| cand_on(i, "host_reboot", 10 - i)).collect();
        c[0].operator = true; // id 1, lost last
        let p = plan(&c, &CFG, 1_000);
        assert_eq!(p.len(), MAX_RESTORES_PER_TICK);
        assert_eq!(p[0].candidate.session_id, 1);
        assert_eq!(p[0].action, Action::RestartOperator);
        assert_eq!(
            p[1..]
                .iter()
                .map(|p| p.candidate.session_id)
                .collect::<Vec<_>>(),
            vec![6, 5, 4]
        );
        assert!(p[1..].iter().all(|p| p.action == Action::Restore));

        // One host that lost six (2026-10-16): one restore per tick, and
        // Control is that one.
        let mut c: Vec<Candidate> = (1..=6)
            .map(|i| cand(i, "tmux_server_gone", 10 - i))
            .collect();
        c[0].operator = true;
        assert_eq!(
            actions(&plan(&c, &CFG, 1_000)),
            vec![(1, Action::RestartOperator)]
        );
        c[0].operator = false;
        assert_eq!(actions(&plan(&c, &CFG, 1_000)), vec![(6, Action::Restore)]);
        // A deferral or a refusal on the same host is not capped.
        c[1].attempts = 2;
        assert_eq!(
            actions(&plan(&c, &CFG, 1_000)),
            vec![(6, Action::Restore), (2, Action::Skip(SKIP_BUDGET))]
        );
    }

    #[test]
    fn a_mass_loss_waits_for_a_sample_taken_after_it() {
        let mut gone = cand(1, "tmux_server_gone", 0);
        gone.sampled_since_loss = false;
        let mut died = cand_on(2, "process_died", 0);
        died.sampled_since_loss = false;
        assert_eq!(
            actions(&plan(&[gone.clone(), died], &CFG, 1_000)),
            vec![(2, Action::Restore)],
            "only a whole-host loss waits for the evidence"
        );
        gone.sampled_since_loss = true;
        assert_eq!(
            actions(&plan(&[gone], &CFG, 1_000)),
            vec![(1, Action::Restore)]
        );
    }

    fn host_row(avail_kb: i64, pressure: Option<f64>, now: i64) -> HostRow {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        let mut h = s.get_host_row("h").unwrap().unwrap();
        h.reachable = true;
        h.mem_avail_kb = Some(avail_kb);
        h.mem_total_kb = Some(16_000_000);
        h.mem_pressure = pressure;
        h.health_at = Some(now);
        h
    }

    #[test]
    fn memory_pressure_and_headroom_are_the_floor() {
        let now = 10_000;
        let calm = host_row(8_000_000, Some(1.0), now);
        assert_eq!(memory_floor(&calm, &CFG, 0, now), None);
        assert_eq!(host_load(&calm, &CFG, 0, now), None);
        // systemd-oomd's signal, well before it acts.
        let pressed = host_row(8_000_000, Some(20.0), now);
        assert_eq!(
            memory_floor(&pressed, &CFG, 0, now),
            Some(DEFER_MEM_PRESSURE)
        );
        assert_eq!(
            memory_floor(&host_row(8_000_000, Some(19.9), now), &CFG, 0, now),
            None
        );
        // Room for one more start, counting those still starting.
        let tight = host_row(3 * RESTORE_MEM_KB - 1, Some(0.0), now);
        assert_eq!(memory_floor(&tight, &CFG, 1, now), None);
        assert_eq!(memory_floor(&tight, &CFG, 2, now), Some(DEFER_MEM_HEADROOM));
        // A stale sample says nothing either way.
        let mut stale = pressed.clone();
        stale.health_at = Some(now - crate::service::health::HEALTH_SAMPLE_FRESH_SECS - 1);
        assert_eq!(host_load(&stale, &CFG, 9, now), None);
    }

    fn advised(advice: Advice, follow: bool) -> Advised {
        Advised {
            advice,
            follow,
            confidence: Some(0.8),
        }
    }

    #[test]
    fn jev_steers_within_bounds_and_only_in_assist() {
        let mut loaded = cand(1, "process_died", 0);
        loaded.load = Some(DEFER_CPU);
        let p = &plan(&[loaded.clone()], &CFG, 1_000)[0];
        // Shadow never changes the rule's action.
        assert_eq!(
            apply_advice(p, Some(&advised(Advice::RecoverNow, false)), true),
            p.action
        );
        // Assist: CPU-loaded but above the memory floor may resume now…
        assert_eq!(
            apply_advice(p, Some(&advised(Advice::RecoverNow, true)), true),
            Action::Restore
        );
        // …unless this tick's cap is full.
        assert_eq!(
            apply_advice(p, Some(&advised(Advice::RecoverNow, true)), false),
            p.action
        );
        // A move to an offered host defers and names it.
        match apply_advice(p, Some(&advised(Advice::Move("calm".into()), true)), true) {
            Action::Defer(d) => assert!(d.starts_with("move:calm: Jev: better on calm"), "{d}"),
            a => panic!("{a:?}"),
        }
        // A move to a host never offered is dropped: the rule decides.
        assert_eq!(
            apply_advice(p, Some(&advised(Advice::Move("busy".into()), true)), true),
            p.action
        );
        // Under the memory floor, recover_now can never resume.
        loaded.load = Some(DEFER_MEMORY_LOW);
        loaded.floor = true;
        let p = &plan(&[loaded], &CFG, 1_000)[0];
        match apply_advice(p, Some(&advised(Advice::RecoverNow, true)), true) {
            Action::Defer(d) => assert!(d.starts_with(DEFER_JEV), "{d}"),
            a => panic!("{a:?}"),
        }
    }

    #[test]
    fn the_operator_note_is_fleets_and_says_what_happened() {
        let n = operator_note("mercury", "process_died", 1_760_000_000);
        assert!(crate::service::prompt_origin::is_fleet_prompt(&n));
        assert!(
            n.contains("on mercury (its process was killed, likely out of memory)"),
            "{n}"
        );
        assert!(n.contains("2025-10-09 08:53:20 UTC"), "{n}");
        assert!(n.contains("tell the person in one line"));
    }

    // ── run_with against a real store and a fake executor ──

    #[derive(Default)]
    struct Fake {
        restored: Mutex<Vec<(String, Vec<i64>)>>,
        restarted: Mutex<Vec<String>>,
        notes: Mutex<Vec<(i64, String)>>,
        moved: Mutex<usize>,
        moved_to: Mutex<Option<SessionRow>>,
        fail_with: Option<&'static str>,
    }

    #[async_trait]
    impl AutoRestoreExec for Fake {
        async fn restore(&self, host: &str, ids: Vec<i64>) -> Result<RestoreReport, IpcError> {
            self.restored
                .lock()
                .unwrap()
                .push((host.to_string(), ids.clone()));
            if let Some(code) = self.fail_with {
                return Err(IpcError::new(code, "boom"));
            }
            Ok(RestoreReport {
                host_alias: host.into(),
                dry_run: false,
                plan: vec![],
                results: ids
                    .iter()
                    .map(|&id| RestoreOutcome {
                        session_id: id,
                        tmux_name: String::new(),
                        ok: true,
                        error: None,
                    })
                    .collect(),
            })
        }
        async fn restart_operator(&self, _host: &str, tmux: &str) -> Result<i64, IpcError> {
            self.restarted.lock().unwrap().push(tmux.to_string());
            Ok(42)
        }
        async fn queue_note(&self, id: i64, body: String) -> Result<(), IpcError> {
            self.notes.lock().unwrap().push((id, body));
            Ok(())
        }
        async fn move_operator(&self) -> Result<SessionRow, IpcError> {
            *self.moved.lock().unwrap() += 1;
            if let Some(code) = self.fail_with {
                return Err(IpcError::new(code, "boom"));
            }
            self.moved_to
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "no row to answer"))
        }
    }

    /// An advisor that always says the same, and counts the questions.
    struct Says(Option<Advised>, Mutex<usize>);

    #[async_trait]
    impl RestoreAdvisor for Says {
        async fn advise(&self, _q: &RestoreQuestion) -> Option<Advised> {
            *self.1.lock().unwrap() += 1;
            self.0.clone()
        }
    }

    /// A store with a reachable host `h` holding one session per `(name,
    /// reason)`, each lost at `lost_at` with a conversation to resume.
    fn store_with(rows: &[(&str, &str)], lost_at: i64) -> (Mutex<Store>, Vec<i64>) {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        s.conn_ref()
            .execute("UPDATE hosts SET reachable = 1 WHERE alias = 'h'", [])
            .unwrap();
        let mut ids = Vec::new();
        for (name, reason) in rows {
            let id = s
                .upsert_session(name, "h", None, None, 1, 1, "running", None)
                .unwrap();
            s.set_claude_session_id(id, &format!("uuid-{name}"))
                .unwrap();
            s.conn_ref()
                .execute(
                    "UPDATE sessions SET status = 'ghost', lost_at = ?2, lost_reason = ?3 \
                     WHERE id = ?1",
                    rusqlite::params![id, lost_at, reason],
                )
                .unwrap();
            ids.push(id);
        }
        (Mutex::new(s), ids)
    }

    fn short_of_memory(store: &Mutex<Store>, now: i64) {
        store
            .lock()
            .unwrap()
            .conn_ref()
            .execute(
                "UPDATE hosts SET mem_avail_kb = 500, mem_total_kb = 16000000, \
                 health_at = ?1 WHERE alias = 'h'",
                [now],
            )
            .unwrap();
    }

    /// A fresh health sample on `h`: `avail_kb` available of 16 GB, and
    /// memory pressure `pressure`.
    fn sampled(store: &Mutex<Store>, avail_kb: i64, pressure: f64, now: i64) {
        store
            .lock()
            .unwrap()
            .conn_ref()
            .execute(
                "UPDATE hosts SET mem_avail_kb = ?1, mem_total_kb = 16000000, \
                 mem_pressure = ?2, health_at = ?3 WHERE alias = 'h'",
                rusqlite::params![avail_kb, pressure, now],
            )
            .unwrap();
    }

    /// An event written at `at` rather than now.
    fn event_at(store: &Mutex<Store>, id: i64, kind: &str, detail: &str, at: i64) {
        store
            .lock()
            .unwrap()
            .conn_ref()
            .execute(
                "INSERT INTO session_events (session_id, at, kind, detail) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![id, at, kind, detail],
            )
            .unwrap();
    }

    fn events(store: &Mutex<Store>, id: i64) -> Vec<(String, Option<String>)> {
        store
            .lock()
            .unwrap()
            .list_session_events(id, 50)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind.starts_with("auto_restore"))
            .map(|e| (e.kind, e.detail))
            .collect()
    }

    #[tokio::test]
    async fn a_killed_session_is_restored_and_leaves_one_event() {
        let now = crate::store::now_unix();
        let (store, ids) = store_with(&[("a", "process_died"), ("b", "missing")], now - 120);
        let fake = Fake::default();
        let r = run_with(&store, &fake, &NoAdvice, &CFG, now).await;
        assert_eq!(r.restored, 1);
        assert_eq!(
            *fake.restored.lock().unwrap(),
            vec![("h".to_string(), vec![ids[0]])]
        );
        assert_eq!(
            events(&store, ids[0]),
            vec![(EVENT_RESTORED.to_string(), Some("process_died".to_string()))]
        );
        assert!(
            events(&store, ids[1]).is_empty(),
            "missing is not resumable"
        );
    }

    #[tokio::test]
    async fn failures_spend_the_budget_and_then_it_refuses_once() {
        let now = crate::store::now_unix();
        let (store, ids) = store_with(&[("a", "process_died")], now - 120);
        let fake = Fake {
            fail_with: Some(codes::E_TMUX),
            ..Default::default()
        };
        for _ in 0..4 {
            run_with(&store, &fake, &NoAdvice, &CFG, now).await;
        }
        let ev = events(&store, ids[0]);
        let failed = ev.iter().filter(|(k, _)| k == EVENT_FAILED).count();
        let skipped: Vec<_> = ev.iter().filter(|(k, _)| k == EVENT_SKIPPED).collect();
        assert_eq!(failed, 2, "two attempts, then the budget: {ev:?}");
        assert_eq!(skipped.len(), 1, "the refusal is written once: {ev:?}");
        assert_eq!(skipped[0].1.as_deref(), Some(SKIP_BUDGET));
        assert_eq!(fake.restored.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_busy_or_offline_host_is_not_an_attempt() {
        let now = crate::store::now_unix();
        let (store, ids) = store_with(&[("a", "host_reboot")], now - MASS_LOSS_GRACE_SECS);
        let fake = Fake {
            fail_with: Some(codes::E_INVALID_STATE),
            ..Default::default()
        };
        run_with(&store, &fake, &NoAdvice, &CFG, now).await;
        assert!(events(&store, ids[0]).is_empty());
    }

    #[tokio::test]
    async fn a_loaded_host_defers_once_per_episode_spends_nothing_and_resumes_later() {
        let now = crate::store::now_unix();
        let (store, ids) = store_with(&[("a", "process_died")], now - 120);
        short_of_memory(&store, now);
        let fake = Fake::default();
        for _ in 0..3 {
            let r = run_with(&store, &fake, &NoAdvice, &CFG, now).await;
            assert_eq!(r.deferred, 1);
        }
        assert!(fake.restored.lock().unwrap().is_empty());
        assert_eq!(
            events(&store, ids[0]),
            vec![(
                EVENT_DEFERRED.to_string(),
                Some(DEFER_MEMORY_LOW.to_string())
            )],
            "one event per episode, not one per tick"
        );
        // Memory is back: the next tick restores, nothing was spent.
        store
            .lock()
            .unwrap()
            .conn_ref()
            .execute(
                "UPDATE hosts SET mem_avail_kb = 8000000 WHERE alias = 'h'",
                [],
            )
            .unwrap();
        let r = run_with(&store, &fake, &NoAdvice, &CFG, now).await;
        assert_eq!(r.restored, 1);
    }

    /// 2026-10-16: oomd killed a tmux server at 87.87 % memory pressure
    /// while plenty of memory still read as available. Pressure defers on
    /// its own, before the low-memory line, and spends nothing.
    #[tokio::test]
    async fn memory_pressure_defers_before_memory_runs_low() {
        let now = crate::store::now_unix();
        let (store, ids) = store_with(&[("a", "process_died")], now - 120);
        sampled(&store, 8_000_000, 35.0, now);
        let fake = Fake::default();
        for _ in 0..2 {
            let r = run_with(&store, &fake, &NoAdvice, &CFG, now).await;
            assert_eq!((r.deferred, r.restored), (1, 0));
        }
        assert_eq!(
            events(&store, ids[0]),
            vec![(
                EVENT_DEFERRED.to_string(),
                Some(DEFER_MEM_PRESSURE.to_string())
            )]
        );
        // Jev cannot talk it past the floor either.
        let says = Says(Some(advised(Advice::RecoverNow, true)), Mutex::new(0));
        run_with(&store, &fake, &says, &CFG, now).await;
        assert!(fake.restored.lock().unwrap().is_empty());
        sampled(&store, 8_000_000, 2.0, now + 1);
        let r = run_with(&store, &fake, &NoAdvice, &CFG, now + 1).await;
        assert_eq!(r.restored, 1);
    }

    /// A restore launched on the last tick is not in the host's sample yet:
    /// it still needs its memory, so the next one waits for room for both.
    #[tokio::test]
    async fn a_restore_still_starting_counts_against_the_hosts_memory() {
        let now = crate::store::now_unix();
        let (store, ids) = store_with(&[("a", "process_died"), ("b", "process_died")], now - 120);
        sampled(&store, RESTORE_MEM_KB * 3 / 2, 0.0, now);
        // A small host, so 900 MB available is well above the low line.
        store
            .lock()
            .unwrap()
            .conn_ref()
            .execute(
                "UPDATE hosts SET mem_total_kb = 2000000 WHERE alias = 'h'",
                [],
            )
            .unwrap();
        let fake = Fake::default();
        let r = run_with(&store, &fake, &NoAdvice, &CFG, now).await;
        assert_eq!(r.restored, 1, "one per host per tick");
        assert_eq!(
            *fake.restored.lock().unwrap(),
            vec![("h".to_string(), vec![ids[0]])]
        );
        // The real restore brings the row back; the fake does not.
        store
            .lock()
            .unwrap()
            .conn_ref()
            .execute(
                "UPDATE sessions SET status = 'running', lost_at = NULL, lost_reason = NULL \
                 WHERE id = ?1",
                [ids[0]],
            )
            .unwrap();
        let r = run_with(&store, &fake, &NoAdvice, &CFG, now + 20).await;
        assert_eq!((r.deferred, r.restored), (1, 0));
        assert_eq!(
            events(&store, ids[1]),
            vec![(
                EVENT_DEFERRED.to_string(),
                Some(DEFER_MEM_HEADROOM.to_string())
            )]
        );
        // Once a has settled, the same reading has room for b. The settle
        // window runs from a's `restored` event, which the store stamps
        // with the wall clock, not with `now`: counted from `now`, a second
        // that ticked over between the two (the template migration of a
        // test process's first store alone can take that long) left a still
        // "starting" at `later`, and b deferred again.
        let restored_at: i64 = store
            .lock()
            .unwrap()
            .conn_ref()
            .query_row(
                "SELECT MAX(at) FROM session_events WHERE session_id = ?1 AND kind = ?2",
                rusqlite::params![ids[0], EVENT_RESTORED],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            restored_at >= now,
            "stamped by the clock, at or after `now`"
        );
        let later = restored_at + RESTORE_SETTLE_SECS + 1;
        let r = run_with(&store, &fake, &NoAdvice, &CFG, later).await;
        assert_eq!(r.restored, 1);
    }

    /// 2026-10-16: two storms on one host spent every session's budget, so
    /// the next loss of each was `budget_spent`. A whole-host loss soon after
    /// an automatic restore gives that restore back; a session's own death
    /// does not, nor does a restore long before the loss.
    #[tokio::test]
    async fn a_mass_loss_soon_after_a_restore_gives_that_restore_back() {
        let now = crate::store::now_unix();
        let lost_at = now - MASS_LOSS_GRACE_SECS;
        let (store, ids) = store_with(
            &[
                ("gone", "tmux_server_gone"),
                ("died", "process_died"),
                ("old", "tmux_server_gone"),
            ],
            lost_at,
        );
        for &id in &ids[..2] {
            event_at(
                &store,
                id,
                EVENT_RESTORED,
                "tmux_server_gone",
                lost_at - 3_600,
            );
            event_at(&store, id, EVENT_RESTORED, "tmux_server_gone", lost_at - 60);
        }
        event_at(
            &store,
            ids[2],
            EVENT_RESTORED,
            "tmux_server_gone",
            lost_at - 7_200,
        );
        event_at(
            &store,
            ids[2],
            EVENT_RESTORED,
            "tmux_server_gone",
            lost_at - MASS_LOSS_REFUND_SECS - 1,
        );
        let fake = Fake::default();
        run_with(&store, &fake, &NoAdvice, &CFG, now).await;
        assert_eq!(
            *fake.restored.lock().unwrap(),
            vec![("h".to_string(), vec![ids[0]])],
            "only the mass loss right after a restore has budget left"
        );
        for &id in &ids[1..] {
            assert!(
                events(&store, id)
                    .contains(&(EVENT_SKIPPED.to_string(), Some(SKIP_BUDGET.to_string()))),
                "{id}"
            );
        }
        // Only the latest restore is given back: one more storm and the
        // session waits for a person.
        let (store, ids) = store_with(&[("gone", "tmux_server_gone")], lost_at);
        for back in [3_600, 600, 60] {
            event_at(
                &store,
                ids[0],
                EVENT_RESTORED,
                "tmux_server_gone",
                lost_at - back,
            );
        }
        let fake = Fake::default();
        run_with(&store, &fake, &NoAdvice, &CFG, now).await;
        assert!(fake.restored.lock().unwrap().is_empty());
    }

    /// Part 3's judgment, end to end: two real samples over
    /// `health.cpu_busy_pct` make the host `cpu_saturated`, which defers
    /// the restore with the CPU reason; one calm sample lets it through.
    #[tokio::test]
    async fn a_saturated_cpu_defers_on_two_samples_and_a_calm_one_lets_it_through() {
        let now = crate::store::now_unix();
        let (store, ids) = store_with(&[("a", "process_died")], now - 120);
        let sample = |busy: i64, total: i64, at: i64| {
            store
                .lock()
                .unwrap()
                .set_host_health(
                    "h",
                    &crate::tmux::HostHealthSample {
                        cpu_busy_ticks: Some(busy),
                        cpu_total_ticks: Some(total),
                        mem_avail_kb: Some(8_000_000),
                        mem_total_kb: Some(16_000_000),
                        ..Default::default()
                    },
                    at,
                )
                .unwrap();
        };
        let fake = Fake::default();
        sample(0, 0, now - 40);
        sample(95, 100, now - 20); // 95 %, but the first reading
        assert!(
            !store
                .lock()
                .unwrap()
                .get_host_row("h")
                .unwrap()
                .unwrap()
                .cpu_saturated
        );
        sample(190, 200, now); // 95 % again: saturated
        assert!(
            store
                .lock()
                .unwrap()
                .get_host_row("h")
                .unwrap()
                .unwrap()
                .cpu_saturated
        );
        let r = run_with(&store, &fake, &NoAdvice, &CFG, now).await;
        assert_eq!((r.deferred, r.restored), (1, 0));
        assert_eq!(
            events(&store, ids[0]),
            vec![(EVENT_DEFERRED.to_string(), Some(DEFER_CPU.to_string()))]
        );
        sample(200, 300, now + 1); // 10 %
        let r = run_with(&store, &fake, &NoAdvice, &CFG, now + 1).await;
        assert_eq!(r.restored, 1);
    }

    #[tokio::test]
    async fn jev_recover_now_never_crosses_the_memory_floor() {
        let now = crate::store::now_unix();
        let (store, ids) = store_with(&[("a", "process_died")], now - 120);
        short_of_memory(&store, now);
        let says = Says(Some(advised(Advice::RecoverNow, true)), Mutex::new(0));
        let fake = Fake::default();
        run_with(&store, &fake, &says, &CFG, now).await;
        assert_eq!(*says.1.lock().unwrap(), 1, "a loaded host asks Jev");
        assert!(fake.restored.lock().unwrap().is_empty());
        let ev = events(&store, ids[0]);
        assert_eq!(ev.len(), 1);
        assert!(ev[0].1.as_deref().unwrap().starts_with(DEFER_JEV), "{ev:?}");
    }

    #[tokio::test]
    async fn a_calm_due_restore_asks_nobody() {
        let now = crate::store::now_unix();
        let (store, _) = store_with(&[("a", "process_died")], now - 120);
        let says = Says(Some(advised(Advice::Defer, true)), Mutex::new(0));
        run_with(&store, &Fake::default(), &says, &CFG, now).await;
        assert_eq!(*says.1.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn the_operator_is_restarted_and_told() {
        let now = crate::store::now_unix();
        let (store, ids) = store_with(&[("fleet-operator", "process_died")], now - 120);
        crate::service::operator::set_operator_ref(
            &store.lock().unwrap(),
            &crate::service::operator::OperatorRef {
                host_alias: "h".into(),
                tmux_name: "fleet-operator".into(),
            },
        )
        .unwrap();
        let fake = Fake::default();
        run_with(&store, &fake, &NoAdvice, &CFG, now).await;
        assert!(
            fake.restored.lock().unwrap().is_empty(),
            "never batch-restored"
        );
        assert_eq!(
            *fake.restarted.lock().unwrap(),
            vec!["fleet-operator".to_string()]
        );
        let notes = fake.notes.lock().unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].0, 42);
        assert!(notes[0]
            .1
            .starts_with("[fleet] Your session was restarted automatically"));
        assert_eq!(events(&store, ids[0]).len(), 1);
    }

    fn stranded_at(silent_for: Option<i64>, attempts: u32) -> Stranded {
        Stranded {
            session_id: 1,
            host: "mac".into(),
            fallback: "mefistos".into(),
            silent_for,
            attempts,
        }
    }

    #[test]
    fn control_moves_only_once_its_host_has_been_silent_a_while_and_within_budget() {
        assert!(
            !move_due(&stranded_at(Some(30), 0), &CFG),
            "a blip is waited out"
        );
        assert!(!move_due(
            &stranded_at(Some(OPERATOR_MOVE_AFTER_SECS - 1), 0),
            &CFG
        ));
        assert!(move_due(
            &stranded_at(Some(OPERATOR_MOVE_AFTER_SECS), 0),
            &CFG
        ));
        assert!(
            move_due(&stranded_at(None, 0), &CFG),
            "never answered, hidden or removed"
        );
        assert!(
            !move_due(&stranded_at(None, CFG.max_attempts), &CFG),
            "budget spent"
        );
        let slow = AutoRestoreConfig {
            grace_secs: 900,
            ..CFG
        };
        assert!(
            !move_due(&stranded_at(Some(600), 0), &slow),
            "a longer grace wins"
        );
    }

    /// Control recorded on `mac`, alive, with MCP on; `mac` stopped answering
    /// `silent_for` seconds ago and `spare` can take it.
    fn store_with_control_stranded(silent_for: i64, now: i64) -> (Mutex<Store>, i64, SessionRow) {
        use crate::service::operator::{set_operator_ref, OperatorRef, OPERATOR_TMUX_NAME};
        let s = Store::open_in_memory().unwrap();
        s.set_setting(crate::mcp::SETTING_ENABLED, "true").unwrap();
        s.set_setting(crate::service::operator::SETTING_OPERATOR_HOST, "mac")
            .unwrap();
        for alias in ["mac", "spare"] {
            s.upsert_host(alias).unwrap();
        }
        s.conn_ref()
            .execute(
                "UPDATE hosts SET reachable = 0, last_reachable_at = ?1 WHERE alias = 'mac'",
                [now - silent_for],
            )
            .unwrap();
        s.conn_ref()
            .execute(
                "UPDATE hosts SET reachable = 1, claude_version = '2.1.0', last_reachable_at = ?1 \
                 WHERE alias = 'spare'",
                [now],
            )
            .unwrap();
        let old = s
            .upsert_session(OPERATOR_TMUX_NAME, "mac", None, None, 1, 1, "running", None)
            .unwrap();
        set_operator_ref(
            &s,
            &OperatorRef {
                host_alias: "mac".into(),
                tmux_name: OPERATOR_TMUX_NAME.into(),
            },
        )
        .unwrap();
        let new = s
            .upsert_session(
                "fleet-operator-2",
                "spare",
                None,
                None,
                1,
                1,
                "running",
                None,
            )
            .unwrap();
        let new = s.get_session_by_id(new).unwrap().unwrap();
        (Mutex::new(s), old, new)
    }

    fn kinds(store: &Mutex<Store>, id: i64) -> Vec<(String, Option<String>)> {
        store
            .lock()
            .unwrap()
            .list_session_events(id, 50)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind.starts_with("operator_move"))
            .map(|e| (e.kind, e.detail))
            .collect()
    }

    #[tokio::test]
    async fn control_on_a_silent_host_is_moved_and_told() {
        let now = crate::store::now_unix();
        let (store, old, new) = store_with_control_stranded(OPERATOR_MOVE_AFTER_SECS + 60, now);
        let fake = Fake::default();
        *fake.moved_to.lock().unwrap() = Some(new.clone());
        let r = run_with(&store, &fake, &NoAdvice, &CFG, now).await;
        assert!(r.operator_moved);
        assert_eq!(*fake.moved.lock().unwrap(), 1);
        assert_eq!(
            kinds(&store, old),
            vec![(
                EVENT_OPERATOR_MOVED.to_string(),
                Some("mac->spare".to_string())
            )]
        );
        let notes = fake.notes.lock().unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].0, new.id);
        assert!(
            notes[0].1.contains("mac stopped answering"),
            "{}",
            notes[0].1
        );
    }

    #[tokio::test]
    async fn control_waits_out_a_short_silence_and_stops_after_its_budget() {
        let now = crate::store::now_unix();
        let (store, _, _) = store_with_control_stranded(60, now);
        let fake = Fake::default();
        run_with(&store, &fake, &NoAdvice, &CFG, now).await;
        assert_eq!(*fake.moved.lock().unwrap(), 0, "a minute is not down");

        let (store, old2, _) = store_with_control_stranded(OPERATOR_MOVE_AFTER_SECS + 60, now);
        let fake = Fake {
            fail_with: Some(codes::E_HOST_OFFLINE),
            ..Default::default()
        };
        for _ in 0..4 {
            run_with(&store, &fake, &NoAdvice, &CFG, now).await;
        }
        assert_eq!(
            *fake.moved.lock().unwrap(),
            CFG.max_attempts as usize,
            "failed moves spend the budget"
        );
        assert_eq!(kinds(&store, old2).len(), CFG.max_attempts as usize);
        assert!(kinds(&store, old2)
            .iter()
            .all(|(k, _)| k == EVENT_OPERATOR_MOVE_FAILED));
    }

    #[tokio::test]
    async fn the_controller_is_left_alone() {
        let now = crate::store::now_unix();
        let (store, ids) = store_with(&[("ctl", "process_died")], now - 120);
        store.lock().unwrap().set_controller("h", "ctl").unwrap();
        let fake = Fake::default();
        run_with(&store, &fake, &NoAdvice, &CFG, now).await;
        assert!(fake.restored.lock().unwrap().is_empty());
        assert!(
            events(&store, ids[0]).is_empty(),
            "the controller is never touched"
        );
    }

    #[tokio::test]
    async fn pause_all_stops_the_auto_restore() {
        let now = crate::store::now_unix();
        let (store, ids) = store_with(&[("a", "process_died")], now - 120);
        settings::set(&store.lock().unwrap(), settings::AUTOMATION_PAUSED, "true").unwrap();
        let fake = Fake::default();
        assert_eq!(
            run_with(&store, &fake, &NoAdvice, &CFG, now).await,
            RunReport::default()
        );
        assert!(fake.restored.lock().unwrap().is_empty());
        assert!(events(&store, ids[0]).is_empty());
        assert!(!crate::service::loops::gate("auto_restore", &store, None));
    }
}
