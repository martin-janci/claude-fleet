//! A mission's run metrics (lanes design 2026-10-23, L0): where its time and
//! its runs went, read from its members' `tasks` rows, so a change to how
//! missions run (lanes, headless checks, a planner that reads the repo) has
//! a baseline to be measured against.
//!
//! [`stats`] is pure over the attempts. Nothing here is stored: every read
//! derives it again, as the graph's states are.

use crate::store::{TaskRow, WorkItemRow};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// What a mission's runs came to.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionStats {
    /// Member items, the root left out unless it is the only one.
    pub items: u32,
    pub items_done: u32,
    /// Every attempt by role (`implement`, `review`, `test`, `integrate`,
    /// `research`); an attempt with no role is an `implement`.
    pub runs_by_role: BTreeMap<String, u32>,
    /// Attempts that ran in a session of their own; the rest ran headless
    /// (`orchestrator.headless_checks`).
    pub session_runs: u32,
    /// Distinct worker sessions and checkouts: each one a cold start (a
    /// fresh context, and in a new checkout a fresh build).
    pub sessions: u32,
    pub checkouts: u32,
    /// Attempts that failed, were cancelled or reported less than `done`.
    pub failed_runs: u32,
    /// Implement attempts past the first, over every member.
    pub retries: u32,
    /// Attempts per done item, in hundredths (`250` = 2.5); `None` before
    /// the first item is done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runs_per_done_item_x100: Option<u32>,
    /// A finished run's wall clock, median and 90th percentile, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_secs_p50: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_secs_p90: Option<i64>,
    /// A done item's first run to its last run's end, median and p90.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_secs_p50: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_secs_p90: Option<i64>,
    /// From the first run to the last run's end (or `now` while one is
    /// open): the mission's working span, in seconds.
    pub span_secs: i64,
    /// The part of the span no run was open: time the mission waited on a
    /// person, a slot, a usage limit or the loop's tick.
    pub idle_secs: i64,
}

/// Whether an attempt ended without the work (the loop's own reading,
/// `orchestrate::steps::attempt_failed`).
fn failed(t: &TaskRow) -> bool {
    crate::service::work::orchestrate::steps::attempt_failed(t)
}

fn role(t: &TaskRow) -> &str {
    t.role.as_deref().unwrap_or("implement")
}

/// The `q`-quantile (0–100) of `v` by nearest rank; `None` when empty.
fn quantile(v: &mut [i64], q: usize) -> Option<i64> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    let rank = (q * v.len()).div_ceil(100).max(1);
    Some(v[rank - 1])
}

/// When an attempt began and ended: its start (else its creation) and its
/// finish, `now` while it is open, `None` for one that ended unrecorded.
fn interval(t: &TaskRow, now: i64) -> Option<(i64, i64)> {
    let from = t.started_at.unwrap_or(t.created_at);
    let to = match t.finished_at {
        Some(f) => f,
        None if matches!(t.state.as_str(), "queued" | "running") => now,
        None => return None,
    };
    (to >= from).then_some((from, to))
}

/// PURE: the metrics of a mission whose members are `items`, `root` its
/// root item, `attempts` every member's attempts (any order), `checkout_of`
/// the worktree a worker session ran in.
pub fn stats(
    items: &[WorkItemRow],
    root: Option<i64>,
    attempts: &[TaskRow],
    checkout_of: &dyn Fn(i64) -> Option<i64>,
    now: i64,
) -> MissionStats {
    let sole_root = items.len() == 1 && root.is_some();
    let members: Vec<&WorkItemRow> = items
        .iter()
        .filter(|i| Some(i.id) != root || sole_root)
        .collect();
    let done: BTreeSet<i64> = members
        .iter()
        .filter(|i| i.status_category == "done")
        .map(|i| i.id)
        .collect();
    let mut out = MissionStats {
        items: members.len() as u32,
        items_done: done.len() as u32,
        ..Default::default()
    };
    let mut sessions = BTreeSet::new();
    let mut checkouts = BTreeSet::new();
    let mut impls: BTreeMap<i64, u32> = BTreeMap::new();
    let mut on_done = 0u32;
    let mut run_secs = Vec::new();
    let mut by_item: BTreeMap<i64, (i64, i64)> = BTreeMap::new();
    let mut spans: Vec<(i64, i64)> = Vec::new();
    for t in attempts {
        *out.runs_by_role.entry(role(t).to_string()).or_default() += 1;
        if let Some(w) = t.worker_session_id {
            out.session_runs += 1;
            sessions.insert(w);
            if let Some(c) = checkout_of(w) {
                checkouts.insert(c);
            }
        }
        if failed(t) {
            out.failed_runs += 1;
        }
        let item = t.work_item_id.unwrap_or(0);
        if role(t) == "implement" {
            *impls.entry(item).or_default() += 1;
        }
        if done.contains(&item) {
            on_done += 1;
        }
        if let Some((from, to)) = interval(t, now) {
            spans.push((from, to));
            if t.finished_at.is_some() {
                run_secs.push(to - from);
            }
            if done.contains(&item) {
                let e = by_item.entry(item).or_insert((from, to));
                e.0 = e.0.min(from);
                e.1 = e.1.max(to);
            }
        }
    }
    out.sessions = sessions.len() as u32;
    out.checkouts = checkouts.len() as u32;
    out.retries = impls.values().map(|n| n.saturating_sub(1)).sum();
    out.runs_per_done_item_x100 = (!done.is_empty()).then(|| on_done * 100 / done.len() as u32);
    out.run_secs_p50 = quantile(&mut run_secs, 50);
    out.run_secs_p90 = quantile(&mut run_secs, 90);
    let mut item_secs: Vec<i64> = by_item.values().map(|(a, b)| b - a).collect();
    out.item_secs_p50 = quantile(&mut item_secs, 50);
    out.item_secs_p90 = quantile(&mut item_secs, 90);
    // The span and the time no run was open: the union of the runs'
    // intervals against the span from the first start to the last end.
    spans.sort_unstable();
    if let (Some(first), Some(last)) = (spans.first(), spans.iter().map(|s| s.1).max()) {
        out.span_secs = last - first.0;
        let mut busy = 0;
        let (mut from, mut to) = *first;
        for &(a, b) in &spans[1..] {
            if a > to {
                busy += to - from;
                (from, to) = (a, b);
            } else {
                to = to.max(b);
            }
        }
        busy += to - from;
        out.idle_secs = out.span_secs - busy;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: i64, done: bool) -> WorkItemRow {
        serde_json::from_value(serde_json::json!({
            "id": id, "source": "local", "title": format!("t{id}"),
            "status_category": if done { "done" } else { "todo" },
            "created_at": 0, "updated_at": 0
        }))
        .unwrap()
    }

    fn task(
        id: i64,
        item: i64,
        role: &str,
        worker: Option<i64>,
        span: (i64, Option<i64>),
        state: &str,
    ) -> TaskRow {
        let mut t: TaskRow = serde_json::from_value(serde_json::json!({
            "id": id, "work_item_id": item, "role": role,
            "worker_session_id": worker, "state": state,
            "created_at": span.0, "started_at": span.0, "finished_at": span.1
        }))
        .unwrap();
        // A run that finished well reported so; with no report it is a
        // failed attempt (`attempt_failed`).
        if state == "done" {
            t.report = Some(crate::store::TaskReport {
                outcome: "done".into(),
                ..Default::default()
            });
        }
        t
    }

    /// Two items: one implemented twice, reviewed in a session and tested
    /// headless; one running. A gap with nothing open is idle time.
    #[test]
    fn runs_retries_cold_starts_and_idle_time() {
        let items = [item(1, false), item(2, true), item(3, false)];
        let attempts = [
            task(10, 2, "implement", Some(100), (0, Some(100)), "failed"),
            task(11, 2, "implement", Some(101), (150, Some(250)), "done"),
            task(12, 2, "review", Some(102), (250, Some(300)), "done"),
            task(13, 2, "test", None, (300, Some(320)), "done"),
            task(14, 3, "implement", Some(103), (400, None), "running"),
        ];
        let checkout = |w: i64| Some(if w == 102 { 7 } else { w });
        let st = stats(&items, Some(1), &attempts, &checkout, 500);
        assert_eq!((st.items, st.items_done), (2, 1), "the root is no member");
        assert_eq!(st.runs_by_role.get("implement"), Some(&3));
        assert_eq!(st.runs_by_role.get("review"), Some(&1));
        assert_eq!(st.runs_by_role.get("test"), Some(&1));
        assert_eq!((st.session_runs, st.sessions, st.checkouts), (4, 4, 4));
        assert_eq!((st.failed_runs, st.retries), (1, 1));
        assert_eq!(st.runs_per_done_item_x100, Some(400));
        assert_eq!(st.run_secs_p50, Some(50), "{:?}", st);
        assert_eq!(st.item_secs_p50, Some(320));
        assert_eq!(st.span_secs, 500);
        // 0–100 and 150–320 and 400–500 open: 50 + 80 idle.
        assert_eq!(st.idle_secs, 130);
    }

    #[test]
    fn a_mission_without_runs_reads_empty() {
        let st = stats(&[item(1, false)], Some(1), &[], &|_| None, 10);
        assert_eq!(st.items, 1, "a sole root is the member");
        assert_eq!(st.span_secs, 0);
        assert_eq!(st.runs_per_done_item_x100, None);
        assert_eq!(st.run_secs_p90, None);
    }

    #[test]
    fn quantiles_take_the_nearest_rank() {
        assert_eq!(quantile(&mut [5, 1, 3], 50), Some(3));
        assert_eq!(quantile(&mut [1, 2, 3, 4, 5, 6, 7, 8, 9, 10], 90), Some(9));
        assert_eq!(quantile(&mut [4], 90), Some(4));
    }
}
