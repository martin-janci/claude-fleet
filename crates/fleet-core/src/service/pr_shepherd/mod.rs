//! PR shepherd: watch the pull requests fleet's own sessions opened and ask
//! the session that opened one to fix it when it conflicts with its base,
//! falls behind, or goes red. Design:
//! `docs/superpowers/specs/2026-10-08-pr-shepherd-design.md`.
//!
//! **Nothing happens without a person's rule.** A project with no row in
//! `pr_shepherd_rules` (migration 136) is never looked at, and only a person
//! writes that row (`fleet-hub shepherd grant`). The rule's level says how
//! far the shepherd goes:
//!
//! | level   | what the shepherd does                                       |
//! |---------|--------------------------------------------------------------|
//! | `watch` | records the episode on the session's timeline, sends nothing |
//! | `nudge` | also sends the session's own Claude one fix prompt           |
//! | `merge` | also merges a green PR, one per project at a time (`merge.rs`) |
//!
//! It reads what the reconcile pass already probed (`sessions.pr_evidence`,
//! the PR probe in `outcome.rs`): no extra GitHub call. An **episode** is one
//! problem on one pushed commit, keyed `(session, head commit, condition)`,
//! and the shepherd acts on an episode at most once: a push that fixes or
//! changes things is a new head and so a new episode.
//!
//! A nudge waits for a quiet session (idle for [`IDLE_GRACE_SECS`], no
//! pending question, not stuck, nobody attached to the pane) and for the
//! worktree to match the PR's head (local commits not yet pushed mean a fix
//! is under way). A session gets at most [`MAX_NUDGES_PER_DAY`] nudges per
//! 24 h; past that the episode is recorded `skipped:budget` and left to a
//! person. The registered controller session is never nudged.
//!
//! The planner is pure; the executor is injected, as in `playbooks.rs`.

pub mod admin;
pub mod merge;
pub mod prompts;
#[cfg(test)]
mod tests;

use crate::ipc_error::IpcError;
use crate::service::outcome::PrEvidence;
use crate::ssh::SshClient;
use crate::store::{SessionRow, ShepherdEpisodeRow, ShepherdMergeRow, ShepherdRuleRow, Store};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

/// How long a session must have been idle before it is nudged: a turn that
/// just ended may be about to push.
pub const IDLE_GRACE_SECS: i64 = 60;

/// Nudges one session may get per [`NUDGE_WINDOW_SECS`].
pub const MAX_NUDGES_PER_DAY: u32 = 3;

pub const NUDGE_WINDOW_SECS: i64 = 86_400;

/// What is wrong with a PR, in the order the shepherd deals with it: a
/// conflicting PR's CI is moot until the conflict is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Condition {
    Conflict,
    Behind,
    CiRed,
}

impl Condition {
    pub fn as_str(self) -> &'static str {
        match self {
            Condition::Conflict => "conflict",
            Condition::Behind => "behind",
            Condition::CiRed => "ci_red",
        }
    }
}

/// The one condition the shepherd would act on for this PR reading, or
/// `None` when nothing is wrong (or the PR is merged or closed).
pub fn condition_of(ev: &PrEvidence) -> Option<Condition> {
    if matches!(ev.state.as_deref(), Some("MERGED") | Some("CLOSED")) {
        return None;
    }
    match ev.merge_state.as_deref() {
        Some("DIRTY") => return Some(Condition::Conflict),
        Some("BEHIND") => return Some(Condition::Behind),
        _ => {}
    }
    (ev.checks.failing_total > 0).then_some(Condition::CiRed)
}

/// The parts of a session row the planner reads.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub session_id: i64,
    pub host_alias: String,
    pub tmux_name: String,
    pub project_id: i64,
    /// The session's org, for Jev's consent (step 4).
    pub org_id: Option<i64>,
    pub pr_url: Option<String>,
    pub evidence: PrEvidence,
    /// A live tmux session with a pane fleet can type into.
    pub live: bool,
    /// Mid-turn, blocked on a question or stuck: not now.
    pub busy: bool,
    pub idle_since: Option<i64>,
    pub is_controller: bool,
}

impl Candidate {
    /// `None` for a row with no project or no PR reading.
    pub fn from_row(r: &SessionRow, controller: Option<&(String, String)>) -> Option<Self> {
        let project_id = r.project_id?;
        let evidence = r.pr_evidence.clone()?;
        let busy = matches!(
            r.claude_status.as_deref(),
            Some("working") | Some("blocked")
        ) || r.pending_input.is_some()
            || r.stuck_kind.is_some();
        Some(Candidate {
            session_id: r.id,
            host_alias: r.host_alias.clone(),
            tmux_name: r.tmux_name.clone(),
            project_id,
            org_id: r.org_id,
            pr_url: r.pr_url.clone(),
            evidence,
            live: r.status == "running" && !crate::store::has_no_pane(&r.kind),
            busy,
            idle_since: r.idle_since,
            is_controller: controller.is_some_and(|(h, t)| h == &r.host_alias && t == &r.tmux_name),
        })
    }
}

/// What the shepherd does about one episode this tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Record the episode with this outcome and send nothing.
    Record(String),
    /// Send this prompt, then record the outcome of the send.
    Nudge(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub session_id: i64,
    pub host_alias: String,
    pub tmux_name: String,
    pub head_oid: String,
    pub pr_url: Option<String>,
    pub condition: Condition,
    pub action: Action,
    /// For Jev's `pr_triage` (step 4), asked once the episode is recorded.
    pub org_id: Option<i64>,
    pub evidence: PrEvidence,
}

/// What the planner needs besides the candidates, read under one lock.
#[derive(Debug, Default)]
pub struct Seen {
    /// Episodes already recorded: `(session, head, condition)`.
    pub episodes: HashSet<(i64, String, &'static str)>,
    /// Nudges per session within [`NUDGE_WINDOW_SECS`].
    pub nudges: HashMap<i64, u32>,
    /// Heads the merge queue already tried: `(session, head)`.
    pub merges_tried: HashSet<(i64, String)>,
    /// When the merge queue last acted on each project.
    pub last_merge: HashMap<i64, i64>,
}

/// Pure: decide what to do this tick. `rules` are the rules in force,
/// by project id. A candidate whose episode is not yet ripe (the session
/// is busy, just went idle, or holds unpushed commits) gets nothing and is
/// looked at again next tick.
pub fn plan(
    candidates: &[Candidate],
    rules: &HashMap<i64, ShepherdRuleRow>,
    seen: &Seen,
    now: i64,
) -> Vec<Planned> {
    let mut out = Vec::new();
    for c in candidates {
        let Some(rule) = rules.get(&c.project_id) else {
            continue;
        };
        let Some(condition) = condition_of(&c.evidence) else {
            continue;
        };
        let Some(head) = c.evidence.head_oid.clone().filter(|h| !h.is_empty()) else {
            continue;
        };
        if seen
            .episodes
            .contains(&(c.session_id, head.clone(), condition.as_str()))
        {
            continue;
        }
        // The worktree is ahead of what GitHub judged: a fix is under way,
        // and this reading is about to be stale.
        let unpushed = c.evidence.ahead.is_some_and(|a| a > 0)
            || c.evidence
                .local_head
                .as_deref()
                .is_some_and(|l| !l.is_empty() && l != head);
        if unpushed {
            continue;
        }
        let action = if rule.level == "watch" {
            Action::Record("watched".into())
        } else if !c.live {
            Action::Record("skipped:no_session".into())
        } else if c.is_controller {
            Action::Record("skipped:controller".into())
        } else if c.busy || c.idle_since.is_none_or(|t| now - t < IDLE_GRACE_SECS) {
            continue;
        } else if seen.nudges.get(&c.session_id).copied().unwrap_or(0) >= MAX_NUDGES_PER_DAY {
            Action::Record("skipped:budget".into())
        } else {
            let failing: Vec<String> = c
                .evidence
                .checks
                .failing
                .iter()
                .map(|f| f.name.clone())
                .collect();
            Action::Nudge(prompts::prompt_for(
                condition,
                c.pr_url.as_deref(),
                &head,
                &failing,
                rule.recipes.as_deref(),
            ))
        };
        out.push(Planned {
            session_id: c.session_id,
            host_alias: c.host_alias.clone(),
            tmux_name: c.tmux_name.clone(),
            head_oid: head,
            pr_url: c.pr_url.clone(),
            condition,
            action,
            org_id: c.org_id,
            evidence: c.evidence.clone(),
        });
    }
    out
}

/// What a send did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NudgeOutcome {
    Sent,
    /// A person is attached to the pane; nothing was typed. The episode is
    /// left open and tried again on a later tick.
    Attached,
}

/// What a merge attempt did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    Merged,
    /// GitHub's live answer disagreed with the stored reading; nothing was
    /// merged. The reason is a vocabulary word ([`merge::live_refusal`]).
    Refused(String),
}

/// The shepherd's side effects, injected so the runner is testable.
#[async_trait::async_trait]
pub trait ShepherdExec: Send + Sync {
    async fn nudge(
        &self,
        host_alias: &str,
        tmux_name: &str,
        prompt: &str,
    ) -> Result<NudgeOutcome, IpcError>;

    /// Ask GitHub again and merge only if it agrees (see `merge.rs`).
    async fn merge_if_green(
        &self,
        host_alias: &str,
        pr_url: &str,
        head_oid: &str,
    ) -> Result<MergeOutcome, IpcError>;

    /// A new episode was recorded: Jev's `pr_triage` (step 4) may ask what
    /// it needs, in shadow. Must not hold up the tick. Default: nothing.
    async fn triage(
        &self,
        _session_id: i64,
        _org_id: Option<i64>,
        _condition: Condition,
        _evidence: &PrEvidence,
    ) {
    }
}

/// Production executor: skip an attached pane (as `press_enter` does), else
/// deliver the prompt as fleet's own (`send_system_prompt`).
pub struct RealShepherdExec {
    pub store: Arc<Mutex<Store>>,
    pub ssh: Arc<SshClient>,
}

#[async_trait::async_trait]
impl ShepherdExec for RealShepherdExec {
    async fn triage(
        &self,
        session_id: i64,
        org_id: Option<i64>,
        condition: Condition,
        evidence: &PrEvidence,
    ) {
        let off = match self.store.lock() {
            Ok(s) => {
                crate::service::decide::FeatureMode::of(
                    &s,
                    crate::service::decide::Feature::PrTriage,
                ) == crate::service::decide::FeatureMode::Off
            }
            Err(_) => true,
        };
        if off {
            return;
        }
        let ctx = crate::service::decide::DecideCtx::jev(Arc::clone(&self.store));
        let evidence = evidence.clone();
        crate::rt::spawn(async move {
            crate::service::decide::pr_triage::triage(
                &ctx, session_id, org_id, condition, &evidence,
            )
            .await;
        });
    }

    async fn nudge(
        &self,
        host_alias: &str,
        tmux_name: &str,
        prompt: &str,
    ) -> Result<NudgeOutcome, IpcError> {
        crate::validate::tmux_name_addressable(tmux_name)?;
        let attached = crate::service::sessions::run_host_script(
            &self.ssh,
            host_alias,
            &crate::service::playbooks::attached_probe_script(tmux_name),
            std::time::Duration::from_secs(10),
        )
        .await?;
        if !attached.status.success()
            || crate::service::playbooks::pane_is_attached(&String::from_utf8_lossy(
                &attached.stdout,
            ))
        {
            return Ok(NudgeOutcome::Attached);
        }
        crate::service::sessions::send_system_prompt(
            host_alias,
            tmux_name,
            prompt,
            true,
            &self.store,
            &self.ssh,
        )
        .await?;
        Ok(NudgeOutcome::Sent)
    }

    async fn merge_if_green(
        &self,
        host_alias: &str,
        pr_url: &str,
        head_oid: &str,
    ) -> Result<MergeOutcome, IpcError> {
        let timeout = std::time::Duration::from_secs(60);
        let view = crate::service::sessions::run_host_script(
            &self.ssh,
            host_alias,
            &merge::view_script(pr_url)?,
            timeout,
        )
        .await?;
        if !view.status.success() {
            return Err(IpcError::new(
                crate::ipc_error::codes::E_SHELL,
                String::from_utf8_lossy(&view.stderr).trim().to_string(),
            ));
        }
        if let Some(why) = merge::live_refusal(&String::from_utf8_lossy(&view.stdout), head_oid) {
            return Ok(MergeOutcome::Refused(why));
        }
        let out = crate::service::sessions::run_host_script(
            &self.ssh,
            host_alias,
            &merge::merge_script(pr_url, head_oid)?,
            timeout,
        )
        .await?;
        if !out.status.success() {
            return Err(IpcError::new(
                crate::ipc_error::codes::E_SHELL,
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ));
        }
        Ok(MergeOutcome::Merged)
    }
}

/// Read the rules in force, the candidates and what was seen, under one lock.
fn snapshot(s: &Store, now: i64) -> (Vec<Candidate>, HashMap<i64, ShepherdRuleRow>, Seen) {
    let rules: HashMap<i64, ShepherdRuleRow> = s
        .list_shepherd_rules()
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.active_at(now))
        .map(|r| (r.project_id, r))
        .collect();
    if rules.is_empty() {
        return (Vec::new(), rules, Seen::default());
    }
    let controller = s.get_controller().ok().flatten();
    let candidates: Vec<Candidate> = s
        .list_all_sessions()
        .unwrap_or_default()
        .iter()
        .filter_map(|r| Candidate::from_row(r, controller.as_ref()))
        .filter(|c| rules.contains_key(&c.project_id))
        .collect();
    let mut seen = Seen::default();
    for c in &candidates {
        if let Some(head) = c.evidence.head_oid.as_deref() {
            if let Ok(true) =
                s.shepherd_merge_tried(c.session_id, head, now - merge::REFUSAL_RETRY_SECS)
            {
                seen.merges_tried.insert((c.session_id, head.to_string()));
            }
        }
        if let std::collections::hash_map::Entry::Vacant(e) = seen.last_merge.entry(c.project_id) {
            if let Ok(Some(t)) = s.last_shepherd_merge_at(c.project_id) {
                e.insert(t);
            }
        }
        let (Some(cond), Some(head)) = (condition_of(&c.evidence), c.evidence.head_oid.as_deref())
        else {
            continue;
        };
        if let Ok(Some(_)) = s.shepherd_episode_outcome(c.session_id, head, cond.as_str()) {
            seen.episodes
                .insert((c.session_id, head.to_string(), cond.as_str()));
        }
        if let Ok(n) = s.count_shepherd_nudges_since(c.session_id, now - NUDGE_WINDOW_SECS) {
            seen.nudges.insert(c.session_id, n);
        }
    }
    (candidates, rules, seen)
}

/// Plan and apply one tick: nudges first, then at most one merge per
/// project. Returns the number of episodes and merges recorded.
pub async fn run_with(store: &Mutex<Store>, exec: &dyn ShepherdExec, now: i64) -> usize {
    let (planned, merges) = {
        let Ok(s) = store.lock() else {
            return 0;
        };
        // Pause all (redesign 8.1): no nudge, no merge.
        if crate::service::loops::paused(&s) {
            return 0;
        }
        let (candidates, rules, seen) = snapshot(&s, now);
        (
            plan(&candidates, &rules, &seen, now),
            merge::plan_merges(
                &candidates,
                &rules,
                &seen.merges_tried,
                &seen.last_merge,
                now,
            ),
        )
    };
    let mut recorded = 0;
    for p in planned {
        let outcome = match &p.action {
            Action::Record(o) => o.clone(),
            Action::Nudge(prompt) => match exec.nudge(&p.host_alias, &p.tmux_name, prompt).await {
                Ok(NudgeOutcome::Sent) => "nudged".to_string(),
                Ok(NudgeOutcome::Attached) => continue,
                Err(e) => {
                    tracing::warn!(
                        host = %p.host_alias,
                        session = %p.tmux_name,
                        error = %e,
                        "[pr_shepherd] nudge failed"
                    );
                    format!("failed:{}", e.message)
                }
            },
        };
        tracing::info!(
            host = %p.host_alias,
            session = %p.tmux_name,
            condition = p.condition.as_str(),
            outcome = %outcome,
            "[pr_shepherd] episode"
        );
        let fresh = {
            let Ok(s) = store.lock() else {
                continue;
            };
            match s.record_shepherd_episode(&ShepherdEpisodeRow {
                session_id: p.session_id,
                head_oid: p.head_oid,
                condition: p.condition.as_str().to_string(),
                pr_url: p.pr_url,
                at: now,
                outcome,
            }) {
                Ok(fresh) => fresh,
                Err(e) => {
                    tracing::warn!(
                        session_id = p.session_id,
                        error = %e,
                        "[pr_shepherd] recording the episode failed"
                    );
                    false
                }
            }
        };
        if fresh {
            recorded += 1;
            exec.triage(p.session_id, p.org_id, p.condition, &p.evidence)
                .await;
        }
    }
    for m in merges {
        // The merges were planned before the nudges went out over SSH: a
        // Pause all pressed meanwhile stops them (review r06 F5).
        if store
            .lock()
            .map_or(true, |s| crate::service::loops::paused(&s))
        {
            break;
        }
        let outcome = match exec
            .merge_if_green(&m.host_alias, &m.pr_url, &m.head_oid)
            .await
        {
            Ok(MergeOutcome::Merged) => "merged".to_string(),
            Ok(MergeOutcome::Refused(why)) => format!("skipped:{why}"),
            Err(e) => {
                tracing::warn!(
                    host = %m.host_alias,
                    pr = %m.pr_url,
                    error = %e,
                    "[pr_shepherd] merge failed"
                );
                format!("failed:{}", e.message)
            }
        };
        tracing::info!(pr = %m.pr_url, outcome = %outcome, "[pr_shepherd] merge");
        let Ok(s) = store.lock() else {
            continue;
        };
        match s.record_shepherd_merge(&ShepherdMergeRow {
            session_id: m.session_id,
            head_oid: m.head_oid,
            project_id: m.project_id,
            pr_url: m.pr_url,
            at: now,
            outcome,
        }) {
            Ok(true) => recorded += 1,
            Ok(false) => {}
            Err(e) => tracing::warn!(
                session_id = m.session_id,
                error = %e,
                "[pr_shepherd] recording the merge failed"
            ),
        }
    }
    recorded
}

/// Reconcile-tick hook: [`run`] in its own single-flight task, as
/// `gc::spawn_sweep` and `usage::spawn_collect` do. A merge awaits two `gh`
/// calls over SSH with 60 s bounds, and inline they held the tick body, which
/// skips missed ticks, so every session's status went stale behind a slow
/// GitHub (review r16). A spawn while one is still running is a no-op.
pub fn spawn_run(store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>, period: std::time::Duration) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static RUNNING: AtomicBool = AtomicBool::new(false);
    struct Flight;
    impl Drop for Flight {
        fn drop(&mut self) {
            RUNNING.store(false, Ordering::SeqCst);
        }
    }
    if RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    let flight = Flight;
    let (store, ssh) = (Arc::clone(store), Arc::clone(ssh));
    tokio::spawn(async move {
        let _flight = flight;
        let n = run(&store, &ssh).await;
        if n > 0 {
            tracing::info!("pr shepherd: recorded {n} episode(s)");
        }
        crate::service::loops::report("pr_shepherd", Ok::<_, String>(()), Some(period));
    });
}

/// Tick entry point with the real executor.
pub async fn run(store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>) -> usize {
    let exec = RealShepherdExec {
        store: Arc::clone(store),
        ssh: Arc::clone(ssh),
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    run_with(store, &exec, now).await
}
