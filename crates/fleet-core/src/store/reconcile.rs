//! The reconcile write burst: one transaction per host, events emitted
//! after commit.

use super::*;

/// Translate `HostReconcile::probe_started_at` into the `last_reconciled_at`
/// cutoff used by [`Store::ghost_and_clean`]: rows stamped at or after the
/// probe start are protected, and `0` ("no guard") protects nothing.
/// `pub(super)`: also reused by [`Store::mark_host_sessions_lost`]
/// (`store/sessions.rs`) for the identical BE-3 guard against the mass-loss
/// verdict marking a row a NEWER reconcile pass already saw live.
pub(super) fn ghost_cutoff(probe_started_at: i64) -> i64 {
    if probe_started_at <= 0 {
        i64::MAX
    } else {
        probe_started_at
    }
}

/// How long (seconds) a kill stays in [`KillMemory`]: double the longest a
/// probe's view of a host can be in flight, DERIVED from the two timeouts
/// that bound it so raising either cannot silently under-size this window.
/// A probe stamps `started_at` before its first await, then spends up to
/// `HOST_PROBE_TIMEOUT` on the host and up to `PR_PROBE_TIMEOUT` on the
/// sequential PR step that follows, and its write still has to queue behind
/// the other hosts' writes — so their sum is the floor, not the bound, and
/// this is twice it.
///
/// Over-keeping an entry costs only the few bytes it occupies: the strict
/// `probe_started_at > killed_at` comparison, not the age of the entry,
/// decides what may be inserted, so a stale entry can never block a pass that
/// probed after the kill. Under-keeping one reopens the race, hence the
/// doubling.
const KILL_MEMORY_SECS: i64 = 2
    * (crate::service::sessions::HOST_PROBE_TIMEOUT.as_secs()
        + crate::service::sessions::PR_PROBE_TIMEOUT.as_secs()) as i64;

/// How much of a failed probe's message the host row keeps.
const PROBE_ERROR_CHARS: usize = 200;

/// The one write of a probe's outcome to its host row (`update_host_probe`
/// and the reconcile burst both use it). `last_pinged_at` moves on every
/// probe, failed ones too (down_hosts and the event diff rely on it);
/// `last_reachable_at` moves only when the host answered, and the last probe
/// error is set by a failed probe and cleared by an answered one (migration
/// 150, review r13). A failed probe with no error to name keeps whatever
/// error was recorded before.
pub(super) fn write_host_probe(
    conn: &rusqlite::Connection,
    alias: &str,
    reachable: bool,
    claude_version: Option<&str>,
    tmux_version: Option<&str>,
    last_pinged_at: i64,
    probe_error: Option<(&str, &str)>,
) -> Result<usize, rusqlite::Error> {
    let (code, message) = match probe_error {
        Some((code, message)) => (
            Some(code),
            Some(message.chars().take(PROBE_ERROR_CHARS).collect::<String>()),
        ),
        None => (None, None),
    };
    conn.execute(
        "UPDATE hosts SET reachable=?1, claude_version=?2, tmux_version=?3, last_pinged_at=?4, \
             last_reachable_at = CASE WHEN ?1 = 1 THEN ?4 ELSE last_reachable_at END, \
             last_probe_error_code = CASE WHEN ?1 = 1 THEN NULL \
                 ELSE COALESCE(?6, last_probe_error_code) END, \
             last_probe_error = CASE WHEN ?1 = 1 THEN NULL \
                 WHEN ?6 IS NOT NULL THEN ?7 ELSE last_probe_error END \
         WHERE alias=?5",
        rusqlite::params![
            if reachable { 1 } else { 0 },
            claude_version,
            tmux_version,
            last_pinged_at,
            alias,
            code,
            message,
        ],
    )
}

type KillMap = std::collections::HashMap<(String, String), i64>;

/// The sessions fleet itself killed recently: `(host_alias, tmux_name)` →
/// the unix second of the kill.
///
/// Deliberately in memory and not a table. The writer this guards against is
/// always a reconcile pass of THIS process that was already in flight when
/// the kill happened (`Store` is behind one `std::sync::Mutex`, so a pass and
/// a kill never interleave), and no in-flight pass survives a restart — a
/// tombstone that outlived the process would have nothing left to refuse.
///
/// Interior mutability (the same `std::sync::Mutex` idiom as [`StoreBus`]) so
/// the `&self` kill path can record without threading `&mut Store` through
/// it. Entries are pruned opportunistically on every read and write; there is
/// no timer, and the map is empty on all but the handful of seconds that
/// follow a kill.
#[derive(Default)]
pub(super) struct KillMemory(std::sync::Mutex<KillMap>);

impl KillMemory {
    /// Drop what has aged out, then hand the map to the caller. Pruning on
    /// the way in to every read AND every write is what keeps the map from
    /// growing without a timer of its own.
    fn pruned(&self) -> std::sync::MutexGuard<'_, KillMap> {
        let mut kills = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let cutoff = now_unix() - KILL_MEMORY_SECS;
        kills.retain(|_, killed_at| *killed_at > cutoff);
        kills
    }
}

// **There is deliberately no owner intent here, and there must not be one
// again** (multi-user M1, T5 and its review).
//
// The shape that kept being reached for: a create path says, before
// `tmux new-session` runs, whose the next `sessions` row under
// `(host_alias, tmux_name)` will be, and whichever reconcile pass inserts
// the row stamps that owner on it. It is tempting because no create path
// inserts a row of its own — `service::sessions::new_session` starts tmux and
// lets the reconcile upsert create the row, the SAME statement that creates a
// row for a session somebody started by hand (spec §3.1 / §3.2) — so at the
// moment of writing, a name is the only thing the create path and the pass
// share.
//
// **A name is not an identity.** Three successive attempts guarded that
// mechanism and each one moved the hole instead of closing it, because a tmux
// name is reused and re-usable by anyone with host access:
//
// * the upsert applied the intent on its `DO UPDATE` branch as well as on
//   INSERT, so an intent filed for a name that already had an `unclaimed`
//   row was stamped onto THAT row by the next pass — somebody else's session
//   acquired by asking to start one under its name, no race needed;
// * refusing a name that already has a row leaves the case with no row YET —
//   a live, hand-started tmux session that reconcile has not reached — still
//   claimable by naming it;
// * the map is one slot per `(host, name)`, so two concurrent creates of one
//   name cross-stamp, last writer wins.
//
// So reconcile names no owner at all. Every row this upsert inserts is
// `owner_person_id = NULL` / `unclaimed` (spec §4.3, the safe holding
// state), and the ONE mechanism that stamps an owner is
// [`Store::claim_if_unclaimed`], keyed on the row id and refusing a row that
// is already somebody else's — `service::sessions::finalize_new_session`
// calls it once the row exists. What that trades, and why the trade is
// right, is written on `finalize_new_session`.

/// Whether an observation whose probe started at `probe_started_at` may
/// INSERT a row for a name fleet killed at `killed_at` (`None` = not killed
/// recently, so nothing to refuse). Mirrors the `NOT_STALE` guard on the
/// `DO UPDATE` branch exactly, including its two conventions: same-second is
/// not newer, and `probe_started_at <= 0` ("probe time unknown", see
/// [`ghost_cutoff`]) never outranks a kill.
fn may_insert_after_kill(probe_started_at: i64, killed_at: Option<i64>) -> bool {
    match killed_at {
        None => true,
        Some(killed_at) => probe_started_at > 0 && probe_started_at > killed_at,
    }
}

/// Classify a [`RowChange`] as a session lifecycle transition, for the R5
/// forensics log (Task 7): a host reboot used to leave almost nothing in the
/// log besides MCP tool calls and tunnel warnings, so every session
/// created/lost/deleted transition now gets one INFO line. Returns `None`
/// for changes that are not a session lifecycle event (including a
/// `SessionUpdated` of a still-live row).
///
/// `SessionUpdated` maps to `"lost"` only when the row's `lost_at` is set.
/// Through the two loops that call this (`apply_host_reconcile`,
/// `mark_host_sessions_lost`), a session is logged `"lost"` exactly once
/// per loss episode: `ghost_and_clean` and `mark_host_sessions_lost` both
/// skip rows already `status = 'ghost'`, and `upsert_session_in_tx` either
/// clears `lost_at` back to `NULL` (the resurrect) or leaves the row — and
/// so the event — untouched (a stale observation, #170), so a
/// `SessionUpdated` from the reconcile upsert never carries
/// `lost_at.is_some()`. Other emitters of `SessionUpdated` (e.g.
/// `set_friendly_name`) go straight to the event bus and bypass these loops
/// entirely, so they never produce a lifecycle line. A second `"lost"` line
/// for the same session with no intervening `"created"`/revival is therefore
/// a bug, not a benign duplicate.
///
/// Two lifecycle lines are logged outside this mapping: a row fleet itself
/// killed is logged `"lost"` (reason `killed`) by `mark_session_killed`, and
/// is then skipped by both loops above since it is already ghost; and a
/// `missing` ghost that a later mass-loss verdict upgrades is logged
/// `"reclassified"` by `mark_host_sessions_lost` — a DISTINCT kind, so a
/// session ghosted by the routine prune and then reclassified legitimately
/// carries one `"lost"` line followed by one `"reclassified"` line, never
/// two `"lost"` lines.
pub(crate) fn lifecycle_kind(change: &RowChange) -> Option<&'static str> {
    match change {
        RowChange::SessionCreated(_) => Some("created"),
        RowChange::SessionUpdated(row) if row.lost_at.is_some() => Some("lost"),
        RowChange::SessionKilled(_) => Some("deleted"),
        _ => None,
    }
}

impl Store {
    // ---- Reconcile write-burst: one SAVEPOINT + emit-after-release ----
    //
    // The `*_in_tx` helpers below run ONLY their SQL against the
    // `&Connection` that [`Store::in_savepoint`] hands them and collect a
    // `RowChange` describing the event to emit — they do NOT touch
    // `self.bus`. `apply_host_reconcile_in_tx` drives them under one
    // SAVEPOINT, releases it, and only THEN hands the collected changes to
    // the bus. Nested in reconcile's per-host `Store::atomically` (the
    // service path) the bus holds them until that transaction commits;
    // standalone (`apply_host_reconcile`) the RELEASE is the commit. A
    // mid-batch error rolls the savepoint back, so no event fires for a
    // write that didn't persist.
    //
    // The public `update_host_probe` / `upsert_session` /
    // `touch_project_last_session_at` / `delete_sessions_not_in` methods are
    // intentionally left untouched — direct (non-reconcile) callers keep
    // emitting immediately.
    //
    // MAINTENANCE: each `*_in_tx` helper deliberately mirrors the SQL of its
    // public twin (same column lists, same upsert ON CONFLICT clause, same
    // SELECT-ids-before-DELETE). They differ in: (a) `tx` vs `self.conn`,
    // and (b) collecting a `RowChange` vs emitting via `self.bus` — plus one
    // deliberate SQL divergence: `upsert_session_in_tx` also writes
    // `last_reconciled_at` (the pass's freshness stamp, folded into the
    // upsert so a pass is one physical UPDATE per row), which the public
    // `upsert_session` never touches. If you change a schema/SQL detail in a
    // public method, change its `_in_tx` twin too.
    // Both paths are test-covered (direct: the `*_emits_*` event tests; tx: the
    // `apply_host_reconcile` rollback + happy-path tests), so a divergence will
    // surface as a test failure rather than silent corruption. The ghost /
    // reap pass is the exception: `ghost_and_clean` is ONE function shared by
    // this write-burst (tmux rows, stale-probe guard on) and by the public
    // `ghost_and_clean_bg_sessions` (pane-less rows, its own SAVEPOINT —
    // nested in reconcile's per-host transaction on the service path), so
    // the two prunes cannot drift apart.
    //
    // `worktree_key` is written by `upsert_session_in_tx` ONLY — the public
    // `upsert_session` intentionally omits it (reconcile is the only path that
    // knows the session's cwd and can compute the key).

    /// Remember that fleet killed `tmux_name` on `host_alias` at `at` (unix
    /// seconds, the same clock as `lost_at` and `probe.started_at`). Called
    /// by [`Store::mark_session_killed`]; see [`KillMemory`].
    pub(super) fn note_kill(&self, host_alias: &str, tmux_name: &str, at: i64) {
        self.kills
            .pruned()
            .insert((host_alias.to_string(), tmux_name.to_string()), at);
    }

    /// Forget the kill of `tmux_name` on `host_alias`. Called by every path
    /// that CREATES (or renames into) that tmux session and then relies on
    /// its own reconcile to insert the row: fleet having just brought the
    /// name back to life is definitive evidence that it is no longer killed,
    /// and outranks any comparison of stamps. Without this, a kill and a
    /// re-create inside one second would leave the create's reconcile
    /// refusing the INSERT and the caller with no row to return.
    pub fn forget_kill(&self, host_alias: &str, tmux_name: &str) {
        self.kills
            .pruned()
            .remove(&(host_alias.to_string(), tmux_name.to_string()));
    }

    /// The still-remembered kills on `host_alias`, as `tmux_name → killed_at`.
    fn recent_kills(&self, host_alias: &str) -> std::collections::HashMap<String, i64> {
        self.kills
            .pruned()
            .iter()
            .filter(|((host, _), _)| host == host_alias)
            .map(|((_, name), killed_at)| (name.clone(), *killed_at))
            .collect()
    }

    /// Two-phase reap for a host nothing will ever probe (`local` on a hub
    /// without a local host — never a user-hidden host, whose rows Unhide
    /// must bring back): every live row is ghosted this call and
    /// every already-ghost row is deleted, both kinds, no TTL exemption —
    /// nothing on such a host can be resumed from here. A ghosted row also
    /// loses `claude_status` / `stuck_kind` / `current_activity`: a dead
    /// row saying `working` kept being counted (data-sync F2). Returns the
    /// number of rows hard-deleted.
    pub fn reap_host_ghosts(&self, host_alias: &str, now: i64) -> Result<usize, rusqlite::Error> {
        let (changes, deleted) = self.in_savepoint("reap_host_ghosts", |tx| {
            let mut out: Vec<RowChange> = Vec::new();
            let before: i64 = tx.query_row(
                "SELECT COUNT(*) FROM sessions WHERE host_alias = ?1",
                [host_alias],
                |r| r.get(0),
            )?;
            tx.execute(
                "UPDATE sessions SET claude_status = NULL, stuck_kind = NULL, current_activity = NULL \
                 WHERE host_alias = ?1 AND status != 'ghost'",
                [host_alias],
            )?;
            for kind in [KIND_TMUX, KIND_PANE_LESS] {
                Self::ghost_and_clean(tx, host_alias, &[], now, kind, None, None, None, &mut out)?;
            }
            let after: i64 = tx.query_row(
                "SELECT COUNT(*) FROM sessions WHERE host_alias = ?1",
                [host_alias],
                |r| r.get(0),
            )?;
            Ok::<_, rusqlite::Error>((out, (before - after).max(0) as usize))
        })?;
        for c in &changes {
            self.bus.emit_change(c);
        }
        Ok(deleted)
    }

    #[allow(clippy::too_many_arguments)]
    fn update_host_probe_in_tx(
        tx: &rusqlite::Connection,
        alias: &str,
        reachable: bool,
        claude_version: Option<&str>,
        tmux_version: Option<&str>,
        last_pinged_at: i64,
        probe_error: Option<(&str, &str)>,
        out: &mut Vec<RowChange>,
    ) -> Result<(), rusqlite::Error> {
        // Read before the write, so the emit below can tell a probe that found
        // something new from one that found the host exactly as it was.
        let prior = fetch_host(tx, alias)?;
        write_host_probe(
            tx,
            alias,
            reachable,
            claude_version,
            tmux_version,
            last_pinged_at,
            probe_error,
        )?;
        if let Some(row) = fetch_host(tx, alias)? {
            // Reconcile probes every host every pass, and `last_pinged_at`
            // moves on each one, so the full row can never be diffed away —
            // which is why this emitted ~265 B per host per pass to every
            // connected client to say nothing had changed. When the stamp is
            // the only thing that moved, say just that.
            // The health sample (task 2) moves every pass too, so it rides
            // the ping rather than forcing a full row.
            let only_the_stamp_moved = prior.is_some_and(|before| {
                HostRow {
                    last_pinged_at: row.last_pinged_at,
                    // Moves with the ping while the host answers; the ping
                    // handlers derive it from `reachable` (hosts.ts).
                    last_reachable_at: row.last_reachable_at,
                    claude_version_at: row.claude_version_at,
                    disk_home_free_kb: row.disk_home_free_kb,
                    disk_home_total_kb: row.disk_home_total_kb,
                    disk_tmp_free_kb: row.disk_tmp_free_kb,
                    load_1m: row.load_1m,
                    mem_avail_kb: row.mem_avail_kb,
                    uptime_secs: row.uptime_secs,
                    health_at: row.health_at,
                    cpu_count: row.cpu_count,
                    mem_total_kb: row.mem_total_kb,
                    boot_at: row.boot_at,
                    latency_ms: row.latency_ms,
                    worktree_kb: row.worktree_kb,
                    ..before
                } == row
            });
            out.push(if only_the_stamp_moved {
                RowChange::HostPinged {
                    health: Some(crate::store::HostHealth::of(&row)),
                    alias: row.alias,
                    last_pinged_at: row.last_pinged_at.unwrap_or(last_pinged_at),
                    reachable: row.reachable,
                    claude_version_at: row.claude_version_at,
                }
            } else {
                RowChange::HostProbed(row)
            });
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn upsert_session_in_tx(
        tx: &rusqlite::Connection,
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
        pending_input: Option<&str>,
        killed_at: Option<i64>,
        reconciled_at: Option<i64>,
        pane_working: bool,
        pr_evidence: Option<&str>,
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

        // The INSERT half of the staleness guard (#171). With no row to
        // conflict with there is no `lost_at` left to compare against: the
        // kill ghosted the row and the kill's OWN reconcile hard-deleted it
        // in the same pass (it was already ghost), while a fleet-wide pass
        // that listed tmux before the kill only writes once every host's
        // probe has joined. Unguarded, that write finds nothing, inserts the
        // dead session afresh as `running`, and announces a `SessionCreated`
        // for it — the killed session reappears in the UI until a later pass
        // ghosts and reaps the phantom. `killed_at` is the remembered kill
        // (see [`KillMemory`]); the row-existence read above and this
        // decision are both inside the caller's transaction AND inside the
        // `Mutex<Store>` the kill path also needs, so there is no window
        // between checking and inserting.
        if prior.is_none() && !may_insert_after_kill(probe_started_at, killed_at) {
            return Ok(());
        }

        // The post-write stuck_kind, spelled out once and reused: SQLite's
        // upsert SET clauses see the OLD row (unqualified) and the candidate
        // (`excluded`), never each other's results.
        const NEW_STUCK: &str = "CASE WHEN kind = 'shell' THEN NULL \
                                 WHEN ?16 THEN excluded.stuck_kind \
                                 ELSE COALESCE(excluded.stuck_kind, stuck_kind) END";
        // The post-write pending_input: the same intel_observed gate as
        // stuck_kind — authoritative (and a NULL clears a stale dialog) when
        // the pane was captured this pass, preserved when it was not.
        const NEW_PENDING: &str = "CASE WHEN kind = 'shell' THEN NULL \
                                   WHEN ?16 THEN excluded.pending_input \
                                   ELSE COALESCE(excluded.pending_input, pending_input) END";
        // The post-write claude_status. A Stop hook that landed at or after
        // this pass's probe STARTED (`last_stop_at >= ?20`) is fresher than
        // the pane the pass captured, so its `idle` must win over the pane
        // heuristic (MCP-1: reconcile used to clobber the hook every tick).
        // `?20 <= 0` disables the guard (store-level tests pass 0).
        // `last_hook_at` is stamped by BOTH hooks (Stop → idle,
        // UserPromptSubmit → working). The guard only covers passes that were
        // already in flight when the hook landed; a pass that starts later
        // observes the pane afresh and wins, as it should.
        // A `shell` row (kind set by `new_shell_session`) has no Claude in
        // it: its bare `❯` is the shell's prompt, never an idle REPL (F8).
        // A StopFailure's `failed` is recognisable without a column: the
        // failure hook stamps `last_hook_at` and `last_stop_at` together, and
        // any later hook moves `last_hook_at` past `last_stop_at`. While that
        // holds, the pane's idle prompt (the input box the failure left) does
        // not overwrite it; a turn (`working`) or a dialog (`blocked`) does.
        const NEW_STATUS: &str = "CASE WHEN kind = 'shell' THEN NULL \
                                       WHEN ?20 > 0 AND last_hook_at IS NOT NULL \
                                            AND last_hook_at >= ?20 \
                                       THEN claude_status \
                                       WHEN claude_status = 'failed' AND last_hook_at IS NOT NULL \
                                            AND last_hook_at = last_stop_at \
                                            AND COALESCE(excluded.claude_status, '') NOT IN ('working','blocked') \
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
        // The post-write claude_session_id. The MCP-1 in-flight guard
        // applies to the id as to the status: a hook that landed at or after
        // this probe STARTED (a SessionStart / UserPromptSubmit rebind, a
        // SessionEnd(clear|resume)) owns the conversation binding (spec
        // §1.3/§1.4), so a pass that read `claude agents` before it must not
        // write the replaced id back.
        const NEW_ID: &str = "CASE WHEN ?20 > 0 AND last_hook_at IS NOT NULL \
                                        AND last_hook_at >= ?20 \
                                   THEN claude_session_id \
                                   ELSE COALESCE(excluded.claude_session_id, claude_session_id) END";
        // The pass moves the row off a conversation it was bound to. SET
        // clauses see the OLD row, so this compares against the prior id. A
        // first sighting (prior id NULL) is not a change: nothing stored
        // belonged to another conversation.
        let id_changes =
            format!("claude_session_id IS NOT NULL AND ({NEW_ID}) IS NOT claude_session_id");
        // Whether this observation may be written onto an EXISTING row at
        // all (#170). A row that is lost (`lost_at` set — ghosted by the
        // keep-set prune, by a mass-loss verdict, or by `mark_session_killed`)
        // is only revived by an observation NEWER than the loss: a pass whose
        // probe listed tmux before `kill_session` ghosted the row still
        // carries its name, and its write can land after the kill — reviving
        // a dead session, dropping its `lost_reason`, and restarting the
        // one-cycle reap clock (so `session:killed` came a cycle late or
        // never). `lost_at` and `?20` are both unix SECONDS off the same
        // clock (`now_unix`), so the comparison is strict: a probe that
        // started within the losing second cannot be shown to have seen the
        // session after it died. `?20 <= 0` is "probe time unknown" (see
        // [`ghost_cutoff`]) — undatable evidence never revives a lost row;
        // no production caller passes it (reconcile always passes
        // `probe.started_at`), only store-level tests do.
        //
        // Why this cannot strand a live session as lost: `lost_at` is FROZEN
        // for as long as a row stays ghost. Every writer of it
        // (`mark_session_killed`, `Store::mark_host_sessions_lost`,
        // `ghost_and_clean` Phase 1) is gated on `status != 'ghost'`, and the
        // reclassify path rewrites only `lost_reason`, deliberately leaving
        // `lost_at` alone — so the threshold never ratchets forward while the
        // row waits. `probe.started_at` only grows (same `now_unix()` clock,
        // same process), so the very next pass that observes the name live
        // clears the bar and revives the row; a backwards clock step costs a
        // bounded delay, never a permanent ghost.
        //
        // The guard covers the WHOLE `DO UPDATE`, not just the three
        // resurrect columns: a stale sighting of a dead session must not
        // repaint its `claude_status`, activity stamp or context either. A
        // live row (`lost_at IS NULL`) takes the branch exactly as before.
        // The INSERT path has the same guard against the remembered kill
        // (`killed_at` above), for the case where the row is already gone.
        const NOT_STALE: &str = "lost_at IS NULL OR (?20 > 0 AND ?20 > lost_at)";
        // Ownership (multi-user M1, T5, and its review). Reconcile NAMES NO
        // OWNER: every row it inserts is `owner_person_id = NULL` /
        // `unclaimed` (spec §4.3), and the `DO UPDATE` branch below touches
        // neither column, so no reconcile pass can stamp, re-home, un-own or
        // widen a session. The one mechanism that writes an owner is
        // `Store::claim_if_unclaimed`, keyed on a row id; the name-keyed
        // intent this statement used to read is gone for good, and the long
        // note above it says why it must not come back.
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
                                   tmux_pane_id, context_source, context_at, pending_input,
                                   last_reconciled_at, pane_working_at, pr_evidence, pr_checked_at,
                                   owner_person_id, visibility)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'running', ?7, ?8, NULL, {guarded_id}, ?10, ?11, ?12, ?13,
                     ?14, ?15, ?17,
                     CASE WHEN ?10 IN ('idle','completed','stopped') THEN ?19 ELSE NULL END,
                     CASE WHEN ?15 IS NULL THEN NULL ELSE ?19 END,
                     ?21,
                     CASE WHEN ?14 IS NULL THEN NULL ELSE 'pane' END,
                     CASE WHEN ?14 IS NULL THEN NULL ELSE ?19 END,
                     ?22,
                     ?23,
                     CASE WHEN ?25 THEN ?19 ELSE NULL END,
                     CASE WHEN ?18 THEN ?26 ELSE NULL END,
                     CASE WHEN ?18 AND ?12 IS NOT NULL THEN ?19 ELSE NULL END,
                     NULL,
                     'unclaimed')
             ON CONFLICT(host_alias, tmux_name) DO UPDATE SET
               -- A pane whose directory is in no project keeps the project
               -- a fleet-run row already has: a person adopted it into one
               -- (Lost and found, step 4.12), or it cd'd out of its checkout.
               project_id=CASE WHEN excluded.project_id IS NULL AND started_at IS NOT NULL
                               THEN project_id ELSE excluded.project_id END,
               last_activity_at=excluded.last_activity_at,
               account_uuid=COALESCE(excluded.account_uuid, account_uuid),
               worktree_key=COALESCE(excluded.worktree_key, worktree_key),
               -- The resurrect. Reached only for a row this observation is
               -- allowed to touch at all (the `WHERE` below, #170).
               status=CASE WHEN status='ghost' THEN 'running' ELSE status END,
               lost_at=NULL,
               lost_reason=NULL,
               claude_session_id={new_id},
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
               -- Result evidence (082) follows the same rule as ci_status:
               -- authoritative when the probe ran (?18), else kept. Every SET
               -- expression reads the OLD row, so `pr_evidence` and `pr_url`
               -- below are the stored values. `pr_checked_at` is stamped at
               -- once for a new or changed reading and otherwise only when
               -- the stored stamp is ?27 old: a PR that sits green must not
               -- make every probe emit. No PR, no stamp.
               pr_evidence=CASE WHEN ?18 THEN ?26 ELSE pr_evidence END,
               pr_checked_at=CASE WHEN NOT ?18 THEN pr_checked_at
                                  WHEN ?12 IS NULL THEN NULL
                                  WHEN pr_checked_at IS NULL
                                       OR ?26 IS NOT pr_evidence
                                       OR ?12 IS NOT pr_url
                                       OR ?19 - pr_checked_at >= ?27 THEN ?19
                                  ELSE pr_checked_at END,
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
               -- An unchanged footer value keeps its stamp: re-stamping it
               -- every pass would make each no-op pass emit (BE-11).
               context_at=CASE WHEN excluded.context_pct IS NULL THEN context_at
                               WHEN {fresh} AND NOT ({id_changes}) THEN context_at
                               WHEN context_source IS 'pane'
                                    AND context_pct IS excluded.context_pct
                                    AND NOT ({id_changes}) THEN context_at
                               ELSE ?19 END,
               -- stuck_kind is authoritative when the pane was observed this
               -- pass (?16): a NULL then CLEARS a stale flag. When the pane was
               -- NOT observed (capture failed) we preserve the prior value.
               stuck_kind={new_stuck},
               -- stuck_since: keep the episode start while the kind is
               -- unchanged, restart it when the kind changes, clear when the
               -- flag clears. An `oom` flag re-appearing within the recreate
               -- spacing (?24) of the playbook's last action is `--resume`
               -- re-rendering the text that action was for: the episode
               -- continues, anchored on that action (F1).
               stuck_since=CASE WHEN ({new_stuck}) IS NULL THEN NULL
                                WHEN ({new_stuck}) IS stuck_kind THEN COALESCE(stuck_since, ?19)
                                WHEN ({new_stuck}) = 'oom' AND last_playbook_at IS NOT NULL
                                     AND ?19 - last_playbook_at < ?24 THEN last_playbook_at
                                ELSE ?19 END,
               idle_since={idle},
               pending_input={new_pending},
               -- A pane that shows a live turn lifts the stale-working
               -- demotion (F2): the stamp and the veto's memory (080) both
               -- go. Anything else keeps them.
               stale_working_at=CASE WHEN ({new_status}) IS 'working' THEN NULL
                                     ELSE stale_working_at END,
               stale_demoted_at=CASE WHEN ({new_status}) IS 'working' THEN NULL
                                     ELSE stale_demoted_at END,
               -- The freshness stamp (Task H / the BE-3 guard's evidence),
               -- folded in here so a pass is ONE physical UPDATE per row
               -- instead of this upsert plus a second stamping UPDATE.
               -- `last_reconciled_at` is not a `SessionRow` field, so it
               -- never makes a no-op pass emit, and migration 063's trigger
               -- does not watch it, so it never bumps `row_version` either.
               last_reconciled_at=COALESCE(?23, last_reconciled_at),
               -- The pane showed a live turn this pass (?25): the stale-working
               -- sweep's evidence of life (migration 081). Bookkeeping like
               -- `last_reconciled_at`: not a `SessionRow` field, not watched
               -- by the row_version trigger, so stamping it never emits.
               pane_working_at=CASE WHEN ?25 THEN ?19 ELSE pane_working_at END
               -- `owner_person_id` and `visibility` are deliberately ABSENT
               -- from this SET list (T5's review): a pass must never be able
               -- to touch either. See the ownership note above.
             WHERE {not_stale}",
            new_stuck = NEW_STUCK,
            new_pending = NEW_PENDING,
            new_status = NEW_STATUS,
            idle = idle_since_sql(NEW_STATUS, "?19"),
            guarded_id = GUARDED_ID,
            id_changes = id_changes,
            new_id = NEW_ID,
            fresh = FRESH_CONTEXT,
            not_stale = NOT_STALE,
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
                tmux_pane_id,
                pending_input,
                reconciled_at,
                crate::service::playbooks::OOM_RECREATE_MIN_SPACING_SECS,
                pane_working,
                pr_evidence,
                crate::service::outcome::PR_CHECKED_REFRESH_SECS
            ],
        )?;
        if let Some(row) = fetch_session(tx, tmux_name, host_alias)? {
            match prior {
                None => out.push(RowChange::SessionCreated(row)),
                // Every wire field identical (modulo `row_version` — see
                // `eq_ignoring_row_version`; since migration 063 a no-op pass
                // leaves it alone too) ⇒ a no-op pass; emit nothing.
                Some(ref before) if before.eq_ignoring_row_version(&row) => {}
                Some(before) => {
                    // Its PR's checks moved: the missions it works for
                    // look now (orchestration O8), not at their timer.
                    if before.ci_status != row.ci_status {
                        super::mission_loop::wake_session_missions_in_tx(tx, row.id)?;
                    }
                    out.push(RowChange::SessionUpdated(row))
                }
            }
        }
        Ok(())
    }

    /// Upsert the PR one session's probe saw (redesign 6.4). The session's
    /// row was written just before, so its id and name are read back here; a
    /// row the stale-sighting guard refused to revive records nothing.
    fn record_session_pr_in_tx(
        tx: &rusqlite::Connection,
        host_alias: &str,
        sess: &ReconcileSession<'_>,
        url: &str,
    ) -> Result<(), rusqlite::Error> {
        let Some(row) = fetch_session(tx, sess.tmux_name, host_alias)? else {
            return Ok(());
        };
        if row.lost_at.is_some() {
            return Ok(());
        }
        let name = row.friendly_name.as_deref().unwrap_or(&row.tmux_name);
        Self::upsert_pull_request_in_tx(
            tx,
            url,
            sess.ci_status.as_deref(),
            sess.pr_evidence.as_ref(),
            super::PrSeenBy {
                session_id: row.id,
                session_name: name,
                host_alias,
                project_id: row.project_id,
            },
            now_unix(),
        )?;
        Ok(())
    }

    fn touch_project_last_session_at_in_tx(
        tx: &rusqlite::Connection,
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
    /// Loss also clears `claude_status`, `stuck_kind`/`stuck_since`,
    /// `current_activity` and `pending_input`: a ghost has no pane to vouch
    /// for them (F4).
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
    ///
    /// `lost_ttl_cutoff` (unix secs) guards Phase 2 against reaping a
    /// resumable mass-loss row too early: a row with `claude_session_id IS
    /// NOT NULL`, `lost_reason IN ('host_reboot','tmux_server_gone')`, and
    /// `lost_at >= lost_ttl_cutoff` is exempt from the hard-delete — the
    /// session can still be resumed, so it survives past the usual one-cycle
    /// grace until it ages out of the TTL. A `missing` row (a single session
    /// that dropped out while its neighbours stayed live) is never exempt,
    /// so it keeps today's one-cycle reap regardless of this cutoff. Nor is
    /// an `external` row: fleet did not start that session and restore never
    /// resumes one, so keeping it to the TTL only piles dead rows into the
    /// "Outside fleet" group. `None`
    /// disables the exemption entirely — today's behaviour, byte-identical
    /// SQL and bindings.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ghost_and_clean(
        tx: &rusqlite::Connection,
        host_alias: &str,
        keep_names: &[String],
        now: i64,
        kind_filter: &str,
        cutoff: Option<i64>,
        lost_ttl_cutoff: Option<i64>,
        external_grace_cutoff: Option<i64>,
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
            // Placeholders are numbered as `head` grows, so `exempt` never
            // has to know whether its neighbour is present; the keep names'
            // bare `?`s continue from the highest number used.
            let mut head: Vec<&dyn rusqlite::ToSql> = vec![&host_alias];
            let mut exempt = String::new();
            // `COALESCE(..., 0)`: `lost_reason` is NULL on every row ghosted
            // before migration 036 introduced the column. SQL's three-valued
            // logic would otherwise make `lost_reason IN (...)` NULL, the
            // inner AND chain NULL, and `NOT NULL` NULL again — which `WHERE`
            // treats as "leave this row out of the reaped set", wrongly
            // exempting it.
            if let Some(c) = &lost_ttl_cutoff {
                head.push(c);
                exempt.push_str(&format!(
                    " AND NOT COALESCE((claude_session_id IS NOT NULL \
                                        AND kind != 'external' \
                                        AND lost_reason IN ('host_reboot','tmux_server_gone') \
                                        AND lost_at >= ?{}), 0)",
                    head.len()
                ));
            }
            // An `external` row is never resumable, so the TTL above never
            // covers it; `gc.external_lost_ttl_secs` keeps it just long
            // enough for the desktop that owns it to restart (F6).
            if let Some(g) = &external_grace_cutoff {
                head.push(g);
                exempt.push_str(&format!(
                    " AND NOT COALESCE((kind = 'external' AND lost_at >= ?{}), 0)",
                    head.len()
                ));
            }
            let sql = format!(
                "SELECT id FROM sessions
                 WHERE host_alias=?1 AND status='ghost' AND {kind_filter}{exempt}{not_in}"
            );
            let params = params_then(&head, keep_names);
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
                "UPDATE sessions SET status='ghost', lost_at=?1, lost_reason='missing', {LOSS_CLEARS}
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
            // Their facts, before the `DELETE` below: `session:killed` is
            // fenced by them (`store::sessions::killed_payloads`).
            let killed = super::sessions::killed_payloads(tx, &pre_ghost_ids)?;
            let phs = in_clause(pre_ghost_ids.len());
            // No FK cascade on session_events — delete them with the row or
            // they linger as orphans forever.
            tx.execute(
                &format!("DELETE FROM session_events WHERE session_id IN ({phs})"),
                rusqlite::params_from_iter(&pre_ghost_ids),
            )?;
            // Tombstone the participant rather than deleting the messages
            // addressed to them, as `delete_session` does — a hard-deleted
            // ghost row must not silently destroy an undelivered inbox.
            tx.execute(
                &format!(
                    "UPDATE participants SET retired_at = ?1, session_id = NULL, client_id = NULL \
                     WHERE session_id IN ({phs}) AND retired_at IS NULL"
                ),
                params_then(rusqlite::params![now], &pre_ghost_ids).as_slice(),
            )?;
            tx.execute(
                &format!("DELETE FROM sessions WHERE id IN ({phs})"),
                rusqlite::params_from_iter(&pre_ghost_ids),
            )?;
            for k in killed {
                out.push(RowChange::SessionKilled(k));
            }
        }

        Ok(())
    }

    /// One probed/live session to apply during a reconcile write-burst, with
    /// its `(project_id, account_uuid)` ALREADY resolved by the caller (those
    /// are reads — `find_project_id_for_path` / `get_session_account` — and
    /// must happen before the transaction opens).
    ///
    /// Standalone: the burst is its own transaction, committed before any
    /// event is emitted. Inside an open transaction use
    /// [`Self::apply_host_reconcile_in_tx`].
    pub fn apply_host_reconcile(&mut self, spec: HostReconcile<'_>) -> Result<(), rusqlite::Error> {
        self.apply_host_reconcile_in_tx(spec)
    }

    /// [`Self::apply_host_reconcile`] for a caller already inside
    /// `Store::atomically`: the reconcile service runs it in the per-host
    /// transaction that also carries the host's identity, PR signals,
    /// timeline events and bg rows (Task 4). Runs under a SAVEPOINT
    /// ([`Store::in_savepoint`]), which is why it works both ways: nested,
    /// the outer `atomically` holds the events until ITS commit and drops
    /// them on rollback; standalone (the `&mut self` wrapper) the savepoint
    /// is the transaction and the events follow the RELEASE that commits.
    pub fn apply_host_reconcile_in_tx(
        &self,
        spec: HostReconcile<'_>,
    ) -> Result<(), rusqlite::Error> {
        self.apply_host_reconcile_with_error(spec, None)
    }

    /// [`Self::apply_host_reconcile`] for a probe that failed: `error`
    /// (code, message) is kept on the host row as its last probe error
    /// (migration 150) so the offline state can say why.
    pub fn apply_host_reconcile_failed(
        &mut self,
        spec: HostReconcile<'_>,
        error: (&str, &str),
    ) -> Result<(), rusqlite::Error> {
        self.apply_host_reconcile_with_error(spec, Some(error))
    }

    fn apply_host_reconcile_with_error(
        &self,
        spec: HostReconcile<'_>,
        probe_error: Option<(&str, &str)>,
    ) -> Result<(), rusqlite::Error> {
        // What this host's names were killed at (#171). Not a
        // check-then-insert window: the caller holds the one `Mutex<Store>`
        // for this whole call, and `mark_session_killed` — the only writer of
        // this map — needs that same lock. No kill can slip in between the
        // read and the inserts.
        let kills = self.recent_kills(spec.alias);
        // Phase 1: run all SQL inside one savepoint, collecting RowChanges.
        let changes = self.in_savepoint("apply_host_reconcile", |tx| {
            let mut out: Vec<RowChange> = Vec::new();
            Self::update_host_probe_in_tx(
                tx,
                spec.alias,
                spec.reachable,
                spec.claude_version,
                spec.tmux_version,
                spec.last_pinged_at,
                probe_error,
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
                    let pending_input_json = encode_pending_input(sess.pending_input.as_ref());
                    let pr_evidence_json = sess
                        .pr_evidence
                        .as_ref()
                        .and_then(|e| serde_json::to_string(e).ok());
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
                        pending_input_json.as_deref(),
                        kills.get(sess.tmux_name).copied(),
                        spec.reconciled_at,
                        sess.pane_working,
                        pr_evidence_json.as_deref(),
                        &mut out,
                    )?;
                    // Redesign 6.4: the PR this pass saw on the session's
                    // branch, kept in `pull_requests` past the session.
                    if let (true, Some(url)) = (sess.pr_observed, sess.pr_url.as_deref()) {
                        Self::record_session_pr_in_tx(tx, spec.alias, sess, url)?;
                    }
                    if let Some(pid) = sess.project_id {
                        let latest = project_touch.entry(pid).or_insert(0);
                        *latest = (*latest).max(sess.last_activity_at);
                    }
                }
                for (pid, ts) in project_touch {
                    Self::touch_project_last_session_at_in_tx(tx, pid, ts, &mut out)?;
                }
                // Task 6: a pass that just mass-marked this host's sessions
                // lost (reboot / vanished tmux server) skips the routine
                // ghost/reap pass entirely — it must not immediately re-ghost
                // (and restart the reap clock on) rows the mass-loss path
                // just stamped with their own `lost_reason`.
                if !spec.skip_prune {
                    Self::ghost_and_clean(
                        tx,
                        spec.alias,
                        spec.keep,
                        now_unix(),
                        KIND_TMUX,
                        Some(ghost_cutoff(spec.probe_started_at)),
                        spec.lost_ttl_cutoff,
                        // A tmux row is never `external`.
                        None,
                        &mut out,
                    )?;
                }
            }
            Ok::<_, rusqlite::Error>(out)
        })?;

        // Phase 2: savepoint released — now it is safe to emit (directly when
        // it was the transaction, held by `atomically` when nested).
        for change in &changes {
            // Task 7 (R5): one INFO line per session lifecycle transition —
            // a host reboot used to leave nothing in the log to forensically
            // reconstruct what happened to the 8+ sessions it took out.
            if let Some(kind) = lifecycle_kind(change) {
                match change {
                    RowChange::SessionCreated(row) | RowChange::SessionUpdated(row) => {
                        tracing::info!(
                            lifecycle = kind,
                            session_id = row.id,
                            host_alias = %row.host_alias,
                            tmux_name = %row.tmux_name,
                            claude_session_id = row.claude_session_id.as_deref().unwrap_or("-"),
                            "[session] {kind}"
                        );
                    }
                    RowChange::SessionKilled(k) => {
                        tracing::info!(
                            lifecycle = kind,
                            host_alias = %spec.alias,
                            session_id = k.id,
                            "[session] {kind}"
                        );
                    }
                    _ => {}
                }
            }
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
    ///
    /// Reconcile itself no longer calls this: the stamp rides the upsert
    /// (`HostReconcile::reconciled_at`), so a pass costs each row one
    /// physical UPDATE, not two. Kept for tests
    /// that need to place a row's stamp at a chosen instant.
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

    /// data-sync F2/F5, hub-ops F6: `local` on a hub without a local host (the
    /// one host nothing will ever probe) kept its rows forever, some still
    /// `working`. A user-hidden host is never reaped (see
    /// `reconcile_keeps_a_hidden_hosts_rows_and_unhide_restores_them`).
    #[test]
    fn reap_host_ghosts_ghosts_live_rows_then_deletes_ghosts_without_a_probe() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let live = s
            .upsert_session("dev-a", "local", None, None, 1, 1, "running", None)
            .unwrap();
        let bg = s
            .upsert_session("bg:abc", "local", None, None, 1, 1, "running", None)
            .unwrap();
        s.conn_for_test()
            .execute(
                "UPDATE sessions SET kind='external', claude_status='working', stuck_kind='oom' WHERE id=?1",
                [bg],
            )
            .unwrap();
        // Pass 1: everything live on the host is ghosted, with its status cleared.
        assert_eq!(s.reap_host_ghosts("local", 100).unwrap(), 0);
        for id in [live, bg] {
            let row = s.get_session_by_id(id).unwrap().unwrap();
            assert_eq!(row.status, "ghost");
            assert_eq!(row.claude_status, None, "a ghost has no live status");
            assert_eq!(row.stuck_kind, None);
        }
        // Pass 2: already-ghost rows are hard-deleted, TTL or not.
        assert_eq!(s.reap_host_ghosts("local", 200).unwrap(), 2);
        assert!(s.get_session_by_id(live).unwrap().is_none());
        assert!(s.get_session_by_id(bg).unwrap().is_none());
    }

    /// A minimal, DB-free `SessionRow` for `lifecycle_kind` classification
    /// tests — only `lost_at` varies between cases, every other field is a
    /// harmless default.
    fn bare_row(lost_at: Option<i64>) -> SessionRow {
        SessionRow {
            id: 1,
            row_version: 0,
            prompt_submit_seq: 0,
            tmux_name: "work-a".into(),
            host_alias: "alpha".into(),
            project_id: None,
            worktree_id: None,
            created_at: 0,
            last_activity_at: 0,
            status: "running".into(),
            notes: None,
            account_uuid: None,
            kind: "work".into(),
            reviews_session_id: None,
            worktree_key: None,
            lost_at,
            lost_reason: None,
            claude_session_id: None,
            claude_status: None,
            effort_level: None,
            pr_url: None,
            current_activity: None,
            context_pct: None,
            stuck_kind: None,
            friendly_name: None,
            safe_kill_state: None,
            safe_kill_nonce: None,
            safe_kill_detail: None,
            safe_kill_requested_at: None,
            idle_since: None,
            stuck_since: None,
            last_playbook_at: None,
            last_prompt: None,
            started_at: None,
            last_turn_at: None,
            ci_status: None,
            turn_seq: 0,
            last_stop_at: None,
            stale_working_at: None,
            stale_demoted_at: None,
            work_rev: 0,
            pr_evidence: None,
            pr_checked_at: None,
            owner_person_id: None,
            visibility: crate::store::VISIBILITY_UNCLAIMED.into(),
            claude_profile: None,
            agent: crate::store::AGENT_CLAUDE.into(),
            origin: None,
            origin_ref: None,
            last_viewed_at: None,
            turn_outcome: None,
            proposals: Vec::new(),
            pending_form: None,
            form_draft: None,
            parent_session_id: None,
            tags: Vec::new(),
            usage: Default::default(),
            context: Default::default(),
            pending_input: None,
            work: None,
            work_rejected: vec![],
            work_suggested: None,
            org_id: None,
        }
    }

    fn bare_host() -> HostRow {
        HostRow {
            alias: "alpha".into(),
            ssh_alias: None,
            reachable: true,
            claude_version: None,
            tmux_version: None,
            hidden: false,
            last_pinged_at: None,
            account_uuid: None,
            provisioned: false,
            transport: "ssh".to_string(),
            org_id: None,
            claude_version_at: None,
            disk_home_free_kb: None,
            disk_home_total_kb: None,
            disk_tmp_free_kb: None,
            load_1m: None,
            mem_avail_kb: None,
            uptime_secs: None,
            health_at: None,
            last_hook_at: None,
            agent_version: None,
            provisioned_at: None,
            provision_stale: false,
            unclaimed_sessions: None,
            provision_warning: None,
            auth_overrides: None,
            claude_profiles: None,
            cpu_count: None,
            mem_total_kb: None,
            boot_at: None,
            latency_ms: None,
            worktree_kb: None,
            worktree_at: None,
            agents_on_path: None,
            last_reachable_at: None,
            last_probe_error_code: None,
            last_probe_error: None,
            harnesses: None,
        }
    }

    #[test]
    fn lifecycle_kind_classifies_created_lost_deleted() {
        assert_eq!(
            lifecycle_kind(&RowChange::SessionCreated(bare_row(None))),
            Some("created")
        );
        assert_eq!(
            lifecycle_kind(&RowChange::SessionUpdated(bare_row(Some(500)))),
            Some("lost")
        );
        assert_eq!(
            lifecycle_kind(&RowChange::SessionKilled(1.into())),
            Some("deleted")
        );
    }

    #[test]
    fn lifecycle_kind_ignores_live_updates_and_non_session_changes() {
        assert_eq!(
            lifecycle_kind(&RowChange::SessionUpdated(bare_row(None))),
            None,
            "an update to a still-live row is not a lifecycle transition"
        );
        assert_eq!(
            lifecycle_kind(&RowChange::HostProbed(bare_host())),
            None,
            "non-session changes never carry a lifecycle kind"
        );
    }

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

    /// Reconcile probes every host every pass and `last_pinged_at` moves each
    /// time, so a full `host:probed` could never be diffed away: on a
    /// five-host fleet that was ~108 KB/h to every connected client, to say
    /// nothing had changed. A probe that finds the host as it was now says so
    /// in three fields.
    #[test]
    fn a_probe_that_changes_nothing_but_the_stamp_sends_a_heartbeat_not_the_row() {
        let (mut store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        let first = HostReconcile {
            claude_version: Some("2.1.0"),
            ..empty_probe("alpha", 10)
        };
        store.apply_host_reconcile(first).unwrap();
        bus.take();

        // Same host, same versions, later stamp.
        store
            .apply_host_reconcile(HostReconcile {
                claude_version: Some("2.1.0"),
                ..empty_probe("alpha", 20)
            })
            .unwrap();
        assert_eq!(
            bus.names(),
            vec!["host:pinged"],
            "nothing moved but the stamp"
        );
        bus.take();

        // A version bump is a real change and still sends the whole row.
        store
            .apply_host_reconcile(HostReconcile {
                claude_version: Some("2.2.0"),
                ..empty_probe("alpha", 30)
            })
            .unwrap();
        assert_eq!(bus.names(), vec!["host:probed"], "a real change is a row");

        // And the stamp the heartbeat reported is the one that was stored.
        assert_eq!(
            store.get_host_row("alpha").unwrap().unwrap().last_pinged_at,
            Some(30)
        );
    }

    #[test]
    fn reconcile_hard_delete_tombstones_the_participant_but_keeps_sent_messages_and_undelivered_mail(
    ) {
        // Previously named `..._reaps_the_inbox_...` and asserted
        // `list_inbox(id, ...)` was empty after the hard-delete — that
        // assertion encoded the pre-existing defect Task 13 fixes (a reaped
        // ghost row used to destroy every undelivered message addressed to
        // it). The message now survives; only the participant identity is
        // tombstoned.
        let mut store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        store.upsert_host("beta").unwrap();
        let id = store
            .upsert_session("work-a", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        let peer = store
            .upsert_session("peer", "beta", None, None, 1, 1, "running", None)
            .unwrap();
        let to_gone = store
            .insert_message(peer, id, "to the gone", "message", None)
            .unwrap();
        store
            .insert_message(id, peer, "from the gone", "message", None)
            .unwrap();
        let participant = store.participant_for_session(id).unwrap().unwrap().id;
        // Two empty reconciles: ghost, then hard-delete.
        for ts in [10, 20] {
            store
                .apply_host_reconcile(empty_probe("alpha", ts))
                .unwrap();
        }
        assert!(store.get_session_by_id(id).unwrap().is_none());
        assert!(
            store.get_message(to_gone).unwrap().is_some(),
            "a hard-deleted ghost row must not destroy undelivered mail"
        );
        let p = store.participant_by_id(participant).unwrap().unwrap();
        assert!(p.retired_at.is_some(), "the identity is tombstoned instead");
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

    /// Redesign 6.4: reconcile records the PR a session's probe saw in
    /// `pull_requests`, marks it merged when the probe says so, and the row
    /// outlives the session.
    #[test]
    fn reconcile_marks_a_pr_merged_and_keeps_it_past_the_session() {
        let (mut store, _bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        let pid = store.upsert_project("o", "r", "/base/r").unwrap();
        let url = "https://github.com/o/r/pull/9";
        let ev = |state: &str, merged_at: Option<i64>| crate::service::outcome::PrEvidence {
            state: Some(state.into()),
            title: Some("Add PRs view".into()),
            head_ref: Some("feat/prs".into()),
            merged_at,
            ..Default::default()
        };
        let pass = |store: &mut Store, ts: i64, sessions: &[ReconcileSession<'_>]| {
            let keep: Vec<String> = sessions.iter().map(|s| s.tmux_name.to_string()).collect();
            store
                .apply_host_reconcile(HostReconcile {
                    sessions,
                    keep: &keep,
                    ..empty_probe("alpha", ts)
                })
                .unwrap();
        };
        let open = vec![ReconcileSession {
            pr_url: Some(url.into()),
            ci_status: Some("pending".into()),
            pr_observed: true,
            pr_evidence: Some(ev("OPEN", None)),
            ..live_session("s1", pid, 10)
        }];
        pass(&mut store, 1, &open);
        let sid = store.get_session("s1", "alpha").unwrap().unwrap().id;
        let rows = store.list_pull_requests(&["OPEN"]).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id, Some(sid), "the session that opened it");
        assert_eq!(rows[0].project_id, Some(pid));
        assert_eq!(rows[0].number, Some(9));
        assert_eq!(rows[0].title.as_deref(), Some("Add PRs view"));
        assert_eq!(rows[0].merged_at, None);

        // A pass that did not probe the PR changes nothing.
        let unprobed = vec![live_session("s1", pid, 11)];
        pass(&mut store, 2, &unprobed);
        assert_eq!(store.list_pull_requests(&["OPEN"]).unwrap().len(), 1);

        let merged = vec![ReconcileSession {
            pr_url: Some(url.into()),
            ci_status: Some("passing".into()),
            pr_observed: true,
            pr_evidence: Some(ev("MERGED", Some(1_791_460_800))),
            ..live_session("s1", pid, 12)
        }];
        pass(&mut store, 3, &merged);
        assert!(store.list_pull_requests(&["OPEN"]).unwrap().is_empty());
        let m = store.list_pull_requests(&["MERGED"]).unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].merged_at, Some(1_791_460_800));
        assert_eq!(m[0].ci_status.as_deref(), Some("passing"));

        // The session goes; the merged PR stays listed.
        store
            .conn_ref()
            .execute("DELETE FROM sessions WHERE id = ?1", [sid])
            .unwrap();
        assert_eq!(store.list_pull_requests(&["MERGED"]).unwrap().len(), 1);
    }

    /// `stale_demoted_at` is a server-only `SessionRow` field (migration
    /// 080): a pass whose only change is lifting it (the row reads
    /// `working`) is not a client-visible change and emits no
    /// `session:updated` — `eq_ignoring_row_version` ignores it.
    #[test]
    fn a_pass_that_only_lifts_stale_demoted_at_emits_no_session_event() {
        let (mut store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        let pid = store.upsert_project("o", "r", "/base/r").unwrap();
        let keep = vec!["s1".to_string()];
        let sessions = vec![ReconcileSession {
            claude_status: Some("working".to_string()),
            ..live_session("s1", pid, 10)
        }];
        let pass = |store: &mut Store, ts: i64| {
            store
                .apply_host_reconcile(HostReconcile {
                    sessions: &sessions,
                    keep: &keep,
                    ..empty_probe("alpha", ts)
                })
                .unwrap();
        };
        pass(&mut store, 1);
        let row = store.get_session("s1", "alpha").unwrap().unwrap();
        assert_eq!(row.claude_status.as_deref(), Some("working"));
        store
            .conn_ref()
            .execute(
                "UPDATE sessions SET stale_demoted_at = 5, context_at = context_at - 5 \
                 WHERE id = ?1",
                [row.id],
            )
            .unwrap();
        let armed = store.get_session_by_id(row.id).unwrap().unwrap();
        assert_eq!(armed.stale_demoted_at, Some(5));
        bus.take();

        pass(&mut store, 2);
        let after = store.get_session_by_id(row.id).unwrap().unwrap();
        assert_eq!(after.stale_demoted_at, None, "a working row lifts the veto");
        assert!(after.eq_ignoring_row_version(&armed));
        assert_eq!(
            bus.take(),
            vec!["host:pinged:alpha".to_string()],
            "a veto-only change must emit no session event"
        );
    }

    /// Migration 099's conversation-owner triggers have to survive the
    /// RECONCILE path, and the first version of them did not.
    ///
    /// `INSERT OR IGNORE` and `ON CONFLICT … DO NOTHING` mean the same thing
    /// in a statement run on its own — and NOT inside a trigger body, when
    /// the statement that FIRES the trigger carries its own conflict clause:
    /// SQLite lets the outer one override an `OR` clause in the body, and
    /// `upsert_session_in_tx` writes `… ON CONFLICT(host_alias, tmux_name)
    /// DO UPDATE SET …`. So the IGNORE was silently an ABORT on exactly this
    /// path, and a second row resuming a conversation another row had
    /// already recorded failed the WHOLE pass with
    /// `UNIQUE constraint failed: conversation_owners.claude_session_id`.
    ///
    /// Nothing in this crate caught it: every other test of those triggers
    /// writes `sessions` with a plain statement, which honours `OR IGNORE`.
    /// `scripts/hub-e2e.sh`'s work-graph block did, 33 failed checks deep in
    /// a cascade from one root failure — which is the argument for keeping
    /// that block gated-but-run in CI rather than trusting the unit suite.
    #[test]
    fn a_reconcile_pass_may_reuse_a_claude_session_id_and_the_first_owner_stands() {
        let (mut store, _bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        let pid = store.upsert_project("o", "r", "/base/r").unwrap();
        let ada = store.create_person("ada", None).unwrap().id;
        let bob = store.create_person("bob", None).unwrap().id;
        assert_ne!(ada, bob, "two distinct owners, or this measures nothing");
        let keep = vec!["a".to_string(), "b".to_string()];

        // Two live rows on one host, one owned by each person.
        store
            .apply_host_reconcile(HostReconcile {
                sessions: &[live_session("a", pid, 10), live_session("b", pid, 10)],
                keep: &keep,
                ..empty_probe("alpha", 1)
            })
            .unwrap();
        let a = store.get_session("a", "alpha").unwrap().unwrap().id;
        let b = store.get_session("b", "alpha").unwrap().unwrap().id;
        store.claim_if_unclaimed(a, Some(ada)).unwrap();
        store.claim_if_unclaimed(b, Some(bob)).unwrap();

        // Ada's row picks up a conversation: the trigger records it as hers.
        store
            .apply_host_reconcile(HostReconcile {
                sessions: &[
                    ReconcileSession {
                        claude_session_id: Some("conv-x".to_string()),
                        ..live_session("a", pid, 11)
                    },
                    live_session("b", pid, 11),
                ],
                keep: &keep,
                ..empty_probe("alpha", 2)
            })
            .unwrap();
        assert_eq!(store.conversation_owner("conv-x").unwrap(), Some(ada));

        // Bob's row now resumes the SAME conversation. This is the pass that
        // used to abort — and the one the e2e hit, because a resume, a
        // recreate and `resume_claude_session_id` all reach it.
        store
            .apply_host_reconcile(HostReconcile {
                sessions: &[
                    ReconcileSession {
                        claude_session_id: Some("conv-x".to_string()),
                        ..live_session("a", pid, 12)
                    },
                    ReconcileSession {
                        claude_session_id: Some("conv-x".to_string()),
                        ..live_session("b", pid, 12)
                    },
                ],
                keep: &keep,
                ..empty_probe("alpha", 3)
            })
            .expect("a reused claude_session_id must not fail the reconcile pass");

        assert_eq!(
            store.conversation_owner("conv-x").unwrap(),
            Some(ada),
            "first writer wins: Bob's row resuming the conversation must not \
             re-home who it belonged to — that record is what the resume gate \
             and the summary fence both ask about"
        );
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
                        None,
                        None,
                        None,
                        false,
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

    /// Ownership (multi-user M1, T5 and its review): **reconcile names no
    /// owner.** The upsert takes no owner argument any more — the name-keyed
    /// reservation it used to read is deleted — so this pins the two
    /// statements that are left: every row it INSERTS is nobody's, and its
    /// `DO UPDATE` branch cannot touch either ownership column, in any
    /// direction, for any row.
    #[test]
    fn a_reconcile_pass_never_writes_ownership() {
        let mut store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        let upsert = |store: &mut Store, activity: i64, probe: i64| {
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
                        probe,
                        None,
                        None,
                        None,
                        None,
                        false,
                        None,
                        &mut out,
                    )?;
                    Ok(out)
                })
                .unwrap()
        };
        upsert(&mut store, 10, 0);
        // The INSERT: nobody's, and `unclaimed` — the safe holding state
        // (spec §4.3), which is the answer for a session reconcile merely
        // found on the host AND for one fleet is in the middle of creating.
        let discovered = store.get_session("s1", "alpha").unwrap().unwrap();
        assert_eq!(discovered.owner_person_id, None);
        assert_eq!(discovered.visibility, crate::store::VISIBILITY_UNCLAIMED);

        // The claim is somebody else's job, keyed on the row id.
        let ann = store.create_person("ann", None).unwrap().id;
        assert!(store.claim_if_unclaimed(discovered.id, Some(ann)).unwrap());
        let claimed = store.get_session("s1", "alpha").unwrap().unwrap();
        assert_eq!(claimed.owner_person_id, Some(ann));
        assert_eq!(claimed.visibility, crate::store::VISIBILITY_PRIVATE);

        // And no number of later passes moves it — neither un-owning the row
        // nor widening its visibility, the two things a pass could break.
        for activity in 11..15 {
            upsert(&mut store, activity, 0);
            let kept = store.get_session("s1", "alpha").unwrap().unwrap();
            assert_eq!(kept.owner_person_id, Some(ann), "pass {activity} re-owned");
            assert_eq!(kept.visibility, crate::store::VISIBILITY_PRIVATE);
        }

        // A row explicitly made somebody's PRIVATE stays private across a
        // resurrect, too: ghost the row, then let a pass revive it.
        store
            .conn_ref()
            .execute(
                "UPDATE sessions SET status='ghost', lost_at=10 WHERE tmux_name='s1'",
                [],
            )
            .unwrap();
        // A probe that started AFTER the loss, or `NOT_STALE` refuses the
        // whole `DO UPDATE` and there is no resurrect to judge.
        upsert(&mut store, 20, 11);
        let revived = store.get_session("s1", "alpha").unwrap().unwrap();
        assert_eq!(revived.status, "running", "the resurrect still happens");
        assert_eq!(revived.owner_person_id, Some(ann));
        assert_eq!(revived.visibility, crate::store::VISIBILITY_PRIVATE);
    }

    #[test]
    fn a_failed_probe_keeps_its_error_and_the_last_reachable_stamp() {
        // Review r13: `last_pinged_at` moves on a failed probe (down_hosts
        // and the event diff rely on it), so "last answered" needs its own
        // stamp, and the offline state needs the reason.
        let (mut store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        store
            .apply_host_reconcile(empty_probe("alpha", 100))
            .unwrap();
        let up = store.get_host_row("alpha").unwrap().unwrap();
        assert_eq!(up.last_reachable_at, Some(100));
        assert_eq!(up.last_probe_error_code, None);

        let down = || HostReconcile {
            reachable: false,
            ..empty_probe("alpha", 200)
        };
        bus.take();
        store
            .apply_host_reconcile_failed(down(), ("E_SSH_TIMEOUT", "ssh timed out"))
            .unwrap();
        let h = store.get_host_row("alpha").unwrap().unwrap();
        assert!(!h.reachable);
        assert_eq!(h.last_pinged_at, Some(200), "the ping still moves");
        assert_eq!(h.last_reachable_at, Some(100), "last answered stays put");
        assert_eq!(h.last_probe_error_code.as_deref(), Some("E_SSH_TIMEOUT"));
        assert_eq!(h.last_probe_error.as_deref(), Some("ssh timed out"));
        assert_eq!(bus.names(), vec!["host:probed"], "the error is news");

        // The same failure again is only a ping.
        bus.take();
        store
            .apply_host_reconcile_failed(
                HostReconcile {
                    last_pinged_at: 300,
                    ..down()
                },
                ("E_SSH_TIMEOUT", "ssh timed out"),
            )
            .unwrap();
        assert_eq!(bus.names(), vec!["host:pinged"]);

        // A long message is cut short.
        let long = "x".repeat(1000);
        store
            .apply_host_reconcile_failed(down(), ("E_SSH", &long))
            .unwrap();
        let h = store.get_host_row("alpha").unwrap().unwrap();
        assert_eq!(
            h.last_probe_error.unwrap().chars().count(),
            PROBE_ERROR_CHARS
        );

        // The next answered probe clears the error and moves the stamp.
        store
            .apply_host_reconcile(empty_probe("alpha", 400))
            .unwrap();
        let h = store.get_host_row("alpha").unwrap().unwrap();
        assert_eq!(h.last_reachable_at, Some(400));
        assert_eq!(h.last_probe_error_code, None);
        assert_eq!(h.last_probe_error, None);

        // update_host_probe shares the write.
        store
            .update_host_probe("alpha", false, None, None, 500)
            .unwrap();
        let h = store.get_host_row("alpha").unwrap().unwrap();
        assert_eq!(
            (h.last_pinged_at, h.last_reachable_at),
            (Some(500), Some(400))
        );
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

        // Pass 2 runs in a later second than pass 1 on every run: backdate
        // the footer stamp so a pass that re-stamps an unchanged footer
        // value fails deterministically, not only across a second boundary.
        store
            .conn_ref()
            .execute(
                "UPDATE sessions SET context_at = context_at - 5 WHERE tmux_name = 's1'",
                [],
            )
            .unwrap();
        let backdated: Option<i64> = store
            .conn_ref()
            .query_row(
                "SELECT context_at FROM sessions WHERE tmux_name = 's1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(backdated.is_some(), "the footer value was stamped");

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
            vec!["host:pinged:alpha".to_string()],
            "an unchanged row must emit neither session nor project events — and \
             the host itself, also unchanged, only its heartbeat"
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
    fn reconcile_clears_pending_input_only_when_pane_observed() {
        let (mut store, _bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        let pid = store.upsert_project("o", "r", "/base/r").unwrap();

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
        let pass = |store: &mut Store, pending: Option<PendingInput>, observed: bool| {
            let sessions = vec![ReconcileSession {
                tmux_name: "s1",
                project_id: Some(pid),
                created_at: 1,
                last_activity_at: 1,
                pending_input: pending,
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
        let pending_of = |store: &Store| {
            store
                .get_session("s1", "alpha")
                .unwrap()
                .unwrap()
                .pending_input
        };

        // Observed pane, dialog detected → stored.
        pass(&mut store, Some(pi.clone()), true);
        assert_eq!(pending_of(&store), Some(pi.clone()));

        // Capture FAILED (pane not observed), no dialog → prior value preserved.
        pass(&mut store, None, false);
        assert_eq!(
            pending_of(&store),
            Some(pi),
            "must preserve pending_input when the pane was not observed"
        );

        // Observed pane, dialog no longer present → CLEARED.
        pass(&mut store, None, true);
        assert_eq!(
            pending_of(&store),
            None,
            "must clear pending_input when the pane was observed and shows no dialog"
        );
    }

    /// Result evidence (migration 082) follows `ci_status`'s rule, and
    /// `pr_checked_at` is stamped for a new or changed reading but not for
    /// every probe of a steady one, so a PR that sits green does not make
    /// each probe emit.
    #[test]
    fn reconcile_writes_pr_evidence_like_ci_status_and_stamps_it_sparingly() {
        use crate::service::outcome::{PrEvidence, PR_CHECKED_REFRESH_SECS};
        let (mut store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        let pr = Some("https://github.com/o/r/pull/1".to_string());
        let ev = |head: &str| PrEvidence {
            head_oid: Some(head.into()),
            local_head: Some(head.into()),
            ahead: Some(0),
            dirty: Some(false),
            ..Default::default()
        };
        let pass = |store: &mut Store,
                    pr_url: Option<String>,
                    evidence: Option<PrEvidence>,
                    observed: bool| {
            let sessions = vec![ReconcileSession {
                tmux_name: "s1",
                created_at: 1,
                last_activity_at: 1,
                pr_url,
                ci_status: observed.then(|| "passing".to_string()),
                pr_observed: observed,
                pr_evidence: evidence,
                ..Default::default()
            }];
            store
                .apply_host_reconcile(HostReconcile {
                    sessions: &sessions,
                    keep: &["s1".to_string()],
                    ..empty_probe("alpha", 1)
                })
                .unwrap();
            store.get_session("s1", "alpha").unwrap().unwrap()
        };
        // The session's own events: every pass also emits `host:probed`.
        let session_events = || {
            bus.take()
                .into_iter()
                .filter(|e| e.starts_with("session:"))
                .count()
        };
        let backdate = |store: &Store, secs: i64| {
            store
                .conn
                .execute(
                    "UPDATE sessions SET pr_checked_at = pr_checked_at - ?1",
                    [secs],
                )
                .unwrap();
        };

        // First sight: stored and stamped.
        let row = pass(&mut store, pr.clone(), Some(ev("aaaaaaa")), true);
        assert_eq!(row.pr_evidence, Some(ev("aaaaaaa")));
        let first = row.pr_checked_at.expect("stamped on first sight");
        bus.take();

        // A pass that did not probe keeps both.
        let row = pass(&mut store, None, None, false);
        assert_eq!(row.pr_evidence, Some(ev("aaaaaaa")));
        assert_eq!(row.pr_checked_at, Some(first));
        assert_eq!(session_events(), 0, "nothing changed, nothing emitted");

        // The same reading inside the refresh window: no stamp, no event.
        backdate(&store, 5);
        let row = pass(&mut store, pr.clone(), Some(ev("aaaaaaa")), true);
        assert_eq!(
            row.pr_checked_at,
            Some(first - 5),
            "steady reading, stamp kept"
        );
        assert_eq!(session_events(), 0, "a steady PR does not emit per probe");

        // The same reading once the stamp is a refresh old: re-stamped.
        backdate(&store, PR_CHECKED_REFRESH_SECS);
        let row = pass(&mut store, pr.clone(), Some(ev("aaaaaaa")), true);
        assert!(row.pr_checked_at.unwrap() >= first, "refreshed");
        assert_eq!(session_events(), 1, "one event per refresh");

        // A changed reading is stamped at once.
        backdate(&store, 5);
        let row = pass(&mut store, pr.clone(), Some(ev("bbbbbbb")), true);
        assert_eq!(row.pr_evidence, Some(ev("bbbbbbb")));
        assert!(row.pr_checked_at.unwrap() >= first, "changed ⇒ stamped");

        // An old `gh` (PR seen, no evidence fields) clears the evidence
        // rather than leaving an old reading that looks current.
        let row = pass(&mut store, pr.clone(), None, true);
        assert_eq!(row.pr_evidence, None);
        assert!(row.pr_checked_at.is_some());

        // A definite "no PR" clears both.
        let row = pass(&mut store, None, None, true);
        assert_eq!((row.pr_evidence, row.pr_checked_at), (None, None));

        // A malformed stored value reads as none instead of failing reads.
        store
            .conn
            .execute("UPDATE sessions SET pr_evidence = '{not json'", [])
            .unwrap();
        assert_eq!(
            store
                .get_session("s1", "alpha")
                .unwrap()
                .unwrap()
                .pr_evidence,
            None
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
                100,
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
                100,
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

    /// Adopt into (step 4.12): a fleet-run row keeps the project a person
    /// put it in while its pane's directory is in no project; a row fleet
    /// does not run follows the observation as before.
    #[test]
    fn a_fleet_run_row_keeps_its_project_when_the_pane_is_in_none() {
        let mut s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let pid = s
            .upsert_project("acme", "papaya-pos", "/p/acme/papaya-pos")
            .unwrap();
        let r = reconcile_one(&mut s, "scratch", None, None, None);
        s.set_session_project(r.id, pid).unwrap();
        let r = reconcile_one(&mut s, "scratch", None, None, None);
        assert_eq!(r.project_id, None, "not fleet's: the observation wins");

        s.set_session_project(r.id, pid).unwrap();
        s.set_started_at(r.id, 5).unwrap();
        let r = reconcile_one(&mut s, "scratch", None, None, None);
        assert_eq!(r.project_id, Some(pid), "adopted into it: kept");
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

    /// A failed turn leaves the input box on screen, which the pane
    /// heuristic reads as `idle` every 20 s. The hook's `failed` must
    /// outlive that read (the in-flight guard only covers a pass already
    /// running when the hook landed), and give way to a real turn.
    #[test]
    fn a_hook_stamped_failure_survives_the_panes_idle_prompt_but_not_a_turn() {
        let mut s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let r = reconcile_one(&mut s, "a", Some("working"), None, None);
        s.record_stop_failure_hook_for_row(r.id).unwrap();
        let pass = |s: &mut Store, status: &str| -> SessionRow {
            s.apply_host_reconcile(HostReconcile {
                alias: "local",
                reachable: true,
                claude_version: None,
                tmux_version: None,
                last_pinged_at: now_unix() + 5,
                // A pass that STARTED after the hook: the guard does not apply.
                probe_started_at: now_unix() + 5,
                sessions: &[ReconcileSession {
                    tmux_name: "a",
                    created_at: 1,
                    last_activity_at: 1,
                    claude_status: Some(status.to_string()),
                    intel_observed: true,
                    ..Default::default()
                }],
                keep: &["a".to_string()],
                lost_ttl_cutoff: None,
                skip_prune: false,
                reconciled_at: None,
            })
            .unwrap();
            s.get_session("a", "local").unwrap().unwrap()
        };
        assert_eq!(
            pass(&mut s, "idle").claude_status.as_deref(),
            Some("failed")
        );
        assert_eq!(
            pass(&mut s, "blocked").claude_status.as_deref(),
            Some("blocked")
        );
        s.record_stop_failure_hook_for_row(r.id).unwrap();
        assert_eq!(
            pass(&mut s, "working").claude_status.as_deref(),
            Some("working")
        );
    }

    /// The routine Phase 1 ghosting clears the same fields
    /// `mark_host_sessions_lost` does (F4).
    #[test]
    fn phase_one_ghosting_clears_the_pane_derived_fields() {
        let mut s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let r = reconcile_one(&mut s, "a", Some("blocked"), Some("press_enter"), None);
        assert_eq!(r.claude_status.as_deref(), Some("blocked"));
        s.apply_host_reconcile(HostReconcile {
            alias: "local",
            reachable: true,
            claude_version: None,
            tmux_version: None,
            last_pinged_at: 2,
            probe_started_at: 0,
            sessions: &[],
            keep: &[],
            lost_ttl_cutoff: None,
            skip_prune: false,
            reconciled_at: None,
        })
        .unwrap();
        let g = s.get_session("a", "local").unwrap().unwrap();
        assert_eq!(g.status, "ghost");
        assert_eq!(g.claude_status, None);
        assert_eq!(g.stuck_kind, None);
        assert_eq!(g.stuck_since, None);
        assert_eq!(g.pending_input, None);
    }

    /// F8: `noble-virgo-term` (kind `shell`) read as `idle` because the
    /// pane heuristic took a bare `❯` for the REPL's prompt. A shell has no
    /// Claude status to derive.
    #[test]
    fn a_shell_row_never_gets_a_pane_derived_claude_status() {
        let mut s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session(
                "noble-virgo-term",
                "local",
                None,
                None,
                1,
                1,
                "running",
                None,
            )
            .unwrap();
        s.conn
            .execute("UPDATE sessions SET kind = 'shell' WHERE id = ?1", [id])
            .unwrap();
        let r = reconcile_one(
            &mut s,
            "noble-virgo-term",
            Some("idle"),
            Some("press_enter"),
            None,
        );
        assert_eq!(r.kind, "shell");
        assert_eq!(
            r.claude_status, None,
            "a bare ❯ is a shell prompt, not an idle REPL"
        );
        assert_eq!(r.stuck_kind, None);
        assert_eq!(r.pending_input, None);
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

    /// F1: `claude --resume` re-renders the last messages, so the `oom`
    /// text the playbook recreated the session for comes straight back.
    /// Today that is a NEW episode (`stuck_since` restarts on NULL → oom)
    /// and only the 1 h spacing stands between two recreates. A re-fire
    /// within that spacing of the playbook's last action is the same
    /// episode, anchored on that action, so the planner's "already acted on
    /// this episode" rule holds. `press_enter` keeps today's rule.
    #[test]
    fn an_oom_refire_within_the_recreate_spacing_continues_the_episode() {
        let mut s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let r = reconcile_one(&mut s, "a", Some("blocked"), Some("oom"), None);
        let acted = now_unix() - 100;
        s.mark_playbook_applied(r.id, acted, "oom:recreate")
            .unwrap();
        // The fresh pane read clean …
        let r = reconcile_one(&mut s, "a", Some("idle"), None, None);
        assert_eq!(r.stuck_since, None);
        // … then `--resume` painted the word again.
        let r = reconcile_one(&mut s, "a", Some("blocked"), Some("oom"), None);
        assert_eq!(
            r.stuck_since,
            Some(acted),
            "the episode is the recreate's, not a new one"
        );
        assert!(r.last_playbook_at.unwrap() >= r.stuck_since.unwrap());

        let p = reconcile_one(&mut s, "b", Some("blocked"), Some("press_enter"), None);
        s.mark_playbook_applied(p.id, now_unix() - 100, "press_enter:press_enter")
            .unwrap();
        reconcile_one(&mut s, "b", Some("idle"), None, None);
        let p = reconcile_one(&mut s, "b", Some("blocked"), Some("press_enter"), None);
        assert!(
            p.stuck_since.unwrap() > now_unix() - 100,
            "a second Enter prompt is a new episode"
        );
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
            // Newer than the seeding passes: each single-row `reconcile_one`
            // above incidentally ghosted the other row, and only a probe
            // newer than that loss revives "a" (#170) — a ghost's claim on
            // ID_A would otherwise not block "b" at all.
            probe_started_at: now_unix() + 1,
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

    fn pass_with_id(s: &mut Store, name: &'static str, id: &str, started: i64) {
        s.apply_host_reconcile(HostReconcile {
            probe_started_at: started,
            sessions: &[ReconcileSession {
                tmux_name: name,
                created_at: 1,
                last_activity_at: 1,
                claude_session_id: Some(id.into()),
                ..Default::default()
            }],
            keep: &[name.to_string()],
            ..empty_probe("local", 1)
        })
        .unwrap();
    }

    #[test]
    fn an_id_change_marks_the_context_stale_but_a_first_sighting_does_not() {
        let (mut s, _) = store_with_recorder();
        s.upsert_host("local").unwrap();
        let stale = |s: &Store, name: &str| {
            s.get_session(name, "local")
                .unwrap()
                .unwrap()
                .context
                .context_stale
        };
        // First sighting of a new row carrying an id: nothing stored was
        // stale, so nothing is marked.
        pass_with_id(&mut s, "a", ID_A, 0);
        assert!(!stale(&s, "a"));
        // A real change: the stored size belongs to the old conversation.
        let a = s.get_session("a", "local").unwrap().unwrap();
        s.set_context(a.id, ID_A, 50_000, 200_000, "transcript", None)
            .unwrap();
        pass_with_id(&mut s, "a", "cccccccc-cccc-cccc-cccc-cccccccccccc", 0);
        assert!(stale(&s, "a"));
        // First sighting of an id on a row whose id was NULL: not stale
        // either. (Each single-row pass ghosts the other row; fine here.)
        reconcile_one(&mut s, "b", None, None, None);
        pass_with_id(&mut s, "b", ID_B, 0);
        assert!(!stale(&s, "b"));
    }

    #[test]
    fn reconcile_never_undoes_a_hook_rebind_newer_than_its_probe() {
        let (mut s, _) = store_with_recorder();
        s.upsert_host("local").unwrap();
        let row = reconcile_one(&mut s, "a", None, None, None);
        s.rebind_conversation(row.id, ID_A, StartSource::Fleet, None, None)
            .unwrap();
        // /clear: the hook moves the row to B and stamps last_hook_at.
        s.close_conversation(row.id, ID_A, "clear").unwrap();
        s.rebind_conversation(row.id, ID_B, StartSource::Clear, Some("/t/b.jsonl"), None)
            .unwrap();
        s.record_hook_seen(row.id).unwrap();
        let hook_at: i64 = s
            .conn
            .query_row(
                "SELECT last_hook_at FROM sessions WHERE id=?1",
                [row.id],
                |r| r.get(0),
            )
            .unwrap();
        // A pass that probed before the hook still reports A.
        pass_with_id(&mut s, "a", ID_A, hook_at - 1);
        let after = s.get_session("a", "local").unwrap().unwrap();
        assert_eq!(after.claude_session_id.as_deref(), Some(ID_B));
        assert_eq!(
            s.session_transcript_path(row.id).unwrap().as_deref(),
            Some("/t/b.jsonl")
        );
        assert!(!after.context.context_stale);
        assert_eq!(after.context_pct, Some(0.0));
        // A pass that probed after the hook is authoritative again.
        pass_with_id(&mut s, "a", ID_A, hook_at + 1);
        assert_eq!(
            s.get_session("a", "local")
                .unwrap()
                .unwrap()
                .claude_session_id
                .as_deref(),
            Some(ID_A)
        );
    }

    /// 14 days, matching `SESSIONS_LOST_TTL_SECS`'s default — kept as a
    /// literal here so these tests don't reach into `service::settings`.
    const TTL_SECS: i64 = 1_209_600;

    /// Seed a row directly as already-ghost (bypassing Phase 1) with the
    /// given `lost_at` / `lost_reason` / `claude_session_id`, so a single
    /// `apply_host_reconcile` pass exercises Phase 2's exemption straight
    /// away. `lost_reason: None` writes SQL `NULL`, matching a row ghosted
    /// before migration 036 introduced the column.
    fn seed_ghost_row(
        store: &Store,
        host: &str,
        name: &str,
        lost_at: i64,
        lost_reason: Option<&str>,
        claude_session_id: Option<&str>,
    ) -> i64 {
        let id = store
            .upsert_session(name, host, None, None, 1, 1, "running", None)
            .unwrap();
        if let Some(uuid) = claude_session_id {
            store.set_claude_session_id(id, uuid).unwrap();
        }
        store
            .conn
            .execute(
                "UPDATE sessions SET status='ghost', lost_at=?1, lost_reason=?2 WHERE id=?3",
                rusqlite::params![lost_at, lost_reason, id],
            )
            .unwrap();
        id
    }

    #[test]
    fn a_resumable_mass_loss_row_survives_the_reap() {
        let mut store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        let now = 2_000_000;
        let cutoff = now - TTL_SECS;
        let id = seed_ghost_row(
            &store,
            "alpha",
            "s1",
            now - 100,
            Some("host_reboot"),
            Some("uuid-a"),
        );
        for ts in [now, now + 10] {
            store
                .apply_host_reconcile(HostReconcile {
                    lost_ttl_cutoff: Some(cutoff),
                    keep: &[],
                    ..empty_probe("alpha", ts)
                })
                .unwrap();
        }
        assert!(
            store.get_session_by_id(id).unwrap().is_some(),
            "a resumable host_reboot row within the TTL must survive the reap"
        );
    }

    #[test]
    fn a_missing_row_is_still_reaped_on_the_next_pass() {
        let mut store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        let now = 2_000_000;
        let cutoff = now - TTL_SECS;
        let id = store
            .upsert_session("s1", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store.set_claude_session_id(id, "uuid-a").unwrap();
        // Pass 1: live and not in keep → ghosted normally with lost_reason='missing'.
        store
            .apply_host_reconcile(HostReconcile {
                lost_ttl_cutoff: Some(cutoff),
                keep: &[],
                ..empty_probe("alpha", now)
            })
            .unwrap();
        assert_eq!(
            store.get_session_by_id(id).unwrap().unwrap().status,
            "ghost",
            "pass 1 ghosts the row"
        );
        // Pass 2: already ghost before this pass — 'missing' is not exempt,
        // even with a TTL cutoff set, so the one-cycle grace still applies.
        store
            .apply_host_reconcile(HostReconcile {
                lost_ttl_cutoff: Some(cutoff),
                keep: &[],
                ..empty_probe("alpha", now + 10)
            })
            .unwrap();
        assert!(
            store.get_session_by_id(id).unwrap().is_none(),
            "a 'missing' row keeps the one-cycle reap even with a TTL cutoff set"
        );
    }

    #[test]
    fn a_mass_loss_row_without_a_claude_id_is_reaped() {
        let mut store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        let now = 2_000_000;
        let cutoff = now - TTL_SECS;
        let id = seed_ghost_row(&store, "alpha", "s1", now - 100, Some("host_reboot"), None);
        for ts in [now, now + 10] {
            store
                .apply_host_reconcile(HostReconcile {
                    lost_ttl_cutoff: Some(cutoff),
                    keep: &[],
                    ..empty_probe("alpha", ts)
                })
                .unwrap();
        }
        assert!(
            store.get_session_by_id(id).unwrap().is_none(),
            "a mass-loss row with no claude_session_id is not resumable and must be reaped"
        );
    }

    #[test]
    fn a_mass_loss_row_older_than_the_ttl_is_reaped() {
        let mut store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        let now = 2_000_000;
        let cutoff = now - TTL_SECS;
        let id = seed_ghost_row(
            &store,
            "alpha",
            "s1",
            cutoff - 1,
            Some("host_reboot"),
            Some("uuid-a"),
        );
        for ts in [now, now + 10] {
            store
                .apply_host_reconcile(HostReconcile {
                    lost_ttl_cutoff: Some(cutoff),
                    keep: &[],
                    ..empty_probe("alpha", ts)
                })
                .unwrap();
        }
        assert!(
            store.get_session_by_id(id).unwrap().is_none(),
            "a mass-loss row whose lost_at is older than the TTL cutoff must be reaped"
        );
    }

    #[test]
    fn with_no_cutoff_nothing_is_exempt() {
        let mut store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        let now = 2_000_000;
        let id = seed_ghost_row(
            &store,
            "alpha",
            "s1",
            now - 100,
            Some("host_reboot"),
            Some("uuid-a"),
        );
        for ts in [now, now + 10] {
            store
                .apply_host_reconcile(HostReconcile {
                    lost_ttl_cutoff: None,
                    keep: &[],
                    ..empty_probe("alpha", ts)
                })
                .unwrap();
        }
        assert!(
            store.get_session_by_id(id).unwrap().is_none(),
            "a None cutoff means no exemption at all — today's behaviour"
        );
    }

    #[test]
    fn a_resumable_row_and_a_missing_row_are_judged_correctly_alongside_a_kept_live_row() {
        // The other tests above all pass an EMPTY `keep`, so `not_in` is the
        // empty string and never appears in the SQL — they can't catch
        // `exempt` drifting to AFTER `not_in` in the query text, which would
        // let a non-empty `not_in`'s bare `?`s claim `?2` before `exempt`'s
        // explicit `?2` does (misbinding the cutoff to a keep name, or
        // erroring on the parameter count once a real keep set is in play).
        // This test exercises `exempt` and a non-empty `not_in` together.
        let mut store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        let now = 2_000_000;
        let cutoff = now - TTL_SECS;

        let kept_id = store
            .upsert_session("keep-me", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        let resumable_id = seed_ghost_row(
            &store,
            "alpha",
            "resumable",
            now - 100,
            Some("host_reboot"),
            Some("uuid-a"),
        );
        let missing_id = seed_ghost_row(&store, "alpha", "gone", now - 100, Some("missing"), None);

        store
            .apply_host_reconcile(HostReconcile {
                lost_ttl_cutoff: Some(cutoff),
                keep: &["keep-me".to_string()],
                ..empty_probe("alpha", now)
            })
            .unwrap();

        assert_eq!(
            store.get_session_by_id(kept_id).unwrap().unwrap().status,
            "running",
            "the kept live row must stay running alongside an active TTL cutoff"
        );
        assert!(
            store.get_session_by_id(resumable_id).unwrap().is_some(),
            "the resumable ghost must survive alongside a non-empty keep set"
        );
        assert!(
            store.get_session_by_id(missing_id).unwrap().is_none(),
            "the 'missing' ghost must still be reaped alongside a non-empty keep set"
        );
    }

    #[test]
    fn a_pre_migration_ghost_row_with_null_lost_reason_is_reaped() {
        // Rows ghosted before migration 036 added `lost_reason` have it
        // NULL. SQL three-valued logic must not let that NULL silently
        // exempt them: `lost_reason IN (...)` on NULL is NULL, so an
        // un-coalesced `NOT (... AND NULL AND ...)` is NULL too, and a
        // `WHERE` clause treats NULL as "leave this row out of the reaped
        // set" — i.e. wrongly exempting it. This would be a behaviour
        // change for every pre-migration row after an upgrade, which the
        // plan forbids.
        let mut store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        let now = 2_000_000;
        let cutoff = now - TTL_SECS;
        let id = seed_ghost_row(&store, "alpha", "s1", now - 100, None, Some("uuid-a"));
        for ts in [now, now + 10] {
            store
                .apply_host_reconcile(HostReconcile {
                    lost_ttl_cutoff: Some(cutoff),
                    keep: &[],
                    ..empty_probe("alpha", ts)
                })
                .unwrap();
        }
        assert!(
            store.get_session_by_id(id).unwrap().is_none(),
            "a NULL lost_reason (a pre-migration row) must not be silently \
             exempted; it keeps today's one-cycle reap"
        );
    }

    #[test]
    fn a_mass_loss_row_exactly_at_the_ttl_cutoff_survives() {
        // Pins the `>=` boundary: `lost_at == cutoff` must be inclusive.
        let mut store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        let now = 2_000_000;
        let cutoff = now - TTL_SECS;
        let id = seed_ghost_row(
            &store,
            "alpha",
            "s1",
            cutoff,
            Some("host_reboot"),
            Some("uuid-a"),
        );
        for ts in [now, now + 10] {
            store
                .apply_host_reconcile(HostReconcile {
                    lost_ttl_cutoff: Some(cutoff),
                    keep: &[],
                    ..empty_probe("alpha", ts)
                })
                .unwrap();
        }
        assert!(
            store.get_session_by_id(id).unwrap().is_some(),
            "lost_at exactly at the cutoff must survive (inclusive >=)"
        );
    }

    // ── #170: a lost row is revived only by an observation newer than the
    // loss ──────────────────────────────────────────────────────────────────

    /// One `upsert_session_in_tx` on `alpha`, as a probe that STARTED at
    /// `probe_started_at` and observed `name` live and `working`, returning
    /// the changes it pushed. `working` is deliberate: a refused resurrect
    /// must not repaint a ghost as busy either.
    fn observe_live(store: &mut Store, name: &str, probe_started_at: i64) -> Vec<RowChange> {
        store
            .with_transaction(|tx| {
                let mut out = Vec::new();
                Store::upsert_session_in_tx(
                    tx,
                    name,
                    "alpha",
                    None,
                    None,
                    1,
                    777,
                    None,
                    None,
                    None,
                    Some("working"),
                    None,
                    None,
                    None,
                    None,
                    None,
                    true,
                    None,
                    false,
                    probe_started_at,
                    None,
                    None,
                    None,
                    None,
                    false,
                    None,
                    &mut out,
                )?;
                Ok(out)
            })
            .unwrap()
    }

    /// The same session as [`observe_live`] sees it, shaped for a full
    /// `apply_host_reconcile` pass (no project, so no FK to satisfy).
    fn live_unbound(tmux_name: &'static str) -> ReconcileSession<'static> {
        ReconcileSession {
            tmux_name,
            created_at: 1,
            last_activity_at: 777,
            claude_status: Some("working".to_string()),
            intel_observed: true,
            ..Default::default()
        }
    }

    fn lost_reason_of(store: &Store, id: i64) -> Option<String> {
        store
            .conn
            .query_row("SELECT lost_reason FROM sessions WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .unwrap()
    }

    /// The row `kill_session` just ghosted, ready for a stale observation.
    fn killed_row(store: &Store, name: &str, killed_at: i64) -> i64 {
        let id = store
            .upsert_session(name, "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store
            .mark_session_killed(id, killed_at)
            .unwrap()
            .expect("ghosted");
        id
    }

    #[test]
    fn a_probe_older_than_the_kill_does_not_resurrect_the_ghost() {
        // #170: a full pass whose probe listed tmux BEFORE `kill_session`
        // ghosted the row still carries its name in `keep`. Its write lands
        // after the kill — and must leave the ghost exactly as it found it.
        let mut store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        let id = killed_row(&store, "s1", 200);

        let changes = observe_live(&mut store, "s1", 199);

        let row = store.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.status, "ghost", "a stale probe must not revive");
        assert_eq!(row.lost_at, Some(200), "lost_at must be left alone");
        assert_eq!(lost_reason_of(&store, id).as_deref(), Some("killed"));
        assert_eq!(
            row.claude_status, None,
            "a stale observation must not repaint the ghost as working"
        );
        assert_eq!(row.last_activity_at, 1, "nor move its activity stamp");
        assert!(
            changes.is_empty(),
            "nothing changed ⇒ nothing to announce; got {} entries",
            changes.len()
        );
    }

    #[test]
    fn a_probe_newer_than_the_kill_resurrects_the_ghost() {
        // The other direction: a session genuinely recreated under the same
        // name still comes back, with one `SessionUpdated` for the revival.
        let mut store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        let id = killed_row(&store, "s1", 200);

        let changes = observe_live(&mut store, "s1", 201);

        let row = store.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.status, "running", "a newer probe revives");
        assert_eq!(row.lost_at, None);
        assert_eq!(lost_reason_of(&store, id), None);
        assert_eq!(row.claude_status.as_deref(), Some("working"));
        assert_eq!(changes.len(), 1, "exactly one revival change");
        assert!(matches!(changes[0], RowChange::SessionUpdated(_)));
    }

    #[test]
    fn a_probe_started_in_the_same_second_as_the_loss_does_not_resurrect() {
        // Both stamps are unix SECONDS off the same clock, so a probe that
        // started within the losing second cannot be shown to have observed
        // the session after it died: same-instant is not newer.
        let mut store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        let id = killed_row(&store, "s1", 200);

        let changes = observe_live(&mut store, "s1", 200);

        let row = store.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.status, "ghost", "same second is not newer");
        assert_eq!(row.lost_at, Some(200));
        assert!(changes.is_empty(), "a refused resurrect announces nothing");
    }

    #[test]
    fn an_unknown_probe_time_does_not_resurrect_a_lost_row() {
        // `probe_started_at <= 0` means "no probe time" (see `ghost_cutoff`).
        // Nothing in production passes it for a real pass; the conservative
        // reading is that an observation that cannot be dated cannot be shown
        // to be newer than the loss.
        let mut store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        let id = killed_row(&store, "s1", 200);

        assert!(observe_live(&mut store, "s1", 0).is_empty());

        let row = store.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.status, "ghost");
        assert_eq!(lost_reason_of(&store, id).as_deref(), Some("killed"));
    }

    #[test]
    fn a_killed_session_is_reaped_on_schedule_despite_a_stale_pass() {
        // The whole #170 sequence: create → kill → a stale full pass that
        // still lists the name → the next (fresh) pass. The row must be
        // hard-deleted on its ordinary one-cycle schedule, `session:killed`
        // must fire exactly once, and no second "lost" may be logged in
        // between (`lifecycle_kind`: a second "lost" with no revival is a bug).
        let (mut store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        let live = vec![live_unbound("s1")];
        let keep = vec!["s1".to_string()];

        // Create, through a real pass.
        store
            .apply_host_reconcile(HostReconcile {
                probe_started_at: 100,
                sessions: &live,
                keep: &keep,
                ..empty_probe("alpha", 100)
            })
            .unwrap();
        let id = store.get_session("s1", "alpha").unwrap().unwrap().id;

        // kill_session: ghost + lost_reason='killed'.
        store
            .mark_session_killed(id, 200)
            .unwrap()
            .expect("ghosted");
        bus.take();

        // The stale pass: its probe started before the kill, but its write
        // lands after it.
        store
            .apply_host_reconcile(HostReconcile {
                probe_started_at: 150,
                sessions: &live,
                keep: &keep,
                ..empty_probe("alpha", 201)
            })
            .unwrap();
        let row = store.get_session_by_id(id).unwrap().expect("still there");
        assert_eq!(row.status, "ghost", "the stale pass must not revive it");
        assert_eq!(lost_reason_of(&store, id).as_deref(), Some("killed"));
        let evts = bus.take();
        assert!(
            evts.iter().all(|e| e.starts_with("host:")),
            "the stale pass must announce nothing about the session; got {evts:?}"
        );

        // The next pass, probed after the kill, no longer sees the session:
        // the row was already ghost before it, so Phase 2 reaps it.
        store
            .apply_host_reconcile(HostReconcile {
                probe_started_at: 220,
                keep: &[],
                ..empty_probe("alpha", 220)
            })
            .unwrap();
        assert!(
            store.get_session_by_id(id).unwrap().is_none(),
            "the killed row must be reaped on the ordinary one-cycle schedule"
        );
        let evts = bus.take();
        assert_eq!(
            evts.iter()
                .filter(|e| *e == &format!("session:killed:{id}"))
                .count(),
            1,
            "session:killed must fire exactly once; got {evts:?}"
        );
    }

    #[test]
    fn a_mass_loss_row_is_revived_by_a_fresh_probe_but_not_a_stale_one() {
        // PR #135's rows (`host_reboot` / `tmux_server_gone`) are kept as
        // ghosts so they can be restored. A session that reappears under the
        // same name must still be resurrected by a probe that ran after the
        // verdict — and must not be by one that ran before it.
        let mut store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        let id = seed_ghost_row(
            &store,
            "alpha",
            "s1",
            200,
            Some("host_reboot"),
            Some("uuid-a"),
        );

        assert!(
            observe_live(&mut store, "s1", 199).is_empty(),
            "a probe older than the verdict must not revive the row"
        );
        assert_eq!(
            store.get_session_by_id(id).unwrap().unwrap().status,
            "ghost"
        );
        assert_eq!(lost_reason_of(&store, id).as_deref(), Some("host_reboot"));

        let changes = observe_live(&mut store, "s1", 201);
        let row = store.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.status, "running", "the host came back: revive");
        assert_eq!(row.lost_at, None);
        assert_eq!(lost_reason_of(&store, id), None);
        assert_eq!(
            row.claude_session_id.as_deref(),
            Some("uuid-a"),
            "the revived row keeps its conversation"
        );
        assert_eq!(changes.len(), 1, "exactly one revival change");
    }

    #[test]
    fn a_new_session_reusing_a_ghosts_tmux_name_ends_up_live() {
        // `new_session` has no upsert of its own: it creates the tmux session
        // and then runs `reconcile_one_host`, whose probe starts after the
        // create — and therefore after any older ghost's `lost_at`. Reusing a
        // dead session's name must still end with one live row.
        let (mut store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        let id = killed_row(&store, "dev-o-r", 200);
        bus.take();

        // The create's own single-host reconcile.
        let live = vec![live_unbound("dev-o-r")];
        store
            .apply_host_reconcile(HostReconcile {
                probe_started_at: 205,
                sessions: &live,
                keep: &["dev-o-r".to_string()],
                ..empty_probe("alpha", 205)
            })
            .unwrap();

        let row = store.get_session("dev-o-r", "alpha").unwrap().unwrap();
        assert_eq!(row.id, id, "the same row is reused");
        assert_eq!(row.status, "running");
        assert_eq!(row.lost_at, None);
        assert_eq!(lost_reason_of(&store, id), None);
    }

    // ── #171: a reaped killed row is re-inserted only by a pass that probed
    // after the kill ────────────────────────────────────────────────────────

    /// Create `name` on `alpha` through a real pass, kill it, and let the
    /// kill's own single-host reconcile reap the (already ghost) row — the
    /// exact sequence `kill_session` runs. Returns the id the row had.
    ///
    /// Unlike the `lost_at` stamps above, `killed_at` must be a REAL clock
    /// value: the kill is remembered in memory and aged out against
    /// `now_unix()`, so a 1970 stamp is pruned before it can refuse anything.
    fn create_kill_and_reap(store: &mut Store, name: &'static str, killed_at: i64) -> i64 {
        let live = vec![live_unbound(name)];
        let keep = vec![name.to_string()];
        store
            .apply_host_reconcile(HostReconcile {
                probe_started_at: killed_at - 100,
                sessions: &live,
                keep: &keep,
                ..empty_probe("alpha", killed_at - 100)
            })
            .unwrap();
        let id = store.get_session(name, "alpha").unwrap().unwrap().id;
        store
            .mark_session_killed(id, killed_at)
            .unwrap()
            .expect("ghosted");
        // The kill's reconcile: the row was already ghost, so Phase 2
        // hard-deletes it in this very pass.
        store
            .apply_host_reconcile(HostReconcile {
                probe_started_at: killed_at + 1,
                keep: &[],
                ..empty_probe("alpha", killed_at + 1)
            })
            .unwrap();
        assert!(
            store.get_session_by_id(id).unwrap().is_none(),
            "precondition: the killed row is reaped in the kill's own pass"
        );
        id
    }

    /// One full-fleet pass over `alpha` that observed `name` live, as a probe
    /// that STARTED at `probe_started_at`; returns the names of the events it
    /// emitted.
    fn pass_observing(
        store: &mut Store,
        bus: &crate::events::RecordingEventBus,
        host: &str,
        name: &'static str,
        probe_started_at: i64,
    ) -> Vec<String> {
        bus.take();
        let live = vec![live_unbound(name)];
        store
            .apply_host_reconcile(HostReconcile {
                probe_started_at,
                sessions: &live,
                keep: &[name.to_string()],
                ..empty_probe(host, probe_started_at)
            })
            .unwrap();
        bus.take()
    }

    #[test]
    fn a_pass_older_than_the_kill_does_not_reinsert_a_reaped_session() {
        // #171: the full-fleet pass listed tmux BEFORE the kill, but only
        // writes once every host's probe has joined — by which time the
        // kill's own reconcile has already hard-deleted the row. Nothing
        // conflicts, so the unguarded INSERT brought the killed session back
        // as a brand-new `running` row with a `session:created` to match.
        let (mut store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        let killed_at = now_unix();
        create_kill_and_reap(&mut store, "s1", killed_at);

        let evts = pass_observing(&mut store, &bus, "alpha", "s1", killed_at - 50);

        assert!(
            store.get_session("s1", "alpha").unwrap().is_none(),
            "a pass that probed before the kill must not resurrect the session"
        );
        assert!(
            evts.iter().all(|e| e.starts_with("host:")),
            "nothing inserted ⇒ nothing to announce; got {evts:?}"
        );
    }

    #[test]
    fn a_pass_newer_than_the_kill_inserts_the_session_again() {
        // The other direction, and the one that matters for usability: a
        // session genuinely created under the freed tmux name probes after
        // the kill, so it must appear immediately.
        let (mut store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        let killed_at = now_unix();
        create_kill_and_reap(&mut store, "dev-o-r", killed_at);

        let evts = pass_observing(&mut store, &bus, "alpha", "dev-o-r", killed_at + 5);

        let row = store
            .get_session("dev-o-r", "alpha")
            .unwrap()
            .expect("the new session must be listed");
        assert_eq!(row.status, "running");
        assert_eq!(row.lost_at, None);
        // `sessions.id` has no AUTOINCREMENT, so the reaped row's id is fair
        // game — it is the `created` event, not the number, that says this
        // was an INSERT rather than a revived ghost.
        assert!(
            evts.contains(&format!("session:created:{}", row.id)),
            "the new session announces itself; got {evts:?}"
        );
    }

    #[test]
    fn a_pass_started_in_the_same_second_as_the_kill_does_not_reinsert() {
        // Same rule as the revive guard: both stamps are unix SECONDS off one
        // clock, so a probe that started within the killing second cannot be
        // shown to have seen the session after it died.
        let (mut store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        let killed_at = now_unix();
        create_kill_and_reap(&mut store, "s1", killed_at);

        pass_observing(&mut store, &bus, "alpha", "s1", killed_at);

        assert!(
            store.get_session("s1", "alpha").unwrap().is_none(),
            "same second is not newer"
        );
    }

    #[test]
    fn an_unknown_probe_time_does_not_reinsert_a_killed_session() {
        // `probe_started_at <= 0` is "probe time unknown" (see `ghost_cutoff`):
        // evidence that cannot be dated never outranks the kill.
        let (mut store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        create_kill_and_reap(&mut store, "s1", now_unix());

        pass_observing(&mut store, &bus, "alpha", "s1", 0);

        assert!(store.get_session("s1", "alpha").unwrap().is_none());
    }

    #[test]
    fn a_name_fleet_creates_again_is_admitted_in_the_killing_second() {
        // Fleet creating a tmux session under a name is proof the name is
        // alive again, so the create path forgets the kill before running its
        // own reconcile. Without that, a kill and a re-create landing in the
        // SAME second leave `new_session` with no row to return — it does not
        // insert one itself — and it fails with "vanished after creation"
        // while the tmux session really exists on the host.
        let (mut store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        let killed_at = now_unix();
        create_kill_and_reap(&mut store, "dev-o-r", killed_at);

        // What the create path does: tmux session created, kill forgotten,
        // then its own reconcile — whose probe can start in the killing
        // second on a local host.
        store.forget_kill("alpha", "dev-o-r");
        let evts = pass_observing(&mut store, &bus, "alpha", "dev-o-r", killed_at);

        let row = store
            .get_session("dev-o-r", "alpha")
            .unwrap()
            .expect("the re-created session must be listed");
        assert_eq!(row.status, "running");
        assert!(
            evts.contains(&format!("session:created:{}", row.id)),
            "and announce itself; got {evts:?}"
        );
    }

    #[test]
    fn a_rename_into_a_just_killed_name_is_admitted_in_the_killing_second() {
        // Same shape through `rename_session`: the name it renames INTO may
        // be one fleet killed a moment ago, and the rename's own reconcile
        // must be allowed to insert the row under the new name.
        let (mut store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        let killed_at = now_unix();
        create_kill_and_reap(&mut store, "dev-o-r", killed_at);

        store.forget_kill("alpha", "dev-o-r");
        pass_observing(&mut store, &bus, "alpha", "dev-o-r", killed_at);

        assert!(
            store.get_session("dev-o-r", "alpha").unwrap().is_some(),
            "the renamed session must be listed under its new name"
        );
    }

    #[test]
    fn a_kill_of_an_already_ghost_row_is_remembered_too() {
        // `kill_session` is reachable on a row reconcile has ALREADY ghosted
        // (the user kills the session that shows as lost). `tmux kill-session`
        // still runs, and the kill's own pass reaps the row immediately —
        // sooner than for a freshly ghosted one, since it was ghost before the
        // pass began. So the kill must be remembered even though the ghosting
        // UPDATE matched nothing.
        let (mut store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        let live = vec![live_unbound("s1")];
        store
            .apply_host_reconcile(HostReconcile {
                probe_started_at: 1,
                sessions: &live,
                keep: &["s1".to_string()],
                ..empty_probe("alpha", 1)
            })
            .unwrap();
        let id = store.get_session("s1", "alpha").unwrap().unwrap().id;
        // A pass that no longer sees it ghosts the row.
        store
            .apply_host_reconcile(HostReconcile {
                probe_started_at: 2,
                keep: &[],
                ..empty_probe("alpha", 2)
            })
            .unwrap();
        assert_eq!(
            store.get_session_by_id(id).unwrap().unwrap().status,
            "ghost",
            "precondition: the row is already a ghost when the user kills it"
        );

        let killed_at = now_unix();
        assert!(
            store.mark_session_killed(id, killed_at).unwrap().is_none(),
            "an already-ghost row reports nothing to announce"
        );
        // The kill's own pass: the row was ghost before it, so Phase 2 reaps.
        store
            .apply_host_reconcile(HostReconcile {
                probe_started_at: killed_at + 1,
                keep: &[],
                ..empty_probe("alpha", killed_at + 1)
            })
            .unwrap();
        assert!(store.get_session_by_id(id).unwrap().is_none());

        let evts = pass_observing(&mut store, &bus, "alpha", "s1", killed_at - 50);

        assert!(
            store.get_session("s1", "alpha").unwrap().is_none(),
            "a pass older than the kill must not resurrect it"
        );
        assert!(
            evts.iter().all(|e| e.starts_with("host:")),
            "nothing inserted ⇒ nothing to announce; got {evts:?}"
        );
    }

    #[test]
    fn a_kill_of_a_row_that_no_longer_exists_records_nothing() {
        // The other no-op branch: the id is gone entirely (a second kill
        // after the reap, a stale id from the UI). There is no host/name to
        // key a memory on, so nothing is recorded — and nothing panics.
        let store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();

        assert!(store
            .mark_session_killed(4242, now_unix())
            .unwrap()
            .is_none());

        assert!(
            store.recent_kills("alpha").is_empty(),
            "a kill of a row that does not exist has no name to remember"
        );
    }

    #[test]
    fn a_kill_older_than_the_memory_window_is_forgotten() {
        // The memory is bounded by age alone (no timer, no table): once a
        // kill is further back than any probe could still be in flight, its
        // entry is dropped on the next read — and an observation from before
        // it is admitted again. That is the trade the bound buys, and why
        // `KILL_MEMORY_SECS` is set well past the worst-case probe.
        let (mut store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        let killed_at = now_unix() - KILL_MEMORY_SECS - 10;
        create_kill_and_reap(&mut store, "s1", killed_at);
        assert!(
            store.recent_kills("alpha").is_empty(),
            "an aged-out kill must not be kept"
        );

        pass_observing(&mut store, &bus, "alpha", "s1", killed_at - 50);

        assert!(store.get_session("s1", "alpha").unwrap().is_some());
    }

    #[test]
    fn a_kill_on_one_host_does_not_block_the_same_name_on_another() {
        // Two hosts routinely carry identically-named sessions (`dev-o-r` in
        // the same repo on each): killing one must say nothing about the other.
        let (mut store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        store.upsert_host("beta").unwrap();
        let killed_at = now_unix();
        create_kill_and_reap(&mut store, "dev-o-r", killed_at);

        pass_observing(&mut store, &bus, "beta", "dev-o-r", killed_at - 50);

        assert!(
            store.get_session("dev-o-r", "beta").unwrap().is_some(),
            "beta's session is unaffected by alpha's kill"
        );
        assert!(store.get_session("dev-o-r", "alpha").unwrap().is_none());
    }

    /// The live row under `name`, observed by a pass that started at `at`.
    fn observed_row(
        store: &mut Store,
        bus: &crate::events::RecordingEventBus,
        name: &'static str,
        at: i64,
    ) -> SessionRow {
        pass_observing(store, bus, "alpha", name, at);
        store.get_session(name, "alpha").unwrap().expect("row")
    }

    #[test]
    fn a_renamed_session_keeps_its_row_participant_and_timeline() {
        // M0.2: reconcile keys rows on the tmux name, so a rename used to
        // insert a fresh row under the new name and reap the old one — the
        // session's id, participant (inbox, address) and timeline with it.
        let (mut store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        let t = now_unix();
        let before = observed_row(&mut store, &bus, "dev-old", t - 100);
        let participant = store.ensure_participant_for_session(before.id).unwrap();
        store
            .insert_session_event(before.id, "prompt_sent", Some("hi"))
            .unwrap();

        let renamed = store
            .rename_session_row("alpha", "dev-old", "dev-new", t)
            .unwrap()
            .expect("the row is carried over");
        assert_eq!(renamed.id, before.id);
        assert_eq!(renamed.tmux_name, "dev-new");

        // The rename's own reconcile now sees only the new name.
        let after = observed_row(&mut store, &bus, "dev-new", t + 1);
        assert_eq!(after.id, before.id, "same row, not a re-insert");
        assert_eq!(after.status, "running");
        assert!(store.get_session("dev-old", "alpha").unwrap().is_none());
        let p = store
            .participant_for_session(before.id)
            .unwrap()
            .expect("participant still bound");
        assert_eq!(p.id, participant);
        let events: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM session_events WHERE session_id=?1 AND kind='prompt_sent'",
                [before.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(events, 1, "the timeline survives the rename");
    }

    #[test]
    fn a_pass_that_listed_tmux_before_the_rename_neither_ghosts_nor_duplicates_it() {
        // A full pass whose probe started BEFORE the rename still lists the
        // old name and lacks the new one. It must not ghost the renamed row
        // (BE-3 via last_reconciled_at) nor insert the old name again (kill
        // memory).
        let (mut store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        let t = now_unix();
        let before = observed_row(&mut store, &bus, "dev-old", t - 100);
        store
            .rename_session_row("alpha", "dev-old", "dev-new", t)
            .unwrap()
            .expect("renamed");

        pass_observing(&mut store, &bus, "alpha", "dev-old", t - 50);

        assert!(
            store.get_session("dev-old", "alpha").unwrap().is_none(),
            "the stale pass must not resurrect the old name"
        );
        let row = store
            .get_session("dev-new", "alpha")
            .unwrap()
            .expect("kept");
        assert_eq!(row.id, before.id);
        assert_eq!(row.status, "running", "the stale pass must not ghost it");
    }

    #[test]
    fn renaming_onto_a_ghosts_name_replaces_the_ghost() {
        // tmux only lets a rename take a name no live session holds, so a row
        // still carrying it is a ghost; it gives way instead of tripping the
        // (host_alias, tmux_name) unique key.
        let (mut store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        let t = now_unix();
        let keep = observed_row(&mut store, &bus, "dev-a", t - 100);
        let ghost = store
            .upsert_session("dev-b", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store.mark_session_killed(ghost, t - 50).unwrap();

        let renamed = store
            .rename_session_row("alpha", "dev-a", "dev-b", t)
            .unwrap()
            .expect("renamed");
        assert_eq!(renamed.id, keep.id);
        assert_eq!(
            store.get_session("dev-b", "alpha").unwrap().unwrap().id,
            keep.id
        );
    }

    #[test]
    fn renaming_onto_a_lost_sessions_name_is_refused_and_keeps_it() {
        // A `lost` row that can still be restored (host reboot survival) is
        // not a ghost to dismiss: tmux accepts the name (the lost session is
        // not in tmux), so the store must refuse, naming it — with its
        // timeline and participant intact, and the renamed row untouched.
        let (mut store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        let t = now_unix();
        let keep = observed_row(&mut store, &bus, "dev-a", t - 100);
        let lost = store
            .upsert_session("dev-b", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store.set_claude_session_id(lost, "c-lost").unwrap();
        store
            .conn
            .execute(
                "UPDATE sessions SET status='ghost', lost_at=?1, lost_reason='host_reboot' \
                 WHERE id=?2",
                rusqlite::params![t - 50, lost],
            )
            .unwrap();
        let participant = store.ensure_participant_for_session(lost).unwrap();
        store
            .insert_session_event(lost, "prompt_sent", Some("hi"))
            .unwrap();

        let err = store
            .rename_session_row("alpha", "dev-a", "dev-b", t)
            .unwrap_err();
        assert_eq!(err.code, crate::ipc_error::codes::E_EXISTS);
        // Multi-user M1 (T10): the lost row's id is NOT in the message — it
        // may be another person's row, and `dev-b` is the caller's own
        // argument. What the refusal must still say is which name is taken.
        assert!(
            err.message.contains("dev-b belongs to a lost session"),
            "{}",
            err.message
        );
        assert!(
            !err.message.contains(&format!("id {lost}")),
            "no row id: {}",
            err.message
        );
        assert_eq!(
            store.get_session("dev-a", "alpha").unwrap().unwrap().id,
            keep.id,
            "nothing was renamed"
        );
        let row = store
            .get_session_by_id(lost)
            .unwrap()
            .expect("the lost row survives");
        assert_eq!(
            (row.tmux_name.as_str(), row.status.as_str()),
            ("dev-b", "ghost")
        );
        assert_eq!(
            store.participant_for_session(lost).unwrap().map(|p| p.id),
            Some(participant)
        );
        let events: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM session_events WHERE session_id=?1",
                [lost],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(events, 1, "its timeline survives");
        // Dismissed (no conversation to resume), it is a ghost and gives way.
        store
            .conn
            .execute(
                "UPDATE sessions SET claude_session_id = NULL WHERE id=?1",
                [lost],
            )
            .unwrap();
        let renamed = store
            .rename_session_row("alpha", "dev-a", "dev-b", t)
            .unwrap()
            .expect("renamed");
        assert_eq!(renamed.id, keep.id);
        assert!(store.get_session_by_id(lost).unwrap().is_none());
    }

    #[test]
    fn renaming_a_name_with_no_row_is_a_no_op() {
        let (store, _bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        assert!(store
            .rename_session_row("alpha", "nobody", "somebody", now_unix())
            .unwrap()
            .is_none());
        assert!(store.get_session("somebody", "alpha").unwrap().is_none());
    }
}
