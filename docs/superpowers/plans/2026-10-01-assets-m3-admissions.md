# Assets M3: admissions, per-catalog grants and per-catalog loading — Implementation Plan

> **Renumbered:** `main` took migration 092 (`092_host_provision_warning.sql`) before this landed, so this plan's migration 092 shipped as **093** (`093_catalog_access.sql`). Read every "092" below as 093.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a host with no org admit an org catalog, let the operator grant a paired client one catalog at a time, and load every configured catalog (keeping a broken one as a problem entry), so that org catalogs can be added, listed, removed, admitted and synced from the hub CLI and `catalog_admin` — without any sync ever removing an asset whose catalog it cannot speak for.

**Architecture:** Migration 092 adds `host_catalogs` (admissions) and `client_catalog_grants` (grants per catalog, personal backfilled from `assets_admin_at`, which stays as the personal grant's mirror). `load_catalog(id)` loads any catalog; `ensure_fresh` walks every `catalogs` row, keeps an org catalog that cannot load as a registry *problem entry* (`Catalog.load_error`), and evicts removed ones. `effective_for_host` reads admissions in its store phase and reports which catalogs *speak for* the host and which are *held back* (not loaded, failed, not accepted); the planner only plans an orphan `Remove` for a manifest entry whose catalog speaks, and reports every other one as a `Noop` with the reason. A new `service/catalog/catalogs.rs` manages the set of catalogs and admissions; `catalog_admin` gains an optional `catalog` parameter, five actions and a grant check per touched catalog; `fleet-hub catalog add|list|remove|admit|unadmit` and `client grant|ungrant … --catalog` are its operator side.

**Tech Stack:** Rust (fleet-core, rusqlite, rmcp; fleet-hub with clap; one line in src-tauri).

**Spec:** `docs/superpowers/specs/2026-09-30-assets-s1b-s2-design.md` — milestone **M3** (Milestones row: "`host_catalogs`, `client_catalog_grants`, `load(id)` / per-catalog `ensure_fresh`"), plus the sections *Data model* (DDL of both tables), *Migration* step 3, *Which catalogs a host accepts*, *Runtime*, *Hub CLI and MCP* and *Testing* (store: admissions, grants per catalog; planning: "a no-org host receives an org asset only when admitted"; authorization). Builds on M1 (PR #414) and M2 (PR #416), both merged. Changesets (M4) and every UI piece (M5/M6) are out of scope.

## Global Constraints

- Migration number **092** (`092_catalog_access.sql`). 091 (`091_catalog_ids.sql`, M2) is the latest on `main`. If `main` has gained a migration by the time this lands, renumber and re-check.
- The DDL is the spec's, verbatim in shape: `host_catalogs (host_alias → hosts(alias) ON DELETE CASCADE, catalog_id → catalogs(id) ON DELETE CASCADE, admitted_at, PK (host_alias, catalog_id))`; `client_catalog_grants (client_id → client_tokens(id) ON DELETE CASCADE, catalog_id → catalogs(id) ON DELETE CASCADE, granted_at, PK (client_id, catalog_id))`.
- Spec, *Migration* step 3: "Every client with `assets_admin_at` gets a `client_catalog_grants` row on `personal`. `assets_admin_at` stays but nothing reads it." (See Rulings R2 for the one narrow exception.)
- Spec, *Which catalogs a host accepts*: "A host with `org_id = X`: catalog(s) of org X, plus the `shared` assets of `personal`. A host with no org: `personal` (all scopes) plus every catalog in its `host_catalogs` admissions. `local` on a hub follows the same rule."
- Spec, *Runtime*: "A catalog whose repo cannot be loaded is kept as a problem entry; the others load."
- Spec, *Hub CLI and MCP*: "`fleet-hub catalog add <name> <path> [--remote URL] [--org NAME]`, `catalog list`, `catalog remove <name>` (config only; never deletes the repo), `catalog admit|unadmit <host> <catalog>`. `catalog set` stays as the alias for `personal`." / "`fleet-hub client grant <name> assets [--catalog NAME]` (default `personal`)" / "MCP `catalog_admin` actions take an optional `catalog` (default `personal`); new actions `list_catalogs`, `add_catalog`, `remove_catalog`, `admit_catalog`, `unadmit_catalog`." / "Every mutating action checks the caller's grant **for the catalogs it touches** (`may_admin_catalog(caller, catalog_id)`); per-host tokens never pass. `list_assets` and the inventory stay readable as today."
- Spec, *Out of scope*: "automatic overwrite / remove under any mode." → losing a catalog (not loaded, failed, unadmitted, org change, removed) never plans a `Remove`.
- Lock rule: **registry → store is allowed; store → registry never.** Read store rows under one guard, drop it, then take the registry. Never call `lock(store)` inside a `registry::with_*` closure.
- Never hold the `Store` guard across an `.await`.
- New serialized fields are `#[serde(default)]` (hub ↔ desktop wire, manifests on hosts).
- A fleet with only `personal` and no org-bound hosts plans, scans, syncs and previews exactly as before, and `catalog_admin` answers the master and a granted client exactly as before.
- Shell-quoting only through `crate::shell::quote`; child processes only through `fleet_core::proc::command` / `std_command` (tests that run `git` in fleet-hub use `fleet_core::proc::std_command`).
- `cargo test` takes ONE name filter per command. Known unrelated failures: `rewind::tests::the_removal_script_leaves_a_tree_a_live_pane_is_in` (socket path); `work::scale_tests::*` (perf under load).
- Tests that touch the catalog registry or `HOME` take `crate::service::catalog::lock_registry_for_test()`.
- Generated files: `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current` after any `#[tool(...)]` description or params doc change; `REGEN_SETTINGS_DOCS=1 cargo test -p fleet-core settings_docs_are_current`; `REGEN_PAGE_DOCS=1 cargo test -p fleet-core page_docs_are_current`; `REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen`.
- Git: branch `feat/assets-m3-admissions` from `main` (which has M2); commit per task; never rebase, never force-push; merge `origin/main` for conflicts.

## Rulings

Where the spec is silent or the M2 carry list asked for a decision. Each ruling states its cost if it turns out wrong.

- **R1 — Harden 091 in place (carry 6).** 091 reached `main` with M2 (PR #416) but no release has carried it yet, and the edit is inert for any database already at 91; so 091 itself gains a leading `CREATE TABLE IF NOT EXISTS host_layers (<033 shape>)`; 092 cannot fix it, since 091 runs first. A database already at 91 skips 091 (`asset_inventory_has_catalog_id` guard), so the edit is inert there. *Cost if wrong:* none on an applied DB; on a DB that skipped 033/035 the migration stops failing.
- **R2 — Where the personal grant lives.** A personal grant is a `client_catalog_grants` row on `personal`, mirrored into `client_tokens.assets_admin_at` (so a downgraded hub keeps it). While no `personal` row exists yet (fresh hub, catalog not configured), `client grant <name> assets` can only record the mirror; `client_is_assets_admin` honours that pending mirror only while no personal catalog exists, and `set_catalog_config` moves it into a grant row (same SQL as 092's backfill). Without this, a granted client could no longer `configure` a fresh hub's catalog, which it can today. *Cost if wrong:* one column read in one SQL predicate.
- **R3 — Grant eligibility is unchanged and the same for every catalog:** a live `full` client bound to no org (never a peer link, never `readonly`). An org-bound client is not granted even its own org's catalog. *Cost if wrong:* an org's operator needs an unbound device; relaxing it later is additive.
- **R4 — Admissions are for hosts with no org.** `admit` refuses an org-bound host and the `personal` catalog; an admission row left on a host that later joins an org is kept but ignored by `acceptance` (which already refuses to cross orgs). `unadmit` works on any host. *Cost if wrong:* a stale row, removable with `unadmit`.
- **R5 — Problem entries.** An org catalog that fails to load becomes a registry entry with `load_error: Some(msg)`, no assets, one `Problem`, and the store's load record as its stamp; `ensure_fresh` does not retry it until that record changes (`catalog reload`, `catalog add` re-point, another process's load), so a broken checkout never costs a git run on every MCP call. An explicit `load_catalog` always tries. `personal` never becomes a problem entry: a personal failure is returned by `ensure_fresh` as today (after the org catalogs were attempted). *Cost if wrong:* a transient failure (network mount) stays a problem until someone reloads; it is visible in `catalog list`.
- **R6 — Losing a catalog is a `Noop`, never a `Remove` (carries 1, 2, 3).** `EffectiveSet` gains `speaks_for` (catalogs composed for this host) and `held_back` (catalog → reason: not loaded / failed to load / not accepted by this host / cannot resolve). `Manifest::orphans` takes `speaks_for`; `Manifest::held` lists the rest; the planner reports each held entry as a `Noop` naming its catalog and the reason ("… its assets are kept, not removed"); an entry naming a catalog that is not configured at all gets "is not configured on this fleet". `Noop`, not `Blocked`: nothing about it blocks the rest of the plan, and it is the same shape as M2's withheld `Noop`. *Cost if wrong:* stale copies linger, visibly, until a person removes them (M4/M6 cards).
- **R7 — An org catalog that cannot resolve for a host is held back, not fatal** (revisits M2's ruling now that org catalogs exist). `personal` keeps failing the host (M1/M2 parity). *Cost if wrong:* a typo'd org layer silently stops that org's updates on that host — but the `Noop`s and `resolve_preview.held_back` say why.
- **R8 — Inventory keeps calling such entries `orphan`.** `compute_states` passes `speaks_for = None`: the inventory diffs against the union of loaded catalogs and decides nothing; only the plan decides removal. *Cost if wrong:* the matrix shows "orphan" for an entry the plan keeps; M5 can add a distinct state.
- **R9 — One registry snapshot per `plan_sync` (carry 5).** `plan_sync` takes `registry::snapshot()` once, derives the scan union with `registry::union_of(&snapshot)` and composes every host with `effective::effective_for_host_in(store, host, &snapshot)`. *Cost if wrong:* one clone of every loaded catalog per plan (the union already cloned them all).
- **R10 — What the `catalog` parameter addresses in M3.** Honoured by `config`, `load`, `list_layers`, `set_host_layers` (and the five new actions name their catalog in `args`). The authoring actions (`configure`, `get_asset`, `template`, `create_asset`, `update_asset`, `delete_asset`, `add_resource_bytes`, `remove_resource`, `lint_asset`, `lint_all`, `commit_pending`, `push`, `repo_status`, `layer_template`, `write_layer`, `delete_layer`, `import_host`) stay personal-only until M4, whose changeset apply needs the same per-catalog repo target; a non-personal `catalog` there is `E_INVALID` naming M4 and the git + `catalog reload --catalog` workaround. The fleet-wide actions (`inventory`, `plan_sync`, `apply_sync`, `last_sync`, `list_secrets`, `set_secret`, `delete_secret`, `resolve_preview`, `propose_layers`, `set_host_harnesses`) refuse a non-personal `catalog` as "not per catalog". *Cost if wrong:* org catalogs are edited in git for one milestone.
- **R11 — The gate per action.** `list_catalogs` needs no grant (any caller that reaches `catalog_admin`: per-host tokens are already refused centrally by `NOT_FOR_HOST_TOKENS`, readonly clients by the mode gate). `add_catalog` is master-only (no grant can name a catalog that does not exist yet, and it points the hub at a path on its machine). `remove_catalog`, `admit_catalog`, `unadmit_catalog` need the grant on the catalog they name. Every other action needs the grant on its catalog (`personal` unless the parameter says otherwise). `apply_sync` additionally needs the grant on every catalog its parked plan writes from (`Action.catalog` of every non-`Noop`/`Blocked` action, `ManifestEntry.catalog` of every `Remove`). A call that does not parse is `E_INVALID` for every caller (the action must be known to know what it touches). *Cost if wrong:* a client granted only personal cannot apply a plan that writes acme assets — which is the point.
- **R12 — CLI verbs.** The spec's "`client revoke … --catalog`" is the existing take-back verb `client ungrant <name> assets --catalog NAME`; `client revoke` keeps revoking tokens. `catalog reload` gains `--catalog NAME` (default personal). `catalog add` on an existing name re-points it (as `catalog set` does for personal); moving a catalog to another org is refused (remove, then add). `catalog add personal <path>` is `catalog set <path>`. *Cost if wrong:* an alias later.
- **R13 — `catalog remove` cascades (carry 8, FK).** The spec's DDL already says `ON DELETE CASCADE` for both new tables, and 091 declared `host_layers.catalog_id … ON DELETE CASCADE` and `asset_inventory.catalog_id … ON DELETE SET NULL`. `personal` cannot be removed. The removal reports how many layer assignments, admissions and grants went, evicts the registry entry, never touches the checkout, and hosts keep what it installed (R6). *Cost if wrong:* re-adding a catalog means re-assigning layers, admissions and grants.
- **R14 — No new Tauri commands in M3.** The admissions/catalogs UI is M6; the five verdict rows arrive with those commands. The only desktop change: the local `catalog_load` calls `catalog::load_all` so a standalone desktop loads its org catalogs too. *Cost if wrong:* M6 adds five rows and runs `REGEN_HUB_VERDICTS`.
- **R15 — `duplicate_from` makes a private copy (carry 8).** A copy is a new asset; nothing becomes shared without an explicit mark (SB1). *Cost if wrong:* the author re-marks the copy shared.
- **R16 — `lint_in_repo` / `lint_everything` borrow, never clone (carry 8 "lint clones").** Behaviour-preserving refactor over `registry::with_personal`; the existing lint tests are its regression net.
- **R17 — `list_assets` stays personal-only in M3.** `AssetSummary` has no catalog field yet (M5's badge brings it). *Cost if wrong:* org assets are visible through `resolve_preview`, `plan_sync` and `catalog list` only until M5.
- **R18 — `load` with no catalog = `load_all`:** personal (pulled if asked) and every other catalog caught up by `ensure_fresh`; `pull` applies to the named catalog only. *Cost if wrong:* an org catalog's pull needs `load --catalog acme --pull`.
- **R19 — Grants are listed per catalog by `catalog list` (GRANTED column); `client list` keeps its ASSETS column = the personal grant (its mirror).** *Cost if wrong:* one more command to see an org grant.
- **R20 — Test (b) gets a real scan (carry 9).** A fake `ssh` binary (the `tools_over_fake_ssh` pattern, `crate::tmux::fake_exec`) runs the remote scan locally against a temp `HOME`, so the remote host is planned, not skipped. Cheap: no new transport code.
- **R21 — Removing an org that still owns a catalog** stays refused by `catalogs.org_id … ON DELETE RESTRICT` with SQLite's message; a friendlier message is out of scope. *Cost if wrong:* an operator reads a constraint error and runs `catalog remove` first.

## M2 carry list → where it lands

| # | Carry | Lands in |
|---|---|---|
| 1 | No orphan `Remove` for an entry whose catalog is not loaded / failed to load | Task 2 (problem entries), Task 3 (`speaks_for`, held `Noop`) |
| 2 | Lost acceptance (org change, unadmit) never auto-removes | Task 3 (R6) |
| 3 | `Manifest::orphans` considers `entry.catalog` | Task 3 |
| 4 | Skip the withheld `Noop` when another catalog supplies the name | Task 3 |
| 5 | One catalog snapshot for plan and scan | Task 3 (R9) |
| 6 | 091 assumes `host_layers` exists | Task 1 (R1) |
| 7 | `list_layers` / `set_host_layers` mix catalogs | Task 4 |
| 8 | `duplicate_from` scope; lint clones; FK during catalog removal | Task 4 (R15, R16); Task 1 (R13) |
| 9 | Test (b) was vacuous | Task 3 (R20) |

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/fleet-core/migrations/092_catalog_access.sql` (new) | `host_catalogs`, `client_catalog_grants`, personal backfill, epoch triggers |
| `crates/fleet-core/migrations/091_catalog_ids.sql` | leading `CREATE TABLE IF NOT EXISTS host_layers` (R1) |
| `crates/fleet-core/src/store/{schema,catalog,clients,rows,hosts_accounts}.rs` | registration; catalog rows by name / upsert / remove; admissions; grants; `CatalogRemoval`; merge carries admissions |
| `crates/fleet-core/src/service/catalog/repo.rs` | `Catalog.load_error` |
| `crates/fleet-core/src/service/catalog/registry.rs` | `union_of`, `snapshot`, `remove`, `evict_org_catalogs_not_in`, `with_catalog_row`, `heads_key` |
| `crates/fleet-core/src/service/catalog/mod.rs` | `load_catalog`, `load`, `load_all`, `ensure_fresh`, problem entries; `list_layers_for`, `set_host_layers_for`; `resolve_preview.held_back` |
| `crates/fleet-core/src/service/catalog/scan_tick.rs` | rescan when any catalog's HEAD moves |
| `crates/fleet-core/src/service/catalog/effective.rs` | admissions, `speaks_for`, `held_back`, `effective_for_host_in` |
| `crates/fleet-core/src/service/catalog/resolve.rs` | `Resolution.held_back` |
| `crates/fleet-core/src/service/catalog/sync/{manifest,plan,mod}.rs`, `inventory.rs`, `sync/apply.rs` (tests) | `orphans`/`held`; `KeepRules`, held `Noop`, `registry_catalogs_written`; snapshot wiring |
| `crates/fleet-core/src/service/catalog/catalogs.rs` (new) | list / add / remove catalogs; admit / unadmit |
| `crates/fleet-core/src/service/catalog/author.rs` | `duplicate_from` → private; lint borrows |
| `crates/fleet-core/src/service/catalog/admin.rs` | five actions, `Touches`, `touches()`, `run(call, catalog, …)` |
| `crates/fleet-core/src/mcp/tools/{assets,params,tests_catalog_admin}.rs` | `catalog` param, per-catalog gate |
| `crates/fleet-hub/src/{catalog,main,pair}.rs` | `catalog add|list|remove|admit|unadmit`, `reload --catalog`, `client grant|ungrant --catalog` |
| `src-tauri/src/commands/assets.rs` | local `catalog_load` → `load_all` |
| `CLAUDE.md`, `docs/hub.md`, `docs/control-api-reference.md` (generated) | docs |

---

### Task 1: Admissions and per-catalog grants in the store (migration 092)

**Files:**
- Create: `crates/fleet-core/migrations/092_catalog_access.sql`
- Modify: `crates/fleet-core/migrations/091_catalog_ids.sql` (prepend one statement)
- Modify: `crates/fleet-core/src/store/schema.rs` (MIGRATIONS entry; tests; one existing test's setup)
- Modify: `crates/fleet-core/src/store/catalog.rs` (catalog rows by name, upsert, remove, admissions; `set_catalog_config` backfill)
- Modify: `crates/fleet-core/src/store/clients.rs` (grants; `set_client_assets_admin`, `client_is_assets_admin`)
- Modify: `crates/fleet-core/src/store/rows.rs` (`CatalogRemoval`)
- Modify: `crates/fleet-core/src/store/hosts_accounts.rs` (`merge_host_alias` carries `host_catalogs`)

**Interfaces:**
- Consumes: `Store::personal_catalog`, `Store::get_catalog`, `Store::get_org`, `get_client_token_by_id`, `map_client_token_row`, `now_unix`.
- Produces:

```rust
// store/rows.rs
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CatalogRemoval { pub id: i64, pub name: String, pub layer_rows: usize, pub admissions: usize, pub grants: usize }

// store/catalog.rs (impl Store)
pub fn get_catalog_by_name(&self, name: &str) -> Result<Option<CatalogRow>, rusqlite::Error>;
pub fn upsert_catalog(&self, name: &str, repo_path: &str, remote_url: Option<&str>, org_id: Option<i64>) -> Result<CatalogRow, IpcError>;
pub fn remove_catalog(&self, name: &str) -> Result<CatalogRemoval, IpcError>;
pub fn admit_host_catalog(&self, host_alias: &str, catalog_id: i64) -> Result<bool, rusqlite::Error>;   // true = newly admitted
pub fn unadmit_host_catalog(&self, host_alias: &str, catalog_id: i64) -> Result<bool, rusqlite::Error>; // true = was admitted
pub fn host_admissions(&self, host_alias: &str) -> Result<Vec<i64>, rusqlite::Error>;
pub fn catalog_admissions(&self, catalog_id: i64) -> Result<Vec<String>, rusqlite::Error>;

// store/clients.rs (impl Store)
pub fn set_client_catalog_grant(&self, name: &str, catalog_id: i64, on: bool) -> Result<ClientTokenRow, IpcError>;
pub fn client_may_admin_catalog(&self, id: i64, catalog_id: i64) -> Result<bool, rusqlite::Error>;
pub fn catalog_grantees(&self, catalog_id: i64) -> Result<Vec<String>, rusqlite::Error>;
// unchanged signatures, new meaning (R2):
pub fn set_client_assets_admin(&self, name: &str, on: bool) -> Result<ClientTokenRow, IpcError>; // = grant on personal (+ mirror)
pub fn client_is_assets_admin(&self, id: i64) -> Result<bool, rusqlite::Error>;                    // = grant on personal (or pending mirror)
```

- [ ] **Step 1: Write the failing tests**

`store/schema.rs` tests module (next to the 091 tests):

```rust
    /// 092 on a database stopped at 091: every client with `assets_admin_at`
    /// gets a grant on `personal` at that time (spec, Migration step 3); a
    /// client without one gets none; re-running is a no-op.
    #[test]
    fn migration_92_backfills_personal_grants_from_assets_admin_at() {
        let old = store_at_version(91);
        old.conn
            .execute_batch(
                "INSERT INTO catalogs (name, repo_path, org_id, created_at) VALUES ('personal', '/p', NULL, 0);\
                 INSERT INTO client_tokens (name, token_sha256, mode, created_at, assets_admin_at) \
                   VALUES ('desk', 'h1', 'full', 1, 77);\
                 INSERT INTO client_tokens (name, token_sha256, mode, created_at) VALUES ('plain', 'h2', 'full', 1);",
            )
            .unwrap();
        old.migrate().expect("092");
        let grants = |s: &Store| -> Vec<(String, i64)> {
            s.conn
                .prepare(
                    "SELECT t.name, g.granted_at FROM client_catalog_grants g \
                     JOIN client_tokens t ON t.id = g.client_id ORDER BY t.name",
                )
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert_eq!(grants(&old), vec![("desk".to_string(), 77)]);
        old.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 92;")
            .unwrap();
        old.migrate().expect("re-running 092 is safe");
        assert_eq!(grants(&old).len(), 1);
    }

    /// No personal catalog yet: nothing to attach a grant to, so 092 writes
    /// none (the grant waits in `assets_admin_at`, Rulings R2).
    #[test]
    fn migration_92_without_a_personal_catalog_backfills_nothing() {
        let old = store_at_version(91);
        old.conn
            .execute_batch(
                "INSERT INTO client_tokens (name, token_sha256, mode, created_at, assets_admin_at) \
                   VALUES ('desk', 'h1', 'full', 1, 77);",
            )
            .unwrap();
        old.migrate().expect("092");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let n: i64 = old
            .conn
            .query_row("SELECT COUNT(*) FROM client_catalog_grants", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    /// M2 carry 6 / Rulings R1: 091 must not assume `host_layers` exists — a
    /// database from the 033 collision family can reach 091 without it, and
    /// `repair_skipped_main_migrations` (which recreates it) runs only after
    /// every pending migration.
    #[test]
    fn migration_91_recreates_a_missing_host_layers_table() {
        let old = store_at_version(90);
        old.conn.execute_batch("DROP TABLE host_layers;").unwrap();
        old.migrate()
            .expect("091 must not assume host_layers exists");
        let n: i64 = old
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('host_layers') WHERE name = 'catalog_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }
```

`store/catalog.rs` tests module:

```rust
    /// `personal`, host `h`, org `acme` and its catalog. Returns
    /// `(store, org_id, acme_catalog_id)`.
    fn store_with_acme() -> (Store, i64, i64) {
        let s = Store::open_in_memory().unwrap();
        s.set_catalog_config("/p", None).unwrap();
        s.upsert_host("h").unwrap();
        let org = s.add_org("acme", None, false).unwrap();
        let acme = s.upsert_catalog("acme", "/a", None, Some(org.id)).unwrap();
        (s, org.id, acme.id)
    }

    #[test]
    fn upsert_catalog_adds_repoints_and_never_moves_a_catalog_between_owners() {
        use crate::ipc_error::codes::{E_INVALID, E_NOTFOUND};
        let (s, org, acme) = store_with_acme();
        s.set_catalog_head_for(acme, "abc", 5).unwrap();
        let again = s
            .upsert_catalog("acme", "/b", Some("git@x:y.git"), Some(org))
            .unwrap();
        assert_eq!(
            (again.id, again.repo_path.as_str(), again.remote_url.as_deref()),
            (acme, "/b", Some("git@x:y.git"))
        );
        assert_eq!(
            (again.head_commit, again.last_loaded_at),
            (None, None),
            "a re-point clears the load record, as set_catalog_config does"
        );
        let other = s.add_org("other", None, false).unwrap();
        assert_eq!(
            s.upsert_catalog("acme", "/b", None, Some(other.id)).unwrap_err().code,
            E_INVALID,
            "moving a catalog between owners is a remove and an add"
        );
        assert_eq!(
            s.upsert_catalog("beta", "/c", None, None).unwrap_err().code,
            E_INVALID,
            "only personal has no org"
        );
        assert_eq!(
            s.upsert_catalog("personal", "/c", None, Some(org)).unwrap_err().code,
            E_INVALID
        );
        assert_eq!(
            s.upsert_catalog("beta", "/c", None, Some(9999)).unwrap_err().code,
            E_NOTFOUND
        );
        let p = s.upsert_catalog("personal", "/p2", None, None).unwrap();
        assert_eq!(p.repo_path, "/p2");
        assert_eq!(s.get_catalog_by_name("personal").unwrap().unwrap().id, p.id);
    }

    #[test]
    fn admissions_are_per_host_and_catalog_and_go_with_the_host() {
        let (s, _org, acme) = store_with_acme();
        assert!(s.admit_host_catalog("h", acme).unwrap());
        assert!(!s.admit_host_catalog("h", acme).unwrap(), "admitting twice is a no-op");
        assert_eq!(s.host_admissions("h").unwrap(), vec![acme]);
        assert_eq!(s.catalog_admissions(acme).unwrap(), vec!["h".to_string()]);
        assert!(s.unadmit_host_catalog("h", acme).unwrap());
        assert!(!s.unadmit_host_catalog("h", acme).unwrap());
        assert!(s.host_admissions("h").unwrap().is_empty());
        s.admit_host_catalog("h", acme).unwrap();
        s.delete_host("h").unwrap();
        assert!(s.catalog_admissions(acme).unwrap().is_empty());
    }

    /// Rulings R13: removing a catalog is config only — its layer
    /// assignments, admissions and grants go (CASCADE), inventory rows lose
    /// their `catalog_id` (SET NULL), `personal` can never be removed.
    #[test]
    fn removing_a_catalog_drops_its_assignments_admissions_and_grants_but_never_personal() {
        use crate::ipc_error::codes::{E_INVALID, E_NOTFOUND};
        let (s, _org, acme) = store_with_acme();
        s.set_host_layers_for("h", acme, Some("ops"), &["extra"]).unwrap();
        s.set_host_layers("h", Some("core"), &[]).unwrap();
        s.admit_host_catalog("h", acme).unwrap();
        s.insert_client_token("desk", "aa11", "full").unwrap();
        s.set_client_catalog_grant("desk", acme, true).unwrap();
        s.replace_host_inventory(
            "h",
            "claude",
            &[AssetInventoryRow {
                host_alias: "h".into(),
                harness: "claude".into(),
                kind: "skill".into(),
                name: "c".into(),
                state: "in_sync".into(),
                scanned_at: 1,
                managed: true,
                catalog_id: Some(acme),
                ..Default::default()
            }],
        )
        .unwrap();

        let gone = s.remove_catalog("acme").unwrap();
        assert_eq!(
            (gone.id, gone.name.as_str(), gone.layer_rows, gone.admissions, gone.grants),
            (acme, "acme", 2, 1, 1)
        );
        assert!(s.get_catalog_by_name("acme").unwrap().is_none());
        assert_eq!(s.get_host_layers("h").unwrap().len(), 1, "personal's assignment stays");
        assert!(s.host_admissions("h").unwrap().is_empty());
        assert_eq!(s.list_inventory().unwrap()[0].catalog_id, None);
        assert_eq!(s.remove_catalog("personal").unwrap_err().code, E_INVALID);
        assert_eq!(s.remove_catalog("acme").unwrap_err().code, E_NOTFOUND);
    }
```

`store/clients.rs` tests module:

```rust
    /// Migration 092: a grant names one catalog, is read live, and holds
    /// only for a live, `full` client bound to no org (Rulings R3). The
    /// personal grant is mirrored into `assets_admin_at` (R2).
    #[test]
    fn grants_are_per_catalog_and_read_live() {
        use crate::ipc_error::codes::{E_NOTFOUND, E_VALIDATE};
        let s = store();
        s.set_catalog_config("/p", None).unwrap();
        let personal = s.personal_catalog().unwrap().unwrap().id;
        let org = s.add_org("A", None, false).unwrap();
        let acme = s.upsert_catalog("acme", "/a", None, Some(org.id)).unwrap().id;
        let desk = s.insert_client_token("desk", "aa11", "full").unwrap();

        let e0 = s.auth_epoch().unwrap();
        let row = s.set_client_catalog_grant("desk", acme, true).unwrap();
        assert!(s.auth_epoch().unwrap() > e0, "a grant bumps the epoch");
        assert!(row.assets_admin_at.is_none(), "an org grant leaves the personal mirror alone");
        assert!(s.client_may_admin_catalog(desk.id, acme).unwrap());
        assert!(!s.client_is_assets_admin(desk.id).unwrap(), "acme's grant is not personal's");
        assert_eq!(s.catalog_grantees(acme).unwrap(), vec!["desk".to_string()]);

        let p = s.set_client_assets_admin("desk", true).unwrap();
        assert!(p.assets_admin_at.is_some(), "the personal grant is mirrored");
        assert!(s.client_is_assets_admin(desk.id).unwrap());
        assert!(s.client_may_admin_catalog(desk.id, personal).unwrap());

        s.set_client_org("desk", Some(org.id)).unwrap();
        assert!(!s.client_may_admin_catalog(desk.id, acme).unwrap(), "bound: no catalog at all");
        assert!(!s.client_is_assets_admin(desk.id).unwrap());
        s.set_client_org("desk", None).unwrap();

        s.set_client_catalog_grant("desk", acme, false).unwrap();
        assert!(!s.client_may_admin_catalog(desk.id, acme).unwrap());
        assert!(s
            .set_client_assets_admin("desk", false)
            .unwrap()
            .assets_admin_at
            .is_none());
        assert!(!s.client_may_admin_catalog(desk.id, personal).unwrap());

        s.set_client_catalog_grant("desk", acme, true).unwrap();
        s.revoke_client_token("desk").unwrap();
        assert!(!s.client_may_admin_catalog(desk.id, acme).unwrap(), "revoked");

        s.insert_client_token("kiosk", "bb22", "readonly").unwrap();
        let code = |r: Result<_, crate::ipc_error::IpcError>| r.map(|_| ()).unwrap_err().code;
        assert_eq!(code(s.set_client_catalog_grant("kiosk", acme, true)), E_VALIDATE);
        assert_eq!(code(s.set_client_catalog_grant("nobody", acme, true)), E_NOTFOUND);
        s.insert_client_token("eve", "cc33", "full").unwrap();
        assert_eq!(code(s.set_client_catalog_grant("eve", 9999, true)), E_NOTFOUND);
    }

    /// Rulings R2: a personal grant made before any catalog exists waits in
    /// `assets_admin_at` (so a granted client can still configure a fresh
    /// hub's catalog), and configuring `personal` turns it into a grant row.
    #[test]
    fn a_personal_grant_made_before_any_catalog_waits_in_assets_admin_at() {
        let s = store();
        let desk = s.insert_client_token("desk", "aa11", "full").unwrap();
        s.set_client_assets_admin("desk", true).unwrap();
        assert!(s.client_is_assets_admin(desk.id).unwrap(), "pending, but honoured");
        s.set_catalog_config("/p", None).unwrap();
        let personal = s.personal_catalog().unwrap().unwrap().id;
        assert!(s.client_may_admin_catalog(desk.id, personal).unwrap(), "moved into a grant row");
        assert!(s.client_is_assets_admin(desk.id).unwrap());
    }
```

`store/hosts_accounts.rs` tests module:

```rust
    /// Migration 092: a merged host keeps its admissions, like its layer
    /// assignments (M2 fix round 1).
    #[test]
    fn merge_host_alias_carries_admissions() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        s.upsert_host("mac").unwrap();
        s.set_catalog_config("/p", None).unwrap();
        let org = s.add_org("acme", None, false).unwrap();
        let acme = s.upsert_catalog("acme", "/a", None, Some(org.id)).unwrap().id;
        s.admit_host_catalog("local", acme).unwrap();
        s.merge_host_alias("local", "mac").unwrap();
        assert_eq!(s.host_admissions("mac").unwrap(), vec![acme]);
        assert!(s.host_admissions("local").unwrap().is_empty());
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p fleet-core migration_92` — Expected: compile errors (`upsert_catalog`, `client_catalog_grants` unknown).

- [ ] **Step 3: Write the migration, harden 091, register**

`crates/fleet-core/migrations/092_catalog_access.sql`:

```sql
-- Assets S1b (M3): which org catalogs a host with no org admits, and which
-- catalogs a paired client may manage. A personal grant is a row here plus
-- its mirror `client_tokens.assets_admin_at` (migration 074), which stays so
-- a downgraded hub keeps the grant; nothing else reads it once a personal
-- catalog exists. CREATE IF NOT EXISTS + INSERT OR IGNORE: safe to re-run.
CREATE TABLE IF NOT EXISTS host_catalogs (
  host_alias  TEXT    NOT NULL REFERENCES hosts(alias) ON DELETE CASCADE,
  catalog_id  INTEGER NOT NULL REFERENCES catalogs(id) ON DELETE CASCADE,
  admitted_at INTEGER NOT NULL,
  PRIMARY KEY (host_alias, catalog_id)
);

CREATE TABLE IF NOT EXISTS client_catalog_grants (
  client_id  INTEGER NOT NULL REFERENCES client_tokens(id) ON DELETE CASCADE,
  catalog_id INTEGER NOT NULL REFERENCES catalogs(id) ON DELETE CASCADE,
  granted_at INTEGER NOT NULL,
  PRIMARY KEY (client_id, catalog_id)
);

-- Spec, Migration step 3: every client with `assets_admin_at` gets a grant on
-- `personal`. With no personal catalog yet there is nothing to attach it to;
-- `Store::set_catalog_config` runs the same statement when it creates one.
INSERT OR IGNORE INTO client_catalog_grants (client_id, catalog_id, granted_at)
  SELECT t.id, c.id, t.assets_admin_at
  FROM client_tokens t JOIN catalogs c ON c.org_id IS NULL
  WHERE t.assets_admin_at IS NOT NULL;

-- Migration 060's rule — every change to who may do what bumps the epoch —
-- held for the new table too, although `catalog_admin` reads grants live.
CREATE TRIGGER IF NOT EXISTS auth_epoch_client_catalog_grants_insert
  AFTER INSERT ON client_catalog_grants
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;
CREATE TRIGGER IF NOT EXISTS auth_epoch_client_catalog_grants_delete
  AFTER DELETE ON client_catalog_grants
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;

INSERT OR IGNORE INTO schema_version (version) VALUES (92);
```

`crates/fleet-core/migrations/091_catalog_ids.sql` — insert right after the header comment, before `CREATE TABLE IF NOT EXISTS host_layers_new`:

```sql
-- Assets M3 (R1): never assume host_layers exists. A database from the
-- 033/035 collision family can reach this migration without it, and
-- `repair_skipped_main_migrations` — which recreates it — only runs after
-- every pending migration. 033's shape; the rebuild below converts it.
CREATE TABLE IF NOT EXISTS host_layers (
  host_alias TEXT    NOT NULL REFERENCES hosts(alias),
  layer_name TEXT    NOT NULL,
  axis       TEXT    NOT NULL,
  position   INTEGER NOT NULL DEFAULT 0,
  active     INTEGER NOT NULL DEFAULT 1,
  PRIMARY KEY (host_alias, layer_name)
);
```

`store/schema.rs` — after the version-91 entry in `MIGRATIONS`:

```rust
    // Assets S1b M3: `host_catalogs` (admissions) and `client_catalog_grants`
    // (personal backfilled from `assets_admin_at`). IF NOT EXISTS + INSERT
    // OR IGNORE: safe to re-run.
    Migration::plain(92, include_str!("../../migrations/092_catalog_access.sql")),
```

In `migration_91_drops_a_preexisting_dangling_host_layers_row`, the store is at version 90, where `client_catalog_grants` does not exist yet and `set_catalog_config` (Step 4) now writes to it. Replace its line `old.set_catalog_config("/p", None).unwrap();` with:

```rust
        // Raw SQL, not `set_catalog_config`: at version 90 there is no
        // `client_catalog_grants` table for its grant backfill to write to.
        old.conn
            .execute_batch(
                "INSERT INTO catalogs (name, repo_path, org_id, created_at) VALUES ('personal', '/p', NULL, 0);",
            )
            .unwrap();
```

(No other test builds a store below 92 and then calls `set_catalog_config` before migrating — checked with `rg -n "store_at_version|set_catalog_config" crates/fleet-core/src/store/schema.rs`.)

- [ ] **Step 4: Implement the store API**

`store/rows.rs` (next to `CatalogRow`):

```rust
/// What `Store::remove_catalog` dropped along with the row (migration 092,
/// Rulings R13): the catalog's layer assignments, admissions and grants all
/// go by `ON DELETE CASCADE`. The checkout on disk is never touched.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CatalogRemoval {
    pub id: i64,
    pub name: String,
    pub layer_rows: usize,
    pub admissions: usize,
    pub grants: usize,
}
```

`store/catalog.rs` — in `set_catalog_config`, after the `INSERT … ON CONFLICT` and before `Ok(...)`:

```rust
        // Migration 092 / Rulings R2: a personal grant made while no personal
        // catalog existed waits in `assets_admin_at`; give it its grant row
        // now that there is a catalog to attach it to. Idempotent.
        self.conn.execute(
            "INSERT OR IGNORE INTO client_catalog_grants (client_id, catalog_id, granted_at)
             SELECT t.id, c.id, t.assets_admin_at
             FROM client_tokens t JOIN catalogs c ON c.org_id IS NULL
             WHERE t.assets_admin_at IS NOT NULL",
            [],
        )?;
```

and in the same `impl Store`:

```rust
    pub fn get_catalog_by_name(&self, name: &str) -> Result<Option<CatalogRow>, rusqlite::Error> {
        use rusqlite::OptionalExtension;
        self.conn
            .prepare_cached(&format!(
                "SELECT {} FROM catalogs WHERE name = ?1",
                Self::CATALOG_COLS
            ))?
            .query_row([name], Self::catalog_row)
            .optional()
    }

    /// Add a catalog, or re-point an existing one (`repo_path`, `remote_url`;
    /// the load record is cleared, as `set_catalog_config` does). `personal`
    /// and only `personal` has no org (the table's CHECK, said in words), and
    /// an existing catalog never changes owner: that is a remove and an add.
    pub fn upsert_catalog(
        &self,
        name: &str,
        repo_path: &str,
        remote_url: Option<&str>,
        org_id: Option<i64>,
    ) -> Result<CatalogRow, crate::ipc_error::IpcError> {
        use crate::ipc_error::{codes, IpcError};
        match (name == "personal", org_id) {
            (true, Some(_)) => {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    "the personal catalog belongs to no org",
                ))
            }
            (false, None) => {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!("catalog {name} needs an org: only `personal` belongs to none"),
                ))
            }
            (true, None) => {
                self.set_catalog_config(repo_path, remote_url)?;
            }
            (false, Some(org)) => match self.get_catalog_by_name(name)? {
                Some(existing) if existing.org_id != Some(org) => {
                    return Err(IpcError::new(
                        codes::E_INVALID,
                        format!(
                            "catalog {name} belongs to another org; remove it and add it again to move it"
                        ),
                    ))
                }
                Some(existing) => {
                    self.conn.execute(
                        "UPDATE catalogs SET repo_path = ?2, remote_url = ?3, \
                         head_commit = NULL, last_loaded_at = NULL WHERE id = ?1",
                        rusqlite::params![existing.id, repo_path, remote_url],
                    )?;
                }
                None => {
                    if self.get_org(org)?.is_none() {
                        return Err(IpcError::new(
                            codes::E_NOTFOUND,
                            format!("org {org} not found"),
                        ));
                    }
                    self.conn.execute(
                        "INSERT INTO catalogs (name, repo_path, remote_url, org_id, created_at) \
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        rusqlite::params![name, repo_path, remote_url, org, now_unix()],
                    )?;
                }
            },
        }
        self.get_catalog_by_name(name)?
            .ok_or_else(|| IpcError::new(codes::E_INTERNAL, format!("catalog {name} vanished")))
    }

    /// Remove an org catalog's row (Rulings R13). Its `host_layers` rows,
    /// admissions and grants go with it (`ON DELETE CASCADE`); inventory rows
    /// keep their place with `catalog_id` NULL (`ON DELETE SET NULL`).
    pub fn remove_catalog(
        &self,
        name: &str,
    ) -> Result<CatalogRemoval, crate::ipc_error::IpcError> {
        use crate::ipc_error::{codes, IpcError};
        let row = self.get_catalog_by_name(name)?.ok_or_else(|| {
            IpcError::new(codes::E_NOTFOUND, format!("no catalog named {name}"))
        })?;
        if row.org_id.is_none() {
            return Err(IpcError::new(
                codes::E_INVALID,
                "the personal catalog cannot be removed; point it elsewhere with `catalog set`",
            ));
        }
        let tx = self.conn.unchecked_transaction()?;
        let count = |sql: &str| -> rusqlite::Result<usize> {
            tx.query_row(sql, [row.id], |r| r.get::<_, i64>(0))
                .map(|n| n as usize)
        };
        let removal = CatalogRemoval {
            id: row.id,
            name: row.name.clone(),
            layer_rows: count("SELECT COUNT(*) FROM host_layers WHERE catalog_id = ?1")?,
            admissions: count("SELECT COUNT(*) FROM host_catalogs WHERE catalog_id = ?1")?,
            grants: count("SELECT COUNT(*) FROM client_catalog_grants WHERE catalog_id = ?1")?,
        };
        tx.execute("DELETE FROM catalogs WHERE id = ?1", [row.id])?;
        tx.commit()?;
        Ok(removal)
    }

    /// Admit `catalog_id` on a host (migration 092). Whether the host may use
    /// an admission (no org, an org catalog) is the service's check.
    pub fn admit_host_catalog(&self, host_alias: &str, catalog_id: i64) -> Result<bool, rusqlite::Error> {
        Ok(self.conn.execute(
            "INSERT OR IGNORE INTO host_catalogs (host_alias, catalog_id, admitted_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![host_alias, catalog_id, now_unix()],
        )? > 0)
    }

    pub fn unadmit_host_catalog(&self, host_alias: &str, catalog_id: i64) -> Result<bool, rusqlite::Error> {
        Ok(self.conn.execute(
            "DELETE FROM host_catalogs WHERE host_alias = ?1 AND catalog_id = ?2",
            rusqlite::params![host_alias, catalog_id],
        )? > 0)
    }

    /// The catalog ids `host_alias` admits, ascending.
    pub fn host_admissions(&self, host_alias: &str) -> Result<Vec<i64>, rusqlite::Error> {
        self.conn
            .prepare_cached("SELECT catalog_id FROM host_catalogs WHERE host_alias = ?1 ORDER BY catalog_id")?
            .query_map([host_alias], |r| r.get(0))?
            .collect()
    }

    /// The hosts that admit `catalog_id`, by alias.
    pub fn catalog_admissions(&self, catalog_id: i64) -> Result<Vec<String>, rusqlite::Error> {
        self.conn
            .prepare_cached("SELECT host_alias FROM host_catalogs WHERE catalog_id = ?1 ORDER BY host_alias")?
            .query_map([catalog_id], |r| r.get(0))?
            .collect()
    }
```

`store/clients.rs` — factor the lookup-and-refusal half of `set_client_assets_admin` into a helper and rebuild the two functions on grants:

```rust
impl Store {
    /// The live client `name`, checked for holding a catalog grant when `on`
    /// (Rulings R3): a peer hub link, a `readonly` client and a client bound
    /// to an org are refused. `E_NOTFOUND` when no live client holds it.
    fn grantable_client(
        &self,
        name: &str,
        on: bool,
    ) -> Result<ClientTokenRow, crate::ipc_error::IpcError> {
        let name = name.trim();
        let row = self
            .conn
            .query_row(
                "SELECT id, name, token_sha256, mode, created_at, last_seen_at, revoked_at, trusted_at, org_id, assets_admin_at \
                 FROM client_tokens WHERE name = ?1 AND revoked_at IS NULL",
                rusqlite::params![name],
                map_client_token_row,
            )
            .optional()?
            .ok_or_else(|| {
                crate::ipc_error::IpcError::new(
                    codes::E_NOTFOUND,
                    format!("no active client token named '{name}'"),
                )
            })?;
        if on {
            let refuse = |why: &str| {
                Err(crate::ipc_error::IpcError::new(
                    codes::E_VALIDATE,
                    format!("'{name}' {why}; it cannot manage the asset catalog"),
                ))
            };
            match row.mode.as_str() {
                "full" => {}
                "peer" => return refuse("is a peer hub link"),
                _ => return refuse("is a readonly client"),
            }
            if row.org_id.is_some() {
                return refuse("is bound to one org, and a catalog sync writes to every host");
            }
        }
        Ok(row)
    }

    /// The personal grant (`fleet-hub client grant <name> assets`): a grant
    /// row on `personal` plus its mirror `assets_admin_at` (Rulings R2).
    /// With no personal catalog yet, only the mirror is written; configuring
    /// `personal` turns it into a row (`set_catalog_config`).
    pub fn set_client_assets_admin(
        &self,
        name: &str,
        on: bool,
    ) -> Result<ClientTokenRow, crate::ipc_error::IpcError> {
        if let Some(personal) = self.personal_catalog()? {
            return self.set_client_catalog_grant(name, personal.id, on);
        }
        let row = self.grantable_client(name, on)?;
        self.conn.execute(
            if on {
                "UPDATE client_tokens SET assets_admin_at = COALESCE(assets_admin_at, ?2) WHERE id = ?1"
            } else {
                "UPDATE client_tokens SET assets_admin_at = NULL WHERE id = ?1 AND ?2 IS NOT NULL"
            },
            rusqlite::params![row.id, now_unix()],
        )?;
        get_client_token_by_id(&self.conn, row.id)?.ok_or_else(|| {
            crate::ipc_error::IpcError::new(
                codes::E_NOTFOUND,
                format!("no active client token named '{}'", name.trim()),
            )
        })
    }

    /// Grant (or take back) one catalog to the live client `name` (migration
    /// 092). On `personal` the mirror `assets_admin_at` follows (R2).
    pub fn set_client_catalog_grant(
        &self,
        name: &str,
        catalog_id: i64,
        on: bool,
    ) -> Result<ClientTokenRow, crate::ipc_error::IpcError> {
        let row = self.grantable_client(name, on)?;
        let catalog = self.get_catalog(catalog_id)?.ok_or_else(|| {
            crate::ipc_error::IpcError::new(
                codes::E_NOTFOUND,
                format!("catalog {catalog_id} not found"),
            )
        })?;
        let tx = self.conn.unchecked_transaction()?;
        if on {
            tx.execute(
                "INSERT OR IGNORE INTO client_catalog_grants (client_id, catalog_id, granted_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![row.id, catalog_id, now_unix()],
            )?;
        } else {
            tx.execute(
                "DELETE FROM client_catalog_grants WHERE client_id = ?1 AND catalog_id = ?2",
                rusqlite::params![row.id, catalog_id],
            )?;
        }
        if catalog.org_id.is_none() {
            tx.execute(
                if on {
                    "UPDATE client_tokens SET assets_admin_at = COALESCE(assets_admin_at, ?2) WHERE id = ?1"
                } else {
                    "UPDATE client_tokens SET assets_admin_at = NULL WHERE id = ?1 AND ?2 IS NOT NULL"
                },
                rusqlite::params![row.id, now_unix()],
            )?;
        }
        tx.commit()?;
        get_client_token_by_id(&self.conn, row.id)?.ok_or_else(|| {
            crate::ipc_error::IpcError::new(
                codes::E_NOTFOUND,
                format!("no active client token named '{}'", name.trim()),
            )
        })
    }

    /// True when the live client `id` may manage the personal catalog: a
    /// grant row on `personal`, or — only while no personal catalog exists —
    /// the pending mirror `assets_admin_at` (R2). `full`, not revoked, no org.
    pub fn client_is_assets_admin(&self, id: i64) -> Result<bool, rusqlite::Error> {
        self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM client_tokens t
               WHERE t.id = ?1 AND t.revoked_at IS NULL AND t.mode = 'full' AND t.org_id IS NULL
                 AND (EXISTS(SELECT 1 FROM client_catalog_grants g JOIN catalogs c ON c.id = g.catalog_id
                             WHERE g.client_id = t.id AND c.org_id IS NULL)
                      OR (t.assets_admin_at IS NOT NULL
                          AND NOT EXISTS(SELECT 1 FROM catalogs WHERE org_id IS NULL))))",
            rusqlite::params![id],
            |r| r.get(0),
        )
    }

    /// True when the live client `id` holds a grant on `catalog_id`: `full`,
    /// not revoked, bound to no org (R3). Read on every `catalog_admin` call.
    pub fn client_may_admin_catalog(&self, id: i64, catalog_id: i64) -> Result<bool, rusqlite::Error> {
        self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM client_tokens t
               JOIN client_catalog_grants g ON g.client_id = t.id
               WHERE t.id = ?1 AND g.catalog_id = ?2
                 AND t.revoked_at IS NULL AND t.mode = 'full' AND t.org_id IS NULL)",
            rusqlite::params![id, catalog_id],
            |r| r.get(0),
        )
    }

    /// The live clients holding a grant on `catalog_id`, by name.
    pub fn catalog_grantees(&self, catalog_id: i64) -> Result<Vec<String>, rusqlite::Error> {
        self.conn
            .prepare_cached(
                "SELECT t.name FROM client_tokens t JOIN client_catalog_grants g ON g.client_id = t.id
                 WHERE g.catalog_id = ?1 AND t.revoked_at IS NULL ORDER BY t.name",
            )?
            .query_map([catalog_id], |r| r.get(0))?
            .collect()
    }
}
```

Delete the old bodies of `set_client_assets_admin` and `client_is_assets_admin` (replaced above). `optional()` needs `rusqlite::OptionalExtension` in scope (it already is in `clients.rs`; if not, add `use rusqlite::OptionalExtension;`).

`store/hosts_accounts.rs` `merge_host_alias` — add one entry to the table list (after `"host_layers"`):

```rust
            ("host_catalogs", "catalog_id, admitted_at"),
```

- [ ] **Step 5: Run to verify pass**

Run, one per command: `cargo test -p fleet-core migration_9`, `cargo test -p fleet-core store::catalog`, `cargo test -p fleet-core store::clients`, `cargo test -p fleet-core merge_host_alias`, `cargo test -p fleet-core tests_catalog_admin`, `cargo test -p fleet-core store::`. Expected: PASS (the existing `the_assets_grant_is_for_a_live_full_unbound_client_only` and the `tests_catalog_admin` cases pass unchanged — they run without a personal catalog, which is R2's pending path).

- [ ] **Step 6: Commit**

```bash
git add crates/fleet-core/migrations/091_catalog_ids.sql crates/fleet-core/migrations/092_catalog_access.sql crates/fleet-core/src/store/
git commit -m "feat(catalog): host admissions and per-catalog client grants (migration 092)"
```

---

### Task 2: Load any catalog; `ensure_fresh` per catalog with problem entries

**Files:**
- Modify: `crates/fleet-core/src/service/catalog/repo.rs` (`Catalog.load_error`)
- Modify: `crates/fleet-core/src/service/catalog/registry.rs`
- Modify: `crates/fleet-core/src/service/catalog/mod.rs` (`load_catalog`, `load`, `load_all`, `ensure_fresh`, `problem_entry`, `is_current`; `expand_home` → `pub(crate)`)
- Modify: `crates/fleet-core/src/service/catalog/scan_tick.rs`
- Modify: `src-tauri/src/commands/assets.rs` (local `catalog_load`)

**Interfaces:**
- Consumes: `Store::list_catalogs`, `Store::get_catalog`, `Store::upsert_catalog`, `Store::remove_catalog` (Task 1).
- Produces:

```rust
// repo.rs — Catalog gains:
/// Assets M3: why this catalog could not be loaded (a problem entry, Rulings R5). `None` on every loaded catalog.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub load_error: Option<String>,

// registry.rs
pub fn union_of(catalogs: &BTreeMap<i64, Catalog>) -> Option<Catalog>;
pub fn snapshot() -> Result<BTreeMap<i64, Catalog>, IpcError>;
pub fn remove(id: i64) -> Result<(), IpcError>;
pub fn evict_org_catalogs_not_in(configured: &BTreeSet<i64>) -> Result<(), IpcError>;
pub fn with_catalog_row<T>(row: &CatalogRow, f: impl FnOnce(&Catalog) -> Result<T, IpcError>) -> Result<T, IpcError>;
pub fn heads_key(catalogs: &BTreeMap<i64, Catalog>) -> String;

// mod.rs
pub fn load_catalog(id: i64, pull: bool, store: &Mutex<Store>) -> Result<CatalogSummary, IpcError>;
pub fn load(pull: bool, store: &Mutex<Store>) -> Result<CatalogSummary, IpcError>;      // personal, as before
pub fn load_all(pull: bool, store: &Mutex<Store>) -> Result<CatalogSummary, IpcError>;  // personal + ensure_fresh (R18)
pub fn ensure_fresh(store: &Mutex<Store>) -> Result<(), IpcError>;                      // every catalog
pub(crate) fn problem_entry(row: &CatalogRow, err: &IpcError) -> repo::Catalog;
pub(crate) fn expand_home(p: &str) -> String;
```

- [ ] **Step 1: Write the failing tests**

`registry.rs` tests module:

```rust
    /// Assets M3: the scan tick rescans when ANY loaded catalog's HEAD moves.
    #[test]
    fn heads_key_changes_when_any_catalogs_head_does() {
        let mut m = BTreeMap::new();
        m.insert(1, cat(1, "personal", None));
        m.insert(2, cat(2, "acme", Some(9)));
        let before = heads_key(&m);
        assert!(before.starts_with("personal=h1"), "{before}");
        m.get_mut(&2).unwrap().head = "moved".into();
        assert_ne!(heads_key(&m), before);
    }
```

`mod.rs` tests module:

```rust
    fn acme_org(store: &Mutex<Store>) -> i64 {
        store.lock().unwrap().add_org("acme", None, false).unwrap().id
    }

    /// Assets M3: `ensure_fresh` loads every configured catalog; an org
    /// catalog whose checkout cannot be read is kept as a problem entry while
    /// the others load (spec, Runtime), and is not retried until the store's
    /// record changes (Rulings R5); an explicit `load_catalog` always tries;
    /// a catalog removed from the store leaves the registry.
    #[test]
    fn ensure_fresh_loads_every_catalog_and_keeps_a_broken_one_as_a_problem() {
        let _g = lock_registry_for_test();
        let store = Mutex::new(Store::open_in_memory().unwrap());
        configure(
            ConfigureArgs {
                repo_path: repo_with_one_skill("m3-personal").to_string_lossy().into(),
                remote_url: None,
            },
            &store,
        )
        .unwrap();
        let org = acme_org(&store);
        let acme_repo = repo_with_one_skill("m3-acme");
        let broken_path = std::env::temp_dir()
            .join(format!("fleet-catalog-svc-m3-broken-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&broken_path);
        let (acme, broken) = {
            let s = store.lock().unwrap();
            (
                s.upsert_catalog("acme", &acme_repo.to_string_lossy(), None, Some(org)).unwrap(),
                s.upsert_catalog("broken", &broken_path.to_string_lossy(), None, Some(org)).unwrap(),
            )
        };

        ensure_fresh(&store).unwrap();
        assert_eq!(registry::personal().unwrap().unwrap().assets.len(), 1);
        let a = registry::get(acme.id).unwrap().expect("acme loaded");
        assert_eq!(
            (a.name.as_str(), a.org_id, a.assets.len(), a.load_error.is_none()),
            ("acme", Some(org), 1, true)
        );
        let b = registry::get(broken.id).unwrap().expect("a problem entry, not a gap");
        assert!(
            b.load_error.as_deref().unwrap_or_default().contains("not a git repository"),
            "{:?}",
            b.load_error
        );
        assert!(b.assets.is_empty());
        assert_eq!(b.problems.len(), 1);

        // A checkout appearing at the path is not picked up by the catch-up
        // while the store's record stands…
        std::fs::rename(repo_with_one_skill("m3-broken-fixed"), &broken_path).unwrap();
        ensure_fresh(&store).unwrap();
        assert!(registry::get(broken.id).unwrap().unwrap().load_error.is_some());
        // …but an explicit load always tries.
        load_catalog(broken.id, false, &store).unwrap();
        assert!(registry::get(broken.id).unwrap().unwrap().load_error.is_none());

        store.lock().unwrap().remove_catalog("acme").unwrap();
        ensure_fresh(&store).unwrap();
        assert!(registry::get(acme.id).unwrap().is_none(), "removed elsewhere: evicted");
        registry::clear().unwrap();
    }

    /// Parity (R5): personal never becomes a problem entry — its failure is
    /// still `ensure_fresh`'s error — but the org catalogs are attempted too.
    #[test]
    fn a_personal_load_failure_is_still_an_error_but_the_org_catalogs_load() {
        let _g = lock_registry_for_test();
        let store = Mutex::new(Store::open_in_memory().unwrap());
        store
            .lock()
            .unwrap()
            .set_catalog_config("/nonexistent/fleet-m3-personal", None)
            .unwrap();
        let org = acme_org(&store);
        let acme = store
            .lock()
            .unwrap()
            .upsert_catalog("acme", &repo_with_one_skill("m3-org-ok").to_string_lossy(), None, Some(org))
            .unwrap();
        assert_eq!(ensure_fresh(&store).unwrap_err().code, E_CATALOG_GIT);
        assert!(registry::personal().unwrap().is_none());
        assert!(registry::get(acme.id)
            .unwrap()
            .is_some_and(|c| c.load_error.is_none()));
        registry::clear().unwrap();
    }

    /// R18: `load_all` loads personal and catches every other catalog up.
    #[test]
    fn load_all_loads_personal_and_catches_up_the_rest() {
        let _g = lock_registry_for_test();
        let store = Mutex::new(Store::open_in_memory().unwrap());
        configure(
            ConfigureArgs {
                repo_path: repo_with_one_skill("m3-all-p").to_string_lossy().into(),
                remote_url: None,
            },
            &store,
        )
        .unwrap();
        let org = acme_org(&store);
        let acme = store
            .lock()
            .unwrap()
            .upsert_catalog("acme", &repo_with_one_skill("m3-all-a").to_string_lossy(), None, Some(org))
            .unwrap();
        let summary = load_all(false, &store).unwrap();
        assert_eq!(summary.asset_count, 1, "the personal catalog's summary");
        assert!(registry::get(acme.id).unwrap().is_some());
        registry::clear().unwrap();
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p fleet-core catalog::tests::ensure_fresh` — Expected: compile errors (`load_catalog`, `load_error`).

- [ ] **Step 3: Implement**

`repo.rs` — add to `struct Catalog` after `origin`:

```rust
    /// Assets M3: why this catalog could not be loaded — a registry *problem
    /// entry* (no assets, one `Problem`), kept so the other catalogs still
    /// load (spec, Runtime) and so a sync never reads its absence as "every
    /// asset dropped" (Rulings R5, R6). `None` on every loaded catalog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_error: Option<String>,
```

Fix every `Catalog { … }` literal the compiler reports that has no `..Default::default()` by adding `load_error: None`.

`registry.rs` — move `union_all`'s closure body into `union_of` and add the rest:

```rust
/// [`union_all`] over a given map, for a caller that took a [`snapshot`].
pub fn union_of(catalogs: &BTreeMap<i64, Catalog>) -> Option<Catalog> {
    let personal = catalogs.values().find(|c| c.org_id.is_none())?;
    let ordered = in_order(catalogs);
    let mut merged = Catalog {
        id: 0,
        name: String::new(),
        head: personal.head.clone(),
        layers: personal.layers.clone(),
        ..Default::default()
    };
    let mut seen: std::collections::BTreeSet<(super::model::Kind, String)> =
        std::collections::BTreeSet::new();
    for cat in ordered {
        merged.problems.extend(cat.problems.iter().cloned());
        for asset in &cat.assets {
            let key = (asset.kind(), asset.header.name.clone());
            if !seen.insert(key) {
                continue;
            }
            merged.origin.insert(
                format!("{}/{}", asset.kind().as_str(), asset.header.name),
                CatalogRef { id: cat.id, name: cat.name.clone() },
            );
            merged.assets.push(asset.clone());
        }
    }
    Some(merged)
}

pub fn union_all() -> Result<Option<Catalog>, IpcError> {
    with_catalogs(|catalogs| Ok(union_of(catalogs)))
}

/// Every loaded catalog, cloned out of the lock: one consistent view for a
/// caller that reads the registry more than once (`plan_sync`, Rulings R9).
pub fn snapshot() -> Result<BTreeMap<i64, Catalog>, IpcError> {
    with_catalogs(|catalogs| Ok(catalogs.clone()))
}

/// Drop one catalog (`catalog remove`).
pub fn remove(id: i64) -> Result<(), IpcError> {
    CATALOGS.write().map_err(|_| poisoned())?.remove(&id);
    Ok(())
}

/// Drop every org catalog whose id the store no longer has (removed by
/// another process). The personal entry is `install_personal`'s to manage.
pub fn evict_org_catalogs_not_in(configured: &std::collections::BTreeSet<i64>) -> Result<(), IpcError> {
    CATALOGS
        .write()
        .map_err(|_| poisoned())?
        .retain(|id, c| c.org_id.is_none() || configured.contains(id));
    Ok(())
}

/// Borrow the loaded catalog a store row names: `personal` through
/// [`with_personal`] (tests install it under id 0), any other by id.
/// Not loaded, or a problem entry: `E_CATALOG_NOT_CONFIGURED` saying which.
pub fn with_catalog_row<T>(
    row: &crate::store::CatalogRow,
    f: impl FnOnce(&Catalog) -> Result<T, IpcError>,
) -> Result<T, IpcError> {
    if row.org_id.is_none() {
        return with_personal(f);
    }
    let guard = CATALOGS.read().map_err(|_| poisoned())?;
    match guard.get(&row.id) {
        Some(c) if c.load_error.is_none() => f(c),
        Some(c) => Err(IpcError::new(
            super::E_CATALOG_NOT_CONFIGURED,
            format!(
                "catalog {} failed to load: {}",
                row.name,
                c.load_error.as_deref().unwrap_or_default()
            ),
        )),
        None => Err(IpcError::new(
            super::E_CATALOG_NOT_CONFIGURED,
            format!("catalog {} is not loaded; load it (catalog: {})", row.name, row.name),
        )),
    }
}

/// `name=head` of every loaded catalog in composition order: what the scan
/// tick compares to decide that a catalog moved.
pub fn heads_key(catalogs: &BTreeMap<i64, Catalog>) -> String {
    in_order(catalogs)
        .iter()
        .map(|c| format!("{}={}", c.name, c.head))
        .collect::<Vec<_>>()
        .join(" ")
}
```

`mod.rs` — make `expand_home` `pub(crate)`, replace `load` and `ensure_fresh`, add the rest (imports: `crate::store::CatalogRow`, `std::collections::BTreeSet`):

```rust
/// (Optionally pull, then) parse catalog `id`'s checkout into the registry
/// and record its HEAD. Always tries, whatever the registry holds.
pub fn load_catalog(id: i64, pull: bool, store: &Mutex<Store>) -> Result<CatalogSummary, IpcError> {
    let row = lock(store)?.get_catalog(id)?.ok_or_else(|| {
        IpcError::new(codes::E_NOTFOUND, format!("catalog {id} not found"))
    })?;
    let root = std::path::PathBuf::from(&row.repo_path);
    repo::ensure_repo(&root, row.remote_url.as_deref())?;
    if pull {
        repo::pull(&root)?;
    }
    let mut cat = repo::load_dir(&root)?;
    cat.head = repo::head(&root)?;
    let summary = CatalogSummary {
        head: cat.head.clone(),
        loaded_at: cat.loaded_at,
        asset_count: cat.assets.len(),
        problem_count: cat.problems.len(),
    };
    cat.id = row.id;
    cat.name = row.name.clone();
    cat.org_id = row.org_id;
    // Store guard and registry lock never overlap (registry.rs module doc).
    if row.org_id.is_none() {
        registry::install_personal(cat)?;
    } else {
        registry::install(cat)?;
    }
    {
        let s = lock(store)?;
        s.set_catalog_head_for(row.id, &summary.head, summary.loaded_at)?;
        s.bus_catalog_loaded(&summary);
    }
    Ok(summary)
}

/// Load the personal catalog — the desktop's and `catalog set`'s load.
pub fn load(pull: bool, store: &Mutex<Store>) -> Result<CatalogSummary, IpcError> {
    let id = lock(store)?
        .personal_catalog()?
        .ok_or_else(|| IpcError::new(E_CATALOG_NOT_CONFIGURED, "configure the catalog repo first"))?
        .id;
    load_catalog(id, pull, store)
}

/// `catalog_load` with no catalog named (Rulings R18): personal, then every
/// other catalog brought up to date — an org catalog that cannot load stays
/// a problem entry. Answers personal's summary.
pub fn load_all(pull: bool, store: &Mutex<Store>) -> Result<CatalogSummary, IpcError> {
    let summary = load(pull, store)?;
    ensure_fresh(store)?;
    Ok(summary)
}

/// A registry entry for an org catalog that failed to load (Rulings R5):
/// stamped with the store's record as it stood, so [`is_current`] holds —
/// and nothing retries — until that record changes.
pub(crate) fn problem_entry(row: &CatalogRow, err: &IpcError) -> repo::Catalog {
    repo::Catalog {
        id: row.id,
        name: row.name.clone(),
        org_id: row.org_id,
        head: row.head_commit.clone().unwrap_or_default(),
        loaded_at: row.last_loaded_at.unwrap_or(0),
        problems: vec![model::Problem {
            path: row.repo_path.clone(),
            message: format!("catalog {} could not be loaded: {}", row.name, err.message),
        }],
        load_error: Some(err.message.clone()),
        ..Default::default()
    }
}

/// Whether the registry's `entry` is the load the store last recorded.
fn is_current(entry: &repo::Catalog, row: &CatalogRow) -> bool {
    if entry.load_error.is_some() {
        entry.head == row.head_commit.clone().unwrap_or_default()
            && entry.loaded_at == row.last_loaded_at.unwrap_or(0)
    } else {
        row.last_loaded_at == Some(entry.loaded_at)
            && row.head_commit.as_deref() == Some(entry.head.as_str())
    }
}

/// Bring every configured catalog's registry entry up to the store's record
/// (spec, Runtime). The registry is one process's memory; the configuration
/// and each catalog's last load are in the store, written by `fleet-hub
/// catalog …` in another process — comparing the two is how a running hub
/// notices. An org catalog that cannot load becomes a problem entry and the
/// others still load; a personal failure is returned, as before (R5), after
/// the org catalogs were attempted. Org entries the store no longer has are
/// evicted.
pub fn ensure_fresh(store: &Mutex<Store>) -> Result<(), IpcError> {
    let rows = lock(store)?.list_catalogs()?;
    let configured: BTreeSet<i64> = rows.iter().map(|r| r.id).collect();
    registry::evict_org_catalogs_not_in(&configured)?;
    let mut personal_err = None;
    for row in &rows {
        let current = registry::with_catalogs(|m| {
            let entry = if row.org_id.is_none() {
                m.values().find(|c| c.org_id.is_none())
            } else {
                m.get(&row.id)
            };
            Ok(entry.is_some_and(|c| is_current(c, row)))
        })
        .unwrap_or(false);
        if current {
            continue;
        }
        match load_catalog(row.id, false, store) {
            Ok(_) => {}
            Err(e) if row.org_id.is_none() => personal_err = Some(e),
            Err(e) => {
                tracing::warn!(catalog = %row.name, error = %e.message, "catalog could not be loaded; kept as a problem");
                registry::install(problem_entry(row, &e))?;
            }
        }
    }
    personal_err.map_or(Ok(()), Err)
}
```

Delete the old `ensure_fresh` and `load` bodies. Keep `config` / `require_config` / `configure` unchanged.

`scan_tick.rs` — replace the `head` computation:

```rust
            // Assets M3: every loaded catalog's HEAD, not just personal's — an
            // org catalog that moves must rescan too. Nothing loaded (no
            // personal), or the lock poisoned: nothing to compare yet.
            let head = match super::registry::with_catalogs(|m| {
                if !m.values().any(|c| c.org_id.is_none()) {
                    return Err(crate::ipc_error::IpcError::new(
                        super::E_CATALOG_NOT_CONFIGURED,
                        "catalog not loaded",
                    ));
                }
                Ok(super::registry::heads_key(m))
            }) {
                Ok(head) => head,
                Err(_) => continue,
            };
```

`src-tauri/src/commands/assets.rs` — in `routed::catalog_load`:

```rust
            None => catalog::load_all(args.pull, store),
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p fleet-core catalog::tests`, `cargo test -p fleet-core catalog::registry`, `cargo test -p fleet-core scan_tick`, `cargo test -p fleet-core tests_catalog_admin`, `cargo test -p fleet-core catalog`, `cargo test -p claude-fleet --lib catalog`. Expected: PASS (`ensure_fresh_follows_the_stores_record` and `catalog_admin_refreshes_the_catalog_for_every_call_but_config_load_and_apply_sync` unchanged).

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/service/catalog/ src-tauri/src/commands/assets.rs
git commit -m "feat(catalog): load any catalog; ensure_fresh keeps a broken org catalog as a problem entry"
```

---

### Task 3: Admissions in the effective set; never remove what no catalog speaks for

**Files:**
- Modify: `crates/fleet-core/src/service/catalog/effective.rs`
- Modify: `crates/fleet-core/src/service/catalog/resolve.rs` (`Resolution.held_back`)
- Modify: `crates/fleet-core/src/service/catalog/mod.rs` (`resolve_preview`)
- Modify: `crates/fleet-core/src/mcp/tools/assets.rs` (`resolve_preview` output + description)
- Modify: `crates/fleet-core/src/service/catalog/sync/manifest.rs` (`orphans`, `held`)
- Modify: `crates/fleet-core/src/service/catalog/sync/plan.rs` (`KeepRules`, held `Noop`)
- Modify: `crates/fleet-core/src/service/catalog/sync/mod.rs` (snapshot, `KeepRules`, withheld `Noop`; tests incl. test (b))
- Modify: `crates/fleet-core/src/service/catalog/inventory.rs` (`orphans(catalog, None)`)
- Modify: `crates/fleet-core/src/service/catalog/sync/apply.rs` (test call sites)

**Interfaces:**
- Consumes: `Store::host_admissions`, `Store::upsert_catalog`, `Store::admit_host_catalog` (Task 1); `Catalog.load_error`, `registry::snapshot`, `registry::union_of` (Task 2).
- Produces:

```rust
// effective.rs — EffectiveSet gains:
/// Catalogs composed for this host (accepted, loaded, resolved). `personal` is always named "personal".
#[serde(default)] pub speaks_for: BTreeSet<String>,
/// Catalog → why its manifest entries on this host are kept, not removed (R6).
#[serde(default)] pub held_back: BTreeMap<String, String>,
pub fn effective_for_host_in(store: &Mutex<Store>, host_alias: &str, catalogs: &BTreeMap<i64, Catalog>) -> Result<EffectiveSet, IpcError>;

// resolve.rs — Resolution gains:
#[serde(default)] pub held_back: BTreeMap<String, String>,

// manifest.rs
pub fn orphans<'a>(&'a self, catalog: &Catalog, speaks_for: Option<&BTreeSet<String>>) -> Vec<(&'a str, &'a ManifestEntry)>;
pub fn held<'a>(&'a self, catalog: &Catalog, speaks_for: &BTreeSet<String>) -> Vec<(&'a str, &'a ManifestEntry)>;

// plan.rs
#[derive(Debug, Clone, Default)]
pub struct KeepRules {
    pub protected: BTreeSet<(Kind, String)>,
    pub speaks_for: Option<BTreeSet<String>>,
    pub held_back: BTreeMap<String, String>,
}
// compute_host_plan's last parameter `protected: &BTreeSet<(Kind, String)>` becomes `keep: &KeepRules`.
pub(crate) fn held_noop(kind: Kind, name: &str, catalog: &str, reason: String) -> Action;
```

- [ ] **Step 1: Write the failing tests**

`effective.rs` tests module (uses its `seeded_store`, `personal_cat`, `acme_cat`, `skill`, `names`, `ORG_10`, `ORG_11`):

```rust
    /// Spec, Testing: a no-org host receives an org asset only when admitted.
    #[test]
    fn a_no_org_host_takes_an_org_catalog_only_when_admitted() {
        let _g = crate::service::catalog::lock_registry_for_test();
        let (store, personal, acme) = seeded_store();
        registry::install_personal(personal_cat(personal, vec![skill("a", "private")], LayerSet::default())).unwrap();
        registry::install_for_test(acme_cat(acme, vec![skill("c", "private")])).unwrap();

        let before = effective_for_host(&store, "h").unwrap();
        assert_eq!(names(&before), vec!["a"]);
        assert!(before.held_back["acme"].contains("not accepted"), "{:?}", before.held_back);
        assert_eq!(before.speaks_for, BTreeSet::from(["personal".to_string()]));

        store.lock().unwrap().admit_host_catalog("h", acme).unwrap();
        let after = effective_for_host(&store, "h").unwrap();
        assert_eq!(names(&after), vec!["a", "c"]);
        assert_eq!(after.catalog.origin_of(Kind::Skill, "c").name, "acme");
        assert!(after.held_back.is_empty(), "{:?}", after.held_back);
        assert_eq!(
            after.speaks_for,
            BTreeSet::from(["acme".to_string(), "personal".to_string()])
        );
    }

    /// An admission left behind when the host joined another org never
    /// crosses orgs (R4).
    #[test]
    fn an_admission_never_reaches_a_host_bound_to_an_org() {
        let _g = crate::service::catalog::lock_registry_for_test();
        let (store, personal, acme) = seeded_store();
        {
            let s = store.lock().unwrap();
            s.admit_host_catalog("h", acme).unwrap();
            s.set_host_org("h", Some(ORG_11)).unwrap();
        }
        registry::install_personal(personal_cat(personal, vec![], LayerSet::default())).unwrap();
        registry::install_for_test(acme_cat(acme, vec![skill("c", "shared")])).unwrap();
        let e = effective_for_host(&store, "h").unwrap();
        assert!(!names(&e).contains(&"c"), "{:?}", names(&e));
        assert!(e.held_back["acme"].contains("not accepted"), "{:?}", e.held_back);
    }

    /// M2 carry 1: a configured org catalog that is not loaded, or failed to
    /// load, never speaks for the host — its entries are held back.
    #[test]
    fn a_configured_catalog_that_is_not_loaded_or_failed_is_held_back() {
        let _g = crate::service::catalog::lock_registry_for_test();
        let (store, personal, _acme) = seeded_store();
        let beta = {
            let s = store.lock().unwrap();
            s.set_host_org("h", Some(ORG_10)).unwrap();
            s.upsert_catalog("beta", "/b", None, Some(ORG_10)).unwrap().id
        };
        registry::install_personal(personal_cat(personal, vec![skill("b", "shared")], LayerSet::default())).unwrap();
        registry::install_for_test(Catalog {
            id: beta,
            name: "beta".into(),
            org_id: Some(ORG_10),
            load_error: Some("boom".into()),
            ..Default::default()
        })
        .unwrap();
        let e = effective_for_host(&store, "h").unwrap();
        assert_eq!(names(&e), vec!["b"]);
        assert!(e.held_back["acme"].contains("is not loaded"), "{:?}", e.held_back);
        assert!(e.held_back["beta"].contains("failed to load (boom)"), "{:?}", e.held_back);
        assert_eq!(e.speaks_for, BTreeSet::from(["personal".to_string()]));
    }

    /// Rulings R7: an org catalog whose layers do not resolve for this host is
    /// held back for it, not fatal to the whole host.
    #[test]
    fn an_org_catalog_that_cannot_resolve_for_a_host_is_held_back_not_fatal() {
        let _g = crate::service::catalog::lock_registry_for_test();
        let (store, personal, acme) = seeded_store();
        {
            let s = store.lock().unwrap();
            s.set_host_org("h", Some(ORG_10)).unwrap();
            s.set_host_layers_for("h", acme, Some("ghost"), &[]).unwrap();
        }
        registry::install_personal(personal_cat(personal, vec![skill("b", "shared")], LayerSet::default())).unwrap();
        registry::install_for_test(acme_cat(acme, vec![skill("c", "private")])).unwrap();
        let e = effective_for_host(&store, "h").unwrap();
        assert_eq!(names(&e), vec!["b"]);
        assert!(e.held_back["acme"].contains("cannot resolve"), "{:?}", e.held_back);
    }

    /// Rulings R9: `effective_for_host_in` composes the map it is given, not
    /// the live registry, so plan and scan read one snapshot.
    #[test]
    fn effective_for_host_in_composes_the_snapshot_it_is_given() {
        let _g = crate::service::catalog::lock_registry_for_test();
        let (store, personal, _acme) = seeded_store();
        registry::install_personal(personal_cat(personal, vec![skill("live", "private")], LayerSet::default())).unwrap();
        let snapshot = BTreeMap::from([(
            personal,
            personal_cat(personal, vec![skill("snap", "private")], LayerSet::default()),
        )]);
        let e = effective_for_host_in(&store, "h", &snapshot).unwrap();
        assert_eq!(names(&e), vec!["snap"]);
    }
```

`manifest.rs` tests module (and change the existing `orphans_reports_keys_missing_from_the_catalog` call to `m.orphans(&catalog, None)`):

```rust
    /// M2 carry 3: an orphan is an entry whose OWN catalog no longer has it;
    /// an entry of a catalog that does not speak for this host is `held`.
    #[test]
    fn orphans_and_held_split_on_the_entrys_catalog() {
        let catalog = Catalog {
            assets: vec![skill("kept")],
            ..Default::default()
        };
        let mut m = Manifest::default();
        m.assets.insert(Manifest::key(Kind::Skill, "kept"), ManifestEntry::default());
        m.assets.insert(Manifest::key(Kind::Skill, "mine"), ManifestEntry::default());
        m.assets.insert(
            Manifest::key(Kind::Skill, "theirs"),
            ManifestEntry {
                catalog: "acme".into(),
                ..Default::default()
            },
        );
        let speaks = std::collections::BTreeSet::from(["personal".to_string()]);
        let keys = |v: Vec<(&str, &ManifestEntry)>| -> Vec<String> {
            v.into_iter().map(|(k, _)| k.to_string()).collect()
        };
        assert_eq!(keys(m.orphans(&catalog, Some(&speaks))), vec!["skill/mine"]);
        assert_eq!(keys(m.held(&catalog, &speaks)), vec!["skill/theirs"]);
        assert_eq!(
            keys(m.orphans(&catalog, None)),
            vec!["skill/mine", "skill/theirs"],
            "None: every catalog speaks (inventory, R8)"
        );
    }
```

`plan.rs` tests module (and change `plan_for` and the two `a_name_filter_narrows_the_plan_including_orphans` calls from `&BTreeSet::new()` to `&KeepRules::default()`):

```rust
    /// R6: personal still speaks — its dropped asset is removed as before —
    /// while an entry of a held-back catalog, or of one that is not
    /// configured at all, is kept with a `Noop` naming its catalog and why.
    #[test]
    fn an_orphan_whose_catalog_does_not_speak_is_kept_with_a_noop() {
        let mut manifest = manifest_with(&[]);
        manifest.assets.insert(
            "skill/gone".into(),
            ManifestEntry {
                files: vec!["~/.claude/skills/gone/SKILL.md".into()],
                ..Default::default()
            },
        );
        manifest.assets.insert(
            "skill/theirs".into(),
            ManifestEntry {
                catalog: "acme".into(),
                ..Default::default()
            },
        );
        manifest.assets.insert(
            "skill/lost".into(),
            ManifestEntry {
                catalog: "removed-one".into(),
                ..Default::default()
            },
        );
        let keep = KeepRules {
            speaks_for: Some(BTreeSet::from(["personal".to_string()])),
            held_back: BTreeMap::from([(
                "acme".to_string(),
                "catalog acme is not accepted by host local; its assets are kept, not removed".to_string(),
            )]),
            ..Default::default()
        };
        let hp = compute_host_plan(
            &Catalog::default(),
            &Claude,
            "local",
            &HostSnapshot::default(),
            &manifest,
            &secrets_map(),
            &PlanFilter::default(),
            &keep,
        );
        assert_eq!(act(&hp, "gone").op, ActionOp::Remove);
        let theirs = act(&hp, "theirs");
        assert_eq!(theirs.op, ActionOp::Noop);
        assert_eq!(theirs.catalog.as_deref(), Some("acme"));
        assert!(theirs.reason.as_deref().unwrap().contains("not accepted"));
        let lost = act(&hp, "lost");
        assert_eq!(lost.op, ActionOp::Noop);
        assert!(lost.reason.as_deref().unwrap().contains("is not configured"));
        assert_eq!(
            hp.actions.iter().filter(|a| a.op == ActionOp::Remove).count(),
            1
        );
    }
```

`sync/mod.rs` tests module — helpers, four new tests, and test (b) rewritten:

```rust
    /// Org 10 (`acme`) and its catalog row. Returns `(personal_id, acme_id)`.
    fn with_acme(store: &Mutex<Store>) -> (i64, i64) {
        let s = store.lock().unwrap();
        let personal_id = s.personal_catalog().unwrap().unwrap().id;
        s.conn_ref()
            .execute("INSERT INTO orgs (id, name, created_at) VALUES (10, 'acme', 0)", [])
            .unwrap();
        let acme = s.upsert_catalog("acme", "/a", None, Some(10)).unwrap();
        (personal_id, acme.id)
    }

    fn body_skill(name: &str, scope: &str) -> Asset {
        let mut a = skill_asset(name, scope);
        a.body = "b\n".into();
        a
    }

    fn install(personal_id: i64, personal: Vec<Asset>, acme_id: i64, acme: repo::Catalog) {
        super::super::registry::install_personal(repo::Catalog {
            id: personal_id,
            name: "personal".into(),
            org_id: None,
            assets: personal,
            ..Default::default()
        })
        .unwrap();
        super::super::registry::install_for_test(repo::Catalog {
            id: acme_id,
            name: "acme".into(),
            org_id: Some(10),
            ..acme
        })
        .unwrap();
    }

    /// `~/.claude/.fleet-assets.json` naming each `(key, catalog)` as synced.
    fn seed_manifest(home: &std::path::Path, entries: &[(&str, &str)]) {
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        let assets: serde_json::Map<String, serde_json::Value> = entries
            .iter()
            .map(|(key, catalog)| {
                let name = key.split('/').nth(1).unwrap();
                (
                    (*key).to_string(),
                    serde_json::json!({
                        "hash": "h",
                        "files": [format!("~/.claude/skills/{name}/SKILL.md")],
                        "merges": [],
                        "synced_at": 0,
                        "catalog": catalog,
                    }),
                )
            })
            .collect();
        std::fs::write(
            home.join(".claude/.fleet-assets.json"),
            serde_json::json!({ "version": 1, "updated_at": 0, "assets": assets }).to_string(),
        )
        .unwrap();
    }

    fn claude_actions<'a>(plan: &'a SyncPlan, host: &str, name: &str) -> Vec<&'a plan::Action> {
        plan.hosts
            .iter()
            .filter(|h| h.harness == "claude" && h.host_alias == host)
            .flat_map(|h| &h.actions)
            .filter(|a| a.name == name)
            .collect()
    }

    fn only(host: &str) -> PlanArgs {
        PlanArgs {
            host_alias: Some(host.into()),
            ..PlanArgs::default()
        }
    }

    /// Spec, Testing (planning): a no-org host receives an org asset only
    /// when admitted.
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_no_org_host_receives_an_org_asset_only_when_admitted() {
        let _lock = super::super::lock_registry_for_test();
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());
        let store = store_with_local(Arc::new(RecordingEventBus::new()));
        let (personal_id, acme_id) = with_acme(&store);
        install(
            personal_id,
            vec![body_skill("p", "private")],
            acme_id,
            repo::Catalog { assets: vec![body_skill("c", "private")], ..Default::default() },
        );
        let ssh = Arc::new(SshClient::new());

        let before = plan_sync(only("local"), &store, &ssh).await.unwrap();
        assert!(claude_actions(&before, "local", "c").is_empty(), "{:?}", before.hosts);
        assert_eq!(claude_actions(&before, "local", "p")[0].op, ActionOp::Create);

        store.lock().unwrap().admit_host_catalog("local", acme_id).unwrap();
        let after = plan_sync(only("local"), &store, &ssh).await.unwrap();
        let c = claude_actions(&after, "local", "c");
        assert_eq!(c.len(), 1, "{c:?}");
        assert_eq!(c[0].op, ActionOp::Create);
        assert_eq!(c[0].catalog.as_deref(), Some("acme"));
    }

    /// M2 carry 2: losing acceptance — an unadmit, or an org change — keeps
    /// what the catalog installed, with a `Noop` saying why; never a `Remove`.
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn losing_acceptance_keeps_the_org_assets_with_a_noop() {
        let _lock = super::super::lock_registry_for_test();
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());
        seed_manifest(home.path(), &[("skill/c", "acme")]);
        let store = store_with_local(Arc::new(RecordingEventBus::new()));
        let (personal_id, acme_id) = with_acme(&store);
        install(
            personal_id,
            vec![],
            acme_id,
            repo::Catalog { assets: vec![body_skill("c", "private")], ..Default::default() },
        );
        let ssh = Arc::new(SshClient::new());

        // Not admitted (an unadmit after the sync that installed it).
        let plan = plan_sync(only("local"), &store, &ssh).await.unwrap();
        let c = claude_actions(&plan, "local", "c");
        assert_eq!(c.len(), 1, "{c:?}");
        assert_eq!(c[0].op, ActionOp::Noop);
        assert_eq!(c[0].catalog.as_deref(), Some("acme"));
        assert!(c[0].reason.as_deref().unwrap().contains("not accepted"), "{:?}", c[0].reason);

        // An org change (local joins org 11): the same.
        {
            let s = store.lock().unwrap();
            s.conn_ref()
                .execute("INSERT INTO orgs (id, name, created_at) VALUES (11, 'other', 0)", [])
                .unwrap();
            s.set_host_org("local", Some(11)).unwrap();
        }
        let plan = plan_sync(only("local"), &store, &ssh).await.unwrap();
        assert!(
            plan.hosts.iter().all(|h| h.actions.iter().all(|a| a.op != ActionOp::Remove)),
            "{:?}",
            plan.hosts
        );
        assert_eq!(claude_actions(&plan, "local", "c")[0].op, ActionOp::Noop);
    }

    /// M2 carry 1: an org catalog that failed to load (a problem entry)
    /// never makes its installed assets read as orphans.
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn an_org_catalog_that_failed_to_load_never_plans_a_remove() {
        let _lock = super::super::lock_registry_for_test();
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());
        seed_manifest(home.path(), &[("skill/c", "acme")]);
        let store = store_with_local(Arc::new(RecordingEventBus::new()));
        let (personal_id, acme_id) = with_acme(&store);
        store.lock().unwrap().set_host_org("local", Some(10)).unwrap();
        install(
            personal_id,
            vec![],
            acme_id,
            repo::Catalog { load_error: Some("boom".into()), ..Default::default() },
        );
        let ssh = Arc::new(SshClient::new());

        let plan = plan_sync(only("local"), &store, &ssh).await.unwrap();
        assert!(
            plan.hosts.iter().all(|h| h.actions.iter().all(|a| a.op != ActionOp::Remove)),
            "{:?}",
            plan.hosts
        );
        let c = claude_actions(&plan, "local", "c");
        assert_eq!(c.len(), 1, "{c:?}");
        assert!(c[0].reason.as_deref().unwrap().contains("failed to load"), "{:?}", c[0].reason);
    }

    /// M2 carry 4: the withheld `Noop` is only for an asset nothing else
    /// supplies — here acme supplies the same name, so acme's action stands
    /// alone.
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn the_withheld_noop_is_skipped_when_another_catalog_supplies_the_name() {
        let _lock = super::super::lock_registry_for_test();
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());
        seed_manifest(home.path(), &[("skill/s", "personal")]);
        let store = store_with_local(Arc::new(RecordingEventBus::new()));
        let (personal_id, acme_id) = with_acme(&store);
        store.lock().unwrap().set_host_org("local", Some(10)).unwrap();
        install(
            personal_id,
            vec![body_skill("s", "private")],
            acme_id,
            repo::Catalog { assets: vec![body_skill("s", "private")], ..Default::default() },
        );
        let ssh = Arc::new(SshClient::new());

        let plan = plan_sync(only("local"), &store, &ssh).await.unwrap();
        let s = claude_actions(&plan, "local", "s");
        assert_eq!(s.len(), 1, "{s:?}");
        assert_eq!(s[0].catalog.as_deref(), Some("acme"));
        assert_ne!(
            s[0].reason.as_deref(),
            Some("private; withheld from org host, not removed")
        );
    }
```

Replace the body of `an_org_bound_unlayered_remote_host_with_allow_unlayered_keeps_withheld_assets` (M2 test (b), carry 9) so the remote host is really scanned:

```rust
    /// A fake `ssh` that runs the remote command locally: everything up to
    /// `-- <host>` is dropped and the rest goes to `sh -c`, which re-parses it
    /// exactly as the remote login shell would (`bash -lc '<script>'`).
    #[cfg(unix)]
    fn ssh_running_locally(dir: &std::path::Path) -> Arc<SshClient> {
        use crate::tmux::fake_exec::{write_exec, PROBE_GUARD};
        let bin = write_exec(
            dir,
            "ssh",
            &format!(
                "#!/bin/sh\n{PROBE_GUARD}\
                 case \"$*\" in *'-O check'*|*'-O exit'*) exit 0;; esac\n\
                 while [ \"$#\" -gt 0 ] && [ \"$1\" != \"--\" ]; do shift; done\n\
                 shift 2\n\
                 exec sh -c \"$*\"\n"
            ),
        );
        Arc::new(SshClient::with_ssh_binary(bin))
    }

    /// Fix round 2, item (b) — no longer vacuous (Rulings R20): a REMOTE
    /// org-bound unlayered host planned with `allow_unlayered` is really
    /// scanned (fake `ssh`, temp `HOME`), and its already-synced private
    /// skill is kept with the withheld `Noop`, never removed.
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn an_org_bound_unlayered_remote_host_with_allow_unlayered_keeps_withheld_assets() {
        let _lock = super::super::lock_registry_for_test();
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());
        seed_manifest(home.path(), &[("skill/s", "personal")]);
        let repo_dir = tempfile::tempdir().unwrap();
        let files = one_skill("b\n");
        load_catalog(
            repo_dir.path(),
            &files.iter().map(|(a, b)| (*a, b.as_str())).collect::<Vec<_>>(),
        );
        let store = store_with_local(Arc::new(RecordingEventBus::new()));
        {
            let s = store.lock().unwrap();
            s.insert_host("oci", Some("oci")).unwrap();
            s.update_host_probe("oci", true, None, None, 1).unwrap();
            s.conn_ref()
                .execute("INSERT INTO orgs (id, name, created_at) VALUES (10, 'acme', 0)", [])
                .unwrap();
            s.set_host_org("oci", Some(10)).unwrap();
        }
        let bin_dir = tempfile::tempdir().unwrap();
        let ssh = ssh_running_locally(bin_dir.path());
        let plan = plan_sync(
            PlanArgs {
                host_alias: Some("oci".into()),
                allow_unlayered: true,
                ..PlanArgs::default()
            },
            &store,
            &ssh,
        )
        .await
        .unwrap();
        let claude: Vec<&HostPlan> = plan.hosts.iter().filter(|h| h.harness == "claude").collect();
        assert_eq!(claude.len(), 1, "{:?}", plan.hosts);
        assert_eq!(claude[0].status, "planned", "really scanned: {:?}", claude[0].detail);
        assert!(
            plan.hosts.iter().all(|h| h.actions.iter().all(|a| a.op != ActionOp::Remove)),
            "{:?}",
            plan.hosts
        );
        let s = claude_actions(&plan, "oci", "s");
        assert_eq!(s.len(), 1, "{s:?}");
        assert_eq!(
            s[0].reason.as_deref(),
            Some("private; withheld from org host, not removed")
        );
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p fleet-core catalog::effective`, `cargo test -p fleet-core catalog::sync` — Expected: compile errors (`held_back`, `KeepRules`, `held`).

- [ ] **Step 3: Implement**

`effective.rs` — imports: add `crate::store::CatalogRow`. Add to `EffectiveSet` (after `withheld`):

```rust
    /// Assets M3: the catalogs composed for this host — accepted, loaded and
    /// resolved. A manifest entry may be removed as an orphan only when its
    /// catalog is one of these (Rulings R6). `personal` is always named
    /// "personal". Never crosses the wire; `#[serde(default)]` for symmetry.
    #[serde(default)]
    pub speaks_for: BTreeSet<String>,
    /// Assets M3: catalog → why its manifest entries on this host are kept
    /// rather than removed: not loaded, failed to load, not accepted (an org
    /// change or an unadmit), or not resolvable for this host (R6, R7).
    /// Copied into `Resolution::held_back` by `resolve_preview`.
    #[serde(default)]
    pub held_back: BTreeMap<String, String>,
```

Replace `effective_for_host` with a shared store phase plus two entry points:

```rust
/// Everything the store says about one host, read under one guard.
struct StorePhase {
    host_org: Option<i64>,
    personal_id: Option<i64>,
    configured: Vec<CatalogRow>,
    admitted: Vec<i64>,
    rows_by_catalog: BTreeMap<i64, Vec<HostLayerRow>>,
}

fn read_store(store: &Mutex<Store>, host_alias: &str) -> Result<StorePhase, IpcError> {
    let s = lock(store)?;
    let host_org = s.host_org(host_alias)?;
    let configured = s.list_catalogs()?;
    let admitted = s.host_admissions(host_alias)?;
    let mut personal_id = None;
    let mut rows_by_catalog = BTreeMap::new();
    for c in &configured {
        if c.org_id.is_none() {
            personal_id = Some(c.id);
        }
        rows_by_catalog.insert(c.id, s.get_host_layers_for(host_alias, c.id)?);
    }
    Ok(StorePhase { host_org, personal_id, configured, admitted, rows_by_catalog })
}

fn compose_from(
    catalogs: &BTreeMap<i64, Catalog>,
    host_alias: &str,
    st: &StorePhase,
) -> Result<EffectiveSet, IpcError> {
    compose(catalogs, &st.configured, host_alias, st.host_org, &st.admitted, |cat: &Catalog| {
        // A hand-built catalog with id 0 stands for the personal one.
        let id = if cat.id == 0 && cat.org_id.is_none() { st.personal_id } else { Some(cat.id) };
        id.and_then(|id| st.rows_by_catalog.get(&id))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    })
}

/// Compute `host_alias`'s effective catalog against the live registry.
///
/// **Lock order.** All store reads (org, catalogs, admissions, every
/// catalog's layer rows for this host) happen under ONE store guard that is
/// dropped before the registry is taken; the registry closure never touches
/// the store. Store → registry is never allowed (see `registry`).
pub fn effective_for_host(store: &Mutex<Store>, host_alias: &str) -> Result<EffectiveSet, IpcError> {
    let st = read_store(store, host_alias)?;
    registry::with_catalogs(|catalogs| compose_from(catalogs, host_alias, &st))
}

/// [`effective_for_host`] against a snapshot the caller already took
/// (`registry::snapshot`), so `plan_sync` scans and plans one view (R9).
pub fn effective_for_host_in(
    store: &Mutex<Store>,
    host_alias: &str,
    catalogs: &BTreeMap<i64, Catalog>,
) -> Result<EffectiveSet, IpcError> {
    let st = read_store(store, host_alias)?;
    compose_from(catalogs, host_alias, &st)
}
```

In `compose`: add the parameter `configured: &[CatalogRow]` after `catalogs`; declare `let mut speaks_for: BTreeSet<String> = BTreeSet::new(); let mut held_back: BTreeMap<String, String> = BTreeMap::new();` next to the other accumulators; replace the head of the per-catalog loop (from `let accepts = …` through `let res = resolve_rows(…)?;`) with:

```rust
        // `personal` is named "personal" whatever a hand-built catalog says
        // (schema CHECK: the no-org catalog's name IS "personal"), so a
        // manifest entry's default catalog always matches it.
        let label = if cat.org_id.is_none() { "personal".to_string() } else { cat.name.clone() };
        let accepts = acceptance(host_org, cat.id, cat.org_id, admitted);
        if accepts == Acceptance::No {
            held_back.insert(
                label.clone(),
                format!(
                    "catalog {label} is not accepted by host {host_alias} (an org change or an unadmit); \
                     its assets are kept, not removed"
                ),
            );
            continue;
        }
        if let Some(err) = &cat.load_error {
            held_back.insert(
                label.clone(),
                format!("catalog {label} failed to load ({err}); its assets are kept, not removed"),
            );
            continue;
        }
        let res = match resolve_rows(cat, host_alias, rows_for(cat)) {
            Ok(res) => res,
            // Rulings R7: an org catalog that cannot resolve for this host is
            // held back for it; personal keeps failing the host (M1/M2).
            Err(e) if cat.org_id.is_some() => {
                held_back.insert(
                    label.clone(),
                    format!(
                        "catalog {label} cannot resolve for {host_alias}: {}; its assets are kept, not removed",
                        e.message
                    ),
                );
                continue;
            }
            Err(e) => return Err(e),
        };
        speaks_for.insert(label);
```

After the loop (before the collision pass):

```rust
    // A configured catalog the registry does not hold at all.
    for row in configured {
        let label = if row.org_id.is_none() { "personal" } else { row.name.as_str() };
        if !speaks_for.contains(label) && !held_back.contains_key(label) {
            held_back.insert(
                label.to_string(),
                format!("catalog {label} is not loaded; its assets are kept, not removed"),
            );
        }
    }
```

and add `speaks_for, held_back,` to the returned `EffectiveSet`. Update the module doc comment ("Admissions … arrive in M3") to say admissions are read from `host_catalogs`.

`resolve.rs` — add to `Resolution` after `withheld`:

```rust
    /// Assets M3: catalog → why its assets on this host are kept but no
    /// longer managed (`effective::EffectiveSet::held_back`). Empty on the
    /// single-catalog path. `#[serde(default)]`: travels the wire.
    #[serde(default)]
    pub held_back: BTreeMap<String, String>,
```

and `held_back: BTreeMap::new(),` in its two literals in `resolve()`. `mod.rs` `resolve_preview`: add `held_back: eff.held_back,`. `mcp/tools/assets.rs` `resolve_preview`: add `"held_back": res.held_back,` to the JSON, and extend the tool description's last sentence: `… every private asset an org host withheld silently, and every catalog held back for this host (not loaded, not accepted) with why. Nothing is written. …`.

`manifest.rs` — replace `orphans` and add `held` (import `std::collections::BTreeSet`):

```rust
    /// Manifest entries whose `(kind, name)` is no longer in `catalog` — the
    /// asset was removed or renamed — and whose own catalog speaks for this
    /// host (`speaks_for`; `None` = every catalog does, Rulings R8). Spec: an
    /// orphan is "a manifest entry whose catalog no longer has it".
    pub fn orphans<'a>(
        &'a self,
        catalog: &Catalog,
        speaks_for: Option<&BTreeSet<String>>,
    ) -> Vec<(&'a str, &'a ManifestEntry)> {
        self.assets
            .iter()
            .filter(|(key, _)| match Self::split_key(key) {
                Some((kind, name)) => catalog.find(kind, &name).is_none(),
                None => true,
            })
            .filter(|(_, entry)| speaks_for.is_none_or(|s| s.contains(&entry.catalog)))
            .map(|(key, entry)| (key.as_str(), entry))
            .collect()
    }

    /// Entries absent from `catalog` whose catalog does NOT speak for this
    /// host: nobody here can say it dropped them, so they are kept (R6).
    /// Unparseable keys are `orphans`' business, never listed here.
    pub fn held<'a>(
        &'a self,
        catalog: &Catalog,
        speaks_for: &BTreeSet<String>,
    ) -> Vec<(&'a str, &'a ManifestEntry)> {
        self.assets
            .iter()
            .filter(|(key, entry)| {
                !speaks_for.contains(&entry.catalog)
                    && Self::split_key(key).is_some_and(|(kind, name)| catalog.find(kind, &name).is_none())
            })
            .map(|(key, entry)| (key.as_str(), entry))
            .collect()
    }
```

`plan.rs` — add the struct and helper; change `compute_host_plan`'s last parameter to `keep: &KeepRules`:

```rust
/// What `compute_host_plan`'s orphan pass must leave on the host (Assets
/// M2 + M3). `Default` keeps nothing extra and lets every catalog speak —
/// what every caller outside `plan_sync` wants.
#[derive(Debug, Clone, Default)]
pub struct KeepRules {
    /// `(kind, name)` refused or withheld by the effective catalog (M2).
    pub protected: BTreeSet<(Kind, String)>,
    /// The catalogs that speak for this host (`EffectiveSet::speaks_for`);
    /// `None` = all. An orphan `Remove` is planned only for an entry of one.
    pub speaks_for: Option<BTreeSet<String>>,
    /// Catalog → why its entries are kept (`EffectiveSet::held_back`).
    pub held_back: BTreeMap<String, String>,
}

impl KeepRules {
    fn held_reason(&self, catalog: &str) -> String {
        self.held_back.get(catalog).cloned().unwrap_or_else(|| {
            format!("catalog {catalog} is not configured on this fleet; its assets are kept, not removed")
        })
    }
}

/// A `Noop` for an entry whose catalog does not speak for this host (R6).
pub(crate) fn held_noop(kind: Kind, name: &str, catalog: &str, reason: String) -> Action {
    Action {
        kind: kind.as_str().to_string(),
        name: name.to_string(),
        op: ActionOp::Noop,
        catalog: Some(catalog.to_string()),
        reason: Some(reason),
        files: Vec::new(),
        merges: Vec::new(),
        backup: false,
        secrets: Vec::new(),
        missing_secrets: Vec::new(),
        plan: None,
        expected: BTreeMap::new(),
        secret_files: BTreeSet::new(),
        remove_entry: None,
        plugin: None,
    }
}
```

In `compute_host_plan`: `for (key, entry) in manifest.orphans(catalog, keep.speaks_for.as_ref()) {` and `if keep.protected.contains(&(kind, name.clone())) {`; then, after that loop:

```rust
    // Assets M3 (R6): an entry whose catalog does not speak for this host —
    // not loaded, failed to load, no longer accepted, not configured — is
    // kept and reported, never removed (spec: no automatic remove).
    if let Some(speaks) = keep.speaks_for.as_ref() {
        for (key, entry) in manifest.held(catalog, speaks) {
            let Some((kind, name)) = Manifest::split_key(key) else {
                continue;
            };
            if !filter.matches(kind, &name) || keep.protected.contains(&(kind, name.clone())) {
                continue;
            }
            actions.push(held_noop(kind, &name, &entry.catalog, keep.held_reason(&entry.catalog)));
        }
    }
```

Update rule 6 of `compute_host_plan`'s doc comment to mention `speaks_for` and the held `Noop`. Change every test call site from `&BTreeSet::new()` to `&KeepRules::default()` (`plan.rs`: `plan_for` and two in `a_name_filter_narrows_the_plan_including_orphans`; `sync/apply.rs`: five, with `plan::KeepRules::default()`), removing a now-unused `BTreeSet` import if the compiler flags it.

`inventory.rs` `compute_states`: `for (key, _entry) in manifest.orphans(catalog, None) {` with the comment `// None (R8): the inventory decides nothing; only the plan keeps held entries.`

`sync/mod.rs` `plan_sync`:
- replace `let catalog = catalog()?;` with

```rust
    // One registry view for the whole plan (Rulings R9, M2 carry 5): the scan
    // union and every host's effective set are built from the same snapshot.
    let snapshot = super::registry::snapshot()?;
    let catalog = super::registry::union_of(&snapshot).ok_or_else(|| {
        IpcError::new(super::E_CATALOG_NOT_CONFIGURED, "catalog not loaded; call catalog_load")
    })?;
```

- `effective::effective_for_host(store, &h.alias)` → `effective::effective_for_host_in(store, &h.alias, &snapshot)`;
- after `protected_keys` is built:

```rust
        let keep = plan::KeepRules {
            protected: protected_keys,
            speaks_for: Some(eff.speaks_for.clone()),
            held_back: eff.held_back.clone(),
        };
```

  and pass `&keep` to `compute_host_plan`;
- the withheld loop condition becomes

```rust
                            // M2 carry 4: only when no other catalog supplies
                            // the same name — then that catalog's own action
                            // already speaks for it.
                            if manifest.assets.contains_key(&Manifest::key(*kind, name))
                                && eff.catalog.find(*kind, name).is_none()
                            {
```

- [ ] **Step 4: Regenerate the control-API reference** (the `resolve_preview` description changed)

Run: `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`, then `cargo test -p fleet-core reference_is_current`. Expected: PASS; `docs/control-api-reference.md` changed.

- [ ] **Step 5: Run to verify pass**

Run: `cargo test -p fleet-core catalog::effective`, `cargo test -p fleet-core catalog::sync`, `cargo test -p fleet-core catalog::inventory`, `cargo test -p fleet-core catalog`, `cargo test -p fleet-core mcp::tools`. Expected: PASS, including every M2 test unchanged in outcome (personal-only parity).

- [ ] **Step 6: Commit**

```bash
git add crates/fleet-core/src/service/catalog/ crates/fleet-core/src/mcp/tools/assets.rs docs/control-api-reference.md
git commit -m "feat(sync): admissions in the effective set; a catalog that cannot speak for a host never plans a remove"
```

---

### Task 4: Catalog management service; per-catalog layers; authoring carries

**Files:**
- Create: `crates/fleet-core/src/service/catalog/catalogs.rs`
- Modify: `crates/fleet-core/src/service/catalog/mod.rs` (`pub mod catalogs;`, `list_layers_for`, `set_host_layers_for`, `list_layers`, `set_host_layers`)
- Modify: `crates/fleet-core/src/service/catalog/author.rs` (`create`, `lint_in_repo`, `lint_everything`)

**Interfaces:**
- Consumes: Task 1 store API; `load_catalog`, `problem_entry`, `expand_home`, `registry::{install, remove, with_catalogs, with_catalog_row}` (Task 2).
- Produces:

```rust
// catalogs.rs
pub const PERSONAL: &str = "personal";
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddCatalogArgs { pub name: String, pub repo_path: String, #[serde(default)] pub remote_url: Option<String>, #[serde(default)] pub org: Option<String> }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogStatus { pub id: i64, pub name: String, pub org_id: Option<i64>, #[serde(default)] pub org: Option<String>, pub repo_path: String, pub remote_url: Option<String>, pub head_commit: Option<String>, pub last_loaded_at: Option<i64>, pub state: String, #[serde(default, skip_serializing_if = "Option::is_none")] pub problem: Option<String>, pub asset_count: usize, #[serde(default)] pub admitted: Vec<String>, #[serde(default)] pub granted: Vec<String> }
pub fn catalog_named(name: &str, store: &Mutex<Store>) -> Result<CatalogRow, IpcError>;
pub fn config_row(row: &CatalogRow) -> CatalogConfigRow;
pub fn list_catalogs(store: &Mutex<Store>) -> Result<Vec<CatalogStatus>, IpcError>;
pub fn add_catalog(args: AddCatalogArgs, store: &Mutex<Store>) -> Result<CatalogStatus, IpcError>;
pub fn remove_catalog(name: &str, store: &Mutex<Store>) -> Result<CatalogRemoval, IpcError>;
pub fn admit(host_alias: &str, catalog: &str, store: &Mutex<Store>) -> Result<Vec<String>, IpcError>;
pub fn unadmit(host_alias: &str, catalog: &str, store: &Mutex<Store>) -> Result<Vec<String>, IpcError>;

// mod.rs
pub fn list_layers_for(row: &CatalogRow, store: &Mutex<Store>) -> Result<LayerListing, IpcError>;
pub fn set_host_layers_for(host_alias: &str, row: &CatalogRow, role: Option<&str>, contexts: &[&str], store: &Mutex<Store>) -> Result<Vec<HostLayerRow>, IpcError>;
```

- [ ] **Step 1: Write the failing tests**

`catalogs.rs` tests module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc_error::codes;
    use crate::service::catalog::lock_registry_for_test;

    /// A committed checkout with `catalog.yaml` at `schema` and skills `s…`.
    fn git_repo(tag: &str, schema: u32, skills: &[&str]) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("fleet-catalogs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("catalog.yaml"), format!("schema_version: {schema}\n")).unwrap();
        for s in skills {
            std::fs::create_dir_all(root.join(format!("skills/{s}"))).unwrap();
            std::fs::write(
                root.join(format!("skills/{s}/asset.yaml")),
                format!("kind: skill\nname: {s}\ndescription: d\n"),
            )
            .unwrap();
            std::fs::write(root.join(format!("skills/{s}/body.md")), "b\n").unwrap();
        }
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["config", "user.email", "t@t"],
            &["config", "user.name", "t"],
            &["add", "."],
            &["commit", "-q", "-m", "init"],
        ] {
            let o = crate::proc::std_command("git").args(args).current_dir(&root).output().unwrap();
            assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        }
        root
    }

    fn store_with_org() -> Mutex<Store> {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        s.add_org("acme", None, false).unwrap();
        Mutex::new(s)
    }

    fn add(name: &str, path: &std::path::Path, org: Option<&str>) -> AddCatalogArgs {
        AddCatalogArgs {
            name: name.into(),
            repo_path: path.to_string_lossy().into(),
            remote_url: None,
            org: org.map(String::from),
        }
    }

    #[test]
    fn add_list_admit_and_remove_an_org_catalog() {
        let _g = lock_registry_for_test();
        let store = store_with_org();
        add_catalog(add("personal", &git_repo("p", 1, &["a"]), None), &store).unwrap();
        let acme = add_catalog(add("acme", &git_repo("acme", 1, &["c", "d"]), Some("ACME")), &store).unwrap();
        assert_eq!(
            (acme.state.as_str(), acme.asset_count, acme.org.as_deref()),
            ("loaded", 2, Some("acme")),
            "the org is matched case-insensitively"
        );

        assert_eq!(admit("h", "acme", &store).unwrap(), vec!["acme".to_string()]);
        store.lock().unwrap().insert_client_token("ops", "aa11", "full").unwrap();
        store.lock().unwrap().set_client_catalog_grant("ops", acme.id, true).unwrap();
        let listed = list_catalogs(&store).unwrap();
        assert_eq!(listed.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["personal", "acme"]);
        assert_eq!(listed[1].admitted, vec!["h".to_string()]);
        assert_eq!(listed[1].granted, vec!["ops".to_string()]);
        assert!(listed[0].admitted.is_empty());

        assert!(unadmit("h", "acme", &store).unwrap().is_empty());
        let gone = remove_catalog("acme", &store).unwrap();
        assert_eq!((gone.id, gone.grants), (acme.id, 1));
        assert!(registry::get(acme.id).unwrap().is_none(), "evicted from the registry");
        assert_eq!(list_catalogs(&store).unwrap().len(), 1);
        registry::clear().unwrap();
    }

    #[test]
    fn a_checkout_that_does_not_parse_is_added_as_a_problem() {
        let _g = lock_registry_for_test();
        let store = store_with_org();
        let st = add_catalog(add("acme", &git_repo("bad-schema", 99, &[]), Some("acme")), &store).unwrap();
        assert_eq!(st.state, "problem");
        assert!(st.problem.as_deref().unwrap().contains("schema_version 99"), "{:?}", st.problem);
        registry::clear().unwrap();
    }

    #[test]
    fn add_admit_and_remove_refuse_what_cannot_be() {
        let _g = lock_registry_for_test();
        let store = store_with_org();
        let repo = git_repo("refuse", 1, &[]);
        let code = |r: Result<CatalogStatus, IpcError>| r.unwrap_err().code;
        assert_eq!(code(add_catalog(add("../x", &repo, Some("acme")), &store)), codes::E_INVALID);
        assert_eq!(code(add_catalog(add("acme", &repo, None)), &store)), codes::E_INVALID);
        assert_eq!(code(add_catalog(add("acme", &repo, Some("nope")), &store)), codes::E_NOTFOUND);
        assert_eq!(code(add_catalog(add("personal", &repo, Some("acme")), &store)), codes::E_INVALID);
        assert_eq!(
            code(add_catalog(add("acme", std::path::Path::new("/nonexistent/fleet-m3"), Some("acme")), &store)),
            codes::E_CATALOG_GIT,
            "a checkout that is not there is refused before anything is recorded"
        );
        assert!(store.lock().unwrap().get_catalog_by_name("acme").unwrap().is_none());

        add_catalog(add("acme", &repo, Some("acme")), &store).unwrap();
        store.lock().unwrap().set_catalog_config(&git_repo("refuse-p", 1, &[]).to_string_lossy(), None).unwrap();
        assert_eq!(admit("h", "personal", &store).unwrap_err().code, codes::E_INVALID);
        assert_eq!(admit("nohost", "acme", &store).unwrap_err().code, codes::E_NOTFOUND);
        assert_eq!(admit("h", "nocat", &store).unwrap_err().code, codes::E_NOTFOUND);
        let org = store.lock().unwrap().list_orgs().unwrap()[0].id;
        store.lock().unwrap().set_host_org("h", Some(org)).unwrap();
        assert_eq!(admit("h", "acme", &store).unwrap_err().code, codes::E_INVALID, "R4: no org only");
        assert_eq!(remove_catalog("personal", &store).unwrap_err().code, codes::E_INVALID);
        registry::clear().unwrap();
    }
}
```

`mod.rs` tests module (M2 carry 7):

```rust
    /// Assets M3 (M2 carry 7): layers are listed and assigned per catalog —
    /// personal's listing no longer shows another catalog's rows, and an
    /// assignment answers with the rows of the catalog it was made in.
    #[test]
    fn layers_are_listed_and_assigned_per_catalog() {
        let _g = lock_registry_for_test();
        let store = configured_store_with_layers("m3-layers");
        let org = store.lock().unwrap().add_org("acme", None, false).unwrap();
        let acme = store
            .lock()
            .unwrap()
            .upsert_catalog("acme", &repo_with_layers("m3-layers-acme").to_string_lossy(), None, Some(org.id))
            .unwrap();
        load_catalog(acme.id, false, &store).unwrap();

        let personal_rows = set_host_layers("local", Some("core"), &[], &store).unwrap();
        assert_eq!(personal_rows.len(), 1);
        let rows = set_host_layers_for("local", &acme, Some("core"), &["extra"], &store).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.catalog_id == acme.id), "{rows:?}");

        assert_eq!(list_layers(&store).unwrap().hosts.len(), 1);
        let listing = list_layers_for(&acme, &store).unwrap();
        assert_eq!((listing.hosts.len(), listing.layers.len()), (2, 2));
        assert_eq!(
            set_host_layers_for("local", &acme, Some("extra"), &[], &store).unwrap_err().code,
            codes::E_INVALID,
            "the axis is checked against THAT catalog's layers"
        );
        registry::clear().unwrap();
    }

    #[test]
    fn an_unloaded_org_catalog_has_no_layers_to_list() {
        let _g = lock_registry_for_test();
        let store = configured_store_with_layers("m3-unloaded");
        let org = store.lock().unwrap().add_org("acme", None, false).unwrap();
        let acme = store.lock().unwrap().upsert_catalog("acme", "/nowhere", None, Some(org.id)).unwrap();
        assert_eq!(list_layers_for(&acme, &store).unwrap_err().code, E_CATALOG_NOT_CONFIGURED);
        registry::clear().unwrap();
    }
```

`author.rs` tests module (add `Scope` to its `model` import):

```rust
    /// Rulings R15: a copy is a new asset — private until marked, whatever
    /// the original's scope.
    #[test]
    fn duplicate_from_makes_a_private_copy_of_a_shared_asset() {
        let _g = lock_registry_for_test();
        let root = init_repo("dup-scope");
        let store = configured_store(&root);
        create(CreateArgs { kind: Kind::Skill, name: "src".into(), duplicate_from: None }, &store).unwrap();
        let mut shared = catalog_asset(Kind::Skill, "src").unwrap();
        shared.header.scope = Scope::Shared;
        shared.header.description = "A shared skill that is worth copying once.".into();
        shared.body = "# src\n\nShared body.\n".into();
        update(UpdateArgs { asset: shared }, &store).unwrap();
        create(
            CreateArgs { kind: Kind::Skill, name: "copy".into(), duplicate_from: Some("src".into()) },
            &store,
        )
        .unwrap();
        assert_eq!(catalog_asset(Kind::Skill, "copy").unwrap().header.scope, Scope::Private);
        assert_eq!(catalog_asset(Kind::Skill, "src").unwrap().header.scope, Scope::Shared);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p fleet-core catalog::catalogs`, `cargo test -p fleet-core layers_are_listed_and_assigned_per_catalog`, `cargo test -p fleet-core duplicate_from_makes_a_private_copy` — Expected: compile errors / the last FAILS (`Shared` copied).

- [ ] **Step 3: Implement**

`catalogs.rs`:

```rust
//! The set of catalogs (Assets M3): add, list and remove them, and which
//! hosts with no org admit an org catalog (`host_catalogs`). Shared by the
//! hub's `catalog_admin` and `fleet-hub catalog …`. Removing is config only:
//! the checkout on disk is never touched (spec, Hub CLI and MCP).

use super::{registry, repo};
use crate::ipc_error::{codes, lock, IpcError};
use crate::store::{CatalogConfigRow, CatalogRemoval, CatalogRow, Store};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

pub const PERSONAL: &str = "personal";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddCatalogArgs {
    pub name: String,
    pub repo_path: String,
    #[serde(default)]
    pub remote_url: Option<String>,
    /// The owning org by name; required for every catalog but `personal`.
    #[serde(default)]
    pub org: Option<String>,
}

/// One catalog as `list_catalogs` / `catalog list` show it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogStatus {
    pub id: i64,
    pub name: String,
    pub org_id: Option<i64>,
    #[serde(default)]
    pub org: Option<String>,
    pub repo_path: String,
    pub remote_url: Option<String>,
    pub head_commit: Option<String>,
    pub last_loaded_at: Option<i64>,
    /// `loaded` | `problem` | `not_loaded`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
    pub asset_count: usize,
    /// Hosts with no org that admit this catalog.
    #[serde(default)]
    pub admitted: Vec<String>,
    /// Live paired clients granted this catalog (R19).
    #[serde(default)]
    pub granted: Vec<String>,
}

pub fn catalog_named(name: &str, store: &Mutex<Store>) -> Result<CatalogRow, IpcError> {
    lock(store)?.get_catalog_by_name(name)?.ok_or_else(|| {
        IpcError::new(codes::E_NOTFOUND, format!("no catalog named {name}; list them with list_catalogs"))
    })
}

pub fn config_row(row: &CatalogRow) -> CatalogConfigRow {
    CatalogConfigRow {
        repo_path: row.repo_path.clone(),
        remote_url: row.remote_url.clone(),
        head_commit: row.head_commit.clone(),
        last_loaded_at: row.last_loaded_at,
    }
}

/// Every catalog with its load state, personal first. Store rows are read
/// under one guard, released, then the registry is read (store → registry
/// is never allowed).
pub fn list_catalogs(store: &Mutex<Store>) -> Result<Vec<CatalogStatus>, IpcError> {
    let mut rows = Vec::new();
    {
        let s = lock(store)?;
        let orgs = s.list_orgs()?;
        for r in s.list_catalogs()? {
            let org = r.org_id.and_then(|id| orgs.iter().find(|o| o.id == id).map(|o| o.name.clone()));
            let admitted = s.catalog_admissions(r.id)?;
            let granted = s.catalog_grantees(r.id)?;
            rows.push((r, org, admitted, granted));
        }
    }
    registry::with_catalogs(|m| {
        let mut out = Vec::new();
        for (r, org, admitted, granted) in rows {
            let entry = if r.org_id.is_none() {
                m.values().find(|c| c.org_id.is_none())
            } else {
                m.get(&r.id)
            };
            let (state, problem, asset_count) = match entry {
                None => ("not_loaded", None, 0),
                Some(c) => match &c.load_error {
                    Some(e) => ("problem", Some(e.clone()), 0),
                    None => ("loaded", None, c.assets.len()),
                },
            };
            out.push(CatalogStatus {
                id: r.id,
                name: r.name,
                org_id: r.org_id,
                org,
                repo_path: r.repo_path,
                remote_url: r.remote_url,
                head_commit: r.head_commit,
                last_loaded_at: r.last_loaded_at,
                state: state.to_string(),
                problem,
                asset_count,
                admitted,
                granted,
            });
        }
        Ok(out)
    })
}

/// Add a catalog (or re-point one, R12), then load it. The checkout must be
/// there or clonable from `remote_url` before anything is recorded. An org
/// catalog that then fails to parse is kept as a problem entry (R5) and
/// reported as `state: problem`; `personal` failing is an error, as
/// `catalog set` has always been.
pub fn add_catalog(args: AddCatalogArgs, store: &Mutex<Store>) -> Result<CatalogStatus, IpcError> {
    super::validate::check_name(&args.name)?;
    let path = super::expand_home(args.repo_path.trim());
    if path.is_empty() {
        return Err(IpcError::new(codes::E_INVALID, "repo_path must not be empty"));
    }
    let remote = args.remote_url.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let org = args.org.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let org_id = match (args.name.as_str(), org) {
        (PERSONAL, None) => None,
        (PERSONAL, Some(_)) => {
            return Err(IpcError::new(codes::E_INVALID, "the personal catalog belongs to no org; drop --org"))
        }
        (name, None) => {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("catalog {name} needs --org: only `personal` belongs to no org"),
            ))
        }
        (_, Some(org)) => Some(
            lock(store)?
                .list_orgs()?
                .into_iter()
                .find(|o| o.name.eq_ignore_ascii_case(org))
                .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no org named {org}")))?
                .id,
        ),
    };
    repo::ensure_repo(std::path::Path::new(&path), remote)?;
    let row = lock(store)?.upsert_catalog(&args.name, &path, remote, org_id)?;
    match super::load_catalog(row.id, false, store) {
        Ok(_) => {}
        Err(e) if row.org_id.is_none() => return Err(e),
        Err(e) => registry::install(super::problem_entry(&row, &e))?,
    }
    list_catalogs(store)?
        .into_iter()
        .find(|c| c.id == row.id)
        .ok_or_else(|| IpcError::new(codes::E_INTERNAL, format!("catalog {} vanished", row.name)))
}

/// Remove an org catalog (R13): config only, the checkout stays.
pub fn remove_catalog(name: &str, store: &Mutex<Store>) -> Result<CatalogRemoval, IpcError> {
    let removal = lock(store)?.remove_catalog(name)?;
    registry::remove(removal.id)?;
    Ok(removal)
}

fn admitted_names(s: &Store, host_alias: &str) -> Result<Vec<String>, IpcError> {
    let ids = s.host_admissions(host_alias)?;
    Ok(s.list_catalogs()?.into_iter().filter(|c| ids.contains(&c.id)).map(|c| c.name).collect())
}

/// Admit an org catalog on a host with no org (spec, Which catalogs a host
/// accepts; R4). Answers the host's admitted catalogs by name.
pub fn admit(host_alias: &str, catalog: &str, store: &Mutex<Store>) -> Result<Vec<String>, IpcError> {
    crate::validate::host_alias(host_alias)?;
    let s = lock(store)?;
    if s.get_host_row(host_alias)?.is_none() {
        return Err(IpcError::new(codes::E_NOTFOUND, format!("host {host_alias} not found")));
    }
    let row = s.get_catalog_by_name(catalog)?.ok_or_else(|| {
        IpcError::new(codes::E_NOTFOUND, format!("no catalog named {catalog}"))
    })?;
    if row.org_id.is_none() {
        return Err(IpcError::new(
            codes::E_INVALID,
            "every host takes the personal catalog already (all of it with no org, its shared \
             assets with one); only an org catalog is admitted",
        ));
    }
    if let Some(org) = s.host_org(host_alias)? {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!(
                "{host_alias} is in org {org}: it takes its own org's catalog and never another's; \
                 admission is for hosts with no org"
            ),
        ));
    }
    s.admit_host_catalog(host_alias, row.id)?;
    admitted_names(&s, host_alias)
}

/// Take an admission back. Works on any host (a leftover from before an org change, R4).
pub fn unadmit(host_alias: &str, catalog: &str, store: &Mutex<Store>) -> Result<Vec<String>, IpcError> {
    crate::validate::host_alias(host_alias)?;
    let s = lock(store)?;
    if s.get_host_row(host_alias)?.is_none() {
        return Err(IpcError::new(codes::E_NOTFOUND, format!("host {host_alias} not found")));
    }
    let row = s.get_catalog_by_name(catalog)?.ok_or_else(|| {
        IpcError::new(codes::E_NOTFOUND, format!("no catalog named {catalog}"))
    })?;
    s.unadmit_host_catalog(host_alias, row.id)?;
    admitted_names(&s, host_alias)
}
```

`mod.rs` — add `pub mod catalogs;` to the module list; replace `list_layers` and `set_host_layers`, add the `_for` variants (import `crate::store::CatalogRow`):

```rust
/// One catalog's layer definitions plus every host's active assignment IN
/// THAT CATALOG (M2 carry 7: the rows of other catalogs no longer appear
/// next to definitions they do not belong to).
pub fn list_layers_for(row: &CatalogRow, store: &Mutex<Store>) -> Result<LayerListing, IpcError> {
    let hosts: Vec<_> = lock(store)?
        .list_all_host_layers()?
        .into_iter()
        .filter(|r| r.active && r.catalog_id == row.id)
        .collect();
    registry::with_catalog_row(row, |cat| {
        Ok(LayerListing { layers: cat.layers.iter().cloned().collect(), hosts })
    })
}

/// The personal catalog's layers and assignments.
pub fn list_layers(store: &Mutex<Store>) -> Result<LayerListing, IpcError> {
    let personal = lock(store)?.personal_catalog()?;
    match personal {
        Some(row) => list_layers_for(&row, store),
        None => with_catalog(|cat| {
            Ok(LayerListing { layers: cat.layers.iter().cloned().collect(), hosts: Vec::new() })
        }),
    }
}

/// Replace a host's assignment in one catalog: one optional role plus
/// contexts in application order, checked against THAT catalog's layer
/// definitions. Answers the host's rows in that catalog only (M2 carry 7).
pub fn set_host_layers_for(
    host_alias: &str,
    row: &CatalogRow,
    role: Option<&str>,
    contexts: &[&str],
    store: &Mutex<Store>,
) -> Result<Vec<crate::store::HostLayerRow>, IpcError> {
    crate::validate::host_alias(host_alias)?;
    require_host_exists(store, host_alias)?;
    check_no_name_collision(role, contexts)?;
    registry::with_catalog_row(row, |cat| {
        if let Some(r) = role {
            check_layer_axis(cat, r, layer::Axis::Role)?;
        }
        for c in contexts {
            check_layer_axis(cat, c, layer::Axis::Context)?;
        }
        Ok(())
    })?;
    let s = lock(store)?;
    s.set_host_layers_for(host_alias, row.id, role, contexts)?;
    Ok(s.get_host_layers_for(host_alias, row.id)?)
}

/// [`set_host_layers_for`] in the personal catalog — the desktop's command.
pub fn set_host_layers(
    host_alias: &str,
    role: Option<&str>,
    contexts: &[&str],
    store: &Mutex<Store>,
) -> Result<Vec<crate::store::HostLayerRow>, IpcError> {
    crate::validate::host_alias(host_alias)?;
    require_host_exists(store, host_alias)?;
    let row = lock(store)?.personal_catalog()?.ok_or_else(|| {
        IpcError::new(E_CATALOG_NOT_CONFIGURED, "catalog not loaded; call catalog_load")
    })?;
    set_host_layers_for(host_alias, &row, role, contexts, store)
}
```

`author.rs`:
- in `create`, in the `Some(from)` arm after `a.header.source = None;` add `a.header.scope = super::model::Scope::Private; // R15: a copy is private until marked`.
- replace `lint_in_repo` and `lint_everything` (R16):

```rust
/// Lint against the catalog as currently loaded (an unloaded catalog lints
/// against an empty one — no rule consults it). Borrows the registry; never
/// clones the catalog (M1 carry).
fn lint_in_repo(asset: &Asset, root: &Path) -> LintReport {
    let names = secrets_example_names(root);
    let exists = root.join(SECRETS_EXAMPLE).exists();
    registry::with_personal(|c| Ok(lint(asset, c, &names, exists)))
        .unwrap_or_else(|_| lint(asset, &Catalog::default(), &names, exists))
}

pub fn lint_everything(store: &Mutex<Store>) -> Result<LintAll, IpcError> {
    let root = repo_root(store)?;
    match registry::with_personal(|c| Ok(lint_all(c, &root))) {
        Ok(all) => Ok(all),
        Err(e) if e.code == super::E_CATALOG_NOT_CONFIGURED => Ok(lint_all(&Catalog::default(), &root)),
        Err(e) => Err(e),
    }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p fleet-core catalog::catalogs`, `cargo test -p fleet-core catalog::tests`, `cargo test -p fleet-core catalog::author` (the existing lint tests are R16's regression net), `cargo test -p fleet-core catalog`, `cargo test -p claude-fleet --lib catalog`. Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/service/catalog/
git commit -m "feat(catalog): add, list, remove and admit catalogs; layers per catalog; private copies; lint borrows"
```

---

### Task 5: `catalog_admin` — the `catalog` parameter, five actions, a grant per touched catalog

**Files:**
- Modify: `crates/fleet-core/src/service/catalog/admin.rs`
- Modify: `crates/fleet-core/src/service/catalog/sync/plan.rs` (`registry_catalogs_written`)
- Modify: `crates/fleet-core/src/mcp/tools/assets.rs` (`catalog_admin`, `may_admin_catalog`, `prepare_admin_call`, `import_assets`)
- Modify: `crates/fleet-core/src/mcp/tools/params.rs` (`CatalogAdminParams`)
- Modify: `crates/fleet-core/src/mcp/tools/tests_catalog_admin.rs`
- Modify: `docs/control-api-reference.md` (generated)

**Interfaces:**
- Consumes: Task 4's `catalogs::*`, `list_layers_for`, `set_host_layers_for`; Task 2's `load_catalog`, `load_all`, `ensure_fresh`; Task 1's grant API.
- Produces:

```rust
// admin.rs
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct CatalogNameArgs { pub name: String }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct AdmitArgs { pub host_alias: String, pub catalog: String }
// new AdminCall variants: "list_catalogs" => ListCatalogs, "add_catalog" => AddCatalog(AddCatalogArgs),
//   "remove_catalog" => RemoveCatalog(CatalogNameArgs), "admit_catalog" => AdmitCatalog(AdmitArgs),
//   "unadmit_catalog" => UnadmitCatalog(AdmitArgs)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Touches { Nothing, NewCatalog, Catalog(String) }
impl AdminCall { pub fn is_per_catalog(&self) -> bool; pub fn touches(&self, catalog: Option<&str>) -> Result<Touches, IpcError>; }
pub async fn run(call: AdminCall, catalog: Option<&str>, store: &Mutex<Store>, ssh: &Arc<SshClient>, reg: &Arc<CancellationRegistry>) -> Result<serde_json::Value, IpcError>;

// plan.rs
pub(crate) fn registry_catalogs_written(id: &str) -> Option<BTreeSet<String>>;

// mcp/tools/params.rs — CatalogAdminParams gains:
#[serde(default)] pub catalog: Option<String>,
```

- [ ] **Step 1: Write the failing tests**

`plan.rs` tests module:

```rust
    /// The `apply_sync` gate peeks which catalogs a parked plan writes from:
    /// every action that changes a host names its own, a `Remove` names the
    /// entry it undoes; `Noop`/`Blocked` name nothing; the plan stays parked.
    #[test]
    fn registry_catalogs_written_peeks_without_taking() {
        let mut hp = plan_for(&cat(), &Claude, &HostSnapshot::default(), &Manifest::default(), &secrets_map());
        for a in &mut hp.actions {
            a.catalog = Some("acme".into());
        }
        let first = hp.actions[0].clone();
        hp.actions.push(Action {
            op: ActionOp::Remove,
            catalog: None,
            remove_entry: Some(ManifestEntry { catalog: "old".into(), ..Default::default() }),
            ..first.clone()
        });
        hp.actions.push(Action { op: ActionOp::Noop, catalog: Some("ignored".into()), ..first });
        let id = registry_put(SyncPlan::new(vec![hp]));
        assert_eq!(
            registry_catalogs_written(&id),
            Some(BTreeSet::from(["acme".to_string(), "old".to_string()]))
        );
        assert!(registry_take(&id).is_some(), "peeking leaves the plan parked");
        assert_eq!(registry_catalogs_written("no-such-plan"), None);
    }
```

`admin.rs` tests module — add the five calls to `every_call()` at the end (declaration order) and a `touches` test:

```rust
            AdminCall::ListCatalogs,
            AdminCall::AddCatalog(super::super::catalogs::AddCatalogArgs {
                name: "acme".into(),
                repo_path: "/a".into(),
                remote_url: Some("git@example.com:a.git".into()),
                org: Some("acme".into()),
            }),
            AdminCall::RemoveCatalog(CatalogNameArgs { name: "acme".into() }),
            AdminCall::AdmitCatalog(AdmitArgs { host_alias: "h".into(), catalog: "acme".into() }),
            AdminCall::UnadmitCatalog(AdmitArgs { host_alias: "h".into(), catalog: "acme".into() }),
```

```rust
    /// R10/R11: which catalog a call needs a grant on, and which `catalog`
    /// parameters it refuses.
    #[test]
    fn touches_names_the_catalog_a_grant_is_checked_on() {
        let admit = AdminCall::AdmitCatalog(AdmitArgs { host_alias: "h".into(), catalog: "acme".into() });
        assert_eq!(admit.touches(None).unwrap(), Touches::Catalog("acme".into()));
        assert_eq!(admit.touches(Some("other")).unwrap_err().code, codes::E_INVALID);
        assert_eq!(AdminCall::ListCatalogs.touches(None).unwrap(), Touches::Nothing);
        let add = AdminCall::AddCatalog(super::super::catalogs::AddCatalogArgs {
            name: "acme".into(),
            repo_path: "/a".into(),
            remote_url: None,
            org: Some("acme".into()),
        });
        assert_eq!(add.touches(None).unwrap(), Touches::NewCatalog);
        assert_eq!(AdminCall::Config.touches(None).unwrap(), Touches::Catalog("personal".into()));
        assert_eq!(AdminCall::ListLayers.touches(Some("acme")).unwrap(), Touches::Catalog("acme".into()));
        assert_eq!(AdminCall::Push.touches(Some("personal")).unwrap(), Touches::Catalog("personal".into()));
        let e = AdminCall::Push.touches(Some("acme")).unwrap_err();
        assert!(e.message.contains("M4"), "{}", e.message);
        let e = AdminCall::LastSync.touches(Some("acme")).unwrap_err();
        assert!(e.message.contains("not per catalog"), "{}", e.message);
    }
```

Update `hostile_names_and_paths_are_refused_before_any_effect`'s call to `run(call, None, &store, &ssh, &reg)`.

`tests_catalog_admin.rs` — `call()` builds `CatalogAdminParams { action: action.into(), args, confirm_nonce, catalog: None }`, the direct call in `catalog_admin_answers_the_master_and_a_granted_full_unbound_client_only` gains `catalog: None`; then add:

```rust
/// [`call`] with the tool's `catalog` parameter.
async fn call_on(
    t: &FleetTools,
    caller: &Caller,
    action: &str,
    args: Option<Value>,
    catalog: Option<&str>,
) -> Result<CallToolResult, McpError> {
    enforce_mode(caller, "catalog_admin")?;
    enforce_admin(caller, "catalog_admin")?;
    t.catalog_admin(
        Extension(caller.clone()),
        Parameters(CatalogAdminParams {
            action: action.into(),
            args,
            confirm_nonce: None,
            catalog: catalog.map(String::from),
        }),
    )
    .await
}

/// `personal` and an `acme` org catalog (neither loadable), host `h` with no
/// org; `desk` holds personal, `ops` holds acme, `plain` holds nothing.
/// Returns `(store, desk_id, ops_id, plain_id, acme_id)`.
fn two_catalog_store() -> (Store, i64, i64, i64, i64) {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h").unwrap();
    s.set_catalog_config("/nonexistent/m3-personal", None).unwrap();
    let org = s.add_org("acme", None, false).unwrap();
    let acme = s.upsert_catalog("acme", "/nonexistent/m3-acme", None, Some(org.id)).unwrap();
    let desk = s.insert_client_token("desk", "aa11", "full").unwrap();
    s.set_client_assets_admin("desk", true).unwrap();
    let ops = s.insert_client_token("ops", "bb22", "full").unwrap();
    s.set_client_catalog_grant("ops", acme.id, true).unwrap();
    let plain = s.insert_client_token("plain", "cc33", "full").unwrap();
    (s, desk.id, ops.id, plain.id, acme.id)
}

/// Spec, Testing (authorization): a client without a grant on the catalog
/// an action touches cannot admit, unadmit, remove or read it; the grant on
/// one catalog says nothing about another; a personal-only or fleet-wide
/// action refuses another catalog instead of running on personal.
#[tokio::test]
async fn each_action_needs_a_grant_on_the_catalog_it_touches() {
    let _g = crate::service::catalog::lock_registry_for_test();
    let (s, desk, ops, _plain, _acme) = two_catalog_store();
    let t = tools(s);
    let desk = client(desk, TokenMode::Full, None);
    let ops = client(ops, TokenMode::Full, None);
    let admit = json!({ "host_alias": "h", "catalog": "acme" });

    let r = call(&t, &desk, "admit_catalog", Some(admit.clone()), None).await;
    assert_eq!(code_of(&r), "E_FORBIDDEN");
    assert!(r.unwrap_err().message.contains("--catalog acme"), "names the remedy");
    let r = call(&t, &ops, "admit_catalog", Some(admit.clone()), None).await;
    assert_eq!(code_of(&r), "OK", "{:?}", r.err());
    assert_eq!(t.store.lock().unwrap().host_admissions("h").unwrap().len(), 1);
    let r = call(&t, &Caller::master(), "unadmit_catalog", Some(admit), None).await;
    assert_eq!(code_of(&r), "OK", "{:?}", r.err());

    assert_eq!(code_of(&call_on(&t, &ops, "config", None, Some("acme")).await), "OK");
    assert_eq!(code_of(&call_on(&t, &ops, "config", None, None).await), "E_FORBIDDEN");
    assert_eq!(code_of(&call_on(&t, &desk, "config", None, Some("acme")).await), "E_FORBIDDEN");
    assert_eq!(code_of(&call_on(&t, &desk, "config", None, None).await), "OK", "parity");

    let m = Caller::master();
    let create = json!({ "kind": "skill", "name": "x" });
    assert_eq!(code_of(&call_on(&t, &m, "create_asset", Some(create), Some("acme")).await), "E_INVALID");
    assert_eq!(code_of(&call_on(&t, &m, "plan_sync", Some(json!({})), Some("acme")).await), "E_INVALID");

    let rm = json!({ "name": "acme" });
    assert_eq!(code_of(&call(&t, &desk, "remove_catalog", Some(rm.clone()), None).await), "E_FORBIDDEN");
    let r = call(&t, &ops, "remove_catalog", Some(rm), None).await;
    assert_eq!(code_of(&r), "OK", "{:?}", r.err());
}

/// R11: `list_catalogs` needs no grant (but a per-host token never reaches
/// `catalog_admin`); `add_catalog` is the master's alone.
#[tokio::test]
async fn list_catalogs_needs_no_grant_and_add_catalog_needs_the_master() {
    let _g = crate::service::catalog::lock_registry_for_test();
    let (s, desk, _ops, plain, _acme) = two_catalog_store();
    let t = tools(s);
    let r = call(&t, &client(plain, TokenMode::Full, None), "list_catalogs", None, None).await;
    assert_eq!(code_of(&r), "OK", "{:?}", r.err());
    assert_eq!(code_of(&call(&t, &host("h1"), "list_catalogs", None, None).await), "E_FORBIDDEN");

    let add = json!({ "name": "beta", "repo_path": "/nonexistent/m3-beta", "org": "acme" });
    let r = call(&t, &client(desk, TokenMode::Full, None), "add_catalog", Some(add.clone()), None).await;
    assert_eq!(code_of(&r), "E_FORBIDDEN");
    assert!(r.unwrap_err().message.contains("master"));
    let r = call(&t, &Caller::master(), "add_catalog", Some(add), None).await;
    assert_eq!(code_of(&r), codes::E_CATALOG_GIT, "past the gate: {:?}", r.err());
}

fn host_plan_writing(catalog: &str) -> crate::service::catalog::sync::plan::HostPlan {
    use crate::service::catalog::sync::plan::{Action, ActionOp, HostPlan};
    HostPlan {
        host_alias: "h".into(),
        harness: "claude".into(),
        status: "planned".into(),
        detail: None,
        actions: vec![Action {
            kind: "skill".into(),
            name: "c".into(),
            op: ActionOp::Create,
            catalog: Some(catalog.into()),
            reason: None,
            files: Vec::new(),
            merges: Vec::new(),
            backup: false,
            secrets: Vec::new(),
            missing_secrets: Vec::new(),
            plan: None,
            expected: Default::default(),
            secret_files: Default::default(),
            remove_entry: None,
            plugin: None,
        }],
        snapshot: Default::default(),
        manifest: Default::default(),
    }
}

/// R11: `apply_sync` needs a grant on every catalog its plan writes from.
#[tokio::test]
async fn apply_sync_needs_a_grant_on_every_catalog_its_plan_writes() {
    use crate::service::catalog::sync::plan;
    let _g = crate::service::catalog::lock_registry_for_test();
    let (s, desk, _ops, _plain, acme) = two_catalog_store();
    s.set_setting(guard::SETTING_CONFIRM_DESTRUCTIVE, "true").unwrap();
    let t = tools(s);
    let id = plan::registry_put(plan::SyncPlan::new(vec![host_plan_writing("acme")]));
    let desk_c = client(desk, TokenMode::Full, None);
    let args = json!({ "plan_id": id, "force_partial": false });

    let r = call(&t, &desk_c, "apply_sync", Some(args.clone()), None).await;
    assert_eq!(code_of(&r), "E_FORBIDDEN");
    assert!(r.unwrap_err().message.contains("acme"));
    t.store.lock().unwrap().set_client_catalog_grant("desk", acme, true).unwrap();
    let r = call(&t, &desk_c, "apply_sync", Some(args), None).await.unwrap_err();
    assert!(r.message.starts_with(codes::E_CONFIRM_REQUIRED), "past the grant: {}", r.message);
    assert!(plan::registry_take(&id).is_some(), "the gate only peeks");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p fleet-core tests_catalog_admin`, `cargo test -p fleet-core catalog::admin` — Expected: compile errors.

- [ ] **Step 3: Implement**

`plan.rs`:

```rust
/// Which catalogs the plan parked under `id` would write from: the
/// `catalog` of every action that changes a host (not `Noop`/`Blocked`), and
/// for a `Remove` the catalog of the entry it undoes. Peeks: the plan stays
/// parked. `None` when no unexpired plan has that id (`apply_sync` reports
/// that itself). For the per-catalog `apply_sync` gate (Assets M3, R11).
pub(crate) fn registry_catalogs_written(id: &str) -> Option<BTreeSet<String>> {
    let now = Instant::now();
    let map = plans();
    let (expires_at, plan) = map.get(id)?;
    if *expires_at <= now {
        return None;
    }
    Some(
        plan.hosts
            .iter()
            .flat_map(|h| &h.actions)
            .filter(|a| !matches!(a.op, ActionOp::Noop | ActionOp::Blocked))
            .filter_map(|a| {
                a.catalog
                    .clone()
                    .or_else(|| a.remove_entry.as_ref().map(|e| e.catalog.clone()))
            })
            .collect(),
    )
}
```

`admin.rs` — imports: `super::catalogs::{self, AddCatalogArgs, PERSONAL}`; add the arg structs; append to `admin_calls!` (after `"template" => Template(AssetRef),`):

```rust
    /// Assets M3: the set of catalogs and admissions (`catalogs.rs`).
    "list_catalogs" => ListCatalogs,
    "add_catalog" => AddCatalog(AddCatalogArgs),
    "remove_catalog" => RemoveCatalog(CatalogNameArgs),
    "admit_catalog" => AdmitCatalog(AdmitArgs),
    "unadmit_catalog" => UnadmitCatalog(AdmitArgs),
```

Add `| AdminCall::ListCatalogs` to `is_read`. Then:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogNameArgs {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdmitArgs {
    pub host_alias: String,
    pub catalog: String,
}

/// What a call needs a grant on (Assets M3, Rulings R11): the tool asks
/// `may_admin_catalog` once per touched catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Touches {
    /// `list_catalogs`: every caller that reaches `catalog_admin`.
    Nothing,
    /// `add_catalog`: no grant can name a catalog not created yet — master only.
    NewCatalog,
    /// One catalog, by name.
    Catalog(String),
}

impl AdminCall {
    /// The calls the tool's `catalog` parameter addresses (R10).
    pub fn is_per_catalog(&self) -> bool {
        matches!(
            self,
            AdminCall::Config | AdminCall::Load(_) | AdminCall::ListLayers | AdminCall::SetHostLayers(_)
        )
    }

    /// The calls that read or write the personal checkout — personal-only
    /// until M4's changeset apply gives them a per-catalog target (R10).
    fn is_authoring(&self) -> bool {
        matches!(
            self,
            AdminCall::Configure(_)
                | AdminCall::GetAsset(_)
                | AdminCall::Template(_)
                | AdminCall::CreateAsset(_)
                | AdminCall::UpdateAsset(_)
                | AdminCall::DeleteAsset(_)
                | AdminCall::AddResourceBytes(_)
                | AdminCall::RemoveResource(_)
                | AdminCall::LintAsset(_)
                | AdminCall::LintAll
                | AdminCall::CommitPending(_)
                | AdminCall::Push
                | AdminCall::RepoStatus
                | AdminCall::LayerTemplate(_)
                | AdminCall::WriteLayer(_)
                | AdminCall::DeleteLayer(_)
                | AdminCall::ImportHost(_)
        )
    }

    /// Which catalog this call touches, given the tool's `catalog` parameter
    /// (default `personal`) — or `E_INVALID` for a parameter it cannot honour.
    pub fn touches(&self, catalog: Option<&str>) -> Result<Touches, IpcError> {
        let named_in_args = |name: &str| match catalog {
            Some(c) if c != name => Err(IpcError::new(
                codes::E_INVALID,
                format!("{} names catalog {name} in its args; drop the catalog parameter ({c})", self.action()),
            )),
            _ => Ok(Touches::Catalog(name.to_string())),
        };
        let no_param = |t: Touches| match catalog {
            Some(c) => Err(IpcError::new(
                codes::E_INVALID,
                format!("{} takes no catalog parameter ({c})", self.action()),
            )),
            None => Ok(t),
        };
        match self {
            AdminCall::ListCatalogs => no_param(Touches::Nothing),
            AdminCall::AddCatalog(_) => no_param(Touches::NewCatalog),
            AdminCall::RemoveCatalog(a) => named_in_args(&a.name),
            AdminCall::AdmitCatalog(a) | AdminCall::UnadmitCatalog(a) => named_in_args(&a.catalog),
            c if c.is_per_catalog() => Ok(Touches::Catalog(catalog.unwrap_or(PERSONAL).to_string())),
            c => match catalog {
                None | Some(PERSONAL) => Ok(Touches::Catalog(PERSONAL.to_string())),
                Some(other) if c.is_authoring() => Err(IpcError::new(
                    codes::E_INVALID,
                    format!(
                        "{} works on the personal catalog only until changesets (Assets M4); edit \
                         catalog {other}'s checkout with git and run `fleet-hub catalog reload --catalog {other}`",
                        c.action()
                    ),
                )),
                Some(other) => Err(IpcError::new(
                    codes::E_INVALID,
                    format!("{} is not per catalog; drop the catalog parameter ({other})", c.action()),
                )),
            },
        }
    }
}
```

Change `run`'s signature to `run(call: AdminCall, catalog: Option<&str>, store, ssh, reg)`, open it with `call.touches(catalog)?;` and resolve a named catalog once:

```rust
    call.touches(catalog)?;
    // A per-catalog call naming a catalog other than personal (R10).
    let named = match catalog {
        Some(name) if name != PERSONAL && call.is_per_catalog() => Some(catalogs::catalog_named(name, store)?),
        _ => None,
    };
```

and replace these arms:

```rust
        AdminCall::Config => match &named {
            Some(row) => json(Some(catalogs::config_row(row))),
            None => json(super::config(store)?),
        },
        AdminCall::Load(a) => match &named {
            Some(row) => json(super::load_catalog(row.id, a.pull, store)?),
            None => json(super::load_all(a.pull, store)?),
        },
        AdminCall::ListLayers => match &named {
            Some(row) => json(super::list_layers_for(row, store)?),
            None => json(super::list_layers(store)?),
        },
        AdminCall::SetHostLayers(a) => {
            let contexts: Vec<&str> = a.contexts.iter().map(String::as_str).collect();
            match &named {
                Some(row) => json(super::set_host_layers_for(&a.host_alias, row, a.role.as_deref(), &contexts, store)?),
                None => json(super::set_host_layers(&a.host_alias, a.role.as_deref(), &contexts, store)?),
            }
        }
        AdminCall::ListCatalogs => {
            // Best effort: a personal that cannot load must not hide the list.
            if let Err(e) = super::ensure_fresh(store) {
                tracing::debug!(error = %e.message, "list_catalogs: refresh failed");
            }
            json(catalogs::list_catalogs(store)?)
        }
        AdminCall::AddCatalog(a) => json(catalogs::add_catalog(a, store)?),
        AdminCall::RemoveCatalog(a) => json(catalogs::remove_catalog(&a.name, store)?),
        AdminCall::AdmitCatalog(a) => json(catalogs::admit(&a.host_alias, &a.catalog, store)?),
        AdminCall::UnadmitCatalog(a) => json(catalogs::unadmit(&a.host_alias, &a.catalog, store)?),
```

Update the module doc's "Who may call it" paragraph: the master, or a client granted the catalog each call touches (`AdminCall::touches`).

`mcp/tools/params.rs` `CatalogAdminParams` — extend the `action` doc list with `|list_catalogs|add_catalog|remove_catalog|admit_catalog|unadmit_catalog`, add `#[derive(…, Default)]` if absent, and:

```rust
    /// config|load|list_layers|set_host_layers: the catalog by name
    /// (default personal). Other actions refuse a non-personal one.
    #[serde(default)]
    pub catalog: Option<String>,
```

`mcp/tools/assets.rs`:

```rust
    #[tool(description = "The Assets tab's catalog operations as one tool, \
        plus the set of catalogs (list_catalogs, add_catalog, remove_catalog, \
        admit_catalog, unadmit_catalog). `catalog` (default personal) picks \
        the catalog for config, load, list_layers and set_host_layers. Each \
        action needs the master or a client granted the catalog it touches; \
        list_catalogs needs no grant; add_catalog is master-only.")]
    pub(super) async fn catalog_admin(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<CatalogAdminParams>,
    ) -> Result<CallToolResult, McpError> {
        use catalog::admin::{AdminCall, Touches};
        audit(
            "catalog_admin",
            &format!(
                "action={} catalog={} caller={}",
                p.action,
                p.catalog.as_deref().unwrap_or("personal"),
                caller.label()
            ),
        );
        let mut wire = serde_json::json!({ "action": p.action });
        if let Some(args) = p.args.filter(|a| !a.is_null()) {
            wire["args"] = args;
        }
        // Parsed first: what a call touches depends on which call it is (R11).
        let mut call: AdminCall = serde_json::from_value(wire).map_err(|e| {
            mcp_err("E_INVALID", format!("catalog_admin {}: {e}", p.action), None)
        })?;
        let touches = call.touches(p.catalog.as_deref()).map_err(to_mcp_err)?;
        let allowed = match &touches {
            Touches::Nothing => true,
            Touches::NewCatalog => caller.is_master(),
            Touches::Catalog(name) => may_admin_catalog(&caller, &self.store, name)?,
        };
        if !allowed {
            return Err(forbidden(&touches, &caller));
        }
        if let AdminCall::ApplySync(a) = &call {
            for name in catalog::sync::plan::registry_catalogs_written(&a.plan_id).unwrap_or_default() {
                if !may_admin_catalog(&caller, &self.store, &name)? {
                    return Err(forbidden(&Touches::Catalog(name), &caller));
                }
            }
        }
        self.prepare_admin_call(&mut call, p.confirm_nonce.as_deref(), &caller)?;
        let value = catalog::admin::run(call, p.catalog.as_deref(), &self.store, &self.ssh, &self.reg)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&value)
    }
```

In `prepare_admin_call`, widen the no-refresh arm:

```rust
            // Loading and configuring are what `ensure_fresh` would do; the
            // catalog-set actions read or write the store and load what they
            // add themselves (`list_catalogs` refreshes best-effort in `run`).
            AdminCall::Config
            | AdminCall::Configure(_)
            | AdminCall::Load(_)
            | AdminCall::ListCatalogs
            | AdminCall::AddCatalog(_)
            | AdminCall::RemoveCatalog(_)
            | AdminCall::AdmitCatalog(_)
            | AdminCall::UnadmitCatalog(_) => {}
```

Replace `may_admin_catalog` and add `forbidden`:

```rust
/// True when `caller` may touch the catalog named `catalog` (spec:
/// `may_admin_catalog(caller, catalog_id)`): the master, or a live `full`
/// paired client bound to no org holding a grant on it (personal: the assets
/// grant, R2). Read from the store on every call. A per-host token never may.
/// An unknown name is "no" for a client (existence is not leaked).
fn may_admin_catalog(caller: &Caller, store: &std::sync::Mutex<Store>, catalog: &str) -> Result<bool, McpError> {
    if caller.is_master() {
        return Ok(true);
    }
    let (None, Some(c)) = (&caller.host_alias, &caller.client) else {
        return Ok(false);
    };
    let s = store
        .lock()
        .map_err(|_| mcp_err(codes::E_LOCK, "store mutex poisoned", None))?;
    let ok = if catalog == "personal" {
        s.client_is_assets_admin(c.id)
    } else {
        match s.get_catalog_by_name(catalog).map_err(|e| to_mcp_err(e.into()))? {
            Some(row) => s.client_may_admin_catalog(c.id, row.id),
            None => Ok(false),
        }
    };
    ok.map_err(|e| to_mcp_err(e.into()))
}

fn forbidden(touches: &catalog::admin::Touches, caller: &Caller) -> McpError {
    use catalog::admin::Touches;
    let message = match touches {
        Touches::NewCatalog => format!(
            "add_catalog needs the master token ({} refused); on the hub: fleet-hub catalog add <name> <path> --org <org>",
            caller.label()
        ),
        Touches::Catalog(name) if name == "personal" => format!(
            "catalog_admin needs the master token or a paired client granted the asset catalog \
             ({} refused); on the hub: fleet-hub client grant <name> assets",
            caller.label()
        ),
        Touches::Catalog(name) => format!(
            "catalog_admin on catalog {name} needs the master token or a paired client granted that \
             catalog ({} refused); on the hub: fleet-hub client grant <name> assets --catalog {name}",
            caller.label()
        ),
        Touches::Nothing => format!("catalog_admin refused {}", caller.label()),
    };
    mcp_err("E_FORBIDDEN", message, None)
}
```

and in `import_assets`: `if !may_admin_catalog(&caller, &self.store, "personal")? {`.

- [ ] **Step 4: Regenerate the control-API reference**

Run: `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`, then `cargo test -p fleet-core reference_is_current`. Expected: PASS; `docs/control-api-reference.md` lists the new actions and parameter.

- [ ] **Step 5: Run to verify pass**

Run: `cargo test -p fleet-core tests_catalog_admin`, `cargo test -p fleet-core catalog::admin`, `cargo test -p fleet-core catalog::sync::plan`, `cargo test -p fleet-core mcp::`, `cargo test -p claude-fleet --lib`. Expected: PASS (every existing `tests_catalog_admin` case unchanged in outcome, `the_action_param_names_every_admin_call` included).

- [ ] **Step 6: Commit**

```bash
git add crates/fleet-core/src/service/catalog/admin.rs crates/fleet-core/src/service/catalog/sync/plan.rs crates/fleet-core/src/mcp/tools/ docs/control-api-reference.md
git commit -m "feat(mcp): catalog_admin takes a catalog, manages catalogs and admissions, and checks a grant per touched catalog"
```

---

### Task 6: `fleet-hub catalog add|list|remove|admit|unadmit`, `reload --catalog`, `client grant|ungrant --catalog`

**Files:**
- Modify: `crates/fleet-hub/src/catalog.rs`
- Modify: `crates/fleet-hub/src/main.rs` (`ClientCmd::Grant`/`Ungrant`, dispatch, `Catalog` doc)
- Modify: `crates/fleet-hub/src/pair.rs` (`client_grant`)

**Interfaces:**
- Consumes: `catalogs::{add_catalog, list_catalogs, remove_catalog, admit, unadmit, catalog_named, AddCatalogArgs}`, `catalog::load_catalog`, `Store::{get_catalog_by_name, set_client_catalog_grant}`.
- Produces: `CatalogCmd::{Add, List, Remove, Admit, Unadmit}`, `CatalogCmd::Reload { pull, catalog }`; `pair::client_grant(opts, env, name, grant, catalog: Option<&str>, on)`.

- [ ] **Step 1: Write the failing tests**

`catalog.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn git_repo(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
        let root = dir.join(name);
        std::fs::create_dir_all(root.join("skills/s")).unwrap();
        std::fs::write(root.join("catalog.yaml"), "schema_version: 1\n").unwrap();
        std::fs::write(root.join("skills/s/asset.yaml"), "kind: skill\nname: s\ndescription: d\n").unwrap();
        std::fs::write(root.join("skills/s/body.md"), "b\n").unwrap();
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["config", "user.email", "t@t"],
            &["config", "user.name", "t"],
            &["add", "."],
            &["commit", "-q", "-m", "init"],
        ] {
            let o = fleet_core::proc::std_command("git").args(args).current_dir(&root).output().unwrap();
            assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        }
        root
    }

    #[test]
    fn add_admit_list_reload_and_remove_an_org_catalog() {
        let dir = tempfile::tempdir().unwrap();
        let opts = HubOptions { data_dir: Some(dir.path().to_path_buf()), ..HubOptions::default() };
        let env = HashMap::new();
        let org_id = {
            let s = Store::open_with_bus(&dir.path().join("state.db"), Arc::new(fleet_core::events::NoopEventBus)).unwrap();
            s.upsert_host("h").unwrap();
            s.add_org("acme", None, false).unwrap().id
        };
        let repo = git_repo(dir.path(), "acme-assets");
        let add = |org: Option<&str>| CatalogCmd::Add {
            name: "acme".into(),
            path: repo.to_string_lossy().into(),
            remote: None,
            org: org.map(String::from),
        };
        assert!(run(add(None), &opts, &env).unwrap_err().contains("--org"));
        assert_eq!(run(add(Some("acme")), &opts, &env).unwrap(), ExitCode::SUCCESS);
        run(CatalogCmd::Admit { host: "h".into(), catalog: "acme".into() }, &opts, &env).unwrap();
        assert_eq!(run(CatalogCmd::List, &opts, &env).unwrap(), ExitCode::SUCCESS);
        run(CatalogCmd::Reload { pull: false, catalog: Some("acme".into()) }, &opts, &env).unwrap();

        let s = serve::open_store(&opts, &env).unwrap();
        let acme = s.get_catalog_by_name("acme").unwrap().expect("added");
        assert_eq!(acme.org_id, Some(org_id));
        assert!(acme.head_commit.is_some(), "loaded and recorded");
        assert_eq!(s.host_admissions("h").unwrap(), vec![acme.id]);
        drop(s);

        run(CatalogCmd::Unadmit { host: "h".into(), catalog: "acme".into() }, &opts, &env).unwrap();
        run(CatalogCmd::Remove { name: "acme".into() }, &opts, &env).unwrap();
        let s = serve::open_store(&opts, &env).unwrap();
        assert!(s.get_catalog_by_name("acme").unwrap().is_none());
        assert!(repo.join(".git").is_dir(), "remove is config only: the checkout stays");
        assert!(run(CatalogCmd::Remove { name: "personal".into() }, &opts, &env).is_err());
    }
}
```

`pair.rs` tests module:

```rust
    /// `client grant <name> assets --catalog acme` grants that catalog only;
    /// `ungrant … --catalog acme` takes it back; an unknown catalog is named.
    #[test]
    fn grant_and_ungrant_one_catalog() {
        let dir = tempfile::tempdir().unwrap();
        let opts = opts_for(&dir, None);
        let env = HashMap::new();
        let (desk, acme) = {
            let s = fleet_core::store::Store::open_with_bus(
                &dir.path().join("state.db"),
                std::sync::Arc::new(fleet_core::events::NoopEventBus),
            )
            .unwrap();
            let org = s.add_org("acme", None, false).unwrap();
            let acme = s.upsert_catalog("acme", "/a", None, Some(org.id)).unwrap();
            (s.insert_client_token("desk", "aa11", "full").unwrap().id, acme.id)
        };
        client_grant(&opts, &env, "desk", crate::Grant::Assets, Some("acme"), true).unwrap();
        let s = crate::serve::open_store(&opts, &env).unwrap();
        assert!(s.client_may_admin_catalog(desk, acme).unwrap());
        assert!(!s.client_is_assets_admin(desk).unwrap(), "personal is untouched");
        drop(s);
        client_grant(&opts, &env, "desk", crate::Grant::Assets, Some("acme"), false).unwrap();
        assert!(!crate::serve::open_store(&opts, &env)
            .unwrap()
            .client_may_admin_catalog(desk, acme)
            .unwrap());
        assert!(client_grant(&opts, &env, "desk", crate::Grant::Assets, Some("nope"), true)
            .unwrap_err()
            .contains("nope"));
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p fleet-hub catalog`, `cargo test -p fleet-hub grant_and_ungrant_one_catalog` — Expected: compile errors.

- [ ] **Step 3: Implement**

`catalog.rs` — module doc gains a paragraph ("`add`/`list`/`remove`/`admit`/`unadmit` manage org catalogs (Assets M3); `set` stays the personal one's"); imports add `fleet_core::service::catalog::catalogs::{self, AddCatalogArgs}`. Replace the enum's `Reload` and add the variants:

```rust
    /// Re-read a checkout, after editing it or pulling by hand.
    Reload {
        /// `git pull --ff-only` first.
        #[arg(long)]
        pull: bool,
        /// The catalog to reload; the personal one by default.
        #[arg(long)]
        catalog: Option<String>,
    },
    /// Add an org's catalog — a git checkout on this machine — and load it.
    /// On an existing name, re-point it. `add personal <path>` is `set`.
    Add {
        /// The catalog's name (`[a-z0-9][a-z0-9-]*`).
        name: String,
        /// The checkout, on this machine. `~/` is this user's home.
        path: String,
        /// Clone from this URL when PATH is not a checkout yet.
        #[arg(long)]
        remote: Option<String>,
        /// The org that owns it (by name). Required for every catalog but `personal`.
        #[arg(long)]
        org: Option<String>,
    },
    /// Print every catalog: owner, load state, HEAD, path, admissions, grants.
    List,
    /// Forget an org catalog (config only: the checkout is never deleted).
    /// Its layer assignments, admissions and grants go; hosts keep what it installed.
    Remove { name: String },
    /// Let a host with no org take an org catalog.
    Admit { host: String, catalog: String },
    /// Take an admission back. Nothing is removed from the host.
    Unadmit { host: String, catalog: String },
```

`run`'s match:

```rust
        CatalogCmd::Reload { pull, catalog: None } => load(&store, pull),
        CatalogCmd::Reload { pull, catalog: Some(name) } if name == catalogs::PERSONAL => load(&store, pull),
        CatalogCmd::Reload { pull, catalog: Some(name) } => {
            let row = catalogs::catalog_named(&name, &store).map_err(|e| e.message)?;
            let s = catalog::load_catalog(row.id, pull, &store).map_err(|e| e.message)?;
            print_loaded(&s);
            Ok(ExitCode::SUCCESS)
        }
        CatalogCmd::Add { name, path, remote, org } => {
            let st = catalogs::add_catalog(
                AddCatalogArgs { name: name.clone(), repo_path: path, remote_url: remote, org },
                &store,
            )
            .map_err(|e| e.message)?;
            if let Some(problem) = st.problem {
                return Err(format!(
                    "added catalog {name}, but it could not be loaded: {problem}; fix the checkout and \
                     run `fleet-hub catalog reload --catalog {name}`"
                ));
            }
            out::line(&format!(
                "added catalog {} ({} asset(s)); a running hub picks it up at its next catalog call",
                st.name, st.asset_count
            ));
            Ok(ExitCode::SUCCESS)
        }
        CatalogCmd::List => list(&store),
        CatalogCmd::Remove { name } => {
            let r = catalogs::remove_catalog(&name, &store).map_err(|e| e.message)?;
            out::line(&format!(
                "removed catalog {} (config only; the checkout is untouched): dropped {} layer \
                 assignment(s), {} admission(s), {} grant(s); hosts keep what it installed",
                r.name, r.layer_rows, r.admissions, r.grants
            ));
            Ok(ExitCode::SUCCESS)
        }
        CatalogCmd::Admit { host, catalog } => {
            let names = catalogs::admit(&host, &catalog, &store).map_err(|e| e.message)?;
            out::line(&format!("{host} admits: {}", names.join(", ")));
            Ok(ExitCode::SUCCESS)
        }
        CatalogCmd::Unadmit { host, catalog } => {
            let names = catalogs::unadmit(&host, &catalog, &store).map_err(|e| e.message)?;
            out::line(&if names.is_empty() {
                format!("{host} admits no org catalog")
            } else {
                format!("{host} admits: {}", names.join(", "))
            });
            Ok(ExitCode::SUCCESS)
        }
```

Split `load` so the line is shared:

```rust
fn print_loaded(s: &fleet_core::events::CatalogSummary) {
    out::line(&format!(
        "loaded {} asset(s) at {}{}; a running hub picks it up at its next catalog call (Refresh on a client)",
        s.asset_count,
        &s.head[..s.head.len().min(12)],
        if s.problem_count > 0 { format!(", {} problem(s)", s.problem_count) } else { String::new() }
    ));
}

fn load(store: &Mutex<Store>, pull: bool) -> Result<ExitCode, String> {
    let s = catalog::load(pull, store).map_err(|e| e.message)?;
    print_loaded(&s);
    Ok(ExitCode::SUCCESS)
}

fn list(store: &Mutex<Store>) -> Result<ExitCode, String> {
    // This process's registry is empty: load what can be loaded so the
    // state column says something (a broken org catalog shows as `problem`).
    let _ = catalog::ensure_fresh(store);
    let all = catalogs::list_catalogs(store).map_err(|e| e.message)?;
    if all.is_empty() {
        out::line("no catalogs; set the personal one with: fleet-hub catalog set <path> [--remote <url>]");
    }
    for c in all {
        out::line(&format!(
            "{:<14} {:<12} {:<10} {:<12} {}",
            c.name,
            c.org.as_deref().unwrap_or("personal"),
            c.state,
            c.head_commit.as_deref().map_or("—", |h| &h[..h.len().min(12)]),
            c.repo_path
        ));
        if !c.admitted.is_empty() {
            out::line(&format!("    admitted: {}", c.admitted.join(", ")));
        }
        if !c.granted.is_empty() {
            out::line(&format!("    granted:  {}", c.granted.join(", ")));
        }
        if let Some(p) = c.problem {
            out::line(&format!("    problem:  {p}"));
        }
    }
    Ok(ExitCode::SUCCESS)
}
```

`main.rs` — `ClientCmd`:

```rust
    /// Let a paired client do what is otherwise the master's. `assets`: manage
    /// an asset catalog (edit, commit, push, Sync, Secrets, layers) from its
    /// Assets tab — the personal one, or `--catalog NAME`. Only a `full`
    /// client bound to no org. No running hub needed.
    Grant {
        name: String,
        grant: Grant,
        /// The catalog to grant; the personal one by default.
        #[arg(long)]
        catalog: Option<String>,
    },
    /// Take a `grant` back.
    Ungrant {
        name: String,
        grant: Grant,
        #[arg(long)]
        catalog: Option<String>,
    },
```

dispatch:

```rust
            ClientCmd::Grant { name, grant, catalog } => {
                pair::client_grant(&opts, &env, &name, grant, catalog.as_deref(), true)
            }
            ClientCmd::Ungrant { name, grant, catalog } => {
                pair::client_grant(&opts, &env, &name, grant, catalog.as_deref(), false)
            }
```

and the `Catalog` command's doc: "Point the hub at its asset catalogs (git checkouts on this machine), list, add, remove or admit them, or reload one. No running hub needed; …".

`pair.rs` `client_grant`:

```rust
pub fn client_grant(
    opts: &HubOptions,
    env: &HashMap<String, String>,
    name: &str,
    grant: crate::Grant,
    catalog: Option<&str>,
    on: bool,
) -> Result<ExitCode, String> {
    crate::serve::existing_db(&crate::config::resolve_data_dir(opts, env))?;
    let store = crate::serve::open_store(opts, env)?;
    let crate::Grant::Assets = grant;
    match catalog.filter(|c| *c != "personal") {
        None => {
            let row = store.set_client_assets_admin(name, on).map_err(|e| e.message)?;
            out::line(&if on {
                format!(
                    "{} may manage the asset catalog (since {}): editing assets, Sync and Secrets \
                     from its Assets tab, on this hub's checkout and hosts",
                    row.name,
                    fmt_time(row.assets_admin_at)
                )
            } else {
                format!(
                    "{} no longer manages the asset catalog; its Assets tab is read-only from its next call",
                    row.name
                )
            });
        }
        Some(cat) => {
            let id = store
                .get_catalog_by_name(cat)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("no catalog named {cat}; see `fleet-hub catalog list`"))?
                .id;
            let row = store.set_client_catalog_grant(name, id, on).map_err(|e| e.message)?;
            out::line(&if on {
                format!("{} may manage catalog {cat} from its next call", row.name)
            } else {
                format!("{} no longer manages catalog {cat}", row.name)
            });
        }
    }
    Ok(ExitCode::SUCCESS)
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p fleet-hub catalog`, `cargo test -p fleet-hub grant_and_ungrant_one_catalog`, `cargo test -p fleet-hub`, `cargo build -p fleet-hub`. Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-hub/src/
git commit -m "feat(hub): catalog add, list, remove, admit, unadmit; reload and client grant take --catalog"
```

---

### Task 7: Verification, generated files and docs

**Files:**
- Modify: `CLAUDE.md` (an M3 paragraph after the M2 one; the M2 paragraph's "admissions arrive in M3; empty here" clause)
- Modify: `docs/hub.md` (*Asset catalog*, *Clients*)
- Possibly modify: generated files the four generators rewrite

- [ ] **Step 1: Full verification**

Run, each on its own: `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace --no-fail-fast`; `pnpm test`; `pnpm check`. Expected: green except `rewind::tests::the_removal_script_leaves_a_tree_a_live_pane_is_in` and `work::scale_tests::*` if they flake.

- [ ] **Step 2: Generators**

Run: `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`; `REGEN_SETTINGS_DOCS=1 cargo test -p fleet-core settings_docs_are_current`; `REGEN_PAGE_DOCS=1 cargo test -p fleet-core page_docs_are_current`; `REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen` (it fails once on purpose when it rewrites; run it again). Expected: only `docs/control-api-reference.md` could still differ (Tasks 3 and 5 committed it); no verdict, settings or page change (R14). `git status` clean after committing anything regenerated, naming its generator in the message.

- [ ] **Step 3: `CLAUDE.md`**

In the M2 paragraph, replace "(admissions arrive in M3; empty here, so an org-bound host gets only the `shared` slice of `personal`)" with "(an org-bound host gets only the `shared` slice of `personal`; a host with no org also takes every org catalog it admits, M3)". Add after it:

```markdown
- **Assets M3 — admissions, grants per catalog, loading every catalog** (plan
  `docs/superpowers/plans/2026-10-01-assets-m3-admissions.md`): migration 092
  adds `host_catalogs` (a host with no org admits an org catalog; `admit`
  refuses an org-bound host and `personal`) and `client_catalog_grants` (a
  grant names one catalog; the personal grant is also mirrored into
  `client_tokens.assets_admin_at`, which is read only while no personal
  catalog exists yet). `load_catalog(id)` loads any catalog; `ensure_fresh`
  walks every `catalogs` row — an org catalog that cannot load becomes a
  registry *problem entry* (`Catalog.load_error`, not retried until the
  store's load record changes), a removed one is evicted, a personal failure
  is still the error. `effective_for_host` reads admissions and reports
  `speaks_for` / `held_back`: a manifest entry is an orphan (`Remove`) only
  when its own catalog speaks for the host; one whose catalog is not loaded,
  failed, no longer accepted (unadmit, org change) or not configured gets a
  `Noop` saying why — never a remove. `plan_sync` reads one
  `registry::snapshot()` for scan and plan. `service/catalog/catalogs.rs`
  adds / lists / removes catalogs (removal is config only and cascades their
  layer rows, admissions and grants) and admits hosts. `catalog_admin` takes
  an optional `catalog` (config, load, list_layers, set_host_layers; the
  authoring actions stay personal-only until M4) and five actions
  (`list_catalogs`, `add_catalog`, `remove_catalog`, `admit_catalog`,
  `unadmit_catalog`); every action checks a grant on the catalog it touches
  (`AdminCall::touches` → `may_admin_catalog`), `list_catalogs` needs none,
  `add_catalog` is master-only, `apply_sync` also needs a grant on every
  catalog its plan writes from. Operator side: `fleet-hub catalog
  add|list|remove|admit|unadmit`, `catalog reload --catalog`, `client
  grant|ungrant <name> assets --catalog`.
```

- [ ] **Step 4: `docs/hub.md`**

In *Asset catalog*, replace "`catalog set` always configures the personal catalog; per-org catalogs arrive later (S1b M3). A host bound to an org receives only the `shared` assets of the personal catalog." with:

```markdown
`catalog set` configures the personal catalog. An org can have its own:
`catalog add <name> <path> --org <org> [--remote <url>]` records and loads
it (on an existing name it re-points it). Which hosts take what: a host
bound to an org receives that org's catalog plus the `shared` assets of the
personal catalog; a host with no org receives all of personal plus every org
catalog it admits (`catalog admit <host> <catalog>`, `catalog unadmit`).
`catalog list` shows each catalog's owner, load state, HEAD, admissions and
grants; `catalog reload --catalog <name>` re-reads one. `catalog remove
<name>` forgets an org catalog — config only, the checkout stays — along
with its layer assignments, admissions and grants.

A catalog whose checkout cannot be loaded is shown as a problem (`catalog
list`) while the others load; it is retried at its next `reload`. Sync never
removes what a catalog installed because that catalog went away — not
loaded, failed, unadmitted, the host changed org, or removed: it reports
those assets as `Noop` "kept, not removed" and leaves them to you.
```

and add to the command list under it:

```markdown
- `add <name> <path> [--remote <url>] --org <org>` records an org catalog
  and loads it; `list`, `remove <name>`, `admit <host> <catalog>`,
  `unadmit <host> <catalog>` manage the set; `reload --catalog <name>`
  reloads one.
```

In *Clients* ("Except what you grant: the asset catalog"), after "`fleet-hub client grant <name> assets` lets that one client manage the asset catalog …" add: "A grant names one catalog: `--catalog <name>` grants an org's catalog instead of the personal one (`client ungrant <name> assets --catalog <name>` takes it back), and the client may then touch only the catalogs it holds — admit, remove, load or list layers in them, and apply a Sync plan only when it holds every catalog that plan writes from. Anyone who reaches `catalog_admin` may `list_catalogs`; only the master token may `add_catalog`. `client list`'s ASSETS column is the personal grant; `catalog list` shows every grant."

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md docs/hub.md docs/control-api-reference.md
git commit -m "docs: admissions, per-catalog grants and org catalogs (M3)"
```

---

## Self-review against the spec (M3)

| Spec item | Task |
|---|---|
| `host_catalogs` table (DDL as specified) | 1 |
| `client_catalog_grants` table (DDL as specified) | 1 |
| Migration step 3: grants backfilled from `assets_admin_at`; `assets_admin_at` stays | 1 (+ R2 pending mirror) |
| `load(id)` | 2 (`load_catalog`; `load` stays personal, `load_all` R18) |
| per-catalog `ensure_fresh` | 2 |
| Runtime: a catalog that cannot load is a problem entry; the others load | 2 (R5) |
| Which catalogs a host accepts: no-org host = personal + admissions; org host = its org + shared personal; `local` the same | 3 (admissions into `acceptance`; `local` is not special-cased) |
| `fleet-hub catalog add <name> <path> [--remote URL] [--org NAME]` | 4 (service), 6 (CLI) |
| `catalog list` | 4, 6 |
| `catalog remove <name>` (config only; never deletes the repo) | 1 (store, R13), 4, 6 |
| `catalog admit|unadmit <host> <catalog>` | 4, 6 |
| `catalog set` stays the alias for `personal` | 6 (unchanged; `add personal` = set) |
| `fleet-hub client grant <name> assets [--catalog NAME]` (default personal), take-back with `--catalog` | 6 (R12) |
| MCP `catalog_admin` optional `catalog` (default personal) | 5 (R10) |
| MCP `list_catalogs`, `add_catalog`, `remove_catalog`, `admit_catalog`, `unadmit_catalog` | 5 |
| Every mutating action checks the grant for the catalogs it touches (`may_admin_catalog(caller, catalog)`) | 5 (`touches`, `apply_sync` per plan catalog, R11) |
| Per-host tokens never pass | 5 (`NOT_FOR_HOST_TOKENS` unchanged; tested for `list_catalogs`) |
| `list_assets` and the inventory stay readable as today | 5 (untouched; R17) |
| Tauri AdminCall verdict table gains the new actions | ruled to M6 with the commands (R14); `REGEN_HUB_VERDICTS` run in 7 shows no change |
| Testing — store: admissions, grants per catalog | 1 |
| Testing — planning: a no-org host receives an org asset only when admitted | 3 (effective + `plan_sync`) |
| Testing — authorization: ungranted client / per-host token cannot admit in a catalog without a grant, but can list | 5 |
| Out of scope: automatic remove under any mode | 3 (R6: held `Noop`) |
| Changesets (M4), UI (M5/M6) | not touched |
| M2 carry 1–9 | see the carry table above |
