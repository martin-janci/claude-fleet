# Assets M1: the catalogs table, the registry and `scope` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the single-row `catalog_config` and the process-global `CATALOG: Option<Catalog>` with a `catalogs` table and a registry keyed by catalog id, and add the typed `scope: private | shared` to assets — with behaviour unchanged: there is still exactly one catalog, `personal`.

**Architecture:** Migration 090 creates `catalogs` and copies the `catalog_config` row into it as `personal`. The store's existing catalog-config API keeps its signatures but reads and writes the `personal` row, so every caller (desktop, hub CLI, MCP) keeps working. A new `service/catalog/registry.rs` holds loaded catalogs by id; the ~12 production reads of `CATALOG` go through `registry::personal()` / `with_personal()`. `Header` gains `scope`, written to YAML only when it is `shared`.

**Tech Stack:** Rust (fleet-core, rusqlite), Svelte/TS types only.

**Spec:** `docs/superpowers/specs/2026-09-30-assets-s1b-s2-design.md` — milestone **M1** (Data model → `catalogs`, Migration step 1, `scope`, Runtime).

## Global Constraints

- Migration number **090** (`090_catalogs.sql`); `main` took 089 for `hosts.harnesses` (F3a), so this one follows it.
- A catalog with `org_id NULL` is exactly the one named `personal` (`CHECK ((name = 'personal') = (org_id IS NULL))`).
- `scope` defaults to **private**; YAML omits `scope` when it is private; the JSON API always carries it.
- Behaviour after M1 is identical to before for a user with one catalog: `catalog_config`/`catalog_load`/`catalog set` work unchanged.
- Never hold the `Store` guard across an `.await`. Every child process through `crate::proc`. Shell values through `crate::shell::quote`.
- `cargo test` takes ONE name filter per command.
- Wire compatibility: new serialized fields are `#[serde(default)]`; TS treats them as optional.
- Known pre-existing unrelated failure: `rewind::tests::the_removal_script_leaves_a_tree_a_live_pane_is_in` (tmux socket path length under a long `$TMPDIR`).

## Scope ruling

The spec lists one migration for all of S1b+S2. This plan gives each milestone the schema it needs: M1 creates `catalogs` only; `host_layers.catalog_id` and `asset_inventory.catalog_id` arrive with M2, `host_catalogs` and `client_catalog_grants` with M3, `changesets` and `asset_triage_verdicts` with M4. Each milestone stays independently shippable.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/fleet-core/migrations/090_catalogs.sql` (new) | `catalogs` table + backfill from `catalog_config` |
| `crates/fleet-core/src/store/schema.rs` | register migration 90 |
| `crates/fleet-core/src/store/rows.rs` | `CatalogRow` |
| `crates/fleet-core/src/store/catalog.rs` | `list_catalogs`, `get_catalog`, `personal_catalog`, `set_catalog_head_for`; the existing config API reimplemented on `catalogs` |
| `crates/fleet-core/src/service/catalog/registry.rs` (new) | loaded catalogs by id: `install`, `get`, `personal`, `with_personal`, `clear` |
| `crates/fleet-core/src/service/catalog/repo.rs` | `Catalog` gains `id`, `name`, `org_id` |
| `crates/fleet-core/src/service/catalog/{mod,author,inventory,scan_tick}.rs`, `sync/mod.rs` | use the registry |
| `crates/fleet-core/src/service/catalog/model.rs` | `Scope`, `Header.scope`, YAML strip |
| `src/lib/assets.ts` | `scope?` on `AssetSummary` and `EditableAsset` |

---

### Task 1: The `catalogs` table under the existing config API

**Files:**
- Create: `crates/fleet-core/migrations/090_catalogs.sql`
- Modify: `crates/fleet-core/src/store/schema.rs` (`MIGRATIONS`, after the 88 entry)
- Modify: `crates/fleet-core/src/store/rows.rs` (next to `CatalogConfigRow`)
- Modify: `crates/fleet-core/src/store/catalog.rs` (`get_catalog_config`, `set_catalog_config`, `set_catalog_head`, + new functions)

**Interfaces:**
- Produces:

```rust
// store/rows.rs
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CatalogRow {
    pub id: i64,
    pub name: String,
    pub repo_path: String,
    pub remote_url: Option<String>,
    pub org_id: Option<i64>,
    pub head_commit: Option<String>,
    pub last_loaded_at: Option<i64>,
}

// store/catalog.rs, impl Store
pub fn list_catalogs(&self) -> Result<Vec<CatalogRow>, rusqlite::Error>;          // personal first, then by name
pub fn get_catalog(&self, id: i64) -> Result<Option<CatalogRow>, rusqlite::Error>;
pub fn personal_catalog(&self) -> Result<Option<CatalogRow>, rusqlite::Error>;
pub fn set_catalog_head_for(&self, id: i64, head: &str, loaded_at: i64) -> Result<(), rusqlite::Error>;
// unchanged signatures, now backed by the `personal` row:
pub fn get_catalog_config(&self) -> Result<Option<CatalogConfigRow>, rusqlite::Error>;
pub fn set_catalog_config(&self, repo_path: &str, remote_url: Option<&str>) -> Result<CatalogConfigRow, rusqlite::Error>;
pub fn set_catalog_head(&self, head: &str, loaded_at: i64) -> Result<(), rusqlite::Error>;
```

- [ ] **Step 1: Write the failing tests** (in `store/catalog.rs` tests; keep the existing `catalog_config_set_get_and_head` test unchanged — it must still pass)

```rust
#[test]
fn set_catalog_config_writes_the_personal_catalog_row() {
    let s = Store::open_in_memory().expect("open");
    assert!(s.personal_catalog().unwrap().is_none());
    s.set_catalog_config("/tmp/assets", Some("git@x:y.git")).unwrap();
    let p = s.personal_catalog().unwrap().expect("personal row");
    assert_eq!(p.name, "personal");
    assert_eq!(p.org_id, None);
    assert_eq!(p.repo_path, "/tmp/assets");
    assert_eq!(p.remote_url.as_deref(), Some("git@x:y.git"));
    assert_eq!(s.list_catalogs().unwrap(), vec![p.clone()]);
    assert_eq!(s.get_catalog(p.id).unwrap(), Some(p));
}

#[test]
fn set_catalog_head_for_targets_one_catalog() {
    let s = Store::open_in_memory().expect("open");
    s.set_catalog_config("/tmp/assets", None).unwrap();
    let id = s.personal_catalog().unwrap().unwrap().id;
    s.set_catalog_head_for(id, "abc", 7).unwrap();
    let cfg = s.get_catalog_config().unwrap().unwrap();
    assert_eq!(cfg.head_commit.as_deref(), Some("abc"));
    assert_eq!(cfg.last_loaded_at, Some(7));
}

#[test]
fn only_the_personal_catalog_may_have_no_org() {
    let s = Store::open_in_memory().expect("open");
    let err = s
        .conn
        .execute(
            "INSERT INTO catalogs (name, repo_path, org_id, created_at) VALUES ('other', '/x', NULL, 0)",
            [],
        )
        .unwrap_err();
    assert!(err.to_string().contains("CHECK"), "{err}");
}
```

In `store/schema.rs` tests, beside the 030 re-run test (same pattern: an in-memory store, roll `schema_version` back, `migrate()` again):

```rust
/// 090 on a database stopped at 089 with a `catalog_config` row: the row
/// becomes the `personal` catalog, and a re-run changes nothing.
#[test]
fn migration_90_copies_catalog_config_into_personal() {
    let old = Store::open_in_memory().expect("open");
    old.conn
        .execute_batch(
            "DROP TABLE catalogs;\
             INSERT INTO catalog_config (id, repo_path, remote_url, head_commit, last_loaded_at) \
               VALUES (1, '/r', 'git@a:b.git', 'h1', 5);\
             DELETE FROM schema_version WHERE version >= 90;",
        )
        .unwrap();
    old.migrate().expect("090 on an existing DB");
    assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    let p = old.personal_catalog().unwrap().expect("personal row");
    assert_eq!((p.name.as_str(), p.repo_path.as_str()), ("personal", "/r"));
    assert_eq!(p.remote_url.as_deref(), Some("git@a:b.git"));
    assert_eq!((p.head_commit.as_deref(), p.last_loaded_at, p.org_id), (Some("h1"), Some(5), None));
    old.conn
        .execute_batch("DELETE FROM schema_version WHERE version >= 90;")
        .unwrap();
    old.migrate().expect("re-running 090 is safe");
    assert_eq!(old.list_catalogs().unwrap().len(), 1, "no second personal row");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p fleet-core store::catalog` and `cargo test -p fleet-core migration_90`
Expected: compile errors (`personal_catalog`, `CatalogRow`, … not found).

- [ ] **Step 3: Implement**

`migrations/090_catalogs.sql`:

```sql
-- Assets S1b (M1): a catalog is a source with an owner. `org_id NULL` is the
-- personal catalog, and only it (the CHECK). Replaces the single-row
-- `catalog_config`, which stays but is no longer read.
CREATE TABLE IF NOT EXISTS catalogs (
  id             INTEGER PRIMARY KEY AUTOINCREMENT,
  name           TEXT    NOT NULL UNIQUE,
  repo_path      TEXT    NOT NULL,
  remote_url     TEXT,
  org_id         INTEGER REFERENCES orgs(id) ON DELETE RESTRICT,
  head_commit    TEXT,
  last_loaded_at INTEGER,
  created_at     INTEGER NOT NULL,
  CHECK ((name = 'personal') = (org_id IS NULL))
);

INSERT OR IGNORE INTO catalogs (name, repo_path, remote_url, org_id, head_commit, last_loaded_at, created_at)
  SELECT 'personal', repo_path, remote_url, NULL, head_commit, last_loaded_at, CAST(strftime('%s','now') AS INTEGER)
  FROM catalog_config WHERE id = 1;

INSERT OR IGNORE INTO schema_version (version) VALUES (90);
```

`store/schema.rs` — append:

```rust
    // Assets S1b M1: `catalogs`, backfilled from `catalog_config` as
    // `personal`. CREATE IF NOT EXISTS + INSERT OR IGNORE: safe to re-run.
    Migration::plain(90, include_str!("../../migrations/090_catalogs.sql")),
```

`store/rows.rs` — `CatalogRow` exactly as in Interfaces.

`store/catalog.rs` — replace the three config functions and add the new ones:

```rust
    const CATALOG_COLS: &'static str =
        "id, name, repo_path, remote_url, org_id, head_commit, last_loaded_at";

    fn catalog_row(r: &rusqlite::Row) -> rusqlite::Result<CatalogRow> {
        Ok(CatalogRow {
            id: r.get(0)?,
            name: r.get(1)?,
            repo_path: r.get(2)?,
            remote_url: r.get(3)?,
            org_id: r.get(4)?,
            head_commit: r.get(5)?,
            last_loaded_at: r.get(6)?,
        })
    }

    pub fn list_catalogs(&self) -> Result<Vec<CatalogRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {} FROM catalogs ORDER BY (org_id IS NOT NULL), name",
            Self::CATALOG_COLS
        ))?;
        let rows = stmt.query_map([], Self::catalog_row)?;
        rows.collect()
    }

    pub fn get_catalog(&self, id: i64) -> Result<Option<CatalogRow>, rusqlite::Error> {
        use rusqlite::OptionalExtension;
        self.conn
            .prepare_cached(&format!("SELECT {} FROM catalogs WHERE id = ?1", Self::CATALOG_COLS))?
            .query_row([id], Self::catalog_row)
            .optional()
    }

    pub fn personal_catalog(&self) -> Result<Option<CatalogRow>, rusqlite::Error> {
        use rusqlite::OptionalExtension;
        self.conn
            .prepare_cached(&format!("SELECT {} FROM catalogs WHERE org_id IS NULL", Self::CATALOG_COLS))?
            .query_row([], Self::catalog_row)
            .optional()
    }

    pub fn get_catalog_config(&self) -> Result<Option<CatalogConfigRow>, rusqlite::Error> {
        Ok(self.personal_catalog()?.map(|c| CatalogConfigRow {
            repo_path: c.repo_path,
            remote_url: c.remote_url,
            head_commit: c.head_commit,
            last_loaded_at: c.last_loaded_at,
        }))
    }

    pub fn set_catalog_config(
        &self,
        repo_path: &str,
        remote_url: Option<&str>,
    ) -> Result<CatalogConfigRow, rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO catalogs (name, repo_path, remote_url, org_id, created_at)
             VALUES ('personal', ?1, ?2, NULL, CAST(strftime('%s','now') AS INTEGER))
             ON CONFLICT(name) DO UPDATE SET repo_path=excluded.repo_path, remote_url=excluded.remote_url,
                                             head_commit=NULL, last_loaded_at=NULL",
            rusqlite::params![repo_path, remote_url],
        )?;
        Ok(self.get_catalog_config()?.expect("row just written"))
    }

    pub fn set_catalog_head(&self, head: &str, loaded_at: i64) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE catalogs SET head_commit=?1, last_loaded_at=?2 WHERE org_id IS NULL",
            rusqlite::params![head, loaded_at],
        )?;
        Ok(())
    }

    pub fn set_catalog_head_for(&self, id: i64, head: &str, loaded_at: i64) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE catalogs SET head_commit=?1, last_loaded_at=?2 WHERE id=?3",
            rusqlite::params![head, loaded_at, id],
        )?;
        Ok(())
    }
```

Keep whatever event/bus emission the old `set_catalog_config` did (check the current body before replacing it). Update the module doc comment at the top of `store/catalog.rs` (it says "`catalog_config` row") to name `catalogs`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p fleet-core store::catalog`, then `cargo test -p fleet-core schema`, then `cargo test -p fleet-core catalog`
Expected: PASS, including the unchanged `catalog_config_set_get_and_head` and every migration re-run test.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/migrations/090_catalogs.sql crates/fleet-core/src/store
git commit -m "feat(catalog): a catalogs table under the existing config API (migration 090)"
```

---

### Task 2: A registry of loaded catalogs instead of one global

**Files:**
- Create: `crates/fleet-core/src/service/catalog/registry.rs`
- Modify: `crates/fleet-core/src/service/catalog/repo.rs:15-23` (`Catalog`)
- Modify: `crates/fleet-core/src/service/catalog/mod.rs` (`CATALOG` static, `load`, `ensure_fresh`, `with_catalog`, tests at ~651-672)
- Modify: `crates/fleet-core/src/service/catalog/author.rs` (~563, ~582, ~950, test ~1986)
- Modify: `crates/fleet-core/src/service/catalog/inventory.rs` (~168, test ~990)
- Modify: `crates/fleet-core/src/service/catalog/scan_tick.rs` (~100)
- Modify: `crates/fleet-core/src/service/catalog/sync/mod.rs` (`catalog()` ~100, test ~612)

**Interfaces:**
- Consumes: `Store::personal_catalog`, `Store::set_catalog_head_for` (Task 1)
- Produces:

```rust
// repo.rs — Catalog gains (it already derives Default):
pub id: i64,              // catalogs.id; 0 in tests that build one by hand
pub name: String,         // "personal", …
pub org_id: Option<i64>,  // None = personal

// registry.rs
pub fn install(cat: repo::Catalog) -> Result<(), IpcError>;                 // insert/replace by cat.id
pub fn get(id: i64) -> Result<Option<repo::Catalog>, IpcError>;             // clone
pub fn personal() -> Result<Option<repo::Catalog>, IpcError>;               // the entry with org_id None, clone
pub fn with_personal<T>(f: impl FnOnce(&repo::Catalog) -> Result<T, IpcError>) -> Result<T, IpcError>;
                                                                            // E_CATALOG_NOT_CONFIGURED "catalog not loaded; call catalog_load" when absent
pub fn clear() -> Result<(), IpcError>;                                     // tests and reconfigure
```

`CATALOG` is removed. `CATALOG_TEST_LOCK` stays (it also guards `HOME`).

- [ ] **Step 1: Write the failing tests** (`registry.rs` tests; take `CATALOG_TEST_LOCK` because the registry is process-global)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::catalog::repo::Catalog;

    fn cat(id: i64, name: &str, org: Option<i64>) -> Catalog {
        Catalog { id, name: name.into(), org_id: org, head: format!("h{id}"), ..Default::default() }
    }

    #[test]
    fn personal_is_the_entry_without_an_org() {
        let _l = crate::service::catalog::CATALOG_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear().unwrap();
        install(cat(2, "papayapos", Some(9))).unwrap();
        assert!(personal().unwrap().is_none());
        install(cat(1, "personal", None)).unwrap();
        assert_eq!(personal().unwrap().unwrap().head, "h1");
        assert_eq!(get(2).unwrap().unwrap().name, "papayapos");
        clear().unwrap();
    }

    #[test]
    fn with_personal_reports_not_configured_when_empty() {
        let _l = crate::service::catalog::CATALOG_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear().unwrap();
        let err = with_personal(|_| Ok(())).unwrap_err();
        assert_eq!(err.code, crate::service::catalog::E_CATALOG_NOT_CONFIGURED);
    }

    #[test]
    fn install_replaces_by_id() {
        let _l = crate::service::catalog::CATALOG_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear().unwrap();
        install(cat(1, "personal", None)).unwrap();
        let mut c = cat(1, "personal", None);
        c.head = "new".into();
        install(c).unwrap();
        assert_eq!(personal().unwrap().unwrap().head, "new");
        clear().unwrap();
    }
}
```

And in `mod.rs` tests, next to the existing load tests: after `load(false, &store)` on a configured temp repo, `registry::personal()` returns a catalog whose `name == "personal"`, `org_id == None`, and `id == store.personal_catalog().unwrap().unwrap().id`.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p fleet-core catalog::registry`
Expected: compile error (module missing).

- [ ] **Step 3: Implement**

`registry.rs`:

```rust
//! Loaded catalogs, by `catalogs.id`. Assets S1b: replaces the single
//! process-global `CATALOG`. Only `load` (and tests) write it.

use super::repo::Catalog;
use crate::ipc_error::{codes, IpcError};
use std::collections::BTreeMap;
use std::sync::{LazyLock, RwLock};

static CATALOGS: LazyLock<RwLock<BTreeMap<i64, Catalog>>> =
    LazyLock::new(|| RwLock::new(BTreeMap::new()));

fn poisoned() -> IpcError {
    IpcError::new(codes::E_LOCK, "catalog lock poisoned")
}

pub fn install(cat: Catalog) -> Result<(), IpcError> {
    CATALOGS.write().map_err(|_| poisoned())?.insert(cat.id, cat);
    Ok(())
}

pub fn get(id: i64) -> Result<Option<Catalog>, IpcError> {
    Ok(CATALOGS.read().map_err(|_| poisoned())?.get(&id).cloned())
}

pub fn personal() -> Result<Option<Catalog>, IpcError> {
    Ok(CATALOGS
        .read()
        .map_err(|_| poisoned())?
        .values()
        .find(|c| c.org_id.is_none())
        .cloned())
}

pub fn with_personal<T>(f: impl FnOnce(&Catalog) -> Result<T, IpcError>) -> Result<T, IpcError> {
    let guard = CATALOGS.read().map_err(|_| poisoned())?;
    match guard.values().find(|c| c.org_id.is_none()) {
        Some(c) => f(c),
        None => Err(IpcError::new(
            super::E_CATALOG_NOT_CONFIGURED,
            "catalog not loaded; call catalog_load",
        )),
    }
}

pub fn clear() -> Result<(), IpcError> {
    CATALOGS.write().map_err(|_| poisoned())?.clear();
    Ok(())
}
```

`repo.rs` — add the three fields to `Catalog` (`pub id: i64`, `pub name: String`, `pub org_id: Option<i64>`) with a doc line each; `load_dir` leaves them default (the caller sets them).

`mod.rs`:
- `pub mod registry;`; delete the `CATALOG` static and its doc comment.
- `load`: read `store.personal_catalog()` (inside the existing `lock(store)` scope, not across any `.await` — `load` is sync) to get `id`/`name`/`org_id`; set them on `cat` before `registry::install(cat)`; replace `s.set_catalog_head(..)` with `s.set_catalog_head_for(id, ..)`. Keep `require_config` as the "configure first" error.
- `ensure_fresh`: compare the store's personal row with `registry::personal()?` exactly as it compared with `CATALOG` (`last_loaded_at == loaded_at && head_commit == head`).
- `with_catalog(f)` becomes a thin wrapper: `registry::with_personal(f)` (keep the name so its callers don't change).
- Tests: `*CATALOG.write().unwrap() = None` → `registry::clear().unwrap()`; `CATALOG.write().unwrap().as_mut().unwrap().assets.clear()` → take `registry::personal().unwrap().unwrap()`, clear its `assets`, `registry::install(..)`.

`author.rs`, `inventory.rs`, `scan_tick.rs`, `sync/mod.rs` — replace each read of `CATALOG` with `registry::personal()?` (cloned) or `registry::with_personal(|c| …)`, keeping each site's existing error message and behaviour (`inventory::scan_hosts` and `sync::catalog()` must still return `E_CATALOG_NOT_CONFIGURED` "catalog not loaded; call catalog_load" when nothing is loaded; `scan_tick` still `continue`s). Test writes `*…CATALOG.write().unwrap() = Some(cat)` → `crate::service::catalog::registry::install(Catalog { org_id: None, ..cat }).unwrap()` (a hand-built catalog has `org_id: None` by default, so plain `install(cat)` works).

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p fleet-core catalog::registry`, `cargo test -p fleet-core catalog`, then `cargo test -p fleet-core` and `cargo test -p claude-fleet --lib`, and `cargo build -p fleet-hub`.
Expected: PASS (only the known `rewind` failure in the full fleet-core run). `grep -rn "CATALOG\b" crates src-tauri --include='*.rs' | grep -v "CATALOG_TEST_LOCK\|E_CATALOG"` prints nothing.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/service/catalog
git commit -m "refactor(catalog): a registry of loaded catalogs replaces the CATALOG global"
```

---

### Task 3: `scope: private | shared` on assets

**Files:**
- Modify: `crates/fleet-core/src/service/catalog/model.rs` (`Header` at ~134, `AssetFile` serializer at ~331)
- Modify: `crates/fleet-core/src/service/catalog/mod.rs` (`AssetSummary`, `list_assets`)
- Modify: every `Header { … }` literal the compiler reports
- Modify: `src/lib/assets.ts` (`AssetSummary`, `EditableAsset`)

**Interfaces:**
- Produces:

```rust
// model.rs
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    #[default]
    Private,
    Shared,
}
// Header gains:  #[serde(default)] pub scope: Scope,
// AssetSummary gains: #[serde(default)] pub scope: model::Scope,
```

```ts
// assets.ts
export type AssetScope = 'private' | 'shared';
// AssetSummary: scope?: AssetScope;   EditableAsset: scope?: AssetScope;
```

- [ ] **Step 1: Write the failing tests** (`model.rs` tests)

```rust
#[test]
fn scope_defaults_to_private_and_is_not_written_to_yaml() {
    let a = Asset::from_yaml(Some(Kind::Skill), "kind: skill\nname: s\ndescription: d\n").unwrap();
    assert_eq!(a.header.scope, Scope::Private);
    let yaml = a.to_yaml();
    assert!(!yaml.contains("scope"), "{yaml}");
}

#[test]
fn shared_scope_round_trips_through_yaml() {
    let a = Asset::from_yaml(Some(Kind::Skill), "kind: skill\nname: s\ndescription: d\nscope: shared\n").unwrap();
    assert_eq!(a.header.scope, Scope::Shared);
    let yaml = a.to_yaml();
    assert!(yaml.contains("scope: shared"), "{yaml}");
    assert_eq!(Asset::from_yaml(Some(Kind::Skill), &yaml).unwrap().header.scope, Scope::Shared);
}

#[test]
fn the_json_api_always_carries_scope() {
    let a = Asset::from_yaml(Some(Kind::Skill), "kind: skill\nname: s\ndescription: d\n").unwrap();
    let v = serde_json::to_value(&a).unwrap();
    assert_eq!(v["scope"], "private");
}
```

(`Asset::to_yaml()` at `model.rs:421` writes `asset.yaml` through `AssetFile`; it returns `String`.)

In `mod.rs` tests: `list_assets` on a catalog with one `scope: shared` asset returns that `AssetSummary` with `scope == Scope::Shared`, and the other with `Scope::Private`.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p fleet-core catalog::model`
Expected: compile error (`Scope` missing).

- [ ] **Step 3: Implement**

`model.rs`: add `Scope` as in Interfaces; in `Header`, after `install_as`:

```rust
    /// Who may receive this asset (Assets S1b). Only meaningful in the
    /// personal catalog: `private` never reaches an org-bound host,
    /// `shared` may. Assets in an org catalog are org-scoped regardless.
    /// Always present in the JSON API; omitted from `asset.yaml` when
    /// private (the default), so existing files stay unchanged.
    #[serde(default)]
    pub scope: Scope,
```

In `impl Serialize for AssetFile`, next to the empty-`tags` strip:

```rust
        if matches!(map.get("scope"), Some(serde_yaml::Value::String(s)) if s == "private") {
            map.remove("scope");
        }
```

Fix every `Header { … }` literal the compiler reports with `scope: Scope::Private` (or `Default::default()`).

`mod.rs`: `AssetSummary` gains `#[serde(default)] pub scope: model::Scope,` and `list_assets` sets `scope: a.header.scope`.

`assets.ts`: add `AssetScope` and the optional fields. No UI change in M1 (the badge is M5); `AssetEditor` round-trips the field through `EditableAsset`'s index signature already — add one Vitest assertion in `assets.test.ts` that an `EditableAsset` with `scope: 'shared'` passed to `updateAsset` is sent unchanged.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p fleet-core catalog::model`, `cargo test -p fleet-core catalog`, `pnpm test src/lib/assets.test.ts`, `pnpm check`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/service/catalog src/lib/assets.ts src/lib/assets.test.ts
git commit -m "feat(catalog): assets carry scope: private | shared (private by default)"
```

---

### Task 4: Verification and docs

**Files:**
- Modify: `CLAUDE.md` (the catalog notes, next to the S1a paragraph)
- Modify: `docs/hub.md` (the "Asset catalog" section)

- [ ] **Step 1: Run every suite**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
pnpm test
pnpm check
```

Expected: all pass except the known `rewind` socket-path test.

- [ ] **Step 2: Regenerate and check nothing is stale**

```bash
REGEN_DOCS=1 cargo test -p fleet-core reference_is_current
REGEN_SETTINGS_DOCS=1 cargo test -p fleet-core settings_docs_are_current
REGEN_PAGE_DOCS=1 cargo test -p fleet-core page_docs_are_current
REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen
git status --short
```

Expected: no diff (M1 changes no tool, setting, page or verdict). Commit any regenerated file if one changes, naming the generator.

- [ ] **Step 3: Docs**

`CLAUDE.md` — one short paragraph: catalogs live in the `catalogs` table (migration 090; `catalog_config` is no longer read), the existing config API and `fleet-hub catalog set` address the `personal` row; loaded catalogs are in `service/catalog/registry.rs` (`personal()`, `with_personal`, `get(id)`); assets carry `scope: private | shared` (private by default, omitted from YAML). Point at this plan and the S1b+S2 spec.

`docs/hub.md` "Asset catalog": one sentence that `catalog set` configures the `personal` catalog, and that per-org catalogs arrive later (S1b M3).

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md docs/hub.md
git commit -m "docs: the catalogs table, the registry and scope"
```

---

## Self-review against the spec (M1)

| Spec item | Task |
|---|---|
| `catalogs` table with the personal/org CHECK | 1 |
| Migration step 1: `catalog_config` → `personal` | 1 |
| `catalog_config` stays but is not read | 1 (store API rewritten on `catalogs`) |
| Runtime: `CATALOGS` registry by id, `with_catalog` | 2 |
| A catalog whose repo cannot load is a problem entry, others load | M3 (only one catalog exists in M1) |
| `scope: private | shared`, default private, typed, YAML-omitted when private | 3 |
| `effective_for_host` | M2 (it is defined by the sync rules) |
| Migration steps 2–3 (`host_layers`, grants) | M2, M3 (Scope ruling above) |
