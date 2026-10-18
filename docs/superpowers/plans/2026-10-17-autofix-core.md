# Autofix core (PR 1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The hub detects known broken states, stores each as a finding with a plain-language explanation and a fix, applies safe fixes by itself, and exposes everything through an `autofix` MCP tool.

**Architecture:** Pure detectors read a `ScanCx` snapshot that the runner gathers under one brief store lock. Findings become episodes in `autofix_findings` (`(kind, subject)` unique while live). A fix is a serialisable `FixPlan` the hub wrote itself, executed by a `FixExec` trait (real SSH impl + a fake for tests). After an apply the runner re-scans at once. A state that clears goes to `cleared`; one still reported after the verify window goes to `apply_failed`. The runner rides the reconcile tick behind `loops::gate_in("autofix")`, detached like `repair_tick`.

**Tech Stack:** Rust (fleet-core: rusqlite, tokio, rmcp, serde, schemars, async_trait), SQLite migrations, TypeScript settings mirror, JSON settings page.

**Spec:** `docs/superpowers/specs/2026-10-17-autofix-design.md`

## Global Constraints

- Work in `/Users/martinjanci/projects/claude-fleet/.worktrees/feat-autofix` on branch `feat/autofix`. Run git as `/usr/bin/git` (an `rtk` rewrite hook blocks plain `git` in this worktree).
- Names: module `service/autofix/`, store `store/autofix.rs`, MCP tool `autofix`, settings `autofix.*`, loop `autofix`. Never `repair`, `playbook`, `doctor`, `proposal`.
- Migration: `195_autofix.sql` for now. PR #932 also uses 195. Whichever merges second runs `scripts/renumber-migrations.sh` after merging `origin/main` (`migrations_are_contiguous_from_one` forbids a gap, so we cannot pre-take 196).
- No token ever leaves the store in a finding, log line or MCP result. Evidence names tokens by `name` and a 12-char sha prefix only.
- Finding copy is English, plain, second person, no jargon without a gloss.
- `apply` / `dismiss` are refused for the operator (`Caller::is_operator`) and for any caller that is not a trusted full device: `E_FORBIDDEN`.
- Validation ladder (CLAUDE.md): `cargo fleet-fast-check` after edits, `cargo fleet-test -- <filter>` for the task's tests, `scripts/verify.sh` before each commit, `scripts/verify.sh full` before the PR. Never `cargo build` to check compilation; never `-p`.
- Heavy runs (`verify.sh full`, the whole suite) go through the `mercury-run` skill or run in the foreground with `timeout: 600000`.

## Deviations from the spec (decided while planning; Task 8 writes them into the spec)

1. **Audit** goes to the existing admin audit log (`Store::record_admin_event_or_log`), not a new `autofix_audit` table.
2. **States** are `open | verifying | cleared | apply_failed | dismissed`. `verifying` = a fix ran, waiting for the next scans to confirm. `AUTOFIX_VERIFY_SECS = 600`.
3. **`host.provision_stale`** is `Safe` but `auto: false`: the existing `reprovision` loop already refreshes stale hosts. Autofix explains it and offers "do it now".
4. **`host.claude_behind`** has no fix (fleet has no Claude update path). It is explanation-only.
5. **Per-host tokens** are not served the tool in v1 (`Access::PersonDevice`).
6. **Detector 1 vs PR #932**: #932 binds the token at birth and backfills it. Detector 1 still ships; it catches tokens unbound later.

## File Structure

| File | Responsibility |
|---|---|
| `crates/fleet-core/migrations/195_autofix.sql` | the `autofix_findings` table |
| `crates/fleet-core/src/store/autofix.rs` | row type and CRUD for findings |
| `crates/fleet-core/src/service/autofix/mod.rs` | types (`Finding`, `Fix`, `FixPlan`, `Severity`, `Safety`), `ScanCx`, `gather`, `scan_all`, detector registry |
| `crates/fleet-core/src/service/autofix/operator_unbound.rs` | detector 1 |
| `crates/fleet-core/src/service/autofix/host_stale.rs` | detector 4 |
| `crates/fleet-core/src/service/autofix/sync.rs` | findings → episodes; clear, expire, verify window |
| `crates/fleet-core/src/service/autofix/apply.rs` | `Config`, `auto_eligible`, `FixExec`, `SshFixExec`, `apply_one`, `run_pass` |
| `crates/fleet-core/src/service/autofix/tick.rs` | `maybe_run` (detached, gated) |
| `crates/fleet-core/src/mcp/tools/autofix.rs` | the `autofix` tool |
| `crates/fleet-core/src/service/settings.rs`, `src/lib/fleet_settings.ts`, `crates/fleet-core/pages/settings.automation.json` | settings |
| `crates/fleet-core/src/service/loops.rs`, `crates/fleet-core/src/service/tick.rs` | loop registry and tick call |
| `crates/fleet-core/src/mcp/tools/mod.rs`, `crates/fleet-core/src/mcp/guard.rs`, `crates/fleet-core/src/mcp/tools/tests.rs` | tool wiring |
| `docs/architecture.md`, `docs/control-api.md`, `docs/control-api-reference.md`, settings docs, spec | docs |

---

### Task 1: Migration and store

**Files:**
- Create: `crates/fleet-core/migrations/195_autofix.sql`
- Create: `crates/fleet-core/src/store/autofix.rs`
- Modify: `crates/fleet-core/src/store/schema.rs` (append to `MIGRATIONS`, after the `194` entry ~:2194)
- Modify: `crates/fleet-core/src/store/mod.rs` (`mod autofix;` in the alphabetical list ~:72; `pub use autofix::{AutofixRow, NewAutofixFinding};` near :247)

**Interfaces:**
- Produces:
  - `pub struct AutofixRow { id: i64, kind: String, subject: String, host_alias: Option<String>, severity: String, body: String, state: String, first_seen_at: i64, last_seen_at: i64, dismissed_until: Option<i64>, attempts: i64, last_attempt_at: Option<i64>, last_error: Option<String> }`
  - `pub struct NewAutofixFinding<'a> { kind: &'a str, subject: &'a str, host_alias: Option<&'a str>, severity: &'a str, body: &'a str }`
  - `pub const AUTOFIX_LIVE: [&str; 4] = ["open", "verifying", "apply_failed", "dismissed"];`
  - `Store::upsert_autofix_finding(&self, f: &NewAutofixFinding<'_>, now: i64) -> rusqlite::Result<(i64, bool)>` (bool = newly opened)
  - `Store::live_autofix_findings(&self) -> rusqlite::Result<Vec<AutofixRow>>`
  - `Store::autofix_finding(&self, id: i64) -> rusqlite::Result<Option<AutofixRow>>`
  - `Store::set_autofix_state(&self, id: i64, state: &str, error: Option<&str>, now: i64) -> rusqlite::Result<bool>`
  - `Store::mark_autofix_attempt(&self, id: i64, now: i64) -> rusqlite::Result<bool>` (attempts+1, last_attempt_at=now, state='verifying')
  - `Store::dismiss_autofix_finding(&self, id: i64, until: i64) -> rusqlite::Result<bool>`

- [ ] **Step 1: Write the migration**

`crates/fleet-core/migrations/195_autofix.sql`:

```sql
-- Autofix (docs/superpowers/specs/2026-10-17-autofix-design.md): one row per
-- EPISODE of a detected broken state. `(kind, subject)` is unique while the
-- row is live; a cleared row stays as history and a recurrence opens a new one.
CREATE TABLE IF NOT EXISTS autofix_findings (
  id              INTEGER PRIMARY KEY,
  kind            TEXT NOT NULL,
  subject         TEXT NOT NULL,
  host_alias      TEXT,
  severity        TEXT NOT NULL,
  body            TEXT NOT NULL,
  state           TEXT NOT NULL DEFAULT 'open',
  first_seen_at   INTEGER NOT NULL,
  last_seen_at    INTEGER NOT NULL,
  dismissed_until INTEGER,
  attempts        INTEGER NOT NULL DEFAULT 0,
  last_attempt_at INTEGER,
  last_error      TEXT
);
CREATE UNIQUE INDEX IF NOT EXISTS autofix_findings_live
  ON autofix_findings(kind, subject)
  WHERE state IN ('open', 'verifying', 'apply_failed', 'dismissed');
INSERT OR IGNORE INTO schema_version (version) VALUES (195);
```

- [ ] **Step 2: Register it**

In `store/schema.rs`, after the `194` entry:

```rust
    // Autofix: `autofix_findings`, one row per episode of a detected broken
    // state (service/autofix). A new table.
    Migration {
        version: 195,
        sql: include_str!("../../migrations/195_autofix.sql"),
        already_applied: None,
    },
```

- [ ] **Step 3: Write the failing store tests**

At the bottom of `store/autofix.rs` (create the file with only the tests module and `use super::Store;` first so it fails to compile on the missing methods):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn new<'a>(kind: &'a str, subject: &'a str) -> NewAutofixFinding<'a> {
        NewAutofixFinding { kind, subject, host_alias: Some("h1"), severity: "blocking", body: "{}" }
    }

    #[test]
    fn the_same_kind_and_subject_is_one_live_episode() {
        let s = Store::open_in_memory().unwrap();
        let (a, opened) = s.upsert_autofix_finding(&new("operator_unbound", "token:ux-agent"), 10).unwrap();
        assert!(opened);
        let (b, opened) = s.upsert_autofix_finding(&new("operator_unbound", "token:ux-agent"), 20).unwrap();
        assert_eq!(a, b);
        assert!(!opened);
        let row = s.autofix_finding(a).unwrap().unwrap();
        assert_eq!((row.first_seen_at, row.last_seen_at, row.state.as_str()), (10, 20, "open"));
    }

    #[test]
    fn a_cleared_episode_lets_a_recurrence_open_a_new_row() {
        let s = Store::open_in_memory().unwrap();
        let (a, _) = s.upsert_autofix_finding(&new("k", "hub"), 10).unwrap();
        assert!(s.set_autofix_state(a, "cleared", None, 11).unwrap());
        let (b, opened) = s.upsert_autofix_finding(&new("k", "hub"), 12).unwrap();
        assert!(opened);
        assert_ne!(a, b);
        assert_eq!(s.live_autofix_findings().unwrap().len(), 1);
    }

    #[test]
    fn an_attempt_counts_and_moves_to_verifying() {
        let s = Store::open_in_memory().unwrap();
        let (a, _) = s.upsert_autofix_finding(&new("k", "hub"), 10).unwrap();
        assert!(s.mark_autofix_attempt(a, 15).unwrap());
        let row = s.autofix_finding(a).unwrap().unwrap();
        assert_eq!((row.attempts, row.last_attempt_at, row.state.as_str()), (1, Some(15), "verifying"));
    }

    #[test]
    fn dismiss_records_until() {
        let s = Store::open_in_memory().unwrap();
        let (a, _) = s.upsert_autofix_finding(&new("k", "hub"), 10).unwrap();
        assert!(s.dismiss_autofix_finding(a, 999).unwrap());
        let row = s.autofix_finding(a).unwrap().unwrap();
        assert_eq!((row.state.as_str(), row.dismissed_until), ("dismissed", Some(999)));
    }
}
```

- [ ] **Step 4: Run them to verify they fail**

Run: `cargo fleet-test -- store::autofix`
Expected: compile error, `no method named upsert_autofix_finding`.

- [ ] **Step 5: Implement the store**

Top of `store/autofix.rs`:

```rust
//! Autofix findings (service/autofix): one row per episode of a detected
//! broken state. See `migrations/195_autofix.sql`.

use super::Store;
use rusqlite::{OptionalExtension, Result};

/// The states in which a row is the live episode for its `(kind, subject)`.
pub const AUTOFIX_LIVE: [&str; 4] = ["open", "verifying", "apply_failed", "dismissed"];

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AutofixRow {
    pub id: i64,
    pub kind: String,
    pub subject: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_alias: Option<String>,
    pub severity: String,
    /// JSON: `service::autofix::FindingBody`.
    pub body: String,
    pub state: String,
    pub first_seen_at: i64,
    pub last_seen_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dismissed_until: Option<i64>,
    pub attempts: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_attempt_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

pub struct NewAutofixFinding<'a> {
    pub kind: &'a str,
    pub subject: &'a str,
    pub host_alias: Option<&'a str>,
    pub severity: &'a str,
    pub body: &'a str,
}

const COLS: &str = "id, kind, subject, host_alias, severity, body, state, first_seen_at, \
                    last_seen_at, dismissed_until, attempts, last_attempt_at, last_error";
const LIVE_SQL: &str = "state IN ('open', 'verifying', 'apply_failed', 'dismissed')";

fn row(r: &rusqlite::Row<'_>) -> Result<AutofixRow> {
    Ok(AutofixRow {
        id: r.get(0)?,
        kind: r.get(1)?,
        subject: r.get(2)?,
        host_alias: r.get(3)?,
        severity: r.get(4)?,
        body: r.get(5)?,
        state: r.get(6)?,
        first_seen_at: r.get(7)?,
        last_seen_at: r.get(8)?,
        dismissed_until: r.get(9)?,
        attempts: r.get(10)?,
        last_attempt_at: r.get(11)?,
        last_error: r.get(12)?,
    })
}

impl Store {
    /// Refresh the live episode for `(kind, subject)`, or open one. Returns
    /// the row id and whether it was newly opened.
    pub fn upsert_autofix_finding(&self, f: &NewAutofixFinding<'_>, now: i64) -> Result<(i64, bool)> {
        let tx = self.conn.unchecked_transaction()?;
        let live: Option<i64> = tx
            .query_row(
                &format!("SELECT id FROM autofix_findings WHERE kind = ?1 AND subject = ?2 AND {LIVE_SQL}"),
                rusqlite::params![f.kind, f.subject],
                |r| r.get(0),
            )
            .optional()?;
        let out = match live {
            Some(id) => {
                tx.execute(
                    "UPDATE autofix_findings SET last_seen_at = ?2, severity = ?3, body = ?4, host_alias = ?5 WHERE id = ?1",
                    rusqlite::params![id, now, f.severity, f.body, f.host_alias],
                )?;
                (id, false)
            }
            None => {
                tx.execute(
                    "INSERT INTO autofix_findings (kind, subject, host_alias, severity, body, state, first_seen_at, last_seen_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, 'open', ?6, ?6)",
                    rusqlite::params![f.kind, f.subject, f.host_alias, f.severity, f.body, now],
                )?;
                (tx.last_insert_rowid(), true)
            }
        };
        tx.commit()?;
        Ok(out)
    }

    pub fn live_autofix_findings(&self) -> Result<Vec<AutofixRow>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {COLS} FROM autofix_findings WHERE {LIVE_SQL} ORDER BY first_seen_at, id"
        ))?;
        let rows = st.query_map([], row)?.collect();
        rows
    }

    pub fn autofix_finding(&self, id: i64) -> Result<Option<AutofixRow>> {
        self.conn
            .query_row(&format!("SELECT {COLS} FROM autofix_findings WHERE id = ?1"), [id], row)
            .optional()
    }

    pub fn set_autofix_state(&self, id: i64, state: &str, error: Option<&str>, now: i64) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE autofix_findings SET state = ?2, last_error = ?3, last_seen_at = MAX(last_seen_at, ?4) WHERE id = ?1",
            rusqlite::params![id, state, error, now],
        )?;
        Ok(n == 1)
    }

    pub fn mark_autofix_attempt(&self, id: i64, now: i64) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE autofix_findings SET attempts = attempts + 1, last_attempt_at = ?2, state = 'verifying', last_error = NULL WHERE id = ?1",
            rusqlite::params![id, now],
        )?;
        Ok(n == 1)
    }

    pub fn dismiss_autofix_finding(&self, id: i64, until: i64) -> Result<bool> {
        let n = self.conn.execute(
            &format!("UPDATE autofix_findings SET state = 'dismissed', dismissed_until = ?2 WHERE id = ?1 AND {LIVE_SQL}"),
            rusqlite::params![id, until],
        )?;
        Ok(n == 1)
    }
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo fleet-test -- store::autofix store::schema`
Expected: PASS, including `migrations_are_contiguous_from_one` and `every_migration_records_its_own_version`.

- [ ] **Step 7: Commit**

```bash
scripts/verify.sh
/usr/bin/git add crates/fleet-core/migrations/195_autofix.sql crates/fleet-core/src/store/autofix.rs crates/fleet-core/src/store/schema.rs crates/fleet-core/src/store/mod.rs
/usr/bin/git commit -m "feat(autofix): findings table and store"
```

---

### Task 2: Types, registry and detector 1 (`operator_unbound`)

**Files:**
- Create: `crates/fleet-core/src/service/autofix/mod.rs`
- Create: `crates/fleet-core/src/service/autofix/operator_unbound.rs`
- Modify: `crates/fleet-core/src/service/mod.rs` (`pub mod autofix;` alphabetically, near `pub mod attention;`)

**Interfaces:**
- Consumes: `ClientTokenRow` (store/rows.rs:1627: `name`, `token_sha256`, `revoked_at`, `person_id`), `OPERATOR_CLIENT_NAME` (service/operator.rs:253).
- Produces:
  - `pub enum Severity { Blocking, Degraded, Info }` (serde `snake_case`, `as_str()`)
  - `pub enum Safety { Safe, NeedsApproval }` (serde `snake_case`)
  - `pub enum FixPlan { BindOperator, ProvisionContent { host: String }, UpdateAgent { host: String } }` (serde `tag = "op"`, `snake_case`)
  - `pub struct Fix { summary: String, changes: Vec<String>, safety: Safety, reversible: bool, auto: bool, plan: FixPlan }`
  - `pub struct FindingBody { title: String, explanation: String, evidence: Vec<String>, fix: Option<Fix> }`
  - `pub struct Finding { kind: String, subject: String, host_alias: Option<String>, severity: Severity, body: FindingBody }`
  - `pub struct ScanCx { now: i64, owner: Option<i64>, client_tokens: Vec<ClientTokenRow>, hosts: Vec<HostRow>, health: Vec<HostHealthRow> }`
  - `pub struct Detector { id: &'static str, scan: fn(&ScanCx) -> Vec<Finding> }` and `pub const DETECTORS: &[Detector]`
  - `pub fn scan_all(cx: &ScanCx) -> Vec<Finding>`
  - `pub fn detector_of(kind: &str) -> Option<&'static Detector>` (matches `kind == id` or `kind.starts_with(id) + '.'`)
  - `operator_unbound::scan(cx: &ScanCx) -> Vec<Finding>`

- [ ] **Step 1: Write the failing detector tests**

`service/autofix/operator_unbound.rs`, tests module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::autofix::{FixPlan, Safety, ScanCx};
    use crate::service::operator::OPERATOR_CLIENT_NAME;
    use crate::store::Store;

    fn cx(s: &Store) -> ScanCx {
        ScanCx {
            now: 100,
            owner: s.personal_owner_id().unwrap(),
            client_tokens: s.active_client_tokens().unwrap(),
            hosts: vec![],
            health: vec![],
        }
    }

    /// The 2026-10-17 case: the operator's token was minted with no person,
    /// so it saw no sessions and `whoami` answered E_NOTFOUND.
    #[test]
    fn an_unbound_operator_token_is_a_blocking_finding_with_a_bind_fix() {
        let s = Store::open_in_memory().unwrap();
        s.insert_client_token(OPERATOR_CLIENT_NAME, "deadbeefcafe0123", "full").unwrap();
        let found = scan(&cx(&s));
        assert_eq!(found.len(), 1);
        let f = &found[0];
        assert_eq!((f.kind.as_str(), f.subject.as_str()), ("operator_unbound", "token:ux-agent"));
        let fix = f.body.fix.as_ref().expect("a fix");
        assert_eq!(fix.plan, FixPlan::BindOperator);
        assert_eq!(fix.safety, Safety::NeedsApproval);
        assert!(!fix.auto);
        assert!(f.body.evidence.iter().all(|e| !e.contains("deadbeefcafe0123")), "no full hash in evidence");
    }

    #[test]
    fn a_bound_operator_token_is_healthy() {
        let s = Store::open_in_memory().unwrap();
        s.insert_client_token(OPERATOR_CLIENT_NAME, "aa11", "full").unwrap();
        s.set_client_person(OPERATOR_CLIENT_NAME, s.personal_owner_id().unwrap()).unwrap();
        assert!(scan(&cx(&s)).is_empty());
    }

    #[test]
    fn no_operator_token_is_healthy() {
        let s = Store::open_in_memory().unwrap();
        assert!(scan(&cx(&s)).is_empty());
    }

    #[test]
    fn a_hub_without_an_owner_gets_an_explanation_and_no_fix() {
        let s = Store::open_in_memory().unwrap();
        s.insert_client_token(OPERATOR_CLIENT_NAME, "aa11", "full").unwrap();
        let mut c = cx(&s);
        c.owner = None;
        let found = scan(&c);
        assert_eq!(found.len(), 1);
        assert!(found[0].body.fix.is_none());
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo fleet-test -- service::autofix::operator_unbound`
Expected: compile error, `cannot find function scan`.

- [ ] **Step 3: Write `service/autofix/mod.rs`**

```rust
//! Autofix: detect known broken states, explain them in plain language, and
//! offer (or, for safe ones, apply) a fix. Spec:
//! `docs/superpowers/specs/2026-10-17-autofix-design.md`.
//!
//! Detectors are PURE over a [`ScanCx`] the runner gathers under one brief
//! lock. A fix is a [`FixPlan`] the hub wrote itself; `apply::FixExec` runs it.

pub mod apply;
pub mod host_stale;
pub mod operator_unbound;
pub mod sync;
pub mod tick;

use crate::service::health::HostHealthRow;
use crate::store::{ClientTokenRow, HostRow};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Blocking,
    Degraded,
    Info,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Blocking => "blocking",
            Severity::Degraded => "degraded",
            Severity::Info => "info",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Safety {
    Safe,
    NeedsApproval,
}

/// What a fix does, as data the hub wrote. Never built from a caller's input.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum FixPlan {
    BindOperator,
    ProvisionContent { host: String },
    UpdateAgent { host: String },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Fix {
    pub summary: String,
    pub changes: Vec<String>,
    pub safety: Safety,
    pub reversible: bool,
    /// Whether the runner may apply it by itself when `safety` is Safe.
    pub auto: bool,
    pub plan: FixPlan,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FindingBody {
    pub title: String,
    pub explanation: String,
    pub evidence: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<Fix>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Detector id, or `<id>.<variant>`.
    pub kind: String,
    /// `token:<name>`, `host:<alias>`, `session:<id>` or `hub`.
    pub subject: String,
    pub host_alias: Option<String>,
    pub severity: Severity,
    pub body: FindingBody,
}

/// Everything a detector may read. Gathered by [`apply::gather`].
#[derive(Debug, Clone, Default)]
pub struct ScanCx {
    pub now: i64,
    pub owner: Option<i64>,
    pub client_tokens: Vec<ClientTokenRow>,
    pub hosts: Vec<HostRow>,
    pub health: Vec<HostHealthRow>,
}

pub struct Detector {
    pub id: &'static str,
    pub scan: fn(&ScanCx) -> Vec<Finding>,
}

pub const DETECTORS: &[Detector] = &[
    Detector { id: "operator_unbound", scan: operator_unbound::scan },
    Detector { id: "host", scan: host_stale::scan },
];

pub fn scan_all(cx: &ScanCx) -> Vec<Finding> {
    DETECTORS.iter().flat_map(|d| (d.scan)(cx)).collect()
}

/// The detector that owns `kind` (`id` or `id.variant`).
pub fn detector_of(kind: &str) -> Option<&'static Detector> {
    DETECTORS
        .iter()
        .find(|d| kind == d.id || kind.strip_prefix(d.id).is_some_and(|r| r.starts_with('.')))
}

/// A token as evidence may name it: its name and a 12-char hash prefix.
pub(crate) fn token_label(name: &str, sha: &str) -> String {
    format!("{name} (sha256 {}…)", &sha[..sha.len().min(12)])
}
```

Create `host_stale.rs` with `pub fn scan(_: &super::ScanCx) -> Vec<super::Finding> { Vec::new() }` and empty `apply.rs`, `sync.rs`, `tick.rs` (just a `//!` line) so the module compiles. Tasks 3–6 fill them.

- [ ] **Step 4: Write `operator_unbound.rs`**

```rust
//! Detector 1: the operator's `ux-agent` client token carries no person, so
//! `Caller::view_scope` gives it a refusing scope. It then sees no sessions,
//! not even its own, and `whoami` answers E_NOTFOUND.

use super::{token_label, Finding, FindingBody, Fix, FixPlan, Safety, ScanCx, Severity};
use crate::service::operator::OPERATOR_CLIENT_NAME;

pub fn scan(cx: &ScanCx) -> Vec<Finding> {
    let Some(t) = cx
        .client_tokens
        .iter()
        .find(|t| t.name == OPERATOR_CLIENT_NAME && t.revoked_at.is_none() && t.person_id.is_none())
    else {
        return Vec::new();
    };
    let fix = cx.owner.map(|_| Fix {
        summary: "Link the agent's token to the hub's owner".into(),
        changes: vec![format!(
            "Sets the owner of the client token `{OPERATOR_CLIENT_NAME}` to the hub's owner. Nothing else changes."
        )],
        safety: Safety::NeedsApproval,
        reversible: true,
        auto: false,
        plan: FixPlan::BindOperator,
    });
    let explanation = if fix.is_some() {
        "The fleet only shows a token the sessions of the person it belongs to. The agent's \
         token belongs to nobody, so the agent sees no sessions, not even its own, and cannot \
         find itself (`whoami` fails). Linking the token to the hub's owner lets the agent \
         see what the owner sees."
    } else {
        "The fleet only shows a token the sessions of the person it belongs to. The agent's \
         token belongs to nobody, and this hub has no owner to link it to. Set the hub's owner \
         in Settings › People first; the fix appears here once there is one."
    };
    vec![Finding {
        kind: "operator_unbound".into(),
        subject: format!("token:{OPERATOR_CLIENT_NAME}"),
        host_alias: None,
        severity: Severity::Blocking,
        body: FindingBody {
            title: "Control can't see any sessions".into(),
            explanation: explanation.into(),
            evidence: vec![format!(
                "client token {} has no owner (person_id is empty)",
                token_label(&t.name, &t.token_sha256)
            )],
            fix,
        },
    }]
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo fleet-test -- service::autofix`
Expected: 4 PASS.

- [ ] **Step 6: Commit**

```bash
scripts/verify.sh
/usr/bin/git add crates/fleet-core/src/service/autofix crates/fleet-core/src/service/mod.rs
/usr/bin/git commit -m "feat(autofix): finding types, registry and the operator_unbound detector"
```

---

### Task 3: Detector 4 (`host.*`)

**Files:**
- Modify: `crates/fleet-core/src/service/autofix/host_stale.rs`

**Interfaces:**
- Consumes: `ScanCx.hosts` (`HostRow { alias, reachable, provisioned, provision_stale, hidden, .. }`), `ScanCx.health` (`HostHealthRow { alias, reachable, hooks_silent, agent_behind, claude_behind, agent_version, claude_version, .. }`). Check the exact field names in `store/rows.rs:1174` and `service/health.rs:234` before writing; use them as they are.
- Produces: `host_stale::scan(cx: &ScanCx) -> Vec<Finding>` with kinds `host.provision_stale`, `host.hooks_silent`, `host.agent_behind`, `host.claude_behind`, subject `host:<alias>`, `host_alias: Some(alias)`.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::autofix::{FixPlan, Safety, ScanCx};
    use crate::service::health::HostHealthRow;
    use crate::store::{HostRow, Store};

    fn host(s: &Store, alias: &str) -> HostRow {
        s.insert_host(alias, Some(alias)).unwrap();
        s.set_host_provisioned(alias, true).unwrap();
        s.list_hosts().unwrap().into_iter().find(|h| h.alias == alias).unwrap()
    }

    fn health(alias: &str) -> HostHealthRow {
        HostHealthRow { alias: alias.into(), reachable: true, ..Default::default() }
    }

    fn kinds(cx: &ScanCx) -> Vec<String> {
        let mut k: Vec<String> = scan(cx).into_iter().map(|f| f.kind).collect();
        k.sort();
        k
    }

    #[test]
    fn a_healthy_host_has_no_findings() {
        let s = Store::open_in_memory().unwrap();
        let mut h = host(&s, "h1");
        h.reachable = true;
        let cx = ScanCx { hosts: vec![h], health: vec![health("h1")], ..Default::default() };
        assert!(scan(&cx).is_empty());
    }

    #[test]
    fn each_flag_is_its_own_finding_with_the_right_safety() {
        let s = Store::open_in_memory().unwrap();
        let mut h = host(&s, "h1");
        h.reachable = true;
        h.provision_stale = true;
        let mut hh = health("h1");
        hh.hooks_silent = true;
        hh.agent_behind = true;
        hh.claude_behind = true;
        let cx = ScanCx { hosts: vec![h], health: vec![hh], ..Default::default() };
        assert_eq!(
            kinds(&cx),
            ["host.agent_behind", "host.claude_behind", "host.hooks_silent", "host.provision_stale"]
        );
        let by = |k: &str| scan(&cx).into_iter().find(|f| f.kind == k).unwrap();
        let stale = by("host.provision_stale").body.fix.unwrap();
        assert_eq!((stale.safety, stale.auto), (Safety::Safe, false), "the reprovision loop owns it");
        assert_eq!(stale.plan, FixPlan::ProvisionContent { host: "h1".into() });
        let hooks = by("host.hooks_silent").body.fix.unwrap();
        assert_eq!((hooks.safety, hooks.auto), (Safety::Safe, true));
        let agent = by("host.agent_behind").body.fix.unwrap();
        assert_eq!(agent.safety, Safety::NeedsApproval);
        assert_eq!(agent.plan, FixPlan::UpdateAgent { host: "h1".into() });
        assert!(by("host.claude_behind").body.fix.is_none(), "fleet has no claude update path");
    }

    #[test]
    fn an_unreachable_host_reports_nothing() {
        let s = Store::open_in_memory().unwrap();
        let mut h = host(&s, "h1");
        h.reachable = false;
        h.provision_stale = true;
        let mut hh = health("h1");
        hh.reachable = false;
        hh.hooks_silent = true;
        let cx = ScanCx { hosts: vec![h], health: vec![hh], ..Default::default() };
        assert!(scan(&cx).is_empty(), "host_down is attention's job, not a fixable state");
    }
}
```

If `HostHealthRow` has no `Default`, add `#[derive(Default)]` to it in `service/health.rs` (all its fields are bools, strings, options and numbers).

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo fleet-test -- service::autofix::host_stale`
Expected: FAIL, the stub returns nothing.

- [ ] **Step 3: Implement**

```rust
//! Detector 4: a host whose fleet files are stale, whose hooks went silent,
//! or whose agent or Claude Code is behind.

use super::{Finding, FindingBody, Fix, FixPlan, Safety, ScanCx, Severity};

fn finding(alias: &str, variant: &str, severity: Severity, title: String, explanation: &str, evidence: Vec<String>, fix: Option<Fix>) -> Finding {
    Finding {
        kind: format!("host.{variant}"),
        subject: format!("host:{alias}"),
        host_alias: Some(alias.to_string()),
        severity,
        body: FindingBody { title, explanation: explanation.into(), evidence, fix },
    }
}

fn provision(alias: &str, auto: bool) -> Fix {
    Fix {
        summary: format!("Refresh fleet's files on {alias}"),
        changes: vec![format!(
            "Rewrites fleet's own skill, hooks and helper files on {alias}. Your files and \
             `~/.claude.json` are not touched."
        )],
        safety: Safety::Safe,
        reversible: true,
        auto,
        plan: FixPlan::ProvisionContent { host: alias.to_string() },
    }
}

pub fn scan(cx: &ScanCx) -> Vec<Finding> {
    let mut out = Vec::new();
    for h in cx.health.iter().filter(|h| h.reachable) {
        let alias = h.alias.as_str();
        let row = cx.hosts.iter().find(|r| r.alias == alias);
        if row.is_some_and(|r| r.provision_stale) {
            out.push(finding(
                alias,
                "provision_stale",
                Severity::Degraded,
                format!("Fleet's files on {alias} are out of date"),
                "This hub ships newer helper files than the ones on this host. Until they are \
                 refreshed, new fleet features may not work there. Fleet refreshes stale hosts \
                 on its own every few minutes; this lets you do it now.",
                vec!["the host's provisioning fingerprint differs from this hub's".into()],
                Some(provision(alias, false)),
            ));
        }
        if h.hooks_silent {
            out.push(finding(
                alias,
                "hooks_silent",
                Severity::Degraded,
                format!("{alias} stopped reporting session events"),
                "Claude Code on this host is not calling fleet's hooks, so fleet learns about \
                 finished or waiting sessions late or not at all. Reinstalling fleet's files \
                 restores the hooks.",
                vec!["no hook call from this host recently, while it has live sessions".into()],
                Some(provision(alias, true)),
            ));
        }
        if h.agent_behind {
            out.push(finding(
                alias,
                "agent_behind",
                Severity::Degraded,
                format!("The fleet agent on {alias} is older than the hub"),
                "An older agent may not understand what this hub asks of it. Updating installs \
                 the hub's release on that host and restarts the agent; its sessions keep running.",
                vec![format!("agent version: {}", h.agent_version.as_deref().unwrap_or("unknown"))],
                Some(Fix {
                    summary: format!("Update the agent on {alias}"),
                    changes: vec![format!("Pins the agent on {alias} to the hub's recommended release and wakes its updater, which restarts the agent.")],
                    safety: Safety::NeedsApproval,
                    reversible: false,
                    auto: false,
                    plan: FixPlan::UpdateAgent { host: alias.to_string() },
                }),
            ));
        }
        if h.claude_behind {
            out.push(finding(
                alias,
                "claude_behind",
                Severity::Info,
                format!("Claude Code on {alias} is behind"),
                "A newer Claude Code is in use on other hosts. Fleet cannot update Claude Code \
                 for you: run `claude update` on that host when no session there is busy.",
                vec![format!("claude version: {}", h.claude_version.as_deref().unwrap_or("unknown"))],
                None,
            ));
        }
    }
    out
}
```

Adjust `h.agent_version` / `h.claude_version` to the real field types (they may be `Option<String>` or `String`).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo fleet-test -- service::autofix`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
scripts/verify.sh
/usr/bin/git add crates/fleet-core/src/service/autofix/host_stale.rs crates/fleet-core/src/service/health.rs
/usr/bin/git commit -m "feat(autofix): host detector for stale files, silent hooks and old versions"
```

---

### Task 4: Sync findings into episodes

**Files:**
- Modify: `crates/fleet-core/src/service/autofix/sync.rs`

**Interfaces:**
- Consumes: Task 1 store methods, Task 2 `Finding`.
- Produces:
  - `pub const AUTOFIX_VERIFY_SECS: i64 = 600;`
  - `pub struct SyncOutcome { pub opened: Vec<i64>, pub cleared: Vec<i64>, pub failed: Vec<i64> }`
  - `pub fn sync(s: &Store, found: &[Finding], scope: Scope, now: i64) -> Result<SyncOutcome, IpcError>`
  - `pub enum Scope<'a> { All, Detector(&'a str) }` (a detector re-scan only clears that detector's kinds)

Rules:
1. Every `found` finding is upserted (body = `serde_json::to_string(&f.body)`).
2. A live row in scope that was not found moves to `cleared`.
3. A `dismissed` row whose `dismissed_until <= now` moves back to `open`.
4. A `verifying` row that is still found and `now - last_attempt_at >= AUTOFIX_VERIFY_SECS` moves to `apply_failed` with error "the fix ran, but the problem is still there".

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::autofix::{FindingBody, Severity};
    use crate::store::Store;

    fn f(kind: &str) -> Finding {
        Finding {
            kind: kind.into(),
            subject: "hub".into(),
            host_alias: None,
            severity: Severity::Info,
            body: FindingBody { title: "t".into(), explanation: "e".into(), evidence: vec![], fix: None },
        }
    }

    #[test]
    fn found_opens_and_vanished_clears() {
        let s = Store::open_in_memory().unwrap();
        let o = sync(&s, &[f("operator_unbound")], Scope::All, 10).unwrap();
        assert_eq!(o.opened.len(), 1);
        let o = sync(&s, &[], Scope::All, 20).unwrap();
        assert_eq!(o.cleared.len(), 1);
        assert!(s.live_autofix_findings().unwrap().is_empty());
    }

    #[test]
    fn a_detector_rescan_leaves_other_detectors_alone() {
        let s = Store::open_in_memory().unwrap();
        sync(&s, &[f("operator_unbound"), f("host.hooks_silent")], Scope::All, 10).unwrap();
        let o = sync(&s, &[], Scope::Detector("operator_unbound"), 20).unwrap();
        assert_eq!(o.cleared.len(), 1);
        let live = s.live_autofix_findings().unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].kind, "host.hooks_silent");
    }

    #[test]
    fn an_expired_dismissal_reopens() {
        let s = Store::open_in_memory().unwrap();
        let id = sync(&s, &[f("k")], Scope::All, 10).unwrap().opened[0];
        s.dismiss_autofix_finding(id, 50).unwrap();
        sync(&s, &[f("k")], Scope::All, 40).unwrap();
        assert_eq!(s.autofix_finding(id).unwrap().unwrap().state, "dismissed");
        sync(&s, &[f("k")], Scope::All, 60).unwrap();
        assert_eq!(s.autofix_finding(id).unwrap().unwrap().state, "open");
    }

    #[test]
    fn a_fix_that_did_not_help_fails_after_the_verify_window() {
        let s = Store::open_in_memory().unwrap();
        let id = sync(&s, &[f("k")], Scope::All, 10).unwrap().opened[0];
        s.mark_autofix_attempt(id, 100).unwrap();
        sync(&s, &[f("k")], Scope::All, 100 + AUTOFIX_VERIFY_SECS - 1).unwrap();
        assert_eq!(s.autofix_finding(id).unwrap().unwrap().state, "verifying");
        let o = sync(&s, &[f("k")], Scope::All, 100 + AUTOFIX_VERIFY_SECS).unwrap();
        assert_eq!(o.failed, vec![id]);
        let row = s.autofix_finding(id).unwrap().unwrap();
        assert_eq!(row.state, "apply_failed");
        assert!(row.last_error.unwrap().contains("still there"));
    }

    #[test]
    fn a_verifying_row_that_vanished_is_cleared() {
        let s = Store::open_in_memory().unwrap();
        let id = sync(&s, &[f("k")], Scope::All, 10).unwrap().opened[0];
        s.mark_autofix_attempt(id, 100).unwrap();
        let o = sync(&s, &[], Scope::All, 101).unwrap();
        assert_eq!(o.cleared, vec![id]);
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo fleet-test -- service::autofix::sync`
Expected: compile error, `cannot find function sync`.

- [ ] **Step 3: Implement**

```rust
//! Findings → episodes in `autofix_findings`.

use super::{detector_of, Finding};
use crate::ipc_error::IpcError;
use crate::store::{NewAutofixFinding, Store};

/// How long a fix has to make its finding disappear before it counts as failed.
pub const AUTOFIX_VERIFY_SECS: i64 = 600;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncOutcome {
    pub opened: Vec<i64>,
    pub cleared: Vec<i64>,
    pub failed: Vec<i64>,
}

#[derive(Debug, Clone, Copy)]
pub enum Scope<'a> {
    All,
    Detector(&'a str),
}

impl Scope<'_> {
    fn covers(&self, kind: &str) -> bool {
        match self {
            Scope::All => true,
            Scope::Detector(id) => detector_of(kind).is_some_and(|d| d.id == *id),
        }
    }
}

pub fn sync(s: &Store, found: &[Finding], scope: Scope<'_>, now: i64) -> Result<SyncOutcome, IpcError> {
    let mut out = SyncOutcome::default();
    let mut seen = std::collections::BTreeSet::new();
    for f in found {
        let body = serde_json::to_string(&f.body)
            .map_err(|e| IpcError::new(crate::ipc_error::codes::E_INTERNAL, format!("finding body: {e}")))?;
        let (id, opened) = s.upsert_autofix_finding(
            &NewAutofixFinding {
                kind: &f.kind,
                subject: &f.subject,
                host_alias: f.host_alias.as_deref(),
                severity: f.severity.as_str(),
                body: &body,
            },
            now,
        )?;
        seen.insert(id);
        if opened {
            out.opened.push(id);
        }
    }
    for row in s.live_autofix_findings()? {
        if !scope.covers(&row.kind) {
            continue;
        }
        if !seen.contains(&row.id) {
            s.set_autofix_state(row.id, "cleared", None, now)?;
            out.cleared.push(row.id);
            continue;
        }
        match row.state.as_str() {
            "dismissed" if row.dismissed_until.is_some_and(|u| u <= now) => {
                s.set_autofix_state(row.id, "open", None, now)?;
            }
            "verifying" if row.last_attempt_at.is_some_and(|t| now - t >= AUTOFIX_VERIFY_SECS) => {
                s.set_autofix_state(row.id, "apply_failed", Some("the fix ran, but the problem is still there"), now)?;
                out.failed.push(row.id);
            }
            _ => {}
        }
    }
    Ok(out)
}
```

Use whatever internal-error code `ipc_error::codes` actually has (grep `pub const E_` in `ipc_error.rs`; `E_INTERNAL` or `E_JSON`).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo fleet-test -- service::autofix`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
scripts/verify.sh
/usr/bin/git add crates/fleet-core/src/service/autofix/sync.rs
/usr/bin/git commit -m "feat(autofix): sync findings into episodes with clear, expiry and verify window"
```

---

### Task 5: Settings, eligibility, executor and `apply_one`

**Files:**
- Modify: `crates/fleet-core/src/service/autofix/apply.rs`
- Modify: `crates/fleet-core/src/service/settings.rs` (consts near the playbook consts ~:360; `SPECS` rows after the playbook rows ~:1015)
- Modify: `src/lib/fleet_settings.ts` (`SETTING_KEYS` ~:28 and `SETTING_DEFAULTS` ~:262)
- Modify: `crates/fleet-core/pages/settings.automation.json` (a new "Autofix" section after "Playbooks")

**Interfaces:**
- Consumes: Tasks 1, 2 and 4; `settings::get_bool`, `settings::get_string`; `Store::set_client_person`, `Store::personal_owner_id`; `provision::provision_content_only(store, ssh, host, base)`; `hub::HubBase::read(&s)`; `update::update_now(store, "agent", host, None, &update::trusted_keys(), now)`, `update::AGENT_POKE_SCRIPT`; `SshExec::run`; `health::hosts_health`, `health::host_thresholds`, `health::overlay_agents`; `Store::record_admin_event_or_log(&AdminEvent)`.
- Produces:
  - settings consts `AUTOFIX_AUTO_SAFE = "autofix.auto_safe"` (bool, "true"), `AUTOFIX_MAX_ATTEMPTS = "autofix.max_attempts"` (int 0..10, "3"), `AUTOFIX_DETECTOR_OPERATOR = "autofix.detector.operator_unbound"` (bool, "true"), `AUTOFIX_DETECTOR_HOST = "autofix.detector.host"` (bool, "true")
  - `pub const AUTOFIX_RETRY_SECS: i64 = 600;`
  - `pub struct Config { pub auto_safe: bool, pub max_attempts: i64, pub detectors_off: Vec<&'static str> }` with `Config::read(s: &Store) -> Config`
  - `pub fn auto_eligible(row: &AutofixRow, fix: &Fix, cfg: &Config, now: i64) -> bool`
  - `#[async_trait] pub trait FixExec: Send + Sync { async fn provision_content(&self, host: &str) -> Result<(), IpcError>; async fn update_agent(&self, host: &str) -> Result<(), IpcError>; }`
  - `pub struct SshFixExec<'a> { pub store: &'a Mutex<Store>, pub ssh: &'a dyn SshExec }`
  - `pub fn gather(s: &Store, live_agents: &[(String, String)], now: i64) -> Result<ScanCx, IpcError>`
  - `pub async fn apply_one(store: &Mutex<Store>, exec: &dyn FixExec, id: i64, actor: &str, live_agents: &[(String, String)], now: i64) -> Result<AutofixRow, IpcError>`
  - `pub fn body_of(row: &AutofixRow) -> Result<FindingBody, IpcError>`

- [ ] **Step 1: Add the settings**

`service/settings.rs` consts:

```rust
/// Autofix (service/autofix): apply fixes marked safe without asking.
pub const AUTOFIX_AUTO_SAFE: &str = "autofix.auto_safe";
/// Autofix: attempts per episode before a safe fix waits for a person.
pub const AUTOFIX_MAX_ATTEMPTS: &str = "autofix.max_attempts";
pub const AUTOFIX_DETECTOR_OPERATOR: &str = "autofix.detector.operator_unbound";
pub const AUTOFIX_DETECTOR_HOST: &str = "autofix.detector.host";
```

`SPECS` rows:

```rust
    Spec::new(
        AUTOFIX_AUTO_SAFE,
        "true",
        Kind::Bool,
        "Apply safe fixes automatically",
        "Fix problems whose fix only touches fleet's own files and can be undone, then tell you. Fixes that touch tokens, sign-in or running sessions always wait for you.",
    ),
    Spec::new(
        AUTOFIX_MAX_ATTEMPTS,
        "3",
        Kind::Int { min: 0, max: 10 },
        "Automatic fix attempts",
        "How many times one problem is fixed automatically before it waits for you.",
    )
    .unit(Unit::Count)
    .zero("never"),
    Spec::new(
        AUTOFIX_DETECTOR_OPERATOR,
        "true",
        Kind::Bool,
        "Check Control's access",
        "Notice when Control's token belongs to nobody, which leaves it unable to see any session.",
    ),
    Spec::new(
        AUTOFIX_DETECTOR_HOST,
        "true",
        Kind::Bool,
        "Check hosts",
        "Notice hosts with out-of-date fleet files, silent hooks, or an old agent or Claude Code.",
    ),
```

`src/lib/fleet_settings.ts`: in `SETTING_KEYS` add `autofixAutoSafe: 'autofix.auto_safe', autofixMaxAttempts: 'autofix.max_attempts', autofixDetectorOperator: 'autofix.detector.operator_unbound', autofixDetectorHost: 'autofix.detector.host',`; in `SETTING_DEFAULTS` add the four keys with `true`, `3`, `true`, `true` in the shape the neighbouring entries use.

`pages/settings.automation.json`: add after the Playbooks section, copying its section shape:

```json
{
  "type": "section",
  "title": "Autofix",
  "children": [
    {"type": "field", "key": "autofix.auto_safe"},
    {"type": "field", "key": "autofix.max_attempts", "when": {"key": "autofix.auto_safe", "truthy": true}},
    {"type": "field", "key": "autofix.detector.operator_unbound"},
    {"type": "field", "key": "autofix.detector.host"}
  ]
}
```

Regenerate (each run fails on purpose the first time; run again without the variable):

```bash
REGEN_SETTINGS_DOCS=1 cargo fleet-test -- settings_docs_are_current
REGEN_PAGE_DOCS=1 cargo fleet-test -- page_docs_are_current
cargo fleet-test -- settings_docs_are_current page_docs_are_current every_spec_is_mirrored_in_fleet_settings_ts every_setting_has_one_home
```

Expected: PASS.

- [ ] **Step 2: Write the failing tests for eligibility and apply**

In `apply.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::autofix::sync::{sync, Scope};
    use crate::service::autofix::{FindingBody, Finding, FixPlan, Safety, Severity};
    use crate::service::operator::OPERATOR_CLIENT_NAME;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct FakeExec {
        provisions: AtomicUsize,
        updates: AtomicUsize,
        fail: bool,
    }
    #[async_trait::async_trait]
    impl FixExec for FakeExec {
        async fn provision_content(&self, _host: &str) -> Result<(), IpcError> {
            self.provisions.fetch_add(1, Ordering::SeqCst);
            if self.fail { Err(IpcError::new(codes::E_SSH, "boom")) } else { Ok(()) }
        }
        async fn update_agent(&self, _host: &str) -> Result<(), IpcError> {
            self.updates.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn safe_fix(auto: bool) -> Fix {
        Fix { summary: "s".into(), changes: vec![], safety: Safety::Safe, reversible: true, auto, plan: FixPlan::ProvisionContent { host: "h1".into() } }
    }

    fn open_row(attempts: i64, last: Option<i64>) -> AutofixRow {
        AutofixRow {
            id: 1, kind: "host.hooks_silent".into(), subject: "host:h1".into(), host_alias: Some("h1".into()),
            severity: "degraded".into(), body: "{}".into(), state: "open".into(), first_seen_at: 0, last_seen_at: 0,
            dismissed_until: None, attempts, last_attempt_at: last, last_error: None,
        }
    }

    fn cfg() -> Config {
        Config { auto_safe: true, max_attempts: 3, detectors_off: vec![] }
    }

    #[test]
    fn only_safe_auto_fixes_within_budget_are_eligible() {
        assert!(auto_eligible(&open_row(0, None), &safe_fix(true), &cfg(), 1000));
        assert!(!auto_eligible(&open_row(0, None), &safe_fix(false), &cfg(), 1000), "auto: false");
        let mut approval = safe_fix(true);
        approval.safety = Safety::NeedsApproval;
        assert!(!auto_eligible(&open_row(0, None), &approval, &cfg(), 1000), "never auto");
        assert!(!auto_eligible(&open_row(0, None), &safe_fix(true), &Config { auto_safe: false, ..cfg() }, 1000));
        assert!(!auto_eligible(&open_row(0, None), &safe_fix(true), &Config { detectors_off: vec!["host"], ..cfg() }, 1000));
        assert!(!auto_eligible(&open_row(3, Some(0)), &safe_fix(true), &cfg(), 100_000), "budget spent");
        assert!(!auto_eligible(&open_row(1, Some(900)), &safe_fix(true), &cfg(), 1000), "retry backoff");
        let mut dismissed = open_row(0, None);
        dismissed.state = "dismissed".into();
        assert!(!auto_eligible(&dismissed, &safe_fix(true), &cfg(), 1000));
    }

    #[tokio::test]
    async fn binding_the_operator_clears_its_finding_at_once() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let id = {
            let s = store.lock().unwrap();
            s.insert_client_token(OPERATOR_CLIENT_NAME, "aa11", "full").unwrap();
            let cx = gather(&s, &[], 10).unwrap();
            sync(&s, &crate::service::autofix::scan_all(&cx), Scope::All, 10).unwrap().opened[0]
        };
        let row = apply_one(&store, &FakeExec::default(), id, "test", &[], 20).await.unwrap();
        assert_eq!(row.state, "cleared");
        let s = store.lock().unwrap();
        let t = s.active_client_tokens().unwrap().into_iter().find(|t| t.name == OPERATOR_CLIENT_NAME).unwrap();
        assert_eq!(t.person_id, s.personal_owner_id().unwrap());
    }

    #[tokio::test]
    async fn a_failed_fix_records_the_error() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let id = {
            let s = store.lock().unwrap();
            let f = Finding {
                kind: "host.hooks_silent".into(), subject: "host:h1".into(), host_alias: Some("h1".into()),
                severity: Severity::Degraded,
                body: FindingBody { title: "t".into(), explanation: "e".into(), evidence: vec![], fix: Some(safe_fix(true)) },
            };
            sync(&s, &[f], Scope::All, 10).unwrap().opened[0]
        };
        let exec = FakeExec { fail: true, ..Default::default() };
        let row = apply_one(&store, &exec, id, "test", &[], 20).await.unwrap();
        assert_eq!(row.state, "apply_failed");
        assert!(row.last_error.unwrap().contains("boom"));
        assert_eq!(exec.provisions.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_finding_without_a_fix_is_refused() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let id = {
            let s = store.lock().unwrap();
            let f = Finding {
                kind: "host.claude_behind".into(), subject: "host:h1".into(), host_alias: Some("h1".into()),
                severity: Severity::Info,
                body: FindingBody { title: "t".into(), explanation: "e".into(), evidence: vec![], fix: None },
            };
            sync(&s, &[f], Scope::All, 10).unwrap().opened[0]
        };
        let err = apply_one(&store, &FakeExec::default(), id, "test", &[], 20).await.unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
    }
}
```

- [ ] **Step 3: Run them to verify they fail**

Run: `cargo fleet-test -- service::autofix::apply`
Expected: compile errors on the missing items.

- [ ] **Step 4: Implement `apply.rs`**

```rust
//! Eligibility, execution and the apply → re-scan step.

use super::sync::{sync, Scope};
use super::{scan_all, detector_of, Fix, FindingBody, FixPlan, ScanCx, Safety};
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::settings;
use crate::ssh::SshExec;
use crate::store::{AutofixRow, Store};
use std::sync::Mutex;

/// Minimum gap between two automatic attempts on one episode.
pub const AUTOFIX_RETRY_SECS: i64 = 600;

#[derive(Debug, Clone)]
pub struct Config {
    pub auto_safe: bool,
    pub max_attempts: i64,
    pub detectors_off: Vec<&'static str>,
}

impl Config {
    pub fn read(s: &Store) -> Config {
        let mut off = Vec::new();
        if !settings::get_bool(s, settings::AUTOFIX_DETECTOR_OPERATOR) {
            off.push("operator_unbound");
        }
        if !settings::get_bool(s, settings::AUTOFIX_DETECTOR_HOST) {
            off.push("host");
        }
        Config {
            auto_safe: settings::get_bool(s, settings::AUTOFIX_AUTO_SAFE),
            max_attempts: settings::get_string(s, settings::AUTOFIX_MAX_ATTEMPTS).parse().unwrap_or(3),
            detectors_off: off,
        }
    }

    pub fn detector_on(&self, kind: &str) -> bool {
        detector_of(kind).is_some_and(|d| !self.detectors_off.contains(&d.id))
    }
}

pub fn auto_eligible(row: &AutofixRow, fix: &Fix, cfg: &Config, now: i64) -> bool {
    fix.safety == Safety::Safe
        && fix.auto
        && cfg.auto_safe
        && cfg.detector_on(&row.kind)
        && matches!(row.state.as_str(), "open" | "apply_failed")
        && row.attempts < cfg.max_attempts
        && row.last_attempt_at.is_none_or(|t| now - t >= AUTOFIX_RETRY_SECS)
}

#[async_trait::async_trait]
pub trait FixExec: Send + Sync {
    async fn provision_content(&self, host: &str) -> Result<(), IpcError>;
    async fn update_agent(&self, host: &str) -> Result<(), IpcError>;
}

pub struct SshFixExec<'a> {
    pub store: &'a Mutex<Store>,
    pub ssh: &'a dyn SshExec,
}

#[async_trait::async_trait]
impl FixExec for SshFixExec<'_> {
    async fn provision_content(&self, host: &str) -> Result<(), IpcError> {
        let base = {
            let s = lock(self.store)?;
            crate::service::hub::HubBase::read(&s)?
        };
        crate::service::provision::provision_content_only(self.store, self.ssh, host, &base).await?;
        Ok(())
    }

    async fn update_agent(&self, host: &str) -> Result<(), IpcError> {
        use crate::service::update;
        let now = crate::store::now_unix();
        update::update_now(self.store, "agent", host, None, &update::trusted_keys(), now)?;
        let script = crate::shell::quote(update::AGENT_POKE_SCRIPT);
        self.ssh
            .run(host, &["bash", "-c", &script], std::time::Duration::from_secs(10))
            .await?;
        Ok(())
    }
}

/// Everything detectors read, under the caller's guard.
pub fn gather(s: &Store, live_agents: &[(String, String)], now: i64) -> Result<ScanCx, IpcError> {
    use crate::service::health;
    let hosts = s.list_hosts()?;
    let sessions = s.list_all_sessions()?;
    let mut rows = health::hosts_health(&hosts, &sessions, &health::host_thresholds(s), live_agents, crate::app_version::get(), now);
    health::overlay_agents(&mut rows, live_agents, crate::app_version::get());
    Ok(ScanCx {
        now,
        owner: s.personal_owner_id()?,
        client_tokens: s.active_client_tokens()?,
        hosts,
        health: rows,
    })
}

pub fn body_of(row: &AutofixRow) -> Result<FindingBody, IpcError> {
    serde_json::from_str(&row.body)
        .map_err(|e| IpcError::new(codes::E_INTERNAL, format!("finding {} has an unreadable body: {e}", row.id)))
}

async fn execute(store: &Mutex<Store>, exec: &dyn FixExec, plan: &FixPlan) -> Result<(), IpcError> {
    match plan {
        FixPlan::BindOperator => {
            let s = lock(store)?;
            let owner = s
                .personal_owner_id()?
                .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, "this hub has no owner to link the token to"))?;
            s.set_client_person(crate::service::operator::OPERATOR_CLIENT_NAME, Some(owner))?;
            Ok(())
        }
        FixPlan::ProvisionContent { host } => exec.provision_content(host).await,
        FixPlan::UpdateAgent { host } => exec.update_agent(host).await,
    }
}

/// Run one finding's fix, re-scan its detector, and return the row as it now
/// stands: `cleared` when the state is gone, `verifying` when it still shows
/// (the next passes decide), `apply_failed` when the fix itself errored.
pub async fn apply_one(
    store: &Mutex<Store>,
    exec: &dyn FixExec,
    id: i64,
    actor: &str,
    live_agents: &[(String, String)],
    now: i64,
) -> Result<AutofixRow, IpcError> {
    let (row, fix) = {
        let s = lock(store)?;
        let row = s
            .autofix_finding(id)?
            .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no finding {id}")))?;
        if !matches!(row.state.as_str(), "open" | "apply_failed" | "dismissed" | "verifying") {
            return Err(IpcError::new(codes::E_INVALID, format!("finding {id} is {}, there is nothing to fix", row.state)));
        }
        let fix = body_of(&row)?.fix.ok_or_else(|| {
            IpcError::new(codes::E_INVALID, format!("finding {id} has no automatic fix; its explanation says what to do"))
        })?;
        s.mark_autofix_attempt(id, now)?;
        (row, fix)
    };
    let result = execute(store, exec, &fix.plan).await;
    let s = lock(store)?;
    s.record_admin_event_or_log(&crate::store::AdminEvent {
        actor_person: None,
        actor_device: None,
        action: "autofix.apply",
        resource_type: "autofix_finding",
        resource_id: &id.to_string(),
        representation: serde_json::json!({
            "kind": row.kind, "subject": row.subject, "actor": actor,
            "plan": fix.plan, "ok": result.is_ok(),
            "error": result.as_ref().err().map(|e| e.message.clone()),
        }),
    });
    match result {
        Err(e) => {
            s.set_autofix_state(id, "apply_failed", Some(&e.message), now)?;
        }
        Ok(()) => {
            let detector = detector_of(&row.kind).map(|d| d.id).unwrap_or("");
            let cx = gather(&s, live_agents, now)?;
            let found: Vec<_> = scan_all(&cx).into_iter().filter(|f| detector_of(&f.kind).is_some_and(|d| d.id == detector)).collect();
            sync(&s, &found, Scope::Detector(detector), now)?;
        }
    }
    s.autofix_finding(id)?.ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no finding {id}")))
}
```

Check while implementing: the exact `AdminEvent` path (`crate::store::AdminEvent` or `crate::store::admin_events::AdminEvent`); `IpcError`'s field names (`code`, `message`); whether `update_now`'s `target` is a host alias (its doc says "one target"; the tool passes `p.target`, so yes for a host); whether `SshClient` is `dyn SshExec`-compatible (it is: `provision_content_only` takes `&dyn SshExec`). `Option::is_none_or` needs Rust 1.82+; the pinned toolchain has it if `rust-toolchain.toml` ≥ 1.82, else use `map_or(true, ..)`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo fleet-test -- service::autofix`
Expected: all PASS.

- [ ] **Step 6: Commit**

```bash
scripts/verify.sh
/usr/bin/git add crates/fleet-core/src/service/autofix/apply.rs crates/fleet-core/src/service/settings.rs src/lib/fleet_settings.ts crates/fleet-core/pages/settings.automation.json docs src/lib/pages
/usr/bin/git commit -m "feat(autofix): settings, safe-fix eligibility and the apply step"
```

---

### Task 6: Runner on the reconcile tick

**Files:**
- Modify: `crates/fleet-core/src/service/autofix/apply.rs` (add `run_pass`)
- Modify: `crates/fleet-core/src/service/autofix/tick.rs`
- Modify: `crates/fleet-core/src/service/loops.rs` (`LOOPS`: `acts("autofix", "Autofix"),` right after `acts("playbooks", ...)` ~:79)
- Modify: `crates/fleet-core/src/service/tick.rs` (call after `repair_tick::maybe_run` ~:268)

**Interfaces:**
- Consumes: Task 5.
- Produces:
  - `pub struct PassOutcome { pub sync: SyncOutcome, pub applied: Vec<i64> }`
  - `pub async fn run_pass(store: &Mutex<Store>, exec: &dyn FixExec, live_agents: &[(String, String)], now: i64) -> Result<PassOutcome, IpcError>`
  - `tick::maybe_run(store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>)`

- [ ] **Step 1: Write the failing tests**

In `apply.rs` tests:

```rust
    fn hooks_silent_finding() -> Finding {
        Finding {
            kind: "host.hooks_silent".into(), subject: "host:h1".into(), host_alias: Some("h1".into()),
            severity: Severity::Degraded,
            body: FindingBody { title: "t".into(), explanation: "e".into(), evidence: vec![], fix: Some(safe_fix(true)) },
        }
    }

    #[tokio::test]
    async fn a_pass_applies_safe_fixes_and_leaves_approval_ones() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        {
            let s = store.lock().unwrap();
            s.insert_client_token(OPERATOR_CLIENT_NAME, "aa11", "full").unwrap(); // NeedsApproval
            sync(&s, &[hooks_silent_finding()], Scope::Detector("host"), 1).unwrap(); // Safe + auto
        }
        let exec = FakeExec::default();
        let out = run_pass_with(&store, &exec, &[], 10, |cx| {
            let mut f = scan_all(cx);
            f.push(hooks_silent_finding());
            f
        }).await.unwrap();
        assert_eq!(out.applied.len(), 1, "only the safe one");
        assert_eq!(exec.provisions.load(Ordering::SeqCst), 1);
        let s = store.lock().unwrap();
        let op = s.live_autofix_findings().unwrap().into_iter().find(|r| r.kind == "operator_unbound").unwrap();
        assert_eq!(op.state, "open", "approval fixes never run by themselves");
    }

    #[tokio::test]
    async fn pause_all_stops_autofix() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        {
            let s = store.lock().unwrap();
            crate::service::settings::set(&s, crate::service::settings::AUTOMATION_PAUSED, "true").unwrap();
        }
        let exec = FakeExec::default();
        let out = run_pass_with(&store, &exec, &[], 10, |_| vec![hooks_silent_finding()]).await.unwrap();
        assert!(out.applied.is_empty());
        assert_eq!(exec.provisions.load(Ordering::SeqCst), 0);
    }
```

Use the real name of the pause setting constant (playbooks.rs:824 `pause_all_stops_the_playbooks` shows it).

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo fleet-test -- service::autofix::apply`
Expected: compile error, `cannot find function run_pass_with`.

- [ ] **Step 3: Implement `run_pass`**

In `apply.rs`:

```rust
#[derive(Debug, Clone, Default)]
pub struct PassOutcome {
    pub sync: super::sync::SyncOutcome,
    pub applied: Vec<i64>,
}

pub async fn run_pass(store: &Mutex<Store>, exec: &dyn FixExec, live_agents: &[(String, String)], now: i64) -> Result<PassOutcome, IpcError> {
    run_pass_with(store, exec, live_agents, now, scan_all).await
}

/// [`run_pass`] with the scan injectable, for tests.
pub(crate) async fn run_pass_with(
    store: &Mutex<Store>,
    exec: &dyn FixExec,
    live_agents: &[(String, String)],
    now: i64,
    scan: impl Fn(&ScanCx) -> Vec<super::Finding>,
) -> Result<PassOutcome, IpcError> {
    let (synced, due) = {
        let s = lock(store)?;
        let cfg = Config::read(&s);
        let cx = gather(&s, live_agents, now)?;
        let found: Vec<_> = scan(&cx).into_iter().filter(|f| cfg.detector_on(&f.kind)).collect();
        let synced = sync(&s, &found, Scope::All, now)?;
        let due: Vec<i64> = if crate::service::loops::paused(&s) {
            Vec::new()
        } else {
            s.live_autofix_findings()?
                .into_iter()
                .filter(|r| body_of(r).ok().and_then(|b| b.fix).is_some_and(|f| auto_eligible(r, &f, &cfg, now)))
                .map(|r| r.id)
                .collect()
        };
        (synced, due)
    };
    let mut applied = Vec::new();
    for id in due {
        match apply_one(store, exec, id, "auto", live_agents, now).await {
            Ok(_) => applied.push(id),
            Err(e) => tracing::warn!(id, code = %e.code, "[autofix] automatic fix refused: {}", e.message),
        }
    }
    Ok(PassOutcome { sync: synced, applied })
}
```

Note: with `detectors_off` filtering the scan, a switched-off detector's live findings clear on the next pass. That is the intended reading of "off".

- [ ] **Step 4: Implement `tick.rs`**

Copy the shape of `service/repair_tick.rs::maybe_run` (~:580):

```rust
//! The autofix loop: one pass per minute on the reconcile tick, detached.

use super::apply::{run_pass, SshFixExec};
use crate::ipc_error::lock;
use crate::ssh::SshClient;
use crate::store::Store;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

const PERIOD: Duration = Duration::from_secs(60);
static RUNNING: AtomicBool = AtomicBool::new(false);
static LAST: LazyLock<Mutex<Option<Instant>>> = LazyLock::new(|| Mutex::new(None));

pub fn maybe_run(store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>) {
    {
        let mut last = LAST.lock().unwrap_or_else(|p| p.into_inner());
        if last.is_some_and(|t| t.elapsed() < PERIOD) {
            return;
        }
        let Ok(s) = lock(store) else { return };
        if !crate::service::loops::gate_in("autofix", &s, Some(PERIOD)) {
            return;
        }
        *last = Some(Instant::now());
    }
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    let store = Arc::clone(store);
    let ssh = Arc::clone(ssh);
    tokio::spawn(async move {
        let live: Vec<(String, String)> = ssh
            .agent_registry()
            .map(|r| r.snapshot().into_iter().map(|a| (a.alias, a.agent_version)).collect())
            .unwrap_or_default();
        let exec = SshFixExec { store: &store, ssh: &*ssh };
        let res = run_pass(&store, &exec, &live, crate::store::now_unix()).await;
        if let Ok(o) = &res {
            if !o.applied.is_empty() || !o.sync.opened.is_empty() {
                tracing::info!(opened = o.sync.opened.len(), applied = o.applied.len(), "[autofix] pass");
            }
        }
        crate::service::loops::report("autofix", res.map(|_| ()).map_err(|e| e.message), Some(PERIOD));
        RUNNING.store(false, Ordering::SeqCst);
    });
}
```

Adjust `LazyLock`/the due check to exactly mirror `repair_tick`'s, and the `report` outcome type to whatever `impl Into<Outcome>` accepts there.

- [ ] **Step 5: Register and wire**

`service/loops.rs`, in `LOOPS` after `acts("playbooks", "Stuck playbooks"),`:

```rust
    acts("autofix", "Autofix"),
```

`service/tick.rs`, after `service::repair_tick::maybe_run(store, ssh);`:

```rust
                service::autofix::tick::maybe_run(store, ssh);
```

- [ ] **Step 6: Run the tests**

Run: `cargo fleet-test -- service::autofix service::loops`
Expected: PASS, including `every_loop_reports_and_every_pausable_one_is_gated`.

- [ ] **Step 7: Commit**

```bash
scripts/verify.sh
/usr/bin/git add crates/fleet-core/src/service/autofix crates/fleet-core/src/service/loops.rs crates/fleet-core/src/service/tick.rs
/usr/bin/git commit -m "feat(autofix): run detectors and safe fixes on the reconcile tick"
```

---

### Task 7: MCP tool `autofix`

**Files:**
- Create: `crates/fleet-core/src/mcp/tools/autofix.rs`
- Modify: `crates/fleet-core/src/mcp/tools/params.rs` (add `AutofixParams`)
- Modify: `crates/fleet-core/src/mcp/tools/mod.rs` (`mod autofix;` in the list :40–92; `+ Self::autofix_router()` in `fn tool_router()` :380–409)
- Modify: `crates/fleet-core/src/mcp/guard.rs` (`TOOL_POLICIES` row)
- Modify: `crates/fleet-core/src/mcp/tools/tests.rs` (`include_str!("autofix.rs"),` in `router_sum_serves_every_tool` ~:2831; access tests ~:9860/:10216/:10234/:10280 that list `pr_shepherd`)
- Modify: `docs/control-api.md` (a bullet next to `- **PR shepherd** — ...` ~:1226)
- Regenerate: `docs/control-api-reference.md`

**Interfaces:**
- Consumes: Tasks 1–6.
- Produces: `FleetTools::autofix(&self, Extension<Caller>, Parameters<AutofixParams>) -> Result<CallToolResult, McpError>`; JSON row shape `{ id, kind, subject, host_alias?, severity, state, first_seen_at, last_seen_at, attempts, last_error?, dismissed_until?, title, explanation, evidence, fix? }`.

- [ ] **Step 1: Write the failing tool tests**

In `mcp/tools/tests.rs` (near the other `pr_shepherd` tests; use the file's existing helpers `test_tools`, `client_caller`, `result_json`):

```rust
#[tokio::test]
async fn autofix_lists_the_operator_finding_and_a_trusted_device_fixes_it() {
    let s = Store::open_in_memory().unwrap();
    s.insert_client_token(crate::service::operator::OPERATOR_CLIENT_NAME, "aa11", "full").unwrap();
    let t = test_tools(s);
    let mut dev = client_caller("laptop", TokenMode::Full);
    dev.client.as_mut().unwrap().trusted = true;

    let scan = result_json(&t.autofix(Extension(dev.clone()), Parameters(AutofixParams { action: "scan".into(), ..Default::default() })).await.unwrap());
    let rows = scan["findings"].as_array().unwrap();
    let op = rows.iter().find(|r| r["kind"] == "operator_unbound").expect("the 2026-10-17 case is found");
    assert_eq!(op["fix"]["safety"], "needs_approval");
    let id = op["id"].as_i64().unwrap();

    let applied = result_json(&t.autofix(Extension(dev), Parameters(AutofixParams { action: "apply".into(), id: Some(id), ..Default::default() })).await.unwrap());
    assert_eq!(applied["state"], "cleared");
}

#[tokio::test]
async fn the_operator_may_list_but_not_apply_or_dismiss() {
    let s = Store::open_in_memory().unwrap();
    s.insert_client_token(crate::service::operator::OPERATOR_CLIENT_NAME, "aa11", "full").unwrap();
    let t = test_tools(s);
    let mut op = client_caller(crate::service::operator::OPERATOR_CLIENT_NAME, TokenMode::Full);
    op.client.as_mut().unwrap().trusted = true;
    let scan = result_json(&t.autofix(Extension(op.clone()), Parameters(AutofixParams { action: "scan".into(), ..Default::default() })).await.unwrap());
    let id = scan["findings"][0]["id"].as_i64().unwrap();
    for action in ["apply", "dismiss"] {
        let err = t.autofix(Extension(op.clone()), Parameters(AutofixParams { action: action.into(), id: Some(id), ..Default::default() })).await.unwrap_err();
        assert!(err.message.contains("E_FORBIDDEN") || format!("{err:?}").contains("E_FORBIDDEN"), "{action}: {err:?}");
    }
}

#[tokio::test]
async fn an_untrusted_device_may_not_apply() {
    let s = Store::open_in_memory().unwrap();
    s.insert_client_token(crate::service::operator::OPERATOR_CLIENT_NAME, "aa11", "full").unwrap();
    let t = test_tools(s);
    let dev = client_caller("phone", TokenMode::Full); // trusted: false
    let scan = result_json(&t.autofix(Extension(dev.clone()), Parameters(AutofixParams { action: "scan".into(), ..Default::default() })).await.unwrap());
    let id = scan["findings"][0]["id"].as_i64().unwrap();
    assert!(t.autofix(Extension(dev), Parameters(AutofixParams { action: "apply".into(), id: Some(id), ..Default::default() })).await.is_err());
}
```

Match the E_FORBIDDEN assertion to how neighbouring tests check an `McpError` code (search tests.rs for `E_FORBIDDEN` and copy that helper, e.g. `err_code(&err)`).

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo fleet-test -- mcp::tools::tests::autofix mcp::tools::tests::the_operator_may_list mcp::tools::tests::an_untrusted_device`
Expected: compile error, `no method named autofix`.

- [ ] **Step 3: Add the params**

In `mcp/tools/params.rs`, following `RunsParams`:

```rust
#[derive(Debug, Clone, Default, serde::Deserialize, schemars::JsonSchema)]
pub struct AutofixParams {
    /// list | explain | scan | apply | dismiss.
    pub action: String,
    /// explain, apply, dismiss: the finding's id.
    #[serde(default)]
    pub id: Option<i64>,
    /// dismiss: until when (unix seconds); default 24 hours from now.
    #[serde(default)]
    pub until: Option<i64>,
    /// list: only findings in this state (open | verifying | apply_failed | dismissed | cleared).
    #[serde(default)]
    pub state: Option<String>,
}
```

Copy the exact derive and `schemars` attributes `RunsParams` uses.

- [ ] **Step 4: Write the tool**

`mcp/tools/autofix.rs`:

```rust
use super::*;
use crate::ipc_error::lock;
use crate::service::autofix::apply::{apply_one, body_of, SshFixExec};
use crate::store::AutofixRow;

/// Only a person's trusted full device decides a fix. Control may list and
/// explain, and post a `fix` card for the person to press.
fn autofix_writer(caller: &Caller) -> Result<(), McpError> {
    if caller.is_operator() {
        return Err(mcp_err(
            codes::E_FORBIDDEN,
            "Control can explain a fix but not apply or dismiss it: show the person the finding and let them decide",
            None,
        ));
    }
    if caller.is_trusted_client() && caller.mode == TokenMode::Full {
        return Ok(());
    }
    Err(mcp_err(codes::E_FORBIDDEN, "only a trusted device with full access can apply or dismiss a fix", None))
}

fn row_json(r: &AutofixRow) -> serde_json::Value {
    let body = body_of(r).ok();
    serde_json::json!({
        "id": r.id, "kind": r.kind, "subject": r.subject, "host_alias": r.host_alias,
        "severity": r.severity, "state": r.state,
        "first_seen_at": r.first_seen_at, "last_seen_at": r.last_seen_at,
        "attempts": r.attempts, "last_error": r.last_error, "dismissed_until": r.dismissed_until,
        "title": body.as_ref().map(|b| b.title.clone()),
        "explanation": body.as_ref().map(|b| b.explanation.clone()),
        "evidence": body.as_ref().map(|b| b.evidence.clone()),
        "fix": body.and_then(|b| b.fix),
    })
}

#[tool_router(router = autofix_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "Known broken fleet states, each with a plain-language explanation and, where one exists, a fix: \
        list {state?} (the open ones), explain {id}, scan (check now), apply {id}, dismiss {id, until?} (later; default 24 h). \
        Safe fixes run by themselves (setting autofix.auto_safe); the rest wait for a person. \
        apply and dismiss need a trusted full device; Control may list and explain only.")]
    pub(super) async fn autofix(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<AutofixParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("autofix", &format!("action={} id={:?}", p.action.escape_debug(), p.id));
        let now = crate::store::now_unix();
        let live: Vec<(String, String)> = self
            .ssh
            .agent_registry()
            .map(|r| r.snapshot().into_iter().map(|a| (a.alias, a.agent_version)).collect())
            .unwrap_or_default();
        let need_id = || p.id.ok_or_else(|| mcp_err(codes::E_INVALID, format!("{} needs an id", p.action), None));
        match p.action.as_str() {
            "list" | "scan" => {
                if p.action == "scan" {
                    // A scan runs detectors and syncs; it never auto-applies
                    // (that is the tick's job), so a pass with an executor
                    // that refuses everything is not needed: sync only.
                    let s = lock(&self.store).map_err(to_mcp_err)?;
                    let cx = crate::service::autofix::apply::gather(&s, &live, now).map_err(to_mcp_err)?;
                    let found = crate::service::autofix::scan_all(&cx);
                    crate::service::autofix::sync::sync(&s, &found, crate::service::autofix::sync::Scope::All, now)
                        .map_err(to_mcp_err)?;
                }
                let s = lock(&self.store).map_err(to_mcp_err)?;
                let rows: Vec<_> = s
                    .live_autofix_findings()
                    .map_err(|e| to_mcp_err(e.into()))?
                    .iter()
                    .filter(|r| p.state.as_deref().is_none_or(|st| r.state == st))
                    .map(row_json)
                    .collect();
                ok_json_compact(&serde_json::json!({ "findings": rows }))
            }
            "explain" => {
                let s = lock(&self.store).map_err(to_mcp_err)?;
                let r = s
                    .autofix_finding(need_id()?)
                    .map_err(|e| to_mcp_err(e.into()))?
                    .ok_or_else(|| mcp_err(codes::E_NOTFOUND, "no such finding", None))?;
                ok_json(&row_json(&r))
            }
            "apply" => {
                autofix_writer(&caller)?;
                let exec = SshFixExec { store: &self.store, ssh: &*self.ssh };
                let r = apply_one(&self.store, &exec, need_id()?, &caller.label(), &live, now)
                    .await
                    .map_err(to_mcp_err)?;
                ok_json(&row_json(&r))
            }
            "dismiss" => {
                autofix_writer(&caller)?;
                let id = need_id()?;
                let s = lock(&self.store).map_err(to_mcp_err)?;
                let ok = s
                    .dismiss_autofix_finding(id, p.until.unwrap_or(now + 86_400))
                    .map_err(|e| to_mcp_err(e.into()))?;
                if !ok {
                    return Err(mcp_err(codes::E_NOTFOUND, "no live finding with that id", None));
                }
                let r = s.autofix_finding(id).map_err(|e| to_mcp_err(e.into()))?.expect("just updated");
                ok_json(&row_json(&r))
            }
            other => Err(mcp_err(
                codes::E_INVALID,
                format!("autofix has no action {other:?}: list | explain | scan | apply | dismiss"),
                None,
            )),
        }
    }
}
```

Check how `self.store` is typed in `FleetTools` (`Arc<Mutex<Store>>` → pass `&self.store`), how `self.ssh` is typed and whether `&*self.ssh` coerces to `&dyn SshExec`, and how other tools turn a `rusqlite::Error` into an `McpError` (copy that instead of `to_mcp_err(e.into())` if there is a helper).

- [ ] **Step 5: Wire it**

`mcp/tools/mod.rs`: `mod autofix;` and `+ Self::autofix_router()`.

`mcp/guard.rs`, `TOOL_POLICIES`, next to `pr_shepherd`:

```rust
    ToolPolicy {
        name: "autofix",
        access: Access::PersonDevice,
        readonly: false,
        confirm: false,
        deadline: Deadline::Lifecycle,
    },
```

`mcp/tools/tests.rs`: add `include_str!("autofix.rs"),` to `router_sum_serves_every_tool`; add `"autofix"` wherever the access tests at ~:9860, :10216, :10234, :10280 list `pr_shepherd` (colleague refused / master not served).

`docs/control-api.md`, next to the PR shepherd bullet:

```markdown
- **Autofix** — `autofix`, known broken fleet states with a plain-language explanation and a fix. Safe fixes run by themselves (`autofix.auto_safe`); the rest wait for a person's trusted device. Control may list and explain, never apply.
```

Regenerate the reference:

```bash
REGEN_DOCS=1 cargo fleet-test -- reference_is_current
cargo fleet-test -- reference_is_current
```

- [ ] **Step 6: Run the tests**

Run: `cargo fleet-test -- mcp::tools::tests mcp::guard mcp::doc_gen`
Expected: PASS, including `router_sum_serves_every_tool`, `every_router_tool_has_exactly_one_tool_policy_row`, `annotations_follow_the_policy_table`, `every_tool_parameter_is_documented`, `narrative_guide_names_every_tool`, `the_served_definition_budget_stays_bounded` and the three new tests.

- [ ] **Step 7: Commit**

```bash
scripts/verify.sh
/usr/bin/git add crates/fleet-core/src/mcp docs/control-api.md docs/control-api-reference.md
/usr/bin/git commit -m "feat(autofix): the autofix MCP tool"
```

---

### Task 8: Docs, spec update, full verification, PR

**Files:**
- Modify: `docs/architecture.md` (a bullet next to `- **Run traces** ...` ~:435)
- Modify: `docs/superpowers/specs/2026-10-17-autofix-design.md` (fold in the plan's deviations)
- Modify: `docs/status.md` if it lists landed subsystems by milestone (one line: autofix core landed, surfaces in PR 2)

- [ ] **Step 1: Architecture bullet**

```markdown
- **Autofix** (`service/autofix/`, `store/autofix.rs`, migration 198, MCP `autofix`, loop `autofix`, settings `autofix.*`): known broken states, each found by a pure detector over a `ScanCx` the runner gathers under one lock, stored as an episode in `autofix_findings` (`(kind, subject)` unique while live: open → verifying → cleared / apply_failed, or dismissed). A fix is a `FixPlan` the hub wrote itself, run by `FixExec` (`SshFixExec` in production), followed at once by a re-scan of its detector. Safe fixes with `auto: true` run on the reconcile tick when `autofix.auto_safe` is on; the rest wait for a trusted device's `autofix { apply }`. Every apply is in the admin audit log (`autofix.apply`). Detectors: `operator_unbound`, `host.*`. Spec: `docs/superpowers/specs/2026-10-17-autofix-design.md`.
```

- [ ] **Step 2: Update the spec**

In the spec, replace the `autofix_audit` table with a sentence pointing to `record_admin_event` (`action: "autofix.apply"`); change the state list to `open | verifying | cleared | apply_failed | dismissed` with the `AUTOFIX_VERIFY_SECS` rule; mark `host.provision_stale` `auto: false` (owned by the reprovision loop); mark `host.claude_behind` explanation-only; change the Access section's per-host-token sentence to "not served in v1 (`Access::PersonDevice`)"; replace the detector-1 backfill paragraph with "PR #932 binds at birth and backfills; detector 1 catches later regressions".

- [ ] **Step 3: Full verification**

Use the `mercury-run` skill (or a foreground run with `timeout: 600000`):

```bash
scripts/verify.sh full
```

Expected: exit 0. If a known flake fails (`service::add_project`), re-run it alone before blaming the change.

- [ ] **Step 4: Commit and push**

```bash
/usr/bin/git add docs
/usr/bin/git commit -m "docs(autofix): architecture entry and spec updates from the core plan"
/usr/bin/git push -u origin feat/autofix
```

- [ ] **Step 5: Open the PR**

Follow the `claude-fleet-repo` skill's PR conventions. Title: `feat(autofix): detect broken fleet states, explain them, fix the safe ones`. The body:
- **Why:** the 2026-10-17 incident (the operator saw no sessions; root cause in #932), and that nothing noticed it or explained it.
- **What:** the core pieces and detectors 1 and 4. Surfaces (chat card, panel, feed) and detectors 2 and 3 come in PR 2.
- **Migration:** 195 collides with #932; whichever merges second renumbers.
- **Not verified:** no run against the live hub; no desktop UI in this PR.

Then bind the PR with `ccd_pr` `get_status` / `bind_pr`.
