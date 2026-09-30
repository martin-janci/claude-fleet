# F3a + F3b — per-host harness set and Codex subagents Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The asset catalog plans and inventories Codex only on hosts that have it (auto-detected, overridable per host, retiring cleanly when turned off), and renders catalog agents as Codex subagents in `~/.codex/agents/<install name>.toml`.

**Architecture:** A new nullable `hosts.harnesses` column (JSON array, `NULL` = auto) holds the per-host choice. The Codex scan — which already runs on every reachable host — prints a `##PRESENT` line when the `codex` CLI is on PATH or `~/.codex` exists, carried as `HostSnapshot::present`. ONE pure function, `harness_set::harness_gate(id, configured, facts) -> HarnessGate { Off | On | Retiring }`, turns the choice plus the scan's facts (detected, managed-by-manifest) into a gate; `plan_sync`, the post-apply rescan and `scan_hosts` read it through `gated_catalog`, which yields the host's catalog (`On`), an empty catalog (`Retiring` — only removals of what fleet installed), or nothing (`Off`). The override is `set_host_harnesses` (service fn, MCP tool, `catalog_admin` action, Tauri command, Host detail control). F3b adds a Codex render arm for agents that builds a `toml::Table` and serialises it with the `toml` crate, plus scan/installed support for `~/.codex/agents/*.toml`.

**Tech Stack:** Rust (fleet-core, src-tauri), rusqlite migrations, `toml` 0.8, Svelte 5 + Vitest.

**Spec:** `docs/superpowers/specs/2026-09-29-multi-harness-agents-design.md` — §5.2 items 2 (Codex agent render) and 3 (per-host harness set), roadmap **F3**. This plan is **F3a + F3b** only; F3c/F3d (Instructions kind, shared `~/.agents/skills`, provision-on-catalog, Codex hooks) are separate plans.

## Global Constraints

- **Build environment (this machine):** every cargo command runs with `export CARGO_TARGET_DIR=<shared-target-dir>`. A fleet-core rebuild takes 30–60 min on this shared, overloaded machine, so each task makes ALL its edits first and runs only its targeted test filters (libtest takes several filters, OR-ed); one whole-crate run happens in Task 7.
- **Known pre-existing failure:** `service::rewind::tests::the_removal_script_leaves_a_tree_a_live_pane_is_in` fails in deep scratch directories (unix socket path too long). It is not ours; do not fix it, report it.
- **User decisions (2026-09-30), binding:** Codex per host = auto-detect (codex CLI on PATH or `~/.codex` exists) + manual override per host; a host that already has a Codex manifest with entries stays served so its removals still run; Claude is always on. Codex agent TOML omits `model` unless the asset sets `targets.codex.model` (no tier → model mapping for Codex).
- **Parallel initiative S1b** (`docs/superpowers/specs/2026-09-29-assets-workspace-design.md`) will also change `plan_sync`'s host filtering. Keep the `sync/mod.rs` change small and composable: the gate decision lives ONLY in `harness_set::harness_gate`; `plan_sync` gains one `listed` line, four `&scanning` → `&listed` edits in the pre-scan skip loops, one argument to `scan_and_persist`, and the gated match arm.
- Every value interpolated into an SSH/bash string goes through `crate::shell::quote`; the scan scripts contain no single quotes (the caller wraps the whole script in `shell::quote`).
- Every child process via `fleet_core::proc::command` / `std_command`; no `eprintln!`/`println!`/`dbg!` in production code (use `tracing`).
- Never hold the `Store` mutex guard across an `.await`.
- New migration = `crates/fleet-core/migrations/NNN_<topic>.sql` + an entry in `store/schema.rs` `MIGRATIONS`; an `ADD COLUMN` needs an `already_applied` guard (as 087).
- Editing a `#[tool(description = …)]`, its params, or `CatalogAdminParams` → `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`.
- Adding a row to `src-tauri/src/backend/verdicts.rs` → `REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen`.
- A new key on a hub-served row type → `REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract` (the regen run fails once by design after rewriting the golden; the second run passes). Adding a key needs no `CONTRACT_REVISION` bump.
- Wire compatibility: every new serialized field is `#[serde(default)]`; the frontend treats it as optional (`harnesses?: string[] | null`), because a paired desktop may talk to an older hub.
- TOML is generated with the `toml` crate (`toml::Table` + `toml::to_string_pretty`), never by string concatenation.
- Every existing test assertion stays. The only existing tests this plan changes are named in their task, with the reason: `sync::tests::store_with_local` (helper pins Codex on — Task 3), `codex::tests::hooks_and_plugins_are_unsupported_agents_unless_render_as_skill` (agents now render — Task 6), `inventory::tests::compute_states_covers_all_five_states` (its Codex `unsupported` example moves from the agent to the hook — Task 6), and in `mcp::tools::tests` two additive lines plus the served-tool count (102 → 103) and the re-measured `BUDGET_BYTES` (Task 4).
- Frontend: `pnpm test`, `pnpm check`. Commits: Conventional Commits, no attribution lines. Work on branch `feat/catalog-harness-set`, never on `main`.

## Decisions this plan makes

1. **Detection chicken-and-egg.** The Codex scan keeps running on every reachable host, whatever the gate: it is the only source of both detection (`##PRESENT`) and the manifest facts (`managed`), and it is already run everywhere today, so F3a adds no SSH round-trip. What the gate controls is *planning* and *inventory*: an `Off` harness gets no `HostPlan` and no inventory rows (a stale row is cleared by persisting the empty list), so no Codex write is ever planned on a host without Codex. A separate presence probe was rejected: it would be a second script and a second round-trip for information the scan prints for free.
2. **Explicit off + manifest = `Retiring`.** The user decision "a host with a Codex manifest stays enabled" is honoured without overriding an explicit off: under auto, a managed host is `On`; with an explicit list that omits Codex, a managed host is `Retiring` — planned against an empty catalog, so its only actions are `Remove`s of what fleet installed (the plan carries `detail = retiring_detail("codex")`). Once those removals land the manifest is empty and the host reads `Off`.
3. **Claude cannot be removed in F3a.** `set_host_harnesses` refuses a list without `"claude"` (`E_INVALID`) and `harness_gate` returns `On` for Claude unconditionally. Fleet's own sessions, hooks, MCP entry and provisioned skills are Claude's; a Claude-less host is F5+ territory (Codex sessions). Accepting such a list would be a setting the engine silently ignores.
4. **Hosts skipped before any scan** (unreachable, secrets / layers error, unlayered refusal) are reported under Claude and the harnesses the host lists explicitly — the gate with `HarnessFacts::UNKNOWN`. Today they also report a Codex row; on an auto host that row carried no information (the Claude row states the same reason). A scan *failure* is still reported under every harness that was attempted.
5. **Codex subagent TOML:** `name` = catalog name, file = `install_name()` (the same split Claude's agent and both skill renderers use); `description`; `developer_instructions` = the prompt body; `model` only from `targets.codex.model`; `targets.codex.extra` keys merged in last (a JSON `null` has no TOML form → skipped with a render warning). `targets.codex.render_as: skill` still wins.
6. **`tools` on a Codex agent → a render warning, not a lint warning.** Codex subagents have no per-agent tool allowlist. A lint warning would fire on nearly every agent (the agent template itself has `tools`), so it goes in `RenderPlan.warnings`, which Asset detail's Codex preview tab already shows.
7. **Cross-kind Codex install-name lint: yes, an error.** An agent with `targets.codex.render_as: skill` lands in `~/.codex/skills/<install name>/`, where a skill of that install name also lands; both manifest entries would claim one path and removing either deletes the other's files — the same failure the existing same-kind rule is an error for.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/fleet-core/migrations/089_host_harnesses.sql` (new) | `hosts.harnesses TEXT` (NULL = auto) |
| `crates/fleet-core/src/store/schema.rs` | guard `hosts_has_harnesses`, `MIGRATIONS` entry 89, migration test |
| `crates/fleet-core/src/store/rows.rs` | `HostRow.harnesses`, `HOST_COLUMNS`, `map_host_row` |
| `crates/fleet-core/src/store/hosts_accounts.rs` | `Store::set_host_harnesses` + tests |
| six fleet-core `HostRow { … }` literals + `src-tauri/src/backend/tests_contract.rs` | `harnesses` field |
| `src-tauri/src/backend/hub_contract.golden.json` | regenerated: `HostRow` gains `harnesses` |
| `crates/fleet-core/src/service/catalog/harness/mod.rs` | `HostSnapshot::present`, `##PRESENT` in `parse_scan_blocks`, `HARNESS_IDS` doc |
| `crates/fleet-core/src/service/catalog/harness_set.rs` (new) | `HarnessGate`, `HarnessFacts`, `harness_gate`, `gated_catalog`, `retiring_detail`, `normalize_harnesses`, `set_host_harnesses` (service) |
| `crates/fleet-core/src/service/catalog/mod.rs` | `pub mod harness_set;` |
| `crates/fleet-core/src/service/catalog/harness/codex.rs` | presence probe, agent → TOML render, `.codex/agents` scan, `installed`/`installed_detail` for agents |
| `crates/fleet-core/src/service/catalog/sync/mod.rs` | gate in `scan_and_persist`, `plan_sync`, `rescan_after_apply`; tests |
| `crates/fleet-core/src/service/catalog/inventory.rs` | gate in `scan_hosts`; tests |
| `crates/fleet-core/src/service/catalog/sync/apply.rs` | Codex agent local e2e test |
| `crates/fleet-core/src/service/catalog/author.rs` | cross-kind Codex install-name lint |
| `crates/fleet-core/src/service/catalog/admin.rs` | `SetHostHarnessesArgs`, `AdminCall::SetHostHarnesses` |
| `crates/fleet-core/src/mcp/tools/{assets,params}.rs`, `mcp/guard.rs` | `set_host_harnesses` tool, params, policy row |
| `crates/fleet-core/src/mcp/tools/{tests,tests_catalog_admin}.rs` | tool tests, count, budget |
| `docs/control-api-reference.md` | regenerated |
| `src-tauri/src/commands/assets.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/backend/{verdicts,tests_routing}.rs` | `catalog_set_host_harnesses` command, routed to `catalog_admin` |
| `src/lib/hub_verdicts.generated.json` | regenerated |
| `src/lib/hosts.ts`, `src/lib/hub.ts`, `src/lib/HostDetail.svelte` (+ tests) | wire field, `setHostHarnesses`, Codex auto/on/off control |
| `docs/concepts.md`, `docs/control-api.md`, `docs/hub.md`, `docs/superpowers/specs/2026-09-14-asset-catalog-design.md`, `CLAUDE.md` | docs |

---

### Task 1: Store a per-host harness choice (migration 089)

**Files:**
- Create: `crates/fleet-core/migrations/089_host_harnesses.sql`
- Modify: `crates/fleet-core/src/store/schema.rs:396-405` (new guard after `asset_inventory_has_fleet_owned`), `:952-957` (MIGRATIONS entry after 88 — main took 088 for guides), tests module (new test after `migration_086_…`, ~line 3930)
- Modify: `crates/fleet-core/src/store/rows.rs:864-871` (field), `:920-924` (`HOST_COLUMNS`), `:955-957` (`map_host_row`)
- Modify: `crates/fleet-core/src/store/hosts_accounts.rs` (setter after `set_host_transport`, ~line 467; tests at the end of `mod tests`)
- Modify (add `harnesses: None,` after `provision_stale: …,`): `crates/fleet-core/src/service/hosts.rs:837`, `crates/fleet-core/src/service/onboarding.rs:186`, `crates/fleet-core/src/service/account_usage_poll.rs:270`, `crates/fleet-core/src/service/health.rs:1033`, `crates/fleet-core/src/service/account_usage.rs:1186`, `crates/fleet-core/src/store/reconcile.rs:1110`
- Modify: `src-tauri/src/backend/tests_contract.rs:160`; regenerate `src-tauri/src/backend/hub_contract.golden.json`

**Interfaces:**
- Produces:
  - `HostRow.harnesses: Option<Vec<String>>` (`#[serde(default)]`; `None` = auto)
  - `Store::set_host_harnesses(&self, alias: &str, harnesses: Option<&[String]>) -> Result<(), IpcError>` — stores the list as JSON as given (no validation), `E_NOTFOUND` for an unknown alias, emits `host:probed`

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/fleet-core/src/store/hosts_accounts.rs`:

```rust
    /// Multi-harness F3a: a new host is on auto (`None`); a list round-trips
    /// through its JSON column, `None` clears it again, every write emits the
    /// row, and an unknown alias is `E_NOTFOUND`.
    #[test]
    fn set_host_harnesses_round_trips_a_list_and_auto() {
        let bus = Arc::new(crate::events::RecordingEventBus::new());
        let s = Store::open_with_bus_in_memory(bus.clone()).unwrap();
        s.insert_host("h", Some("h")).unwrap();
        assert_eq!(
            s.get_host_row("h").unwrap().unwrap().harnesses,
            None,
            "a new host is on auto"
        );
        bus.take();
        let both = vec!["claude".to_string(), "codex".to_string()];
        s.set_host_harnesses("h", Some(both.as_slice())).unwrap();
        assert_eq!(s.get_host_row("h").unwrap().unwrap().harnesses, Some(both));
        assert!(bus.take().contains(&"host:probed:h".to_string()));
        s.set_host_harnesses("h", None).unwrap();
        assert_eq!(s.get_host_row("h").unwrap().unwrap().harnesses, None);
        let err = s.set_host_harnesses("nope", None).unwrap_err();
        assert_eq!(err.code, "E_NOTFOUND");
    }

    /// A stored value that is not a JSON string array (hand-edited, or from a
    /// future schema) reads as auto instead of failing every host read.
    #[test]
    fn an_unreadable_harnesses_value_reads_as_auto() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h", Some("h")).unwrap();
        s.conn
            .execute("UPDATE hosts SET harnesses='not json' WHERE alias='h'", [])
            .unwrap();
        assert_eq!(s.get_host_row("h").unwrap().unwrap().harnesses, None);
    }
```

Append to `mod tests` in `crates/fleet-core/src/store/schema.rs` (after `migration_086_adds_the_shared_work_columns_backfills_origin_and_is_safe_to_rerun`):

```rust
    #[test]
    fn migration_089_adds_hosts_harnesses_as_auto_and_is_safe_to_rerun() {
        let s = store_at_version(88);
        s.conn
            .execute_batch("INSERT INTO hosts (alias, reachable) VALUES ('h', 1);")
            .unwrap();
        assert!(!hosts_has_harnesses(&s.conn).unwrap());
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert!(hosts_has_harnesses(&s.conn).unwrap());
        let v: Option<String> = s
            .conn
            .query_row("SELECT harnesses FROM hosts WHERE alias = 'h'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(v, None, "an existing host starts on auto");
        s.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 89;")
            .unwrap();
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run:
```bash
export CARGO_TARGET_DIR=<shared-target-dir>
cargo test -p fleet-core --lib set_host_harnesses an_unreadable_harnesses migration_089 2>&1 | tail -20
```
Expected: compile errors — `no method named set_host_harnesses`, `no field harnesses on type HostRow`, `cannot find function hosts_has_harnesses`.

- [ ] **Step 3: Implement**

Create `crates/fleet-core/migrations/089_host_harnesses.sql`:

```sql
-- Multi-harness F3a: which harnesses the asset catalog syncs on a host.
-- NULL = auto: Claude always, Codex where the scan finds it (the codex CLI on
-- PATH or ~/.codex) or fleet already manages Codex assets there. A JSON array
-- (e.g. ["claude","codex"]) is an explicit choice made with
-- set_host_harnesses; it always contains "claude". ADD COLUMN is not
-- idempotent: guarded in schema.rs.
ALTER TABLE hosts ADD COLUMN harnesses TEXT;

INSERT OR IGNORE INTO schema_version (version) VALUES (89);
```

In `crates/fleet-core/src/store/schema.rs`, after `asset_inventory_has_fleet_owned` (line 405):

```rust
/// `already_applied` guard of migration 089 (`hosts.harnesses`,
/// multi-harness F3a).
fn hosts_has_harnesses(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('hosts') WHERE name = 'harnesses'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}
```

and in `MIGRATIONS`, after the version-88 entry (main's `088_guides.sql`):

```rust
    // Multi-harness F3a: which harnesses the asset catalog syncs on a host
    // (NULL = auto). ADD COLUMN is not idempotent: the same guard 087 uses.
    Migration {
        version: 89,
        sql: include_str!("../../migrations/089_host_harnesses.sql"),
        already_applied: Some(hosts_has_harnesses),
    },
```

In `crates/fleet-core/src/store/rows.rs`, add the field after `provision_stale` (line 870):

```rust
    /// Which harnesses the asset catalog syncs on this host (multi-harness
    /// F3a, migration 089). `None` = auto: Claude, plus Codex where a scan
    /// finds it or fleet already manages Codex assets there. `Some` = exactly
    /// these (always including `claude`). Per-field default: an older hub
    /// omits it.
    #[serde(default)]
    pub harnesses: Option<Vec<String>>,
```

Replace `HOST_COLUMNS` (lines 920-924):

```rust
pub(super) const HOST_COLUMNS: &str =
    "alias, ssh_alias, reachable, claude_version, tmux_version, hidden, \
     last_pinged_at, account_uuid, provisioned, transport, org_id, claude_version_at, \
     disk_home_free_kb, disk_home_total_kb, disk_tmp_free_kb, load_1m, mem_avail_kb, \
     uptime_secs, health_at, last_hook_at, agent_version, provisioned_at, provision_fingerprint, \
     harnesses";
```

In `map_host_row`, after the `provision_stale: …` field (line 956-957):

```rust
        // Migration 089. A value that is not a JSON string array reads as
        // auto rather than failing the whole row.
        harnesses: row
            .get::<_, Option<String>>(23)?
            .and_then(|t| serde_json::from_str::<Vec<String>>(&t).ok()),
```

In `crates/fleet-core/src/store/hosts_accounts.rs`, after `set_host_transport` (line 467):

```rust
    /// Set which harnesses the asset catalog syncs on a host (multi-harness
    /// F3a, migration 089): `None` = auto, `Some` = exactly this list, stored
    /// as JSON. The list is stored as given —
    /// `service::catalog::harness_set::set_host_harnesses` validates and
    /// normalises it first. An unknown alias is `E_NOTFOUND`, as in
    /// `set_host_transport`.
    pub fn set_host_harnesses(
        &self,
        alias: &str,
        harnesses: Option<&[String]>,
    ) -> Result<(), crate::ipc_error::IpcError> {
        let json = match harnesses {
            Some(list) => Some(serde_json::to_string(list).map_err(|e| {
                crate::ipc_error::IpcError::new(
                    codes::E_SERIALIZE,
                    format!("encode harnesses: {e}"),
                )
            })?),
            None => None,
        };
        let n = self.conn.execute(
            "UPDATE hosts SET harnesses=?1 WHERE alias=?2",
            rusqlite::params![json, alias],
        )?;
        if n == 0 {
            return Err(crate::ipc_error::IpcError::new(
                codes::E_NOTFOUND,
                format!("host {alias} not found"),
            ));
        }
        self.emit_host(alias, |bus, row| bus.host_probed(row))?;
        Ok(())
    }
```

Add `harnesses: None,` directly after the `provision_stale: …,` line of each `HostRow { … }` literal: `service/hosts.rs:837`, `service/onboarding.rs:186`, `service/account_usage_poll.rs:270`, `service/health.rs:1033`, `service/account_usage.rs:1186`, `store/reconcile.rs:1110`. (`store/reconcile.rs:261` spreads `..before` and needs nothing.)

In `src-tauri/src/backend/tests_contract.rs`, `sample_host()` (line 160), after `provision_stale: true,`:

```rust
        harnesses: Some(vec!["claude".into(), "codex".into()]),
```

- [ ] **Step 4: Run the tests to verify they pass, then regenerate the contract golden**

Run:
```bash
export CARGO_TARGET_DIR=<shared-target-dir>
cargo test -p fleet-core --lib set_host_harnesses an_unreadable_harnesses migration_089 every_migration_records_its_own_version host_row 2>&1 | tail -8
REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract 2>&1 | tail -5
cargo test -p claude-fleet --lib contract 2>&1 | tail -5
git diff --stat src-tauri/src/backend/hub_contract.golden.json
```
Expected: the fleet-core tests PASS; the REGEN run rewrites the golden and fails once by design; the second contract run PASSES; the golden's diff is exactly one added line, `"harnesses",` in `"HostRow"` between `"disk_tmp_free_kb"` and `"health_at"`.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/migrations/089_host_harnesses.sql crates/fleet-core/src/store crates/fleet-core/src/service/hosts.rs crates/fleet-core/src/service/onboarding.rs crates/fleet-core/src/service/account_usage_poll.rs crates/fleet-core/src/service/health.rs crates/fleet-core/src/service/account_usage.rs src-tauri/src/backend/tests_contract.rs src-tauri/src/backend/hub_contract.golden.json
git commit -m "feat(store): hosts.harnesses, a per-host harness choice (migration 089)"
```

---

### Task 2: Detect Codex in its scan, and the one harness gate

**Files:**
- Modify: `crates/fleet-core/src/service/catalog/harness/mod.rs:14-17` (`HARNESS_IDS` doc), `:142-148` (`HostSnapshot`), `:503-512` (`parse_scan_blocks`), tests module (end, ~line 716)
- Modify: `crates/fleet-core/src/service/catalog/harness/codex.rs:236-264` (`scan_script`), tests module
- Create: `crates/fleet-core/src/service/catalog/harness_set.rs`
- Modify: `crates/fleet-core/src/service/catalog/mod.rs:9` (`pub mod harness_set;` after `pub mod harness;`)

**Interfaces:**
- Consumes: `Manifest` (`sync::manifest`), `Catalog` (`repo`), `HARNESS_IDS`.
- Produces:
  - `HostSnapshot.present: bool` — set by a `##PRESENT` line in any scan output
  - Codex scan prints `##PRESENT` when `command -v codex` succeeds or `~/.codex` is a directory
  - in `service::catalog::harness_set`:
    - `pub const ALWAYS_ON: &str = "claude";`
    - `pub enum HarnessGate { Off, On, Retiring }` (`Debug, Clone, Copy, PartialEq, Eq`)
    - `pub struct HarnessFacts { pub detected: bool, pub managed: bool }` with `pub const UNKNOWN: HarnessFacts` and `pub fn of(snap: &HostSnapshot, manifest: &Manifest) -> HarnessFacts`
    - `pub fn harness_gate(id: &str, configured: Option<&[String]>, facts: HarnessFacts) -> HarnessGate`
    - `pub fn gated_catalog(gate: HarnessGate, catalog: &Catalog) -> Option<&Catalog>`
    - `pub fn retiring_detail(id: &str) -> String`
    - `pub fn normalize_harnesses(list: &[String]) -> Result<Vec<String>, IpcError>`

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/fleet-core/src/service/catalog/harness/mod.rs`:

```rust
    /// `##PRESENT` (multi-harness F3a) says the scan saw the harness itself
    /// on the host; its absence says it did not. It is neither a hash line
    /// nor a config body, wherever it appears.
    #[test]
    fn parse_scan_blocks_reads_the_presence_line() {
        let present =
            parse_scan_blocks("##PRESENT\n##HASHES\naaaa  .codex/skills/s/SKILL.md\n##END\n", &|_, _| None)
                .unwrap();
        assert!(present.present);
        assert_eq!(present.files.len(), 1);
        let absent = parse_scan_blocks("##HASHES\n##END\n", &|_, _| None).unwrap();
        assert!(!absent.present);
    }
```

Append to `mod tests` in `crates/fleet-core/src/service/catalog/harness/codex.rs`:

```rust
    /// F3a: the scan probes for Codex itself — the CLI on PATH or a
    /// `~/.codex` directory — without any single quote (the caller wraps
    /// the whole script in `shell::quote`).
    #[test]
    fn scan_script_probes_for_codex() {
        let s = Codex.scan_script().unwrap();
        assert!(
            s.contains("if command -v codex >/dev/null 2>&1 || [ -d .codex ]; then echo \"##PRESENT\"; fi; "),
            "{s}"
        );
        assert!(!s.contains('\''));
    }

    /// The real scan under `bash -lc` against a temp `$HOME` that has a
    /// `~/.codex` directory reads back as present, whether or not the
    /// machine running the test has the codex CLI.
    #[cfg(unix)]
    #[test]
    fn a_codex_directory_reads_as_present_under_bash() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join(".codex")).unwrap();
        let out = std::process::Command::new("bash")
            .arg("-lc")
            .arg(Codex.scan_script().unwrap())
            .env("HOME", tmp.path())
            .output()
            .expect("run scan script");
        assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
        let snap = Codex.parse_scan(&String::from_utf8(out.stdout).unwrap()).unwrap();
        assert!(snap.present);
    }
```

Create `crates/fleet-core/src/service/catalog/harness_set.rs` with the module doc and tests only (the items come in Step 3):

```rust
//! Which harnesses the asset catalog serves on a host (multi-harness F3a).

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::catalog::model::Asset;
    use crate::service::catalog::sync::manifest::ManifestEntry;

    fn list(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    const NOTHING: HarnessFacts = HarnessFacts { detected: false, managed: false };
    const DETECTED: HarnessFacts = HarnessFacts { detected: true, managed: false };
    const MANAGED: HarnessFacts = HarnessFacts { detected: false, managed: true };

    #[test]
    fn claude_is_always_on() {
        for configured in [None, Some(list(&["claude"])), Some(list(&["claude", "codex"]))] {
            for facts in [NOTHING, DETECTED, MANAGED, HarnessFacts::UNKNOWN] {
                assert_eq!(
                    harness_gate("claude", configured.as_deref(), facts),
                    HarnessGate::On
                );
            }
        }
    }

    #[test]
    fn auto_follows_detection_and_the_manifest() {
        assert_eq!(harness_gate("codex", None, NOTHING), HarnessGate::Off);
        assert_eq!(harness_gate("codex", None, DETECTED), HarnessGate::On);
        assert_eq!(
            harness_gate("codex", None, MANAGED),
            HarnessGate::On,
            "a host fleet already manages Codex on stays served"
        );
        assert_eq!(harness_gate("codex", None, HarnessFacts::UNKNOWN), HarnessGate::Off);
    }

    #[test]
    fn an_explicit_list_wins_and_an_explicit_off_retires_what_fleet_installed() {
        let on = list(&["claude", "codex"]);
        let off = list(&["claude"]);
        let (on, off) = (Some(on.as_slice()), Some(off.as_slice()));
        assert_eq!(harness_gate("codex", on, NOTHING), HarnessGate::On);
        assert_eq!(harness_gate("codex", on, HarnessFacts::UNKNOWN), HarnessGate::On);
        assert_eq!(harness_gate("codex", off, DETECTED), HarnessGate::Off);
        assert_eq!(harness_gate("codex", off, MANAGED), HarnessGate::Retiring);
        assert_eq!(
            harness_gate("codex", off, HarnessFacts { detected: true, managed: true }),
            HarnessGate::Retiring
        );
    }

    #[test]
    fn gated_catalog_is_the_hosts_own_an_empty_one_or_none() {
        let mut cat = Catalog::default();
        cat.assets.push(Asset::from_yaml(None, "kind: skill\nname: s\ndescription: d\n").unwrap());
        assert_eq!(gated_catalog(HarnessGate::On, &cat).unwrap().assets.len(), 1);
        assert!(gated_catalog(HarnessGate::Retiring, &cat).unwrap().assets.is_empty());
        assert!(gated_catalog(HarnessGate::Off, &cat).is_none());
    }

    #[test]
    fn facts_come_from_the_presence_line_and_a_non_empty_manifest() {
        let mut snap = HostSnapshot::default();
        let mut manifest = Manifest::default();
        assert_eq!(HarnessFacts::of(&snap, &manifest), NOTHING);
        snap.present = true;
        manifest.assets.insert("skill/s".into(), ManifestEntry::default());
        assert_eq!(
            HarnessFacts::of(&snap, &manifest),
            HarnessFacts { detected: true, managed: true }
        );
    }

    #[test]
    fn retiring_detail_names_the_harness() {
        assert_eq!(
            retiring_detail("codex"),
            "codex is turned off on this host: only what fleet installed there is removed"
        );
    }

    #[test]
    fn normalize_harnesses_orders_dedupes_and_requires_claude() {
        assert_eq!(
            normalize_harnesses(&list(&["codex", "claude", "codex"])).unwrap(),
            list(&["claude", "codex"])
        );
        assert_eq!(normalize_harnesses(&list(&["claude"])).unwrap(), list(&["claude"]));
        let err = normalize_harnesses(&list(&["codex"])).unwrap_err();
        assert_eq!(err.code, "E_INVALID");
        assert!(err.message.contains("claude"), "{}", err.message);
        assert_eq!(normalize_harnesses(&[]).unwrap_err().code, "E_INVALID");
        let err = normalize_harnesses(&list(&["claude", "gemini"])).unwrap_err();
        assert_eq!(err.code, "E_INVALID");
        assert!(err.message.contains("gemini"), "{}", err.message);
    }
}
```

Add `pub mod harness_set;` in `crates/fleet-core/src/service/catalog/mod.rs` after `pub mod harness;` (line 9).

- [ ] **Step 2: Run the tests to verify they fail**

Run:
```bash
export CARGO_TARGET_DIR=<shared-target-dir>
cargo test -p fleet-core --lib service::catalog::harness_set service::catalog::harness::tests::parse_scan_blocks_reads_the_presence_line service::catalog::harness::codex::tests 2>&1 | tail -20
```
Expected: compile errors — `no field present on type HostSnapshot`, `cannot find function harness_gate`, `cannot find type HarnessGate`, … (`scan_script_probes_for_codex` would fail its `contains` assertion once the crate compiles).

- [ ] **Step 3: Implement**

In `harness/mod.rs`, replace the `HARNESS_IDS` doc and attribute (lines 14-17):

```rust
/// Every harness id the catalog knows, in the order an explicit
/// `hosts.harnesses` list is normalised to. Read by the lint (`targets.<h>`)
/// and by `harness_set` (multi-harness F3a).
pub const HARNESS_IDS: &[&str] = &["claude", "codex"];
```

Replace `HostSnapshot` (lines 142-148):

```rust
/// What a host scan found: file hashes keyed by `~/`-relative path, parsed
/// JSON config files keyed the same way, and whether the scan saw the
/// harness itself on the host.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct HostSnapshot {
    pub files: BTreeMap<String, String>,
    pub configs: BTreeMap<String, serde_json::Value>,
    /// A `##PRESENT` line was in the scan output (multi-harness F3a). Only
    /// a scan that probes for its harness prints one — Codex's does (the
    /// `codex` CLI on PATH, or a `~/.codex` directory); Claude's does not,
    /// and `harness_set::harness_gate` never asks for Claude.
    pub present: bool,
}
```

In `parse_scan_blocks`, after the `##HASHES` branch (after line 512):

```rust
        if line == "##PRESENT" {
            snap.present = true;
            current_config = None;
            continue;
        }
```

In `codex.rs` `scan_script`, right after the hasher-detection `push_str` (line 241), before `echo "##HASHES"`:

```rust
        // Multi-harness F3a: is Codex itself here? `harness_set::harness_gate`
        // serves Codex on an auto host only when this line is printed (or
        // fleet already manages Codex assets there).
        s.push_str(
            "if command -v codex >/dev/null 2>&1 || [ -d .codex ]; then echo \"##PRESENT\"; fi; ",
        );
```

Replace the contents of `crates/fleet-core/src/service/catalog/harness_set.rs` above `#[cfg(test)]` (keep the tests module from Step 1 at the bottom):

```rust
//! Which harnesses the asset catalog serves on a host (multi-harness F3a).
//!
//! Claude is always served. Another harness (today: Codex) is served where
//! the host says so (`hosts.harnesses`, migration 089) or — when the host
//! leaves it to fleet (`NULL`, "auto") — where a scan finds it: the Codex
//! scan prints `##PRESENT` when the `codex` CLI is on PATH or `~/.codex`
//! exists (`HostSnapshot::present`). Every scanning harness is still scanned
//! on every reachable host, because the scan is the only place detection and
//! the host's manifest come from; the gate decides what is *planned* and
//! *inventoried*.
//!
//! A harness fleet already manages on a host (its manifest names assets)
//! stays served under auto. Turned off explicitly, it is
//! [`HarnessGate::Retiring`]: planned and inventoried against an empty
//! catalog, so the only actions are removals of what fleet installed.
//!
//! [`harness_gate`] is the one decision. `sync::plan_sync`,
//! `sync::rescan_after_apply` (both through `sync::scan_and_persist`) and
//! `inventory::scan_hosts` call it.

use super::harness::{HostSnapshot, HARNESS_IDS};
use super::repo::Catalog;
use super::sync::manifest::Manifest;
use crate::ipc_error::codes::E_INVALID;
use crate::ipc_error::IpcError;
use std::sync::LazyLock;

/// The harness no host can turn off in F3a: fleet's own sessions, hooks,
/// MCP entry and provisioned skills are Claude's.
pub const ALWAYS_ON: &str = "claude";

/// What the catalog does with one harness on one host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessGate {
    /// Not served: nothing planned, no inventory rows (persisting the empty
    /// list clears rows an earlier scan left).
    Off,
    /// Served: planned and inventoried against the host's catalog.
    On,
    /// Turned off, but fleet still manages assets there: planned and
    /// inventoried against an empty catalog, so only removals come back.
    Retiring,
}

/// What one scan says about a harness on a host.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HarnessFacts {
    /// The scan saw the harness itself (`HostSnapshot::present`).
    pub detected: bool,
    /// The harness's manifest on the host names at least one asset.
    pub managed: bool,
}

impl HarnessFacts {
    /// Before any scan: nothing detected, nothing managed. Only Claude and
    /// the harnesses a host lists explicitly pass the gate with these.
    pub const UNKNOWN: HarnessFacts = HarnessFacts {
        detected: false,
        managed: false,
    };

    pub fn of(snap: &HostSnapshot, manifest: &Manifest) -> HarnessFacts {
        HarnessFacts {
            detected: snap.present,
            managed: !manifest.assets.is_empty(),
        }
    }
}

/// The one decision. `configured` is the host's `harnesses` column (`None`
/// = auto). Claude is always `On`. Another harness is `On` when the host
/// lists it, or — on auto — when the scan detected it or fleet manages it
/// there; otherwise `Retiring` while fleet still manages something there,
/// else `Off`.
pub fn harness_gate(id: &str, configured: Option<&[String]>, facts: HarnessFacts) -> HarnessGate {
    if id == ALWAYS_ON {
        return HarnessGate::On;
    }
    let wanted = match configured {
        Some(list) => list.iter().any(|h| h == id),
        None => facts.detected || facts.managed,
    };
    if wanted {
        HarnessGate::On
    } else if facts.managed {
        HarnessGate::Retiring
    } else {
        HarnessGate::Off
    }
}

static EMPTY: LazyLock<Catalog> = LazyLock::new(Catalog::default);

/// The catalog a harness in `gate` is planned and inventoried against: the
/// host's own for `On`; an empty one for `Retiring` (every manifest entry
/// then reads as an orphan — `Remove` in a plan, `orphan` in the inventory);
/// none for `Off`.
pub fn gated_catalog(gate: HarnessGate, catalog: &Catalog) -> Option<&Catalog> {
    match gate {
        HarnessGate::On => Some(catalog),
        HarnessGate::Retiring => Some(&EMPTY),
        HarnessGate::Off => None,
    }
}

/// `HostPlan::detail` of a `Retiring` plan.
pub fn retiring_detail(id: &str) -> String {
    format!("{id} is turned off on this host: only what fleet installed there is removed")
}

/// An explicit harness list, checked and put in `HARNESS_IDS` order: every
/// id known, `claude` present, duplicates dropped.
pub fn normalize_harnesses(list: &[String]) -> Result<Vec<String>, IpcError> {
    if let Some(unknown) = list.iter().find(|h| !HARNESS_IDS.contains(&h.as_str())) {
        return Err(IpcError::new(
            E_INVALID,
            format!(
                "unknown harness '{unknown}' (known: {})",
                HARNESS_IDS.join(", ")
            ),
        ));
    }
    if !list.iter().any(|h| h == ALWAYS_ON) {
        return Err(IpcError::new(
            E_INVALID,
            "claude cannot be turned off on a host: fleet's own sessions, hooks and skills are Claude's",
        ));
    }
    Ok(HARNESS_IDS
        .iter()
        .filter(|id| list.iter().any(|h| h == *id))
        .map(|id| id.to_string())
        .collect())
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run:
```bash
export CARGO_TARGET_DIR=<shared-target-dir>
cargo test -p fleet-core --lib service::catalog::harness_set service::catalog::harness::tests service::catalog::harness::codex::tests service::catalog::harness::claude::tests 2>&1 | tail -8
```
Expected: all PASS (every existing harness/codex/claude test included — `scan_script_runs_under_bash_and_parses_cleanly` still sees exactly its two files; `##PRESENT` is not a hash line).

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/service/catalog/harness/mod.rs crates/fleet-core/src/service/catalog/harness/codex.rs crates/fleet-core/src/service/catalog/harness_set.rs crates/fleet-core/src/service/catalog/mod.rs
git commit -m "feat(catalog): detect Codex in its scan and gate harnesses per host"
```

---

### Task 3: Plan and inventory each harness only where the gate lets it

**Files:**
- Modify: `crates/fleet-core/src/service/catalog/sync/mod.rs:28-40` (imports), `:128-167` (`scan_and_persist`), `:218-293` (`plan_sync` per-host loop), `:363-391` (`rescan_after_apply`), tests (`store_with_local` `:675-680`, new tests)
- Modify: `crates/fleet-core/src/service/catalog/inventory.rs:4-16` (imports), `:192-247` (`scan_hosts` loop), tests (new `HomeGuard`, new test)

**Interfaces:**
- Consumes: `harness_set::{harness_gate, gated_catalog, retiring_detail, HarnessFacts, HarnessGate}` (Task 2); `Store::set_host_harnesses`, `HostRow.harnesses` (Task 1).
- Produces:
  - `plan_sync`: no `HostPlan` for an `Off` harness; a `Retiring` harness's plan is computed against an empty catalog and carries `detail = Some(retiring_detail(id))`; a host skipped before scanning gets skipped plans for Claude and its explicitly listed harnesses only.
  - `scan_and_persist(…, configured: Option<&[String]>) -> Result<(HostSnapshot, Manifest, HarnessGate), IpcError>` (private)
  - `scan_hosts` and the post-apply rescan persist gated rows (empty for `Off`, clearing stale ones).

- [ ] **Step 1: Write the failing tests**

In `crates/fleet-core/src/service/catalog/sync/mod.rs` tests, **change** `store_with_local` (lines 675-680). Reason: with F3a, `local` on auto would plan Codex only when the machine running the tests has the codex CLI (or `~/.codex`), so the existing assertions that expect two `HostPlan`s (`plan.hosts.len() == 2`, `create == 2`, `blocked == 2`, `noop == 2`, `sync:progress:::2/2`) would become machine-dependent. Pinning Codex on keeps every one of them as it is:

```rust
    fn store_with_local(bus: Arc<RecordingEventBus>) -> Mutex<Store> {
        let dyn_bus: Arc<dyn crate::events::EventBus> = bus;
        let store = Mutex::new(Store::open_with_bus_in_memory(dyn_bus).unwrap());
        {
            let s = store.lock().unwrap();
            s.insert_host("local", None).unwrap();
            // Multi-harness F3a: pin Codex on, so these tests plan it whether
            // or not the machine running them has the codex CLI (auto would
            // follow detection).
            s.set_host_harnesses(
                "local",
                Some(&["claude".to_string(), "codex".to_string()][..]),
            )
            .unwrap();
        }
        store
    }

    fn harness_ids(plan: &SyncPlan) -> Vec<&str> {
        plan.hosts.iter().map(|h| h.harness.as_str()).collect()
    }

    fn load_one_skill() -> tempfile::TempDir {
        let repo_dir = tempfile::tempdir().unwrap();
        let files = one_skill("b\n");
        load_catalog(
            repo_dir.path(),
            &files
                .iter()
                .map(|(a, b)| (*a, b.as_str()))
                .collect::<Vec<_>>(),
        );
        repo_dir
    }
```

Append these tests to the same module:

```rust
    /// F3a: a host that turned Codex off (and has no Codex manifest) gets no
    /// Codex plan, and a Codex inventory row an earlier scan left is gone.
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn plan_sync_leaves_codex_out_on_a_host_that_turned_it_off() {
        let _lock = super::super::CATALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());
        let _repo = load_one_skill();
        let store = store_with_local(Arc::new(RecordingEventBus::new()));
        {
            let s = store.lock().unwrap();
            s.set_host_harnesses("local", Some(&["claude".to_string()][..]))
                .unwrap();
            s.replace_host_inventory(
                "local",
                "codex",
                &[crate::store::AssetInventoryRow {
                    host_alias: "local".into(),
                    harness: "codex".into(),
                    kind: "skill".into(),
                    name: "s".into(),
                    state: "missing".into(),
                    ..Default::default()
                }],
            )
            .unwrap();
        }
        let ssh = Arc::new(SshClient::new());

        let plan = plan_sync(PlanArgs::default(), &store, &ssh).await.unwrap();
        assert_eq!(harness_ids(&plan), vec!["claude"], "{:?}", plan.hosts);
        let rows = store.lock().unwrap().list_inventory().unwrap();
        assert!(rows.iter().any(|r| r.name == "s" && r.harness == "claude"), "{rows:?}");
        assert!(rows.iter().all(|r| r.harness != "codex"), "{rows:?}");
    }

    /// F3a: on auto, a host with a `~/.codex` directory is planned and
    /// inventoried for Codex (deterministic whatever PATH holds: the
    /// directory alone is detection).
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn plan_sync_plans_codex_where_auto_finds_it() {
        let _lock = super::super::CATALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());
        std::fs::create_dir_all(home.path().join(".codex")).unwrap();
        let _repo = load_one_skill();
        let store = store_with_local(Arc::new(RecordingEventBus::new()));
        store.lock().unwrap().set_host_harnesses("local", None).unwrap();
        let ssh = Arc::new(SshClient::new());

        let plan = plan_sync(PlanArgs::default(), &store, &ssh).await.unwrap();
        let codex = plan
            .hosts
            .iter()
            .find(|h| h.harness == "codex")
            .expect("codex is planned");
        assert_eq!(codex.status, "planned");
        assert_eq!(codex.detail, None);
        assert_eq!(
            codex.actions.iter().find(|a| a.name == "s").map(|a| a.op),
            Some(ActionOp::Create)
        );
        let rows = store.lock().unwrap().list_inventory().unwrap();
        assert!(
            rows.iter()
                .any(|r| r.harness == "codex" && r.name == "s" && r.state == "missing"),
            "{rows:?}"
        );
    }

    /// F3a: turning Codex off on a host fleet already synced Codex assets to
    /// retires it — the plan is removals only, and says why — while Claude
    /// is planned as before.
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn plan_sync_retires_codex_when_turned_off_but_still_managed() {
        let _lock = super::super::CATALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());
        std::fs::create_dir_all(home.path().join(".codex")).unwrap();
        std::fs::write(
            home.path().join(".codex/.fleet-assets.json"),
            r#"{"version":1,"updated_at":0,"assets":{"skill/s":
                {"hash":"h","files":["~/.codex/skills/s/SKILL.md"],"merges":[],"synced_at":0}}}"#,
        )
        .unwrap();
        let _repo = load_one_skill();
        let store = store_with_local(Arc::new(RecordingEventBus::new()));
        store
            .lock()
            .unwrap()
            .set_host_harnesses("local", Some(&["claude".to_string()][..]))
            .unwrap();
        let ssh = Arc::new(SshClient::new());

        let plan = plan_sync(PlanArgs::default(), &store, &ssh).await.unwrap();
        let codex = plan.hosts.iter().find(|h| h.harness == "codex").unwrap();
        assert_eq!(codex.status, "planned");
        assert_eq!(
            codex.detail.as_deref(),
            Some(harness_set::retiring_detail("codex").as_str())
        );
        assert_eq!(
            codex
                .actions
                .iter()
                .map(|a| (a.name.as_str(), a.op))
                .collect::<Vec<_>>(),
            vec![("s", ActionOp::Remove)]
        );
        let claude = plan.hosts.iter().find(|h| h.harness == "claude").unwrap();
        assert_eq!(
            claude.actions.iter().find(|a| a.name == "s").map(|a| a.op),
            Some(ActionOp::Create)
        );
        let rows = store.lock().unwrap().list_inventory().unwrap();
        assert!(
            rows.iter()
                .any(|r| r.harness == "codex" && r.name == "s" && r.state == "orphan"),
            "{rows:?}"
        );
    }

    /// F3a: a host skipped before its scan (here: unlayered and remote) is
    /// reported under Claude and the harnesses it lists — nothing is known
    /// about the others until a scan runs.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_host_skipped_before_its_scan_reports_claude_and_its_listed_harnesses() {
        let _lock = super::super::CATALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _repo = load_one_skill();
        let store = store_with_local(Arc::new(RecordingEventBus::new()));
        {
            let s = store.lock().unwrap();
            s.insert_host("oci", Some("oci")).unwrap();
            s.update_host_probe("oci", true, None, None, 1).unwrap();
        }
        let ssh = Arc::new(SshClient::new());
        let args = || PlanArgs {
            host_alias: Some("oci".into()),
            ..PlanArgs::default()
        };

        let auto = plan_sync(args(), &store, &ssh).await.unwrap();
        assert_eq!(harness_ids(&auto), vec!["claude"], "{:?}", auto.hosts);

        store
            .lock()
            .unwrap()
            .set_host_harnesses("oci", Some(&["claude".to_string(), "codex".to_string()][..]))
            .unwrap();
        let listed = plan_sync(args(), &store, &ssh).await.unwrap();
        assert_eq!(harness_ids(&listed), vec!["claude", "codex"]);
        assert!(listed
            .hosts
            .iter()
            .all(|h| h.status == "skipped" && h.detail.as_deref() == Some(UNLAYERED_DETAIL)));
    }
```

In `crates/fleet-core/src/service/catalog/inventory.rs` tests, append:

```rust
    /// Restores `HOME` when the test (or a panic) ends. Copied from
    /// `sync::apply`'s test module, which cannot export it.
    #[cfg(unix)]
    struct HomeGuard(Option<String>);
    #[cfg(unix)]
    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match self.0.take() {
                Some(home) => std::env::set_var("HOME", home),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    /// F3a: `scan_hosts` keeps no Codex rows for a host that turned Codex
    /// off, even with `~/.codex` present; back on auto, the same host is
    /// inventoried for Codex again.
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn scan_hosts_follows_the_hosts_harness_choice() {
        let _g = crate::service::catalog::CATALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());
        std::fs::create_dir_all(home.path().join(".codex")).unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("skills/s")).unwrap();
        std::fs::write(root.path().join("catalog.yaml"), "schema_version: 1\n").unwrap();
        std::fs::write(
            root.path().join("skills/s/asset.yaml"),
            "kind: skill\nname: s\ndescription: d\n",
        )
        .unwrap();
        std::fs::write(root.path().join("skills/s/body.md"), "b\n").unwrap();
        let cat = crate::service::catalog::repo::load_dir(root.path()).unwrap();
        *crate::service::catalog::CATALOG.write().unwrap() = Some(cat);

        let store = std::sync::Mutex::new(crate::store::Store::open_in_memory().unwrap());
        {
            let s = store.lock().unwrap();
            s.insert_host("local", None).unwrap();
            s.set_host_harnesses("local", Some(&["claude".to_string()][..]))
                .unwrap();
        }
        let ssh = std::sync::Arc::new(crate::ssh::SshClient::new());
        let results = scan_hosts(&store, &ssh, Some("local")).await.unwrap();
        assert_eq!(results[0].status, "scanned", "{:?}", results[0].detail);
        let rows = store.lock().unwrap().list_inventory().unwrap();
        assert!(rows.iter().any(|r| r.harness == "claude" && r.name == "s"));
        assert!(rows.iter().all(|r| r.harness != "codex"), "{rows:?}");

        store.lock().unwrap().set_host_harnesses("local", None).unwrap();
        scan_hosts(&store, &ssh, Some("local")).await.unwrap();
        let rows = store.lock().unwrap().list_inventory().unwrap();
        assert!(
            rows.iter()
                .any(|r| r.harness == "codex" && r.name == "s" && r.state == "missing"),
            "{rows:?}"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run:
```bash
export CARGO_TARGET_DIR=<shared-target-dir>
cargo test -p fleet-core --lib service::catalog::sync::tests service::catalog::inventory::tests 2>&1 | tail -25
```
Expected: `harness_set` is not in scope in `sync::tests` (compile error `use of undeclared crate or module harness_set`) — once that import exists, the four new sync tests and `scan_hosts_follows_the_hosts_harness_choice` FAIL (a Codex plan is still pushed for an off host, no retiring detail, two skipped rows on the auto host, Codex rows kept).

- [ ] **Step 3: Implement**

In `crates/fleet-core/src/service/catalog/sync/mod.rs`, add to the imports (after line 36):

```rust
use super::harness_set::{self, gated_catalog, harness_gate, HarnessFacts, HarnessGate};
```

Replace `scan_and_persist` (lines 128-167) with:

```rust
/// Scan one (host, harness), persist the inventory rows that scan implies
/// (so the asset matrix refreshes through the row events
/// `replace_host_inventory` already emits) and hand the snapshot, its
/// managed manifest and the harness's gate back to the caller. `secrets` is
/// resolved by the caller BEFORE this await — `secrets::resolve` takes the
/// store lock internally. `configured` is the host's `harnesses` column
/// (`None` = auto): the rows follow `harness_set::harness_gate` — none for
/// `Off` (persisting the empty list clears rows an earlier scan left), only
/// fleet's own installs (as orphans) for `Retiring`. Persisting is
/// best-effort: a scan is still usable if the rows could not be written.
async fn scan_and_persist(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    catalog: &super::repo::Catalog,
    harness: &dyn super::harness::Harness,
    host_alias: &str,
    secrets: &BTreeMap<String, String>,
    configured: Option<&[String]>,
) -> Result<(super::harness::HostSnapshot, Manifest, HarnessGate), IpcError> {
    let scanned_at = super::now_secs();
    let snap = super::inventory::scan_host_harness(ssh, host_alias, harness).await?;
    let manifest = Manifest::from_snapshot(&snap, harness.manifest_path());
    let gate = harness_gate(harness.id(), configured, HarnessFacts::of(&snap, &manifest));
    let rows = gated_catalog(gate, catalog).map_or_else(Vec::new, |c| {
        super::inventory::compute_states(c, harness, host_alias, &snap, &manifest, secrets, scanned_at)
    });
    match store.lock() {
        Ok(s) => {
            if let Err(e) = s.replace_host_inventory(host_alias, harness.id(), &rows) {
                tracing::warn!(
                    host = host_alias,
                    harness = harness.id(),
                    error = %e,
                    "could not persist host inventory"
                );
            }
        }
        Err(_) => tracing::warn!(
            host = host_alias,
            harness = harness.id(),
            "store mutex poisoned while persisting inventory"
        ),
    }
    Ok((snap, manifest, gate))
}
```

In `plan_sync`, replace the per-host loop (lines 218-293, from `let mut host_plans` through the loop's closing `}`) with:

```rust
    let mut host_plans: Vec<HostPlan> = Vec::new();
    for h in hosts {
        if h.hidden || filter.host_alias.as_deref().is_some_and(|o| o != h.alias) {
            continue;
        }
        // Multi-harness F3a: the host's own harness choice (`None` = auto).
        // Before a scan nothing is detected, so a host skipped before
        // scanning is reported under Claude and the harnesses it lists only.
        let configured = h.harnesses.as_deref();
        let listed: Vec<&dyn super::harness::Harness> = scanning
            .iter()
            .copied()
            .filter(|hn| harness_gate(hn.id(), configured, HarnessFacts::UNKNOWN) != HarnessGate::Off)
            .collect();
        if h.alias != "local" && !h.reachable {
            for hn in &listed {
                host_plans.push(skipped_plan(&h.alias, hn.id(), "unreachable"));
            }
            continue;
        }
        // Before any await: `resolve` takes the store lock internally.
        let secrets = match secrets::resolve(store, &h.alias) {
            Ok(v) => v,
            Err(e) => {
                for hn in &listed {
                    host_plans.push(skipped_plan(&h.alias, hn.id(), &e.message));
                }
                continue;
            }
        };
        // Resolve the host's layers ONCE per host, before its harnesses are
        // planned. The scan below still uses the FULL catalog: inventory is
        // about the whole catalog's drift, while the PLAN is about what this
        // host is supposed to have.
        let resolved = match layers::resolve_for_host(store, &catalog, &h.alias) {
            Ok(r) => r,
            Err(e) => {
                for harness in &listed {
                    host_plans.push(skipped_plan(&h.alias, harness.id(), &e.message));
                }
                continue;
            }
        };
        // A remote host with no layers would otherwise receive the whole
        // catalog — the spec's first critical finding. Refuse it up front,
        // before the scan: every listed harness gets the same skipped
        // plan and the host is never touched over SSH.
        if refuse_unlayered(
            &h.alias,
            resolved.layered,
            args.allow_unlayered,
            resolved.catalog.assets.is_empty(),
        ) {
            for harness in &listed {
                host_plans.push(skipped_plan(&h.alias, harness.id(), UNLAYERED_DETAIL));
            }
            continue;
        }
        // A host with no `host_layers` row must plan a dropped `plugin_ref`
        // exactly as it did before layers existed (`Remove`), not the
        // "reported, not removed" `Noop` that only makes sense once a host
        // opts into layers. `resolved.layered` answers that from the same
        // read `resolve_for_host` already made — a second read would be a
        // second failure path, and one that used to abort the whole plan
        // instead of skipping just this host.
        let host_filter = PlanFilter {
            layered: resolved.layered,
            ..filter.clone()
        };
        // Every scanning harness is scanned, even one this host may not
        // serve: the scan is what detects it and reads its manifest.
        for harness in &scanning {
            let harness = *harness;
            match scan_and_persist(store, ssh, &catalog, harness, &h.alias, &secrets, configured)
                .await
            {
                Ok((snap, manifest, gate)) => {
                    // `Off` plans nothing; `Retiring` plans against an empty
                    // catalog, so only removals of fleet's own installs remain.
                    let Some(planned) = gated_catalog(gate, &resolved.catalog) else {
                        continue;
                    };
                    let mut hp = plan::compute_host_plan(
                        planned,
                        harness,
                        &h.alias,
                        &snap,
                        &manifest,
                        &secrets,
                        &host_filter,
                    );
                    if gate == HarnessGate::Retiring {
                        hp.detail = Some(harness_set::retiring_detail(harness.id()));
                    }
                    host_plans.push(hp);
                }
                Err(e) => host_plans.push(skipped_plan(&h.alias, harness.id(), &e.message)),
            }
        }
    }
```

In `rescan_after_apply`, replace the final `if let Err(e) = scan_and_persist(…)` block (lines 383-390) with:

```rust
    // F3a: the host's harness choice decides which rows the re-scan keeps.
    // Read before the await, as everywhere else.
    let configured = match store.lock() {
        Ok(s) => s
            .get_host_row(host_alias)
            .ok()
            .flatten()
            .and_then(|r| r.harnesses),
        Err(_) => None,
    };
    if let Err(e) = scan_and_persist(
        store,
        ssh,
        catalog,
        harness,
        host_alias,
        &secrets,
        configured.as_deref(),
    )
    .await
    {
        tracing::warn!(
            host = host_alias,
            harness = harness.id(),
            error = %e.message,
            "post-sync re-scan failed; the asset matrix is stale for this host"
        );
    }
```

In `crates/fleet-core/src/service/catalog/inventory.rs`, add to the imports (after line 7):

```rust
use super::harness_set::{gated_catalog, harness_gate, HarnessFacts};
```

In `scan_hosts`, after the `secrets` match (after line 211) add:

```rust
        // Multi-harness F3a: the host's own harness choice (`None` = auto).
        let configured = h.harnesses.clone();
```

and replace the `Ok(snap) => { … let rows = compute_states(…); …` head (lines 218-228) with:

```rust
                Ok(snap) => {
                    let manifest = Manifest::from_snapshot(&snap, harness.manifest_path());
                    // `Off` persists no rows (clearing stale ones); `Retiring`
                    // only fleet's own installs, as orphans.
                    let gate = harness_gate(
                        harness.id(),
                        configured.as_deref(),
                        HarnessFacts::of(&snap, &manifest),
                    );
                    let rows = gated_catalog(gate, &catalog).map_or_else(Vec::new, |c| {
                        compute_states(
                            c,
                            harness.as_ref(),
                            &h.alias,
                            &snap,
                            &manifest,
                            &secrets,
                            scanned_at,
                        )
                    });
```

(the rest of the arm — `total += rows.len();` and `replace_host_inventory` — stays as it is). Update the `scan_hosts` doc comment's first sentence to: "Scan every non-hidden reachable host (or just `only_host`) with every harness that supports scanning, persisting per (host, harness) the rows `harness_set::harness_gate` lets through."

- [ ] **Step 4: Run the tests to verify they pass**

Run:
```bash
export CARGO_TARGET_DIR=<shared-target-dir>
cargo test -p fleet-core --lib service::catalog::sync service::catalog::inventory service::catalog::scan_tick service::catalog::harness_set 2>&1 | tail -10
```
Expected: all PASS — the new tests, and every existing `sync::tests` test with its original assertions (two `HostPlan`s on the pinned `local`).

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/service/catalog/sync/mod.rs crates/fleet-core/src/service/catalog/inventory.rs
git commit -m "feat(catalog): plan and inventory Codex only where the host has it"
```

---

### Task 4: `set_host_harnesses` — service, `catalog_admin`, MCP tool

**Files:**
- Modify: `crates/fleet-core/src/service/catalog/harness_set.rs` (service fn + test)
- Modify: `crates/fleet-core/src/service/catalog/admin.rs:51-58` (args), `:129` (macro), `:209-214` (run), `:320-324` (`every_call`)
- Modify: `crates/fleet-core/src/mcp/tools/params.rs:998-1025`
- Modify: `crates/fleet-core/src/mcp/tools/assets.rs:299-331` (new tool after `set_host_layers`)
- Modify: `crates/fleet-core/src/mcp/guard.rs:914-925` (policy row after `set_host_layers`)
- Modify: `crates/fleet-core/src/mcp/tools/tests.rs:281-288`, `:328-340`, `:2137-2182`, `:3405-3427`
- Modify: `crates/fleet-core/src/mcp/tools/tests_catalog_admin.rs` (new test)
- Regenerate: `docs/control-api-reference.md`

**Interfaces:**
- Consumes: `normalize_harnesses` (Task 2), `Store::set_host_harnesses`, `HostRow.harnesses` (Task 1).
- Produces:
  - `pub fn harness_set::set_host_harnesses(host_alias: &str, harnesses: Option<&[String]>, store: &Mutex<Store>) -> Result<HostRow, IpcError>` — validates the alias, normalises the list, writes, returns the new row
  - `pub struct admin::SetHostHarnessesArgs { pub host_alias: String, #[serde(default)] pub harnesses: Option<Vec<String>> }` and `AdminCall::SetHostHarnesses(SetHostHarnessesArgs)` on the wire as `{"action":"set_host_harnesses","args":{…}}`
  - MCP tool `set_host_harnesses { host_alias, harnesses: null | [..] }` → the `HostRow`, master token only

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `harness_set.rs`:

```rust
    #[test]
    fn set_host_harnesses_normalises_validates_and_clears() {
        let store = std::sync::Mutex::new(crate::store::Store::open_in_memory().unwrap());
        store.lock().unwrap().insert_host("h", Some("h")).unwrap();
        let row =
            set_host_harnesses("h", Some(list(&["codex", "claude", "codex"]).as_slice()), &store)
                .unwrap();
        assert_eq!(row.harnesses, Some(list(&["claude", "codex"])));
        let err = set_host_harnesses("h", Some(list(&["codex"]).as_slice()), &store).unwrap_err();
        assert_eq!(err.code, "E_INVALID");
        let err = set_host_harnesses("h", Some(list(&["claude", "gemini"]).as_slice()), &store)
            .unwrap_err();
        assert_eq!(err.code, "E_INVALID");
        assert_eq!(
            store.lock().unwrap().get_host_row("h").unwrap().unwrap().harnesses,
            Some(list(&["claude", "codex"])),
            "a refused call writes nothing"
        );
        assert_eq!(set_host_harnesses("h", None, &store).unwrap().harnesses, None);
        assert_eq!(
            set_host_harnesses("ghost", None, &store).unwrap_err().code,
            "E_NOTFOUND"
        );
    }
```

In `admin.rs` `every_call()`, directly after the `AdminCall::SetHostLayers(…)` element (line 324):

```rust
            AdminCall::SetHostHarnesses(SetHostHarnessesArgs {
                host_alias: "h".into(),
                harnesses: Some(vec!["claude".into(), "codex".into()]),
            }),
```

In `mcp/tools/tests.rs`:
- in `layer_read_tools_are_readonly_and_the_setter_is_not` (line 287), add after the `set_host_layers` line: `assert!(!is_readonly_tool("set_host_harnesses"));`
- in `fleet_admin_tools_are_master_only`, add `"set_host_harnesses",` after `"set_host_layers",` (line 339)
- in `router_sum_serves_every_tool`, change `assert_eq!(served, 102);` (line 2182) to `assert_eq!(served, 103);` and extend the doc comment's last sentence to: "`catalog_admin`, and host identity & health's `merge_host` and `forget_project`: 95; `update_status` / `update_admin`: 97; `session_tool_detail`: 98 (102 with the tools main added alongside it); multi-harness F3a's `set_host_harnesses`: 103.)"

Append to `crates/fleet-core/src/mcp/tools/tests_catalog_admin.rs`:

```rust
/// Multi-harness F3a: the tool sets, normalises and clears a host's harness
/// choice; `catalog_admin` reaches the same function (the way a granted
/// desktop does); a per-host token is refused.
#[tokio::test]
async fn set_host_harnesses_sets_normalises_and_clears() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("local").unwrap();
    let t = tools(s);
    let harnesses_of = |t: &FleetTools| {
        t.store
            .lock()
            .unwrap()
            .get_host_row("local")
            .unwrap()
            .unwrap()
            .harnesses
    };

    t.set_host_harnesses(Parameters(SetHostHarnessesParams {
        host_alias: "local".into(),
        harnesses: Some(vec!["codex".into(), "claude".into()]),
    }))
    .await
    .unwrap();
    assert_eq!(
        harnesses_of(&t),
        Some(vec!["claude".to_string(), "codex".to_string()])
    );

    let err = t
        .set_host_harnesses(Parameters(SetHostHarnessesParams {
            host_alias: "local".into(),
            harnesses: Some(vec!["codex".into()]),
        }))
        .await
        .unwrap_err();
    assert!(err.message.starts_with("E_INVALID"), "{}", err.message);

    t.set_host_harnesses(Parameters(SetHostHarnessesParams {
        host_alias: "local".into(),
        harnesses: None,
    }))
    .await
    .unwrap();
    assert_eq!(harnesses_of(&t), None);

    call(
        &t,
        &Caller::master(),
        "set_host_harnesses",
        Some(json!({ "host_alias": "local", "harnesses": ["claude"] })),
        None,
    )
    .await
    .unwrap();
    assert_eq!(harnesses_of(&t), Some(vec!["claude".to_string()]));

    let err = enforce_admin(&host("local"), "set_host_harnesses").unwrap_err();
    assert!(err.message.starts_with("E_FORBIDDEN"), "{}", err.message);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run:
```bash
export CARGO_TARGET_DIR=<shared-target-dir>
cargo test -p fleet-core --lib service::catalog::harness_set service::catalog::admin mcp::tools::tests_catalog_admin mcp::tools::tests::fleet_admin_tools_are_master_only mcp::tools::tests::layer_read_tools mcp::tools::tests::router_sum_serves_every_tool 2>&1 | tail -20
```
Expected: compile errors — `cannot find function set_host_harnesses in this scope` (harness_set), `no variant SetHostHarnesses`, `cannot find type SetHostHarnessesArgs`, `cannot find struct SetHostHarnessesParams`, `no method named set_host_harnesses on FleetTools`.

- [ ] **Step 3: Implement**

In `harness_set.rs`, add to the imports and below `normalize_harnesses`:

```rust
use crate::ipc_error::lock;
use crate::ipc_error::codes::E_NOTFOUND;
use crate::store::{HostRow, Store};
use std::sync::Mutex;
```

```rust
/// Set a host's harness choice: `None` = auto, `Some` = an explicit list
/// (checked and ordered by [`normalize_harnesses`]; `claude` required).
/// Edits fleet state only — the next `plan_sync` follows it. Returns the
/// host's new row. The `set_host_harnesses` MCP tool, `catalog_admin`'s
/// `set_host_harnesses` action and the `catalog_set_host_harnesses`
/// desktop command all land here.
pub fn set_host_harnesses(
    host_alias: &str,
    harnesses: Option<&[String]>,
    store: &Mutex<Store>,
) -> Result<HostRow, IpcError> {
    crate::validate::host_alias(host_alias)?;
    let normalized = harnesses.map(normalize_harnesses).transpose()?;
    let s = lock(store)?;
    s.set_host_harnesses(host_alias, normalized.as_deref())?;
    s.get_host_row(host_alias)?
        .ok_or_else(|| IpcError::new(E_NOTFOUND, format!("host {host_alias} not found")))
}
```

In `admin.rs`, after `SetHostLayersArgs` (line 58):

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetHostHarnessesArgs {
    pub host_alias: String,
    /// `None` = auto; otherwise the harness ids, `claude` among them.
    #[serde(default)]
    pub harnesses: Option<Vec<String>>,
}
```

in `admin_calls!`, after `"set_host_layers" => SetHostLayers(SetHostLayersArgs),` (line 129):

```rust
    "set_host_harnesses" => SetHostHarnesses(SetHostHarnessesArgs),
```

in `run`, after the `AdminCall::SetHostLayers(a) => …` arm (line 214):

```rust
        AdminCall::SetHostHarnesses(a) => json(super::harness_set::set_host_harnesses(
            &a.host_alias,
            a.harnesses.as_deref(),
            store,
        )?),
```

In `params.rs`, after `SetHostLayersParams` (line 1008):

```rust
#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SetHostHarnessesParams {
    /// The host.
    pub host_alias: String,
    /// null = auto; else harness ids, "claude" required.
    #[serde(default)]
    pub harnesses: Option<Vec<String>>,
}
```

and in `CatalogAdminParams`'s `action` doc (line 1016) change `list_layers|resolve_preview|propose_layers|set_host_layers|` to `list_layers|resolve_preview|propose_layers|set_host_layers|set_host_harnesses|`.

In `mcp/tools/assets.rs`, after `set_host_layers` (after line 331, inside the `#[tool_router]` impl):

```rust
    #[tool(description = "Choose which harnesses the asset catalog syncs \
        on one host. harnesses null = auto: Claude, plus Codex where a scan \
        finds the codex CLI or ~/.codex, or where fleet already manages \
        Codex assets. Otherwise a list that must include \"claude\"; \
        [\"claude\"] turns Codex off, and the next sync then removes what \
        fleet installed for Codex there. Edits fleet state only. Master \
        token only.")]
    pub(super) async fn set_host_harnesses(
        &self,
        Parameters(p): Parameters<SetHostHarnessesParams>,
    ) -> Result<CallToolResult, McpError> {
        // Master-only, like set_host_layers (`guard::TOOL_POLICIES`): which
        // harnesses a host serves decides what the NEXT apply_sync writes to
        // — or removes from — its filesystem.
        audit(
            "set_host_harnesses",
            &format!("host_alias={} harnesses={:?}", p.host_alias, p.harnesses),
        );
        let row = catalog::harness_set::set_host_harnesses(
            &p.host_alias,
            p.harnesses.as_deref(),
            &self.store,
        )
        .map_err(to_mcp_err)?;
        ok_json(&row)
    }
```

In `mcp/guard.rs`, after the `set_host_layers` row (line 925):

```rust
    // Multi-harness F3a: which harnesses a host serves decides what the NEXT
    // apply_sync writes to (or removes from) its filesystem — the same
    // reasoning as set_host_layers.
    ToolPolicy {
        name: "set_host_harnesses",
        access: Access::Master,
        readonly: false,
        confirm: false,
        deadline: Deadline::Quick,
    },
```

- [ ] **Step 4: Run the tests, regenerate the reference, re-measure the budget**

Run:
```bash
export CARGO_TARGET_DIR=<shared-target-dir>
cargo test -p fleet-core --lib service::catalog::harness_set service::catalog::admin mcp::tools::tests_catalog_admin mcp::tools::tests::fleet_admin_tools_are_master_only mcp::tools::tests::layer_read_tools mcp::tools::tests::router_sum_serves_every_tool mcp::tools::tests::every_router_tool_has_exactly_one_tool_policy_row 2>&1 | tail -8
REGEN_DOCS=1 cargo test -p fleet-core reference_is_current 2>&1 | tail -3
cargo test -p fleet-core --lib mcp::tools::tests::the_served_definition_budget_stays_bounded -- --nocapture 2>&1 | grep -E '^master:|test result|panicked'
```
Expected: the listed tests PASS (`the_action_param_names_every_admin_call` in `tests_catalog_admin` included); `docs/control-api-reference.md` gains a `set_host_harnesses` section and the `catalog_admin` action list names `set_host_harnesses`; the budget test prints `master: 103 tools / <B> bytes …` and FAILS because `<B>` exceeds 68,619.

Then in `mcp/tools/tests.rs` set `const BUDGET_BYTES: usize = <B + 100>;` (the printed master byte count plus 100) and append to its doc comment, before the `const` line: "Measured at <B> on 2026-09-30 after multi-harness F3a (`set_host_harnesses` and its `catalog_admin` action)." — with the real number written in, not the letter. Re-run:

```bash
cargo test -p fleet-core --lib mcp::tools::tests::the_served_definition_budget_stays_bounded mcp::doc_gen 2>&1 | tail -3
```
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/service/catalog/harness_set.rs crates/fleet-core/src/service/catalog/admin.rs crates/fleet-core/src/mcp docs/control-api-reference.md
git commit -m "feat(mcp): set_host_harnesses chooses a host's Codex sync (auto / on / off)"
```

---

### Task 5: Desktop — routed command and Host detail's Codex control

**Files:**
- Modify: `src-tauri/src/commands/assets.rs:16-37` (imports), `:110-117` (command), `:463-481` (routed fn)
- Modify: `src-tauri/src/lib.rs:536` (register)
- Modify: `src-tauri/src/backend/verdicts.rs:1076-1081` (row)
- Modify: `src-tauri/src/backend/tests_routing.rs:4346-4351` (imports), `:4458-4476` (case)
- Regenerate: `src/lib/hub_verdicts.generated.json` (and `docs/hub.md`'s generated table if it moves)
- Modify: `src/lib/hosts.ts:6-38` (field), after `hideHost` `:146-156` (helpers)
- Modify: `src/lib/hub.ts:297-300` (`ROUTED_ACTIONS`)
- Modify: `src/lib/HostDetail.svelte:10` (import), after `:181` (derived + handler), `:486-487` (control)
- Test: `src/lib/hosts.test.ts`, `src/lib/HostDetail.test.ts`

**Interfaces:**
- Consumes: `admin::SetHostHarnessesArgs`, `AdminCall::SetHostHarnesses`, `harness_set::set_host_harnesses` (Task 4); `HostRow.harnesses` (Task 1).
- Produces:
  - Tauri command `catalog_set_host_harnesses { args: { host_alias, harnesses } } -> HostRow`, verdict `Routed { tool: "catalog_admin" }`
  - TS: `HostRow.harnesses?: string[] | null`, `type HarnessMode = 'auto' | 'on' | 'off'`, `codexModeOf(h)`, `harnessesFor(mode)`, `setHostHarnesses(alias, harnesses)`
  - `HostDetail`: `<select data-testid="detail-codex">` with `auto` / `on` / `off`

- [ ] **Step 1: Write the failing tests**

In `src-tauri/src/backend/tests_routing.rs` `catalog_admin_cases()`: add `SetHostHarnessesArgs` to the `fleet_core::service::catalog::admin::{…}` import (line 4348-4349), add `let host_row = payload_of(&super::tests_contract::sample_host());` next to `let layer = …`, and add this case directly after the `catalog_set_host_layers` case (after line 4476):

```rust
        (
            "catalog_set_host_harnesses",
            "catalog_admin",
            json!({ "action": "set_host_harnesses",
                    "args": { "host_alias": "nas", "harnesses": ["claude", "codex"] } }),
            host_row,
            Box::new(|b, s, _| {
                block_on(r::catalog_set_host_harnesses(
                    b,
                    SetHostHarnessesArgs {
                        host_alias: "nas".into(),
                        harnesses: Some(vec!["claude".into(), "codex".into()]),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
```

Append to `src/lib/hosts.test.ts` (and add `setHostHarnesses, codexModeOf, harnessesFor` to its `./hosts` import):

```ts
describe('host harnesses (F3a)', () => {
  it('reads Codex as auto / on / off from the harnesses field', () => {
    expect(codexModeOf({})).toBe('auto');
    expect(codexModeOf({ harnesses: null })).toBe('auto');
    expect(codexModeOf({ harnesses: ['claude', 'codex'] })).toBe('on');
    expect(codexModeOf({ harnesses: ['claude'] })).toBe('off');
  });

  it('a mode stores null for auto and always keeps claude in a list', () => {
    expect(harnessesFor('auto')).toBeNull();
    expect(harnessesFor('on')).toEqual(['claude', 'codex']);
    expect(harnessesFor('off')).toEqual(['claude']);
  });

  it('setHostHarnesses sends the list and merges the answered row', async () => {
    hosts.set([sampleLocal]);
    const answered = { ...sampleLocal, harnesses: ['claude', 'codex'] };
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(answered);
    const r = await setHostHarnesses('local', ['claude', 'codex']);
    expect(r.ok).toBe(true);
    expect((mockedInvoke as ReturnType<typeof vi.fn>).mock.calls[0]).toEqual([
      'catalog_set_host_harnesses',
      { args: { host_alias: 'local', harnesses: ['claude', 'codex'] } },
    ]);
    expect(get(hosts)[0].harnesses).toEqual(['claude', 'codex']);
  });
});
```

In `src/lib/HostDetail.test.ts`, add after the `vi.mock('./sessions', …)` block:

```ts
vi.mock('./hosts', async () => {
  const actual = await vi.importActual<typeof import('./hosts')>('./hosts');
  return { ...actual, setHostHarnesses: vi.fn() };
});
```

change the `./hosts` import (line 24) to `import { hostFilter, setHostHarnesses } from './hosts';`, add `const mockedSetHarnesses = setHostHarnesses as unknown as ReturnType<typeof vi.fn>;` next to the other mocked consts, and append:

```ts
describe('HostDetail Codex assets (F3a)', () => {
  beforeEach(() => mockedSetHarnesses.mockReset());
  afterEach(() => {
    hubStatus.set({ ...STANDALONE });
    hubConnection.set({ state: 'standalone' });
  });

  it('a host with no choice reads auto, and picking on sends the explicit list', async () => {
    mockedSetHarnesses.mockResolvedValueOnce({ ok: true, value: host('mefistos', { harnesses: ['claude', 'codex'] }) });
    mount('mefistos');
    const sel = screen.getByTestId('detail-codex') as HTMLSelectElement;
    expect(sel.value).toBe('auto');
    await fireEvent.change(sel, { target: { value: 'on' } });
    expect(mockedSetHarnesses).toHaveBeenCalledWith('mefistos', ['claude', 'codex']);
  });

  it('an explicit list without codex reads off, and auto sends null', async () => {
    mockedSetHarnesses.mockResolvedValueOnce({ ok: true, value: host('mefistos') });
    mount('mefistos', { host: host('mefistos', { harnesses: ['claude'] }) });
    const sel = screen.getByTestId('detail-codex') as HTMLSelectElement;
    expect(sel.value).toBe('off');
    await fireEvent.change(sel, { target: { value: 'auto' } });
    expect(mockedSetHarnesses).toHaveBeenCalledWith('mefistos', null);
  });

  it('an offline paired desktop cannot change it and says why', () => {
    hubStatus.set({ ...STANDALONE, remote: true, url: 'https://hub.example' });
    hubConnection.set({ state: 'offline', attempt: 1, retry_in_secs: 5, reason: 'refused' });
    mount('mefistos');
    const sel = screen.getByTestId('detail-codex') as HTMLSelectElement;
    expect(sel.disabled).toBe(true);
    expect(sel.title).toContain('https://hub.example');
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run:
```bash
pnpm test src/lib/hosts.test.ts src/lib/HostDetail.test.ts 2>&1 | tail -15
export CARGO_TARGET_DIR=<shared-target-dir>
cargo test -p claude-fleet --lib backend::tests_routing 2>&1 | tail -10
```
Expected: Vitest FAILS (`setHostHarnesses is not a function` / `codexModeOf is not a function`, no `detail-codex` element); cargo fails to compile (`cannot find function catalog_set_host_harnesses in module r`).

- [ ] **Step 3: Implement**

`src-tauri/src/commands/assets.rs`: add `SetHostHarnessesArgs` to the `admin::{…}` import (line 19-20) and `HostRow` to the `fleet_core::store::{…}` import (line 36-37). After `catalog_set_host_layers` (line 117):

```rust
#[tauri::command]
pub async fn catalog_set_host_harnesses(
    backend: State<'_, Arc<FleetBackend>>,
    args: SetHostHarnessesArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<HostRow, IpcError> {
    routed::catalog_set_host_harnesses(&backend, args, &store).await
}
```

and in `mod routed`, after `routed::catalog_set_host_layers` (line 481):

```rust
    pub async fn catalog_set_host_harnesses(
        backend: &FleetBackend,
        args: SetHostHarnessesArgs,
        store: &Mutex<Store>,
    ) -> Result<HostRow, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route(
                    "catalog_set_host_harnesses",
                    &AdminCall::SetHostHarnesses(args),
                )
                .await
            }
            None => catalog::harness_set::set_host_harnesses(
                &args.host_alias,
                args.harnesses.as_deref(),
                store,
            ),
        }
    }
```

`src-tauri/src/lib.rs`, after line 536: `            commands::assets::catalog_set_host_harnesses,`

`src-tauri/src/backend/verdicts.rs`, after the `catalog_set_host_layers` row (line 1081):

```rust
    (
        "catalog_set_host_harnesses",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
```

`src/lib/hosts.ts` — in `HostRow`, after `provision_stale?: boolean;`:

```ts
  /** Harnesses the asset catalog syncs here (multi-harness F3a): null or absent
   *  = auto (Claude, plus Codex where a scan finds it); a list always holds claude. */
  harnesses?: string[] | null;
```

after `hideHost`:

```ts
/** Codex's place in a host's harness set: `auto` = `harnesses` null (Codex
 *  where the scan finds it), `on` / `off` = an explicit list. */
export type HarnessMode = 'auto' | 'on' | 'off';

export function codexModeOf(h: Pick<HostRow, 'harnesses'>): HarnessMode {
  if (h.harnesses == null) return 'auto';
  return h.harnesses.includes('codex') ? 'on' : 'off';
}

/** The `harnesses` value a mode stores; Claude is always in an explicit list. */
export function harnessesFor(mode: HarnessMode): string[] | null {
  if (mode === 'auto') return null;
  return mode === 'on' ? ['claude', 'codex'] : ['claude'];
}

export async function setHostHarnesses(alias: string, harnesses: string[] | null): Promise<Result<HostRow>> {
  const r = await invokeCmd<HostRow>('catalog_set_host_harnesses', {
    args: { host_alias: alias, harnesses },
  });
  if (r.ok) rows.accept(r.value);
  return r;
}
```

`src/lib/hub.ts` — in `ROUTED_ACTIONS`, after `'catalog_import_host',` (line 300):

```ts
  // Multi-harness F3a: a host's Codex choice is a catalog_admin action too.
  'catalog_set_host_harnesses',
```

`src/lib/HostDetail.svelte` — replace line 10 with:

```ts
  import { deleteHost, setHostHarnesses, codexModeOf, harnessesFor, type HarnessMode } from './hosts';
```

after `const hostTokensBlocked = …` (line 181):

```ts
  // Which harnesses the asset catalog syncs here (F3a) routes to the hub's
  // catalog_admin, so a paired desktop only needs the live link.
  const harnessBlocked = $derived(hubActionBlocked('catalog_set_host_harnesses', $hubStatus, $hubConnection));

  async function onCodexMode(mode: HarnessMode) {
    busy = true;
    const r = await setHostHarnesses(host.alias, harnessesFor(mode));
    busy = false;
    if (!r.ok) pushError(r.error, 'Codex setting not changed');
  }
```

and in the Integration section, directly after `<h3>Integration</h3>` (line 487):

```svelte
    <div class="kv">
      <span
        class="label"
        title="Which harnesses the asset catalog syncs here. auto = Codex where the codex CLI or ~/.codex is found; off = the next sync removes what fleet installed for Codex"
        >Codex</span
      >
      <select
        value={codexModeOf(host)}
        disabled={busy || harnessBlocked !== null}
        title={harnessBlocked ?? ''}
        aria-label="Codex assets"
        data-testid="detail-codex"
        onchange={(e) => onCodexMode((e.currentTarget as HTMLSelectElement).value as HarnessMode)}
      >
        <option value="auto">auto</option>
        <option value="on">on</option>
        <option value="off">off</option>
      </select>
    </div>
```

- [ ] **Step 4: Regenerate verdicts, run the tests to verify they pass**

Run:
```bash
export CARGO_TARGET_DIR=<shared-target-dir>
REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen 2>&1 | tail -3
cargo test -p claude-fleet --lib backend:: 2>&1 | tail -5
git diff --stat src/lib/hub_verdicts.generated.json docs/hub.md
pnpm test src/lib/hosts.test.ts src/lib/HostDetail.test.ts src/lib/hub_verdicts.test.ts src/lib/hub_disabled.test.ts 2>&1 | tail -8
pnpm check 2>&1 | tail -3
```
Expected: `hub_verdicts.generated.json` gains `"catalog_set_host_harnesses"` under `routed` (the `docs/hub.md` refusal table is unchanged — the command is not a refusal); `backend::` tests PASS (routing covers the new case, every `lib.rs` handler has a verdict); Vitest PASS; `svelte-check` reports 0 errors.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/commands/assets.rs src-tauri/src/lib.rs src-tauri/src/backend/verdicts.rs src-tauri/src/backend/tests_routing.rs src/lib/hub_verdicts.generated.json src/lib/hosts.ts src/lib/hub.ts src/lib/HostDetail.svelte src/lib/hosts.test.ts src/lib/HostDetail.test.ts
git commit -m "feat(ui): Codex auto / on / off in Host detail"
```

---

### Task 6: Codex subagents — render, scan, installed; cross-kind lint

**Files:**
- Modify: `crates/fleet-core/src/service/catalog/harness/codex.rs:1-12` (module doc), `:23-27` (const), `:137-229` (`render`), `:245-247` (scan dirs), `:278-336` (`installed`, `installed_detail`), tests (`:530-550` changed; new tests)
- Modify: `crates/fleet-core/src/service/catalog/inventory.rs:671-687` (changed assertion in `compute_states_covers_all_five_states`)
- Modify: `crates/fleet-core/src/service/catalog/sync/apply.rs` tests (new helper + e2e test after `a_file_dropped_from_an_asset_is_deleted_with_the_update`, ~line 2302)
- Modify: `crates/fleet-core/src/service/catalog/author.rs:14` (import), lint after `:378`, helper fn, tests after `:1331`

**Interfaces:**
- Consumes: `TargetOverride { model, render_as, extra }`, `Asset::install_name()`, `strip_nulls` (codex.rs).
- Produces:
  - `pub const CODEX_AGENTS_DIR: &str = "~/.codex/agents";`
  - `Codex::render(Agent)` → one `FileWrite { path: "~/.codex/agents/<install_name>.toml" }` whose TOML has `name`, `description`, `developer_instructions`, optional `model`, then `targets.codex.extra`; `render_as: skill` unchanged
  - Codex scan hashes `.codex/agents`; `installed()` lists `(Kind::Agent, <stem>)` for `~/.codex/agents/<stem>.toml`; `installed_detail` hashes it
  - lint error when a Codex-enabled skill and a Codex `render_as: skill` agent share an install name

- [ ] **Step 1: Write the failing tests (and change the three existing assertions)**

In `codex.rs` tests, **change** `hooks_and_plugins_are_unsupported_agents_unless_render_as_skill` (lines 530-550). Reason: F3b makes a plain agent renderable, so its `assert!(Codex.render(&agent).is_err());` no longer holds. Rename it and replace only that assertion; every other assertion stays:

```rust
    #[test]
    fn hooks_and_plugins_are_unsupported_and_render_as_skill_still_wins() {
        let hook = Asset::from_yaml(None, "kind: hook\nname: h\ndescription: d\nevent: stop\naction: { type: command, command: x }\n").unwrap();
        assert!(Codex.render(&hook).is_err());
        let plugin = Asset::from_yaml(None, "kind: plugin_ref\nname: p\ndescription: d\nharness: claude\nmarketplace: { name: m, source: github, repo: o/r }\nplugin: p\nversion: latest\n").unwrap();
        assert!(Codex.render(&plugin).is_err());
        let agent = Asset::from_yaml(None, "kind: agent\nname: pm\ndescription: d\n").unwrap();
        assert_eq!(
            Codex.render(&agent).unwrap().files[0].path,
            "~/.codex/agents/pm.toml",
            "since F3b a plain agent is a Codex subagent"
        );
        let mut as_skill = Asset::from_yaml(
            None,
            "kind: agent\nname: pm\ndescription: d\ntargets:\n  codex:\n    render_as: skill\n",
        )
        .unwrap();
        as_skill.body = "prompt\n".into();
        let plan = Codex.render(&as_skill).unwrap();
        assert_eq!(plan.files[0].path, "~/.codex/skills/pm/SKILL.md");
        assert_eq!(
            plan.warnings,
            vec!["agent rendered as a codex skill (targets.codex.render_as)"]
        );
    }
```

Append to `codex.rs` tests:

```rust
    fn toml_of(plan: &RenderPlan) -> toml::Table {
        toml::from_str(std::str::from_utf8(&plan.files[0].bytes).unwrap()).expect("valid TOML")
    }

    #[test]
    fn agent_renders_a_codex_subagent_toml() {
        let mut a = Asset::from_yaml(None, "kind: agent\nname: pm\ndescription: Plans the work.\n").unwrap();
        a.body = "You plan.\nStep by step.\n".into();
        let plan = Codex.render(&a).unwrap();
        assert_eq!(plan.files.len(), 1);
        assert_eq!(plan.files[0].path, "~/.codex/agents/pm.toml");
        let mut want = toml::Table::new();
        want.insert("name".into(), "pm".into());
        want.insert("description".into(), "Plans the work.".into());
        want.insert("developer_instructions".into(), "You plan.\nStep by step.\n".into());
        assert_eq!(toml_of(&plan), want, "no model unless targets.codex.model");
        assert!(plan.warnings.is_empty());
        assert!(plan.merges.is_empty());
    }

    #[test]
    fn agent_model_and_extra_come_from_targets_codex_only() {
        let mut a = Asset::from_yaml(
            None,
            "kind: agent\nname: pm\ndescription: d\ntools: [read, bash]\nmodel: strong\ntargets:\n  codex:\n    model: gpt-5.4\n    extra:\n      model_reasoning_effort: high\n      sandbox_mode: read-only\n      dropped: null\n",
        )
        .unwrap();
        a.body = "b\n".into();
        let plan = Codex.render(&a).unwrap();
        let v = toml_of(&plan);
        assert_eq!(v["model"].as_str(), Some("gpt-5.4"), "the tier is never mapped for codex");
        assert_eq!(v["model_reasoning_effort"].as_str(), Some("high"));
        assert_eq!(v["sandbox_mode"].as_str(), Some("read-only"));
        assert!(v.get("dropped").is_none());
        assert!(v.get("tools").is_none());
        assert_eq!(
            plan.warnings,
            vec![
                CODEX_AGENT_TOOLS_WARNING.to_string(),
                "targets.codex.extra.dropped has no TOML form; not written".to_string(),
            ]
        );
    }

    /// The TOML comes from the toml crate, so nothing in a description or
    /// prompt can close a string and inject a key.
    #[test]
    fn agent_toml_escapes_whatever_the_text_holds() {
        let mut a = Asset::from_yaml(
            None,
            "kind: agent\nname: pm\ndescription: 'Says \"hi\" = [x]'\n",
        )
        .unwrap();
        a.body = "'''\n\"\"\"\nname = \"evil\"\n".into();
        let v = toml_of(&Codex.render(&a).unwrap());
        assert_eq!(v["name"].as_str(), Some("pm"));
        assert_eq!(v["description"].as_str(), Some("Says \"hi\" = [x]"));
        assert_eq!(
            v["developer_instructions"].as_str(),
            Some("'''\n\"\"\"\nname = \"evil\"\n")
        );
    }

    #[test]
    fn agent_renders_under_install_as_keeping_the_catalog_name() {
        let mut a = Asset::from_yaml(None, "kind: agent\nname: pm\ndescription: d\ninstall_as: pm_agent\n").unwrap();
        a.body = "b\n".into();
        let plan = Codex.render(&a).unwrap();
        assert_eq!(plan.files[0].path, "~/.codex/agents/pm_agent.toml");
        assert_eq!(toml_of(&plan)["name"].as_str(), Some("pm"));
    }

    #[test]
    fn a_codex_disabled_agent_renders_nothing() {
        let a = Asset::from_yaml(
            None,
            "kind: agent\nname: pm\ndescription: d\ntargets:\n  codex:\n    enabled: false\n",
        )
        .unwrap();
        let plan = Codex.render(&a).unwrap();
        assert!(plan.files.is_empty());
        assert_eq!(plan.warnings, vec!["disabled for codex by targets.codex.enabled"]);
    }

    #[test]
    fn installed_lists_agents_and_detail_hashes_their_toml() {
        let mut s = HostSnapshot::default();
        s.files.insert(format!("{CODEX_AGENTS_DIR}/pm.toml"), "ab".into());
        s.files.insert(format!("{CODEX_AGENTS_DIR}/notes.md"), "cd".into());
        s.files.insert(format!("{CODEX_AGENTS_DIR}/nested/x.toml"), "ef".into());
        assert_eq!(Codex.installed(&s), vec![(Kind::Agent, "pm".to_string())]);
        let d = Codex.installed_detail(&s);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].hash.as_deref(), Some("ab"));
        assert!(!d[0].secret_like && !d[0].fleet_owned);
    }

    /// The real scan hashes `~/.codex/agents`, and `installed` reads the
    /// agent back from it.
    #[cfg(unix)]
    #[test]
    fn scan_script_hashes_codex_agents_under_bash() {
        let tmp = tempfile::TempDir::new().unwrap();
        let agents = tmp.path().join(".codex/agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(agents.join("pm.toml"), b"name = \"pm\"\n").unwrap();
        let out = std::process::Command::new("bash")
            .arg("-lc")
            .arg(Codex.scan_script().unwrap())
            .env("HOME", tmp.path())
            .output()
            .expect("run scan script");
        assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
        let snap = Codex.parse_scan(&String::from_utf8(out.stdout).unwrap()).unwrap();
        assert!(snap.files.contains_key("~/.codex/agents/pm.toml"), "{:?}", snap.files);
        assert_eq!(Codex.installed(&snap), vec![(Kind::Agent, "pm".to_string())]);
    }
```

In `inventory.rs` `compute_states_covers_all_five_states`, **change** the Codex half (lines 681-684). Reason: the agent `gone` now renders for Codex, so it is `missing`, not `unsupported`; the hook `h` keeps the test's coverage of `unsupported`. Replace

```rust
        assert_eq!(
            rows.iter().find(|r| r.name == "gone").unwrap().state,
            "unsupported"
        );
```

with

```rust
        assert_eq!(
            rows.iter().find(|r| r.name == "h").unwrap().state,
            "unsupported",
            "codex renders no hooks"
        );
        assert_eq!(
            rows.iter().find(|r| r.name == "gone").unwrap().state,
            "missing",
            "codex renders agents since F3b"
        );
```

In `sync/apply.rs` tests, after `a_file_dropped_from_an_asset_is_deleted_with_the_update` (line 2302):

```rust
    #[cfg(unix)]
    async fn codex_plan_for(ssh: &Arc<SshClient>, catalog: &Catalog) -> HostPlan {
        use crate::service::catalog::harness::codex::Codex;
        let snap = inventory::scan_host_harness(ssh, "local", &Codex)
            .await
            .expect("scan");
        let manifest = Manifest::from_snapshot(&snap, Codex.manifest_path());
        plan::compute_host_plan(
            catalog,
            &Codex,
            "local",
            &snap,
            &manifest,
            &BTreeMap::new(),
            &PlanFilter::default(),
        )
    }

    /// F3b end to end against a real temp `$HOME`, through the real Codex
    /// scan and planner: a catalog agent is written as
    /// `~/.codex/agents/pm.toml`, a second plan is a no-op, and dropping it
    /// from the catalog removes the file and its manifest entry.
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_codex_agent_is_written_as_toml_and_removed_locally() {
        use crate::service::catalog::harness::codex::Codex;
        let _lock = crate::service::catalog::CATALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());
        let repo_dir = tempfile::tempdir().unwrap();
        let catalog = write_catalog(
            repo_dir.path(),
            &[
                (
                    "agents/pm/asset.yaml",
                    "kind: agent\nname: pm\ndescription: Plans the work.\ntargets:\n  codex:\n    model: gpt-5.4\n",
                ),
                ("agents/pm/prompt.md", "You plan \"carefully\".\n"),
            ],
        );
        let ssh = Arc::new(SshClient::new());
        let ctx = ApplyCtx {
            ssh: &ssh,
            token: CancellationToken::new(),
            now: 1_000,
        };
        let agent_file = home.path().join(".codex/agents/pm.toml");
        let manifest_file = home.path().join(".codex/.fleet-assets.json");

        // 1. Create.
        let create = codex_plan_for(&ssh, &catalog).await;
        assert_eq!(create.actions.len(), 1);
        assert_eq!(create.actions[0].op, ActionOp::Create);
        assert_eq!(create.actions[0].files, vec!["~/.codex/agents/pm.toml".to_string()]);
        let res = apply_host(&ctx, &Codex, &create).await;
        assert_eq!(res.status, "applied", "{res:?}");
        assert_eq!(res.actions[0].outcome, DONE);
        let v: toml::Table =
            toml::from_str(&std::fs::read_to_string(&agent_file).expect("agent written"))
                .expect("valid TOML");
        assert_eq!(v["name"].as_str(), Some("pm"));
        assert_eq!(v["description"].as_str(), Some("Plans the work."));
        assert_eq!(v["developer_instructions"].as_str(), Some("You plan \"carefully\".\n"));
        assert_eq!(v["model"].as_str(), Some("gpt-5.4"));
        let manifest: Manifest =
            serde_json::from_str(&std::fs::read_to_string(&manifest_file).unwrap()).unwrap();
        assert_eq!(
            manifest.assets["agent/pm"].files,
            vec!["~/.codex/agents/pm.toml".to_string()]
        );

        // 2. Nothing left to do.
        let again = codex_plan_for(&ssh, &catalog).await;
        assert_eq!(again.actions[0].op, ActionOp::Noop);

        // 3. The agent leaves the catalog: removed, with its manifest entry.
        let removal = codex_plan_for(&ssh, &Catalog::default()).await;
        assert_eq!(removal.actions[0].op, ActionOp::Remove);
        let res = apply_host(&ctx, &Codex, &removal).await;
        assert_eq!(res.status, "applied", "{res:?}");
        assert!(!agent_file.exists(), "the agent file is gone");
        let manifest: Manifest =
            serde_json::from_str(&std::fs::read_to_string(&manifest_file).unwrap()).unwrap();
        assert!(manifest.assets.is_empty(), "{manifest:?}");
    }
```

In `author.rs` tests, after `lint_allows_the_same_install_name_across_kinds` (line 1331):

```rust
    /// F3b: an agent Codex renders as a skill lands in the same
    /// `~/.codex/skills/<install name>/` as a skill of that install name —
    /// reported from both sides; a skill with Codex disabled does not collide.
    #[test]
    fn lint_errors_when_a_codex_skill_agent_shares_a_skills_install_name() {
        let mut skill = clean_skill();
        skill.header.name = "pm".into();
        let mut agent = Asset::from_yaml(
            None,
            "kind: agent\nname: pm\ndescription: A reasonably long description here.\ntools: [read]\ntargets:\n  codex:\n    render_as: skill\n",
        )
        .unwrap();
        agent.body = "You plan.\n".into();
        let catalog = Catalog {
            assets: vec![skill.clone(), agent.clone()],
            ..Default::default()
        };
        let report = lint(&agent, &catalog, &[], true);
        assert_eq!(fields(&report.errors), vec!["name"], "{:?}", report.errors);
        assert!(
            report.errors[0].message.contains("~/.codex/skills/pm") && report.errors[0].message.contains("skill/pm"),
            "{:?}",
            report.errors
        );
        let report = lint(&skill, &catalog, &[], true);
        assert_eq!(fields(&report.errors), vec!["name"], "{:?}", report.errors);
        assert!(report.errors[0].message.contains("agent/pm"), "{:?}", report.errors);

        let mut off = skill.clone();
        off.header.targets.insert(
            "codex".into(),
            TargetOverride {
                enabled: false,
                ..Default::default()
            },
        );
        let catalog = Catalog {
            assets: vec![off, agent.clone()],
            ..Default::default()
        };
        assert!(lint(&agent, &catalog, &[], true).errors.is_empty());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run:
```bash
export CARGO_TARGET_DIR=<shared-target-dir>
cargo test -p fleet-core --lib service::catalog::harness::codex service::catalog::inventory::tests::compute_states_covers_all_five_states service::catalog::sync::apply::tests::a_codex_agent service::catalog::author::tests::lint_ 2>&1 | tail -25
```
Expected: compile errors `cannot find value CODEX_AGENTS_DIR` / `CODEX_AGENT_TOOLS_WARNING`; once those exist, the agent render tests, `compute_states_covers_all_five_states`, the apply e2e (Blocked, not Create) and the new lint test FAIL.

- [ ] **Step 3: Implement**

`codex.rs` module doc, first line: replace `//! Codex CLI renderer (experimental): skills and MCP servers only.` with `//! Codex CLI renderer (experimental): skills, subagents (TOML, one file per agent) and MCP servers.`

After `CODEX_MANIFEST_PATH` (line 25):

```rust
pub const CODEX_AGENTS_DIR: &str = "~/.codex/agents";

/// Render warning for an agent with `tools` (F3b): Codex subagents have no
/// per-agent tool allowlist. Shown in Asset detail's Codex preview.
pub const CODEX_AGENT_TOOLS_WARNING: &str = "codex subagents have no tool allowlist; `tools` is not applied (targets.codex.extra.sandbox_mode can restrict the agent)";

/// A `targets.codex.extra` value as TOML: nulls stripped (TOML has none);
/// `None` when nothing representable is left.
fn json_to_toml(v: &Value) -> Option<toml::Value> {
    let mut v = v.clone();
    strip_nulls(&mut v);
    if v.is_null() {
        return None;
    }
    toml::Value::try_from(&v).ok()
}
```

In `render`, replace the final unsupported arm (lines 224-226) with:

```rust
            AssetSpec::Agent { tools, .. } => {
                // A Codex subagent (F3b): one TOML file per agent with the
                // required `name`, `description` and `developer_instructions`.
                // Built as a table and serialised by the toml crate, so
                // whatever the description or prompt holds is escaped.
                let mut table = toml::Table::new();
                table.insert(
                    "name".into(),
                    toml::Value::String(asset.header.name.clone()),
                );
                table.insert(
                    "description".into(),
                    toml::Value::String(asset.header.description.clone()),
                );
                table.insert(
                    "developer_instructions".into(),
                    toml::Value::String(asset.body.clone()),
                );
                // No tier → model mapping for Codex: without an explicit
                // `targets.codex.model` the user's configured model applies.
                if let Some(model) = &t.model {
                    table.insert("model".into(), toml::Value::String(model.clone()));
                }
                if !tools.is_empty() {
                    plan.warnings.push(CODEX_AGENT_TOOLS_WARNING.into());
                }
                for (k, v) in &t.extra {
                    match json_to_toml(v) {
                        Some(tv) => {
                            table.insert(k.clone(), tv);
                        }
                        None => plan.warnings.push(format!(
                            "targets.codex.extra.{k} has no TOML form; not written"
                        )),
                    }
                }
                let text = toml::to_string_pretty(&table).unwrap_or_default();
                plan.note_placeholders(&text);
                plan.files.push(FileWrite {
                    path: format!("{CODEX_AGENTS_DIR}/{}.toml", asset.install_name()),
                    bytes: text.into_bytes(),
                });
            }
            AssetSpec::Hook { .. } | AssetSpec::PluginRef { .. } => {
                return Err(unsupported());
            }
```

(The `AssetSpec::Agent { .. } if t.render_as.as_deref() == Some("skill")` arm stays above it, so `render_as: skill` still wins.)

In `scan_script`, change the find loop (line 246) from `for d in .codex/skills; do` to `for d in .codex/skills .codex/agents; do`, and update the doc comment's "file hashes under `.codex/skills`" to "file hashes under `.codex/skills` and `.codex/agents`".

In `installed`, inside the `for path in snap.files.keys()` loop, after the skills `if let` (after line 290):

```rust
            if let Some(rest) = path.strip_prefix(&format!("{CODEX_AGENTS_DIR}/")) {
                if let Some(stem) = rest.strip_suffix(".toml") {
                    if !stem.contains('/') {
                        push(Kind::Agent, stem.to_string());
                    }
                }
            }
```

In `installed_detail`, add an arm before `_ => (None, false),` (line 325):

```rust
                    Kind::Agent => (
                        snap.files
                            .get(&format!("{CODEX_AGENTS_DIR}/{name}.toml"))
                            .cloned(),
                        false,
                    ),
```

In `author.rs`, add `use super::harness::codex::CODEX_SKILLS_DIR;` after line 14, the helper before `pub fn lint`:

```rust
/// Whether Codex renders `a` into `~/.codex/skills/`: a skill, or an agent
/// with `targets.codex.render_as: skill` — in both cases only while its
/// Codex target is enabled.
fn lands_in_codex_skills(a: &Asset) -> bool {
    let t = a.target("codex");
    t.enabled
        && match a.kind() {
            Kind::Skill => true,
            Kind::Agent => t.render_as.as_deref() == Some("skill"),
            _ => false,
        }
}
```

and in `lint`, directly after the same-kind install-name block (after line 378):

```rust
    // F3b: Codex renders an agent with `targets.codex.render_as: skill` into
    // `~/.codex/skills/<install name>/`, where a skill of that install name
    // also lands — across kinds, so the rule above cannot see it. Both
    // manifest entries would claim one path, and removing either asset would
    // delete the other's installed copy.
    if lands_in_codex_skills(asset) {
        if let Some(other) = catalog.assets.iter().find(|a| {
            a.kind() != kind && lands_in_codex_skills(a) && a.install_name() == install_name
        }) {
            report.error(
                if asset.header.install_as.is_some() {
                    "install_as"
                } else {
                    "name"
                },
                format!(
                    "install name '{install_name}' also renders to {CODEX_SKILLS_DIR}/{install_name} for {}/{} (targets.codex.render_as: skill)",
                    other.kind().as_str(),
                    other.header.name
                ),
            );
        }
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run:
```bash
export CARGO_TARGET_DIR=<shared-target-dir>
cargo test -p fleet-core --lib service::catalog 2>&1 | tail -8
```
Expected: all `service::catalog` tests PASS — including `lint_allows_the_same_install_name_across_kinds` (its agent has no `render_as`, so it renders to `~/.codex/agents`), `templates_are_clean_except_the_plugin_ref_todos`, `plan::tests::an_unsupported_kind_blocks_and_a_disabled_target_noops` (a hook) and `catalog::tests` previews (a hook).

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/service/catalog/harness/codex.rs crates/fleet-core/src/service/catalog/inventory.rs crates/fleet-core/src/service/catalog/sync/apply.rs crates/fleet-core/src/service/catalog/author.rs
git commit -m "feat(catalog): render agents as Codex subagents in ~/.codex/agents"
```

---

### Task 7: Docs and full verification

**Files:**
- Modify: `docs/concepts.md:93-95`, after `:111` (new paragraph)
- Modify: `docs/control-api.md` (after the "Asset catalog layers" bullet, ~line 370)
- Modify: `docs/hub.md:990`
- Modify: `docs/superpowers/specs/2026-09-14-asset-catalog-design.md:40-41`, `:162`, `:178`
- Modify: `CLAUDE.md` (after the "Assets S1a" bullet, line 182)

**Interfaces:**
- Consumes: everything above. Produces: nothing new.

- [ ] **Step 1: Write the docs**

`docs/concepts.md` line 94: change `(Claude Code fully; Codex CLI for skills and MCP servers),` to `(Claude Code fully; Codex CLI for skills, subagents and MCP servers),`. After the `install_as` paragraph (ends line 111 "…when one is set."), insert:

```markdown
**Harnesses per host.** Claude Code is synced on every host. Codex is synced
only where the host has it: by default (*auto*) where the scan finds the
`codex` CLI on the PATH or a `~/.codex` directory, or where fleet already
manages Codex assets (`~/.codex/.fleet-assets.json` names some). Host
detail's **Codex** control — `auto` / `on` / `off`, the `set_host_harnesses`
tool (`null` = auto, or a list that always includes `claude`) — overrides
it. A host with Codex turned off that still holds what fleet installed for
Codex is *retiring*: its Codex plan only removes those assets, and once
they are gone Codex is neither planned nor listed there. Every reachable
host is still scanned for Codex, since the scan is what detects it. A
catalog agent becomes a Codex subagent at `~/.codex/agents/<install
name>.toml` — `name`, `description`, `developer_instructions` (the
prompt), `model` only when `targets.codex.model` is set (no tier mapping),
plus `targets.codex.extra`; `targets.codex.render_as: skill` renders it as
a Codex skill instead, and the lint refuses such an agent whose install
name a skill also uses. Codex subagents have no tool allowlist, so an
agent's `tools` do not apply there (the Codex preview says so).
```

`docs/control-api.md`, after the "**Asset catalog layers**" bullet:

```markdown
- **Asset catalog harnesses** — `set_host_harnesses` (which harnesses the
  catalog syncs on one host: `harnesses: null` = auto — Claude, plus Codex
  where a scan finds the codex CLI or `~/.codex`, or where fleet already
  manages Codex assets — or a list that must include `"claude"`; with
  Codex turned off, the next sync removes what fleet installed for Codex
  there; edits fleet state only; master token only, for the same reason as
  `set_host_layers`). `catalog_admin`'s `set_host_harnesses` action is the
  same call for a granted desktop.
```

`docs/hub.md` line 990: change `` `remove_host`, `hide_host`, `apply_sync`, `set_secret`, `set_host_layers`, `` to `` `remove_host`, `hide_host`, `apply_sync`, `set_secret`, `set_host_layers`, `set_host_harnesses`, ``.

`docs/superpowers/specs/2026-09-14-asset-catalog-design.md`:
- lines 40-41 become:
  ```markdown
  - Second harness validated now is Codex CLI. The Codex adapter renders skills
    and MCP servers only and is marked experimental; no Codex host scanning.
    (Later: Codex hosts are scanned, and since multi-harness F3b agents render
    as Codex subagents — `docs/superpowers/plans/2026-09-30-f3ab-harness-set-and-codex-agents.md`.)
  ```
- line 162 becomes: `` | agent | `~/.codex/agents/<name>.toml` subagent (`name`, `description`, `developer_instructions`, `model` only from `targets.codex.model`) since multi-harness F3b; `targets.codex.render_as: skill` renders a skill instead | ``
- line 178 becomes: `  codex.rs    skill + agent + mcp_server; experimental`

`CLAUDE.md`, after the "Assets S1a — unmanaged inventory" bullet (ends line 182):

```markdown
- **Multi-harness F3a / F3b** (plan
  `docs/superpowers/plans/2026-09-30-f3ab-harness-set-and-codex-agents.md`):
  `hosts.harnesses` (migration 089, NULL = auto) and the one gate
  `service/catalog/harness_set.rs::harness_gate` decide per host whether
  Codex is planned and inventoried (`plan_sync`, `scan_hosts`, the
  post-apply rescan) — `Off`, `On`, or `Retiring` (turned off but still
  managed: removals only). The Codex scan prints `##PRESENT`
  (`HostSnapshot::present`) and still runs on every reachable host, since it
  is the detection. `set_host_harnesses` (MCP tool, `catalog_admin` action,
  Host detail's Codex control) sets it; `claude` cannot be removed. Codex
  renders agents to `~/.codex/agents/<install name>.toml` via the `toml`
  crate.
```

- [ ] **Step 2: Format, lint, and run the whole suites**

Run:
```bash
export CARGO_TARGET_DIR=<shared-target-dir>
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -5
cargo test -p fleet-core 2>&1 | grep -E '^test result|FAILED|panicked' | tail -15
cargo test -p claude-fleet --lib 2>&1 | tail -3
cargo build -p fleet-hub --locked 2>&1 | tail -2
pnpm test 2>&1 | tail -5
pnpm check 2>&1 | tail -3
```
Expected: fmt shows no diff (if it does, run `cargo fmt --all` and include it in the commit); clippy clean; `fleet-core` all PASS except the known pre-existing `service::rewind::tests::the_removal_script_leaves_a_tree_a_live_pane_is_in` (socket path too long in this scratch dir) — if anything else fails, confirm it also fails on `origin/main` before calling it pre-existing, and say so in the report; `claude-fleet` PASS; fleet-hub builds; Vitest PASS; svelte-check 0 errors.

- [ ] **Step 3: Commit**

```bash
git add docs/concepts.md docs/control-api.md docs/hub.md docs/superpowers/specs/2026-09-14-asset-catalog-design.md CLAUDE.md
git commit -m "docs: per-host harness set and Codex subagents (F3a, F3b)"
```
