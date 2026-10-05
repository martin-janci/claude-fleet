# Project picker, phase 1 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the flat, alphabetical "+ New session" project popover with a searchable picker that shows Pinned, For this context, Recent, Popular, groups, and a folded Other for noise — backed by stored pins/hides/groups and a 30-day start count.

**Architecture:** Backend: migration 102 adds two TEXT-keyed tables (`project_picks`, `project_starts`); reconcile records starts idempotently; two new hub tools / Tauri commands (`project_picks`, `set_project_pick`) with `Access::PersonDevice`, `Routed` on a hub client. Frontend: a pure ranking module (`project_rank.ts`) turns projects + picks + context into a `PickerView`; a new `ProjectPicker.svelte` renders it inside the Sidebar's existing popover slot; the quick switcher drops noise from its empty-query list.

**Tech Stack:** Rust (rusqlite, rmcp `#[tool]`, Tauri 2 commands), Svelte 5 runes + svelte/store, Vitest + @testing-library/svelte.

**Spec:** `docs/superpowers/specs/2026-10-05-project-picker-design.md` (phase 1 only; phase 2 / Jev is a later plan).

## Global Constraints

- Everything stored is keyed by `owner` + `repo` TEXT, **never** `project_id` (project rows are deleted and re-created; review C22, `migrations/050_orgs.sql`).
- The picker state is **not** added to `ProjectRow` (offsets in `list_projects_joined`, and `project:updated` swaps the row whole).
- `pick` ∈ `pin | hide | keep | null`; `grp` is trimmed, ≤ 40 chars, blank = null; both null deletes the row.
- Section caps: context 5, recent 5, popular 5 with `starts_30d ≥ 2`; a group over 8 rows starts folded; Other always starts folded.
- Noise: `pick = hide`; or `last_session_at` null; or `starts_30d = 0` and repo matches `^(test|tmp|example)-`, `-analysis$`, `-epic-\d+$`. `pick = keep | pin` overrides all. `system` projects are excluded outright.
- New hub tools: `Access::PersonDevice`; `project_picks` readonly, `set_project_pick` not. Do **not** bump `CONTRACT_REVISION`.
- One canonical shell-quoting etc. is irrelevant here (no shell); no blocking I/O on sync Tauri commands (both new commands are `async`).
- Frontend tests: `npx vitest run <file>` (the `pnpm test` binary is not on PATH here); type-check: `npx svelte-check`. Run `pnpm install --frozen-lockfile` once first.
- Rust: run cargo builds/tests in the **foreground**, one command per call (fmt, clippy, test separately). Before calling a test failure "pre-existing", check it on `origin/main`.
- Never judge a test run through `| tail`; a pass is the summary line, not the exit code alone.
- UI copy is English, sentence case: "For this context", "Recent", "Popular", "Other", "Pin", "Unpin", "Hide", "Keep", "Group…", "Search projects…".

## File map

| File | Status | Responsibility |
|------|--------|----------------|
| `crates/fleet-core/migrations/102_project_picker.sql` | create | the two tables |
| `crates/fleet-core/src/store/schema.rs` | modify | register migration 102 |
| `crates/fleet-core/src/store/project_picks.rs` | create | `ProjectPickRow`, list/set/sweep, `record_project_start_in_tx` |
| `crates/fleet-core/src/store/mod.rs` | modify | `mod project_picks;` + re-export |
| `crates/fleet-core/src/store/reconcile.rs` | modify | call `record_project_start_in_tx` per live session |
| `crates/fleet-core/src/service/gc.rs` | modify | 90-day sweep of `project_starts` |
| `crates/fleet-core/src/service/project_picks.rs` | create | `SetProjectPickArgs`, `list`, `set` |
| `crates/fleet-core/src/service/mod.rs` | modify | `pub mod project_picks;` |
| `crates/fleet-core/src/mcp/tools/repo.rs` | modify | `project_picks`, `set_project_pick` tools |
| `crates/fleet-core/src/mcp/guard.rs` | modify | two `ToolPolicy` rows |
| `crates/fleet-core/src/mcp/tools/tests.rs` | modify | access + round-trip tests |
| `src-tauri/src/commands/projects.rs` | modify | two Tauri commands + `routed::` fns |
| `src-tauri/src/lib.rs` | modify | register the commands |
| `src-tauri/src/backend/verdicts.rs` | modify | two `Routed` rows |
| `src-tauri/src/backend/tests_routing.rs` | modify | one read case, one mutation case |
| generated: `docs/control-api-reference.md`, `src/lib/hub_verdicts.generated.json`, `docs/hub.md`, goldens | regen | via the REGEN env vars |
| `src/lib/project_picks.ts` (+ `.test.ts`) | create | frontend store + IPC wrappers |
| `src/lib/project_rank.ts` (+ `.test.ts`) | create | pure: noise, groups, context, `rankProjects` |
| `src/lib/ProjectPicker.svelte` (+ `.test.ts`) | create | the popover UI |
| `src/lib/Sidebar.svelte` (+ `Sidebar.test.ts`) | modify | mount `ProjectPicker`, context, group dialog, keep-on-add |
| `src/lib/quick_switcher.ts` (+ test), `src/lib/QuickSwitcher.svelte` | modify | drop noise when the query is empty |

---

### Task 1: Storage — migration 102 and the `project_picks` store module

**Files:**
- Create: `crates/fleet-core/migrations/102_project_picker.sql`
- Create: `crates/fleet-core/src/store/project_picks.rs`
- Modify: `crates/fleet-core/src/store/schema.rs` (end of `MIGRATIONS`, after the `version: 101` entry, ~line 1156)
- Modify: `crates/fleet-core/src/store/mod.rs` (module list ~line 27, re-exports ~line 99)

**Interfaces:**
- Produces (Rust, `crate::store`):
  - `pub struct ProjectPickRow { pub owner: String, pub repo: String, pub pick: Option<String>, pub grp: Option<String>, pub starts_30d: i64 }` (Serialize, Deserialize, Debug, Clone, PartialEq, Eq; `#[serde(default)]` on the last three)
  - `pub const PROJECT_PICKS: [&str; 3]`, `pub const PROJECT_GROUP_MAX_CHARS: usize = 40`, `pub const PROJECT_STARTS_WINDOW_SECS: i64`, `pub const PROJECT_STARTS_RETENTION_SECS: i64`
  - `Store::list_project_picks(&self, now: i64) -> Result<Vec<ProjectPickRow>, IpcError>`
  - `Store::set_project_pick(&self, owner: &str, repo: &str, pick: Option<&str>, grp: Option<&str>, now: i64) -> Result<ProjectPickRow, IpcError>`
  - `Store::sweep_project_starts_older_than(&self, cutoff: i64) -> Result<usize, IpcError>`
  - `Store::record_project_start_in_tx(tx: &rusqlite::Connection, project_id: i64, host_alias: &str, tmux_name: &str, created_at: i64) -> Result<(), rusqlite::Error>` (associated fn, `pub(super)`)

- [ ] **Step 1: Write the migration**

`crates/fleet-core/migrations/102_project_picker.sql`:

```sql
-- The New session project picker, phase 1
-- (docs/superpowers/specs/2026-10-05-project-picker-design.md).
--
-- project_picks   a person's choice for one project: pin | hide | keep, and
--                 the picker group they put it in. Both null = no row.
-- project_starts  one row per tmux session ever seen in a project, for the
--                 picker's "Popular" (starts in the last 30 days). Reconcile
--                 inserts with OR IGNORE on every pass; the UNIQUE key makes
--                 the repeat a no-op. `at` is the session's created_at.
--
-- Both are keyed by owner/repo TEXT, never project_id: project rows are
-- deleted and re-created and their ids re-derived (review C22, see 050).
CREATE TABLE IF NOT EXISTS project_picks (
  owner      TEXT    NOT NULL,
  repo       TEXT    NOT NULL,
  pick       TEXT    CHECK (pick IS NULL OR pick IN ('pin', 'hide', 'keep')),
  grp        TEXT,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (owner, repo)
);

CREATE TABLE IF NOT EXISTS project_starts (
  owner      TEXT    NOT NULL,
  repo       TEXT    NOT NULL,
  host_alias TEXT    NOT NULL,
  tmux_name  TEXT    NOT NULL,
  at         INTEGER NOT NULL,
  UNIQUE (host_alias, tmux_name, at)
);
CREATE INDEX IF NOT EXISTS idx_project_starts_project ON project_starts(owner, repo, at);

INSERT OR IGNORE INTO schema_version (version) VALUES (102);
```

If `102` is taken on `origin/main` by the time you implement, use the next free number everywhere in this task.

- [ ] **Step 2: Register it** — in `schema.rs`, append to `MIGRATIONS` after the `version: 101` entry:

```rust
    // The New session project picker (phase 1): `project_picks` (pin / hide /
    // keep and the picker group) and `project_starts` (Popular), both keyed
    // by owner/repo TEXT. New tables only, so plain.
    Migration::plain(102, include_str!("../../migrations/102_project_picker.sql")),
```

- [ ] **Step 3: Write the failing tests** — create `crates/fleet-core/src/store/project_picks.rs` with only the test module first:

```rust
//! The New session picker's per-project state (project picker spec, phase
//! 1): a person's pick and group, and the sessions started in a project.
//! Keyed by `owner`/`repo` TEXT, never `project_id` — project rows are
//! deleted and re-created, their ids re-derived (review C22, migration 050).

use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn store_with(projects: &[(&str, &str)]) -> Store {
        let s = Store::open_in_memory().unwrap();
        for (o, r) in projects {
            s.upsert_project(o, r, &format!("/p/{o}/{r}")).unwrap();
        }
        s
    }

    fn start(s: &Store, owner: &str, repo: &str, tmux: &str, at: i64) {
        let pid: i64 = s
            .conn
            .query_row(
                "SELECT id FROM projects WHERE owner=?1 AND repo=?2",
                rusqlite::params![owner, repo],
                |r| r.get(0),
            )
            .unwrap();
        Store::record_project_start_in_tx(&s.conn, pid, "mac", tmux, at).unwrap();
    }

    #[test]
    fn every_non_system_project_is_listed_with_empty_state() {
        let s = store_with(&[("o", "a"), ("o", "b")]);
        s.upsert_system_project("fleet", "operator", "/op").unwrap();
        let rows = s.list_project_picks(NOW).unwrap();
        let names: Vec<_> = rows.iter().map(|r| format!("{}/{}", r.owner, r.repo)).collect();
        assert_eq!(names, ["o/a", "o/b"], "system project excluded, ordered");
        assert!(rows.iter().all(|r| r.pick.is_none() && r.grp.is_none() && r.starts_30d == 0));
    }

    #[test]
    fn set_round_trips_and_both_null_deletes_the_row() {
        let s = store_with(&[("o", "a")]);
        let row = s.set_project_pick("o", "a", Some("pin"), Some("  tools "), NOW).unwrap();
        assert_eq!(row.pick.as_deref(), Some("pin"));
        assert_eq!(row.grp.as_deref(), Some("tools"), "trimmed");
        let row = s.set_project_pick("o", "a", None, None, NOW).unwrap();
        assert_eq!((row.pick, row.grp), (None, None));
        let n: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM project_picks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "both null deletes");
    }

    #[test]
    fn set_validates_pick_group_and_project() {
        let s = store_with(&[("o", "a")]);
        let e = s.set_project_pick("o", "a", Some("star"), None, NOW).unwrap_err();
        assert_eq!(e.code, crate::ipc_error::codes::E_INVALID);
        let long = "x".repeat(PROJECT_GROUP_MAX_CHARS + 1);
        let e = s.set_project_pick("o", "a", None, Some(&long), NOW).unwrap_err();
        assert_eq!(e.code, crate::ipc_error::codes::E_INVALID);
        let e = s.set_project_pick("o", "nope", Some("pin"), None, NOW).unwrap_err();
        assert_eq!(e.code, crate::ipc_error::codes::E_NOTFOUND);
        // A blank group is a clear, not an error.
        let row = s.set_project_pick("o", "a", Some("keep"), Some("   "), NOW).unwrap();
        assert_eq!(row.grp, None);
    }

    #[test]
    fn starts_count_once_per_session_inside_the_window() {
        let s = store_with(&[("o", "a")]);
        start(&s, "o", "a", "one", NOW - 10);
        start(&s, "o", "a", "one", NOW - 10); // the next reconcile pass
        start(&s, "o", "a", "two", NOW - 5 * 86_400);
        start(&s, "o", "a", "old", NOW - PROJECT_STARTS_WINDOW_SECS - 1);
        let row = &s.list_project_picks(NOW).unwrap()[0];
        assert_eq!(row.starts_30d, 2);
    }

    #[test]
    fn picks_and_starts_survive_the_project_row_being_recreated() {
        let s = store_with(&[("o", "a")]);
        s.set_project_pick("o", "a", Some("pin"), None, NOW).unwrap();
        start(&s, "o", "a", "one", NOW - 10);
        s.conn.execute("DELETE FROM projects", []).unwrap();
        s.upsert_project("o", "a", "/p/o/a").unwrap();
        let row = &s.list_project_picks(NOW).unwrap()[0];
        assert_eq!(row.pick.as_deref(), Some("pin"));
        assert_eq!(row.starts_30d, 1);
    }

    #[test]
    fn the_sweep_deletes_only_older_starts() {
        let s = store_with(&[("o", "a")]);
        start(&s, "o", "a", "old", NOW - PROJECT_STARTS_RETENTION_SECS - 1);
        start(&s, "o", "a", "new", NOW - 10);
        let n = s
            .sweep_project_starts_older_than(NOW - PROJECT_STARTS_RETENTION_SECS)
            .unwrap();
        assert_eq!(n, 1);
        assert_eq!(s.list_project_picks(NOW).unwrap()[0].starts_30d, 1);
    }
}
```

Register the module in `store/mod.rs` next to `mod projects;`:

```rust
mod project_picks;
```

and next to `pub use reports::{ReportFilter, ReportRow};`:

```rust
pub use project_picks::{
    ProjectPickRow, PROJECT_GROUP_MAX_CHARS, PROJECT_PICKS, PROJECT_STARTS_RETENTION_SECS,
    PROJECT_STARTS_WINDOW_SECS,
};
```

(If `delete_project` or a test helper deletes `projects` rows through a path with foreign keys on, the `DELETE FROM projects` in the test still works: the new tables have no FK to `projects`, by design.)

- [ ] **Step 4: Run the tests to verify they fail**

Run: `cargo test -p fleet-core --lib store::project_picks`
Expected: compile errors — `ProjectPickRow`, `list_project_picks`, etc. not found.

- [ ] **Step 5: Implement** — above the test module in `project_picks.rs`:

```rust
use crate::ipc_error::{codes, IpcError};

/// One project's picker state: the person's pick and group, and how many
/// sessions started in it inside [`PROJECT_STARTS_WINDOW_SECS`].
///
/// `serde(default)` on the optional fields: this row crosses the hub wire.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProjectPickRow {
    pub owner: String,
    pub repo: String,
    #[serde(default)]
    pub pick: Option<String>,
    #[serde(default)]
    pub grp: Option<String>,
    #[serde(default)]
    pub starts_30d: i64,
}

/// The values `project_picks.pick` takes (the migration's CHECK).
pub const PROJECT_PICKS: [&str; 3] = ["pin", "hide", "keep"];
/// A picker group's name, at most.
pub const PROJECT_GROUP_MAX_CHARS: usize = 40;
/// "Popular" counts starts this recent.
pub const PROJECT_STARTS_WINDOW_SECS: i64 = 30 * 86_400;
/// The GC sweep keeps starts this long.
pub const PROJECT_STARTS_RETENTION_SECS: i64 = 90 * 86_400;

/// Every non-system project with its state; `?1` is the window's start.
const PICKS_SELECT: &str = "SELECT p.owner, p.repo, k.pick, k.grp,
        (SELECT COUNT(*) FROM project_starts s
          WHERE s.owner = p.owner AND s.repo = p.repo AND s.at >= ?1)
   FROM projects p
   LEFT JOIN project_picks k ON k.owner = p.owner AND k.repo = p.repo
  WHERE p.system = 0";

fn map_pick_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ProjectPickRow> {
    Ok(ProjectPickRow {
        owner: r.get(0)?,
        repo: r.get(1)?,
        pick: r.get(2)?,
        grp: r.get(3)?,
        starts_30d: r.get(4)?,
    })
}

impl Store {
    /// The picker state of every non-system project, by owner then repo.
    pub fn list_project_picks(&self, now: i64) -> Result<Vec<ProjectPickRow>, IpcError> {
        let mut stmt = self
            .conn
            .prepare_cached(&format!("{PICKS_SELECT} ORDER BY p.owner, p.repo"))?;
        let rows = stmt.query_map(
            rusqlite::params![now - PROJECT_STARTS_WINDOW_SECS],
            map_pick_row,
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Replace one project's pick and group (full replace; both `None`
    /// deletes the row) and return its state. `E_INVALID` for a pick outside
    /// [`PROJECT_PICKS`] or a group over [`PROJECT_GROUP_MAX_CHARS`];
    /// `E_NOTFOUND` for a project fleet does not know. A blank group clears.
    pub fn set_project_pick(
        &self,
        owner: &str,
        repo: &str,
        pick: Option<&str>,
        grp: Option<&str>,
        now: i64,
    ) -> Result<ProjectPickRow, IpcError> {
        if let Some(p) = pick {
            if !PROJECT_PICKS.contains(&p) {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!("pick must be pin, hide or keep, not {p:?}"),
                ));
            }
        }
        let grp = grp.map(str::trim).filter(|g| !g.is_empty());
        if let Some(g) = grp {
            if g.chars().count() > PROJECT_GROUP_MAX_CHARS {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!("a group name is at most {PROJECT_GROUP_MAX_CHARS} characters"),
                ));
            }
        }
        let known: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE owner = ?1 AND repo = ?2 AND system = 0)",
            rusqlite::params![owner, repo],
            |r| r.get(0),
        )?;
        if !known {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("no project {owner}/{repo}"),
            ));
        }
        if pick.is_none() && grp.is_none() {
            self.conn.execute(
                "DELETE FROM project_picks WHERE owner = ?1 AND repo = ?2",
                rusqlite::params![owner, repo],
            )?;
        } else {
            self.conn.execute(
                "INSERT INTO project_picks (owner, repo, pick, grp, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(owner, repo) DO UPDATE SET
                   pick = excluded.pick, grp = excluded.grp, updated_at = excluded.updated_at",
                rusqlite::params![owner, repo, pick, grp, now],
            )?;
        }
        let mut stmt = self.conn.prepare_cached(&format!(
            "{PICKS_SELECT} AND p.owner = ?2 AND p.repo = ?3"
        ))?;
        Ok(stmt.query_row(
            rusqlite::params![now - PROJECT_STARTS_WINDOW_SECS, owner, repo],
            map_pick_row,
        )?)
    }

    /// Delete starts before `cutoff`; returns how many.
    pub fn sweep_project_starts_older_than(&self, cutoff: i64) -> Result<usize, IpcError> {
        Ok(self.conn.execute(
            "DELETE FROM project_starts WHERE at < ?1",
            rusqlite::params![cutoff],
        )?)
    }

    /// Record that tmux session `tmux_name` on `host_alias`, created at
    /// `created_at`, lives in project `project_id`. Idempotent (the UNIQUE
    /// key), so reconcile calls it on every pass; a no-op for an unknown
    /// project or an unknown creation time (`created_at <= 0`).
    pub(super) fn record_project_start_in_tx(
        tx: &rusqlite::Connection,
        project_id: i64,
        host_alias: &str,
        tmux_name: &str,
        created_at: i64,
    ) -> Result<(), rusqlite::Error> {
        if created_at <= 0 {
            return Ok(());
        }
        tx.prepare_cached(
            "INSERT OR IGNORE INTO project_starts (owner, repo, host_alias, tmux_name, at)
             SELECT owner, repo, ?2, ?3, ?4 FROM projects WHERE id = ?1",
        )?
        .execute(rusqlite::params![project_id, host_alias, tmux_name, created_at])?;
        Ok(())
    }
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p fleet-core --lib store::project_picks`
Expected: `6 passed`.

- [ ] **Step 7: Run the whole store/schema suite** (a new migration moves `LATEST_SCHEMA_VERSION`; some goldens may pin it)

Run: `cargo test -p fleet-core --lib store::`
Expected: all pass. If a test fails naming a golden/schema version, regenerate with the env var that test's message names and re-run; never hand-edit a golden.

- [ ] **Step 8: fmt + clippy**

Run: `cargo fmt --all` then `cargo clippy -p fleet-core --all-targets -- -D warnings`
Expected: no diff after fmt is committed; clippy clean.

- [ ] **Step 9: Commit**

```bash
git add crates/fleet-core/migrations/102_project_picker.sql crates/fleet-core/src/store/project_picks.rs crates/fleet-core/src/store/schema.rs crates/fleet-core/src/store/mod.rs
git commit -m "feat(picker): project_picks and project_starts, keyed by owner/repo"
```

---

### Task 2: Reconcile records starts; GC sweeps them

**Files:**
- Modify: `crates/fleet-core/src/store/reconcile.rs` (the `for sess in spec.sessions` loop, ~line 909–950; tests module ~line 1321+)
- Modify: `crates/fleet-core/src/service/gc.rs` (the ungated sweeps block, after the peer outbox sweep ~line 418)

**Interfaces:**
- Consumes: `Store::record_project_start_in_tx`, `Store::sweep_project_starts_older_than`, `PROJECT_STARTS_RETENTION_SECS` (Task 1).
- Produces: nothing new; `list_project_picks(...).starts_30d` becomes live.

- [ ] **Step 1: Write the failing test** — in `reconcile.rs`'s test module, next to `a_reconcile_pass_may_reuse_a_claude_session_id_and_the_first_owner_stands` (it uses the same helpers `store_with_recorder`, `live_session`, `empty_probe`, `HostReconcile`):

```rust
    /// The picker's Popular: every live session with a project is a start,
    /// recorded once however many passes see it (project picker spec).
    #[test]
    fn a_reconcile_pass_records_each_session_start_once() {
        let (mut store, _bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        let pid = store.upsert_project("o", "r", "/base/r").unwrap();
        let keep = vec!["a".to_string(), "b".to_string()];
        let now = crate::store::now_unix();
        for pass in 1..=2 {
            store
                .apply_host_reconcile(HostReconcile {
                    sessions: &[
                        ReconcileSession { created_at: now - 60, ..live_session("a", pid, 10) },
                        ReconcileSession { created_at: now - 30, ..live_session("b", pid, 10) },
                    ],
                    keep: &keep,
                    ..empty_probe("alpha", pass)
                })
                .unwrap();
        }
        let row = &store.list_project_picks(now).unwrap()[0];
        assert_eq!(row.starts_30d, 2, "two sessions, two passes, two starts");
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p fleet-core --lib a_reconcile_pass_records_each_session_start_once`
Expected: FAIL — `assertion left == right` with `left: 0`.

- [ ] **Step 3: Implement** — in the loop, right after the existing

```rust
                    if let Some(pid) = sess.project_id {
                        let latest = project_touch.entry(pid).or_insert(0);
                        *latest = (*latest).max(sess.last_activity_at);
                    }
```

replace that block with:

```rust
                    if let Some(pid) = sess.project_id {
                        let latest = project_touch.entry(pid).or_insert(0);
                        *latest = (*latest).max(sess.last_activity_at);
                        // The picker's Popular: idempotent, so every pass may
                        // repeat it (project picker spec, `project_starts`).
                        Self::record_project_start_in_tx(
                            tx,
                            pid,
                            spec.alias,
                            sess.tmux_name,
                            sess.created_at,
                        )?;
                    }
```

- [ ] **Step 4: Run it to verify it passes**

Run: `cargo test -p fleet-core --lib a_reconcile_pass_records_each_session_start_once`
Expected: `1 passed`.

- [ ] **Step 5: Add the GC sweep** — in `gc.rs`, directly after the `sweep_peer_outbox` block:

```rust
    // The project picker's start counts: kept 90 days (Popular reads 30).
    // Ungated, like the sweeps above: bookkeeping, not the idle killer.
    if let Ok(s) = store.lock() {
        let cutoff = now - crate::store::PROJECT_STARTS_RETENTION_SECS;
        if let Err(e) = s.sweep_project_starts_older_than(cutoff) {
            tracing::warn!(error = %e, "[gc] project starts sweep failed");
        }
    }
```

Check `now` there is `i64` unix seconds (it is passed to `sweep_peer_outbox(now, …)`; if it is a different type in your checkout, convert the same way that call does).

- [ ] **Step 6: Run the reconcile and gc suites**

Run: `cargo test -p fleet-core --lib store::reconcile`
Then: `cargo test -p fleet-core --lib service::gc`
Expected: all pass.

- [ ] **Step 7: fmt + clippy, then commit**

Run: `cargo fmt --all`, then `cargo clippy -p fleet-core --all-targets -- -D warnings`

```bash
git add crates/fleet-core/src/store/reconcile.rs crates/fleet-core/src/service/gc.rs
git commit -m "feat(picker): reconcile records each session start; GC keeps 90 days"
```

---

### Task 3: Service and hub tools — `project_picks`, `set_project_pick`

**Files:**
- Create: `crates/fleet-core/src/service/project_picks.rs`
- Modify: `crates/fleet-core/src/service/mod.rs` (next to `pub mod projects;`)
- Modify: `crates/fleet-core/src/mcp/tools/repo.rs` (inside `#[tool_router(router = repo_router, …)] impl FleetTools`, after `forget_project`)
- Modify: `crates/fleet-core/src/mcp/guard.rs` (`TOOL_POLICIES`, after the `forget_project` row ~line 823)
- Modify: `crates/fleet-core/src/mcp/tools/tests.rs` (after `settings_reach_a_persons_device_and_never_a_host_or_an_org_bound_client`)
- Regenerate: `docs/control-api-reference.md`

**Interfaces:**
- Consumes: Task 1's store API.
- Produces:
  - `crate::service::project_picks::SetProjectPickArgs { owner: String, repo: String, pick: Option<String>, grp: Option<String> }` (Debug, Clone, Serialize, Deserialize, JsonSchema; `#[serde(default)]` on `pick`, `grp`)
  - `crate::service::project_picks::list(store: &Mutex<Store>) -> Result<Vec<ProjectPickRow>, IpcError>`
  - `crate::service::project_picks::set(store: &Mutex<Store>, args: &SetProjectPickArgs) -> Result<ProjectPickRow, IpcError>`
  - Hub tools `project_picks` (no params) and `set_project_pick` (params = `SetProjectPickArgs`), both answering compact JSON.

- [ ] **Step 1: Write the failing tests** — in `mcp/tools/tests.rs`:

```rust
// ---- the New session project picker (phase 1) ----

/// The picker's state is a person's preference: their paired device reads
/// (any mode) and writes (full); never a host's token; not served to the
/// master (its description budget).
#[test]
fn project_picks_reach_a_persons_device_only() {
    let master = Caller::master();
    let laptop = client_caller("laptop", TokenMode::Full);
    let phone_ro = client_caller("phone", TokenMode::Readonly);
    let host = host_caller("hosta", TokenMode::Full);
    let can = |c: &Caller, t: &str| {
        enforce_mode(c, t).and_then(|()| enforce_admin(c, t)).is_ok() && present::visible_to(c, t)
    };
    assert!(can(&laptop, "project_picks") && can(&phone_ro, "project_picks"));
    assert!(can(&laptop, "set_project_pick"));
    assert!(!can(&phone_ro, "set_project_pick"), "a write");
    for t in ["project_picks", "set_project_pick"] {
        assert!(!can(&host, t), "{t}: never a host's token");
        assert!(!can(&master, t), "{t}: not served to the master");
    }
    assert!(guard::is_readonly_tool("project_picks"));
    assert!(!guard::is_readonly_tool("set_project_pick"));
}

#[tokio::test]
async fn set_project_pick_round_trips_through_the_tools() {
    let (tools, _guards, store) = client_tools();
    store.lock().unwrap().upsert_project("o", "r", "/p/o/r").unwrap();
    let set = tools
        .set_project_pick(Parameters(crate::service::project_picks::SetProjectPickArgs {
            owner: "o".into(),
            repo: "r".into(),
            pick: Some("pin".into()),
            grp: Some("tools".into()),
        }))
        .await
        .unwrap();
    let v = result_json(&set);
    assert_eq!(v["pick"], "pin");
    assert_eq!(v["grp"], "tools");
    let list = result_json(&tools.project_picks().await.unwrap());
    assert_eq!(list[0]["owner"], "o");
    assert_eq!(list[0]["pick"], "pin");
    assert_eq!(list[0]["starts_30d"], 0);
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p fleet-core --lib project_pick`
Expected: compile error — no method `set_project_pick` / module `project_picks`.

- [ ] **Step 3: The service module** — `crates/fleet-core/src/service/project_picks.rs`:

```rust
//! The New session picker's per-project state (project picker spec, phase
//! 1): the transport-agnostic layer the Tauri commands and the hub tools
//! share. The rules live in `Store::set_project_pick`.

use crate::ipc_error::{lock, IpcError};
use crate::store::{now_unix, ProjectPickRow, Store};
use std::sync::Mutex;

/// One project's new picker state — a full replace: send the current value
/// of the field you are not changing. Both null removes the project's row.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct SetProjectPickArgs {
    /// The project's owner, as list_projects names it.
    pub owner: String,
    /// The project's repo, as list_projects names it.
    pub repo: String,
    /// pin | hide | keep; null clears.
    #[serde(default)]
    pub pick: Option<String>,
    /// The picker group (at most 40 characters); null or blank clears.
    #[serde(default)]
    pub grp: Option<String>,
}

pub fn list(store: &Mutex<Store>) -> Result<Vec<ProjectPickRow>, IpcError> {
    lock(store)?.list_project_picks(now_unix())
}

pub fn set(store: &Mutex<Store>, args: &SetProjectPickArgs) -> Result<ProjectPickRow, IpcError> {
    lock(store)?.set_project_pick(
        &args.owner,
        &args.repo,
        args.pick.as_deref(),
        args.grp.as_deref(),
        now_unix(),
    )
}
```

Add `pub mod project_picks;` to `service/mod.rs` next to `pub mod projects;`. If `lock`'s error type is not `IpcError` in your checkout, mirror how `service::projects` takes the store lock.

- [ ] **Step 4: The tools** — in `repo.rs`, extend the import line `use crate::service::{add_project, repo, repo_read};` to `use crate::service::{add_project, project_picks, repo, repo_read};`, then after `forget_project`:

```rust
    #[tool(description = "The New session picker's state per project: \
        pick (pin|hide|keep), group, sessions started in 30 days.")]
    pub(super) async fn project_picks(&self) -> Result<CallToolResult, McpError> {
        audit("project_picks", "");
        ok_json_compact(&project_picks::list(&self.store).map_err(to_mcp_err)?)
    }

    #[tool(description = "Replace one project's picker state: pick \
        pin|hide|keep or null, group or null. Both null clears.")]
    pub(super) async fn set_project_pick(
        &self,
        Parameters(args): Parameters<project_picks::SetProjectPickArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "set_project_pick",
            &format!(
                "project={:?}/{:?} pick={:?} grp={:?}",
                args.owner, args.repo, args.pick, args.grp
            ),
        );
        ok_json_compact(&project_picks::set(&self.store, &args).map_err(to_mcp_err)?)
    }
```

- [ ] **Step 5: The policy rows** — in `guard.rs` `TOOL_POLICIES`, after the `forget_project` row:

```rust
    // The New session project picker (phase 1): a person's preference, so a
    // person's own device only, under the desktop commands' own names. Not
    // served to the master — nothing an agent needs, and every byte of the
    // master's surface is budgeted.
    ToolPolicy {
        name: "project_picks",
        access: Access::PersonDevice,
        readonly: true,
        confirm: false,
        deadline: Deadline::Quick,
    },
    ToolPolicy {
        name: "set_project_pick",
        access: Access::PersonDevice,
        readonly: false,
        confirm: false,
        deadline: Deadline::Quick,
    },
```

- [ ] **Step 6: Run the new tests**

Run: `cargo test -p fleet-core --lib project_pick`
Expected: `2 passed` (plus Task 1's `store::project_picks` tests matching the filter).

- [ ] **Step 7: Regenerate the reference, then run the whole fleet-core suite**

Run: `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`
Then: `cargo test -p fleet-core`
Expected: all pass. Tests that enumerate tools or policies (served-surface budget, policy-row-per-tool, readonly lists, isolation matrix) will name what they need if they fail — add exactly that (e.g. a row in a list), never loosen an assertion. `tests_isolation.rs` covers `work`/`work_link` actions only; these tools need no row there.

- [ ] **Step 8: fmt + clippy, then commit**

Run: `cargo fmt --all`, then `cargo clippy -p fleet-core --all-targets -- -D warnings`

```bash
git add crates/fleet-core/src/service/project_picks.rs crates/fleet-core/src/service/mod.rs crates/fleet-core/src/mcp/tools/repo.rs crates/fleet-core/src/mcp/guard.rs crates/fleet-core/src/mcp/tools/tests.rs docs/control-api-reference.md
git commit -m "feat(picker): project_picks and set_project_pick, a person's device only"
```

---

### Task 4: Desktop commands, hub routing, verdicts

**Files:**
- Modify: `src-tauri/src/commands/projects.rs`
- Modify: `src-tauri/src/lib.rs` (`generate_handler!`, next to `commands::projects::list_projects,` ~line 363)
- Modify: `src-tauri/src/backend/verdicts.rs` (the `// ── projects ──` block, after `list_github_repos` ~line 165)
- Modify: `src-tauri/src/backend/tests_routing.rs` (`routed_read_cases()` near the `list_projects` case ~line 416; `routed_mutation_cases()` near `add_project` ~line 2463)
- Regenerate: `src/lib/hub_verdicts.generated.json`, `docs/hub.md`, `docs/control-api-reference.md`, and any golden a test names

**Interfaces:**
- Consumes: Task 3's `service::project_picks::{list, set, SetProjectPickArgs}`; hub tools `project_picks`, `set_project_pick`.
- Produces: Tauri commands `project_picks` (no args) → `Vec<ProjectPickRow>`; `set_project_pick { args: SetProjectPickArgs }` → `ProjectPickRow`. Frontend calls them as `invokeCmd('project_picks')` and `invokeCmd('set_project_pick', { args })`.

- [ ] **Step 1: Write the failing routing cases** — in `routed_read_cases()`, after the `list_projects` case:

```rust
        (
            "project_picks",
            "project_picks",
            json!({}),
            r#"[{"owner":"o","repo":"r","pick":"pin","grp":"tools","starts_30d":3}]"#,
            Box::new(|b, s, _| {
                let v = block_on(commands::projects::routed::project_picks(b, s))?;
                assert_eq!(v[0].pick.as_deref(), Some("pin"), "the hub's answer");
                assert_eq!(v[0].starts_30d, 3);
                Ok(())
            }),
        ),
```

In `routed_mutation_cases()`, after the `add_project` case (all four fields non-default, so the whole struct is proven to cross the wire):

```rust
        (
            "set_project_pick",
            "set_project_pick",
            json!({ "owner": "o", "repo": "r", "pick": "hide", "grp": "tools" }),
            r#"{"owner":"o","repo":"r","pick":"hide","grp":"tools","starts_30d":0}"#,
            Box::new(|b, s, _| {
                block_on(commands::projects::routed::set_project_pick(
                    b,
                    s,
                    fleet_core::service::project_picks::SetProjectPickArgs {
                        owner: "o".into(),
                        repo: "r".into(),
                        pick: Some("hide".into()),
                        grp: Some("tools".into()),
                    },
                ))
                .map(|_| ())
            }),
        ),
```

Match the exact tuple shape of the neighbouring cases in your checkout (closure arity, `json!` import); if they differ from the above, follow them.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p claude-fleet --lib backend::tests_routing`
Expected: compile error — `routed::project_picks` not found.

- [ ] **Step 3: The commands** — in `commands/projects.rs`, extend the imports:

```rust
use fleet_core::service::project_picks::{self, SetProjectPickArgs};
use fleet_core::store::ProjectPickRow;
```

Add the commands after `list_github_repos`:

```rust
/// The New session picker's state per project (pin / hide / keep, group,
/// starts in 30 days). The picker reloads it each time it opens.
#[tauri::command]
pub async fn project_picks(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<ProjectPickRow>, IpcError> {
    routed::project_picks(&backend, &store).await
}

/// Replace one project's picker state (full replace). Returns the row.
#[tauri::command]
pub async fn set_project_pick(
    args: SetProjectPickArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<ProjectPickRow, IpcError> {
    routed::set_project_pick(&backend, &store, args).await
}
```

and in `mod routed`:

```rust
    pub async fn project_picks(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<Vec<ProjectPickRow>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("project_picks", &serde_json::json!({})).await,
            None => project_picks::list(store),
        }
    }

    pub async fn set_project_pick(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: SetProjectPickArgs,
    ) -> Result<ProjectPickRow, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("set_project_pick", &args).await,
            None => project_picks::set(store, &args),
        }
    }
```

Update the module doc's "all four commands route…" sentence to "all six". Register both in `lib.rs` after `commands::projects::list_projects,`:

```rust
            commands::projects::project_picks,
            commands::projects::set_project_pick,
```

- [ ] **Step 4: The verdict rows** — in `verdicts.rs`, after `list_github_repos`:

```rust
    (
        "project_picks",
        Verdict::Routed {
            tool: "project_picks",
        },
    ),
    (
        "set_project_pick",
        Verdict::Routed {
            tool: "set_project_pick",
        },
    ),
```

- [ ] **Step 5: Regenerate and run the crate suite**

Run: `REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen`
Then: `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current` (the reference lists every frontend command too)
Then: `cargo test -p claude-fleet --lib`
Expected: all pass. If `tests_contract` fails on `hub_contract.golden.json`, run it once with `REGEN_HUB_CONTRACT=1` (that run still reports FAILED), re-run without the var, and confirm the golden diff only adds the two tools. Do **not** touch `CONTRACT_REVISION`.

- [ ] **Step 6: fmt + clippy (workspace), then commit**

Run: `cargo fmt --all`, then `cargo clippy --workspace --all-targets -- -D warnings`

```bash
git add src-tauri/src/commands/projects.rs src-tauri/src/lib.rs src-tauri/src/backend/verdicts.rs src-tauri/src/backend/tests_routing.rs src/lib/hub_verdicts.generated.json docs/hub.md docs/control-api-reference.md src-tauri/src/backend/*.golden.json
git commit -m "feat(picker): project_picks / set_project_pick commands, routed on a hub"
```

---

### Task 5: Frontend store — `project_picks.ts`

**Files:**
- Create: `src/lib/project_picks.ts`
- Test: `src/lib/project_picks.test.ts`

**Interfaces:**
- Consumes: Tauri commands from Task 4.
- Produces:
  - `type Pick = 'pin' | 'hide' | 'keep'`
  - `interface ProjectPick { owner: string; repo: string; pick: Pick | null; grp: string | null; starts_30d: number }`
  - `pickKey(owner: string, repo: string): string` → `"owner/repo"`
  - `projectPicks: Writable<ReadonlyMap<string, ProjectPick>>`
  - `loadProjectPicks(): Promise<Result<ProjectPick[]>>`
  - `setProjectPick(owner: string, repo: string, patch: { pick?: Pick | null; grp?: string | null }): Promise<Result<ProjectPick>>`

- [ ] **Step 1: Write the failing test** — `src/lib/project_picks.test.ts`:

```ts
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { loadProjectPicks, pickKey, projectPicks, setProjectPick } from './project_picks';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;

beforeEach(() => {
  inv.mockReset();
  projectPicks.set(new Map());
});

describe('project_picks', () => {
  it('loads into a map keyed owner/repo', async () => {
    inv.mockResolvedValue([{ owner: 'o', repo: 'r', pick: 'pin', grp: null, starts_30d: 2 }]);
    await loadProjectPicks();
    expect(inv).toHaveBeenCalledWith('project_picks', undefined);
    expect(get(projectPicks).get(pickKey('o', 'r'))?.pick).toBe('pin');
  });

  it('an older hub (an error, or a null answer) leaves the map as it was', async () => {
    inv.mockResolvedValue(null);
    await loadProjectPicks();
    expect(get(projectPicks).size).toBe(0);
    inv.mockRejectedValue({ code: 'E_HUB', message: 'unknown tool' });
    const r = await loadProjectPicks();
    expect(r.ok).toBe(false);
    expect(get(projectPicks).size).toBe(0);
  });

  it('set sends a full replace, keeping the field not being changed', async () => {
    projectPicks.set(new Map([[pickKey('o', 'r'), { owner: 'o', repo: 'r', pick: 'pin', grp: 'tools', starts_30d: 1 }]]));
    inv.mockResolvedValue({ owner: 'o', repo: 'r', pick: 'pin', grp: 'infra', starts_30d: 1 });
    await setProjectPick('o', 'r', { grp: 'infra' });
    expect(inv).toHaveBeenCalledWith('set_project_pick', {
      args: { owner: 'o', repo: 'r', pick: 'pin', grp: 'infra' },
    });
    expect(get(projectPicks).get('o/r')?.grp).toBe('infra');
  });
});
```

- [ ] **Step 2: Run to verify failure**

Run: `npx vitest run src/lib/project_picks.test.ts`
Expected: FAIL — cannot resolve `./project_picks`.

- [ ] **Step 3: Implement** — `src/lib/project_picks.ts`:

```ts
// The New session picker's per-project state (project picker spec, phase 1):
// a person's pick (pin | hide | keep), their group, and the sessions started
// in the last 30 days. Keyed by `owner/repo`, never the project id — the
// backend re-creates project rows (review C22). Not patched by events: the
// picker reloads it each time it opens.
import { get, writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';

export type Pick = 'pin' | 'hide' | 'keep';

export interface ProjectPick {
  owner: string;
  repo: string;
  pick: Pick | null;
  grp: string | null;
  starts_30d: number;
}

export const pickKey = (owner: string, repo: string): string => `${owner}/${repo}`;

export const projectPicks = writable<ReadonlyMap<string, ProjectPick>>(new Map());

export async function loadProjectPicks(): Promise<Result<ProjectPick[]>> {
  const r = await invokeCmd<ProjectPick[]>('project_picks');
  // A hub older than this feature has no such tool: the picker then works
  // from the rules alone, with nothing pinned.
  if (r.ok && Array.isArray(r.value)) {
    projectPicks.set(new Map(r.value.map((p) => [pickKey(p.owner, p.repo), p])));
  }
  return r;
}

/** Change one project's pick and/or group. The command is a full replace,
 *  so the field not in `patch` is sent as it is now. */
export async function setProjectPick(
  owner: string,
  repo: string,
  patch: { pick?: Pick | null; grp?: string | null },
): Promise<Result<ProjectPick>> {
  const cur = get(projectPicks).get(pickKey(owner, repo));
  const args = {
    owner,
    repo,
    pick: patch.pick !== undefined ? patch.pick : (cur?.pick ?? null),
    grp: patch.grp !== undefined ? patch.grp : (cur?.grp ?? null),
  };
  const r = await invokeCmd<ProjectPick>('set_project_pick', { args });
  if (r.ok && r.value) {
    const row = r.value;
    projectPicks.update((m) => new Map(m).set(pickKey(owner, repo), row));
  }
  return r;
}
```

If `invokeCmd` passes `undefined` args differently than the test expects (`toHaveBeenCalledWith('project_picks', undefined)`), adjust the test's expectation to what `invokeCmd` actually forwards — not the implementation.

- [ ] **Step 4: Run to verify pass**

Run: `npx vitest run src/lib/project_picks.test.ts`
Expected: `3 passed`.

- [ ] **Step 5: Commit**

```bash
git add src/lib/project_picks.ts src/lib/project_picks.test.ts
git commit -m "feat(picker): the project picks store"
```

---

### Task 6: The ranking module — `project_rank.ts`

**Files:**
- Create: `src/lib/project_rank.ts`
- Test: `src/lib/project_rank.test.ts`

**Interfaces:**
- Consumes: `ProjectTreeRow` (`./projects`), `ProjectPick`, `pickKey` (Task 5), `fuzzyMatchFields` (`./fuzzy`).
- Produces:
  - constants `CONTEXT_CAP = 5`, `RECENT_CAP = 5`, `POPULAR_CAP = 5`, `POPULAR_MIN = 2`, `GROUP_FOLD_OVER = 8`
  - `isNoise(p: ProjectTreeRow, pick: ProjectPick | undefined): boolean`
  - `groupProjects(rows: readonly ProjectTreeRow[], picks: ReadonlyMap<string, ProjectPick>): Map<number, string>`
  - `interface ContextSignal { projectId: number; reason: string }`
  - `interface ContextInput { preferredHost: string | null; selectedProjectId: number | null; sessions: readonly { project_id: number | null; host_alias: string }[]; filterLabels: readonly string[]; filteredProjectIds: Iterable<number> }`
  - `contextSignals(input: ContextInput): ContextSignal[]`
  - `interface PickerRow { project: ProjectTreeRow; id: number; key: string; label: string; pick: Pick | null; manualGroup: string | null; group: string; starts: number; noise: boolean; reason: string | null }`
  - `type SectionId = 'pinned' | 'context' | 'recent' | 'popular' | 'group' | 'other'`
  - `interface PickerSection { id: SectionId; key: string; label: string; rows: PickerRow[]; foldedByDefault: boolean }`
  - `type PickerView = { mode: 'sections'; sections: PickerSection[] } | { mode: 'search'; rows: PickerRow[] }`
  - `rankProjects(input: { projects: readonly ProjectTreeRow[]; picks: ReadonlyMap<string, ProjectPick>; context: readonly ContextSignal[]; query: string; now: number }): PickerView`

- [ ] **Step 1: Write the failing tests** — `src/lib/project_rank.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import type { ProjectTreeRow } from './projects';
import { pickKey, type ProjectPick } from './project_picks';
import { contextSignals, groupProjects, isNoise, rankProjects, type PickerView } from './project_rank';

const NOW = 1_800_000_000;
const DAY = 86_400;
let nextId = 1;
const proj = (owner: string, repo: string, last: number | null = NOW - DAY, system = false): ProjectTreeRow => ({
  project: { id: nextId++, owner, repo, base_path: `/p/${owner}/${repo}`, last_session_at: last, adopted: false, system },
  worktrees: [],
});
const picks = (...ps: Array<Partial<ProjectPick> & { owner: string; repo: string }>) =>
  new Map(ps.map((p) => [pickKey(p.owner, p.repo), { pick: null, grp: null, starts_30d: 0, ...p } as ProjectPick]));
const sections = (v: PickerView) => (v.mode === 'sections' ? v.sections : []);
const labels = (v: PickerView, key: string) => sections(v).find((s) => s.key === key)?.rows.map((r) => r.label) ?? [];

describe('isNoise', () => {
  const p = (repo: string, last: number | null = NOW) => proj('o', repo, last);
  it('a project that never had a session is noise', () => {
    expect(isNoise(p('a', null), undefined)).toBe(true);
  });
  it('keep and pin override every rule; hide wins over use', () => {
    expect(isNoise(p('a', null), { owner: 'o', repo: 'a', pick: 'keep', grp: null, starts_30d: 0 })).toBe(false);
    expect(isNoise(p('a', null), { owner: 'o', repo: 'a', pick: 'pin', grp: null, starts_30d: 0 })).toBe(false);
    expect(isNoise(p('a'), { owner: 'o', repo: 'a', pick: 'hide', grp: null, starts_30d: 9 })).toBe(true);
  });
  it('a noise-looking name is noise only while unused for 30 days', () => {
    for (const repo of ['test-x', 'tmp-x', 'example-x', 'pos-frontend-analysis', 'ppt-epic-145']) {
      expect(isNoise(p(repo), undefined), repo).toBe(true);
      expect(isNoise(p(repo), { owner: 'o', repo, pick: null, grp: null, starts_30d: 1 }), repo).toBe(false);
    }
    expect(isNoise(p('contest-app'), undefined)).toBe(false);
  });
});

describe('groupProjects', () => {
  it('clusters leading and trailing tokens within one owner; a lone repo is its owner', () => {
    const rows = [
      proj('papayapos', 'openmarket-app'),
      proj('papayapos', 'openmarket-docs'),
      proj('papayapos', 'openmarket'),
      proj('papayapos', 'dwh'),
      proj('me', 'gmail-mcp'),
      proj('me', 'nas-mcp'),
    ];
    const g = groupProjects(rows, new Map());
    expect(rows.map((r) => g.get(r.project.id))).toEqual(['openmarket', 'openmarket', 'openmarket', 'papayapos', 'mcp', 'mcp']);
  });
  it('a repo in two clusters is ambiguous and falls to its owner', () => {
    const rows = [proj('me', 'ppt-bridge-mcp'), proj('me', 'ppt-x'), proj('me', 'gmail-mcp')];
    expect(groupProjects(rows, new Map()).get(rows[0].project.id)).toBe('me');
  });
  it("a person's group wins; noise does not seed a cluster", () => {
    const rows = [proj('me', 'test-a', null), proj('me', 'test-b', null), proj('me', 'test-c'), proj('me', 'thing')];
    const g = groupProjects(rows, picks({ owner: 'me', repo: 'thing', grp: 'Mine' }));
    expect(g.get(rows[2].project.id)).toBe('me');
    expect(g.get(rows[3].project.id)).toBe('Mine');
  });
});

describe('contextSignals', () => {
  it('host, then the selected session, then the filter; first reason wins', () => {
    const s = contextSignals({
      preferredHost: 'mefistos',
      selectedProjectId: 2,
      sessions: [
        { project_id: 1, host_alias: 'mefistos' },
        { project_id: 2, host_alias: 'mac' },
        { project_id: null, host_alias: 'mefistos' },
      ],
      filterLabels: ['Host: mac'],
      filteredProjectIds: [2, 3],
    });
    expect(s).toEqual([
      { projectId: 1, reason: 'on mefistos' },
      { projectId: 2, reason: 'current session' },
      { projectId: 3, reason: 'from filter: Host: mac' },
    ]);
  });
  it('no active filter, no filter signal', () => {
    const s = contextSignals({ preferredHost: null, selectedProjectId: null, sessions: [], filterLabels: [], filteredProjectIds: [1, 2] });
    expect(s).toEqual([]);
  });
});

describe('rankProjects — sections', () => {
  it('orders Pinned, context, Recent, Popular, groups, Other; each project once', () => {
    const pinned = proj('o', 'pinned-one', NOW - 9 * DAY);
    const ctx = proj('o', 'ctx', NOW - 8 * DAY);
    const fresh = proj('o', 'fresh', NOW - 60);
    const pop = proj('o', 'pop', NOW - 20 * DAY);
    const plain = proj('o', 'plain', NOW - 30 * DAY);
    const never = proj('o', 'never', null);
    const sys = proj('fleet', 'operator', NOW, true);
    const v = rankProjects({
      projects: [pinned, ctx, fresh, pop, plain, never, sys],
      picks: picks({ owner: 'o', repo: 'pinned-one', pick: 'pin' }, { owner: 'o', repo: 'pop', starts_30d: 4 }),
      context: [{ projectId: ctx.project.id, reason: 'on mac' }],
      query: '',
      now: NOW,
    });
    // Popular and the groups are empty here (Recent took every remaining
    // non-noise project), and an empty section is not rendered.
    expect(sections(v).map((s) => s.id)).toEqual(['pinned', 'context', 'recent', 'other']);
    expect(labels(v, 'pinned')).toEqual(['pinned-one']);
    expect(labels(v, 'context')).toEqual(['ctx']);
    expect(sections(v)[1].rows[0].reason).toBe('on mac');
    expect(labels(v, 'recent')).toEqual(['fresh', 'pop', 'plain']);
    expect(labels(v, 'popular')).toEqual([]); // pop was taken by Recent
    expect(labels(v, 'other')).toEqual(['never']);
    expect(sections(v).find((s) => s.id === 'other')?.foldedByDefault).toBe(true);
    const all = sections(v).flatMap((s) => s.rows.map((r) => r.id));
    expect(new Set(all).size).toBe(all.length);
    expect(all).not.toContain(sys.project.id);
  });

  it('caps Recent at 5, Popular needs 2 starts, groups fold over 8', () => {
    const many = Array.from({ length: 16 }, (_, i) => proj('o', `r${String(i).padStart(2, '0')}`, NOW - (i + 1) * DAY));
    const v = rankProjects({
      projects: many,
      picks: picks({ owner: 'o', repo: 'r10', starts_30d: 5 }, { owner: 'o', repo: 'r11', starts_30d: 1 }),
      context: [],
      query: '',
      now: NOW,
    });
    expect(labels(v, 'recent')).toEqual(['r00', 'r01', 'r02', 'r03', 'r04']);
    expect(labels(v, 'popular')).toEqual(['r10']);
    const group = sections(v).find((s) => s.id === 'group');
    expect(group?.label).toBe('o');
    expect(group?.rows.length).toBe(10);
    expect(group?.foldedByDefault).toBe(true);
  });

  it('labels a repo name that two owners share with its owner', () => {
    const v = rankProjects({ projects: [proj('a', 'app'), proj('b', 'app')], picks: new Map(), context: [], query: '', now: NOW });
    expect(labels(v, 'recent')).toEqual(['a/app', 'b/app']);
  });
});

describe('rankProjects — search', () => {
  it('finds noise too, but below an equal match that is not noise', () => {
    const used = proj('o', 'shop-api', NOW - DAY);
    const hidden = proj('o', 'shop-app', NOW - DAY);
    const v = rankProjects({
      projects: [hidden, used],
      picks: picks({ owner: 'o', repo: 'shop-app', pick: 'hide' }),
      context: [],
      query: 'shop',
      now: NOW,
    });
    expect(v.mode).toBe('search');
    expect(v.mode === 'search' && v.rows.map((r) => r.label)).toEqual(['shop-api', 'shop-app']);
  });

  it('matches the group name too, and drops what does not match', () => {
    const rows = [proj('o', 'openmarket-app'), proj('o', 'openmarket-docs'), proj('o', 'zzz')];
    const v = rankProjects({ projects: rows, picks: new Map(), context: [], query: 'openm', now: NOW });
    expect(v.mode === 'search' && v.rows.map((r) => r.label).sort()).toEqual(['openmarket-app', 'openmarket-docs']);
  });
});
```

- [ ] **Step 2: Run to verify failure**

Run: `npx vitest run src/lib/project_rank.test.ts`
Expected: FAIL — cannot resolve `./project_rank`.

- [ ] **Step 3: Implement** — `src/lib/project_rank.ts`:

```ts
// The New session project picker's ranking (project picker spec, phase 1):
// pure functions from projects + picks + context to what the popover shows.
// Pinned → For this context → Recent → Popular → groups → Other (noise), a
// project once, in the highest section that claims it. With a query: one
// fuzzy-ranked list, noise included but penalised.
import { fuzzyMatchFields } from './fuzzy';
import type { ProjectTreeRow } from './projects';
import { pickKey, type Pick, type ProjectPick } from './project_picks';

export const CONTEXT_CAP = 5;
export const RECENT_CAP = 5;
export const POPULAR_CAP = 5;
export const POPULAR_MIN = 2;
export const GROUP_FOLD_OVER = 8;

const NOISE_NAMES = [/^(test|tmp|example)-/i, /-analysis$/i, /-epic-\d+$/i];

/** Noise: hidden; never had a session; or a throwaway-looking name unused
 *  for 30 days. `keep` and `pin` override every rule. */
export function isNoise(p: ProjectTreeRow, pick: ProjectPick | undefined): boolean {
  const k = pick?.pick ?? null;
  if (k === 'keep' || k === 'pin') return false;
  if (k === 'hide') return true;
  if (p.project.last_session_at == null) return true;
  return (pick?.starts_30d ?? 0) === 0 && NOISE_NAMES.some((re) => re.test(p.project.repo));
}

const tokens = (repo: string) => repo.toLowerCase().split(/[-_]/).filter(Boolean);

/** Each project's group: the person's group, else its one prefix/suffix
 *  cluster among the owner's non-noise repos, else its owner. */
export function groupProjects(
  rows: readonly ProjectTreeRow[],
  picks: ReadonlyMap<string, ProjectPick>,
): Map<number, string> {
  const pickOf = (r: ProjectTreeRow) => picks.get(pickKey(r.project.owner, r.project.repo));
  const byOwner = new Map<string, ProjectTreeRow[]>();
  for (const r of rows) byOwner.set(r.project.owner, [...(byOwner.get(r.project.owner) ?? []), r]);
  const out = new Map<number, string>();
  for (const [owner, list] of byOwner) {
    const lead = new Map<string, number>();
    const trail = new Map<string, number>();
    for (const r of list) {
      if (isNoise(r, pickOf(r))) continue;
      const t = tokens(r.project.repo);
      if (t.length < 2) continue;
      lead.set(t[0], (lead.get(t[0]) ?? 0) + 1);
      trail.set(t[t.length - 1], (trail.get(t[t.length - 1]) ?? 0) + 1);
    }
    for (const r of list) {
      const manual = pickOf(r)?.grp;
      if (manual) {
        out.set(r.project.id, manual);
        continue;
      }
      const t = tokens(r.project.repo);
      const l = (lead.get(t[0]) ?? 0) >= 2 ? t[0] : null;
      const tr = t.length >= 2 && (trail.get(t[t.length - 1]) ?? 0) >= 2 ? t[t.length - 1] : null;
      out.set(r.project.id, l && tr ? owner : (l ?? tr ?? owner));
    }
  }
  return out;
}

export interface ContextSignal {
  projectId: number;
  reason: string;
}

export interface ContextInput {
  /** *New session on host*: the host the dialog will preselect. */
  preferredHost: string | null;
  selectedProjectId: number | null;
  sessions: readonly { project_id: number | null; host_alias: string }[];
  /** The active sidebar filters' chip labels; empty = no filter. */
  filterLabels: readonly string[];
  /** Projects of the sessions the filtered sidebar shows. */
  filteredProjectIds: Iterable<number>;
}

/** Strongest first: the preferred host, the selected session, the filter.
 *  A project keeps the first reason it is given. */
export function contextSignals(input: ContextInput): ContextSignal[] {
  const out: ContextSignal[] = [];
  const seen = new Set<number>();
  const add = (id: number | null, reason: string) => {
    if (id == null || seen.has(id)) return;
    seen.add(id);
    out.push({ projectId: id, reason });
  };
  if (input.preferredHost) {
    for (const s of input.sessions) if (s.host_alias === input.preferredHost) add(s.project_id, `on ${input.preferredHost}`);
  }
  add(input.selectedProjectId, 'current session');
  if (input.filterLabels.length > 0) {
    const why = `from filter: ${input.filterLabels.join(', ')}`;
    for (const id of input.filteredProjectIds) add(id, why);
  }
  return out;
}

export interface PickerRow {
  project: ProjectTreeRow;
  id: number;
  key: string;
  /** `repo`, or `owner/repo` when two owners share the repo name. */
  label: string;
  pick: Pick | null;
  manualGroup: string | null;
  group: string;
  starts: number;
  noise: boolean;
  reason: string | null;
}

export type SectionId = 'pinned' | 'context' | 'recent' | 'popular' | 'group' | 'other';

export interface PickerSection {
  id: SectionId;
  /** Unique per section: the id, or `group:<name>`. */
  key: string;
  label: string;
  rows: PickerRow[];
  foldedByDefault: boolean;
}

export type PickerView = { mode: 'sections'; sections: PickerSection[] } | { mode: 'search'; rows: PickerRow[] };

const byLabel = (a: PickerRow, b: PickerRow) => a.label.localeCompare(b.label, undefined, { sensitivity: 'base' });

export function rankProjects(input: {
  projects: readonly ProjectTreeRow[];
  picks: ReadonlyMap<string, ProjectPick>;
  context: readonly ContextSignal[];
  query: string;
  now: number;
}): PickerView {
  const visible = input.projects.filter((p) => !p.project.system);
  const groups = groupProjects(visible, input.picks);
  const repoCount = new Map<string, number>();
  for (const p of visible) repoCount.set(p.project.repo, (repoCount.get(p.project.repo) ?? 0) + 1);
  const reasons = new Map(input.context.map((c) => [c.projectId, c.reason]));
  const rows: PickerRow[] = visible.map((p) => {
    const key = pickKey(p.project.owner, p.project.repo);
    const pk = input.picks.get(key);
    return {
      project: p,
      id: p.project.id,
      key,
      label: (repoCount.get(p.project.repo) ?? 0) > 1 ? key : p.project.repo,
      pick: pk?.pick ?? null,
      manualGroup: pk?.grp ?? null,
      group: groups.get(p.project.id) ?? p.project.owner,
      starts: pk?.starts_30d ?? 0,
      noise: isNoise(p, pk),
      reason: reasons.get(p.project.id) ?? null,
    };
  });

  const q = input.query.trim();
  if (q) {
    const recentCut = input.now - 7 * 86_400;
    const scored = rows
      .map((r) => ({ r, s: fuzzyMatchFields(q, [r.key, r.project.project.repo, r.group]) }))
      .filter((x): x is { r: PickerRow; s: number } => x.s !== null)
      .map(({ r, s }) => {
        let bonus = 0;
        if (r.reason) bonus += 3;
        if (r.pick === 'pin') bonus += 3;
        bonus += Math.min(r.starts, 10) * 0.3;
        if ((r.project.project.last_session_at ?? 0) >= recentCut) bonus += 2;
        if (r.noise) bonus -= 10;
        return { r, total: s + bonus };
      })
      .sort((a, b) => b.total - a.total || byLabel(a.r, b.r));
    return { mode: 'search', rows: scored.map((x) => x.r) };
  }

  const taken = new Set<number>();
  const take = (list: PickerRow[]) => {
    for (const r of list) taken.add(r.id);
    return list;
  };
  const free = (r: PickerRow) => !taken.has(r.id);
  const byId = new Map(rows.map((r) => [r.id, r]));

  const sections: PickerSection[] = [];
  const push = (id: SectionId, key: string, label: string, list: PickerRow[], foldedByDefault = false) => {
    if (list.length > 0) sections.push({ id, key, label, rows: take(list), foldedByDefault });
  };

  push('pinned', 'pinned', 'Pinned', rows.filter((r) => r.pick === 'pin').sort(byLabel));
  push(
    'context',
    'context',
    'For this context',
    input.context
      .map((c) => byId.get(c.projectId))
      .filter((r): r is PickerRow => !!r && free(r))
      .slice(0, CONTEXT_CAP),
  );
  push(
    'recent',
    'recent',
    'Recent',
    rows
      .filter((r) => free(r) && !r.noise && r.project.project.last_session_at != null)
      .sort((a, b) => (b.project.project.last_session_at ?? 0) - (a.project.project.last_session_at ?? 0))
      .slice(0, RECENT_CAP),
  );
  push(
    'popular',
    'popular',
    'Popular',
    rows
      .filter((r) => free(r) && !r.noise && r.starts >= POPULAR_MIN)
      .sort((a, b) => b.starts - a.starts || byLabel(a, b))
      .slice(0, POPULAR_CAP),
  );
  const grouped = new Map<string, PickerRow[]>();
  for (const r of rows) if (free(r) && !r.noise) grouped.set(r.group, [...(grouped.get(r.group) ?? []), r]);
  for (const name of [...grouped.keys()].sort((a, b) => a.localeCompare(b, undefined, { sensitivity: 'base' }))) {
    const list = grouped.get(name)!.sort(byLabel);
    push('group', `group:${name}`, name, list, list.length > GROUP_FOLD_OVER);
  }
  push('other', 'other', 'Other', rows.filter((r) => free(r) && r.noise).sort(byLabel), true);
  return { mode: 'sections', sections };
}
```

Note on the first section test: `pinned-one` (9 days) is pinned, `ctx` is context; Recent takes the remaining non-noise by recency (`fresh`, `pop`, `plain`), so Popular and the groups are empty (not rendered) and only `never` is Other — the assertions encode exactly that. The second test is where Popular and a group section appear. If a test and this code disagree, fix whichever contradicts the **spec**, not whichever is easier.

- [ ] **Step 4: Run to verify pass**

Run: `npx vitest run src/lib/project_rank.test.ts`
Expected: all pass (13 tests).

- [ ] **Step 5: Commit**

```bash
git add src/lib/project_rank.ts src/lib/project_rank.test.ts
git commit -m "feat(picker): the pure ranking — noise, groups, context, sections, search"
```

---

### Task 7: The popover — `ProjectPicker.svelte`

**Files:**
- Create: `src/lib/ProjectPicker.svelte`
- Test: `src/lib/ProjectPicker.test.ts`

**Interfaces:**
- Consumes: `rankProjects`, `PickerRow`, `ContextSignal` (Task 6); `ProjectPick`, `Pick` (Task 5); `ProjectTreeRow`.
- Produces: component props
  ```ts
  {
    projects: readonly ProjectTreeRow[];
    picks: ReadonlyMap<string, ProjectPick>;
    context: readonly ContextSignal[];
    now: number;                     // unix seconds
    addProjectBlocked: string | null;
    onpick: (p: ProjectTreeRow) => void;
    onaddproject: () => void;
    onsetpick: (row: PickerRow, pick: Pick | null) => void;
    ongroup: (row: PickerRow) => void;
  }
  ```
  DOM contract kept for existing tests: root `.picker`; `role="listbox"` with `aria-label="Pick project for new session"`, whose **first element child** is `data-testid="add-project-row"`; project buttons have class `picker-item`. New test ids: `picker-search`, `section-<key>`, `fold-<key>`, `pick-row-<id>`, `pin-<id>`, `hide-<id>`, `keep-<id>`, `group-<id>`.

- [ ] **Step 1: Write the failing test** — `src/lib/ProjectPicker.test.ts`:

```ts
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi } from 'vitest';
import { tick } from 'svelte';
import ProjectPicker from './ProjectPicker.svelte';
import type { ProjectTreeRow } from './projects';
import { pickKey } from './project_picks';

const NOW = 1_800_000_000;
const proj = (id: number, repo: string, last: number | null): ProjectTreeRow => ({
  project: { id, owner: 'o', repo, base_path: `/p/${repo}`, last_session_at: last, adopted: false, system: false },
  worktrees: [],
});
const projects = [proj(1, 'alpha', NOW - 60), proj(2, 'beta', NOW - 3600), proj(3, 'ghost', null)];

function setup(over: Record<string, unknown> = {}) {
  const props = {
    projects,
    picks: new Map(),
    context: [],
    now: NOW,
    addProjectBlocked: null,
    onpick: vi.fn(),
    onaddproject: vi.fn(),
    onsetpick: vi.fn(),
    ongroup: vi.fn(),
    ...over,
  };
  render(ProjectPicker, { props });
  return props;
}

describe('ProjectPicker', () => {
  it('keeps Add project first in the listbox and folds Other', async () => {
    setup();
    const lb = screen.getByRole('listbox', { name: 'Pick project for new session' });
    expect(lb.firstElementChild).toBe(screen.getByTestId('add-project-row'));
    expect(screen.getByTestId('section-recent').textContent).toContain('alpha');
    expect(screen.getByTestId('section-other').textContent).toContain('Other (1)');
    expect(screen.queryByTestId('pick-row-3')).toBeNull();
    await fireEvent.click(screen.getByTestId('fold-other'));
    expect(screen.getByTestId('pick-row-3')).toBeInTheDocument();
  });

  it('typing searches, noise included; Enter opens the highlighted project', async () => {
    const p = setup();
    const input = screen.getByTestId('picker-search');
    await fireEvent.input(input, { target: { value: 'gho' } });
    await tick();
    expect(screen.getByTestId('pick-row-3')).toBeInTheDocument();
    expect(screen.queryByTestId('section-recent')).toBeNull();
    await fireEvent.keyDown(input, { key: 'Enter' });
    expect(p.onpick).toHaveBeenCalledWith(projects[2]);
  });

  it('arrow keys move the highlight', async () => {
    const p = setup();
    const input = screen.getByTestId('picker-search');
    await fireEvent.keyDown(input, { key: 'ArrowDown' });
    await fireEvent.keyDown(input, { key: 'Enter' });
    expect(p.onpick).toHaveBeenCalledWith(projects[1]);
  });

  it('Pin, Hide, Keep and Group call back with the row', async () => {
    const picks = new Map([[pickKey('o', 'beta'), { owner: 'o', repo: 'beta', pick: 'pin' as const, grp: null, starts_30d: 0 }]]);
    const p = setup({ picks });
    await fireEvent.click(screen.getByTestId('pin-2'));
    expect(p.onsetpick).toHaveBeenLastCalledWith(expect.objectContaining({ id: 2 }), null); // Unpin
    await fireEvent.click(screen.getByTestId('pin-1'));
    expect(p.onsetpick).toHaveBeenLastCalledWith(expect.objectContaining({ id: 1 }), 'pin');
    await fireEvent.click(screen.getByTestId('hide-1'));
    expect(p.onsetpick).toHaveBeenLastCalledWith(expect.objectContaining({ id: 1 }), 'hide');
    await fireEvent.click(screen.getByTestId('fold-other'));
    await fireEvent.click(screen.getByTestId('keep-3'));
    expect(p.onsetpick).toHaveBeenLastCalledWith(expect.objectContaining({ id: 3 }), 'keep');
    await fireEvent.click(screen.getByTestId('group-1'));
    expect(p.ongroup).toHaveBeenCalledWith(expect.objectContaining({ id: 1 }));
  });

  it('shows the context reason', () => {
    setup({ context: [{ projectId: 2, reason: 'on mefistos' }] });
    expect(screen.getByTestId('section-context').textContent).toContain('on mefistos');
  });
});
```

- [ ] **Step 2: Run to verify failure**

Run: `npx vitest run src/lib/ProjectPicker.test.ts`
Expected: FAIL — cannot resolve `./ProjectPicker.svelte`.

- [ ] **Step 3: Implement** — `src/lib/ProjectPicker.svelte`. Move the `.picker`, `.picker-item`, `.picker-item:hover`, `.picker-item.add-project` rules out of `Sidebar.svelte`'s `<style>` (Task 8 deletes them there) into this file:

```svelte
<script lang="ts">
  // The New session project picker (project picker spec, phase 1): a search
  // field over sections — Pinned, For this context, Recent, Popular, groups,
  // Other (noise, folded). The ranking is `project_rank.ts`; this file only
  // renders it and owns the query, the folds and the keyboard highlight.
  import type { ProjectTreeRow } from './projects';
  import type { Pick, ProjectPick } from './project_picks';
  import { rankProjects, type ContextSignal, type PickerRow, type PickerSection } from './project_rank';

  let {
    projects,
    picks,
    context,
    now,
    addProjectBlocked,
    onpick,
    onaddproject,
    onsetpick,
    ongroup,
  }: {
    projects: readonly ProjectTreeRow[];
    picks: ReadonlyMap<string, ProjectPick>;
    context: readonly ContextSignal[];
    now: number;
    addProjectBlocked: string | null;
    onpick: (p: ProjectTreeRow) => void;
    onaddproject: () => void;
    onsetpick: (row: PickerRow, pick: Pick | null) => void;
    ongroup: (row: PickerRow) => void;
  } = $props();

  let query = $state('');
  let toggled = $state<ReadonlySet<string>>(new Set());
  let active = $state(0);

  const view = $derived(rankProjects({ projects, picks, context, query, now }));
  const isOpen = (s: PickerSection) => (s.foldedByDefault ? toggled.has(s.key) : !toggled.has(s.key));
  const navRows: PickerRow[] = $derived(
    view.mode === 'search' ? view.rows : view.sections.flatMap((s) => (isOpen(s) ? s.rows : [])),
  );

  $effect(() => {
    void query;
    active = 0;
  });

  function toggle(key: string) {
    const next = new Set(toggled);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    toggled = next;
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      active = Math.min(active + 1, navRows.length - 1);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      active = Math.max(active - 1, 0);
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const r = navRows[active];
      if (r) onpick(r.project);
    }
  }
</script>

{#snippet row(r: PickerRow)}
  <div class="picker-row" class:active={navRows[active]?.id === r.id} data-testid="pick-row-{r.id}">
    <button class="picker-item" role="option" aria-selected={navRows[active]?.id === r.id} onclick={() => onpick(r.project)}>
      <span class="name">{r.label}</span>
      {#if r.reason}<span class="why">{r.reason}</span>{/if}
    </button>
    <span class="acts">
      <button type="button" data-testid="pin-{r.id}" title={r.pick === 'pin' ? 'Unpin' : 'Pin'}
        onclick={() => onsetpick(r, r.pick === 'pin' ? null : 'pin')}>{r.pick === 'pin' ? 'Unpin' : 'Pin'}</button>
      {#if r.noise}
        <button type="button" data-testid="keep-{r.id}" title="Not noise: keep it in the list" onclick={() => onsetpick(r, 'keep')}>Keep</button>
      {:else}
        <button type="button" data-testid="hide-{r.id}" title="Move to Other" onclick={() => onsetpick(r, 'hide')}>Hide</button>
      {/if}
      <button type="button" data-testid="group-{r.id}" title="Put in a group" onclick={() => ongroup(r)}>Group…</button>
    </span>
  </div>
{/snippet}

<div class="picker" data-testid="project-picker">
  <!-- svelte-ignore a11y_autofocus -->
  <input
    class="picker-search"
    data-testid="picker-search"
    placeholder="Search projects…"
    aria-label="Search projects"
    autofocus
    bind:value={query}
    onkeydown={onKey}
  />
  <div class="picker-list" role="listbox" aria-label="Pick project for new session">
    <button
      class="picker-item add-project"
      disabled={addProjectBlocked !== null}
      title={addProjectBlocked ?? ''}
      onclick={onaddproject}
      data-testid="add-project-row"
    >＋ Add project…</button>
    {#if view.mode === 'search'}
      {#each view.rows as r (r.id)}{@render row(r)}{/each}
      {#if view.rows.length === 0}<p class="empty">No project matches.</p>{/if}
    {:else}
      {#each view.sections as s (s.key)}
        <div class="picker-section" data-testid="section-{s.key}">
          <button type="button" class="section-head" data-testid="fold-{s.key}" aria-expanded={isOpen(s)} onclick={() => toggle(s.key)}>
            {isOpen(s) ? '▾' : '▸'} {s.label} ({s.rows.length})
          </button>
          {#if isOpen(s)}
            {#each s.rows as r (r.id)}{@render row(r)}{/each}
          {/if}
        </div>
      {/each}
      {#if view.sections.length === 0}<p class="empty">No projects yet. Add one, or refresh.</p>{/if}
    {/if}
  </div>
</div>

<style>
  .picker {
    position: absolute;
    bottom: 100%;
    left: 0;
    right: 0;
    margin-bottom: 0.3rem;
    border: 1px solid var(--border);
    background: var(--bg);
    border-radius: 5px;
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.3);
    z-index: 5;
    display: flex;
    flex-direction: column;
    max-height: min(60vh, 420px);
  }
  .picker-search {
    margin: 0.4rem;
    padding: 0.3rem 0.5rem;
    font-size: 0.85rem;
    flex: 0 0 auto;
  }
  .picker-list { overflow: auto; min-height: 0; }
  .picker-item {
    display: block;
    width: 100%;
    text-align: left;
    border: none;
    background: transparent;
    color: var(--fg);
    font-size: 0.85rem;
    padding: 0.4rem 0.6rem;
    cursor: pointer;
  }
  .picker-item:hover { background: var(--bg-pane); }
  .picker-item.add-project { color: var(--accent); border-bottom: 1px solid var(--border); }
  .section-head {
    display: block;
    width: 100%;
    text-align: left;
    border: none;
    background: transparent;
    color: var(--fg-muted);
    font-size: 0.72rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 0.45rem 0.6rem 0.2rem;
    cursor: pointer;
  }
  .picker-row { display: flex; align-items: center; }
  .picker-row .picker-item { flex: 1 1 auto; min-width: 0; display: flex; gap: 0.5rem; align-items: baseline; }
  .picker-row.active .picker-item { background: var(--bg-pane); }
  .name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .why { color: var(--fg-muted); font-size: 0.75rem; white-space: nowrap; }
  .acts { display: none; gap: 0.2rem; padding-right: 0.3rem; flex: 0 0 auto; }
  .picker-row:hover .acts, .picker-row:focus-within .acts { display: flex; }
  .acts button {
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-muted);
    font-size: 0.7rem;
    padding: 0.05rem 0.3rem;
    border-radius: 3px;
    cursor: pointer;
  }
  .acts button:hover { color: var(--fg); border-color: var(--accent); }
  .empty { color: var(--fg-muted); font-size: 0.8rem; padding: 0.5rem 0.6rem; margin: 0; }
</style>
```

(`.acts` are hidden until hover/focus; jsdom still finds them by test id — `display: none` from a scoped stylesheet is not applied in jsdom, and `getByTestId` ignores visibility either way.)

- [ ] **Step 4: Run to verify pass**

Run: `npx vitest run src/lib/ProjectPicker.test.ts`
Expected: `5 passed`.

- [ ] **Step 5: Type-check, then commit**

Run: `npx svelte-check --threshold error`
Expected: 0 errors.

```bash
git add src/lib/ProjectPicker.svelte src/lib/ProjectPicker.test.ts
git commit -m "feat(picker): the ProjectPicker popover — search, sections, folds, row actions"
```

---

### Task 8: Sidebar integration

**Files:**
- Modify: `src/lib/Sidebar.svelte` (picker markup ~line 1599–1620; state ~842–911; `onProjectAdded`; the `newSessionHostRequest` effect; `.picker*` styles ~2094–2117)
- Modify: `src/lib/Sidebar.test.ts` (picker tests ~805–960; `mockBackend` ~93)

**Interfaces:**
- Consumes: `ProjectPicker` (Task 7), `contextSignals` + `PickerRow` (Task 6), `projectPicks`, `loadProjectPicks`, `setProjectPick` (Task 5), existing `PromptDialog.svelte`, `listFacets`, `filteredSessionsByProject`, `pickerHost`, `$selectedSession`, `$sessions`, `addProjectBlocked`, `openNew`, `openAddProject`.
- Produces: nothing new for later tasks.

- [ ] **Step 1: Update and add Sidebar tests first** — in `Sidebar.test.ts`:

1. In `mockBackend`, add before `return null;`:
```ts
    if (cmd === 'project_picks') return [];
    if (cmd === 'set_project_pick') {
      const a = (args?.args ?? {}) as { owner: string; repo: string; pick: string | null; grp: string | null };
      return { owner: a.owner, repo: a.repo, pick: a.pick, grp: a.grp, starts_30d: 0 };
    }
```
(widen the `args` parameter type of the `mockImplementation` callback if TypeScript complains.)

2. Replace the body of `'project picker shows ALL projects regardless of recency/search filter'` from `const listbox = …` on with:
```ts
    const listbox = screen.getByRole('listbox');
    // The sidebar's filter never hides a project from the picker: the two
    // used ones are listed, and phone-manager (never had a session) is in
    // the folded Other, one click away — and found by typing.
    expect(listbox.textContent).toContain('claude-fleet');
    expect(listbox.textContent).toContain('pos-frontend');
    expect(listbox.textContent).toContain('Other (1)');
    await fireEvent.input(screen.getByTestId('picker-search'), { target: { value: 'phone' } });
    await tick();
    expect(screen.getByRole('listbox').textContent).toContain('phone-manager');
```

3. Add:
```ts
  it('the picker loads project picks when it opens and pins through set_project_pick', async () => {
    mockBackend(fakeProjects, [sessionFor(1)]);
    render(Sidebar);
    await tick(); await tick();
    await fireEvent.click(screen.getByTestId('new-session-footer'));
    await tick(); await tick();
    const calls = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.map((c) => c[0]);
    expect(calls).toContain('project_picks');
    await fireEvent.click(screen.getByTestId('pin-2'));
    await tick(); await tick();
    expect(mockedInvoke).toHaveBeenCalledWith('set_project_pick', {
      args: { owner: 'papayapos', repo: 'pos-frontend', pick: 'pin', grp: null },
    });
    expect(screen.getByTestId('section-pinned').textContent).toContain('pos-frontend');
  });

  it('a project added from the picker is kept (never noise)', async () => {
    const added = {
      project: { id: 42, owner: 'newowner', repo: 'fresh-repo', base_path: '/r/fresh', last_session_at: null, adopted: false, system: false },
      worktrees: [],
    };
    mockBackend(fakeProjects, []);
    const base = (mockedInvoke as ReturnType<typeof vi.fn>).getMockImplementation() as (cmd: string, args?: unknown) => Promise<unknown>;
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string, args?: unknown) =>
      cmd === 'add_project' ? added : base(cmd, args),
    );
    render(Sidebar);
    await tick(); await tick();
    await fireEvent.click(screen.getByTestId('new-session-footer'));
    await tick();
    await fireEvent.click(screen.getByTestId('add-project-row'));
    await tick();
    await fireEvent.input(screen.getByTestId('clone-url'), { target: { value: 'newowner/fresh-repo' } });
    await fireEvent.click(screen.getByTestId('add-create'));
    await vi.waitFor(() => expect(screen.queryByTestId('add-project-dialog')).toBeNull());
    expect(mockedInvoke).toHaveBeenCalledWith('set_project_pick', {
      args: { owner: 'newowner', repo: 'fresh-repo', pick: 'keep', grp: null },
    });
  });
```

4. If any test asserts focus on the first `.picker-item` after a *New session on host* request, change it to expect `screen.getByTestId('picker-search')` to be `document.activeElement`.

- [ ] **Step 2: Run to verify the new/changed tests fail**

Run: `npx vitest run src/lib/Sidebar.test.ts`
Expected: the updated "ALL projects" test and the two new tests FAIL (no `picker-search`, no `pin-2`); the rest pass.

- [ ] **Step 3: Implement in `Sidebar.svelte`**

Imports (script top):
```ts
  import ProjectPicker from './ProjectPicker.svelte';
  import PromptDialog from './PromptDialog.svelte';
  import { contextSignals, type PickerRow } from './project_rank';
  import { loadProjectPicks, projectPicks, setProjectPick, type Pick } from './project_picks';
```
(Skip `PromptDialog` if Sidebar already imports it.)

State and derivations, next to `let showProjectPicker = $state(false);`:
```ts
  // The picker's context (project picker spec): the preferred host, the
  // selected session, and — only while a filter narrows the list — the
  // projects of the sessions the filtered tree shows.
  const pickerContext = $derived(
    showProjectPicker
      ? contextSignals({
          preferredHost: pickerHost ?? null,
          selectedProjectId: $selectedSession?.project_id ?? null,
          sessions: $sessions,
          filterLabels: listFacets.map((f) => f.label),
          filteredProjectIds: filteredSessionsByProject.keys(),
        })
      : [],
  );
  let groupingRow = $state<PickerRow | null>(null);

  $effect(() => {
    if (showProjectPicker) void loadProjectPicks();
  });

  function onSetPick(row: PickerRow, pick: Pick | null) {
    void setProjectPick(row.project.project.owner, row.project.project.repo, { pick });
  }

  function onGroup(row: PickerRow) {
    showProjectPicker = false;
    groupingRow = row;
  }
```
Because `pickerContext` reads `listFacets` and `filteredSessionsByProject`, place it **after** both are declared (move it below line ~800 if needed; `$derived` order matters only for readability, not correctness, but keep declarations before use).

`onProjectAdded` — first line of the body:
```ts
    // Adding a project is the person saying it matters: never noise.
    void setProjectPick(row.project.owner, row.project.repo, { pick: 'keep' });
```

The `newSessionHostRequest` effect — replace its `tick().then(...)` body with:
```ts
    void tick().then(() => {
      sidebarEl?.querySelector<HTMLElement>('[data-testid="picker-search"]')?.focus();
    });
```

Markup — replace the whole `{#if showProjectPicker} <div class="picker" …> … </div> {/if}` block with:
```svelte
    {#if showProjectPicker}
      <ProjectPicker
        projects={$projects}
        picks={$projectPicks}
        context={pickerContext}
        now={Math.floor(Date.now() / 1000)}
        {addProjectBlocked}
        onpick={(p) => openNew(p)}
        onaddproject={openAddProject}
        onsetpick={onSetPick}
        ongroup={onGroup}
      />
    {/if}
```

After the `{#if showAddProject}` block, add the group dialog:
```svelte
{#if groupingRow}
  {@const g = groupingRow}
  <PromptDialog
    title={`Group for ${g.key}`}
    label="Group"
    initialValue={g.manualGroup ?? g.group}
    confirmLabel="Save"
    validate={(v) => (v.length > 40 ? 'At most 40 characters' : null)}
    onsubmit={(v) => {
      void setProjectPick(g.project.project.owner, g.project.project.repo, { grp: v });
      groupingRow = null;
    }}
    oncancel={() => (groupingRow = null)}
  >
    {#if g.manualGroup}
      <button
        type="button"
        data-testid="ungroup"
        onclick={() => {
          void setProjectPick(g.project.project.owner, g.project.project.repo, { grp: null });
          groupingRow = null;
        }}>Back to automatic grouping</button
      >
    {/if}
  </PromptDialog>
{/if}
```

Delete `allProjectsSorted` if nothing else uses it (check with a search of the file), and delete the `.picker`, `.picker-item`, `.picker-item:hover`, `.picker-item.add-project` rules from Sidebar's `<style>` (they now live in `ProjectPicker.svelte`). Keep `collidingRepos` only if still used elsewhere in the file.

- [ ] **Step 4: Run Sidebar and hub-disabled tests**

Run: `npx vitest run src/lib/Sidebar.test.ts src/lib/hub_disabled.test.ts`
Expected: all pass.

- [ ] **Step 5: Full frontend suite + type-check**

Run: `npx vitest run`
Then: `npx svelte-check --threshold error`
Expected: all pass, 0 errors.

- [ ] **Step 6: Commit**

```bash
git add src/lib/Sidebar.svelte src/lib/Sidebar.test.ts
git commit -m "feat(picker): the sidebar opens the new picker with its context; added projects are kept"
```

---

### Task 9: Quick switcher drops noise from its idle list

**Files:**
- Modify: `src/lib/quick_switcher.ts` (add an exported helper)
- Modify: `src/lib/QuickSwitcher.svelte` (~line 119–125)
- Test: `src/lib/quick_switcher.test.ts`

**Interfaces:**
- Consumes: `isNoise` (Task 6), `projectPicks`, `loadProjectPicks`, `pickKey` (Task 5).
- Produces: `dropIdleNoise(entries: readonly SwitcherEntry[], query: string, isNoiseProject: (p: ProjectTreeRow) => boolean): SwitcherEntry[]`

- [ ] **Step 1: Write the failing test** — append to `src/lib/quick_switcher.test.ts` (reuse its existing project/session fixtures if it has them; otherwise these literals):

```ts
import { dropIdleNoise, type SwitcherEntry } from './quick_switcher';

describe('dropIdleNoise', () => {
  const project = (id: number, repo: string) =>
    ({
      kind: 'project',
      key: `project:${id}`,
      label: `New session in o/${repo}`,
      description: '',
      meta: '+',
      fields: [`o/${repo}`],
      project: { project: { id, owner: 'o', repo, base_path: '', last_session_at: null, adopted: false, system: false }, worktrees: [] },
    }) as SwitcherEntry;
  const entries = [project(1, 'keep-me'), project(2, 'noise')];
  const noisy = (p: { project: { id: number } }) => p.project.id === 2;

  it('drops noise projects while the query is empty', () => {
    expect(dropIdleNoise(entries, '  ', noisy).map((e) => e.key)).toEqual(['project:1']);
  });
  it('keeps them once something is typed', () => {
    expect(dropIdleNoise(entries, 'noi', noisy).map((e) => e.key)).toEqual(['project:1', 'project:2']);
  });
});
```

- [ ] **Step 2: Run to verify failure**

Run: `npx vitest run src/lib/quick_switcher.test.ts`
Expected: FAIL — `dropIdleNoise` is not exported.

- [ ] **Step 3: Implement** — in `quick_switcher.ts`, after `rankEntries`:

```ts
/**
 * The project picker's noise (`project_rank.isNoise`) stays out of the idle
 * list — the same projects the picker folds into Other — and comes back as
 * soon as anything is typed, so a hidden project is always findable.
 */
export function dropIdleNoise(
  entries: readonly SwitcherEntry[],
  query: string,
  isNoiseProject: (p: ProjectTreeRow) => boolean,
): SwitcherEntry[] {
  if (query.trim()) return [...entries];
  return entries.filter((e) => e.kind !== 'project' || !e.project || !isNoiseProject(e.project));
}
```

In `QuickSwitcher.svelte`, import `isNoise` from `./project_rank` and `projectPicks, loadProjectPicks, pickKey` from `./project_picks`; load picks when the switcher mounts/opens (`$effect(() => { void loadProjectPicks(); })` in the component that exists only while open — check how the switcher is mounted and put the load where it runs once per open); and change the ranked line to:

```ts
  const ranked: SwitcherEntry[] = $derived(
    rankEntries(
      dropIdleNoise(entries, query, (p) => isNoise(p, $projectPicks.get(pickKey(p.project.owner, p.project.repo)))),
      query,
      $recentSessions,
    ),
  );
```

- [ ] **Step 4: Run switcher tests and the whole suite**

Run: `npx vitest run src/lib/quick_switcher.test.ts src/lib/QuickSwitcher.test.ts`
Then: `npx vitest run`
Expected: all pass. If a QuickSwitcher test fixture has a project with `last_session_at: null` and expects it on the empty-query list, update that test to type its name first — that is the new behaviour the spec asks for — and say so in the commit message.

- [ ] **Step 5: Commit**

```bash
git add src/lib/quick_switcher.ts src/lib/quick_switcher.test.ts src/lib/QuickSwitcher.svelte
git commit -m "feat(picker): the quick switcher hides picker noise until you type"
```

---

### Task 10: Whole-tree verification

**Files:** none new (fixes only, if CI finds something).

- [ ] **Step 1: Sync check** — `git fetch origin && git log --oneline HEAD..origin/main | head`. If `main` moved, merge `origin/main` locally (migration number collision is the likely conflict: renumber 102 if `main` took it), then continue.

- [ ] **Step 2: Run the local CI** in the foreground, unpiped:

```bash
scripts/ci-local.sh
```

Expected: every stage green. Read the full output; do not judge by a tail.

- [ ] **Step 3: Generated files are current** — `git status --short` must show nothing after the run (a regen that the run rewrote means a REGEN step was missed: re-run it and commit).

- [ ] **Step 4: Manual smoke (optional, desktop):** don't start a dev build against the real HOME (it migrates the production `state.db`); if you want to see it, use the project's sandboxed run path. Otherwise the component tests are the evidence.

- [ ] **Step 5: Commit any fixes**

```bash
git add -A
git commit -m "chore(picker): CI fixes"
```

(Only if Step 2/3 required changes.)
