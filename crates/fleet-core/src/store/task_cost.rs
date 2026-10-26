//! What a lane's tasks cost when they share one lead session (mission
//! improvements M1, design 2026-10-26 §2, migration 213). A session's spend
//! is one number; a lead works on several tasks of several items, so each
//! task records the session's spend when it was handed over
//! (`tasks.cost_start_micros`) and its cost is the difference to the next
//! one's. Tasks in a session of their own leave the column NULL and keep
//! the whole session's spend, as before.

use super::Store;
use crate::ipc_error::IpcError;
use std::collections::HashMap;

/// `cost_start_micros` of a lane task from before migration 213.
pub const COST_START_UNKNOWN: i64 = -1;

/// One task of a shared session, in the order it was handed over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SharedTask {
    pub item_id: i64,
    /// The session's spend when the task began; [`COST_START_UNKNOWN`].
    pub cost_start: i64,
}

/// PURE: a shared session's spend split among its tasks, per item. A task
/// costs from its start to the next task's (the last one's to `total`), so
/// spend a usage read brings in late lands on the task it was made in, and
/// the parts add up to `total`: what came before the first task (the lead's
/// brief) is the first task's. A session with a task of unknown start is
/// split evenly. Never negative: a start past the next one (a usage read
/// that went down) counts as no spend.
pub fn split_session_spend(tasks: &[SharedTask], total: i64) -> HashMap<i64, i64> {
    let mut out: HashMap<i64, i64> = HashMap::new();
    let total = total.max(0);
    if tasks.is_empty() {
        return out;
    }
    if tasks.iter().any(|t| t.cost_start <= COST_START_UNKNOWN) {
        let n = tasks.len() as i64;
        for (i, t) in tasks.iter().enumerate() {
            let share = total / n + i64::from(i as i64 == n - 1) * (total % n);
            *out.entry(t.item_id).or_default() += share;
        }
        return out;
    }
    for (i, t) in tasks.iter().enumerate() {
        let start = if i == 0 { 0 } else { t.cost_start.min(total) };
        let end = tasks.get(i + 1).map_or(total, |n| n.cost_start.min(total));
        *out.entry(t.item_id).or_default() += (end - start).max(0);
    }
    out
}

impl Store {
    /// Record what `task_id`'s worker session had spent when it got the
    /// task: done for a task handed to a session that already did others
    /// (a lane's lead).
    pub fn set_task_cost_start(&self, task_id: i64) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE tasks SET cost_start_micros = COALESCE( \
                 (SELECT s.usage_cost_micros FROM sessions s \
                   WHERE s.id = tasks.worker_session_id), 0) \
             WHERE id = ?1",
            [task_id],
        )?;
        Ok(())
    }

    /// Every shared session's spend by item ([`split_session_spend`]):
    /// session id → item id → micro-USD. A session absent from it keeps its
    /// whole spend on each task it worked on.
    pub fn shared_session_spend(&self) -> Result<HashMap<i64, HashMap<i64, i64>>, IpcError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT t.worker_session_id, t.work_item_id, t.cost_start_micros, \
                    s.usage_cost_micros \
               FROM tasks t JOIN sessions s ON s.id = t.worker_session_id \
              WHERE t.cost_start_micros IS NOT NULL AND t.work_item_id IS NOT NULL \
              ORDER BY t.worker_session_id, t.id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                SharedTask {
                    item_id: r.get(1)?,
                    cost_start: r.get(2)?,
                },
                r.get::<_, i64>(3)?,
            ))
        })?;
        let mut by_session: Vec<(i64, i64, Vec<SharedTask>)> = Vec::new();
        for row in rows {
            let (sid, task, total) = row?;
            match by_session.last_mut() {
                Some((s, _, tasks)) if *s == sid => tasks.push(task),
                _ => by_session.push((sid, total, vec![task])),
            }
        }
        Ok(by_session
            .into_iter()
            .map(|(sid, total, tasks)| (sid, split_session_spend(&tasks, total)))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(item_id: i64, cost_start: i64) -> SharedTask {
        SharedTask {
            item_id,
            cost_start,
        }
    }

    #[test]
    fn each_task_costs_until_the_next_one_began() {
        // A lead's spend went 10 → 30 → 45: the brief and the first task
        // are the first task's 30, the second task's the rest.
        let m = split_session_spend(&[t(1, 10), t(2, 30)], 45);
        assert_eq!(m, HashMap::from([(1, 30), (2, 15)]));
        assert_eq!(
            m.values().sum::<i64>(),
            45,
            "the parts add up to the session"
        );
        // Two tries at one item and another item between them.
        let m = split_session_spend(&[t(1, 0), t(2, 5), t(1, 9)], 20);
        assert_eq!(m, HashMap::from([(1, 16), (2, 4)]));
    }

    #[test]
    fn an_unknown_start_splits_the_session_evenly() {
        let m = split_session_spend(&[t(1, -1), t(2, -1), t(3, -1)], 10);
        assert_eq!(m, HashMap::from([(1, 3), (2, 3), (3, 4)]));
        let m = split_session_spend(&[t(1, -1), t(2, 4)], 9);
        assert_eq!(m.values().sum::<i64>(), 9);
    }

    #[test]
    fn a_spend_that_went_down_is_no_spend() {
        let m = split_session_spend(&[t(1, 0), t(2, 50)], 30);
        assert_eq!(m, HashMap::from([(1, 30), (2, 0)]));
        assert!(split_session_spend(&[], 5).is_empty());
    }

    #[test]
    fn a_lead_session_splits_its_spend_by_the_tasks_it_got() {
        let s = Store::open_in_memory().unwrap();
        let a = s.create_local_work_item(None, "a").unwrap().id;
        let b = s.create_local_work_item(None, "b").unwrap().id;
        s.upsert_host("h").unwrap();
        let lead = s
            .upsert_session("lead", "h", None, None, 1, 1, "running", None)
            .unwrap();
        let own = s
            .upsert_session("own", "h", None, None, 1, 1, "running", None)
            .unwrap();
        let spend = |sid: i64, micros: i64| {
            s.conn
                .execute(
                    "UPDATE sessions SET usage_cost_micros = ?1 WHERE id = ?2",
                    [micros, sid],
                )
                .unwrap();
        };
        let task = |sid: i64, item: i64| {
            let t = s.insert_task(None, Some(sid), "p", "n").unwrap();
            s.set_task_run(t.id, item, 1, "implement").unwrap();
            t.id
        };
        spend(lead, 10);
        let t1 = task(lead, a);
        s.set_task_cost_start(t1).unwrap();
        spend(lead, 30);
        let t2 = task(lead, b);
        s.set_task_cost_start(t2).unwrap();
        spend(lead, 45);
        // A run in a session of its own records no start.
        spend(own, 7);
        task(own, a);
        let m = s.shared_session_spend().unwrap();
        assert_eq!(m.len(), 1, "only the lead is shared: {m:?}");
        assert_eq!(m[&lead], HashMap::from([(a, 30), (b, 15)]));
    }
}
