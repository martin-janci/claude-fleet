# Assets S1a: inventory foundation — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the unmanaged-asset list collapse from one row per host copy to one row per asset, with content hashes, fleet-internal and secret flags, an automatic scan, import from any host, and a guard that stops Sync from installing the whole catalog on an unlayered remote host.

**Architecture:** The scan already hashes every file; a new `Harness::installed_detail` keeps a per-asset hash plus two flags for assets the catalog does not know, persisted in two new `asset_inventory` columns. A pure `identity` module groups inventory rows into identities and classifies them by rules; `list_assets` returns them beside the old rows. A background tick rescans stale hosts. Remote import stages the host's files into a temp directory through one SSH script and reuses the existing importer. Planning refuses unlayered remote hosts unless the caller opts in.

**Tech Stack:** Rust (fleet-core, src-tauri, fleet-hub), rusqlite migrations, Svelte 5 + Vitest.

**Spec:** `docs/superpowers/specs/2026-09-29-assets-workspace-design.md` — sub-project **S1**. This plan is **S1a**. S1's other half, **S1b — multiple catalogs, scopes and the scope boundary**, is a separate plan: it changes the global `CATALOG` into a set of catalogs and needs its own data-model decisions (catalogs table, `host_catalogs`), so it is planned once S1a lands.

## Global Constraints

- Every value interpolated into an SSH/bash string goes through `crate::shell::quote`.
- Every child process is built by `fleet_core::proc::command` / `std_command`, never `Command::new`.
- Never hold the `Store` mutex guard across an `.await`.
- New migration = `crates/fleet-core/migrations/NNN_<topic>.sql` + an entry in `store/schema.rs` `MIGRATIONS`; an `ADD COLUMN` needs an `already_applied` guard (as 084).
- A new `SPECS` row in `service/settings.rs` needs a `field` on a settings page (`every_setting_has_one_home`), then `REGEN_SETTINGS_DOCS=1 cargo test -p fleet-core settings_docs_are_current` and `REGEN_PAGE_DOCS=1 cargo test -p fleet-core page_docs_are_current`.
- Editing a `#[tool(description = …)]` or its params → `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`.
- Editing a row in `src-tauri/src/backend/verdicts.rs` → `REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen`.
- Frontend: `pnpm test`, `pnpm check`. Backend: `cargo test --workspace` before the final commit.
- Wire compatibility: every new field on a serialized struct is `#[serde(default)]`, and the frontend treats it as optional, because a paired desktop may talk to an older hub.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/fleet-core/src/service/catalog/harness/mod.rs` | + `InstalledAsset`, `Harness::installed_detail` (default), `canonical_json` |
| `crates/fleet-core/src/service/catalog/harness/claude.rs` | `installed_detail` for skills, agents, hooks, MCP servers, plugins |
| `crates/fleet-core/src/service/catalog/harness/codex.rs` | `installed_detail` for skills and MCP servers |
| `crates/fleet-core/src/service/hooks_install.rs` | `is_fleet_hook_entry` → `pub(crate)`; + `is_fleet_command_entry` |
| `crates/fleet-core/src/service/provision.rs` | + `FLEET_SKILL_NAMES`, `FLEET_MCP_SERVER` |
| `crates/fleet-core/migrations/086_inventory_flags.sql` | `secret_like`, `fleet_owned` columns |
| `crates/fleet-core/src/store/{rows,catalog,schema}.rs` | row fields, insert/select, migration entry, `inventory_last_scans` |
| `crates/fleet-core/src/service/catalog/inventory.rs` | unmanaged rows carry hash and flags |
| `crates/fleet-core/src/service/catalog/identity.rs` (new) | `group_identities`, `IdentityClass` rules |
| `crates/fleet-core/src/service/catalog/mod.rs` | `AssetListing.identities`; async `import_host` with remote staging and `only` |
| `crates/fleet-core/src/service/catalog/import.rs` | `REMOTE_SOURCES_SCRIPT`, `parse_remote_dump`, `import_claude_only` |
| `crates/fleet-core/src/service/catalog/scan_tick.rs` (new) | `hosts_due` (pure) and `spawn_catalog_scan_tick` |
| `crates/fleet-core/src/service/settings.rs` + `pages/settings.automation.json` | two `catalog.*` settings |
| `crates/fleet-core/src/service/catalog/sync/mod.rs` | `PlanArgs.allow_unlayered`, the unlayered guard |
| `crates/fleet-core/src/service/catalog/admin.rs` | + `AdminCall::ImportHost` |
| `crates/fleet-core/src/mcp/tools/{assets,params}.rs` | import passes SSH; `allow_unlayered` param |
| `src-tauri/src/commands/assets.rs`, `src-tauri/src/backend/verdicts.rs` | import routed to the hub |
| `src-tauri/src/backend/startup.rs`, `src-tauri/src/bootstrap/tasks.rs`, `crates/fleet-hub/src/serve.rs` | start the scan tick |
| `src/lib/assets.ts` | `AssetIdentity` type, `identitiesOf`, `importHost(…, only)` |
| `src/lib/HostStrip.svelte` (new) | the dot strip |
| `src/lib/AssetList.svelte`, `src/lib/ImportDialog.svelte`, `src/lib/AssetsPanel.svelte` | identity rows, internals toggle, import any host |

---

### Task 1: Per-asset detail from a host snapshot

**Files:**
- Modify: `crates/fleet-core/src/service/catalog/harness/mod.rs`
- Modify: `crates/fleet-core/src/service/catalog/harness/claude.rs`
- Modify: `crates/fleet-core/src/service/catalog/harness/codex.rs`
- Modify: `crates/fleet-core/src/service/hooks_install.rs:55`
- Modify: `crates/fleet-core/src/service/provision.rs` (constants near line 15)

**Interfaces:**
- Produces:
  - `pub struct InstalledAsset { pub kind: Kind, pub name: String, pub hash: Option<String>, pub secret_like: bool, pub fleet_owned: bool }` in `harness/mod.rs`
  - `fn installed_detail(&self, snap: &HostSnapshot) -> Vec<InstalledAsset>` on `Harness`, with a default built from `installed()` (hash `None`, both flags `false`)
  - `pub fn canonical_json(v: &serde_json::Value) -> String` in `harness/mod.rs`
  - `pub(crate) fn is_fleet_hook_entry(h: &Value) -> bool` and `pub(crate) fn is_fleet_command_entry(h: &Value) -> bool` in `hooks_install.rs`
  - `pub const FLEET_SKILL_NAMES: &[&str] = &["claude-fleet-control", "fleet-friendly-name"];` and `pub const FLEET_MCP_SERVER: &str = "claude-fleet";` in `provision.rs`

Hash rules (one per kind; `snap.files` values are already sha256 hex):
- skill: sha256 of the sorted lines `<path relative to the skill dir>=<file hash>` for every file under `<skills dir>/<name>/`
- agent: the hash of `<agents dir>/<name>.md`
- hook: sha256 of `canonical_json` of the array of this name's matcher groups, each reduced to its **non-fleet** entries; `fleet_owned` = every entry under the name is a fleet entry; `secret_like` = any non-fleet entry has a non-empty `headers` object
- MCP server: sha256 of `canonical_json` of the server's value; `secret_like` = a non-empty `env` or `headers` object, or a `url` containing `token=`, `key=` or `secret=`; `fleet_owned` = name is `FLEET_MCP_SERVER`
- plugin: sha256 of `canonical_json` of `{"key": <plugin@marketplace>, "value": <entry>}`

- [ ] **Step 1: Write the failing tests** (append to the `tests` module of `harness/claude.rs`)

```rust
fn detail_snap() -> HostSnapshot {
    let mut s = HostSnapshot::default();
    s.files.insert(format!("{SKILLS_DIR}/worktree/SKILL.md"), "aa".into());
    s.files.insert(format!("{SKILLS_DIR}/worktree/scripts/go.sh"), "bb".into());
    s.files.insert(format!("{SKILLS_DIR}/claude-fleet-control/SKILL.md"), "cc".into());
    s.files.insert(format!("{AGENTS_DIR}/pm-qa.md"), "dd".into());
    s.configs.insert(
        SETTINGS_PATH.into(),
        serde_json::json!({"hooks": {
            "Stop": [
                {"hooks": [{"type": "command", "command": "node stop.mjs"}]},
                {"hooks": [{"type": "http", "url": "http://127.0.0.1:4180/hook",
                             "headers": {"Authorization": "Bearer T"}, "timeout": 5}]}
            ],
            "UserPromptSubmit": [
                {"hooks": [{"type": "http", "url": "http://127.0.0.1:4180/hook",
                             "headers": {"Authorization": "Bearer T"}, "timeout": 5}]}
            ]
        }}),
    );
    s.configs.insert(
        CLAUDE_JSON_PATH.into(),
        serde_json::json!({"mcpServers": {
            "claude-fleet": {"type": "http", "url": "http://x/mcp", "headers": {"Authorization": "Bearer T"}},
            "jira": {"type": "stdio", "command": "npx", "env": {"JIRA_TOKEN": "abc"}},
            "fs": {"type": "stdio", "command": "mcp-fs"}
        }}),
    );
    s
}

fn detail<'a>(d: &'a [InstalledAsset], kind: Kind, name: &str) -> &'a InstalledAsset {
    d.iter().find(|a| a.kind == kind && a.name == name).unwrap()
}

#[test]
fn installed_detail_hashes_skill_dirs_order_independently() {
    let d = Claude.installed_detail(&detail_snap());
    let want = sha256_hex(b"SKILL.md=aa\nscripts/go.sh=bb");
    assert_eq!(detail(&d, Kind::Skill, "worktree").hash.as_deref(), Some(want.as_str()));
    assert_eq!(detail(&d, Kind::Agent, "pm-qa").hash.as_deref(), Some("dd"));
}

#[test]
fn installed_detail_marks_fleet_internals() {
    let d = Claude.installed_detail(&detail_snap());
    assert!(detail(&d, Kind::Skill, "claude-fleet-control").fleet_owned);
    assert!(!detail(&d, Kind::Skill, "worktree").fleet_owned);
    assert!(detail(&d, Kind::McpServer, "claude-fleet").fleet_owned);
    // Only fleet's entry under this name.
    assert!(detail(&d, Kind::Hook, "prompt-submit").fleet_owned || detail(&d, Kind::Hook, "user-prompt-submit").fleet_owned);
    // "stop" mixes the user's hook with fleet's: not fleet-owned, and the
    // hash covers the user's entry only (no token in it).
    let stop = detail(&d, Kind::Hook, "stop");
    assert!(!stop.fleet_owned);
    let user_only = serde_json::json!([{"hooks": [{"type": "command", "command": "node stop.mjs"}]}]);
    assert_eq!(stop.hash.as_deref(), Some(sha256_hex(canonical_json(&user_only).as_bytes()).as_str()));
}

#[test]
fn installed_detail_flags_secret_like_mcp_servers() {
    let d = Claude.installed_detail(&detail_snap());
    assert!(detail(&d, Kind::McpServer, "jira").secret_like);
    assert!(!detail(&d, Kind::McpServer, "fs").secret_like);
}

#[test]
fn canonical_json_sorts_keys_recursively() {
    let a = serde_json::json!({"b": 1, "a": {"d": 2, "c": [ {"y": 1, "x": 2} ]}});
    assert_eq!(canonical_json(&a), r#"{"a":{"c":[{"x":2,"y":1}],"d":2},"b":1}"#);
}
```

(The `UserPromptSubmit` hook's asset name comes from `hook_asset_name`; assert against whichever name `Claude.installed(&detail_snap())` reports for it — print it once while writing the test and pin the literal, dropping the `||`.)

In `harness/codex.rs` tests:

```rust
#[test]
fn codex_installed_detail_hashes_skills_and_flags_env() {
    let mut s = HostSnapshot::default();
    s.files.insert(format!("{CODEX_SKILLS_DIR}/worktree/SKILL.md"), "aa".into());
    s.configs.insert(
        CODEX_CONFIG_PATH.into(),
        serde_json::json!({"mcp_servers": {"jira": {"command": "npx", "env": {"T": "x"}}}}),
    );
    let d = Codex.installed_detail(&s);
    let skill = d.iter().find(|a| a.kind == Kind::Skill).unwrap();
    assert_eq!(skill.hash.as_deref(), Some(sha256_hex(b"SKILL.md=aa").as_str()));
    assert!(d.iter().find(|a| a.kind == Kind::McpServer).unwrap().secret_like);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p fleet-core catalog::harness`
Expected: compile errors — `InstalledAsset`, `installed_detail`, `canonical_json` not found.

- [ ] **Step 3: Implement**

In `provision.rs`, next to the existing skill constants:

```rust
/// Skills fleet provisions on every host. The catalog treats them as fleet
/// internals: they are never offered for import.
pub const FLEET_SKILL_NAMES: &[&str] = &["claude-fleet-control", "fleet-friendly-name"];
/// The `mcpServers` key fleet provisions (it carries the host's token).
pub const FLEET_MCP_SERVER: &str = "claude-fleet";
```

In `hooks_install.rs`, make the existing predicate crate-visible and add the command form:

```rust
pub(crate) fn is_fleet_hook_entry(h: &serde_json::Value) -> bool { /* body unchanged */ }

/// True when `h` is fleet's SessionStart `command` hook: a curl that sends
/// the headers file fleet writes and the pane header.
pub(crate) fn is_fleet_command_entry(h: &serde_json::Value) -> bool {
    h.get("type").and_then(|t| t.as_str()) == Some("command")
        && h.get("command").and_then(|c| c.as_str()).is_some_and(|c| {
            c.contains(&format!(".claude/{HOOK_HEADERS_FILE}")) && c.contains("X-Fleet-Pane")
        })
}
```

In `harness/mod.rs`:

```rust
/// One installed asset with what a scan can say about it without the
/// catalog: a content hash (so identical copies on different hosts are
/// recognisable), whether it looks like it carries a secret, and whether
/// fleet itself put it there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledAsset {
    pub kind: Kind,
    pub name: String,
    pub hash: Option<String>,
    pub secret_like: bool,
    pub fleet_owned: bool,
}

/// JSON with every object's keys sorted, recursively: a stable input for
/// hashing whatever order the host wrote.
pub fn canonical_json(v: &serde_json::Value) -> String {
    fn sorted(v: &serde_json::Value) -> serde_json::Value {
        match v {
            serde_json::Value::Object(m) => {
                let mut keys: Vec<&String> = m.keys().collect();
                keys.sort();
                let mut out = serde_json::Map::new();
                for k in keys {
                    out.insert(k.clone(), sorted(&m[k]));
                }
                serde_json::Value::Object(out)
            }
            serde_json::Value::Array(a) => serde_json::Value::Array(a.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    sorted(v).to_string()
}

/// sha256 over `<relative path>=<file hash>` lines for every file under
/// `prefix` (which ends in `/`), sorted by path. `None` when there is none.
pub fn dir_hash(snap: &HostSnapshot, prefix: &str) -> Option<String> {
    let mut lines: Vec<String> = snap
        .files
        .iter()
        .filter_map(|(p, h)| p.strip_prefix(prefix).map(|rel| format!("{rel}={h}")))
        .collect();
    if lines.is_empty() {
        return None;
    }
    lines.sort();
    Some(super::model::sha256_hex(lines.join("\n").as_bytes()))
}

/// Does an MCP server entry look like it carries a credential?
pub fn mcp_secret_like(v: &serde_json::Value) -> bool {
    let non_empty = |k: &str| v.get(k).and_then(|o| o.as_object()).is_some_and(|o| !o.is_empty());
    let url_secret = v
        .get("url")
        .and_then(|u| u.as_str())
        .is_some_and(|u| ["token=", "key=", "secret="].iter().any(|s| u.to_lowercase().contains(s)));
    non_empty("env") || non_empty("headers") || url_secret
}
```

Add to the `Harness` trait, right after `installed`:

```rust
    /// `installed()` with a content hash and flags per asset. The default
    /// knows nothing beyond the identity.
    fn installed_detail(&self, snap: &HostSnapshot) -> Vec<InstalledAsset> {
        self.installed(snap)
            .into_iter()
            .map(|(kind, name)| InstalledAsset { kind, name, hash: None, secret_like: false, fleet_owned: false })
            .collect()
    }
```

In `harness/claude.rs`, override it inside `impl Harness for Claude`:

```rust
    fn installed_detail(&self, snap: &HostSnapshot) -> Vec<InstalledAsset> {
        use crate::service::hooks_install::{is_fleet_command_entry, is_fleet_hook_entry};
        use crate::service::provision::{FLEET_MCP_SERVER, FLEET_SKILL_NAMES};
        let is_fleet = |e: &Value| is_fleet_hook_entry(e) || is_fleet_command_entry(e);
        self.installed(snap)
            .into_iter()
            .map(|(kind, name)| {
                let (hash, secret_like, fleet_owned) = match kind {
                    Kind::Skill => (
                        dir_hash(snap, &format!("{SKILLS_DIR}/{name}/")),
                        false,
                        FLEET_SKILL_NAMES.contains(&name.as_str()),
                    ),
                    Kind::Agent => (snap.files.get(&format!("{AGENTS_DIR}/{name}.md")).cloned(), false, false),
                    Kind::Hook => {
                        let mut groups: Vec<Value> = Vec::new();
                        let (mut any, mut all_fleet, mut secret) = (false, true, false);
                        if let Some(hooks) = snap.configs.get(SETTINGS_PATH).and_then(|v| v.get("hooks")).and_then(Value::as_object) {
                            for (event, entries) in hooks {
                                for g in entries.as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
                                    let matcher = g.get("matcher").and_then(Value::as_str);
                                    if hook_asset_name(event, matcher) != name {
                                        continue;
                                    }
                                    let inner: Vec<Value> = g.get("hooks").and_then(Value::as_array).cloned().unwrap_or_default();
                                    let own: Vec<Value> = inner.iter().filter(|e| !is_fleet(e)).cloned().collect();
                                    any |= !inner.is_empty();
                                    all_fleet &= own.is_empty();
                                    secret |= own.iter().any(|e| e.get("headers").and_then(Value::as_object).is_some_and(|h| !h.is_empty()));
                                    if !own.is_empty() {
                                        let mut kept = serde_json::Map::new();
                                        if let Some(m) = matcher { kept.insert("matcher".into(), Value::String(m.into())); }
                                        kept.insert("hooks".into(), Value::Array(own));
                                        groups.push(Value::Object(kept));
                                    }
                                }
                            }
                        }
                        let hash = (!groups.is_empty()).then(|| sha256_hex(canonical_json(&Value::Array(groups)).as_bytes()));
                        (hash, secret, any && all_fleet)
                    }
                    Kind::McpServer => {
                        let v = snap.configs.get(CLAUDE_JSON_PATH).and_then(|c| c.get("mcpServers")).and_then(|m| m.get(&name));
                        (
                            v.map(|v| sha256_hex(canonical_json(v).as_bytes())),
                            v.is_some_and(mcp_secret_like),
                            name == FLEET_MCP_SERVER,
                        )
                    }
                    Kind::PluginRef => {
                        let entry = snap.configs.get(PLUGINS_PATH).and_then(|v| v.get("plugins")).and_then(Value::as_object)
                            .and_then(|m| m.iter().find(|(k, _)| k.split('@').next() == Some(name.as_str())));
                        (
                            entry.map(|(k, v)| sha256_hex(canonical_json(&serde_json::json!({"key": k, "value": v})).as_bytes())),
                            false,
                            false,
                        )
                    }
                };
                InstalledAsset { kind, name, hash, secret_like, fleet_owned }
            })
            .collect()
    }
```

Import `InstalledAsset`, `canonical_json`, `dir_hash`, `mcp_secret_like` from `super` and `sha256_hex` from `super::super::model` at the top of `claude.rs` (match the file's existing `use` style).

In `harness/codex.rs`:

```rust
    fn installed_detail(&self, snap: &HostSnapshot) -> Vec<InstalledAsset> {
        self.installed(snap)
            .into_iter()
            .map(|(kind, name)| {
                let (hash, secret_like) = match kind {
                    Kind::Skill => (dir_hash(snap, &format!("{CODEX_SKILLS_DIR}/{name}/")), false),
                    Kind::McpServer => {
                        let v = snap.configs.get(CODEX_CONFIG_PATH).and_then(|c| c.get("mcp_servers")).and_then(|m| m.get(&name));
                        (v.map(|v| sha256_hex(canonical_json(v).as_bytes())), v.is_some_and(mcp_secret_like))
                    }
                    _ => (None, false),
                };
                InstalledAsset { kind, name, hash, secret_like, fleet_owned: false }
            })
            .collect()
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p fleet-core catalog::harness hooks_install`
Expected: PASS, including the existing `installed_enumerates_every_kind` and `hooks_install` tests.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/service/catalog/harness crates/fleet-core/src/service/hooks_install.rs crates/fleet-core/src/service/provision.rs
git commit -m "feat(catalog): a content hash and fleet/secret flags for every installed asset"
```

---

### Task 2: Persist hash and flags on unmanaged inventory rows

**Files:**
- Create: `crates/fleet-core/migrations/086_inventory_flags.sql`
- Modify: `crates/fleet-core/src/store/schema.rs` (MIGRATIONS list, guard fn near line 377)
- Modify: `crates/fleet-core/src/store/rows.rs:1170`
- Modify: `crates/fleet-core/src/store/catalog.rs:56-100`
- Modify: `crates/fleet-core/src/service/catalog/inventory.rs:505-519`
- Modify: every `AssetInventoryRow { … }` literal the compiler reports (14 today)
- Modify: `src/lib/assets.ts` (`AssetInventoryRow` interface)

**Interfaces:**
- Consumes: `Harness::installed_detail` (Task 1)
- Produces: `AssetInventoryRow.secret_like: bool`, `AssetInventoryRow.fleet_owned: bool` (both `#[serde(default)]`); `Store::inventory_last_scans(&self) -> Result<BTreeMap<String, i64>, rusqlite::Error>` (newest `scanned_at` per host)

- [ ] **Step 1: Write the failing tests**

In `store/catalog.rs` tests:

```rust
#[test]
fn inventory_round_trips_flags_and_reports_last_scans() {
    let s = Store::open_in_memory().unwrap();
    let row = |host: &str, name: &str, at: i64| AssetInventoryRow {
        host_alias: host.into(), harness: "claude".into(), kind: "skill".into(), name: name.into(),
        state: "unmanaged".into(), catalog_hash: None, host_hash: Some("h".into()), scanned_at: at,
        managed: false, secret_like: true, fleet_owned: true,
    };
    s.replace_host_inventory("local", "claude", &[row("local", "a", 5), row("local", "b", 9)]).unwrap();
    s.replace_host_inventory("oci", "claude", &[row("oci", "a", 7)]).unwrap();
    let got = s.list_inventory().unwrap();
    assert!(got.iter().all(|r| r.secret_like && r.fleet_owned));
    let last = s.inventory_last_scans().unwrap();
    assert_eq!(last.get("local"), Some(&9));
    assert_eq!(last.get("oci"), Some(&7));
}
```

In `service/catalog/inventory.rs` tests:

```rust
#[test]
fn unmanaged_rows_carry_hash_and_flags() {
    let cat = Catalog::default();
    let mut snap = HostSnapshot::default();
    snap.files.insert("~/.claude/skills/extra/SKILL.md".into(), "aa".into());
    snap.files.insert("~/.claude/skills/claude-fleet-control/SKILL.md".into(), "bb".into());
    let rows = compute_states(&cat, &Claude, "local", &snap, &Manifest::default(), &empty(), 1);
    let extra = rows.iter().find(|r| r.name == "extra").unwrap();
    assert_eq!(extra.state, "unmanaged");
    assert_eq!(extra.host_hash.as_deref(), Some(crate::service::catalog::model::sha256_hex(b"SKILL.md=aa").as_str()));
    assert!(!extra.fleet_owned);
    assert!(rows.iter().find(|r| r.name == "claude-fleet-control").unwrap().fleet_owned);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p fleet-core inventory_round_trips_flags unmanaged_rows_carry_hash`
Expected: compile errors on `secret_like` / `fleet_owned` / `inventory_last_scans`.

- [ ] **Step 3: Implement**

`migrations/086_inventory_flags.sql`:

```sql
-- Assets S1a: what a scan can tell about an installed asset the catalog does
-- not know. `secret_like` = its config looks like it carries a credential;
-- `fleet_owned` = fleet provisioned it (its own hooks, MCP entry, skills).
-- `host_hash` is now also filled for these rows. ADD COLUMN is not
-- idempotent: guarded in schema.rs.
ALTER TABLE asset_inventory ADD COLUMN secret_like INTEGER NOT NULL DEFAULT 0;
ALTER TABLE asset_inventory ADD COLUMN fleet_owned INTEGER NOT NULL DEFAULT 0;

INSERT OR IGNORE INTO schema_version (version) VALUES (86);
```

`store/schema.rs` — append to `MIGRATIONS` and add the guard beside `work_items_has_status_set_at`:

```rust
    // Assets S1a: `secret_like` / `fleet_owned` on `asset_inventory`.
    Migration {
        version: 86,
        sql: include_str!("../../migrations/086_inventory_flags.sql"),
        already_applied: Some(asset_inventory_has_fleet_owned),
    },
```

```rust
fn asset_inventory_has_fleet_owned(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('asset_inventory') WHERE name = 'fleet_owned'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}
```

`store/rows.rs`, in `AssetInventoryRow` after `managed`:

```rust
    /// Its config looks like it carries a credential (migration 086).
    #[serde(default)]
    pub secret_like: bool,
    /// Fleet provisioned it: its own hooks, MCP entry or skills (086).
    #[serde(default)]
    pub fleet_owned: bool,
```

`store/catalog.rs` — extend the INSERT and SELECT column lists with `secret_like, fleet_owned` (`?10`, `?11`, stored as 0/1, read with `row.get::<_, i64>(9)? != 0` and `(10)`), and add:

```rust
    /// The newest `scanned_at` per host, for hosts with any inventory row.
    pub fn inventory_last_scans(&self) -> Result<std::collections::BTreeMap<String, i64>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT host_alias, MAX(scanned_at) FROM asset_inventory GROUP BY host_alias",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        rows.collect()
    }
```

`inventory.rs` — replace `for (kind, name) in harness.installed(snap)` with `for a in harness.installed_detail(snap)`, using `let key = (a.kind, a.name.clone());` and pushing:

```rust
            rows.push(AssetInventoryRow {
                host_alias: host_alias.to_string(),
                harness: harness.id().to_string(),
                kind: a.kind.as_str().to_string(),
                name: a.name,
                state: AssetState::Unmanaged.as_str().into(),
                catalog_hash: None,
                host_hash: a.hash,
                scanned_at,
                managed: false,
                secret_like: a.secret_like,
                fleet_owned: a.fleet_owned,
            });
```

Keep the `tracing::debug!` branch and the orphan check, reading `a.kind` / `a.name`. Fix every other `AssetInventoryRow { … }` literal the compiler reports by adding `secret_like: false, fleet_owned: false` (or `..Default::default()` where the literal already lists everything else as defaults).

`src/lib/assets.ts`, in `AssetInventoryRow`:

```ts
  /** Present from hubs with migration 086; absent on older ones. */
  secret_like?: boolean; fleet_owned?: boolean;
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p fleet-core catalog store::catalog schema`
Expected: PASS, including the migration re-run tests.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/migrations/086_inventory_flags.sql crates/fleet-core/src/store crates/fleet-core/src/service/catalog src/lib/assets.ts
git commit -m "feat(catalog): unmanaged inventory rows keep their content hash and flags (migration 086)"
```

---

### Task 3: Identities — one row per asset, classified by rules

**Files:**
- Create: `crates/fleet-core/src/service/catalog/identity.rs`
- Modify: `crates/fleet-core/src/service/catalog/mod.rs` (`pub mod identity;`, `AssetListing`, `list_assets`)

**Interfaces:**
- Consumes: `AssetInventoryRow` with `host_hash`, `secret_like`, `fleet_owned` (Task 2)
- Produces:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityClass { Normal, FleetInternal, HarnessInternal, NeedsPerson }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityHost { pub host_alias: String, pub harness: String, pub host_hash: Option<String> }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetIdentity {
    pub kind: String,
    pub name: String,
    pub hosts: Vec<IdentityHost>,
    /// Sorted distinct host aliases joined by ',' — the host-set signature.
    pub signature: String,
    /// Distinct known content hashes across copies (0 when none is known).
    pub variants: usize,
    pub class: IdentityClass,
    /// Why a `needs_person` identity needs one.
    pub reason: Option<String>,
}

pub fn group_identities(rows: &[AssetInventoryRow]) -> Vec<AssetIdentity>;
```

and `AssetListing.identities: Vec<AssetIdentity>` (`#[serde(default)]`).

Rules, first match wins: a name starting with `.` → `harness_internal`; every copy `fleet_owned` → `fleet_internal`; any copy `secret_like` → `needs_person` ("carries a secret"); `variants > 1` → `needs_person` ("copies differ on <hosts that differ from the most common hash>"); otherwise `normal`. Only `state == "unmanaged"` rows are grouped; orphans stay in `unmanaged` as today. Output is sorted by kind, then name.

- [ ] **Step 1: Write the failing tests** (`identity.rs`, `#[cfg(test)] mod tests`)

```rust
use super::*;
use crate::store::AssetInventoryRow;

fn r(host: &str, kind: &str, name: &str, hash: Option<&str>) -> AssetInventoryRow {
    AssetInventoryRow {
        host_alias: host.into(), harness: "claude".into(), kind: kind.into(), name: name.into(),
        state: "unmanaged".into(), host_hash: hash.map(String::from), scanned_at: 1, ..Default::default()
    }
}

#[test]
fn groups_copies_into_one_identity_with_a_signature() {
    let rows = vec![r("oci", "skill", "w", Some("h")), r("local", "skill", "w", Some("h")), r("trn", "skill", "w", Some("h"))];
    let ids = group_identities(&rows);
    assert_eq!(ids.len(), 1);
    assert_eq!(ids[0].signature, "local,oci,trn");
    assert_eq!(ids[0].variants, 1);
    assert_eq!(ids[0].class, IdentityClass::Normal);
}

#[test]
fn differing_copies_need_a_person_and_name_the_odd_host() {
    let rows = vec![r("local", "skill", "w", Some("a")), r("oci", "skill", "w", Some("b")), r("trn", "skill", "w", Some("a"))];
    let id = &group_identities(&rows)[0];
    assert_eq!(id.class, IdentityClass::NeedsPerson);
    assert_eq!(id.reason.as_deref(), Some("copies differ on oci"));
}

#[test]
fn rules_hide_internals_and_flag_secrets() {
    let mut fleet = r("local", "hook", "stop", None);
    fleet.fleet_owned = true;
    let mut secret = r("local", "mcp_server", "jira", Some("x"));
    secret.secret_like = true;
    let rows = vec![fleet, secret, r("oci", "skill", ".system", Some("s"))];
    let ids = group_identities(&rows);
    let class = |n: &str| ids.iter().find(|i| i.name == n).unwrap().class.clone();
    assert_eq!(class("stop"), IdentityClass::FleetInternal);
    assert_eq!(class("jira"), IdentityClass::NeedsPerson);
    assert_eq!(class(".system"), IdentityClass::HarnessInternal);
}

#[test]
fn orphans_are_not_grouped() {
    let mut o = r("local", "skill", "gone", None);
    o.state = "orphan".into();
    assert!(group_identities(&[o]).is_empty());
}

/// The live fleet's shape (2026-09-29): 520 rows are 164 identities in 8
/// host-set signatures. Synthetic names, real distribution.
#[test]
fn live_shape_collapses_520_rows_to_164_identities() {
    let sets: &[(&[&str], usize)] = &[
        (&["local", "mefistos", "oci", "trn"], 82),
        (&["local"], 30),
        (&["local", "mefistos", "oci", "trn", "htz"], 17),
        (&["local", "oci", "trn"], 11),
        (&["trn"], 9),
        (&["local", "mefistos"], 9),
        (&["oci", "htz"], 5),
        (&["htz"], 1),
    ];
    let mut rows = Vec::new();
    let mut n = 0;
    for (hosts, count) in sets {
        for _ in 0..*count {
            n += 1;
            for h in *hosts {
                rows.push(r(h, "skill", &format!("s{n}"), Some("same")));
            }
        }
    }
    let ids = group_identities(&rows);
    assert_eq!(ids.len(), 164);
    let mut sigs: Vec<&str> = ids.iter().map(|i| i.signature.as_str()).collect();
    sigs.sort();
    sigs.dedup();
    assert_eq!(sigs.len(), 8);
    assert_eq!(rows.len(), 82 * 4 + 30 + 17 * 5 + 11 * 3 + 9 + 9 * 2 + 5 * 2 + 1);
}
```

(The last assertion documents the row count this distribution produces; it is not 520, because the live 520 also counts per-harness and per-kind duplicates. Keep it as the arithmetic of the fixture.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p fleet-core catalog::identity`
Expected: FAIL — module does not exist.

- [ ] **Step 3: Implement** `identity.rs`

```rust
//! Assets S1a: collapse per-host inventory rows into one identity per
//! (kind, name), and classify each by rules. Pure: no store, no I/O.

use crate::store::AssetInventoryRow;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// (IdentityClass, IdentityHost, AssetIdentity exactly as in Interfaces above)

pub fn group_identities(rows: &[AssetInventoryRow]) -> Vec<AssetIdentity> {
    let mut by_key: BTreeMap<(String, String), Vec<&AssetInventoryRow>> = BTreeMap::new();
    for r in rows.iter().filter(|r| r.state == "unmanaged") {
        by_key.entry((r.kind.clone(), r.name.clone())).or_default().push(r);
    }
    by_key
        .into_iter()
        .map(|((kind, name), copies)| {
            let mut aliases: Vec<&str> = copies.iter().map(|c| c.host_alias.as_str()).collect();
            aliases.sort();
            aliases.dedup();
            let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
            for c in &copies {
                if let Some(h) = c.host_hash.as_deref() {
                    *counts.entry(h).or_default() += 1;
                }
            }
            let variants = counts.len();
            let (class, reason) = if name.starts_with('.') {
                (IdentityClass::HarnessInternal, None)
            } else if copies.iter().all(|c| c.fleet_owned) {
                (IdentityClass::FleetInternal, None)
            } else if copies.iter().any(|c| c.secret_like) {
                (IdentityClass::NeedsPerson, Some("carries a secret".to_string()))
            } else if variants > 1 {
                let common = counts.iter().max_by_key(|(_, n)| **n).map(|(h, _)| *h);
                let mut odd: Vec<&str> = copies
                    .iter()
                    .filter(|c| c.host_hash.as_deref().is_some_and(|h| Some(h) != common))
                    .map(|c| c.host_alias.as_str())
                    .collect();
                odd.sort();
                odd.dedup();
                (IdentityClass::NeedsPerson, Some(format!("copies differ on {}", odd.join(", "))))
            } else {
                (IdentityClass::Normal, None)
            };
            AssetIdentity {
                hosts: copies
                    .iter()
                    .map(|c| IdentityHost { host_alias: c.host_alias.clone(), harness: c.harness.clone(), host_hash: c.host_hash.clone() })
                    .collect(),
                signature: aliases.join(","),
                variants,
                class,
                reason,
                kind,
                name,
            }
        })
        .collect()
}
```

`mod.rs`: add `pub mod identity;`, add to `AssetListing`:

```rust
    /// Assets S1a: `unmanaged` rows grouped per (kind, name) and classified.
    /// Absent from older hubs; the frontend groups client-side then.
    #[serde(default)]
    pub identities: Vec<identity::AssetIdentity>,
```

and in `list_assets` compute `let identities = identity::group_identities(&rows);` before `with_catalog`, setting `identities: identities.clone()` in the listing (clone because the closure is `Fn`; or move it in if the closure allows).

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p fleet-core catalog::identity catalog::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/service/catalog/identity.rs crates/fleet-core/src/service/catalog/mod.rs
git commit -m "feat(catalog): list_assets groups unmanaged copies into classified identities"
```

---

### Task 4: The list shows identities, with a host strip

**Files:**
- Modify: `src/lib/assets.ts`
- Create: `src/lib/HostStrip.svelte`
- Modify: `src/lib/AssetList.svelte`
- Modify: `src/lib/AssetsPanel.svelte:201-204` (import handler)
- Test: `src/lib/assets.test.ts`, `src/lib/AssetsPanel.test.ts:110-222`, `src/lib/hub_disabled.test.ts:212`

**Interfaces:**
- Consumes: `AssetListing.identities` (Task 3)
- Produces (TS):

```ts
export type IdentityClass = 'normal' | 'fleet_internal' | 'harness_internal' | 'needs_person';
export interface IdentityHost { host_alias: string; harness: string; host_hash: string | null }
export interface AssetIdentity {
  kind: string; name: string; hosts: IdentityHost[]; signature: string;
  variants: number; class: IdentityClass; reason: string | null;
}
export function identitiesOf(listing: AssetListing): AssetIdentity[];
export function hostOrder(ids: AssetIdentity[]): string[];
```

`identitiesOf` returns `listing.identities` when present, otherwise groups `listing.unmanaged` rows with `state === 'unmanaged'` client-side (class `normal`, `variants` 0). `hostOrder` returns every alias seen, `local` first, then alphabetical — the fixed dot order.

Row test ids: identity rows are `identity-row-${kind}-${name}`; orphan rows keep `unmanaged-row-${host}-${harness}-${kind}-${name}` and `orphan-badge-…`.

- [ ] **Step 1: Write the failing tests**

`assets.test.ts`:

```ts
it('identitiesOf prefers the server grouping and falls back to client-side', () => {
  const rows = [
    row({ host_alias: 'oci', name: 'w', state: 'unmanaged', managed: false }),
    row({ host_alias: 'local', name: 'w', state: 'unmanaged', managed: false }),
    row({ host_alias: 'local', name: 'gone', state: 'orphan' }),
  ];
  const fallback = identitiesOf({ ...listing, unmanaged: rows, identities: undefined });
  expect(fallback.map((i) => [i.name, i.signature])).toEqual([['w', 'local,oci']]);
  const server = { ...listing, unmanaged: rows, identities: [{ kind: 'skill', name: 'x', hosts: [], signature: 'trn', variants: 1, class: 'normal' as const, reason: null }] };
  expect(identitiesOf(server)[0].name).toBe('x');
});

it('hostOrder puts local first, then alphabetical', () => {
  const ids = [{ kind: 'skill', name: 'a', signature: 'oci,local', variants: 1, class: 'normal' as const, reason: null,
    hosts: [{ host_alias: 'trn', harness: 'claude', host_hash: null }, { host_alias: 'local', harness: 'claude', host_hash: null }, { host_alias: 'htz', harness: 'claude', host_hash: null }] }];
  expect(hostOrder(ids)).toEqual(['local', 'htz', 'trn']);
});
```

(`AssetListing.identities` becomes optional in the type: `identities?: AssetIdentity[]`.)

`AssetsPanel.test.ts` — replace the expectations at lines ~116-117 and ~216-221:

```ts
expect(screen.getByText('On hosts, not in catalog')).toBeTruthy();
expect(screen.getByTestId('identity-row-skill-extra')).toBeTruthy();
// … orphan case unchanged:
const orphan = await screen.findByTestId('unmanaged-row-mefistos-claude-skill-ghost');
expect(screen.getByTestId('orphan-badge-mefistos-claude-skill-ghost')).toBeTruthy();
expect(screen.getByTestId('identity-row-skill-extra').textContent).toContain('Import');
```

Add:

```ts
it('hides fleet internals behind a toggle', async () => {
  // listing fixture with identities: one normal `extra`, one fleet_internal `stop`
  // … render AssetsPanel as the neighbouring tests do …
  expect(screen.queryByTestId('identity-row-hook-stop')).toBeNull();
  await fireEvent.click(screen.getByRole('button', { name: /Show 1 fleet internal/ }));
  expect(screen.getByTestId('identity-row-hook-stop')).toBeTruthy();
});
```

`hub_disabled.test.ts:212` → `expect(screen.getByTestId('identity-row-skill-extra').textContent).not.toContain('Import');`

- [ ] **Step 2: Run to verify failure**

Run: `pnpm test src/lib/assets.test.ts src/lib/AssetsPanel.test.ts src/lib/hub_disabled.test.ts`
Expected: FAIL — `identitiesOf` missing, test ids missing.

- [ ] **Step 3: Implement**

`assets.ts` — the types above, `identities?: AssetIdentity[]` on `AssetListing`, and:

```ts
export function identitiesOf(listing: AssetListing): AssetIdentity[] {
  if (listing.identities) return listing.identities;
  const by = new Map<string, AssetIdentity>();
  for (const r of listing.unmanaged.filter((r) => r.state === 'unmanaged')) {
    const key = `${r.kind}\u0000${r.name}`;
    const id = by.get(key) ?? { kind: r.kind, name: r.name, hosts: [], signature: '', variants: 0, class: 'normal' as IdentityClass, reason: null };
    id.hosts.push({ host_alias: r.host_alias, harness: r.harness, host_hash: r.host_hash });
    by.set(key, id);
  }
  return [...by.values()]
    .map((id) => ({ ...id, signature: [...new Set(id.hosts.map((h) => h.host_alias))].sort().join(',') }))
    .sort((a, b) => a.kind.localeCompare(b.kind) || a.name.localeCompare(b.name));
}

export function hostOrder(ids: AssetIdentity[]): string[] {
  const all = new Set(ids.flatMap((i) => i.hosts.map((h) => h.host_alias)));
  const rest = [...all].filter((a) => a !== 'local').sort();
  return all.has('local') ? ['local', ...rest] : rest;
}
```

`HostStrip.svelte`:

```svelte
<script lang="ts">
  /** One dot per host in `order`. Filled = present (and, with `odd`, a
   *  half dot = present but different); ring = absent. Shape carries the
   *  state, colour only reinforces it (app.css: never colour alone). */
  let { order, present, odd = [] }: { order: string[]; present: string[]; odd?: string[] } = $props();
  const state = (h: string) => (!present.includes(h) ? 'absent' : odd.includes(h) ? 'differs' : 'present');
  const label = $derived(order.map((h) => `${h}: ${state(h)}`).join(', '));
</script>

<span class="strip" role="img" aria-label={label} title={label}>
  {#each order as h (h)}<span class="dot {state(h)}"></span>{/each}
</span>

<style>
  .strip { display: inline-flex; gap: 3px; align-items: center; }
  .dot { width: 9px; height: 9px; border-radius: 50%; box-sizing: border-box; }
  .present { background: var(--usage-ok); }
  .differs { background: linear-gradient(90deg, var(--usage-warn) 50%, transparent 50%); box-shadow: inset 0 0 0 1.5px var(--usage-warn); }
  .absent { box-shadow: inset 0 0 0 1.5px var(--control-border-strong); }
</style>
```

`AssetList.svelte` — keep the managed groups; replace the unmanaged block:

```svelte
  const ids = $derived(identitiesOf(listing).filter((i) => filter === '' || i.name.toLowerCase().includes(filter.toLowerCase())));
  const order = $derived(hostOrder(identitiesOf(listing)));
  let showInternals = $state(false);
  const internal = (i: AssetIdentity) => i.class === 'fleet_internal' || i.class === 'harness_internal';
  const visible = $derived(ids.filter((i) => showInternals || !internal(i)));
  const hiddenCount = $derived(ids.filter(internal).length);
  const orphans = $derived(listing.unmanaged.filter((r) => r.state === 'orphan' && (filter === '' || r.name.includes(filter))));
  const oddHosts = (i: AssetIdentity) => (i.reason?.startsWith('copies differ on ') ? i.reason.slice(17).split(', ') : []);
```

```svelte
  {#if visible.length > 0 || hiddenCount > 0}
    <div class="group-header">On hosts, not in catalog <span class="count">{visible.length}</span></div>
    {#each visible as i (`${i.kind}:${i.name}`)}
      <div class="row unmanaged" data-testid={`identity-row-${i.kind}-${i.name}`}>
        <span class="name">{i.name}</span>
        <span class="meta">{i.kind}</span>
        {#if i.class === 'needs_person'}<span class="badge warn" title={i.reason ?? ''}>{i.reason}</span>{/if}
        <HostStrip {order} present={[...new Set(i.hosts.map((h) => h.host_alias))]} odd={oddHosts(i)} />
        {#if !readonly && !internal(i)}
          <button class="link" onclick={() => onimport(i)} title="Import this asset">Import</button>
        {/if}
      </div>
    {/each}
    {#if hiddenCount > 0}
      <button class="link toggle" onclick={() => (showInternals = !showInternals)}>
        {showInternals ? 'Hide' : 'Show'} {hiddenCount} fleet internal{hiddenCount === 1 ? '' : 's'}
      </button>
    {/if}
  {/if}
  {#each orphans as r (`${r.host_alias}:${r.harness}:${r.kind}:${r.name}`)}
    <div class="row unmanaged" data-testid={`unmanaged-row-${r.host_alias}-${r.harness}-${r.kind}-${r.name}`}>
      <span class="name">{r.name}</span>
      <span class="meta">{r.kind} · {r.host_alias}</span>
      <span class="badge orphan" data-testid={`orphan-badge-${r.host_alias}-${r.harness}-${r.kind}-${r.name}`}>orphan</span>
    </div>
  {/each}
```

Change the prop type to `onimport: (identity: AssetIdentity) => void`, import `HostStrip`, `identitiesOf`, `hostOrder`, `type AssetIdentity`. Replace the hard-coded `#16a34a` / `#d97706` in this file's styles with `var(--usage-ok)` / `var(--usage-warn)` and add `.badge.warn { color: var(--usage-warn); }`.

`AssetsPanel.svelte` — the handler now receives the identity and opens the import dialog preset to it (Task 7 wires the dialog's `host` and `only`):

```ts
  let importPreset = $state<{ host: string; only: string[] } | null>(null);
  function onImportUnmanaged(i: AssetIdentity) {
    const host = i.hosts.find((h) => h.host_alias === 'local')?.host_alias ?? i.hosts[0]?.host_alias ?? 'local';
    importPreset = { host, only: [`${i.kind}:${i.name}`] };
    showImport = true;
  }
```

The toolbar's "Import from host" button (line ~364) sets `importPreset = null` before `showImport = true`, and the dialog's `onclose` / `ondone` reset it to `null`. Task 6 adds the dialog props this preset feeds: `<ImportDialog host={importPreset?.host ?? 'local'} only={importPreset?.only ?? []} … />`.

- [ ] **Step 4: Run to verify pass**

Run: `pnpm test src/lib && pnpm check`
Expected: PASS, no type errors.

- [ ] **Step 5: Commit**

```bash
git add src/lib/assets.ts src/lib/HostStrip.svelte src/lib/AssetList.svelte src/lib/AssetsPanel.svelte src/lib/*.test.ts
git commit -m "feat(assets): one row per asset with a host strip; fleet internals folded away"
```

---

### Task 5: Scan automatically

**Files:**
- Create: `crates/fleet-core/src/service/catalog/scan_tick.rs`
- Modify: `crates/fleet-core/src/service/catalog/mod.rs` (`pub mod scan_tick;`)
- Modify: `crates/fleet-core/src/service/settings.rs` (constants + two `Spec::new` rows beside the work ones near line 902)
- Modify: `crates/fleet-core/pages/settings.automation.json` (new "Assets" section)
- Modify: `src-tauri/src/backend/startup.rs` (trait method + Local branch), `src-tauri/src/bootstrap/tasks.rs`, `src-tauri/src/backend/tests_startup.rs` (recorder)
- Modify: `crates/fleet-hub/src/serve.rs:1041` (spawn beside the tracker tick)

**Interfaces:**
- Consumes: `Store::inventory_last_scans` (Task 2), `inventory::scan_hosts`, `Store::last_sync_run`, `super::CATALOG`
- Produces:

```rust
pub const CATALOG_SCAN_CHECK_SECS: &str = "catalog.scan_check_secs";   // settings.rs, default "3600", 0 = off, min 300
pub const CATALOG_SCAN_MAX_AGE_SECS: &str = "catalog.scan_max_age_secs"; // settings.rs, default "86400"

pub struct HostDue { pub alias: String, pub reachable: bool, pub hidden: bool, pub last_scan: Option<i64> }
pub fn hosts_due(now: i64, hosts: &[HostDue], max_age: i64, everything_changed: bool) -> Vec<String>;
pub fn spawn_catalog_scan_tick(store: Arc<Mutex<Store>>, ssh: Arc<SshClient>, token: CancellationToken) -> Option<JoinHandle<()>>;
```

`everything_changed` is true when the catalog HEAD or the last sync run's `finished_at` differs from what the previous pass saw (the first pass counts as changed). A host is due when it is not hidden, is reachable (or is `local`), and either `everything_changed`, or it has no scan, or its last scan is older than `max_age`. A host that comes back online is therefore picked up at the next check once its scan is stale; that is the "on reconnect" trigger.

- [ ] **Step 1: Write the failing tests** (`scan_tick.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    fn h(alias: &str, reachable: bool, last: Option<i64>) -> HostDue {
        HostDue { alias: alias.into(), reachable, hidden: false, last_scan: last }
    }

    #[test]
    fn stale_or_never_scanned_reachable_hosts_are_due() {
        let hosts = vec![h("fresh", true, Some(990)), h("stale", true, Some(100)), h("never", true, None), h("down", false, None)];
        assert_eq!(hosts_due(1000, &hosts, 500, false), vec!["stale".to_string(), "never".to_string()]);
    }

    #[test]
    fn a_catalog_or_sync_change_makes_every_reachable_host_due() {
        let hosts = vec![h("fresh", true, Some(990)), h("down", false, Some(990))];
        assert_eq!(hosts_due(1000, &hosts, 500, true), vec!["fresh".to_string()]);
    }

    #[test]
    fn local_is_due_even_when_its_reachable_flag_is_false_and_hidden_never_is() {
        let mut hidden = h("gone", true, None);
        hidden.hidden = true;
        let hosts = vec![h("local", false, None), hidden];
        assert_eq!(hosts_due(1000, &hosts, 500, false), vec!["local".to_string()]);
    }
}
```

In `src-tauri/src/backend/tests_startup.rs`, extend the recorder with `start_catalog_scan_tick` and assert it is started for `Backend::Local` and not for `Remote` / `Unavailable`, next to the existing `start_tracker_sync` assertions.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p fleet-core catalog::scan_tick && cargo test -p claude-fleet --lib startup`
Expected: FAIL — module and trait method missing.

- [ ] **Step 3: Implement**

`scan_tick.rs`:

```rust
//! Assets S1a: rescan hosts without anyone pressing Scan. Hourly by
//! default, it rescans every reachable host whose inventory is older than a
//! day, and all of them after the catalog HEAD or a sync changed.

use crate::ssh::SshClient;
use crate::store::Store;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub struct HostDue { pub alias: String, pub reachable: bool, pub hidden: bool, pub last_scan: Option<i64> }

pub fn hosts_due(now: i64, hosts: &[HostDue], max_age: i64, everything_changed: bool) -> Vec<String> {
    hosts
        .iter()
        .filter(|h| !h.hidden && (h.reachable || h.alias == "local"))
        .filter(|h| everything_changed || h.last_scan.is_none_or(|t| now - t > max_age))
        .map(|h| h.alias.clone())
        .collect()
}

fn setting_secs(store: &Mutex<Store>, key: &str) -> i64 {
    let raw = store.lock().ok().and_then(|s| s.get_setting(key).ok().flatten());
    crate::service::settings::resolve(key, raw.as_deref()).parse::<i64>().unwrap_or(0)
}

pub fn spawn_catalog_scan_tick(
    store: Arc<Mutex<Store>>,
    ssh: Arc<SshClient>,
    token: CancellationToken,
) -> Option<tokio::task::JoinHandle<()>> {
    use crate::service::settings::{CATALOG_SCAN_CHECK_SECS, CATALOG_SCAN_MAX_AGE_SECS};
    let check = setting_secs(&store, CATALOG_SCAN_CHECK_SECS);
    if check <= 0 {
        tracing::info!("catalog scan tick disabled (catalog.scan_check_secs=0)");
        return None;
    }
    let period = Duration::from_secs(check.max(300) as u64);
    Some(crate::rt::spawn(async move {
        let mut ticker = tokio::time::interval(period);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut seen: Option<(String, Option<i64>)> = None;
        loop {
            tokio::select! {
                biased;
                _ = token.cancelled() => break,
                _ = ticker.tick() => {}
            }
            let head = match super::CATALOG.read() {
                Ok(g) => match g.as_ref() { Some(c) => c.head.clone(), None => continue },
                Err(_) => continue,
            };
            // All store reads in one scoped guard, dropped before any await.
            let (hosts, last_sync) = {
                let Ok(s) = store.lock() else { continue };
                let Ok(list) = s.list_hosts() else { continue };
                let last = s.inventory_last_scans().unwrap_or_default();
                let sync = s.last_sync_run().ok().flatten().map(|r| r.finished_at);
                let hosts: Vec<HostDue> = list
                    .into_iter()
                    .map(|h| HostDue { last_scan: last.get(&h.alias).copied(), alias: h.alias, reachable: h.reachable, hidden: h.hidden })
                    .collect();
                (hosts, sync)
            };
            let now_key = (head, last_sync);
            let changed = seen.as_ref() != Some(&now_key);
            let due = hosts_due(super::now_secs(), &hosts, setting_secs(&store, CATALOG_SCAN_MAX_AGE_SECS), changed);
            for alias in due {
                if let Err(e) = super::inventory::scan_hosts(&store, &ssh, Some(&alias)).await {
                    tracing::warn!(host = %alias, "catalog scan tick: {}", e.message);
                }
            }
            seen = Some(now_key);
        }
    }))
}
```

(`finished_at` is the field name on `SyncRunRow`; use whatever `last_sync_run` returns. `is_none_or` needs Rust 1.82+; if the toolchain is older, write `h.last_scan.map_or(true, |t| now - t > max_age)`.)

`settings.rs` — constants beside the other groups, and in `SPECS`:

```rust
/// Assets S1a: how often the catalog scan tick checks for stale hosts.
pub const CATALOG_SCAN_CHECK_SECS: &str = "catalog.scan_check_secs";
/// A host whose newest inventory row is older than this is rescanned.
pub const CATALOG_SCAN_MAX_AGE_SECS: &str = "catalog.scan_max_age_secs";
```

```rust
    Spec::new(
        CATALOG_SCAN_CHECK_SECS,
        "3600",
        Kind::Secs,
        "Asset scan check",
        "How often fleet looks for hosts whose asset scan is stale, and rescans them. Under five minutes is raised to five.",
    )
    .unit(Unit::Minutes)
    .zero("off")
    .restart(Restart::App),
    Spec::new(
        CATALOG_SCAN_MAX_AGE_SECS,
        "86400",
        Kind::Secs,
        "Asset scan age",
        "A host's assets are rescanned once its last scan is older than this, and every host after the catalog or a sync changes.",
    )
    .unit(Unit::Hours)
    .tags(&[Tag::Advanced]),
```

(`every_spec_has_consistent_metadata` checks the metadata; run it with the other settings tests.)

`pages/settings.automation.json` — add a section:

```json
    {
      "title": "Assets",
      "items": [
        { "type": "field", "key": "catalog.scan_check_secs" },
        { "type": "field", "key": "catalog.scan_max_age_secs" }
      ]
    }
```

`startup.rs` — add to `FleetTasks`:

```rust
    /// Assets S1a: rescan stale hosts' assets. Fleet-owning, like the
    /// tracker sync: a paired desktop must not run it.
    fn start_catalog_scan_tick(&self);
```

and call `tasks.start_catalog_scan_tick();` in the `Backend::Local` arm after `start_tracker_sync()`. `bootstrap/tasks.rs`:

```rust
    fn start_catalog_scan_tick(&self) {
        std::mem::drop(fleet_core::service::catalog::scan_tick::spawn_catalog_scan_tick(
            Arc::clone(&self.store),
            Arc::clone(&self.ssh),
            tokio_util::sync::CancellationToken::new(),
        ));
    }
```

`fleet-hub/src/serve.rs`, after `tracker_handle`:

```rust
    let catalog_scan_handle = fleet_core::service::catalog::scan_tick::spawn_catalog_scan_tick(
        Arc::clone(&store),
        Arc::clone(&ssh),
        ticks_cancel.clone(),
    );
```

and await/abort it wherever `tracker_handle` is handled at shutdown.

- [ ] **Step 4: Regenerate docs and run**

```bash
REGEN_SETTINGS_DOCS=1 cargo test -p fleet-core settings_docs_are_current
REGEN_PAGE_DOCS=1 cargo test -p fleet-core page_docs_are_current
cargo test -p fleet-core catalog::scan_tick settings pages
cargo test -p claude-fleet --lib startup
cargo build -p fleet-hub
```

Expected: PASS; `docs/settings-reference.md`, `docs/page-catalog.json`, `src/lib/pages/registry.generated.json` updated.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/service/catalog/scan_tick.rs crates/fleet-core/src/service/catalog/mod.rs crates/fleet-core/src/service/settings.rs crates/fleet-core/pages/settings.automation.json src-tauri/src crates/fleet-hub/src/serve.rs docs src/lib/pages/registry.generated.json
git commit -m "feat(catalog): a scan tick that rescans stale hosts and every host after a catalog or sync change"
```

---

### Task 6: Import from any host, and only what was asked

**Files:**
- Modify: `crates/fleet-core/Cargo.toml` (move `tempfile = "3"` into `[dependencies]`)
- Modify: `crates/fleet-core/src/service/catalog/import.rs`
- Modify: `crates/fleet-core/src/service/catalog/mod.rs:287-310` (`ImportArgs`, `import_host`)
- Modify: `crates/fleet-core/src/service/catalog/admin.rs` (`AdminCall::ImportHost`)
- Modify: `crates/fleet-core/src/mcp/tools/assets.rs:36-61`
- Modify: `src-tauri/src/commands/assets.rs:146-157` (+ `routed::catalog_import_host`)
- Modify: `src-tauri/src/backend/verdicts.rs:1090`
- Modify: `src/lib/assets.ts:128`, `src/lib/ImportDialog.svelte:29`, `src/lib/AssetsPanel.svelte`
- Modify: `docs/hub.md` (Asset catalog section: import works from any host)

**Interfaces:**
- Consumes: the identity preset from Task 4
- Produces:
  - `ImportArgs { host_alias: String, dry_run: bool, #[serde(default)] only: Vec<String> }` — `only` holds `"<kind>:<name>"` keys; empty = everything
  - `pub async fn import_host(args: ImportArgs, store: &Mutex<Store>, ssh: &Arc<SshClient>, fleet_token: Option<&str>) -> Result<ImportReport, IpcError>`
  - `pub fn import_claude_only(src: &ImportSources, repo_root: &Path, host: &str, fleet_token: Option<&str>, dry_run: bool, only: &[String]) -> Result<ImportReport, IpcError>` (`import_claude` becomes a wrapper passing `&[]`)
  - `pub const REMOTE_SOURCES_SCRIPT: &str`, `pub fn parse_remote_dump(stdout: &str, root: &Path) -> Result<ImportSources, IpcError>`
  - `AdminCall::ImportHost(ImportArgs)` with wire name `"import_host"`
  - TS: `importHost(hostAlias: string, dryRun: boolean, only: string[] = [])`

The remote script prints one record per file, the same framing idea as the harness scan:

```
##FILE <home-relative path>
<base64 of the file, one line>
```

It covers `~/.claude/skills/**` and `~/.claude/agents/*.md` (following symlinks, files under 1 MiB), and the four config files `~/.claude/settings.json`, `~/.claude.json`, `~/.claude/plugins/installed_plugins.json`, `~/.claude/plugins/known_marketplaces.json`. The parser rejects any path that is not under `.claude/` or is exactly `.claude.json`, and any path containing `..`.

- [ ] **Step 1: Write the failing tests** (`import.rs` tests)

```rust
#[test]
fn parse_remote_dump_rebuilds_a_home_tree() {
    use base64::Engine;
    let b = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
    let out = format!(
        "##FILE .claude/skills/w/SKILL.md\n{}\n##FILE .claude.json\n{}\n",
        b("---\nname: w\ndescription: Make a worktree.\n---\nbody\n"),
        b(r#"{"mcpServers":{}}"#),
    );
    let dir = tempfile::tempdir().unwrap();
    let src = parse_remote_dump(&out, dir.path()).unwrap();
    assert!(src.claude_dir.join("skills/w/SKILL.md").is_file());
    assert_eq!(std::fs::read_to_string(&src.claude_json).unwrap(), r#"{"mcpServers":{}}"#);
}

#[test]
fn parse_remote_dump_refuses_paths_outside_claude() {
    for bad in ["../etc/passwd", ".claude/../x", ".ssh/id_ed25519", "/abs"] {
        let out = format!("##FILE {bad}\nAAAA\n");
        let dir = tempfile::tempdir().unwrap();
        assert!(parse_remote_dump(&out, dir.path()).is_err(), "{bad}");
    }
}

#[cfg(unix)]
#[test]
fn only_imports_the_named_assets() {
    let (src, repo) = fixture("only");
    let rep = import_claude_only(&src, &repo, "oci", Some("SECRET123"), true, &["skill:worktree".to_string()]).unwrap();
    assert_eq!(rep.created, vec![("skill".to_string(), "worktree".to_string())]);
}

#[test]
fn remote_script_quotes_nothing_and_frames_files() {
    assert!(REMOTE_SOURCES_SCRIPT.contains("##FILE"));
    assert!(!REMOTE_SOURCES_SCRIPT.contains('\''), "passed through shell::quote whole");
}
```

In `admin.rs` tests, add `AdminCall::ImportHost(ImportArgs { host_alias: "oci".into(), dry_run: true, only: vec![] })` to whichever exhaustive list of calls the existing tests build (they enumerate every action).

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p fleet-core catalog::import catalog::admin`
Expected: FAIL — functions and variant missing.

- [ ] **Step 3: Implement**

`import.rs`:

```rust
/// Printed on the host by `import_host` for a remote alias: every file the
/// importer reads, framed as `##FILE <home-relative path>` + one base64 line.
/// No single quotes: the whole script is one `shell::quote`d word.
pub const REMOTE_SOURCES_SCRIPT: &str = r#"cd "$HOME" || exit 1
emit() { printf "##FILE %s\n" "$1"; base64 < "$1" | tr -d "\n"; printf "\n"; }
for f in .claude/settings.json .claude.json .claude/plugins/installed_plugins.json .claude/plugins/known_marketplaces.json; do
  [ -f "$f" ] && emit "$f"
done
for d in .claude/skills .claude/agents; do
  [ -d "$d" ] || continue
  find -L "$d" -type f -size -1024k 2>/dev/null | while IFS= read -r f; do emit "$f"; done
done
"#;

/// Rebuild the files `REMOTE_SOURCES_SCRIPT` printed under `root`, and
/// point `ImportSources` at them. Refuses anything outside `.claude/` and
/// `.claude.json`.
pub fn parse_remote_dump(stdout: &str, root: &Path) -> Result<ImportSources, IpcError> {
    use base64::Engine;
    let bad = |p: &str| IpcError::new(crate::ipc_error::codes::E_INVALID, format!("remote import: refusing path {p}"));
    let mut lines = stdout.lines();
    while let Some(line) = lines.next() {
        let Some(path) = line.strip_prefix("##FILE ") else { continue };
        let ok = (path == ".claude.json" || path.starts_with(".claude/"))
            && !path.split('/').any(|seg| seg == ".." || seg.is_empty());
        if !ok {
            return Err(bad(path));
        }
        let data = base64::engine::general_purpose::STANDARD
            .decode(lines.next().unwrap_or("").trim())
            .map_err(|e| IpcError::new(crate::ipc_error::codes::E_INVALID, format!("remote import: {path}: {e}")))?;
        let dest = root.join(path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&dest, data)?;
    }
    Ok(ImportSources { claude_dir: root.join(".claude"), claude_json: root.join(".claude.json") })
}
```

Rename the body of `import_claude` to `import_claude_only(…, only: &[String])`, and in its final `for a in assets` loop skip assets not asked for:

```rust
        if !only.is_empty() && !only.iter().any(|k| *k == format!("{}:{}", a.kind().as_str(), a.header.name)) {
            continue;
        }
```

Keep `pub fn import_claude(src, repo_root, host, fleet_token, dry_run)` as `import_claude_only(src, repo_root, host, fleet_token, dry_run, &[])`.

`mod.rs`:

```rust
#[derive(Debug, Clone, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "ImportAssetsParams")]
pub struct ImportArgs {
    /// Any host alias; `local` reads this machine's config.
    pub host_alias: String,
    /// Report, write nothing.
    #[serde(default)]
    pub dry_run: bool,
    /// Only these `<kind>:<name>` assets; empty imports everything.
    #[serde(default)]
    pub only: Vec<String>,
}

/// Import from a host's Claude config. `local` reads this machine's files;
/// any other host is copied over SSH into a temporary directory first.
pub async fn import_host(
    args: ImportArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    fleet_token: Option<&str>,
) -> Result<import::ImportReport, IpcError> {
    let cfg = require_config(store)?;
    let repo = std::path::PathBuf::from(&cfg.repo_path);
    if args.host_alias == "local" {
        crate::service::hub::ensure_local_allowed(&args.host_alias)?;
        let src = import::ImportSources::for_local()?;
        return import::import_claude_only(&src, &repo, "local", fleet_token, args.dry_run, &args.only);
    }
    let script = import::REMOTE_SOURCES_SCRIPT;
    let out = inventory::run_host_script(ssh, &args.host_alias, script).await?;
    let tmp = tempfile::tempdir()?;
    let src = import::parse_remote_dump(&out, tmp.path())?;
    import::import_claude_only(&src, &repo, &args.host_alias, fleet_token, args.dry_run, &args.only)
}
```

(`run_host_script` already wraps the script as one quoted `bash -lc` word. `tmp` is removed when it drops. Add `use std::sync::Arc; use crate::ssh::SshClient;` if missing. If `IpcError` has no `From<std::io::Error>`, map with `.map_err(|e| IpcError::new(codes::E_IO, e.to_string()))`.)

`admin.rs` — add `"import_host" => ImportHost(ImportArgs),` to `admin_calls!`, and in `run`:

```rust
        AdminCall::ImportHost(a) => {
            let token = lock(store)?.get_setting(crate::mcp::SETTING_TOKEN)?;
            json(super::import_host(a, store, ssh, token.as_deref()).await?)
        }
```

MCP `import_assets`: pass `&self.ssh`, `.await` the call, and update the description's last sentence from "Only host_alias `local`." to "Any host: `local` reads this machine, others are read over SSH. `only` limits it to `<kind>:<name>` assets." Then `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`.

`src-tauri/src/commands/assets.rs` — make the command async and routed like `catalog_create_asset`:

```rust
#[tauri::command]
pub async fn catalog_import_host(
    backend: State<'_, Arc<FleetBackend>>,
    args: ImportArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<ImportReport, IpcError> {
    routed::catalog_import_host(&backend, args, &store, &ssh).await
}
```

```rust
    pub async fn catalog_import_host(
        backend: &FleetBackend,
        args: ImportArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<ImportReport, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("catalog_import_host", &AdminCall::ImportHost(args)).await,
            None => {
                let token = lock(store)?.get_setting(fleet_core::mcp::SETTING_TOKEN)?;
                catalog::import_host(args, store, ssh, token.as_deref()).await
            }
        }
    }
```

Remove the `refuse_local_only` call and update the module doc comment that says import still refuses. In `verdicts.rs`, change the `catalog_import_host` row to `Verdict::Routed { tool: "catalog_admin" }`, then `REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen`.

Frontend — `assets.ts`:

```ts
export function importHost(hostAlias: string, dryRun: boolean, only: string[] = []): Promise<Result<ImportReport>> {
  return invokeCmd<ImportReport>('catalog_import_host', { args: { host_alias: hostAlias, dry_run: dryRun, only } });
}
```

`ImportDialog.svelte:29` — drop the `disabled` and the "(local only in this version)" suffix; accept `host` and `only` props (defaults `'local'`, `[]`) from `AssetsPanel`'s `importPreset`, show "Only: kind:name" when `only` is non-empty, and pass `only` to `importHost`. Update `assets.test.ts`'s `importHost` expectation to include `only: []`.

`docs/hub.md` — in "Asset catalog", replace the sentence implying import needs the hub's own `~/.claude` with: "Import reads any host over SSH (`import_assets { host_alias }`), so a hub imports from the machines it manages."

- [ ] **Step 4: Run to verify pass**

```bash
cargo test -p fleet-core catalog
cargo test -p claude-fleet --lib
pnpm test src/lib && pnpm check
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A crates/fleet-core src-tauri src/lib docs
git commit -m "feat(catalog): import from any host over SSH, optionally only the assets asked for"
```

---

### Task 7: Refuse to sync the whole catalog onto an unlayered remote host

**Files:**
- Modify: `crates/fleet-core/src/service/catalog/sync/mod.rs:45-49, 196-240`
- Modify: `crates/fleet-core/src/mcp/tools/params.rs:871` and the `plan_sync` tool in `mcp/tools/assets.rs:63-95`
- Modify: any `PlanArgs { … }` literal that does not already end in `..PlanArgs::default()`
- Modify: `src/lib/assets.ts` (`planSync` filter), `src/lib/SyncPlanDialog.svelte` (show the skipped reason, add a confirm for unlayered)

**Interfaces:**
- Produces: `PlanArgs.allow_unlayered: bool` (`#[serde(default)]`), `PlanSyncParams.allow_unlayered: Option<bool>`, `pub const UNLAYERED_DETAIL: &str` and `pub(crate) fn refuse_unlayered(alias: &str, layered: bool, allow: bool, catalog_empty: bool) -> bool` in `sync/mod.rs`

Rule: for a host other than `local`, when `resolved.layered` is false, the resolved catalog is non-empty and `allow_unlayered` is false, the host gets `skipped_plan(alias, harness, UNLAYERED_DETAIL)` for every scanning harness and is not scanned. `local` is exempt: a single-machine user syncs its own catalog back to itself.

- [ ] **Step 1: Write the failing tests** (in `sync/mod.rs` tests)

The decision is a pure function, so the "allowed" and "local" cases need no SSH. One integration test proves `plan_sync` uses it; it never reaches SSH because the refused host is not scanned.

```rust
#[test]
fn refuse_unlayered_only_for_remote_unlayered_non_empty_unless_allowed() {
    assert!(refuse_unlayered("oci", false, false, false));
    assert!(!refuse_unlayered("oci", false, true, false), "allowed");
    assert!(!refuse_unlayered("oci", true, false, false), "layered");
    assert!(!refuse_unlayered("oci", false, false, true), "empty catalog");
    assert!(!refuse_unlayered("local", false, false, false), "local is exempt");
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn plan_sync_skips_an_unlayered_remote_host() {
    let _lock = super::super::CATALOG_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let repo_dir = tempfile::tempdir().unwrap();
    let files = one_skill("b\n");
    load_catalog(repo_dir.path(), &files.iter().map(|(a, b)| (*a, b.as_str())).collect::<Vec<_>>());
    let store = store_with_local(Arc::new(RecordingEventBus::new()));
    {
        let s = store.lock().unwrap();
        s.insert_host("oci", Some("oci")).unwrap();
        s.update_host_probe("oci", true, None, None, 1).unwrap();
    }
    let ssh = Arc::new(SshClient::new());
    let plan = plan_sync(PlanArgs { host_alias: Some("oci".into()), ..PlanArgs::default() }, &store, &ssh)
        .await
        .unwrap();
    assert!(!plan.hosts.is_empty());
    assert!(
        plan.hosts.iter().all(|h| h.status == "skipped" && h.detail.as_deref() == Some(UNLAYERED_DETAIL)),
        "{:?}",
        plan.hosts
    );
}
```

(`one_skill`, `load_catalog`, `store_with_local`, `RecordingEventBus` and `CATALOG_TEST_LOCK` are the helpers the existing `plan_sync_*` tests in this module already use; `PlanArgs` already derives `Default`.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p fleet-core catalog::sync`
Expected: compile error on `allow_unlayered` / `UNLAYERED_DETAIL`.

- [ ] **Step 3: Implement**

```rust
pub struct PlanArgs {
    pub host_alias: Option<String>,
    pub kind: Option<super::model::Kind>,
    pub name: Option<String>,
    /// Plan a remote host that has no layers assigned. Without it such a
    /// host is skipped: with no layers it would receive the whole catalog.
    #[serde(default)]
    pub allow_unlayered: bool,
}

pub const UNLAYERED_DETAIL: &str = "no layers assigned: syncing would install the whole catalog here. \
    Assign a role first (set_host_layers), or plan with allow_unlayered.";
```

```rust
/// A remote host with no layers would receive the whole catalog. `local` is
/// exempt: a single-machine user syncs its own catalog back to itself.
pub(crate) fn refuse_unlayered(alias: &str, layered: bool, allow: bool, catalog_empty: bool) -> bool {
    alias != "local" && !layered && !allow && !catalog_empty
}
```

In `plan_sync`, right after `resolved` is computed and before the per-harness loop:

```rust
        if refuse_unlayered(&h.alias, resolved.layered, args.allow_unlayered, resolved.catalog.assets.is_empty()) {
            for harness in &scanning {
                host_plans.push(skipped_plan(&h.alias, harness.id(), UNLAYERED_DETAIL));
            }
            continue;
        }
```

`PlanSyncParams`: add

```rust
    /// Plan remote hosts that have no layers (they would get the whole
    /// catalog). Off by default.
    #[serde(default)]
    pub allow_unlayered: Option<bool>,
```

and pass `allow_unlayered: p.allow_unlayered.unwrap_or(false)` where the tool builds `PlanArgs`. Add `allow_unlayered: false` to every other `PlanArgs` literal the compiler lists; for existing tests that plan a **remote unlayered** host and expect actions, set it to `true` (they test planning, not the guard). Regenerate the reference: `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`.

Frontend: in `SyncPlanDialog.svelte`, a host plan whose `detail` starts with `no layers assigned` renders the detail as a warning line with a quiet "Plan anyway" button that re-runs `planSync({ …filter, allowUnlayered: true })`; `planSync` in `assets.ts` sends `allow_unlayered: filter.allowUnlayered ?? false`. Add a Vitest case to `assets.test.ts` asserting the argument is sent (`false` by default, `true` when given).

- [ ] **Step 4: Run to verify pass**

```bash
cargo test -p fleet-core catalog
pnpm test src/lib && pnpm check
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A crates/fleet-core src/lib docs
git commit -m "feat(sync): an unlayered remote host is skipped instead of getting the whole catalog"
```

---

### Task 8: Whole-workspace verification

**Files:** none new.

- [ ] **Step 1: Run every suite**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pnpm test
pnpm check
```

Expected: all pass. Fix anything they report in the task that introduced it, with its own commit.

- [ ] **Step 2: Check generated files are current**

```bash
REGEN_DOCS=1 cargo test -p fleet-core reference_is_current
REGEN_SETTINGS_DOCS=1 cargo test -p fleet-core settings_docs_are_current
REGEN_PAGE_DOCS=1 cargo test -p fleet-core page_docs_are_current
REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen
git status --short
```

Expected: no changes after regeneration (everything was committed in its task).

- [ ] **Step 3: Update CLAUDE.md**

Add one short paragraph to the catalog notes in `CLAUDE.md`: unmanaged rows carry `host_hash` / `secret_like` / `fleet_owned` (086); `list_assets.identities` groups them (`service/catalog/identity.rs`); the scan tick (`scan_tick.rs`, `catalog.scan_*`); import reads any host (`REMOTE_SOURCES_SCRIPT`); `plan_sync` skips unlayered remote hosts unless `allow_unlayered`. Point at this plan and the spec.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: CLAUDE.md notes for the assets S1a foundation"
```

---

## Self-review against the spec

| Spec S1 item | Task |
|---|---|
| persist `host_hash` on unmanaged rows | 1, 2 |
| group by identity in `list_assets` and the list | 3, 4 |
| rule classifier (infra, `.`-names, variants, secrets) | 1 (flags), 3 (rules), 4 (folded in UI) |
| scan tick + reconnect / HEAD / after-sync triggers | 5 (staleness covers reconnect; HEAD and last sync force a full pass) |
| import from any host | 6 |
| guard on Sync when no layers exist | 7 |
| multiple catalogs, scopes, scope boundary | **S1b — separate plan** |
