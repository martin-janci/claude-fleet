# Account model Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make "claude account" a first-class entity in claude-fleet — auto-detected via probe from each host's `~/.claude.json`, normalized into an `accounts` table with FK from `hosts`, and surfaced in 3 UI places (Settings dialog Account column, host pill tooltip, SessionDetails Account row + AddHostPicker preview).

**Architecture:** Migration 003 adds `accounts` table + `hosts.account_uuid` FK. Probe v2 extends the iter-1 bash script with a third section that reads `~/.claude.json | jq -c .oauthAccount` (python3 fallback) — one extra ~50ms section, no new round trips. `add_host` and `probe_host` upsert into accounts and link the host's FK. Frontend gets a `accounts.ts` store that components consume via a `$derived` Map keyed by uuid (no SQL JOIN — TS-side lookup keeps stores independent and testable).

**Tech Stack:** Rust + Tauri 2 backend, Svelte 5 (runes) frontend, SQLite via rusqlite, serde for JSON parsing.

**Spec:** `docs/specs/2026-05-20-account-model-design.md`

---

## File Structure

**Created:**
- `src-tauri/migrations/003_accounts.sql` — schema (new table + ALTER hosts)
- `src/lib/accounts.ts` — frontend store + IPC wrapper
- `src/lib/accounts.test.ts` — store tests

**Modified:**
- `src-tauri/src/store.rs` — `AccountRow` struct + 4 helpers + migrate() applies 003
- `src-tauri/src/commands/health.rs` — bump expected schema_version 2 → 3
- `src-tauri/src/commands/hosts.rs` — probe v2 (oauthAccount); account link in add_host + probe_host; new `list_accounts` + `probe_ssh_alias` Tauri commands
- `src-tauri/src/lib.rs` — register `list_accounts` + `probe_ssh_alias`
- `src/lib/hosts.ts` — `HostRow` interface gains `account_uuid: string | null`
- `vitest.setup.ts` — mock `list_accounts` + `probe_ssh_alias`
- `src/lib/Sidebar.svelte` — load accounts on mount; host pill tooltip includes account info
- `src/lib/SettingsDialog.svelte` — Account column between claude and Status
- `src/lib/SettingsDialog.test.ts` — 2 new tests
- `src/lib/AddHostPicker.svelte` — split click into probe-preview → confirm-add
- `src/lib/AddHostPicker.test.ts` — 2 new tests
- `src/lib/SessionDetails.svelte` — Host + Account rows above Project
- `src/lib/SessionDetails.test.ts` if exists, else create — 1 new test

---

## Task 1: Migration 003 + schema bump

**Files:**
- Create: `src-tauri/migrations/003_accounts.sql`
- Modify: `src-tauri/src/store.rs` — extend `migrate()` + rename idempotency tests
- Modify: `src-tauri/src/commands/health.rs` — bump expected `schema_version`

- [ ] **Step 1: Write the failing test in `src-tauri/src/store.rs` `mod tests`**

Append inside the existing `mod tests` block:

```rust
    #[test]
    fn migration_003_adds_accounts_table_and_host_account_uuid_column() {
        let s = Store::open_in_memory().expect("open");
        // accounts table exists
        assert!(s.has_table("accounts").expect("has_table"), "expected accounts table");
        // hosts.account_uuid column exists
        let mut stmt = s
            .conn
            .prepare("SELECT name FROM pragma_table_info('hosts')")
            .unwrap();
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            cols.iter().any(|c| c == "account_uuid"),
            "expected `account_uuid` column on hosts; got: {cols:?}"
        );
    }

    #[test]
    fn schema_version_is_three_after_migration() {
        let s = Store::open_in_memory().expect("open");
        assert_eq!(s.schema_version().expect("version"), 3);
    }
```

- [ ] **Step 2: Update the existing `schema_version_is_two` test**

Rename from `schema_version_is_two` to `schema_version_is_three` and update body:

```rust
    #[test]
    fn schema_version_is_three() {
        let store = Store::open_in_memory().expect("open");
        assert_eq!(store.schema_version().expect("version"), 3);
    }
```

Also update `migrate_is_idempotent` to assert `== 3`:

```rust
    #[test]
    fn migrate_is_idempotent() {
        let store = Store::open_in_memory().expect("open");
        store.migrate().expect("re-migrate");
        assert_eq!(store.schema_version().expect("version"), 3);
    }
```

- [ ] **Step 3: Run tests, expect failures**

```bash
cd /Users/martinjanci/projects/github.com/martin-janci/claude-fleet
cargo test --manifest-path src-tauri/Cargo.toml --lib store::tests 2>&1 | tail -20
```

Expected: 3 failures — both new tests fail (no accounts table, no account_uuid column, version is 2), and `migrate_is_idempotent` fails (still gets 2).

- [ ] **Step 4: Create `src-tauri/migrations/003_accounts.sql`**

```sql
-- Migration 003: normalized accounts + per-host account FK.
-- Step 2 of the multi-host iteration (account model). Auto-populated from
-- each host's ~/.claude.json oauthAccount during probe. Nullable FK on
-- hosts so a host without claude installed or logged in stays usable.

CREATE TABLE IF NOT EXISTS accounts (
  uuid                TEXT PRIMARY KEY,
  email               TEXT,
  display_name        TEXT,
  organization_name   TEXT,
  organization_uuid   TEXT,
  seat_tier           TEXT,
  last_seen_at        INTEGER
);

ALTER TABLE hosts ADD COLUMN account_uuid TEXT REFERENCES accounts(uuid);

INSERT OR IGNORE INTO schema_version (version) VALUES (3);
```

- [ ] **Step 5: Update `src-tauri/src/store.rs::migrate()` to apply 003 conditionally**

Replace the existing `migrate` body:

```rust
    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        self.conn
            .execute_batch(include_str!("../migrations/001_init.sql"))?;
        let v: i64 = self
            .conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap_or(0);
        if v < 2 {
            self.conn
                .execute_batch(include_str!("../migrations/002_hosts_ssh.sql"))?;
        }
        if v < 3 {
            self.conn
                .execute_batch(include_str!("../migrations/003_accounts.sql"))?;
        }
        Ok(())
    }
```

Note: the `v < 2` clause runs the 002 ALTER even if we just landed at v=1 from a fresh 001. After 002 applies, version is 2. We then check `v < 3` against the ORIGINAL `v` (still 1 in this code path), so 003 also runs. That's the intended behavior — fresh DBs cascade through all migrations in order.

- [ ] **Step 6: Update `src-tauri/src/commands/health.rs` test expectation**

Find the existing assertion:

```rust
        assert_eq!(h.schema_version, 2);
```

Change to:

```rust
        assert_eq!(h.schema_version, 3);
```

- [ ] **Step 7: Run all store + health tests**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib store:: 2>&1 | tail -20
cargo test --manifest-path src-tauri/Cargo.toml --lib commands::health:: 2>&1 | tail -5
```

Expected: all pass.

- [ ] **Step 8: Run full lib suite to make sure nothing else broke**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib 2>&1 | tail -5
```

Expected: `test result: ok. 69+ passed; 0 failed` (was 67 in iter 1; +2 new schema tests).

- [ ] **Step 9: Commit**

```bash
git add src-tauri/migrations/003_accounts.sql src-tauri/src/store.rs src-tauri/src/commands/health.rs
git commit -m "store: migration 003 (accounts table + hosts.account_uuid FK)"
```

---

## Task 2: AccountRow + Store helpers

**Files:**
- Modify: `src-tauri/src/store.rs` — add `AccountRow` struct + 4 helper methods + tests

- [ ] **Step 1: Add `AccountRow` struct near `HostRow`**

```rust
#[derive(Debug, Clone, serde::Serialize)]
pub struct AccountRow {
    pub uuid: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub organization_name: Option<String>,
    pub organization_uuid: Option<String>,
    pub seat_tier: Option<String>,
    pub last_seen_at: Option<i64>,
}
```

- [ ] **Step 2: Add 4 helper methods inside `impl Store`**

Append after the existing host helpers:

```rust
    pub fn list_accounts(&self) -> Result<Vec<AccountRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
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
        Ok(())
    }

    pub fn get_account_by_uuid(
        &self,
        uuid: &str,
    ) -> Result<Option<AccountRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
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
        Ok(())
    }
```

- [ ] **Step 3: Update `list_hosts` to include `account_uuid`**

Find the existing `list_hosts` SQL. It currently selects 7 cols. Add `account_uuid` as col 8:

```rust
    pub fn list_hosts(&self) -> Result<Vec<HostRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT alias, ssh_alias, reachable, claude_version, tmux_version, hidden,
                    last_pinged_at, account_uuid
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
            })
        })?;
        rows.collect()
    }
```

Also extend the `HostRow` struct (above) to include the new field:

```rust
#[derive(Debug, Clone, serde::Serialize)]
pub struct HostRow {
    pub alias: String,
    pub ssh_alias: Option<String>,
    pub reachable: bool,
    pub claude_version: Option<String>,
    pub tmux_version: Option<String>,
    pub hidden: bool,
    pub last_pinged_at: Option<i64>,
    pub account_uuid: Option<String>,
}
```

- [ ] **Step 4: Add 6 tests in the existing `mod tests` block**

```rust
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
        // Update with new email + last_seen_at
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
        let row = s.list_hosts().unwrap().into_iter().find(|r| r.alias == "h").unwrap();
        assert_eq!(row.account_uuid.as_deref(), Some("u1"));
        s.set_host_account("h", None).unwrap();
        let row = s.list_hosts().unwrap().into_iter().find(|r| r.alias == "h").unwrap();
        assert!(row.account_uuid.is_none());
    }

    #[test]
    fn list_hosts_includes_account_uuid_in_output() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h", Some("h")).unwrap();
        let row = s.list_hosts().unwrap().into_iter().find(|r| r.alias == "h").unwrap();
        // Newly inserted host has no account yet.
        assert!(row.account_uuid.is_none());
    }
```

- [ ] **Step 5: Run tests**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib store:: 2>&1 | tail -15
```

Expected: 6 new tests pass, plus all existing tests.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/store.rs
git commit -m "store: AccountRow + CRUD (list/upsert/get_by_uuid/set_host_account)"
```

---

## Task 3: Probe v2 — fetch oauthAccount

**Files:**
- Modify: `src-tauri/src/commands/hosts.rs`

- [ ] **Step 1: Add `OauthAccount` struct + parser at the bottom of `hosts.rs`**

Just below `parse_claude_version`, add:

```rust
/// Subset of `~/.claude.json`'s `oauthAccount` we care about. All fields
/// optional so a partial JSON shape (e.g., older claude versions, missing
/// org fields) still parses cleanly.
#[derive(serde::Deserialize, Default, Debug, Clone)]
pub struct OauthAccount {
    #[serde(rename = "accountUuid")]
    pub uuid: Option<String>,
    #[serde(rename = "emailAddress")]
    pub email: Option<String>,
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    #[serde(rename = "organizationName")]
    pub organization_name: Option<String>,
    #[serde(rename = "organizationUuid")]
    pub organization_uuid: Option<String>,
    #[serde(rename = "seatTier")]
    pub seat_tier: Option<String>,
}

/// Parse the third probe section. Empty / "null" / "{}" → None.
/// Treats account-without-uuid as None (we use uuid as PK).
fn parse_oauth_account(line: &str) -> Option<OauthAccount> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed == "{}" || trimmed == "null" {
        return None;
    }
    serde_json::from_str::<OauthAccount>(trimmed)
        .ok()
        .filter(|a| a.uuid.is_some())
}
```

- [ ] **Step 2: Update probe script + return shape**

Replace the `probe` function with:

```rust
/// Strict probe — returns Err(E_PROBE) if the SSH round trip fails. Used by
/// add_host. Reads tmux + claude versions AND the oauthAccount in a single
/// round trip (sections separated by literal `---`).
fn probe(
    ssh: &Arc<SshClient>,
    host: &str,
) -> Result<(bool, Option<String>, Option<String>, Option<OauthAccount>), IpcError> {
    let script = r#"tmux -V 2>/dev/null || true
echo ---
claude --version 2>/dev/null || true
echo ---
( cat "$HOME/.claude.json" 2>/dev/null | jq -c .oauthAccount 2>/dev/null \
  || python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(json.dumps(d.get("oauthAccount") or {}))' "$HOME/.claude.json" 2>/dev/null \
  || true )"#;
    let out = ssh
        .run(host, &["bash", "-lc", script], Duration::from_secs(5))
        .map_err(|e| IpcError::new("E_PROBE", format!("ssh {host}: {}", e.message)))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(IpcError::new(
            "E_PROBE",
            format!("ssh {host} exited {:?}: {}", out.status.code(), stderr.trim()),
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut parts = stdout.split("---");
    let tmux_line = parts.next().unwrap_or("").trim().to_string();
    let claude_line = parts.next().unwrap_or("").trim().to_string();
    let oauth_line = parts.next().unwrap_or("").trim().to_string();
    Ok((
        true,
        parse_claude_version(&claude_line),
        parse_tmux_version(&tmux_line),
        parse_oauth_account(&oauth_line),
    ))
}

fn probe_lenient(
    ssh: &Arc<SshClient>,
    host: &str,
) -> (bool, Option<String>, Option<String>, Option<OauthAccount>) {
    match probe(ssh, host) {
        Ok(v) => v,
        Err(_) => (false, None, None, None),
    }
}
```

- [ ] **Step 3: Update `probe_local` to read the file directly**

Replace:

```rust
fn probe_local() -> (bool, Option<String>, Option<String>, Option<OauthAccount>) {
    let tmux = std::process::Command::new("tmux")
        .arg("-V")
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        });
    let claude = std::process::Command::new("claude")
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        });
    // Read local ~/.claude.json directly — no subprocess needed.
    let account = std::env::var("HOME").ok().and_then(|home| {
        let path = std::path::Path::new(&home).join(".claude.json");
        let contents = std::fs::read_to_string(path).ok()?;
        let v: serde_json::Value = serde_json::from_str(&contents).ok()?;
        let oa = v.get("oauthAccount")?;
        serde_json::from_value::<OauthAccount>(oa.clone())
            .ok()
            .filter(|a| a.uuid.is_some())
    });
    (
        true,
        parse_claude_version(claude.as_deref().unwrap_or("")),
        parse_tmux_version(tmux.as_deref().unwrap_or("")),
        account,
    )
}
```

- [ ] **Step 4: Update callers to handle the 4-tuple**

Find both call sites:
- `add_host` calls `probe(...)?`
- `probe_host` calls `probe_local()` or `probe_lenient(...)`

Update destructuring from `(reachable, claude_ver, tmux_ver)` to `(reachable, claude_ver, tmux_ver, account)`.

In `add_host`:

```rust
    let (reachable, claude_ver, tmux_ver, account) = probe(&ssh, &args.ssh_alias)?;
    {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        s.insert_host(&args.alias, Some(&args.ssh_alias))?;
        // Link account if probe found one
        if let Some(acc) = account.as_ref().and_then(|a| account_row_from(a, now_unix())) {
            s.upsert_account(&acc)?;
            s.set_host_account(&args.alias, Some(&acc.uuid))?;
        } else {
            s.set_host_account(&args.alias, None)?;
        }
        s.update_host_probe(
            &args.alias,
            reachable,
            claude_ver.as_deref(),
            tmux_ver.as_deref(),
            now_unix(),
        )?;
    }
    list_one(&store, &args.alias)
```

In `probe_host`:

```rust
    let (reachable, claude_ver, tmux_ver, account) = if args.alias == "local" {
        probe_local()
    } else {
        probe_lenient(&ssh, target)
    };
    {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        if let Some(acc) = account.as_ref().and_then(|a| account_row_from(a, now_unix())) {
            s.upsert_account(&acc)?;
            s.set_host_account(&args.alias, Some(&acc.uuid))?;
        } else {
            s.set_host_account(&args.alias, None)?;
        }
        s.update_host_probe(
            &args.alias,
            reachable,
            claude_ver.as_deref(),
            tmux_ver.as_deref(),
            now_unix(),
        )?;
    }
    list_one(&store, &args.alias)
```

- [ ] **Step 5: Add helper `account_row_from` at the bottom of `hosts.rs`**

```rust
/// Convert a probed `OauthAccount` into a storable `AccountRow`, dropping
/// records without a uuid (can't be primary-keyed).
fn account_row_from(a: &OauthAccount, now: i64) -> Option<crate::store::AccountRow> {
    let uuid = a.uuid.clone()?;
    Some(crate::store::AccountRow {
        uuid,
        email: a.email.clone(),
        display_name: a.display_name.clone(),
        organization_name: a.organization_name.clone(),
        organization_uuid: a.organization_uuid.clone(),
        seat_tier: a.seat_tier.clone(),
        last_seen_at: Some(now),
    })
}
```

- [ ] **Step 6: Add 4 unit tests for `parse_oauth_account`**

In the existing `#[cfg(test)] mod tests` block at the bottom of `hosts.rs`:

```rust
    #[test]
    fn parse_oauth_account_handles_full_json() {
        let line = r#"{"accountUuid":"abc","emailAddress":"a@b.com","displayName":"A B","organizationName":"32bit","organizationUuid":"org-1","seatTier":"max"}"#;
        let a = parse_oauth_account(line).unwrap();
        assert_eq!(a.uuid.as_deref(), Some("abc"));
        assert_eq!(a.email.as_deref(), Some("a@b.com"));
        assert_eq!(a.display_name.as_deref(), Some("A B"));
        assert_eq!(a.organization_name.as_deref(), Some("32bit"));
        assert_eq!(a.organization_uuid.as_deref(), Some("org-1"));
        assert_eq!(a.seat_tier.as_deref(), Some("max"));
    }

    #[test]
    fn parse_oauth_account_tolerates_missing_optional_fields() {
        let line = r#"{"accountUuid":"abc","emailAddress":"a@b.com"}"#;
        let a = parse_oauth_account(line).unwrap();
        assert_eq!(a.uuid.as_deref(), Some("abc"));
        assert_eq!(a.email.as_deref(), Some("a@b.com"));
        assert!(a.display_name.is_none());
        assert!(a.organization_name.is_none());
        assert!(a.seat_tier.is_none());
    }

    #[test]
    fn parse_oauth_account_returns_none_for_empty_or_null_or_empty_obj() {
        assert!(parse_oauth_account("").is_none());
        assert!(parse_oauth_account("   ").is_none());
        assert!(parse_oauth_account("{}").is_none());
        assert!(parse_oauth_account("null").is_none());
    }

    #[test]
    fn parse_oauth_account_returns_none_when_uuid_missing() {
        let line = r#"{"emailAddress":"a@b.com","seatTier":"max"}"#;
        assert!(parse_oauth_account(line).is_none());
    }

    #[test]
    fn parse_oauth_account_returns_none_for_malformed_json() {
        assert!(parse_oauth_account("{not-json").is_none());
        assert!(parse_oauth_account("not even an object").is_none());
    }
```

- [ ] **Step 7: Run tests**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib commands::hosts:: 2>&1 | tail -15
```

Expected: existing 2 tests pass + 5 new `parse_oauth_account` tests pass.

- [ ] **Step 8: Commit**

```bash
git add src-tauri/src/commands/hosts.rs
git commit -m "hosts: probe v2 — fetch oauthAccount in single round trip"
```

---

## Task 4: list_accounts + probe_ssh_alias Tauri commands

**Files:**
- Modify: `src-tauri/src/commands/hosts.rs` — add 2 new commands
- Modify: `src-tauri/src/lib.rs` — register them

- [ ] **Step 1: Add `list_accounts` command in `hosts.rs`**

Add near the existing `list_hosts` command:

```rust
#[tauri::command]
pub fn list_accounts(store: State<'_, Mutex<Store>>) -> Result<Vec<crate::store::AccountRow>, IpcError> {
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.list_accounts().map_err(IpcError::from)
}
```

- [ ] **Step 2: Add `probe_ssh_alias` command in `hosts.rs`**

Add near `add_host`:

```rust
/// Preview-only probe used by AddHostPicker before the user confirms `Add`.
/// Does NOT persist anything; just runs the strict probe and returns versions
/// + the detected account so the picker can show it for confirmation.
#[derive(serde::Serialize)]
pub struct ProbePreview {
    pub reachable: bool,
    pub claude_version: Option<String>,
    pub tmux_version: Option<String>,
    pub account: Option<OauthAccount>,
}

#[derive(Deserialize)]
pub struct ProbeSshAliasArgs {
    pub ssh_alias: String,
}

#[tauri::command]
pub fn probe_ssh_alias(
    args: ProbeSshAliasArgs,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<ProbePreview, IpcError> {
    let (reachable, claude_version, tmux_version, account) = probe(&ssh, &args.ssh_alias)?;
    Ok(ProbePreview {
        reachable,
        claude_version,
        tmux_version,
        account,
    })
}
```

Also add `#[derive(serde::Serialize)]` to `OauthAccount` so it can cross the IPC boundary:

```rust
#[derive(serde::Deserialize, serde::Serialize, Default, Debug, Clone)]
pub struct OauthAccount { ... }
```

- [ ] **Step 3: Register both commands in `src-tauri/src/lib.rs`**

Find the `generate_handler![...]` block in `run()`. Add:

```rust
            commands::hosts::list_accounts,
            commands::hosts::probe_ssh_alias,
```

(Insert near the existing `commands::hosts::*` lines.)

- [ ] **Step 4: Build + test**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib 2>&1 | tail -8
```

Expected: clean build, all tests pass (no new tests for the commands themselves — they're thin wrappers; the logic was tested in Task 3).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/commands/hosts.rs src-tauri/src/lib.rs
git commit -m "hosts: list_accounts + probe_ssh_alias Tauri commands"
```

---

## Task 5: Frontend accounts.ts store

**Files:**
- Create: `src/lib/accounts.ts`
- Create: `src/lib/accounts.test.ts`
- Modify: `src/lib/hosts.ts` — add `account_uuid: string | null` to `HostRow`
- Modify: `vitest.setup.ts` — mock `list_accounts` + `probe_ssh_alias`

- [ ] **Step 1: Create `src/lib/accounts.ts`**

```typescript
import { writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';

export interface AccountRow {
  uuid: string;
  email: string | null;
  display_name: string | null;
  organization_name: string | null;
  organization_uuid: string | null;
  seat_tier: string | null;
  last_seen_at: number | null;
}

export const accounts = writable<AccountRow[]>([]);

export async function loadAccounts(): Promise<Result<AccountRow[]>> {
  const r = await invokeCmd<AccountRow[]>('list_accounts');
  if (r.ok) accounts.set(r.value);
  return r;
}

/// Used by AddHostPicker to preview probe results without persisting.
export interface ProbePreview {
  reachable: boolean;
  claude_version: string | null;
  tmux_version: string | null;
  account: {
    uuid: string | null;
    email: string | null;
    display_name: string | null;
    organization_name: string | null;
    organization_uuid: string | null;
    seat_tier: string | null;
  } | null;
}

export async function probeSshAlias(sshAlias: string): Promise<Result<ProbePreview>> {
  return invokeCmd<ProbePreview>('probe_ssh_alias', { args: { ssh_alias: sshAlias } });
}
```

- [ ] **Step 2: Update `src/lib/hosts.ts` HostRow interface**

Find the existing `HostRow` interface and add `account_uuid`:

```typescript
export interface HostRow {
  alias: string;
  ssh_alias: string | null;
  reachable: boolean;
  claude_version: string | null;
  tmux_version: string | null;
  hidden: boolean;
  last_pinged_at: number | null;
  account_uuid: string | null;
}
```

- [ ] **Step 3: Update `vitest.setup.ts` mocks**

In the existing invoke mock body, append before the final `return null`:

```ts
    if (cmd === 'list_accounts') return [];
    if (cmd === 'probe_ssh_alias') return {
      reachable: true,
      claude_version: '2.1.144',
      tmux_version: '3.6a',
      account: null,
    };
```

Also update the existing `list_hosts` mock to include the new field (the new HostRow type requires `account_uuid`):

```ts
    if (cmd === 'list_hosts') return [{
      alias: 'local',
      ssh_alias: null,
      reachable: true,
      claude_version: '2.1.145',
      tmux_version: '3.5a',
      hidden: false,
      last_pinged_at: 1,
      account_uuid: null,
    }];
```

- [ ] **Step 4: Create `src/lib/accounts.test.ts`**

```typescript
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { accounts, loadAccounts, probeSshAlias } from './accounts';

const sample = {
  uuid: 'u1',
  email: 'a@b.com',
  display_name: 'A B',
  organization_name: '32bit',
  organization_uuid: 'org-1',
  seat_tier: 'max',
  last_seen_at: 1000,
};

beforeEach(() => {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
  accounts.set([]);
});

describe('accounts store', () => {
  it('loadAccounts populates the store on success', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce([sample]);
    const r = await loadAccounts();
    expect(r.ok).toBe(true);
    expect(get(accounts)).toHaveLength(1);
    expect(get(accounts)[0].uuid).toBe('u1');
  });

  it('loadAccounts handles empty list', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce([]);
    const r = await loadAccounts();
    expect(r.ok).toBe(true);
    expect(get(accounts)).toHaveLength(0);
  });

  it('probeSshAlias passes ssh_alias and returns preview', async () => {
    const preview = {
      reachable: true,
      claude_version: '2.1.144',
      tmux_version: '3.6a',
      account: { uuid: 'u1', email: 'a@b.com', display_name: 'A B', organization_name: null, organization_uuid: null, seat_tier: 'max' },
    };
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(preview);
    const r = await probeSshAlias('mefistos');
    expect(r.ok).toBe(true);
    expect((mockedInvoke as ReturnType<typeof vi.fn>).mock.calls[0]).toEqual([
      'probe_ssh_alias',
      { args: { ssh_alias: 'mefistos' } },
    ]);
    if (r.ok) {
      expect(r.value.account?.uuid).toBe('u1');
      expect(r.value.tmux_version).toBe('3.6a');
    }
  });

  it('probeSshAlias handles probe failure', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockRejectedValueOnce({ code: 'E_PROBE', message: 'unreachable' });
    const r = await probeSshAlias('bad-host');
    expect(r.ok).toBe(false);
  });

  it('store is empty after reset (beforeEach hygiene)', () => {
    expect(get(accounts)).toHaveLength(0);
  });
});
```

- [ ] **Step 5: Run tests**

```bash
cd /Users/martinjanci/projects/github.com/martin-janci/claude-fleet && pnpm vitest run src/lib/accounts.test.ts 2>&1 | tail -10
```

Expected: 5 tests pass.

- [ ] **Step 6: Run full vitest to verify the HostRow shape update didn't break existing tests**

```bash
pnpm vitest run 2>&1 | tail -10
```

Expected: all tests pass (existing tests need the new `account_uuid: null` field in their fixtures — check Sidebar.test.ts and SettingsDialog.test.ts; the mock update in vitest.setup.ts covers the global helper, but inline fixtures within test files may need patching).

If any tests fail due to missing `account_uuid` in inline HostRow literals, add `account_uuid: null` to each:

```bash
grep -rn "alias: 'local'" src/lib/ | grep -v node_modules
```

Patch each occurrence to include `account_uuid: null` in the HostRow object.

- [ ] **Step 7: Commit**

```bash
git add src/lib/accounts.ts src/lib/accounts.test.ts src/lib/hosts.ts vitest.setup.ts
git commit -m "accounts: frontend store + probe_ssh_alias wrapper + HostRow.account_uuid"
```

---

## Task 6: Sidebar host pill tooltip + load accounts

**Files:**
- Modify: `src/lib/Sidebar.svelte` — import accounts; load on mount; extend host pill tooltip
- Modify: `src/lib/Sidebar.test.ts` — 1 new test

- [ ] **Step 1: Update `Sidebar.svelte` imports + onMount**

In the existing imports block, add:

```ts
  import { accounts, loadAccounts, type AccountRow } from './accounts';
```

In the existing `onMount` block, append a `loadAccounts()` call after `loadHosts`:

```ts
  onMount(async () => {
    const pr = await loadProjects();
    if (!pr.ok) loadError = pr.error.message;
    const sr = await loadSessions();
    if (!sr.ok) loadError = sr.error.message;
    const hr = await loadHosts();
    if (!hr.ok) loadError = hr.error.message;
    const ar = await loadAccounts();
    if (!ar.ok) loadError = ar.error.message;
  });
```

- [ ] **Step 2: Add `$derived` accountByUuid map**

Below the existing `$derived` declarations (e.g., `filtered`, `collidingRepos`), add:

```ts
  // Lookup map for tooltips + components that resolve a host's account.
  const accountByUuid = $derived(
    new Map<string, AccountRow>($accounts.map((a) => [a.uuid, a])),
  );

  function accountLabel(host: { account_uuid: string | null }): string {
    if (!host.account_uuid) return '';
    const acc = accountByUuid.get(host.account_uuid);
    if (!acc) return `\n${host.account_uuid}`;
    const email = acc.email ?? acc.uuid;
    return acc.seat_tier ? `\n${email} (${acc.seat_tier})` : `\n${email}`;
  }
```

- [ ] **Step 3: Extend the host pill tooltip**

Find the existing host pill `<button>` with the `title` attribute. It currently looks like:

```svelte
title="{h.alias}{h.tmux_version ? ` · tmux ${h.tmux_version}` : ''}{h.claude_version ? ` · claude ${h.claude_version}` : ''}"
```

Replace with (note the multi-line title via template literal):

```svelte
title={`${h.alias}${h.tmux_version ? ` · tmux ${h.tmux_version}` : ''}${h.claude_version ? ` · claude ${h.claude_version}` : ''}${accountLabel(h)}`}
```

- [ ] **Step 4: Add a test in `Sidebar.test.ts`**

In the existing `describe('Sidebar (sessions-grouped view)')` block, append:

```typescript
  it('host pill tooltip includes account info when present', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_projects') return fakeProjects;
      if (cmd === 'list_sessions') return [];
      if (cmd === 'list_hosts') return [
        {
          alias: 'mefistos',
          ssh_alias: 'mefistos',
          reachable: true,
          claude_version: '2.1.144',
          tmux_version: '3.6a',
          hidden: false,
          last_pinged_at: 1,
          account_uuid: 'u1',
        },
      ];
      if (cmd === 'list_accounts') return [
        {
          uuid: 'u1',
          email: 'm.janci@32bit.sk',
          display_name: 'Martin Janci',
          organization_name: '32bit',
          organization_uuid: 'org-1',
          seat_tier: 'max',
          last_seen_at: 1,
        },
      ];
      return null;
    });
    render(Sidebar);
    await tick(); await tick(); await tick(); await tick();
    const pills = document.querySelectorAll('.hosts .pill');
    const mef = Array.from(pills).find((p) => p.textContent?.includes('mefistos'));
    expect(mef).toBeDefined();
    expect(mef!.getAttribute('title')).toContain('m.janci@32bit.sk');
    expect(mef!.getAttribute('title')).toContain('max');
  });

  it('host pill tooltip omits account info when host has no account', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_projects') return fakeProjects;
      if (cmd === 'list_sessions') return [];
      if (cmd === 'list_hosts') return [
        {
          alias: 'noaccount',
          ssh_alias: 'noaccount',
          reachable: true,
          claude_version: '2.1.144',
          tmux_version: '3.6a',
          hidden: false,
          last_pinged_at: 1,
          account_uuid: null,
        },
      ];
      if (cmd === 'list_accounts') return [];
      return null;
    });
    render(Sidebar);
    await tick(); await tick(); await tick(); await tick();
    const pills = document.querySelectorAll('.hosts .pill');
    const noaccount = Array.from(pills).find((p) => p.textContent?.includes('noaccount'));
    expect(noaccount).toBeDefined();
    const title = noaccount!.getAttribute('title') ?? '';
    expect(title).not.toContain('@');
    expect(title).not.toContain('(max)');
  });
```

- [ ] **Step 5: Run tests**

```bash
pnpm vitest run src/lib/Sidebar.test.ts 2>&1 | tail -10
```

Expected: all sidebar tests pass (including 2 new).

- [ ] **Step 6: Commit**

```bash
git add src/lib/Sidebar.svelte src/lib/Sidebar.test.ts
git commit -m "Sidebar: load accounts; host pill tooltip surfaces account"
```

---

## Task 7: SettingsDialog Account column

**Files:**
- Modify: `src/lib/SettingsDialog.svelte` — add Account column
- Modify: `src/lib/SettingsDialog.test.ts` — 2 new tests

- [ ] **Step 1: Update `SettingsDialog.svelte` imports + accountByUuid derived**

Add to the existing imports:

```ts
  import { accounts, type AccountRow } from './accounts';
```

(`accounts` store was already populated by Sidebar's onMount — SettingsDialog just consumes.)

Add the derived map near the existing top-level declarations:

```ts
  const accountByUuid = $derived(
    new Map<string, AccountRow>($accounts.map((a) => [a.uuid, a])),
  );

  function accountCell(h: { account_uuid: string | null }): string {
    if (!h.account_uuid) return '—';
    const acc = accountByUuid.get(h.account_uuid);
    if (!acc) return h.account_uuid;
    const email = acc.email ?? acc.uuid;
    return acc.seat_tier ? `${email} (${acc.seat_tier})` : email;
  }
```

- [ ] **Step 2: Update the table header**

Find the existing `<thead><tr>...</tr></thead>` block. Insert `<th>Account</th>` between `<th>claude</th>` and `<th>Status</th>`:

```svelte
        <thead>
          <tr>
            <th>Alias</th>
            <th>tmux</th>
            <th>claude</th>
            <th>Account</th>
            <th>Status</th>
            <th></th>
          </tr>
        </thead>
```

- [ ] **Step 3: Update the table body cells**

Find the existing `<tbody>` `<tr>` loop. Insert `<td class="account">{accountCell(h)}</td>` between the `claude_version` cell and the `Status` cell:

```svelte
            <tr class:hidden-row={h.hidden}>
              <td class="alias">{h.alias}{#if h.ssh_alias && h.ssh_alias !== h.alias}<span class="muted"> ({h.ssh_alias})</span>{/if}</td>
              <td>{h.tmux_version ?? '—'}</td>
              <td>{h.claude_version ?? '—'}</td>
              <td class="account" data-testid="account-cell">{accountCell(h)}</td>
              <td>
                <span class="status status-{h.reachable ? 'on' : 'off'}">
                  {h.reachable ? 'online' : 'offline'}
                </span>
              </td>
              <td class="row-actions">
                ... existing actions ...
              </td>
            </tr>
```

- [ ] **Step 4: Add CSS for `.account`**

In the existing `<style>` block:

```css
  .hosts-table td.account {
    font-size: 0.8rem;
    max-width: 220px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--fg);
  }
```

- [ ] **Step 5: Add 2 tests in `SettingsDialog.test.ts`**

Inside the existing `describe('SettingsDialog')` block, append:

```typescript
  it('Account column shows email (seatTier) when account is known', async () => {
    hosts.set([
      { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: 'u1' },
    ]);
    accountsStore.set([
      { uuid: 'u1', email: 'm.janci@32bit.sk', display_name: 'Martin', organization_name: '32bit', organization_uuid: 'org-1', seat_tier: 'max', last_seen_at: 1 },
    ]);
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick();
    const cell = await screen.findByTestId('account-cell');
    expect(cell.textContent).toContain('m.janci@32bit.sk');
    expect(cell.textContent).toContain('max');
  });

  it('Account column shows — when host has no account', async () => {
    hosts.set([
      { alias: 'noaccount', ssh_alias: 'noaccount', reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: 1, account_uuid: null },
    ]);
    accountsStore.set([]);
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick();
    const cell = await screen.findByTestId('account-cell');
    expect(cell.textContent?.trim()).toBe('—');
  });
```

Also add the imports at the top of `SettingsDialog.test.ts`:

```ts
import { accounts as accountsStore } from './accounts';
```

(Renamed to `accountsStore` to avoid shadowing if a local var is also called `accounts`.)

- [ ] **Step 6: Run tests**

```bash
pnpm vitest run src/lib/SettingsDialog.test.ts 2>&1 | tail -10
```

Expected: all SettingsDialog tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/lib/SettingsDialog.svelte src/lib/SettingsDialog.test.ts
git commit -m "SettingsDialog: Account column with email/tier or — fallback"
```

---

## Task 8: AddHostPicker preview gate

**Files:**
- Modify: `src/lib/AddHostPicker.svelte` — split click → probe-preview → confirm-add
- Modify: `src/lib/AddHostPicker.test.ts` — 2 new tests

- [ ] **Step 1: Update `AddHostPicker.svelte` to add preview state**

Replace the existing `<script>` content with:

```svelte
<script lang="ts">
  import { onMount } from 'svelte';
  import { discoverHosts, addHost, type SshHost } from './hosts';
  import { probeSshAlias, type ProbePreview } from './accounts';

  let { onClose }: { onClose: () => void } = $props();

  let available = $state<SshHost[]>([]);
  let loading = $state(true);
  let error: string | null = $state(null);
  // Preview state: the host the user clicked + the probe result so far.
  // null = no row clicked yet; { host, preview: null } = probing; { host, preview: ProbePreview } = ready to confirm.
  let previewing = $state<{ host: SshHost; preview: ProbePreview | null } | null>(null);
  let adding = $state(false);

  onMount(async () => {
    const r = await discoverHosts();
    loading = false;
    if (r.ok) {
      available = r.value;
    } else {
      error = r.error.message;
    }
  });

  async function pick(host: SshHost) {
    previewing = { host, preview: null };
    error = null;
    const r = await probeSshAlias(host.alias);
    if (!previewing || previewing.host.alias !== host.alias) {
      // User clicked Cancel during the probe — discard
      return;
    }
    if (!r.ok) {
      error = r.error.message;
      previewing = null;
      return;
    }
    previewing = { host, preview: r.value };
  }

  async function confirmAdd() {
    if (!previewing?.preview) return;
    adding = true;
    error = null;
    const r = await addHost(previewing.host.alias, previewing.host.alias);
    adding = false;
    if (!r.ok) {
      error = r.error.message;
      return;
    }
    onClose();
  }

  function cancelPreview() {
    previewing = null;
  }

  function describe(h: SshHost): string {
    const parts: string[] = [];
    if (h.hostname) parts.push(h.hostname);
    if (h.user) parts.push(`user=${h.user}`);
    if (h.port) parts.push(`port=${h.port}`);
    return parts.join(' · ');
  }

  function accountLine(p: ProbePreview): string {
    if (!p.account) return '— (claude not logged in)';
    const email = p.account.email ?? p.account.uuid ?? 'unknown';
    return p.account.seat_tier ? `${email} (${p.account.seat_tier})` : email;
  }
</script>

<div class="modal-backdrop" onclick={onClose} role="presentation">
  <div class="dialog" onclick={(e) => e.stopPropagation()} role="dialog" aria-label="Add SSH host">
    {#if !previewing}
      <h3>Add SSH host</h3>
      {#if loading}
        <p class="muted">Scanning ~/.ssh/config…</p>
      {:else if available.length === 0}
        <p class="muted">No hosts found in ~/.ssh/config. Add one there first.</p>
      {:else}
        <ul class="hosts-list">
          {#each available as h (h.alias)}
            <li>
              <button
                class="host-row"
                data-testid="picker-row"
                onclick={() => pick(h)}
              >
                <span class="alias">{h.alias}</span>
                {#if describe(h)}
                  <span class="desc">{describe(h)}</span>
                {/if}
              </button>
            </li>
          {/each}
        </ul>
      {/if}
      {#if error}<p class="err">{error}</p>{/if}
      <div class="actions">
        <button onclick={onClose}>Close</button>
      </div>
    {:else}
      <h3>Add host: {previewing.host.alias}</h3>
      {#if !previewing.preview}
        <p class="muted" data-testid="preview-probing">Probing…</p>
      {:else}
        <dl class="preview" data-testid="preview-result">
          <dt>Hostname</dt><dd>{previewing.host.hostname ?? '—'}</dd>
          <dt>tmux</dt><dd>{previewing.preview.tmux_version ?? '— (not installed)'}</dd>
          <dt>claude</dt><dd>{previewing.preview.claude_version ?? '— (not installed)'}</dd>
          <dt>Account</dt><dd data-testid="preview-account">{accountLine(previewing.preview)}</dd>
        </dl>
      {/if}
      {#if error}<p class="err">{error}</p>{/if}
      <div class="actions">
        <button onclick={cancelPreview} disabled={adding}>Cancel</button>
        <button
          class="primary"
          disabled={!previewing.preview || adding}
          onclick={confirmAdd}
          data-testid="preview-confirm"
        >{adding ? 'Adding…' : 'Add'}</button>
      </div>
    {/if}
  </div>
</div>

<style>
  /* keep existing styles; add: */
  .preview {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 0.4rem 1rem;
    margin: 0;
  }
  .preview dt {
    color: var(--fg-muted);
    font-size: 0.7rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .preview dd { margin: 0; font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 0.9rem; }
  .actions button.primary {
    border-color: var(--accent);
    color: var(--fg);
  }
  /* (keep all the pre-existing .modal-backdrop, .dialog, .hosts-list, .host-row, .alias, .desc, .status, .err, .actions, .actions button styles unchanged) */
</style>
```

**Important**: This rewrites the `<script>` and template but keeps the pre-existing styles. When applying, preserve all existing CSS rules from the original file — only ADD the new `.preview` + `.actions button.primary` blocks shown.

- [ ] **Step 2: Update `AddHostPicker.test.ts`**

Replace the 3 existing tests with 5 (3 covering existing flow + 2 new for preview gate):

```typescript
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import AddHostPicker from './AddHostPicker.svelte';
import { hosts } from './hosts';

beforeEach(() => {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
  hosts.set([]);
});

describe('AddHostPicker', () => {
  it('lists discovered hosts', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'discover_hosts') {
        return [
          { alias: 'mefistos', hostname: '192.168.1.50', user: 'mjanci', port: 22 },
          { alias: 'mac', hostname: null, user: null, port: null },
        ];
      }
      return null;
    });
    render(AddHostPicker, { props: { onClose: () => {} } });
    for (let i = 0; i < 8; i++) await tick();
    const rows = await screen.findAllByTestId('picker-row');
    expect(rows).toHaveLength(2);
  });

  it('clicking a row probes (without adding yet)', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'discover_hosts') return [{ alias: 'mefistos', hostname: null, user: null, port: null }];
      if (cmd === 'probe_ssh_alias') return {
        reachable: true,
        claude_version: '2.1.144',
        tmux_version: '3.6a',
        account: { uuid: 'u1', email: 'm.janci@32bit.sk', display_name: 'M', organization_name: null, organization_uuid: null, seat_tier: 'max' },
      };
      return null;
    });
    render(AddHostPicker, { props: { onClose: () => {} } });
    for (let i = 0; i < 8; i++) await tick();
    const row = await screen.findByTestId('picker-row');
    await fireEvent.click(row);
    for (let i = 0; i < 8; i++) await tick();
    // Preview is shown, NOT yet added
    const preview = await screen.findByTestId('preview-result');
    expect(preview).toBeInTheDocument();
    const calls = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls;
    expect(calls.some((c) => c[0] === 'probe_ssh_alias')).toBe(true);
    expect(calls.some((c) => c[0] === 'add_host')).toBe(false);
  });

  it('preview shows account email + seatTier', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'discover_hosts') return [{ alias: 'mefistos', hostname: null, user: null, port: null }];
      if (cmd === 'probe_ssh_alias') return {
        reachable: true,
        claude_version: '2.1.144',
        tmux_version: '3.6a',
        account: { uuid: 'u1', email: 'm.janci@32bit.sk', display_name: 'M', organization_name: null, organization_uuid: null, seat_tier: 'max' },
      };
      return null;
    });
    render(AddHostPicker, { props: { onClose: () => {} } });
    for (let i = 0; i < 8; i++) await tick();
    await fireEvent.click(await screen.findByTestId('picker-row'));
    for (let i = 0; i < 8; i++) await tick();
    const accountCell = screen.getByTestId('preview-account');
    expect(accountCell.textContent).toContain('m.janci@32bit.sk');
    expect(accountCell.textContent).toContain('max');
  });

  it('preview shows — when account is missing', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'discover_hosts') return [{ alias: 'noaccount', hostname: null, user: null, port: null }];
      if (cmd === 'probe_ssh_alias') return {
        reachable: true,
        claude_version: null,
        tmux_version: '3.6a',
        account: null,
      };
      return null;
    });
    render(AddHostPicker, { props: { onClose: () => {} } });
    for (let i = 0; i < 8; i++) await tick();
    await fireEvent.click(await screen.findByTestId('picker-row'));
    for (let i = 0; i < 8; i++) await tick();
    const accountCell = screen.getByTestId('preview-account');
    expect(accountCell.textContent).toContain('—');
  });

  it('confirm-Add invokes add_host then closes', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'discover_hosts') return [{ alias: 'mefistos', hostname: null, user: null, port: null }];
      if (cmd === 'probe_ssh_alias') return { reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', account: null };
      if (cmd === 'add_host') return { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: null };
      if (cmd === 'list_hosts') return [];
      return null;
    });
    let closed = false;
    render(AddHostPicker, { props: { onClose: () => { closed = true; } } });
    for (let i = 0; i < 8; i++) await tick();
    await fireEvent.click(await screen.findByTestId('picker-row'));
    for (let i = 0; i < 8; i++) await tick();
    await fireEvent.click(await screen.findByTestId('preview-confirm'));
    for (let i = 0; i < 8; i++) await tick();
    const calls = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls;
    expect(calls.some((c) => c[0] === 'add_host')).toBe(true);
    expect(closed).toBe(true);
  });
});
```

- [ ] **Step 3: Run tests**

```bash
pnpm vitest run src/lib/AddHostPicker.test.ts 2>&1 | tail -10
```

Expected: 5 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/lib/AddHostPicker.svelte src/lib/AddHostPicker.test.ts
git commit -m "AddHostPicker: probe-preview gate before confirm-add"
```

---

## Task 9: SessionDetails Host + Account rows

**Files:**
- Modify: `src/lib/SessionDetails.svelte`
- Modify: `src/lib/SessionDetails.test.ts` (or create if absent)

- [ ] **Step 1: Update `SessionDetails.svelte` imports + derived**

Add to imports:

```ts
  import { hosts, type HostRow } from './hosts';
  import { accounts, type AccountRow } from './accounts';
```

Add derived:

```ts
  const hostRow = $derived(
    $hosts.find((h) => h.alias === session.host_alias) ?? null,
  );
  const accountRow = $derived(
    hostRow?.account_uuid ? $accounts.find((a) => a.uuid === hostRow.account_uuid) ?? null : null,
  );
  function accountText(a: AccountRow | null): string {
    if (!a) return '—';
    const email = a.email ?? a.uuid;
    return a.seat_tier ? `${email} (${a.seat_tier})` : email;
  }
```

- [ ] **Step 2: Update the meta-grid template**

Find the existing `<dl class="meta">` block. Insert Host + Account rows BEFORE the existing `<dt>Project</dt>`:

```svelte
  <dl class="meta">
    <dt>Host</dt>
    <dd data-testid="session-host">{session.host_alias}</dd>

    <dt>Account</dt>
    <dd data-testid="session-account">{accountText(accountRow)}</dd>

    <dt>Project</dt>
    <dd>
      {#if parentProject}
        {parentProject.project.owner}/{parentProject.project.repo}
      {:else}
        <span class="muted">unmapped (orphan)</span>
      {/if}
    </dd>
    ... existing Created / Last activity ...
  </dl>
```

- [ ] **Step 3: Check if `SessionDetails.test.ts` exists**

```bash
ls src/lib/SessionDetails.test.ts 2>&1
```

If it doesn't exist, create it with basic structure (Step 4). If it exists, append the tests (Step 4 variants).

- [ ] **Step 4a: If creating SessionDetails.test.ts from scratch**

```typescript
import { render, screen } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import SessionDetails from './SessionDetails.svelte';
import { hosts } from './hosts';
import { accounts } from './accounts';
import { projects } from './projects';

const sampleSession = {
  id: 1,
  tmux_name: 'dev-foo',
  host_alias: 'mefistos',
  project_id: null,
  worktree_id: null,
  created_at: 1,
  last_activity_at: 1,
  status: 'running',
  notes: null,
};

beforeEach(() => {
  hosts.set([]);
  accounts.set([]);
  projects.set([]);
});

describe('SessionDetails', () => {
  it('shows host alias from session', async () => {
    hosts.set([
      { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: null },
    ]);
    render(SessionDetails, { props: { session: sampleSession } });
    await tick();
    expect((await screen.findByTestId('session-host')).textContent).toBe('mefistos');
  });

  it('shows account when host has one linked', async () => {
    hosts.set([
      { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: 'u1' },
    ]);
    accounts.set([
      { uuid: 'u1', email: 'm.janci@32bit.sk', display_name: 'M', organization_name: null, organization_uuid: null, seat_tier: 'max', last_seen_at: 1 },
    ]);
    render(SessionDetails, { props: { session: sampleSession } });
    await tick();
    const cell = await screen.findByTestId('session-account');
    expect(cell.textContent).toContain('m.janci@32bit.sk');
    expect(cell.textContent).toContain('max');
  });

  it('shows — when host has no account', async () => {
    hosts.set([
      { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: null },
    ]);
    accounts.set([]);
    render(SessionDetails, { props: { session: sampleSession } });
    await tick();
    expect((await screen.findByTestId('session-account')).textContent?.trim()).toBe('—');
  });
});
```

- [ ] **Step 4b: If SessionDetails.test.ts exists, just append the 3 new test blocks inside the existing `describe`**

- [ ] **Step 5: Run tests**

```bash
pnpm vitest run src/lib/SessionDetails.test.ts 2>&1 | tail -10
```

Expected: 3 tests pass (or all if existing + 3 new).

- [ ] **Step 6: Full vitest sanity sweep**

```bash
pnpm vitest run 2>&1 | tail -8
```

Expected: all tests pass across all files.

- [ ] **Step 7: Commit**

```bash
git add src/lib/SessionDetails.svelte src/lib/SessionDetails.test.ts
git commit -m "SessionDetails: Host + Account rows above Project"
```

---

## Task 10: Live verify (local + mefistos when online)

This is manual but scripted for reproducibility.

- [ ] **Step 1: Build the release bundle**

```bash
cd /Users/martinjanci/projects/github.com/martin-janci/claude-fleet
pnpm tauri build --bundles app 2>&1 | tail -8
```

Expected: clean build.

- [ ] **Step 2: Restart claude-fleet**

```bash
pkill -f claude-fleet 2>/dev/null; sleep 1
open -a /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/src-tauri/target/release/bundle/macos/claude-fleet.app
```

- [ ] **Step 3: Probe local in UI**

In the UI:
1. Open Settings (`⚙`)
2. Click `↻ Re-probe` next to the `local` row
3. Verify the Account column now shows `m.janci@32bit.sk (max)` (or whatever the user's logged-in account is)

If still `—`: check `~/.claude.json` has `oauthAccount` populated:

```bash
jq '.oauthAccount | {accountUuid, emailAddress, seatTier}' ~/.claude.json
```

- [ ] **Step 4: Re-probe mefistos (when SSH is online again)**

In the UI:
1. Settings → click `↻` next to `mefistos`
2. Verify Account column shows the same account email (Mode B confirmed — same account on two hosts) OR a different one (Mode A)

If `—` on mefistos:
```bash
ssh mefistos 'cat ~/.claude.json | jq -c .oauthAccount'
```

- [ ] **Step 5: Sidebar tooltip + SessionDetails check**

In the UI:
1. Hover over the `local` host pill → tooltip should include account line
2. Hover over `mefistos` host pill → same
3. Click an existing session → SessionDetails right pane shows Host + Account rows
4. If session is on `mefistos`, account should match the mefistos host's account

- [ ] **Step 6: AddHostPicker preview test**

In the UI:
1. Settings → `+ Add host`
2. The picker should still list aliases from `~/.ssh/config`
3. Click any alias → modal transitions to preview state showing tmux/claude/account
4. Cancel returns to picker; Add proceeds normally

- [ ] **Step 7: Final test pass + commit notes**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib 2>&1 | tail -5
pnpm vitest run 2>&1 | tail -5
```

Expected: all green. (Should be ~77 Rust + ~124 vitest if all task-counted tests landed.)

If any edge cases surfaced during live testing, append them to the spec's "Open risks" section then commit:

```bash
git add docs/specs/2026-05-20-account-model-design.md
git commit -m "docs: account-model spec — live verification notes"
```

---

## Self-Review (filled in by plan author)

**Spec coverage check:**
- Data model migration → Task 1 ✓
- AccountRow + 4 store helpers → Task 2 ✓
- Probe v2 + parse_oauth_account → Task 3 ✓
- list_accounts + probe_ssh_alias commands → Task 4 ✓
- Frontend accounts store + HostRow update → Task 5 ✓
- Sidebar host pill tooltip → Task 6 ✓
- SettingsDialog Account column → Task 7 ✓
- AddHostPicker probe preview gate → Task 8 ✓
- SessionDetails Host + Account rows → Task 9 ✓
- Live verify on local + mefistos → Task 10 ✓

**Placeholder scan:** No "TBD", "TODO", "implement later" in steps. Each step has the full code/command to run.

**Type consistency:**
- `AccountRow` shape consistent across `src-tauri/src/store.rs` (Rust struct) and `src/lib/accounts.ts` (TS interface): `uuid`, `email`, `display_name`, `organization_name`, `organization_uuid`, `seat_tier`, `last_seen_at` — all match (snake_case in both ends per serde defaults).
- `OauthAccount` (probe parsing struct) uses camelCase via `#[serde(rename = "...")]` since Anthropic's JSON is camelCase; that's distinct from `AccountRow` (snake_case for our DB/IPC).
- `HostRow` now has `account_uuid: Option<String>` / `account_uuid: string | null` consistently.
- `ProbePreview` shape mirrors between Rust (`hosts.rs::ProbePreview`) and TS (`accounts.ts::ProbePreview`).

**Scope:** 10 tasks, ~half day of focused work. Each commits independently. Live verify is final task.
