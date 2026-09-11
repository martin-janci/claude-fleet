//! Hosts, accounts and per-host MCP tokens.

use super::*;

impl Store {
    // ---- host_tokens (migration 018) ----

    /// Every per-host token row, alias-ordered. The MCP auth layer loads
    /// this per request and compares in constant time, so a token never
    /// runs through a SQL string comparison.
    pub fn list_host_tokens(&self) -> Result<Vec<HostTokenRow>, crate::ipc_error::IpcError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT host_alias, token, created_at, mode FROM host_tokens ORDER BY host_alias",
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        let rows = stmt
            .query_map([], |row| {
                Ok(HostTokenRow {
                    host_alias: row.get(0)?,
                    token: row.get(1)?,
                    created_at: row.get(2)?,
                    mode: row.get(3)?,
                })
            })
            .map_err(crate::ipc_error::IpcError::from)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(crate::ipc_error::IpcError::from)?);
        }
        Ok(out)
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
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.conn
            .execute(
                "INSERT INTO host_tokens (host_alias, token, created_at, mode) \
                 VALUES (?1, ?2, ?3, 'full') \
                 ON CONFLICT(host_alias) DO UPDATE SET \
                   token = excluded.token, created_at = excluded.created_at",
                rusqlite::params![host_alias, token, at],
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        Ok(())
    }

    /// Set a host's token mode (`full` | `readonly`). `E_NOTFOUND` when the
    /// host has no token yet (it must be provisioned first).
    pub fn set_host_token_mode(
        &self,
        host_alias: &str,
        mode: &str,
    ) -> Result<(), crate::ipc_error::IpcError> {
        let n = self
            .conn
            .execute(
                "UPDATE host_tokens SET mode = ?2 WHERE host_alias = ?1",
                rusqlite::params![host_alias, mode],
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        if n == 0 {
            return Err(crate::ipc_error::IpcError::new(
                "E_NOTFOUND",
                format!("host {host_alias} has no control-API token (provision it first)"),
            ));
        }
        Ok(())
    }

    /// Drop a host's token (e.g. when the host is removed). Idempotent.
    pub fn delete_host_token(&self, host_alias: &str) -> Result<(), crate::ipc_error::IpcError> {
        self.conn
            .execute(
                "DELETE FROM host_tokens WHERE host_alias = ?1",
                rusqlite::params![host_alias],
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        Ok(())
    }

    fn get_host(&self, alias: &str) -> Result<Option<HostRow>, rusqlite::Error> {
        fetch_host(&self.conn, alias)
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
            if let Some(row) = self.get_host(alias)? {
                self.bus.host_added(&row);
            }
        }
        Ok(())
    }

    pub fn list_hosts(&self) -> Result<Vec<HostRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT alias, ssh_alias, reachable, claude_version, tmux_version, hidden,
                    last_pinged_at, account_uuid, provisioned
             FROM hosts
             ORDER BY (alias='local') DESC, alias ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(HostRow {
                alias: row.get(0)?,
                ssh_alias: row.get(1)?,
                reachable: row.get::<_, i64>(2)? != 0,
                claude_version: row.get(3)?,
                tmux_version: row.get(4)?,
                hidden: row.get::<_, i64>(5)? != 0,
                last_pinged_at: row.get(6)?,
                account_uuid: row.get(7)?,
                provisioned: row.get::<_, i64>(8)? != 0,
            })
        })?;
        rows.collect()
    }

    pub fn insert_host(&self, alias: &str, ssh_alias: Option<&str>) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO hosts (alias, ssh_alias, reachable, hidden) VALUES (?1, ?2, 0, 0)
             ON CONFLICT(alias) DO UPDATE SET ssh_alias=excluded.ssh_alias",
            rusqlite::params![alias, ssh_alias],
        )?;
        if let Some(row) = self.get_host(alias)? {
            self.bus.host_added(&row);
        }
        Ok(())
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
        if let Some(row) = self.get_host(alias)? {
            self.bus.host_probed(&row);
        }
        Ok(())
    }

    pub fn set_host_hidden(&self, alias: &str, hidden: bool) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE hosts SET hidden=?1 WHERE alias=?2",
            rusqlite::params![if hidden { 1 } else { 0 }, alias],
        )?;
        // Emit like every other host mutation — the HostRow carries `hidden`,
        // so subscribers see the toggle without a manual refetch.
        if let Some(row) = self.get_host(alias)? {
            self.bus.host_probed(&row);
        }
        Ok(())
    }

    pub fn list_accounts(&self) -> Result<Vec<AccountRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT uuid, email, display_name, organization_name, organization_uuid,
                    seat_tier, last_seen_at
             FROM accounts
             ORDER BY uuid ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(AccountRow {
                uuid: row.get(0)?,
                email: row.get(1)?,
                display_name: row.get(2)?,
                organization_name: row.get(3)?,
                organization_uuid: row.get(4)?,
                seat_tier: row.get(5)?,
                last_seen_at: row.get(6)?,
            })
        })?;
        rows.collect()
    }

    pub fn upsert_account(&self, a: &AccountRow) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO accounts (uuid, email, display_name, organization_name,
                                   organization_uuid, seat_tier, last_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(uuid) DO UPDATE SET
               email=excluded.email,
               display_name=excluded.display_name,
               organization_name=excluded.organization_name,
               organization_uuid=excluded.organization_uuid,
               seat_tier=excluded.seat_tier,
               last_seen_at=excluded.last_seen_at",
            rusqlite::params![
                a.uuid,
                a.email,
                a.display_name,
                a.organization_name,
                a.organization_uuid,
                a.seat_tier,
                a.last_seen_at
            ],
        )?;
        self.bus.account_upserted(a);
        Ok(())
    }

    pub fn get_account_by_uuid(&self, uuid: &str) -> Result<Option<AccountRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT uuid, email, display_name, organization_name, organization_uuid,
                    seat_tier, last_seen_at
             FROM accounts WHERE uuid=?1",
        )?;
        let mut rows = stmt.query_map(rusqlite::params![uuid], |row| {
            Ok(AccountRow {
                uuid: row.get(0)?,
                email: row.get(1)?,
                display_name: row.get(2)?,
                organization_name: row.get(3)?,
                organization_uuid: row.get(4)?,
                seat_tier: row.get(5)?,
                last_seen_at: row.get(6)?,
            })
        })?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
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
        if let Some(row) = self.get_host(alias)? {
            self.bus.host_probed(&row);
        }
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

        s.delete_host_token("mefistos").unwrap();
        s.delete_host_token("mefistos").unwrap(); // idempotent
        assert_eq!(s.list_host_tokens().unwrap().len(), 1);
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
            organization_name: None,
            organization_uuid: None,
            seat_tier: Some("max".into()),
            last_seen_at: Some(1000),
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
                email: None,
                display_name: None,
                organization_name: None,
                organization_uuid: None,
                seat_tier: None,
                last_seen_at: None,
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
            display_name: None,
            organization_name: None,
            organization_uuid: None,
            seat_tier: None,
            last_seen_at: None,
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
            email: None,
            display_name: None,
            organization_name: None,
            organization_uuid: None,
            seat_tier: None,
            last_seen_at: None,
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
}
