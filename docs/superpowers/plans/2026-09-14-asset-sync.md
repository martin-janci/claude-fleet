# Asset Sync Engine (sub-project 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let fleet write catalog assets to hosts: plan first, apply on confirmation, guard every write against concurrent edits, back up before overwriting, record a managed manifest per harness, resolve `${NAME}` secrets from the fleet database, install plugins through the Claude CLI, and support Codex (scan + TOML merge).

**Architecture:** A new `service/catalog/sync/` module (manifest, secrets, plan, apply, orchestration) sits on the existing catalog module. The `Harness` trait gains `manifest_path()` and `merge_config()`; Claude merges JSON, Codex merges TOML, and both scan their manifest and config-file hashes so the applier can compare-and-swap. Plans live in an in-memory registry with a TTL; apply re-verifies every write. Store gains migration 031 (managed flag, secrets tables, sync runs). Six Tauri commands, three MCP tools (one behind the confirm gate), and a plan dialog + secrets panel in the Assets tab.

**Tech Stack:** Rust (tauri 2, serde, serde_json, serde_yaml, toml 0.8 new, sha2, base64, rusqlite, rmcp, tokio, tracing), Svelte 5 + TypeScript, Vitest + @testing-library/svelte.

**Spec:** `docs/superpowers/specs/2026-09-14-asset-sync-design.md` (binding). Sub-project 1 spec for the IR and inventory: `docs/superpowers/specs/2026-09-14-asset-catalog-design.md`.

## Global Constraints

- Service functions take `&Mutex<Store>` / `&dyn SshExec` (or `&Arc<SshClient>` where the existing catalog code does), never `tauri::State`. Never hold the `Store` mutex or the `CATALOG` lock across an `.await`.
- Every host-side script is one `bash -lc` word passed through `crate::shell::quote`; scripts contain **no single quotes**; `~/` paths are rendered as `"$HOME"/...` inside scripts. Only catalog-controlled values (kebab-case names, fixed directory constants, base64) are interpolated.
- Secrets: values never appear in plans, previews, logs, `audit()` lines, MCP output, events, error messages, or the manifest. Secret-bearing files are written through `provision::write_host_file_secret`. `${NAME}` is `[A-Z0-9_]+`.
- Writes are compare-and-swap against the hash the scan recorded (`absent` when the file did not exist). A mismatch is a `conflict` action result, never a write.
- Backups: `<path>.fleet-bak-<unix time>` before an `overwrite` or `remove` of an existing file. Fleet removes only files/merges listed in its own manifest.
- Manifest paths: `~/.claude/.fleet-assets.json`, `~/.codex/.fleet-assets.json`; written last per (host, harness), only if no action failed (conflicts excluded).
- Migration `031_asset_sync.sql` registered with an `already_applied` guard (it has an `ALTER TABLE`); schema version assertions move from 30 to 31 (`store/schema.rs` tests and `service/health.rs`).
- MCP: tools added are exactly `plan_sync`, `apply_sync`, `set_secret`. `apply_sync` joins `CONFIRM_TOOLS` (count test 7 → 8) and is admin-only when no `host_alias` is given; `set_secret` is admin-only; none join `READONLY_TOOLS`. Regenerate `docs/control-api-reference.md` (`REGEN_DOCS=1 cargo test --manifest-path Cargo.toml reference_is_current`) and keep the tool-count guard test honest.
- No `eprintln!` in production code (use `tracing`). Use `ipc_error::codes` and `IpcError::lock()`. New codes: `E_SYNC_PLAN_STALE`, `E_SECRET_MISSING`.
- Build/verify on this host only after `cd src-tauri && source /home/dev/.local/tauri-sysroot/env.sh` in the same shell (plus `export RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu` if cargo complains about a root-owned rustup). CI mirror: `scripts/ci-local.sh` (or `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test` plus `CI=true pnpm install --frozen-lockfile && pnpm run check && pnpm run test && pnpm run build`). Base: 1348 Rust tests, 1187 frontend tests.
- Commits: Conventional Commits, each ending with `Claude-Session: https://claude.ai/code/session_015qiY9nS4iAUqiuLXumrMvS`. Never touch version fields or CHANGELOG.md.

## File Structure

| File | Responsibility |
|---|---|
| `src-tauri/migrations/031_asset_sync.sql` | `managed` column, secrets tables, `sync_runs` |
| `src-tauri/src/store/rows.rs` | `AssetInventoryRow.managed`, `SecretRow`, `SyncRunRow` |
| `src-tauri/src/store/catalog.rs` | inventory helpers updated; secrets CRUD; sync-run record |
| `src-tauri/src/store/schema.rs` | migration entry + guard fn + expected tables |
| `src-tauri/src/events.rs` | `SyncProgress` payload + `sync_progress` method (`sync:progress`) |
| `src-tauri/src/service/catalog/harness/mod.rs` | trait additions; shared scan-block parser; JSON merge/unmerge helpers; `ManifestMerge` |
| `src-tauri/src/service/catalog/harness/claude.rs` | manifest path, config hashes in scan, `merge_config` (JSON) |
| `src-tauri/src/service/catalog/harness/codex.rs` | scan script, parse, installed, `merge_config` (TOML) |
| `src-tauri/src/service/catalog/sync/manifest.rs` | `Manifest`, `ManifestEntry`, parse/serialise/diff |
| `src-tauri/src/service/catalog/sync/secrets.rs` | resolve per host; substitute into a render plan |
| `src-tauri/src/service/catalog/sync/plan.rs` | `SyncPlan`/`HostPlan`/`Action`/`ActionOp`; `compute_host_plan`; plan registry |
| `src-tauri/src/service/catalog/sync/apply.rs` | guarded write scripts, output parsing, secret uploads, config merge writes, plugins, manifest write, `apply_host` |
| `src-tauri/src/service/catalog/sync/mod.rs` | `plan_sync`, `apply_sync`, results, progress events, sync-run record |
| `src-tauri/src/service/catalog/inventory.rs` | `scan_host_harness` extracted; `compute_states` gains manifest (managed + orphan) |
| `src-tauri/src/commands/assets.rs` | six new commands |
| `src-tauri/src/mcp/tools/assets.rs`, `params.rs`, `guard.rs` | three tools, params, classification |
| `src/lib/assets.ts` | types + wrappers: `planSync`, `applySync`, `lastSync`, secrets |
| `src/lib/SyncPlanDialog.svelte`, `src/lib/SecretsPanel.svelte` | new UI |
| `src/lib/AssetsPanel.svelte`, `src/lib/AssetDetail.svelte`, `src/lib/events.ts` | wiring |
| `docs/concepts.md`, `docs/control-api.md` | docs |

---

### Task 1: Migration 031, store rows and helpers, sync progress event

**Files:**
- Create: `src-tauri/migrations/031_asset_sync.sql`
- Modify: `src-tauri/src/store/schema.rs` (MIGRATIONS entry + guard fn + EXPECTED_TABLES + version asserts), `src-tauri/src/store/rows.rs`, `src-tauri/src/store/catalog.rs`, `src-tauri/src/service/health.rs` (30 → 31), `src-tauri/src/events.rs`
- Modify: `src/lib/assets.ts` is NOT touched here (Task 9 adds `managed` to the TS type).

**Interfaces (Produces):**
- `AssetInventoryRow { …existing…, pub managed: bool }` (serialized as JSON bool).
- `SecretRow { pub name: String, pub host_alias: Option<String>, pub updated_at: i64 }` — never carries a value.
- `SyncRunRow { pub id: i64, pub started_at: i64, pub finished_at: i64, pub summary_json: String }`.
- `Store::list_secrets() -> Result<Vec<SecretRow>>` (global rows have `host_alias: None`, then per-host rows).
- `Store::secret_values_for_host(host_alias: &str) -> Result<BTreeMap<String, String>>` (global overlaid by that host's overrides).
- `Store::set_secret(name: &str, host_alias: Option<&str>, value: &str) -> Result<()>` (upsert).
- `Store::delete_secret(name: &str, host_alias: Option<&str>) -> Result<bool>`.
- `Store::record_sync_run(started_at: i64, finished_at: i64, summary_json: &str) -> Result<i64>`; `Store::last_sync_run() -> Result<Option<SyncRunRow>>`.
- `events::SyncProgress { pub plan_id: String, pub host_alias: String, pub harness: String, pub done: usize, pub total: usize }`; `EventBus::sync_progress(&self, p: &SyncProgress)` default no-op; `AppHandleEventBus` emits `sync:progress`; `RecordingEventBus` records `sync:progress:<host>:<harness>:<done>/<total>`.
- `Store::bus_sync_progress(&self, p: &SyncProgress)` pass-through.

- [ ] **Step 1: Migration**

`src-tauri/migrations/031_asset_sync.sql`:

```sql
-- Asset catalog sub-project 2 (sync engine). `managed` marks inventory rows
-- that the host's fleet manifest names. Secrets resolve ${NAME} placeholders
-- at apply time (global value, optional per-host override). sync_runs keeps
-- the last apply summary across restarts.
-- See docs/superpowers/specs/2026-09-14-asset-sync-design.md.
ALTER TABLE asset_inventory ADD COLUMN managed INTEGER NOT NULL DEFAULT 0;

CREATE TABLE IF NOT EXISTS catalog_secrets (
  name TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS catalog_secrets_host (
  host_alias TEXT NOT NULL,
  name TEXT NOT NULL,
  value TEXT NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (host_alias, name)
);

CREATE TABLE IF NOT EXISTS sync_runs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  started_at INTEGER NOT NULL,
  finished_at INTEGER NOT NULL,
  summary_json TEXT NOT NULL
);

INSERT OR IGNORE INTO schema_version (version) VALUES (31);
```

In `store/schema.rs`, after the 030 entry:

```rust
    // `ALTER TABLE ... ADD COLUMN` fails if the column is already there.
    Migration {
        version: 31,
        sql: include_str!("../../migrations/031_asset_sync.sql"),
        already_applied: Some(asset_inventory_has_managed),
    },
```

with, next to `projects_has_adopted`:

```rust
fn asset_inventory_has_managed(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('asset_inventory') WHERE name = 'managed'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}
```

Add `"catalog_secrets", "catalog_secrets_host", "sync_runs"` to `EXPECTED_TABLES`. Find every `30` schema-version assertion (`grep -rn "30)" src-tauri/src/store/schema.rs src-tauri/src/service/health.rs` and any `migration_030_is_idempotent`-style test) and bump/add for 31, including a `migration_031_is_idempotent` test in the same shape as 030's (roll back the version, migrate again, rows survive, `managed` column present once).

- [ ] **Step 2: Failing store tests** (in `store/catalog.rs` tests module)

```rust
    #[test]
    fn secrets_global_and_host_override_resolve_in_order() {
        let s = Store::open_in_memory().unwrap();
        s.set_secret("JIRA_TOKEN", None, "global").unwrap();
        s.set_secret("JIRA_TOKEN", Some("mefistos"), "host").unwrap();
        s.set_secret("OTHER", None, "o").unwrap();
        let local = s.secret_values_for_host("local").unwrap();
        assert_eq!(local["JIRA_TOKEN"], "global");
        assert_eq!(local["OTHER"], "o");
        let mef = s.secret_values_for_host("mefistos").unwrap();
        assert_eq!(mef["JIRA_TOKEN"], "host");
        let names = s.list_secrets().unwrap();
        assert_eq!(names.len(), 3);
        assert!(names.iter().all(|r| !format!("{r:?}").contains("global")), "list rows must not carry values");
        assert!(s.delete_secret("JIRA_TOKEN", Some("mefistos")).unwrap());
        assert!(!s.delete_secret("JIRA_TOKEN", Some("mefistos")).unwrap());
        assert_eq!(s.secret_values_for_host("mefistos").unwrap()["JIRA_TOKEN"], "global");
    }

    #[test]
    fn inventory_round_trips_managed_flag() {
        let s = Store::open_in_memory().unwrap();
        let row = AssetInventoryRow { host_alias: "local".into(), harness: "claude".into(), kind: "skill".into(), name: "s".into(), state: "in_sync".into(), managed: true, ..Default::default() };
        s.replace_host_inventory("local", "claude", &[row]).unwrap();
        assert!(s.list_inventory().unwrap()[0].managed);
    }

    #[test]
    fn sync_runs_record_and_last() {
        let s = Store::open_in_memory().unwrap();
        assert!(s.last_sync_run().unwrap().is_none());
        let id = s.record_sync_run(1, 2, "{\"hosts\":[]}").unwrap();
        assert!(id > 0);
        let id2 = s.record_sync_run(3, 4, "{}").unwrap();
        assert_eq!(s.last_sync_run().unwrap().unwrap().id, id2);
    }
```

And in `events.rs` tests (or `store/catalog.rs`): a `RecordingEventBus` receives `sync:progress:local:claude:1/3` after `bus_sync_progress`.

- [ ] **Step 3: Run to verify failure** — `cargo test store::catalog` (compile errors).

- [ ] **Step 4: Implement**

`rows.rs`: add `pub managed: bool` to `AssetInventoryRow` (keep `Default`); add:

```rust
/// A secret name known to the sync engine. Never carries the value.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SecretRow {
    pub name: String,
    pub host_alias: Option<String>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SyncRunRow {
    pub id: i64,
    pub started_at: i64,
    pub finished_at: i64,
    pub summary_json: String,
}
```

`store/catalog.rs`: `replace_host_inventory` inserts `managed` (`if r.managed {1} else {0}`), `list_inventory` reads it (`row.get::<_, i64>(8)? != 0`); add the six helpers with plain SQL (`INSERT … ON CONFLICT(name) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at`; host variant on `(host_alias, name)`; `secret_values_for_host` = select all global into a `BTreeMap`, then overlay `SELECT name, value FROM catalog_secrets_host WHERE host_alias=?1`; `delete_secret` returns `changes() > 0`; `record_sync_run` returns `last_insert_rowid()`; `last_sync_run` = `ORDER BY id DESC LIMIT 1`). Add `pub fn bus_sync_progress(&self, p: &crate::events::SyncProgress) { self.bus.sync_progress(p) }`.

`events.rs`: `#[derive(Serialize, Clone, Debug)] pub struct SyncProgress { pub plan_id: String, pub host_alias: String, pub harness: String, pub done: usize, pub total: usize }`; trait `fn sync_progress(&self, _p: &SyncProgress) {}` (default no-op, documented like `catalog_loaded`); `AppHandleEventBus` → `self.queue("sync:progress", p)`; `RecordingEventBus` → `format!("sync:progress:{}:{}:{}/{}", p.host_alias, p.harness, p.done, p.total)`.

- [ ] **Step 5: Verify** — `cargo test store:: && cargo test service::health && cargo test service::catalog` all green; then `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, full `cargo test`.

- [ ] **Step 6: Commit** — `feat(store): migration 031 — managed inventory flag, catalog secrets, sync runs, sync:progress event`.

---

### Task 2: Harness trait additions, JSON merge/unmerge, Claude manifest + config hashes

**Files:**
- Modify: `src-tauri/src/service/catalog/harness/mod.rs`, `harness/claude.rs`, `harness/codex.rs` (trait conformance stubs only; Task 3 fills them)

**Interfaces (Produces):**
- `harness/mod.rs`:
  - `pub struct ManifestMerge { pub file: String, pub json_path: Vec<String>, pub mode: MergeMode, pub value_hash: String }` (Serialize/Deserialize; `MergeMode` gains `Deserialize`).
  - `pub fn value_hash(v: &serde_json::Value) -> String` (sha256 of canonical `to_string`).
  - `pub fn apply_merges(root: &mut serde_json::Value, merges: &[ConfigMerge])` — Set: create intermediate objects, set value; AppendUnique: ensure array at path, push value if no equal element; Subset: if `value` is an array, ensure array at path and push `value[0]` unless some existing element is a superset of it (use the same `is_subset` semantics as `inventory::merge_satisfied`); if `value` is an object, ensure object at path and set each key.
  - `pub fn remove_merges(root: &mut serde_json::Value, merges: &[ManifestMerge])` — Set/Subset: delete the last path segment's key (and drop now-empty parent objects up to but not including the root key, e.g. an empty `hooks.Stop` array is removed but `hooks` stays); AppendUnique: retain array elements whose `value_hash` differs; delete the array if it becomes empty.
  - `pub fn parse_scan_blocks(stdout: &str, decode: &dyn Fn(&str, &[u8]) -> Option<serde_json::Value>) -> Result<HostSnapshot, IpcError>` — the existing Claude `parse_scan` body generalised: `##HASHES` lines, `##CONFIG <path>` + base64 line decoded by `decode(path, bytes)`, `##END` required (`E_SCAN`), hash line whose path is `-` skipped.
  - trait `Harness` gains `fn manifest_path(&self) -> &'static str;` and `fn merge_config(&self, file: &str, existing: &str, merges: &[ConfigMerge], remove: &[ManifestMerge]) -> Result<String, IpcError>;` (apply `merges`, then `remove`, return the full new file text with a trailing newline). Empty `existing` means an empty document.
- `claude.rs`: `pub const MANIFEST_PATH: &str = "~/.claude/.fleet-assets.json"`; `CONFIG_FILES` includes it; the scan script also hashes every `CONFIG_FILES` entry (`for f in …; do if [ -f "$f" ]; then $H "$f"; fi; done`) so `snap.files` holds `~/.claude/settings.json` etc.; `parse_scan` delegates to `parse_scan_blocks` with a JSON decoder; `merge_config` parses JSON (empty → `{}`; parse failure → `E_INVALID` "…is not a JSON object"), applies, removes, returns `serde_json::to_string_pretty` + `"\n"`.
- `codex.rs`: `manifest_path()` returns `"~/.codex/.fleet-assets.json"`; `merge_config` returns `Err(E_ASSET_UNSUPPORTED)` for now (Task 3 replaces it).

- [ ] **Step 1: Failing tests** (in `harness/mod.rs` and `claude.rs`)

```rust
    #[test]
    fn apply_and_remove_merges_cover_every_mode() {
        let mut root = json!({});
        let set = ConfigMerge { file: "f".into(), json_path: vec!["mcpServers".into(), "x".into()], mode: MergeMode::Set, value: json!({"type":"http"}) };
        let app = ConfigMerge { file: "f".into(), json_path: vec!["hooks".into(), "Stop".into()], mode: MergeMode::AppendUnique, value: json!({"hooks":[{"type":"command","command":"x"}]}) };
        let sub = ConfigMerge { file: "f".into(), json_path: vec!["plugins".into(), "p@m".into()], mode: MergeMode::Subset, value: json!([{"version":"1"}]) };
        apply_merges(&mut root, &[set.clone(), app.clone(), sub.clone()]);
        apply_merges(&mut root, &[app.clone(), sub.clone()]); // idempotent
        assert_eq!(root["mcpServers"]["x"]["type"], "http");
        assert_eq!(root["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert_eq!(root["plugins"]["p@m"].as_array().unwrap().len(), 1);
        let rm = |m: &ConfigMerge| ManifestMerge { file: m.file.clone(), json_path: m.json_path.clone(), mode: m.mode, value_hash: value_hash(&m.value) };
        remove_merges(&mut root, &[rm(&set), rm(&app), rm(&sub)]);
        assert!(root["mcpServers"].get("x").is_none());
        assert!(root["hooks"].get("Stop").is_none(), "empty array removed");
        assert!(root.get("hooks").is_some(), "top-level key kept");
        assert!(root["plugins"].get("p@m").is_none());
    }

    #[test]
    fn remove_append_unique_keeps_other_elements() {
        let mut root = json!({"hooks":{"Stop":[{"a":1},{"b":2}]}});
        let m = ManifestMerge { file: "f".into(), json_path: vec!["hooks".into(),"Stop".into()], mode: MergeMode::AppendUnique, value_hash: value_hash(&json!({"a":1})) };
        remove_merges(&mut root, &[m]);
        assert_eq!(root["hooks"]["Stop"], json!([{"b":2}]));
    }
```

`claude.rs`:

```rust
    #[test]
    fn merge_config_json_round_trip_and_empty_existing() {
        let m = ConfigMerge { file: SETTINGS_PATH.into(), json_path: vec!["hooks".into(),"Stop".into()], mode: MergeMode::AppendUnique, value: json!({"hooks":[{"type":"command","command":"x"}]}) };
        let out = Claude.merge_config(SETTINGS_PATH, "", &[m.clone()], &[]).unwrap();
        assert!(out.ends_with('\n'));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["command"], "x");
        let again = Claude.merge_config(SETTINGS_PATH, &out, &[m], &[]).unwrap();
        assert_eq!(again, out);
        assert!(Claude.merge_config(SETTINGS_PATH, "not json", &[], &[]).is_err());
    }

    #[test]
    fn scan_script_hashes_config_files_and_manifest() {
        let s = Claude.scan_script().unwrap();
        assert!(s.contains("##CONFIG ~/.claude/.fleet-assets.json"));
        assert!(s.contains(".claude/settings.json"));
        assert!(!s.contains('\''));
        assert_eq!(Claude.manifest_path(), MANIFEST_PATH);
    }
```

Also extend the existing bash smoke test (`scan_script_runs_under_bash_and_parses_cleanly`) to write a `settings.json` under the temp HOME and assert `snap.files` contains `~/.claude/settings.json` with a 64-hex hash.

- [ ] **Step 2: Run to verify failure**, **Step 3: implement** as specified (keep existing goldens byte-exact), **Step 4: verify** `cargo test service::catalog` + fmt/clippy/full test, **Step 5: commit** `feat(catalog): harness merge_config/manifest_path, JSON merge and unmerge helpers, config hashes in the Claude scan`.

---

### Task 3: Codex scan, installed, and TOML merge

**Files:**
- Modify: `src-tauri/Cargo.toml` (add `toml = "0.8"` under the asset catalog deps), `src-tauri/src/service/catalog/harness/codex.rs`

**Interfaces (Produces):**
- `pub const CODEX_MANIFEST_PATH: &str = "~/.codex/.fleet-assets.json"`.
- `Codex::scan_script()` → `Some(script)`: same shape as Claude's (hasher detection, `##HASHES`, `find -L .codex/skills -type f -exec $H {} +`, hash of each config file, `##CONFIG ~/.codex/config.toml`, `##CONFIG ~/.codex/.fleet-assets.json`, `##END`; no single quotes).
- `Codex::parse_scan()` → `parse_scan_blocks` with a decoder that parses `config.toml` via `toml::from_str::<toml::Value>` then `serde_json::to_value`, and the manifest as JSON.
- `Codex::installed()` → skills from `~/.codex/skills/<name>/…` paths; MCP servers from `configs["~/.codex/config.toml"]["mcp_servers"]` keys.
- `Codex::merge_config()` → parse existing TOML (empty → empty table; parse failure → `E_INVALID`), convert to JSON, `apply_merges` + `remove_merges`, convert back (`toml::Value::try_from(json)`), `toml::to_string_pretty` + trailing newline. Document that comments in `config.toml` are not preserved (Codex support is experimental).
- `Codex::manifest_path()` → `CODEX_MANIFEST_PATH`.
- `harness/codex.rs` tests: script has no single quotes and names both config paths; `parse_scan` on a fixture with a base64 TOML block yields `configs["~/.codex/config.toml"]["mcp_servers"]["fleet"]["url"]`; `installed` lists the skill and the server; `merge_config` golden: existing `[mcp_servers.a]\nurl = "x"\n` + Set merge for `mcp_servers.fleet` → output parses back with both tables; removal of `a` leaves only `fleet`; a bash smoke test under a temp HOME (like Claude's) with one skill file and a `config.toml`.

- [ ] Steps: failing tests → run → implement → `cargo test service::catalog::harness::codex` → fmt/clippy/full test → commit `feat(catalog): Codex host scan and TOML config merge`.

---

### Task 4: Manifest and secrets

**Files:**
- Create: `src-tauri/src/service/catalog/sync/mod.rs` (module declarations only for now + `pub mod manifest; pub mod secrets;`), `sync/manifest.rs`, `sync/secrets.rs`
- Modify: `src-tauri/src/service/catalog/mod.rs` (`pub mod sync;`)

**Interfaces (Produces):**

`manifest.rs`:
```rust
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ManifestEntry { pub hash: String, pub files: Vec<String>, pub merges: Vec<ManifestMerge>, pub synced_at: i64 }
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Manifest { pub version: u32, pub updated_at: i64, pub assets: BTreeMap<String, ManifestEntry> }
impl Manifest {
    pub fn key(kind: Kind, name: &str) -> String            // "skill/worktree"
    pub fn split_key(key: &str) -> Option<(Kind, String)>
    pub fn from_snapshot(snap: &HostSnapshot, path: &str) -> Manifest   // missing/invalid → Manifest::default() with version 1
    pub fn to_json(&self) -> String                          // pretty + "\n", version forced to 1
    pub fn entry_for(hash: &str, plan: &RenderPlan, now: i64) -> ManifestEntry  // files = plan.files paths, merges = ManifestMerge from plan.merges (value_hash of the SUBSTITUTED value)
    pub fn orphans<'a>(&'a self, catalog: &Catalog) -> Vec<(&'a str, &'a ManifestEntry)> // keys whose (kind,name) is not in the catalog
}
```
`secrets.rs`:
```rust
pub const BUILTIN_TOKEN: &str = "FLEET_MCP_TOKEN";
pub const BUILTIN_PORT: &str = "FLEET_MCP_PORT";
/// host override > global > built-ins. Never logs values.
pub fn resolve(store: &Mutex<Store>, host_alias: &str) -> Result<BTreeMap<String, String>, IpcError>
pub struct Substituted { pub plan: RenderPlan, pub missing: Vec<String>, pub secret_files: BTreeSet<String>, pub secret_merge_files: BTreeSet<String> }
/// Replace ${NAME} in every file body (UTF-8 files only) and every string inside merge values.
pub fn substitute(plan: &RenderPlan, values: &BTreeMap<String, String>) -> Substituted
```
Built-ins: `FLEET_MCP_TOKEN` = `store.get_host_token(host)?.map(|r| r.token)` (absent → not provided → will be reported missing if referenced); `FLEET_MCP_PORT` = `get_setting(mcp::SETTING_PORT)` or `"4180"`.

- [ ] Tests (write first): manifest round trip through JSON; `from_snapshot` tolerates missing/invalid; `orphans` on a catalog lacking one key; `entry_for` records substituted value hashes; secrets: store with global `A`, host override for `mefistos`, host token row for `mefistos` → `resolve(mefistos)` has `A` = host value and `FLEET_MCP_TOKEN` = that token, `resolve(local)` lacks the token when no row exists; `substitute` replaces in file bytes and nested merge values, reports `missing` for unknown names, leaves them verbatim, and lists `secret_files` only for files that changed.
- [ ] Implement, verify (`cargo test service::catalog::sync`), fmt/clippy/full, commit `feat(catalog): managed manifest and secret resolution for sync`.

---

### Task 5: Plan computation, plan registry, managed/orphan inventory

**Files:**
- Create: `src-tauri/src/service/catalog/sync/plan.rs`
- Modify: `sync/mod.rs` (`pub mod plan;`), `src-tauri/src/service/catalog/inventory.rs` (`compute_states` gains `manifest: &Manifest`; extract `scan_host_harness`)

**Interfaces (Produces):**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "snake_case")]
pub enum ActionOp { Create, Update, Overwrite, Adopt, Remove, PluginInstall, PluginUpdate, Noop, Blocked }
#[derive(Debug, Clone, Serialize)]
pub struct Action {
    pub kind: String, pub name: String, pub op: ActionOp,
    pub reason: Option<String>,            // for blocked / overwrite ("edited on host") / plugin notes
    pub files: Vec<String>, pub merges: Vec<String>,   // display: paths and "file:json/path"
    pub backup: bool, pub secrets: Vec<String>, pub missing_secrets: Vec<String>,
    #[serde(skip)] pub plan: Option<RenderPlan>,        // substituted; None for Remove/Noop/Blocked/plugin ops
    #[serde(skip)] pub expected: BTreeMap<String, Option<String>>, // path → scanned hash (None = absent)
    #[serde(skip)] pub secret_files: BTreeSet<String>,
    #[serde(skip)] pub remove_entry: Option<ManifestEntry>,   // for Remove
    #[serde(skip)] pub plugin: Option<PluginTarget>,          // { plugin, marketplace_name, marketplace_repo, version }
}
#[derive(Debug, Clone, Serialize)]
pub struct HostPlan { pub host_alias: String, pub harness: String, pub status: String /* planned|skipped */, pub detail: Option<String>, pub actions: Vec<Action>, #[serde(skip)] pub snapshot: HostSnapshot, #[serde(skip)] pub manifest: Manifest }
#[derive(Debug, Clone, Serialize)]
pub struct SyncPlan { pub id: String, pub computed_at: i64, pub hosts: Vec<HostPlan>, pub counts: BTreeMap<String, usize> }
pub struct PlanFilter { pub host_alias: Option<String>, pub kind: Option<Kind>, pub name: Option<String> }
pub fn compute_host_plan(catalog: &Catalog, harness: &dyn Harness, host_alias: &str, snap: &HostSnapshot, manifest: &Manifest, secrets: &BTreeMap<String,String>, filter: &PlanFilter) -> HostPlan
pub fn counts(plan: &SyncPlan) -> BTreeMap<String, usize>
// registry
pub fn registry_put(plan: SyncPlan) -> String   // returns id (uuid v4), evicts expired (10 min)
pub fn registry_take(id: &str) -> Option<SyncPlan>
```

Decision rules in `compute_host_plan` (document them in a doc comment):
1. For every catalog asset matching the filter: `render` → `Unsupported` ⇒ `Blocked("unsupported on <harness>")`. Empty plan (disabled target) ⇒ `Noop`.
2. `substitute` with the host's secrets; any `missing` ⇒ `Blocked("missing secrets: A, B")` with `missing_secrets`.
3. Plugin refs: installed record present (`merge_satisfied` on the Subset merge with version stripped, i.e. key exists) ⇒ if version matches or ref is `latest` and present ⇒ `Noop`/`Adopt` per manifest; present with different version ⇒ ref `latest` → `PluginUpdate`, pinned → `Blocked("installed <v>, catalog pins <w>; the CLI cannot pin versions")`; absent ⇒ `PluginInstall`. `plugin` target filled from the spec.
4. Other kinds: `present` = every file exists in `snap.files` or every merge's path resolves; `matches` = every file hash equals sha256 of the substituted bytes and every merge is `merge_satisfied` (after substitution). Then: absent ⇒ `Create`; present & matches ⇒ in manifest ? `Noop` : `Adopt`; present & !matches ⇒ in manifest && host file hashes all equal the manifest's recorded file hashes? — the manifest stores the render hash, not per-file hashes, so use: in manifest && manifest.hash == catalog render hash ⇒ `Overwrite` (host edited); in manifest && manifest.hash != render hash ⇒ `Update` (catalog changed; host may also have changed → still backup: `backup = true` whenever any existing file is replaced); not in manifest ⇒ `Overwrite` with reason "present but differs; not managed" and backup.
5. `expected` for every file path in the plan = `snap.files.get(path).cloned()`; for every merge file likewise.
6. Orphans: `manifest.orphans(catalog)` (respecting the name filter) ⇒ `Remove` with `remove_entry`, `files` and `merges` from the entry, `backup = true`, `expected` for those files.
7. Skipped hosts: caller passes status `skipped` with detail (unreachable) without calling this fn.

`inventory.rs` changes: `pub async fn scan_host_harness(ssh: &Arc<SshClient>, host: &str, harness: &dyn Harness) -> Result<HostSnapshot, IpcError>` (script → run → parse); `scan_hosts` uses it; `compute_states(catalog, harness, host_alias, snap, manifest, scanned_at)` sets `managed = manifest.assets.contains_key(&Manifest::key(kind, name))` on catalog rows and appends `orphan` rows (`state: "orphan", managed: true`) for manifest keys not in the catalog; `AssetState::Orphan` added with `as_str() == "orphan"`. `scan_hosts` builds the manifest via `Manifest::from_snapshot(&snap, harness.manifest_path())`.

- [ ] Tests first (table-driven in `plan.rs`): a catalog with one skill, one hook, one MCP server, one plugin ref; snapshots/manifests producing each op: create (empty snapshot), adopt (present matching, empty manifest), noop (present matching, in manifest), update (manifest hash ≠ render hash, host file equals nothing), overwrite (in manifest with same hash, host hash differs → `backup`), blocked missing secret, blocked unsupported (codex + hook), plugin install/update/blocked-pinned, orphan remove from a manifest key the catalog lacks; filter by name yields one action; `counts` tallies; registry put/take/expiry (inject a clock or test with a 0-TTL constructor). `inventory.rs`: `compute_states` sets `managed` and emits an `orphan` row.
- [ ] Implement, verify (`cargo test service::catalog`), fmt/clippy/full, commit `feat(catalog): sync plan computation, plan registry, managed and orphan inventory states`.

---

### Task 6: The applier

**Files:**
- Create: `src-tauri/src/service/catalog/sync/apply.rs`
- Modify: `sync/mod.rs` (`pub mod apply;`)

**Interfaces (Produces):**

```rust
pub struct GuardedWrite { pub path: String, pub expected: Option<String>, pub bytes: Vec<u8>, pub backup: bool, pub delete: bool }
/// Bash scripts (no single quotes) that apply the writes with compare-and-swap; chunked at ~512 KB of base64.
pub fn write_scripts(writes: &[GuardedWrite]) -> Vec<String>
#[derive(Debug, Clone, PartialEq, Serialize)] #[serde(rename_all = "snake_case")]
pub enum WriteOutcome { Ok, Conflict, Failed }
/// Parse `OK <path>` / `CONFLICT <path>` / `FAIL <path>` lines.
pub fn parse_write_output(stdout: &str) -> BTreeMap<String, WriteOutcome>
#[derive(Debug, Clone, Serialize)] pub struct ActionResult { pub kind: String, pub name: String, pub op: ActionOp, pub outcome: String /* done|conflict|failed|blocked|skipped */, pub detail: Option<String> }
#[derive(Debug, Clone, Serialize)] pub struct HostSyncResult { pub host_alias: String, pub harness: String, pub status: String /* applied|partial|skipped|failed */, pub detail: Option<String>, pub restart_required: bool, pub actions: Vec<ActionResult> }
pub struct ApplyCtx<'a> { pub ssh: &'a Arc<SshClient>, pub token: CancellationToken, pub now: i64 }
pub async fn apply_host(ctx: &ApplyCtx<'_>, harness: &dyn Harness, plan: &HostPlan) -> HostSyncResult
```

Script shape (`write_scripts`), one script per chunk:

```
cd "$HOME" || exit 1; T=$(date +%s)
if command -v sha256sum >/dev/null 2>&1; then H=sha256sum; else H="shasum -a 256"; fi
h() { if [ -f "$1" ]; then $H "$1" | cut -d " " -f1; else echo absent; fi; }
# per write (paths are ~/-relative constants, rendered as "$HOME"/rel; NAME is the ~/ path for the report line)
p="$HOME"/.claude/skills/worktree/SKILL.md; d=$(dirname "$p"); cur=$(h "$p")
if [ "$cur" != "<expected or absent>" ]; then echo "CONFLICT ~/.claude/skills/worktree/SKILL.md"; else
  [backup:] if [ -f "$p" ]; then cp -p "$p" "$p.fleet-bak-$T"; fi
  [delete:] rm -f "$p" && rmdir "$d" 2>/dev/null; echo "OK <path>"
  [write:]  mkdir -p "$d" && printf %s "<base64>" | base64 -d > "$p" && echo "OK <path>" || echo "FAIL <path>"
fi
```

Rules: `expected` compares against the scanned hash; the applier records the intended new hash so a re-run after success sees `noop`. Report lines use the `~/` path verbatim; paths never contain spaces or quotes because names are validated kebab-case and directories are constants (assert with `debug_assert!` and skip + `Failed` otherwise).

`apply_host` sequence for `status: planned` hosts (skipped hosts are returned as `skipped` untouched):
1. Partition actions: `Blocked`/`Noop` → results `blocked`/`skipped` immediately. `Adopt` → manifest only.
2. Plain-file writes from `Create/Update/Overwrite` (`plan.files` minus `secret_files`) and `Remove` (files from `remove_entry`, `delete: true`, `backup: true`) → `write_scripts` → run each via `inventory::run_host_script` (extend it, or add `run_host_script_cancellable`, to accept the token and a 5-minute wall clock) → `parse_write_output` → per-action outcomes (an action with any `CONFLICT` is `conflict`; any `FAIL` is `failed`; no further writes for that action).
3. Secret files: for each, run a one-line `h` check script; on match `provision::write_host_file_secret(ssh, host, dir, path, content)`; else `conflict`. Content must be UTF-8 (it was substituted as text).
4. Config merges: group by file across all non-conflicted actions (adds from `plan.merges`, removes from `remove_entry.merges`); existing text = `serde_json::to_string_pretty(&snapshot.configs[file])` (or the TOML re-serialisation for Codex; both come through `harness.merge_config` which accepts empty for a missing file); expected hash = `snapshot.files.get(file)`; write via one `GuardedWrite` (secret-bearing if any action's `secret_merge_files` names it → secret path instead). Outcome propagates to every action touching that file.
5. Plugins: for `PluginInstall`/`PluginUpdate`: `command -v claude >/dev/null || { echo NOCLI; exit 0; }`; marketplace add when `configs[PLUGINS_PATH]` … cannot tell → run `claude plugin marketplace add <repo> --scope user >/dev/null 2>&1 || true` first (idempotent), then `claude plugin install <plugin>@<marketplace> --scope user --json -y` (or `update <plugin> --scope user --json -y`); parse the last JSON line (`{"success":…}` or an error field); read back `PLUGINS_PATH` via `provision::read_host_file` and compare the recorded version with the pinned one → `done` with detail `installed <v>` or detail `installed <v>, catalog pins <w>`. `NOCLI` → `blocked` "claude CLI not found on host". Plugin `Remove` runs `claude plugin uninstall <plugin>@<marketplace> --scope user --json -y`.
6. Manifest: start from `plan.manifest`, apply `entry_for` for every `done` create/update/overwrite/adopt/plugin action and drop entries for `done` removes; write with `provision::write_host_file` (no CAS — it is fleet-owned) only when no action is `failed`; a failure to write the manifest makes the host `partial`.
7. `restart_required` = any done action of kind hook/mcp_server/plugin_ref. `status` = `applied` (all done/skipped/blocked, no failed/conflict), `partial` (some conflict/failed), `failed` (host script could not run at all).
8. Cancellation: check `token.is_cancelled()` between steps; if set, stop and return `partial` with detail `cancelled`.

- [ ] Tests first: `write_scripts` golden for one create (no backup), one overwrite (backup line present before write), one delete (rm + rmdir), chunking (three 300 KB payloads → two scripts), no single quotes; `parse_write_output`; then an **end-to-end local test** with a temp HOME (serialise with `CATALOG_TEST_LOCK`, restore `HOME` with a drop guard): build a `HostPlan` by hand with a `Create` for `~/.claude/skills/s/SKILL.md`, apply → file exists with the bytes and manifest written; edit the file on disk, plan `Overwrite` with `expected` = old hash → conflict, nothing written; plan `Overwrite` with the current hash → backup file exists, content replaced; a `Remove` with `remove_entry` → file gone, backup exists, manifest entry dropped; a config merge into `~/.claude/settings.json` from empty → valid JSON with the hook, second apply is a `noop`-shaped plan. Plugin steps are tested only through `parse` helpers (the CLI is not invoked in unit tests).
- [ ] Implement, verify (`cargo test service::catalog::sync::apply`), fmt/clippy/full, commit `feat(catalog): guarded per-host applier with backups, secret uploads, config merges, plugins and manifest`.

---

### Task 7: Orchestration — plan_sync and apply_sync

**Files:**
- Modify: `src-tauri/src/service/catalog/sync/mod.rs`, `src-tauri/src/service/catalog/mod.rs` (error codes)

**Interfaces (Produces):**

```rust
pub const E_SYNC_PLAN_STALE: &str = "E_SYNC_PLAN_STALE";   // in catalog/mod.rs next to the others
pub const E_SECRET_MISSING: &str = "E_SECRET_MISSING";
#[derive(Deserialize)] pub struct PlanArgs { pub host_alias: Option<String>, pub kind: Option<Kind>, pub name: Option<String> }
#[derive(Deserialize)] pub struct ApplyArgs { pub plan_id: String, #[serde(default)] pub force_partial: bool, pub call_id: Option<u64> }
#[derive(Serialize)] pub struct SyncRunSummary { pub plan_id: String, pub started_at: i64, pub finished_at: i64, pub hosts: Vec<HostSyncResult> }
pub async fn plan_sync(args: PlanArgs, store: &Mutex<Store>, ssh: &Arc<SshClient>) -> Result<SyncPlan, IpcError>
pub async fn apply_sync(args: ApplyArgs, store: &Mutex<Store>, ssh: &Arc<SshClient>, reg: &Arc<CancellationRegistry>) -> Result<SyncRunSummary, IpcError>
pub fn last_sync(store: &Mutex<Store>) -> Result<Option<SyncRunSummary>, IpcError>
```

`plan_sync`: require catalog loaded (clone out of `CATALOG`); list hosts (skip hidden; filter); for each reachable host and each harness with a scan script: `scan_host_harness` → `Manifest::from_snapshot` → `secrets::resolve` (store lock scoped, before the await) → `compute_host_plan`; unreachable → `HostPlan { status: "skipped", detail: "unreachable" }`; scan failure → `skipped` with the error message; persist the fresh inventory rows too (same as `scan_hosts`), then `registry_put` and return the plan (with `counts`).

`apply_sync`: `registry_take(plan_id)` or `E_SYNC_PLAN_STALE`; if any action is `Blocked` with `missing_secrets` and `!force_partial` → `E_SECRET_MISSING` listing names; bind cancellation (`call_id` → `reg.bind`, else `register_anonymous`, with `CancelGuard`); for each planned host/harness: emit `sync:progress` (done/total over host plans), `apply_host`, then re-scan that host/harness and `replace_host_inventory` (so the matrix updates through the existing events); if the token is cancelled, remaining hosts get `skipped` "cancelled"; `record_sync_run` with the summary JSON; return the summary. Use `tracing::info!` for per-host outcomes (never values).

- [ ] Tests first: `plan_sync` on a store with only `local` and a temp catalog (like `scan_hosts_scans_local_and_persists_rows`) returns one HostPlan per harness and puts it in the registry; `apply_sync` with an unknown id → `E_SYNC_PLAN_STALE`; a plan containing a blocked action with missing secrets → `E_SECRET_MISSING` unless `force_partial`; an end-to-end `plan_sync` → `apply_sync` against `local` with a temp HOME for a single tiny catalog skill (guarded by `CATALOG_TEST_LOCK`, HOME restored) results in `applied` and a `sync_runs` row, and a second plan shows `noop`.
- [ ] Implement, verify, fmt/clippy/full, commit `feat(catalog): plan_sync and apply_sync orchestration with progress and run history`.

---

### Task 8: Commands, MCP tools, guard, docs regeneration

**Files:**
- Modify: `src-tauri/src/commands/assets.rs`, `src-tauri/src/lib.rs` (register), `src-tauri/src/mcp/tools/assets.rs`, `src-tauri/src/mcp/tools/params.rs`, `src-tauri/src/mcp/guard.rs` (+ its tests), `src-tauri/src/mcp/tools/mod.rs` (tool-count guard), `docs/control-api-reference.md` (regenerated)

**Commands** (thin wrappers): `catalog_plan_sync(args: PlanArgs, store, ssh)`, `catalog_apply_sync(args: ApplyArgs, store, ssh, reg)`, `catalog_last_sync(store)`, `catalog_list_secrets(store) -> Vec<SecretRow>`, `catalog_set_secret(args: { name, host_alias?, value }, store)` (validate `name` matches `[A-Z0-9_]+`, else `E_INVALID`; never log the value), `catalog_delete_secret(args: { name, host_alias? }, store) -> bool`.

**MCP tools** in `mcp/tools/assets.rs`:
- `plan_sync(PlanSyncParams { host_alias?, kind?, name? })` — description: "Compute a sync plan: scan the selected hosts, compare every catalog asset with what is installed, and return per-host actions (create | update | overwrite | adopt | remove | plugin_install | plugin_update | noop | blocked) plus a plan_id valid for 10 minutes. Nothing is written. Pass the plan_id to apply_sync." Audit: filters only.
- `apply_sync(ApplySyncParams { plan_id, host_alias?, force_partial, confirm_nonce? })` — `confirm_gate("apply_sync", nonce, "plan_id=<id> host=<alias|*>", &caller)`; when `host_alias` is None require the master token (mirror how `provision_hosts` checks `is_admin_tool` / caller identity — read `guard.rs` and `tools/hosts.rs` for the exact helper); description: "Apply a plan from plan_sync on hosts: writes files with compare-and-swap, backs up overwritten files, merges config, installs plugins, writes the managed manifest, then re-scans. Requires confirmation. Returns per-host results; restart_required marks hosts whose Claude must be restarted."
- `set_secret(SetSecretParams { name, value, host_alias? })` — admin-only; audit `name=<name> host=<alias|global>` only; returns `{ "ok": true }`.
- `guard.rs`: add `"apply_sync"` to `CONFIRM_TOOLS` (test 7 → 8) and `"apply_sync", "set_secret"` to `ADMIN_TOOLS` semantics as the spec describes (if `ADMIN_TOOLS` is an all-or-nothing list, add `set_secret` there and gate `apply_sync`'s no-host case inline); extend the classification tests.
- Bump the tool-count guard in `mcp/tools/mod.rs` (60 → 63) and regenerate the reference.

- [ ] Steps: implement, `cargo test mcp::`, `REGEN_DOCS=1 cargo test reference_is_current`, fmt/clippy/full, commit `feat(catalog): sync and secrets commands and MCP tools (plan_sync, apply_sync, set_secret)`.

---

### Task 9: Frontend — store, plan dialog, secrets panel, wiring

**Files:**
- Modify: `src/lib/assets.ts` (+ `assets.test.ts`), `src/lib/events.ts` (+ test), `src/lib/AssetsPanel.svelte` (+ test), `src/lib/AssetDetail.svelte`
- Create: `src/lib/SyncPlanDialog.svelte` (+ `SyncPlanDialog.test.ts`), `src/lib/SecretsPanel.svelte` (+ `SecretsPanel.test.ts`)

**assets.ts additions:**
```ts
export interface AssetInventoryRow { …; managed: boolean }        // add
export type AssetState = … | 'orphan';
export type ActionOp = 'create'|'update'|'overwrite'|'adopt'|'remove'|'plugin_install'|'plugin_update'|'noop'|'blocked';
export interface SyncAction { kind: string; name: string; op: ActionOp; reason: string | null; files: string[]; merges: string[]; backup: boolean; secrets: string[]; missing_secrets: string[] }
export interface HostPlan { host_alias: string; harness: string; status: string; detail: string | null; actions: SyncAction[] }
export interface SyncPlan { id: string; computed_at: number; hosts: HostPlan[]; counts: Record<string, number> }
export interface ActionResult { kind: string; name: string; op: ActionOp; outcome: string; detail: string | null }
export interface HostSyncResult { host_alias: string; harness: string; status: string; detail: string | null; restart_required: boolean; actions: ActionResult[] }
export interface SyncRunSummary { plan_id: string; started_at: number; finished_at: number; hosts: HostSyncResult[] }
export interface SecretRow { name: string; host_alias: string | null; updated_at: number }
export interface SyncProgress { plan_id: string; host_alias: string; harness: string; done: number; total: number }
export function planSync(f: { hostAlias?: string; kind?: string; name?: string }): Promise<Result<SyncPlan>>   // 'catalog_plan_sync' { args: { host_alias, kind, name } } (nulls for absent)
export function applySync(planId: string, forcePartial: boolean, signal?: AbortSignal): Promise<Result<SyncRunSummary>> // invokeCmdAbortable('catalog_apply_sync', { args: { plan_id, force_partial } }, signal)
export function lastSync(): Promise<Result<SyncRunSummary | null>>
export function listSecrets(): Promise<Result<SecretRow[]>>; setSecret(name, value, hostAlias?: string); deleteSecret(name, hostAlias?: string)
export function isDestructive(plan: SyncPlan): boolean   // any overwrite/remove
export const lastSyncRun = writable<SyncRunSummary | null>(null)
```
`events.ts`: `onSyncProgress?: (p: SyncProgress) => void` on `sync:progress` (follow the batching-queue pattern used for the other catalog events).

**SyncPlanDialog** (`props: { plan: SyncPlan; onclose(); onapplied(summary) }`): built on `Modal`; header counts (`plan.counts`); per host/harness section (`data-testid="plan-host-<alias>-<harness>"`) with rows `data-testid="plan-action-<host>-<harness>-<kind>-<name>"` showing op badge, asset, backup marker, secret names, `reason` for blocked; blocked-with-missing-secrets rows link to the secrets panel; **Apply** button (`data-testid="plan-apply"`, red via `danger` when `isDestructive`), disabled when there are zero applicable actions; a `force_partial` checkbox appears only when some actions are blocked; during apply a progress line (`data-testid="plan-progress"`, from `sync:progress`) and a Cancel button that aborts the call; after apply rows show outcome (`done/conflict/failed/blocked/skipped`) and a per-host strip with "restart Claude on <host>" when `restart_required`.

**SecretsPanel** (`props: { names: string[] /* from secrets.example + stored */, onclose() }`): list of rows (name, "set"/"not set" per global and per host override), masked input + Set, per-host override select + Set, Delete; never displays values; calls `listSecrets`/`setSecret`/`deleteSecret`; `data-testid="secret-row-<name>"`, `secret-set-<name>`.

**AssetsPanel**: toolbar gains **Sync** (`data-testid="assets-sync"`, `busy: 'plan'`) → `planSync({})` → open `SyncPlanDialog`; **Secrets** (`data-testid="assets-secrets"`) → `SecretsPanel`; last-sync strip from `lastSync()` on mount (`data-testid="assets-last-sync"`). `AssetDetail`: **Sync this asset** button (`data-testid="asset-sync"`) → `planSync({ kind, name })` → dialog; matrix cells with state `missing | drifted | orphan` get a small **Sync** link (`data-testid="cell-sync-<host>-<harness>"`) → `planSync({ hostAlias, kind, name })`. `App.svelte` subscribes `onSyncProgress` and forwards into a `syncProgress` store in `assets.ts` (`export const syncProgress = writable<SyncProgress | null>(null)`).

- [ ] Tests first: `assets.test.ts` — arg shapes for the six wrappers, `isDestructive`; `events.test.ts` — `sync:progress` handler; `SyncPlanDialog.test.ts` — renders counts and grouped rows, red apply on overwrite, blocked row shows reason and hides apply when nothing applicable, apply calls `catalog_apply_sync` with the id and renders outcomes and restart note; `SecretsPanel.test.ts` — masked input, set calls `catalog_set_secret` with `{ name, host_alias: null, value }`, delete; `AssetsPanel.test.ts` — Sync button calls `catalog_plan_sync` and opens the dialog; Secrets button opens the panel.
- [ ] Implement; `pnpm run check && pnpm run test && pnpm run build`; commit `feat(ui): sync plan dialog, secrets panel, and sync actions in the Assets tab`.

---

### Task 10: Docs

- `docs/concepts.md` "Asset catalog" section: add a paragraph on sync (plan first, compare-and-swap, backups, manifest, secrets, plugins via the CLI, Codex experimental).
- `docs/control-api.md`: add the three tools to the Asset catalog bullet/table with the confirm and admin notes; mention `apply_sync` needs `mcp.confirm_destructive` acknowledgement.
- Spec `2026-09-14-asset-sync-design.md`: verify the manifest example matches the implemented `ManifestEntry` (files, merges with `value_hash`, `synced_at`) and that Codex TOML comment loss is stated; adjust if not.
- Commit `docs: describe the sync engine and its MCP tools`.

---

## Plan self-review notes

- Spec coverage: manifest (T4), inventory `managed`/`orphan` (T1, T5), plan actions and registry (T5), secrets tables/resolution/substitution (T1, T4), apply sequence incl. CAS, backups, secret uploads, config merges, plugins, manifest-last, re-scan (T6, T7), Codex scan + TOML (T3), Harness trait additions (T2), commands/MCP/guard/events/errors (T7, T8), UI (T9), docs (T10), testing per section.
- Deliberate deviations from the spec: the manifest stores `value_hash` per merge and removal matches AppendUnique elements by hash (no substituted values in the manifest); config-file hashes are added to the scan so config writes can be compare-and-swapped; `PluginRef` removal uses `claude plugin uninstall`; `run_host_script` gains a cancellable variant.
- Type consistency: `ManifestMerge` (T2) is used by `Manifest` (T4), `Action.remove_entry` (T5) and `apply_host` (T6); `HostSyncResult`/`ActionResult` (T6) are returned by `apply_sync` (T7), wrapped by T8 and typed in T9; `SyncProgress` (T1) is emitted in T7 and consumed in T9; `AssetInventoryRow.managed` (T1) is set in T5 and typed in T9.
