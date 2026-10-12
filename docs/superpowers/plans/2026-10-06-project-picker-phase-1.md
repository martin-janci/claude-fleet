# New session picker in ⌘K, phase 1 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the sidebar's flat "+ New session" popover with a *New session* mode of the existing ⌘K switcher: Start from work (tickets), Pinned, Suggested (frecency + context), every project in its group (dormant dimmed), Hidden folded — backed by stored pin/visibility/group per project and keyboard actions with undo.

**Architecture:** Backend: migration 104 adds one TEXT-keyed table `project_picks`; two hub tools / Tauri commands (`project_picks`, `set_project_pick`), `Access::PersonDevice`, `Routed` on a hub client. Frontend: a picks store, a local frecency pref, a pure ranking module (`project_rank.ts`) that builds the sections, small extensions to `PickerList`, an actions menu component, a `mode` in `QuickSwitcher`, `autostart` in `NewSessionDialog`; the Sidebar popover is deleted and every entry point opens the switcher.

**Tech Stack:** Rust (rusqlite, rmcp `#[tool]`, Tauri 2 commands), Svelte 5 runes + svelte/store, Vitest + @testing-library/svelte.

**Spec:** `docs/superpowers/specs/2026-10-05-project-picker-design.md` (v2; phase 1 only). Mockup: https://claude.ai/artifact/KzEbmrQvhhBiQq54fDs5Rj

## Global Constraints

- Stored state is keyed by `owner` + `repo` TEXT, **never** `project_id` (project rows are re-created; review C22, `migrations/050_orgs.sql`). Picker state is **not** added to `ProjectRow`.
- `project_picks` row: `pinned` (bool), `vis` ∈ `hide | keep | null`, `grp` (trimmed, ≤ 40 chars, blank = null). `pinned=false, vis=null, grp=null` deletes the row.
- Hidden = `vis='hide'`, or (`vis≠keep`, not pinned, no `grp`, no session in 30 days, repo matches `^(test|tmp|example)-` / `-analysis$` / `-epic-\d+$`). Dormant = no `last_session_at` or older than 90 days. `system` projects never appear.
- Clusters: per owner, over all that owner's projects, longest leading dash-prefix shared by ≥ 3 repos; a single-token repo equal to a cluster prefix joins it. Fallbacks: `More from <owner>` (owner has ≥ 3 projects), else `Forks & others`.
- Suggested ≤ 7: selected session's project (`current session`), then projects with a session on the preferred host (`on <host>`), then frecency. ⌘1…⌘9 over Pinned then Suggested.
- Frecency: local pref `newsession.frecency`, half-life 7 days, score ×10 + `20·2^(−days/7)` from `last_session_at`, cap 200 keys.
- Search boosts on the fuzzy scale: pinned +40, frecency ≤ +60, current session +80, hidden −400.
- New hub tools `Access::PersonDevice`; `project_picks` readonly, `set_project_pick` not. Do **not** bump `CONTRACT_REVISION`.
- The ranking is snapshotted when the switcher opens; recomputed only on query change, a fold, or the person's own action. Highlight by key.
- UI copy: English, sentence case — "New session in", "project or ticket…", "Start from work", "Pinned", "Suggested", "Hidden", "More from <owner>", "Forks & others", "Add project…", "Pin to top", "Unpin", "Hide", "Unhide", "Move to group…", "Back to automatic". No emoji in shipped UI; icons are inline stroke SVGs.
- Frontend tests: `npx vitest run <file>`; type-check `npx svelte-check --threshold error`; run `pnpm install --frozen-lockfile` once first.
- Rust: the repo's validation ladder (CLAUDE.md): `cargo fleet-fast-check` while editing, `cargo fleet-test -- <filter>` for tests, `cargo fleet-lint` for clippy, `scripts/verify.sh` before each commit; never `-p <crate>`, never `cargo build` to check compilation. A `REGEN_*` run is meant to fail once; re-run without the variable. Foreground, one command per call. Before calling a failure "pre-existing", check `origin/main`. Never judge a run through `| tail`.

## File map

| File | Status | Task |
|------|--------|------|
| `crates/fleet-core/migrations/104_project_picks.sql` | create | 1 |
| `crates/fleet-core/src/store/project_picks.rs` | create | 1 |
| `crates/fleet-core/src/store/schema.rs`, `store/mod.rs` | modify | 1 |
| `crates/fleet-core/src/service/project_picks.rs`, `service/mod.rs` | create / modify | 2 |
| `crates/fleet-core/src/mcp/tools/repo.rs`, `mcp/guard.rs`, `mcp/tools/tests.rs` | modify | 2 |
| `src-tauri/src/commands/projects.rs`, `src-tauri/src/lib.rs` | modify | 3 |
| `src-tauri/src/backend/verdicts.rs`, `backend/tests_routing.rs` | modify | 3 |
| generated: `docs/control-api-reference.md`, `src/lib/hub_verdicts.generated.json`, `docs/hub.md`, goldens | regen | 2, 3 |
| `src/lib/project_picks.ts` (+test) | create | 4 |
| `src/App.svelte` | modify | 4, 9, 11 |
| `src/lib/frecency.ts` (+test) | create | 5 |
| `src/lib/project_rank.ts` (+test) | create | 6 |
| `src/lib/PickerList.svelte` (+test) | modify | 7 |
| `src/lib/ProjectActionsMenu.svelte` (+test) | create | 8 |
| `src/lib/NewSessionDialog.svelte` (+test), `src/lib/new_session_request.ts` | modify | 9 |
| `src/lib/switcher_request.ts`, `src/lib/app_views.ts`, `src/lib/quick_switcher.ts`, `src/lib/QuickSwitcher.svelte` (+tests) | create / modify | 10 |
| `src/lib/Sidebar.svelte`, `src/lib/AddProjectDialog.svelte` (+tests), `src/lib/hub_disabled.test.ts` | modify | 11 |

---

### Task 1: Storage — migration 104 and the store module

**Files:**
- Create: `crates/fleet-core/migrations/104_project_picks.sql`
- Create: `crates/fleet-core/src/store/project_picks.rs`
- Modify: `crates/fleet-core/src/store/schema.rs` (append to `MIGRATIONS` after the `version: 103` entry)
- Modify: `crates/fleet-core/src/store/mod.rs` (`mod project_picks;` beside `mod projects;`; re-export beside `pub use reports::…`)

**Interfaces — produces (`crate::store`):**
- `pub struct ProjectPickRow { pub owner: String, pub repo: String, pub pinned: bool, pub vis: Option<String>, pub grp: Option<String> }` — Serialize, Deserialize, Debug, Clone, PartialEq, Eq; `#[serde(default)]` on `pinned`, `vis`, `grp`.
- `pub const PROJECT_VIS: [&str; 2] = ["hide", "keep"]`, `pub const PROJECT_GROUP_MAX_CHARS: usize = 40`.
- `Store::list_project_picks(&self) -> Result<Vec<ProjectPickRow>, IpcError>`
- `Store::set_project_pick(&self, owner: &str, repo: &str, pinned: bool, vis: Option<&str>, grp: Option<&str>, now: i64) -> Result<ProjectPickRow, IpcError>`

- [ ] **Step 1: The migration** — `104_project_picks.sql` (renumber everywhere in this task if 104 is taken on `origin/main`):

```sql
-- The New session picker (docs/superpowers/specs/2026-10-05-project-picker-design.md):
-- a person's choices per project — pinned, visibility (hide | keep) and the
-- picker group. Keyed by owner/repo TEXT, never project_id: project rows are
-- deleted and re-created, their ids re-derived (review C22, see 050). No
-- foreign key to projects, by design: a choice outlives a re-scan.
CREATE TABLE IF NOT EXISTS project_picks (
  owner      TEXT    NOT NULL,
  repo       TEXT    NOT NULL,
  pinned     INTEGER NOT NULL DEFAULT 0 CHECK (pinned IN (0, 1)),
  vis        TEXT    CHECK (vis IS NULL OR vis IN ('hide', 'keep')),
  grp        TEXT,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (owner, repo)
);

INSERT OR IGNORE INTO schema_version (version) VALUES (104);
```

- [ ] **Step 2: Register it** — in `schema.rs` after the `version: 103` entry:

```rust
    // The New session picker: a person's pin / visibility / group per
    // project, keyed by owner/repo TEXT. A new table only, so plain.
    Migration::plain(104, include_str!("../../migrations/104_project_picks.sql")),
```

- [ ] **Step 3: Write the failing tests** — create `store/project_picks.rs` with `use super::*;` and:

```rust
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

    #[test]
    fn every_non_system_project_is_listed_with_empty_state() {
        let s = store_with(&[("o", "a"), ("o", "b")]);
        s.upsert_system_project("fleet", "operator", "/op").unwrap();
        let rows = s.list_project_picks().unwrap();
        let names: Vec<_> = rows.iter().map(|r| format!("{}/{}", r.owner, r.repo)).collect();
        assert_eq!(names, ["o/a", "o/b"]);
        assert!(rows.iter().all(|r| !r.pinned && r.vis.is_none() && r.grp.is_none()));
    }

    #[test]
    fn set_round_trips_and_the_empty_state_deletes_the_row() {
        let s = store_with(&[("o", "a")]);
        let row = s.set_project_pick("o", "a", true, Some("keep"), Some("  tools "), NOW).unwrap();
        assert!(row.pinned);
        assert_eq!(row.vis.as_deref(), Some("keep"));
        assert_eq!(row.grp.as_deref(), Some("tools"), "trimmed");
        let row = s.set_project_pick("o", "a", false, None, None, NOW).unwrap();
        assert_eq!((row.pinned, row.vis, row.grp), (false, None, None));
        let n: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM project_picks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn pinned_and_visibility_are_independent() {
        let s = store_with(&[("o", "a")]);
        s.set_project_pick("o", "a", true, Some("hide"), None, NOW).unwrap();
        let row = s.set_project_pick("o", "a", false, Some("hide"), None, NOW).unwrap();
        assert_eq!(row.vis.as_deref(), Some("hide"), "unpinning never touches visibility");
    }

    #[test]
    fn set_validates_vis_group_and_project() {
        let s = store_with(&[("o", "a")]);
        let e = s.set_project_pick("o", "a", false, Some("star"), None, NOW).unwrap_err();
        assert_eq!(e.code, crate::ipc_error::codes::E_INVALID);
        let long = "x".repeat(PROJECT_GROUP_MAX_CHARS + 1);
        let e = s.set_project_pick("o", "a", false, None, Some(&long), NOW).unwrap_err();
        assert_eq!(e.code, crate::ipc_error::codes::E_INVALID);
        let e = s.set_project_pick("o", "nope", true, None, None, NOW).unwrap_err();
        assert_eq!(e.code, crate::ipc_error::codes::E_NOTFOUND);
        let row = s.set_project_pick("o", "a", true, None, Some("   "), NOW).unwrap();
        assert_eq!(row.grp, None, "a blank group clears");
    }

    #[test]
    fn a_pick_survives_the_project_row_being_recreated() {
        let s = store_with(&[("o", "a")]);
        s.set_project_pick("o", "a", true, None, None, NOW).unwrap();
        s.conn.execute("DELETE FROM projects", []).unwrap();
        s.upsert_project("o", "a", "/p/o/a").unwrap();
        assert!(s.list_project_picks().unwrap()[0].pinned);
    }
}
```

Register in `store/mod.rs`: `mod project_picks;` and `pub use project_picks::{ProjectPickRow, PROJECT_GROUP_MAX_CHARS, PROJECT_VIS};`.

- [ ] **Step 4: Run to verify failure** — `cargo fleet-test -- store::project_picks` → compile errors (types/methods missing).

- [ ] **Step 5: Implement** — above the tests:

```rust
//! The New session picker's per-project choices (project picker spec v2):
//! pinned, visibility (hide | keep) and the picker group. Keyed by
//! `owner`/`repo` TEXT, never `project_id` — project rows are deleted and
//! re-created, their ids re-derived (review C22, migration 050).

use super::*;
use crate::ipc_error::{codes, IpcError};

/// One project's picker choices. `serde(default)`: this row crosses the
/// hub wire.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProjectPickRow {
    pub owner: String,
    pub repo: String,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub vis: Option<String>,
    #[serde(default)]
    pub grp: Option<String>,
}

/// The values `project_picks.vis` takes (the migration's CHECK).
pub const PROJECT_VIS: [&str; 2] = ["hide", "keep"];
/// A picker group's name, at most.
pub const PROJECT_GROUP_MAX_CHARS: usize = 40;

const PICKS_SELECT: &str = "SELECT p.owner, p.repo, COALESCE(k.pinned, 0), k.vis, k.grp
   FROM projects p
   LEFT JOIN project_picks k ON k.owner = p.owner AND k.repo = p.repo
  WHERE p.system = 0";

fn map_pick_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ProjectPickRow> {
    Ok(ProjectPickRow {
        owner: r.get(0)?,
        repo: r.get(1)?,
        pinned: r.get::<_, i64>(2)? != 0,
        vis: r.get(3)?,
        grp: r.get(4)?,
    })
}

impl Store {
    /// Every non-system project's picker choices, by owner then repo.
    pub fn list_project_picks(&self) -> Result<Vec<ProjectPickRow>, IpcError> {
        let mut stmt = self
            .conn
            .prepare_cached(&format!("{PICKS_SELECT} ORDER BY p.owner, p.repo"))?;
        let rows = stmt.query_map([], map_pick_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Replace one project's choices (full replace; the empty state deletes
    /// the row) and return them. `E_INVALID` for a `vis` outside
    /// [`PROJECT_VIS`] or a group over [`PROJECT_GROUP_MAX_CHARS`];
    /// `E_NOTFOUND` for a project fleet does not know. A blank group clears.
    pub fn set_project_pick(
        &self,
        owner: &str,
        repo: &str,
        pinned: bool,
        vis: Option<&str>,
        grp: Option<&str>,
        now: i64,
    ) -> Result<ProjectPickRow, IpcError> {
        if let Some(v) = vis {
            if !PROJECT_VIS.contains(&v) {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!("vis must be hide or keep, not {v:?}"),
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
        if !pinned && vis.is_none() && grp.is_none() {
            self.conn.execute(
                "DELETE FROM project_picks WHERE owner = ?1 AND repo = ?2",
                rusqlite::params![owner, repo],
            )?;
        } else {
            self.conn.execute(
                "INSERT INTO project_picks (owner, repo, pinned, vis, grp, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(owner, repo) DO UPDATE SET
                   pinned = excluded.pinned, vis = excluded.vis, grp = excluded.grp,
                   updated_at = excluded.updated_at",
                rusqlite::params![owner, repo, pinned as i64, vis, grp, now],
            )?;
        }
        let mut stmt = self
            .conn
            .prepare_cached(&format!("{PICKS_SELECT} AND p.owner = ?1 AND p.repo = ?2"))?;
        Ok(stmt.query_row(rusqlite::params![owner, repo], map_pick_row)?)
    }
}
```

- [ ] **Step 6: Run** — `cargo fleet-test -- store::project_picks` → `5 passed`.
- [ ] **Step 7: Store suite** — `cargo fleet-test -- store::` → all pass. A golden pinning the schema version names its REGEN variable in its failure; regenerate, never hand-edit.
- [ ] **Step 8: fmt + clippy** — `cargo fmt --all`; `cargo fleet-lint`.
- [ ] **Step 9: Commit**

```bash
git add crates/fleet-core/migrations/104_project_picks.sql crates/fleet-core/src/store/project_picks.rs crates/fleet-core/src/store/schema.rs crates/fleet-core/src/store/mod.rs
git commit -m "feat(picker): project_picks — pinned, visibility and group per owner/repo"
```

---

### Task 2: Service and hub tools

**Files:**
- Create: `crates/fleet-core/src/service/project_picks.rs`; Modify: `service/mod.rs` (`pub mod project_picks;` beside `pub mod projects;`)
- Modify: `crates/fleet-core/src/mcp/tools/repo.rs` (import line `use crate::service::{add_project, repo, repo_read};` → add `project_picks`; two tools after `forget_project`)
- Modify: `crates/fleet-core/src/mcp/guard.rs` (`TOOL_POLICIES`, after the `forget_project` row)
- Modify: `crates/fleet-core/src/mcp/tools/tests.rs` (after `settings_reach_a_persons_device_and_never_a_host_or_an_org_bound_client`)
- Regen: `docs/control-api-reference.md`

**Interfaces:**
- Consumes: Task 1.
- Produces: `service::project_picks::SetProjectPickArgs { owner: String, repo: String, pinned: bool, vis: Option<String>, grp: Option<String> }` (Debug, Clone, Serialize, Deserialize, JsonSchema; `#[serde(default)]` on the last three); `list(&Mutex<Store>) -> Result<Vec<ProjectPickRow>, IpcError>`; `set(&Mutex<Store>, &SetProjectPickArgs) -> Result<ProjectPickRow, IpcError>`; hub tools `project_picks` (no params) and `set_project_pick` (params = `SetProjectPickArgs`).

- [ ] **Step 1: Failing tests** in `mcp/tools/tests.rs`:

```rust
// ---- the New session picker (phase 1) ----

/// A person's preference: their paired device reads (any mode) and writes
/// (full); never a host's token; not served to the master.
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
            pinned: true,
            vis: None,
            grp: Some("tools".into()),
        }))
        .await
        .unwrap();
    let v = result_json(&set);
    assert_eq!(v["pinned"], true);
    assert_eq!(v["grp"], "tools");
    let list = result_json(&tools.project_picks().await.unwrap());
    assert_eq!(list[0]["repo"], "r");
    assert_eq!(list[0]["pinned"], true);
}
```

- [ ] **Step 2: Verify failure** — `cargo fleet-test -- project_pick` → compile error.

- [ ] **Step 3: Service** — `service/project_picks.rs`:

```rust
//! The New session picker's per-project choices (project picker spec v2):
//! the transport-agnostic layer the Tauri commands and the hub tools share.
//! The rules live in `Store::set_project_pick`.

use crate::ipc_error::{lock, IpcError};
use crate::store::{now_unix, ProjectPickRow, Store};
use std::sync::Mutex;

/// One project's new picker choices — a full replace: send the current
/// value of what you are not changing. The empty state removes the row.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct SetProjectPickArgs {
    /// The project's owner, as list_projects names it.
    pub owner: String,
    /// The project's repo, as list_projects names it.
    pub repo: String,
    /// Pinned to the top of the picker.
    #[serde(default)]
    pub pinned: bool,
    /// hide | keep; null = the rules decide.
    #[serde(default)]
    pub vis: Option<String>,
    /// The picker group (at most 40 characters); null or blank = automatic.
    #[serde(default)]
    pub grp: Option<String>,
}

pub fn list(store: &Mutex<Store>) -> Result<Vec<ProjectPickRow>, IpcError> {
    lock(store)?.list_project_picks()
}

pub fn set(store: &Mutex<Store>, args: &SetProjectPickArgs) -> Result<ProjectPickRow, IpcError> {
    lock(store)?.set_project_pick(
        &args.owner,
        &args.repo,
        args.pinned,
        args.vis.as_deref(),
        args.grp.as_deref(),
        now_unix(),
    )
}
```

(If `lock`'s error type differs in your checkout, mirror `service::projects`.)

- [ ] **Step 4: Tools** — in `repo.rs` after `forget_project`:

```rust
    #[tool(description = "The New session picker's choices per project: \
        pinned, vis (hide|keep), group.")]
    pub(super) async fn project_picks(&self) -> Result<CallToolResult, McpError> {
        audit("project_picks", "");
        ok_json_compact(&project_picks::list(&self.store).map_err(to_mcp_err)?)
    }

    #[tool(description = "Replace one project's picker choices: pinned, \
        vis hide|keep|null, group or null. The empty state clears.")]
    pub(super) async fn set_project_pick(
        &self,
        Parameters(args): Parameters<project_picks::SetProjectPickArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "set_project_pick",
            &format!(
                "project={:?}/{:?} pinned={} vis={:?} grp={:?}",
                args.owner, args.repo, args.pinned, args.vis, args.grp
            ),
        );
        ok_json_compact(&project_picks::set(&self.store, &args).map_err(to_mcp_err)?)
    }
```

- [ ] **Step 5: Policies** — in `guard.rs` after the `forget_project` row:

```rust
    // The New session picker (phase 1): a person's preference, so a person's
    // own device only, under the desktop commands' own names. Not served to
    // the master — nothing an agent needs, and its surface is budgeted.
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

- [ ] **Step 6: Run** — `cargo fleet-test -- project_pick` → the 2 new tests plus Task 1's pass.
- [ ] **Step 7: Regen + suite** — `REGEN_DOCS=1 cargo fleet-test -- reference_is_current` (meant to fail once — re-run without the variable), then `scripts/verify.sh` → all pass. A test enumerating tools/policies will name what it needs; add exactly that, never loosen an assertion. `tests_isolation.rs` covers `work*` actions only.
- [ ] **Step 8: fmt + clippy**, then commit:

```bash
git add crates/fleet-core/src/service/project_picks.rs crates/fleet-core/src/service/mod.rs crates/fleet-core/src/mcp/tools/repo.rs crates/fleet-core/src/mcp/guard.rs crates/fleet-core/src/mcp/tools/tests.rs docs/control-api-reference.md
git commit -m "feat(picker): project_picks / set_project_pick tools, a person's device only"
```

---

### Task 3: Desktop commands, hub routing, verdicts

**Files:**
- Modify: `src-tauri/src/commands/projects.rs`, `src-tauri/src/lib.rs` (after `commands::projects::list_projects,`)
- Modify: `src-tauri/src/backend/verdicts.rs` (projects block, after `list_github_repos`)
- Modify: `src-tauri/src/backend/tests_routing.rs` (`routed_read_cases()` after the `list_projects` case; `routed_mutation_cases()` after `add_project`)
- Regen: `src/lib/hub_verdicts.generated.json`, `docs/hub.md`, `docs/control-api-reference.md`, any golden a test names

**Interfaces:** produces Tauri commands `project_picks` → `Vec<ProjectPickRow>`, `set_project_pick { args: SetProjectPickArgs }` → `ProjectPickRow`.

- [ ] **Step 1: Failing routing cases** — read case:

```rust
        (
            "project_picks",
            "project_picks",
            json!({}),
            r#"[{"owner":"o","repo":"r","pinned":true,"vis":"keep","grp":"tools"}]"#,
            Box::new(|b, s, _| {
                let v = block_on(commands::projects::routed::project_picks(b, s))?;
                assert!(v[0].pinned, "the hub's answer");
                assert_eq!(v[0].grp.as_deref(), Some("tools"));
                Ok(())
            }),
        ),
```

Mutation case (every field non-default, so the whole struct is proven to cross the wire):

```rust
        (
            "set_project_pick",
            "set_project_pick",
            json!({ "owner": "o", "repo": "r", "pinned": true, "vis": "hide", "grp": "tools" }),
            r#"{"owner":"o","repo":"r","pinned":true,"vis":"hide","grp":"tools"}"#,
            Box::new(|b, s, _| {
                block_on(commands::projects::routed::set_project_pick(
                    b,
                    s,
                    fleet_core::service::project_picks::SetProjectPickArgs {
                        owner: "o".into(),
                        repo: "r".into(),
                        pinned: true,
                        vis: Some("hide".into()),
                        grp: Some("tools".into()),
                    },
                ))
                .map(|_| ())
            }),
        ),
```

Match the neighbouring cases' exact tuple shape if it differs.

- [ ] **Step 2: Verify failure** — `cargo fleet-test -- backend::tests_routing` → compile error.

- [ ] **Step 3: Commands** — imports in `commands/projects.rs`:

```rust
use fleet_core::service::project_picks::{self, SetProjectPickArgs};
use fleet_core::store::ProjectPickRow;
```

After `list_github_repos`:

```rust
/// The New session picker's choices per project (pinned, hide/keep, group).
#[tauri::command]
pub async fn project_picks(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<ProjectPickRow>, IpcError> {
    routed::project_picks(&backend, &store).await
}

/// Replace one project's picker choices (full replace). Returns the row.
#[tauri::command]
pub async fn set_project_pick(
    args: SetProjectPickArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<ProjectPickRow, IpcError> {
    routed::set_project_pick(&backend, &store, args).await
}
```

In `mod routed`:

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

Update the module doc ("all four commands route…" → "all six"). Register in `lib.rs`:

```rust
            commands::projects::project_picks,
            commands::projects::set_project_pick,
```

- [ ] **Step 4: Verdicts** — after `list_github_repos`:

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

- [ ] **Step 5: Regen + suite** — `REGEN_HUB_VERDICTS=1 cargo fleet-test -- verdict_gen`; `REGEN_DOCS=1 cargo fleet-test -- reference_is_current`; `scripts/verify.sh` → all pass. If `tests_contract` fails on `hub_contract.golden.json`, run once with `REGEN_HUB_CONTRACT=1` (that run still says FAILED), re-run without it, and check the diff only adds the two tools. Leave `CONTRACT_REVISION` alone.
- [ ] **Step 6: fmt + clippy (workspace)**, then commit:

```bash
git add src-tauri/src/commands/projects.rs src-tauri/src/lib.rs src-tauri/src/backend/verdicts.rs src-tauri/src/backend/tests_routing.rs src/lib/hub_verdicts.generated.json docs/hub.md docs/control-api-reference.md src-tauri/src/backend/*.golden.json
git commit -m "feat(picker): project_picks / set_project_pick commands, routed on a hub"
```

---

### Task 4: Frontend picks store

**Files:**
- Create: `src/lib/project_picks.ts`, `src/lib/project_picks.test.ts`
- Modify: `src/App.svelte` (after the startup `Promise.all([loadProjects(), …])` ~line 309)

**Interfaces — produces:**
- `type Vis = 'hide' | 'keep'`
- `interface ProjectPick { owner: string; repo: string; pinned: boolean; vis: Vis | null; grp: string | null }`
- `pickKey(owner, repo): string` → `"owner/repo"`
- `projectPicks: Writable<ReadonlyMap<string, ProjectPick>>`
- `loadProjectPicks(): Promise<Result<ProjectPick[]>>`
- `setProjectPick(owner, repo, patch: { pinned?: boolean; vis?: Vis | null; grp?: string | null }): Promise<Result<ProjectPick>>` — optimistic; rolls back + `pushError` on failure
- `previousPick(owner, repo): ProjectPick | undefined` — the state before the last `setProjectPick` on that key (Undo)

- [ ] **Step 1: Failing test** — `project_picks.test.ts`:

```ts
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { loadProjectPicks, pickKey, previousPick, projectPicks, setProjectPick } from './project_picks';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;
const row = (over = {}) => ({ owner: 'o', repo: 'r', pinned: false, vis: null, grp: null, ...over });

beforeEach(() => {
  inv.mockReset();
  projectPicks.set(new Map());
});

describe('project_picks', () => {
  it('loads into a map keyed owner/repo', async () => {
    inv.mockResolvedValue([row({ pinned: true })]);
    await loadProjectPicks();
    expect(get(projectPicks).get(pickKey('o', 'r'))?.pinned).toBe(true);
  });

  it('an older hub (an error or null) leaves the map as it was', async () => {
    inv.mockResolvedValue(null);
    await loadProjectPicks();
    expect(get(projectPicks).size).toBe(0);
    inv.mockRejectedValue({ code: 'E_HUB', message: 'unknown tool' });
    expect((await loadProjectPicks()).ok).toBe(false);
    expect(get(projectPicks).size).toBe(0);
  });

  it('set is a full replace, optimistic, and remembers the previous state', async () => {
    projectPicks.set(new Map([[pickKey('o', 'r'), row({ grp: 'tools' })]]));
    let resolve!: (v: unknown) => void;
    inv.mockReturnValue(new Promise((r) => (resolve = r)));
    const p = setProjectPick('o', 'r', { pinned: true });
    expect(get(projectPicks).get('o/r')?.pinned).toBe(true); // before the answer
    expect(inv).toHaveBeenCalledWith('set_project_pick', {
      args: { owner: 'o', repo: 'r', pinned: true, vis: null, grp: 'tools' },
    });
    resolve(row({ pinned: true, grp: 'tools' }));
    await p;
    expect(previousPick('o', 'r')?.pinned).toBe(false);
  });

  it('a failed set rolls back', async () => {
    projectPicks.set(new Map([[pickKey('o', 'r'), row()]]));
    inv.mockRejectedValue({ code: 'E_HUB', message: 'down' });
    const r = await setProjectPick('o', 'r', { vis: 'hide' });
    expect(r.ok).toBe(false);
    expect(get(projectPicks).get('o/r')?.vis).toBe(null);
  });
});
```

- [ ] **Step 2: Verify failure** — `npx vitest run src/lib/project_picks.test.ts` → cannot resolve module.

- [ ] **Step 3: Implement** — `project_picks.ts`:

```ts
// The New session picker's per-project choices (project picker spec v2):
// pinned, visibility (hide | keep) and the picker group. Keyed by
// `owner/repo`, never the project id — the backend re-creates project rows
// (review C22). Loaded at startup and whenever the switcher opens; writes
// patch this store optimistically.
import { get, writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';
import { pushError } from './toasts';

export type Vis = 'hide' | 'keep';

export interface ProjectPick {
  owner: string;
  repo: string;
  pinned: boolean;
  vis: Vis | null;
  grp: string | null;
}

export const pickKey = (owner: string, repo: string): string => `${owner}/${repo}`;

export const projectPicks = writable<ReadonlyMap<string, ProjectPick>>(new Map());

const previous = new Map<string, ProjectPick>();

/** The state before the last `setProjectPick` on this project (Undo). */
export function previousPick(owner: string, repo: string): ProjectPick | undefined {
  return previous.get(pickKey(owner, repo));
}

export async function loadProjectPicks(): Promise<Result<ProjectPick[]>> {
  const r = await invokeCmd<ProjectPick[]>('project_picks');
  // A hub older than this feature has no such tool: the picker then runs on
  // the rules alone, nothing pinned.
  if (r.ok && Array.isArray(r.value)) {
    projectPicks.set(new Map(r.value.map((p) => [pickKey(p.owner, p.repo), p])));
  }
  return r;
}

const EMPTY = (owner: string, repo: string): ProjectPick => ({ owner, repo, pinned: false, vis: null, grp: null });

/** Change some of one project's choices. The command is a full replace, so
 *  the fields not in `patch` are sent as they are now. Optimistic: the store
 *  changes at once and rolls back (with a toast) if the write fails. */
export async function setProjectPick(
  owner: string,
  repo: string,
  patch: { pinned?: boolean; vis?: Vis | null; grp?: string | null },
): Promise<Result<ProjectPick>> {
  const key = pickKey(owner, repo);
  const before = get(projectPicks).get(key) ?? EMPTY(owner, repo);
  const next: ProjectPick = {
    owner,
    repo,
    pinned: patch.pinned ?? before.pinned,
    vis: patch.vis !== undefined ? patch.vis : before.vis,
    grp: patch.grp !== undefined ? patch.grp : before.grp,
  };
  previous.set(key, before);
  projectPicks.update((m) => new Map(m).set(key, next));
  const r = await invokeCmd<ProjectPick>('set_project_pick', {
    args: { owner, repo, pinned: next.pinned, vis: next.vis, grp: next.grp },
  });
  if (r.ok && r.value) {
    const saved = r.value;
    projectPicks.update((m) => new Map(m).set(key, saved));
  } else if (!r.ok) {
    projectPicks.update((m) => new Map(m).set(key, before));
    pushError(r.error, 'Could not save the project choice');
  }
  return r;
}
```

- [ ] **Step 4: Load at startup** — in `App.svelte`, import `loadProjectPicks` (and `hubConnection` if not imported) and add `void loadProjectPicks();` right after the startup `Promise.all([...])` resolves (not inside it — a failure here must not count as a startup failure). Then re-load when the hub connection changes:

```ts
  // The picker's choices live on the hub when paired: re-read them when the
  // connection comes (back).
  let lastHubState: string | null = null;
  $effect(() => {
    const st = $hubConnection.state;
    if (lastHubState !== null && st !== lastHubState) void loadProjectPicks();
    lastHubState = st;
  });
```

- [ ] **Step 5: Run** — `npx vitest run src/lib/project_picks.test.ts` → `4 passed`; `npx vitest run src/App.test.ts` → still passing (add `if (cmd === 'project_picks') return [];` to its invoke mock if it rejects unknown commands).
- [ ] **Step 6: Commit**

```bash
git add src/lib/project_picks.ts src/lib/project_picks.test.ts src/App.svelte
git commit -m "feat(picker): the project picks store, optimistic, loaded at startup"
```

---

### Task 5: Frecency — `frecency.ts`

**Files:** Create `src/lib/frecency.ts`, `src/lib/frecency.test.ts`

**Interfaces — produces:**
- `FRECENCY_KEY = 'newsession.frecency'`, `HALF_LIFE_DAYS = 7`, `FRECENCY_CAP = 200`
- `type FrecencyMap = Record<string, { score: number; at: number }>`
- `decayed(entry: { score: number; at: number } | undefined, now: number): number`
- `recordPick(key: string, now?: number): void`
- `readFrecency(): FrecencyMap`
- `recencyTerm(lastSessionAt: number | null, now: number): number` → `20 · 2^(−days/7)`, 0 for null

- [ ] **Step 1: Failing test**:

```ts
import { describe, it, expect, beforeEach } from 'vitest';
import { decayed, FRECENCY_CAP, readFrecency, recencyTerm, recordPick } from './frecency';

const NOW = 1_800_000_000;
const DAY = 86_400;

beforeEach(() => localStorage.clear());

describe('frecency', () => {
  it('halves every 7 days', () => {
    expect(decayed({ score: 4, at: NOW - 7 * DAY }, NOW)).toBeCloseTo(2);
    expect(decayed(undefined, NOW)).toBe(0);
  });
  it('a pick decays the old score, then adds one', () => {
    recordPick('o/a', NOW - 7 * DAY);
    recordPick('o/a', NOW);
    expect(readFrecency()['o/a'].score).toBeCloseTo(1.5);
  });
  it('keeps at most FRECENCY_CAP keys, dropping the weakest', () => {
    for (let i = 0; i < FRECENCY_CAP + 5; i++) recordPick(`o/r${i}`, NOW - (FRECENCY_CAP + 5 - i) * DAY);
    const m = readFrecency();
    expect(Object.keys(m)).toHaveLength(FRECENCY_CAP);
    expect(m['o/r0']).toBeUndefined();
  });
  it('recency from last_session_at', () => {
    expect(recencyTerm(NOW, NOW)).toBeCloseTo(20);
    expect(recencyTerm(NOW - 7 * DAY, NOW)).toBeCloseTo(10);
    expect(recencyTerm(null, NOW)).toBe(0);
  });
});
```

- [ ] **Step 2: Verify failure** — `npx vitest run src/lib/frecency.test.ts`.

- [ ] **Step 3: Implement**:

```ts
// What the person picks in the New session picker, decayed (project picker
// spec v2, D3): per device, like Raycast / Alfred. Not a server table —
// sessions started by agents or outside fleet say nothing about choice.
import { readPref, writePref } from './prefs';

export const FRECENCY_KEY = 'newsession.frecency';
export const HALF_LIFE_DAYS = 7;
export const FRECENCY_CAP = 200;
const DAY = 86_400;

export type FrecencyMap = Record<string, { score: number; at: number }>;

const isMap = (v: unknown): v is FrecencyMap =>
  typeof v === 'object' &&
  v !== null &&
  Object.values(v as Record<string, unknown>).every(
    (e) =>
      typeof e === 'object' &&
      e !== null &&
      typeof (e as { score: unknown }).score === 'number' &&
      typeof (e as { at: unknown }).at === 'number',
  );

export function readFrecency(): FrecencyMap {
  return readPref<FrecencyMap>(FRECENCY_KEY, {}, isMap);
}

export function decayed(entry: { score: number; at: number } | undefined, now: number): number {
  if (!entry) return 0;
  const days = Math.max(0, (now - entry.at) / DAY);
  return entry.score * Math.pow(2, -days / HALF_LIFE_DAYS);
}

export function recordPick(key: string, now: number = Math.floor(Date.now() / 1000)): void {
  const m = { ...readFrecency() };
  m[key] = { score: decayed(m[key], now) + 1, at: now };
  const keys = Object.keys(m);
  if (keys.length > FRECENCY_CAP) {
    keys
      .sort((a, b) => decayed(m[a], now) - decayed(m[b], now))
      .slice(0, keys.length - FRECENCY_CAP)
      .forEach((k) => delete m[k]);
  }
  writePref(FRECENCY_KEY, m);
}

export function recencyTerm(lastSessionAt: number | null, now: number): number {
  if (lastSessionAt == null) return 0;
  const days = Math.max(0, (now - lastSessionAt) / DAY);
  return 20 * Math.pow(2, -days / 7);
}
```

- [ ] **Step 4: Run** → `4 passed`. **Step 5: Commit**

```bash
git add src/lib/frecency.ts src/lib/frecency.test.ts
git commit -m "feat(picker): local frecency of the person's own picks"
```

---

### Task 6: The ranking — `project_rank.ts`

**Files:** Create `src/lib/project_rank.ts`, `src/lib/project_rank.test.ts`

**Interfaces:**
- Consumes: `ProjectTreeRow`, `SessionRow`, `ProjectPick` + `pickKey`, `fuzzyMatchFields`, `decayed` + `recencyTerm` + `FrecencyMap`.
- Produces:
  - constants `SUGGESTED_CAP = 7`, `CLUSTER_MIN = 3`, `OWNER_GROUP_MIN = 3`, `ACTIVE_DAYS = 30`, `DORMANT_DAYS = 90`, `FORKS = 'Forks & others'`
  - `type HiddenReason = 'hidden by you' | 'throwaway name'`
  - `hiddenReason(p: ProjectTreeRow, pick: ProjectPick | undefined, now: number): HiddenReason | null`
  - `isDormant(p: ProjectTreeRow, now: number): boolean`
  - `interface GroupInfo { key: string; name: string; sub: string; ownerOrder: number }` — `key` is `m:<name>` (person's), `c:<owner>:<prefix>` (cluster), `o:<owner>` (More from), `f` (Forks)
  - `groupProjects(rows: readonly ProjectTreeRow[], picks: ReadonlyMap<string, ProjectPick>): Map<number, GroupInfo>`
  - `interface Entry { project: ProjectTreeRow; id: number; key: string; label: string; pinned: boolean; hidden: HiddenReason | null; dormant: boolean; group: GroupInfo }`
  - `entriesOf(projects, picks, now): Entry[]` (non-system only)
  - `ago(lastSessionAt: number | null, now: number): string`
  - `interface Ctx { selectedProjectId: number | null; preferredHost: string | null; sessions: readonly Pick<SessionRow, 'project_id' | 'host_alias' | 'last_activity_at'>[] }`
  - `interface ViewRow { entry: Entry; chip: string; kbd: string; meta: string }`
  - `interface ViewSection { key: string; label: string; sub: string; foldable: boolean; openByDefault: boolean; rows: ViewRow[] }` — keys `pinned`, `suggested`, `g:<GroupInfo.key>`, `hidden`
  - `buildSections(entries: Entry[], ctx: Ctx, frecency: FrecencyMap, now: number): ViewSection[]`
  - `searchEntries(entries: Entry[], query: string, ctx: Ctx, frecency: FrecencyMap, now: number): ViewRow[]`

- [ ] **Step 1: Failing tests** — `project_rank.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import type { ProjectTreeRow } from './projects';
import { pickKey, type ProjectPick } from './project_picks';
import {
  buildSections, entriesOf, groupProjects, hiddenReason, isDormant, searchEntries, FORKS, type Ctx,
} from './project_rank';

const NOW = 1_800_000_000;
const DAY = 86_400;
let id = 1;
const proj = (owner: string, repo: string, daysAgo: number | null = 1, system = false): ProjectTreeRow => ({
  project: { id: id++, owner, repo, base_path: `/p/${repo}`, last_session_at: daysAgo === null ? null : NOW - daysAgo * DAY, adopted: false, system },
  worktrees: [],
});
const picks = (...ps: Array<Partial<ProjectPick> & { owner: string; repo: string }>) =>
  new Map(ps.map((p) => [pickKey(p.owner, p.repo), { pinned: false, vis: null, grp: null, ...p } as ProjectPick]));
const noCtx: Ctx = { selectedProjectId: null, preferredHost: null, sessions: [] };
const labels = (s: { rows: { entry: { label: string } }[] } | undefined) => s?.rows.map((r) => r.entry.label) ?? [];

describe('hidden and dormant', () => {
  it('no session record is NOT hidden; a throwaway name unused for 30 days is', () => {
    expect(hiddenReason(proj('o', 'openmarket-app', null), undefined, NOW)).toBeNull();
    expect(hiddenReason(proj('o', 'ppt-epic-145', null), undefined, NOW)).toBe('throwaway name');
    expect(hiddenReason(proj('o', 'test-x', 2), undefined, NOW)).toBeNull();
    expect(hiddenReason(proj('o', 'contest-app', null), undefined, NOW)).toBeNull();
  });
  it('hide wins; keep, pin and a person’s group protect a throwaway name', () => {
    const p = proj('o', 'tmp-x', null);
    expect(hiddenReason(proj('o', 'a'), { owner: 'o', repo: 'a', pinned: false, vis: 'hide', grp: null }, NOW)).toBe('hidden by you');
    for (const k of [{ vis: 'keep' as const }, { pinned: true }, { grp: 'g' }]) {
      expect(hiddenReason(p, { owner: 'o', repo: 'tmp-x', pinned: false, vis: null, grp: null, ...k }, NOW)).toBeNull();
    }
  });
  it('dormant: no record, or nothing for 90 days', () => {
    expect(isDormant(proj('o', 'a', null), NOW)).toBe(true);
    expect(isDormant(proj('o', 'a', 91), NOW)).toBe(true);
    expect(isDormant(proj('o', 'a', 10), NOW)).toBe(false);
  });
});

describe('groupProjects', () => {
  it('longest shared prefix of ≥3, single token joins, others fall to owner buckets', () => {
    const rows = [
      proj('F', 'sales-twins-app'), proj('F', 'sales-twins-mobile', null), proj('F', 'sales-twins-revonaut-fixes', null),
      proj('F', 'stw-fix2', null),
      proj('p', 'openmarket-ai'), proj('p', 'openmarket-docs'), proj('p', 'openmarket-app', null), proj('p', 'openmarket', null),
      proj('p', 'dwh', null),
      proj('x', 'gods-eye-view', null),
    ];
    const g = groupProjects(rows, new Map());
    const name = (r: ProjectTreeRow) => g.get(r.project.id)?.name;
    expect(rows.slice(0, 3).map(name)).toEqual(['sales-twins', 'sales-twins', 'sales-twins']);
    expect(name(rows[3])).toBe('More from F');
    expect(rows.slice(4, 8).map(name)).toEqual(['openmarket', 'openmarket', 'openmarket', 'openmarket']);
    expect(name(rows[8])).toBe('More from p');
    expect(name(rows[9])).toBe(FORKS);
    expect(g.get(rows[0].project.id)?.sub).toBe('sales-twins-* · F');
  });
  it('a person’s group wins; a hide never moves a neighbour (clusters use every project)', () => {
    const rows = [proj('o', 'ab-1'), proj('o', 'ab-2'), proj('o', 'ab-3', null), proj('o', 'zz')];
    const g = groupProjects(rows, picks({ owner: 'o', repo: 'ab-3', vis: 'hide' }, { owner: 'o', repo: 'zz', grp: 'Mine' }));
    expect(g.get(rows[0].project.id)?.name).toBe('ab');
    expect(g.get(rows[3].project.id)).toMatchObject({ name: 'Mine', sub: 'your group' });
  });
});

describe('buildSections', () => {
  it('Pinned, Suggested (context first, chips, ⌘ numbers), groups list every member, Hidden folded', () => {
    const fleet = proj('me', 'claude-fleet', 0.01);
    const backend = proj('p', 'papayapos-backend', 5);
    const docs = proj('p', 'openmarket-docs', 0.5);
    const om1 = proj('p', 'openmarket-ai', 23);
    const om2 = proj('p', 'openmarket-app', null);
    const epic = proj('me', 'ppt-epic-145', null);
    const sys = proj('fleet', 'operator', 0, true);
    const e = entriesOf([fleet, backend, docs, om1, om2, epic, sys], picks({ owner: 'me', repo: 'claude-fleet', pinned: true }), NOW);
    const ctx: Ctx = { selectedProjectId: backend.project.id, preferredHost: null, sessions: [] };
    const s = buildSections(e, ctx, {}, NOW);
    const by = (k: string) => s.find((x) => x.key === k);
    expect(labels(by('pinned'))).toEqual(['claude-fleet']);
    expect(by('pinned')?.rows[0].kbd).toBe('⌘1');
    expect(labels(by('suggested'))[0]).toBe('papayapos-backend');
    expect(by('suggested')?.rows[0]).toMatchObject({ chip: 'current session', kbd: '⌘2' });
    const om = s.find((x) => x.label === 'openmarket');
    expect(labels(om)).toEqual(['openmarket-ai', 'openmarket-docs', 'openmarket-app']); // every member; dormant last
    expect(om?.openByDefault).toBe(true);
    expect(by('hidden')).toMatchObject({ foldable: true, openByDefault: false });
    expect(labels(by('hidden'))).toEqual(['ppt-epic-145']);
    expect(s.flatMap((x) => x.rows).some((r) => r.entry.project.project.system)).toBe(false);
  });
  it('a group with no session in 30 days starts folded', () => {
    const e = entriesOf([proj('o', 'a-1', 40), proj('o', 'a-2', null), proj('o', 'a-3', null)], new Map(), NOW);
    expect(buildSections(e, noCtx, {}, NOW).find((x) => x.label === 'a')?.openByDefault).toBe(false);
  });
  it('preferred host: projects with a session there, chip "on host"', () => {
    const a = proj('o', 'alpha', 3);
    const b = proj('o', 'beta', 2);
    const ctx: Ctx = {
      selectedProjectId: null,
      preferredHost: 'mefistos',
      sessions: [
        { project_id: a.project.id, host_alias: 'mefistos', last_activity_at: NOW - 10 },
        { project_id: b.project.id, host_alias: 'mac', last_activity_at: NOW },
      ],
    };
    const s = buildSections(entriesOf([a, b], new Map(), NOW), ctx, {}, NOW);
    expect(s.find((x) => x.key === 'suggested')?.rows[0]).toMatchObject({ chip: 'on mefistos' });
  });
  it('frecency outranks plain recency in Suggested; at most 7', () => {
    const many = Array.from({ length: 10 }, (_, i) => proj('o', `r${i}`, i + 1));
    const s = buildSections(entriesOf(many, new Map(), NOW), noCtx, { 'o/r9': { score: 5, at: NOW } }, NOW);
    const sugg = labels(s.find((x) => x.key === 'suggested'));
    expect(sugg[0]).toBe('r9');
    expect(sugg).toHaveLength(7);
  });
});

describe('searchEntries', () => {
  it('hidden is found but ranked last and tagged', () => {
    const used = proj('o', 'shop-api', 1);
    const hidden = proj('o', 'shop-app', 1);
    const e = entriesOf([hidden, used], picks({ owner: 'o', repo: 'shop-app', vis: 'hide' }), NOW);
    const r = searchEntries(e, 'shop', noCtx, {}, NOW);
    expect(r.map((x) => x.entry.label)).toEqual(['shop-api', 'shop-app']);
    expect(r[1].meta).toBe('hidden · hidden by you');
  });
  it('matches the group name too', () => {
    const e = entriesOf([proj('p', 'openmarket-ai'), proj('p', 'openmarket-docs'), proj('p', 'openmarket-app'), proj('p', 'zzz')], new Map(), NOW);
    expect(searchEntries(e, 'openm', noCtx, {}, NOW).map((x) => x.entry.label).sort()).toEqual(['openmarket-ai', 'openmarket-app', 'openmarket-docs']);
  });
});
```

- [ ] **Step 2: Verify failure** — `npx vitest run src/lib/project_rank.test.ts`.

- [ ] **Step 3: Implement** — `project_rank.ts`:

```ts
// The New session picker's ranking (project picker spec v2): pure functions
// from projects + picks + context + frecency to the switcher's sections.
// Pinned → Suggested (≤7) → every project in its group (dormant dimmed,
// last) → Hidden. With a query: one fuzzy list, hidden kept but last.
import { fuzzyMatchFields } from './fuzzy';
import type { ProjectTreeRow } from './projects';
import type { SessionRow } from './sessions';
import { pickKey, type ProjectPick } from './project_picks';
import { decayed, recencyTerm, type FrecencyMap } from './frecency';

export const SUGGESTED_CAP = 7;
export const CLUSTER_MIN = 3;
export const OWNER_GROUP_MIN = 3;
export const ACTIVE_DAYS = 30;
export const DORMANT_DAYS = 90;
export const FORKS = 'Forks & others';
const DAY = 86_400;
const THROWAWAY = [/^(test|tmp|example)-/i, /-analysis$/i, /-epic-\d+$/i];

export type HiddenReason = 'hidden by you' | 'throwaway name';

const daysSince = (at: number | null, now: number) => (at == null ? Infinity : (now - at) / DAY);

export function hiddenReason(p: ProjectTreeRow, pick: ProjectPick | undefined, now: number): HiddenReason | null {
  if (pick?.vis === 'hide') return 'hidden by you';
  if (pick?.vis === 'keep' || pick?.pinned || pick?.grp) return null;
  if (daysSince(p.project.last_session_at, now) <= ACTIVE_DAYS) return null;
  return THROWAWAY.some((re) => re.test(p.project.repo)) ? 'throwaway name' : null;
}

export function isDormant(p: ProjectTreeRow, now: number): boolean {
  return daysSince(p.project.last_session_at, now) > DORMANT_DAYS;
}

export interface GroupInfo {
  key: string;
  name: string;
  sub: string;
  /** Sort rank: person's groups 0, then owners by recency, forks last. */
  ownerOrder: number;
}

export function groupProjects(
  rows: readonly ProjectTreeRow[],
  picks: ReadonlyMap<string, ProjectPick>,
): Map<number, GroupInfo> {
  const byOwner = new Map<string, ProjectTreeRow[]>();
  for (const r of rows) byOwner.set(r.project.owner, [...(byOwner.get(r.project.owner) ?? []), r]);
  const owners = [...byOwner.entries()]
    .map(([owner, list]) => ({ owner, list, last: Math.max(0, ...list.map((r) => r.project.last_session_at ?? 0)) }))
    .sort((a, b) => b.last - a.last || a.owner.localeCompare(b.owner));
  const out = new Map<number, GroupInfo>();
  owners.forEach(({ owner, list }, i) => {
    const toks = (r: ProjectTreeRow) => r.project.repo.toLowerCase().split('-').filter(Boolean);
    const count = new Map<string, number>();
    for (const r of list) {
      const t = toks(r);
      for (let n = 1; n < t.length; n++) {
        const pre = t.slice(0, n).join('-');
        count.set(pre, (count.get(pre) ?? 0) + 1);
      }
    }
    for (const r of list) {
      const manual = picks.get(pickKey(owner, r.project.repo))?.grp;
      if (manual) {
        out.set(r.project.id, { key: `m:${manual}`, name: manual, sub: 'your group', ownerOrder: 0 });
        continue;
      }
      const t = toks(r);
      let best: string | null = null;
      for (let n = t.length - 1; n >= 1; n--) {
        const pre = t.slice(0, n).join('-');
        if ((count.get(pre) ?? 0) >= CLUSTER_MIN) {
          best = pre;
          break;
        }
      }
      if (!best && t.length === 1 && (count.get(t[0]) ?? 0) >= CLUSTER_MIN) best = t[0];
      if (best) {
        out.set(r.project.id, { key: `c:${owner}:${best}`, name: best, sub: `${best}-* · ${owner}`, ownerOrder: 1 + i });
      } else if (list.length >= OWNER_GROUP_MIN) {
        out.set(r.project.id, { key: `o:${owner}`, name: `More from ${owner}`, sub: '', ownerOrder: 1 + i });
      } else {
        out.set(r.project.id, { key: 'f', name: FORKS, sub: '', ownerOrder: 10_000 });
      }
    }
  });
  return out;
}

export interface Entry {
  project: ProjectTreeRow;
  id: number;
  key: string;
  label: string;
  pinned: boolean;
  hidden: HiddenReason | null;
  dormant: boolean;
  group: GroupInfo;
}

export function entriesOf(
  projects: readonly ProjectTreeRow[],
  picks: ReadonlyMap<string, ProjectPick>,
  now: number,
): Entry[] {
  const visible = projects.filter((p) => !p.project.system);
  const groups = groupProjects(visible, picks);
  const repoCount = new Map<string, number>();
  for (const p of visible) repoCount.set(p.project.repo, (repoCount.get(p.project.repo) ?? 0) + 1);
  return visible.map((p) => {
    const key = pickKey(p.project.owner, p.project.repo);
    const pk = picks.get(key);
    return {
      project: p,
      id: p.project.id,
      key,
      label: (repoCount.get(p.project.repo) ?? 0) > 1 ? key : p.project.repo,
      pinned: !!pk?.pinned,
      hidden: hiddenReason(p, pk, now),
      dormant: isDormant(p, now),
      group: groups.get(p.project.id)!,
    };
  });
}

export function ago(lastSessionAt: number | null, now: number): string {
  if (lastSessionAt == null) return 'no sessions yet';
  const s = Math.max(0, now - lastSessionAt);
  if (s < 3600) return `${Math.max(1, Math.round(s / 60))}m`;
  if (s < DAY) return `${Math.round(s / 3600)}h`;
  if (s < 60 * DAY) return `${Math.round(s / DAY)}d`;
  return `${Math.round(s / (30 * DAY))}mo`;
}

export interface Ctx {
  selectedProjectId: number | null;
  preferredHost: string | null;
  sessions: readonly Pick<SessionRow, 'project_id' | 'host_alias' | 'last_activity_at'>[];
}

export interface ViewRow {
  entry: Entry;
  chip: string;
  kbd: string;
  meta: string;
}

export interface ViewSection {
  key: string;
  label: string;
  sub: string;
  foldable: boolean;
  openByDefault: boolean;
  rows: ViewRow[];
}

const byLabel = (a: Entry, b: Entry) => a.label.localeCompare(b.label, undefined, { sensitivity: 'base' });
const score = (e: Entry, f: FrecencyMap, now: number) =>
  decayed(f[e.key], now) * 10 + recencyTerm(e.project.project.last_session_at, now);

export function buildSections(entries: Entry[], ctx: Ctx, frecency: FrecencyMap, now: number): ViewSection[] {
  const out: ViewSection[] = [];
  let n = 0;
  const num = () => (n < 9 ? `⌘${++n}` : '');
  const meta = (e: Entry) => ago(e.project.project.last_session_at, now);

  const pinned = entries.filter((e) => e.pinned).sort(byLabel);
  if (pinned.length) {
    out.push({
      key: 'pinned', label: 'Pinned', sub: '', foldable: false, openByDefault: true,
      rows: pinned.map((e) => ({ entry: e, chip: '', kbd: num(), meta: meta(e) })),
    });
  }

  const taken = new Set<number>();
  const sugg: ViewRow[] = [];
  const add = (e: Entry | undefined, chip: string) => {
    if (!e || e.pinned || e.hidden || taken.has(e.id) || sugg.length >= SUGGESTED_CAP) return;
    taken.add(e.id);
    sugg.push({ entry: e, chip, kbd: '', meta: meta(e) });
  };
  const byId = new Map(entries.map((e) => [e.id, e]));
  if (ctx.selectedProjectId != null) add(byId.get(ctx.selectedProjectId), 'current session');
  if (ctx.preferredHost) {
    [...ctx.sessions]
      .filter((s) => s.host_alias === ctx.preferredHost && s.project_id != null)
      .sort((a, b) => (b.last_activity_at ?? 0) - (a.last_activity_at ?? 0))
      .forEach((s) => add(byId.get(s.project_id!), `on ${ctx.preferredHost}`));
  }
  entries
    .filter((e) => score(e, frecency, now) > 0)
    .sort((a, b) => score(b, frecency, now) - score(a, frecency, now) || byLabel(a, b))
    .forEach((e) => add(e, ''));
  if (sugg.length) {
    sugg.forEach((r) => (r.kbd = num()));
    out.push({ key: 'suggested', label: 'Suggested', sub: 'what you open often, and lately', foldable: false, openByDefault: true, rows: sugg });
  }

  const groups = new Map<string, { info: GroupInfo; members: Entry[] }>();
  for (const e of entries) {
    if (e.hidden) continue;
    const g = groups.get(e.group.key) ?? { info: e.group, members: [] };
    g.members.push(e);
    groups.set(e.group.key, g);
  }
  [...groups.values()]
    .sort(
      (a, b) =>
        a.info.ownerOrder - b.info.ownerOrder ||
        Number(a.info.key.startsWith('o:')) - Number(b.info.key.startsWith('o:')) ||
        a.info.name.localeCompare(b.info.name, undefined, { sensitivity: 'base' }),
    )
    .forEach(({ info, members }) => {
      const live = members.filter((e) => !e.dormant).sort(byLabel);
      const dormant = members
        .filter((e) => e.dormant)
        .sort(
          (a, b) =>
            Number(a.project.project.last_session_at == null) - Number(b.project.project.last_session_at == null) ||
            byLabel(a, b),
        );
      const active = members.some((e) => daysSince(e.project.project.last_session_at, now) <= ACTIVE_DAYS);
      out.push({
        key: `g:${info.key}`,
        label: info.name,
        sub: info.sub || `${members.length} projects`,
        foldable: true,
        openByDefault: active,
        rows: [...live, ...dormant].map((e) => ({ entry: e, chip: '', kbd: '', meta: meta(e) })),
      });
    });

  const hidden = entries.filter((e) => e.hidden).sort(byLabel);
  if (hidden.length) {
    out.push({
      key: 'hidden', label: 'Hidden', sub: 'throwaway names and what you hid · search still finds them',
      foldable: true, openByDefault: false,
      rows: hidden.map((e) => ({ entry: e, chip: '', kbd: '', meta: e.hidden! })),
    });
  }
  return out;
}

export function searchEntries(entries: Entry[], query: string, ctx: Ctx, frecency: FrecencyMap, now: number): ViewRow[] {
  const q = query.trim();
  if (!q) return [];
  return entries
    .map((e) => ({ e, s: fuzzyMatchFields(q, [e.key, e.project.project.repo, e.group.name]) }))
    .filter((x): x is { e: Entry; s: number } => x.s !== null)
    .map(({ e, s }) => {
      let total = s;
      if (e.pinned) total += 40;
      total += Math.min(60, score(e, frecency, now));
      if (ctx.selectedProjectId === e.id) total += 80;
      if (e.hidden) total -= 400;
      return { e, total };
    })
    .sort((a, b) => b.total - a.total || byLabel(a.e, b.e))
    .map(({ e }) => ({
      entry: e,
      chip: ctx.selectedProjectId === e.id ? 'current session' : '',
      kbd: '',
      meta: e.hidden ? `hidden · ${e.hidden}` : ago(e.project.project.last_session_at, now),
    }));
}
```

- [ ] **Step 4: Run** → all pass (12 tests). If a test and the code disagree, fix whichever contradicts the **spec**.
- [ ] **Step 5: Commit**

```bash
git add src/lib/project_rank.ts src/lib/project_rank.test.ts
git commit -m "feat(picker): the ranking — hidden, dormant, clusters, Suggested, search"
```

---

### Task 7: `PickerList` extensions

**Files:** Modify `src/lib/PickerList.svelte`; Create/extend `src/lib/PickerList.test.ts`

**Interfaces — produces** (additive; the switcher and the dialog's worktree picker keep working):
- `PickerItem` gains: `dim?: boolean`, `chip?: string`, `kbd?: string`, `groupKey?: string`, `groupSub?: string`, `actionable?: boolean`
- props gain: `rowActions?: Snippet<[PickerItem]>` (rendered for `actionable` items, inside `span.acts[aria-hidden=true]`), `ongroupclick?: (groupKey: string) => void`, `oncontext?: (key: string, e: MouseEvent) => void`

- [ ] **Step 1: Failing test**:

```ts
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi } from 'vitest';
import { createRawSnippet } from 'svelte';
import PickerList from './PickerList.svelte';

const acts = createRawSnippet(() => ({ render: () => '<button type="button" tabindex="-1" data-testid="act">x</button>' }));

describe('PickerList extensions', () => {
  it('renders chip, kbd, dim, a group subtitle, and mouse-only actions', async () => {
    const ongroupclick = vi.fn();
    const oncontext = vi.fn();
    render(PickerList, {
      props: {
        items: [
          { key: 'a', label: 'alpha', group: 'Pinned', groupKey: 'pinned', groupSub: 'yours', chip: 'current session', kbd: '⌘1', actionable: true },
          { key: 'b', label: 'beta', group: 'Pinned', groupKey: 'pinned', dim: true },
        ],
        onpick: () => {},
        rowActions: acts,
        ongroupclick,
        oncontext,
        listId: 'l',
      },
    });
    expect(screen.getByText('current session')).toBeTruthy();
    expect(screen.getByText('⌘1')).toBeTruthy();
    expect(screen.getByText('yours')).toBeTruthy();
    expect(document.querySelector('[data-key="b"]')?.classList.contains('dim')).toBe(true);
    expect(screen.getByTestId('act').closest('[aria-hidden="true"]')).not.toBeNull();
    expect(screen.getAllByTestId('act')).toHaveLength(1); // only the actionable row
    await fireEvent.click(screen.getByText('Pinned'));
    expect(ongroupclick).toHaveBeenCalledWith('pinned');
    await fireEvent.contextMenu(document.querySelector('[data-key="a"]')!);
    expect(oncontext).toHaveBeenCalledWith('a', expect.anything());
  });
});
```

- [ ] **Step 2: Verify failure** — `npx vitest run src/lib/PickerList.test.ts`.

- [ ] **Step 3: Implement** — extend `PickerItem` in the module script:

```ts
    /** Dimmed (a dormant project): still pickable. */
    dim?: boolean;
    /** A small pill before the meta ("current session", "on mefistos"). */
    chip?: string;
    /** A shortcut hint after the meta ("⌘1"). */
    kbd?: string;
    /** Identity of the group heading, reported by `ongroupclick`. */
    groupKey?: string;
    /** A muted subtitle after the group heading. */
    groupSub?: string;
    /** Render `rowActions` on this row (hover; mouse only). */
    actionable?: boolean;
```

Add `import type { Snippet } from 'svelte';` and the props `rowActions?: Snippet<[PickerItem]>`, `ongroupclick?: (groupKey: string) => void`, `oncontext?: (key: string, e: MouseEvent) => void` (destructured, documented). Markup:

```svelte
    {#if item.group && (i === 0 || items[i - 1].group !== item.group)}
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <div
        class="group"
        class:clickable={!!ongroupclick && !!item.groupKey}
        role="presentation"
        onclick={() => item.groupKey && ongroupclick?.(item.groupKey)}
      >{item.group}{#if item.groupSub}<span class="gsub">{item.groupSub}</span>{/if}</div>
    {/if}
    <div
      class="row"
      class:active={item.key === activeKey}
      class:dim={item.dim}
      …the existing attributes and handlers, unchanged…
      oncontextmenu={(e) => {
        if (!oncontext) return;
        e.preventDefault();
        oncontext(item.key, e);
      }}
    >
      <div class="main">…unchanged…</div>
      {#if item.chip}<span class="chip">{item.chip}</span>{/if}
      {#if item.meta || item.kbd}
        <span class="meta">{item.meta ?? ''}{#if item.kbd}<kbd class="kbd">{item.kbd}</kbd>{/if}</span>
      {/if}
      {#if item.actionable && rowActions}
        <span class="acts" aria-hidden="true">{@render rowActions(item)}</span>
      {/if}
    </div>
```

Styles to add:

```css
  .group.clickable { cursor: pointer; }
  .group.clickable:hover { color: var(--fg); }
  .gsub { margin-left: 0.5rem; text-transform: none; letter-spacing: 0; }
  .row { position: relative; }
  .row.dim .label { color: var(--fg-muted); }
  .chip {
    flex-shrink: 0;
    font-size: 0.7rem;
    padding: 0.05rem 0.45rem;
    border-radius: var(--radius-pill);
    background: var(--accent-soft);
    color: var(--accent);
  }
  .kbd {
    margin-left: 0.4rem;
    font: inherit;
    font-size: 0.65rem;
    padding: 0 0.25rem;
    border: 1px solid var(--border);
    border-radius: 3px;
  }
  .acts {
    display: none;
    position: absolute;
    right: 0.4rem;
    top: 50%;
    transform: translateY(-50%);
    gap: 2px;
    padding: 2px;
    border-radius: var(--radius-md);
    background: var(--bg-pane);
    border: 1px solid var(--border);
  }
  .row:hover .acts { display: flex; }
  .row:hover .meta { visibility: hidden; }
```

- [ ] **Step 4: Run** `npx vitest run src/lib/PickerList.test.ts src/lib/QuickSwitcher.test.ts src/lib/NewSessionDialog.test.ts` → all pass (existing users unaffected).
- [ ] **Step 5: Commit**

```bash
git add src/lib/PickerList.svelte src/lib/PickerList.test.ts
git commit -m "feat(picker): PickerList — chip, kbd, dim rows, group subtitle, mouse-only row actions"
```

---

### Task 8: The actions menu — `ProjectActionsMenu.svelte`

**Files:** Create `src/lib/ProjectActionsMenu.svelte`, `src/lib/ProjectActionsMenu.test.ts`

**Interfaces — produces:** props `{ title: string; pinned: boolean; hidden: boolean; groups: readonly string[]; currentGroup: string | null; manualGroup: boolean; startIn: 'main' | 'groups'; onpin: () => void; onhide: () => void; ongroup: (name: string | null) => void; onclose: () => void }` — `ongroup(null)` = back to automatic. A `role="menu"`; focus moves in (first item, or the group input in `groups` mode); Esc calls `onclose` and stops propagation; ↑/↓ move between items.

- [ ] **Step 1: Failing test**:

```ts
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi } from 'vitest';
import { tick } from 'svelte';
import ProjectActionsMenu from './ProjectActionsMenu.svelte';

const base = { title: 'o/kuk-agent', pinned: false, hidden: false, groups: ['claude', 'openmarket', 'sales-twins'], currentGroup: null, manualGroup: false };

describe('ProjectActionsMenu', () => {
  it('main: Pin, Move to group…, Hide; Esc closes', async () => {
    const p = { ...base, startIn: 'main' as const, onpin: vi.fn(), onhide: vi.fn(), ongroup: vi.fn(), onclose: vi.fn() };
    render(ProjectActionsMenu, { props: p });
    await tick();
    expect(document.activeElement?.textContent).toContain('Pin to top');
    await fireEvent.click(screen.getByRole('menuitem', { name: /Hide/ }));
    expect(p.onhide).toHaveBeenCalled();
    await fireEvent.keyDown(screen.getByRole('menu'), { key: 'Escape' });
    expect(p.onclose).toHaveBeenCalled();
  });

  it('groups: filter existing, create new, back to automatic', async () => {
    const p = { ...base, manualGroup: true, currentGroup: 'claude', startIn: 'groups' as const, onpin: vi.fn(), onhide: vi.fn(), ongroup: vi.fn(), onclose: vi.fn() };
    render(ProjectActionsMenu, { props: p });
    const input = screen.getByLabelText('Group name');
    await fireEvent.input(input, { target: { value: 'open' } });
    await tick();
    expect(screen.getAllByRole('menuitemradio').map((b) => b.textContent?.trim())).toEqual(['openmarket', 'New group “open”']);
    await fireEvent.click(screen.getByText('New group “open”'));
    expect(p.ongroup).toHaveBeenCalledWith('open');
    await fireEvent.input(input, { target: { value: '' } });
    await tick();
    await fireEvent.click(screen.getByText('Back to automatic'));
    expect(p.ongroup).toHaveBeenLastCalledWith(null);
  });
});
```

- [ ] **Step 2: Verify failure.**
- [ ] **Step 3: Implement**:

```svelte
<script lang="ts">
  // The New session picker's per-project actions (project picker spec v2):
  // ⇧F10 / right-click / ⌘G on the highlighted project. A `role=menu`;
  // focus moves in, and Esc hands it back to the switcher's input.
  import { onMount, tick } from 'svelte';

  let {
    title, pinned, hidden, groups, currentGroup, manualGroup, startIn,
    onpin, onhide, ongroup, onclose,
  }: {
    title: string;
    pinned: boolean;
    hidden: boolean;
    groups: readonly string[];
    currentGroup: string | null;
    manualGroup: boolean;
    startIn: 'main' | 'groups';
    onpin: () => void;
    onhide: () => void;
    ongroup: (name: string | null) => void;
    onclose: () => void;
  } = $props();

  let mode = $state<'main' | 'groups'>(startIn);
  let draft = $state('');
  let root: HTMLElement | undefined = $state();

  const shown = $derived.by(() => {
    const d = draft.trim();
    const list = groups.filter((g) => !d || g.toLowerCase().includes(d.toLowerCase())).slice(0, 6);
    const out: { label: string; value: string | null; current: boolean }[] = list.map((g) => ({ label: g, value: g, current: g === currentGroup }));
    if (d && !groups.some((g) => g.toLowerCase() === d.toLowerCase())) out.push({ label: `New group “${d}”`, value: d, current: false });
    if (!d && manualGroup) out.push({ label: 'Back to automatic', value: null, current: false });
    return out;
  });

  async function focusFirst() {
    await tick();
    const el =
      mode === 'groups'
        ? root?.querySelector<HTMLElement>('input')
        : root?.querySelector<HTMLElement>('[role=menuitem]');
    el?.focus();
  }
  onMount(focusFirst);

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      onclose();
      return;
    }
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
    const items = [...(root?.querySelectorAll<HTMLElement>('[role^=menuitem]') ?? [])];
    if (!items.length) return;
    e.preventDefault();
    const i = items.indexOf(document.activeElement as HTMLElement);
    const n = e.key === 'ArrowDown' ? (i + 1) % items.length : (i - 1 + items.length) % items.length;
    items[n].focus();
  }
</script>

<!-- svelte-ignore a11y_interactive_supports_focus -->
<div class="menu" role="menu" aria-label={title} bind:this={root} onkeydown={onKey} data-testid="project-actions">
  <div class="title">{title}</div>
  {#if mode === 'main'}
    <button type="button" role="menuitem" class="mi" onclick={onpin}>{pinned ? 'Unpin' : 'Pin to top'}<kbd>⌘P</kbd></button>
    <button type="button" role="menuitem" class="mi" onclick={() => { mode = 'groups'; void focusFirst(); }}>Move to group…<kbd>⌘G</kbd></button>
    <button type="button" role="menuitem" class="mi" onclick={onhide}>{hidden ? 'Unhide' : 'Hide'}<kbd>⌘⌫</kbd></button>
  {:else}
    <input
      class="gi"
      type="text"
      aria-label="Group name"
      placeholder="Group name…"
      bind:value={draft}
      autocomplete="off"
      spellcheck="false"
      onkeydown={(e) => {
        if (e.key === 'Enter' && shown[0]) {
          e.preventDefault();
          ongroup(shown[0].value);
        }
      }}
    />
    {#each shown as g (g.label)}
      <button type="button" role="menuitemradio" aria-checked={g.current} class="mi" onclick={() => ongroup(g.value)}>{g.label}</button>
    {/each}
  {/if}
</div>

<style>
  .menu {
    position: absolute;
    right: 1rem;
    z-index: 3;
    width: 16rem;
    padding: 0.35rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    box-shadow: 0 12px 32px rgba(0, 0, 0, 0.25);
  }
  .title { padding: 0.3rem 0.5rem; font-size: 0.7rem; color: var(--fg-muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .mi {
    display: flex; align-items: center; width: 100%; gap: 0.5rem;
    height: var(--control-h-lg); padding: 0 0.5rem;
    border: none; background: transparent; color: var(--fg);
    font: inherit; font-size: 0.8rem; text-align: left; border-radius: var(--radius-sm); cursor: pointer;
  }
  .mi:hover, .mi:focus-visible { background: var(--accent-soft); outline: none; }
  .mi[aria-checked='true'] { font-weight: 600; }
  kbd { margin-left: auto; font: inherit; font-size: 0.65rem; color: var(--fg-muted); }
  .gi { width: 100%; box-sizing: border-box; margin-bottom: 0.3rem; padding: 0.3rem 0.5rem; font: inherit; font-size: 0.8rem; }
</style>
```

- [ ] **Step 4: Run** → `2 passed`; `npx svelte-check --threshold error` → 0 errors.
- [ ] **Step 5: Commit**

```bash
git add src/lib/ProjectActionsMenu.svelte src/lib/ProjectActionsMenu.test.ts
git commit -m "feat(picker): the project actions menu — pin, group combobox, hide"
```

---

### Task 9: `NewSessionDialog` autostart

**Files:** Modify `src/lib/NewSessionDialog.svelte`, `src/lib/NewSessionDialog.test.ts`, `src/lib/new_session_request.ts`, `src/App.svelte` (the `{#if $newSessionRequest}` mount ~line 822)

**Interfaces — produces:** `NewSessionRequest.autostart?: boolean`; dialog prop `autostart?: boolean` — submits once when ready.

- [ ] **Step 1: Failing tests** — in `NewSessionDialog.test.ts`, copy the arrange block of the file's existing successful-create test verbatim (its render helper, its invoke mock that answers `new_session`, its way of reading the invoke calls) and add `autostart: true` to the props. Two tests:
  1. *autostart submits once with the remembered choices*: after `await vi.waitFor(...)` on one `new_session` call, two more `await tick()` still show exactly **one** `new_session` call.
  2. *autostart does nothing while a hub blocks new_session*: arrange exactly like the file's existing hub-blocked test, plus `autostart: true`; after three ticks there are **zero** `new_session` calls and the dialog (`getByRole('dialog', { name: 'New session' })`) is still there.

- [ ] **Step 2: Verify failure** (`autostart` is not a prop).

- [ ] **Step 3: Implement** — `new_session_request.ts`, in `NewSessionRequest`:

```ts
  /** Start at once with the remembered choices (the picker's ⌘↵); the
   *  dialog stays open only if something needs a person. */
  autostart?: boolean;
```

`NewSessionDialog.svelte`: add `autostart = false,` to the props destructure and `autostart?: boolean;` (with that doc comment) to its type. After `submit` is defined:

```ts
  // The picker's ⌘↵ (project picker spec v2): once this host's worktree
  // list is in, submit once with what the dialog remembered. Anything that
  // needs a person — a new worktree without a name, a blocked hub, an error —
  // leaves the dialog open, as if the person had pressed Create.
  let autostarted = false;
  $effect(() => {
    if (!autostart || autostarted || busy) return;
    if (hostWorktrees.status === 'loading') return;
    if (newSessionBlocked || (inNewMode && !newWorktreeName.trim())) return;
    autostarted = true;
    void submit();
  });
```

(`hostWorktrees.status`, `newSessionBlocked`, `inNewMode`, `newWorktreeName`, `busy`, `submit` are the dialog's existing names.)

`App.svelte`: pass `autostart={$newSessionRequest.autostart}` to the mounted `NewSessionDialog`.

- [ ] **Step 4: Run** `npx vitest run src/lib/NewSessionDialog.test.ts` → all pass.
- [ ] **Step 5: Commit**

```bash
git add src/lib/NewSessionDialog.svelte src/lib/NewSessionDialog.test.ts src/lib/new_session_request.ts src/App.svelte
git commit -m "feat(picker): NewSessionDialog autostart — ⌘↵ starts with the last settings"
```

---

### Task 10: The switcher's New session mode

**Files:**
- Create: `src/lib/switcher_request.ts`
- Modify: `src/lib/app_views.ts` (add `addProjectRequest`), `src/lib/quick_switcher.ts` (+ `src/lib/quick_switcher.test.ts`), `src/lib/QuickSwitcher.svelte` (+ `src/lib/QuickSwitcher.test.ts`)

**Interfaces:**
- Consumes: Tasks 4–9 (`projectPicks`, `loadProjectPicks`, `setProjectPick`, `previousPick`, `pickKey`, `recordPick`, `readFrecency`, `entriesOf`, `buildSections`, `searchEntries`, `hiddenReason`, `Entry`, `ViewRow`, the `PickerItem` extensions, `ProjectActionsMenu`, `autostart`), `newSessionHostRequest` (`app_views.ts`).
- Produces:
  - `switcher_request.ts`: `switcherRequest: Writable<{ mode: 'new'; host?: string } | null>`, `openNewSessionPicker(host?: string): void`
  - `app_views.ts`: `addProjectRequest: Writable<{ cloneUrl?: string } | null>` (consumed by the Sidebar in Task 11)
  - `quick_switcher.ts`: `isNewSessionChord(e, isMac): boolean`, `workBlock(tickets: readonly SwitcherTicket[], cap = 3): SwitcherTicket[]`

- [ ] **Step 1: Pure helpers, test first** — append to `quick_switcher.test.ts`:

```ts
import { isNewSessionChord, workBlock } from './quick_switcher';

describe('isNewSessionChord', () => {
  const k = (o: Partial<KeyboardEvent>) => ({ key: 'n', metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, ...o });
  it('⌘N on macOS, Ctrl+Shift+N elsewhere; nothing else', () => {
    expect(isNewSessionChord(k({ metaKey: true }), true)).toBe(true);
    expect(isNewSessionChord(k({ metaKey: true, shiftKey: true }), true)).toBe(false);
    expect(isNewSessionChord(k({ ctrlKey: true, shiftKey: true, key: 'N' }), false)).toBe(true);
    expect(isNewSessionChord(k({ ctrlKey: true }), false)).toBe(false); // readline's Ctrl+N stays the terminal's
  });
});

describe('workBlock', () => {
  const t = (key: string, section: string, live: number[] = []) => ({ ticket: { key, live_session_ids: live } as never, section });
  it('My work tickets with no live session, at most 3', () => {
    const out = workBlock([t('A-1', 'My work'), t('A-2', 'My work', [5]), t('A-3', 'Recent'), t('A-4', 'My work'), t('A-5', 'My work'), t('A-6', 'My work')]);
    expect(out.map((x) => (x.ticket as { key: string }).key)).toEqual(['A-1', 'A-4', 'A-5']);
  });
});
```

Implement in `quick_switcher.ts`:

```ts
/** The New session picker's chord (project picker spec v2): ⌘N on macOS,
 *  Ctrl+Shift+N elsewhere — plain Ctrl+N stays readline's next-history. */
export function isNewSessionChord(
  e: { key: string; metaKey: boolean; ctrlKey: boolean; altKey: boolean; shiftKey: boolean },
  isMac: boolean,
): boolean {
  if (e.key.toLowerCase() !== 'n' || e.altKey) return false;
  if (isMac) return e.metaKey && !e.ctrlKey && !e.shiftKey;
  return e.ctrlKey && e.shiftKey && !e.metaKey;
}

/** "Start from work": My work tickets nobody has a session on, at most `cap`. */
export function workBlock(tickets: readonly SwitcherTicket[], cap = 3): SwitcherTicket[] {
  return tickets
    .filter((t) => t.section === 'My work' && (t.ticket.live_session_ids ?? []).length === 0)
    .slice(0, cap);
}
```

Run `npx vitest run src/lib/quick_switcher.test.ts` → pass. Create `switcher_request.ts`:

```ts
// "Open the switcher in New session mode" (project picker spec v2) — from
// the sidebar's "+ New session" button, which cannot reach the switcher
// mounted in App.svelte.
import { writable } from 'svelte/store';

export const switcherRequest = writable<{ mode: 'new'; host?: string } | null>(null);

export function openNewSessionPicker(host?: string): void {
  switcherRequest.set({ mode: 'new', host });
}
```

In `app_views.ts`, beside `newSessionHostRequest`:

```ts
/** "Open Add project" from the switcher's Add row; the Sidebar owns the
 *  dialog. `cloneUrl` prefills the Clone URL field. */
export const addProjectRequest = writable<{ cloneUrl?: string } | null>(null);
```

- [ ] **Step 2: Failing component tests** — append to `QuickSwitcher.test.ts` (reuse its `sess` fixture; `__invoke` is the name the existing ticket tests use for the mocked `invoke` — use whatever that file actually names it):

```ts
import { switcherRequest, openNewSessionPicker } from './switcher_request';
import { projectPicks } from './project_picks';
import { newSessionHostRequest } from './app_views';

describe('QuickSwitcher — New session mode', () => {
  const NOW = Math.floor(Date.now() / 1000);
  const p = (id: number, owner: string, repo: string, ago: number | null) => ({
    project: { id, owner, repo, base_path: `/r/${repo}`, last_session_at: ago === null ? null : NOW - ago, adopted: false, system: false },
    worktrees: [],
  });
  const fleet = p(1, 'me', 'claude-fleet', 60);
  const om = [p(2, 'pp', 'openmarket-ai', 3600), p(3, 'pp', 'openmarket-docs', 7200), p(4, 'pp', 'openmarket-app', null)];
  const epic = p(5, 'me', 'ppt-epic-145', null);

  beforeEach(() => {
    projects.set([fleet, ...om, epic]);
    sessions.set([]);
    projectPicks.set(new Map());
    switcherRequest.set(null);
  });

  async function openNew(host?: string) {
    openNewSessionPicker(host);
    await tick();
    return screen.getByTestId('switcher-input') as HTMLInputElement;
  }

  it('opens from the request with the mode chip, projects only, Hidden folded', async () => {
    sessions.set([sess({ id: 1, project_id: 1 })]);
    render(QuickSwitcher);
    await openNew();
    expect(screen.getByTestId('mode-chip').textContent).toBe('New session in');
    expect(screen.queryAllByTestId('switcher-session')).toHaveLength(0);
    expect(screen.getByText('openmarket-* · pp')).toBeTruthy(); // the cluster heading's subtitle
    expect(screen.getByText(/Show 1 in Hidden/)).toBeTruthy();
    expect(screen.queryByText('ppt-epic-145')).toBeNull();
  });

  it('Ctrl+Shift+N opens it; Backspace on an empty query leaves the mode', async () => {
    render(QuickSwitcher);
    await fireEvent.keyDown(window, { key: 'N', ctrlKey: true, shiftKey: true });
    await tick();
    const input = screen.getByTestId('switcher-input');
    expect(screen.getByTestId('mode-chip')).toBeTruthy();
    await fireEvent.keyDown(input, { key: 'Backspace' });
    await tick();
    expect(screen.queryByTestId('mode-chip')).toBeNull();
  });

  it('the Hosts view request opens it with that host as context', async () => {
    sessions.set([sess({ id: 9, project_id: 3, host_alias: 'mefistos' })]);
    render(QuickSwitcher);
    newSessionHostRequest.set('mefistos');
    await tick(); await tick();
    expect(screen.getByText('on mefistos')).toBeTruthy();
  });

  it('Enter opens the dialog and records the pick; Ctrl+Enter asks for autostart', async () => {
    render(QuickSwitcher);
    const input = await openNew();
    await fireEvent.input(input, { target: { value: 'openmarket-docs' } });
    await tick();
    await fireEvent.keyDown(input, { key: 'Enter' });
    await tick();
    expect(get(newSessionRequest)?.project.project.repo).toBe('openmarket-docs');
    expect(JSON.parse(localStorage.getItem('newsession.frecency') ?? '{}')['pp/openmarket-docs']).toBeTruthy();
    clearNewSessionRequest();
    const input2 = await openNew();
    await fireEvent.input(input2, { target: { value: 'openmarket-ai' } });
    await tick();
    await fireEvent.keyDown(input2, { key: 'Enter', ctrlKey: true });
    await tick();
    expect(get(newSessionRequest)?.autostart).toBe(true);
  });

  it('Ctrl+P pins the highlighted project; Ctrl+Z undoes it', async () => {
    vi.mocked(__invoke).mockImplementation(async (cmd: string, args?: unknown) =>
      cmd === 'set_project_pick' ? (args as { args: unknown }).args : null);
    render(QuickSwitcher);
    const input = await openNew();
    await fireEvent.input(input, { target: { value: 'openmarket-ai' } });
    await tick();
    await fireEvent.keyDown(input, { key: 'p', ctrlKey: true });
    await vi.waitFor(() => expect(get(projectPicks).get('pp/openmarket-ai')?.pinned).toBe(true));
    await fireEvent.keyDown(input, { key: 'z', ctrlKey: true });
    await vi.waitFor(() => expect(get(projectPicks).get('pp/openmarket-ai')?.pinned).toBe(false));
  });

  it('the order is frozen while open: picks arriving later do not move the highlight', async () => {
    render(QuickSwitcher);
    const input = await openNew();
    const before = input.getAttribute('aria-activedescendant');
    projectPicks.set(new Map([['pp/openmarket-app', { owner: 'pp', repo: 'openmarket-app', pinned: true, vis: null, grp: null }]]));
    await tick();
    expect(input.getAttribute('aria-activedescendant')).toBe(before);
    expect(screen.queryByText('Pinned')).toBeNull(); // shown on the next open
  });

  it('Esc clears a query first, then closes', async () => {
    render(QuickSwitcher);
    const input = await openNew();
    await fireEvent.input(input, { target: { value: 'zz' } });
    await fireEvent.keyDown(input, { key: 'Escape' });
    await tick();
    expect(input.value).toBe('');
    expect(screen.getByTestId('quick-switcher')).toBeTruthy();
  });

  it('no match offers Add project with the query', async () => {
    render(QuickSwitcher);
    const input = await openNew();
    await fireEvent.input(input, { target: { value: 'acme/widgets' } });
    await tick();
    expect(screen.getByText('Add project “acme/widgets”…')).toBeTruthy();
  });

  it('Enter on a fold row unfolds it', async () => {
    render(QuickSwitcher);
    const input = await openNew();
    const fold = screen.getByText(/Show 1 in Hidden/).closest('[role=option]')!;
    await fireEvent.mouseMove(fold);
    await fireEvent.keyDown(input, { key: 'Enter' });
    await tick();
    expect(screen.getByText('ppt-epic-145')).toBeTruthy();
  });
});
```

Run `npx vitest run src/lib/QuickSwitcher.test.ts` → the new block fails.

- [ ] **Step 3: Implement in `QuickSwitcher.svelte`.** Imports to add:

```ts
  import { get } from 'svelte/store';
  import { untrack } from 'svelte';
  import ProjectActionsMenu from './ProjectActionsMenu.svelte';
  import { switcherRequest } from './switcher_request';
  import { newSessionHostRequest, addProjectRequest } from './app_views';
  import { loadProjectPicks, pickKey, previousPick, projectPicks, setProjectPick } from './project_picks';
  import { readFrecency, recordPick } from './frecency';
  import { buildSections, entriesOf, hiddenReason, searchEntries, type Entry, type ViewRow } from './project_rank';
  import { isNewSessionChord, workBlock } from './quick_switcher';
  import { fuzzyMatchFields } from './fuzzy';
```

State:

```ts
  let mode = $state<'switch' | 'new'>('switch');
  let preferredHost = $state<string | null>(null);
  let toggled = $state<ReadonlySet<string>>(new Set());
  // Re-rank triggers besides the query: open, the person's own actions, folds.
  let seq = $state(0);
  let menu = $state<{ key: string; startIn: 'main' | 'groups' } | null>(null);
  let lastUndo: (() => void) | null = null;
  const nowSec = () => Math.floor(Date.now() / 1000);
```

The frozen view — the stores are read **untracked**, so data arriving while open never re-ranks:

```ts
  const newView = $derived.by(() => {
    void seq;
    void query;
    void toggled;
    if (mode !== 'new') return null;
    return untrack(() => {
      const now = nowSec();
      const entries = entriesOf(get(projects), get(projectPicks), now);
      const ctx = { selectedProjectId: get(selectedSession)?.project_id ?? null, preferredHost, sessions: get(sessions) };
      const f = readFrecency();
      return { entries, sections: buildSections(entries, ctx, f, now), search: searchEntries(entries, query, ctx, f, now) };
    });
  });
```

Items in New session mode (tickets first, then sections or search, fold rows, Add last):

```ts
  const projectItem = (r: ViewRow, group: string, groupKey: string, groupSub: string): PickerItem => ({
    key: `project:${r.entry.id}`,
    label: r.entry.label,
    meta: r.meta,
    chip: r.chip || undefined,
    kbd: r.kbd || undefined,
    dim: r.entry.dormant || !!r.entry.hidden,
    group,
    groupKey,
    groupSub,
    actionable: true,
    testid: 'switcher-project',
  });
  const newItems: PickerItem[] = $derived.by(() => {
    const v = newView;
    if (!v) return [];
    const out: PickerItem[] = [];
    const q = query.trim();
    const work = q
      ? ticketRows.filter((e) => fuzzyMatchFields(q, e.fields) !== null)
      : workBlock(tickets)
          .map((t) => ticketRows.find((e) => e.ticket?.key === t.ticket.key))
          .filter((e): e is SwitcherEntry => !!e);
    for (const e of work) {
      const place = e.ticket
        ? placeForTicket(e.ticket.key ?? '', $sessions, $projects, (s) => workKeyFor(s, branchById)?.key ?? null)
        : null;
      out.push({
        key: e.key,
        label: e.label,
        description: place ? `→ ${place.project.project.repo} · ${place.host}` : e.description,
        badge: e.badge,
        group: q ? 'Tickets' : 'Start from work',
        groupKey: 'work',
        groupSub: q ? '' : 'My work · no session yet',
        testid: 'switcher-ticket',
      });
    }
    if (q) {
      const n = v.search.length;
      v.search.forEach((r) => out.push(projectItem(r, `${n} project${n === 1 ? '' : 's'}`, 'search', '')));
    } else {
      for (const s of v.sections) {
        const open = s.foldable ? (s.openByDefault ? !toggled.has(s.key) : toggled.has(s.key)) : true;
        if (open) s.rows.forEach((r) => out.push(projectItem(r, s.label, s.key, s.sub)));
        else
          out.push({
            key: `fold:${s.key}`,
            label: s.key === 'hidden' ? `Show ${s.rows.length} in Hidden` : `${s.label} · ${s.rows.length} project${s.rows.length === 1 ? '' : 's'}`,
            description: s.sub,
            meta: '▸',
            group: s.label,
            groupKey: s.key,
            testid: 'switcher-fold',
          });
      }
    }
    out.push({ key: 'add', label: q ? `Add project “${q}”…` : 'Add project…', group: q ? 'Not here?' : ' ', testid: 'switcher-add' });
    return out;
  });
```

(`ticketRows`, `tickets`, `branchById`, `placeForTicket`, `workKeyFor` already exist in this file.)

Point the list at the mode: in the markup use `items={mode === 'new' ? newItems : items}`; the "keep the highlight on a row that still exists" `$effect` must read `(mode === 'new' ? newItems : items).map((i) => i.key)`; `move()` walks the same list.

Normal ⌘K mode drops picker-hidden projects from the empty-query list, so both surfaces agree on what is hidden — change the `ranked` line to:

```ts
  const visibleEntries = $derived(
    query.trim()
      ? entries
      : entries.filter(
          (e) =>
            e.kind !== 'project' ||
            !e.project ||
            !hiddenReason(e.project, $projectPicks.get(pickKey(e.project.project.owner, e.project.project.repo)), nowSec()),
        ),
  );
  const ranked: SwitcherEntry[] = $derived(rankEntries(visibleEntries, query, $recentSessions));
```

Opening:

```ts
  function show(next: 'switch' | 'new' = 'switch', host: string | null = null) {
    query = '';
    activeKey = null;
    mode = next;
    preferredHost = host;
    toggled = new Set();
    menu = null;
    open = true;
    seq++;
    void loadTickets();
    // Fresh picks for the NEXT open: this one's view is frozen.
    if (next === 'new') void loadProjectPicks();
  }
  const unsubReq = switcherRequest.subscribe((r) => {
    if (!r) return;
    switcherRequest.set(null);
    show('new', r.host ?? null);
  });
  const unsubHost = newSessionHostRequest.subscribe((h) => {
    if (h === null) return;
    newSessionHostRequest.set(null);
    show('new', h);
  });
  onDestroy(() => {
    unsubReq();
    unsubHost();
  });
```

At the top of `onWindowKeydown`:

```ts
    if (isNewSessionChord(e, isMac)) {
      if (!open && (e.target as Element | null)?.closest?.('dialog')) return;
      e.preventDefault();
      e.stopPropagation();
      if (open && mode === 'new') hide();
      else show('new');
      return;
    }
```

Actions:

```ts
  const entryOf = (key: string | null): Entry | null =>
    key?.startsWith('project:') ? (newView?.entries.find((e) => `project:${e.id}` === key) ?? null) : null;

  function act(e: Entry, patch: Parameters<typeof setProjectPick>[2], said: string) {
    const { owner, repo } = e.project.project;
    seq++;
    void setProjectPick(owner, repo, patch).then((r) => {
      seq++;
      if (!r.ok) return;
      const prev = previousPick(owner, repo);
      lastUndo = prev
        ? () => void setProjectPick(owner, repo, { pinned: prev.pinned, vis: prev.vis, grp: prev.grp }).then(() => seq++)
        : null;
      push({ kind: 'info', message: said, action: lastUndo ? { label: 'Undo', run: lastUndo } : undefined });
    });
  }
  const togglePin = (e: Entry) => act(e, { pinned: !e.pinned }, `${e.pinned ? 'Unpinned' : 'Pinned'} ${e.label}`);
  const toggleHide = (e: Entry) => act(e, e.hidden ? { vis: 'keep' } : { vis: 'hide' }, `${e.hidden ? 'Unhid' : 'Hid'} ${e.label}`);
  function setGroup(e: Entry, g: string | null) {
    act(e, { grp: g }, g ? `Moved ${e.label} to ${g}` : `${e.label} is grouped automatically`);
    menu = null;
  }
  function pickProject(e: Entry, autostart = false) {
    recordPick(e.key);
    requestNewSession({ project: e.project, initialHost: preferredHost ?? undefined, autostart });
    hide();
  }
  function toggleFold(sectionKey: string) {
    const n = new Set(toggled);
    if (n.has(sectionKey)) n.delete(sectionKey);
    else n.add(sectionKey);
    toggled = n;
  }
```

Note: `setProjectPick` patches the store optimistically **before** its promise resolves, so the `seq++` before it re-ranks with the new state at once (the person sees their own change); the frozen view only ignores changes the person did not make.

At the top of `pick(key)`:

```ts
    if (mode === 'new') {
      if (key === 'add') {
        addProjectRequest.set({ cloneUrl: query.trim() || undefined });
        hide();
        return;
      }
      if (key.startsWith('fold:')) {
        toggleFold(key.slice(5));
        activeKey = null;
        return;
      }
      const e = entryOf(key);
      if (e) {
        pickProject(e);
        return;
      }
    }
```

(tickets fall through to the existing ticket branch.)

At the top of `onInputKeydown`:

```ts
    if (mode === 'new') {
      const mod = e.metaKey || e.ctrlKey;
      const cur = entryOf(activeKey);
      if (e.key === 'Escape' && (menu || query)) {
        e.preventDefault();
        e.stopPropagation();
        if (menu) menu = null;
        else {
          query = '';
          activeKey = null;
        }
        return;
      }
      if (e.key === 'Backspace' && query === '' && !mod) {
        e.preventDefault();
        mode = 'switch';
        activeKey = null;
        return;
      }
      if (mod && e.key === 'Enter' && cur) { e.preventDefault(); pickProject(cur, true); return; }
      if (mod && e.key.toLowerCase() === 'p' && cur) { e.preventDefault(); togglePin(cur); return; }
      if (mod && e.key === 'Backspace' && cur) { e.preventDefault(); toggleHide(cur); return; }
      if (mod && e.key.toLowerCase() === 'g' && cur) { e.preventDefault(); menu = { key: activeKey!, startIn: 'groups' }; return; }
      if (mod && e.key.toLowerCase() === 'z' && lastUndo) {
        e.preventDefault();
        const u = lastUndo;
        lastUndo = null;
        u();
        return;
      }
      if ((e.key === 'F10' && e.shiftKey) || e.key === 'ContextMenu') {
        if (cur) {
          e.preventDefault();
          menu = { key: activeKey!, startIn: 'main' };
        }
        return;
      }
      if (mod && /^[1-9]$/.test(e.key)) {
        const it = newItems.find((i) => i.kbd === `⌘${e.key}`);
        const en = entryOf(it?.key ?? null);
        if (en) {
          e.preventDefault();
          pickProject(en);
        }
        return;
      }
      if (e.key === 'ArrowRight' && activeKey?.startsWith('fold:')) { e.preventDefault(); toggleFold(activeKey.slice(5)); return; }
      if (e.key === 'ArrowLeft' && cur) {
        const sec = newItems.find((i) => i.key === activeKey)?.groupKey;
        if (sec && sec.startsWith('g:')) {
          e.preventDefault();
          toggleFold(sec);
          activeKey = `fold:${sec}`;
          return;
        }
      }
    }
```

The existing Enter branch then calls `pick(activeKey)`; the existing Ctrl/Cmd+Enter on a ticket keeps calling `startTicketNow`. (`kbd` labels use `⌘` on every platform in the ranking; the hint row shows `modKey` — keep both consistent with how this file already labels Ctrl on Linux if you prefer `Ctrl+1`.)

Markup (inside the Modal):

```svelte
    <div class="qrow">
      {#if mode === 'new'}<span class="mode-chip" data-testid="mode-chip">New session in</span>{/if}
      <input
        …the existing attributes…
        placeholder={mode === 'new'
          ? 'project or ticket…'
          : 'Jump to a session, host or ticket… (name, key, project, host, branch, status, or paste a ticket URL)'}
      />
    </div>
    <PickerList
      items={mode === 'new' ? newItems : items}
      …the existing props…
      maxHeight={mode === 'new' ? 'min(70vh, 34rem)' : 'min(60vh, 24rem)'}
      ongroupclick={mode === 'new' ? (k) => { if (k !== 'work' && k !== 'search') toggleFold(k); } : undefined}
      oncontext={mode === 'new' ? (k) => { if (entryOf(k)) { activeKey = k; menu = { key: k, startIn: 'main' }; } } : undefined}
      rowActions={mode === 'new' ? actions : undefined}
    />
    {#if menu && entryOf(menu.key)}
      {@const e = entryOf(menu.key)!}
      <ProjectActionsMenu
        title={e.key}
        pinned={e.pinned}
        hidden={!!e.hidden}
        groups={[...new Set((newView?.entries ?? []).map((x) => x.group).filter((g) => !g.key.startsWith('o:') && g.key !== 'f').map((g) => g.name))].sort()}
        currentGroup={e.group.name}
        manualGroup={e.group.key.startsWith('m:')}
        startIn={menu.startIn}
        onpin={() => { togglePin(e); menu = null; }}
        onhide={() => { toggleHide(e); menu = null; }}
        ongroup={(g) => setGroup(e, g)}
        onclose={() => {
          menu = null;
          void tick().then(() => document.querySelector<HTMLInputElement>('[data-testid=switcher-input]')?.focus());
        }}
      />
    {/if}
```

The row-actions snippet (a mouse convenience; the keys and the menu are the accessible path), with inline 14px stroke SVGs copied from the mockup (`Main.dc.html`: pin, folder, eye-off) — no emoji:

```svelte
{#snippet actions(item: PickerItem)}
  {@const e = entryOf(item.key)}
  {#if e}
    <button type="button" tabindex="-1" class="ib" class:on={e.pinned} title={e.pinned ? 'Unpin' : 'Pin to top'}
      onclick={(ev) => { ev.stopPropagation(); togglePin(e); }}><!-- pin svg --></button>
    <button type="button" tabindex="-1" class="ib" title="Move to group…"
      onclick={(ev) => { ev.stopPropagation(); activeKey = item.key; menu = { key: item.key, startIn: 'groups' }; }}><!-- folder svg --></button>
    <button type="button" tabindex="-1" class="ib" title={e.hidden ? 'Unhide' : 'Hide'}
      onclick={(ev) => { ev.stopPropagation(); toggleHide(e); }}><!-- eye-off svg --></button>
  {/if}
{/snippet}
```

Hint row in new mode: `↵ open · {modKey}↵ start with last settings · {modKey}P pin · {modKey}⌫ hide · ⇧F10 more · esc close`. Styles: `.qrow{display:flex;align-items:center;gap:.5rem}`; `.mode-chip{flex:0 0 auto;font-size:.75rem;font-weight:600;padding:.15rem .5rem;border-radius:var(--radius-sm);background:var(--accent-soft);color:var(--accent)}`; `.ib{width:24px;height:24px;display:inline-flex;align-items:center;justify-content:center;border:none;background:transparent;border-radius:4px;color:var(--fg-muted);cursor:pointer}` `.ib:hover{background:var(--bg);color:var(--fg)}` `.ib.on{color:var(--accent)}`. The menu is `position:absolute` — wrap the Modal's content in `<div class="body" style="position: relative">` if Modal does not already provide a positioned box.

- [ ] **Step 4: Run** `npx vitest run src/lib/QuickSwitcher.test.ts src/lib/quick_switcher.test.ts` → all pass (old and new). Then `npx vitest run` and `npx svelte-check --threshold error`.
- [ ] **Step 5: Commit**

```bash
git add src/lib/switcher_request.ts src/lib/app_views.ts src/lib/quick_switcher.ts src/lib/quick_switcher.test.ts src/lib/QuickSwitcher.svelte src/lib/QuickSwitcher.test.ts
git commit -m "feat(picker): the switcher's New session mode — tickets, pins, suggestions, groups, keys, undo"
```

---

### Task 11: Entry points — the sidebar opens the switcher; Add project round-trips

**Files:** Modify `src/lib/Sidebar.svelte`, `src/lib/Sidebar.test.ts`, `src/lib/hub_disabled.test.ts`, `src/lib/AddProjectDialog.svelte` (+ `src/lib/AddProjectDialog.test.ts`), `src/App.svelte` and `src/lib/new_session_request.ts` (comments only)

**Interfaces:** consumes `openNewSessionPicker`, `addProjectRequest`, `setProjectPick`. `AddProjectDialog` gains `initialCloneUrl?: string` (prefills the Clone URL field and selects that mode).

- [ ] **Step 1: Tests first.**
  - `AddProjectDialog.test.ts`: `render(AddProjectDialog, { props: { onCreated: vi.fn(), onCancel: vi.fn(), initialCloneUrl: 'acme/widgets' } })` → `expect((screen.getByTestId('clone-url') as HTMLInputElement).value).toBe('acme/widgets')`.
  - `Sidebar.test.ts`: delete the four popover tests (`footer "+ New session" button opens project picker`, `project picker shows ALL projects regardless of recency/search filter`, `the project picker hides the UX agent's system project…`, `the project picker offers Add project, which opens the dialog`) and `Add project is reachable with no projects at all`; add:

```ts
  it('"+ New session" opens the switcher in New session mode', async () => {
    mockBackend(fakeProjects, []);
    render(Sidebar);
    await tick(); await tick();
    await fireEvent.click(screen.getByTestId('new-session-footer'));
    expect(get(switcherRequest)).toEqual({ mode: 'new', host: undefined });
    expect(screen.queryByRole('listbox', { name: 'Pick project for new session' })).toBeNull();
  });

  it('an Add project request opens the dialog prefilled, and the added project is kept', async () => {
    const added = {
      project: { id: 42, owner: 'newowner', repo: 'fresh-repo', base_path: '/r/fresh', last_session_at: null, adopted: false, system: false },
      worktrees: [],
    };
    mockBackend(fakeProjects, []);
    const base = (mockedInvoke as ReturnType<typeof vi.fn>).getMockImplementation() as (cmd: string, args?: unknown) => Promise<unknown>;
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string, args?: unknown) =>
      cmd === 'add_project' ? added : cmd === 'set_project_pick' ? (args as { args: unknown }).args : base(cmd, args),
    );
    render(Sidebar);
    await tick(); await tick();
    addProjectRequest.set({ cloneUrl: 'newowner/fresh-repo' });
    await tick(); await tick();
    expect((screen.getByTestId('clone-url') as HTMLInputElement).value).toBe('newowner/fresh-repo');
    await fireEvent.click(screen.getByTestId('add-create'));
    await vi.waitFor(() => expect(screen.queryByTestId('add-project-dialog')).toBeNull());
    expect(mockedInvoke).toHaveBeenCalledWith('set_project_pick', {
      args: { owner: 'newowner', repo: 'fresh-repo', pinned: false, vis: 'keep', grp: null },
    });
    expect(screen.getByRole('heading', { name: /New session/ }).textContent).toContain('newowner/fresh-repo');
  });
```

  - The remaining tests that reached Add project through `add-project-row` (e.g. *adopting a folder while a remote host is chosen…*, and the ones near Sidebar.test.ts:1937/1950): open the dialog with `addProjectRequest.set({})` instead; keep their assertions.
  - `hub_disabled.test.ts` ~611–634 (`add-project-row`): open the dialog with `addProjectRequest.set({})` and assert the dialog's create control the same way they assert the row today (disabled on a blocked hub, enabled otherwise); keep `+ New session is enabled on a hub client`.
  - Import `switcherRequest` from `./switcher_request` and `addProjectRequest` from `./app_views` in the test files.

- [ ] **Step 2: Verify failure** — `npx vitest run src/lib/Sidebar.test.ts src/lib/AddProjectDialog.test.ts src/lib/hub_disabled.test.ts`.

- [ ] **Step 3: Implement.**
  - `AddProjectDialog.svelte`: prop `initialCloneUrl?: string`; initialise the Clone URL field's state from `untrack(() => initialCloneUrl ?? '')` and the mode to `'clone'` when it is set.
  - `Sidebar.svelte`:
    - The footer button (`data-testid="new-session-footer"`) calls `openNewSessionPicker()`; give it `title="New session (⌘N)"` on macOS / `"New session (Ctrl+Shift+N)"` otherwise and the matching `aria-keyshortcuts` (`Meta+N` / `Control+Shift+N`).
    - Delete `showProjectPicker`, `pickerHost`, `openNewSession`'s popover logic, `toggleProjectPicker`, `allProjectsSorted` and `collidingRepos` if nothing else uses them (search the file), the `{#if showProjectPicker}` popover markup, the `.picker*` styles, the picker's `svelte:window` Escape handler, and the `$newSessionHostRequest` effect (the switcher consumes that request now). `openAddProject` stays (other callers).
    - Subscribe to `addProjectRequest`: when it is set, keep `initialCloneUrl = req.cloneUrl`, set `showAddProject = true`, and clear the request; pass `initialCloneUrl` to `AddProjectDialog`.
    - First line of `onProjectAdded`: `void setProjectPick(row.project.owner, row.project.repo, { vis: 'keep' });` (adding a project is the person saying it matters).
    - A project row's own `+` (`openNew(row, e)`) keeps opening `NewSessionDialog` for that project directly — unchanged.
  - Update the comment above `<QuickSwitcher />` in `App.svelte` and the header of `new_session_request.ts`: the switcher is now the one place a project is picked; the Sidebar's own dialog mount remains for a row's `+` and Add project.

- [ ] **Step 4: Run** `npx vitest run` (whole suite) and `npx svelte-check --threshold error` → all pass, 0 errors.
- [ ] **Step 5: Commit**

```bash
git add src/lib/Sidebar.svelte src/lib/Sidebar.test.ts src/lib/hub_disabled.test.ts src/lib/AddProjectDialog.svelte src/lib/AddProjectDialog.test.ts src/App.svelte src/lib/new_session_request.ts
git commit -m "feat(picker): + New session and Hosts n open the switcher; the sidebar popover is gone"
```

---

### Task 12: Whole-tree verification

- [ ] **Step 1: Sync** — `git fetch origin && git log --oneline HEAD..origin/main | head`. If `main` moved, merge `origin/main` locally (likely conflict: the migration number — renumber 104), resolve, continue.
- [ ] **Step 2: Local CI**, foreground, unpiped: `scripts/ci-local.sh` → every stage green; read the whole output.
- [ ] **Step 3: Generated files current** — `git status --short` shows nothing after the run; a rewritten generated file means a missed REGEN: re-run it, commit.
- [ ] **Step 4: What jsdom cannot prove** (WKWebView): Esc with a query clears without closing the Modal; ⌘N reaches the window handler (no native menu item takes it); hover actions do not cover the chip; dark-theme contrast of chip, kbd and dimmed rows. Check in a sandboxed dev run only — never against the real HOME (the dev build migrates the production `state.db` and kills the installed app).
- [ ] **Step 5: Commit any fixes** — `git add -A && git commit -m "chore(picker): CI fixes"` (only if needed).
