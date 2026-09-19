# Host-Reboot Safety Net Implementation Plan (PR 1 of 2)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A reachable host whose tmux server vanished (or whose machine rebooted) marks its sessions *lost* instead of deleting them, and a resumable lost session survives reconcile until it is restored, dismissed, or ages out.

**Architecture:** Each probe additionally reads a host identity (boot id + tmux server pid) through a defaulted `TmuxExec::host_identity`. The per-host writer compares it with the identity stored on the `hosts` row; a changed boot id or a missing/changed tmux server is a *mass-loss verdict*, which marks every session not seen live as lost with a `lost_reason` and skips the normal prune for that pass. The existing two-phase reap then exempts resumable mass-loss rows until a TTL. Restore and discovery (R3/R4) are PR 2.

**Tech Stack:** Rust (`crates/fleet-core`), rusqlite, tokio, `tracing`.

**Spec:** `docs/superpowers/specs/2026-09-17-host-reboot-session-survival-design.md` (findings re-verified on `origin/main` `1d6bedb`).

## Global Constraints

- **Tree:** all backend code lives in `crates/fleet-core/src/`. Workspace commands only: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all`. Never `--manifest-path src-tauri/Cargo.toml`.
- **Migration number is `034`** (`032_client_tokens`, `033_asset_layers` exist). It uses `ALTER TABLE ... ADD COLUMN`, which is NOT idempotent — it must be registered with an `already_applied` guard, exactly like migration 027 (`projects_has_adopted`, `store/schema.rs:75,192`).
- **`HostRow` does not change.** `boot_id` / `tmux_server_pid` are `hosts` columns read and written only through the new accessors in Task 1. `HostRow` is serialised to the frontend and read everywhere; keeping it untouched avoids that blast radius.
- **`SessionRow` does not change.** `lost_reason` is a `sessions` column used by SQL in this PR; exposing it on the row is PR 2 (restore), which is its first consumer.
- **Fail safe:** an identity that could not be read, or was read garbled, must produce NO verdict. A wrong mass-loss verdict marks every session on a host lost.
- **Backward compatibility:** a host with no stored identity (every host on first run after upgrade) and a pass with no verdict behave exactly as today, including today's one-cycle reap of a `missing` session.
- `Store` is behind a `std::sync::Mutex`; never hold the guard across an `.await`.
- Every implementer runs the **full** `cargo test --workspace` before committing and reports pass/fail counts. A filtered suite hid a red test for five tasks on the previous plan.
- Values the verdict writes into `lost_reason`: exactly `host_reboot`, `tmux_server_gone`, `missing`.

## Deliberate deviations from the spec (rulings)

1. **`RowChange::HostSessionsLost` is deferred to PR 2.** It has no consumer until the restore UI exists, and a new `RowChange` variant touches five exhaustive `match` sites in `events.rs`. PR 1 still emits one `SessionUpdated` per lost row and one `lost` `session_events` row each — the per-session signal the spec asks for.
2. **The "host has no live rows" guard is implemented structurally, not as a branch.** The mass-loss path marks only rows not seen live and only non-ghost rows, so on a host with nothing to lose it changes nothing; the only effect is `skip_prune`, which is a no-op when there is nothing to prune.
3. **`tmux_server_gone` marks tmux-backed rows only; `host_reboot` marks every kind.** A tmux server restart does not kill `claude --bg` agents (they run outside tmux); a reboot does. The spec marked both kinds in both cases, which would ghost live background agents on a plain tmux restart.

---

### Task 1: Migration 034 and host-identity accessors

**Files:**
- Create: `crates/fleet-core/migrations/034_host_boot_identity.sql`
- Modify: `crates/fleet-core/src/store/schema.rs` (guard fn next to `projects_has_adopted` at `:75`; `MIGRATIONS` entry after the `033` one)
- Modify: `crates/fleet-core/src/store/hosts_accounts.rs` (two accessors + tests)

**Interfaces:**
- Produces: `Store::get_host_identity(&self, alias: &str) -> rusqlite::Result<StoredIdentity>`, `Store::set_host_identity(&self, alias: &str, boot_id: Option<&str>, tmux_server_pid: Option<i64>) -> rusqlite::Result<()>`, and `pub struct StoredIdentity { pub boot_id: Option<String>, pub tmux_server_pid: Option<i64> }` (derive `Debug, Clone, Default, PartialEq`) in `store/rows.rs`, re-exported from `store`.

- [ ] **Step 1: Write the failing test** (append to the `tests` module of `store/hosts_accounts.rs`, using that module's existing in-memory store helper)

```rust
    #[test]
    fn host_identity_round_trips_and_defaults_to_unknown() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("mefistos").unwrap();
        assert_eq!(s.get_host_identity("mefistos").unwrap(), StoredIdentity::default());

        s.set_host_identity("mefistos", Some("boot-a"), Some(4242)).unwrap();
        assert_eq!(
            s.get_host_identity("mefistos").unwrap(),
            StoredIdentity { boot_id: Some("boot-a".into()), tmux_server_pid: Some(4242) }
        );

        // "No server" is stored as a NULL pid, distinct from a changed pid.
        s.set_host_identity("mefistos", Some("boot-a"), None).unwrap();
        assert_eq!(s.get_host_identity("mefistos").unwrap().tmux_server_pid, None);
    }

    #[test]
    fn host_identity_of_an_unknown_host_is_unknown_not_an_error() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.get_host_identity("ghost").unwrap(), StoredIdentity::default());
    }
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p fleet-core --lib host_identity` → compile error, `get_host_identity` not found.

- [ ] **Step 3: Implement.** Migration file:

```sql
-- Host boot identity (host-reboot safety net). `boot_id` is the kernel boot
-- id (the HOST kernel's, even from inside a container); `tmux_server_pid` is
-- the pid of the host's tmux server, NULL when none is running. A change in
-- either between probes marks the host's sessions lost instead of deleting
-- them. `lost_reason` says why a ghost row is lost:
-- host_reboot | tmux_server_gone | missing.
ALTER TABLE hosts ADD COLUMN boot_id TEXT;
ALTER TABLE hosts ADD COLUMN tmux_server_pid INTEGER;
ALTER TABLE sessions ADD COLUMN lost_reason TEXT;

INSERT OR IGNORE INTO schema_version (version) VALUES (34);
```

Guard in `schema.rs` (mirror `projects_has_adopted`):

```rust
fn sessions_has_lost_reason(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'lost_reason'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}
```

Registration after `033`:

```rust
    // `ALTER TABLE ... ADD COLUMN` fails if the column is already there.
    Migration {
        version: 34,
        sql: include_str!("../../migrations/034_host_boot_identity.sql"),
        already_applied: Some(sessions_has_lost_reason),
    },
```

Accessors in `hosts_accounts.rs`:

```rust
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

    pub fn set_host_identity(
        &self,
        alias: &str,
        boot_id: Option<&str>,
        tmux_server_pid: Option<i64>,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE hosts SET boot_id = ?1, tmux_server_pid = ?2 WHERE alias = ?3",
            rusqlite::params![boot_id, tmux_server_pid, alias],
        )?;
        Ok(())
    }
```

(Add `use rusqlite::OptionalExtension;` if the file does not already import it.)

- [ ] **Step 4: Run** `cargo test -p fleet-core --lib host_identity` → PASS, then the full `cargo test --workspace` (the migration-rerun and latest-version tests must stay green).
- [ ] **Step 5: Commit** — `feat(store): migration 034 and host boot-identity accessors`

---

### Task 2: `HostIdentity`, its script and parser, and `TmuxExec::host_identity`

**Files:** Modify `crates/fleet-core/src/tmux.rs`.

**Interfaces:**
- Produces: `pub struct HostIdentity { pub boot_id: Option<String>, pub tmux_server_pid: Option<i64> }` (derive `Debug, Clone, Default, PartialEq`), `pub const HOST_IDENTITY_SCRIPT: &str`, `pub fn parse_host_identity(stdout: &str) -> Option<HostIdentity>`, and a defaulted trait method `async fn host_identity(&self) -> Option<HostIdentity> { None }`, overridden by `LocalTmux` and `RemoteTmux`.

The `Option` wrapper is load-bearing and must not be flattened: **outer `None` = the read failed or was unreadable (no verdict ever)**; `Some(HostIdentity { tmux_server_pid: None, .. })` = the script ran and found no tmux server (the signal).

- [ ] **Step 1: Write the failing tests** (in `tmux.rs`'s `tests` module)

```rust
    #[test]
    fn parse_host_identity_reads_both_fields() {
        let id = parse_host_identity("boot=abc-123\ntmuxpid=4242\n").unwrap();
        assert_eq!(id.boot_id.as_deref(), Some("abc-123"));
        assert_eq!(id.tmux_server_pid, Some(4242));
    }

    #[test]
    fn an_empty_tmuxpid_means_no_server_not_unknown() {
        let id = parse_host_identity("boot=abc\ntmuxpid=\n").unwrap();
        assert_eq!(id.tmux_server_pid, None);
    }

    #[test]
    fn unreadable_output_is_unknown_so_it_can_never_mark_a_host_lost() {
        // No tmuxpid line at all: a login banner, a wrapper, a truncated run.
        assert_eq!(parse_host_identity("Welcome to Ubuntu\n"), None);
        assert_eq!(parse_host_identity(""), None);
        // A pid line that is not a number is garbage, not "no server".
        assert_eq!(parse_host_identity("boot=a\ntmuxpid=not-a-pid\n"), None);
    }

    #[test]
    fn a_missing_boot_id_is_just_unknown_boot() {
        let id = parse_host_identity("boot=\ntmuxpid=7\n").unwrap();
        assert_eq!(id.boot_id, None);
        assert_eq!(id.tmux_server_pid, Some(7));
    }

    #[tokio::test]
    async fn the_identity_script_runs_under_local_bash() {
        // Real bash, real `tmux` if installed: the script must produce a
        // parseable answer whether or not a tmux server is running here.
        let out = tokio::process::Command::new("bash")
            .args(["-c", HOST_IDENTITY_SCRIPT])
            .output()
            .await
            .unwrap();
        assert!(out.status.success());
        assert!(parse_host_identity(&String::from_utf8_lossy(&out.stdout)).is_some());
    }
```

- [ ] **Step 2: Run** `cargo test -p fleet-core --lib host_identity` → compile error.

- [ ] **Step 3: Implement**

```rust
/// A host's boot identity, read once per reconcile probe.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HostIdentity {
    /// Kernel boot id — `/proc/sys/kernel/random/boot_id` (the HOST's, even
    /// inside a container), else `sysctl -n kern.boottime` (macOS), else
    /// `uptime -s`. `None` when none of them produced anything.
    pub boot_id: Option<String>,
    /// Pid of the tmux server. `None` ⇒ no tmux server is running.
    pub tmux_server_pid: Option<i64>,
}

/// Prints `boot=<id>` and `tmuxpid=<pid or empty>`. Never fails: every
/// command is guarded, so a non-zero exit means the transport failed.
pub const HOST_IDENTITY_SCRIPT: &str = "printf 'boot=%s\\n' \"$(cat /proc/sys/kernel/random/boot_id 2>/dev/null || sysctl -n kern.boottime 2>/dev/null || uptime -s 2>/dev/null)\"; printf 'tmuxpid=%s\\n' \"$(tmux display-message -p '#{pid}' 2>/dev/null)\"";

/// Parse [`HOST_IDENTITY_SCRIPT`] output. `None` unless the `tmuxpid=` line is
/// present and its value is empty or a number — anything else is output we
/// cannot trust, and an untrusted "no server" would mark every session on
/// the host lost.
pub fn parse_host_identity(stdout: &str) -> Option<HostIdentity> {
    let mut boot_id = None;
    let mut pid_line: Option<&str> = None;
    for line in stdout.lines() {
        if let Some(v) = line.strip_prefix("boot=") {
            let v = v.trim();
            if !v.is_empty() {
                boot_id = Some(v.to_string());
            }
        } else if let Some(v) = line.strip_prefix("tmuxpid=") {
            pid_line = Some(v.trim());
        }
    }
    let tmux_server_pid = match pid_line? {
        "" => None,
        v => Some(v.parse::<i64>().ok()?),
    };
    Some(HostIdentity { boot_id, tmux_server_pid })
}
```

Trait method (beside `read_oauth_account`, with a doc comment stating the `None` semantics above):

```rust
    async fn host_identity(&self) -> Option<HostIdentity> {
        None
    }
```

`LocalTmux` (mirror its `transcript_mtimes`, including the `local_allowed()` guard):

```rust
    async fn host_identity(&self) -> Option<HostIdentity> {
        local_allowed().ok()?;
        let out = tokio::process::Command::new("bash")
            .args(["-c", HOST_IDENTITY_SCRIPT])
            .output()
            .await
            .ok()
            .filter(|o| o.status.success())?;
        parse_host_identity(&String::from_utf8_lossy(&out.stdout))
    }
```

`RemoteTmux` (mirror its `read_oauth_account`):

```rust
    async fn host_identity(&self) -> Option<HostIdentity> {
        let out = self
            .remote_bash(HOST_IDENTITY_SCRIPT)
            .await
            .ok()
            .filter(|o| o.status.success())?;
        parse_host_identity(&String::from_utf8_lossy(&out.stdout))
    }
```

- [ ] **Step 4: Run** the new tests → PASS; full `cargo test --workspace`.
- [ ] **Step 5: Commit** — `feat(tmux): read a host's boot identity alongside its sessions`

---

### Task 3: The probe carries the identity

**Files:** Modify `crates/fleet-core/src/service/sessions/reconcile.rs` (`HostProbe`, `probe_with_timeout` at `:831-887`); tests in `crates/fleet-core/src/service/sessions/tests.rs`.

**Interfaces:**
- Consumes: `TmuxExec::host_identity`, `HostIdentity` (Task 2).
- Produces: `HostProbe.identity: Option<crate::tmux::HostIdentity>` — `None` whenever `list_sessions` failed or the probe timed out.

- [ ] **Step 1: Write the failing test.** Add a fake to `tests.rs` that mirrors `ScriptedTmux` (`tests.rs:1330`) — every required `TmuxExec` method returning the same trivial values — plus a configurable `host_identity`:

```rust
struct IdentityTmux {
    sessions: Vec<crate::tmux::TmuxSession>,
    identity: Option<crate::tmux::HostIdentity>,
}
// impl TmuxExec for IdentityTmux: list_sessions -> Ok(self.sessions.clone());
// host_identity -> self.identity.clone(); every other method as ScriptedTmux.
```

```rust
#[tokio::test]
async fn a_probe_records_the_host_identity_only_when_the_list_succeeded() {
    let id = crate::tmux::HostIdentity { boot_id: Some("b".into()), tmux_server_pid: Some(9) };
    let probe = probe_with_timeout(
        test_host_row("mefistos"),
        Box::new(IdentityTmux { sessions: vec![], identity: Some(id.clone()) }),
        std::time::Duration::from_secs(5),
        None,
    )
    .await;
    assert_eq!(probe.identity, Some(id));
}
```

Use whatever `HostRow` helper the neighbouring probe tests already use in place of `test_host_row`; if none exists, build the `HostRow` literal inline.

- [ ] **Step 2: Run** → compile error, no field `identity`.
- [ ] **Step 3: Implement.** Add the field to `HostProbe` with a doc comment; in `probe_with_timeout`'s async block read it right after the account read, under the same guard:

```rust
        let identity = if tmux_result.is_ok() {
            tmux.host_identity().await
        } else {
            None
        };
```

Extend the tuple `(tmux_result, agent_rows, agent_mtimes, intel, account, identity)` and both `HostProbe { .. }` constructions (the timeout path sets `identity: None`). Fix any other `HostProbe` literal the compiler flags by adding `identity: None`.

- [ ] **Step 4: Run** the new test → PASS; full suite.
- [ ] **Step 5: Commit** — `feat(reconcile): carry the host identity on each probe`

---

### Task 4: Lost reasons, and marking a host's sessions lost in one pass

**Files:** Modify `crates/fleet-core/src/store/reconcile.rs` (Phase 1 `UPDATE` at `:292`, upsert `lost_at=NULL` at `:142`); add `mark_host_sessions_lost` to `crates/fleet-core/src/store/sessions.rs` beside `ghost_and_clean_bg_sessions` (`:171`).

**Interfaces:**
- Produces: `Store::mark_host_sessions_lost(&self, host_alias: &str, reason: &str, keep_names: &[String], now: i64) -> rusqlite::Result<Vec<SessionRow>>` — ghosts every non-ghost row of the host not in `keep_names` (tmux-backed rows only when `reason == "tmux_server_gone"`, every kind otherwise), sets `lost_at = now` and `lost_reason = reason`, commits, emits one `SessionUpdated` per row after commit, and returns the rows. Also: Phase 1 ghosting now writes `lost_reason = 'missing'`, and the upsert clears `lost_reason` alongside `lost_at`.

- [ ] **Step 1: Write the failing tests** (store tests, `store/sessions.rs`; seed live rows with the store helpers the neighbouring `ghost_and_clean_bg_sessions` tests use, including a `claude_session_id` on at least one)

```rust
    #[test]
    fn mark_host_sessions_lost_ghosts_unseen_rows_and_keeps_every_identity_field() {
        // two live tmux rows on "h", one with claude_session_id "abc"; keep "b"
        let rows = s.mark_host_sessions_lost("h", "host_reboot", &["b".into()], 500).unwrap();
        assert_eq!(rows.len(), 1);
        let a = s.get_session("a", "h").unwrap().unwrap();
        assert_eq!(a.status, "ghost");
        assert_eq!(a.lost_at, Some(500));
        assert_eq!(a.claude_session_id.as_deref(), Some("abc"));
        assert_eq!(lost_reason_of(&s, a.id), Some("host_reboot".into()));
        assert_eq!(s.get_session("b", "h").unwrap().unwrap().status, "running");
    }

    #[test]
    fn a_tmux_restart_does_not_mark_background_agents_lost() {
        // one live tmux row + one live `bg` row on "h"
        s.mark_host_sessions_lost("h", "tmux_server_gone", &[], 500).unwrap();
        // tmux row ghosted, bg row untouched
    }

    #[test]
    fn a_reboot_marks_background_agents_lost_too() {
        // same seed; reason "host_reboot" ⇒ both rows ghosted
    }

    #[test]
    fn phase_one_ghosting_records_missing_and_a_resurrection_clears_it() {
        // apply_host_reconcile with an empty keep ⇒ the row is ghost with
        // lost_reason 'missing'; upserting it live again ⇒ lost_reason NULL
    }
```

Add a test-only helper `fn lost_reason_of(s: &Store, id: i64) -> Option<String>` that selects `lost_reason` through `s.conn_ref()`. Fill each commented body with real seeding and assertions following the first test's shape.

- [ ] **Step 2: Run** → compile error.
- [ ] **Step 3: Implement.** Phase 1 SQL becomes `UPDATE sessions SET status='ghost', lost_at=?1, lost_reason='missing'`. Upsert gains `lost_reason=NULL,` directly after `lost_at=NULL,`. `mark_host_sessions_lost` follows `ghost_and_clean_bg_sessions` exactly (own `unchecked_transaction`, collect `RowChange`s, commit, then emit):

```rust
    pub fn mark_host_sessions_lost(
        &self,
        host_alias: &str,
        reason: &str,
        keep_names: &[String],
        now: i64,
    ) -> Result<Vec<SessionRow>, rusqlite::Error> {
        // A tmux restart does not kill `claude --bg` agents; a reboot does.
        let kind_filter = if reason == "tmux_server_gone" { KIND_TMUX } else { "1=1" };
        let not_in = if keep_names.is_empty() {
            String::new()
        } else {
            format!(" AND tmux_name NOT IN ({})", in_clause(keep_names.len()))
        };
        let tx = self.conn.unchecked_transaction()?;
        let sql = format!(
            "UPDATE sessions SET status='ghost', lost_at=?1, lost_reason=?2
             WHERE host_alias=?3 AND status!='ghost' AND {kind_filter}{not_in}
             RETURNING id"
        );
        let head: Vec<&dyn rusqlite::ToSql> = vec![&now, &reason, &host_alias];
        let params = params_then(&head, keep_names);
        let ids: Vec<i64> = tx
            .prepare(&sql)?
            .query_map(params.as_slice(), |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut rows = Vec::new();
        for id in &ids {
            if let Some(row) = fetch_session_by_id(&tx, *id)? {
                rows.push(row);
            }
        }
        tx.commit()?;
        for row in &rows {
            self.bus.emit_change(&RowChange::SessionUpdated(row.clone()));
        }
        Ok(rows)
    }
```

Match `in_clause`, `params_then`, `fetch_session_by_id` and `KIND_TMUX` to how `store/reconcile.rs` imports them; add `use` lines as the compiler asks.

- [ ] **Step 4: Run** the new tests → PASS; full suite.
- [ ] **Step 5: Commit** — `feat(store): record why a session is lost; mark a host's sessions lost in one pass`

---

### Task 5: Resumable mass-loss rows survive the reap until a TTL

**Files:** Modify `crates/fleet-core/src/store/reconcile.rs` (`ghost_and_clean` Phase 2 prep at `:270-281`, `apply_host_reconcile` at `:395`); `crates/fleet-core/src/store/rows.rs` (`HostReconcile`); `crates/fleet-core/src/store/sessions.rs` (`ghost_and_clean_bg_sessions`); `crates/fleet-core/src/service/settings.rs` (new key); `crates/fleet-core/src/service/sessions/reconcile.rs:737` (bg caller); `src/lib/fleet_settings.ts` if it mirrors backend keys.

**Interfaces:**
- Produces: `ghost_and_clean(..., lost_ttl_cutoff: Option<i64>, out)` — Phase 2 never hard-deletes a row with `claude_session_id IS NOT NULL AND lost_reason IN ('host_reboot','tmux_server_gone') AND lost_at >= cutoff`; `None` = no exemption (today's behaviour). `HostReconcile.lost_ttl_cutoff: Option<i64>`; `ghost_and_clean_bg_sessions(host, keep, now, lost_ttl_cutoff: Option<i64>)`; setting key `pub const SESSIONS_LOST_TTL_SECS: &str = "sessions.lost_ttl_secs"`, default `"1209600"` (14 days), `Kind::Secs`.

- [ ] **Step 1: Write the failing tests** (store-level, `store/reconcile.rs` tests; seed ghost rows directly through `conn_ref()` with `status='ghost'`, a `lost_at`, a `lost_reason` and optionally a `claude_session_id`, then run two `apply_host_reconcile` passes with an empty `keep` and `lost_ttl_cutoff: Some(now - 1_209_600)`)

```rust
    #[test]
    fn a_resumable_mass_loss_row_survives_the_reap() { /* host_reboot + claude id ⇒ still present */ }
    #[test]
    fn a_missing_row_is_still_reaped_on_the_next_pass() { /* lost_reason 'missing' ⇒ deleted */ }
    #[test]
    fn a_mass_loss_row_without_a_claude_id_is_reaped() { /* not resumable ⇒ deleted */ }
    #[test]
    fn a_mass_loss_row_older_than_the_ttl_is_reaped() { /* lost_at before cutoff ⇒ deleted */ }
    #[test]
    fn with_no_cutoff_nothing_is_exempt() { /* None ⇒ today's behaviour, deleted */ }
```

Write each body with real seeding and a `get_session_by_id(..).is_some()` / `.is_none()` assertion; the comment in each stub names the exact case.

- [ ] **Step 2: Run** → compile error (no field `lost_ttl_cutoff`).
- [ ] **Step 3: Implement.** Phase 2 prep SQL gains the exclusion when a cutoff is given:

```rust
        let exempt = if lost_ttl_cutoff.is_some() {
            " AND NOT (claude_session_id IS NOT NULL \
                       AND lost_reason IN ('host_reboot','tmux_server_gone') \
                       AND lost_at >= ?2)"
        } else {
            ""
        };
```

Renumber the placeholder so `?2` is the cutoff and `keep_names` follow it (`params_then` over `[&host_alias, &cutoff]`); when `None`, keep today's placeholder layout exactly. Thread `lost_ttl_cutoff` from `HostReconcile` (default `None`) and from `ghost_and_clean_bg_sessions`'s new parameter. Register the setting in `SPECS` beside `RECONCILE_INTERVAL_SECS`; if `src/lib/fleet_settings.ts` mirrors backend keys and a test enforces the mirror, add it there too.

- [ ] **Step 4: Run** the new tests → PASS; full suite.
- [ ] **Step 5: Commit** — `feat(store): keep resumable mass-loss sessions through the reap until a TTL`

---

### Task 6: The mass-loss verdict and branch

**Files:** Modify `crates/fleet-core/src/service/sessions/reconcile.rs` (`reconcile_write_one_host`, `:389`); `crates/fleet-core/src/store/rows.rs` (`HostReconcile.skip_prune`); `crates/fleet-core/src/store/reconcile.rs` (`apply_host_reconcile` honours it). Tests in `service/sessions/tests.rs` (or `service/reconcile_tests.rs`, whichever holds the reconcile end-to-end tests).

**Interfaces:**
- Consumes: `HostProbe.identity` (Task 3), `get_host_identity` / `set_host_identity` (Task 1), `mark_host_sessions_lost` (Task 4), `lost_ttl_cutoff` + setting (Task 5).
- Produces: `pub(super) fn mass_loss_verdict(stored: &StoredIdentity, observed: Option<&HostIdentity>) -> Option<&'static str>`; `HostReconcile.skip_prune: bool` (default `false`).

- [ ] **Step 1: Write the failing tests**

Pure verdict tests:

```rust
    #[test]
    fn verdict_needs_a_readable_identity() {
        let stored = StoredIdentity { boot_id: Some("a".into()), tmux_server_pid: Some(1) };
        assert_eq!(mass_loss_verdict(&stored, None), None);
    }
    #[test]
    fn a_changed_boot_id_is_a_reboot() {
        let stored = StoredIdentity { boot_id: Some("a".into()), tmux_server_pid: Some(1) };
        let obs = HostIdentity { boot_id: Some("b".into()), tmux_server_pid: Some(2) };
        assert_eq!(mass_loss_verdict(&stored, Some(&obs)), Some("host_reboot"));
    }
    #[test]
    fn no_tmux_server_or_a_new_server_pid_means_the_server_is_gone() {
        let stored = StoredIdentity { boot_id: Some("a".into()), tmux_server_pid: Some(1) };
        let none = HostIdentity { boot_id: Some("a".into()), tmux_server_pid: None };
        let new = HostIdentity { boot_id: Some("a".into()), tmux_server_pid: Some(2) };
        assert_eq!(mass_loss_verdict(&stored, Some(&none)), Some("tmux_server_gone"));
        assert_eq!(mass_loss_verdict(&stored, Some(&new)), Some("tmux_server_gone"));
    }
    #[test]
    fn a_first_probe_after_upgrade_is_never_a_verdict() {
        let obs = HostIdentity { boot_id: Some("b".into()), tmux_server_pid: Some(2) };
        assert_eq!(mass_loss_verdict(&StoredIdentity::default(), Some(&obs)), None);
    }
    #[test]
    fn an_unchanged_identity_is_a_normal_pass() {
        let stored = StoredIdentity { boot_id: Some("a".into()), tmux_server_pid: Some(1) };
        let same = HostIdentity { boot_id: Some("a".into()), tmux_server_pid: Some(1) };
        assert_eq!(mass_loss_verdict(&stored, Some(&same)), None);
    }
```

End-to-end through `reconcile_sessions_with` with `ReconcileDeps::fake` returning an `IdentityTmux` (Task 3). These are the **spec's acceptance criteria 1 and 4**:

```rust
#[tokio::test]
async fn a_vanished_tmux_server_keeps_the_rows_lost_with_their_claude_ids() {
    // pass 1: identity {boot a, pid 1}, sessions [x (claude id "cid-x")] ⇒ live
    // pass 2: identity {boot a, pid None}, sessions [] (reachable, no server)
    // assert: row x exists, status ghost, lost_at set, claude_session_id "cid-x",
    //         lost_reason tmux_server_gone
    // pass 3 (same as 2): row x STILL exists — the exemption, not one-cycle reap
    // list_sessions with include_lost shows x
}

#[tokio::test]
async fn a_changed_boot_id_marks_every_session_lost_as_a_reboot() { /* lost_reason host_reboot */ }

#[tokio::test]
async fn an_unreadable_identity_leaves_today_s_behaviour_intact() {
    // identity None on every pass; a session that disappears is ghosted
    // ('missing') and reaped on the following pass, exactly as before
}
```

Write each body with the reconcile harness the existing end-to-end reconcile tests use (store seeding, `ReconcileDeps::fake`, `reconcile_sessions_with`), and assert through `get_session` / the include-lost listing the MCP tool uses.

- [ ] **Step 2: Run** → compile errors / failures.
- [ ] **Step 3: Implement.** Verdict:

```rust
/// Why every session on a reachable host should be treated as lost this
/// pass, or `None` for a normal pass. Each comparison needs BOTH sides
/// known, so a first probe after upgrade or a failed identity read never
/// mass-marks a host.
pub(super) fn mass_loss_verdict(
    stored: &StoredIdentity,
    observed: Option<&crate::tmux::HostIdentity>,
) -> Option<&'static str> {
    let obs = observed?;
    if let (Some(s), Some(o)) = (stored.boot_id.as_deref(), obs.boot_id.as_deref()) {
        if s != o {
            return Some("host_reboot");
        }
    }
    match (stored.tmux_server_pid, obs.tmux_server_pid) {
        (Some(_), None) => Some("tmux_server_gone"),
        (Some(s), Some(o)) if s != o => Some("tmux_server_gone"),
        _ => None,
    }
}
```

Note `(None, None)` is deliberately NOT a verdict: a host whose server was already absent last pass has already been marked, and a host first seen with no server has no stored evidence of loss.

In the `Ok(live)` branch of `reconcile_write_one_host`, after `keep` is computed and before `apply_host_reconcile`:

```rust
            let stored = s.get_host_identity(&host.alias).unwrap_or_default();
            let verdict = mass_loss_verdict(&stored, probe.identity.as_ref());
            if let Some(reason) = verdict {
                match s.mark_host_sessions_lost(&host.alias, reason, &keep, now_unix()) {
                    Ok(rows) => {
                        for row in &rows {
                            if let Err(e) = s.insert_session_event(row.id, "lost", Some(reason)) {
                                tracing::warn!(host = %host.alias, error = %e, "[reconcile] lost event insert failed");
                            }
                        }
                    }
                    Err(e) => tracing::warn!(host = %host.alias, error = %e, "[reconcile] mark lost failed"),
                }
            }
            if let Some(id) = &probe.identity {
                if let Err(e) = s.set_host_identity(&host.alias, id.boot_id.as_deref(), id.tmux_server_pid) {
                    tracing::warn!(host = %host.alias, error = %e, "[reconcile] identity write failed");
                }
            }
```

Pass `skip_prune: verdict.is_some()` and `lost_ttl_cutoff: Some(now_unix() - ttl)` into `HostReconcile`, with `ttl` read the way `read_reconcile_interval_secs` reads its key (`s.get_setting(SESSIONS_LOST_TTL_SECS)`, `settings::resolve`, parse, fall back to `1_209_600`). Pass the same cutoff to `ghost_and_clean_bg_sessions` at `:737`. In `apply_host_reconcile`, wrap the `ghost_and_clean` call in `if !spec.skip_prune { .. }`.

- [ ] **Step 4: Run** the new tests → PASS; full suite (every existing reconcile test must pass unmodified — they use no identity, so they must take the no-verdict path).
- [ ] **Step 5: Commit** — `feat(reconcile): a vanished tmux server or a reboot marks sessions lost, not deleted`

---

### Task 7: Lifecycle logging (R5)

**Files:** Modify `crates/fleet-core/src/store/reconcile.rs` (the post-commit flush in `apply_host_reconcile`, `:408`), `crates/fleet-core/src/store/sessions.rs` (`mark_host_sessions_lost` flush).

**Interfaces:**
- Produces: `pub(crate) fn lifecycle_kind(change: &RowChange) -> Option<&'static str>` — `SessionCreated` ⇒ `"created"`, `SessionUpdated` of a row with `lost_at.is_some()` ⇒ `"lost"`, `SessionKilled` ⇒ `"deleted"`, everything else ⇒ `None`.

- [ ] **Step 1: Write the failing test** covering each `RowChange` arm above plus a `SessionUpdated` of a live row (⇒ `None`) and a non-session change such as `HostProbed` (⇒ `None`).
- [ ] **Step 2: Run** → compile error.
- [ ] **Step 3: Implement** the pure function, then in each flush loop emit, for every change with a kind:

```rust
tracing::info!(
    lifecycle = kind,
    host_alias = %row.host_alias,
    tmux_name = %row.tmux_name,
    claude_session_id = row.claude_session_id.as_deref().unwrap_or("-"),
    "[session] {kind}"
);
```

For `SessionKilled(id)` only the id survives the delete; log `session_id = id` and the host alias the flush already has. `mark_host_sessions_lost` additionally logs its `reason`.

- [ ] **Step 4: Run** → PASS; full suite.
- [ ] **Step 5: Commit** — `feat(reconcile): log session lifecycle transitions at INFO`

---

### Task 8: Verification and PR

- [ ] **Step 1:** `git fetch origin` and confirm `git rev-list --count HEAD..origin/main` is `0`; if not, merge `origin/main` into the branch and re-run everything below.
- [ ] **Step 2:** `scripts/ci-local.sh`, judged with `grep -E "==>|test result|advisories|Tests  |all selected|FAILED|^error"`, never through `tail`.
- [ ] **Step 3:** Spec acceptance criterion 3 (kill a real tmux server, rows go lost not deleted) is a **manual** check on a live host, stated in the PR description as not yet run unless the operator runs it. Do not claim it.
- [ ] **Step 4:** `git push -u origin feat/host-reboot-survival`, then `gh pr create --base main` with a description listing the three deliberate deviations above and what PR 2 will add.
