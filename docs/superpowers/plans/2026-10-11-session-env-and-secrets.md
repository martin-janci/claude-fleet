# Session Environment Variables and Secrets Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every session fleet launches gets environment variables scoped
global < host < project < session. Secrets among them never appear in an
argv, a tmux environment, the MCP audit or a listing.

**Architecture:** A new `env_vars` table and store module hold the entries.
`service::env` does four things:

1. Resolves a session's merged set, including `env:`/`file:` references.
2. Writes it to a 0600 nonce file on the host through
   `provision::write_host_file_secret`.
3. Wraps the finished pane command so that it sources that file.
4. Records the host so the tick sweep deletes files older than 10 minutes.

Surfaces: an `env_vars` MCP tool, `new_session { env }`, a form field
`env_target`, a routed Tauri command, and a resource page under Settings.

**Tech Stack:** Rust (fleet-core, src-tauri), rusqlite, rmcp, Svelte 5
(resource pages are spec-driven; no Svelte code is needed).

**Spec:** `docs/superpowers/specs/2026-10-11-session-env-and-secrets-design.md`.
Read it all. Its last section, *Revisions after the code survey*, wins
over the sections above it.

## Global Constraints

- Work only in this worktree: `…/scratchpad/cf-env`, branch
  `feat/session-env-secrets`.
  - Run every git command as `git -C <worktree>`.
  - Never run pull, push, rebase, checkout, stash or merge.
- **Inner loop:** `cargo fleet-fast-check`.
- **Tests:** `cargo fleet-test -- <filter>`. Never use `-p`, plain `cargo
  build`, or `cargo check` without `--all-targets`.
- **Before each commit:** `scripts/verify.sh`. Run long cargo commands in
  the foreground with `timeout: 600000`. Split fmt, lint and test into
  separate calls if one call would pass 600 s.
- **Errors:** `IpcError::new(codes::E_*, msg)`. Validation uses
  `codes::E_INVALID`. The one new code is `E_ENV_UNRESOLVED`.
- **No value of a variable** may appear in:
  - a pane command;
  - a tmux argv or tmux environment;
  - an `IpcError` message;
  - a `tracing` line;
  - the MCP audit;
  - a listing of a secret or reference entry.
- **Shell quoting** is `crate::shell::quote` only.
- **Child processes** are built through `fleet_core::proc`, never
  `Command::new`. Test helpers may follow existing test practice.
- **Name rule:** `^[A-Za-z_][A-Za-z0-9_]{0,127}$`.
- **Reserved names:**
  - exact: `PATH HOME SHELL USER TERM COLORTERM LANG TMUX CLAUDE_CONFIG_DIR ANTHROPIC_API_KEY`;
  - by prefix: `LC_`, `TMUX_`, `CLAUDE_FLEET_`.
- **Sizes:**
  - a value is at most 32 KiB, with no NUL byte;
  - a merged set is at most 256 KiB, measured as the sum of
    `name.len() + value.len()`.
- **Env file:** `~/.claude-fleet/env/<uuid v4 simple>.sh`, mode 0600, in
  directory `~/.claude-fleet/env`. The settings key is
  `internal.env_files_hosts`. The sweep age is 10 minutes.
- **Migration number:** take it from origin/main when Task 1 runs
  (`git -C <wt> ls-tree --name-only origin/main crates/fleet-core/migrations/ | tail -1`
  and add 1). It is 173 at the time of writing; this plan writes `173`.
  Substitute the real number everywhere.
- **Contract:** `CONTRACT_REVISION` and `MAX_HUB_CONTRACT` go to 18.
  `MIN_HUB_CONTRACT` stays at 16.

## File structure

| File | Responsibility |
|---|---|
| `crates/fleet-core/migrations/173_env_vars.sql` (new) | table + session-delete trigger |
| `crates/fleet-core/src/store/env_vars.rs` (new) | every SQL read and write of `env_vars`, name, scope and value validation, precedence merge |
| `crates/fleet-core/src/service/env.rs` (new) | resolve references, render, write the file, wrap the pane command, sweep |
| `crates/fleet-core/src/mcp/tools/env_vars.rs` (new) | the `env_vars` MCP tool |
| `src-tauri/src/commands/env_vars.rs` (new) | the routed Tauri command |
| `crates/fleet-core/pages/settings.env.json` (new) | the Settings page |
| existing files | one wiring change each, named per task |

---

### Task 1: Migration and store

**Files:**
- Create: `crates/fleet-core/migrations/173_env_vars.sql`
- Create: `crates/fleet-core/src/store/env_vars.rs`
- Modify: `crates/fleet-core/src/store/mod.rs` (add `mod env_vars;` next to
  `mod forms;` (~:26), the `pub use`, and a `pub(crate) use` for
  `read_credential_ref`)
- Modify: `crates/fleet-core/src/store/trackers.rs:825` (`pub(super) fn
  read_credential_ref` → `pub(crate) fn read_credential_ref`)
- Modify: `crates/fleet-core/src/store/schema.rs` (MIGRATIONS entry after
  the 172 entry at ~:1965; add `"env_vars"` to `EXPECTED_TABLES` ~:2398)
- Modify: `crates/fleet-core/src/store/schema/tests_upgrade.rs` (add the
  trigger name to the trigger list ~:332)
- Modify: `crates/fleet-core/src/store/hosts_accounts.rs` (in
  `merge_host_alias`, after the per-host table loop ~:705-716)

**Interfaces:**
- Produces, re-exported from `crate::store`:
  - `EnvValue { Plain(String), Secret(Secret), Ref(String) }`
  - `EnvSource { Value(Secret), Ref(String) }`
  - `EnvVarRow` (Serialize)
  - `ENV_SCOPES: &[&str]`
  - `check_env_name(&str) -> Result<(), IpcError>`
  - `check_env_value(&str) -> Result<(), IpcError>`
  - `check_env_scope(scope, scope_key) -> Result<(), IpcError>`
- Produces `Store` methods:
  - `set_env_var(&self, scope: &str, scope_key: &str, name: &str, v: &EnvValue) -> Result<(), IpcError>`
  - `unset_env_var(&self, scope: &str, scope_key: &str, name: &str) -> Result<bool, IpcError>`
  - `list_env_vars(&self) -> Result<Vec<EnvVarRow>, IpcError>`
  - `env_for_launch(&self, host: &str, project: Option<&str>, session: Option<i64>) -> Result<Vec<(String, EnvSource)>, IpcError>`
  - `env_secret_literals(&self) -> Result<Vec<String>, IpcError>`
- Produces: `pub(crate) crate::store::read_credential_ref(&str) -> Option<String>`.

- [ ] **Step 1: Write the migration**

```sql
-- Session environment variables (spec 2026-10-11-session-env-and-secrets-design.md).
-- Exactly one of value / credential_ref. scope_key: '' (global), host alias,
-- 'owner/repo' (project) or the session id as text (session).
CREATE TABLE IF NOT EXISTS env_vars (
  id             INTEGER PRIMARY KEY,
  scope          TEXT NOT NULL CHECK (scope IN ('global','host','project','session')),
  scope_key      TEXT NOT NULL,
  name           TEXT NOT NULL,
  value          TEXT,
  credential_ref TEXT,
  secret         INTEGER NOT NULL DEFAULT 0,
  updated_at     INTEGER NOT NULL,
  UNIQUE (scope, scope_key, name),
  CHECK ((value IS NULL) <> (credential_ref IS NULL)),
  CHECK (scope <> 'global' OR scope_key = '')
);

CREATE TRIGGER IF NOT EXISTS trg_env_vars_on_session_delete
AFTER DELETE ON sessions
BEGIN
  DELETE FROM env_vars WHERE scope = 'session' AND scope_key = CAST(old.id AS TEXT);
END;

INSERT OR IGNORE INTO schema_version (version) VALUES (173);
```

Register it after the 172 entry in `schema.rs`:

```rust
    // Session environment variables and secrets.
    Migration::plain(173, include_str!("../../migrations/173_env_vars.sql")),
```

- [ ] **Step 2: Write the failing store tests**

Put them at the bottom of the new `store/env_vars.rs` under `#[cfg(test)] mod tests`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn store() -> Store {
        Store::open_in_memory().unwrap()
    }
    fn plain(s: &str) -> EnvValue {
        EnvValue::Plain(s.into())
    }
    fn val(src: &EnvSource) -> String {
        match src {
            EnvSource::Value(s) => s.expose().to_string(),
            EnvSource::Ref(r) => format!("ref:{r}"),
        }
    }

    #[test]
    fn the_narrower_scope_wins() {
        let s = store();
        s.set_env_var("global", "", "A", &plain("g")).unwrap();
        s.set_env_var("host", "mercury", "A", &plain("h")).unwrap();
        s.set_env_var("project", "o/r", "A", &plain("p")).unwrap();
        s.set_env_var("session", "7", "A", &plain("s")).unwrap();
        s.set_env_var("global", "", "B", &plain("gb")).unwrap();
        s.set_env_var("host", "other", "C", &plain("no")).unwrap();
        let get = |h, p, sid| {
            s.env_for_launch(h, p, sid)
                .unwrap()
                .into_iter()
                .map(|(n, v)| format!("{n}={}", val(&v)))
                .collect::<Vec<_>>()
        };
        assert_eq!(get("mercury", Some("o/r"), Some(7)), ["A=s", "B=gb"]);
        assert_eq!(get("mercury", Some("o/r"), None), ["A=p", "B=gb"]);
        assert_eq!(get("mercury", None, None), ["A=h", "B=gb"]);
        assert_eq!(get("local", None, None), ["A=g", "B=gb"]);
    }

    #[test]
    fn set_replaces_and_unset_removes() {
        let s = store();
        s.set_env_var("global", "", "A", &plain("1")).unwrap();
        s.set_env_var("global", "", "A", &EnvValue::Ref("env:X".into())).unwrap();
        let rows = s.list_env_vars().unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].is_ref && rows[0].secret && rows[0].value.is_none());
        assert!(s.unset_env_var("global", "", "A").unwrap());
        assert!(!s.unset_env_var("global", "", "A").unwrap());
    }

    #[test]
    fn a_listing_never_carries_a_secret_or_a_reference() {
        let s = store();
        s.set_env_var("global", "", "PLAIN", &plain("visible")).unwrap();
        s.set_env_var("global", "", "SEC", &EnvValue::Secret(Secret::new("hunter2-hunter2"))).unwrap();
        s.set_env_var("global", "", "REF", &EnvValue::Ref("file:/run/secrets/x".into())).unwrap();
        let json = serde_json::to_string(&s.list_env_vars().unwrap()).unwrap();
        assert!(json.contains("visible"), "{json}");
        assert!(!json.contains("hunter2") && !json.contains("/run/secrets"), "{json}");
    }

    #[test]
    fn names_scopes_and_values_are_checked() {
        let s = store();
        for bad in ["", "1A", "A-B", "PATH", "LC_ALL", "TMUX_PANE", "CLAUDE_FLEET_X", "ANTHROPIC_API_KEY"] {
            let e = s.set_env_var("global", "", bad, &plain("v")).unwrap_err();
            assert_eq!(e.code, crate::ipc_error::codes::E_INVALID, "{bad}");
        }
        assert!(s.set_env_var("global", "x", "A", &plain("v")).is_err());
        assert!(s.set_env_var("host", "", "A", &plain("v")).is_err());
        assert!(s.set_env_var("session", "abc", "A", &plain("v")).is_err());
        assert!(s.set_env_var("project", "norepo", "A", &plain("v")).is_err());
        assert!(s.set_env_var("planet", "x", "A", &plain("v")).is_err());
        assert!(s.set_env_var("global", "", "A", &plain("a\0b")).is_err());
        assert!(s.set_env_var("global", "", "A", &plain(&"x".repeat(32 * 1024 + 1))).is_err());
        assert!(s.set_env_var("global", "", "A", &EnvValue::Ref("file:relative".into())).is_err());
        let e = s.set_env_var("global", "", "PATH", &plain("v")).unwrap_err();
        assert!(!e.message.contains('v') || e.message.contains("PATH"));
    }

    #[test]
    fn deleting_a_session_drops_its_rows() {
        let s = store();
        s.conn
            .execute(
                "INSERT INTO sessions (id, tmux_name, host_alias) VALUES (7, 't', 'local')",
                [],
            )
            .unwrap();
        s.set_env_var("session", "7", "A", &plain("v")).unwrap();
        s.conn.execute("DELETE FROM sessions WHERE id = 7", []).unwrap();
        assert!(s.list_env_vars().unwrap().is_empty());
    }

    #[test]
    fn secret_literals_are_the_stored_secrets_only() {
        let s = store();
        s.set_env_var("global", "", "P", &plain("plainplain")).unwrap();
        s.set_env_var("global", "", "S", &EnvValue::Secret(Secret::new("sekret-sekret"))).unwrap();
        assert_eq!(s.env_secret_literals().unwrap(), ["sekret-sekret"]);
    }

    #[test]
    fn only_these_statements_read_the_table() {
        let src = include_str!("env_vars.rs");
        let src = &src[..src.find("#[cfg(test)]").unwrap()];
        for ty in ["pub enum EnvSource", "pub enum EnvValue"] {
            let at = src.find(ty).unwrap();
            let derive = &src[src[..at].rfind("#[derive").unwrap()..at];
            assert!(!derive.contains("Serialize"), "{ty} derives Serialize");
        }
        assert_eq!(
            src.matches("FROM env_vars").count(),
            4,
            "env_vars is read by list_env_vars, env_for_launch, \
             env_secret_literals and deleted by unset_env_var only"
        );
    }
}
```

The `sessions` INSERT in `deleting_a_session_drops_its_rows` has to satisfy
the real `sessions` NOT NULL columns. Check `001_init.sql:28` and the later
`ALTER`s, and add any required columns to the INSERT. Do not loosen the
schema.

- [ ] **Step 3: Run them to verify they fail**

Run: `cargo fleet-test -- store::env_vars`
Expected: compile error (module missing).

- [ ] **Step 4: Implement `store/env_vars.rs`**

```rust
//! Session environment variables (spec 2026-10-11-session-env-and-secrets-design.md).
//! Every statement that reads `env_vars` lives here (pinned by a test).

use std::collections::BTreeMap;

use super::{now_unix, Secret, Store};
use crate::ipc_error::{codes, IpcError};
use serde::Serialize;

pub const ENV_SCOPES: &[&str] = &["global", "host", "project", "session"];
const RESERVED: &[&str] = &[
    "PATH", "HOME", "SHELL", "USER", "TERM", "COLORTERM", "LANG", "TMUX",
    "CLAUDE_CONFIG_DIR", "ANTHROPIC_API_KEY",
];
const RESERVED_PREFIXES: &[&str] = &["LC_", "TMUX_", "CLAUDE_FLEET_"];
pub const MAX_ENV_VALUE: usize = 32 * 1024;

/// What `set_env_var` stores.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvValue {
    Plain(String),
    Secret(Secret),
    /// `env:NAME` | `file:/abs`, resolved by the controlling process at launch.
    Ref(String),
}

/// A merged entry for one launch. No `Serialize`; `Secret` redacts its Debug.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvSource {
    Value(Secret),
    Ref(String),
}

/// A listing row: `value` only for a plain, non-reference entry.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EnvVarRow {
    pub id: i64,
    pub scope: String,
    pub scope_key: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub secret: bool,
    pub is_ref: bool,
    pub updated_at: i64,
}

fn invalid(msg: impl Into<String>) -> IpcError {
    IpcError::new(codes::E_INVALID, msg)
}

pub fn check_env_name(name: &str) -> Result<(), IpcError> {
    let mut chars = name.chars();
    let ok = name.len() <= 128
        && matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric());
    if !ok {
        return Err(invalid(format!(
            "{name:?} is not a variable name: letters, digits and _, not starting with a digit"
        )));
    }
    if RESERVED.contains(&name) || RESERVED_PREFIXES.iter().any(|p| name.starts_with(p)) {
        return Err(invalid(format!("{name} is set by fleet or the session itself")));
    }
    Ok(())
}

pub fn check_env_value(value: &str) -> Result<(), IpcError> {
    if value.len() > MAX_ENV_VALUE {
        return Err(invalid(format!("a value is at most {MAX_ENV_VALUE} bytes")));
    }
    if value.contains('\0') {
        return Err(invalid("a value cannot contain a NUL byte"));
    }
    Ok(())
}

pub fn check_env_scope(scope: &str, key: &str) -> Result<(), IpcError> {
    let ok = match scope {
        "global" => key.is_empty(),
        "host" => !key.is_empty() && key.len() <= 255,
        "project" => matches!(key.split_once('/'), Some((o, r)) if !o.is_empty() && !r.is_empty() && !r.contains('/')),
        "session" => key.parse::<i64>().map(|id| id > 0).unwrap_or(false),
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(invalid(format!(
            "scope {scope:?} needs scope_key: '' (global), a host alias, owner/repo (project) or a session id"
        )))
    }
}

fn rank(scope: &str) -> u8 {
    ENV_SCOPES.iter().position(|s| *s == scope).unwrap_or(0) as u8
}

impl Store {
    pub fn set_env_var(&self, scope: &str, scope_key: &str, name: &str, v: &EnvValue) -> Result<(), IpcError> {
        check_env_scope(scope, scope_key)?;
        check_env_name(name)?;
        let (value, cref, secret): (Option<&str>, Option<&str>, bool) = match v {
            EnvValue::Plain(s) => {
                check_env_value(s)?;
                (Some(s), None, false)
            }
            EnvValue::Secret(s) => {
                check_env_value(s.expose())?;
                (Some(s.expose()), None, true)
            }
            EnvValue::Ref(r) => {
                super::validate_credential_ref(r)?;
                (None, Some(r), true)
            }
        };
        self.conn.execute(
            "INSERT INTO env_vars (scope, scope_key, name, value, credential_ref, secret, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
             ON CONFLICT(scope, scope_key, name) DO UPDATE SET value = excluded.value, \
               credential_ref = excluded.credential_ref, secret = excluded.secret, \
               updated_at = excluded.updated_at",
            rusqlite::params![scope, scope_key, name, value, cref, secret as i64, now_unix()],
        )?;
        Ok(())
    }

    pub fn unset_env_var(&self, scope: &str, scope_key: &str, name: &str) -> Result<bool, IpcError> {
        let n = self.conn.execute(
            "DELETE FROM env_vars WHERE scope = ?1 AND scope_key = ?2 AND name = ?3",
            rusqlite::params![scope, scope_key, name],
        )?;
        Ok(n > 0)
    }

    pub fn list_env_vars(&self) -> Result<Vec<EnvVarRow>, IpcError> {
        let mut st = self.conn.prepare(
            "SELECT id, scope, scope_key, name, \
               CASE WHEN secret = 0 AND credential_ref IS NULL THEN value END, \
               secret, credential_ref IS NOT NULL, updated_at \
             FROM env_vars ORDER BY scope, scope_key, name",
        )?;
        let rows = st
            .query_map([], |r| {
                Ok(EnvVarRow {
                    id: r.get(0)?,
                    scope: r.get(1)?,
                    scope_key: r.get(2)?,
                    name: r.get(3)?,
                    value: r.get(4)?,
                    secret: r.get::<_, i64>(5)? != 0,
                    is_ref: r.get(6)?,
                    updated_at: r.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The merged set for one launch, narrower scope winning, sorted by name.
    pub fn env_for_launch(&self, host: &str, project: Option<&str>, session: Option<i64>) -> Result<Vec<(String, EnvSource)>, IpcError> {
        let session = session.map(|id| id.to_string());
        let mut st = self.conn.prepare(
            "SELECT scope, name, value, credential_ref FROM env_vars \
             WHERE scope = 'global' OR (scope = 'host' AND scope_key = ?1) \
                OR (scope = 'project' AND scope_key = ?2) OR (scope = 'session' AND scope_key = ?3)",
        )?;
        let mut merged: BTreeMap<String, (u8, EnvSource)> = BTreeMap::new();
        let rows = st.query_map(rusqlite::params![host, project, session], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?))
        })?;
        for row in rows {
            let (scope, name, value, cref) = row?;
            let src = match (value, cref) {
                (_, Some(r)) => EnvSource::Ref(r),
                (Some(v), None) => EnvSource::Value(Secret::new(v)),
                (None, None) => continue,
            };
            let rk = rank(&scope);
            if merged.get(&name).is_none_or(|(have, _)| rk > *have) {
                merged.insert(name, (rk, src));
            }
        }
        Ok(merged.into_iter().map(|(n, (_, s))| (n, s)).collect())
    }

    /// Stored secret values, for the diagnostics bundle's literal masking.
    pub fn env_secret_literals(&self) -> Result<Vec<String>, IpcError> {
        let mut st = self.conn.prepare("SELECT value FROM env_vars WHERE secret = 1 AND value IS NOT NULL")?;
        let out = st.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
        Ok(out)
    }
}
```

If the toolchain lacks `Option::is_none_or`, write
`merged.get(&name).map_or(true, |(have, _)| rk > *have)`.

In `store/mod.rs`:

```rust
mod env_vars;
pub use env_vars::{
    check_env_name, check_env_scope, check_env_value, EnvSource, EnvValue, EnvVarRow, ENV_SCOPES,
};
pub(crate) use trackers::read_credential_ref;
```

`validate_credential_ref` is already re-exported from `store`. If it is
not reachable as `super::validate_credential_ref`, use
`super::trackers::validate_credential_ref`.

- [ ] **Step 5: Carry host rows in `merge_host_alias`**

After the per-host table loop, inside the same transaction `tx`:

```rust
        // Host-scope environment: into's own entry wins on a name clash.
        tx.execute(
            "UPDATE OR IGNORE env_vars SET scope_key = ?2 WHERE scope = 'host' AND scope_key = ?1",
            rusqlite::params![from, into],
        )?;
        tx.execute(
            "DELETE FROM env_vars WHERE scope = 'host' AND scope_key = ?1",
            [from],
        )?;
```

Add a test next to the existing `merge_host_alias` tests. Use the existing
test's host setup helper.

```rust
    #[test]
    fn merging_hosts_carries_their_environment_and_into_wins() {
        let s = /* the existing merge test's store with hosts "old" and "new" */;
        s.set_env_var("host", "old", "A", &crate::store::EnvValue::Plain("old".into())).unwrap();
        s.set_env_var("host", "old", "B", &crate::store::EnvValue::Plain("b".into())).unwrap();
        s.set_env_var("host", "new", "A", &crate::store::EnvValue::Plain("new".into())).unwrap();
        s.merge_host_alias("old", "new").unwrap();
        let rows = s.list_env_vars().unwrap();
        let got: Vec<_> = rows.iter().map(|r| (r.scope_key.as_str(), r.name.as_str(), r.value.as_deref())).collect();
        assert_eq!(got, [("new", "A", Some("new")), ("new", "B", Some("b"))]);
    }
```

Copy the host setup lines of the nearest existing `merge_host_alias` test
in place of the comment.

- [ ] **Step 6: Run the tests**

Run: `cargo fleet-test -- store::env_vars merging_hosts_carries schema::`
Expected: all PASS, including `migrations_are_contiguous_from_one` and
`open_in_memory_creates_all_tables`.

- [ ] **Step 7: Verify and commit**

```bash
scripts/verify.sh
git -C <wt> add crates/fleet-core/migrations/173_env_vars.sql crates/fleet-core/src/store
git -C <wt> commit -m "feat(env): env_vars table and store"
```

---

### Task 2: `service::env`, resolve, render, write and wrap

**Files:**
- Create: `crates/fleet-core/src/service/env.rs`
- Modify: `crates/fleet-core/src/service/mod.rs` (add `pub mod env;`)
- Modify: `crates/fleet-core/src/ipc_error.rs` (`codes` module, next to `E_SECRET_MISSING` ~:253)

**Interfaces:**
- Consumes (Task 1): `Store::env_for_launch`, `EnvSource`,
  `check_env_name`, `check_env_value`, `read_credential_ref`, `Secret`.
- Produces:
  - `pub const ENV_DIR: &str = "~/.claude-fleet/env";`
  - `pub const ENV_FILES_HOSTS_KEY: &str = "internal.env_files_hosts";`
  - `pub struct EnvFile` with `is_empty()` and `render() -> String`
  - `pub fn resolve(store: &Mutex<Store>, host: &str, project: Option<&str>, session: Option<i64>, extra: &BTreeMap<String, String>) -> Result<EnvFile, IpcError>`
  - `pub fn wrap(tilde_path: &str, pane_cmd: &str) -> String`
  - `pub async fn prepare(store: &Mutex<Store>, ssh: &dyn SshExec, host: &str, project_id: Option<i64>, session: Option<i64>, extra: &BTreeMap<String, String>, pane_cmd: &str) -> Result<String, IpcError>`.
    It returns `pane_cmd` unchanged when the set is empty, and the wrapped
    command otherwise.
  - `pub async fn sweep_env_files(store: &Mutex<Store>, ssh: &dyn SshExec) -> usize` (used by Task 5)
  - `codes::E_ENV_UNRESOLVED`

- [ ] **Step 1: Add the error code**

```rust
    /// Session environment: a variable's `credential_ref` resolved to
    /// nothing at launch, so the launch was refused. The message names the
    /// variable and the reference kind, never a value or a path.
    pub const E_ENV_UNRESOLVED: &str = "E_ENV_UNRESOLVED";
```

- [ ] **Step 2: Write the failing tests** (`service/env.rs`, `#[cfg(test)] mod tests`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{EnvValue, Store};
    use std::sync::Mutex;

    fn store() -> Mutex<Store> {
        Mutex::new(Store::open_in_memory().unwrap())
    }

    #[test]
    fn render_round_trips_awkward_values_through_sh() {
        let values = [
            "plain", "it's", "a\nb", "$HOME `id` $(id)", "\\back\\", "\"dq\"", " lead and trail ", "",
        ];
        let file = EnvFile {
            lines: values
                .iter()
                .enumerate()
                .map(|(i, v)| (format!("V{i}"), Secret::new(*v)))
                .collect(),
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("e.sh");
        std::fs::write(&path, file.render()).unwrap();
        for (i, v) in values.iter().enumerate() {
            let out = std::process::Command::new("sh")
                .arg("-c")
                .arg(format!(". {}; printf %s \"$V{i}\"", crate::shell::quote(path.to_str().unwrap())))
                .output()
                .unwrap();
            assert_eq!(String::from_utf8(out.stdout).unwrap(), *v, "V{i}");
        }
    }

    #[test]
    fn wrap_names_only_the_path_and_reads_it_under_home() {
        let w = wrap("~/.claude-fleet/env/abc.sh", "cl --name 'x'");
        assert_eq!(
            w,
            "f=\"$HOME\"/'.claude-fleet/env/abc.sh'; if [ -r \"$f\" ]; then . \"$f\"; fi; unset f; cl --name 'x'"
        );
    }

    #[test]
    fn a_wrapped_command_sees_the_variables() {
        let home = tempfile::tempdir().unwrap();
        let envdir = home.path().join(".claude-fleet/env");
        std::fs::create_dir_all(&envdir).unwrap();
        let file = EnvFile { lines: vec![("GREETING".into(), Secret::new("hi there"))] };
        std::fs::write(envdir.join("n.sh"), file.render()).unwrap();
        let cmd = wrap("~/.claude-fleet/env/n.sh", "printf %s \"$GREETING\"");
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .env("HOME", home.path())
            .output()
            .unwrap();
        assert_eq!(String::from_utf8(out.stdout).unwrap(), "hi there");
    }

    #[test]
    fn extra_values_win_and_are_checked() {
        let s = store();
        s.lock().unwrap().set_env_var("global", "", "A", &EnvValue::Plain("g".into())).unwrap();
        let extra = BTreeMap::from([("A".to_string(), "x".to_string())]);
        let f = resolve(&s, "local", None, None, &extra).unwrap();
        assert!(f.render().contains("export A='x'"));
        let bad = BTreeMap::from([("PATH".to_string(), "x".to_string())]);
        assert_eq!(resolve(&s, "local", None, None, &bad).unwrap_err().code, codes::E_INVALID);
    }

    #[test]
    fn an_unresolved_reference_refuses_without_leaking() {
        let s = store();
        s.lock()
            .unwrap()
            .set_env_var("global", "", "TOKEN", &EnvValue::Ref("file:/nonexistent/fleet-env-test".into()))
            .unwrap();
        let e = resolve(&s, "local", None, None, &BTreeMap::new()).unwrap_err();
        assert_eq!(e.code, codes::E_ENV_UNRESOLVED);
        assert!(e.message.contains("TOKEN") && !e.message.contains("nonexistent"), "{}", e.message);
    }

    #[test]
    fn a_reference_resolves_from_the_environment() {
        let s = store();
        // SAFETY: a test-unique variable name; no other test reads it.
        unsafe { std::env::set_var("FLEET_ENV_TEST_REF_7731", "from-env") };
        s.lock()
            .unwrap()
            .set_env_var("global", "", "R", &EnvValue::Ref("env:FLEET_ENV_TEST_REF_7731".into()))
            .unwrap();
        let f = resolve(&s, "local", None, None, &BTreeMap::new()).unwrap();
        assert!(f.render().contains("export R='from-env'"));
    }

    #[test]
    fn the_merged_set_is_capped() {
        let s = store();
        let big = "x".repeat(32 * 1024);
        for i in 0..8 {
            s.lock().unwrap().set_env_var("global", "", &format!("V{i}"), &EnvValue::Plain(big.clone())).unwrap();
        }
        assert_eq!(resolve(&s, "local", None, None, &BTreeMap::new()).unwrap_err().code, codes::E_INVALID);
    }

    #[test]
    fn an_empty_set_leaves_the_pane_command_alone() {
        let f = resolve(&store(), "local", None, None, &BTreeMap::new()).unwrap();
        assert!(f.is_empty());
    }
}
```

If the crate's test convention needs a `proc::std_command` for spawning in
tests (`no_eprintln_tests::production_code_spawns_through_proc` checks
production code only), keep `std::process::Command` here. Switch to
`crate::proc::std_command("sh")` if that test flags it.

- [ ] **Step 3: Run them to verify they fail**

Run: `cargo fleet-test -- service::env`
Expected: compile error.

- [ ] **Step 4: Implement**

```rust
//! Session environment at launch (spec 2026-10-11-session-env-and-secrets-design.md).
//! The merged set is written to a 0600 nonce file on the host and the pane
//! command sources it; no value is ever in an argv. Files are deleted by
//! the tick sweep after ENV_FILE_MAX_AGE_MIN (several launchers may run one
//! pane command, so the pane does not delete its own file).

use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::ipc_error::{codes, lock, IpcError};
use crate::ssh::SshExec;
use crate::store::{check_env_name, check_env_value, read_credential_ref, EnvSource, Secret, Store};

pub const ENV_DIR: &str = "~/.claude-fleet/env";
pub const ENV_FILES_HOSTS_KEY: &str = "internal.env_files_hosts";
pub const ENV_FILE_MAX_AGE_MIN: u32 = 10;
const MAX_SET_BYTES: usize = 256 * 1024;

pub struct EnvFile {
    lines: Vec<(String, Secret)>,
}

impl EnvFile {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
    pub fn render(&self) -> String {
        let mut s = String::from("# claude-fleet session environment; removed by the sweep\n");
        for (name, v) in &self.lines {
            s.push_str(&format!("export {name}={}\n", crate::shell::quote(v.expose())));
        }
        s
    }
}

pub fn resolve(
    store: &Mutex<Store>,
    host: &str,
    project: Option<&str>,
    session: Option<i64>,
    extra: &BTreeMap<String, String>,
) -> Result<EnvFile, IpcError> {
    for (n, v) in extra {
        check_env_name(n)?;
        check_env_value(v)?;
    }
    // Read rows under the lock; resolve references (file/env reads) after it.
    let rows = lock(store)?.env_for_launch(host, project, session)?;
    let mut merged: BTreeMap<String, Secret> = BTreeMap::new();
    for (name, src) in rows {
        let v = match src {
            EnvSource::Value(v) => v,
            EnvSource::Ref(r) => {
                let kind = r.split_once(':').map(|(k, _)| k).unwrap_or("ref");
                match read_credential_ref(&r) {
                    Some(v) => Secret::new(v),
                    None => {
                        return Err(IpcError::new(
                            codes::E_ENV_UNRESOLVED,
                            format!("{name}: its {kind}: reference resolved to nothing"),
                        ))
                    }
                }
            }
        };
        merged.insert(name, v);
    }
    for (n, v) in extra {
        merged.insert(n.clone(), Secret::new(v.clone()));
    }
    let total: usize = merged.iter().map(|(n, v)| n.len() + v.expose().len()).sum();
    if total > MAX_SET_BYTES {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("the session's environment is {total} bytes; at most {MAX_SET_BYTES}"),
        ));
    }
    Ok(EnvFile { lines: merged.into_iter().collect() })
}

/// Prefix `pane_cmd` so it sources `tilde_path` (`~/...`). Only the path is
/// in the result.
pub fn wrap(tilde_path: &str, pane_cmd: &str) -> String {
    let rest = tilde_path
        .strip_prefix("~/")
        .expect("env file paths are under ~/");
    format!(
        "f=\"$HOME\"/{}; if [ -r \"$f\" ]; then . \"$f\"; fi; unset f; {pane_cmd}",
        crate::shell::quote(rest)
    )
}

fn project_slug(store: &Mutex<Store>, project_id: Option<i64>) -> Result<Option<String>, IpcError> {
    let Some(id) = project_id else { return Ok(None) };
    Ok(lock(store)?.get_project(id)?.map(|p| format!("{}/{}", p.owner, p.repo)))
}

fn note_host(store: &Mutex<Store>, host: &str) -> Result<(), IpcError> {
    let s = lock(store)?;
    let mut hosts: Vec<String> = s
        .get_setting(ENV_FILES_HOSTS_KEY)?
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default();
    if !hosts.iter().any(|h| h == host) {
        hosts.push(host.to_string());
        s.set_setting(ENV_FILES_HOSTS_KEY, &serde_json::to_string(&hosts).unwrap_or_default())?;
    }
    Ok(())
}

/// Resolve, write the file and wrap `pane_cmd`. An empty set returns
/// `pane_cmd` unchanged and writes nothing.
pub async fn prepare(
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    host: &str,
    project_id: Option<i64>,
    session: Option<i64>,
    extra: &BTreeMap<String, String>,
    pane_cmd: &str,
) -> Result<String, IpcError> {
    let project = project_slug(store, project_id)?;
    let file = resolve(store, host, project.as_deref(), session, extra)?;
    if file.is_empty() {
        return Ok(pane_cmd.to_string());
    }
    let path = format!("{ENV_DIR}/{}.sh", uuid::Uuid::new_v4().simple());
    // Record the host first: a file written but never swept is the leak.
    note_host(store, host)?;
    crate::service::provision::write_host_file_secret(ssh, host, ENV_DIR, &path, &file.render()).await?;
    Ok(wrap(&path, pane_cmd))
}
```

`get_project` and `get_setting`/`set_setting` return
`rusqlite::Result`, and `?` converts through
`From<rusqlite::Error> for IpcError`. If `lock` is not
`crate::ipc_error::lock`, use the import `mcp/tools/start_rules.rs` uses.

- [ ] **Step 5: Run the tests**

Run: `cargo fleet-test -- service::env`
Expected: all PASS.

- [ ] **Step 6: Verify and commit**

```bash
scripts/verify.sh
git -C <wt> add crates/fleet-core/src/service/env.rs crates/fleet-core/src/service/mod.rs crates/fleet-core/src/ipc_error.rs
git -C <wt> commit -m "feat(env): resolve, render and wrap a session's environment"
```

---

### Task 3: Launch wiring in lifecycle (create, restart, recreate) and `NewSessionArgs.env`

**Files:**
- Modify: `crates/fleet-core/src/service/sessions/lifecycle.rs` (`NewSessionArgs` :11-110; create ~:1121-1180; restart ~:2110; recreate ~:2469)
- Modify: every `NewSessionArgs {` struct literal (21 sites). Find them with
  `graft grep "NewSessionArgs {"` and add `env: None,` to each. Production
  sites:
  - `mcp/tools/session_ops.rs:447`, `:535`
  - `mcp/tools/orchestration.rs:638`
  - `service/operator.rs:638`
  - `service/rewind.rs:842`
  - `service/bg_sessions.rs:352`
  - `service/catalog/author_session.rs:273`
  - `service/trackers/tickets.rs:1553`
  - `service/work/resume.rs:920`
  - `service/routines/tick.rs:118`

  The rest are in tests.
- Test: `crates/fleet-core/src/service/sessions/tests.rs`

**Interfaces:**
- Consumes (Task 2): `service::env::prepare`.
- Produces: `NewSessionArgs.env: Option<BTreeMap<String, String>>`
  (`#[serde(default)]`, `Serialize` if the struct derives it).
  Session-scope rows are stored from it after create.

- [ ] **Step 1: Add the field**

In `NewSessionArgs`:

```rust
    /// Session-scope environment variables (plain values) for this session;
    /// stored as session-scope `env_vars` rows once the row exists.
    #[serde(default)]
    pub env: Option<std::collections::BTreeMap<String, String>>,
```

Add `env: None,` at all 21 literals. Run `cargo fleet-fast-check` and then
`cargo fleet-lint`; both must be clean. The lint covers the test literals.

- [ ] **Step 2: Write the failing tests**

Model them on the nearest existing create/restart tests in
`service/sessions/tests.rs`: search for a test that calls `new_session_inner`
with a fake tmux and records `new_session` pane commands. Then add:

```rust
#[tokio::test]
async fn a_new_session_sources_its_environment_and_stores_the_session_scope() {
    // Arrange: the existing create-test harness (fake tmux recording pane
    // commands, local host, a project). Set one global plain variable and
    // pass env {"FOO": "bar"} in NewSessionArgs.
    // Act: create.
    // Assert:
    //  - the recorded pane command starts with `f="$HOME"/'.claude-fleet/env/`
    //  - it contains neither "bar" nor the global value
    //  - the env file exists under the harness HOME with mode 0o600 and
    //    contains `export FOO='bar'`
    //  - list_env_vars() has a ("session", "<new id>", "FOO") row
}

#[tokio::test]
async fn a_restart_writes_a_fresh_file_with_the_edited_value() {
    // create with global A=1; edit A=2; restart; the restart's pane command
    // names a different file than create's, and that file has A='2'.
}

#[tokio::test]
async fn an_unresolved_reference_refuses_the_create_and_starts_no_tmux_session() {
    // global R = Ref("file:/nonexistent/…"); create returns E_ENV_UNRESOLVED
    // and the fake tmux recorded no new_session.
}
```

Fill in each test body from the harness you find. The comments state the
exact assertions; every one is required. Point `HOME` at a tempdir the way
`tmux.rs`'s `a_profile_session_runs_under_its_config_dir…` test does, so
the local write lands in the tempdir.

- [ ] **Step 3: Run them to verify they fail**

Run: `cargo fleet-test -- service::sessions::tests::a_new_session_sources service::sessions::tests::a_restart_writes service::sessions::tests::an_unresolved_reference`
Expected: FAIL (the pane command is not wrapped).

- [ ] **Step 4: Wire create**

At `lifecycle.rs:1121`, right after `let (claude_id, pane_cmd) = claude_id_and_pane_cmd(&args);`:

```rust
    let env_extra = args.env.clone().unwrap_or_default();
    let pane_cmd = crate::service::env::prepare(
        store, &**ssh, &args.host_alias, Some(args.project_id), None, &env_extra, &pane_cmd,
    )
    .await?;
```

This must come before `ensure_for_new_session` (~:1130), so both launchers
get the wrapped command.

After the new row's id is known (~:1180, where the created session is
looked up), store the session scope:

```rust
    if !env_extra.is_empty() {
        let s = lock(store)?;
        for (name, value) in &env_extra {
            s.set_env_var("session", &session_id.to_string(), name, &crate::store::EnvValue::Plain(value.clone()))?;
        }
    }
```

Use the actual variable name for the new row's id at that point.

- [ ] **Step 5: Wire restart and recreate**

Restart (~:2110), right after `recreate_pane_command(...)`:

```rust
    let project_id = lock(store)?.get_session(/* the args the surrounding code already uses */)?.and_then(|r| r.project_id);
    let pane_cmd = crate::service::env::prepare(
        store, &**ssh, &args.host_alias, project_id, session_id, &Default::default(), &pane_cmd,
    )
    .await?;
```

Replace the `get_session` call with the lookup the surrounding function
already does for this session, if there is one. `session_id:
Option<i64>` is in scope there. All three launch branches (`:2141`,
`:2145`, `:2149`) and `restart_unrepaired` receive this `pane_cmd`.

Recreate (~:2469), after `pane_cmd` is built under the lock and that lock
is dropped (before `:2484`):

```rust
    let pane_cmd = crate::service::env::prepare(
        store, &**ssh, &sess.host_alias, sess.project_id, Some(sess.id), &Default::default(), &pane_cmd,
    )
    .await?;
```

Never call `prepare` while holding the store guard: it locks internally
and awaits.

- [ ] **Step 6: Run the tests**

Run the three tests, then `cargo fleet-test -- service::sessions`.
Expected: all PASS.

- [ ] **Step 7: Verify and commit**

```bash
scripts/verify.sh
git -C <wt> add -u
git -C <wt> commit -m "feat(env): sessions source their environment on create, restart and recreate"
```

---

### Task 4: Launch wiring in repair, review and move

**Files:**
- Modify: `crates/fleet-core/src/service/repair.rs` (~:2469, where the explicit repair builds `pane_cmd`)
- Modify: `crates/fleet-core/src/service/sessions/review.rs` (~:102)
- Modify: `crates/fleet-core/src/service/move_session/mod.rs` (~:2855)

**Interfaces:**
- Consumes: `service::env::prepare` (Task 2).

- [ ] **Step 1: Write the failing tests**

Each file has existing tests that drive its launch with a fake: the move
fake `start_target` at `move_session/mod.rs:3759`, repair's
`Step::TmuxCreate` tests, and review's tests. In each, add one test that
sets a global plain variable `FLEET_T=1`. It asserts that the launched
pane command starts with `f="$HOME"/'.claude-fleet/env/` and does not
contain `FLEET_T=1`.

Name the tests:
- `move_launches_the_target_with_its_environment`
- `an_explicit_repair_launches_with_its_environment`
- `a_review_session_launches_with_its_environment`

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo fleet-test -- with_its_environment`
Expected: 3 FAIL.

- [ ] **Step 3: Wire each site**

For each site, right after the line that builds `pane_cmd`, outside any
store guard:

```rust
let pane_cmd = crate::service::env::prepare(store, ssh_dyn, host, project_id, session_id, &Default::default(), &pane_cmd).await?;
```

Bindings per site:
- **Move:**
  - `host` = `&target` (the target alias)
  - `project_id` = `snap.row.project_id`
  - `session_id` = `Some(snap.row.id)`: the source row. Its session scope
    follows the session; Task 1's trigger drops the rows only when the
    source row is deleted.
  - Copy the source's session-scope rows to the new row where move calls
    `store_launch` (~:3320):
    ```rust
    for r in s.list_env_vars()?.into_iter().filter(|r| r.scope == "session" && r.scope_key == snap.row.id.to_string()) { /* see note */ }
    ```
    `list_env_vars` hides secret values, so a copy must go through the
    store. Add the store method
    `Store::copy_session_env(&self, from: i64, to: i64) -> Result<(), IpcError>`
    to `store/env_vars.rs`:
    ```sql
    INSERT OR REPLACE INTO env_vars (scope, scope_key, name, value, credential_ref, secret, updated_at)
      SELECT 'session', ?2, name, value, credential_ref, secret, updated_at FROM env_vars
      WHERE scope = 'session' AND scope_key = ?1
    ```
    Call it with `(snap.row.id, new_id)`. Update Task 1's pin test count
    from 4 to 5, adding `copy_session_env` to its message.
- **Repair:** the repaired session's host, `project_id` and id, from the
  row the repair already holds.
- **Review:** the review session's host and its project id. `session_id` is
  `None`, because the review row does not exist yet.

`ssh_dyn` is `&**ssh` where `ssh: &Arc<SshClient>`, or the `&dyn SshExec`
the function already has.

- [ ] **Step 4: Run the tests**

Run: `cargo fleet-test -- with_its_environment store::env_vars move_session repair review`
Expected: all PASS.

- [ ] **Step 5: Verify and commit**

```bash
scripts/verify.sh
git -C <wt> add -u
git -C <wt> commit -m "feat(env): repair, review and move launch with the session's environment"
```

---

### Task 5: Tick sweep and diagnostics masking

**Files:**
- Modify: `crates/fleet-core/src/service/env.rs` (add `sweep_env_files`)
- Modify: `crates/fleet-core/src/service/tick.rs` (~:248, after `sweep_secret_dirs`)
- Modify: `crates/fleet-core/src/service/loops.rs` (`LOOPS` ~:63)
- Modify: `crates/fleet-core/src/service/diagnostics.rs` (~:247-271)

**Interfaces:**
- Produces: `pub async fn sweep_env_files(store: &Mutex<Store>, ssh: &dyn SshExec) -> usize`.
  It returns the number of hosts swept.

- [ ] **Step 1: Write the failing tests** (in `service/env.rs` tests)

```rust
    #[tokio::test]
    async fn the_sweep_removes_old_files_and_forgets_a_clean_host() {
        // A temp HOME for the local host (as Task 3's tests do).
        let home = tempfile::tempdir().unwrap();
        let _g = crate::testkit::home_guard(home.path()); // use the helper Task 3 used to point HOME
        let dir = home.path().join(".claude-fleet/env");
        std::fs::create_dir_all(&dir).unwrap();
        let old = dir.join("old.sh");
        let fresh = dir.join("fresh.sh");
        std::fs::write(&old, "x").unwrap();
        std::fs::write(&fresh, "x").unwrap();
        let t = std::time::SystemTime::now() - std::time::Duration::from_secs(11 * 60);
        std::fs::File::options().write(true).open(&old).unwrap().set_modified(t).unwrap();
        let s = store();
        s.lock().unwrap().set_setting(ENV_FILES_HOSTS_KEY, r#"["local"]"#).unwrap();
        let ssh = /* the SshExec fake/local executor Task 3's harness uses */;
        assert_eq!(sweep_env_files(&s, &ssh).await, 1);
        assert!(!old.exists() && fresh.exists());
        // A fresh file remains, so the host is still recorded.
        assert!(s.lock().unwrap().get_setting(ENV_FILES_HOSTS_KEY).unwrap().unwrap().contains("local"));
    }
```

Take the HOME-guard and SshExec bindings from the harness Task 3 used.
They must be real bindings, not new helpers.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo fleet-test -- the_sweep_removes_old_files`
Expected: compile error.

- [ ] **Step 3: Implement**

```rust
/// Delete env files older than ENV_FILE_MAX_AGE_MIN on every recorded host;
/// forget a host once its directory holds no file. Unreachable hosts stay
/// recorded and are retried next tick.
pub async fn sweep_env_files(store: &Mutex<Store>, ssh: &dyn SshExec) -> usize {
    let hosts: Vec<String> = match lock(store).ok().and_then(|s| s.get_setting(ENV_FILES_HOSTS_KEY).ok().flatten()) {
        Some(v) => serde_json::from_str(&v).unwrap_or_default(),
        None => return 0,
    };
    let script = format!(
        "d=\"$HOME\"/'.claude-fleet/env'; [ -d \"$d\" ] || exit 0; \
         find \"$d\" -type f -name '*.sh' -mmin +{ENV_FILE_MAX_AGE_MIN} -exec rm -f {{}} +; \
         ls -A \"$d\" | grep -q . && echo left || echo clean"
    );
    let mut swept = 0;
    let mut keep = Vec::new();
    for host in hosts {
        match crate::ssh::run_shell(ssh, &host, &script, std::time::Duration::from_secs(20)).await {
            Ok(out) => {
                swept += 1;
                if out.trim_end().ends_with("left") {
                    keep.push(host);
                }
            }
            Err(e) => {
                tracing::debug!(host = %host, code = %e.code, "env file sweep failed; retrying next tick");
                keep.push(host);
            }
        }
    }
    if let Ok(s) = lock(store) {
        let _ = if keep.is_empty() {
            s.delete_setting(ENV_FILES_HOSTS_KEY)
        } else {
            s.set_setting(ENV_FILES_HOSTS_KEY, &serde_json::to_string(&keep).unwrap_or_default())
        };
    }
    swept
}
```

Check `run_shell`'s exact signature and return type at `ssh.rs:1281`, and
adapt `out` to it (it may return a struct with `.stdout`). A record
written by `prepare` while the sweep runs could be lost in the
read-modify-write. Re-read the setting under the lock before writing, and
merge in any hosts added since the read.

- [ ] **Step 4: Schedule it and register the loop**

In `tick.rs`, right after `sweep_secret_dirs` (~:248):

```rust
                let env_swept = service::env::sweep_env_files(store, &**ssh).await;
                if env_swept > 0 {
                    tracing::debug!(hosts = env_swept, "env files swept");
                }
```

This reports under the existing `"forms"` loop, which is `keeps` (not
pausable). Edit that loop's `LOOPS` description to "Expires unanswered
forms and deletes their secrets and session environment files from
hosts; pausing would leave secrets behind."

- [ ] **Step 5: Mask stored secrets in diagnostics**

In `diagnostics.rs` (~:247-271), add `s.env_secret_literals()?` to the
tuple that collects `tracker_secret_literals` and
`decision_secret_literals`, and `secrets.extend(env_secrets);` beside
`secrets.extend(decision_secrets);`. Extend the existing diagnostics
masking test with a stored env secret: it must not appear in the bundle.

- [ ] **Step 6: Run the tests and commit**

Run: `cargo fleet-test -- the_sweep_removes_old_files diagnostics loops`, then `scripts/verify.sh`.

```bash
git -C <wt> add -u
git -C <wt> commit -m "feat(env): sweep env files from hosts and mask stored secrets in diagnostics"
```

---

### Task 6: MCP `env_vars` tool, `new_session { env }`, audit redaction

**Files:**
- Create: `crates/fleet-core/src/mcp/tools/env_vars.rs`
- Modify: `crates/fleet-core/src/service/env.rs` (add `EnvVarsArgs` + `run`)
- Modify: `crates/fleet-core/src/mcp/tools/mod.rs` (`mod env_vars;` ~:64; `+ Self::env_vars_router()` in `tool_router()` ~:374-398)
- Modify: `crates/fleet-core/src/mcp/guard.rs` (`TOOL_POLICIES` row; `SKIP_KEYS` add `"ref"`; `COUNT_KEYS` add `"env"`; tests ~:3134)
- Modify: `crates/fleet-core/src/mcp/tools/params.rs:68-123` (`NewSessionParams.env`)
- Modify: `crates/fleet-core/src/mcp/tools/session_ops.rs:447-470` (`env: p.env` in place of Task 3's `env: None`)
- Modify: `crates/fleet-core/src/mcp/tools/tests.rs:1993,2238` (add `env: None`)
- Modify: `skills/claude-fleet-control/SKILL.md` (a short *Environment* section)
- Regenerate: `docs/control-api-reference.md`

**Interfaces:**
- Produces: `service::env::EnvVarsArgs` (Serialize, Deserialize,
  JsonSchema, Default, Clone, Debug).
- Produces:
  `service::env::run(store: &Mutex<Store>, args: &EnvVarsArgs, refs_allowed: bool) -> Result<serde_json::Value, IpcError>`.
  Task 8 consumes both.

- [ ] **Step 1: Write the failing tests**

In `service/env.rs` tests:

```rust
    #[test]
    fn run_sets_lists_and_unsets_without_echoing_secrets() {
        let s = store();
        let set = |name: &str, value: Option<&str>, secret: bool, r: Option<&str>| EnvVarsArgs {
            action: "set".into(), scope: Some("global".into()), scope_key: None,
            name: Some(name.into()), value: value.map(Into::into), r#ref: r.map(Into::into), secret: Some(secret),
        };
        run(&s, &set("A", Some("plain"), false, None), false).unwrap();
        run(&s, &set("S", Some("top-secret-1"), true, None), false).unwrap();
        let listed = run(&s, &EnvVarsArgs { action: "list".into(), ..Default::default() }, false).unwrap().to_string();
        assert!(listed.contains("plain") && !listed.contains("top-secret-1"), "{listed}");
        let e = run(&s, &set("R", None, true, Some("env:X")), false).unwrap_err();
        assert_eq!(e.code, codes::E_FORBIDDEN, "a device may not set a reference");
        run(&s, &set("R", None, true, Some("env:X")), true).unwrap();
        let both = run(&s, &set("B", Some("v"), false, Some("env:X")), true).unwrap_err();
        assert_eq!(both.code, codes::E_INVALID);
        run(&s, &EnvVarsArgs { action: "unset".into(), scope: Some("global".into()), name: Some("A".into()), ..Default::default() }, false).unwrap();
    }
```

Use the same forbidden code the r04 S1 precedent returns
(`orchestration.rs:2650-2661`); if it is not `E_FORBIDDEN`, use that code
here.

In `guard.rs` tests, next to `redact_args_never_keeps_an_api_key`:

```rust
    #[test]
    fn redact_args_never_keeps_an_env_value_or_reference() {
        let args = serde_json::json!({"action":"set","scope":"global","name":"TOKEN","value":"sk-live-abcdef123456","ref":"file:/run/secrets/t"});
        let out = redact_args(args.as_object());
        assert!(!out.contains("sk-live") && !out.contains("/run/secrets"), "{out}");
        assert!(out.contains("name=TOKEN"), "{out}");
        let ns = serde_json::json!({"host_alias":"local","env":{"A":"secret-value-xyz","B":"2"}});
        let out = redact_args(ns.as_object());
        assert!(out.contains("<2 fields>") && !out.contains("secret-value-xyz"), "{out}");
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo fleet-test -- run_sets_lists_and_unsets redact_args_never_keeps_an_env`
Expected: compile error, then FAIL for the guard test.

- [ ] **Step 3: Implement args and `run` in `service/env.rs`**

```rust
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct EnvVarsArgs {
    /// list | set | unset.
    pub action: String,
    /// global | host | project | session (set, unset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// '' (global), host alias, owner/repo, or session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// set: the value (with secret:true it is write-only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// set: env:NAME or file:/abs, resolved at launch (master only).
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub r#ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<bool>,
}

pub fn run(store: &Mutex<Store>, a: &EnvVarsArgs, refs_allowed: bool) -> Result<serde_json::Value, IpcError> {
    use crate::store::EnvValue;
    let need = |v: &Option<String>, what: &str| {
        v.clone().ok_or_else(|| IpcError::new(codes::E_INVALID, format!("{} needs {what}", a.action)))
    };
    match a.action.as_str() {
        "list" => Ok(serde_json::to_value(lock(store)?.list_env_vars()?).unwrap_or_default()),
        "set" => {
            let (scope, name) = (need(&a.scope, "scope")?, need(&a.name, "name")?);
            let key = a.scope_key.clone().unwrap_or_default();
            let v = match (&a.value, &a.r#ref) {
                (Some(_), Some(_)) | (None, None) => {
                    return Err(IpcError::new(codes::E_INVALID, "pass exactly one of value or ref"))
                }
                (None, Some(r)) => {
                    if !refs_allowed {
                        return Err(IpcError::new(
                            codes::E_FORBIDDEN,
                            "only the master token may set a reference: it makes the hub read its own files or environment",
                        ));
                    }
                    EnvValue::Ref(r.clone())
                }
                (Some(v), None) if a.secret.unwrap_or(false) => EnvValue::Secret(Secret::new(v.clone())),
                (Some(v), None) => EnvValue::Plain(v.clone()),
            };
            lock(store)?.set_env_var(&scope, &key, &name, &v)?;
            Ok(serde_json::json!({ "set": name, "scope": scope, "scope_key": key, "applies": "on the next start or restart" }))
        }
        "unset" => {
            let (scope, name) = (need(&a.scope, "scope")?, need(&a.name, "name")?);
            let key = a.scope_key.clone().unwrap_or_default();
            let removed = lock(store)?.unset_env_var(&scope, &key, &name)?;
            Ok(serde_json::json!({ "removed": removed }))
        }
        other => Err(IpcError::new(codes::E_INVALID, format!("unknown action {other:?}: list | set | unset"))),
    }
}
```

- [ ] **Step 4: The MCP tool** (`mcp/tools/env_vars.rs`)

```rust
use super::*;
use crate::service::env;

#[tool_router(router = env_vars_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "Session environment variables, applied on a session's next start: list; set {scope: global|host|project|session, scope_key, name, value|ref, secret?}; unset {scope, scope_key, name}. Secrets and refs are write-only; ref (env:NAME|file:/abs) is master-only. E_INVALID, E_FORBIDDEN.")]
    pub(super) async fn env_vars(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<env::EnvVarsArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit("env_vars", &format!("action={} scope={:?} name={:?}", p.action.escape_debug(), p.scope, p.name));
        if p.action != "list" {
            super::fleet::owner_device_admin(&caller, "change session environment variables")?;
        }
        ok_json_compact(&env::run(&self.store, &p, caller.is_master()).map_err(to_mcp_err)?)
    }
}
```

Check the exact return type of `owner_device_admin` (`fleet.rs:1571`) and
adapt the `?`.

Add the policy row in `guard.rs` `TOOL_POLICIES`:

```rust
    ToolPolicy { name: "env_vars", access: Access::Person, readonly: false, confirm: false, deadline: Deadline::Quick },
```

Update the redaction lists:

```rust
const SKIP_KEYS: &[&str] = &["confirm_nonce", "value", "secret", "code", "api_key", "ref"];
const COUNT_KEYS: &[&str] = &["values", "args", "env"];
```

- [ ] **Step 5: `new_session { env }`**

In `params.rs` `NewSessionParams`, after `confirm_nonce`:

```rust
    /// Session-scope environment variables {NAME: value}; plain values only (use env_vars set secret:true for secrets).
    #[serde(default)]
    pub env: Option<std::collections::BTreeMap<String, String>>,
```

Map it at `session_ops.rs:447` (`env: p.env.clone(),` or `env: p.env,`,
depending on ownership there). Add `env: None` to the two test literals
in `mcp/tools/tests.rs`.

- [ ] **Step 6: Run, regenerate, document**

```bash
cargo fleet-test -- run_sets_lists_and_unsets redact_args_never_keeps_an_env every_router_tool_has_exactly_one_tool_policy_row the_served_definition_budget_stays_bounded
REGEN_DOCS=1 cargo fleet-test -- reference_is_current   # fails on purpose; read the diff
cargo fleet-test -- reference_is_current
```

If the budget test fails, shorten the tool description; do not raise the
budget. Add to `skills/claude-fleet-control/SKILL.md` a 6–10 line
*Environment* section covering:
- the scopes and their precedence;
- that changes apply on the next start;
- that secrets go in as `secret:true` or through an `ask` secret field with
  `env_target`;
- that `ref` is master-only.

- [ ] **Step 7: Verify and commit**

```bash
scripts/verify.sh
git -C <wt> add -A crates/fleet-core skills/claude-fleet-control/SKILL.md docs/control-api-reference.md
git -C <wt> commit -m "feat(env): env_vars MCP tool and new_session env"
```

---

### Task 7: Form field `env_target`

**Files:**
- Modify: `crates/fleet-core/src/pages/forms.rs` (`FormField` ~:161-210; validation ~:511-515)
- Modify: `crates/fleet-core/src/service/forms.rs` (`answer` ~:479-558, secrets loop ~:518)
- Modify: `docs/forms.md` (*Secrets* section, one paragraph)
- Regenerate: `docs/form-spec.schema.json`

**Interfaces:**
- Consumes: `Store::set_env_var`, `check_env_name`, `EnvValue::Secret`.

- [ ] **Step 1: Write the failing tests**

In `pages/forms_tests.rs`:

```rust
#[test]
fn env_target_is_for_secret_fields_and_must_be_a_variable_name() {
    let ok = r#"{"title":"t","fields":[{"name":"tok","type":"secret","label":"Token","env_target":"GITHUB_TOKEN"}]}"#;
    assert!(parse_form(ok).is_ok());
    let not_secret = r#"{"title":"t","fields":[{"name":"x","type":"text","label":"X","env_target":"A"}]}"#;
    assert!(parse_form(not_secret).is_err());
    let reserved = r#"{"title":"t","fields":[{"name":"x","type":"secret","label":"X","env_target":"PATH"}]}"#;
    assert!(parse_form(reserved).is_err());
}
```

Use the spec-parsing entry point the neighbouring tests in that file use
in place of `parse_form`, and its minimal valid spec shape. Then add a
service test in `service/forms.rs`'s tests, following the existing
`answer` secret-write test:

```rust
// a form with field tok {type: secret, env_target: "GITHUB_TOKEN"} answered
// with "ghp_xyz…" stores ("session", "<form's session id>", "GITHUB_TOKEN")
// as a secret, writes NO host file for that field, and the answer's
// `secrets` map does not name it.
```

Implement it with that test's harness. Every assertion in the comment is
required.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo fleet-test -- env_target`
Expected: FAIL.

- [ ] **Step 3: Implement**

In `FormField`, after `secret_note`:

```rust
    /// secret: store the answer as this session's environment variable NAME
    /// (secret, write-only) instead of writing a host file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_target: Option<String>,
```

In validation, next to the `secret_note` match:

```rust
    match (&f.env_target, f.kind) {
        (Some(t), Secret) => {
            if let Err(e) = crate::store::check_env_name(t) {
                v.error(at, "env_target", &e.message);
            }
        }
        (Some(_), _) => not_for(v, "env_target"),
        (None, _) => {}
    }
```

Use the validator's real error-recording method in place of `v.error(...)`,
the one `v.text` uses internally.

In `service::forms::answer`, before the secrets loop, split off the fields
that have an `env_target`:

```rust
        let env_fields: Vec<(String, String)> = form
            .fields
            .iter()
            .filter_map(|f| f.env_target.clone().map(|t| (f.name.clone(), t)))
            .collect();
        for (field, target) in &env_fields {
            if let Some(secret) = answers.secrets.remove(field) {
                lock(store)?.set_env_var(
                    "session",
                    &row.session_id.to_string(),
                    target,
                    &crate::store::EnvValue::Secret(crate::store::Secret::new(secret)),
                )?;
            }
        }
```

`answers` must be `mut`. If `answers.secrets` is now empty, the
host-write branch must be skipped entirely: no `secret_dir`, no
`mark_form_secrets_pending`. Check the existing guard condition and keep
it keyed on the now-reduced map.

- [ ] **Step 4: Run, regenerate, document**

```bash
cargo fleet-test -- env_target forms
REGEN_FORM_DOCS=1 cargo fleet-test -- form_docs_are_current   # fails on purpose
cargo fleet-test -- form_docs_are_current
```

Add one paragraph to `docs/forms.md` → *Secrets*: `env_target` stores the
answer as the session's secret environment variable. No host file is
written, and it applies on the session's next start or restart.

- [ ] **Step 5: Verify and commit**

```bash
scripts/verify.sh
git -C <wt> add -u docs/form-spec.schema.json
git -C <wt> commit -m "feat(env): form secret fields can target a session environment variable"
```

---

### Task 8: Tauri command, hub routing, contract 18

**Files:**
- Create: `src-tauri/src/commands/env_vars.rs`
- Modify: `src-tauri/src/commands/mod.rs` (`pub mod env_vars;`)
- Modify: `src-tauri/src/lib.rs` (~:640, invoke handler: `commands::env_vars::env_vars, commands::env_vars::env_vars_list,`)
- Modify: `src-tauri/src/backend/verdicts.rs` (~:1003)
- Modify: `src-tauri/src/backend/tests_routing.rs` (case in `routed_mutation_cases_but_the_catalog` ~:2921; `SOURCES` ~:7283)
- Modify: `crates/fleet-core/src/wire_contract.rs` (history bullet + `CONTRACT_REVISION = 18`)
- Modify: `src-tauri/src/backend/contract.rs` (`MAX_HUB_CONTRACT = 18` ~:206 with its doc paragraph; MIN unchanged)
- Modify: `docs/status.md` (~:28, "The hub contract is revision 18")
- Create: `src/lib/env_vars.ts`
- Regenerate: hub verdicts, hub contract, local-only list

**Interfaces:**
- Consumes (Task 6): `service::env::{EnvVarsArgs, run}`.
- Produces:
  - Tauri command `env_vars(args: EnvVarsArgs)`.
  - Tauri command `env_vars_list()` with no arguments, for the resource
    page's list (Task 9). It routes to tool `env_vars` with
    `{action:"list"}`.

- [ ] **Step 1: Write the routing cases (failing)**

In `routed_mutation_cases_but_the_catalog`:

```rust
        (
            "env_vars",
            "env_vars",
            json!({ "action": "set", "scope": "global", "name": "A", "value": "v", "secret": false }),
            r#"{"set":"A","scope":"global","scope_key":"","applies":"on the next start or restart"}"#,
            Box::new(|b, s, _| {
                block_on(commands::env_vars::routed::env_vars(b, s, fleet_core::service::env::EnvVarsArgs {
                    action: "set".into(), scope: Some("global".into()), scope_key: None,
                    name: Some("A".into()), value: Some("v".into()), r#ref: None, secret: Some(false),
                }))
                .map(|_| ())
            }),
        ),
```

In `routed_read_cases_but_org_admin`:

```rust
        (
            "env_vars_list",
            "env_vars",
            json!({ "action": "list" }),
            r#"[]"#,
            Box::new(|b, s, _| block_on(commands::env_vars::routed::env_vars_list(b, s)).map(|_| ())),
        ),
```

Add `("commands/env_vars.rs", include_str!("../commands/env_vars.rs")),`
to `SOURCES`.

- [ ] **Step 2: Implement the command**

```rust
//! Session environment variables (spec 2026-10-11-session-env-and-secrets-design.md).
use std::sync::{Arc, Mutex};

use fleet_core::ipc_error::IpcError;
use fleet_core::service::env::{self, EnvVarsArgs};
use fleet_core::store::Store;
use tauri::State;

use crate::backend::FleetBackend;

#[tauri::command]
pub async fn env_vars(
    args: EnvVarsArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<serde_json::Value, IpcError> {
    routed::env_vars(&backend, &store, args).await
}

#[tauri::command]
pub async fn env_vars_list(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<serde_json::Value, IpcError> {
    routed::env_vars_list(&backend, &store).await
}

pub(crate) mod routed {
    use super::*;

    pub async fn env_vars(backend: &FleetBackend, store: &Mutex<Store>, args: EnvVarsArgs) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("env_vars", &args).await,
            // The standalone desktop is its own master: references allowed.
            None => env::run(store, &args, true),
        }
    }

    pub async fn env_vars_list(backend: &FleetBackend, store: &Mutex<Store>) -> Result<serde_json::Value, IpcError> {
        env_vars(backend, store, EnvVarsArgs { action: "list".into(), ..Default::default() }).await
    }
}
```

Copy the `use` lines for `FleetBackend` and `State` from
`commands/api_tokens.rs`. `route` looks the tool up in `VERDICTS` by
command name. Read `remote.rs:605`: if it keys on the command, the
`env_vars_list` row must name tool `env_vars`, which these rows do.

Verdict rows:

```rust
    ("env_vars", Verdict::Routed { tool: "env_vars" }),
    ("env_vars_list", Verdict::Routed { tool: "env_vars" }),
```

- [ ] **Step 3: Contract 18**

In `wire_contract.rs`, add before `CONTRACT_REVISION`:

```rust
//! - **18** — *session environment variables* (spec 2026-10-11). New tool:
//!   `env_vars`; `new_session` gains `env`. A desktop at 18 still accepts a
//!   16/17 hub; there its environment commands fail as unknown tools.
```

Set `pub const CONTRACT_REVISION: u32 = 18;`.

In `contract.rs`, set `pub const MAX_HUB_CONTRACT: u32 = 18;` and append
"Raised to 18 with revision 18 (session environment variables)." to its
doc. Leave `MIN_HUB_CONTRACT` at 16.

- [ ] **Step 4: Frontend wrapper** (`src/lib/env_vars.ts`)

```ts
import { invokeCmd, type Result } from './result';

export type EnvScope = 'global' | 'host' | 'project' | 'session';
export interface EnvVarRow {
  id: number;
  scope: EnvScope;
  scope_key: string;
  name: string;
  value?: string;
  secret: boolean;
  is_ref: boolean;
  updated_at: number;
}
export interface EnvVarsArgs {
  action: 'list' | 'set' | 'unset';
  scope?: EnvScope;
  scope_key?: string;
  name?: string;
  value?: string;
  ref?: string;
  secret?: boolean;
}

export const envVars = <T = unknown>(args: EnvVarsArgs): Promise<Result<T>> =>
  invokeCmd<T>('env_vars', { args });
export const listEnvVars = (): Promise<Result<EnvVarRow[]>> => invokeCmd<EnvVarRow[]>('env_vars_list');
```

- [ ] **Step 5: Run and regenerate**

```bash
mkdir -p /dev/shm/fleet-tests 2>/dev/null; cargo fleet-test -- backend::   # TMPDIR=/dev/shm/fleet-tests on Linux
REGEN_HUB_VERDICTS=1 cargo fleet-test -- verdict_gen          # fails on purpose
REGEN_HUB_CONTRACT=1 cargo fleet-test -- contract              # fails on purpose; use the contract test's real filter
REGEN_LOCAL_ONLY=1 cargo fleet-test -- local_only              # only if that test lists it; use its real filter
REGEN_DOCS=1 cargo fleet-test -- reference_is_current          # new frontend commands are listed too
cargo fleet-test -- backend:: verdict_gen reference_is_current
npx svelte-check
```

Expected: all pass on the second run. If the contract golden test reports
a new wire field without `serde(default)`, that is a shipped outage
against older hubs. Every new field must default.

- [ ] **Step 6: Verify and commit**

```bash
scripts/verify.sh
git -C <wt> add -A src-tauri crates/fleet-core/src/wire_contract.rs src/lib docs
git -C <wt> commit -m "feat(env): routed env_vars command and hub contract 18"
```

---

### Task 9: Settings page and docs

**Files:**
- Modify: `crates/fleet-core/src/pages/resources.rs` (a `const ENV_VAR: ResourceType` after `PEER_LINK` ~:1398; add it to `RESOURCES` ~:1400)
- Create: `crates/fleet-core/pages/settings.env.json`
- Modify: `crates/fleet-core/src/pages/mod.rs:62` (`PAGE_FILES`)
- Modify: `crates/fleet-core/pages/settings.json` (a link)
- Modify: `src/lib/settings_tree.ts` (a leaf)
- Modify: `docs/concepts.md` (a new `## Environment` section before `## Asset catalog`)
- Modify: `docs/status.md` (one paragraph near the chat-forms paragraph ~:734)
- Regenerate: page docs

- [ ] **Step 1: The resource type**

The delete action needs `scope`, `scope_key` and `name` from the record.
`ActionSpec` binds `Bind::Record(field)` and `Bind::Const`. Copy the
binding API from `PEER_LINK` and the resources of other pages; if
`Bind::Const` does not exist, use whatever constant binding the module
offers.

```rust
const ENV_VAR: ResourceType = ResourceType {
    id: "env_var",
    label: "Environment variable",
    plural: "Environment variables",
    help: "Set in every session fleet starts, by scope: global, then host, then project, then session (narrower wins). Changes apply when a session next starts or restarts. Secret values are never shown again.",
    list: "env_vars_list",
    id_field: "id",
    title_field: "name",
    color_field: None,
    empty: "No variables. Add one to have it in every new session.",
    fields: &[
        FieldSpec::new("name", "Name", "The variable's name.", FieldKind::Text { max: 128 }),
        FieldSpec::new("scope", "Scope", "global, host, project or session.", FieldKind::Text { max: 16 }),
        FieldSpec::new("scope_key", "Applies to", "The host, owner/repo or session id; empty for global.", FieldKind::Text { max: 255 }),
        FieldSpec::new("value", "Value", "Shown for plain values only.", FieldKind::Text { max: 32768 }),
        FieldSpec::new("secret", "Secret", "Write-only.", FieldKind::Bool),
    ],
    create: Some(
        ActionSpec::new("env_var.set", "Add variable", "env_vars", &[
            ("action", Bind::Const("set")),
            ("scope", Bind::Param("scope")),
            ("scope_key", Bind::Param("scope_key")),
            ("name", Bind::Param("name")),
            ("value", Bind::Param("value")),
            ("secret", Bind::Param("secret")),
        ])
        .params(&[
            param("scope", "Scope", ParamKind::Choice { options: &["global", "host", "project"] }, true),
            param("scope_key", "Host or owner/repo", text(255, "mercury  ·  owner/repo  ·  empty for global"), false),
            param("name", "Name", text(128, "DATABASE_URL"), true),
            param("value", "Value", ParamKind::Secret, true),
            param("secret", "Keep it secret (never shown again)", ParamKind::Toggle { default: true }, false),
        ]),
    ),
    update: None,
    delete: Some(
        ActionSpec::new("env_var.remove", "Remove", "env_vars", &[
            ("action", Bind::Const("unset")),
            ("scope", Bind::Record("scope")),
            ("scope_key", Bind::Record("scope_key")),
            ("name", Bind::Record("name")),
        ])
        .confirm("Remove this variable? Sessions keep it until they restart."),
    ),
    actions: &[],
    create_flow: None,
    variant_by: None,
};
```

Match `FieldKind`/`ParamKind` variant spellings to `resources.rs`. A
missing `FieldKind::Bool` → the module's boolean kind; `Choice { options }`
→ its real shape. Use `Envelope::Args` if the ActionSpec builder needs it
named (the command takes `{ args }`).

- [ ] **Step 2: The page and its links**

`crates/fleet-core/pages/settings.env.json` copies the structure of
`pages/settings.devices.json` exactly, with:
- `"id": "settings.env"`
- `"title": "Environment"`
- the `master_detail` widget on resource `"env_var"`
- `"table": {"columns": ["name", "scope", "scope_key", "secret"], "filters": ["scope"], "group_by": "scope"}`

Add it to `PAGE_FILES`, add `{"type":"link","page":"settings.env"}` to
`settings.json`, and add the leaf in `settings_tree.ts`, all by the
`settings.devices` pattern.

- [ ] **Step 3: Run the page tests and regenerate**

```bash
cargo fleet-test -- pages:: resource_commands_exist
REGEN_PAGE_DOCS=1 cargo fleet-test -- page_docs_are_current   # fails on purpose
cargo fleet-test -- page_docs_are_current
npx vitest run src/lib/pages src/lib/settings_tree
npx svelte-check
```

- [ ] **Step 4: Docs**

Add `docs/concepts.md` → `## Environment`, 8–12 lines, covering:
- scopes and precedence;
- plain values against secrets, and references (`env:`/`file:`, resolved
  by the hub or desktop that starts the session, master-only, trimmed);
- that changes apply on the next start or restart;
- how values reach the session (a 0600 file under `~/.claude-fleet/env`,
  sourced by the pane, swept after 10 minutes; never in an argv or the
  tmux environment);
- that the values are stored unencrypted in `state.db`, like the other
  fleet secrets;
- that a paired desktop's page is read-only, so edit through Control or
  MCP.

Add `docs/status.md`, one paragraph:
- session environment variables landed (spec name);
- contract 18;
- not built: encryption at rest, `op://`, per-worktree scope, a session
  panel editor.

- [ ] **Step 5: Final verification and commit**

```bash
scripts/verify.sh full
git -C <wt> add -A crates/fleet-core/pages crates/fleet-core/src/pages src/lib/settings_tree.ts docs
git -C <wt> commit -m "feat(env): Settings › Environment page and docs"
```

---

## Self-review notes

| Spec item | Where |
|---|---|
| Table, trigger, precedence | Task 1 |
| Host merge carries host-scope rows (rev 9) | Task 1 |
| Validation: `E_INVALID`, reserved names, sizes | Tasks 1 and 2 |
| `E_ENV_UNRESOLVED` | Task 2 |
| Nonce file, `"$HOME"/'…'` wrap, no self-delete (rev 2, 3, 4) | Task 2 |
| Every launch site (rev 5) | Tasks 3 and 4 |
| Session scope stored after create (rev 2) | Task 3 |
| Move copies the session scope | Task 4 |
| Sweep with the host record surviving restarts | Task 5 |
| Diagnostics masking (rev 10) | Task 5 |
| MCP tool | Task 6 |
| Ref master-only (rev 6) | Task 6 |
| Audit redaction | Task 6 |
| `new_session.env` | Task 6 (mapping); field in Task 3 |
| Forms `env_target` | Task 7 |
| Routed commands, contract 18 with MIN unchanged (rev 8) | Task 8 |
| Resource page (rev 7) | Task 9 |
| Docs | Tasks 6–9 |
