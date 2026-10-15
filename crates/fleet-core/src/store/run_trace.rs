//! Run traces (migration 194): one row per step of a run, so a run can be
//! drawn while it goes and read afterwards. What a step says is
//! `service::run_trace`'s; this file keeps the rows bounded.

use super::Store;
use crate::events::{EventBus as _, RowChange};
use crate::ipc_error::IpcError;
use serde::{Deserialize, Serialize};

/// The phases a run passes through, in order. A step names one.
pub const RUN_PHASES: [&str; 8] = [
    "trigger",
    "checks",
    "placement",
    "spawn",
    "prompt",
    "working",
    "settle",
    "outcome",
];
/// Steps kept per run (and attempt); later ones are dropped, the last kept
/// row saying so. Phase steps past it still land: a run's end always shows.
pub const RUN_TRACE_MAX_STEPS: i64 = 2000;
/// How long a trace is kept, from its row's `at`.
pub const RUN_TRACE_KEEP_SECS: i64 = 14 * 24 * 3600;
/// The largest `data` kept, in bytes; a larger one is replaced by a note.
pub const RUN_TRACE_DATA_MAX: usize = 4096;

/// One step, as a reader receives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunTraceRow {
    pub seq: i64,
    pub attempt: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_ref: Option<String>,
    /// Unix seconds.
    pub at: i64,
    /// One of [`RUN_PHASES`].
    pub phase: String,
    /// What kind of step: trigger | check | placement | spawn | prompt |
    /// tool_call | thinking | waiting | decision | settle | outcome | retry
    /// | error | note.
    pub kind: String,
    /// Who took it: scheduler | hub | claude | jev | rule | person.
    pub actor: String,
    /// One line for a person, shown as it is.
    pub summary: String,
    /// A JSON object with the step's facts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_micros: Option<i64>,
    /// The Jev decision (`decision_runs.id`) the step records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_id: Option<i64>,
}

/// A step to record. `seq` is the store's to give.
#[derive(Debug, Clone, Default)]
pub struct NewRunTrace<'a> {
    pub run_ref: &'a str,
    pub attempt: i64,
    pub parent_ref: Option<&'a str>,
    pub at: i64,
    pub phase: &'a str,
    pub kind: &'a str,
    pub actor: &'a str,
    pub summary: &'a str,
    pub data: Option<&'a serde_json::Value>,
    pub duration_ms: Option<i64>,
    pub cost_micros: Option<i64>,
    pub decision_id: Option<i64>,
}

const COLS: &str = "seq, attempt, parent_ref, at, phase, kind, actor, summary, data, \
                    duration_ms, cost_micros, decision_id";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RunTraceRow> {
    let data: Option<String> = r.get(8)?;
    Ok(RunTraceRow {
        seq: r.get(0)?,
        attempt: r.get(1)?,
        parent_ref: r.get(2)?,
        at: r.get(3)?,
        phase: r.get(4)?,
        kind: r.get(5)?,
        actor: r.get(6)?,
        summary: r.get(7)?,
        data: data.and_then(|d| serde_json::from_str(&d).ok()),
        duration_ms: r.get(9)?,
        cost_micros: r.get(10)?,
        decision_id: r.get(11)?,
    })
}

/// Whether `kind` is a step a run's cap may drop: the work in between, not
/// a phase boundary.
fn droppable(kind: &str) -> bool {
    matches!(kind, "tool_call" | "thinking" | "note")
}

impl Store {
    /// Record one step of run `t.run_ref` and tell clients (`run:changed`).
    /// Answers its `seq`, or `None` when the run's cap dropped it. The
    /// first step of a run also drops every row older than
    /// [`RUN_TRACE_KEEP_SECS`].
    pub fn insert_run_trace(&self, t: &NewRunTrace<'_>) -> Result<Option<i64>, IpcError> {
        if !RUN_PHASES.contains(&t.phase) {
            return Err(IpcError::new(
                crate::ipc_error::codes::E_INVALID,
                format!("run trace phase must be one of {}", RUN_PHASES.join(" | ")),
            ));
        }
        let last: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM run_trace WHERE run_ref = ?1",
            [t.run_ref],
            |r| r.get(0),
        )?;
        if last == 0 {
            self.conn.execute(
                "DELETE FROM run_trace WHERE at < ?1",
                [t.at - RUN_TRACE_KEEP_SECS],
            )?;
        }
        if droppable(t.kind) && last >= RUN_TRACE_MAX_STEPS {
            return Ok(None);
        }
        let data = t.data.map(|d| {
            let text = d.to_string();
            if text.len() <= RUN_TRACE_DATA_MAX {
                text
            } else {
                serde_json::json!({ "truncated": true, "bytes": text.len() }).to_string()
            }
        });
        let seq = last + 1;
        self.conn.execute(
            "INSERT INTO run_trace (run_ref, seq, attempt, parent_ref, at, phase, kind, actor, \
               summary, data, duration_ms, cost_micros, decision_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            rusqlite::params![
                t.run_ref,
                seq,
                t.attempt.max(1),
                t.parent_ref,
                t.at,
                t.phase,
                t.kind,
                t.actor,
                t.summary,
                data,
                t.duration_ms,
                t.cost_micros,
                t.decision_id
            ],
        )?;
        self.bus.emit(&RowChange::RunChanged(t.run_ref.to_string()));
        Ok(Some(seq))
    }

    /// Run `run_ref`'s steps after `after_seq`, oldest first, at most
    /// `limit`.
    pub fn run_trace(
        &self,
        run_ref: &str,
        after_seq: i64,
        limit: i64,
    ) -> Result<Vec<RunTraceRow>, IpcError> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {COLS} FROM run_trace WHERE run_ref = ?1 AND seq > ?2 \
             ORDER BY seq LIMIT ?3"
        ))?;
        let rows = stmt.query_map(rusqlite::params![run_ref, after_seq, limit], row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The newest phase each of `run_refs` reached: `(run_ref, phase)`.
    /// A run with no trace is absent.
    pub fn run_trace_phases(&self, run_refs: &[String]) -> Result<Vec<(String, String)>, IpcError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT phase FROM run_trace WHERE run_ref = ?1 ORDER BY seq DESC LIMIT 1",
        )?;
        let mut out = Vec::new();
        for r in run_refs {
            use rusqlite::OptionalExtension;
            if let Some(p) = stmt
                .query_row([r], |row| row.get::<_, String>(0))
                .optional()?
            {
                out.push((r.clone(), p));
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step<'a>(run: &'a str, phase: &'a str, kind: &'a str, at: i64) -> NewRunTrace<'a> {
        NewRunTrace {
            run_ref: run,
            attempt: 1,
            at,
            phase,
            kind,
            actor: "scheduler",
            summary: "a step",
            ..NewRunTrace::default()
        }
    }

    #[test]
    fn steps_count_from_one_per_run_and_read_back_in_order() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(
            s.insert_run_trace(&step("routine:1", "trigger", "trigger", 10))
                .unwrap(),
            Some(1)
        );
        assert_eq!(
            s.insert_run_trace(&step("routine:2", "trigger", "trigger", 10))
                .unwrap(),
            Some(1)
        );
        let data = serde_json::json!({ "host": "htz" });
        s.insert_run_trace(&NewRunTrace {
            data: Some(&data),
            duration_ms: Some(4200),
            ..step("routine:1", "spawn", "spawn", 11)
        })
        .unwrap();
        let rows = s.run_trace("routine:1", 0, 100).unwrap();
        assert_eq!(rows.iter().map(|r| r.seq).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(rows[1].data, Some(data));
        assert_eq!(rows[1].duration_ms, Some(4200));
        assert_eq!(s.run_trace("routine:1", 1, 100).unwrap().len(), 1);
        assert_eq!(
            s.run_trace_phases(&["routine:1".into(), "routine:9".into()])
                .unwrap(),
            vec![("routine:1".to_string(), "spawn".to_string())]
        );
    }

    #[test]
    fn an_unknown_phase_is_refused() {
        let s = Store::open_in_memory().unwrap();
        assert!(s
            .insert_run_trace(&step("routine:1", "lunch", "note", 1))
            .is_err());
    }

    #[test]
    fn large_data_is_replaced_by_a_note() {
        let s = Store::open_in_memory().unwrap();
        let big = serde_json::json!({ "out": "x".repeat(RUN_TRACE_DATA_MAX) });
        s.insert_run_trace(&NewRunTrace {
            data: Some(&big),
            ..step("agent:1", "working", "tool_call", 1)
        })
        .unwrap();
        let row = &s.run_trace("agent:1", 0, 10).unwrap()[0];
        assert_eq!(row.data.as_ref().unwrap()["truncated"], true);
    }

    #[test]
    fn the_cap_drops_work_steps_but_never_a_phase_boundary() {
        let s = Store::open_in_memory().unwrap();
        s.conn
            .execute(
                "INSERT INTO run_trace (run_ref, seq, at, phase, kind, actor, summary) \
                 VALUES ('routine:1', ?1, 1, 'working', 'tool_call', 'claude', 'x')",
                [RUN_TRACE_MAX_STEPS],
            )
            .unwrap();
        assert_eq!(
            s.insert_run_trace(&step("routine:1", "working", "tool_call", 2))
                .unwrap(),
            None
        );
        assert_eq!(
            s.insert_run_trace(&step("routine:1", "settle", "settle", 2))
                .unwrap(),
            Some(RUN_TRACE_MAX_STEPS + 1)
        );
    }

    #[test]
    fn a_new_run_drops_traces_past_the_keep_window() {
        let s = Store::open_in_memory().unwrap();
        s.insert_run_trace(&step("routine:1", "trigger", "trigger", 100))
            .unwrap();
        let later = 100 + RUN_TRACE_KEEP_SECS + 1;
        // Another step of the same run does not sweep.
        s.insert_run_trace(&step("routine:1", "checks", "check", later))
            .unwrap();
        assert_eq!(s.run_trace("routine:1", 0, 10).unwrap().len(), 2);
        // A new run's first step does.
        s.insert_run_trace(&step("routine:2", "trigger", "trigger", later))
            .unwrap();
        let left = s.run_trace("routine:1", 0, 10).unwrap();
        assert_eq!(left.iter().map(|r| r.seq).collect::<Vec<_>>(), vec![2]);
    }
}
