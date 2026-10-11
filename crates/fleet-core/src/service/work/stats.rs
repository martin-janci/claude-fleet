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

/// `work { action: stats, filters? }`.
pub fn stats(
    store: &Mutex<Store>,
    scope: &ViewScope,
    filters: Option<&WorkTreeFilters>,
) -> Result<WorkStats, IpcError> {
    let filters = filters.cloned().unwrap_or_default();
    let (tasks, now) = view::matching_tasks(store, scope, &filters)?;
    let group_by = filters.group_by.clone().unwrap_or_else(|| "group".into());
    let mut out = stats_of(&tasks, &group_by, now);
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
