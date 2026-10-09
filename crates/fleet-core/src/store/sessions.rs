//! Session rows: CRUD, field setters, ghosting and hook writes.

use super::*;

/// SQL fragment for the turn-boundary hook writes: a turn starting or
/// ending ends a `compacting` activity (PreCompact set it; spec: "the next
/// UserPromptSubmit / Stop clears it"). Any other activity is left alone.
const END_COMPACTING: &str = ", current_activity = CASE WHEN current_activity = 'compacting' \
     THEN NULL ELSE current_activity END";

/// What a sender needs to know to wait for the REPL's acknowledgement of a
/// prompt: the submit counter to watch, and whether this row has EVER been
/// stamped by a hook (a host without hooks can never ack, so the sender must
/// not wait for one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptAckState {
    pub prompt_submit_seq: i64,
    pub hooks_seen: bool,
}

/// The columns a `session:killed` frame's facts are read from, in
/// [`map_killed`] order. The org is COMPUTED (`session_org_sql!`), exactly
/// as it is on a `SessionRow`, so a frame and a row cannot disagree about
/// which company a session belonged to.
const KILLED_COLS: &str = concat!(
    "id, host_alias, visibility, owner_person_id, ",
    crate::session_org_sql!("sessions")
);

fn map_killed(r: &rusqlite::Row<'_>) -> rusqlite::Result<crate::events::SessionKilledPayload> {
    Ok(crate::events::SessionKilledPayload {
        id: r.get(0)?,
        host_alias: r.get(1)?,
        visibility: r.get(2)?,
        owner_person_id: r.get(3)?,
        org_id: r.get(4)?,
    })
}

/// The facts a `session:killed` frame must carry, for every one of `ids`
/// that is still in the table (multi-user M1, T9).
///
/// It takes a bare `&Connection` for one reason, and it is the whole point:
/// the frame fires AFTER the row is deleted, so the only moment these values
/// exist is inside the caller's own transaction, before its `DELETE`. A
/// caller that reads them afterwards gets nothing — and a payload with no
/// facts is dropped by `events_route::fence_frame` for every caller but the
/// hub's own readers, which is the fail-closed direction but also an
/// invisible row left on a phone.
///
/// An id with no row (already gone, or never there) is answered id-only
/// rather than skipped: the frame still has to be emitted, since on the
/// desktop it is what removes the row from the store.
pub(super) fn killed_payloads(
    conn: &rusqlite::Connection,
    ids: &[i64],
) -> rusqlite::Result<Vec<crate::events::SessionKilledPayload>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT {KILLED_COLS} FROM sessions WHERE id IN ({phs})",
        phs = in_clause(ids.len())
    );
    let mut stmt = conn.prepare(&sql)?;
    let found: Vec<crate::events::SessionKilledPayload> = stmt
        .query_map(rusqlite::params_from_iter(ids), map_killed)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(ids
        .iter()
        .map(|id| {
            found
                .iter()
                .find(|p| p.id == *id)
                .cloned()
                .unwrap_or_else(|| (*id).into())
        })
        .collect())
}

impl Store {
    /// [`killed_payloads`] for one id, through this store's own connection.
    /// Call it BEFORE the delete.
    pub(crate) fn killed_payload(&self, id: i64) -> crate::events::SessionKilledPayload {
        match killed_payloads(&self.conn, &[id]) {
            Ok(mut v) if !v.is_empty() => v.remove(0),
            Ok(_) => id.into(),
            Err(e) => {
                tracing::warn!(
                    session_id = id,
                    error = %e,
                    "[store] could not read a killed session's facts; the frame will be fenced out"
                );
                id.into()
            }
        }
    }

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
    /// `lost_at=NULL`), mirroring the tmux upsert's ghost revival — including
    /// its staleness guard: `probe_started_at` is when this pass's probe
    /// began, and a lost row is rewritten only by an observation newer than
    /// the loss (see the `WHERE` on the `DO UPDATE` below).
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
        probe_started_at: i64,
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
             WHERE lost_at IS NULL OR (?9 > 0 AND ?9 > lost_at)
             RETURNING id",
            idle = idle_since_sql("COALESCE(excluded.claude_status, claude_status)", "?7"),
        );
        // A lost row (ghosted by `ghost_and_clean_bg_sessions` after an empty
        // `claude agents` list, or marked by a `host_reboot` verdict) is
        // rewritten only by an observation NEWER than the loss — the same
        // guard, in the same unix seconds off the same clock, as the tmux
        // upsert's. A pass whose probe listed the agents before the loss can
        // otherwise land afterwards and resurrect a row that is really gone,
        // costing another ghost/reap cycle. Same two conventions: the losing
        // second is not newer, and `?9 <= 0` ("probe time unknown", only
        // store tests) never revives.
        let id: Option<i64> = self
            .conn
            .query_row(
                &sql,
                rusqlite::params![
                    tmux_name,
                    host_alias,
                    project_id,
                    last_activity_at,
                    claude_session_id,
                    claude_status,
                    now_unix(),
                    kind,
                    probe_started_at
                ],
                |row| row.get(0),
            )
            .optional()?;
        // No row back ⇒ the guard refused this observation; the row is
        // untouched, so there is nothing to announce either.
        let Some(id) = id else {
            return existing_id.ok_or(rusqlite::Error::QueryReturnedNoRows);
        };
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
    /// The transaction is a SAVEPOINT ([`Store::in_savepoint`]), so this
    /// also runs inside reconcile's per-host `Store::atomically`
    /// transaction; nested, its events are held until that commits.
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
        external_grace_cutoff: Option<i64>,
    ) -> Result<(), rusqlite::Error> {
        let changes = self.in_savepoint("ghost_and_clean_bg_sessions", |tx| {
            let mut changes: Vec<RowChange> = Vec::new();
            Self::ghost_and_clean(
                tx,
                host_alias,
                keep_names,
                now,
                KIND_PANE_LESS,
                None,
                lost_ttl_cutoff,
                external_grace_cutoff,
                &mut changes,
            )?;
            Ok::<_, rusqlite::Error>(changes)
        })?;
        // Emit only after the release so no event fires for a rolled-back
        // write (nested, `atomically` holds them until its own commit).
        for change in &changes {
            self.bus.emit_change(change);
        }
        Ok(())
    }

    /// One-shot: mark every non-ghost row of `host_alias` NOT in `keep_names`
    /// as lost, recording WHY (`reason`, one of `host_reboot` /
    /// `tmux_server_gone` / `missing`). Loss also clears `claude_status`,
    /// `stuck_kind`/`stuck_since`, `current_activity` and `pending_input`: a
    /// ghost has no pane to vouch for them (F4). Called by the reboot/vanished-tmux
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
    /// SAVEPOINT ([`Store::in_savepoint`], so it nests inside reconcile's
    /// per-host transaction), collect the affected rows, release, and only
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
        let out = self.in_savepoint("mark_host_sessions_lost", |tx| {
            let fetch_all = |ids: &[i64]| -> Result<Vec<SessionRow>, rusqlite::Error> {
                let mut rows = Vec::new();
                for id in ids {
                    if let Some(row) = fetch_session_by_id(tx, *id)? {
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
                "UPDATE sessions SET status='ghost', lost_at=?1, lost_reason=?2, {LOSS_CLEARS}
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
            Ok::<_, rusqlite::Error>(MarkedLost {
                marked: fetch_all(&ids)?,
                reclassified: fetch_all(&reclassified_ids)?,
            })
        })?;
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
            &format!(
                "UPDATE sessions SET status='ghost', lost_at=?1, lost_reason='killed', \
                 {LOSS_CLEARS} WHERE id=?2 AND status!='ghost'"
            ),
            rusqlite::params![now, id],
        )?;
        let row = fetch_session_by_id(&self.conn, id)?;
        if let Some(row) = &row {
            // Remember the kill by name: the reconcile the caller runs next
            // hard-deletes this (already ghost) row in its own pass, after
            // which a fleet-wide pass still carrying the name from a probe
            // that ran BEFORE the kill would find nothing to conflict with
            // and insert the dead session again.
            //
            // Recorded even when the UPDATE matched nothing, i.e. the user
            // killed a row reconcile had ALREADY ghosted: `tmux kill-session`
            // ran all the same, and such a row is reaped by the kill's own
            // pass even sooner (it was ghost before that pass began), so it
            // is the same window with a shorter fuse.
            self.note_kill(&row.host_alias, &row.tmux_name, now);
        }
        // Nothing was written — the row is gone, or was already a ghost — so
        // nothing is logged or announced. `None` is the caller's "no change
        // to report" signal.
        if changed == 0 {
            return Ok(None);
        }
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

    /// Fleet sessions by `claude_status` (NULL counted as `unknown`), with
    /// `external` and `shell` rows left out — exactly the `by_status` of
    /// `health::summarize`, counted in SQL so `/metrics` decodes no row.
    pub fn count_sessions_by_claude_status(
        &self,
    ) -> Result<std::collections::BTreeMap<String, u32>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT COALESCE(claude_status, 'unknown'), COUNT(*) FROM sessions \
             WHERE kind NOT IN ('external', 'shell') GROUP BY 1",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u32>(1)?)))?;
        rows.collect()
    }

    /// Every session named `tmux_name` (optionally on just one host), by a
    /// direct `WHERE tmux_name=?` — used by `whoami`'s resolution
    /// (`find_session_by_tmux_name_scoped`), which used to call
    /// [`Self::list_all_sessions`] and filter every row in Rust. Deliberately
    /// NOT filtered by `lost_at`: a ghosted row must still be considered (a
    /// live match is preferred over one, but two ghosts of the same name are
    /// still `E_AMBIGUOUS`, not silently invisible). Ordered like
    /// [`Self::list_all_sessions`] (`last_activity_at DESC`), so the
    /// ambiguity's candidates list is the one the old path produced.
    pub fn find_sessions_by_tmux_name(
        &self,
        tmux_name: &str,
        host_alias: Option<&str>,
    ) -> Result<Vec<SessionRow>, rusqlite::Error> {
        match host_alias {
            Some(host) => {
                let mut stmt = self.conn.prepare_cached(&format!(
                    "SELECT {SESSION_COLUMNS} FROM sessions WHERE tmux_name=?1 AND host_alias=?2 \
                     ORDER BY last_activity_at DESC"
                ))?;
                let rows = stmt.query_map(rusqlite::params![tmux_name, host], map_session_row)?;
                rows.collect()
            }
            None => {
                let mut stmt = self.conn.prepare_cached(&format!(
                    "SELECT {SESSION_COLUMNS} FROM sessions WHERE tmux_name=?1 \
                     ORDER BY last_activity_at DESC"
                ))?;
                let rows = stmt.query_map(rusqlite::params![tmux_name], map_session_row)?;
                rows.collect()
            }
        }
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

    /// Record that a person looked at the session at `now` (migration 125).
    /// Never moves the stamp backwards, so a late call from a second window
    /// cannot make a seen turn unread again. Answers whether it moved; a
    /// move emits `session_updated` (the row's unread state changed).
    pub fn touch_session_viewed(&self, id: i64, now: i64) -> Result<bool, rusqlite::Error> {
        let n = self.conn.execute(
            "UPDATE sessions SET last_viewed_at = ?1 \
             WHERE id = ?2 AND (last_viewed_at IS NULL OR last_viewed_at < ?1)",
            rusqlite::params![now, id],
        )?;
        if n > 0 {
            self.emit_session(id)?;
        }
        Ok(n > 0)
    }

    /// Set (or, with `None`, clear) what a finished turn came to (migration
    /// 129; J2, step 5.11, writes it). Refuses a value outside
    /// [`TURN_OUTCOMES`](super::TURN_OUTCOMES). Answers whether the row
    /// changed; a change emits `session_updated`.
    pub fn set_turn_outcome(
        &self,
        id: i64,
        outcome: Option<&str>,
    ) -> Result<bool, crate::ipc_error::IpcError> {
        if let Some(o) = outcome {
            if !super::TURN_OUTCOMES.contains(&o) {
                return Err(crate::ipc_error::IpcError::new(
                    crate::ipc_error::codes::E_INVALID,
                    format!(
                        "turn_outcome is one of {}, not {o:?}",
                        super::TURN_OUTCOMES.join(", ")
                    ),
                ));
            }
        }
        let n = self.conn.execute(
            "UPDATE sessions SET turn_outcome = ?1 WHERE id = ?2 AND turn_outcome IS NOT ?1",
            rusqlite::params![outcome, id],
        )?;
        if n > 0 {
            self.emit_session(id)?;
        }
        Ok(n > 0)
    }

    /// J2 (step 5.11): set what turn `turn_seq` came to, as Jev read it —
    /// only while NO hook has spoken since that turn's Stop. The write
    /// lands when the row is still on that turn (`turn_seq`), still `idle`
    /// (a Notification would have made it `blocked`, a prompt `working`, a
    /// StopFailure `failed`, a SessionEnd `stopped`), no hook stamped
    /// `last_hook_at` after the Stop's own stamp, and nothing answered the
    /// turn yet. Every hook write clears `turn_outcome`, so a hook that
    /// arrives after this wins too: hooks always win. Answers whether the
    /// row changed (a change emits `session_updated`).
    pub fn set_jev_turn_outcome(
        &self,
        id: i64,
        turn_seq: i64,
        outcome: &str,
    ) -> Result<bool, crate::ipc_error::IpcError> {
        if !super::TURN_OUTCOMES.contains(&outcome) {
            return Err(crate::ipc_error::IpcError::new(
                crate::ipc_error::codes::E_INVALID,
                format!(
                    "turn_outcome is one of {}, not {outcome:?}",
                    super::TURN_OUTCOMES.join(", ")
                ),
            ));
        }
        let n = self.conn.execute(
            "UPDATE sessions SET turn_outcome = ?1 \
             WHERE id = ?2 AND turn_seq = ?3 AND claude_status = 'idle' \
               AND turn_outcome IS NULL AND last_stop_at IS NOT NULL \
               AND last_hook_at IS last_stop_at",
            rusqlite::params![outcome, id, turn_seq],
        )?;
        if n > 0 {
            self.emit_session(id)?;
        }
        Ok(n > 0)
    }

    /// Record who or what started a session (migration 124). Every start
    /// path writes it once the row exists; a restart, recreate or repair
    /// keeps the row and so keeps its origin. On the row, so this emits
    /// `session_updated`.
    pub fn set_session_origin(
        &self,
        id: i64,
        origin: &SessionOrigin,
    ) -> Result<(), rusqlite::Error> {
        let n = self.conn.execute(
            "UPDATE sessions SET origin = ?1, origin_ref = ?2 \
             WHERE id = ?3 AND (origin IS NOT ?1 OR origin_ref IS NOT ?2)",
            rusqlite::params![origin.origin, origin.origin_ref, id],
        )?;
        if n > 0 {
            self.emit_session(id)?;
        }
        Ok(())
    }

    /// Record which agent runs in the session's pane (`sessions.agent`,
    /// migration 121), for an agent other than the Claude Code a row starts
    /// with. A shell row's agent follows its kind instead
    /// ([`Store::set_session_kind`]). The column's `CHECK` refuses a value
    /// outside [`crate::store::AGENTS`].
    pub fn set_session_agent(&self, id: i64, agent: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions SET agent = ?1 WHERE id = ?2",
            rusqlite::params![agent, id],
        )?;
        self.emit_session(id)?;
        Ok(())
    }

    /// The tmux names of `host_alias`'s rows running `agent` (shell rows
    /// aside). Reconcile asks the host where those of them that are live
    /// keep their Codex conversations.
    pub fn pane_names_running(
        &self,
        host_alias: &str,
        agent: &str,
    ) -> Result<Vec<String>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT tmux_name FROM sessions \
             WHERE host_alias = ?1 AND agent = ?2 AND kind != 'shell' \
             ORDER BY tmux_name",
        )?;
        let rows = stmt.query_map(rusqlite::params![host_alias, agent], |r| r.get(0))?;
        rows.collect()
    }

    /// Mark a session as a review of `reviews_session_id` (or back to 'work' with
    /// None). Write-once at spawn_review time. Reconcile never touches these
    /// columns — they survive re-probe because upsert_session's ON CONFLICT clause
    /// omits them.
    ///
    /// `agent` (migration 121) follows a move into or out of `shell`: a
    /// shell session runs no agent, and a session that stops being a shell
    /// runs Claude Code, the only agent fleet launches today. Any other
    /// kind change leaves the agent alone.
    pub fn set_session_kind(
        &self,
        id: i64,
        kind: &str,
        reviews_session_id: Option<i64>,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions SET kind = ?1, reviews_session_id = ?2, \
             agent = CASE WHEN ?1 = 'shell' THEN 'shell' \
                          WHEN agent = 'shell' THEN 'claude' ELSE agent END \
             WHERE id = ?3",
            rusqlite::params![kind, reviews_session_id, id],
        )?;
        // A review inherits the reviewed session's primary work (M2.2).
        if let Some(parent) = reviews_session_id {
            if let Err(e) = self.inherit_work(id, parent, "review") {
                tracing::warn!(session_id = id, error = %e.message, "[work] review inherit failed");
            }
        }
        self.emit_session(id)?;
        Ok(())
    }

    /// The lost row (`lost_at` set) named `tmux_name` on `host_alias` that
    /// still carries a Claude conversation id — i.e. one `restore_host_sessions`
    /// could bring back. `new_session` refuses such a name: reconcile's upsert
    /// would revive the lost row and overwrite its conversation id.
    pub fn lost_resumable_session_named(
        &self,
        host_alias: &str,
        tmux_name: &str,
    ) -> rusqlite::Result<Option<SessionRow>> {
        self.conn
            .prepare_cached(&format!(
                "SELECT {SESSION_COLUMNS} FROM sessions
                 WHERE host_alias=?1 AND tmux_name=?2
                   AND lost_at IS NOT NULL AND claude_session_id IS NOT NULL"
            ))?
            .query_row(rusqlite::params![host_alias, tmux_name], map_session_row)
            .optional()
    }

    /// Any LOST row named `tmux_name` on `host_alias` that belongs to somebody
    /// other than `person` (multi-user M1, T5).
    ///
    /// The hole this closes: `reject_lost_session_name` refuses only a lost row
    /// with a `claude_session_id` ([`Self::lost_resumable_session_named`]),
    /// because that is the one a restore could bring back. A lost row WITHOUT
    /// one — a shell session, a work session that never bound a conversation —
    /// is not refused, and reconcile's `ON CONFLICT DO UPDATE` revives it with
    /// its `owner_person_id` intact. So person B starting a session under a
    /// tmux name person A once used would come up owned by, and readable to, A:
    /// DoD 7 broken by a live code path rather than by the migration.
    ///
    /// `person` is an `Option` because a caller may have none (a per-host
    /// token, a pre-M1 device), and the SQL answers fail-closed for that case:
    /// `owner_person_id IS NOT NULL AND owner_person_id IS NOT ?3` with a NULL
    /// `?3` matches every OWNED lost row, so a person-less caller is refused
    /// by all of them and inherits none. (`IS NOT` and not `!=`: SQL's `<>`
    /// against NULL is NULL, which `WHERE` drops — the row would be let
    /// through.) An unowned lost row matches nothing here and keeps its
    /// existing behaviour: it is revived, owned by nobody, which is what
    /// `unclaimed` is for.
    pub fn lost_session_named_owned_by_other(
        &self,
        host_alias: &str,
        tmux_name: &str,
        person: Option<i64>,
    ) -> rusqlite::Result<Option<SessionRow>> {
        self.conn
            .prepare_cached(&format!(
                "SELECT {SESSION_COLUMNS} FROM sessions
                 WHERE host_alias=?1 AND tmux_name=?2
                   AND lost_at IS NOT NULL
                   AND owner_person_id IS NOT NULL
                   AND owner_person_id IS NOT ?3"
            ))?
            .query_row(
                rusqlite::params![host_alias, tmux_name, person],
                map_session_row,
            )
            .optional()
    }

    /// Stamp `owner` on session `session_id` — but only while nobody owns it.
    ///
    /// **The one mechanism that stamps an owner** (T5 and its review): the
    /// create path calls this once the row exists, so the row a reconcile
    /// pass inserted `unclaimed` — or one a `move_session` or a
    /// `spawn_review` inherits an owner for — is the owner's by the time the
    /// create path returns it. It is keyed on the ROW ID, which is why it
    /// works where the deleted name-keyed reservation could not: an id names
    /// one row that already exists, a tmux name names whatever is reused
    /// under it next.
    ///
    /// `Ok(true)` when this call claimed the row, `Ok(false)` when there was
    /// nothing to do: either `owner` is `None` (no person to attribute it to —
    /// the row stays `unclaimed`, which is the answer and not a failure), or
    /// the row is already owned by exactly that person (a `move_session` whose
    /// target row already carries the source's owner, a re-entered claim).
    ///
    /// `E_FORBIDDEN` when the row is owned by someone ELSE. That is never a
    /// benign outcome: it means the caller is about to hand back, as the
    /// session it just created, a row that belongs to another person. Refusing
    /// is what makes the caller's `?` a real guard — it never re-owns, and it
    /// never lets a create path return somebody else's session either.
    /// `E_NOTFOUND` when the row is gone.
    pub fn claim_if_unclaimed(
        &self,
        session_id: i64,
        owner: Option<i64>,
    ) -> Result<bool, crate::ipc_error::IpcError> {
        use crate::ipc_error::{codes, IpcError};
        let Some(owner) = owner else {
            return Ok(false);
        };
        // The claim, as one statement whose `WHERE` is the rule: an owned row
        // is not matched, so there is no window between checking and writing.
        let claimed = self.conn.execute(
            "UPDATE sessions SET owner_person_id = ?2, visibility = 'private' \
              WHERE id = ?1 AND owner_person_id IS NULL",
            rusqlite::params![session_id, owner],
        )?;
        if claimed > 0 {
            self.emit_session(session_id)?;
            return Ok(true);
        }
        match self.get_session_by_id(session_id)? {
            None => Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("session {session_id} is gone; nothing to claim"),
            )),
            // Already this person's — the upsert got there first.
            Some(row) if row.owner_person_id == Some(owner) => Ok(false),
            Some(_) => Err(IpcError::new(
                codes::E_FORBIDDEN,
                format!(
                    "session {session_id} already belongs to another person; \
                     ownership is never transferred by a create path"
                ),
            )),
        }
    }

    /// The session on `host_alias` (live or lost) holding Claude conversation
    /// `claude_id`, if any. `new_session` refuses to resume a held conversation.
    pub fn session_with_claude_id(
        &self,
        host_alias: &str,
        claude_id: &str,
    ) -> rusqlite::Result<Option<SessionRow>> {
        self.conn
            .prepare_cached(&format!(
                "SELECT {SESSION_COLUMNS} FROM sessions
                 WHERE host_alias=?1 AND claude_session_id=?2 LIMIT 1"
            ))?
            .query_row(rusqlite::params![host_alias, claude_id], map_session_row)
            .optional()
    }

    /// Record the Claude Code session id fleet launched this session with
    /// (create / recreate / move / review). Opens that conversation with
    /// source `fleet` via [`Self::rebind_conversation`], which closes any
    /// previous one as `replaced` and resets the context. Reconcile's
    /// upsert only replaces the id with that of an agent matched BY NAME (or
    /// fills a NULL from an unambiguous cwd match — see
    /// `service::sessions::reconcile::pair_session_agents`), so a minted id
    /// survives reconciliation.
    pub fn set_claude_session_id(
        &self,
        id: i64,
        uuid: &str,
    ) -> Result<(), crate::ipc_error::IpcError> {
        self.rebind_conversation(id, uuid, StartSource::Fleet, None, None)?;
        Ok(())
    }

    /// Who a Claude CONVERSATION belonged to the first time a session bound
    /// it to an owner (`conversation_owners`, migration 099); `None` for a
    /// conversation no owned session ever held.
    ///
    /// The point of the table is that this answer outlives the session.
    /// `new_session { resume_claude_session_id }` resurrects a transcript
    /// precisely when the row is gone — `Store::delete_session` deletes it
    /// outright and `conversations` cascades with it — so a check against
    /// live or lost rows cannot close the takeover (spec §5.2). Two triggers
    /// on `sessions` fill the record, so no writer of `claude_session_id` can
    /// skip it, and they `INSERT OR IGNORE`: the FIRST owner wins, and a
    /// later re-attribution of the session cannot rewrite history.
    ///
    /// NOT the answer to "who owns this session": that is
    /// `SessionRow::owner_person_id`, read from the live row.
    pub fn conversation_owner(&self, claude_session_id: &str) -> Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT owner_person_id FROM conversation_owners \
                 WHERE claude_session_id = ?1",
                [claude_session_id],
                |r| r.get(0),
            )
            .optional()
    }

    /// The `claude --model` / `--effort` / credential profile a session
    /// launches with (`sessions.launch_model` / `effort_level` /
    /// `claude_profile`); all `None` for a missing row. Unvalidated: callers
    /// pass them to `tmux::ClaudeLaunch::checked`.
    #[allow(clippy::type_complexity)]
    pub fn session_launch(
        &self,
        id: i64,
    ) -> Result<(Option<String>, Option<String>, Option<String>), rusqlite::Error> {
        use rusqlite::OptionalExtension;
        Ok(self
            .conn
            .query_row(
                "SELECT launch_model, effort_level, claude_profile FROM sessions WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?
            .unwrap_or((None, None, None)))
    }

    /// Record the credential profile a session runs under (`None` = the
    /// host's own login). A change also drops the session's account link:
    /// the account it recorded belonged to the login it is leaving, and
    /// reconcile only ever fills an empty link (a profile session's from
    /// nothing yet, a host-login session's from the host). `claude_profile`
    /// is on the row (the sidebar shows it), so this emits `session_updated`.
    pub fn set_session_profile(
        &self,
        id: i64,
        profile: Option<&str>,
    ) -> Result<(), rusqlite::Error> {
        let n = self.conn.execute(
            "UPDATE sessions SET claude_profile = ?1, account_uuid = NULL \
             WHERE id = ?2 AND claude_profile IS NOT ?1",
            rusqlite::params![profile, id],
        )?;
        if n > 0 {
            self.emit_session(id)?;
        }
        Ok(())
    }

    /// Drop the account link of a session on the HOST's own login
    /// (`claude_profile IS NULL`), after its `claude` was relaunched. A
    /// relaunched `claude` reads whatever login the host holds now (a `/login`
    /// as another account since the pane started moves it), while reconcile
    /// keeps an existing link forever; an empty one it fills from the host's
    /// current account on the next pass. A profile session is left alone: its
    /// link follows the profile's login. Emits `session_updated` on a change.
    pub fn clear_host_login_account(&self, id: i64) -> Result<(), rusqlite::Error> {
        let n = self.conn.execute(
            "UPDATE sessions SET account_uuid = NULL \
             WHERE id = ?1 AND claude_profile IS NULL AND account_uuid IS NOT NULL",
            [id],
        )?;
        if n > 0 {
            self.emit_session(id)?;
        }
        Ok(())
    }

    /// Record the model a session launches with (`None` = the host's
    /// default). Not client-visible, so no event.
    pub fn set_session_launch_model(
        &self,
        id: i64,
        model: Option<&str>,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions SET launch_model = ?1 WHERE id = ?2",
            rusqlite::params![model, id],
        )?;
        Ok(())
    }

    /// Record the effort a session runs at (`None` = the host's default).
    /// `effort_level` is on the row (the sidebar's badge), so this emits
    /// `session_updated`.
    pub fn set_session_effort(&self, id: i64, effort: Option<&str>) -> Result<(), rusqlite::Error> {
        let n = self.conn.execute(
            "UPDATE sessions SET effort_level = ?1 WHERE id = ?2 AND effort_level IS NOT ?1",
            rusqlite::params![effort, id],
        )?;
        if n > 0 {
            self.emit_session(id)?;
        }
        Ok(())
    }

    /// Link a session to its worktree row (`sessions.worktree_id`), which
    /// reconcile never sets. Emits `session_updated`.
    pub fn link_session_worktree(&self, id: i64, worktree_id: i64) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions SET worktree_id = ?1 WHERE id = ?2",
            rusqlite::params![worktree_id, id],
        )?;
        self.emit_session(id)?;
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

    /// Label an `external` agent row (`bg:<uuid>`) with the name Claude shows
    /// for that session (`claude agents --json` `name`, e.g. a Claude Desktop
    /// title) on every reconcile pass, so the label follows a retitle. Only
    /// `external` rows: fleet did not start them and the UI offers no label
    /// edit there, so the agent's name is the only label they have. A `bg`
    /// row keeps its prompt-derived label. Emits `session_updated` only when
    /// the label actually changed, so a steady name costs no event per pass.
    pub fn set_external_agent_name(
        &self,
        host_alias: &str,
        tmux_name: &str,
        name: &str,
    ) -> Result<Option<SessionRow>, rusqlite::Error> {
        let changed = self.conn.execute(
            "UPDATE sessions SET friendly_name = ?3 \
             WHERE host_alias = ?1 AND tmux_name = ?2 AND kind = 'external' \
               AND friendly_name IS NOT ?3",
            rusqlite::params![host_alias, tmux_name, name],
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

    /// Announce every live session on `account_uuid` again, unchanged, so
    /// the hub re-stamps their derived `needs_attention` after the account
    /// moved into or out of a limit or a lost login (review r05 F9). A dead
    /// row (ghost or lost) and a shell or external one never carry an
    /// account `Blocked` reason, so they are left out. Returns how many.
    pub fn reemit_live_sessions_on_account(
        &self,
        account_uuid: &str,
    ) -> Result<usize, rusqlite::Error> {
        let ids: Vec<i64> = self
            .conn
            .prepare_cached(
                "SELECT id FROM sessions WHERE account_uuid = ?1 AND status != 'ghost' \
                 AND lost_at IS NULL AND kind NOT IN ('shell', 'external') ORDER BY id",
            )?
            .query_map([account_uuid], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        for &id in &ids {
            self.emit_session(id)?;
        }
        Ok(ids.len())
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

    /// Put `last_prompt` back to an earlier value (possibly none): the send
    /// it was stamped ahead of failed, so the prompt never reached the pane.
    /// Emits `session_updated`.
    pub fn restore_last_prompt(
        &self,
        id: i64,
        prompt: Option<&str>,
    ) -> Result<Option<SessionRow>, rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions SET last_prompt=?1 WHERE id=?2",
            rusqlite::params![prompt, id],
        )?;
        self.emit_session(id)
    }

    /// Stamp when fleet created this session (migration 019). Only sets the
    /// value once — a re-create keeps the original start. Emits
    /// `session_updated` so the sidebar's elapsed label does not wait for a
    /// re-list.
    /// Put session `id` in project `project_id` (Adopt into, step 4.12).
    /// The worktree it named belonged to its old project, so it is cleared.
    pub fn set_session_project(&self, id: i64, project_id: i64) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions SET project_id=?1, worktree_id=NULL WHERE id=?2",
            rusqlite::params![project_id, id],
        )?;
        self.emit_session(id)?;
        Ok(())
    }

    pub fn set_started_at(&self, id: i64, at: i64) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions SET started_at=COALESCE(started_at, ?1) WHERE id=?2",
            rusqlite::params![at, id],
        )?;
        self.emit_session(id)?;
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

    /// How many times the `oom` playbook recreated session `id` — or tried
    /// and failed — since `since`: `playbook_applied` rows whose detail is
    /// exactly `oom:recreate` or starts with `oom:recreate:failed:`. A
    /// refusal (`…:skipped:…`) is not an attempt.
    pub fn count_oom_recreates_since(&self, id: i64, since: i64) -> Result<u32, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM session_events \
                 WHERE session_id = ?1 AND kind = 'playbook_applied' AND at >= ?2 \
                   AND (detail = 'oom:recreate' OR detail LIKE 'oom:recreate:failed:%')",
                rusqlite::params![id, since],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n as u32)
    }

    /// Carry the row `(host_alias, old)` over to `new` after fleet renamed its
    /// tmux session (`rename_session`). Returns the renamed row, or `None`
    /// when no row held `old`.
    ///
    /// Without this a rename was a delete plus an insert: reconcile keys rows
    /// on `(host_alias, tmux_name)`, so the pass after the rename INSERTed a
    /// fresh row under `new` and ghosted — then reaped — the old one, and with
    /// it the session's id, participant (its inbox and address), timeline and
    /// conversations. Renaming the row in place keeps all of them.
    ///
    /// * A row still holding `new` is stale by construction: tmux refused the
    ///   rename if a live session had that name, so whatever row carries it
    ///   (a ghost fleet killed or lost) is dismissed through
    ///   [`Self::delete_session`] first, or the UPDATE would hit the unique
    ///   key.
    /// * `last_reconciled_at = now` puts the renamed row under the BE-3
    ///   guard: a pass whose probe listed tmux before the rename (and so
    ///   lacks `new` in its keep set) cannot ghost it.
    /// * `old` is remembered like a kill: that same stale pass still lists
    ///   `old`, and with no row left under that name it would insert the
    ///   session a second time.
    ///
    /// A `lost` row under `new` that is still restorable (host reboot
    /// survival: `lost_at` set and a conversation to resume) is NOT a ghost
    /// to dismiss: the rename is refused with `E_EXISTS` naming it, so its
    /// timeline, participant and restore entry survive. The service refuses
    /// before tmux renames anything (`reject_lost_session_name`); this is
    /// the store's own guard. The dismissal and the rename are one
    /// transaction.
    pub fn rename_session_row(
        &self,
        host_alias: &str,
        old: &str,
        new: &str,
        now: i64,
    ) -> Result<Option<SessionRow>, crate::ipc_error::IpcError> {
        if old == new {
            return Ok(self.get_session(old, host_alias)?);
        }
        // Multi-user M1 (T10): no row id in the message, for the reason
        // `sessions::lifecycle::reject_lost_session_name` gives — `new` is
        // the caller's own argument and the advice needs no id, while the
        // id named another person's lost row.
        if self
            .lost_resumable_session_named(host_alias, new)?
            .is_some()
        {
            return Err(crate::ipc_error::IpcError::new(
                crate::ipc_error::codes::E_EXISTS,
                format!(
                    "{new} belongs to a lost session; restore it with \
                     restore_host_sessions or dismiss it first"
                ),
            ));
        }
        let tx = self.conn.unchecked_transaction()?;
        let stale: Option<i64> = tx
            .query_row(
                "SELECT id FROM sessions WHERE host_alias=?1 AND tmux_name=?2",
                rusqlite::params![host_alias, new],
                |r| r.get(0),
            )
            .optional()?;
        // The ghost's facts, read while it is still there: the frame that
        // announces its death is fenced by them (see `killed_payloads`).
        let stale_killed = killed_payloads(&tx, stale.as_slice())?;
        if let Some(id) = stale {
            // A dead ghost gives way: what `delete_session` does, inside
            // this transaction (its event is emitted after the commit).
            tx.execute(
                "DELETE FROM session_events WHERE session_id=?1",
                rusqlite::params![id],
            )?;
            tx.execute(
                "UPDATE participants SET retired_at = ?1, session_id = NULL, client_id = NULL \
                 WHERE session_id = ?2 AND retired_at IS NULL",
                rusqlite::params![now_unix(), id],
            )?;
            tx.execute("DELETE FROM sessions WHERE id=?1", rusqlite::params![id])?;
        }
        let id: Option<i64> = tx
            .query_row(
                "UPDATE sessions SET tmux_name=?3, last_reconciled_at=?4 \
                 WHERE host_alias=?1 AND tmux_name=?2 RETURNING id",
                rusqlite::params![host_alias, old, new, now],
                |r| r.get(0),
            )
            .optional()?;
        tx.commit()?;
        for gone in stale_killed {
            self.bus.session_killed(gone);
        }
        self.note_kill(host_alias, old, now);
        match id {
            Some(id) => Ok(self.emit_session(id)?),
            None => Ok(None),
        }
    }

    /// Hard-delete one session row (ghost dismissal) together with what dies
    /// with it, in one transaction: its `session_events` timeline. Neither
    /// table has an FK cascade, and `sessions.id` has no AUTOINCREMENT, so
    /// leftover events would surface on the next session that reuses the id.
    /// Kept: messages it SENT (they live in the recipients' inboxes) and
    /// tasks it requested or worked — the task sweep fails a task whose
    /// worker row is gone.
    ///
    /// Deliberately NOT `DELETE FROM session_messages`: a kill used to
    /// destroy every undelivered message addressed to this session, and a
    /// MOVE goes through here too (the source row is killed after the
    /// target is created), so a move silently lost the inbox. The identity
    /// is tombstoned instead; `service/gc.rs` sweeps the mail after the
    /// retention window, and a move re-points the participant BEFORE this
    /// runs, so there is nothing here left to retire.
    ///
    /// `session_grants` (migration 100) is NOT deleted here and must not be:
    /// it is `REFERENCES sessions(id) ON DELETE CASCADE`, so it goes with the
    /// row by itself — which matters for the same id-reuse reason as the
    /// events above, and is pinned by
    /// `store/session_grants.rs::deleting_a_session_cascades_its_grants_and_a_reused_rowid_inherits_none`.
    pub fn delete_session(&self, id: i64) -> Result<(), rusqlite::Error> {
        // Before the delete: afterwards there is nothing to read.
        let killed = self.killed_payload(id);
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM session_events WHERE session_id=?1",
            rusqlite::params![id],
        )?;
        tx.execute(
            "UPDATE participants SET retired_at = ?1, session_id = NULL, client_id = NULL \
             WHERE session_id = ?2 AND retired_at IS NULL",
            rusqlite::params![now_unix(), id],
        )?;
        tx.execute("DELETE FROM sessions WHERE id=?1", rusqlite::params![id])?;
        tx.commit()?;
        self.bus.session_killed(killed);
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
            // The rows are already gone here (`DELETE ... RETURNING`), so
            // these frames carry no facts — a test-only path, and the
            // fence's fail-closed answer is the right one for it.
            self.bus.session_killed((*id).into());
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
    /// bumps `turn_seq`, stamps `last_stop_at` / `last_turn_at`, maintains
    /// `idle_since`, ends a `compacting` activity, and clears `pending_input`
    /// (a dialog on the just-ended turn's pane does not survive it). Keyed
    /// by row id (the hook resolver already picked the row: two rows may
    /// share one `claude_session_id`); returns the updated row (`None` when
    /// the row is gone). Emits `session_updated`.
    pub fn record_stop_hook_for_row(
        &self,
        row_id: i64,
    ) -> Result<Option<SessionRow>, crate::ipc_error::IpcError> {
        let now = now_unix();
        let changed = self.conn.execute(
            &format!(
                "UPDATE sessions SET claude_status = 'idle', turn_seq = turn_seq + 1, \
                     last_stop_at = ?2, last_turn_at = ?2, last_hook_at = ?2, \
                     idle_since = COALESCE(idle_since, ?2), pending_input = NULL, \
                     stale_working_at = NULL, stale_demoted_at = NULL, \
                     turn_outcome = NULL{END_COMPACTING} \
                 WHERE id = ?1"
            ),
            rusqlite::params![row_id, now],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        if let Err(e) = self.insert_session_event(row_id, "status_change", Some("idle")) {
            tracing::warn!(session_id = row_id, error = %e, "[hook] status_change not recorded");
        }
        Ok(self.emit_session(row_id)?)
    }

    /// The UserPromptSubmit hook's write: a turn is starting. Sets
    /// `claude_status = working`, clears `idle_since` so "idle because
    /// never started" and "idle after a turn" are distinguishable from
    /// "busy", ends a `compacting` activity, and clears `pending_input` (a
    /// dialog on the pane before this prompt is stale the moment a new turn
    /// starts). Returns the updated row (`None` when the row is gone).
    /// Emits `session_updated`.
    pub fn record_prompt_submit_hook_for_row(
        &self,
        row_id: i64,
    ) -> Result<Option<SessionRow>, crate::ipc_error::IpcError> {
        self.record_prompt_submit_hook_for_row_with(row_id, true)
    }

    /// [`Self::record_prompt_submit_hook_for_row`] with the touch decided by
    /// the caller: `touch = false` for a prompt fleet itself typed (a peer's
    /// wake nudge, the safe-kill instructions, an inbox delivery), which
    /// marks the session working like any other but is nobody's touch and
    /// must not keep a done session out of tidy-up or un-archive it.
    pub fn record_prompt_submit_hook_for_row_with(
        &self,
        row_id: i64,
        touch: bool,
    ) -> Result<Option<SessionRow>, crate::ipc_error::IpcError> {
        let changed = self.conn.execute(
            &format!(
                "UPDATE sessions SET claude_status = 'working', idle_since = NULL, \
                     last_hook_at = ?2, prompt_submit_seq = prompt_submit_seq + 1, \
                     pending_input = NULL, stale_working_at = NULL, stale_demoted_at = NULL, \
                     turn_outcome = NULL{END_COMPACTING} WHERE id = ?1"
            ),
            rusqlite::params![row_id, now_unix()],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        if let Err(e) = self.insert_session_event(row_id, "status_change", Some("working")) {
            tracing::warn!(session_id = row_id, error = %e, "[hook] status_change not recorded");
        }
        // A person's prompt is a touch (work graph M7): it protects the
        // session from tidy-up for an hour and un-archives it.
        if touch {
            self.touch_for_prompt(row_id)?;
        }
        Ok(self.emit_session(row_id)?)
    }

    /// Read [`PromptAckState`] for a row: `prompt_submit_seq` to watch, and
    /// whether a hook has EVER stamped `last_hook_at` on it. `None` when the
    /// row is gone.
    pub fn prompt_ack_state(&self, row_id: i64) -> Result<Option<PromptAckState>, rusqlite::Error> {
        use rusqlite::OptionalExtension;
        self.conn
            .query_row(
                "SELECT prompt_submit_seq, last_hook_at IS NOT NULL FROM sessions WHERE id = ?1",
                rusqlite::params![row_id],
                |r| {
                    Ok(PromptAckState {
                        prompt_submit_seq: r.get(0)?,
                        hooks_seen: r.get::<_, i64>(1)? != 0,
                    })
                },
            )
            .optional()
    }

    /// The SessionEnd hook's write: the Claude process is gone. Sets
    /// `claude_status = stopped`, starts `idle_since` if not already idle,
    /// clears any stuck episode and `pending_input` (no pane is left to show
    /// a dialog), and stamps `last_hook_at` so the reconcile guard keeps the
    /// verdict until a later pass observes the pane afresh. Returns the row
    /// (`None` when the row is gone). Emits `session_updated`.
    pub fn record_session_end_hook_for_row(
        &self,
        row_id: i64,
    ) -> Result<Option<SessionRow>, crate::ipc_error::IpcError> {
        let now = now_unix();
        let changed = self.conn.execute(
            "UPDATE sessions SET claude_status = 'stopped', last_turn_at = ?2, \
                 last_hook_at = ?2, idle_since = COALESCE(idle_since, ?2), \
                 stuck_kind = NULL, stuck_since = NULL, pending_input = NULL, \
                 stale_working_at = NULL, stale_demoted_at = NULL, turn_outcome = NULL \
                 WHERE id = ?1",
            rusqlite::params![row_id, now],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        if let Err(e) = self.insert_session_event(row_id, "status_change", Some("stopped")) {
            tracing::warn!(session_id = row_id, error = %e, "[hook] status_change not recorded");
        }
        Ok(self.emit_session(row_id)?)
    }

    /// The StopFailure hook's write: the turn ended in an API error (F3).
    /// The Stop stamps are all made (`turn_seq`, `last_stop_at`,
    /// `last_turn_at`, `last_hook_at`, `idle_since`, `pending_input`) so
    /// waiters return and the GC clocks run, but the row reads `failed`
    /// — until the next UserPromptSubmit (`working`) or Stop (`idle`), and
    /// through reconcile's pane reads of the input box the failure left
    /// behind (`store/reconcile.rs`, `NEW_STATUS`). The handler records the
    /// `stop_failure` timeline event that says which error.
    pub fn record_stop_failure_hook_for_row(
        &self,
        row_id: i64,
    ) -> Result<Option<SessionRow>, crate::ipc_error::IpcError> {
        let now = now_unix();
        let changed = self.conn.execute(
            &format!(
                "UPDATE sessions SET claude_status = 'failed', turn_seq = turn_seq + 1, \
                     last_stop_at = ?2, last_turn_at = ?2, last_hook_at = ?2, \
                     idle_since = COALESCE(idle_since, ?2), pending_input = NULL, \
                     stale_working_at = NULL, stale_demoted_at = NULL, \
                     turn_outcome = NULL{END_COMPACTING} \
                 WHERE id = ?1"
            ),
            rusqlite::params![row_id, now],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        if let Err(e) = self.insert_session_event(row_id, "status_change", Some("failed")) {
            tracing::warn!(session_id = row_id, error = %e, "[hook] status_change not recorded");
        }
        Ok(self.emit_session(row_id)?)
    }

    /// The tick's stale-`working` rule (lifecycle F2): a live tmux row that
    /// says `working` but has had no hook, no turn, no transcript growth
    /// (`context_at`, `usage_updated_at`), no spinner on its pane
    /// (`pane_working_at`, stamped by every reconcile pass that captured the
    /// pane showing a live turn) and no tmux session activity
    /// (`last_activity_at`) for `stale_secs` is demoted to `idle` and stamped
    /// `stale_working_at = now`, which is what the attention model reads,
    /// and `stale_demoted_at = now`, which arms the reconcile's
    /// `stale_working_veto`. `last_activity_at` is tmux's
    /// `#{session_activity}`, which moves on a client's input and an attach,
    /// NOT on pane output (a detached pane can print for hours without
    /// moving it) — the spinner stamp is the pane evidence. A demoted row's
    /// `idle` is a guess: `store::trusted_status` makes turn-over checks ask
    /// the pane before believing it while `stale_demoted_at` is set.
    /// Only a row a reconcile pass observed live within the window
    /// (`last_reconciled_at >= now - stale_secs`) is judged: on a host that
    /// is unreachable or has not been probed, the missing hooks and the
    /// still tmux clock say nothing about the session, and demoting it
    /// would stamp `stale_working` on every `working` row of a host that
    /// merely went offline.
    /// Pane-less and `shell` rows are never judged (no hooks or turns to
    /// miss); a demoted row is not judged twice — even once an attach or the
    /// TTL has lifted its attention stamp, since the demotion itself still
    /// stands; `stale_secs <= 0` is off.
    /// Returns the demoted rows; each gets `session_updated`, a
    /// `status_change idle` and a `stale_working` timeline entry.
    pub fn age_out_stale_working(
        &self,
        now: i64,
        stale_secs: i64,
    ) -> Result<Vec<SessionRow>, rusqlite::Error> {
        if stale_secs <= 0 {
            return Ok(Vec::new());
        }
        let cutoff = now - stale_secs;
        let ids: Vec<i64> = self
            .conn
            .prepare(
                "UPDATE sessions SET claude_status = 'idle', idle_since = ?1, \
                     stale_working_at = ?1, stale_demoted_at = ?1 \
                 WHERE status = 'running' AND claude_status = 'working' \
                   AND kind NOT IN ('bg','external','shell') AND stale_demoted_at IS NULL \
                   AND COALESCE(last_hook_at, 0) < ?2 AND COALESCE(last_turn_at, 0) < ?2 \
                   AND COALESCE(context_at, 0) < ?2 AND COALESCE(usage_updated_at, 0) < ?2 \
                   AND COALESCE(pane_working_at, 0) < ?2 \
                   AND last_activity_at < ?2 AND created_at < ?2 \
                   AND COALESCE(last_reconciled_at, 0) >= ?2 \
                 RETURNING id",
            )?
            .query_map(rusqlite::params![now, cutoff], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut out = Vec::with_capacity(ids.len());
        let detail = format!(
            "no hook, turn, transcript growth, pane spinner or tmux session activity for {stale_secs}s"
        );
        for id in ids {
            for (kind, d) in [
                ("status_change", "idle"),
                ("stale_working", detail.as_str()),
            ] {
                if let Err(e) = self.insert_session_event(id, kind, Some(d)) {
                    tracing::warn!(session_id = id, kind, error = %e, "[reconcile] session_event insert failed");
                }
            }
            if let Some(row) = self.emit_session(id)? {
                out.push(row);
            }
        }
        Ok(out)
    }

    /// Lift `stale_working_at` where it no longer asks anything of a person:
    /// the row is `working` or `blocked` again (a pane read lifted the
    /// demotion without a hook — `stale_working_veto`), or the stamp is
    /// older than `ttl_secs` (nobody looked; `0` = never by age). An attach
    /// clears it too (`touch_session`), and every hook does. Only a row that
    /// is `working` / `blocked` again also loses `stale_demoted_at` (the
    /// veto): the TTL ends the reason, not the demotion. Returns the rows
    /// whose stamp it lifted; each gets `session_updated` once. A row whose
    /// only change is `stale_demoted_at` is not emitted — nothing a client
    /// sees changed.
    pub fn expire_stale_working(
        &self,
        now: i64,
        ttl_secs: i64,
    ) -> Result<Vec<SessionRow>, rusqlite::Error> {
        let cutoff = if ttl_secs > 0 {
            now - ttl_secs
        } else {
            i64::MIN
        };
        let ids: Vec<i64> = self
            .conn
            .prepare(
                "UPDATE sessions SET stale_working_at = NULL, \
                     stale_demoted_at = CASE WHEN claude_status IN ('working', 'blocked') \
                                             THEN NULL ELSE stale_demoted_at END \
                 WHERE stale_working_at IS NOT NULL \
                   AND (claude_status IN ('working', 'blocked') OR stale_working_at < ?1) \
                 RETURNING id",
            )?
            .query_map(rusqlite::params![cutoff], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        // An acknowledged demotion (stamp already gone) whose row works
        // again: disarm the veto too. Silent — `stale_demoted_at` is not on
        // the wire, and the rows above already lost theirs.
        self.conn.execute(
            "UPDATE sessions SET stale_demoted_at = NULL \
             WHERE stale_demoted_at IS NOT NULL AND stale_working_at IS NULL \
               AND claude_status IN ('working', 'blocked')",
            [],
        )?;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(row) = self.emit_session(id)? {
                out.push(row);
            }
        }
        Ok(out)
    }

    /// The Notification hook's write. `status` is the mapped status;
    /// `stuck` is `Some(Some(kind))` to set (restarting `stuck_since` when
    /// the kind changes, keeping it when equal), `Some(None)` to clear,
    /// `None` to leave the stuck fields untouched. Stamps `last_hook_at`;
    /// `idle_since` follows the status. Returns the row (`None` when the row
    /// is gone). Emits `session_updated`.
    pub fn record_notification_hook_for_row(
        &self,
        row_id: i64,
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
             stale_working_at = NULL, stale_demoted_at = NULL, turn_outcome = NULL, \
             idle_since = {idle}{stuck_sql} WHERE id = ?1",
            idle = idle_since_sql("?4", "?2"),
        );
        let kind = match stuck {
            Some(Some(k)) => Some(k.as_str()),
            _ => None,
        };
        let changed = self
            .conn
            .execute(&sql, rusqlite::params![row_id, now, kind, status])?;
        if changed == 0 {
            return Ok(None);
        }
        if let Err(e) = self.insert_session_event(row_id, "status_change", Some(status)) {
            tracing::warn!(session_id = row_id, error = %e, "[hook] status_change not recorded");
        }
        Ok(self.emit_session(row_id)?)
    }

    /// Test shorthand: [`Self::record_stop_hook_for_row`] on the row bound
    /// to `claude_session_id`.
    #[cfg(test)]
    pub fn record_stop_hook(
        &self,
        claude_session_id: &str,
    ) -> Result<Option<SessionRow>, crate::ipc_error::IpcError> {
        match self.fetch_session_by_claude_id(claude_session_id)? {
            Some(r) => self.record_stop_hook_for_row(r.id),
            None => Ok(None),
        }
    }

    /// Test shorthand: [`Self::record_prompt_submit_hook_for_row`] by
    /// `claude_session_id`.
    #[cfg(test)]
    pub fn record_prompt_submit_hook(
        &self,
        claude_session_id: &str,
    ) -> Result<Option<SessionRow>, crate::ipc_error::IpcError> {
        match self.fetch_session_by_claude_id(claude_session_id)? {
            Some(r) => self.record_prompt_submit_hook_for_row(r.id),
            None => Ok(None),
        }
    }

    /// Test shorthand: [`Self::record_session_end_hook_for_row`] by
    /// `claude_session_id`.
    #[cfg(test)]
    pub fn record_session_end_hook(
        &self,
        claude_session_id: &str,
    ) -> Result<Option<SessionRow>, crate::ipc_error::IpcError> {
        match self.fetch_session_by_claude_id(claude_session_id)? {
            Some(r) => self.record_session_end_hook_for_row(r.id),
            None => Ok(None),
        }
    }

    /// Test shorthand: [`Self::record_stop_failure_hook_for_row`] by
    /// `claude_session_id`.
    #[cfg(test)]
    pub fn record_stop_failure_hook(
        &self,
        claude_session_id: &str,
    ) -> Result<Option<SessionRow>, crate::ipc_error::IpcError> {
        match self.fetch_session_by_claude_id(claude_session_id)? {
            Some(r) => self.record_stop_failure_hook_for_row(r.id),
            None => Ok(None),
        }
    }

    /// Test shorthand: [`Self::record_notification_hook_for_row`] by
    /// `claude_session_id`.
    #[cfg(test)]
    pub fn record_notification_hook(
        &self,
        claude_session_id: &str,
        status: crate::service::pane_intel::ClaudeStatus,
        stuck: Option<Option<crate::service::pane_intel::StuckKind>>,
    ) -> Result<Option<SessionRow>, crate::ipc_error::IpcError> {
        match self.fetch_session_by_claude_id(claude_session_id)? {
            Some(r) => self.record_notification_hook_for_row(r.id, status, stuck),
            None => Ok(None),
        }
    }

    /// Set (or clear) the row's `current_activity`. Also clears
    /// `pending_input`: every caller of this method is overriding the pane
    /// guess with something authoritative (a hook, not the reconcile pass
    /// that derives `pending_input` from the same pane read), so whatever
    /// dialog was last seen no longer describes what the pane is showing
    /// now. Emits `session_updated`.
    pub fn set_current_activity(
        &self,
        id: i64,
        activity: Option<&str>,
    ) -> Result<Option<SessionRow>, crate::ipc_error::IpcError> {
        self.conn.execute(
            "UPDATE sessions SET current_activity = ?2, pending_input = NULL WHERE id = ?1",
            rusqlite::params![id, activity],
        )?;
        Ok(self.emit_session(id)?)
    }

    /// How many `unclaimed` sessions each host carries — the ONE thing an
    /// out-of-scope caller ever learns about a row nobody can speak for
    /// (multi-user M1, spec §4.3, *`'unclaimed'`: the safe holding state*).
    ///
    /// A host with none is **absent from the map**, not present with `0`:
    /// `0` is a claim about the host, and the carrier
    /// (`HostRow.unclaimed_sessions`) distinguishes "no unclaimed rows here"
    /// from "you are not being told", so the two must not collapse on the
    /// way out. `service::hosts::list_hosts` turns an absent entry into
    /// `Some(0)` for a caller entitled to the count, and into `None` for
    /// everyone else.
    ///
    /// Ghost rows are excluded, exactly as [`Self::find_session_by_pane`]
    /// excludes them: a ghost is a row reconcile is about to delete, and a
    /// badge that counts it sends someone looking for a session that is not
    /// there. Lost rows are counted — they are real sessions whose host went
    /// away, and they are claimable when it comes back.
    pub fn unclaimed_counts_by_host(
        &self,
    ) -> Result<std::collections::BTreeMap<String, i64>, crate::ipc_error::IpcError> {
        let mut st = self.conn.prepare(
            "SELECT host_alias, COUNT(*) FROM sessions \
              WHERE visibility = ?1 AND status != 'ghost' GROUP BY host_alias",
        )?;
        let rows = st.query_map(rusqlite::params![crate::store::VISIBILITY_UNCLAIMED], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Per host, what the N5 host placement weighs (redesign step 4.11): the
    /// live sessions there (ghosts excluded, as [`Self::unclaimed_counts_by_host`]
    /// does), the sessions `project_id` started there since `since`, and
    /// whether the project has a worktree row there.
    pub fn host_placement_counts(
        &self,
        project_id: i64,
        since: i64,
    ) -> Result<std::collections::BTreeMap<String, (i64, i64, bool)>, crate::ipc_error::IpcError>
    {
        let mut out: std::collections::BTreeMap<String, (i64, i64, bool)> = Default::default();
        let mut st = self.conn.prepare(
            "SELECT host_alias, SUM(status != 'ghost'), \
                    SUM(project_id = ?1 AND created_at >= ?2) \
               FROM sessions GROUP BY host_alias",
        )?;
        let rows = st.query_map(rusqlite::params![project_id, since], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                r.get::<_, Option<i64>>(2)?.unwrap_or(0),
            ))
        })?;
        for row in rows {
            let (host, live, recent) = row?;
            out.insert(host, (live, recent, false));
        }
        let mut st = self
            .conn
            .prepare("SELECT DISTINCT host_alias FROM worktrees WHERE project_id = ?1")?;
        let hosts = st.query_map([project_id], |r| r.get::<_, String>(0))?;
        for h in hosts {
            out.entry(h?).or_default().2 = true;
        }
        Ok(out)
    }

    /// The one live row on `host_alias` whose last-seen pane is `pane_id`.
    /// `None` when there is none or more than one (a stale pane id after a
    /// tmux server restart shared with a new row).
    pub fn find_session_by_pane(
        &self,
        host_alias: &str,
        pane_id: &str,
    ) -> Result<Option<SessionRow>, crate::ipc_error::IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {SESSION_COLUMNS} FROM sessions \
             WHERE host_alias = ?1 AND tmux_pane_id = ?2 AND status != 'ghost' LIMIT 2"
        ))?;
        let rows: Vec<SessionRow> = stmt
            .query_map(rusqlite::params![host_alias, pane_id], map_session_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(match rows.len() {
            1 => rows.into_iter().next(),
            _ => None,
        })
    }

    /// Every row bound to `claude_session_id` (normally zero or one; two rows
    /// sharing an id has been observed live, and the hook resolver then
    /// refuses to guess).
    pub fn sessions_by_claude_id(
        &self,
        claude_session_id: &str,
    ) -> Result<Vec<SessionRow>, crate::ipc_error::IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {SESSION_COLUMNS} FROM sessions WHERE claude_session_id = ?1"
        ))?;
        let rows = stmt
            .query_map(rusqlite::params![claude_session_id], map_session_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
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

    /// [`Self::set_transcript_path_by_claude_id`] for one row, and only while
    /// `claude_session_id` is still its current conversation. A repeat of
    /// the same path is a no-op write (Task 3: every hook of a conversation
    /// resends the same `transcript_path`, and an unconditional write here
    /// was a physical UPDATE under the store lock on every one of them —
    /// which, before migration 063, also bumped `row_version`).
    pub fn set_transcript_path_for_row(
        &self,
        row_id: i64,
        claude_session_id: &str,
        path: &str,
    ) -> Result<(), crate::ipc_error::IpcError> {
        self.conn.execute(
            "UPDATE sessions SET transcript_path = ?1 WHERE id = ?2 AND claude_session_id = ?3 \
             AND transcript_path IS NOT ?1",
            rusqlite::params![path, row_id, claude_session_id],
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

    /// Emit `move:progress` (not a store row).
    pub fn bus_move_progress(&self, p: &crate::events::MoveProgress) {
        self.bus.move_progress(p);
    }

    /// Emit `start:progress` (not a store row): a `new_session` step
    /// boundary (redesign step 5.13).
    pub fn bus_start_progress(&self, p: &crate::events::StartProgress) {
        self.bus.start_progress(p);
    }

    /// Emit `confirm:changed` (not a store row): the confirmation queue
    /// moved (redesign step 9.2).
    pub fn bus_confirm_changed(&self) {
        self.bus.confirm_changed();
    }

    /// Emit `handoff:changed` (redesign step 9.3): Control's agent handed
    /// work on and a receipt was written.
    pub fn bus_handoff_changed(&self) {
        self.bus.handoff_changed();
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

    /// In-memory store with one running session on `local`, for the
    /// participant-tombstone tests below.
    fn seed(s: &Store, name: &str) -> i64 {
        s.upsert_host("local").unwrap();
        s.upsert_session(name, "local", None, None, 0, 0, "running", None)
            .unwrap()
    }

    /// `find_sessions_by_tmux_name` is a direct `WHERE tmux_name=?` (plus an
    /// optional host filter) — this pins that it finds every host's row of
    /// that name (including a lost one, which `whoami`'s ambiguity fallback
    /// still needs to see), names none of a different name, and that the
    /// host filter narrows to just that host's row.
    #[test]
    fn find_sessions_by_tmux_name_matches_every_host_or_just_one() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("alpha").unwrap();
        s.upsert_host("beta").unwrap();
        let a = s
            .upsert_session("dev-a", "alpha", None, None, 0, 0, "running", None)
            .unwrap();
        let b = s
            .upsert_session("dev-a", "beta", None, None, 0, 0, "running", None)
            .unwrap();
        s.upsert_session("dev-other", "alpha", None, None, 0, 0, "running", None)
            .unwrap();
        // A lost row must still be found — `whoami`'s ambiguity fallback
        // treats "only ghosts left" as one match, not zero.
        s.conn
            .execute(
                "UPDATE sessions SET status='ghost', lost_at=5 WHERE id=?1",
                rusqlite::params![b],
            )
            .unwrap();

        let mut all = s.find_sessions_by_tmux_name("dev-a", None).unwrap();
        all.sort_by_key(|r| r.id);
        assert_eq!(all.iter().map(|r| r.id).collect::<Vec<_>>(), vec![a, b]);
        assert!(all.iter().any(|r| r.id == b && r.lost_at.is_some()));

        let scoped = s
            .find_sessions_by_tmux_name("dev-a", Some("alpha"))
            .unwrap();
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].id, a);

        assert!(s
            .find_sessions_by_tmux_name("no-such-name", None)
            .unwrap()
            .is_empty());
    }

    /// `/metrics` counts in SQL what `fleet_health` counts in Rust: the
    /// GROUP BY and the reachable count must equal `summarize`'s
    /// `by_status` and `hosts_reachable`, external / shell rows and a NULL
    /// status included.
    #[test]
    fn metrics_counts_equal_the_summarize_roll_up() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("alpha").unwrap(); // reachable
        s.insert_host("beta", Some("beta")).unwrap(); // never probed
        let mk = |name: &str, host: &str, kind: &str, status: Option<&str>| {
            let id = s
                .upsert_session(name, host, None, None, 0, 0, "running", None)
                .unwrap();
            s.conn
                .execute(
                    "UPDATE sessions SET kind=?1, claude_status=?2 WHERE id=?3",
                    rusqlite::params![kind, status, id],
                )
                .unwrap();
        };
        mk("w1", "alpha", "work", Some("working"));
        mk("w2", "alpha", "work", Some("idle"));
        mk("w3", "beta", "work", None);
        mk("bg", "beta", "bg", Some("working"));
        mk("ext", "alpha", "external", Some("working"));
        mk("ext2", "alpha", "external", None);
        mk("sh", "beta", "shell", Some("idle"));

        let summary = crate::service::health::summarize(
            &s.list_all_sessions().unwrap(),
            &s.list_hosts().unwrap(),
            90.0,
        );
        let by_status = s.count_sessions_by_claude_status().unwrap();
        assert_eq!(by_status, summary.by_status);
        assert_eq!(by_status.get("working"), Some(&2));
        assert_eq!(by_status.get("unknown"), Some(&1));
        assert_eq!(s.count_reachable_hosts().unwrap(), summary.hosts_reachable);
        assert_eq!(summary.hosts_reachable, 1);
    }

    #[test]
    fn deleting_a_session_retires_its_participant_and_keeps_undelivered_mail() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        let b = seed(&s, "beta");
        let m = s.insert_message(a, b, "unread", "message", None).unwrap();
        let p = s.participant_for_session(b).unwrap().unwrap().id;

        s.delete_session(b).unwrap();

        assert!(
            s.get_message(m).unwrap().is_some(),
            "the message survives the kill"
        );
        let row = s
            .participant_by_id(p)
            .unwrap()
            .expect("the identity survives");
        assert!(row.retired_at.is_some(), "and is tombstoned");
    }

    #[test]
    fn a_move_repoints_the_participant_so_mail_follows_the_session() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        let src = seed(&s, "worker");
        let dst = seed(&s, "worker-moved");
        let m = s
            .insert_message(a, src, "follow me", "message", None)
            .unwrap();
        let p = s.participant_for_session(src).unwrap().unwrap().id;

        // What finalise does: re-point, then kill the source row.
        s.repoint_participant(p, dst).unwrap();
        s.delete_session(src).unwrap();

        assert_eq!(
            s.participant_by_id(p).unwrap().unwrap().session_id,
            Some(dst)
        );
        let pending = s.list_undelivered_for_session(dst, 10).unwrap();
        assert_eq!(
            pending.iter().map(|x| x.id).collect::<Vec<_>>(),
            vec![m],
            "the moved session inherits its undelivered mail"
        );
    }

    #[test]
    fn pending_input_round_trips_and_defaults_to_none() {
        // The reconcile upsert (`Store::apply_host_reconcile`) is the only
        // production writer of this column; seed it directly here with a
        // raw UPDATE, the same way `map_session_row`/`encode_pending_input`
        // read and write it, to check the round trip without a dedicated
        // single-row setter.
        let s = store();
        let id = s
            .upsert_session("a", "local", None, None, 1, 1, "running", None)
            .unwrap();
        let pi = PendingInput {
            kind: "permission".into(),
            question: Some("Do it?".into()),
            options: vec![PendingOption {
                n: 1,
                label: "Yes".into(),
                selected: true,
                checked: false,
            }],
            multi: false,
            detail: None,
        };
        s.conn_ref()
            .execute(
                "UPDATE sessions SET pending_input = ?2 WHERE id = ?1",
                rusqlite::params![id, encode_pending_input(Some(&pi))],
            )
            .unwrap();
        assert_eq!(
            s.get_session_by_id(id).unwrap().unwrap().pending_input,
            Some(pi)
        );
        s.conn_ref()
            .execute(
                "UPDATE sessions SET pending_input = NULL WHERE id = ?1",
                rusqlite::params![id],
            )
            .unwrap();
        assert_eq!(
            s.get_session_by_id(id).unwrap().unwrap().pending_input,
            None
        );

        // An older hub's JSON (no key at all) still parses.
        let row: SessionRow = serde_json::from_str(
            r#"{"id":1,"tmux_name":"s","host_alias":"h","created_at":0,
                "last_activity_at":0,"status":"running","kind":"work","turn_seq":0}"#,
        )
        .unwrap();
        assert!(row.pending_input.is_none());
    }

    #[test]
    fn lost_resumable_session_named_needs_lost_and_a_claude_id() {
        let s = store();
        let id_of = |name: &str| s.get_session(name, "local").unwrap().unwrap().id;
        for name in ["live", "lost-with", "lost-without"] {
            s.upsert_session(name, "local", None, None, 1, 1, "running", None)
                .unwrap();
        }
        s.set_claude_session_id(id_of("live"), "u-live").unwrap();
        s.set_claude_session_id(id_of("lost-with"), "u-lost")
            .unwrap();
        s.mark_session_killed(id_of("lost-with"), 50).unwrap();
        s.mark_session_killed(id_of("lost-without"), 50).unwrap();

        let hit = s
            .lost_resumable_session_named("local", "lost-with")
            .unwrap()
            .expect("lost row with a conversation");
        assert_eq!(hit.id, id_of("lost-with"));
        assert!(s
            .lost_resumable_session_named("local", "lost-without")
            .unwrap()
            .is_none());
        assert!(s
            .lost_resumable_session_named("local", "live")
            .unwrap()
            .is_none());
        assert!(s
            .lost_resumable_session_named("other", "lost-with")
            .unwrap()
            .is_none());
        assert!(s
            .lost_resumable_session_named("local", "nope")
            .unwrap()
            .is_none());
    }

    #[test]
    fn session_with_claude_id_matches_only_its_host() {
        let s = store();
        s.upsert_host("other").unwrap();
        s.upsert_session("a", "local", None, None, 1, 1, "running", None)
            .unwrap();
        s.upsert_session("b", "other", None, None, 1, 1, "running", None)
            .unwrap();
        let a = s.get_session("a", "local").unwrap().unwrap().id;
        let b = s.get_session("b", "other").unwrap().unwrap().id;
        s.set_claude_session_id(a, "u-a").unwrap();
        s.set_claude_session_id(b, "u-b").unwrap();
        s.mark_session_killed(a, 50).unwrap();

        let hit = s.session_with_claude_id("local", "u-a").unwrap();
        assert_eq!(hit.map(|r| r.id), Some(a), "a lost holder still matches");
        assert!(s.session_with_claude_id("local", "u-b").unwrap().is_none());
        assert!(s.session_with_claude_id("other", "u-a").unwrap().is_none());
        assert_eq!(
            s.session_with_claude_id("other", "u-b")
                .unwrap()
                .map(|r| r.id),
            Some(b)
        );
    }

    #[test]
    fn upsert_bg_session_writes_kind_and_flips_a_misfiled_row() {
        let s = store();
        s.upsert_bg_session("local", "bg:u1", None, "u1", Some("idle"), 1, "bg", 1)
            .unwrap();
        s.upsert_bg_session("local", "bg:u1", None, "u1", Some("idle"), 2, "external", 2)
            .unwrap();
        assert_eq!(
            s.get_session("bg:u1", "local").unwrap().unwrap().kind,
            "external"
        );
    }

    #[test]
    fn cleanup_ghosts_external_rows_too() {
        let s = store();
        s.upsert_bg_session("local", "bg:e1", None, "e1", Some("idle"), 1, "external", 1)
            .unwrap();
        s.ghost_and_clean_bg_sessions("local", &[], 10, None, None)
            .unwrap();
        assert_eq!(
            s.get_session("bg:e1", "local").unwrap().unwrap().status,
            "ghost"
        );
        s.ghost_and_clean_bg_sessions("local", &[], 20, None, None)
            .unwrap();
        assert!(s.get_session("bg:e1", "local").unwrap().is_none());
    }

    /// F4: `local` ghosts said `working` a day after loss and a `mac` ghost
    /// said `blocked` — a dialog nobody can answer. Loss keeps identity
    /// (`claude_session_id`, names, project) and drops what only a live pane
    /// can vouch for.
    #[test]
    fn mark_host_sessions_lost_clears_the_fields_only_a_live_pane_can_vouch_for() {
        let mut s = store();
        let r = reconcile_one(&mut s, "a", Some("blocked"), Some("press_enter"), None);
        s.conn_ref()
            .execute(
                "UPDATE sessions SET current_activity = 'waiting for permission: rm -rf' WHERE id = ?1",
                [r.id],
            )
            .unwrap();
        s.mark_host_sessions_lost("local", "host_reboot", &[], 500, 0)
            .unwrap();
        let g = s.get_session("a", "local").unwrap().unwrap();
        assert_eq!(g.status, "ghost");
        assert_eq!(
            g.claude_status, None,
            "nobody can answer a dead pane's dialog"
        );
        assert_eq!(g.stuck_kind, None);
        assert_eq!(g.stuck_since, None);
        assert_eq!(g.current_activity, None);
        assert_eq!(g.pending_input, None);
        assert_eq!(
            crate::service::attention::needs_attention(&g).map(|a| a.reason),
            Some(crate::service::attention::Reason::Lifecycle)
        );
    }

    /// F6: an `external` row (a Code-tab / terminal Claude fleet only
    /// observes) can never be resumed, so the 14 d TTL bought nothing — but a
    /// desktop merely restarting must not lose its rows either. One hour.
    #[test]
    fn a_lost_external_row_is_kept_for_the_grace_then_reaped() {
        let s = store();
        s.upsert_host("h").unwrap();
        s.upsert_bg_session("h", "bg:e1", None, "e1", Some("idle"), 1, "external", 1)
            .unwrap();
        s.mark_host_sessions_lost("h", "host_reboot", &[], 500, 0)
            .unwrap();
        // Inside the grace (lost at 500, grace cutoff 400): kept, though it
        // was already ghost before this pass.
        s.ghost_and_clean_bg_sessions("h", &[], 600, None, Some(400))
            .unwrap();
        assert!(
            s.get_session("bg:e1", "h").unwrap().is_some(),
            "a desktop restart must not reap its rows"
        );
        // Past it (cutoff 700 > lost_at 500): gone.
        s.ghost_and_clean_bg_sessions("h", &[], 4200, None, Some(700))
            .unwrap();
        assert!(s.get_session("bg:e1", "h").unwrap().is_none());
    }

    #[test]
    fn a_rebooted_external_row_is_reaped_inside_the_lost_ttl() {
        // A `host_reboot` verdict marks every kind lost, and a lost row with
        // a claude id is kept to the TTL so it can be resumed. Restore never
        // resumes an `external` row (fleet did not start it), so keeping one
        // only piles dead rows into the "Outside fleet" group. A `bg` row in
        // the same state keeps the exemption.
        let s = store();
        s.upsert_host("h").unwrap();
        s.upsert_bg_session("h", "bg:e1", None, "e1", Some("idle"), 1, "external", 1)
            .unwrap();
        s.upsert_bg_session("h", "bg:b1", None, "b1", Some("idle"), 1, "bg", 1)
            .unwrap();
        let lost = s
            .mark_host_sessions_lost("h", "host_reboot", &[], 500, 0)
            .unwrap();
        assert_eq!(lost.marked.len(), 2);

        let cutoff = Some(100); // lost_at 500 is well inside the TTL
        s.ghost_and_clean_bg_sessions("h", &[], 600, cutoff, None)
            .unwrap();
        assert!(
            s.get_session("bg:e1", "h").unwrap().is_none(),
            "a lost external row must be reaped, not kept to the TTL"
        );
        assert!(
            s.get_session("bg:b1", "h").unwrap().is_some(),
            "a lost bg row keeps the TTL exemption"
        );
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
            .upsert_bg_session("h", "bg:u1", None, "u1", Some("working"), 1, "bg", 1)
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
            .upsert_bg_session("h", "bg:u1", None, "u1", Some("working"), 1, "bg", 1)
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
            .upsert_bg_session("h", "bg:u1", None, "u1", Some("working"), 1, "bg", 1)
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
        // The probe must be newer than the loss for the resurrect to apply
        // (#170), and Phase 1 stamped `lost_at` with the real `now_unix()`.
        s.apply_host_reconcile(HostReconcile {
            sessions: &[ReconcileSession {
                tmux_name: "a",
                created_at: 1,
                last_activity_at: 2,
                ..Default::default()
            }],
            keep: &["a".to_string()],
            probe_started_at: now_unix() + 1,
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
        s.upsert_bg_session("local", "bg:u2", None, "u2", Some("stopped"), 1, "bg", 1)
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
            .upsert_bg_session(
                "alpha",
                "bg:u1",
                None,
                "u1",
                Some("working"),
                100,
                "bg",
                100,
            )
            .unwrap();
        store
            .insert_session_event(id, "status_change", None)
            .unwrap();
        // A bg row on ANOTHER host must never be touched.
        let other = store
            .upsert_bg_session("beta", "bg:u9", None, "u9", Some("working"), 100, "bg", 100)
            .unwrap();
        bus.take();

        // Pass 1: agent vanished → row is ghosted (soft), not deleted.
        store
            .ghost_and_clean_bg_sessions("alpha", &[], 200, None, None)
            .unwrap();
        let row = store.get_session_by_id(id).unwrap().expect("still present");
        assert_eq!(row.status, "ghost");
        assert_eq!(row.lost_at, Some(200));
        assert!(bus.take().contains(&format!("session:updated:{id}")));

        // Pass 2: still vanished → hard-deleted, events reaped, kill emitted.
        store
            .ghost_and_clean_bg_sessions("alpha", &[], 300, None, None)
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
            .upsert_bg_session(
                "alpha",
                "bg:live",
                None,
                "live",
                Some("working"),
                100,
                "bg",
                100,
            )
            .unwrap();
        // A normal tmux-backed row — ghosted or not, the bg pruner must skip it.
        store
            .upsert_session("work-a", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store.apply_host_reconcile(empty_probe("alpha", 1)).unwrap(); // ghosts work-a

        let keep = vec!["bg:live".to_string()];
        store
            .ghost_and_clean_bg_sessions("alpha", &keep, 200, None, None)
            .unwrap();
        store
            .ghost_and_clean_bg_sessions("alpha", &keep, 300, None, None)
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
            .upsert_bg_session(
                "alpha",
                "bg:u1",
                None,
                "u1",
                Some("working"),
                100,
                "bg",
                100,
            )
            .unwrap();
        s.ghost_and_clean_bg_sessions("alpha", &[], 200, None, None)
            .unwrap();
        assert_eq!(s.get_session_by_id(id).unwrap().unwrap().status, "ghost");

        // Agent reappears (e.g. the previous probe transiently failed).
        let id2 = s
            .upsert_bg_session(
                "alpha",
                "bg:u1",
                None,
                "u1",
                Some("working"),
                300,
                "bg",
                300,
            )
            .unwrap();
        assert_eq!(id2, id, "same row, not a new one");
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.status, "running");
        assert_eq!(row.lost_at, None);
    }

    // ── #172: a lost bg row is revived only by an observation newer than the
    // loss, as the tmux upsert's rows already were (#170) ────────────────────

    /// A `bg:u1` row on `alpha` ghosted at `lost_at` by the pane-less pruner
    /// (`lost_reason='missing'`), ready for a stale observation.
    fn ghosted_bg_row(s: &Store, lost_at: i64) -> i64 {
        let id = s
            .upsert_bg_session("alpha", "bg:u1", None, "u1", Some("working"), 1, "bg", 1)
            .unwrap();
        s.ghost_and_clean_bg_sessions("alpha", &[], lost_at, None, None)
            .unwrap();
        assert_eq!(
            s.get_session_by_id(id).unwrap().unwrap().status,
            "ghost",
            "precondition: the row is lost"
        );
        id
    }

    /// One `upsert_bg_session` that saw `bg:u1` live and `working`, as a
    /// probe that STARTED at `probe_started_at`.
    fn observe_bg_live(s: &Store, probe_started_at: i64) {
        s.upsert_bg_session(
            "alpha",
            "bg:u1",
            None,
            "u1",
            Some("working"),
            probe_started_at,
            "bg",
            probe_started_at,
        )
        .unwrap();
    }

    #[test]
    fn a_bg_probe_older_than_the_loss_does_not_resurrect_the_row() {
        // The pass listed `claude agents` BEFORE the row was ghosted (or
        // before the reboot verdict) and only wrote afterwards. It cannot
        // show the agent alive after the loss, so it must leave the ghost —
        // and its `lost_reason` — exactly as it found them.
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("alpha").unwrap();
        let id = ghosted_bg_row(&s, 200);

        observe_bg_live(&s, 199);

        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.status, "ghost", "a stale probe must not revive");
        assert_eq!(row.lost_at, Some(200), "lost_at must be left alone");
        assert_eq!(lost_reason_of(&s, id), Some("missing".to_string()));
        assert_eq!(
            row.last_activity_at, 1,
            "nor may it move the activity stamp"
        );
    }

    #[test]
    fn a_bg_probe_newer_than_the_loss_resurrects_the_row() {
        // The transient-empty-listing case the one-cycle grace exists for:
        // the agent is really back, so the row comes back with it.
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("alpha").unwrap();
        let id = ghosted_bg_row(&s, 200);

        observe_bg_live(&s, 201);

        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.status, "running", "a newer probe revives");
        assert_eq!(row.lost_at, None);
        assert_eq!(lost_reason_of(&s, id), None);
    }

    #[test]
    fn a_bg_probe_started_in_the_same_second_as_the_loss_does_not_resurrect() {
        // Both stamps are unix SECONDS off one clock, so same-instant is not
        // newer — the same choice the tmux upsert makes.
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("alpha").unwrap();
        let id = ghosted_bg_row(&s, 200);

        observe_bg_live(&s, 200);

        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.status, "ghost", "same second is not newer");
        assert_eq!(row.lost_at, Some(200));
    }

    #[test]
    fn a_bg_row_kept_over_a_reboot_is_revived_by_a_fresh_probe_only() {
        // A `host_reboot` verdict marks bg rows lost too (only
        // `tmux_server_gone` is tmux-only), and such rows are kept as ghosts
        // so the agent can be picked up again. A probe from before the
        // verdict must not clear it; one from after must.
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("alpha").unwrap();
        let id = s
            .upsert_bg_session("alpha", "bg:u1", None, "u1", Some("working"), 1, "bg", 1)
            .unwrap();
        s.mark_host_sessions_lost("alpha", "host_reboot", &[], 200, 0)
            .unwrap();
        assert_eq!(lost_reason_of(&s, id), Some("host_reboot".to_string()));

        observe_bg_live(&s, 199);
        assert_eq!(
            s.get_session_by_id(id).unwrap().unwrap().status,
            "ghost",
            "a probe older than the verdict must not revive the row"
        );
        assert_eq!(lost_reason_of(&s, id), Some("host_reboot".to_string()));

        observe_bg_live(&s, 201);
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.status, "running", "the host came back: revive");
        assert_eq!(row.lost_at, None);
        assert_eq!(row.claude_session_id.as_deref(), Some("u1"));
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
            .upsert_bg_session(
                "alpha",
                "bg:u1",
                None,
                "u1",
                Some("working"),
                100,
                "bg",
                100,
            )
            .unwrap();
        s.ghost_and_clean_bg_sessions("alpha", &[], 200, None, None)
            .unwrap();
        assert_eq!(
            lost_reason_of(&s, id),
            Some("missing".to_string()),
            "precondition: the ghost carries a reason"
        );

        s.upsert_bg_session(
            "alpha",
            "bg:u1",
            None,
            "u1",
            Some("working"),
            300,
            "bg",
            300,
        )
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
    fn bg_reconcile_hard_delete_reaps_the_timeline_but_tombstones_the_inbox_instead_of_deleting_it()
    {
        // Previously named `..._reaps_timeline_and_inbox_...` and asserted
        // `list_inbox(bg, ...)` was empty after the hard-delete — that
        // assertion encoded the pre-existing defect Task 13 fixes (this
        // shares `Store::ghost_and_clean` with the tmux reconcile path, so
        // the same bulk-delete used to destroy undelivered mail here too).
        // The message now survives; only the participant identity is
        // tombstoned.
        let store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        let bg = store
            .upsert_bg_session(
                "alpha",
                "bg:gone",
                None,
                "uuid-gone",
                Some("idle"),
                1,
                "bg",
                1,
            )
            .unwrap();
        let peer = store
            .upsert_session("peer", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store
            .insert_session_event(bg, "status_change", Some("idle"))
            .unwrap();
        let to_gone = store
            .insert_message(peer, bg, "to the gone bg", "message", None)
            .unwrap();
        store
            .insert_message(bg, peer, "from the gone bg", "message", None)
            .unwrap();
        let participant = store.participant_for_session(bg).unwrap().unwrap().id;
        // Two passes without the agent: ghost, then hard-delete.
        store
            .ghost_and_clean_bg_sessions("alpha", &[], 10, None, None)
            .unwrap();
        assert!(
            store.get_session_by_id(bg).unwrap().is_some(),
            "ghosted first"
        );
        store
            .ghost_and_clean_bg_sessions("alpha", &[], 20, None, None)
            .unwrap();
        assert!(store.get_session_by_id(bg).unwrap().is_none());
        assert!(
            store.list_session_events(bg, 10).unwrap().is_empty(),
            "the timeline goes with the row"
        );
        assert!(
            store.get_message(to_gone).unwrap().is_some(),
            "a hard-deleted bg row must not destroy undelivered mail"
        );
        let p = store.participant_by_id(participant).unwrap().unwrap();
        assert!(p.retired_at.is_some(), "the identity is tombstoned instead");
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
    fn delete_session_reaps_the_timeline_but_tombstones_the_inbox_instead_of_deleting_it() {
        // Previously named `..._reaps_timeline_and_inbox_...` and asserted
        // `list_inbox(dead, ...)` was empty after the kill — that assertion
        // encoded the pre-existing defect Task 13 fixes (a kill, and
        // therefore a move, used to destroy every undelivered message
        // addressed to the session). The message now survives; only the
        // participant identity is tombstoned, and `service/gc.rs` sweeps the
        // mail itself after the retention window.
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
        let to_dead = store
            .insert_message(peer, dead, "to the dead", "message", None)
            .unwrap();
        store
            .insert_message(dead, peer, "from the dead", "message", None)
            .unwrap();
        let task = store.insert_task(Some(peer), Some(dead), "p", "n").unwrap();
        let participant = store.participant_for_session(dead).unwrap().unwrap().id;

        store.delete_session(dead).unwrap();

        assert!(store.list_session_events(dead, 10).unwrap().is_empty());
        assert!(
            store.get_message(to_dead).unwrap().is_some(),
            "a kill must not destroy undelivered mail"
        );
        let p = store.participant_by_id(participant).unwrap().unwrap();
        assert!(p.retired_at.is_some(), "the identity is tombstoned instead");
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

    /// Migration 121: a shell session runs no agent, and the agent follows a
    /// kind change into and out of `shell` and survives a re-probe.
    #[test]
    fn the_agent_follows_a_move_into_and_out_of_shell() {
        let store = Store::open_in_memory().expect("store");
        store.upsert_host("alpha").unwrap();
        let id = store
            .upsert_session("s", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        let agent = || store.get_session_by_id(id).unwrap().unwrap().agent;
        assert_eq!(agent(), AGENT_CLAUDE, "a discovered row runs Claude Code");
        store.set_session_kind(id, "shell", None).unwrap();
        assert_eq!(agent(), AGENT_SHELL);
        store
            .upsert_session("s", "alpha", None, None, 1, 2, "running", None)
            .unwrap();
        assert_eq!(agent(), AGENT_SHELL, "a re-probe keeps the agent");
        store.set_session_kind(id, "work", None).unwrap();
        assert_eq!(agent(), AGENT_CLAUDE);
        // A kind change that does not touch shell leaves a stored agent be.
        let before = store.get_session_by_id(id).unwrap().unwrap().row_version;
        store.set_session_agent(id, AGENT_CODEX).unwrap();
        let after = store.get_session_by_id(id).unwrap().unwrap().row_version;
        assert!(after > before, "a client sees the agent change");
        store.set_session_kind(id, "review", None).unwrap();
        assert_eq!(agent(), AGENT_CODEX);
        store
            .upsert_session("s", "alpha", None, None, 1, 3, "running", None)
            .unwrap();
        assert_eq!(agent(), AGENT_CODEX, "a re-probe keeps a Codex row Codex");
        // The column refuses a name migration 121 does not know.
        assert!(store.set_session_agent(id, "gemini").is_err());
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
            s.upsert_bg_session("local", name, None, &name[3..], Some("idle"), 1, kind, 1)
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
    fn set_external_agent_name_writes_only_external_rows_and_only_on_change() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        for (name, kind) in [("bg:u-bg", "bg"), ("bg:u-ext", "external")] {
            s.upsert_bg_session("local", name, None, &name[3..], Some("idle"), 1, kind, 1)
                .unwrap();
        }
        let row = s
            .set_external_agent_name("local", "bg:u-ext", "Release cut")
            .unwrap()
            .expect("first label is a change");
        assert_eq!(row.friendly_name.as_deref(), Some("Release cut"));
        assert!(
            s.set_external_agent_name("local", "bg:u-ext", "Release cut")
                .unwrap()
                .is_none(),
            "an unchanged name is no write and no event"
        );
        assert!(s
            .set_external_agent_name("local", "bg:u-bg", "review-pr-42")
            .unwrap()
            .is_none());
        let bg = s.get_session("bg:u-bg", "local").unwrap().unwrap();
        assert_eq!(bg.friendly_name, None, "a bg row keeps its own label");
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
        // Same rule for pending_input: a malformed column reads as no dialog.
        assert_eq!(decode_pending_input(Some("not json".into())), None);
        assert_eq!(decode_pending_input(None), None);
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
        s.upsert_bg_session("local", "bg:u1", None, "u1", Some("working"), 1, "bg", 1)
            .unwrap();
        assert_eq!(
            s.get_session("bg:u1", "local").unwrap().unwrap().idle_since,
            None
        );
        s.upsert_bg_session("local", "bg:u1", None, "u1", Some("completed"), 2, "bg", 2)
            .unwrap();
        let stamp = s
            .get_session("bg:u1", "local")
            .unwrap()
            .unwrap()
            .idle_since
            .expect("stamped");
        s.upsert_bg_session("local", "bg:u1", None, "u1", Some("completed"), 3, "bg", 3)
            .unwrap();
        assert_eq!(
            s.get_session("bg:u1", "local").unwrap().unwrap().idle_since,
            Some(stamp)
        );
        s.upsert_bg_session("local", "bg:u1", None, "u1", Some("working"), 4, "bg", 4)
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
        assert_eq!(
            bus.take(),
            vec![
                format!("session:event:{id}:playbook_applied"),
                format!("session:updated:{id}")
            ]
        );
        let events = s.list_session_events(id, 10).unwrap();
        assert!(events
            .iter()
            .any(|e| e.kind == "playbook_applied" && e.detail.as_deref() == Some("oom:recreate")));
    }

    /// F2: two `working` rows on trn had not moved for ~40 h.
    #[test]
    fn age_out_stale_working_demotes_a_quiet_working_row_and_leaves_the_rest() {
        let s = store();
        let quiet = s
            .upsert_session("quiet", "local", None, None, 1, 1_000, "running", None)
            .unwrap();
        let busy = s
            .upsert_session("busy", "local", None, None, 1, 9_500, "running", None)
            .unwrap();
        let sh = s
            .upsert_session("sh-term", "local", None, None, 1, 1_000, "running", None)
            .unwrap();
        s.conn_ref()
            .execute(
                "UPDATE sessions SET claude_status = 'working' WHERE id IN (?1, ?2, ?3)",
                rusqlite::params![quiet, busy, sh],
            )
            .unwrap();
        s.conn_ref()
            .execute("UPDATE sessions SET kind = 'shell' WHERE id = ?1", [sh])
            .unwrap();
        // A reconcile pass observed all three just now.
        s.conn_ref()
            .execute("UPDATE sessions SET last_reconciled_at = 9990", [])
            .unwrap();

        let demoted = s.age_out_stale_working(10_000, 1_800).unwrap();
        assert_eq!(
            demoted.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![quiet]
        );
        let q = s.get_session_by_id(quiet).unwrap().unwrap();
        assert_eq!(q.claude_status.as_deref(), Some("idle"));
        assert_eq!(q.stale_working_at, Some(10_000));
        assert_eq!(q.idle_since, Some(10_000));
        assert_eq!(
            s.get_session_by_id(busy)
                .unwrap()
                .unwrap()
                .claude_status
                .as_deref(),
            Some("working"),
            "tmux session activity 500 s ago is not stale"
        );
        assert_eq!(
            s.get_session_by_id(sh).unwrap().unwrap().stale_working_at,
            None,
            "a shell has no turns to miss"
        );
        let kinds: Vec<String> = s
            .list_session_events(quiet, 10)
            .unwrap()
            .into_iter()
            .map(|e| e.kind)
            .collect();
        assert!(kinds.contains(&"stale_working".to_string()));
        assert!(kinds.contains(&"status_change".to_string()));
        // Stamped rows are not judged again: a later sweep demotes only
        // `busy` (unstamped, and quiet since 9_500 by then), never `quiet`
        // a second time; `0` turns the rule off.
        s.conn_ref()
            .execute("UPDATE sessions SET last_reconciled_at = 19990", [])
            .unwrap();
        assert_eq!(
            s.age_out_stale_working(20_000, 1_800)
                .unwrap()
                .iter()
                .map(|r| r.id)
                .collect::<Vec<_>>(),
            vec![busy]
        );
        assert!(s.age_out_stale_working(40_000, 0).unwrap().is_empty());
        // The next prompt clears the stamp.
        s.record_prompt_submit_hook_for_row(quiet).unwrap();
        assert_eq!(
            s.get_session_by_id(quiet)
                .unwrap()
                .unwrap()
                .stale_working_at,
            None
        );
    }

    /// Review follow-up (F1): the sweep judges only rows a reconcile pass
    /// has observed within the window. A host that went offline (or was
    /// never probed) stops stamping `last_reconciled_at`, and its quiet
    /// `working` rows must not all turn `idle` with a `stale_working`
    /// reason; the same row seen by a pass inside the window is demoted,
    /// and the read-back carries the demotion's memory (`stale_demoted_at`,
    /// a server-only `SessionRow` field).
    #[test]
    fn age_out_stale_working_judges_only_rows_a_pass_observed() {
        let s = store();
        let id = s
            .upsert_session("dark", "local", None, None, 1, 1_000, "running", None)
            .unwrap();
        let never = s
            .upsert_session("never", "local", None, None, 1, 1_000, "running", None)
            .unwrap();
        s.conn_ref()
            .execute(
                "UPDATE sessions SET claude_status = 'working', last_reconciled_at = 2000 \
                 WHERE id = ?1",
                [id],
            )
            .unwrap();
        s.conn_ref()
            .execute(
                "UPDATE sessions SET claude_status = 'working' WHERE id = ?1",
                [never],
            )
            .unwrap();

        // Last observed at 2_000, judged at 10_000 with a 1_800 s window:
        // the host has been dark since, nothing is known.
        assert!(s.age_out_stale_working(10_000, 1_800).unwrap().is_empty());
        for row_id in [id, never] {
            let row = s.get_session_by_id(row_id).unwrap().unwrap();
            assert_eq!(row.claude_status.as_deref(), Some("working"));
            assert_eq!(row.stale_working_at, None);
            assert_eq!(row.stale_demoted_at, None);
            assert!(
                !s.list_session_events(row_id, 10)
                    .unwrap()
                    .iter()
                    .any(|e| e.kind == "stale_working"),
                "an unobserved row gets no stale_working event"
            );
        }

        // A pass sees it again (still no hook, no turn, no spinner): now
        // the quiet spell is evidence.
        s.conn_ref()
            .execute(
                "UPDATE sessions SET last_reconciled_at = 9950 WHERE id = ?1",
                [id],
            )
            .unwrap();
        let demoted = s.age_out_stale_working(10_000, 1_800).unwrap();
        assert_eq!(demoted.iter().map(|r| r.id).collect::<Vec<_>>(), vec![id]);
        assert_eq!(demoted[0].stale_demoted_at, Some(10_000));
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.claude_status.as_deref(), Some("idle"));
        assert_eq!(row.stale_working_at, Some(10_000));
        assert_eq!(
            row.stale_demoted_at,
            Some(10_000),
            "get_session_by_id reads the demotion's memory"
        );
        assert!(s
            .list_session_events(id, 10)
            .unwrap()
            .iter()
            .any(|e| e.kind == "stale_working"));
        // An attach acknowledges the reason; the row still carries the
        // demotion.
        assert!(s.touch_session(id).unwrap());
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.stale_working_at, None);
        assert_eq!(row.stale_demoted_at, Some(10_000));
        assert_eq!(
            s.get_session_by_id(never)
                .unwrap()
                .unwrap()
                .claude_status
                .as_deref(),
            Some("working"),
            "a row no pass ever observed is never judged"
        );
    }

    /// 2026-09-28: the stamp outlived its point. It lifts when the row is
    /// working or blocked again (a pane read lifted the demotion without a
    /// hook) and after `ttl_secs`; a fresh stamp on an idle row stays.
    #[test]
    fn expire_stale_working_lifts_a_resumed_or_old_stamp_and_keeps_a_fresh_one() {
        let s = store();
        let mk = |name: &str, status: &str, at: i64| -> i64 {
            let id = s
                .upsert_session(name, "local", None, None, 1, 1, "running", None)
                .unwrap();
            s.conn_ref()
                .execute(
                    "UPDATE sessions SET claude_status = ?2, stale_working_at = ?3 WHERE id = ?1",
                    rusqlite::params![id, status, at],
                )
                .unwrap();
            id
        };
        let fresh = mk("fresh", "idle", 9_000);
        let old = mk("old", "idle", 1_000);
        let resumed = mk("resumed", "working", 9_000);
        let asking = mk("asking", "blocked", 9_000);

        let mut lifted: Vec<i64> = s
            .expire_stale_working(10_000, 3_600)
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect();
        lifted.sort();
        let mut want = vec![old, resumed, asking];
        want.sort();
        assert_eq!(lifted, want);
        assert_eq!(
            s.get_session_by_id(fresh)
                .unwrap()
                .unwrap()
                .stale_working_at,
            Some(9_000),
            "a fresh stamp on an idle row still asks for a look"
        );

        // `0`: never by age — but a resumed row still lifts.
        let ancient = mk("ancient", "idle", 1);
        let back = mk("back", "working", 1);
        let lifted: Vec<i64> = s
            .expire_stale_working(10_000, 0)
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(lifted, vec![back]);
        assert_eq!(
            s.get_session_by_id(ancient)
                .unwrap()
                .unwrap()
                .stale_working_at,
            Some(1)
        );
    }

    /// Final review of the acknowledgement: `stale_demoted_at` (the veto's
    /// memory, migration 080) outlives the attention stamp. Every hook
    /// clears both; the TTL clears only the stamp; a row working or blocked
    /// again clears both — silently when the stamp was already gone, since
    /// nothing a client sees changes.
    #[test]
    fn stale_demoted_at_ends_with_a_hook_or_a_resumed_row_but_not_the_ttl() {
        let (s, bus) = crate::store::test_support::store_with_recorder();
        s.upsert_host("local").unwrap();
        let mk = |name: &str, status: &str, stamp: Option<i64>| -> i64 {
            let id = s
                .upsert_session(name, "local", None, None, 1, 1, "running", None)
                .unwrap();
            s.conn_ref()
                .execute(
                    "UPDATE sessions SET claude_status = ?2, stale_working_at = ?3, \
                         stale_demoted_at = 500 WHERE id = ?1",
                    rusqlite::params![id, status, stamp],
                )
                .unwrap();
            id
        };
        let demoted = |name: &str| {
            s.get_session(name, "local")
                .unwrap()
                .unwrap()
                .stale_demoted_at
                .is_some()
        };
        let stamp = |id: i64| s.get_session_by_id(id).unwrap().unwrap().stale_working_at;

        // Every hook write clears both.
        let stop = mk("stop", "idle", Some(500));
        s.record_stop_hook_for_row(stop).unwrap();
        let prompt = mk("prompt", "idle", Some(500));
        s.record_prompt_submit_hook_for_row(prompt).unwrap();
        let end = mk("end", "idle", Some(500));
        s.record_session_end_hook_for_row(end).unwrap();
        let fail = mk("fail", "idle", Some(500));
        s.record_stop_failure_hook_for_row(fail).unwrap();
        let note = mk("note", "idle", Some(500));
        s.record_notification_hook_for_row(
            note,
            crate::service::pane_intel::ClaudeStatus::Blocked,
            None,
        )
        .unwrap();
        let start = mk("start", "idle", None);
        s.clear_ended_turn_state(start).unwrap();
        for (id, name) in [
            (stop, "stop"),
            (prompt, "prompt"),
            (end, "end"),
            (fail, "fail"),
            (note, "note"),
            (start, "start"),
        ] {
            assert_eq!(stamp(id), None, "{name}: the hook clears the stamp");
            assert!(!demoted(name), "{name}: the hook clears the veto");
        }

        // The TTL ends the reason, not the demotion.
        let expired = mk("expired", "idle", Some(1_000));
        // A pane read lifted the demotion (`working` / `blocked` again).
        let resumed = mk("resumed", "working", Some(9_000));
        let asking = mk("asking", "blocked", Some(9_000));
        // Acknowledged earlier (stamp gone), working again: veto only.
        let acked = mk("acked", "working", None);
        // Acknowledged and still idle: nothing to lift.
        let quiet = mk("quiet", "idle", None);
        bus.take();
        let mut lifted: Vec<i64> = s
            .expire_stale_working(10_000, 3_600)
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect();
        lifted.sort();
        let mut want = vec![expired, resumed, asking];
        want.sort();
        assert_eq!(lifted, want, "only a lifted stamp is returned");
        assert_eq!(stamp(expired), None);
        assert!(demoted("expired"), "the TTL keeps the veto");
        assert!(!demoted("resumed"), "a working row lifts the veto");
        assert!(!demoted("asking"), "a blocked row lifts the veto");
        assert!(
            !demoted("acked"),
            "an acknowledged working row lifts it too"
        );
        assert!(demoted("quiet"), "an acknowledged idle row keeps it");
        let events = bus.take();
        for id in [expired, resumed, asking] {
            let ev = format!("session:updated:{id}");
            assert_eq!(
                events.iter().filter(|e| **e == ev).count(),
                1,
                "{id} is emitted once: {events:?}"
            );
        }
        for id in [acked, quiet] {
            assert!(
                !events.contains(&format!("session:updated:{id}")),
                "{id}: a veto-only change is not a visible change: {events:?}"
            );
        }
    }

    #[test]
    fn count_oom_recreates_since_counts_recreates_and_failures_not_refusals() {
        let s = store();
        let id = s
            .upsert_session("a", "local", None, None, 1, 1, "running", None)
            .unwrap();
        for detail in [
            "oom:recreate",
            "oom:recreate:failed:boom",
            "oom:recreate:skipped:working",
            "press_enter:press_enter",
        ] {
            s.mark_playbook_applied(id, 100, detail).unwrap();
        }
        let now = now_unix();
        assert_eq!(s.count_oom_recreates_since(id, now - 60).unwrap(), 2);
        assert_eq!(s.count_oom_recreates_since(id, now + 60).unwrap(), 0);
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

    /// The Stop stamps are all made, but the row reads `failed` (F3), so a
    /// 429 no longer hides as a plain idle prompt.
    #[test]
    fn record_stop_failure_hook_stamps_like_stop_but_reads_failed() {
        let s = Store::open_in_memory().unwrap();
        hooked_session(&s);
        let a = s.record_stop_failure_hook("uuid-h").unwrap().unwrap();
        assert_eq!(a.claude_status.as_deref(), Some("failed"));
        assert_eq!(a.turn_seq, 1);
        assert_eq!(a.last_stop_at, a.last_turn_at);
        assert_eq!(a.last_stop_at, last_hook_at(&s));
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

    #[test]
    fn bus_move_progress_records_expected_event() {
        use crate::events::{MoveProgress, MoveStep, MoveStepState};
        let bus = std::sync::Arc::new(crate::events::RecordingEventBus::new());
        let dyn_bus: std::sync::Arc<dyn crate::events::EventBus> = bus.clone();
        let s = crate::store::Store::open_with_bus_in_memory(dyn_bus).expect("open");
        s.bus_move_progress(&MoveProgress {
            session_id: 7,
            to_host: "beta".into(),
            step: MoveStep::Git,
            index: 4,
            total: 9,
            state: MoveStepState::Done,
            detail: None,
        });
        assert_eq!(bus.take(), vec!["move:progress:7:git:done"]);
    }

    /// Migration 063: `row_version` moves once per UPDATE that changes a
    /// column, and not for one that changes nothing (or only the
    /// reconcile's `last_reconciled_at` stamp). An explicit
    /// `row_version + 1` (a `work` change the row's own columns do not
    /// show) still moves it by exactly one.
    #[test]
    fn row_version_bumps_once_per_visible_change_and_rides_the_row() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("sess", "local", None, None, 1, 1, "running", None)
            .unwrap();
        let v = || s.get_session_by_id(id).unwrap().unwrap().row_version;
        let v0 = v();
        s.set_friendly_name("local", "sess", Some("one")).unwrap();
        let v1 = v();
        s.set_started_at(id, 99).unwrap();
        let v2 = v();
        assert_eq!(v1, v0 + 1, "a changing UPDATE bumps row_version by one");
        assert_eq!(v2, v1 + 1, "every changing UPDATE bumps it");
        // The upsert's DO UPDATE arm is an UPDATE too.
        s.upsert_session("sess", "local", None, None, 1, 2, "running", None)
            .unwrap();
        let v3 = v();
        assert_eq!(v3, v2 + 1, "an upsert that changes the row bumps it");

        // Same values back: no client-visible change, no bump.
        s.upsert_session("sess", "local", None, None, 1, 2, "running", None)
            .unwrap();
        s.conn
            .execute(
                "UPDATE sessions SET status = status, friendly_name = 'one' WHERE id = ?1",
                [id],
            )
            .unwrap();
        assert_eq!(v(), v3, "a same-value UPDATE must not bump row_version");
        // The reconcile's freshness stamp alone is bookkeeping, not a change.
        s.conn
            .execute(
                "UPDATE sessions SET last_reconciled_at = 12345 WHERE id = ?1",
                [id],
            )
            .unwrap();
        assert_eq!(
            v(),
            v3,
            "a last_reconciled_at-only UPDATE must not bump row_version"
        );
        // The explicit bump still counts exactly once (the trigger's WHEN
        // sees row_version itself moved and stays out of it).
        s.conn
            .execute(
                "UPDATE sessions SET row_version = row_version + 1 WHERE id = ?1",
                [id],
            )
            .unwrap();
        assert_eq!(v(), v3 + 1, "an explicit bump moves row_version by one");
        // A column that is not on the wire but is not the reconcile's
        // per-pass stamp either (here `transcript_path`) still counts: only
        // the listed bookkeeping columns are exempt.
        s.conn
            .execute(
                "UPDATE sessions SET transcript_path = '/t.jsonl' WHERE id = ?1",
                [id],
            )
            .unwrap();
        assert_eq!(v(), v3 + 2, "a real change to any other column bumps");
    }

    #[test]
    fn prompt_submit_seq_counts_submits_and_reports_whether_hooks_were_ever_seen() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("sess", "local", None, None, 1, 1, "running", None)
            .unwrap();
        let st = s.prompt_ack_state(id).unwrap().expect("row exists");
        assert_eq!(st.prompt_submit_seq, 0);
        assert!(!st.hooks_seen, "no hook has stamped this row yet");
        s.record_prompt_submit_hook_for_row(id).unwrap();
        s.record_prompt_submit_hook_for_row(id).unwrap();
        let st = s.prompt_ack_state(id).unwrap().unwrap();
        assert_eq!(st.prompt_submit_seq, 2);
        assert!(st.hooks_seen);
        assert!(s.prompt_ack_state(999_999).unwrap().is_none());
    }

    /// The composer's delivery receipt reads the counter off the row the
    /// hook emits: "Claude is on it" is the count moving past the one the
    /// send started from, so the emitted row must carry the new count.
    #[test]
    fn prompt_submit_seq_rides_the_emitted_row() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("sess", "local", None, None, 1, 1, "running", None)
            .unwrap();
        assert_eq!(
            s.get_session_by_id(id).unwrap().unwrap().prompt_submit_seq,
            0
        );
        let emitted = s
            .record_prompt_submit_hook_for_row(id)
            .unwrap()
            .expect("the hook emits the row");
        assert_eq!(emitted.prompt_submit_seq, 1);
        assert_eq!(
            s.get_session_by_id(id).unwrap().unwrap().prompt_submit_seq,
            1
        );
    }

    #[test]
    fn set_started_at_emits_session_updated() {
        let bus = std::sync::Arc::new(crate::events::RecordingEventBus::new());
        let s = Store::open_with_bus_in_memory(bus.clone()).unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("sess", "local", None, None, 1, 1, "running", None)
            .unwrap();
        bus.take();
        s.set_started_at(id, 42).unwrap();
        assert!(
            bus.names().contains(&"session:updated"),
            "started_at must reach the UI by event, not only by re-list: {:?}",
            bus.names()
        );
        assert_eq!(
            s.get_session_by_id(id).unwrap().unwrap().started_at,
            Some(42)
        );
    }

    /// Migration 099's conversation-owner record, the INSERT half: a session
    /// row that arrives with both a `claude_session_id` and an owner is
    /// recorded by `trg_conversation_owner_on_session_insert`, and the record
    /// is written by a TRIGGER rather than by a call at each write site for
    /// the reason 045 gives — `claude_session_id` has three writers today
    /// and a fourth must not be able to skip it.
    ///
    /// `INSERT OR IGNORE` means the FIRST owner wins: a later
    /// re-attribution of the session (a claim, a move, an operator's
    /// correction) must not rewrite who the conversation belonged to, which
    /// is the only question the resume gate asks.
    #[test]
    fn an_insert_of_a_row_with_a_claude_session_id_and_an_owner_records_a_conversation_owner() {
        let s = store();
        let ada = s.create_person("ada", None).unwrap().id;
        let bob = s.create_person("bob", None).unwrap().id;
        // A row that arrives owned, as T5's create path will write it.
        s.conn
            .execute(
                "INSERT INTO sessions (tmux_name, host_alias, created_at, last_activity_at, \
                                       status, claude_session_id, owner_person_id, visibility) \
                 VALUES ('owned', 'local', 1, 1, 'running', 'conv-a', ?1, 'private')",
                rusqlite::params![ada],
            )
            .unwrap();
        assert_eq!(s.conversation_owner("conv-a").unwrap(), Some(ada));

        // A row that arrives unowned records nothing — there is no owner to
        // record, and `unclaimed` is not a person.
        let later = seed(&s, "unowned");
        s.set_claude_session_id(later, "conv-b").unwrap();
        assert_eq!(s.conversation_owner("conv-b").unwrap(), None);

        // …until it is owned, which the UPDATE trigger catches. Both orders
        // of the two writes therefore end up recorded.
        s.conn
            .execute(
                "UPDATE sessions SET owner_person_id = ?1, visibility = 'private' WHERE id = ?2",
                rusqlite::params![bob, later],
            )
            .unwrap();
        assert_eq!(s.conversation_owner("conv-b").unwrap(), Some(bob));

        // First writer wins: re-attributing the session leaves the record.
        s.conn
            .execute(
                "UPDATE sessions SET owner_person_id = ?1 WHERE id = ?2",
                rusqlite::params![ada, later],
            )
            .unwrap();
        assert_eq!(
            s.conversation_owner("conv-b").unwrap(),
            Some(bob),
            "a later re-attribution must not rewrite the record"
        );
        assert_eq!(s.conversation_owner("never-seen").unwrap(), None);
    }

    /// `claim_if_unclaimed` (multi-user M1, T5) — the second half of the create
    /// seam, and the one a create path hard-fails on.
    ///
    /// Four answers, and the two that are NOT errors are the interesting ones:
    /// "no person to attribute it to" leaves the row `unclaimed`, and "already
    /// this very person" is the ordinary case, because the reconcile upsert
    /// usually claimed the row from the reservation before this ever runs.
    #[test]
    fn claim_if_unclaimed_claims_once_and_never_re_owns() {
        let s = store();
        let ada = s.create_person("ada", None).unwrap().id;
        let bob = s.create_person("bob", None).unwrap().id;
        let id = seed(&s, "fresh");
        let before = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(before.owner_person_id, None);
        assert_eq!(before.visibility, VISIBILITY_UNCLAIMED);

        // No owner: nothing to do, and the row stays nobody's. Not an error —
        // a per-host token's session legitimately has no person.
        assert!(!s.claim_if_unclaimed(id, None).unwrap());
        assert_eq!(
            s.get_session_by_id(id).unwrap().unwrap().visibility,
            VISIBILITY_UNCLAIMED
        );

        // The claim.
        assert!(s.claim_if_unclaimed(id, Some(ada)).unwrap());
        let owned = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(owned.owner_person_id, Some(ada));
        assert_eq!(owned.visibility, VISIBILITY_PRIVATE);
        assert!(
            owned.row_version > before.row_version,
            "an open client must learn the row is now private"
        );

        // Again, same person: no write, no error — this is what the create
        // path sees once the upsert has claimed the row from the reservation.
        assert!(!s.claim_if_unclaimed(id, Some(ada)).unwrap());
        assert_eq!(
            s.get_session_by_id(id).unwrap().unwrap().row_version,
            owned.row_version,
            "a no-op claim writes nothing"
        );

        // Somebody else: refused, loudly. A create path that got here is about
        // to return another person's session as the one it just made.
        let e = s
            .claim_if_unclaimed(id, Some(bob))
            .expect_err("never re-owned");
        assert_eq!(e.code, crate::ipc_error::codes::E_FORBIDDEN);
        assert_eq!(
            s.get_session_by_id(id).unwrap().unwrap().owner_person_id,
            Some(ada)
        );

        // A row that is gone is `E_NOTFOUND`, not a silent success.
        s.delete_session(id).unwrap();
        assert_eq!(
            s.claim_if_unclaimed(id, Some(ada))
                .expect_err("no row")
                .code,
            crate::ipc_error::codes::E_NOTFOUND
        );
        // …but with no owner to write there is still nothing to fail at.
        assert!(!s.claim_if_unclaimed(id, None).unwrap());
    }

    /// `lost_session_named_owned_by_other` (multi-user M1, T5): the read behind
    /// `reject_lost_session_name`'s second refusal.
    ///
    /// The row it exists for is the one the resumable guard misses — lost, with
    /// NO `claude_session_id` — because reconcile's `ON CONFLICT DO UPDATE`
    /// revives exactly that row with its old owner intact. The NULL handling is
    /// the point: a caller who is nobody must be refused by every owned lost
    /// row, not pass as its owner.
    #[test]
    fn a_lost_row_is_only_anothers_when_somebody_else_owns_it() {
        let s = store();
        let ada = s.create_person("ada", None).unwrap().id;
        let bob = s.create_person("bob", None).unwrap().id;
        // Ada's lost row, no conversation: invisible to the resumable guard.
        let adas = seed(&s, "shared-name");
        s.conn
            .execute(
                "UPDATE sessions SET owner_person_id = ?1, visibility = 'private' WHERE id = ?2",
                rusqlite::params![ada, adas],
            )
            .unwrap();
        s.mark_session_killed(adas, 100).unwrap().expect("ghosted");
        assert!(
            s.lost_resumable_session_named("local", "shared-name")
                .unwrap()
                .is_none(),
            "the resumable guard does not see it — that is why this read exists"
        );

        let hit = |person| {
            s.lost_session_named_owned_by_other("local", "shared-name", person)
                .unwrap()
                .map(|r| r.id)
        };
        assert_eq!(hit(Some(bob)), Some(adas), "it is not bob's to reuse");
        assert_eq!(
            hit(None),
            Some(adas),
            "a caller who is nobody is not the owner either — two NULLs must              never compare equal"
        );
        assert_eq!(
            hit(Some(ada)),
            None,
            "ada's own lost row does not block her"
        );

        // An UNOWNED lost row blocks nobody: the row it revives is `unclaimed`,
        // which is no one's, so there is nothing to inherit.
        let nobodys = seed(&s, "free-name");
        s.mark_session_killed(nobodys, 100)
            .unwrap()
            .expect("ghosted");
        for person in [Some(ada), Some(bob), None] {
            assert_eq!(
                s.lost_session_named_owned_by_other("local", "free-name", person)
                    .unwrap()
                    .map(|r| r.id),
                None
            );
        }
        // A LIVE row of somebody else's is not this read's business either —
        // tmux refuses a duplicate name long before the row would matter.
        let live = seed(&s, "live-name");
        s.conn
            .execute(
                "UPDATE sessions SET owner_person_id = ?1, visibility = 'private' WHERE id = ?2",
                rusqlite::params![ada, live],
            )
            .unwrap();
        assert!(s
            .lost_session_named_owned_by_other("local", "live-name", Some(bob))
            .unwrap()
            .is_none());
        // And another host's lost row is another host's problem.
        s.upsert_host("other").unwrap();
        assert!(s
            .lost_session_named_owned_by_other("other", "shared-name", Some(bob))
            .unwrap()
            .is_none());
    }

    /// The record's whole purpose: it outlives the session.
    ///
    /// `new_session { resume_claude_session_id }` resurrects a transcript
    /// precisely when the row is GONE, so the gate T10 builds cannot read
    /// live rows. `delete_session` deletes the `sessions` row outright and
    /// `conversations` is `ON DELETE CASCADE` on it (037), so both of the
    /// places a `claude_session_id` otherwise lives disappear with the
    /// session. `conversation_owners` has no foreign key to either table, and
    /// this is the test that would fail if one were added.
    #[test]
    fn a_reaped_sessions_conversation_owner_survives_delete_session() {
        let s = store();
        let ada = s.create_person("ada", None).unwrap().id;
        let id = seed(&s, "doomed");
        s.set_claude_session_id(id, "conv-gone").unwrap();
        s.conn
            .execute(
                "UPDATE sessions SET owner_person_id = ?1, visibility = 'private' WHERE id = ?2",
                rusqlite::params![ada, id],
            )
            .unwrap();
        assert_eq!(s.conversation_owner("conv-gone").unwrap(), Some(ada));
        let conversations: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM conversations WHERE session_id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(conversations > 0, "the rebind opened one");

        s.delete_session(id).unwrap();
        let count = |sql: &str| -> i64 { s.conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(
            count(&format!("SELECT COUNT(*) FROM sessions WHERE id = {id}")),
            0,
            "the row is gone, not lost"
        );
        assert_eq!(
            count(
                "SELECT COUNT(*) FROM conversations \
                 WHERE claude_session_id = 'conv-gone'"
            ),
            0,
            "and its conversation cascaded away with it"
        );
        assert_eq!(
            s.conversation_owner("conv-gone").unwrap(),
            Some(ada),
            "the owner record is what is left to refuse a resume with"
        );
    }
}
