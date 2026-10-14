//! `work { action: stats, filters }`: the Work view's tasks counted, not
//! listed (persona dashboards, design 2026-10-11, step 3).
//!
//! A dashboard asks the same question the Work board answers ("which tasks,
//! grouped how") and wants numbers back: per group, how many tasks sit in
//! each stage, how many are done, what they cost. The answer is computed
//! from [`view::matching_tasks`], the board's own filtering under the
//! caller's [`ViewScope`], so a count never includes a task the caller
//! could not open and "6 blocked" here is the board's Blocked column.
//!
//! Grouped by sprint or release, each group also carries its bucket's
//! health: its dates, the share of its time gone, and how many items joined
//! or left it after it started (scope change).
//!
//! With `since`, the answer also carries a burn-up (persona dashboards, step
//! 8): for each day from `since`'s to today, how many of these tasks there were
//! and how many were done at the day's end, read from `work_status_log`
//! (migration 170). History starts the day that migration ran: a task with
//! no history row (older, or a bare tracker key) counts for the whole window
//! in the stage it has now.

use super::buckets;
use super::view::{self, GroupRef, WorkTask, WorkTreeFilters};
use crate::ipc_error::IpcError;
use crate::service::view_scope::ViewScope;
use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Mutex;

/// At most this many groups come back; the rest are counted in `total`
/// and named by `groups_more`.
pub const STATS_MAX_GROUPS: usize = 200;
/// The longest burn-up, in days.
pub const BURNUP_MAX_DAYS: u32 = 90;
const DAY: i64 = 86_400;

/// Tasks per stage (`WorkTask::stage`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageCounts {
    pub backlog: u32,
    pub in_progress: u32,
    pub in_review: u32,
    pub blocked: u32,
    pub done: u32,
}

impl StageCounts {
    fn add(&mut self, stage: &str) {
        match stage {
            "done" => self.done += 1,
            "blocked" => self.blocked += 1,
            "in_review" => self.in_review += 1,
            "in_progress" => self.in_progress += 1,
            _ => self.backlog += 1,
        }
    }
}

/// A sprint's or release's health, on a group of that bucket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BucketHealth {
    pub bucket_id: i64,
    /// sprint | release
    pub kind: String,
    /// sprint: planned | active | closed; release: planned | released.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starts_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ends_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    /// How much of `starts_at..ends_at` has gone, 0–100; absent without
    /// both dates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pct_time_elapsed: Option<u32>,
    /// Items (the caller sees) that joined after `starts_at`.
    pub added_after_start: u32,
    /// Items (the caller sees) that left after `starts_at`.
    pub removed_after_start: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatsGroup {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
    pub group: GroupRef,
    pub total: u32,
    pub stages: StageCounts,
    /// done / total, 0–100.
    pub pct_done: u32,
    /// Tasks that need a person now.
    pub needs_you: u32,
    pub cost_micros: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bucket: Option<BucketHealth>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkStats {
    /// The grouping the groups follow (`filters.group_by`, else `group`).
    pub group_by: String,
    pub total: u32,
    pub stages: StageCounts,
    pub pct_done: u32,
    pub needs_you: u32,
    pub cost_micros: i64,
    /// Largest first, then by label; at most [`STATS_MAX_GROUPS`].
    pub groups: Vec<StatsGroup>,
    /// Groups left out past the cap.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub groups_more: u32,
    pub generated_at: i64,
    /// With `since`: one point a day, oldest first, today last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub burnup: Option<Vec<BurnupDay>>,
}

/// One day of a burn-up: the tasks there were and the ones done by its end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BurnupDay {
    /// The day's start, unix seconds (UTC midnight).
    pub day: i64,
    pub total: u32,
    pub done: u32,
}

/// PURE: the burn-up of `tasks` over the `days` days ending today. `logs`
/// is each item's `(at, category)` history, oldest first.
pub fn burnup_of(
    tasks: &[WorkTask],
    logs: &std::collections::HashMap<i64, Vec<(i64, String)>>,
    days: u32,
    now: i64,
) -> Vec<BurnupDay> {
    let today = now.div_euclid(DAY) * DAY;
    let days = days.clamp(1, BURNUP_MAX_DAYS) as i64;
    (0..days)
        .map(|k| {
            let day = today - (days - 1 - k) * DAY;
            // The day's end; today's is now.
            let end = (day + DAY).min(now + 1);
            let (mut total, mut done) = (0u32, 0u32);
            for t in tasks {
                match t
                    .item_id
                    .and_then(|id| logs.get(&id))
                    .filter(|h| !h.is_empty())
                {
                    Some(h) => {
                        let Some((_, cat)) = h.iter().take_while(|(at, _)| *at < end).last() else {
                            continue; // not there yet
                        };
                        total += 1;
                        done += u32::from(cat == "done");
                    }
                    None => {
                        total += 1;
                        done += u32::from(t.stage == "done");
                    }
                }
            }
            BurnupDay { day, total, done }
        })
        .collect()
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

fn pct(part: u32, whole: u32) -> u32 {
    if whole == 0 {
        0
    } else {
        (u64::from(part) * 100 / u64::from(whole)) as u32
    }
}

/// PURE: the tasks counted, grouped by their `group` (and org, as the
/// tree's sections are).
pub fn stats_of(tasks: &[WorkTask], group_by: &str, now: i64) -> WorkStats {
    let mut groups: BTreeMap<(Option<i64>, String), StatsGroup> = BTreeMap::new();
    let mut stages = StageCounts::default();
    let (mut needs_you, mut cost_micros) = (0u32, 0i64);
    for t in tasks {
        stages.add(&t.stage);
        needs_you += u32::from(t.needs_you);
        cost_micros += t.cost_micros;
        let g = groups
            .entry((t.org_id, t.group.id.clone()))
            .or_insert_with(|| StatsGroup {
                org_id: t.org_id,
                group: t.group.clone(),
                total: 0,
                stages: StageCounts::default(),
                pct_done: 0,
                needs_you: 0,
                cost_micros: 0,
                bucket: None,
            });
        g.total += 1;
        g.stages.add(&t.stage);
        g.needs_you += u32::from(t.needs_you);
        g.cost_micros += t.cost_micros;
    }
    let mut groups: Vec<StatsGroup> = groups
        .into_values()
        .map(|mut g| {
            g.pct_done = pct(g.stages.done, g.total);
            g
        })
        .collect();
    groups.sort_by(|a, b| {
        b.total
            .cmp(&a.total)
            .then_with(|| (a.group.id == "none").cmp(&(b.group.id == "none")))
            .then_with(|| a.group.label.cmp(&b.group.label))
    });
    let groups_more = groups.len().saturating_sub(STATS_MAX_GROUPS) as u32;
    groups.truncate(STATS_MAX_GROUPS);
    let total = tasks.len() as u32;
    WorkStats {
        group_by: group_by.to_string(),
        total,
        pct_done: pct(stages.done, total),
        stages,
        needs_you,
        cost_micros,
        groups,
        groups_more,
        generated_at: now,
        burnup: None,
    }
}

/// PURE: a bucket's health from its row and its members' join and leave
/// times, `(added_at, removed_at)` (only members the caller sees).
pub fn health_of(
    b: &crate::store::BucketRow,
    members: &[(i64, Option<i64>)],
    now: i64,
) -> BucketHealth {
    let pct_time_elapsed = match (b.starts_at, b.ends_at) {
        (Some(s), Some(e)) if e > s => {
            Some(((now.clamp(s, e) - s) * 100 / (e - s)).clamp(0, 100) as u32)
        }
        _ => None,
    };
    let after = |at: Option<i64>| matches!((b.starts_at, at), (Some(s), Some(a)) if a > s);
    BucketHealth {
        bucket_id: b.id,
        kind: b.kind.clone(),
        state: b.state.clone(),
        starts_at: b.starts_at,
        ends_at: b.ends_at,
        goal: b.goal.clone(),
        pct_time_elapsed,
        added_after_start: members.iter().filter(|m| after(Some(m.0))).count() as u32,
        removed_after_start: members.iter().filter(|m| after(m.1)).count() as u32,
    }
}

/// PURE: how many days a burn-up from `since` to `now` spans, both days
/// counted; `None` past [`BURNUP_MAX_DAYS`] or after now.
pub fn burnup_days(since: i64, now: i64) -> Option<u32> {
    let span = now.div_euclid(DAY) - since.div_euclid(DAY) + 1;
    (1..=i64::from(BURNUP_MAX_DAYS))
        .contains(&span)
        .then_some(span as u32)
}

/// `work { action: stats, filters?, since? }`.
pub fn stats(
    store: &Mutex<Store>,
    scope: &ViewScope,
    filters: Option<&WorkTreeFilters>,
    since: Option<i64>,
) -> Result<WorkStats, IpcError> {
    let filters = filters.cloned().unwrap_or_default();
    let (tasks, now) = view::matching_tasks(store, scope, &filters)?;
    let group_by = filters.group_by.clone().unwrap_or_else(|| "group".into());
    let mut out = stats_of(&tasks, &group_by, now);
    if let Some(since) = since {
        let days = burnup_days(since, now).ok_or_else(|| {
            IpcError::new(
                crate::ipc_error::codes::E_INVALID,
                format!(
                    "a burn-up starts at most {BURNUP_MAX_DAYS} days ago, and not later than now"
                ),
            )
        })?;
        let ids: Vec<i64> = tasks.iter().filter_map(|t| t.item_id).collect();
        let logs = crate::ipc_error::lock(store)?.status_logs(&ids)?;
        out.burnup = Some(burnup_of(&tasks, &logs, days, now));
    }
    if matches!(group_by.as_str(), "sprint" | "release") {
        for g in &mut out.groups {
            let Some(id) = g
                .group
                .id
                .split_once(':')
                .and_then(|(_, id)| id.parse::<i64>().ok())
            else {
                continue;
            };
            // The tree named this bucket to the caller, so it is in scope;
            // a bucket gone since answers as unknown and keeps no health.
            if let Ok(d) = buckets::bucket(store, scope, id) {
                let joins: Vec<(i64, Option<i64>)> = d
                    .members
                    .iter()
                    .map(|m| (m.added_at, m.removed_at))
                    .collect();
                g.bucket = Some(health_of(&d.bucket, &joins, now));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
