# Assets M2: sync across catalogs — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Plan and scan against the set of catalogs a host accepts — each catalog's own layers, the scope boundary, and collisions between catalogs — while a fleet with only the `personal` catalog behaves exactly as today.

**Architecture:** Migration 091 gives `host_layers` and `asset_inventory` a `catalog_id`. `repo::Catalog` gains an `origin` map (asset key → which catalog it came from). A pure `acceptance()` rule decides what a host may take from a catalog. `effective_for_host()` reads the store first (host org, layers per catalog), releases it, then borrows the registry and composes one effective `Catalog` for the host: per catalog `resolve()`, filter by acceptance, merge, and remove collisions. The existing planner and `compute_states` run unchanged on that effective catalog; conflicts and scope-boundary refusals become per-asset `blocked` actions with a reason, and every action and manifest entry records its catalog.

**Tech Stack:** Rust (fleet-core, rusqlite).

**Spec:** `docs/superpowers/specs/2026-09-30-assets-s1b-s2-design.md` — milestone **M2** ("Sync across catalogs"; Data model: `host_layers.catalog_id`, `asset_inventory.catalog_id`, manifest `catalog`; `effective_for_host`, `with_catalog(id, f)`; layers must not override `scope`).

## Global Constraints

- Migration number **091** (`091_catalog_ids.sql`); 090 (`catalogs`) is the latest on `main`.
- Lock rule: **registry → store is allowed; store → registry never.** `effective_for_host` reads all store rows first, drops the guard, then takes the registry.
- A fleet with only `personal` must plan, scan, sync and preview exactly as before (same actions, same inventory rows, same manifest bytes apart from the new `catalog` field).
- New serialized fields are `#[serde(default)]` (hub ↔ desktop wire; older manifests on hosts).
- Admissions (`host_catalogs`) arrive in M3: in M2 a host with no org accepts only `personal`.
- `cargo test` takes ONE name filter per command. Known unrelated failures: `rewind::tests::the_removal_script_leaves_a_tree_a_live_pane_is_in` (socket path); `work::scale_tests::*` perf under load.
- Tests touching the registry take `crate::service::catalog::lock_registry_for_test()`.

## Rulings

- **Scope-boundary "fails validation" granularity.** The spec says a layer that would put a private asset on an org host "fails validation with the reason". This plan fails it **per asset**: that asset's action for that host is `blocked` with the reason `private asset "<name>" (layer <layer>, catalog personal) may not go to org host <host>; mark it shared or remove it from the layer`, and the rest of the host plans normally. Refusing the whole host would stall every other asset for one mislabelled skill. Same shape as the collision rule.
- **Inventory across catalogs.** A scan compares the host against the union of all loaded catalogs (personal first, then by name; on a name collision the first one's row wins and the plan reports the conflict), stamping each managed row's `catalog_id`.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/fleet-core/migrations/091_catalog_ids.sql` (new) | `host_layers` rebuilt with `catalog_id`; `asset_inventory.catalog_id` |
| `crates/fleet-core/src/store/{schema,layers,catalog,rows}.rs` | migration entry + guard; per-catalog layer API; inventory `catalog_id` |
| `crates/fleet-core/src/service/catalog/repo.rs` | `CatalogRef`, `Catalog.origin` |
| `crates/fleet-core/src/service/catalog/registry.rs` | `with_catalogs` (borrow all), `union_all` |
| `crates/fleet-core/src/service/catalog/effective.rs` (new) | `Acceptance`, `acceptance()`, `effective_for_host()`, `EffectiveSet` |
| `crates/fleet-core/src/service/catalog/resolve.rs` | `apply_override` rejects `scope`; `Provenance.catalog` |
| `crates/fleet-core/src/service/catalog/sync/{mod,plan,manifest,apply,layers}.rs`, `inventory.rs`, `mod.rs` | wiring |

---

### Task 1: `catalog_id` on layers and inventory (migration 091)

**Files:**
- Create: `crates/fleet-core/migrations/091_catalog_ids.sql`
- Modify: `crates/fleet-core/src/store/schema.rs` (MIGRATIONS + a guard fn; `LATEST/KNOWN_SCHEMA_VERSION` if they are explicit constants)
- Modify: `crates/fleet-core/src/store/layers.rs` (`HostLayerRow`, `COLS`, `row_from`, `get_host_layers`, `set_host_layers`, new functions)
- Modify: `crates/fleet-core/src/store/rows.rs` (`AssetInventoryRow.catalog_id`) and `store/catalog.rs` (insert/select)

**Interfaces:**
- Produces:

```rust
// store/layers.rs
pub struct HostLayerRow { /* existing fields */ pub catalog_id: i64 }
pub fn get_host_layers(&self, host_alias: &str) -> Result<Vec<HostLayerRow>, rusqlite::Error>;            // all catalogs (unchanged signature)
pub fn get_host_layers_for(&self, host_alias: &str, catalog_id: i64) -> Result<Vec<HostLayerRow>, rusqlite::Error>;
pub fn set_host_layers(&self, host_alias: &str, role: Option<&str>, contexts: &[&str]) -> Result<(), rusqlite::Error>;  // unchanged signature → personal
pub fn set_host_layers_for(&self, host_alias: &str, catalog_id: i64, role: Option<&str>, contexts: &[&str]) -> Result<(), rusqlite::Error>;
// store/rows.rs — AssetInventoryRow gains:  #[serde(default)] pub catalog_id: Option<i64>,
```

- [ ] **Step 1: Write the failing tests**

`store/layers.rs` tests:

```rust
#[test]
fn layers_are_kept_per_catalog() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h").unwrap();
    s.set_catalog_config("/p", None).unwrap();
    let personal = s.personal_catalog().unwrap().unwrap().id;
    s.conn
        .execute(
            "INSERT INTO orgs (name, created_at) VALUES ('acme', 0);
             ",
            [],
        )
        .unwrap();
    let org: i64 = s.conn.query_row("SELECT id FROM orgs WHERE name='acme'", [], |r| r.get(0)).unwrap();
    s.conn
        .execute(
            "INSERT INTO catalogs (name, repo_path, org_id, created_at) VALUES ('acme', '/a', ?1, 0)",
            [org],
        )
        .unwrap();
    let acme: i64 = s.conn.query_row("SELECT id FROM catalogs WHERE name='acme'", [], |r| r.get(0)).unwrap();

    s.set_host_layers("h", Some("core"), &[]).unwrap();                  // personal
    s.set_host_layers_for("h", acme, Some("ops"), &["extra"]).unwrap();  // same host, other catalog

    let p = s.get_host_layers_for("h", personal).unwrap();
    assert_eq!(p.iter().map(|r| r.layer_name.as_str()).collect::<Vec<_>>(), vec!["core"]);
    let a = s.get_host_layers_for("h", acme).unwrap();
    assert_eq!(a.len(), 2);
    assert!(a.iter().all(|r| r.catalog_id == acme));
    assert_eq!(s.get_host_layers("h").unwrap().len(), 3);
    // Re-setting one catalog's layers leaves the other's alone.
    s.set_host_layers_for("h", acme, None, &[]).unwrap();
    assert_eq!(s.get_host_layers("h").unwrap().len(), 1);
}
```

`store/schema.rs` tests (same pattern as the 090 test: in-memory store, roll back, `migrate()` again):

```rust
/// 091 on a database stopped at 090: existing host_layers rows and managed
/// inventory rows get the personal catalog's id; a re-run is safe.
#[test]
fn migration_91_backfills_catalog_ids() {
    let old = Store::open_in_memory().expect("open");
    old.set_catalog_config("/p", None).unwrap();
    let personal = old.personal_catalog().unwrap().unwrap().id;
    old.upsert_host("h").unwrap();
    // Recreate the 090 shape of host_layers and drop the new inventory column.
    old.conn
        .execute_batch(
            "DROP TABLE host_layers;\
             CREATE TABLE host_layers (host_alias TEXT NOT NULL REFERENCES hosts(alias), layer_name TEXT NOT NULL, \
               axis TEXT NOT NULL, position INTEGER NOT NULL DEFAULT 0, active INTEGER NOT NULL DEFAULT 1, \
               PRIMARY KEY (host_alias, layer_name));\
             INSERT INTO host_layers (host_alias, layer_name, axis) VALUES ('h', 'core', 'role');\
             ALTER TABLE asset_inventory DROP COLUMN catalog_id;\
             INSERT INTO asset_inventory (host_alias, harness, kind, name, state, scanned_at, managed) \
               VALUES ('h','claude','skill','a','in_sync',1,1), ('h','claude','skill','u','unmanaged',1,0);\
             DELETE FROM schema_version WHERE version >= 91;",
        )
        .unwrap();
    old.migrate().expect("091");
    let rows = old.get_host_layers_for("h", personal).unwrap();
    assert_eq!(rows.len(), 1);
    let inv = old.list_inventory().unwrap();
    assert_eq!(inv.iter().find(|r| r.name == "a").unwrap().catalog_id, Some(personal));
    assert_eq!(inv.iter().find(|r| r.name == "u").unwrap().catalog_id, None);
    old.conn.execute_batch("DELETE FROM schema_version WHERE version >= 91;").unwrap();
    old.migrate().expect("re-running 091 is safe");
    assert_eq!(old.get_host_layers("h").unwrap().len(), 1);
}
```

(If this SQLite build lacks `ALTER TABLE … DROP COLUMN`, rebuild `asset_inventory` without the column in the test instead — check how the 087 guard test simulates a pre-migration table and copy it.)

- [ ] **Step 2: Run to verify failure** — `cargo test -p fleet-core store::layers`, `cargo test -p fleet-core migration_91` → compile errors.

- [ ] **Step 3: Implement**

`migrations/091_catalog_ids.sql` (guarded in schema.rs because `ADD COLUMN` is not idempotent; the table rebuild is written to be re-runnable):

```sql
-- Assets S1b (M2): a layer belongs to a catalog; a managed inventory row
-- names the catalog its asset came from. Existing rows belong to
-- `personal`. host_layers is rebuilt (its primary key changes).
CREATE TABLE IF NOT EXISTS host_layers_new (
  host_alias TEXT    NOT NULL REFERENCES hosts(alias),
  catalog_id INTEGER NOT NULL REFERENCES catalogs(id) ON DELETE CASCADE,
  layer_name TEXT    NOT NULL,
  axis       TEXT    NOT NULL,
  position   INTEGER NOT NULL DEFAULT 0,
  active     INTEGER NOT NULL DEFAULT 1,
  PRIMARY KEY (host_alias, catalog_id, layer_name)
);
INSERT OR IGNORE INTO host_layers_new (host_alias, catalog_id, layer_name, axis, position, active)
  SELECT hl.host_alias, c.id, hl.layer_name, hl.axis, hl.position, hl.active
  FROM host_layers hl JOIN catalogs c ON c.org_id IS NULL;
DROP TABLE host_layers;
ALTER TABLE host_layers_new RENAME TO host_layers;
CREATE UNIQUE INDEX IF NOT EXISTS idx_host_active_role
  ON host_layers(host_alias, catalog_id) WHERE axis = 'role' AND active = 1;

ALTER TABLE asset_inventory ADD COLUMN catalog_id INTEGER REFERENCES catalogs(id) ON DELETE SET NULL;
UPDATE asset_inventory SET catalog_id = (SELECT id FROM catalogs WHERE org_id IS NULL)
  WHERE state NOT IN ('unmanaged', 'orphan');

INSERT OR IGNORE INTO schema_version (version) VALUES (91);
```

Host-layer rows that exist without any catalog configured are dropped by the `JOIN` (they could not resolve against anything). Register:

```rust
    // Assets S1b M2: catalog_id on host_layers (rebuilt) and asset_inventory.
    Migration {
        version: 91,
        sql: include_str!("../../migrations/091_catalog_ids.sql"),
        already_applied: Some(asset_inventory_has_catalog_id),
    },
```

```rust
fn asset_inventory_has_catalog_id(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('asset_inventory') WHERE name = 'catalog_id'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}
```

`store/layers.rs`: add `catalog_id` to `COLS`, `row_from` and `HostLayerRow`; `get_host_layers` keeps returning every catalog's active rows (order `catalog_id, axis, position, layer_name`); add `get_host_layers_for` (`… AND catalog_id=?2`); move the current body of `set_host_layers` into `set_host_layers_for(host, catalog_id, role, contexts)`, scoping its DELETE and INSERTs to `catalog_id`; `set_host_layers(host, role, contexts)` becomes: look up `personal_catalog()` (error `QueryReturnedNoRows`-style if none, matching how the store reports a missing row elsewhere) and call `set_host_layers_for`. Update the `host_layers` insert in the `layers.rs` test that inserts raw rows to include `catalog_id`.

`store/rows.rs` + `store/catalog.rs`: `AssetInventoryRow` gains `#[serde(default)] pub catalog_id: Option<i64>`; `replace_host_inventory` inserts it; `list_inventory` selects it. Fix every `AssetInventoryRow { … }` literal the compiler reports (`catalog_id: None`, or `..Default::default()`).

- [ ] **Step 4: Run to verify pass** — `cargo test -p fleet-core store::layers`, `cargo test -p fleet-core schema`, `cargo test -p fleet-core store::catalog`, `cargo test -p fleet-core catalog`.

- [ ] **Step 5: Commit** — `git commit -m "feat(catalog): layers and inventory rows carry their catalog (migration 091)"`

---

### Task 2: Origin, a borrowing registry view, and the acceptance rule

**Files:**
- Modify: `crates/fleet-core/src/service/catalog/repo.rs` (`Catalog`)
- Modify: `crates/fleet-core/src/service/catalog/registry.rs`
- Create: `crates/fleet-core/src/service/catalog/effective.rs` (the acceptance part; Task 3 adds the rest)
- Modify: `crates/fleet-core/src/service/catalog/mod.rs` (`pub mod effective;`)

**Interfaces:**
- Produces:

```rust
// repo.rs
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogRef { pub id: i64, pub name: String }
// Catalog gains:
/// `<kind>/<name>` → the catalog that asset came from, on a composed
/// (effective / union) catalog. Empty on a catalog loaded from one repo:
/// then every asset is from `self`.
#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
pub origin: BTreeMap<String, CatalogRef>,
impl Catalog {
    /// Which catalog `kind/name` came from: `origin`, else this catalog itself.
    pub fn origin_of(&self, kind: Kind, name: &str) -> CatalogRef;
}

// registry.rs
pub fn with_catalogs<T>(f: impl FnOnce(&BTreeMap<i64, Catalog>) -> Result<T, IpcError>) -> Result<T, IpcError>;
/// Every loaded catalog merged: personal first, then by name; on a
/// (kind, name) clash the first wins. `origin` filled. `None` when nothing
/// is loaded.
pub fn union_all() -> Result<Option<Catalog>, IpcError>;

// effective.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Acceptance { No, SharedOnly, All }
/// What a host with `host_org` may take from a catalog owned by
/// `catalog_org` (None = personal), given the host's admissions (M3; empty now).
pub fn acceptance(host_org: Option<i64>, catalog_id: i64, catalog_org: Option<i64>, admitted: &[i64]) -> Acceptance;
```

Rule (spec, "Which catalogs a host accepts"):
- personal catalog: host with no org → `All`; host with an org → `SharedOnly`.
- org catalog X: host with org X → `All`; host with no org and `catalog_id` in `admitted` → `All`; else `No`.

- [ ] **Step 1: Write the failing tests** (`effective.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    const PERSONAL: i64 = 1;
    const ACME: i64 = 2;
    const ACME_ORG: i64 = 10;

    #[test]
    fn a_host_without_an_org_takes_all_of_personal_and_only_admitted_org_catalogs() {
        assert_eq!(acceptance(None, PERSONAL, None, &[]), Acceptance::All);
        assert_eq!(acceptance(None, ACME, Some(ACME_ORG), &[]), Acceptance::No);
        assert_eq!(acceptance(None, ACME, Some(ACME_ORG), &[ACME]), Acceptance::All);
    }

    #[test]
    fn an_org_host_takes_its_org_catalog_and_only_shared_personal_assets() {
        assert_eq!(acceptance(Some(ACME_ORG), PERSONAL, None, &[]), Acceptance::SharedOnly);
        assert_eq!(acceptance(Some(ACME_ORG), ACME, Some(ACME_ORG), &[]), Acceptance::All);
        assert_eq!(acceptance(Some(99), ACME, Some(ACME_ORG), &[ACME]), Acceptance::No, "admission never crosses orgs");
    }
}
```

`registry.rs` tests (take `lock_registry_for_test()`): `union_all` of `personal` (assets a, b) and `acme` (assets b, c) yields a, b (from personal), c (from acme) with `origin_of(skill,"b").name == "personal"` and `origin_of(skill,"c").name == "acme"`; `with_catalogs` sees both ids.

- [ ] **Step 2: Run to verify failure** — `cargo test -p fleet-core catalog::effective`, `cargo test -p fleet-core catalog::registry`.

- [ ] **Step 3: Implement**

```rust
// effective.rs
pub fn acceptance(host_org: Option<i64>, catalog_id: i64, catalog_org: Option<i64>, admitted: &[i64]) -> Acceptance {
    match (catalog_org, host_org) {
        (None, None) => Acceptance::All,
        (None, Some(_)) => Acceptance::SharedOnly,
        (Some(c), Some(h)) if c == h => Acceptance::All,
        (Some(_), None) if admitted.contains(&catalog_id) => Acceptance::All,
        _ => Acceptance::No,
    }
}
```

`repo.rs`: `CatalogRef`, `origin`, and

```rust
    pub fn origin_of(&self, kind: Kind, name: &str) -> CatalogRef {
        self.origin
            .get(&format!("{}/{name}", kind.as_str()))
            .cloned()
            .unwrap_or_else(|| CatalogRef { id: self.id, name: self.name.clone() })
    }
```

`registry.rs`: `with_catalogs` takes the read lock and passes the map; `union_all` builds a `Catalog { id: 0, name: "".into(), .. }` whose `assets` are the concatenation in order (personal first, then by name), skipping an asset whose `(kind, name)` is already present, filling `origin` for every asset, `head` = the personal catalog's head (so existing callers that read `head` see what they saw before), `layers` = the personal catalog's layers, `problems` = concatenated.

- [ ] **Step 4: Run to verify pass** — the two filters above, then `cargo test -p fleet-core catalog`.

- [ ] **Step 5: Commit** — `git commit -m "feat(catalog): asset origin, a borrowing registry view and the catalog acceptance rule"`

---

### Task 3: The effective catalog for a host

**Files:**
- Modify: `crates/fleet-core/src/service/catalog/effective.rs`
- Modify: `crates/fleet-core/src/service/catalog/sync/layers.rs` (factor the chain building out of `resolve_for_host`)
- Modify: `crates/fleet-core/src/service/catalog/resolve.rs` (`apply_override` rejects `scope`; `Provenance.catalog`)

**Interfaces:**
- Consumes: `Store::get_host_layers_for`, `Store::host_org`, `Store::list_catalogs`, `registry::with_catalogs`, `acceptance`, `Catalog.origin`.
- Produces:

```rust
// effective.rs
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refusal { pub kind: String, pub name: String, pub reason: String }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectiveSet {
    /// What the host should have: every accepted catalog's resolved assets,
    /// merged, minus refusals. `origin` says which catalog each came from.
    pub catalog: Catalog,
    /// `<kind>/<name>` → provenance (layer + catalog).
    pub provenance: BTreeMap<String, Provenance>,
    /// Assets that would have gone to this host but may not: scope boundary
    /// or a collision between catalogs. The planner turns each into a
    /// `blocked` action with this reason.
    pub refused: Vec<Refusal>,
    /// Whether any accepted catalog has layers assigned on this host.
    pub layered: bool,
}

pub fn effective_for_host(store: &Mutex<Store>, host_alias: &str) -> Result<EffectiveSet, IpcError>;

// sync/layers.rs — the existing per-catalog logic, reusable:
pub fn resolve_rows(catalog: &Catalog, host_alias: &str, rows: &[HostLayerRow]) -> Result<Resolution, IpcError>;
// resolve_for_host(store, catalog, host) stays (M1 callers) and becomes:
//   rows = store.get_host_layers_for(host, catalog.id)  (personal id when catalog.id == 0 → personal)
//   resolve_rows(catalog, host, &rows)

// resolve.rs — Provenance gains  #[serde(default)] pub catalog: String,
```

Algorithm of `effective_for_host`:
1. **Store phase** (one guard, dropped before step 2): `host_org(host)`, `list_catalogs()`, and for each catalog `get_host_layers_for(host, id)`. Admissions: `&[]` (M3).
2. **Registry phase** (`with_catalogs`): for each loaded catalog in order (personal first, then by name) whose `acceptance(...) != No`:
   - `res = resolve_rows(catalog, host, rows_for_this_catalog)` (a catalog with no rows for this host resolves to its whole content, as today).
   - For each asset in `res.catalog.assets`: if acceptance is `SharedOnly` and `asset.header.scope == Private` → push `Refusal { reason: "private asset \"<name>\" (catalog <catalog>, layer <res.provenance[key].introduced_by or 'none'>) may not go to org host <host>; mark it shared or remove it from the layer" }` and skip; else collect `(kind, install_name) → (asset, CatalogRef, provenance)`.
   - Only a **no-layers** personal catalog on an org host would otherwise hand the whole personal catalog to it: in that case (`SharedOnly` and no rows) keep only `shared` assets silently (no refusals) — it is the unlayered default, not a mistake in a layer.
3. **Collisions:** any `(kind, install_name)` collected from two or more catalogs → remove all of them and push one `Refusal` per catalog's copy: `conflict: <catalogA>/<name> vs <catalogB>/<name> — use install_as or move one`.
4. Build `catalog` (`id: 0`, `name: ""`, `head`/`layers` of personal, assets in order, `origin` filled) and `provenance` (with `catalog`), `layered` = any accepted catalog had rows.

- [ ] **Step 1: Write the failing tests** (`effective.rs`; build catalogs by hand, `registry::install_personal` / a new test helper `registry::install_for_test(cat)` that inserts any catalog by id; seed the store with orgs, a second catalog row, host org and layers as in Task 1's test; take `lock_registry_for_test()`)

```rust
#[test]
fn only_personal_behaves_as_before() {
    // personal with skills a, b; host with no org, no layers
    // → effective assets [a, b], no refusals, layered false,
    //   origin_of(a).name == "personal"
}

#[test]
fn an_org_host_gets_shared_personal_assets_and_its_org_catalog() {
    // personal: a (private), b (shared); acme (org 10): c; host org 10, no layers
    // → effective [b, c]; no refusals (unlayered personal on an org host keeps shared silently)
}

#[test]
fn a_private_asset_in_a_layer_assigned_to_an_org_host_is_refused_with_a_reason() {
    // personal layer "core" members [a (private), b (shared)]; host org 10 with role core in personal
    // → effective [b] (+ acme's), refused contains a with reason containing "private asset \"a\"" and "mark it shared"
}

#[test]
fn a_collision_between_catalogs_refuses_both_copies() {
    // personal: x (shared); acme: x; host org 10
    // → effective has no x; refused has two entries, reasons contain "conflict: personal/x vs acme/x"
}

#[test]
fn an_org_catalog_never_reaches_a_host_of_another_org_or_no_org() {
    // acme (org 10): c; host org 11 → no c; host no org → no c (no admissions in M2)
}
```

Write each test fully (the comments above state the fixture and the exact assertions); use `Asset::from_yaml(Some(Kind::Skill), "kind: skill\nname: a\ndescription: d\nscope: shared\n")` style fixtures and `Layer::from_yaml` as in `sync/layers.rs` tests.

`resolve.rs` test:

```rust
#[test]
fn an_override_may_not_change_scope() {
    // a layer override `skill/a: { scope: shared }` on a private asset → apply_override Err containing "scope"
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p fleet-core catalog::effective`, `cargo test -p fleet-core catalog::resolve`.

- [ ] **Step 3: Implement** as described. In `apply_override`, after the `install_as` check:

```rust
    if patched.header.scope != asset.header.scope {
        return Err(format!(
            "an override may not change an asset's scope ({:?} to {:?})",
            asset.header.scope, patched.header.scope
        ));
    }
```

Never call `lock(store)` inside a `with_catalogs` closure.

- [ ] **Step 4: Run to verify pass** — the filters above, `cargo test -p fleet-core catalog`.

- [ ] **Step 5: Commit** — `git commit -m "feat(catalog): the effective catalog for a host — acceptance, scope boundary, collisions"`

---

### Task 4: Plan, scan, apply and preview on the effective catalog

**Files:**
- Modify: `crates/fleet-core/src/service/catalog/sync/mod.rs` (`plan_sync`, `catalog()`, `scan_and_persist`)
- Modify: `crates/fleet-core/src/service/catalog/sync/plan.rs` (`Action.catalog`; blocked actions from refusals)
- Modify: `crates/fleet-core/src/service/catalog/sync/manifest.rs` (`ManifestEntry.catalog`; `entry_for`)
- Modify: `crates/fleet-core/src/service/catalog/sync/apply.rs` (write the entry's catalog)
- Modify: `crates/fleet-core/src/service/catalog/inventory.rs` (`scan_hosts` on `union_all`; `compute_states` stamps `catalog_id`)
- Modify: `crates/fleet-core/src/service/catalog/mod.rs` (`resolve_preview` via `effective_for_host`)

**Interfaces:**
- Consumes: `effective_for_host`, `EffectiveSet`, `registry::union_all`, `Catalog::origin_of`.
- Produces:
  - `Action` gains `#[serde(default, skip_serializing_if = "Option::is_none")] pub catalog: Option<String>`
  - `ManifestEntry` gains `#[serde(default = "personal")] pub catalog: String` (`fn personal() -> String { "personal".into() }`)
  - `AssetInventoryRow.catalog_id` filled for catalog-asset rows (`None` for unmanaged/orphan)

Wiring:
1. `plan_sync`: replace `layers::resolve_for_host(store, &catalog, &h.alias)` with `effective::effective_for_host(store, &h.alias)` (it already does store-then-registry; call it where the old call was — no store guard held there). Use `eff.layered` in `refuse_unlayered`, and `eff.catalog` for `compute_host_plan`. Then, for every `eff.refused` entry that `filter.matches(kind, name)`, push an `Action { op: ActionOp::Blocked, reason: Some(refusal.reason), catalog: …, ..Default }` onto that harness's `HostPlan` (use whatever constructor/default the planner already uses for `Blocked`). Set `action.catalog = Some(eff.catalog.origin_of(kind, name).name)` on every action `compute_host_plan` produced for a catalog asset.
2. `sync::catalog()` (the full catalog the scan diffs against) → `registry::union_all()` with the same "not loaded" error.
3. `inventory::scan_hosts` → `registry::union_all()` instead of `personal()`; `compute_states` sets `catalog_id: Some(catalog.origin_of(kind, name).id)` on catalog-asset rows (for a single loaded catalog `origin_of` returns that catalog's own id — the personal id).
4. `ManifestEntry.catalog`: `Manifest::entry_for` takes the catalog name (add a parameter; update its callers in plan.rs / apply.rs from the action's `catalog`, defaulting to `"personal"`). An old manifest without the field reads as `personal`.
5. `resolve_preview` (mod.rs): build from `effective_for_host` so provenance shows the catalog; keep its response type (add fields only, `#[serde(default)]`).

- [ ] **Step 1: Write the failing tests**

In `sync/mod.rs` tests (existing helpers `one_skill`, `load_catalog`, `store_with_local`, `RecordingEventBus`, `CATALOG_TEST_LOCK` → now `lock_registry_for_test()`):

```rust
#[tokio::test]
async fn plan_records_each_actions_catalog() {
    // personal catalog with skill s, local unlayered → the create action has catalog == Some("personal")
}

#[tokio::test]
async fn a_refused_asset_is_a_blocked_action_with_its_reason() {
    // install a second catalog "acme" (org X) with skill s, and personal with shared skill s;
    // make local org-bound to X (UPDATE hosts SET org_id) → the plan for local has a Blocked action for s
    // whose reason contains "conflict: personal/s vs acme/s"
}
```

In `manifest.rs` tests: an old manifest JSON without `catalog` deserializes with `catalog == "personal"`; `entry_for(.., "acme")` records `acme`.

In `inventory.rs` tests: `compute_states` on a single catalog stamps its id on managed rows and `None` on unmanaged rows.

Existing tests must keep passing unchanged in outcome (the `personal`-only equivalence).

- [ ] **Step 2: Run to verify failure** — `cargo test -p fleet-core catalog::sync`, `cargo test -p fleet-core catalog::inventory`.

- [ ] **Step 3: Implement** the wiring above.

- [ ] **Step 4: Run to verify pass** — `cargo test -p fleet-core catalog`, `cargo test -p fleet-core`, `cargo test -p claude-fleet --lib`, `cargo build -p fleet-hub`.

- [ ] **Step 5: Commit** — `git commit -m "feat(sync): plan, scan and apply against the effective catalog of each host"`

---

### Task 5: Verification and docs

- [ ] **Step 1:** `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace --no-fail-fast`; `pnpm test`; `pnpm check`. Expected: green except the known failures.
- [ ] **Step 2:** the four generators (`REGEN_DOCS`, `REGEN_SETTINGS_DOCS`, `REGEN_PAGE_DOCS`, `REGEN_HUB_VERDICTS`) — expect the control-API reference to change only if an MCP-visible type gained fields; commit any regenerated file naming its generator; `git status` clean after.
- [ ] **Step 3:** `CLAUDE.md` — one paragraph after the M1 one: migration 091 (`catalog_id` on `host_layers` / `asset_inventory`), `effective.rs` (`acceptance`, `effective_for_host`: store first, then registry), per-asset `blocked` for scope-boundary and collisions, `Action.catalog` / `ManifestEntry.catalog`, `union_all` for scans, overrides cannot change `scope`. Point at this plan. `docs/hub.md` "Asset catalog": one sentence that a host bound to an org receives only `shared` assets of the personal catalog.
- [ ] **Step 4:** commit `docs: sync across catalogs (M2)`.

---

## Self-review against the spec (M2)

| Spec item | Task |
|---|---|
| `host_layers.catalog_id` (PK per catalog, one role per catalog) | 1 |
| `asset_inventory.catalog_id` for managed rows | 1, 4 |
| manifest `catalog` (old manifests = personal) | 4 |
| effective set = ⋃ accepted catalogs' resolutions, filtered by scope | 2, 3 |
| private never to an org host; org asset only to accepting hosts | 2, 3 |
| layer that would break the boundary → reason (per asset, see Rulings) | 3, 4 |
| collision blocks both sides with the reason | 3, 4 |
| unlayered guard per host across catalogs | 4 (`eff.layered`) |
| `with_catalog(id, f)` / borrowing registry API | 2 (`with_catalogs`) |
| layers must not override `scope` | 3 |
| admissions | M3 (empty in M2) |
