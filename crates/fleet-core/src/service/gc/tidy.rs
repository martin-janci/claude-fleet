//! Work-aware tidy-up (work graph M7.1): which live sessions look finished,
//! and why. A pure planner next to the idle killer's [`super::plan`]; its
//! output is SHOWN (the Tidy-up sheet, `work { action: tidy }`) and acted on
//! only by a person's confirm — or, with `work.auto_tidy` on, by the sweep
//! for the `SafeKill` / `Archive` candidates of the allowed reasons.
//!
//! The protections in [`protection`] are not settings: nothing this module
//! returns ever names a session that is working, blocked, stuck or waiting on
//! a dialog, the controller or the operator, a session a person prompted or
//! attached to within [`TOUCH_GRACE_SECS`], a background agent with open
//! tasks, or a session whose linked item is `in_progress`.
//!
//! Actions (the executor lives with the storage, M7.2):
//! - `SafeKill`: inspect the worktree; dirty or unpushed ⇒ the safe-kill path
//!   (Claude commits and pushes first), clean ⇒ a plain tmux kill.
//! - `Kill`: a plain tmux kill, only for rows whose worktree is not theirs to
//!   remove — a worktree another live session shares (a safe-remove would
//!   delete a tree still in use; a plain kill leaves every file on disk),
//!   and `bg` / `shell` / `review` rows, which the idle killer plain-kills too.
//! - `Archive`: UI-only (`work_links.archived_at`); tmux keeps running.
//! - `ResumeOrExpire`: a ghost whose resumable row is about to be reaped; no
//!   kill, the row is already gone from tmux.

use crate::store::SessionRow;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// A prompt or an attach within this many seconds protects a session.
pub const TOUCH_GRACE_SECS: i64 = 3600;
/// A resumable ghost is suggested this long before `sessions.lost_ttl_secs`
/// reaps it.
pub const GHOST_WARN_SECS: i64 = 86_400;

/// Why a session is a candidate. The order of [`TidyReason::RANKED`] is the
/// primary-reason ranking and the sheet's group order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TidyReason {
    /// The linked item is done for ≥ `work.tidy_done_days` and the session
    /// has been idle ≥ `work.tidy_idle_hours`.
    DoneIdle,
    /// The session's PR is merged and it has been idle ≥ `tidy_idle_hours`.
    PrMergedIdle,
    /// The item was resolved as won't-do or duplicate, and the session is idle.
    NotPlanned,
    /// Another live session works in the same worktree and this one is the
    /// idler of the two (idle ≥ `tidy_idle_hours`).
    DuplicateWorktree,
    /// A resumable ghost within a day of its `lost_ttl` reap.
    GhostExpiring,
    /// A reason a newer peer knows (wire enums never force a contract bump).
    #[serde(other)]
    Unknown,
}

impl TidyReason {
    /// Every known reason, most important first.
    pub const RANKED: &'static [TidyReason] = &[
        TidyReason::DoneIdle,
        TidyReason::PrMergedIdle,
        TidyReason::NotPlanned,
        TidyReason::DuplicateWorktree,
        TidyReason::GhostExpiring,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            TidyReason::DoneIdle => "done_idle",
            TidyReason::PrMergedIdle => "pr_merged_idle",
            TidyReason::NotPlanned => "not_planned",
            TidyReason::DuplicateWorktree => "duplicate_worktree",
            TidyReason::GhostExpiring => "ghost_expiring",
            TidyReason::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Option<TidyReason> {
        TidyReason::RANKED
            .iter()
            .copied()
            .find(|r| r.as_str() == s.trim())
    }

    fn rank(self) -> usize {
        TidyReason::RANKED
            .iter()
            .position(|r| *r == self)
            .unwrap_or(usize::MAX)
    }

    /// What the sheet preselects for this reason.
    fn default_action(self) -> TidyAction {
        match self {
            TidyReason::DoneIdle | TidyReason::PrMergedIdle | TidyReason::NotPlanned => {
                TidyAction::SafeKill
            }
            TidyReason::DuplicateWorktree => TidyAction::Kill,
            TidyReason::GhostExpiring => TidyAction::ResumeOrExpire,
            TidyReason::Unknown => TidyAction::Archive,
        }
    }
}

/// What applying a candidate does. See the module doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TidyAction {
    Archive,
    SafeKill,
    Kill,
    ResumeOrExpire,
    #[serde(other)]
    Unknown,
}

impl TidyAction {
    pub fn as_str(self) -> &'static str {
        match self {
            TidyAction::Archive => "archive",
            TidyAction::SafeKill => "safe_kill",
            TidyAction::Kill => "kill",
            TidyAction::ResumeOrExpire => "resume_or_expire",
            TidyAction::Unknown => "unknown",
        }
    }
}

/// The thresholds and the auto-tidy opt-in (`work.*` settings).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TidyConfig {
    /// `work.tidy_done_days`, in seconds.
    pub done_secs: i64,
    /// `work.tidy_idle_hours`, in seconds.
    pub idle_secs: i64,
    /// `sessions.lost_ttl_secs`; `0` keeps lost rows forever (no ghost ever
    /// expires).
    pub lost_ttl_secs: i64,
    /// `work.auto_tidy`: off by default.
    pub auto: bool,
    /// `work.auto_tidy_reasons`: the reasons auto-tidy may act on.
    pub auto_reasons: Vec<TidyReason>,
    /// Per-org overrides of [`Self::auto`] (`orgs.auto_tidy`, work graph M5):
    /// a session of an org listed here follows its org, the rest `auto`.
    pub org_auto: HashMap<i64, bool>,
}

impl TidyConfig {
    /// Whether auto-tidy is on for a session of `org`.
    pub fn auto_for(&self, org: Option<i64>) -> bool {
        org.and_then(|o| self.org_auto.get(&o).copied())
            .unwrap_or(self.auto)
    }

    /// Whether auto-tidy is on anywhere (globally or for some org).
    pub fn auto_anywhere(&self) -> bool {
        self.auto || self.org_auto.values().any(|on| *on)
    }
}

impl Default for TidyConfig {
    fn default() -> Self {
        TidyConfig {
            done_secs: 2 * 86_400,
            idle_secs: 4 * 3600,
            lost_ttl_secs: 14 * 86_400,
            auto: false,
            auto_reasons: vec![TidyReason::DoneIdle, TidyReason::PrMergedIdle],
            org_auto: HashMap::new(),
        }
    }
}

/// The session's primary confirmed live link, as the planner needs it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TidyLink {
    pub link_id: i64,
    pub key: Option<String>,
    /// The item's `status_category` (todo | in_progress | done | unknown);
    /// `None` for a bare key.
    pub status_category: Option<String>,
    pub status_name: Option<String>,
    /// completed | not_planned | duplicate.
    pub resolution: Option<String>,
    pub status_changed_at: Option<i64>,
    pub archived_at: Option<i64>,
    pub snoozed_until: Option<i64>,
    pub never: bool,
    /// The linked item's org (its tracker's, work graph M5).
    pub org_id: Option<i64>,
}

/// One session and everything the planner reads about it.
#[derive(Debug, Clone)]
pub struct TidySession {
    pub row: SessionRow,
    pub link: Option<TidyLink>,
    /// Any live confirmed link of the session is to an `in_progress` item.
    pub in_progress: bool,
    /// The PR probe saw the branch's PR merged.
    pub pr_merged: bool,
    /// Last prompt or attach (`sessions.last_touch_at`).
    pub last_touch_at: Option<i64>,
    /// The session is the worker or the requester of a queued / running task.
    pub open_tasks: bool,
    /// The live branch, for the preview.
    pub branch: Option<String>,
}

/// Who and what is off limits, and the clock.
#[derive(Debug, Clone, Copy)]
pub struct TidyContext<'a> {
    pub controller: Option<&'a (String, String)>,
    pub operator: Option<&'a (String, String)>,
    pub reachable: &'a HashSet<String>,
    pub now: i64,
}

/// One suggestion: a session, why, and the preselected action. The preview
/// fields are for the sheet; all `serde(default)`, so an older peer's
/// candidate still reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TidyCandidate {
    pub session_id: i64,
    /// The primary link: what snooze / never / archive write to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_id: Option<i64>,
    pub host_alias: String,
    pub tmux_name: String,
    #[serde(default)]
    pub kind: String,
    /// The session's org (work graph M5); `None` = unassigned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
    pub reason: TidyReason,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secondary: Vec<TidyReason>,
    pub action: TidyAction,
    /// When the primary reason's clock started (idle since; lost at).
    pub since: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// The item's status name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
    #[serde(default)]
    pub idle_secs: i64,
    /// A ghost: when its row is reaped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    /// The live link is archived (collapsed in the sidebar).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub archived: bool,
    /// Auto-tidy (when on) would act on it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto: bool,
    /// The linked item's org, for scoping the link's details (never sent).
    #[serde(skip)]
    pub link_org_id: Option<i64>,
}

impl TidyCandidate {
    /// Drop what names the linked work (a per-host token outside the link's
    /// org): the key, the item's status and the link itself.
    pub fn redact_link(&mut self) {
        self.key = None;
        self.item_status = None;
        self.link_id = None;
    }
}

/// Why a session is never a candidate, or `None`. Hard-coded: no setting
/// reaches this function.
pub fn protection(s: &TidySession, ctx: &TidyContext<'_>) -> Option<&'static str> {
    let r = &s.row;
    let is = |who: Option<&(String, String)>| {
        who.is_some_and(|(h, t)| h == &r.host_alias && t == &r.tmux_name)
    };
    if is(ctx.controller) {
        return Some("controller");
    }
    if is(ctx.operator) {
        return Some("operator");
    }
    if s.in_progress {
        return Some("in_progress");
    }
    if matches!(
        r.claude_status.as_deref(),
        Some("working" | "blocked" | "failed")
    ) {
        return Some("active");
    }
    if r.stuck_kind.is_some() || r.pending_input.is_some() {
        return Some("needs_you");
    }
    if s.last_touch_at
        .is_some_and(|t| ctx.now - t < TOUCH_GRACE_SECS)
    {
        return Some("touched");
    }
    if s.open_tasks {
        return Some("open_tasks");
    }
    if r.safe_kill_state.is_some() {
        return Some("safe_kill_in_flight");
    }
    if r.kind == "external" {
        return Some("external");
    }
    None
}

/// The unix second a row has been idle since, per the idle killer's rules.
fn idle_since(r: &SessionRow) -> Option<i64> {
    super::idle_reference(r)
}

/// Live `work` sessions grouped by worktree: `(host, project, worktree_key)`.
/// Reviews share their source's worktree by design and are not duplicates.
fn worktree_group(r: &SessionRow) -> Option<(String, Option<i64>, String)> {
    (r.status == "running" && r.kind == "work")
        .then(|| r.worktree_key.clone())
        .flatten()
        .map(|k| (r.host_alias.clone(), r.project_id, k))
}

/// How recently a session was in use, for "keep the most recent".
fn recency(s: &TidySession, now: i64) -> i64 {
    let busy = if idle_since(&s.row).is_none() { now } else { 0 };
    [
        busy,
        s.last_touch_at.unwrap_or(0),
        s.row.last_activity_at,
        s.row.idle_since.unwrap_or(0),
        s.row.last_turn_at.unwrap_or(0),
    ]
    .into_iter()
    .max()
    .unwrap_or(0)
}

/// Pure: the tidy-up candidates, ordered by reason rank then session id.
pub fn plan_tidy(
    sessions: &[TidySession],
    cfg: &TidyConfig,
    ctx: &TidyContext<'_>,
) -> Vec<TidyCandidate> {
    let now = ctx.now;
    // Shared worktrees: every live work session counts, protected or not —
    // a protected sibling is still using the tree.
    let mut groups: HashMap<(String, Option<i64>, String), Vec<usize>> = HashMap::new();
    for (i, s) in sessions.iter().enumerate() {
        if let Some(g) = worktree_group(&s.row) {
            groups.entry(g).or_default().push(i);
        }
    }
    let mut shared: HashSet<usize> = HashSet::new();
    let mut duplicate: HashSet<usize> = HashSet::new();
    for members in groups.values().filter(|m| m.len() >= 2) {
        shared.extend(members.iter().copied());
        let keep = members
            .iter()
            .copied()
            .max_by_key(|&i| (recency(&sessions[i], now), sessions[i].row.id))
            .unwrap_or(members[0]);
        duplicate.extend(members.iter().copied().filter(|&i| i != keep));
    }

    let mut out = Vec::new();
    for (i, s) in sessions.iter().enumerate() {
        let r = &s.row;
        if protection(s, ctx).is_some() {
            continue;
        }
        if let Some(l) = &s.link {
            if l.never || l.snoozed_until.is_some_and(|t| t > now) {
                continue;
            }
        }
        let mut reasons: Vec<(TidyReason, i64)> = Vec::new();
        let mut expires_at = None;
        match r.status.as_str() {
            "running" => {
                if !ctx.reachable.contains(&r.host_alias) {
                    continue;
                }
                let idle = idle_since(r).filter(|t| now - t >= cfg.idle_secs);
                if let (Some(since), Some(l)) = (idle, &s.link) {
                    let resolved_away =
                        matches!(l.resolution.as_deref(), Some("not_planned" | "duplicate"));
                    let done_long = l.status_category.as_deref() == Some("done")
                        && l.status_changed_at
                            .is_some_and(|t| now - t >= cfg.done_secs);
                    if done_long && !resolved_away {
                        reasons.push((TidyReason::DoneIdle, since));
                    }
                    if resolved_away {
                        reasons.push((TidyReason::NotPlanned, since));
                    }
                }
                if let Some(since) = idle {
                    if s.pr_merged {
                        reasons.push((TidyReason::PrMergedIdle, since));
                    }
                    if duplicate.contains(&i) {
                        reasons.push((TidyReason::DuplicateWorktree, since));
                    }
                }
            }
            "ghost" => {
                // Only linked, resumable work: the host's restore list covers
                // the rest, and a snooze needs a link to live on.
                if cfg.lost_ttl_secs <= 0 || s.link.is_none() || r.claude_session_id.is_none() {
                    continue;
                }
                if let Some(lost) = r.lost_at {
                    let expires = lost + cfg.lost_ttl_secs;
                    if expires > now && expires - now <= GHOST_WARN_SECS {
                        reasons.push((TidyReason::GhostExpiring, lost));
                        expires_at = Some(expires);
                    }
                }
            }
            _ => continue,
        }
        if reasons.is_empty() {
            continue;
        }
        reasons.sort_by_key(|(reason, _)| reason.rank());
        let (reason, since) = reasons[0];
        let mut action = reason.default_action();
        if action == TidyAction::SafeKill {
            if shared.contains(&i) || matches!(r.kind.as_str(), "bg" | "shell" | "review") {
                // Never safe-remove a tree another session uses; bg / shell /
                // review rows have no tree of their own to remove.
                action = TidyAction::Kill;
            } else if r.kind == "work" && r.worktree_id.is_none() {
                // No tracked worktree: the inspection cannot see it, so
                // nothing kills it; archiving is all that is offered.
                action = TidyAction::Archive;
            }
        }
        if action == TidyAction::Archive && s.link.is_none() {
            continue;
        }
        let auto = cfg.auto_for(r.org_id)
            && matches!(action, TidyAction::SafeKill | TidyAction::Archive)
            && cfg.auto_reasons.contains(&reason);
        out.push(TidyCandidate {
            session_id: r.id,
            link_id: s.link.as_ref().map(|l| l.link_id),
            host_alias: r.host_alias.clone(),
            tmux_name: r.tmux_name.clone(),
            kind: r.kind.clone(),
            org_id: r.org_id,
            reason,
            secondary: reasons[1..].iter().map(|(r, _)| *r).collect(),
            action,
            since,
            label: r.friendly_name.clone(),
            key: s.link.as_ref().and_then(|l| l.key.clone()),
            item_status: s
                .link
                .as_ref()
                .and_then(|l| l.status_name.clone().or_else(|| l.status_category.clone())),
            branch: s.branch.clone(),
            pr_url: r.pr_url.clone(),
            idle_secs: idle_since(r).map(|t| (now - t).max(0)).unwrap_or(0),
            expires_at,
            archived: s.link.as_ref().is_some_and(|l| l.archived_at.is_some()),
            auto,
            link_org_id: s.link.as_ref().and_then(|l| l.org_id),
        });
    }
    out.sort_by_key(|c| (c.reason.rank(), c.session_id));
    out
}

/// The candidates auto-tidy acts on: [`TidyCandidate::auto`], which is only
/// ever set for `SafeKill` / `Archive` of an allowed reason with
/// `work.auto_tidy` on.
pub fn auto_selection(candidates: &[TidyCandidate]) -> Vec<&TidyCandidate> {
    candidates.iter().filter(|c| c.auto).collect()
}

#[cfg(test)]
mod tests;
