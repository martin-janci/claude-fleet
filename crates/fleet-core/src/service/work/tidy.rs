//! Tidy-up (work graph M7.2): the one transport-agnostic entry for
//! `work { action: tidy }`, `work_link { action: tidy_apply }` and the
//! auto-tidy pass of the GC sweep. The planner is `service::gc::tidy`; this
//! module gathers its input, applies a person's choices, and executes
//! through the idle killer's executor ([`GcExec`]), so a tidy kill is the
//! same kill (and the same safe kill) the GC already performs.
//!
//! Safety, restated where it is enforced:
//! - every destructive item is re-checked against the planner's protections
//!   at apply time (the sheet may be minutes old);
//! - `kill` and `safe_kill` both inspect a work session's own worktree, so a
//!   dirty or unpushed tree only ever goes through the safe-kill path;
//! - a worktree another live session shares is only plain-killed (its files
//!   stay; a safe-remove would delete a tree in use);
//! - nothing here deletes a transcript, a branch or a journal row.

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::gc::tidy::{
    self as planner, TidyAction, TidyCandidate, TidyConfig, TidyContext, TidyReason, TidySession,
};
use crate::service::gc::{needs_safe_remove, GcExec};
use crate::service::orgs::OrgScope;
use crate::service::settings;
use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Mutex;

/// Default and bounds of a snooze, in days.
pub const SNOOZE_DEFAULT_DAYS: u32 = 7;
pub const SNOOZE_MAX_DAYS: u32 = 365;
/// Most items one `tidy_apply` takes.
pub const APPLY_MAX_ITEMS: usize = 200;

/// `work { action: tidy }`: the candidates and the policy they were planned
/// under (for the sheet's footer and the dry-run preview).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TidyReport {
    #[serde(default)]
    pub candidates: Vec<TidyCandidate>,
    #[serde(default)]
    pub auto_tidy: bool,
    #[serde(default)]
    pub auto_reasons: Vec<TidyReason>,
    #[serde(default)]
    pub done_days: i64,
    #[serde(default)]
    pub idle_hours: i64,
}

// One choice in the sheet (no doc comment: it would ride the tool schema).
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, rmcp::schemars::JsonSchema,
)]
#[schemars(crate = "rmcp::schemars")]
pub struct TidyApplyItem {
    pub session_id: i64,
    /// safe_kill|kill|archive|snooze|never
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub days: Option<u32>,
}

/// What happened to one item. A failure never aborts the batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TidyApplyResult {
    pub session_id: i64,
    pub action: String,
    pub ok: bool,
    /// archived | killed | safe_kill_requested | snoozed | never
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TidyApplyReport {
    #[serde(default)]
    pub results: Vec<TidyApplyResult>,
}

/// The `work.*` tidy settings and `sessions.lost_ttl_secs`.
pub fn tidy_config(s: &Store) -> TidyConfig {
    let int = |key: &str, fallback: i64| {
        settings::get_string(s, key)
            .parse::<i64>()
            .unwrap_or(fallback)
    };
    let reasons = settings::get_string(s, settings::WORK_AUTO_TIDY_REASONS);
    TidyConfig {
        done_secs: int(settings::WORK_TIDY_DONE_DAYS, 2) * 86_400,
        idle_secs: int(settings::WORK_TIDY_IDLE_HOURS, 4) * 3600,
        lost_ttl_secs: int(settings::SESSIONS_LOST_TTL_SECS, 14 * 86_400),
        auto: settings::get_bool(s, settings::WORK_AUTO_TIDY),
        auto_reasons: reasons.split(',').filter_map(TidyReason::parse).collect(),
        org_auto: s.org_auto_tidy_overrides().unwrap_or_default(),
    }
}

/// Everything one planning pass reads, taken under one brief lock.
struct Snapshot {
    sessions: Vec<TidySession>,
    controller: Option<(String, String)>,
    operator: Option<(String, String)>,
    reachable: HashSet<String>,
    cfg: TidyConfig,
}

impl Snapshot {
    fn take(store: &Mutex<Store>) -> Result<Snapshot, IpcError> {
        let s = lock(store)?;
        Ok(Snapshot {
            sessions: s.tidy_sessions()?,
            controller: s.get_controller().ok().flatten(),
            operator: crate::service::operator::operator_ref(&s)
                .map(|r| (r.host_alias, r.tmux_name)),
            reachable: s
                .list_hosts()
                .unwrap_or_default()
                .into_iter()
                .filter(|h| h.reachable)
                .map(|h| h.alias)
                .collect(),
            cfg: tidy_config(&s),
        })
    }

    fn ctx(&self, now: i64) -> TidyContext<'_> {
        TidyContext {
            controller: self.controller.as_ref(),
            operator: self.operator.as_ref(),
            reachable: &self.reachable,
            now,
        }
    }

    fn plan(&self, now: i64) -> Vec<TidyCandidate> {
        planner::plan_tidy(&self.sessions, &self.cfg, &self.ctx(now))
    }

    /// Another live work session shares this one's worktree.
    fn shares_worktree(&self, s: &TidySession) -> bool {
        let r = &s.row;
        r.kind == "work"
            && r.worktree_key.is_some()
            && self.sessions.iter().any(|o| {
                o.row.id != r.id
                    && o.row.status == "running"
                    && o.row.kind == "work"
                    && o.row.host_alias == r.host_alias
                    && o.row.project_id == r.project_id
                    && o.row.worktree_key == r.worktree_key
            })
    }
}

/// A per-host token's view of a session: its own host only, and only a
/// session of an org it sees (work graph M5). Everyone else sees all.
fn in_scope(scope: &OrgScope, host_alias: &str, org: Option<i64>) -> bool {
    scope.host().is_none_or(|h| h == host_alias) && scope.sees_org(org)
}

/// A per-host token flags only the primary link, and only one of an org it
/// sees; another org's link reads as one that does not exist.
fn link_visible(scope: &OrgScope, s: &TidySession, link_id: Option<i64>) -> bool {
    s.link
        .as_ref()
        .is_some_and(|l| link_id.is_none_or(|id| id == l.link_id) && scope.sees_org(l.org_id))
}

/// `work { action: tidy }`. A per-host token sees only its own host's
/// candidates, and never another org's.
pub fn work_tidy(store: &Mutex<Store>, scope: &OrgScope, now: i64) -> Result<TidyReport, IpcError> {
    let snap = Snapshot::take(store)?;
    let mut candidates = snap.plan(now);
    candidates.retain(|c| in_scope(scope, &c.host_alias, c.org_id));
    // A session of the caller's org can carry another org's ticket (a
    // forced cross-org link): the session is the caller's, the ticket not.
    for c in &mut candidates {
        if !scope.sees_org(c.link_org_id) {
            c.redact_link();
        }
    }
    Ok(TidyReport {
        candidates,
        auto_tidy: snap.cfg.auto,
        auto_reasons: snap.cfg.auto_reasons.clone(),
        done_days: snap.cfg.done_secs / 86_400,
        idle_hours: snap.cfg.idle_secs / 3600,
    })
}

/// `work { action: reopened }`. A per-host token reads only work whose
/// newest past session ran on its host and whose item its org sees.
pub fn reopened(
    store: &Mutex<Store>,
    scope: &OrgScope,
) -> Result<Vec<crate::store::ReopenedWork>, IpcError> {
    let mut rows = lock(store)?.reopened_work()?;
    if let Some(h) = scope.host() {
        rows.retain(|r| r.last_host.as_deref() == Some(h) && scope.sees_org(r.org_id));
    }
    Ok(rows)
}

/// Record a tidy action on the session's timeline (`gc_tidied`) and in its
/// conversation's journal (`tidy`). Best-effort, before the action: a plain
/// kill reaps the row moments later.
fn record(store: &Mutex<Store>, s: &TidySession, detail: &str) {
    if let Ok(st) = store.lock() {
        if let Err(e) = st.insert_session_event(s.row.id, "gc_tidied", Some(detail)) {
            tracing::warn!(session_id = s.row.id, error = %e, "[tidy] session_event insert failed");
        }
        if let Some(c) = s.row.claude_session_id.as_deref() {
            let _ = st.append_journal(
                Some(c),
                None,
                "tidy",
                "fleet",
                Some(&format!("tidy: {detail}")),
                None,
            );
        }
    }
}

/// Kill a session the tidy way. See the module doc for the rules.
async fn tidy_kill(
    store: &Mutex<Store>,
    exec: &dyn GcExec,
    snap: &Snapshot,
    s: &TidySession,
    label: &str,
) -> Result<&'static str, IpcError> {
    let r = &s.row;
    if r.status != "running" {
        return Err(IpcError::new(
            codes::E_INVALID,
            "the session is not running (nothing to kill)",
        ));
    }
    let plain = matches!(r.kind.as_str(), "bg" | "shell" | "review") || snap.shares_worktree(s);
    let via_claude = if plain {
        false
    } else if r.kind == "work" && r.worktree_id.is_some() {
        match exec.inspect(&r.host_alias, &r.tmux_name).await {
            Ok(insp) => needs_safe_remove(&insp),
            Err(e) => {
                tracing::warn!(session = %r.tmux_name, error = %e, "[tidy] inspect failed; using safe-remove");
                true
            }
        }
    } else {
        return Err(IpcError::new(
            codes::E_INVALID,
            "no tracked worktree fleet can inspect: archive it instead",
        ));
    };
    let outcome = if via_claude {
        "safe_kill_requested"
    } else {
        "killed"
    };
    record(store, s, &format!("{label}:{outcome}"));
    if via_claude {
        exec.safe_kill(&r.host_alias, &r.tmux_name).await?;
    } else {
        exec.kill(&r.host_alias, &r.tmux_name).await?;
    }
    Ok(outcome)
}

/// The item's session under the caller's scope. Outside it (or unknown)
/// reads exactly as a session that does not exist (no existence oracle, as
/// M5's fence) — and, the id being caller-supplied, nothing is ever written
/// to it: [`tidy_apply`] records a failure only once this has succeeded.
fn resolve<'a>(
    snap: &'a Snapshot,
    scope: &OrgScope,
    session_id: i64,
) -> Result<&'a TidySession, IpcError> {
    snap.sessions
        .iter()
        .find(|s| s.row.id == session_id)
        .filter(|s| in_scope(scope, &s.row.host_alias, s.row.org_id))
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("session {session_id} not found")))
}

/// Apply one item to a session [`resolve`] found in the caller's scope,
/// against a fresh snapshot.
async fn apply_one(
    store: &Mutex<Store>,
    exec: &dyn GcExec,
    snap: &Snapshot,
    s: &TidySession,
    item: &TidyApplyItem,
    scope: &OrgScope,
    source: &str,
    now: i64,
) -> Result<&'static str, IpcError> {
    let action = item.action.as_str();
    let destructive = matches!(action, "safe_kill" | "kill" | "archive");
    if destructive {
        if let Some(why) = planner::protection(s, &snap.ctx(now)) {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("protected ({why}): tidy-up never acts on it"),
            ));
        }
    }
    match action {
        "safe_kill" | "kill" => {
            tidy_kill(store, exec, snap, s, &format!("{source}:{action}")).await
        }
        "archive" => {
            record(store, s, &format!("{source}:archive:archived"));
            lock(store)?.archive_session_work(s.row.id)?;
            Ok("archived")
        }
        "snooze" | "never" if !scope.is_all() && !link_visible(scope, s, item.link_id) => {
            Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("session {} has no such live work link", s.row.id),
            ))
        }
        "snooze" => {
            let days = item.days.unwrap_or(SNOOZE_DEFAULT_DAYS);
            if !(1..=SNOOZE_MAX_DAYS).contains(&days) {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!("snooze days must be 1..={SNOOZE_MAX_DAYS}"),
                ));
            }
            lock(store)?.snooze_tidy(s.row.id, item.link_id, now + i64::from(days) * 86_400)?;
            Ok("snoozed")
        }
        "never" => {
            lock(store)?.never_tidy(s.row.id, item.link_id)?;
            Ok("never")
        }
        other => Err(IpcError::new(
            codes::E_INVALID,
            format!(
                "unknown tidy action {other:?}; one of safe_kill, kill, archive, snooze, never"
            ),
        )),
    }
}

/// `work_link { action: tidy_apply, items }`: a batch. Every item is
/// reported; one failing never stops the rest.
pub async fn tidy_apply(
    store: &Mutex<Store>,
    exec: &dyn GcExec,
    items: &[TidyApplyItem],
    scope: &OrgScope,
    now: i64,
) -> Result<TidyApplyReport, IpcError> {
    if items.is_empty() || items.len() > APPLY_MAX_ITEMS {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("tidy_apply takes 1..={APPLY_MAX_ITEMS} items"),
        ));
    }
    let snap = Snapshot::take(store)?;
    let mut report = TidyApplyReport::default();
    for item in items {
        let result = match resolve(&snap, scope, item.session_id) {
            // Not visible: refused in the report only. An id outside the
            // caller's scope never gets a timeline row (a per-host token
            // could otherwise flood, and so evict, any session's history).
            Err(e) => Err(e),
            Ok(s) => {
                let result = apply_one(store, exec, &snap, s, item, scope, "manual", now).await;
                if let Err(e) = &result {
                    tracing::info!(session_id = s.row.id, action = %item.action, error = %e.message, "[tidy] item failed");
                    if let Ok(st) = store.lock() {
                        let _ = st.insert_session_event(s.row.id, "gc_failed", Some(&e.message));
                    }
                }
                result
            }
        };
        report.results.push(TidyApplyResult {
            session_id: item.session_id,
            action: item.action.clone(),
            ok: result.is_ok(),
            outcome: result.as_ref().ok().map(|o| o.to_string()),
            error: result.err().map(|e| e.message),
        });
    }
    Ok(report)
}

/// The GC sweep's auto-tidy pass: with `work.auto_tidy` on, act on the
/// candidates [`planner::auto_selection`] picks (safe kill / archive of the
/// allowed reasons), skipping sessions the idle killer acted on this pass.
/// Off (the default), it reads the settings and returns. Returns how many
/// were acted on.
pub async fn auto_tidy(
    store: &Mutex<Store>,
    exec: &dyn GcExec,
    skip: &HashSet<i64>,
    now: i64,
) -> usize {
    // On globally, or for some org (`orgs.auto_tidy`); the planner decides
    // per session which applies.
    let auto_on = match store.lock() {
        Ok(s) => tidy_config(&s).auto_anywhere(),
        Err(_) => false,
    };
    if !auto_on {
        return 0;
    }
    let snap = match Snapshot::take(store) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e.message, "[tidy] auto-tidy could not read the fleet");
            return 0;
        }
    };
    let plan = snap.plan(now);
    let mut acted = 0;
    for c in planner::auto_selection(&plan) {
        if skip.contains(&c.session_id) {
            continue;
        }
        let item = TidyApplyItem {
            session_id: c.session_id,
            action: match c.action {
                TidyAction::Archive => "archive",
                _ => "safe_kill",
            }
            .into(),
            link_id: c.link_id,
            days: None,
        };
        let source = format!("auto:{}", c.reason.as_str());
        let Ok(s) = resolve(&snap, &OrgScope::All, c.session_id) else {
            continue;
        };
        match apply_one(store, exec, &snap, s, &item, &OrgScope::All, &source, now).await {
            Ok(_) => acted += 1,
            Err(e) => {
                tracing::warn!(session_id = c.session_id, error = %e.message, "[tidy] auto-tidy failed");
                if let Ok(s) = store.lock() {
                    let _ = s.insert_session_event(c.session_id, "gc_failed", Some(&e.message));
                }
            }
        }
    }
    acted
}

#[cfg(test)]
mod tests;
