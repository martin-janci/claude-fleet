//! Session rows: CRUD, field setters, ghosting and hook writes.

use super::*;

impl Store {
    // ---- Private fetch helpers used after writes to produce emit payloads ----
    //
    // The row-mapping SQL lives in free `fetch_*` functions that take a bare
    // `&Connection` so it can be reused both by these `&self` helpers AND by
    // the `_in_tx` mutation variants (a `&Transaction` derefs to `&Connection`).

    pub fn get_session(
        &self,
        tmux_name: &str,
        host_alias: &str,
    ) -> Result<Option<SessionRow>, rusqlite::Error> {
        fetch_session(&self.conn, tmux_name, host_alias)
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_session(
        &self,
        tmux_name: &str,
        host_alias: &str,
        project_id: Option<i64>,
        worktree_id: Option<i64>,
        created_at: i64,
        last_activity_at: i64,
        status: &str,
        account_uuid: Option<&str>,
    ) -> Result<i64, rusqlite::Error> {
        // Check existence before the write so we can distinguish created vs updated.
        let existing_id: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM sessions WHERE tmux_name=?1 AND host_alias=?2",
                rusqlite::params![tmux_name, host_alias],
                |row| row.get(0),
            )
            .optional()?;

        // INSERT ... RETURNING id — one statement instead of the old
        // INSERT then separate `SELECT id`.
        let id: i64 = self.conn.query_row(
            "INSERT INTO sessions (tmux_name, host_alias, project_id, worktree_id,
                                   created_at, last_activity_at, status, account_uuid)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(host_alias, tmux_name) DO UPDATE SET
               project_id=excluded.project_id,
               worktree_id=excluded.worktree_id,
               last_activity_at=excluded.last_activity_at,
               status=excluded.status,
               account_uuid=excluded.account_uuid
             RETURNING id",
            rusqlite::params![
                tmux_name,
                host_alias,
                project_id,
                worktree_id,
                created_at,
                last_activity_at,
                status,
                account_uuid
            ],
            |row| row.get(0),
        )?;
        if let Some(row) = self.get_session(tmux_name, host_alias)? {
            if existing_id.is_none() {
                self.bus.session_created(&row);
            } else {
                self.bus.session_updated(&row);
            }
        }
        Ok(id)
    }

    /// Upsert a synthetic `kind IN ('bg','external')` session for a
    /// `claude --bg` (`kind='bg'`) or interactive-but-tmux-less (`kind='external'`)
    /// agent that has NO matching tmux session. The sentinel `tmux_name`
    /// (`bg:<sessionId>`) keeps it unique under the `(host_alias, tmux_name)`
    /// constraint and signals to the UI that there is no tmux pane to attach.
    /// Refreshes the live `claude_status` and `kind` on every reconcile (an
    /// existing row is reclassified when the agent's kind changes); the
    /// synthetic kinds exempt the row from the tmux-keyed ghost cleanup (it
    /// is never in the tmux `keep` set) — such rows are instead pruned
    /// against the `claude agents --json` result by
    /// `ghost_and_clean_bg_sessions`. A row that was ghosted by that pruner
    /// and whose agent reappears is resurrected here (`status='running'`,
    /// `lost_at=NULL`), mirroring the tmux upsert's ghost revival.
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_bg_session(
        &self,
        host_alias: &str,
        tmux_name: &str,
        project_id: Option<i64>,
        claude_session_id: &str,
        claude_status: Option<&str>,
        last_activity_at: i64,
        kind: &str,
    ) -> Result<i64, rusqlite::Error> {
        debug_assert!(
            kind == "bg" || kind == "external",
            "upsert_bg_session: invalid agent kind {kind:?}"
        );
        let existing_id: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM sessions WHERE tmux_name=?1 AND host_alias=?2",
                rusqlite::params![tmux_name, host_alias],
                |row| row.get(0),
            )
            .optional()?;

        let sql = format!(
            "INSERT INTO sessions (tmux_name, host_alias, project_id, worktree_id,
                                   created_at, last_activity_at, status, kind,
                                   claude_session_id, claude_status, idle_since)
             VALUES (?1, ?2, ?3, NULL, ?4, ?4, 'running', ?8, ?5, ?6,
                     CASE WHEN ?6 IN ('idle','completed','stopped') THEN ?7 ELSE NULL END)
             ON CONFLICT(host_alias, tmux_name) DO UPDATE SET
               project_id=COALESCE(excluded.project_id, project_id),
               last_activity_at=excluded.last_activity_at,
               kind=excluded.kind,
               status='running',
               lost_at=NULL,
               lost_reason=NULL,
               claude_session_id=COALESCE(excluded.claude_session_id, claude_session_id),
               claude_status=COALESCE(excluded.claude_status, claude_status),
               idle_since={idle}
             RETURNING id",
            idle = idle_since_sql("COALESCE(excluded.claude_status, claude_status)", "?7"),
        );
        let id: i64 = self.conn.query_row(
            &sql,
            rusqlite::params![
                tmux_name,
                host_alias,
                project_id,
                last_activity_at,
                claude_session_id,
                claude_status,
                now_unix(),
                kind
            ],
            |row| row.get(0),
        )?;
        if let Some(row) = self.get_session(tmux_name, host_alias)? {
            if existing_id.is_none() {
                self.bus.session_created(&row);
            } else {
                self.bus.session_updated(&row);
            }
        }
        Ok(id)
    }

    /// Two-phase cleanup for synthetic `kind IN ('bg','external')` rows on one
    /// host, keyed on the CURRENT `claude agents --json` result (`keep_names`
    /// = the sentinel `bg:<sessionId>` names observed this reconcile pass)
    /// instead of the tmux `keep` set. The two-phase ghost-then-reap itself is
    /// [`Store::ghost_and_clean`], shared with the tmux-keyed reconcile pass;
    /// this wrapper only owns the transaction and the post-commit emit.
    ///
    /// The one-cycle grace matters because a failed
    /// `claude agents` probe is indistinguishable from "no agents" (both come
    /// back as an empty list): a transient miss only ghosts, and
    /// `upsert_bg_session` resurrects the row when the agent reappears.
    ///
    /// Without this pruner, dead bg rows accumulate forever (observed:
    /// 22k rows / 88MB state.db).
    pub fn ghost_and_clean_bg_sessions(
        &self,
        host_alias: &str,
        keep_names: &[String],
        now: i64,
        lost_ttl_cutoff: Option<i64>,
    ) -> Result<(), rusqlite::Error> {
        let tx = self.conn.unchecked_transaction()?;
        let mut changes: Vec<RowChange> = Vec::new();
        Self::ghost_and_clean(
            &tx,
            host_alias,
            keep_names,
            now,
            KIND_PANE_LESS,
            None,
            lost_ttl_cutoff,
            &mut changes,
        )?;
        tx.commit()?;
        // Emit only after the commit so no event fires for a rolled-back write.
        for change in &changes {
            self.bus.emit_change(change);
        }
        Ok(())
    }

    /// One-shot: mark every non-ghost row of `host_alias` NOT in `keep_names`
    /// as lost, recording WHY (`reason`, one of `host_reboot` /
    /// `tmux_server_gone` / `missing`). Called by the reboot/vanished-tmux
    /// detector (Task 6) instead of waiting out the normal one-cycle ghost
    /// grace, so the resume path can tell a reboot apart from a routine probe
    /// miss.
    ///
    /// `reason == "tmux_server_gone"` only ghosts tmux-backed rows
    /// ([`KIND_TMUX`]) — a tmux restart does not kill a `claude --bg` agent,
    /// so those rows are left alone. Any other reason (namely `host_reboot`)
    /// ghosts every kind, since a reboot kills bg agents too.
    ///
    /// Reclassification: the same call ALSO upgrades already-ghost rows of
    /// the host that are not in `keep_names` and carry `lost_reason =
    /// 'missing'` (or NULL) to `reason`, keeping their existing `lost_at`.
    /// Those are rows the routine keep-set prune ghosted on a FAILED first
    /// post-loss pass (the identity read, the stored-identity read or this
    /// very mark failed, so no verdict was recorded and the prune ran); the
    /// verdict firing on the next pass must still record them as a mass
    /// loss, or Phase 2 would reap them as ordinary `missing` rows. A
    /// `missing` ghost is at most one pass old by construction (never
    /// exempt, reaped next pass). Accepted trade-off: a session that
    /// genuinely ended within that one pass is indistinguishable from one a
    /// failed pass ghosted, so it is reclassified and kept to the TTL too
    /// (still dismissable) — erring toward keeping is the feature's point.
    /// A row fleet itself killed carries `lost_reason = 'killed'`
    /// ([`Self::mark_session_killed`]): fleet knows that loss is not
    /// ambiguous, so it is never reclassified. Same kind
    /// filter and same BE-3 guard as the main mark. Reclassified rows DO
    /// change on the wire (`lost_reason` is a `SessionRow` field), so a
    /// `SessionUpdated` is emitted for each of them too, after the tx
    /// commits, the same way `marked` rows are; they are also logged with
    /// the distinct lifecycle kind `"reclassified"`.
    ///
    /// Mirrors [`Self::ghost_and_clean_bg_sessions`]'s shape: its own
    /// `unchecked_transaction`, collect the affected rows, commit, and only
    /// THEN emit one `SessionUpdated` per newly marked row.
    ///
    /// `probe_started_at` is the BE-3 guard, identical to
    /// [`Store::ghost_and_clean`]'s Phase 1: a row whose `last_reconciled_at`
    /// is at or after this probe's start was reconciled by a NEWER pass
    /// (e.g. `new_session`'s own single-host reconcile, which runs outside
    /// the fleet-wide gate and can commit before an in-flight tick's stale
    /// write lands) — its absence from this verdict's evidence is not
    /// evidence it is lost, so it is left alone. `0` disables the guard
    /// (every row eligible), exactly as [`super::reconcile::ghost_cutoff`]
    /// defines for Phase 1; reused here rather than reimplemented.
    pub fn mark_host_sessions_lost(
        &self,
        host_alias: &str,
        reason: &str,
        keep_names: &[String],
        now: i64,
        probe_started_at: i64,
    ) -> Result<MarkedLost, rusqlite::Error> {
        // A tmux restart does not kill `claude --bg` agents; a reboot does.
        let kind_filter = if reason == "tmux_server_gone" {
            KIND_TMUX
        } else {
            "1=1"
        };
        let not_in = if keep_names.is_empty() {
            String::new()
        } else {
            format!(" AND tmux_name NOT IN ({})", in_clause(keep_names.len()))
        };
        let cutoff = super::reconcile::ghost_cutoff(probe_started_at);
        let tx = self.conn.unchecked_transaction()?;
        let fetch_all = |ids: &[i64]| -> Result<Vec<SessionRow>, rusqlite::Error> {
            let mut rows = Vec::new();
            for id in ids {
                if let Some(row) = fetch_session_by_id(&tx, *id)? {
                    rows.push(row);
                }
            }
            Ok(rows)
        };
        // Reclassify FIRST, while the rows the main mark is about to ghost
        // are still live: those get `reason` directly and must not be
        // counted twice. `lost_at` is deliberately left untouched.
        let reclassify_sql = format!(
            "UPDATE sessions SET lost_reason=?1
             WHERE host_alias=?2 AND status='ghost'
               AND COALESCE(lost_reason, 'missing')='missing' AND {kind_filter}
               AND COALESCE(last_reconciled_at, 0) < ?3{not_in}
             RETURNING id"
        );
        let head: Vec<&dyn rusqlite::ToSql> = vec![&reason, &host_alias, &cutoff];
        let params = params_then(&head, keep_names);
        let reclassified_ids: Vec<i64> = tx
            .prepare(&reclassify_sql)?
            .query_map(params.as_slice(), |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let sql = format!(
            "UPDATE sessions SET status='ghost', lost_at=?1, lost_reason=?2
             WHERE host_alias=?3 AND status!='ghost' AND {kind_filter}
               AND COALESCE(last_reconciled_at, 0) < ?4{not_in}
             RETURNING id"
        );
        let head: Vec<&dyn rusqlite::ToSql> = vec![&now, &reason, &host_alias, &cutoff];
        let params = params_then(&head, keep_names);
        let ids: Vec<i64> = tx
            .prepare(&sql)?
            .query_map(params.as_slice(), |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let out = MarkedLost {
            marked: fetch_all(&ids)?,
            reclassified: fetch_all(&reclassified_ids)?,
        };
        tx.commit()?;
        for row in &out.reclassified {
            // `lost_reason` is on the wire now, so this IS a change the
            // frontend sees — emit it, re-reading the committed row the same
            // way every other mutation here announces itself. A distinct
            // lifecycle kind, NOT "lost": the row was already logged "lost"
            // when Phase 1 ghosted it.
            tracing::info!(
                lifecycle = "reclassified",
                session_id = row.id,
                host_alias = %row.host_alias,
                tmux_name = %row.tmux_name,
                claude_session_id = row.claude_session_id.as_deref().unwrap_or("-"),
                reason,
                "[session] reclassified"
            );
            let _ = self.emit_session(row.id);
        }
        for row in &out.marked {
            let change = RowChange::SessionUpdated(row.clone());
            // Task 7 (R5): the mass-loss verdict is exactly the reboot-
            // forensics case this logging exists for, so log it with the
            // `reason` this call already knows (`host_reboot` /
            // `tmux_server_gone`) alongside the shared `lifecycle` kind.
            if let Some(kind) = super::reconcile::lifecycle_kind(&change) {
                tracing::info!(
                    lifecycle = kind,
                    session_id = row.id,
                    host_alias = %row.host_alias,
                    tmux_name = %row.tmux_name,
                    claude_session_id = row.claude_session_id.as_deref().unwrap_or("-"),
                    reason,
                    "[session] {kind}"
                );
            }
            self.bus.emit_change(&change);
        }
        Ok(out)
    }

    /// Ghost the row `id` right after fleet ITSELF killed its tmux session
    /// (`kill_session`, which `move_session` also reaches): `status='ghost'`,
    /// `lost_at=now`, `lost_reason='killed'`. tmux exits when its last
    /// session closes, so killing a host's only session makes the next probe
    /// see no tmux server — a `tmux_server_gone` verdict. Recording the kill
    /// first keeps that verdict (which only marks `status != 'ghost'` rows)
    /// off this row, and `'killed'` is neither TTL-exempt in Phase 2 nor
    /// eligible for [`Self::mark_host_sessions_lost`]'s reclassification of
    /// `missing` ghosts, so the ordinary one-cycle reap removes it. No-op
    /// (returns `None`) when the row is gone or already ghost. Emits
    /// `SessionUpdated` like the routine Phase 1 ghosting it stands in for.
    pub fn mark_session_killed(
        &self,
        id: i64,
        now: i64,
    ) -> Result<Option<SessionRow>, rusqlite::Error> {
        let changed = self.conn.execute(
            "UPDATE sessions SET status='ghost', lost_at=?1, lost_reason='killed'
             WHERE id=?2 AND status!='ghost'",
            rusqlite::params![now, id],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        let row = fetch_session_by_id(&self.conn, id)?;
        if let Some(row) = &row {
            tracing::info!(
                lifecycle = "lost",
                session_id = row.id,
                host_alias = %row.host_alias,
                tmux_name = %row.tmux_name,
                claude_session_id = row.claude_session_id.as_deref().unwrap_or("-"),
                reason = "killed",
                "[session] lost"
            );
            self.bus
                .emit_change(&RowChange::SessionUpdated(row.clone()));
        }
        Ok(row)
    }

    /// User-initiated removal of an agent row (`claude agents --json`, not a
    /// tmux session) from the list: records `claude_session_id` as dismissed
    /// as of `now` in `dismissed_agents`, then hard-deletes its `sessions`
    /// row the same way `delete_session` does (and emits the same
    /// `session:removed`/killed event via it). Reconcile is expected to skip
    /// re-creating the row while the agent's last activity is no newer than
    /// `dismissed_at` — see `dismissed_agents`.
    pub fn dismiss_agent(
        &self,
        host_alias: &str,
        claude_session_id: &str,
        now: i64,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO dismissed_agents (host_alias, claude_session_id, dismissed_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(host_alias, claude_session_id) DO UPDATE SET
               dismissed_at=excluded.dismissed_at",
            rusqlite::params![host_alias, claude_session_id, now],
        )?;
        let tmux_name = format!("bg:{claude_session_id}");
        let id: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM sessions WHERE host_alias=?1 AND tmux_name=?2",
                rusqlite::params![host_alias, tmux_name],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(id) = id {
            self.delete_session(id)?;
        }
        Ok(())
    }

    /// Every dismissed `claude_session_id` on `host_alias`, mapped to its
    /// `dismissed_at` stamp. Used by reconcile to skip re-surfacing an agent
    /// the user removed (see `dismiss_agent`).
    pub fn dismissed_agents(
        &self,
        host_alias: &str,
    ) -> Result<std::collections::HashMap<String, i64>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT claude_session_id, dismissed_at FROM dismissed_agents WHERE host_alias=?1",
        )?;
        let rows = stmt.query_map(rusqlite::params![host_alias], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        rows.collect()
    }

    /// Clear a dismissal (e.g. the agent produced new activity and should be
    /// surfaced again).
    pub fn clear_agent_dismissal(
        &self,
        host_alias: &str,
        claude_session_id: &str,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "DELETE FROM dismissed_agents WHERE host_alias=?1 AND claude_session_id=?2",
            rusqlite::params![host_alias, claude_session_id],
        )?;
        Ok(())
    }

    pub fn get_session_account(
        &self,
        host_alias: &str,
        tmux_name: &str,
    ) -> Result<Option<String>, rusqlite::Error> {
        self.conn
            .prepare_cached(
                "SELECT account_uuid FROM sessions WHERE host_alias=?1 AND tmux_name=?2",
            )?
            .query_row(rusqlite::params![host_alias, tmux_name], |row| {
                row.get::<_, Option<String>>(0)
            })
            .optional()
            .map(Option::flatten)
    }

    pub fn list_sessions_for_host(
        &self,
        host_alias: &str,
    ) -> Result<Vec<SessionRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            &format!(
             "SELECT {SESSION_COLUMNS} FROM sessions WHERE host_alias=?1 ORDER BY last_activity_at DESC"),
        )?;
        let rows = stmt.query_map(rusqlite::params![host_alias], map_session_row)?;
        rows.collect()
    }

    /// All sessions across every host, in one query. Used by `reconcile_sessions`
    /// to collect its return value once at the end instead of N per-host reads.
    pub fn list_all_sessions(&self) -> Result<Vec<SessionRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {SESSION_COLUMNS} FROM sessions ORDER BY last_activity_at DESC"
        ))?;
        let rows = stmt.query_map([], map_session_row)?;
        rows.collect()
    }

    pub fn list_related_sessions(
        &self,
        session_id: i64,
    ) -> Result<Vec<SessionRow>, rusqlite::Error> {
        // Look up source's (project_id, worktree_key) first.
        let (proj, key): (Option<i64>, Option<String>) = self.conn.query_row(
            "SELECT project_id, worktree_key FROM sessions WHERE id=?1",
            rusqlite::params![session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        // Orphans (project_id=NULL) have no relateds — they share no identity.
        let Some(project_id) = proj else {
            return Ok(Vec::new());
        };
        // A project-having session always has a worktree_key after reconcile
        // ("main" at minimum). A NULL key (legacy/pre-reconcile) matches nothing.
        let Some(key) = key else {
            return Ok(Vec::new());
        };
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {SESSION_COLUMNS} FROM sessions
             WHERE project_id=?1 AND worktree_key=?2 AND id<>?3
             ORDER BY host_alias ASC, tmux_name ASC"
        ))?;
        let rows = stmt.query_map(rusqlite::params![project_id, key, session_id], |row| {
            map_session_row(row)
        })?;
        rows.collect()
    }

    /// Mark a session as a review of `reviews_session_id` (or back to 'work' with
    /// None). Write-once at spawn_review time. Reconcile never touches these
    /// columns — they survive re-probe because upsert_session's ON CONFLICT clause
    /// omits them.
    pub fn set_session_kind(
        &self,
        id: i64,
        kind: &str,
        reviews_session_id: Option<i64>,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions SET kind = ?1, reviews_session_id = ?2 WHERE id = ?3",
            rusqlite::params![kind, reviews_session_id, id],
        )?;
        self.emit_session(id)?;
        Ok(())
    }

    /// Record the Claude Code session id minted for a session. Reconcile's
    /// `upsert_session` never writes this column, so the value survives
    /// reconciliation.
    pub fn set_claude_session_id(&self, id: i64, uuid: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions SET claude_session_id=?1 WHERE id=?2",
            rusqlite::params![uuid, id],
        )?;
        Ok(())
    }

    /// Set a session's portable worktree key (derived from its cwd by reconcile).
    /// Emits `session_updated` so the frontend patches in place.
    pub fn set_worktree_key(&self, id: i64, key: Option<&str>) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions SET worktree_key = ?1 WHERE id = ?2",
            rusqlite::params![key, id],
        )?;
        self.emit_session(id)?;
        Ok(())
    }

    /// One-shot startup pass: give every session row with `friendly_name IS
    /// NULL` a deterministic label derived from its branch (or the worktree
    /// name when no branch is recorded, or the tmux name as last resort).
    /// Skips pane-less rows (`kind IN ('bg','external')`) — their synthetic
    /// `bg:<uuid>` tmux names humanise poorly. Bypasses the event bus by design: the frontend hasn't
    /// subscribed yet, and emitting one event per row at boot is pure noise.
    /// Returns the number of rows updated.
    pub fn backfill_friendly_names(&self) -> Result<usize, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.tmux_name,
                    COALESCE(p.owner, '') AS owner,
                    COALESCE(p.repo, '') AS repo,
                    COALESCE(w.branch, w.name) AS branch
               FROM sessions s
               LEFT JOIN projects p ON p.id = s.project_id
               LEFT JOIN worktrees w ON w.id = s.worktree_id
              WHERE s.friendly_name IS NULL
                AND COALESCE(s.kind, 'work') NOT IN ('bg','external')",
        )?;
        let rows: Vec<(i64, String, String, String, Option<String>)> = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        let mut updated = 0usize;
        for (id, tmux_name, owner, repo, branch) in rows {
            let source = branch.unwrap_or(tmux_name);
            let label = crate::humanize::humanize_branch(&source, &owner, &repo);
            if label.is_empty() {
                continue;
            }
            self.conn.execute(
                "UPDATE sessions SET friendly_name = ?1 WHERE id = ?2",
                rusqlite::params![label, id],
            )?;
            updated += 1;
        }
        Ok(updated)
    }

    /// The deterministic branch-derived label `new_session` /
    /// `backfill_friendly_names` would give this row (PR #28), or `None` for
    /// pane-less (bg / external) rows and rows whose humanised name is empty. Lets callers tell a
    /// still-default label from one a human or agent chose.
    pub fn default_friendly_name(&self, id: i64) -> Result<Option<String>, rusqlite::Error> {
        let row: Option<(String, String, String, Option<String>, String)> = self
            .conn
            .query_row(
                "SELECT s.tmux_name,
                        COALESCE(p.owner, '') AS owner,
                        COALESCE(p.repo, '') AS repo,
                        COALESCE(w.branch, w.name) AS branch,
                        COALESCE(s.kind, 'work')
                   FROM sessions s
                   LEFT JOIN projects p ON p.id = s.project_id
                   LEFT JOIN worktrees w ON w.id = s.worktree_id
                  WHERE s.id = ?1",
                rusqlite::params![id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()?;
        let Some((tmux_name, owner, repo, branch, kind)) = row else {
            return Ok(None);
        };
        if super::has_no_pane(&kind) {
            return Ok(None);
        }
        let source = branch.unwrap_or(tmux_name);
        let label = crate::humanize::humanize_branch(&source, &owner, &repo);
        Ok(if label.is_empty() { None } else { Some(label) })
    }

    /// Set the session's display label (migration 016). `None` clears it.
    /// Emits `session_updated` so the sidebar patches in place.
    pub fn set_friendly_name(
        &self,
        host_alias: &str,
        tmux_name: &str,
        friendly_name: Option<&str>,
    ) -> Result<Option<SessionRow>, rusqlite::Error> {
        let changed = self.conn.execute(
            "UPDATE sessions SET friendly_name = ?1 \
             WHERE host_alias = ?2 AND tmux_name = ?3",
            rusqlite::params![friendly_name, host_alias, tmux_name],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        let row = fetch_session(&self.conn, tmux_name, host_alias)?;
        if let Some(ref r) = row {
            self.bus.session_updated(r);
        }
        Ok(row)
    }

    /// Mark a safe-kill request: stamp state="requested", store the nonce we
    /// embedded in the prompt, and clear any prior failure detail. Emits
    /// session_updated.
    pub fn set_safe_kill_requested(
        &self,
        id: i64,
        nonce: &str,
        requested_at: i64,
    ) -> Result<Option<SessionRow>, rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions
                SET safe_kill_state='requested',
                    safe_kill_nonce=?1,
                    safe_kill_detail=NULL,
                    safe_kill_requested_at=?2
              WHERE id=?3",
            rusqlite::params![nonce, requested_at, id],
        )?;
        self.emit_session(id)
    }

    /// Transition a safe-kill request to its terminal state ("ready" or
    /// "failed") and store the optional failure detail. Emits session_updated.
    pub fn set_safe_kill_outcome(
        &self,
        id: i64,
        state: &str,
        detail: Option<&str>,
    ) -> Result<Option<SessionRow>, rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions
                SET safe_kill_state=?1,
                    safe_kill_detail=?2
              WHERE id=?3",
            rusqlite::params![state, detail, id],
        )?;
        self.emit_session(id)
    }

    /// Clear the safe-kill state (used when the user cancels or retries a
    /// failed attempt). Emits session_updated.
    pub fn clear_safe_kill(&self, id: i64) -> Result<Option<SessionRow>, rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions
                SET safe_kill_state=NULL,
                    safe_kill_nonce=NULL,
                    safe_kill_detail=NULL,
                    safe_kill_requested_at=NULL
              WHERE id=?1",
            rusqlite::params![id],
        )?;
        self.emit_session(id)
    }

    /// Transition a session back to running (clears `lost_at`). Called by the
    /// `recreate_session` flow after `new_session` rebuilds the tmux session on
    /// the host — for both ghost and live (RAM/wedged) recreates.
    pub fn restore_session(&self, id: i64) -> Result<Option<SessionRow>, rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions SET status='running', lost_at=NULL, lost_reason=NULL WHERE id=?1",
            rusqlite::params![id],
        )?;
        self.emit_session(id)
    }

    pub fn get_session_by_id(&self, id: i64) -> Result<Option<SessionRow>, rusqlite::Error> {
        fetch_session_by_id(&self.conn, id)
    }

    /// Re-read `id` after a write and announce it: `session_updated` when
    /// the row exists, nothing when it is gone. Returns the row.
    pub(super) fn emit_session(&self, id: i64) -> Result<Option<SessionRow>, rusqlite::Error> {
        let row = fetch_session_by_id(&self.conn, id)?;
        if let Some(ref r) = row {
            self.bus.session_updated(r);
        }
        Ok(row)
    }

    /// [`Self::emit_session`] keyed by `claude_session_id` (the hook writes).
    fn emit_session_by_claude_id(
        &self,
        claude_session_id: &str,
    ) -> Result<Option<SessionRow>, crate::ipc_error::IpcError> {
        let row = self.fetch_session_by_claude_id(claude_session_id)?;
        if let Some(ref r) = row {
            self.bus.session_updated(r);
        }
        Ok(row)
    }

    /// Remember the most recent prompt sent to a session (first 200 chars,
    /// migration 019). Emits `session_updated`.
    pub fn set_last_prompt(
        &self,
        id: i64,
        prompt: &str,
    ) -> Result<Option<SessionRow>, rusqlite::Error> {
        let truncated: String = prompt.chars().take(LAST_PROMPT_CHARS).collect();
        self.conn.execute(
            "UPDATE sessions SET last_prompt=?1 WHERE id=?2",
            rusqlite::params![truncated, id],
        )?;
        self.emit_session(id)
    }

    /// Stamp when fleet created this session (migration 019). Only sets the
    /// value once — a re-create keeps the original start.
    pub fn set_started_at(&self, id: i64, at: i64) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions SET started_at=COALESCE(started_at, ?1) WHERE id=?2",
            rusqlite::params![at, id],
        )?;
        Ok(())
    }

    /// Record that a stuck playbook acted on this row: stamps
    /// `last_playbook_at`, appends a `playbook_applied` timeline event carrying
    /// the kind, and emits `session_updated`.
    pub fn mark_playbook_applied(
        &self,
        id: i64,
        at: i64,
        detail: &str,
    ) -> Result<Option<SessionRow>, rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions SET last_playbook_at=?1 WHERE id=?2",
            rusqlite::params![at, id],
        )?;
        if let Err(e) = self.insert_session_event(id, "playbook_applied", Some(detail)) {
            tracing::warn!(
                session_id = id,
                error = %e,
                "[playbook] session_event insert failed"
            );
        }
        self.emit_session(id)
    }

    /// Hard-delete one session row (ghost dismissal) together with what dies
    /// with it, in one transaction: its `session_events` timeline and the
    /// messages addressed TO it (an inbox nobody can read). Neither table has
    /// an FK cascade, and `sessions.id` has no AUTOINCREMENT, so leftovers
    /// would surface on the next session that reuses the id. Kept: messages it
    /// SENT (they live in the recipients' inboxes) and tasks it requested or
    /// worked — the task sweep fails a task whose worker row is gone.
    pub fn delete_session(&self, id: i64) -> Result<(), rusqlite::Error> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM session_events WHERE session_id=?1",
            rusqlite::params![id],
        )?;
        tx.execute(
            "DELETE FROM session_messages WHERE to_session_id=?1",
            rusqlite::params![id],
        )?;
        tx.execute("DELETE FROM sessions WHERE id=?1", rusqlite::params![id])?;
        tx.commit()?;
        self.bus.session_killed(id);
        Ok(())
    }

    #[cfg(test)]
    pub fn delete_sessions_not_in(
        &self,
        host_alias: &str,
        keep_names: &[String],
    ) -> Result<usize, rusqlite::Error> {
        // `DELETE ... RETURNING id` — delete and collect deleted ids in one
        // statement (no separate SELECT-then-DELETE).
        let ids_to_delete: Vec<i64> = if keep_names.is_empty() {
            let mut stmt = self
                .conn
                .prepare_cached("DELETE FROM sessions WHERE host_alias=?1 RETURNING id")?;
            let ids = stmt
                .query_map(rusqlite::params![host_alias], |r| r.get::<_, i64>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids
        } else {
            let sql = format!(
                "DELETE FROM sessions WHERE host_alias=?1 AND tmux_name NOT IN ({phs}) RETURNING id",
                phs = in_clause(keep_names.len())
            );
            let params = params_then(rusqlite::params![host_alias], keep_names);
            let mut stmt = self.conn.prepare(&sql)?;
            let ids = stmt
                .query_map(params.as_slice(), |r| r.get::<_, i64>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids
        };

        for id in &ids_to_delete {
            self.bus.session_killed(*id);
        }
        Ok(ids_to_delete.len())
    }

    /// Update `claude_status` for the session whose `claude_session_id` matches.
    /// No-ops silently when no row matches (hook arrived before reconcile enriched it).
    #[cfg(test)]
    pub fn set_claude_status_by_session_id(
        &self,
        claude_session_id: &str,
        status: &str,
    ) -> Result<(), crate::ipc_error::IpcError> {
        // Only the Stop hook calls this (a turn just completed), so the
        // write doubles as the `last_turn_at` stamp and maintains `idle_since`
        // for the GC sweeper — the hook handler lives in another track's
        // file, so the lifecycle bookkeeping is kept here in the store.
        let now = now_unix();
        let sql = format!(
            "UPDATE sessions SET claude_status = ?1, last_turn_at = ?3, idle_since = {idle} \
             WHERE claude_session_id = ?2",
            idle = idle_since_sql("?1", "?3"),
        );
        let changed = self
            .conn
            .execute(&sql, rusqlite::params![status, claude_session_id, now])?;
        if changed > 0 {
            // Emit session_updated so the frontend patches the row in real-time.
            if let Ok(Some(row)) = self.fetch_session_by_claude_id(claude_session_id) {
                self.bus.session_updated(&row);
            }
        }
        Ok(())
    }

    // ── Orchestration (migration 020) ────────────────────────────────────

    /// The Stop hook's write: the turn is over. Sets `claude_status = idle`,
    /// bumps `turn_seq`, stamps `last_stop_at` / `last_turn_at` and maintains
    /// `idle_since`. Matches by `claude_session_id`; returns the updated row
    /// (`None` when no row carries this id yet). Emits `session_updated`.
    pub fn record_stop_hook(
        &self,
        claude_session_id: &str,
    ) -> Result<Option<SessionRow>, crate::ipc_error::IpcError> {
        let now = now_unix();
        let changed = self.conn.execute(
            "UPDATE sessions SET claude_status = 'idle', turn_seq = turn_seq + 1, \
                 last_stop_at = ?2, last_turn_at = ?2, last_hook_at = ?2, \
                 idle_since = COALESCE(idle_since, ?2) \
                 WHERE claude_session_id = ?1",
            rusqlite::params![claude_session_id, now],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        self.emit_session_by_claude_id(claude_session_id)
    }

    /// The UserPromptSubmit hook's write: a turn is starting. Sets
    /// `claude_status = working` and clears `idle_since` so "idle because
    /// never started" and "idle after a turn" are distinguishable from
    /// "busy". Returns the updated row (`None` when unmatched). Emits
    /// `session_updated`.
    pub fn record_prompt_submit_hook(
        &self,
        claude_session_id: &str,
    ) -> Result<Option<SessionRow>, crate::ipc_error::IpcError> {
        let changed = self.conn.execute(
            "UPDATE sessions SET claude_status = 'working', idle_since = NULL, \
                 last_hook_at = ?2 WHERE claude_session_id = ?1",
            rusqlite::params![claude_session_id, now_unix()],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        self.emit_session_by_claude_id(claude_session_id)
    }

    /// The SessionEnd hook's write: the Claude process is gone. Sets
    /// `claude_status = stopped`, starts `idle_since` if not already idle,
    /// clears any stuck episode and stamps `last_hook_at` so the reconcile
    /// guard keeps the verdict until a later pass observes the pane afresh.
    /// Returns the row (`None` when unmatched). Emits `session_updated`.
    pub fn record_session_end_hook(
        &self,
        claude_session_id: &str,
    ) -> Result<Option<SessionRow>, crate::ipc_error::IpcError> {
        let now = now_unix();
        let changed = self.conn.execute(
            "UPDATE sessions SET claude_status = 'stopped', last_turn_at = ?2, \
                 last_hook_at = ?2, idle_since = COALESCE(idle_since, ?2), \
                 stuck_kind = NULL, stuck_since = NULL \
                 WHERE claude_session_id = ?1",
            rusqlite::params![claude_session_id, now],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        self.emit_session_by_claude_id(claude_session_id)
    }

    /// The StopFailure hook's write: the turn ended in an API error. The row
    /// effect is exactly `Stop`'s (idle, `turn_seq` bump, stamps) so waiters
    /// return and read the error from the transcript; the handler records
    /// the `stop_failure` timeline event that tells the two apart.
    pub fn record_stop_failure_hook(
        &self,
        claude_session_id: &str,
    ) -> Result<Option<SessionRow>, crate::ipc_error::IpcError> {
        self.record_stop_hook(claude_session_id)
    }

    /// The Notification hook's write. `status` is the mapped status;
    /// `stuck` is `Some(Some(kind))` to set (restarting `stuck_since` when
    /// the kind changes, keeping it when equal), `Some(None)` to clear,
    /// `None` to leave the stuck fields untouched. Stamps `last_hook_at`;
    /// `idle_since` follows the status. Returns the row (`None` when
    /// unmatched). Emits `session_updated`.
    pub fn record_notification_hook(
        &self,
        claude_session_id: &str,
        status: crate::service::pane_intel::ClaudeStatus,
        stuck: Option<Option<crate::service::pane_intel::StuckKind>>,
    ) -> Result<Option<SessionRow>, crate::ipc_error::IpcError> {
        let now = now_unix();
        let status = status.as_str();
        // SQLite evaluates every right-hand side against the row's OLD
        // values, so `stuck_since` may compare against `stuck_kind` even
        // though `stuck_kind` is assigned in the same statement.
        let stuck_sql = match stuck {
            None => "",
            Some(None) => ", stuck_kind = NULL, stuck_since = NULL",
            Some(Some(_)) => {
                ", stuck_since = CASE WHEN ?3 IS stuck_kind \
                   THEN COALESCE(stuck_since, ?2) ELSE ?2 END, \
                   stuck_kind = ?3"
            }
        };
        let sql = format!(
            "UPDATE sessions SET claude_status = ?4, last_hook_at = ?2, \
             idle_since = {idle}{stuck_sql} WHERE claude_session_id = ?1",
            idle = idle_since_sql("?4", "?2"),
        );
        let kind = match stuck {
            Some(Some(k)) => Some(k.as_str()),
            _ => None,
        };
        let changed = self.conn.execute(
            &sql,
            rusqlite::params![claude_session_id, now, kind, status],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        self.emit_session_by_claude_id(claude_session_id)
    }

    /// Replace a session's tags (migration 020). Emits `session_updated`.
    pub fn set_session_tags(
        &self,
        id: i64,
        tags: &[String],
    ) -> Result<Option<SessionRow>, rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions SET tags=?1 WHERE id=?2",
            rusqlite::params![encode_tags(tags), id],
        )?;
        self.emit_session(id)
    }

    /// Record which requester dispatched work to this session. Emits
    /// `session_updated`.
    pub fn set_parent_session_id(
        &self,
        id: i64,
        parent: Option<i64>,
    ) -> Result<Option<SessionRow>, rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions SET parent_session_id=?1 WHERE id=?2",
            rusqlite::params![parent, id],
        )?;
        self.emit_session(id)
    }

    /// Store the transcript path a hook reported for this Claude session.
    /// The caller validates it (`service::hooks::valid_transcript_path`).
    pub fn set_transcript_path_by_claude_id(
        &self,
        claude_session_id: &str,
        path: &str,
    ) -> Result<(), crate::ipc_error::IpcError> {
        self.conn.execute(
            "UPDATE sessions SET transcript_path = ?1 WHERE claude_session_id = ?2",
            rusqlite::params![path, claude_session_id],
        )?;
        Ok(())
    }

    /// The hook-reported transcript path of a session, if any.
    pub fn session_transcript_path(&self, id: i64) -> Result<Option<String>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT transcript_path FROM sessions WHERE id = ?1",
                rusqlite::params![id],
                |r| r.get(0),
            )
            .optional()
            .map(Option::flatten)
    }

    /// Lookup helper: returns `None` rather than erroring when no row matches.
    /// Used by the safe-kill flow (Stop hook may arrive before reconcile
    /// enriched the row).
    pub fn get_session_by_claude_id(
        &self,
        claude_session_id: &str,
    ) -> Result<Option<SessionRow>, crate::ipc_error::IpcError> {
        self.fetch_session_by_claude_id(claude_session_id)
    }

    fn fetch_session_by_claude_id(
        &self,
        claude_session_id: &str,
    ) -> Result<Option<SessionRow>, crate::ipc_error::IpcError> {
        Ok(self
            .conn
            .prepare(&format!(
                "SELECT {SESSION_COLUMNS} FROM sessions WHERE claude_session_id = ?1"
            ))?
            .query_row(rusqlite::params![claude_session_id], map_session_row)
            .optional()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_support::*;

    /// In-memory store with the `local` host already registered, for the
    /// `kind`/dismissal tests below (`sessions.host_alias` has a FK to
    /// `hosts.alias`, enforced under `PRAGMA foreign_keys = ON`).
    fn store() -> Store {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        s
    }

    #[test]
    fn upsert_bg_session_writes_kind_and_flips_a_misfiled_row() {
        let s = store();
        s.upsert_bg_session("local", "bg:u1", None, "u1", Some("idle"), 1, "bg")
            .unwrap();
        s.upsert_bg_session("local", "bg:u1", None, "u1", Some("idle"), 2, "external")
            .unwrap();
        assert_eq!(
            s.get_session("bg:u1", "local").unwrap().unwrap().kind,
            "external"
        );
    }

    #[test]
    fn cleanup_ghosts_external_rows_too() {
        let s = store();
        s.upsert_bg_session("local", "bg:e1", None, "e1", Some("idle"), 1, "external")
            .unwrap();
        s.ghost_and_clean_bg_sessions("local", &[], 10, None)
            .unwrap();
        assert_eq!(
            s.get_session("bg:e1", "local").unwrap().unwrap().status,
            "ghost"
        );
        s.ghost_and_clean_bg_sessions("local", &[], 20, None)
            .unwrap();
        assert!(s.get_session("bg:e1", "local").unwrap().is_none());
    }

    /// `lost_reason` is also on `SessionRow` now, but most of these tests
    /// predate that and read it straight off the connection; kept as a
    /// terser assertion helper than `get_session_by_id(id)...lost_reason`.
    fn lost_reason_of(s: &Store, id: i64) -> Option<String> {
        s.conn_ref()
            .query_row(
                "SELECT lost_reason FROM sessions WHERE id=?1",
                rusqlite::params![id],
                |r| r.get(0),
            )
            .unwrap()
    }

    #[test]
    fn mark_host_sessions_lost_ghosts_unseen_rows_and_keeps_every_identity_field() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        let pid = s.upsert_project("o", "r", "/tmp/r").unwrap();
        let wid = s
            .upsert_worktree(pid, "main", "/tmp/r", Some("main"))
            .unwrap();
        // Two live tmux rows on "h", one carrying a claude_session_id, a
        // friendly_name, a last_prompt, and a project/worktree; keep "b".
        let a = s
            .upsert_session("a", "h", Some(pid), Some(wid), 1, 1, "running", None)
            .unwrap();
        s.set_claude_session_id(a, "abc").unwrap();
        s.set_friendly_name("h", "a", Some("My Friendly Name"))
            .unwrap();
        s.set_last_prompt(a, "do the thing").unwrap();
        s.upsert_session("b", "h", None, None, 1, 1, "running", None)
            .unwrap();

        let rows = s
            .mark_host_sessions_lost("h", "host_reboot", &["b".to_string()], 500, 0)
            .unwrap();
        assert_eq!(rows.marked.len(), 1);
        assert!(rows.reclassified.is_empty());

        let a_row = s.get_session("a", "h").unwrap().unwrap();
        assert_eq!(a_row.status, "ghost");
        assert_eq!(a_row.lost_at, Some(500));
        assert_eq!(a_row.claude_session_id.as_deref(), Some("abc"));
        assert_eq!(a_row.friendly_name.as_deref(), Some("My Friendly Name"));
        assert_eq!(a_row.last_prompt.as_deref(), Some("do the thing"));
        assert_eq!(a_row.project_id, Some(pid));
        assert_eq!(a_row.worktree_id, Some(wid));
        assert_eq!(
            lost_reason_of(&s, a_row.id),
            Some("host_reboot".to_string())
        );
        assert_eq!(s.get_session("b", "h").unwrap().unwrap().status, "running");
    }

    #[test]
    fn a_tmux_restart_does_not_mark_background_agents_lost() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        // One live tmux row + one live `bg` row on "h".
        let tmux_id = s
            .upsert_session("work-a", "h", None, None, 1, 1, "running", None)
            .unwrap();
        let bg_id = s
            .upsert_bg_session("h", "bg:u1", None, "u1", Some("working"), 1, "bg")
            .unwrap();

        s.mark_host_sessions_lost("h", "tmux_server_gone", &[], 500, 0)
            .unwrap();

        assert_eq!(
            s.get_session_by_id(tmux_id).unwrap().unwrap().status,
            "ghost",
            "the tmux row is ghosted"
        );
        assert_eq!(
            lost_reason_of(&s, tmux_id),
            Some("tmux_server_gone".to_string())
        );
        assert_eq!(
            s.get_session_by_id(bg_id).unwrap().unwrap().status,
            "running",
            "a tmux restart does not kill a claude --bg agent"
        );
    }

    #[test]
    fn mark_host_sessions_lost_is_idempotent_and_does_not_re_stamp_an_already_lost_row() {
        // Task 6 calls this on every pass while a tmux server stays down, so a
        // second call must never clobber the first ghosting's lost_at/lost_reason
        // — that is exactly what the `status!='ghost'` guard in the UPDATE buys.
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        let a = s
            .upsert_session("a", "h", None, None, 1, 1, "running", None)
            .unwrap();

        let first = s
            .mark_host_sessions_lost("h", "host_reboot", &[], 500, 0)
            .unwrap();
        assert_eq!(first.marked.len(), 1);

        let second = s
            .mark_host_sessions_lost("h", "tmux_server_gone", &[], 900, 0)
            .unwrap();
        assert!(
            second.is_empty(),
            "an already-ghosted row must not be re-stamped; got {second:?}"
        );

        let row = s.get_session_by_id(a).unwrap().unwrap();
        assert_eq!(
            row.lost_at,
            Some(500),
            "lost_at must stay at the first stamp"
        );
        assert_eq!(
            lost_reason_of(&s, a),
            Some("host_reboot".to_string()),
            "lost_reason must stay the FIRST reason, not be overwritten by the second call"
        );
    }

    /// Force a row into a ghost state straight on the connection, as a
    /// prior pass (Phase 1 / a verdict / a kill) would have left it.
    fn set_ghost(s: &Store, id: i64, lost_at: i64, reason: &str) {
        s.conn_ref()
            .execute(
                "UPDATE sessions SET status='ghost', lost_at=?1, lost_reason=?2 WHERE id=?3",
                rusqlite::params![lost_at, reason, id],
            )
            .unwrap();
    }

    #[test]
    fn a_verdict_reclassifies_a_missing_ghost_and_keeps_its_lost_at() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        // "a": a `missing` ghost with a claude id, left by a failed first
        // post-loss pass — must be reclassified.
        let a = s
            .upsert_session("a", "h", None, None, 1, 1, "running", None)
            .unwrap();
        s.set_claude_session_id(a, "cid-a").unwrap();
        set_ghost(&s, a, 400, "missing");
        // "k": a `missing` ghost that IS in keep — untouched.
        let k = s
            .upsert_session("k", "h", None, None, 1, 1, "running", None)
            .unwrap();
        set_ghost(&s, k, 400, "missing");
        // "r": already carries a mass-loss reason — untouched.
        let r = s
            .upsert_session("r", "h", None, None, 1, 1, "running", None)
            .unwrap();
        set_ghost(&s, r, 300, "host_reboot");
        // "f": fleet's own kill — never reclassified.
        let f = s
            .upsert_session("f", "h", None, None, 1, 1, "running", None)
            .unwrap();
        set_ghost(&s, f, 400, "killed");

        let out = s
            .mark_host_sessions_lost("h", "host_reboot", &["k".to_string()], 500, 0)
            .unwrap();
        assert!(out.marked.is_empty(), "nothing was live: {out:?}");
        assert_eq!(
            out.reclassified.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![a]
        );

        let a_row = s.get_session_by_id(a).unwrap().unwrap();
        assert_eq!(a_row.status, "ghost");
        assert_eq!(a_row.lost_at, Some(400), "reclassification keeps lost_at");
        assert_eq!(a_row.claude_session_id.as_deref(), Some("cid-a"));
        assert_eq!(lost_reason_of(&s, a), Some("host_reboot".to_string()));

        assert_eq!(lost_reason_of(&s, k), Some("missing".to_string()));
        assert_eq!(s.get_session_by_id(k).unwrap().unwrap().lost_at, Some(400));
        assert_eq!(lost_reason_of(&s, r), Some("host_reboot".to_string()));
        assert_eq!(s.get_session_by_id(r).unwrap().unwrap().lost_at, Some(300));
        assert_eq!(lost_reason_of(&s, f), Some("killed".to_string()));
    }

    #[test]
    fn reclassifying_a_missing_ghost_emits_an_update() {
        // `lost_reason` is on the wire now, so upgrading a `missing` ghost's
        // reason to a verdict must announce it — the sidebar's "host
        // rebooted" label depends on this event, not just a future refetch.
        let (s, bus) = store_with_recorder();
        s.upsert_host("h").unwrap();
        let a = s
            .upsert_session("a", "h", None, None, 1, 1, "running", None)
            .unwrap();
        set_ghost(&s, a, 400, "missing");
        bus.take(); // drain the create/ghost setup

        let out = s
            .mark_host_sessions_lost("h", "host_reboot", &[], 500, 0)
            .unwrap();
        assert_eq!(
            out.reclassified.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![a]
        );

        let evts = bus.take();
        assert!(
            evts.contains(&format!("session:updated:{a}")),
            "reclassification must emit session:updated; got {evts:?}"
        );
        assert_eq!(
            s.get_session_by_id(a)
                .unwrap()
                .unwrap()
                .lost_reason
                .as_deref(),
            Some("host_reboot")
        );
    }

    #[test]
    fn a_tmux_server_verdict_reclassifies_tmux_ghosts_only() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        let t = s
            .upsert_session("t", "h", None, None, 1, 1, "running", None)
            .unwrap();
        set_ghost(&s, t, 400, "missing");
        let bg = s
            .upsert_bg_session("h", "bg:u1", None, "u1", Some("working"), 1, "bg")
            .unwrap();
        set_ghost(&s, bg, 400, "missing");

        let out = s
            .mark_host_sessions_lost("h", "tmux_server_gone", &[], 500, 0)
            .unwrap();
        assert_eq!(
            out.reclassified.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![t]
        );
        assert_eq!(lost_reason_of(&s, t), Some("tmux_server_gone".to_string()));
        assert_eq!(lost_reason_of(&s, bg), Some("missing".to_string()));
    }

    #[test]
    fn a_reclassification_honours_the_stale_probe_guard() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        let a = s
            .upsert_session("a", "h", None, None, 1, 1, "running", None)
            .unwrap();
        set_ghost(&s, a, 400, "missing");
        // A newer pass saw it (stamped at 1000) after this probe began (900).
        s.mark_sessions_reconciled("h", &["a".to_string()], 1000)
            .unwrap();
        let out = s
            .mark_host_sessions_lost("h", "host_reboot", &[], 950, 900)
            .unwrap();
        assert!(out.is_empty(), "{out:?}");
        assert_eq!(lost_reason_of(&s, a), Some("missing".to_string()));
    }

    #[test]
    fn mark_session_killed_ghosts_the_row_as_an_ordinary_loss() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        let id = s
            .upsert_session("x", "h", None, None, 1, 1, "running", None)
            .unwrap();
        s.set_claude_session_id(id, "cid-x").unwrap();

        let row = s.mark_session_killed(id, 700).unwrap().expect("row");
        assert_eq!(row.status, "ghost");
        assert_eq!(row.lost_at, Some(700));
        assert_eq!(lost_reason_of(&s, id), Some("killed".to_string()));
        // Idempotent: an already-ghost row is not re-stamped.
        assert!(s.mark_session_killed(id, 900).unwrap().is_none());
        assert_eq!(s.get_session_by_id(id).unwrap().unwrap().lost_at, Some(700));
        // A later verdict neither marks nor reclassifies it.
        let out = s
            .mark_host_sessions_lost("h", "tmux_server_gone", &[], 800, 0)
            .unwrap();
        assert!(out.is_empty(), "{out:?}");
        assert_eq!(lost_reason_of(&s, id), Some("killed".to_string()));
    }

    #[test]
    fn a_reboot_marks_background_agents_lost_too() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        // Same seed as the tmux-restart case: one tmux row + one bg row.
        let tmux_id = s
            .upsert_session("work-a", "h", None, None, 1, 1, "running", None)
            .unwrap();
        let bg_id = s
            .upsert_bg_session("h", "bg:u1", None, "u1", Some("working"), 1, "bg")
            .unwrap();

        s.mark_host_sessions_lost("h", "host_reboot", &[], 500, 0)
            .unwrap();

        assert_eq!(
            s.get_session_by_id(tmux_id).unwrap().unwrap().status,
            "ghost"
        );
        assert_eq!(
            s.get_session_by_id(bg_id).unwrap().unwrap().status,
            "ghost",
            "a reboot kills bg agents too"
        );
        assert_eq!(lost_reason_of(&s, bg_id), Some("host_reboot".to_string()));
    }

    #[test]
    fn phase_one_ghosting_records_missing_and_a_resurrection_clears_it() {
        let mut s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        let id = s
            .upsert_session("a", "h", None, None, 1, 1, "running", None)
            .unwrap();

        // apply_host_reconcile with an empty keep ⇒ the row is ghost with
        // lost_reason 'missing'.
        s.apply_host_reconcile(empty_probe("h", 100)).unwrap();
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.status, "ghost");
        assert_eq!(lost_reason_of(&s, id), Some("missing".to_string()));

        // Upserting it live again (the tmux session reappears) ⇒ lost_reason NULL.
        s.apply_host_reconcile(HostReconcile {
            sessions: &[ReconcileSession {
                tmux_name: "a",
                created_at: 1,
                last_activity_at: 2,
                ..Default::default()
            }],
            keep: &["a".to_string()],
            ..empty_probe("h", 200)
        })
        .unwrap();
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.status, "running");
        assert_eq!(row.lost_at, None);
        assert_eq!(lost_reason_of(&s, id), None);
    }

    #[test]
    fn dismiss_agent_records_and_deletes_the_row() {
        let s = store();
        s.upsert_bg_session("local", "bg:u2", None, "u2", Some("stopped"), 1, "bg")
            .unwrap();
        s.dismiss_agent("local", "u2", 100).unwrap();
        assert!(s.get_session("bg:u2", "local").unwrap().is_none());
        assert_eq!(s.dismissed_agents("local").unwrap().get("u2"), Some(&100));
        assert!(s.dismissed_agents("other").unwrap().is_empty());
        s.clear_agent_dismissal("local", "u2").unwrap();
        assert!(s.dismissed_agents("local").unwrap().is_empty());
    }

    #[test]
    fn ghost_and_clean_bg_sessions_two_phase_with_event_cleanup() {
        let (store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        store.upsert_host("beta").unwrap();
        bus.take();
        let id = store
            .upsert_bg_session("alpha", "bg:u1", None, "u1", Some("working"), 100, "bg")
            .unwrap();
        store
            .insert_session_event(id, "status_change", None)
            .unwrap();
        // A bg row on ANOTHER host must never be touched.
        let other = store
            .upsert_bg_session("beta", "bg:u9", None, "u9", Some("working"), 100, "bg")
            .unwrap();
        bus.take();

        // Pass 1: agent vanished → row is ghosted (soft), not deleted.
        store
            .ghost_and_clean_bg_sessions("alpha", &[], 200, None)
            .unwrap();
        let row = store.get_session_by_id(id).unwrap().expect("still present");
        assert_eq!(row.status, "ghost");
        assert_eq!(row.lost_at, Some(200));
        assert!(bus.take().contains(&format!("session:updated:{id}")));

        // Pass 2: still vanished → hard-deleted, events reaped, kill emitted.
        store
            .ghost_and_clean_bg_sessions("alpha", &[], 300, None)
            .unwrap();
        assert!(store.get_session_by_id(id).unwrap().is_none());
        let orphans: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM session_events WHERE session_id=?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphans, 0, "events must not outlive the row");
        assert!(bus.take().contains(&format!("session:killed:{id}")));

        // The other host's bg row is untouched throughout.
        let other_row = store.get_session_by_id(other).unwrap().expect("beta row");
        assert_eq!(other_row.status, "running");
    }

    #[test]
    fn ghost_and_clean_bg_sessions_keeps_listed_agents_and_tmux_rows() {
        let mut store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        let kept = store
            .upsert_bg_session("alpha", "bg:live", None, "live", Some("working"), 100, "bg")
            .unwrap();
        // A normal tmux-backed row — ghosted or not, the bg pruner must skip it.
        store
            .upsert_session("work-a", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store.apply_host_reconcile(empty_probe("alpha", 1)).unwrap(); // ghosts work-a

        let keep = vec!["bg:live".to_string()];
        store
            .ghost_and_clean_bg_sessions("alpha", &keep, 200, None)
            .unwrap();
        store
            .ghost_and_clean_bg_sessions("alpha", &keep, 300, None)
            .unwrap();

        let rows = store.list_sessions_for_host("alpha").unwrap();
        let live = rows.iter().find(|r| r.tmux_name == "bg:live").unwrap();
        assert_eq!(live.status, "running", "listed agent's row stays live");
        let work = rows.iter().find(|r| r.tmux_name == "work-a").unwrap();
        assert_eq!(
            work.status, "ghost",
            "tmux row is left for the tmux-keyed cleanup, not deleted here"
        );
        assert!(store.get_session_by_id(kept).unwrap().is_some());
    }

    #[test]
    fn upsert_bg_session_resurrects_ghosted_row() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("alpha").unwrap();
        let id = s
            .upsert_bg_session("alpha", "bg:u1", None, "u1", Some("working"), 100, "bg")
            .unwrap();
        s.ghost_and_clean_bg_sessions("alpha", &[], 200, None)
            .unwrap();
        assert_eq!(s.get_session_by_id(id).unwrap().unwrap().status, "ghost");

        // Agent reappears (e.g. the previous probe transiently failed).
        let id2 = s
            .upsert_bg_session("alpha", "bg:u1", None, "u1", Some("working"), 300, "bg")
            .unwrap();
        assert_eq!(id2, id, "same row, not a new one");
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.status, "running");
        assert_eq!(row.lost_at, None);
    }

    #[test]
    fn upsert_bg_session_resurrection_also_clears_lost_reason() {
        // The pane-less pruner (ghost_and_clean, shared with Phase 1) now
        // stamps lost_reason='missing' on ghosting. A live row must never
        // carry a stale reason once the agent reappears — PR 2 will surface
        // lost_reason on the wire, and a "running" row with a leftover
        // reason would be a lie.
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("alpha").unwrap();
        let id = s
            .upsert_bg_session("alpha", "bg:u1", None, "u1", Some("working"), 100, "bg")
            .unwrap();
        s.ghost_and_clean_bg_sessions("alpha", &[], 200, None)
            .unwrap();
        assert_eq!(
            lost_reason_of(&s, id),
            Some("missing".to_string()),
            "precondition: the ghost carries a reason"
        );

        s.upsert_bg_session("alpha", "bg:u1", None, "u1", Some("working"), 300, "bg")
            .unwrap();
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.status, "running");
        assert_eq!(
            lost_reason_of(&s, id),
            None,
            "a revived bg row must not keep the old lost_reason"
        );
    }

    #[test]
    fn bg_reconcile_hard_delete_reaps_timeline_and_inbox_but_keeps_sent_messages() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        let bg = store
            .upsert_bg_session("alpha", "bg:gone", None, "uuid-gone", Some("idle"), 1, "bg")
            .unwrap();
        let peer = store
            .upsert_session("peer", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store
            .insert_session_event(bg, "status_change", Some("idle"))
            .unwrap();
        store
            .insert_message(peer, bg, "to the gone bg", "message", None)
            .unwrap();
        store
            .insert_message(bg, peer, "from the gone bg", "message", None)
            .unwrap();
        // Two passes without the agent: ghost, then hard-delete.
        store
            .ghost_and_clean_bg_sessions("alpha", &[], 10, None)
            .unwrap();
        assert!(
            store.get_session_by_id(bg).unwrap().is_some(),
            "ghosted first"
        );
        store
            .ghost_and_clean_bg_sessions("alpha", &[], 20, None)
            .unwrap();
        assert!(store.get_session_by_id(bg).unwrap().is_none());
        assert!(
            store.list_session_events(bg, 10).unwrap().is_empty(),
            "the timeline goes with the row"
        );
        assert!(
            store.list_inbox(bg, false, 10).unwrap().is_empty(),
            "an inbox nobody can read goes with the row"
        );
        assert_eq!(
            store.list_inbox(peer, false, 10).unwrap().len(),
            1,
            "a message the gone bg session SENT stays in the recipient's inbox"
        );
        assert!(
            store.get_session_by_id(peer).unwrap().is_some(),
            "the bg pruner never touches tmux sessions"
        );
    }

    #[test]
    fn upsert_and_list_sessions_roundtrip() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("dev-foo", "local", None, None, 1000, 2000, "running", None)
            .unwrap();
        assert!(id > 0);
        let id2 = s
            .upsert_session("dev-foo", "local", None, None, 1000, 3000, "running", None)
            .unwrap();
        assert_eq!(id, id2);
        let rows = s.list_sessions_for_host("local").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].last_activity_at, 3000);
    }

    #[test]
    fn sessions_prune_removes_stale_rows() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        s.upsert_session("dev-a", "local", None, None, 1, 1, "running", None)
            .unwrap();
        s.upsert_session("dev-b", "local", None, None, 1, 1, "running", None)
            .unwrap();
        s.upsert_session("dev-c", "local", None, None, 1, 1, "running", None)
            .unwrap();
        let removed = s
            .delete_sessions_not_in("local", &["dev-a".to_string()])
            .unwrap();
        assert_eq!(removed, 2);
        assert_eq!(s.list_sessions_for_host("local").unwrap().len(), 1);
    }

    #[test]
    fn deleting_a_reviewed_source_nulls_the_review_link_not_errors() {
        // Self-FK uses ON DELETE SET NULL: deleting a source session that a
        // review still points at must succeed (link nulls), not fail the FK.
        let store = Store::open_in_memory().expect("store");
        store.upsert_host("alpha").unwrap();
        let src = store
            .upsert_session("src", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        let rev = store
            .upsert_session("src--review-1", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store.set_session_kind(rev, "review", Some(src)).unwrap();
        // Delete the source while the review still references it.
        store
            .delete_session(src)
            .expect("delete source must not trip the self-FK");
        let row = store
            .list_sessions_for_host("alpha")
            .unwrap()
            .into_iter()
            .find(|r| r.tmux_name == "src--review-1")
            .unwrap();
        assert_eq!(
            row.reviews_session_id, None,
            "link should be nulled by ON DELETE SET NULL"
        );
        assert_eq!(row.kind, "review", "the review row itself survives");
    }

    #[test]
    fn get_session_account_returns_none_for_missing_then_some_after_upsert() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        // No session yet → None
        assert!(s.get_session_account("h", "dev-foo").unwrap().is_none());
        // Upsert with an account uuid
        s.upsert_account(&AccountRow {
            uuid: "u1".into(),
            ..Default::default()
        })
        .unwrap();
        s.upsert_session("dev-foo", "h", None, None, 1, 1, "running", Some("u1"))
            .unwrap();
        assert_eq!(
            s.get_session_account("h", "dev-foo").unwrap().as_deref(),
            Some("u1")
        );
    }

    #[test]
    fn related_matches_same_project_and_worktree_key() {
        let store = Store::open_in_memory().expect("store");
        store.upsert_host("local").unwrap();
        let pid = store.upsert_project("o", "r", "/tmp/r").unwrap();
        let a = store
            .upsert_session("a", "local", Some(pid), None, 1, 1, "running", None)
            .unwrap();
        let b = store
            .upsert_session("b", "local", Some(pid), None, 1, 1, "running", None)
            .unwrap();
        store.set_worktree_key(a, Some("main")).unwrap();
        store.set_worktree_key(b, Some("main")).unwrap();
        let r = store.list_related_sessions(a).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].tmux_name, "b");
    }

    #[test]
    fn related_excludes_different_worktree_key() {
        let store = Store::open_in_memory().expect("store");
        store.upsert_host("local").unwrap();
        let pid = store.upsert_project("o", "r", "/tmp/r").unwrap();
        let a = store
            .upsert_session("a", "local", Some(pid), None, 1, 1, "running", None)
            .unwrap();
        let b = store
            .upsert_session("b", "local", Some(pid), None, 1, 1, "running", None)
            .unwrap();
        store.set_worktree_key(a, Some("main")).unwrap();
        store.set_worktree_key(b, Some("feat-x")).unwrap();
        assert!(store.list_related_sessions(a).unwrap().is_empty());
    }

    #[test]
    fn related_matches_across_hosts_same_key() {
        let store = Store::open_in_memory().expect("store");
        store.upsert_host("local").unwrap();
        store.upsert_host("mefistos").unwrap();
        let pid = store.upsert_project("o", "r", "/tmp/r").unwrap();
        let a = store
            .upsert_session("a", "local", Some(pid), None, 1, 1, "running", None)
            .unwrap();
        let b = store
            .upsert_session("b", "mefistos", Some(pid), None, 1, 1, "running", None)
            .unwrap();
        store.set_worktree_key(a, Some("main")).unwrap();
        store.set_worktree_key(b, Some("main")).unwrap();
        let r = store.list_related_sessions(a).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].host_alias, "mefistos");
    }

    #[test]
    fn related_returns_empty_for_null_key() {
        let store = Store::open_in_memory().expect("store");
        store.upsert_host("local").unwrap();
        let pid = store.upsert_project("o", "r", "/tmp/r").unwrap();
        let a = store
            .upsert_session("a", "local", Some(pid), None, 1, 1, "running", None)
            .unwrap();
        let _b = store
            .upsert_session("b", "local", Some(pid), None, 1, 1, "running", None)
            .unwrap();
        assert!(store.list_related_sessions(a).unwrap().is_empty());
    }

    #[test]
    fn list_related_sessions_excludes_orphans() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        let a = s
            .upsert_session("dev-a", "h", None, None, 1, 1, "running", None)
            .unwrap();
        let _b = s
            .upsert_session("dev-b", "h", None, None, 1, 1, "running", None)
            .unwrap();
        let related = s.list_related_sessions(a).unwrap();
        assert!(
            related.is_empty(),
            "orphans should not match each other; got: {:?}",
            related.iter().map(|r| &r.tmux_name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn upsert_session_emits_created_then_updated() {
        let (store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        bus.take(); // drain host:added
        store
            .upsert_session("s1", "alpha", None, None, 100, 100, "running", None)
            .unwrap();
        store
            .upsert_session("s1", "alpha", None, None, 100, 200, "running", None)
            .unwrap();
        let evts = bus.take();
        assert_eq!(
            evts.len(),
            2,
            "expected one created + one updated, got {evts:?}"
        );
        assert!(evts[0].starts_with("session:created:"), "got: {}", evts[0]);
        assert!(evts[1].starts_with("session:updated:"), "got: {}", evts[1]);
    }

    #[test]
    fn delete_session_emits_killed() {
        let (store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        bus.take(); // drain host:added
        store
            .upsert_session("s1", "alpha", None, None, 100, 100, "running", None)
            .unwrap();
        let id = store.get_session("s1", "alpha").unwrap().expect("row").id;
        bus.take(); // drain created event
        store.delete_session(id).unwrap();
        let evts = bus.take();
        assert_eq!(evts.len(), 1);
        assert_eq!(evts[0], format!("session:killed:{id}"));
    }

    #[test]
    fn delete_session_reaps_timeline_and_inbox_but_keeps_sent_messages_and_tasks() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        let dead = store
            .upsert_session("dead", "alpha", None, None, 1, 1, "ghost", None)
            .unwrap();
        let peer = store
            .upsert_session("peer", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store
            .insert_session_event(dead, "status_change", Some("idle"))
            .unwrap();
        store
            .insert_session_event(peer, "status_change", Some("idle"))
            .unwrap();
        store
            .insert_message(peer, dead, "to the dead", "message", None)
            .unwrap();
        store
            .insert_message(dead, peer, "from the dead", "message", None)
            .unwrap();
        let task = store.insert_task(Some(peer), Some(dead), "p", "n").unwrap();

        store.delete_session(dead).unwrap();

        assert!(store.list_session_events(dead, 10).unwrap().is_empty());
        assert!(store.list_inbox(dead, false, 10).unwrap().is_empty());
        assert_eq!(store.list_session_events(peer, 10).unwrap().len(), 1);
        assert_eq!(
            store.list_inbox(peer, false, 10).unwrap().len(),
            1,
            "a message the dead session SENT stays in the recipient's inbox"
        );
        assert!(
            store.get_task(task.id).unwrap().is_some(),
            "the task sweep needs the task to fail it"
        );
    }

    #[test]
    fn delete_sessions_not_in_emits_killed_per_row() {
        let (store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        bus.take(); // drain host:added
        store
            .upsert_session("s1", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store
            .upsert_session("s2", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store
            .upsert_session("s3", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        bus.take(); // drain creates
        store
            .delete_sessions_not_in("alpha", &["s2".to_string()])
            .unwrap();
        let evts = bus.take();
        assert_eq!(evts.len(), 2, "expected 2 killed (s1, s3), got {evts:?}");
        assert!(evts.iter().all(|e| e.starts_with("session:killed:")));
    }

    #[test]
    fn restore_session_clears_ghost_status_and_lost_at() {
        let (store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        store
            .upsert_session("s1", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        let id = store.get_session("s1", "alpha").unwrap().unwrap().id;
        // Manually ghost it
        store
            .conn
            .execute(
                "UPDATE sessions SET status='ghost', lost_at=999 WHERE id=?1",
                rusqlite::params![id],
            )
            .unwrap();
        bus.take(); // drain

        let row = store.restore_session(id).unwrap().expect("row must exist");
        assert_eq!(row.status, "running");
        assert_eq!(row.lost_at, None);

        let evts = bus.take();
        assert!(
            evts.iter().any(|e| e.starts_with("session:updated:")),
            "restore must emit session:updated; got: {evts:?}"
        );
    }

    #[test]
    fn restore_session_also_clears_lost_reason() {
        // `recreate_session` calls this after rebuilding the tmux session on
        // the host. A row lost with a recorded reason (e.g. `host_reboot`)
        // must not keep carrying it once it's manually restored — a live row
        // carries no reason.
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("alpha").unwrap();
        let id = s
            .upsert_session("s1", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        s.mark_host_sessions_lost("alpha", "host_reboot", &[], 999, 0)
            .unwrap();
        assert_eq!(
            lost_reason_of(&s, id),
            Some("host_reboot".to_string()),
            "precondition: the ghost carries a reason"
        );

        let row = s.restore_session(id).unwrap().expect("row must exist");
        assert_eq!(row.status, "running");
        assert_eq!(row.lost_at, None);
        assert_eq!(
            lost_reason_of(&s, id),
            None,
            "a manually restored row must not keep the old lost_reason"
        );
    }

    #[test]
    fn lost_reason_is_on_the_row() {
        // PR 2: `lost_reason` is now a `SessionRow` field, not just a raw
        // column — assert on it directly, the way the frontend will read it
        // off the wire.
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        let id = s
            .upsert_session("a", "h", None, None, 1, 1, "running", None)
            .unwrap();
        s.set_claude_session_id(id, "abc").unwrap();

        s.mark_host_sessions_lost("h", "host_reboot", &[], 500, 0)
            .unwrap();
        assert_eq!(
            s.get_session_by_id(id)
                .unwrap()
                .unwrap()
                .lost_reason
                .as_deref(),
            Some("host_reboot")
        );

        s.restore_session(id).unwrap();
        assert_eq!(
            s.get_session_by_id(id).unwrap().unwrap().lost_reason,
            None,
            "a restored row carries no lost_reason"
        );
    }

    #[test]
    fn set_session_kind_marks_review_and_survives_reupsert() {
        let store = Store::open_in_memory().expect("store");
        store.upsert_host("alpha").unwrap();
        let src = store
            .upsert_session("src", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        let rev = store
            .upsert_session("src--review-1", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store.set_session_kind(rev, "review", Some(src)).unwrap();
        store
            .upsert_session("src--review-1", "alpha", None, None, 1, 2, "running", None)
            .unwrap();
        let row = store
            .list_sessions_for_host("alpha")
            .unwrap()
            .into_iter()
            .find(|r| r.tmux_name == "src--review-1")
            .unwrap();
        assert_eq!(row.kind, "review", "kind must survive re-upsert");
        assert_eq!(row.reviews_session_id, Some(src));
    }

    #[test]
    fn claude_session_id_round_trips_and_defaults_none() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        s.upsert_session("dev", "local", None, None, 1, 1, "running", None)
            .unwrap();
        let id = s.get_session("dev", "local").unwrap().unwrap().id;
        assert_eq!(
            s.get_session_by_id(id).unwrap().unwrap().claude_session_id,
            None
        );
        s.set_claude_session_id(id, "550e8400-e29b-41d4-a716-446655440000")
            .unwrap();
        assert_eq!(
            s.get_session_by_id(id)
                .unwrap()
                .unwrap()
                .claude_session_id
                .as_deref(),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
    }

    #[test]
    fn upsert_session_preserves_claude_session_id() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        s.upsert_session("dev", "local", None, None, 1, 1, "running", None)
            .unwrap();
        let id = s.get_session("dev", "local").unwrap().unwrap().id;
        s.set_claude_session_id(id, "550e8400-e29b-41d4-a716-446655440000")
            .unwrap();
        s.upsert_session("dev", "local", None, None, 1, 2, "running", None)
            .unwrap();
        assert_eq!(
            s.get_session_by_id(id)
                .unwrap()
                .unwrap()
                .claude_session_id
                .as_deref(),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
    }

    #[test]
    fn upsert_session_preserves_friendly_name_on_conflict() {
        // Regression guard for the "deterministic backup, agent refines"
        // design: once a friendly_name is set, a subsequent reconcile-driven
        // upsert must NOT NULL it back out.
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        s.upsert_session("dev", "local", None, None, 1, 1, "running", None)
            .unwrap();
        s.set_friendly_name("local", "dev", Some("Friendly name"))
            .unwrap();
        s.upsert_session("dev", "local", None, None, 1, 2, "running", None)
            .unwrap();
        assert_eq!(
            s.get_session("dev", "local")
                .unwrap()
                .unwrap()
                .friendly_name
                .as_deref(),
            Some("Friendly name")
        );
    }

    #[test]
    fn friendly_name_defaults_skip_pane_less_rows() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        for (name, kind) in [("bg:u-bg", "bg"), ("bg:u-ext", "external")] {
            s.upsert_bg_session("local", name, None, &name[3..], Some("idle"), 1, kind)
                .unwrap();
        }
        assert_eq!(s.backfill_friendly_names().unwrap(), 0);
        for name in ["bg:u-bg", "bg:u-ext"] {
            let row = s.get_session(name, "local").unwrap().unwrap();
            assert_eq!(row.friendly_name, None, "{name}");
            assert_eq!(s.default_friendly_name(row.id).unwrap(), None, "{name}");
        }
    }

    #[test]
    fn has_no_pane_covers_bg_and_external_only() {
        assert!(crate::store::has_no_pane("bg"));
        assert!(crate::store::has_no_pane("external"));
        for kind in ["work", "shell", "review", ""] {
            assert!(!crate::store::has_no_pane(kind), "{kind}");
        }
    }

    #[test]
    fn backfill_friendly_names_humanises_null_rows() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        // Project + worktree so the JOIN finds owner/repo and a branch.
        let project_id = s
            .upsert_project("martin-janci", "claude-fleet", "/p")
            .unwrap();
        let wt_id = s
            .upsert_worktree(
                project_id,
                "friendly-name",
                "/p/.claude/worktrees/friendly-name",
                Some("friendly-name"),
            )
            .unwrap();
        s.upsert_session(
            "dev-martin-janci-claude-fleet--friendly-name",
            "local",
            Some(project_id),
            Some(wt_id),
            1,
            1,
            "running",
            None,
        )
        .unwrap();
        // A row that already has a label must NOT be touched.
        s.upsert_session(
            "dev-other",
            "local",
            Some(project_id),
            None,
            1,
            1,
            "running",
            None,
        )
        .unwrap();
        s.set_friendly_name("local", "dev-other", Some("Already set"))
            .unwrap();

        let updated = s.backfill_friendly_names().unwrap();
        assert_eq!(updated, 1);
        assert_eq!(
            s.get_session("dev-martin-janci-claude-fleet--friendly-name", "local")
                .unwrap()
                .unwrap()
                .friendly_name
                .as_deref(),
            Some("Friendly name")
        );
        assert_eq!(
            s.get_session("dev-other", "local")
                .unwrap()
                .unwrap()
                .friendly_name
                .as_deref(),
            Some("Already set")
        );
    }

    #[test]
    fn tags_round_trip_as_a_json_array_and_null_when_empty() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("sess", "local", None, None, 1, 1, "running", None)
            .unwrap();
        let row = s
            .set_session_tags(id, &["review".to_string(), "wip".to_string()])
            .unwrap()
            .unwrap();
        assert_eq!(row.tags, vec!["review".to_string(), "wip".to_string()]);
        let raw: Option<String> = s
            .conn
            .query_row("SELECT tags FROM sessions WHERE id=?1", [id], |r| r.get(0))
            .unwrap();
        assert_eq!(raw.as_deref(), Some("[\"review\",\"wip\"]"));
        let row = s.set_session_tags(id, &[]).unwrap().unwrap();
        assert!(row.tags.is_empty());
        let raw: Option<String> = s
            .conn
            .query_row("SELECT tags FROM sessions WHERE id=?1", [id], |r| r.get(0))
            .unwrap();
        assert_eq!(raw, None, "an empty list is stored as NULL");
        // A hand-edited / malformed column reads as no tags rather than failing.
        assert!(decode_tags(Some("not json".into())).is_empty());
        assert!(decode_tags(Some("".into())).is_empty());
        assert_eq!(decode_tags(Some("[\"a\"]".into())), vec!["a".to_string()]);
        assert_eq!(encode_tags(&[]), None);
        // A reconcile pass does not touch tags / parent / turn_seq.
        s.set_session_tags(id, &["keep".to_string()]).unwrap();
        s.set_parent_session_id(id, Some(7)).unwrap();
        s.set_claude_session_id(id, "uuid-1").unwrap();
        s.record_stop_hook("uuid-1").unwrap();
        let mut s = s;
        let row = reconcile_one(&mut s, "sess", Some("idle"), None, None);
        assert_eq!(row.tags, vec!["keep".to_string()]);
        assert_eq!(row.parent_session_id, Some(7));
        assert_eq!(row.turn_seq, 1);
    }

    #[test]
    fn stop_hook_write_bumps_turn_seq_and_prompt_submit_marks_working() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("sess", "local", None, None, 1, 1, "running", None)
            .unwrap();
        // Unmatched id: None, nothing changes.
        assert!(s.record_stop_hook("nope").unwrap().is_none());
        assert!(s.record_prompt_submit_hook("nope").unwrap().is_none());
        s.set_claude_session_id(id, "uuid-1").unwrap();
        let row = s.record_stop_hook("uuid-1").unwrap().unwrap();
        assert_eq!(row.turn_seq, 1);
        assert_eq!(row.claude_status.as_deref(), Some("idle"));
        let stop = row.last_stop_at.expect("stamped");
        assert_eq!(row.last_turn_at, Some(stop));
        assert!(row.idle_since.is_some());
        let row = s.record_prompt_submit_hook("uuid-1").unwrap().unwrap();
        assert_eq!(row.claude_status.as_deref(), Some("working"));
        assert_eq!(row.idle_since, None);
        assert_eq!(row.turn_seq, 1, "a submit is not a turn");
        assert_eq!(row.last_stop_at, Some(stop), "a submit keeps the last stop");
        let row = s.record_stop_hook("uuid-1").unwrap().unwrap();
        assert_eq!(row.turn_seq, 2);
    }

    #[test]
    fn stop_hook_status_write_stamps_last_turn_and_idle_since() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("sess", "local", None, None, 1, 1, "running", None)
            .unwrap();
        s.set_claude_session_id(id, "uuid-1").unwrap();
        s.set_claude_status_by_session_id("uuid-1", "idle").unwrap();
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert!(row.last_turn_at.is_some());
        let idle = row.idle_since.expect("idle stamped by the hook");
        s.set_claude_status_by_session_id("uuid-1", "working")
            .unwrap();
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.idle_since, None);
        s.set_claude_status_by_session_id("uuid-1", "idle").unwrap();
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert!(row.idle_since.unwrap() >= idle);
    }

    #[test]
    fn bg_upsert_maintains_idle_since_from_agent_status() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        s.upsert_bg_session("local", "bg:u1", None, "u1", Some("working"), 1, "bg")
            .unwrap();
        assert_eq!(
            s.get_session("bg:u1", "local").unwrap().unwrap().idle_since,
            None
        );
        s.upsert_bg_session("local", "bg:u1", None, "u1", Some("completed"), 2, "bg")
            .unwrap();
        let stamp = s
            .get_session("bg:u1", "local")
            .unwrap()
            .unwrap()
            .idle_since
            .expect("stamped");
        s.upsert_bg_session("local", "bg:u1", None, "u1", Some("completed"), 3, "bg")
            .unwrap();
        assert_eq!(
            s.get_session("bg:u1", "local").unwrap().unwrap().idle_since,
            Some(stamp)
        );
        s.upsert_bg_session("local", "bg:u1", None, "u1", Some("working"), 4, "bg")
            .unwrap();
        assert_eq!(
            s.get_session("bg:u1", "local").unwrap().unwrap().idle_since,
            None
        );
    }

    #[test]
    fn last_prompt_is_truncated_started_at_set_once_and_playbook_stamp_emits() {
        let (s, bus) = store_with_recorder();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("sess", "local", None, None, 1, 1, "running", None)
            .unwrap();
        let long: String = "x".repeat(LAST_PROMPT_CHARS + 50);
        let row = s.set_last_prompt(id, &long).unwrap().unwrap();
        assert_eq!(
            row.last_prompt.as_deref().map(|p| p.chars().count()),
            Some(LAST_PROMPT_CHARS)
        );

        s.set_started_at(id, 100).unwrap();
        s.set_started_at(id, 200).unwrap();
        assert_eq!(
            s.get_session_by_id(id).unwrap().unwrap().started_at,
            Some(100)
        );

        let _ = bus.take();
        let row = s
            .mark_playbook_applied(id, 555, "oom:recreate")
            .unwrap()
            .unwrap();
        assert_eq!(row.last_playbook_at, Some(555));
        assert_eq!(bus.take(), vec![format!("session:updated:{id}")]);
        let events = s.list_session_events(id, 10).unwrap();
        assert!(events
            .iter()
            .any(|e| e.kind == "playbook_applied" && e.detail.as_deref() == Some("oom:recreate")));
    }

    // ---- hook writes: SessionEnd / StopFailure / Notification ----

    fn hooked_session(s: &Store) -> i64 {
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("sess", "local", None, None, 0, 0, "running", None)
            .unwrap();
        s.set_claude_session_id(id, "uuid-h").unwrap();
        id
    }

    fn last_hook_at(s: &Store) -> Option<i64> {
        s.conn
            .query_row(
                "SELECT last_hook_at FROM sessions WHERE claude_session_id='uuid-h'",
                [],
                |r| r.get::<_, Option<i64>>(0),
            )
            .unwrap()
    }

    #[test]
    fn record_session_end_hook_marks_stopped_and_clears_stuck() {
        use crate::service::pane_intel::{ClaudeStatus, StuckKind};
        let s = Store::open_in_memory().unwrap();
        let id = hooked_session(&s);
        s.record_notification_hook(
            "uuid-h",
            ClaudeStatus::Blocked,
            Some(Some(StuckKind::PressEnter)),
        )
        .unwrap();
        let row = s
            .record_session_end_hook("uuid-h")
            .unwrap()
            .expect("matched");
        assert_eq!(row.claude_status.as_deref(), Some("stopped"));
        assert!(row.idle_since.is_some());
        assert!(row.stuck_kind.is_none() && row.stuck_since.is_none());
        assert!(last_hook_at(&s).is_some());
        assert_eq!(row.id, id);
        assert!(s.record_session_end_hook("nope").unwrap().is_none());
    }

    #[test]
    fn record_stop_failure_hook_writes_like_stop() {
        let s = Store::open_in_memory().unwrap();
        hooked_session(&s);
        let a = s.record_stop_failure_hook("uuid-h").unwrap().unwrap();
        assert_eq!(a.claude_status.as_deref(), Some("idle"));
        assert_eq!(a.turn_seq, 1);
        assert_eq!(a.last_stop_at, a.last_turn_at);
        let b = s.record_stop_failure_hook("uuid-h").unwrap().unwrap();
        assert_eq!(b.turn_seq, 2);
    }

    #[test]
    fn record_notification_hook_maps_status_and_stuck_episodes() {
        use crate::service::pane_intel::{ClaudeStatus, StuckKind};
        let s = Store::open_in_memory().unwrap();
        hooked_session(&s);
        // blocked, stuck untouched (None) → stays NULL
        let r = s
            .record_notification_hook("uuid-h", ClaudeStatus::Blocked, None)
            .unwrap()
            .unwrap();
        assert_eq!(r.claude_status.as_deref(), Some("blocked"));
        assert!(r.stuck_kind.is_none());
        assert!(r.idle_since.is_none(), "blocked is not idle");
        // set press_enter → episode starts
        let r = s
            .record_notification_hook(
                "uuid-h",
                ClaudeStatus::Blocked,
                Some(Some(StuckKind::PressEnter)),
            )
            .unwrap()
            .unwrap();
        assert_eq!(r.stuck_kind.as_deref(), Some("press_enter"));
        let since = r.stuck_since.expect("episode start");
        // same kind again → episode start kept
        let r = s
            .record_notification_hook(
                "uuid-h",
                ClaudeStatus::Blocked,
                Some(Some(StuckKind::PressEnter)),
            )
            .unwrap()
            .unwrap();
        assert_eq!(r.stuck_since, Some(since));
        // a different kind → episode restarts (same second is fine: still Some)
        let r = s
            .record_notification_hook(
                "uuid-h",
                ClaudeStatus::Blocked,
                Some(Some(StuckKind::AuthMenu)),
            )
            .unwrap()
            .unwrap();
        assert_eq!(r.stuck_kind.as_deref(), Some("auth_menu"));
        assert!(r.stuck_since.is_some());
        // None → untouched
        let r = s
            .record_notification_hook("uuid-h", ClaudeStatus::Blocked, None)
            .unwrap()
            .unwrap();
        assert_eq!(r.stuck_kind.as_deref(), Some("auth_menu"));
        // working + clear
        let r = s
            .record_notification_hook("uuid-h", ClaudeStatus::Working, Some(None))
            .unwrap()
            .unwrap();
        assert_eq!(r.claude_status.as_deref(), Some("working"));
        assert!(r.stuck_kind.is_none() && r.stuck_since.is_none());
        assert!(last_hook_at(&s).is_some());
        assert!(s
            .record_notification_hook("nope", ClaudeStatus::Blocked, None)
            .unwrap()
            .is_none());
    }
}
