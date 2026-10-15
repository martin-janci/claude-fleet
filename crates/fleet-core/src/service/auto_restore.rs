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
//! [`MAX_RESTORES_PER_TICK`] restores per tick. A spent budget is refused
//! once per loss episode ([`EVENT_SKIPPED`]).
//!
//! **Load-aware, not a hard skip.** A host under load — memory at or under
//! the low-memory line (`health::mem_low`, also the hard floor), or its CPU
//! saturated (`health::cpu_saturated`) — DEFERS the restore: no attempt is
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

/// [`EVENT_SKIPPED`] detail: `recovery.max_attempts` is spent.
pub const SKIP_BUDGET: &str = "budget_spent";
/// [`EVENT_DEFERRED`] detail: the host is short of memory.
pub const DEFER_MEMORY_LOW: &str = "memory_low";
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

/// PURE: the rule. A candidate still inside its grace, on a host that is
/// not ready (it comes back with the host), or refused for a reason
/// already written this episode produces nothing. A loaded host defers
/// (planned every tick so Jev may weigh in, written once per episode).
/// Restores are capped at [`MAX_RESTORES_PER_TICK`] in `lost_at` order
/// (oldest first); refusals and deferrals are not capped.
pub fn plan(cands: &[Candidate], cfg: &AutoRestoreConfig, now: i64) -> Vec<Planned> {
    if !cfg.enabled {
        return Vec::new();
    }
    let mut ordered: Vec<&Candidate> = cands.iter().collect();
    ordered.sort_by_key(|c| (c.lost_at, c.session_id));
    let mut out = Vec::new();
    let mut restores = 0usize;
    for c in ordered {
        if !RESUMABLE_LOST_REASONS.contains(&c.lost_reason.as_str())
            || now - c.lost_at < cfg.grace_secs
            || !c.host_ready
        {
            continue;
        }
        let (action, ask) = if c.attempts >= cfg.max_attempts {
            if c.skipped.iter().any(|d| d == SKIP_BUDGET) {
                continue;
            }
            (Action::Skip(SKIP_BUDGET), false)
        } else if let Some(why) = c.load {
            (Action::Defer(why.to_string()), true)
        } else if restores >= MAX_RESTORES_PER_TICK {
            continue;
        } else {
            restores += 1;
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
}

/// What a run did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunReport {
    pub restored: usize,
    pub failed: usize,
    pub skipped: usize,
    /// Planned deferrals, written or not (one event per episode).
    pub deferred: usize,
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
/// floor), then CPU. `None` when calm or unknown.
///
/// Both are `fleet_health`'s own judgments (`health::mem_low`,
/// `health::cpu_saturated`: `health.cpu_busy_pct` on two samples in a
/// row), so a host the Hosts view shows as loaded is the host this defers
/// on, and the reverse.
pub fn host_load(h: &HostRow, cfg: &AutoRestoreConfig, now: i64) -> Option<&'static str> {
    if crate::service::health::mem_low(h, cfg.mem_low_pct, now) {
        Some(DEFER_MEMORY_LOW)
    } else if crate::service::health::cpu_saturated(h, now) {
        Some(DEFER_CPU)
    } else {
        None
    }
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
                let attempts = s
                    .count_session_events_since(r.id, &[EVENT_RESTORED, EVENT_FAILED], since)
                    .unwrap_or(u32::MAX);
                let load = host.and_then(|h| host_load(h, cfg, now));
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
                                    && host_load(h, cfg, now).is_none()
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
                    load,
                    floor: host
                        .is_some_and(|h| crate::service::health::mem_low(h, cfg.mem_low_pct, now)),
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
    let mut restores = planned
        .iter()
        .filter(|p| matches!(p.action, Action::Restore | Action::RestartOperator))
        .count();
    let mut by_host: BTreeMap<String, Vec<Candidate>> = BTreeMap::new();
    for p in planned {
        let advised = if p.ask {
            advisor.advise(&p.candidate.question).await
        } else {
            None
        };
        let action = apply_advice(&p, advised.as_ref(), restores < MAX_RESTORES_PER_TICK);
        if matches!(p.action, Action::Defer(_))
            && matches!(action, Action::Restore | Action::RestartOperator)
        {
            restores += 1;
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
        if r.restored + r.failed + r.skipped > 0 {
            tracing::info!(
                "[auto-restore] restored={} failed={} skipped={} deferred={}",
                r.restored,
                r.failed,
                r.skipped,
                r.deferred
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

    #[test]
    fn restores_resumable_rows_past_the_grace_only() {
        let c = vec![
            cand(1, "process_died", 100),
            cand(2, "tmux_server_gone", 100),
            cand(3, "host_reboot", 100),
            cand(4, "missing", 100),
            cand(5, "killed", 100),
            cand(6, "process_died", 150),
        ];
        assert_eq!(
            actions(&plan(&c, &CFG, 200)),
            vec![
                (1, Action::Restore),
                (2, Action::Restore),
                (3, Action::Restore)
            ],
            "6 is still inside its 60 s grace; missing and killed are never restored"
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
        let mut c: Vec<Candidate> = (1..=6).map(|i| cand(i, "host_reboot", 10 - i)).collect();
        c[5].operator = true; // id 6, lost first
        let p = plan(&c, &CFG, 1_000);
        assert_eq!(p.len(), MAX_RESTORES_PER_TICK);
        assert_eq!(p[0].candidate.session_id, 6);
        assert_eq!(p[0].action, Action::RestartOperator);
        assert!(p[1..].iter().all(|p| p.action == Action::Restore));
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
        let (store, ids) = store_with(&[("a", "host_reboot")], now - 120);
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
