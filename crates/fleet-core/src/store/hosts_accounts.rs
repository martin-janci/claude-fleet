//! Hosts, accounts and per-host MCP tokens.

use super::*;
use crate::ipc_error::codes;

/// Whether `last_seen_at` moved enough to be worth a database write: newly
/// known, cleared, or more than 10 minutes later (or earlier) than the
/// stored value. See `Store::upsert_account`.
const LAST_SEEN_MATERIAL_DELTA_SECS: i64 = 600;

fn last_seen_moved_materially(prior: Option<i64>, incoming: Option<i64>) -> bool {
    match (prior, incoming) {
        (None, None) => false,
        (None, Some(_)) | (Some(_), None) => true,
        (Some(p), Some(n)) => (n - p).abs() >= LAST_SEEN_MATERIAL_DELTA_SECS,
    }
}

impl Store {
    // ---- host_tokens (migration 018) ----

    /// Every per-host token row, alias-ordered. The MCP auth layer loads
    /// this per request and compares in constant time, so a token never
    /// runs through a SQL string comparison.
    pub fn list_host_tokens(&self) -> Result<Vec<HostTokenRow>, crate::ipc_error::IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT host_alias, token, created_at, mode FROM host_tokens ORDER BY host_alias",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(HostTokenRow {
                host_alias: row.get(0)?,
                token: row.get(1)?,
                created_at: row.get(2)?,
                mode: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The token row for one host, if it has been provisioned.
    pub fn get_host_token(
        &self,
        host_alias: &str,
    ) -> Result<Option<HostTokenRow>, crate::ipc_error::IpcError> {
        Ok(self
            .list_host_tokens()?
            .into_iter()
            .find(|r| r.host_alias == host_alias))
    }

    /// Insert or replace a host's token, keeping its mode when the row
    /// already exists. `created_at` is stamped now.
    pub fn upsert_host_token(
        &self,
        host_alias: &str,
        token: &str,
    ) -> Result<(), crate::ipc_error::IpcError> {
        let at = now_unix();
        self.conn.execute(
            "INSERT INTO host_tokens (host_alias, token, created_at, mode) \
                 VALUES (?1, ?2, ?3, 'full') \
                 ON CONFLICT(host_alias) DO UPDATE SET \
                   token = excluded.token, created_at = excluded.created_at",
            rusqlite::params![host_alias, token, at],
        )?;
        Ok(())
    }

    /// Set a host's token mode (`full` | `readonly`). `E_NOTFOUND` when the
    /// host has no token yet (it must be provisioned first).
    pub fn set_host_token_mode(
        &self,
        host_alias: &str,
        mode: &str,
    ) -> Result<(), crate::ipc_error::IpcError> {
        let n = self.conn.execute(
            "UPDATE host_tokens SET mode = ?2 WHERE host_alias = ?1",
            rusqlite::params![host_alias, mode],
        )?;
        if n == 0 {
            return Err(crate::ipc_error::IpcError::new(
                codes::E_NOTFOUND,
                format!("host {host_alias} has no control-API token (provision it first)"),
            ));
        }
        Ok(())
    }

    /// Re-read `alias` after a write and announce it through `emit`
    /// (`EventBus::host_added` or `EventBus::host_probed`); nothing when the
    /// row is gone.
    fn emit_host(
        &self,
        alias: &str,
        emit: fn(&dyn EventBus, &HostRow),
    ) -> Result<(), rusqlite::Error> {
        if let Some(row) = fetch_host(&self.conn, alias)? {
            emit(self.bus.as_ref(), &row);
        }
        Ok(())
    }

    pub fn upsert_host(&self, alias: &str) -> Result<(), rusqlite::Error> {
        // Check existence first so `host_added` fires only on a genuine
        // insert. Reconcile calls this for `local` every run; without the
        // check it emitted a spurious host:added (plus a get_host fetch)
        // on every window focus.
        let existed = self
            .conn
            .query_row("SELECT 1 FROM hosts WHERE alias=?1", [alias], |_| Ok(()))
            .optional()?
            .is_some();
        self.conn.execute(
            "INSERT INTO hosts (alias, reachable) VALUES (?1, 1)
             ON CONFLICT(alias) DO UPDATE SET reachable=1",
            rusqlite::params![alias],
        )?;
        if !existed {
            self.emit_host(alias, |bus, row| bus.host_added(row))?;
        }
        Ok(())
    }

    pub fn list_hosts(&self) -> Result<Vec<HostRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {HOST_COLUMNS} FROM hosts ORDER BY (alias='local') DESC, alias ASC"
        ))?;
        let rows = stmt.query_map([], map_host_row)?;
        rows.collect()
    }

    /// The fleet alias of the **agent** host addressed by `key`, or `None` if
    /// `key` names no agent host (including an alias with no row at all).
    ///
    /// `key` is whatever the service layer handed the transport as its `host`
    /// argument. That is the fleet alias everywhere except `service::hosts`,
    /// which probes a host by its `ssh_alias`, so both columns are matched —
    /// and the *fleet alias* is what comes back, because that is the name an
    /// agent registers under.
    ///
    /// A `key` that is some host's fleet alias names THAT host, whatever its
    /// transport: an SSH host whose alias happens to be an agent host's
    /// `ssh_alias` stays on SSH, and its commands never run on the agent's
    /// machine. Only a key that is no host's alias is matched against the
    /// `ssh_alias` column, and only when it is unambiguous across the WHOLE
    /// table — one claimant, on the agent transport. Two hosts claiming one
    /// `ssh_alias` route to neither, and the count deliberately includes SSH
    /// hosts: were it agent rows only, `mefistos` (ssh) and `laptop` (agent)
    /// both claiming `box.example` would leave a single agent claimant, and
    /// `probe_host(mefistos)` — which addresses a host by the `ssh_alias` on
    /// its row — would run on the laptop and stamp the laptop's versions onto
    /// `mefistos`.
    pub fn agent_host_alias(&self, key: &str) -> Result<Option<String>, rusqlite::Error> {
        let exact: Option<String> = self
            .conn
            .prepare_cached("SELECT transport FROM hosts WHERE alias=?1")?
            .query_row([key], |r| r.get(0))
            .optional()?;
        if let Some(transport) = exact {
            return Ok((transport == "agent").then(|| key.to_string()));
        }
        let mut stmt = self
            .conn
            .prepare_cached("SELECT alias, transport FROM hosts WHERE ssh_alias=?1 LIMIT 2")?;
        let matches: Vec<(String, String)> = stmt
            .query_map([key], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        Ok(match matches.as_slice() {
            [(alias, transport)] if transport == "agent" => Some(alias.clone()),
            _ => None,
        })
    }

    pub fn insert_host(&self, alias: &str, ssh_alias: Option<&str>) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO hosts (alias, ssh_alias, reachable, hidden) VALUES (?1, ?2, 0, 0)
             ON CONFLICT(alias) DO UPDATE SET ssh_alias=excluded.ssh_alias",
            rusqlite::params![alias, ssh_alias],
        )?;
        self.emit_host(alias, |bus, row| bus.host_added(row))
    }

    pub fn update_host_probe(
        &self,
        alias: &str,
        reachable: bool,
        claude_version: Option<&str>,
        tmux_version: Option<&str>,
        last_pinged_at: i64,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE hosts SET reachable=?1, claude_version=?2, tmux_version=?3, last_pinged_at=?4 WHERE alias=?5",
            rusqlite::params![
                if reachable { 1 } else { 0 },
                claude_version,
                tmux_version,
                last_pinged_at,
                alias
            ],
        )?;
        self.emit_host(alias, |bus, row| bus.host_probed(row))
    }

    pub fn set_host_hidden(&self, alias: &str, hidden: bool) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE hosts SET hidden=?1 WHERE alias=?2",
            rusqlite::params![if hidden { 1 } else { 0 }, alias],
        )?;
        // Emit like every other host mutation — the HostRow carries `hidden`,
        // so subscribers see the toggle without a manual refetch.
        self.emit_host(alias, |bus, row| bus.host_probed(row))
    }

    pub fn list_accounts(&self) -> Result<Vec<AccountRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {ACCOUNT_COLUMNS} FROM accounts ORDER BY uuid ASC"
        ))?;
        let rows = stmt.query_map([], map_account_row)?;
        rows.collect()
    }

    /// Insert or refresh a probed account. `nickname` is user data and is
    /// NEVER written here — only [`Store::set_account_nickname`] changes it
    /// (a probe run every reconcile tick must not clobber it).
    ///
    /// No-op-free (see the W1 Track A pattern in `store/reconcile.rs`): the
    /// prior row is read first and compared against the incoming one on the
    /// fields a user can see (`email`, `display_name`, `organization_name`,
    /// `organization_uuid`, `seat_tier`, `has_extra_usage`). `sync_local_account`
    /// calls this every ~20s background tick for an account that essentially
    /// never changes, so without this diff every tick wrote the row and fired
    /// `account_upserted` — a store write and a frontend patch, forever, for
    /// nothing. `last_seen_at` moves every call (it's `now`), so it can't be
    /// diffed the same way: it is refreshed in the database only when it has
    /// moved materially (more than 10 minutes since the stored value), so the
    /// column still means "recently seen" without a write every tick. When
    /// NEITHER a visible field changed NOR `last_seen_at` moved materially,
    /// this is a complete no-op: no write, no event.
    pub fn upsert_account(&self, a: &AccountRow) -> Result<(), rusqlite::Error> {
        let prior = self.get_account_by_uuid(&a.uuid)?;
        let visible_changed = match &prior {
            None => true,
            Some(p) => {
                p.email != a.email
                    || p.display_name != a.display_name
                    || p.organization_name != a.organization_name
                    || p.organization_uuid != a.organization_uuid
                    || p.seat_tier != a.seat_tier
                    || p.has_extra_usage != a.has_extra_usage
            }
        };
        let seen_moved =
            last_seen_moved_materially(prior.as_ref().and_then(|p| p.last_seen_at), a.last_seen_at);
        if !visible_changed && !seen_moved {
            return Ok(());
        }
        let last_seen_at = if seen_moved {
            a.last_seen_at
        } else {
            prior.as_ref().and_then(|p| p.last_seen_at)
        };
        self.conn.execute(
            "INSERT INTO accounts (uuid, email, display_name, organization_name,
                                   organization_uuid, seat_tier, last_seen_at, has_extra_usage)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(uuid) DO UPDATE SET
               email=excluded.email,
               display_name=excluded.display_name,
               organization_name=excluded.organization_name,
               organization_uuid=excluded.organization_uuid,
               seat_tier=excluded.seat_tier,
               last_seen_at=excluded.last_seen_at,
               has_extra_usage=excluded.has_extra_usage",
            rusqlite::params![
                a.uuid,
                a.email,
                a.display_name,
                a.organization_name,
                a.organization_uuid,
                a.seat_tier,
                last_seen_at,
                a.has_extra_usage
            ],
        )?;
        if visible_changed {
            if let Some(row) = self.get_account_by_uuid(&a.uuid)? {
                self.bus.account_upserted(&row);
            }
        }
        Ok(())
    }

    pub(super) fn get_account_by_uuid(
        &self,
        uuid: &str,
    ) -> Result<Option<AccountRow>, rusqlite::Error> {
        self.conn
            .prepare_cached(&format!(
                "SELECT {ACCOUNT_COLUMNS} FROM accounts WHERE uuid=?1"
            ))?
            .query_row(rusqlite::params![uuid], map_account_row)
            .optional()
    }

    /// Set (clear, if `nickname` is `None` or blank after trimming) an
    /// account's nickname (migration 028). Always emits `account_upserted`
    /// with the updated row — unlike `upsert_account`, this is a deliberate
    /// user edit, never a background no-op. `E_NOTFOUND` when the uuid is
    /// unknown; `E_INVALID` when the trimmed nickname exceeds 32 characters.
    pub fn set_account_nickname(
        &self,
        uuid: &str,
        nickname: Option<&str>,
    ) -> Result<AccountRow, crate::ipc_error::IpcError> {
        let trimmed = nickname.map(str::trim).filter(|n| !n.is_empty());
        if let Some(n) = trimmed {
            if n.chars().count() > 32 {
                return Err(crate::ipc_error::IpcError::new(
                    codes::E_INVALID,
                    "nickname must be 32 characters or fewer",
                ));
            }
        }
        let n = self.conn.execute(
            "UPDATE accounts SET nickname = ?1 WHERE uuid = ?2",
            rusqlite::params![trimmed, uuid],
        )?;
        if n == 0 {
            return Err(crate::ipc_error::IpcError::new(
                codes::E_NOTFOUND,
                format!("account {uuid} not found"),
            ));
        }
        let row = self.get_account_by_uuid(uuid)?.ok_or_else(|| {
            crate::ipc_error::IpcError::new(codes::E_INTERNAL, format!("account {uuid} vanished"))
        })?;
        self.bus.account_upserted(&row);
        Ok(row)
    }

    pub fn set_host_account(
        &self,
        alias: &str,
        account_uuid: Option<&str>,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE hosts SET account_uuid=?1 WHERE alias=?2",
            rusqlite::params![account_uuid, alias],
        )?;
        self.emit_host(alias, |bus, row| bus.host_probed(row))
    }

    /// Set a host's transport (migration 034): `"ssh"` (the default, reached
    /// over SSH as today) or `"agent"` (reached through an outbound
    /// fleet-agent connection). `E_INVALID` for any other value — an
    /// unvalidated write here would silently strand every future command
    /// against the host. `E_NOTFOUND` for an unknown alias (same
    /// affected-row-count check as `set_host_token_mode`) — unlike
    /// `set_host_hidden`/`set_host_account`/`set_host_provisioned` (which
    /// return a bare `rusqlite::Error` and can't express it), this setter
    /// already returns `IpcError`, so a typo'd alias gets a real signal
    /// instead of a silent no-op.
    pub fn set_host_transport(
        &self,
        alias: &str,
        transport: &str,
    ) -> Result<(), crate::ipc_error::IpcError> {
        if !HOST_TRANSPORTS.contains(&transport) {
            return Err(crate::ipc_error::IpcError::new(
                codes::E_INVALID,
                format!("unknown transport {transport:?}: must be \"ssh\" or \"agent\""),
            ));
        }
        let n = self.conn.execute(
            "UPDATE hosts SET transport=?1 WHERE alias=?2",
            rusqlite::params![transport, alias],
        )?;
        if n == 0 {
            return Err(crate::ipc_error::IpcError::new(
                codes::E_NOTFOUND,
                format!("host {alias} not found"),
            ));
        }
        self.emit_host(alias, |bus, row| bus.host_probed(row))?;
        Ok(())
    }

    pub fn set_host_provisioned(
        &self,
        alias: &str,
        provisioned: bool,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE hosts SET provisioned=?1 WHERE alias=?2",
            rusqlite::params![if provisioned { 1 } else { 0 }, alias],
        )?;
        Ok(())
    }

    pub fn delete_host(&self, alias: &str) -> Result<(), rusqlite::Error> {
        // The `local` host is never removed.
        if alias == "local" {
            return Ok(());
        }
        // Collect orphaned session ids first so we can emit a `session_killed`
        // event per row — otherwise frontend stores subscribed to session events
        // would carry stale rows that point to a host that no longer exists.
        let tx = self.conn.unchecked_transaction()?;
        let orphan_ids: Vec<i64> = {
            let mut stmt = tx.prepare_cached("SELECT id FROM sessions WHERE host_alias=?1")?;
            let ids = stmt
                .query_map(rusqlite::params![alias], |row| row.get::<_, i64>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            ids
        };
        // What dies with the sessions (as `delete_session` does): their
        // timeline and the messages addressed to them.
        tx.execute(
            "DELETE FROM session_events
              WHERE session_id IN (SELECT id FROM sessions WHERE host_alias=?1)",
            rusqlite::params![alias],
        )?;
        tx.execute(
            "DELETE FROM session_messages
              WHERE to_session_id IN (SELECT id FROM sessions WHERE host_alias=?1)",
            rusqlite::params![alias],
        )?;
        tx.execute(
            "DELETE FROM sessions WHERE host_alias=?1",
            rusqlite::params![alias],
        )?;
        // Its layer assignment (migration 033) is FK-enforced against
        // hosts(alias) like sessions, so it too must go before the hosts row.
        tx.execute(
            "DELETE FROM host_layers WHERE host_alias=?1",
            rusqlite::params![alias],
        )?;
        tx.execute("DELETE FROM hosts WHERE alias=?1", rusqlite::params![alias])?;
        // A removed host's control-API token must stop authenticating.
        tx.execute(
            "DELETE FROM host_tokens WHERE host_alias=?1",
            rusqlite::params![alias],
        )?;
        // Its recorded parent fingerprints (repair) go with it.
        tx.execute(
            "DELETE FROM worktree_parent_fingerprints WHERE host_alias=?1",
            rusqlite::params![alias],
        )?;
        // And its worktree rows (host-scoped, migration 024). A session on
        // another host that still points at one is cleared first, and told.
        const HOST_WORKTREES: &str =
            "worktree_id IN (SELECT id FROM worktrees WHERE host_alias=?1)";
        let cleared: Vec<i64> = {
            let mut stmt =
                tx.prepare(&format!("SELECT id FROM sessions WHERE {HOST_WORKTREES}"))?;
            let ids = stmt
                .query_map(rusqlite::params![alias], |row| row.get::<_, i64>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            ids
        };
        tx.execute(
            &format!("UPDATE sessions SET worktree_id = NULL WHERE {HOST_WORKTREES}"),
            rusqlite::params![alias],
        )?;
        tx.execute(
            "DELETE FROM worktrees WHERE host_alias=?1",
            rusqlite::params![alias],
        )?;
        // Its estimated-usage history goes with it (migration 025).
        tx.execute(
            "DELETE FROM usage_daily WHERE host_alias=?1",
            rusqlite::params![alias],
        )?;
        tx.commit()?;
        for id in &orphan_ids {
            self.bus.session_killed(*id);
        }
        self.emit_sessions_updated(&cleared);
        self.bus.host_removed(alias);
        Ok(())
    }

    pub fn get_host_row(&self, alias: &str) -> Result<Option<HostRow>, rusqlite::Error> {
        fetch_host(&self.conn, alias)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_support::*;

    #[test]
    fn host_tokens_upsert_keeps_mode_and_rotates_token() {
        let s = Store::open_in_memory().expect("open");
        assert!(s.list_host_tokens().unwrap().is_empty());
        assert!(s.get_host_token("mefistos").unwrap().is_none());
        // A mode change on an unprovisioned host is a typed error.
        assert_eq!(
            s.set_host_token_mode("mefistos", "readonly")
                .unwrap_err()
                .code,
            "E_NOTFOUND"
        );

        s.upsert_host_token("mefistos", "tok-1").unwrap();
        let row = s.get_host_token("mefistos").unwrap().unwrap();
        assert_eq!(row.token, "tok-1");
        assert_eq!(row.mode, "full", "new rows default to full");

        s.set_host_token_mode("mefistos", "readonly").unwrap();
        // Rotating the token must not reset the operator's mode choice.
        s.upsert_host_token("mefistos", "tok-2").unwrap();
        let row = s.get_host_token("mefistos").unwrap().unwrap();
        assert_eq!(row.token, "tok-2");
        assert_eq!(row.mode, "readonly");

        s.upsert_host_token("local", "tok-3").unwrap();
        let all = s.list_host_tokens().unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].host_alias, "local", "alias-ordered");
    }

    #[test]
    fn list_hosts_orders_local_first_then_alpha() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        s.insert_host("zebra", Some("zebra")).unwrap();
        s.insert_host("mefistos", Some("mefistos")).unwrap();
        let names: Vec<String> = s
            .list_hosts()
            .unwrap()
            .into_iter()
            .map(|h| h.alias)
            .collect();
        assert_eq!(names, vec!["local", "mefistos", "zebra"]);
    }

    /// The `ssh_alias` fallback answers `probe_host`, which addresses a host
    /// by the `ssh_alias` on its row. It may only answer when the key is
    /// unambiguous across the WHOLE table: one SSH host and one agent host
    /// claiming the same key is exactly as ambiguous as two agent hosts, and
    /// picking the agent would run the SSH host's probe on the agent's box.
    #[test]
    fn the_ssh_alias_fallback_answers_only_for_an_unshared_key() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("laptop", Some("box.example")).unwrap();
        s.set_host_transport("laptop", "agent").unwrap();
        assert_eq!(
            s.agent_host_alias("box.example").unwrap().as_deref(),
            Some("laptop"),
            "one claimant, and it is an agent host"
        );

        // A second claimant on ANY transport takes the key away again.
        s.insert_host("mefistos", Some("box.example")).unwrap();
        assert_eq!(s.agent_host_alias("box.example").unwrap(), None);
        // …while each host's own fleet alias still names it.
        assert_eq!(
            s.agent_host_alias("laptop").unwrap().as_deref(),
            Some("laptop")
        );
        assert_eq!(s.agent_host_alias("mefistos").unwrap(), None);
    }

    #[test]
    fn insert_host_records_ssh_alias() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("mefistos", Some("mefistos")).unwrap();
        let row = s
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|h| h.alias == "mefistos")
            .unwrap();
        assert_eq!(row.ssh_alias.as_deref(), Some("mefistos"));
        assert!(!row.reachable);
        assert!(!row.hidden);
    }

    #[test]
    fn update_host_probe_persists_versions_and_reachability() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h", Some("h")).unwrap();
        s.update_host_probe("h", true, Some("2.1.144"), Some("3.6a"), 1000)
            .unwrap();
        let row = s
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|x| x.alias == "h")
            .unwrap();
        assert!(row.reachable);
        assert_eq!(row.claude_version.as_deref(), Some("2.1.144"));
        assert_eq!(row.tmux_version.as_deref(), Some("3.6a"));
        assert_eq!(row.last_pinged_at, Some(1000));
    }

    #[test]
    fn delete_host_removes_host_and_its_sessions() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h", Some("h")).unwrap();
        s.upsert_session("dev-a", "h", None, None, 1, 1, "running", None)
            .unwrap();
        assert_eq!(s.list_sessions_for_host("h").unwrap().len(), 1);
        s.delete_host("h").unwrap();
        assert_eq!(
            s.list_hosts()
                .unwrap()
                .iter()
                .filter(|x| x.alias == "h")
                .count(),
            0
        );
        assert_eq!(s.list_sessions_for_host("h").unwrap().len(), 0);
    }

    #[test]
    fn delete_host_reaps_timeline_inbox_and_fingerprints_in_one_go() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h", Some("h")).unwrap();
        s.upsert_host("local").unwrap();
        let gone = s
            .upsert_session("dev-a", "h", None, None, 1, 1, "running", None)
            .unwrap();
        let peer = s
            .upsert_session("peer", "local", None, None, 1, 1, "running", None)
            .unwrap();
        s.insert_session_event(gone, "status_change", Some("idle"))
            .unwrap();
        s.insert_session_event(peer, "status_change", Some("idle"))
            .unwrap();
        s.insert_message(peer, gone, "to the gone", "message", None)
            .unwrap();
        s.insert_message(gone, peer, "from the gone", "message", None)
            .unwrap();
        s.record_parent_fingerprint("h", "/h/r/w", "1:2", 1)
            .unwrap();
        s.record_parent_fingerprint("local", "/l/r/w", "3:4", 1)
            .unwrap();

        s.delete_host("h").unwrap();

        assert!(s.list_session_events(gone, 10).unwrap().is_empty());
        assert!(s.list_inbox(gone, false, 10).unwrap().is_empty());
        assert_eq!(s.list_session_events(peer, 10).unwrap().len(), 1);
        assert_eq!(
            s.list_inbox(peer, false, 10).unwrap().len(),
            1,
            "a message the gone session SENT stays in the recipient's inbox"
        );
        assert_eq!(s.parent_fingerprint("h", "/h/r/w").unwrap(), None);
        assert_eq!(
            s.parent_fingerprint("local", "/l/r/w").unwrap().as_deref(),
            Some("3:4")
        );
    }

    #[test]
    fn delete_host_purges_its_usage_history_only() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h", Some("h")).unwrap();
        s.insert_host("k", Some("k")).unwrap();
        for host in ["h", "k"] {
            let id = s
                .upsert_session("dev-a", host, None, None, 1, 1, "running", None)
                .unwrap();
            s.apply_usage(
                id,
                host,
                &UsageDelta {
                    reset: false,
                    totals: UsageTotals {
                        input_tokens: 5,
                        cost_micros: 25,
                        ..Default::default()
                    },
                    model: None,
                    offset: 1,
                    source: "x.jsonl".into(),
                    last_msg_id: None,
                    last_msg_usage: None,
                    now: 86_400,
                },
            )
            .unwrap();
        }
        assert_eq!(s.usage_daily_since(0, None).unwrap().len(), 2);
        s.delete_host("h").unwrap();
        let left = s.usage_daily_since(0, None).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].1, "k");
    }

    /// `host_layers.host_alias` is FK-enforced against `hosts(alias)`
    /// (migration 033) with no `ON DELETE` clause, so a host carrying a
    /// layer assignment must have its `host_layers` rows cleaned up before
    /// the `hosts` row goes, or `DELETE FROM hosts` trips the constraint.
    #[test]
    fn delete_host_removes_its_layer_assignment() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h", Some("h")).unwrap();
        s.set_host_layers("h", Some("workstation"), &["papayapos"])
            .unwrap();
        assert_eq!(s.get_host_layers("h").unwrap().len(), 2);

        s.delete_host("h").unwrap();

        assert_eq!(
            s.list_hosts()
                .unwrap()
                .iter()
                .filter(|x| x.alias == "h")
                .count(),
            0
        );
        assert!(s.get_host_layers("h").unwrap().is_empty());
        assert!(
            s.list_all_host_layers().unwrap().is_empty(),
            "no host_layers row should survive the host it belonged to"
        );
    }

    #[test]
    fn delete_host_refuses_to_remove_local() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        s.delete_host("local").unwrap();
        assert!(s.list_hosts().unwrap().iter().any(|h| h.alias == "local"));
    }

    #[test]
    fn set_host_hidden_toggles() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h", Some("h")).unwrap();
        s.set_host_hidden("h", true).unwrap();
        assert!(
            s.list_hosts()
                .unwrap()
                .iter()
                .find(|x| x.alias == "h")
                .unwrap()
                .hidden
        );
        s.set_host_hidden("h", false).unwrap();
        assert!(
            !s.list_hosts()
                .unwrap()
                .iter()
                .find(|x| x.alias == "h")
                .unwrap()
                .hidden
        );
    }

    #[test]
    fn fresh_host_row_defaults_to_ssh_transport() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        assert_eq!(s.get_host_row("local").unwrap().unwrap().transport, "ssh");
        s.insert_host("h", Some("h")).unwrap();
        assert_eq!(s.get_host_row("h").unwrap().unwrap().transport, "ssh");
    }

    #[test]
    fn set_host_transport_round_trips_and_rejects_unknown_values() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h", Some("h")).unwrap();
        s.set_host_transport("h", "agent").unwrap();
        assert_eq!(s.get_host_row("h").unwrap().unwrap().transport, "agent");
        s.set_host_transport("h", "ssh").unwrap();
        assert_eq!(s.get_host_row("h").unwrap().unwrap().transport, "ssh");

        let err = s.set_host_transport("h", "carrier-pigeon").unwrap_err();
        assert_eq!(err.code, "E_INVALID");
        // The rejected write left the prior value untouched.
        assert_eq!(s.get_host_row("h").unwrap().unwrap().transport, "ssh");
    }

    /// An unknown alias is `E_NOTFOUND`, not a silent no-op — the same
    /// affected-row-count check `set_host_token_mode` uses, so a typo'd
    /// alias gets a signal instead of a success that changed nothing.
    #[test]
    fn set_host_transport_rejects_an_unknown_alias() {
        let s = Store::open_in_memory().unwrap();
        let err = s.set_host_transport("nope", "agent").unwrap_err();
        assert_eq!(err.code, "E_NOTFOUND");
    }

    #[test]
    fn host_provisioned_defaults_false_and_round_trips() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        assert!(!s.get_host_row("local").unwrap().unwrap().provisioned);
        s.set_host_provisioned("local", true).unwrap();
        assert!(s.get_host_row("local").unwrap().unwrap().provisioned);
    }

    #[test]
    fn upsert_account_inserts_then_updates_keeping_uuid_pk() {
        let s = Store::open_in_memory().unwrap();
        let a = AccountRow {
            uuid: "uuid-1".into(),
            email: Some("a@b.com".into()),
            display_name: Some("A".into()),
            seat_tier: Some("max".into()),
            last_seen_at: Some(1000),
            ..Default::default()
        };
        s.upsert_account(&a).unwrap();
        let mut a2 = a.clone();
        a2.email = Some("a@c.com".into());
        a2.last_seen_at = Some(2000);
        s.upsert_account(&a2).unwrap();
        let listed = s.list_accounts().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].uuid, "uuid-1");
        assert_eq!(listed[0].email.as_deref(), Some("a@c.com"));
        assert_eq!(listed[0].last_seen_at, Some(2000));
    }

    #[test]
    fn list_accounts_orders_by_uuid_ascending() {
        let s = Store::open_in_memory().unwrap();
        for uuid in ["zzz", "aaa", "mmm"] {
            s.upsert_account(&AccountRow {
                uuid: uuid.into(),
                ..Default::default()
            })
            .unwrap();
        }
        let listed = s.list_accounts().unwrap();
        assert_eq!(
            listed.iter().map(|a| a.uuid.as_str()).collect::<Vec<_>>(),
            vec!["aaa", "mmm", "zzz"]
        );
    }

    #[test]
    fn get_account_by_uuid_returns_none_when_missing() {
        let s = Store::open_in_memory().unwrap();
        assert!(s.get_account_by_uuid("nope").unwrap().is_none());
    }

    #[test]
    fn get_account_by_uuid_returns_some_when_present() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_account(&AccountRow {
            uuid: "u1".into(),
            email: Some("x@y.com".into()),
            ..Default::default()
        })
        .unwrap();
        let got = s.get_account_by_uuid("u1").unwrap().unwrap();
        assert_eq!(got.email.as_deref(), Some("x@y.com"));
    }

    #[test]
    fn set_host_account_assigns_and_clears() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h", Some("h")).unwrap();
        s.upsert_account(&AccountRow {
            uuid: "u1".into(),
            ..Default::default()
        })
        .unwrap();
        s.set_host_account("h", Some("u1")).unwrap();
        let row = s
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|r| r.alias == "h")
            .unwrap();
        assert_eq!(row.account_uuid.as_deref(), Some("u1"));
        s.set_host_account("h", None).unwrap();
        let row = s
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|r| r.alias == "h")
            .unwrap();
        assert!(row.account_uuid.is_none());
    }

    #[test]
    fn list_hosts_includes_account_uuid_in_output() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h", Some("h")).unwrap();
        let row = s
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|r| r.alias == "h")
            .unwrap();
        assert!(row.account_uuid.is_none());
    }

    #[test]
    fn delete_host_emits_session_killed_per_orphaned_session() {
        let (store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        store
            .upsert_session("s1", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store
            .upsert_session("s2", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        bus.take(); // drain host:added + 2x session:created
        store.delete_host("alpha").unwrap();
        let evts = bus.take();
        // Expected order: 2x session:killed (one per orphan), then host:removed.
        assert_eq!(
            evts.len(),
            3,
            "expected 2 session:killed + 1 host:removed, got {evts:?}"
        );
        assert!(evts[0].starts_with("session:killed:"), "got: {}", evts[0]);
        assert!(evts[1].starts_with("session:killed:"), "got: {}", evts[1]);
        assert_eq!(evts[2], "host:removed:alpha");
    }

    /// A removed host's worktree rows go with it: local rows and other hosts'
    /// rows stay, and a session elsewhere that pointed at a removed row is
    /// cleared (and told) rather than left dangling.
    #[test]
    fn delete_host_drops_only_its_own_worktree_rows() {
        let bus = Arc::new(crate::events::RecordingEventBus::new());
        let s = Store::open_with_bus_in_memory(bus.clone()).unwrap();
        s.upsert_host("vps").unwrap();
        s.upsert_host("local").unwrap();
        let pid = s.upsert_project("o", "r", "/p/r").unwrap();
        let local = s
            .upsert_worktree(pid, "feat", "/p/r/.worktrees/feat", None)
            .unwrap();
        let remote = s
            .upsert_worktree_on("vps", pid, "feat", "/srv/r/.worktrees/feat", None)
            .unwrap();
        let elsewhere = s
            .upsert_session(
                "dev",
                "local",
                Some(pid),
                Some(remote),
                1,
                1,
                "running",
                None,
            )
            .unwrap();
        bus.take();
        s.delete_host("vps").unwrap();
        assert!(s.get_worktree_row(remote).unwrap().is_none());
        assert!(s.get_worktree_row(local).unwrap().is_some());
        assert_eq!(
            s.get_session_by_id(elsewhere).unwrap().unwrap().worktree_id,
            None
        );
        assert!(
            bus.take().contains(&format!("session:updated:{elsewhere}")),
            "the cleared session is announced"
        );
    }

    // ── upsert_account: no-op-free (W1 Track A pattern) + nickname ──

    fn account(uuid: &str, last_seen_at: Option<i64>) -> AccountRow {
        AccountRow {
            uuid: uuid.into(),
            email: Some("a@b.com".into()),
            last_seen_at,
            ..Default::default()
        }
    }

    #[test]
    fn upsert_account_two_identical_upserts_emit_exactly_one_event() {
        let (s, bus) = store_with_recorder();
        let a = account("u1", Some(1_000));
        s.upsert_account(&a).unwrap();
        s.upsert_account(&a).unwrap();
        let evts = bus.take();
        assert_eq!(
            evts,
            vec!["account:upserted:u1".to_string()],
            "the second, identical upsert emits nothing"
        );
    }

    #[test]
    fn upsert_account_last_seen_at_drift_alone_emits_nothing_and_does_not_write() {
        let (s, bus) = store_with_recorder();
        s.upsert_account(&account("u1", Some(1_000))).unwrap();
        bus.take();
        // Same visible fields, `last_seen_at` a few seconds later — well under
        // the 10-minute material-change floor.
        s.upsert_account(&account("u1", Some(1_010))).unwrap();
        assert!(
            bus.take().is_empty(),
            "a few seconds of last_seen_at drift is not worth an event"
        );
        let row = s.get_account_by_uuid("u1").unwrap().unwrap();
        assert_eq!(
            row.last_seen_at,
            Some(1_000),
            "the stored last_seen_at was not written either"
        );
    }

    #[test]
    fn upsert_account_last_seen_at_moving_materially_writes_but_does_not_emit() {
        let (s, bus) = store_with_recorder();
        s.upsert_account(&account("u1", Some(1_000))).unwrap();
        bus.take();
        // Same visible fields, last_seen_at moved by more than 10 minutes.
        s.upsert_account(&account("u1", Some(1_000 + 601))).unwrap();
        assert!(
            bus.take().is_empty(),
            "last_seen_at alone is not user-visible; no event"
        );
        let row = s.get_account_by_uuid("u1").unwrap().unwrap();
        assert_eq!(
            row.last_seen_at,
            Some(1_000 + 601),
            "but the material move IS written"
        );
    }

    #[test]
    fn upsert_account_changed_email_emits_once() {
        let (s, bus) = store_with_recorder();
        s.upsert_account(&account("u1", Some(1_000))).unwrap();
        bus.take();
        let mut changed = account("u1", Some(1_000));
        changed.email = Some("new@b.com".into());
        s.upsert_account(&changed).unwrap();
        assert_eq!(bus.take(), vec!["account:upserted:u1".to_string()]);
        assert_eq!(
            s.get_account_by_uuid("u1")
                .unwrap()
                .unwrap()
                .email
                .as_deref(),
            Some("new@b.com")
        );
    }

    /// The nickname is user data: a probe upsert (what `upsert_account`
    /// always receives — probes never know about nicknames, see
    /// `service::hosts::account_row_from`) must never clobber an existing
    /// one, even when it also changes a visible field and so does write.
    #[test]
    fn upsert_account_probe_keeps_existing_nickname() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_account(&account("u1", Some(1_000))).unwrap();
        s.set_account_nickname("u1", Some("Home")).unwrap();
        let mut changed = account("u1", Some(2_000));
        changed.seat_tier = Some("max".into());
        s.upsert_account(&changed).unwrap();
        let row = s.get_account_by_uuid("u1").unwrap().unwrap();
        assert_eq!(row.nickname.as_deref(), Some("Home"));
        assert_eq!(row.seat_tier.as_deref(), Some("max"));
    }

    // ── set_account_nickname ──

    #[test]
    fn set_account_nickname_sets_clears_and_trims() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_account(&account("u1", Some(1_000))).unwrap();
        let row = s.set_account_nickname("u1", Some("  Home  ")).unwrap();
        assert_eq!(row.nickname.as_deref(), Some("Home"), "trimmed");
        assert_eq!(
            s.get_account_by_uuid("u1")
                .unwrap()
                .unwrap()
                .nickname
                .as_deref(),
            Some("Home")
        );
        // Clear with an empty string.
        let row = s.set_account_nickname("u1", Some("")).unwrap();
        assert_eq!(row.nickname, None);
        s.set_account_nickname("u1", Some("Home")).unwrap();
        // Clear with whitespace only.
        let row = s.set_account_nickname("u1", Some("   ")).unwrap();
        assert_eq!(row.nickname, None, "whitespace-only clears");
        // Clear with None.
        s.set_account_nickname("u1", Some("Home")).unwrap();
        let row = s.set_account_nickname("u1", None).unwrap();
        assert_eq!(row.nickname, None);
    }

    #[test]
    fn set_account_nickname_rejects_too_long() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_account(&account("u1", Some(1_000))).unwrap();
        let too_long = "x".repeat(33);
        let err = s.set_account_nickname("u1", Some(&too_long)).unwrap_err();
        assert_eq!(err.code, "E_INVALID");
        assert_eq!(s.get_account_by_uuid("u1").unwrap().unwrap().nickname, None);
        // Exactly 32 is fine.
        let exactly_32 = "x".repeat(32);
        let row = s.set_account_nickname("u1", Some(&exactly_32)).unwrap();
        assert_eq!(row.nickname.as_deref(), Some(exactly_32.as_str()));
    }

    #[test]
    fn set_account_nickname_unknown_uuid_is_not_found() {
        let s = Store::open_in_memory().unwrap();
        let err = s.set_account_nickname("nope", Some("Home")).unwrap_err();
        assert_eq!(err.code, "E_NOTFOUND");
    }

    #[test]
    fn set_account_nickname_always_emits() {
        let (s, bus) = store_with_recorder();
        s.upsert_account(&account("u1", Some(1_000))).unwrap();
        bus.take();
        s.set_account_nickname("u1", Some("Home")).unwrap();
        assert_eq!(bus.take(), vec!["account:upserted:u1".to_string()]);
        // Setting the SAME nickname again still emits — a deliberate user
        // edit, not folded into upsert_account's no-op detection.
        s.set_account_nickname("u1", Some("Home")).unwrap();
        assert_eq!(bus.take(), vec!["account:upserted:u1".to_string()]);
    }
}
