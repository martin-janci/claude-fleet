//! Control sweeps what it handed out (review 2026-10-17, F15).
//!
//! Martin should not have to type "fix conflicts". This module adds lines
//! to the operator feed's `[fleet events]` digest (`service::operator_feed`)
//! for what Control handed out and went wrong on its own:
//!
//! * a PR of a session Control started or briefed that conflicts with its
//!   base (`mergeStateStatus` DIRTY) or has failing checks, read off the
//!   reconcile pass's own probe (`sessions.pr_evidence`) through the PR
//!   shepherd's [`pr_shepherd::condition_of`], so the two never disagree on
//!   what "wrong" means;
//! * a mission Control handed off that is active, waits on nobody, and has
//!   finished no task for [`STALL_SECS`].
//!
//! Only threads Control still follows are swept, by the operator feed's
//! own rule (`service::control_follow::follows`, F10).
//!
//! They are standing conditions, so the feed's `raised` set reports each
//! once and again only after it cleared. A PR's key carries its head commit:
//! a push that is still red is a new change and is reported again; a
//! mission's carries its done count, so progress followed by a new stall is
//! too.
//!
//! **Autonomy.** Each line starts with [`ACT`] or [`REPORT`]: at "Ask
//! first", and at Custom, which acts as it
//! (`autonomy::Position::is_ask_first`), Control only tells the person;
//! within its brief it fixes or reassigns, every step still behind
//! the usual confirmation (`mcp::guard::OPERATOR_CONFIRMS`). The rule the
//! agent follows is `service::control_rules::SECTION`.
//!
//! **Beside #933.** The self debug agent's proposer (open PR #933) proposes
//! fixes from logs for a person to approve; this sweep only tells Control,
//! about its own handoffs, within the brief it already has.
//!
//! Off with `control.sweep`. What a mission's progress was last seen at is
//! kept in one setting, [`SWEEP_STATE_KEY`]: no migration.

use crate::service::operator_feed::Condition;
use crate::service::pr_shepherd::{self, Condition as PrCondition};
use crate::service::settings;
use crate::store::{MissionRow, SessionRow, Store};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The mark of a line Control may act on within its brief.
pub const ACT: &str = "sweep: act";
/// The mark of a line Control only reports (Ask first).
pub const REPORT: &str = "sweep: report";
/// A handed-off mission that finished no task for this long has stalled.
pub const STALL_SECS: i64 = 2 * 3600;
/// Internal bookkeeping (no `SPECS` row): when each mission's done count was
/// last seen to change.
pub const SWEEP_STATE_KEY: &str = "control.sweep_state";

fn mark(ask_first: bool) -> &'static str {
    if ask_first {
        REPORT
    } else {
        ACT
    }
}

/// PURE: the sweep condition of one handed-off session's PR, `None` when
/// nothing is wrong (or it is behind only, which the next rebase fixes).
pub fn pr_condition(row: &SessionRow, ask_first: bool) -> Option<Condition> {
    let ev = row.pr_evidence.as_ref()?;
    let cond = pr_shepherd::condition_of(ev)?;
    let what = match cond {
        PrCondition::Conflict => "has merge conflicts with its base",
        PrCondition::CiRed => "has failing checks",
        PrCondition::Behind => return None,
    };
    let head = ev.head_oid.as_deref().unwrap_or("-");
    let pr = row.pr_url.as_deref().unwrap_or("its PR");
    Some(Condition {
        key: format!("sw:pr:{}:{head}:{}", row.id, cond.as_str()),
        text: format!(
            "{} — {pr} of session {} on {} (#{}) {what}",
            mark(ask_first),
            row.tmux_name,
            row.host_alias,
            row.id
        ),
    })
}

/// When each mission's done count last changed: `id -> (done, since)`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SweepState {
    #[serde(default)]
    pub missions: BTreeMap<i64, (i64, i64)>,
}

impl SweepState {
    fn load(s: &Store) -> Self {
        s.get_setting(SWEEP_STATE_KEY)
            .ok()
            .flatten()
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default()
    }

    fn save(&self, s: &Store) {
        let j = serde_json::to_string(self).unwrap_or_default();
        if let Err(e) = s.set_setting(SWEEP_STATE_KEY, &j) {
            tracing::debug!(error = %e, "[control-sweep] state not saved");
        }
    }
}

/// PURE: since when `m`'s done count has stood still, updating `state`.
pub fn progress_since(state: &mut SweepState, m: &MissionRow, now: i64) -> i64 {
    let e = state.missions.entry(m.id).or_insert((m.done, now));
    if e.0 != m.done {
        *e = (m.done, now);
    }
    e.1
}

/// PURE: the stall condition of a handed-off mission, given since when its
/// done count has stood still. Only an active mission that waits on nobody
/// (a wait is the feed's own line) and has work left.
pub fn mission_condition(
    m: &MissionRow,
    still_since: i64,
    now: i64,
    ask_first: bool,
) -> Option<Condition> {
    if m.state != "active" || m.waiting_on.is_some() {
        return None;
    }
    if m.total > 0 && m.done >= m.total {
        return None;
    }
    let idle = now - still_since;
    if idle < STALL_SECS {
        return None;
    }
    Some(Condition {
        key: format!("sw:m{}:stalled:{}", m.id, m.done),
        text: format!(
            "{} — mission #{} \"{}\" finished no task for {} h ({} of {} done)",
            mark(ask_first),
            m.id,
            m.name,
            idle / 3600,
            m.done,
            m.total
        ),
    })
}

/// PURE: whether the thread `kind`/`id` is swept: Control still follows it
/// (`control_follow::follows`, the operator feed's rule).
pub fn swept(follows: &[crate::service::control_follow::Follow], kind: &str, id: i64) -> bool {
    follows
        .iter()
        .any(|f| f.kind == kind && f.id == id && f.state.following())
}

/// Every sweep condition now, for the feed's standing conditions. Empty
/// with `control.sweep` off. `exclude` is the operator's own row.
pub fn conditions(s: &Store, exclude: Option<i64>, now: i64) -> Vec<Condition> {
    if !settings::get_bool(s, settings::CONTROL_SWEEP) {
        return Vec::new();
    }
    let ask = crate::service::autonomy::position(s).is_ask_first();
    // The threads the feed follows (F10): a session done, killed, merged,
    // quiet or muted, and a muted or finished mission, are not swept.
    let follows = crate::service::control_follow::follows(s, exclude, now);
    let followed = |kind: &str, id: i64| swept(&follows, kind, id);
    let mut out = Vec::new();
    for id in s.handed_off_session_ids(exclude).unwrap_or_default() {
        if !followed("session", id) {
            continue;
        }
        if let Ok(Some(row)) = s.get_session_by_id(id) {
            out.extend(pr_condition(&row, ask));
        }
    }
    let mut state = SweepState::load(s);
    let before = state.clone();
    let mut seen = Vec::new();
    for id in s.handed_off_mission_ids().unwrap_or_default() {
        if !followed("mission", id) {
            continue;
        }
        let Ok(Some(mut m)) = s.get_mission(id) else {
            continue;
        };
        if m.state != "active" {
            continue;
        }
        if crate::service::work::orchestrate::fill_spend(s, &mut m, now).is_err() {
            continue;
        }
        seen.push(m.id);
        let since = progress_since(&mut state, &m, now);
        out.extend(mission_condition(&m, since, now, ask));
    }
    state.missions.retain(|id, _| seen.contains(id));
    if state != before {
        state.save(s);
    }
    out
}

#[cfg(test)]
#[path = "control_sweep_tests.rs"]
mod tests;
