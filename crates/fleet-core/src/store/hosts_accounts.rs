//! Hosts, accounts and per-host MCP tokens.

use super::*;
use crate::ipc_error::codes;

/// Whether `last_seen_at` moved enough to be worth a database write: newly
/// known, cleared, or more than 10 minutes later (or earlier) than the
/// stored value. See `Store::upsert_account`.
const LAST_SEEN_MATERIAL_DELTA_SECS: i64 = 600;

/// How often a host's `last_hook_at` is rewritten: a hook within this many
/// seconds of the stored stamp leaves it alone. Its reader, the health
/// check's `hooks_silent`, works in hours; minute freshness is plenty.
pub const HOST_HOOK_STAMP_EVERY_SECS: i64 = 60;

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
            "SELECT host_alias, token, created_at, mode, last_used_at, rotated_at \
             FROM host_tokens ORDER BY host_alias",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(HostTokenRow {
                host_alias: row.get(0)?,
                token: row.get(1)?,
                created_at: row.get(2)?,
                mode: row.get(3)?,
                last_used_at: row.get(4)?,
                rotated_at: row.get(5)?,
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
    /// already exists. A new row's `created_at` is stamped now; replacing an
    /// existing row's token with a different one keeps `created_at` and
    /// stamps `rotated_at` instead (writing the same token back changes
    /// nothing), and `last_used_at` starts over, since the new token has
    /// not been used yet.
    pub fn upsert_host_token(
        &self,
        host_alias: &str,
        token: &str,
    ) -> Result<(), crate::ipc_error::IpcError> {
        self.upsert_host_token_at(host_alias, token, now_unix())
    }

    /// [`Self::upsert_host_token`] at `at` (unix seconds).
    pub fn upsert_host_token_at(
        &self,
        host_alias: &str,
        token: &str,
        at: i64,
    ) -> Result<(), crate::ipc_error::IpcError> {
        self.conn.execute(
            "INSERT INTO host_tokens (host_alias, token, created_at, mode) \
                 VALUES (?1, ?2, ?3, 'full') \
                 ON CONFLICT(host_alias) DO UPDATE SET \
                   rotated_at = excluded.created_at, last_used_at = NULL, \
                   token = excluded.token \
                 WHERE host_tokens.token IS NOT excluded.token",
            rusqlite::params![host_alias, token, at],
        )?;
        Ok(())
    }

    /// Stamp `last_used_at` on `host_alias`'s token at `now`, but only when
    /// the stored value is unset or more than 60 seconds stale, so a busy
    /// host writes at most once a minute. Liveness only: it does not move
    /// `auth_epoch` (migration 126).
    pub fn touch_host_token(
        &self,
        host_alias: &str,
        now: i64,
    ) -> Result<(), crate::ipc_error::IpcError> {
        self.conn.execute(
            "UPDATE host_tokens SET last_used_at = ?2 \
             WHERE host_alias = ?1 AND (last_used_at IS NULL OR last_used_at <= ?2 - 60)",
            rusqlite::params![host_alias, now],
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
            emit(&self.bus, &row);
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

    /// Every host on the agent transport, by alias.
    pub fn agent_host_aliases(&self) -> Result<Vec<String>, rusqlite::Error> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT alias FROM hosts WHERE transport = 'agent' ORDER BY alias")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect()
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
        super::reconcile::write_host_probe(
            &self.conn,
            alias,
            reachable,
            claude_version,
            tmux_version,
            last_pinged_at,
            None,
        )?;
        self.emit_host(alias, |bus, row| bus.host_probed(row))
    }

    /// Stamp when the host's versions were last read from the host itself
    /// (migration 076). Written by the reconcile pass only on a pass whose
    /// probe carried a `versions` section, and by `probe_host`; the
    /// versions themselves still travel through `update_host_probe`. No
    /// event: the same transaction's `update_host_probe_in_tx` announces
    /// the row (as a ping, carrying this stamp).
    pub fn set_host_versions_at(&self, alias: &str, at: i64) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE hosts SET claude_version_at = ?1 WHERE alias = ?2",
            rusqlite::params![at, alias],
        )?;
        Ok(())
    }

    /// Write one health sample (migration 077). No event: the reconcile
    /// transaction's `update_host_probe_in_tx` announces the row and puts
    /// this sample on the ping.
    pub fn set_host_health(
        &self,
        alias: &str,
        h: &crate::tmux::HostHealthSample,
        at: i64,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE hosts SET disk_home_free_kb = ?1, disk_home_total_kb = ?2, \
             disk_tmp_free_kb = ?3, load_1m = ?4, mem_avail_kb = ?5, uptime_secs = ?6, \
             health_at = ?7, auth_overrides = ?8, cpu_count = ?9, mem_total_kb = ?10, \
             boot_at = ?11, latency_ms = ?12, agents_on_path = ?14 WHERE alias = ?13",
            rusqlite::params![
                h.disk_home_free_kb,
                h.disk_home_total_kb,
                h.disk_tmp_free_kb,
                h.load_1m,
                h.mem_avail_kb,
                h.uptime_secs,
                at,
                h.auth_overrides
                    .as_ref()
                    .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "[]".into())),
                h.cpu_count,
                h.mem_total_kb,
                h.boot_at,
                h.latency_ms,
                alias,
                h.agents_on_path
                    .as_ref()
                    .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "[]".into())),
            ],
        )?;
        Ok(())
    }

    /// Record a worktree-size read (Orbit Fleet 4.6, migration 123). The
    /// stamp moves on every ask so a `du` that timed out waits the full
    /// interval before the next try; the size moves only on an answer.
    /// No event: the reconcile pass's ping carries it.
    pub fn set_host_worktree_size(
        &self,
        alias: &str,
        kb: Option<i64>,
        at: i64,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE hosts SET worktree_kb = COALESCE(?1, worktree_kb), worktree_at = ?2 \
             WHERE alias = ?3",
            rusqlite::params![kb, at, alias],
        )?;
        Ok(())
    }

    /// Stamp the last hook accepted from this host's own token (hosts F9).
    /// Silent, and throttled to one write per [`HOST_HOOK_STAMP_EVERY_SECS`]:
    /// hooks arrive several times per turn, and the only reader (the
    /// `hooks_silent` health check) compares against hours.
    pub fn set_host_last_hook_at(&self, alias: &str, at: i64) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE hosts SET last_hook_at = ?1 WHERE alias = ?2 \
             AND (last_hook_at IS NULL OR last_hook_at < ?1 - ?3)",
            rusqlite::params![at, alias, HOST_HOOK_STAMP_EVERY_SECS],
        )?;
        Ok(())
    }

    /// Hosts whose last probe succeeded — the `fleet_hosts_reachable` gauge,
    /// counted in SQL (equal to `health::summarize(..).hosts_reachable`).
    pub fn count_reachable_hosts(&self) -> Result<u32, rusqlite::Error> {
        self.conn
            .prepare_cached("SELECT COUNT(*) FROM hosts WHERE reachable != 0")?
            .query_row([], |r| r.get(0))
    }

    /// The fleet-agent version the host's hello reported (hosts F5).
    pub fn set_host_agent_version(
        &self,
        alias: &str,
        version: &str,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE hosts SET agent_version = ?1 WHERE alias = ?2 AND agent_version IS NOT ?1",
            rusqlite::params![version, alias],
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

    pub(crate) fn get_account_by_uuid(
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

    /// Record a host's login profiles (migration 114). Emits `host:probed`
    /// only when the list changed, so a steady host costs no event per pass.
    pub fn set_host_profiles(
        &self,
        alias: &str,
        profiles: &[crate::store::HostProfileRow],
    ) -> Result<(), rusqlite::Error> {
        let json = serde_json::to_string(profiles).unwrap_or_else(|_| "[]".into());
        let n = self.conn.execute(
            "UPDATE hosts SET claude_profiles = ?1 WHERE alias = ?2 AND claude_profiles IS NOT ?1",
            rusqlite::params![json, alias],
        )?;
        if n > 0 {
            self.emit_host(alias, |bus, row| bus.host_probed(row))?;
        }
        Ok(())
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

    /// Set which harnesses the asset catalog syncs on a host (multi-harness
    /// F3a, migration 089): `None` = auto, `Some` = exactly this list, stored
    /// as JSON. The list is stored as given —
    /// `service::catalog::harness_set::set_host_harnesses` validates and
    /// normalises it first. An unknown alias is `E_NOTFOUND`, as in
    /// `set_host_transport`.
    pub fn set_host_harnesses(
        &self,
        alias: &str,
        harnesses: Option<&[String]>,
    ) -> Result<(), crate::ipc_error::IpcError> {
        let json = match harnesses {
            Some(list) => Some(serde_json::to_string(list).map_err(|e| {
                crate::ipc_error::IpcError::new(
                    codes::E_SERIALIZE,
                    format!("encode harnesses: {e}"),
                )
            })?),
            None => None,
        };
        let n = self.conn.execute(
            "UPDATE hosts SET harnesses=?1 WHERE alias=?2",
            rusqlite::params![json, alias],
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

    /// Mark a host provisioned (or not). With `true` the content fingerprint
    /// this build ships and the time are recorded too (migration 078), so
    /// `HostRow::provision_stale` can compare on every later read; with
    /// `false` both are cleared.
    pub fn set_host_provisioned(
        &self,
        alias: &str,
        provisioned: bool,
    ) -> Result<(), rusqlite::Error> {
        if provisioned {
            self.conn.execute(
                "UPDATE hosts SET provisioned=1, provision_fingerprint=?1, provisioned_at=?2 \
                 WHERE alias=?3",
                rusqlite::params![crate::service::provision::fingerprint(), now_unix(), alias],
            )?;
        } else {
            self.conn.execute(
                "UPDATE hosts SET provisioned=0, provision_fingerprint=NULL, provisioned_at=NULL \
                 WHERE alias=?1",
                rusqlite::params![alias],
            )?;
        }
        Ok(())
    }

    /// Record how the last provisioning went, beyond "it ran".
    ///
    /// `warning: None` is a clean run: the fingerprint `set_host_provisioned`
    /// just stamped stands, and any previous warning is cleared.
    ///
    /// `warning: Some(_)` keeps the text, so it survives the call that
    /// produced it and can reach `fleet_health` and the host's Attention row.
    ///
    /// `owed_retry` is a SEPARATE question from whether there is a warning,
    /// and the two must not be conflated. It forgets the fingerprint, so
    /// `HostRow::provision_stale` (`provisioned && fingerprint != current`)
    /// turns true and `spawn_reprovision_stale` picks the host up again.
    /// Set it for a step that MIGHT succeed next time — a failed `ag`
    /// install. Do NOT set it for a warning about a persistent condition of
    /// the host itself, such as WSL needing mirrored networking: that recurs
    /// until the operator changes their WSL config, so retrying would leave
    /// the host forever stale and re-provisioned on every tick. Such a
    /// warning is still worth keeping — it just is not a reason to retry.
    ///
    /// `provisioned` / `provisioned_at` are left alone either way: the
    /// content WAS delivered.
    pub fn record_host_provision_outcome(
        &self,
        alias: &str,
        warning: Option<&str>,
        owed_retry: bool,
    ) -> Result<(), rusqlite::Error> {
        if owed_retry {
            self.conn.execute(
                "UPDATE hosts SET provision_fingerprint=NULL WHERE alias=?1",
                rusqlite::params![alias],
            )?;
        }
        self.conn.execute(
            "UPDATE hosts SET provision_warning=?1 WHERE alias=?2",
            rusqlite::params![warning, alias],
        )?;
        Ok(())
    }

    /// Re-home everything keyed on `from` under `into` in ONE transaction,
    /// then delete the `from` host row (data-sync F2/F5: the `local` → `mac`
    /// rename left 249 worktree rows and 6 duplicate agent rows on a hidden
    /// alias). Rules: a worktree, fingerprint, dismissal, layer or secret
    /// that `into` already has keeps `into`'s; `usage_daily` sums per day; a
    /// `from` session whose `claude_session_id` already exists under `into`
    /// is dropped (the newer alias observed the same agent), a `from`
    /// session whose `tmux_name` clashes is dropped too, the rest move.
    /// An asset-inventory row or org rule `into` already has keeps
    /// `into`'s; `into` takes `from`'s org only when it has none.
    pub fn merge_host_alias(
        &self,
        from: &str,
        into: &str,
    ) -> Result<MergeReport, crate::ipc_error::IpcError> {
        use crate::ipc_error::{codes, IpcError};
        if from == into {
            return Err(IpcError::new(
                codes::E_INVALID,
                "from and into are the same host",
            ));
        }
        for alias in [from, into] {
            if fetch_host(&self.conn, alias)?.is_none() {
                return Err(IpcError::new(
                    codes::E_NOTFOUND,
                    format!("host {alias} not found"),
                ));
            }
        }
        let tx = self.conn.unchecked_transaction()?;
        // Worktrees: move the ones `into` lacks; point sessions at the twin
        // for the ones it has, then drop the leftovers.
        let worktrees_moved = tx.execute(
            "UPDATE worktrees SET host_alias = ?2 WHERE host_alias = ?1 AND NOT EXISTS (
               SELECT 1 FROM worktrees w2 WHERE w2.host_alias = ?2
                  AND w2.project_id = worktrees.project_id AND w2.name = worktrees.name)",
            rusqlite::params![from, into],
        )?;
        tx.execute(
            "UPDATE sessions SET worktree_id = (
               SELECT w2.id FROM worktrees w1 JOIN worktrees w2
                 ON w2.host_alias = ?2 AND w2.project_id = w1.project_id AND w2.name = w1.name
               WHERE w1.id = sessions.worktree_id)
             WHERE worktree_id IN (SELECT id FROM worktrees WHERE host_alias = ?1)",
            rusqlite::params![from, into],
        )?;
        tx.execute("DELETE FROM worktrees WHERE host_alias = ?1", [from])?;
        for (table, cols) in [
            (
                "worktree_parent_fingerprints",
                "wt_path, parent_fp, recorded_at",
            ),
            ("dismissed_agents", "claude_session_id, dismissed_at"),
            (
                "host_layers",
                "catalog_id, layer_name, axis, position, active",
            ),
            ("host_catalogs", "catalog_id, admitted_at"),
            ("catalog_secrets_host", "name, value, updated_at"),
        ] {
            tx.execute(
                &format!(
                    "INSERT OR IGNORE INTO {table} (host_alias, {cols}) \
                     SELECT ?2, {cols} FROM {table} WHERE host_alias = ?1"
                ),
                rusqlite::params![from, into],
            )?;
            tx.execute(
                &format!("DELETE FROM {table} WHERE host_alias = ?1"),
                [from],
            )?;
        }
        // Keyed (day, host_alias, backfill) since plan D's migration 071: a
        // collected day and a first-cursor backfill day sum separately.
        let usage_days_merged = tx.execute(
            "INSERT INTO usage_daily (day, host_alias, backfill, input_tokens, output_tokens, \
               cache_write_tokens, cache_read_tokens, cost_micros)
             SELECT day, ?2, backfill, input_tokens, output_tokens, cache_write_tokens, \
               cache_read_tokens, cost_micros
               FROM usage_daily WHERE host_alias = ?1
             ON CONFLICT(day, host_alias, backfill) DO UPDATE SET
               input_tokens = input_tokens + excluded.input_tokens,
               output_tokens = output_tokens + excluded.output_tokens,
               cache_write_tokens = cache_write_tokens + excluded.cache_write_tokens,
               cache_read_tokens = cache_read_tokens + excluded.cache_read_tokens,
               cost_micros = cost_micros + excluded.cost_micros",
            rusqlite::params![from, into],
        )?;
        tx.execute("DELETE FROM usage_daily WHERE host_alias = ?1", [from])?;
        // Sessions: duplicates and name clashes go (events with them, the
        // inbox tombstoned, as `delete_host` does); the rest move.
        let dropped: Vec<i64> = {
            let mut stmt = tx.prepare(
                "SELECT id FROM sessions s WHERE s.host_alias = ?1 AND (
                   (s.claude_session_id IS NOT NULL AND EXISTS (
                      SELECT 1 FROM sessions t WHERE t.host_alias = ?2
                        AND t.claude_session_id = s.claude_session_id))
                   OR EXISTS (SELECT 1 FROM sessions t WHERE t.host_alias = ?2
                        AND t.tmux_name = s.tmux_name))",
            )?;
            let ids = stmt
                .query_map(rusqlite::params![from, into], |r| r.get::<_, i64>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            ids
        };
        // Their facts, while the rows are still there: `session:killed` is
        // fenced by them (`store::sessions::killed_payloads`).
        let dropped_killed = super::sessions::killed_payloads(&tx, &dropped)?;
        for id in &dropped {
            tx.execute("DELETE FROM session_events WHERE session_id = ?1", [id])?;
            tx.execute(
                "UPDATE participants SET retired_at = ?1, session_id = NULL, client_id = NULL \
                 WHERE session_id = ?2 AND retired_at IS NULL",
                rusqlite::params![now_unix(), id],
            )?;
            tx.execute("DELETE FROM sessions WHERE id = ?1", [id])?;
        }
        let moved: Vec<i64> = {
            let mut stmt = tx.prepare("SELECT id FROM sessions WHERE host_alias = ?1")?;
            let ids = stmt
                .query_map([from], |r| r.get::<_, i64>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            ids
        };
        tx.execute(
            "UPDATE sessions SET host_alias = ?2 WHERE host_alias = ?1",
            rusqlite::params![from, into],
        )?;
        tx.execute("DELETE FROM host_tokens WHERE host_alias = ?1", [from])?;
        // Routines follow the host to its new name (review r02).
        tx.execute(
            "UPDATE routines SET host_alias = ?2 WHERE host_alias = ?1",
            [from, into],
        )?;
        // Asset inventory (migrations 030/031): `into`'s own scan wins a
        // clash on (harness, kind, name); the rest move.
        tx.execute(
            "UPDATE OR IGNORE asset_inventory SET host_alias = ?2 WHERE host_alias = ?1",
            rusqlite::params![from, into],
        )?;
        tx.execute("DELETE FROM asset_inventory WHERE host_alias = ?1", [from])?;
        // Update state (migration 079) is keyed on `agent:<alias>`: `into`'s
        // own observed row and pins win a clash; the rest, and the
        // transition log, move.
        for table in ["update_observed", "update_desired", "update_events"] {
            tx.execute(
                &format!(
                    "UPDATE OR IGNORE {table} SET target = 'agent:' || ?2 \
                     WHERE target = 'agent:' || ?1"
                ),
                rusqlite::params![from, into],
            )?;
            tx.execute(
                &format!("DELETE FROM {table} WHERE target = 'agent:' || ?1"),
                [from],
            )?;
        }
        // Org rules (migration 050): a `from` rule identical to one `into`
        // already has goes (the same test `add_org_rule` refuses a
        // duplicate by, across orgs); the rest now name `into`.
        tx.execute(
            "DELETE FROM org_rules WHERE host_alias = ?1 AND EXISTS (
               SELECT 1 FROM org_rules r2 WHERE r2.host_alias = ?2
                  AND r2.owner IS org_rules.owner AND r2.repo IS org_rules.repo
                  AND r2.path_prefix IS org_rules.path_prefix)",
            rusqlite::params![from, into],
        )?;
        tx.execute(
            "UPDATE org_rules SET host_alias = ?2 WHERE host_alias = ?1",
            rusqlite::params![from, into],
        )?;
        // The host's org: an org `into` already has is kept.
        tx.execute(
            "UPDATE hosts SET org_id = (SELECT org_id FROM hosts WHERE alias = ?1) \
             WHERE alias = ?2 AND org_id IS NULL",
            rusqlite::params![from, into],
        )?;
        tx.execute("DELETE FROM hosts WHERE alias = ?1", [from])?;
        tx.commit()?;
        for killed in dropped_killed {
            self.bus.session_killed(killed);
        }
        self.emit_sessions_updated(&moved);
        self.bus.host_removed(from);
        self.emit_host(into, |bus, row| bus.host_probed(row))?;
        Ok(MergeReport {
            from: from.to_string(),
            into: into.to_string(),
            worktrees_moved,
            sessions_moved: moved.len(),
            sessions_dropped: dropped.len(),
            usage_days_merged,
        })
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
        // Their facts, read before the rows go (see `killed_payloads`).
        let orphan_killed = super::sessions::killed_payloads(&tx, &orphan_ids)?;
        // What dies with the sessions (as `delete_session` does): their
        // timeline.
        tx.execute(
            "DELETE FROM session_events
              WHERE session_id IN (SELECT id FROM sessions WHERE host_alias=?1)",
            rusqlite::params![alias],
        )?;
        // Tombstone the participant rather than deleting the messages
        // addressed to it, as `delete_session` does — removing a host must
        // not silently destroy an undelivered inbox.
        tx.execute(
            "UPDATE participants SET retired_at = ?1, session_id = NULL, client_id = NULL
              WHERE session_id IN (SELECT id FROM sessions WHERE host_alias=?2)
                AND retired_at IS NULL",
            rusqlite::params![now_unix(), alias],
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
        // Its agent's update state (migration 079, target `agent:<alias>`):
        // an observed row nothing will refresh, and a pin for a target that
        // is gone. The transition log is left to its retention.
        tx.execute(
            "DELETE FROM update_observed WHERE target = 'agent:' || ?1",
            rusqlite::params![alias],
        )?;
        tx.execute(
            "DELETE FROM update_desired WHERE target = 'agent:' || ?1",
            rusqlite::params![alias],
        )?;
        // A routine that starts its sessions here can no longer fire; it
        // stops with the reason (review r02).
        tx.execute(
            "UPDATE routines SET enabled = 0, paused_reason = ?2, next_run_at = NULL, \
               updated_at = ?3 WHERE host_alias = ?1 AND enabled = 1",
            rusqlite::params![alias, super::routines::PAUSED_HOST_REMOVED, now_unix()],
        )?;
        tx.commit()?;
        for killed in orphan_killed {
            self.bus.session_killed(killed);
        }
        self.emit_sessions_updated(&cleared);
        self.bus.host_removed(alias);
        Ok(())
    }

    pub fn get_host_row(&self, alias: &str) -> Result<Option<HostRow>, rusqlite::Error> {
        fetch_host(&self.conn, alias)
    }

    // ---- host boot identity (migration 036) ----

    /// The boot identity recorded by the last probe that could read it.
    /// An unknown host, or one never probed, is `StoredIdentity::default()`
    /// (both `None`) — "unknown", which never produces a mass-loss verdict.
    pub fn get_host_identity(&self, alias: &str) -> rusqlite::Result<StoredIdentity> {
        let row = self
            .conn
            .query_row(
                "SELECT boot_id, tmux_server_pid FROM hosts WHERE alias = ?1",
                rusqlite::params![alias],
                |r| {
                    Ok(StoredIdentity {
                        boot_id: r.get(0)?,
                        tmux_server_pid: r.get(1)?,
                    })
                },
            )
            .optional()?;
        Ok(row.unwrap_or_default())
    }

    /// Store a host's observed boot identity. Writes only when a value
    /// differs from the stored one: reconcile calls this on every pass of
    /// every reachable host, and the identity almost never moves, so an
    /// unconditional UPDATE was a write under the store lock for nothing.
    pub fn set_host_identity(
        &self,
        alias: &str,
        boot_id: Option<&str>,
        tmux_server_pid: Option<i64>,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE hosts SET boot_id = ?1, tmux_server_pid = ?2 WHERE alias = ?3 \
             AND (boot_id IS NOT ?1 OR tmux_server_pid IS NOT ?2)",
            rusqlite::params![boot_id, tmux_server_pid, alias],
        )?;
        Ok(())
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

    /// Orbit Fleet 11.4, the rotate test: a rotation keeps when the host's
    /// first token was minted, says when it was replaced, and starts "last
    /// used" over; writing the same token back is no rotation. Use is
    /// stamped at most once a minute and never moves the auth epoch, which
    /// a rotation does.
    #[test]
    fn a_rotation_keeps_created_stamps_rotated_and_use_is_liveness_only() {
        let s = Store::open_in_memory().expect("open");
        s.upsert_host_token_at("mercury", "tok-1", 1_000).unwrap();
        let row = s.get_host_token("mercury").unwrap().unwrap();
        assert_eq!(
            (row.created_at, row.rotated_at, row.last_used_at),
            (1_000, None, None)
        );

        let epoch = s.auth_epoch().unwrap();
        s.touch_host_token("mercury", 2_000).unwrap();
        s.touch_host_token("mercury", 2_030).unwrap(); // inside the minute: ignored
        assert_eq!(
            s.get_host_token("mercury").unwrap().unwrap().last_used_at,
            Some(2_000)
        );
        s.touch_host_token("mercury", 2_100).unwrap();
        assert_eq!(
            s.get_host_token("mercury").unwrap().unwrap().last_used_at,
            Some(2_100)
        );
        assert_eq!(
            s.auth_epoch().unwrap(),
            epoch,
            "liveness rebuilds no token cache"
        );

        s.upsert_host_token_at("mercury", "tok-1", 3_000).unwrap();
        let same = s.get_host_token("mercury").unwrap().unwrap();
        assert_eq!(
            (same.rotated_at, same.last_used_at),
            (None, Some(2_100)),
            "the same token is no rotation"
        );
        assert_eq!(s.auth_epoch().unwrap(), epoch);

        s.upsert_host_token_at("mercury", "tok-2", 4_000).unwrap();
        let row = s.get_host_token("mercury").unwrap().unwrap();
        assert_eq!(row.token, "tok-2");
        assert_eq!(
            (row.created_at, row.rotated_at, row.last_used_at),
            (1_000, Some(4_000), None)
        );
        assert!(
            s.auth_epoch().unwrap() > epoch,
            "a rotation changes what a token resolves to"
        );
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
    fn set_host_versions_at_stamps_the_row_and_lists_it() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h", Some("h")).unwrap();
        let before = s.get_host_row("h").unwrap().unwrap();
        assert_eq!(
            before.claude_version_at, None,
            "a fresh host has never been version-probed"
        );
        s.set_host_versions_at("h", 1_700_000_000).unwrap();
        let row = s.get_host_row("h").unwrap().unwrap();
        assert_eq!(row.claude_version_at, Some(1_700_000_000));
    }

    #[test]
    fn a_worktree_size_moves_on_an_answer_and_its_stamp_on_every_ask() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h", Some("h")).unwrap();
        s.set_host_worktree_size("h", Some(9_400_000), 100).unwrap();
        let row = s.get_host_row("h").unwrap().unwrap();
        assert_eq!(
            (row.worktree_kb, row.worktree_at),
            (Some(9_400_000), Some(100))
        );
        // A `du` that did not answer keeps the last size, but not its stamp.
        s.set_host_worktree_size("h", None, 200).unwrap();
        let row = s.get_host_row("h").unwrap().unwrap();
        assert_eq!(
            (row.worktree_kb, row.worktree_at),
            (Some(9_400_000), Some(200))
        );
    }

    #[test]
    fn set_host_health_writes_every_column_and_the_stamp() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h", Some("h")).unwrap();
        let sample = crate::tmux::HostHealthSample {
            disk_home_free_kb: Some(3_600_000),
            disk_home_total_kb: Some(150_000_000),
            disk_tmp_free_kb: Some(5_900_000),
            load_1m: Some(5.25),
            mem_avail_kb: Some(1_234_567),
            uptime_secs: Some(144 * 86400),
            cpu_count: Some(16),
            mem_total_kb: Some(65_842_312),
            boot_at: Some(1_687_558_400),
            latency_ms: Some(18),
            auth_overrides: Some(vec!["CLAUDE_CODE_USE_BEDROCK".into()]),
            agents_on_path: Some(vec!["claude".into(), "codex".into()]),
        };
        s.set_host_health("h", &sample, 1_700_000_000).unwrap();
        let row = s.get_host_row("h").unwrap().unwrap();
        assert_eq!(row.cpu_count, Some(16));
        assert_eq!(row.mem_total_kb, Some(65_842_312));
        assert_eq!(row.boot_at, Some(1_687_558_400));
        assert_eq!(row.latency_ms, Some(18));
        let ping = crate::store::HostHealth::of(&row);
        assert_eq!((ping.cpu_count, ping.latency_ms), (Some(16), Some(18)));
        let agents = Some(vec!["claude".to_string(), "codex".to_string()]);
        assert_eq!(row.agents_on_path, agents);
        assert_eq!(ping.agents_on_path, agents, "the ping carries it");
        assert_eq!(row.disk_home_free_kb, Some(3_600_000));
        assert_eq!(row.disk_home_total_kb, Some(150_000_000));
        assert_eq!(row.disk_tmp_free_kb, Some(5_900_000));
        assert_eq!(row.load_1m, Some(5.25));
        assert_eq!(row.mem_avail_kb, Some(1_234_567));
        assert_eq!(row.uptime_secs, Some(144 * 86400));
        assert_eq!(
            row.auth_overrides,
            Some(vec!["CLAUDE_CODE_USE_BEDROCK".to_string()])
        );
        // A sample that could not tell clears the column to "unknown".
        s.set_host_health(
            "h",
            &crate::tmux::HostHealthSample::default(),
            1_700_000_001,
        )
        .unwrap();
        assert_eq!(s.get_host_row("h").unwrap().unwrap().auth_overrides, None);
        assert_eq!(s.get_host_row("h").unwrap().unwrap().agents_on_path, None);
        assert_eq!(row.health_at, Some(1_700_000_000));
        s.set_host_last_hook_at("h", 1_700_000_100).unwrap();
        s.set_host_agent_version("h", "0.2.26").unwrap();
        let row = s.get_host_row("h").unwrap().unwrap();
        assert_eq!(row.last_hook_at, Some(1_700_000_100));
        assert_eq!(row.agent_version.as_deref(), Some("0.2.26"));
    }

    #[test]
    fn set_host_last_hook_at_is_throttled_to_one_write_a_minute() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h", Some("h")).unwrap();
        let t = 1_700_000_000;
        let stamp = |s: &Store| s.get_host_row("h").unwrap().unwrap().last_hook_at;
        assert_eq!(stamp(&s), None);
        // A NULL row is stamped.
        s.set_host_last_hook_at("h", t).unwrap();
        assert_eq!(stamp(&s), Some(t));
        // Within the window: left at t.
        s.set_host_last_hook_at("h", t + 30).unwrap();
        assert_eq!(stamp(&s), Some(t));
        s.set_host_last_hook_at("h", t + HOST_HOOK_STAMP_EVERY_SECS)
            .unwrap();
        assert_eq!(stamp(&s), Some(t));
        // Past it: moved.
        s.set_host_last_hook_at("h", t + 61).unwrap();
        assert_eq!(stamp(&s), Some(t + 61));
    }

    /// hosts F1: `provisioned` was a boolean set once; now it carries which
    /// content and when, so an older provisioning reads as stale.
    #[test]
    fn set_host_provisioned_records_the_fingerprint_and_a_changed_one_reads_stale() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h", Some("h")).unwrap();
        let fresh = s.get_host_row("h").unwrap().unwrap();
        assert!(
            !fresh.provisioned && !fresh.provision_stale,
            "unprovisioned is not stale"
        );
        s.set_host_provisioned("h", true).unwrap();
        let row = s.get_host_row("h").unwrap().unwrap();
        assert!(row.provisioned);
        assert!(row.provisioned_at.is_some());
        assert!(!row.provision_stale);
        s.conn_for_test()
            .execute(
                "UPDATE hosts SET provision_fingerprint='old' WHERE alias='h'",
                [],
            )
            .unwrap();
        assert!(s.get_host_row("h").unwrap().unwrap().provision_stale);
        // A provisioning from before the fingerprint existed is stale too.
        s.conn_for_test()
            .execute(
                "UPDATE hosts SET provision_fingerprint=NULL WHERE alias='h'",
                [],
            )
            .unwrap();
        assert!(s.get_host_row("h").unwrap().unwrap().provision_stale);
        s.set_host_provisioned("h", false).unwrap();
        let row = s.get_host_row("h").unwrap().unwrap();
        assert!(!row.provisioned && !row.provision_stale && row.provisioned_at.is_none());
    }

    /// Migration 093: a merged host keeps its admissions, like its layer
    /// assignments (M2 fix round 1).
    #[test]
    fn merge_host_alias_carries_admissions() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        s.upsert_host("mac").unwrap();
        s.set_catalog_config("/p", None).unwrap();
        let org = s.add_org("acme", None, false).unwrap();
        let acme = s
            .upsert_catalog("acme", "/a", None, Some(org.id))
            .unwrap()
            .id;
        s.admit_host_catalog("local", acme).unwrap();
        s.merge_host_alias("local", "mac").unwrap();
        assert_eq!(s.host_admissions("mac").unwrap(), vec![acme]);
        assert!(s.host_admissions("local").unwrap().is_empty());
    }

    /// data-sync F2/F5: the `local` → `mac` rename left 249 worktree rows
    /// and 6 duplicate agent rows on a hidden alias nothing merged.
    #[test]
    fn merge_host_alias_rehomes_rows_dedupes_sessions_and_sums_usage_in_one_go() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        s.insert_host("mac", Some("mac")).unwrap();
        // Layer assignments in two catalogs on the merged-away host: both
        // must arrive under `into` carrying their own `catalog_id` (a
        // NOT NULL column with no default — `INSERT OR IGNORE` silently
        // drops any row missing it instead of erroring).
        s.set_catalog_config("/p", None).unwrap();
        let personal = s.personal_catalog().unwrap().unwrap().id;
        let org = s.add_org("acme", None, false).unwrap().id;
        s.conn_for_test()
            .execute(
                "INSERT INTO catalogs (name, repo_path, org_id, created_at) VALUES ('acme', '/a', ?1, 0)",
                [org],
            )
            .unwrap();
        let acme: i64 = s
            .conn_for_test()
            .query_row("SELECT id FROM catalogs WHERE name='acme'", [], |r| {
                r.get(0)
            })
            .unwrap();
        s.set_host_layers("local", Some("core"), &[]).unwrap();
        s.set_host_layers_for("local", acme, Some("ops"), &["extra"])
            .unwrap();
        let pid = s.upsert_project("o", "r", "/Users/m/o/r").unwrap();
        let wt = s
            .upsert_worktree_on(
                "local",
                pid,
                "feat",
                "/Users/m/o/r/.claude/worktrees/feat",
                Some("feat"),
            )
            .unwrap();
        s.record_parent_fingerprint("local", "/Users/m/o/r/.claude/worktrees/feat", "1:2", 1)
            .unwrap();
        s.conn_for_test()
            .execute_batch(
                "INSERT INTO dismissed_agents (host_alias, claude_session_id, dismissed_at) VALUES ('local','sid-x',1);
                 INSERT INTO usage_daily (day, host_alias, input_tokens, output_tokens, cache_write_tokens, cache_read_tokens, cost_micros) VALUES (20000,'local',10,1,0,0,100),(20000,'mac',5,1,0,0,50),(20001,'local',7,0,0,0,70);",
            )
            .unwrap();
        // `upsert_session`'s last argument is the account, not the Claude
        // session id; the ids that make two rows "the same agent" are set
        // directly.
        let with_claude_id = |name: &str, host: &str, wt: Option<i64>, status: &str, sid: &str| {
            let id = s
                .upsert_session(name, host, Some(pid), wt, 1, 1, status, None)
                .unwrap();
            s.conn_for_test()
                .execute(
                    "UPDATE sessions SET claude_session_id=?1 WHERE id=?2",
                    rusqlite::params![sid, id],
                )
                .unwrap();
            id
        };
        let dup_local = with_claude_id("bg:dup", "local", Some(wt), "ghost", "sid-dup");
        let dup_mac = with_claude_id("bg:dup2", "mac", None, "running", "sid-dup");
        let moved = with_claude_id("bg:only", "local", Some(wt), "ghost", "sid-only");
        let clash = s
            .upsert_session("dev-same", "local", None, None, 1, 1, "ghost", None)
            .unwrap();
        s.upsert_session("dev-same", "mac", None, None, 3, 3, "running", None)
            .unwrap();

        let rep = s.merge_host_alias("local", "mac").unwrap();
        assert_eq!(
            (
                rep.worktrees_moved,
                rep.sessions_moved,
                rep.sessions_dropped,
                rep.usage_days_merged
            ),
            (1, 1, 2, 2)
        );
        assert!(
            s.get_host_row("local").unwrap().is_none(),
            "the from row is gone"
        );
        assert_eq!(s.get_worktree_row(wt).unwrap().unwrap().host_alias, "mac");
        assert_eq!(
            s.parent_fingerprint("mac", "/Users/m/o/r/.claude/worktrees/feat")
                .unwrap()
                .as_deref(),
            Some("1:2")
        );
        assert!(
            s.get_session_by_id(dup_local).unwrap().is_none(),
            "the duplicate claude_session_id under from is dropped"
        );
        assert!(s.get_session_by_id(dup_mac).unwrap().is_some());
        assert_eq!(
            s.get_session_by_id(moved).unwrap().unwrap().host_alias,
            "mac"
        );
        assert!(
            s.get_session_by_id(clash).unwrap().is_none(),
            "a tmux_name the target already has keeps the target's row"
        );
        let (i, c): (i64, i64) = s
            .conn_for_test()
            .query_row(
                "SELECT input_tokens, cost_micros FROM usage_daily WHERE day=20000 AND host_alias='mac'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((i, c), (15, 150));
        let n: i64 = s
            .conn_for_test()
            .query_row(
                "SELECT COUNT(*) FROM usage_daily WHERE host_alias='local'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0);
        let d: i64 = s
            .conn_for_test()
            .query_row(
                "SELECT COUNT(*) FROM dismissed_agents WHERE host_alias='mac'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(d, 1);
        // Both catalogs' layer assignments arrived under `into` with their
        // own catalog id, and nothing is left on `from`.
        let p = s.get_host_layers_for("mac", personal).unwrap();
        assert_eq!(
            p.iter().map(|r| r.layer_name.as_str()).collect::<Vec<_>>(),
            vec!["core"]
        );
        let a = s.get_host_layers_for("mac", acme).unwrap();
        assert_eq!(a.len(), 2);
        assert!(a.iter().all(|r| r.catalog_id == acme));
        assert_eq!(s.get_host_layers("mac").unwrap().len(), 3);
        assert!(s.get_host_layers("local").unwrap().is_empty());
    }

    #[test]
    fn merge_host_alias_refuses_the_same_alias_and_an_unknown_target() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("a", None).unwrap();
        assert_eq!(s.merge_host_alias("a", "a").unwrap_err().code, "E_INVALID");
        assert_eq!(
            s.merge_host_alias("a", "nope").unwrap_err().code,
            "E_NOTFOUND"
        );
        assert_eq!(
            s.merge_host_alias("nope", "a").unwrap_err().code,
            "E_NOTFOUND"
        );
    }

    /// The merge left `asset_inventory`, `org_rules.host_alias` and
    /// `hosts.org_id` on the deleted alias.
    #[test]
    fn merge_host_alias_moves_inventory_org_rules_and_org_id() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("from", None).unwrap();
        s.insert_host("into", None).unwrap();
        let org1 = s.add_org("one", None, false).unwrap().id;
        let org2 = s.add_org("two", None, false).unwrap().id;
        s.set_host_org("from", Some(org1)).unwrap();
        s.conn_for_test()
            .execute_batch(
                "INSERT INTO asset_inventory (host_alias, harness, kind, name, state, scanned_at) VALUES
                   ('from','claude','skill','clash','drift',1),
                   ('into','claude','skill','clash','in_sync',2),
                   ('from','claude','skill','only','in_sync',1);",
            )
            .unwrap();
        let rule = |org_id: i64, host: &str, owner: Option<&str>| OrgRuleRow {
            org_id,
            owner: owner.map(str::to_string),
            host_alias: Some(host.to_string()),
            ..Default::default()
        };
        s.add_org_rule(rule(org1, "from", Some("acme"))).unwrap();
        s.add_org_rule(rule(org1, "into", Some("acme"))).unwrap();
        s.add_org_rule(rule(org1, "from", None)).unwrap();

        s.merge_host_alias("from", "into").unwrap();

        assert_eq!(s.host_org("into").unwrap(), Some(org1));
        let count =
            |sql: &str| -> i64 { s.conn_for_test().query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(
            count("SELECT COUNT(*) FROM asset_inventory WHERE host_alias='from'"),
            0
        );
        assert_eq!(
            count("SELECT COUNT(*) FROM org_rules WHERE host_alias='from'"),
            0
        );
        let clash_state: String = s
            .conn_for_test()
            .query_row(
                "SELECT state FROM asset_inventory WHERE host_alias='into' AND name='clash'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(clash_state, "in_sync", "into's own scan wins a clash");
        assert_eq!(
            count("SELECT COUNT(*) FROM asset_inventory WHERE host_alias='into' AND name='only'"),
            1
        );
        assert_eq!(
            count("SELECT COUNT(*) FROM org_rules WHERE host_alias='into' AND owner='acme'"),
            1,
            "the duplicate rule collapsed to one"
        );
        assert_eq!(
            count("SELECT COUNT(*) FROM org_rules WHERE host_alias='into' AND owner IS NULL"),
            1,
            "the host-only rule moved"
        );

        // An `into` that already has an org keeps it.
        s.insert_host("b", None).unwrap();
        s.insert_host("c", None).unwrap();
        s.set_host_org("b", Some(org1)).unwrap();
        s.set_host_org("c", Some(org2)).unwrap();
        s.merge_host_alias("b", "c").unwrap();
        assert_eq!(s.host_org("c").unwrap(), Some(org2));
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
    fn delete_host_reaps_timeline_and_fingerprints_but_tombstones_the_inbox() {
        // Previously named `..._reaps_timeline_inbox_and_...` and asserted
        // `list_inbox(gone, ...)` was empty after the delete — that
        // assertion encoded the pre-existing defect Task 13 fixes (removing
        // a host used to destroy every undelivered message addressed to its
        // sessions). The message now survives; only the participant
        // identity is tombstoned.
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
        let to_gone = s
            .insert_message(peer, gone, "to the gone", "message", None)
            .unwrap();
        s.insert_message(gone, peer, "from the gone", "message", None)
            .unwrap();
        let participant = s.participant_for_session(gone).unwrap().unwrap().id;
        s.record_parent_fingerprint("h", "/h/r/w", "1:2", 1)
            .unwrap();
        s.record_parent_fingerprint("local", "/l/r/w", "3:4", 1)
            .unwrap();

        s.delete_host("h").unwrap();

        assert!(s.list_session_events(gone, 10).unwrap().is_empty());
        assert!(
            s.get_message(to_gone).unwrap().is_some(),
            "removing a host must not destroy undelivered mail"
        );
        let p = s.participant_by_id(participant).unwrap().unwrap();
        assert!(p.retired_at.is_some(), "the identity is tombstoned instead");
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
                    by_day: Vec::new(),
                    backfill_until: None,
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
        s.set_catalog_config("/p", None).unwrap();
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

    fn seed_update_state(s: &Store, alias: &str, version: &str) {
        s.upsert_update_observed(&crate::store::UpdateObservedRow {
            target: format!("agent:{alias}"),
            component: "agent".into(),
            platform: None,
            version: version.into(),
            commit_sha: None,
            build_id: None,
            digest: None,
            speaks: None,
            phase: "idle".into(),
            attempt: None,
            last_error: None,
            reported_at: 1,
            last_checked_at: None,
        })
        .unwrap();
        s.set_update_desired(&crate::store::UpdateDesiredRow {
            component: "agent".into(),
            target: format!("agent:{alias}"),
            version: version.into(),
            mandatory: false,
            reason: None,
            set_by: "operator".into(),
            set_at: 1,
        })
        .unwrap();
    }

    /// A deleted host's `agent:<alias>` observed row and pin went on
    /// counting in `update_status` forever.
    #[test]
    fn delete_host_drops_its_agent_update_state() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h", Some("h")).unwrap();
        s.insert_host("other", Some("other")).unwrap();
        seed_update_state(&s, "h", "0.3.3");
        seed_update_state(&s, "other", "0.3.3");

        s.delete_host("h").unwrap();

        assert!(s.update_observed("agent:h").unwrap().is_none());
        assert!(s.update_desired_for("agent", "agent:h").unwrap().is_none());
        assert!(s.update_observed("agent:other").unwrap().is_some());
        assert!(s
            .update_desired_for("agent", "agent:other")
            .unwrap()
            .is_some());
    }

    /// The merge re-homes `agent:<from>` under `agent:<into>`; `into`'s own
    /// observed row and pin win a clash.
    #[test]
    fn merge_host_alias_rehomes_agent_update_state() {
        let s = Store::open_in_memory().unwrap();
        for h in ["a", "b", "c"] {
            s.insert_host(h, None).unwrap();
        }
        // `a` has state, `b` has none: it moves.
        seed_update_state(&s, "a", "0.3.3");
        assert!(s
            .insert_update_event(
                "agent:a",
                Some("att1"),
                "success",
                None,
                None,
                None,
                None,
                1
            )
            .unwrap());
        s.merge_host_alias("a", "b").unwrap();
        assert!(s.update_observed("agent:a").unwrap().is_none());
        assert!(s.update_events("agent:a", 10).unwrap().is_empty());
        assert_eq!(s.update_events("agent:b", 10).unwrap().len(), 1);
        assert_eq!(
            s.update_observed("agent:b").unwrap().unwrap().version,
            "0.3.3"
        );
        assert_eq!(
            s.update_desired_for("agent", "agent:b")
                .unwrap()
                .unwrap()
                .target,
            "agent:b"
        );

        // Both have state: `c`'s own is kept, `b`'s is gone.
        seed_update_state(&s, "c", "0.3.4");
        s.merge_host_alias("b", "c").unwrap();
        assert!(s.update_observed("agent:b").unwrap().is_none());
        assert_eq!(
            s.update_observed("agent:c").unwrap().unwrap().version,
            "0.3.4"
        );
        assert_eq!(
            s.update_desired_for("agent", "agent:c")
                .unwrap()
                .unwrap()
                .version,
            "0.3.4"
        );
        let leftover: i64 = s
            .conn_for_test()
            .query_row(
                "SELECT COUNT(*) FROM update_desired WHERE target IN ('agent:a','agent:b')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(leftover, 0);
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

    // ── host boot identity (migration 036) ──

    #[test]
    fn host_identity_round_trips_and_defaults_to_unknown() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("mefistos").unwrap();
        assert_eq!(
            s.get_host_identity("mefistos").unwrap(),
            StoredIdentity::default()
        );

        s.set_host_identity("mefistos", Some("boot-a"), Some(4242))
            .unwrap();
        assert_eq!(
            s.get_host_identity("mefistos").unwrap(),
            StoredIdentity {
                boot_id: Some("boot-a".into()),
                tmux_server_pid: Some(4242)
            }
        );

        // "No server" is stored as a NULL pid, distinct from a changed pid.
        s.set_host_identity("mefistos", Some("boot-a"), None)
            .unwrap();
        assert_eq!(
            s.get_host_identity("mefistos").unwrap().tmux_server_pid,
            None
        );
    }

    #[test]
    fn host_identity_of_an_unknown_host_is_unknown_not_an_error() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(
            s.get_host_identity("ghost").unwrap(),
            StoredIdentity::default()
        );
    }

    /// Multi-harness F3a: a new host is on auto (`None`); a list round-trips
    /// through its JSON column, `None` clears it again, every write emits the
    /// row, and an unknown alias is `E_NOTFOUND`.
    #[test]
    fn set_host_harnesses_round_trips_a_list_and_auto() {
        let bus = Arc::new(crate::events::RecordingEventBus::new());
        let s = Store::open_with_bus_in_memory(bus.clone()).unwrap();
        s.insert_host("h", Some("h")).unwrap();
        assert_eq!(
            s.get_host_row("h").unwrap().unwrap().harnesses,
            None,
            "a new host is on auto"
        );
        bus.take();
        let both = vec!["claude".to_string(), "codex".to_string()];
        s.set_host_harnesses("h", Some(both.as_slice())).unwrap();
        assert_eq!(s.get_host_row("h").unwrap().unwrap().harnesses, Some(both));
        assert!(bus.take().contains(&"host:probed:h".to_string()));
        s.set_host_harnesses("h", None).unwrap();
        assert_eq!(s.get_host_row("h").unwrap().unwrap().harnesses, None);
        let err = s.set_host_harnesses("nope", None).unwrap_err();
        assert_eq!(err.code, "E_NOTFOUND");
    }

    /// A stored value that is not a JSON string array (hand-edited, or from a
    /// future schema) reads as auto instead of failing every host read.
    #[test]
    fn an_unreadable_harnesses_value_reads_as_auto() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h", Some("h")).unwrap();
        s.conn
            .execute("UPDATE hosts SET harnesses='not json' WHERE alias='h'", [])
            .unwrap();
        assert_eq!(s.get_host_row("h").unwrap().unwrap().harnesses, None);
    }
}
