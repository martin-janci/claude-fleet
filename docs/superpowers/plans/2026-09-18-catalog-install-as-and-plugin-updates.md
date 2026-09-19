# Catalog install names and plugin updates — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task.

**Goal:** Let an imported asset keep its original host identifier (`install_as`) so sync adopts the file the host already has, and let the planner schedule one `claude plugin update` when the catalog's pinned plugin version changes.

**Architecture:** `Header.install_as` + `Asset::install_name()`; harness renders and inventory use the install name for host paths and config keys; the importer sets it when the slug diverges. Plugin manifest entries record the rendered merge's `value_hash`; `plugin_op` compares it with the current render to decide `plugin_update` vs `blocked`.

**Tech stack:** Rust (`crates/fleet-core/src/service/catalog/**`, `src-tauri/src/commands/assets.rs`), Svelte 5 + TypeScript (`src/lib/*`).

**Spec:** `docs/superpowers/specs/2026-09-18-catalog-install-as-and-plugin-updates-design.md`

## Global Constraints

- Install name rule: `[A-Za-z0-9._-]+`, not `.` or `..`, never on `hook` / `plugin_ref`; validation error code `E_INVALID` at command boundaries, `Asset::validate` message `install_as must match [A-Za-z0-9._-] and not be . or ..` / `install_as is not allowed for <kind>`.
- Manifest keys, inventory rows, plan actions, the UI and the authoring session all keep the catalog `name`; only host paths and config keys use `install_name()`.
- `latest` plugin refs never produce `plugin_update`.
- No `eprintln!`; `Store` mutex never held across an `.await`; scripts contain no single quotes; commit messages end with `Claude-Session: https://claude.ai/code/session_01YBfzniZTvavDZZzCgumQcd`.
- Build/test on this host: `source /home/dev/.local/tauri-sysroot/env.sh && export RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu` (plus cargo on PATH), then `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`; frontend `pnpm run check && pnpm run test && pnpm run build`. Foreground only.

---

### Task 1: `install_as` in the model, validate and lint

**Files:** Modify `crates/fleet-core/src/service/catalog/model.rs` (Header, validate, tests), `crates/fleet-core/src/service/catalog/author.rs` (lint warning + test).

**Produces:** `Header.install_as: Option<String>` (`#[serde(default, skip_serializing_if = "Option::is_none")]`), `pub fn is_valid_install_name(s: &str) -> bool`, `impl Asset { pub fn install_name(&self) -> &str }`.

- [ ] Tests: `install_as_round_trips_through_yaml_and_json` (asset with `install_as: foo_bar` serialises the key, deserialises back, `install_name()` returns `foo_bar`; without it the key is absent and `install_name()` is `name`); `validate_rejects_bad_install_as` table: `Some("")`, `Some("a b")`, `Some("..")`, `Some("a/b")`, and `Some("ok")` on a hook and on a plugin_ref → errors with the constraint messages; `Some("foo_bar")` on skill/agent/mcp_server → clean.
- [ ] Implement; keep the on-disk `AssetFile` serializer emitting the key only when set (it reuses `merge_header_and_spec`, so nothing extra is needed — assert it in the round-trip test by checking `to_yaml` output).
- [ ] Lint (`author::lint`): warning `install_as equals name` when they match; test.
- [ ] `cargo test -p fleet-core service::catalog::model service::catalog::author`; commit `feat(catalog): install_as header field`.

### Task 2: harness renders and inventory use the install name

**Files:** Modify `crates/fleet-core/src/service/catalog/harness/claude.rs` (skill dir ~line 162, agent path ~203, `mcpServers` key ~321), `harness/codex.rs` (skill dir ~153/164, `mcp_servers` key ~202), `inventory.rs` (`compute_states` unmanaged loop ~line 462).

- [ ] Tests (claude.rs): `skill_renders_under_install_as` (`install_as: foo_bar` → `~/.claude/skills/foo_bar/SKILL.md`, frontmatter `name` stays the catalog name), `agent_renders_under_install_as`, `mcp_server_merges_under_install_as` (json_path `["mcpServers","claude_ai_Docs"]`); codex.rs: skill dir and `mcp_servers` key likewise.
- [ ] Tests (inventory.rs): `install_as_matches_the_installed_identifier`: snapshot with `~/.claude/skills/foo_bar/SKILL.md` at the rendered hash, catalog `skill/foo-bar` with `install_as: foo_bar` → row `foo-bar` `in_sync`, no `unmanaged` row for `foo_bar`; the same snapshot with the catalog asset lacking `install_as` → `foo-bar` `missing` and `foo_bar` `unmanaged`.
- [ ] Implement: replace `a.header.name` with `a.install_name()` at the five path/key sites only (frontmatter `name` fields keep the catalog name); in `compute_states` build `BTreeSet<(Kind, String)>` of `(asset.kind(), asset.install_name())` before the `installed()` loop and test membership against it instead of `catalog.find(kind, &name)`.
- [ ] `cargo test -p fleet-core service::catalog`; commit `feat(catalog): render and inventory by install name`.

### Task 3: importer sets `install_as` and reports naming warnings

**Files:** Modify `crates/fleet-core/src/service/catalog/import.rs` (`ImportReport`, `slug_or_problem`, skill/agent/MCP construction sites ~257, ~300, ~487/537), `src/lib/assets.ts` (`ImportReport.warnings?: Problem[]`), `src/lib/ImportDialog.svelte` (Warnings list, test id `import-warnings`).

**Produces:** `ImportReport.warnings: Vec<Problem>` (serde default), `fn install_as_for(original: &str, slug: &str) -> Option<String>` (Some(original) when `original != slug && is_valid_install_name(original)`).

- [ ] Tests (import.rs, extend the existing temp-`~/.claude` fixture): a skill dir `foo_bar` → asset `skill/foo-bar` with `install_as: foo_bar`; an agent file `PM Review.md` → `agent/pm-review`, no `install_as`, one warning `agent pm-review: installs under a new name; PM Review stays unmanaged`; an MCP server key `claude_ai_Docs` → `install_as: claude_ai_Docs`; kebab originals → no `install_as`, no warning; hooks and plugin refs never carry `install_as`.
- [ ] Implement; warnings go in `report.warnings`, never in `problems` (the asset is created).
- [ ] Frontend: type + dialog rendering (`{#if report.warnings?.length}` under heading "Warnings"); one Vitest case in `ImportDialog.test.ts` (create if absent) asserting the list renders.
- [ ] `cargo test -p fleet-core service::catalog::import`, `pnpm run test -- src/lib/ImportDialog.test.ts`; commit `feat(catalog): importer keeps the host identifier as install_as`.

### Task 4: plugin updates when the pin changes

**Files:** Modify `crates/fleet-core/src/service/catalog/sync/plan.rs` (`plugin_op` ~line 539 and its call ~430), `sync/apply.rs` (`plugin_entry` ~468 and its call ~1254), `sync/manifest.rs` (helper only if needed), `docs/concepts.md`, `docs/control-api.md`.

- [ ] Tests (plan.rs, next to `plugin_ops_install_adopt_and_block_on_a_pin`): `pin_change_schedules_a_plugin_update` — installed `5.0.0`, catalog pins `6.0.0`, manifest entry for the plugin with `value_hash = value_hash(&json!([{"version":"5.0.0"}]))` → `PluginUpdate`, reason contains `catalog pin changed to 6.0.0`; `unchanged_pin_stays_blocked` — same but entry hash equals `value_hash(&json!([{"version":"6.0.0"}]))` → `Blocked`; `legacy_empty_hash_updates_once` — entry with empty `value_hash` → `PluginUpdate`; `latest_never_updates` — catalog `latest`, installed anything → `Noop` when in manifest, `Adopt` otherwise.
- [ ] Tests (apply.rs): the manifest entry written for an install/adopt/update carries `merges[0].value_hash == value_hash(&<rendered plugin merge value>)` (extend the existing manifest-writing test for plugins).
- [ ] Implement planner: `plugin_op(plan, snap, target, entry: Option<&ManifestEntry>)`; the caller passes `manifest.assets.get(&key)`. Decide `PluginUpdate` when the render's `value_hash(&merge.value)` differs from `entry.merges.iter().find(|m| m.file == PLUGINS_PATH).map(|m| m.value_hash.as_str())` or that is `None`/empty. Ensure plugin actions carry `plan: Some(secret_plan)` (check the plugin branch at ~430; attach the render plan if it does not).
- [ ] Implement applier: `plugin_entry(target, merge_value: &serde_json::Value, now)` sets `value_hash: value_hash(merge_value)`; at the call site take the value from `action.plan.as_ref().and_then(|p| p.inner().merges.first()).map(|m| &m.value)`, falling back to `json!([{}])` for `latest` and `json!([{"version": target.version}])` otherwise (must equal `harness/claude.rs` ~344's render).
- [ ] Docs: `docs/concepts.md` sync paragraph — "A pinned plugin is updated through the harness CLI only when the catalog's pin changes; `latest` refs are never updated automatically." `docs/control-api.md` `plan_sync` bullet — add `plugin_update` to the listed ops if ops are enumerated there.
- [ ] `cargo test -p fleet-core service::catalog::sync`; commit `feat(sync): update a pinned plugin when the catalog pin changes`.

### Task 5: editor and detail show `install_as`; docs

**Files:** Modify `src/lib/assets.ts` (`EditableAsset.install_as?: string | null`), `src/lib/AssetEditor.svelte` (+test), `src/lib/AssetDetail.svelte` (+test if one exists), `docs/concepts.md`.

- [ ] Tests: `AssetEditor.test.ts` — for a skill, input `editor-install-as` renders with the current value, editing it marks the draft changed and Save sends `install_as` in the asset (empty input sends the key absent/null → verify the Rust side treats `null` as `None`, which serde does); for a hook the input is not rendered. `AssetDetail`: shows `installs as foo_bar` (`asset-install-as`) when set, nothing otherwise.
- [ ] Implement; client-side check mirrors the install-name rule and blocks Save with an inline error when violated.
- [ ] Docs: `docs/concepts.md` asset catalog section — one paragraph on `install_as` (what it is, when the importer sets it).
- [ ] `pnpm run check && pnpm run test && pnpm run build`; commit `feat(ui): install_as in the asset editor and detail`.
