//! Dispatched tasks and their state transitions.

use super::*;

impl Store {
    /// Create a task in state `queued`. Returns the row. Emits `task_updated`.
    pub fn insert_task(
        &self,
        requester_session_id: Option<i64>,
        worker_session_id: Option<i64>,
        prompt: &str,
        nonce: &str,
    ) -> Result<TaskRow, crate::ipc_error::IpcError> {
        self.conn
            .execute(
                "INSERT INTO tasks (requester_session_id, worker_session_id, prompt, state, \
                                    created_at, nonce) \
                 VALUES (?1, ?2, ?3, 'queued', ?4, ?5)",
                rusqlite::params![
                    requester_session_id,
                    worker_session_id,
                    prompt,
                    now_unix(),
                    nonce
                ],
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        let id = self.conn.last_insert_rowid();
        let row = self
            .fetch_task(id)?
            .ok_or_else(|| crate::ipc_error::IpcError::new("E_DB", "task vanished after insert"))?;
        self.bus.task_updated(&row);
        Ok(row)
    }

    pub fn get_task(&self, id: i64) -> Result<Option<TaskRow>, crate::ipc_error::IpcError> {
        self.fetch_task(id)
    }

    fn fetch_task(&self, id: i64) -> Result<Option<TaskRow>, crate::ipc_error::IpcError> {
        self.conn
            .query_row(
                &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?1"),
                rusqlite::params![id],
                map_task_row,
            )
            .optional()
            .map_err(crate::ipc_error::IpcError::from)
    }

    /// Tasks newest-first, optionally narrowed by requester and/or state,
    /// capped at `limit`.
    ///
    /// `host` scopes the result for a per-host caller IN SQL: only tasks
    /// whose requester or worker session lives on that host.
    pub fn list_tasks(
        &self,
        requester_session_id: Option<i64>,
        state: Option<&str>,
        host: Option<&str>,
        limit: i64,
    ) -> Result<Vec<TaskRow>, crate::ipc_error::IpcError> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {cols} FROM tasks t \
                 LEFT JOIN sessions r ON r.id = t.requester_session_id \
                 LEFT JOIN sessions w ON w.id = t.worker_session_id \
                 WHERE (?1 IS NULL OR t.requester_session_id = ?1) \
                   AND (?2 IS NULL OR t.state = ?2) \
                   AND (?3 IS NULL OR r.host_alias = ?3 OR w.host_alias = ?3) \
                 ORDER BY t.created_at DESC, t.id DESC LIMIT ?4",
                cols = task_columns_t()
            ))
            .map_err(crate::ipc_error::IpcError::from)?;
        let rows = stmt
            .query_map(
                rusqlite::params![requester_session_id, state, host, limit],
                map_task_row,
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(crate::ipc_error::IpcError::from)?);
        }
        Ok(out)
    }

    /// Every `queued` / `running` task (oldest first), for the liveness sweep.
    pub fn open_tasks(&self) -> Result<Vec<TaskRow>, crate::ipc_error::IpcError> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {TASK_COLUMNS} FROM tasks WHERE state IN ('queued','running') \
                 ORDER BY created_at ASC, id ASC"
            ))
            .map_err(crate::ipc_error::IpcError::from)?;
        let rows = stmt
            .query_map([], map_task_row)
            .map_err(crate::ipc_error::IpcError::from)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(crate::ipc_error::IpcError::from)?);
        }
        Ok(out)
    }

    /// Remember the worker's Claude conversation id for a task.
    pub fn set_task_worker_claude_id(
        &self,
        id: i64,
        claude_session_id: &str,
    ) -> Result<(), crate::ipc_error::IpcError> {
        self.conn
            .execute(
                "UPDATE tasks SET worker_claude_session_id = ?1 WHERE id = ?2",
                rusqlite::params![claude_session_id, id],
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        Ok(())
    }

    /// The `queued` / `running` tasks a worker session is executing (oldest
    /// first — the marker scan resolves them in dispatch order).
    pub fn open_tasks_for_worker(
        &self,
        worker_session_id: i64,
    ) -> Result<Vec<TaskRow>, crate::ipc_error::IpcError> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {TASK_COLUMNS} FROM tasks \
                 WHERE worker_session_id = ?1 AND state IN ('queued','running') \
                 ORDER BY created_at ASC, id ASC"
            ))
            .map_err(crate::ipc_error::IpcError::from)?;
        let rows = stmt
            .query_map(rusqlite::params![worker_session_id], map_task_row)
            .map_err(crate::ipc_error::IpcError::from)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(crate::ipc_error::IpcError::from)?);
        }
        Ok(out)
    }

    /// Attach (or replace) the worker of a queued task.
    pub fn set_task_worker(
        &self,
        id: i64,
        worker_session_id: i64,
    ) -> Result<Option<TaskRow>, crate::ipc_error::IpcError> {
        self.conn
            .execute(
                "UPDATE tasks SET worker_session_id = ?1 WHERE id = ?2",
                rusqlite::params![worker_session_id, id],
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        self.emit_task(id)
    }

    /// `queued → running`; stamps `started_at`. A no-op (returns the current
    /// row) for any other state.
    pub fn mark_task_running(
        &self,
        id: i64,
    ) -> Result<Option<TaskRow>, crate::ipc_error::IpcError> {
        self.conn
            .execute(
                "UPDATE tasks SET state = 'running', started_at = ?1 \
                 WHERE id = ?2 AND state = 'queued'",
                rusqlite::params![now_unix(), id],
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        self.emit_task(id)
    }

    /// Move a task to a terminal state (`done` / `failed` / `cancelled`),
    /// storing `result` / `error` and stamping `finished_at`. Only an open
    /// (`queued` / `running`) task transitions — a terminal task is never
    /// rewritten, so a late marker scan cannot resurrect a cancelled task.
    /// Returns the row and whether THIS call performed the transition.
    pub fn finish_task(
        &self,
        id: i64,
        state: &str,
        result: Option<&str>,
        error: Option<&str>,
    ) -> Result<(Option<TaskRow>, bool), crate::ipc_error::IpcError> {
        if !TASK_TERMINAL_STATES.contains(&state) {
            return Err(crate::ipc_error::IpcError::new(
                "E_INVALID",
                format!("{state} is not a terminal task state"),
            ));
        }
        let changed = self
            .conn
            .execute(
                "UPDATE tasks SET state = ?1, result = ?2, error = ?3, finished_at = ?4 \
                 WHERE id = ?5 AND state IN ('queued','running')",
                rusqlite::params![state, result, error, now_unix(), id],
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        let row = if changed > 0 {
            self.emit_task(id)?
        } else {
            self.fetch_task(id)?
        };
        Ok((row, changed > 0))
    }

    fn emit_task(&self, id: i64) -> Result<Option<TaskRow>, crate::ipc_error::IpcError> {
        let row = self.fetch_task(id)?;
        if let Some(ref r) = row {
            self.bus.task_updated(r);
        }
        Ok(row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tasks_crud_and_terminal_transitions() {
        let s = Store::open_in_memory().unwrap();
        let t = s
            .insert_task(Some(1), Some(2), "do it", "abcd1234")
            .unwrap();
        assert_eq!((t.state.as_str(), t.nonce.as_str()), ("queued", "abcd1234"));
        assert_eq!(s.get_task(t.id).unwrap().unwrap(), t);
        assert!(s.get_task(999).unwrap().is_none());
        let t2 = s.insert_task(None, None, "later", "ffff0000").unwrap();
        let t2 = s.set_task_worker(t2.id, 2).unwrap().unwrap();
        assert_eq!(t2.worker_session_id, Some(2));
        // Listing: newest first, filters.
        let all = s.list_tasks(None, None, None, 50).unwrap();
        assert_eq!(
            all.iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![t2.id, t.id]
        );
        assert_eq!(s.list_tasks(Some(1), None, None, 50).unwrap().len(), 1);
        assert_eq!(
            s.list_tasks(None, Some("queued"), None, 1).unwrap().len(),
            1
        );
        assert_eq!(s.open_tasks_for_worker(2).unwrap().len(), 2);
        // running stamps started_at once.
        let r = s.mark_task_running(t.id).unwrap().unwrap();
        assert!(r.started_at.is_some());
        assert_eq!(s.mark_task_running(t.id).unwrap().unwrap(), r);
        // A non-terminal state is refused; a terminal one flips once.
        assert_eq!(
            s.finish_task(t.id, "running", None, None).unwrap_err().code,
            "E_INVALID"
        );
        let (row, changed) = s.finish_task(t.id, "done", Some("ok"), None).unwrap();
        assert!(changed);
        let row = row.unwrap();
        assert_eq!(
            (row.state.as_str(), row.result.as_deref()),
            ("done", Some("ok"))
        );
        assert!(row.finished_at.is_some());
        let (row, changed) = s
            .finish_task(t.id, "cancelled", None, Some("late"))
            .unwrap();
        assert!(!changed, "terminal tasks are never rewritten");
        assert_eq!(row.unwrap().state, "done");
        assert_eq!(s.open_tasks_for_worker(2).unwrap().len(), 1);
        // The nonce never serialises to the wire.
        let json = serde_json::to_value(&t).unwrap();
        assert!(json.get("nonce").is_none());
        assert_eq!(json["state"], "queued");
    }

    #[test]
    fn list_tasks_scopes_by_host_in_sql() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("a").unwrap();
        s.upsert_host("b").unwrap();
        let ra = s
            .upsert_session("r", "a", None, None, 1, 1, "running", None)
            .unwrap();
        let wb = s
            .upsert_session("w", "b", None, None, 1, 1, "running", None)
            .unwrap();
        let rb = s
            .upsert_session("r2", "b", None, None, 1, 1, "running", None)
            .unwrap();
        let t1 = s.insert_task(Some(ra), Some(wb), "x", "n1").unwrap();
        let t2 = s.insert_task(Some(rb), Some(wb), "y", "n2").unwrap();
        let _orphan = s.insert_task(None, None, "z", "n3").unwrap();
        let ids = |h: Option<&str>| -> Vec<i64> {
            let mut v: Vec<i64> = s
                .list_tasks(None, None, h, 50)
                .unwrap()
                .iter()
                .map(|t| t.id)
                .collect();
            v.sort();
            v
        };
        assert_eq!(ids(Some("a")), vec![t1.id]);
        assert_eq!(ids(Some("b")), vec![t1.id, t2.id]);
        assert_eq!(ids(None).len(), 3);
        assert!(ids(Some("c")).is_empty());
    }

    #[test]
    fn transcript_path_and_task_worker_claude_id_round_trip() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("a", "local", None, None, 1, 1, "running", None)
            .unwrap();
        assert_eq!(s.session_transcript_path(id).unwrap(), None);
        s.set_claude_session_id(id, "uuid-a").unwrap();
        s.set_transcript_path_by_claude_id("uuid-a", "/h/.claude/projects/x/uuid-a.jsonl")
            .unwrap();
        assert_eq!(
            s.session_transcript_path(id).unwrap().as_deref(),
            Some("/h/.claude/projects/x/uuid-a.jsonl")
        );
        let t = s.insert_task(None, Some(id), "p", "n").unwrap();
        assert_eq!(t.worker_claude_session_id, None);
        s.set_task_worker_claude_id(t.id, "uuid-a").unwrap();
        let t = s.get_task(t.id).unwrap().unwrap();
        assert_eq!(t.worker_claude_session_id.as_deref(), Some("uuid-a"));
        assert_eq!(s.open_tasks().unwrap().len(), 1);
        assert!(serde_json::to_value(&t)
            .unwrap()
            .get("worker_claude_session_id")
            .is_none());
    }

    #[test]
    fn task_writes_emit_task_updated_events() {
        let bus = Arc::new(crate::events::RecordingEventBus::new());
        let s = Store::open_with_bus_in_memory(bus.clone()).unwrap();
        let t = s.insert_task(None, Some(1), "x", "n").unwrap();
        s.mark_task_running(t.id).unwrap();
        s.finish_task(t.id, "failed", None, Some("boom")).unwrap();
        s.finish_task(t.id, "done", None, None).unwrap();
        assert_eq!(
            bus.take(),
            vec![
                format!("task:updated:{}:queued", t.id),
                format!("task:updated:{}:running", t.id),
                format!("task:updated:{}:failed", t.id),
            ],
            "no event for the refused rewrite of a terminal task"
        );
    }
}
