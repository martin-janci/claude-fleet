//! The reconcile write burst: one transaction per host, events emitted
//! after commit.

use super::*;

/// Translate `HostReconcile::probe_started_at` into the `last_reconciled_at`
/// cutoff used by [`Store::ghost_and_clean`]: rows stamped at or after the
/// probe start are protected, and `0` ("no guard") protects nothing.
fn ghost_cutoff(probe_started_at: i64) -> i64 {
    if probe_started_at <= 0 {
        i64::MAX
    } else {
        probe_started_at
    }
}

impl Store {
    // ---- Reconcile write-burst: single transaction + emit-after-commit ----
    //
    // The `*_in_tx` helpers below run ONLY their SQL against an ambient
    // `&Transaction` and return a `RowChange` describing the event to emit —
    // they do NOT touch `self.bus`. `apply_host_reconcile` drives them inside
    // one transaction, commits, and only THEN flushes the collected changes to
    // the bus. A mid-batch error rolls the whole transaction back, so no event
    // fires for a write that didn't persist.
    //
    // The public `update_host_probe` / `upsert_session` /
    // `touch_project_last_session_at` / `delete_sessions_not_in` methods are
    // intentionally left untouched — direct (non-reconcile) callers keep
    // emitting immediately.
    //
    // MAINTENANCE: each `*_in_tx` helper deliberately mirrors the SQL of its
    // public twin (same column lists, same upsert ON CONFLICT clause, same
    // SELECT-ids-before-DELETE). They differ ONLY in: (a) `tx` vs `self.conn`,
    // and (b) collecting a `RowChange` vs emitting via `self.bus`. If you change
    // a schema/SQL detail in a public method, change its `_in_tx` twin too.
    // Both paths are test-covered (direct: the `*_emits_*` event tests; tx: the
    // `apply_host_reconcile` rollback + happy-path tests), so a divergence will
    // surface as a test failure rather than silent corruption. The ghost /
    // reap pass is the exception: `ghost_and_clean` is ONE function shared by
    // this write-burst (tmux rows, stale-probe guard on) and by the public
    // `ghost_and_clean_bg_sessions` (pane-less rows, own transaction), so the
    // two prunes cannot drift apart.
    //
    // `worktree_key` is written by `upsert_session_in_tx` ONLY — the public
    // `upsert_session` intentionally omits it (reconcile is the only path that
    // knows the session's cwd and can compute the key).

    fn update_host_probe_in_tx(
        tx: &rusqlite::Transaction,
        alias: &str,
        reachable: bool,
        claude_version: Option<&str>,
        tmux_version: Option<&str>,
        last_pinged_at: i64,
        out: &mut Vec<RowChange>,
    ) -> Result<(), rusqlite::Error> {
        tx.execute(
            "UPDATE hosts SET reachable=?1, claude_version=?2, tmux_version=?3, last_pinged_at=?4 WHERE alias=?5",
            rusqlite::params![
                if reachable { 1 } else { 0 },
                claude_version,
                tmux_version,
                last_pinged_at,
                alias
            ],
        )?;
        if let Some(row) = fetch_host(tx, alias)? {
            out.push(RowChange::HostProbed(row));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn upsert_session_in_tx(
        tx: &rusqlite::Transaction,
        tmux_name: &str,
        host_alias: &str,
        project_id: Option<i64>,
        worktree_id: Option<i64>,
        created_at: i64,
        last_activity_at: i64,
        account_uuid: Option<&str>,
        worktree_key: Option<&str>,
        claude_session_id: Option<&str>,
        claude_status: Option<&str>,
        effort_level: Option<&str>,
        pr_url: Option<&str>,
        current_activity: Option<&str>,
        context_pct: Option<f64>,
        stuck_kind: Option<&str>,
        intel_observed: bool,
        ci_status: Option<&str>,
        pr_observed: bool,
        probe_started_at: i64,
        tmux_pane_id: Option<&str>,
        out: &mut Vec<RowChange>,
    ) -> Result<(), rusqlite::Error> {
        // Read the prior row (not just its id) before the write: it tells us
        // created-vs-updated AND, after the write, whether anything the
        // frontend can see actually changed. Reconcile upserts every live
        // session every pass; without this diff each pass emitted one
        // `session:updated` per session — sixty store flushes per tick for a
        // fleet that had not changed at all (BE-11 / FE-10). Note that
        // `update_host_probe_in_tx` still emits one `host:probed` per host
        // per pass: `last_pinged_at` moves every time, so it cannot be diffed
        // away — one event per host, not per session.
        let prior: Option<SessionRow> = fetch_session(tx, tmux_name, host_alias)?;

        // The post-write stuck_kind, spelled out once and reused: SQLite's
        // upsert SET clauses see the OLD row (unqualified) and the candidate
        // (`excluded`), never each other's results.
        const NEW_STUCK: &str = "CASE WHEN ?16 THEN excluded.stuck_kind \
                                 ELSE COALESCE(excluded.stuck_kind, stuck_kind) END";
        // The post-write claude_status. A Stop hook that landed at or after
        // this pass's probe STARTED (`last_stop_at >= ?20`) is fresher than
        // the pane the pass captured, so its `idle` must win over the pane
        // heuristic (MCP-1: reconcile used to clobber the hook every tick).
        // `?20 <= 0` disables the guard (store-level tests pass 0).
        // `last_hook_at` is stamped by BOTH hooks (Stop → idle,
        // UserPromptSubmit → working). The guard only covers passes that were
        // already in flight when the hook landed; a pass that starts later
        // observes the pane afresh and wins, as it should.
        const NEW_STATUS: &str = "CASE WHEN ?20 > 0 AND last_hook_at IS NOT NULL \
                                            AND last_hook_at >= ?20 \
                                       THEN claude_status \
                                       ELSE COALESCE(excluded.claude_status, claude_status) END";
        // The candidate claude_session_id, refused when another live row on
        // the host already holds it: `claude agents`' cwd match can be
        // ambiguous, and one conversation must never be bound to two rows
        // (the hooks' pane binding settles it). Evaluated in VALUES, so
        // `excluded.claude_session_id` below is already the guarded value and
        // the id write, the transcript reset and the stale flag all share
        // this one condition.
        const GUARDED_ID: &str = "CASE WHEN EXISTS (SELECT 1 FROM sessions o \
                                      WHERE o.claude_session_id = ?9 AND o.host_alias = ?2 \
                                        AND o.tmux_name != ?1 AND o.status != 'ghost') \
                                  THEN NULL ELSE ?9 END";
        // The pass moves the row onto another conversation. SET clauses see
        // the OLD row, so this compares against the prior id.
        const ID_CHANGES: &str = "excluded.claude_session_id IS NOT NULL \
                                  AND excluded.claude_session_id IS NOT claude_session_id";
        // A hook/transcript context value younger than 120 s outranks the
        // pane footer (spec §1.5) — unless it belongs to the conversation
        // this pass moves the row away from.
        const FRESH_CONTEXT: &str = "context_source IN ('transcript','hook') \
                                     AND context_at >= ?19 - 120";
        let sql = format!(
            "INSERT INTO sessions (tmux_name, host_alias, project_id, worktree_id,
                                   created_at, last_activity_at, status, account_uuid,
                                   worktree_key, lost_at,
                                   claude_session_id, claude_status, effort_level, pr_url, current_activity,
                                   context_pct, stuck_kind, ci_status, idle_since, stuck_since,
                                   tmux_pane_id, context_source, context_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'running', ?7, ?8, NULL, {guarded_id}, ?10, ?11, ?12, ?13,
                     ?14, ?15, ?17,
                     CASE WHEN ?10 IN ('idle','completed','stopped') THEN ?19 ELSE NULL END,
                     CASE WHEN ?15 IS NULL THEN NULL ELSE ?19 END,
                     ?21,
                     CASE WHEN ?14 IS NULL THEN NULL ELSE 'pane' END,
                     CASE WHEN ?14 IS NULL THEN NULL ELSE ?19 END)
             ON CONFLICT(host_alias, tmux_name) DO UPDATE SET
               project_id=excluded.project_id,
               last_activity_at=excluded.last_activity_at,
               account_uuid=COALESCE(excluded.account_uuid, account_uuid),
               worktree_key=COALESCE(excluded.worktree_key, worktree_key),
               status=CASE WHEN status='ghost' THEN 'running' ELSE status END,
               lost_at=NULL,
               claude_session_id=COALESCE(excluded.claude_session_id, claude_session_id),
               -- A new conversation: the old transcript is not its transcript,
               -- and the old context size is not its size.
               transcript_path=CASE WHEN {id_changes} THEN NULL ELSE transcript_path END,
               context_stale=CASE WHEN {id_changes} THEN 1 ELSE context_stale END,
               tmux_pane_id=COALESCE(excluded.tmux_pane_id, tmux_pane_id),
               claude_status={new_status},
               effort_level=COALESCE(excluded.effort_level, effort_level),
               -- pr_url / ci_status are authoritative when the gh probe ran
               -- this pass (?18) so a closed PR's link clears; otherwise the
               -- prior values are preserved.
               pr_url=CASE WHEN ?18 THEN excluded.pr_url ELSE COALESCE(excluded.pr_url, pr_url) END,
               ci_status=CASE WHEN ?18 THEN excluded.ci_status
                              ELSE COALESCE(excluded.ci_status, ci_status) END,
               current_activity=COALESCE(excluded.current_activity, current_activity),
               -- The pane footer is a fallback (spec §1.5): it applies only
               -- when no fresh hook/transcript value exists, and a missing
               -- footer never overwrites anything.
               context_pct=CASE WHEN excluded.context_pct IS NULL THEN context_pct
                                WHEN {fresh} AND NOT ({id_changes}) THEN context_pct
                                ELSE excluded.context_pct END,
               context_source=CASE WHEN excluded.context_pct IS NULL THEN context_source
                                   WHEN {fresh} AND NOT ({id_changes}) THEN context_source
                                   ELSE 'pane' END,
               context_at=CASE WHEN excluded.context_pct IS NULL THEN context_at
                               WHEN {fresh} AND NOT ({id_changes}) THEN context_at
                               ELSE ?19 END,
               -- stuck_kind is authoritative when the pane was observed this
               -- pass (?16): a NULL then CLEARS a stale flag. When the pane was
               -- NOT observed (capture failed) we preserve the prior value.
               stuck_kind={new_stuck},
               -- stuck_since: keep the episode start while the kind is
               -- unchanged, restart it when the kind changes, clear when the
               -- flag clears.
               stuck_since=CASE WHEN ({new_stuck}) IS NULL THEN NULL
                                WHEN ({new_stuck}) IS stuck_kind THEN COALESCE(stuck_since, ?19)
                                ELSE ?19 END,
               idle_since={idle}",
            new_stuck = NEW_STUCK,
            new_status = NEW_STATUS,
            idle = idle_since_sql(NEW_STATUS, "?19"),
            guarded_id = GUARDED_ID,
            id_changes = ID_CHANGES,
            fresh = FRESH_CONTEXT,
        );
        tx.execute(
            &sql,
            rusqlite::params![
                tmux_name,
                host_alias,
                project_id,
                worktree_id,
                created_at,
                last_activity_at,
                account_uuid,
                worktree_key,
                claude_session_id,
                claude_status,
                effort_level,
                pr_url,
                current_activity,
                context_pct,
                stuck_kind,
                intel_observed,
                ci_status,
                pr_observed,
                now_unix(),
                probe_started_at,
                tmux_pane_id
            ],
        )?;
        if let Some(row) = fetch_session(tx, tmux_name, host_alias)? {
            match prior {
                None => out.push(RowChange::SessionCreated(row)),
                // Every wire field identical ⇒ a no-op pass; emit nothing.
                Some(ref before) if *before == row => {}
                Some(_) => out.push(RowChange::SessionUpdated(row)),
            }
        }
        Ok(())
    }

    fn touch_project_last_session_at_in_tx(
        tx: &rusqlite::Transaction,
        project_id: i64,
        ts: i64,
        out: &mut Vec<RowChange>,
    ) -> Result<(), rusqlite::Error> {
        // Same no-op filter as `upsert_session_in_tx`: the MAX() update
        // matches the row every pass even when `last_session_at` is already
        // at least `ts`, so diff before/after instead of trusting the
        // affected-row count.
        let before = fetch_project(tx, project_id)?;
        tx.execute(
            "UPDATE projects SET last_session_at = MAX(COALESCE(last_session_at, 0), ?1) WHERE id = ?2",
            rusqlite::params![ts, project_id],
        )?;
        if let Some(row) = fetch_project(tx, project_id)? {
            if before.as_ref() != Some(&row) {
                out.push(RowChange::ProjectUpdated(row));
            }
        }
        Ok(())
    }

    /// Two-phase ghost-then-reap of the rows `kind_filter` selects on one
    /// host, keyed on the set of names this pass observed live. Runs only its
    /// SQL against `tx` and pushes what to announce onto `out`; the caller
    /// commits and flushes (`apply_host_reconcile` for tmux rows,
    /// [`Store::ghost_and_clean_bg_sessions`] for pane-less ones).
    ///
    /// Phase 1: rows not in `keep_names` that are currently live (`status !=
    /// 'ghost'`) are soft-deleted by setting `status='ghost'` and `lost_at=now`.
    /// Phase 2: rows that were already ghost BEFORE this pass and are still
    /// not in `keep_names` are hard-deleted, together with their
    /// `session_events` timeline and the messages addressed to them (neither
    /// table has an FK cascade). The one-cycle grace is what makes a
    /// transient probe miss recoverable: the row is only ghosted, and the
    /// next upsert resurrects it.
    ///
    /// `kind_filter` is [`KIND_TMUX`] or [`KIND_PANE_LESS`]: tmux-backed rows
    /// and pane-less (`bg` / `external`) rows are pruned by different callers
    /// against different `keep` sets (the tmux list vs `claude agents
    /// --json`), so each pruner must leave the other's rows alone.
    ///
    /// `cutoff` (unix secs, see [`ghost_cutoff`]) guards Phase 1 against a
    /// stale probe: a row stamped `last_reconciled_at >= cutoff` was observed
    /// live by a writer whose probe began after this one's, so its absence
    /// from `keep_names` only means this probe is older than the row (e.g. a
    /// tick that listed tmux just before `new_session` created it). Such rows
    /// are left alone; the next pass, whose probe starts later, judges them.
    /// `None` disables the guard (the pane-less pruner has no such race).
    pub(super) fn ghost_and_clean(
        tx: &rusqlite::Transaction,
        host_alias: &str,
        keep_names: &[String],
        now: i64,
        kind_filter: &str,
        cutoff: Option<i64>,
        out: &mut Vec<RowChange>,
    ) -> Result<(), rusqlite::Error> {
        let not_in = if keep_names.is_empty() {
            String::new()
        } else {
            format!(" AND tmux_name NOT IN ({})", in_clause(keep_names.len()))
        };

        // ── Phase 2 prep: collect already-ghost IDs BEFORE Phase 1 modifies rows
        // so that sessions newly ghosted in Phase 1 are not immediately deleted.
        let pre_ghost_ids: Vec<i64> = {
            let sql = format!(
                "SELECT id FROM sessions
                 WHERE host_alias=?1 AND status='ghost' AND {kind_filter}{not_in}"
            );
            let params = params_then(rusqlite::params![host_alias], keep_names);
            tx.prepare(&sql)?
                .query_map(params.as_slice(), |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };

        // ── Phase 1: ghost live sessions not in keep ──────────────────────────
        // Rows reconciled by a NEWER probe than ours are skipped (see doc).
        let ghost_ids: Vec<i64> = {
            let guard = if cutoff.is_some() {
                " AND COALESCE(last_reconciled_at, 0) < ?3"
            } else {
                ""
            };
            let sql = format!(
                "UPDATE sessions SET status='ghost', lost_at=?1
                 WHERE host_alias=?2 AND status!='ghost' AND {kind_filter}{guard}{not_in}
                 RETURNING id"
            );
            let head: Vec<&dyn rusqlite::ToSql> = match &cutoff {
                Some(c) => vec![&now, &host_alias, c],
                None => vec![&now, &host_alias],
            };
            let params = params_then(&head, keep_names);
            tx.prepare(&sql)?
                .query_map(params.as_slice(), |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for id in &ghost_ids {
            if let Some(row) = fetch_session_by_id(tx, *id)? {
                out.push(RowChange::SessionUpdated(row));
            }
        }

        // ── Phase 2: hard-delete sessions that were already ghost before this cycle
        if !pre_ghost_ids.is_empty() {
            let phs = in_clause(pre_ghost_ids.len());
            // No FK cascade on session_events — delete them with the row or
            // they linger as orphans forever.
            tx.execute(
                &format!("DELETE FROM session_events WHERE session_id IN ({phs})"),
                rusqlite::params_from_iter(&pre_ghost_ids),
            )?;
            // And the messages addressed to them (an inbox nobody can read),
            // as `delete_session` does.
            tx.execute(
                &format!("DELETE FROM session_messages WHERE to_session_id IN ({phs})"),
                rusqlite::params_from_iter(&pre_ghost_ids),
            )?;
            tx.execute(
                &format!("DELETE FROM sessions WHERE id IN ({phs})"),
                rusqlite::params_from_iter(&pre_ghost_ids),
            )?;
            for id in &pre_ghost_ids {
                out.push(RowChange::SessionKilled(*id));
            }
        }

        Ok(())
    }

    /// One probed/live session to apply during a reconcile write-burst, with
    /// its `(project_id, account_uuid)` ALREADY resolved by the caller (those
    /// are reads — `find_project_id_for_path` / `get_session_account` — and
    /// must happen before the transaction opens).
    pub fn apply_host_reconcile(&mut self, spec: HostReconcile<'_>) -> Result<(), rusqlite::Error> {
        // Phase 1: run all SQL inside one transaction, collecting RowChanges.
        let changes = self.with_transaction(|tx| {
            let mut out: Vec<RowChange> = Vec::new();
            Self::update_host_probe_in_tx(
                tx,
                spec.alias,
                spec.reachable,
                spec.claude_version,
                spec.tmux_version,
                spec.last_pinged_at,
                &mut out,
            )?;
            // Only a reachable probe rewrites the session set. An unreachable
            // host keeps its last-known rows (no upserts, no delete-not-in).
            if spec.reachable {
                // Accumulate the latest activity per project, then touch each
                // project ONCE — N sessions in one project would otherwise
                // fire N redundant UPDATEs + N `project:updated` events.
                let mut project_touch: std::collections::HashMap<i64, i64> =
                    std::collections::HashMap::new();
                for sess in spec.sessions {
                    Self::upsert_session_in_tx(
                        tx,
                        sess.tmux_name,
                        spec.alias,
                        sess.project_id,
                        None,
                        sess.created_at,
                        sess.last_activity_at,
                        sess.account_uuid.as_deref(),
                        sess.worktree_key.as_deref(),
                        sess.claude_session_id.as_deref(),
                        sess.claude_status.as_deref(),
                        sess.effort_level.as_deref(),
                        sess.pr_url.as_deref(),
                        sess.current_activity.as_deref(),
                        sess.context_pct,
                        sess.stuck_kind.as_deref(),
                        sess.intel_observed,
                        sess.ci_status.as_deref(),
                        sess.pr_observed,
                        spec.probe_started_at,
                        sess.tmux_pane_id.as_deref(),
                        &mut out,
                    )?;
                    if let Some(pid) = sess.project_id {
                        let latest = project_touch.entry(pid).or_insert(0);
                        *latest = (*latest).max(sess.last_activity_at);
                    }
                }
                for (pid, ts) in project_touch {
                    Self::touch_project_last_session_at_in_tx(tx, pid, ts, &mut out)?;
                }
                Self::ghost_and_clean(
                    tx,
                    spec.alias,
                    spec.keep,
                    now_unix(),
                    KIND_TMUX,
                    Some(ghost_cutoff(spec.probe_started_at)),
                    &mut out,
                )?;
            }
            Ok(out)
        })?;

        // Phase 2: transaction committed — now it is safe to emit.
        for change in &changes {
            self.bus.emit_change(change);
        }
        Ok(())
    }

    /// Stamp `last_reconciled_at = at` on the sessions a reconcile pass just
    /// observed live on `host_alias` (the `keep` set). This is the proactive
    /// freshness marker the Wave-2 background tick (Task H) relies on so the
    /// frontend can gray out rows whose host has gone quiet. Best-effort and
    /// emit-free: it does not change any user-visible row field, so it neither
    /// fires row events nor aborts reconcile on failure.
    pub fn mark_sessions_reconciled(
        &self,
        host_alias: &str,
        keep_names: &[String],
        at: i64,
    ) -> Result<usize, rusqlite::Error> {
        if keep_names.is_empty() {
            return Ok(0);
        }
        let sql = format!(
            "UPDATE sessions SET last_reconciled_at=?1 \
             WHERE host_alias=?2 AND tmux_name IN ({phs})",
            phs = in_clause(keep_names.len())
        );
        let params = params_then(rusqlite::params![at, host_alias], keep_names);
        self.conn.execute(&sql, params.as_slice())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_support::*;

    #[test]
    fn reconcile_hard_delete_reaps_session_events() {
        let mut store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        let id = store
            .upsert_session("work-a", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store
            .insert_session_event(id, "status_change", None)
            .unwrap();
        // Two empty reconciles: ghost, then hard-delete.
        for ts in [10, 20] {
            store
                .apply_host_reconcile(empty_probe("alpha", ts))
                .unwrap();
        }
        assert!(store.get_session_by_id(id).unwrap().is_none());
        let orphans: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM session_events WHERE session_id=?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphans, 0, "events must not outlive the hard-deleted row");
    }

    #[test]
    fn reconcile_hard_delete_reaps_the_inbox_but_keeps_sent_messages() {
        let mut store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        store.upsert_host("beta").unwrap();
        let id = store
            .upsert_session("work-a", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        let peer = store
            .upsert_session("peer", "beta", None, None, 1, 1, "running", None)
            .unwrap();
        store
            .insert_message(peer, id, "to the gone", "message", None)
            .unwrap();
        store
            .insert_message(id, peer, "from the gone", "message", None)
            .unwrap();
        // Two empty reconciles: ghost, then hard-delete.
        for ts in [10, 20] {
            store
                .apply_host_reconcile(empty_probe("alpha", ts))
                .unwrap();
        }
        assert!(store.get_session_by_id(id).unwrap().is_none());
        assert!(
            store.list_inbox(id, false, 10).unwrap().is_empty(),
            "an inbox nobody can read goes with the row"
        );
        assert_eq!(
            store.list_inbox(peer, false, 10).unwrap().len(),
            1,
            "a message the gone session SENT stays in the recipient's inbox"
        );
    }

    /// One live `ReconcileSession` for the no-op / changed-row event tests.
    fn live_session(tmux_name: &'static str, pid: i64, activity: i64) -> ReconcileSession<'static> {
        ReconcileSession {
            tmux_name,
            project_id: Some(pid),
            created_at: 1,
            last_activity_at: activity,
            worktree_key: Some("main".to_string()),
            claude_status: Some("idle".to_string()),
            context_pct: Some(12.5),
            intel_observed: true,
            ..Default::default()
        }
    }

    #[test]
    fn upsert_session_in_tx_identical_row_pushes_no_change() {
        // Direct, transaction-level check of the BE-11 diff: the same upsert
        // twice yields one `SessionCreated` and then nothing at all; a single
        // changed field yields exactly one `SessionUpdated`.
        let mut store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        let upsert = |store: &mut Store, activity: i64| -> Vec<RowChange> {
            store
                .with_transaction(|tx| {
                    let mut out = Vec::new();
                    Store::upsert_session_in_tx(
                        tx,
                        "s1",
                        "alpha",
                        None,
                        None,
                        1,
                        activity,
                        None,
                        Some("main"),
                        None,
                        Some("idle"),
                        None,
                        None,
                        None,
                        Some(12.5),
                        None,
                        true,
                        None,
                        false,
                        0,
                        None,
                        &mut out,
                    )?;
                    Ok(out)
                })
                .unwrap()
        };
        let first = upsert(&mut store, 10);
        assert_eq!(first.len(), 1);
        assert!(matches!(first[0], RowChange::SessionCreated(_)));
        let second = upsert(&mut store, 10);
        assert!(
            second.is_empty(),
            "identical row must push no change, got {} entries",
            second.len()
        );
        let third = upsert(&mut store, 11);
        assert_eq!(third.len(), 1);
        assert!(matches!(third[0], RowChange::SessionUpdated(_)));
    }

    #[test]
    fn apply_host_reconcile_identical_row_emits_no_session_or_project_event() {
        // BE-11 / FE-10: the tick upserts every live session every pass. A
        // pass that observes exactly what the store already holds must not
        // fan `session:updated` / `project:updated` out to the frontend.
        let (mut store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        let pid = store.upsert_project("o", "r", "/base/r").unwrap();
        let keep = vec!["s1".to_string()];

        // Pass 1: the row is new → created + project touched.
        let sessions = vec![live_session("s1", pid, 10)];
        store
            .apply_host_reconcile(HostReconcile {
                sessions: &sessions,
                keep: &keep,
                ..empty_probe("alpha", 1)
            })
            .unwrap();
        let evts = bus.take();
        assert!(
            evts.iter().any(|e| e.starts_with("session:created:")),
            "first pass creates; got {evts:?}"
        );
        assert!(
            evts.contains(&format!("project:updated:{pid}")),
            "first pass touches the project; got {evts:?}"
        );

        // Pass 2: identical observation → only the host probe stamp moves.
        let sessions = vec![live_session("s1", pid, 10)];
        store
            .apply_host_reconcile(HostReconcile {
                sessions: &sessions,
                keep: &keep,
                ..empty_probe("alpha", 2)
            })
            .unwrap();
        assert_eq!(
            bus.take(),
            vec!["host:probed:alpha".to_string()],
            "an unchanged row must emit neither session nor project events"
        );

        // Pass 3: one field changed → exactly one session:updated and, since
        // last_session_at moves too, exactly one project:updated.
        let sessions = vec![live_session("s1", pid, 20)];
        store
            .apply_host_reconcile(HostReconcile {
                sessions: &sessions,
                keep: &keep,
                ..empty_probe("alpha", 3)
            })
            .unwrap();
        let evts = bus.take();
        assert_eq!(
            evts.iter()
                .filter(|e| e.starts_with("session:updated:"))
                .count(),
            1,
            "changed row emits once; got {evts:?}"
        );
        assert_eq!(
            evts.iter()
                .filter(|e| e.starts_with("project:updated:"))
                .count(),
            1,
            "project touched once; got {evts:?}"
        );
    }

    #[test]
    fn stale_probe_does_not_ghost_row_reconciled_after_its_start() {
        // BE-3: a tick that listed tmux BEFORE `new_session` created a
        // session, but whose write lands AFTER the create's own reconcile
        // stamped the new row, carries a `keep` set without the new name.
        // The row must survive that write; a probe started after the stamp
        // ghosts it normally.
        let (mut store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        let probe_started = 1_000;
        store
            .upsert_session("fresh", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        // Same second as the probe start: the guard must be inclusive.
        store
            .mark_sessions_reconciled("alpha", &["fresh".to_string()], probe_started)
            .unwrap();
        store
            .upsert_session("old", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store
            .mark_sessions_reconciled("alpha", &["old".to_string()], probe_started - 1)
            .unwrap();
        // A row that was never stamped at all is also fair game.
        store
            .upsert_session("never", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        bus.take();

        let stale_write = |store: &mut Store, keep: &[String]| {
            store
                .apply_host_reconcile(HostReconcile {
                    probe_started_at: probe_started,
                    keep,
                    ..empty_probe("alpha", 5)
                })
                .unwrap();
        };
        // Both keep shapes take different SQL paths; exercise each.
        stale_write(&mut store, &[]);
        let status =
            |store: &Store, name: &str| store.get_session(name, "alpha").unwrap().unwrap().status;
        assert_eq!(status(&store, "fresh"), "running", "newer row untouched");
        assert!(store
            .get_session("fresh", "alpha")
            .unwrap()
            .unwrap()
            .lost_at
            .is_none());
        assert_eq!(status(&store, "old"), "ghost", "older row still ghosted");
        assert_eq!(
            status(&store, "never"),
            "ghost",
            "unstamped row still ghosted"
        );
        let evts = bus.take();
        assert_eq!(
            evts.iter()
                .filter(|e| e.starts_with("session:updated:"))
                .count(),
            2,
            "exactly the two ghosted rows emit; got {evts:?}"
        );

        // Non-empty keep path: `fresh` is still absent from keep and still safe.
        stale_write(&mut store, &["unrelated".to_string()]);
        assert_eq!(status(&store, "fresh"), "running");

        // A probe that started after the stamp is authoritative again.
        store
            .apply_host_reconcile(HostReconcile {
                probe_started_at: probe_started + 1,
                ..empty_probe("alpha", 6)
            })
            .unwrap();
        assert_eq!(status(&store, "fresh"), "ghost", "later probe ghosts it");
    }

    #[test]
    fn mark_sessions_reconciled_stamps_only_kept_rows() {
        // Task H freshness marker: the background tick stamps last_reconciled_at
        // on every session it observed live (the keep set) and leaves the rest
        // (and a fresh row's default) NULL.
        let store = Store::open_in_memory().expect("store");
        store.upsert_host("alpha").unwrap();
        store
            .upsert_session("kept", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store
            .upsert_session("gone", "alpha", None, None, 1, 1, "running", None)
            .unwrap();

        let updated = store
            .mark_sessions_reconciled("alpha", &["kept".to_string()], 1234)
            .expect("mark");
        assert_eq!(updated, 1, "only the kept session is stamped");

        let kept_at: Option<i64> = store
            .conn
            .query_row(
                "SELECT last_reconciled_at FROM sessions WHERE tmux_name='kept'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kept_at, Some(1234));

        let gone_at: Option<i64> = store
            .conn
            .query_row(
                "SELECT last_reconciled_at FROM sessions WHERE tmux_name='gone'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(gone_at, None, "non-kept session keeps NULL");

        // Empty keep set is a no-op (no rows touched, no error).
        let none = store
            .mark_sessions_reconciled("alpha", &[], 9999)
            .expect("empty keep");
        assert_eq!(none, 0);
    }

    #[test]
    fn reconcile_clears_stuck_kind_only_when_pane_observed() {
        let (mut store, _bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        let pid = store.upsert_project("o", "r", "/base/r").unwrap();

        let pass = |store: &mut Store, stuck: Option<&str>, observed: bool| {
            let sessions = vec![ReconcileSession {
                tmux_name: "s1",
                project_id: Some(pid),
                created_at: 1,
                last_activity_at: 1,
                stuck_kind: stuck.map(|s| s.to_string()),
                intel_observed: observed,
                ..Default::default()
            }];
            store
                .apply_host_reconcile(HostReconcile {
                    sessions: &sessions,
                    keep: &["s1".to_string()],
                    ..empty_probe("alpha", 1)
                })
                .unwrap();
        };
        let stuck_of = |store: &Store| {
            store
                .get_session("s1", "alpha")
                .unwrap()
                .unwrap()
                .stuck_kind
        };

        // Observed pane, stuck detected → flag stored.
        pass(&mut store, Some("reconnect"), true);
        assert_eq!(stuck_of(&store).as_deref(), Some("reconnect"));

        // Capture FAILED (pane not observed), no stuck → prior flag preserved.
        pass(&mut store, None, false);
        assert_eq!(
            stuck_of(&store).as_deref(),
            Some("reconnect"),
            "must preserve stuck_kind when the pane was not observed"
        );

        // Observed pane, stuck no longer present → flag CLEARED.
        pass(&mut store, None, true);
        assert_eq!(
            stuck_of(&store),
            None,
            "must clear stuck_kind when the pane was observed and shows no stuck state"
        );
    }

    #[test]
    fn apply_host_reconcile_happy_path_persists_all_and_emits_after_commit() {
        let (mut store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        let pid = store.upsert_project("o", "r", "/base/r").unwrap();
        // Pre-seed a stale row that should be pruned (kill), and one that
        // already exists so it produces an `updated` (not `created`).
        store
            .upsert_session("stale", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store
            .upsert_session("keep-existing", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        let stale_id = store.get_session("stale", "alpha").unwrap().unwrap().id;
        bus.take(); // drain all setup events

        let sessions = vec![
            // existing → update
            ReconcileSession {
                tmux_name: "keep-existing",
                project_id: Some(pid),
                created_at: 1,
                last_activity_at: 50,
                worktree_key: Some("main".to_string()),
                ..Default::default()
            },
            // brand new → create
            ReconcileSession {
                tmux_name: "fresh",
                project_id: Some(pid),
                created_at: 10,
                last_activity_at: 60,
                worktree_key: Some("main".to_string()),
                ..Default::default()
            },
        ];
        let keep = vec!["keep-existing".to_string(), "fresh".to_string()];
        store
            .apply_host_reconcile(HostReconcile {
                claude_version: Some("2.1"),
                tmux_version: Some("3.6"),
                sessions: &sessions,
                keep: &keep,
                ..empty_probe("alpha", 999)
            })
            .expect("reconcile ok");

        // (a) rows persisted: stale ghosted, two live, host probe updated.
        let live: Vec<String> = store
            .list_sessions_for_host("alpha")
            .unwrap()
            .into_iter()
            .filter(|r| r.status != "ghost")
            .map(|r| r.tmux_name)
            .collect();
        assert_eq!(live, vec!["fresh", "keep-existing"], "two live sessions");
        let ghosts: Vec<String> = store
            .list_sessions_for_host("alpha")
            .unwrap()
            .into_iter()
            .filter(|r| r.status == "ghost")
            .map(|r| r.tmux_name)
            .collect();
        assert_eq!(ghosts, vec!["stale"], "stale is now ghost");
        let host = store
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|h| h.alias == "alpha")
            .unwrap();
        assert_eq!(host.claude_version.as_deref(), Some("2.1"));
        assert_eq!(host.last_pinged_at, Some(999));
        assert_eq!(store.list_projects().unwrap()[0].last_session_at, Some(60));

        // (b) the events fired — and only after commit (we drained pre-batch, so
        // everything here was emitted by the flush phase).
        let evts = bus.take();
        assert!(
            evts.contains(&"host:probed:alpha".to_string()),
            "got: {evts:?}"
        );
        assert!(
            evts.iter().any(|e| e.starts_with("session:updated:")),
            "expected an update for keep-existing; got: {evts:?}"
        );
        assert!(
            evts.iter().any(|e| e.starts_with("session:created:")),
            "expected a create for fresh; got: {evts:?}"
        );
        assert!(
            evts.contains(&format!("session:updated:{stale_id}")),
            "stale becomes ghost (session:updated); got: {evts:?}"
        );
        assert!(
            evts.iter().any(|e| e.starts_with("project:updated:")),
            "expected project:updated; got: {evts:?}"
        );
    }

    #[test]
    fn external_session_survives_reconcile_with_empty_tmux() {
        // An interactive Claude session outside tmux (`kind='external'`) has no
        // pane either, so the tmux-keyed cleanup must leave it alone on every
        // pass; only `ghost_and_clean_bg_sessions` prunes it.
        let mut store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        store
            .upsert_bg_session(
                "alpha",
                "bg:ext-uuid-1",
                None,
                "ext-uuid-1",
                Some("idle"),
                100,
                "external",
            )
            .unwrap();
        for pass in 1..=3 {
            store
                .apply_host_reconcile(empty_probe("alpha", pass))
                .expect("reconcile ok");
            let row = store
                .get_session("bg:ext-uuid-1", "alpha")
                .unwrap()
                .unwrap_or_else(|| panic!("external row reaped on pass {pass}"));
            assert_eq!(row.kind, "external");
            assert_eq!(row.status, "running", "external row ghosted on pass {pass}");
        }
    }

    #[test]
    fn bg_session_survives_reconcile_with_empty_tmux() {
        // A `kind='bg'` row is never a tmux session, so it never appears in the
        // `keep` set. Ghost cleanup must NOT reap it — even when the host's tmux
        // list is empty and a normal (work) row gets ghosted.
        let mut store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        // A bg row + a normal work row.
        store
            .upsert_bg_session(
                "alpha",
                "bg:sess-uuid-1",
                None,
                "sess-uuid-1",
                Some("working"),
                100,
                "bg",
            )
            .unwrap();
        store
            .upsert_session("work-a", "alpha", None, None, 1, 1, "running", None)
            .unwrap();

        // Reconcile with NO live tmux sessions (empty keep).
        store
            .apply_host_reconcile(empty_probe("alpha", 1))
            .expect("reconcile ok");

        let rows = store.list_sessions_for_host("alpha").unwrap();
        let bg = rows
            .iter()
            .find(|r| r.tmux_name == "bg:sess-uuid-1")
            .expect("bg row must survive");
        assert_eq!(bg.kind, "bg");
        assert_eq!(bg.status, "running", "bg row must NOT be ghosted");
        assert_eq!(bg.claude_session_id.as_deref(), Some("sess-uuid-1"));
        // The plain work row, in contrast, gets ghosted.
        let work = rows.iter().find(|r| r.tmux_name == "work-a").unwrap();
        assert_eq!(work.status, "ghost", "work row IS ghosted when not in tmux");

        // A SECOND reconcile (the bg row is now an old row) still doesn't reap
        // it via the Phase-2 hard-delete.
        store
            .apply_host_reconcile(empty_probe("alpha", 2))
            .expect("reconcile ok");
        assert!(
            store
                .list_sessions_for_host("alpha")
                .unwrap()
                .iter()
                .any(|r| r.tmux_name == "bg:sess-uuid-1"),
            "bg row must survive repeated reconciles"
        );
    }

    #[test]
    fn reconcile_batch_rolls_back_and_emits_nothing_on_error() {
        let (mut store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        bus.take(); // drain host:added

        let row_count = |s: &Store| -> i64 {
            s.conn
                .query_row(
                    "SELECT COUNT(*) FROM sessions WHERE host_alias='alpha'",
                    [],
                    |r| r.get(0),
                )
                .unwrap()
        };
        assert_eq!(row_count(&store), 0);

        // First a good upsert, then one whose project_id points at a
        // non-existent project — with foreign_keys=ON this trips the
        // sessions.project_id FK mid-batch and aborts the transaction.
        let sessions = vec![
            ReconcileSession {
                tmux_name: "good",
                created_at: 1,
                last_activity_at: 1,
                ..Default::default()
            },
            ReconcileSession {
                tmux_name: "bad",
                project_id: Some(999_999), // no such project → FK violation
                created_at: 1,
                last_activity_at: 1,
                ..Default::default()
            },
        ];
        let keep = vec!["good".to_string(), "bad".to_string()];
        let res = store.apply_host_reconcile(HostReconcile {
            claude_version: Some("9.9"),
            sessions: &sessions,
            keep: &keep,
            ..empty_probe("alpha", 12345)
        });

        assert!(res.is_err(), "FK violation should abort the batch");
        // (a) NO rows persisted — not even the 'good' one before the failure.
        assert_eq!(
            row_count(&store),
            0,
            "transaction must have rolled back all writes"
        );
        // host probe row must also be untouched (it was part of the same tx).
        let host = store
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|h| h.alias == "alpha")
            .unwrap();
        assert_ne!(
            host.claude_version.as_deref(),
            Some("9.9"),
            "host probe rolled back"
        );
        assert_eq!(host.last_pinged_at, None, "host probe rolled back");
        // (b) NO events emitted — the flush phase never runs on rollback.
        assert!(
            bus.take().is_empty(),
            "no event may fire for a rolled-back batch"
        );
    }

    #[test]
    fn reconcile_ghosts_sessions_on_first_empty_probe_then_deletes_on_second() {
        let (mut store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        store
            .upsert_session("s1", "alpha", None, None, 1, 10, "running", None)
            .unwrap();
        let s1_id = store.get_session("s1", "alpha").unwrap().unwrap().id;
        bus.take(); // drain setup events

        // First reachable probe with no sessions — s1 should become ghost
        store
            .apply_host_reconcile(empty_probe("alpha", 100))
            .unwrap();

        let s1 = store.get_session_by_id(s1_id).unwrap().unwrap();
        assert_eq!(s1.status, "ghost", "first empty probe should ghost s1");
        assert!(s1.lost_at.is_some(), "lost_at must be set");

        let evts = bus.take();
        assert!(
            evts.iter().any(|e| e.starts_with("session:updated:")),
            "ghost transition should emit session:updated; got: {evts:?}"
        );
        assert!(
            !evts.iter().any(|e| e.starts_with("session:killed:")),
            "no kill event on first cycle; got: {evts:?}"
        );

        // Second reachable probe with no sessions — ghost s1 should be deleted
        store
            .apply_host_reconcile(empty_probe("alpha", 200))
            .unwrap();

        assert!(
            store.get_session_by_id(s1_id).unwrap().is_none(),
            "second empty probe should hard-delete the ghost"
        );
        let evts2 = bus.take();
        assert!(
            evts2.contains(&format!("session:killed:{s1_id}")),
            "second cycle must emit session:killed; got: {evts2:?}"
        );
    }

    #[test]
    fn reconcile_does_not_clobber_a_hook_stamped_idle_newer_than_the_pass() {
        // MCP-1: the reconcile pass captured the pane at T0; the Stop hook
        // stamped idle at T1 >= T0; the pass's write lands at T2 with the
        // stale "working" it derived from the pane. The hook must win.
        let mut s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("a", "local", None, None, 1, 1, "running", None)
            .unwrap();
        s.set_claude_session_id(id, "uuid-a").unwrap();
        let stop_at = s
            .record_stop_hook("uuid-a")
            .unwrap()
            .unwrap()
            .last_stop_at
            .unwrap();
        let write = |s: &mut Store, status: &str, probe_started_at: i64| -> SessionRow {
            s.apply_host_reconcile(HostReconcile {
                probe_started_at,
                sessions: &[ReconcileSession {
                    tmux_name: "a",
                    created_at: 1,
                    last_activity_at: 1,
                    claude_status: Some(status.to_string()),
                    intel_observed: true,
                    ..Default::default()
                }],
                keep: &["a".to_string()],
                ..empty_probe("local", 1)
            })
            .unwrap();
            s.get_session("a", "local").unwrap().unwrap()
        };
        // Pass started before (or at) the hook stamp: hook wins, idle_since kept.
        let row = write(&mut s, "working", stop_at - 5);
        assert_eq!(row.claude_status.as_deref(), Some("idle"));
        assert!(row.idle_since.is_some());
        let row = write(&mut s, "working", stop_at);
        assert_eq!(row.claude_status.as_deref(), Some("idle"));
        // Pass started after the hook stamp: the pane observation is fresher.
        let row = write(&mut s, "working", stop_at + 5);
        assert_eq!(row.claude_status.as_deref(), Some("working"));
        assert_eq!(row.idle_since, None);
        // Guard disabled (0): legacy COALESCE behaviour.
        s.record_stop_hook("uuid-a").unwrap();
        let row = write(&mut s, "working", 0);
        assert_eq!(row.claude_status.as_deref(), Some("working"));
    }

    #[test]
    fn reconcile_preserves_a_submit_stamped_working_status() {
        // S3: UserPromptSubmit stamped `working` at T1; a pass that started at
        // T0 <= T1 derived `idle` from a pane captured before the submit.
        let mut s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("a", "local", None, None, 1, 1, "running", None)
            .unwrap();
        s.set_claude_session_id(id, "uuid-a").unwrap();
        s.record_prompt_submit_hook("uuid-a").unwrap();
        let hook_at: i64 = s
            .conn
            .query_row("SELECT last_hook_at FROM sessions WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        let pass = |s: &mut Store, started: i64| {
            s.apply_host_reconcile(HostReconcile {
                probe_started_at: started,
                sessions: &[ReconcileSession {
                    tmux_name: "a",
                    created_at: 1,
                    last_activity_at: 1,
                    claude_status: Some("idle".into()),
                    intel_observed: true,
                    ..Default::default()
                }],
                keep: &["a".to_string()],
                ..empty_probe("local", 1)
            })
            .unwrap();
            s.get_session("a", "local").unwrap().unwrap()
        };
        let row = pass(&mut s, hook_at - 3);
        assert_eq!(row.claude_status.as_deref(), Some("working"));
        assert_eq!(row.idle_since, None);
        // A pass that started after the submit is authoritative.
        let row = pass(&mut s, hook_at + 3);
        assert_eq!(row.claude_status.as_deref(), Some("idle"));
    }

    #[test]
    fn reconcile_stamps_idle_since_on_entering_idle_and_clears_on_leaving() {
        let mut s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let r = reconcile_one(&mut s, "a", Some("working"), None, None);
        assert_eq!(r.idle_since, None);
        let r = reconcile_one(&mut s, "a", Some("idle"), None, None);
        let stamp = r.idle_since.expect("stamped on entering idle");
        assert!(stamp > 0);
        // Staying idle keeps the ORIGINAL stamp (the GC TTL counts from it).
        let r = reconcile_one(&mut s, "a", Some("completed"), None, None);
        assert_eq!(r.idle_since, Some(stamp));
        let r = reconcile_one(&mut s, "a", Some("working"), None, None);
        assert_eq!(r.idle_since, None);
        // A fresh row inserted already idle is stamped on insert.
        let r = reconcile_one(&mut s, "b", Some("stopped"), None, None);
        assert!(r.idle_since.is_some());
    }

    #[test]
    fn reconcile_tracks_stuck_since_per_episode() {
        let mut s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let r = reconcile_one(&mut s, "a", Some("blocked"), None, None);
        assert_eq!(r.stuck_since, None);
        let r = reconcile_one(&mut s, "a", Some("blocked"), Some("press_enter"), None);
        let start = r.stuck_since.expect("episode start stamped");
        // Same kind on the next pass: the episode start is preserved.
        let r = reconcile_one(&mut s, "a", Some("blocked"), Some("press_enter"), None);
        assert_eq!(r.stuck_since, Some(start));
        // The flag clearing (pane observed, no stuck) clears the stamp …
        let r = reconcile_one(&mut s, "a", Some("working"), None, None);
        assert_eq!(r.stuck_kind, None);
        assert_eq!(r.stuck_since, None);
        // … and a different kind later starts a new episode.
        let r = reconcile_one(&mut s, "a", Some("blocked"), Some("oom"), None);
        assert!(r.stuck_since.is_some());
    }

    #[test]
    fn reconcile_pr_fields_are_authoritative_only_when_probed() {
        let mut s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let r = reconcile_one(
            &mut s,
            "a",
            None,
            None,
            Some((Some("https://github.com/o/r/pull/1"), Some("pending"))),
        );
        assert_eq!(r.pr_url.as_deref(), Some("https://github.com/o/r/pull/1"));
        assert_eq!(r.ci_status.as_deref(), Some("pending"));
        // Unprobed pass (cache fresh): both survive.
        let r = reconcile_one(&mut s, "a", None, None, None);
        assert_eq!(r.pr_url.as_deref(), Some("https://github.com/o/r/pull/1"));
        assert_eq!(r.ci_status.as_deref(), Some("pending"));
        // Probed again, PR now closed: both clear.
        let r = reconcile_one(&mut s, "a", None, None, Some((None, None)));
        assert_eq!(r.pr_url, None);
        assert_eq!(r.ci_status, None);
    }

    const ID_A: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    const ID_B: &str = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";

    #[test]
    fn a_reset_context_is_not_resurrected_by_an_empty_pane_footer() {
        let (mut s, _) = store_with_recorder();
        s.upsert_host("local").unwrap();
        let row = reconcile_one(&mut s, "a", Some("idle"), None, None);
        s.rebind_conversation(row.id, ID_A, StartSource::Clear, None, None)
            .unwrap();
        // pane footer shows nothing this pass
        let after = reconcile_one(&mut s, "a", Some("idle"), None, None);
        assert_eq!(after.context_pct, Some(0.0));
    }

    #[test]
    fn a_fresh_transcript_value_beats_a_pane_value() {
        let (mut s, _) = store_with_recorder();
        s.upsert_host("local").unwrap();
        let row = reconcile_one(&mut s, "a", Some("idle"), None, None);
        s.rebind_conversation(row.id, ID_A, StartSource::Fleet, None, None)
            .unwrap();
        s.set_context(row.id, ID_A, 100_000, 200_000, "transcript", None)
            .unwrap();
        s.apply_host_reconcile(HostReconcile {
            sessions: &[ReconcileSession {
                tmux_name: "a",
                created_at: 1,
                last_activity_at: 1,
                context_pct: Some(12.0),
                intel_observed: true,
                ..Default::default()
            }],
            keep: &["a".to_string()],
            ..empty_probe("local", 1)
        })
        .unwrap();
        let row = s.get_session("a", "local").unwrap().unwrap();
        assert_eq!(row.context_pct, Some(50.0));
        assert_eq!(row.context.context_source.as_deref(), Some("transcript"));
    }

    #[test]
    fn a_stale_transcript_value_yields_to_a_pane_value() {
        let (mut s, _) = store_with_recorder();
        s.upsert_host("local").unwrap();
        let row = reconcile_one(&mut s, "a", Some("idle"), None, None);
        s.rebind_conversation(row.id, ID_A, StartSource::Fleet, None, None)
            .unwrap();
        s.set_context(row.id, ID_A, 100_000, 200_000, "transcript", None)
            .unwrap();
        // Age the transcript value past the 120 s freshness window.
        s.conn
            .execute(
                "UPDATE sessions SET context_at = context_at - 121 WHERE id = ?1",
                [row.id],
            )
            .unwrap();
        s.apply_host_reconcile(HostReconcile {
            sessions: &[ReconcileSession {
                tmux_name: "a",
                created_at: 1,
                last_activity_at: 1,
                context_pct: Some(12.0),
                intel_observed: true,
                ..Default::default()
            }],
            keep: &["a".to_string()],
            ..empty_probe("local", 1)
        })
        .unwrap();
        let row = s.get_session("a", "local").unwrap().unwrap();
        assert_eq!(row.context_pct, Some(12.0));
        assert_eq!(row.context.context_source.as_deref(), Some("pane"));
    }

    #[test]
    fn a_pane_value_applies_when_no_other_source_wrote() {
        let (mut s, _) = store_with_recorder();
        s.upsert_host("local").unwrap();
        reconcile_one(&mut s, "a", Some("idle"), None, None);
        s.apply_host_reconcile(HostReconcile {
            sessions: &[ReconcileSession {
                tmux_name: "a",
                created_at: 1,
                last_activity_at: 1,
                context_pct: Some(12.0),
                tmux_pane_id: Some("%4".into()),
                intel_observed: true,
                ..Default::default()
            }],
            keep: &["a".to_string()],
            ..empty_probe("local", 1)
        })
        .unwrap();
        let row = s.get_session("a", "local").unwrap().unwrap();
        assert_eq!(row.context_pct, Some(12.0));
        assert_eq!(row.context.context_source.as_deref(), Some("pane"));
        assert_eq!(row.context.tmux_pane_id.as_deref(), Some("%4"));
        // A pass without a pane id keeps the last one seen.
        let row = reconcile_one(&mut s, "a", Some("idle"), None, None);
        assert_eq!(row.context.tmux_pane_id.as_deref(), Some("%4"));
    }

    #[test]
    fn an_id_change_from_claude_agents_clears_the_transcript_path() {
        let (mut s, _) = store_with_recorder();
        s.upsert_host("local").unwrap();
        let row = reconcile_one(&mut s, "a", None, None, None);
        s.rebind_conversation(
            row.id,
            ID_A,
            StartSource::Fleet,
            Some("/h/.claude/projects/x/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa.jsonl"),
            None,
        )
        .unwrap();
        s.apply_host_reconcile(HostReconcile {
            sessions: &[ReconcileSession {
                tmux_name: "a",
                created_at: 1,
                last_activity_at: 1,
                claude_session_id: Some(ID_B.into()),
                ..Default::default()
            }],
            keep: &["a".to_string()],
            ..empty_probe("local", 1)
        })
        .unwrap();
        assert_eq!(s.session_transcript_path(row.id).unwrap(), None);
    }

    #[test]
    fn reconcile_never_binds_one_claude_id_to_two_rows() {
        let (mut s, _) = store_with_recorder();
        s.upsert_host("local").unwrap();
        let a = reconcile_one(&mut s, "a", None, None, None);
        s.rebind_conversation(a.id, ID_A, StartSource::Fleet, Some("/t/a.jsonl"), None)
            .unwrap();
        let b = reconcile_one(&mut s, "b", None, None, None);
        s.rebind_conversation(b.id, ID_B, StartSource::Fleet, Some("/t/b.jsonl"), None)
            .unwrap();
        // `claude agents` matched row `b` by cwd to `a`'s conversation.
        s.apply_host_reconcile(HostReconcile {
            sessions: &[
                ReconcileSession {
                    tmux_name: "a",
                    created_at: 1,
                    last_activity_at: 1,
                    claude_session_id: Some(ID_A.into()),
                    ..Default::default()
                },
                ReconcileSession {
                    tmux_name: "b",
                    created_at: 1,
                    last_activity_at: 1,
                    claude_session_id: Some(ID_A.into()),
                    ..Default::default()
                },
            ],
            keep: &["a".to_string(), "b".to_string()],
            ..empty_probe("local", 1)
        })
        .unwrap();
        let b = s.get_session("b", "local").unwrap().unwrap();
        assert_eq!(b.claude_session_id.as_deref(), Some(ID_B));
        // The transcript reset is gated on the same condition.
        assert_eq!(
            s.session_transcript_path(b.id).unwrap().as_deref(),
            Some("/t/b.jsonl")
        );
        assert_eq!(s.sessions_by_claude_id(ID_A).unwrap().len(), 1);
    }
}
