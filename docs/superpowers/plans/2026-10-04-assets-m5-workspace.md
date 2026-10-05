# Assets M5: the workspace shell — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Assets tab's twelve-button toolbar with the S2 workspace shell — a rail (Inbox, Library, Secrets), a list with a sentence header and a token query, a tabbed Inspector pane, and a footer of catalog chips, `auto` and a `JobChip` — fed by read-only views of M4's cards and M3's catalogs, keyboard-driven, with a read-only mode for an ungranted hub client; and first land M4's carry list, led by the planner's "host edited vs host behind the catalog" signal that lets a card say which side moved and lets Rollout/SB6 update a stale copy but never an edited one.

**Architecture:** Backend first (Tasks 1–4, `fleet-core` + `src-tauri`): the host's fleet manifest entry records a sha256 per file it wrote (`ManifestEntry.file_hashes`), so the planner and the scan can tell `HostCopy::{Unchanged, Edited, Unverified}`; the planner turns an edited copy into an `Overwrite` (never an `Update`), the Additive filter takes an `Update` only over a verified-untouched copy, the inventory stores `drift_side` (migration 096), the Drift rule stops proposing cards for copies that are only behind, and SB6 brings those up to date. The other carries (slug collisions, decision-time ordering with `changeset_items.decided_at` — migration 097, bounded per-tick cost and retention, person-first `last_sync`) follow; then the read surface the shell needs: `list_assets` spans every loaded catalog with a `catalog` per asset and per-catalog host states, a new `catalog_admin { asset_history }`, and four read-only Tauri commands routed through the hub with verdict rows. Frontend (Tasks 5–12, Svelte 5): pure TS modules (`assets_visual.ts`, `assets_workspace.ts`, `assets_query.ts`, `assets_inbox.ts`), leaf components each with its own vitest file (`Badge`, extended `HostStrip`, `AssetsInbox`, `QueryInput`, `Inspector` + `AssetInspector`, `CatalogChip`, `JobChip`, `AssetsFooter`, `AssetsRail`), then `AssetsWorkspace` assembling them inside the existing `AssetsPanel` (which keeps its load/probe logic and every dialog), then the keyboard. Existing components are wrapped, not rewritten: `AssetList` is the Library, `AssetDetail` gains a `section` prop for Overview/Hosts/Source, `AssetEditor` and `SyncPlanDialog` are untouched apart from tokens and `Badge`.

**Tech Stack:** Rust (fleet-core: rusqlite, rmcp, serde; src-tauri: Tauri 2 commands, `backend::verdicts`), SQLite migrations 096/097, Svelte 5 (runes, `$bindable`, snippets), TypeScript, Vitest + @testing-library/svelte, `controls.css` tokens.

**Spec:** `docs/superpowers/specs/2026-09-30-assets-s1b-s2-design.md` — milestone **M5** (Milestones row: "shell: `AssetsWorkspace`, rail, Inbox sections, Library, Inspector, footer chips, `JobChip`, `QueryInput`, keyboard, `Badge`, read-only chip"), its *Design coverage* rows marked M5, and the whole *Workspace shell* section; parent `docs/superpowers/specs/2026-09-29-assets-workspace-design.md` (*Information architecture*, *Visual system*, *Accessibility*, *States*) and the mockups `docs/superpowers/specs/2026-09-29-assets-workspace-mockups.html` (screens 2 Inbox and 4 Library are the visual reference). Where mockup and spec disagree, the spec wins. Builds on M1–M4 (merged; v0.4.6). Layers view, Hosts view, admissions UI, `ChangesetCard` apply/undo, `DiffView`, Settings → Catalogs and QuickSwitcher are **M6**.

## Global Constraints

- Spec, M5 row: "shell: `AssetsWorkspace`, rail, Inbox sections, Library, Inspector, footer chips, `JobChip`, `QueryInput`, keyboard, `Badge`, read-only chip".
- Spec, Workspace shell: "The layout, rows and cards follow the mockups. Existing components are kept and wrapped, not rewritten."
- Spec: "`AssetsWorkspace.svelte` (new) replaces the toolbar layout of `AssetsPanel.svelte`; the twelve buttons go. The only primary button is contextual (`Adopt 110 as 6 layers`, `Roll out to core`, `Sync fleet`)."
- Spec: "**Rail:** Inbox (default), Layers, Hosts, Library, Secrets. A Tests entry appears only when S4 lands." (Layers and Hosts views are M6 — Rulings R15.)
- Spec: "**Inbox:** proposed cards on top (`ChangesetCard`), then sections; the in-sync block folds to one line. Rows reuse S1a's identity rows and `HostStrip`, plus a `Badge` for scope/catalog."
- Spec: "**Library:** every asset once, grouped by kind; managed-elsewhere assets read-only."
- Spec: "**Inspector:** a pane. `AssetDetail` becomes Overview + Hosts; `AssetEditor` stays in Source (drafts arrive in S3); History lists commits. Drift shows a `DiffView` with Take / Restore." (`DiffView` is M6.)
- Spec: "**Footer:** a chip per catalog (HEAD, ahead count; popover with pull, push, commit), `auto: on|off`, and `JobChip` for scans and syncs. The modal `SyncPlanDialog` becomes the Rollout card; its "Plan anyway" stays host-scoped." (The Rollout card is M6 — R18.)
- Spec: "**Query:** `QueryInput` on `/` with tokens `host:` `kind:` `state:` `layer:` `catalog:` `scope:`, case-insensitive, with completion."
- Spec: "**Keyboard:** `j/k`, `space`, `a` adopt, `i` ignore, `s` sync, `e` edit, `⌘↵` primary."
- Spec: "**Hub client without a grant:** the same views without mutating controls, and one scope chip." Parent: "one scope chip ("read-only · ask the operator to grant `assets` on `personal`") replacing the CLI paragraph at `AssetsPanel.svelte:308-315`."
- Spec: "**Visuals:** the existing tokens and `controls.css`; the hard-coded hex colours in `AssetDetail` and `SyncPlanDialog` move to tokens; one `Badge` replaces `.chip`, `.op-badge`, `.count-chip`. State is never colour alone."
- Parent, Accessibility: "Fully keyboard-operable with a visible focus ring. Host dots carry `aria-label` ("oci: differs"). Contrast follows the tokens. No state is conveyed by colour alone."
- M4 safety invariants stay: nothing ever removes from a host through a card or SB6; `overwrite` only through a Drift restore a person picked; SB6 only on layers a Rollout card applied; a `person` verdict is never replaced by another decider.
- Migrations: **096** (`096_inventory_drift_side.sql`) and **097** (`097_changeset_item_decided_at.sql`). On 2026-10-04 `crates/fleet-core/migrations/` ends at `095_downloads.sql` (the brief's "094 is latest" predates the file-downloads merge). If `main` gains a migration first, renumber (file, `MIGRATIONS` entry, `schema_version` insert, guard name, test names). Both are `ALTER TABLE … ADD COLUMN`, so both carry an `already_applied` guard, like 087/089/091/092.
- Lock rule: **registry → store is allowed; store → registry never.** Read store rows under one guard, drop it, then take the registry. Nothing inside a `registry::with_catalogs` closure calls `registry::` again (a second read lock while a writer waits deadlocks `std::sync::RwLock`).
- Never hold the `Store` guard across an `.await`.
- New serialized fields are `#[serde(default)]` (and `skip_serializing_if` when absent must read as before); internal bookkeeping is `#[serde(skip)]`.
- Personal-only parity: a fleet with only `personal`, no org-bound hosts and no cards lists, plans, scans and syncs exactly as before, except (a) each `AssetSummary` and `HostState` carries the new optional fields, (b) a planned `Update` over a manifest entry written before M5 carries a reason, and (c) Rollout/SB6 no longer update a copy whose manifest entry predates the file hashes (R3).
- Frontend: Svelte 5 runes only (`$props`, `$state`, `$derived`, `$effect`, `$bindable`); stores stay `svelte/store` writables in `src/lib/*.ts` like `assets.ts`; controls use `controls.css` classes (`.btn`, `.btn--primary`, `.btn--quiet`, `.btn--chip`, `.is-bounded`); colours only through tokens (`--usage-ok|warn|crit`, `--accent`, `--accent-soft`, `--control-*`, `--fg-muted`, `--border`, `--bg-pane`); no hex literal in any Assets component's `<style>` (held by `src/lib/assets_tokens.test.ts`, Task 5); `prefers-reduced-motion` respected for every animation.
- Every new component gets its own `*.test.ts`; `pnpm test` and `pnpm check` green at every task end that touches the frontend.
- Hub routing: a new Tauri command means a `VERDICTS` row, a `routed::` body routing **by its own command name**, a case in `tests_routing.rs`, registration in `src-tauri/src/lib.rs`, then `REGEN_HUB_VERDICTS=1 cargo fleet-test -- verdict_gen`.
- The served-definition budget (`mcp::tools::tests::the_served_definition_budget_stays_bounded`, `BUDGET_BYTES`, currently 72_787): Task 4 changes `CatalogAdminParams::action`'s doc (adds `asset_history`), so it re-measures and sets the constant to the printed measurement + 100, naming what was measured in the commit message, and regenerates `docs/control-api-reference.md` with `REGEN_DOCS=1 cargo fleet-test -- reference_is_current`.
- Validation: the ladder in `CLAUDE.md` (`cargo fleet-fast-check` while editing, `cargo fleet-check` at checkpoints, `cargo fleet-test -- <filter>` for touched tests, `cargo fmt --all --check` + `cargo fleet-lint` before each commit, `cargo test --workspace` + `pnpm test` + `pnpm check` before the PR). **Rust builds and tests run on mercury through the `mercury-run` skill** (the local Mac cargo is unreliable); `pnpm test` / `pnpm check` run locally or on mercury.
- `cargo test` takes ONE name filter per command. Known unrelated flakes: `rewind::tests::the_removal_script_leaves_a_tree_a_live_pane_is_in`, `work::scale_tests::*`, the `CHAIN_BUDGET` tests in `store/schema/tests_upgrade.rs`, `service::add_project`. Re-run a suspected flake alone before touching anything.
- Tests that touch the catalog registry or `HOME` take `crate::service::catalog::lock_registry_for_test()`.
- Shell-quoting only through `crate::shell::quote`; child processes only through `fleet_core::proc::command` / `std_command` (`repo::git` already does).
- Git: branch `feat/assets-m5-workspace` from `origin/main`; one commit per task; never rebase, never force-push; merge `origin/main` for conflicts.

## Pre-flight (verify before Task 1)

Every task leans on these facts of the merged code (read 2026-10-04 at `f229fd66`). Check each; if one differs, stop and re-rule before coding.

| Fact | Where |
|---|---|
| Latest migration is 095: `Migration::plain(95, include_str!("../../migrations/095_downloads.sql"))`; `LATEST_SCHEMA_VERSION` is derived from the last entry | `crates/fleet-core/src/store/schema.rs:1028`, `:1060` |
| ADD COLUMN guard pattern `asset_inventory_has_catalog_id(conn)` + `Migration { version, sql, already_applied: Some(..) }`; test pattern `migration_092_adds_the_provision_warning_as_none_and_is_safe_to_rerun` with `store_at_version(91)` | `store/schema.rs:399`, `:1014`, `:4436`, `:1708` |
| `ManifestEntry { hash, files, merges, synced_at, catalog }` with a manual `Default`; `Manifest::entry_for(hash, plan, now, catalog)` is the one writer besides `plugin_entry` | `service/catalog/sync/manifest.rs:22-52`, `:107`; `sync/apply.rs:470`, `:1292` |
| `HostSnapshot.files` is path → **sha256 hex of the bytes**; there are no bytes in a snapshot, so `RenderPlan::hash` (sha over path+bytes) cannot be recomputed from a scan | `service/catalog/harness/mod.rs:147`, `:100` |
| Rule 4 of `compute_host_plan`: present+differing → `Update` when `entry.hash != plan.hash()` — **even over a host edit** — else `Overwrite("edited on host; the catalog has not changed")` | `sync/plan.rs:770-801` |
| `Action` has full-literal constructions (no `..Default`) in `plan.rs` (`blank()` and tests), `sync/apply.rs` ×3, `sync/mod.rs`, `changesets/apply.rs` `filter_tests::action`, `mcp/tools/tests_catalog_admin.rs` | `grep -rn 'plugin: None,$' crates` |
| `compute_states(catalog, harness, host_alias, snap, manifest, secrets, scanned_at)`; a managed row's `state` is `drifted` when present and not matching | `service/catalog/inventory.rs:347-470` |
| `AssetInventoryRow` derives `Default`; `replace_host_inventory` INSERT and `list_inventory` SELECT name every column | `store/rows.rs:1223`; `store/catalog.rs:319-379` |
| `op_allowed(OpFilter, ActionOp)`, `narrow(hp, f, assets, catalogs)`; SB6's `due` match treats `drifted` as never due ("final review I1 (interim)") | `changesets/apply.rs:1416-1446`, `:1880-1900` |
| `rejected_rollouts(s)` sorts cards by `id` | `changesets/apply.rs:2005` |
| `DriftFacts` built from every `drifted` managed Claude row | `changesets/reconcile.rs:158-170`; `rules.rs:52`, `:641` |
| `bootstrap_card` groups eligible normal identities; `look_item` derives its reason itself; imports must come back as `(kind, slugify(name))` or the group fails | `rules.rs:368`, `:400`; `import.rs:77` |
| `set_item_states(conn, id, positions, state)` is the one item-state writer; `replace_changeset_items_keeping` re-rejects by position; `ITEM_COLS`/`item_row` | `store/changesets.rs:93-160`, `:282-320` |
| `changesets::list` calls `is_undoable` → `later_card` per card, which reads every card's items again | `changesets/mod.rs:420-520` |
| `reconcile::gather` and `auto_additive` clone every loaded catalog with `registry::snapshot()` on each pass | `reconcile.rs:92`; `apply.rs:1858`; `registry.rs:176` |
| `sync::last_sync` answers the newest run, SB6's included; `Store::last_person_sync_run_id` exists | `sync/mod.rs:792`; `store/catalog.rs:526` |
| `list_assets` lists the **personal** catalog only (`with_catalog` = `registry::with_personal`); `host_states(rows, kind, name)` ignores `catalog_id`; `get_asset_in` uses it too | `service/catalog/mod.rs:351`, `:407`, `:427`, `:570` |
| `Catalog { id, name, org_id, … }`; `registry::with_catalogs`, `registry::in_order`, `registry::entry_for` | `repo.rs:22`; `registry.rs:104`, `:115`, `:217` |
| `AdminCall` declared by `admin_calls!`; `the_action_param_names_every_admin_call` holds `CatalogAdminParams::action`'s doc to `AdminCall::ACTIONS`; `every_call()` lists one of each in declaration order | `admin.rs:142-183`; `mcp/tools/tests_catalog_admin.rs:405`; `admin.rs:506` |
| MCP `list_assets` takes no `Caller`; `catalog_admin` builds `{action, args}` + the top-level `catalog` parameter | `mcp/tools/assets.rs:14`, `:175` |
| Hub routing: `HubBackend::route(command, args)` serialises `args` whole as the tool's arguments; `catalog_admin_cases()` drives every catalog row; `every_routed_row_is_driven_by_a_case` | `src-tauri/src/backend/remote.rs:603`; `backend/tests_routing.rs:4463`, `:317` |
| Contract: `CONTRACT_REVISION = 7`, `MIN_HUB_CONTRACT = MAX_HUB_CONTRACT = 7`; revision 7 (file downloads) was cut after `d140e9e2 Merge origin/main (Assets M4) into file downloads`, so every revision-7 hub serves `changesets` | `crates/fleet-core/src/wire_contract.rs`; `src-tauri/src/backend/contract.rs:121`, `:130` |
| Frontend: `AssetsPanel.svelte` owns probe/load state and every dialog; toolbar testids `assets-scan|import|sync|secrets|new|lint-all|commit-pending|push|last-sync|problems|head|repo-status`; only `AssetsPanel.test.ts` and `hub_disabled.test.ts` reference them | `src/lib/AssetsPanel.svelte`; `grep -rl "assets-" src` |
| Hex colours: `AssetDetail.svelte:270-287`, `SyncPlanDialog.svelte:196-218`, `AssetEditor.svelte:524-526`, `AssetsPanel.svelte:517,528` | as listed |
| `isEditable(el)` and the `j/k` list pattern | `src/lib/terminal_keys.ts:250`; `src/lib/HostsView.svelte:280-340` |
| `fleetSettings` / `settingBool` / `SETTING_KEYS.catalogAuto` | `src/lib/fleet_settings.ts:58`, `:251`, `:265` |
| No `createRawSnippet` anywhere in the tests; the node-fs shim declares only `readFileSync` | `src/node-fs.d.ts` |

## Rulings

Where the spec is silent, the carry list asked for a decision, or the code forced one. Each ruling states its cost if it turns out wrong.

- **R1 — The host-copy signal is per-file hashes recorded at apply time, not a recomputed `RenderPlan::hash`.** The brief asked to "hash the host's current files the way `RenderPlan::hash` does and compare with the manifest entry's hash". A scan carries a sha256 per file, never the bytes, so the plan hash cannot be rebuilt from it. Equivalent and exact: `Manifest::entry_for` also records `file_hashes: path → sha256(bytes written)`, and `ManifestEntry::host_copy(snap)` compares each recorded file with the scan's hash and each recorded merge's `value_hash` with the host's current value (`Set`: the value at the path; `AppendUnique`: some element of the array) → `Unchanged` (all equal, at least one location recorded) / `Edited` (one differs or is gone) / `Unverified` (an entry written before M5 has no `file_hashes`; a `Subset` merge is never hashed whole; nothing recorded). *Cost if wrong:* the manifest on each host grows by one 64-char hash per file.
- **R2 — What the planner does with it.** Present and differing, managed: `Edited` → `Overwrite` always — reason "edited on host, and the catalog changed too" when the catalog moved, the M4 reason "edited on host; the catalog has not changed" when it did not; `Unchanged` → `Update` (no reason); `Unverified` → M4's rule (`Update` when the catalog moved, now with the reason "the catalog changed; the host copy predates fleet's file hashes, so a host edit cannot be ruled out"; else `Overwrite`). `Action.host_copy` (`#[serde(skip)]`) carries the verdict to the applier and the card filters. A person's own sync therefore sees an `overwrite` (red, backed up) where M4 showed an `update` over an edit — the point of the carry. *Cost if wrong:* a host edit made identical to a new catalog render still reads `Noop` (matches), as before.
- **R3 — Rollout and SB6 (`OpFilter::Additive`) take an `Update` only over a copy the planner verified `Unchanged`.** `Edited` is an `Overwrite` (never Additive); `Unverified` is dropped from Additive too. `OpFilter::Restore` (one asset, one host, a person's pick) is unchanged: update or overwrite, with backup. Nothing removes, under any filter. *Cost if wrong:* a host whose manifest predates M5 needs one sync by a person before a Rollout or SB6 can update it — the safe direction.
- **R4 — `asset_inventory.drift_side` (migration 096).** On a `drifted` managed row: `host` (the copy is no longer what fleet wrote — a person edited it, whether or not the catalog also moved), `catalog` (the copy is exactly what fleet wrote; the catalog moved on), `NULL` otherwise or when `host_copy` is `Unverified`. It travels on `AssetInventoryRow` and on `HostState` (so `list_assets` carries it to a read-only client that cannot read the inventory). *Cost if wrong:* one nullable column.
- **R5 — The Drift rule reads which side moved.** A row with `drift_side = catalog` gets no Drift card (a catalog-only change no longer opens one card per host; Rollout, SB6 or a person's Sync bring it up); an open Drift card whose row turns `catalog` is withdrawn by the pass (R2 of M4: subject no longer produced). `host` → the summary says "`<kind>/<name>` was edited on `<host>` (catalog `<c>`)"; `NULL` → M4's "differs on … from catalog …". *Cost if wrong:* an unverified copy still gets a card per host until its next sync records hashes.
- **R6 — SB6 also brings a copy that is only behind.** `sb6_due(row)`: `missing`; `in_sync` and unmanaged (adopt); **`drifted`, managed, `drift_side = catalog`** (new). The planner re-checks the fresh snapshot (R2/R3), so a copy edited between the scan and the plan is an `Overwrite` and dropped. *Cost if wrong:* one extra plan of such a host per pass.
- **R7 — Slug collisions in one Bootstrap card become "needs a look".** Two eligible normal identities of one kind whose names slugify alike (`My_Skill`, `my-skill`) both go to the card's `needs a look` group with "imports as `<slug>`, as `<other>` does", instead of failing the whole card at apply. New cards are one identity each and cannot collide in a card. *Cost if wrong:* a person names one of them in `positions`.
- **R8 — Decision time is `changeset_items.decided_at` (migration 097, Unix ms).** `set_item_states` stamps it whenever an item leaves `pending` (applied, skipped, rejected) and clears it on a return to `pending`; a refresh that re-rejects an item keeps the time the person decided (matched on `(grp, kind, name, action, catalog_id)` inside the store's transaction). `rejected_rollouts` orders every rollout `sync` decision by `decided_at`, else (decided before 097) its card's `applied_at`, else `created_at × 1000`, then card id and position, and the last one per `(catalog, layer, host)` wins. This adds a column to the spec's verbatim M4 DDL (M4 R1). *Cost if wrong:* a nullable column.
- **R9 — Per-tick cost and retention.** (a) `reconcile::gather` and `auto_additive` borrow the registry under its read lock (`registry::with_catalogs`) instead of cloning every catalog each pass; registry → store is allowed, and nothing inside calls `registry::` again. (b) `changesets::list` computes undoability once over all cards (`undoable_ids`, linear) instead of `later_card` per card. (c) After each reconcile pass, cards the pass withdrew (`dismissed`, error `withdrawn: …`), created more than `WITHDRAWN_RETENTION_SECS` (7 days) ago, with **no item ever out of `pending`**, are deleted (items cascade). Everything a person or an apply decided stays forever (undo, `rejected_rollouts`, `rolled_out_layers` read it). *Cost if wrong:* a withdrawn, untouched card older than a week is not in `list`'s 20 recent closed cards.
- **R10 — `last_sync` prefers a person's run.** `sync::last_sync` answers the newest run without `"auto": true`, else the newest run of any kind (a fleet only SB6 has synced still shows one). `SyncRunSummary.auto?: boolean` joins the TS type; the footer marks an auto run with an `auto` badge. *Cost if wrong:* a hub-side behaviour change of `catalog_last_sync` an older desktop shows identically.
- **R11 — `list_assets` spans every loaded catalog, for the callers who may see every catalog.** `AssetSummary.catalog` (`#[serde(default = personal)]`) names the asset's catalog (`personal` or the org catalog's name); assets come from `registry::in_order` (personal first), problem entries skipped; `head`, `loaded_at`, `problems` stay the personal catalog's. The desktop's own listing and the MCP tool for the master or an unbound full person device (the `list_catalogs` audience, M3 PF15) span every catalog; a per-host token, an org-bound or a readonly client keep the personal-only listing (`ListingScope::Personal`). The tool's description does not change. *Cost if wrong:* those callers see org catalogs' assets only through their unmanaged rows, as today.
- **R12 — Host states are per catalog (carry 3).** `host_states(rows, catalog, kind, name)` keeps a row when its `catalog_id` is that catalog's id, or when it has none and the catalog is personal (a row scanned before M2). `get_asset_in` and `list_assets` both use it, so a collision `personal/x` vs `acme/x` no longer shows one asset the other's hosts.
- **R13 — M5 adds four read-only Tauri commands (amends M4 R25 for reads only).** `catalog_list_catalogs` (→ `catalog_admin { list_catalogs }`), `catalog_list_changesets` (→ `changesets { list }`), `catalog_repo_status_in { name }` (→ `catalog_admin { repo_status, catalog }`), `catalog_asset_history { kind, name, catalog? }` (→ `catalog_admin { asset_history, catalog? }`). Each has a `Routed` verdict row, a routing case, and a local body. The named catalog always travels as the tool's own top-level `catalog` parameter, never inside `args`, so no hub can read it as personal. **No `CONTRACT_REVISION` bump:** every revision-7 hub serves `changesets` and per-catalog `catalog_admin` (M4 was merged before revision 7, `d140e9e2`); `asset_history` is a new action of an existing tool, which a revision-7 hub before M5 refuses with `E_INVALID` ("unknown variant") — a clear refusal the History tab turns into "update the hub", the precedent `wire_contract.rs` records for `new_worktree`. No mutating command: apply / undo / dismiss / reject stay M6. *Cost if wrong:* four rows and four cases.
- **R14 — Cards are read-only in M5.** The Inbox lists the open cards (`proposed`, `failed`) first: kind, sentence, group counts, failure. Selecting one shows its summary in the Inspector. Apply, Edit, Skip, Undo, Dismiss and the full `ChangesetCard` with items are M6. Without row events for cards (M4 R28), the list refreshes with the panel's `refresh()` (mount, scan, sync, import, every authoring write, catalog reload).
- **R15 — The rail has Inbox, Library and Secrets in M5.** Layers and Hosts are entries of views that land in M6; an entry with nothing behind it is dead UI, so they arrive with their views (`RAIL_VIEWS` gains two rows). Secrets stays the existing modal (spec: "Secrets — unchanged"), opened from the rail; hidden in read-only mode. *Cost if wrong:* two rows in M6.
- **R16 — Inbox sections.** In order: **Proposed** (open cards), **Needs you** (`needs_person` identities and orphans), **Drifted** (managed assets with a copy edited on a host, or one whose side is unknown), **Behind the catalog** (managed assets whose only differing copies are `drift_side = catalog` — a section the spec predates, introduced by R4/R5), **New on hosts** (normal unmanaged identities), **In sync** (folded to one line; it includes assets with `missing` dots, because M5 computes no footprints — that is the Hosts view's provenance in M6). Fleet internals are counted, not listed. *Cost if wrong:* one section header.
- **R17 — Where the twelve buttons went** (testids kept so the tests move, not rewrite):

  | Toolbar button | M5 home |
  |---|---|
  | Pull, Commit pending, Push, repo status line | the `personal` catalog chip's popover in the footer (`assets-pull`, `assets-commit-pending`, `assets-push`, `assets-repo-status`); HEAD on the chip (`assets-head`) |
  | Scan hosts | header "Rescan", quiet (`assets-scan`) |
  | Sync | the one primary button "Sync fleet" (`assets-sync`, `⌘↵`) |
  | Import from host, New asset, Lint all | Library header, quiet (`assets-import`, `assets-new`, `assets-lint-all`) |
  | Secrets | rail (`assets-secrets`) |
  | N problems | header badge button (`assets-problems`) |
  | filter | `QueryInput` (`assets-query`) |
  | last-sync strip | footer (`assets-last-sync`) |
- **R18 — The contextual primary in M5 is "Sync fleet".** It opens the existing `SyncPlanDialog`, modal as today, with its host-scoped "Plan anyway"; Adopt / Roll out are card verbs (M6), and the dialog becomes the Rollout card in M6. Hidden in read-only mode.
- **R19 — The Inspector by selection.** A personal asset this window may write: tabs Overview · Source · Hosts · History; one `AssetDetail` instance takes a `section` prop (`overview` = title, actions, lint, description, tags; `hosts` = the matrix; `source` = `AssetEditor` or the rendered preview; `all` = today's whole detail, the default), so switching tabs never refetches; Edit switches to Source. An asset of an org catalog, or any asset in read-only mode: Overview and Hosts from the listing's summary (no `catalog_get_asset`, which is personal-only on the desktop until M6), plus History when not read-only. An identity: Overview (where, which hashes, why it needs a look; Import) and Hosts. An orphan: Overview. A card: Overview (R14). History lists `asset_history`'s newest 50 commits. The generic `Inspector.svelte` (tabs, Left/Right, a tabpanel) is shared with M6's Layers and Hosts.
- **R20 — "Managed elsewhere, read-only" in M5** is what this window can only look at: every row in read-only mode (one chip says so, the rows show no controls), and assets of an org catalog this paired client holds no grant on (a `managed` badge, a static row). claude.ai and marketplace-provided skills are not in the inventory yet; they join when a scan reports them (out of M5). For the master and a granted client an org catalog's asset is an ordinary row with its catalog badge (opening it in Source is M6, R19).
- **R21 — Query semantics.** Whitespace-separated; `key:v1,v2` tokens AND together, values within a token OR; unknown keys and bare words are free text matched against name and description; case-insensitive. `host:` = present there (`in_sync`, `drifted`, `unmanaged`, `orphan`); `kind:` accepts `mcp`, `plugin` and `-` for `_`; `state:` = any host in that state, plus `edited` / `behind` (drift side); `layer:` = a member of that personal layer; `catalog:` = the asset's catalog; `scope:` = `private`, `shared` (personal assets), `org` (any non-personal catalog), `managed` (R20). A trailing `key:` with no value matches everything while typing. Completion offers keys, then that key's values (hosts, kinds, states, layers, catalogs, scopes), at most 8, Tab/Enter to take.
- **R22 — The keyboard.** In the list: `j`/`↓` and `k`/`↑` move focus between rows (`[data-row-key]`, the DOM order is the display order); `space`/`Enter` select the focused row (native button activation; static read-only rows handle both keys themselves); `a` adopts the focused unmanaged identity (opens Import preset to it); `s` syncs the focused asset (plan scoped to it); `e` opens the focused personal asset in Source, editing; `/` focuses the query (Esc in it closes the completions, then clears, then returns to the list); `⌘↵`/`Ctrl+↵` runs the primary. `i` (ignore) is a card verb — `reject_item` — and is bound in M6 with the cards; nothing is bound to it in M5. Read-only: only movement, select, `/`. Keys typed in an input, a textarea, a select, a dialog, or with a modifier (other than `⌘↵`) are never taken. Esc stays the App's (it closes the Assets overlay).
- **R23 — The read-only chip.** One chip button "read-only · ask the operator to grant assets on personal" in the header; pressing it shows the exact command with this client's name (`fleet-hub client grant <client> assets`, `assets-grant-cmd`). It replaces the paragraph.
- **R24 — Catalog chips.** One chip per row of `catalog_list_catalogs` (HEAD, `↑ahead`, `±dirty`, a problem mark); when that listing is refused (a readonly or org-bound client) or empty, one `personal` chip from the repo status, the config or the listing's head. Ahead/dirty come from `catalog_repo_status` for personal and `catalog_repo_status_in` for an org catalog (refused without a grant → HEAD only). Only the personal chip's popover has Pull / Commit pending / Push in M5 — the desktop's authoring commands are personal (M6 makes them per catalog); an org chip's popover shows its status and that `catalog.auto_push` pushes it. *Cost if wrong:* org pushes from the desktop wait for M6.
- **R25 — `JobChip`** shows the panel's background work: Scanning hosts, Planning, Syncing (with `sync:progress` done/total while applying), Pulling, Committing, Pushing. Otherwise the footer shows the last sync (R10). The sync dialog stays modal in M5 (R18), so the chip is mostly seen during scans; it is the Rollout card's progress in M6.
- **R26 — `Badge`** is information, never a control: `label` (required text), `tone` (`neutral|ok|warn|crit|accent|muted`), optional `glyph` (aria-hidden), `dashed` (private scope), `mono`, `title`, `testid`. It replaces `.chip` (AssetList state counts), `.badge.warn` / `.badge.orphan` (AssetList), `.op-badge`, `.count-chip`, `.outcome` (SyncPlanDialog) in the Assets components. The chat composer's `.chip` and others outside Assets are untouched. A clickable badge is a `.btn` containing one.
- **R27 — `HostStrip` gains `states`** (host → `DotState`: `in_sync`, `differs`, `missing`, `na` dash "not here", `stale` hatched, `blocked` cross, plus the existing `present`/`absent`), keeping its `present`/`odd` API and labels for identity rows. A host the hosts store says is unreachable reads `stale`.
- **R28 — Generated files.** `REGEN_HUB_VERDICTS` (four rows) and `REGEN_DOCS` + `BUDGET_BYTES` (the `asset_history` word in `CatalogAdminParams::action`) in Task 4; no setting, page or tool description otherwise changes.
- **R29 — Docs.** `CLAUDE.md` gains an *Assets M5* paragraph (the repo's per-milestone convention); `docs/hub.md`'s refusal table is generated and unchanged (no `LocalOnly` row). The spec's M7 still owns `docs/hub.md`'s catalog section.

## Carry list → where it lands

| # | Carry (from the M4 ledger) | Lands in |
|---|---|---|
| 1 | Planner "host edited vs host stale": the signal, (a) the Drift rule, (b) `plan.rs` Update vs Overwrite so SB6/Rollout update only a stale copy | Task 1 (signal, planner, Additive filter — R1–R3), Task 2 (inventory `drift_side`, Drift rule, SB6 — R4–R6) |
| 2 | Slug collisions in one card → "needs a look" | Task 3 (R7) |
| 3 | `get_asset_in` host states per catalog | Task 4 (R12) |
| 4 | `rejected_rollouts` ordered by decision time | Task 3 (R8, migration 097) |
| 5 | Card retention / per-tick cost (snapshot clone, quadratic `later_card` in `list`) | Task 3 (R9) |
| 6 | `SyncRunSummary.auto` in TS; last-sync prefers person runs | Task 3 (backend, R10), Task 6 (type), Task 10 (footer badge) |

## File Structure

| File | Responsibility |
|---|---|
| `crates/fleet-core/src/service/catalog/sync/manifest.rs` | `ManifestEntry.file_hashes`, `HostCopy`, `ManifestEntry::host_copy` |
| `crates/fleet-core/src/service/catalog/sync/plan.rs` | `Action.host_copy`; rule 4 reads it (R2) |
| `crates/fleet-core/src/service/catalog/sync/apply.rs` | `plugin_entry` literal gains `file_hashes` |
| `crates/fleet-core/migrations/096_inventory_drift_side.sql` (new), `097_changeset_item_decided_at.sql` (new) | the two columns |
| `crates/fleet-core/src/store/{schema,rows,catalog,changesets}.rs` | registration + guards; `AssetInventoryRow.drift_side`; `ChangesetItemRow.decided_at`; stamping; prune; `last_person_sync_run` |
| `crates/fleet-core/src/service/catalog/inventory.rs` | `drift_side` per drifted managed row |
| `crates/fleet-core/src/service/catalog/changesets/{rules,reconcile,apply,mod}.rs` | Drift by side, slug collisions, `sb6_due`, `action_allowed`, `rejected_from`, `undoable_ids`, registry borrow, prune call |
| `crates/fleet-core/src/service/catalog/sync/mod.rs` | `last_sync` person-first |
| `crates/fleet-core/src/service/catalog/mod.rs` | `AssetSummary.catalog`, `HostState.drift_side`, `ListingScope`, `list_assets_in`, per-catalog `host_states` |
| `crates/fleet-core/src/service/catalog/{repo,author,admin}.rs` | `CommitEntry`, `asset_log`; `asset_history_in`; `AdminCall::AssetHistory` |
| `crates/fleet-core/src/mcp/tools/{assets,params,tests_catalog_admin,tests}.rs` | `list_assets` scope by caller; `asset_history` in the action doc; budget |
| `src-tauri/src/commands/assets.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/backend/{verdicts,tests_routing}.rs` | four read-only commands, rows, cases |
| `src/lib/assets_visual.ts` (new) | `BadgeTone`, `DotState`, labels, op/outcome tones |
| `src/lib/Badge.svelte` (new), `src/lib/HostStrip.svelte` | the one badge; dot states |
| `src/lib/assets_tokens.test.ts` (new) | no literal colour in Assets components |
| `src/lib/assets.ts` | `catalog`, `drift_side`, `auto` on the wire types |
| `src/lib/assets_workspace.ts` (new) | catalogs / cards / layers stores and loaders, history and repo-status reads, selection keys, scope badge, write rights, run summary, `ago` |
| `src/lib/assets_query.ts` (new) | parse, match, complete the token query; query rows |
| `src/lib/assets_inbox.ts` (new) | Inbox sections, dots, the sentence header |
| `src/lib/AssetsInbox.svelte` (new) | the Inbox view |
| `src/lib/QueryInput.svelte` (new), `src/lib/AssetList.svelte` | the query field; Library = AssetList with badges, predicates, selection keys |
| `src/lib/Inspector.svelte` (new), `src/lib/AssetInspector.svelte` (new), `src/lib/AssetDetail.svelte` | tabbed pane; what it shows per selection; `section` prop |
| `src/lib/JobChip.svelte` (new), `src/lib/CatalogChip.svelte` (new), `src/lib/AssetsFooter.svelte` (new) | footer |
| `src/lib/AssetsRail.svelte` (new), `src/lib/Icon.svelte` | rail; three icons |
| `src/lib/AssetsWorkspace.svelte` (new), `src/lib/AssetsPanel.svelte` | the shell; the panel renders it |
| `src/lib/SyncPlanDialog.svelte`, `src/lib/AssetEditor.svelte` | tokens + `Badge` only |
| `src/lib/AssetsPanel.test.ts`, `src/lib/hub_disabled.test.ts` | moved testids |
| `CLAUDE.md` | *Assets M5* paragraph |

---

### Task 1: The host-copy signal — manifest file hashes, the planner, and the Additive filter (carry 1, R1–R3)

**Files:**
- Modify: `crates/fleet-core/src/service/catalog/sync/manifest.rs` (struct `ManifestEntry` `:22-52`, `entry_for` `:107`, tests)
- Modify: `crates/fleet-core/src/service/catalog/sync/plan.rs` (`Action` `:85-140`, `blank` `:627`, rule 4 `:770-801`, the final `Action { … }` `:812-830`, tests)
- Modify: `crates/fleet-core/src/service/catalog/sync/apply.rs` (`plugin_entry` `:470`; full `Action { … }` literals in tests)
- Modify: `crates/fleet-core/src/service/catalog/sync/mod.rs` (one full `Action { … }` literal)
- Modify: `crates/fleet-core/src/service/catalog/changesets/apply.rs` (`op_allowed`/`narrow` `:1416-1446`, `filter_tests`)
- Modify: `crates/fleet-core/src/mcp/tools/tests_catalog_admin.rs` (one full `Action { … }` literal)

**Interfaces:**
- Consumes: `HostSnapshot { files: BTreeMap<String /*path*/, String /*sha256 hex*/>, configs: BTreeMap<String, Value>, .. }`; `harness::{json_get, value_hash, ManifestMerge, MergeMode}`; `model::sha256_hex`.
- Produces:
  - `pub struct ManifestEntry { …, pub file_hashes: BTreeMap<String, String> }` (`#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]`).
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "snake_case")] pub enum HostCopy { Unchanged, Edited, Unverified }` in `sync::manifest`.
  - `impl ManifestEntry { pub fn host_copy(&self, snap: &HostSnapshot) -> HostCopy }`.
  - `pub struct Action { …, #[serde(skip)] pub host_copy: Option<HostCopy> }`.
  - `pub(crate) const EDITED_AND_MOVED_REASON: &str`, `pub(crate) const EDITED_ON_HOST_REASON: &str`, `pub(crate) const UNVERIFIED_UPDATE_REASON: &str` in `sync::plan`.
  - `pub(crate) fn action_allowed(f: OpFilter, a: &Action) -> bool` in `changesets::apply`; `narrow` uses it.

- [ ] **Step 1: Write the failing manifest tests**

Add to `mod tests` in `sync/manifest.rs` (it already imports `FileWrite`, `ConfigMerge`, `MergeMode`, `json!`, `value_hash`):

```rust
    use crate::service::catalog::model::sha256_hex;

    /// Assets M5 (R1): an entry records the hash of every file it wrote, so
    /// a later scan tells "as fleet left it" from "a person edited it".
    #[test]
    fn host_copy_tells_an_untouched_copy_from_an_edited_one() {
        let mut plan = RenderPlan::default();
        plan.files.push(FileWrite {
            path: "~/.claude/skills/s/SKILL.md".into(),
            bytes: b"body".to_vec(),
        });
        let hook = json!({"type": "command", "command": "x"});
        plan.merges.push(ConfigMerge {
            file: "~/.claude/settings.json".into(),
            json_path: vec!["hooks".into(), "Stop".into()],
            mode: MergeMode::AppendUnique,
            value: hook.clone(),
        });
        let entry = Manifest::entry_for("h", &plan, 1, "personal");
        assert_eq!(
            entry.file_hashes.get("~/.claude/skills/s/SKILL.md"),
            Some(&sha256_hex(b"body"))
        );

        let mut snap = HostSnapshot::default();
        snap.files
            .insert("~/.claude/skills/s/SKILL.md".into(), sha256_hex(b"body"));
        snap.configs.insert(
            "~/.claude/settings.json".into(),
            json!({"hooks": {"Stop": [{"type": "command", "command": "other"}, hook]}}),
        );
        assert_eq!(entry.host_copy(&snap), HostCopy::Unchanged);

        let mut edited = snap.clone();
        edited
            .files
            .insert("~/.claude/skills/s/SKILL.md".into(), sha256_hex(b"edited"));
        assert_eq!(entry.host_copy(&edited), HostCopy::Edited);

        let mut gone = snap.clone();
        gone.files.remove("~/.claude/skills/s/SKILL.md");
        assert_eq!(entry.host_copy(&gone), HostCopy::Edited, "a deleted file is an edit");

        let mut hook_changed = snap.clone();
        hook_changed.configs.insert(
            "~/.claude/settings.json".into(),
            json!({"hooks": {"Stop": [{"type": "command", "command": "y"}]}}),
        );
        assert_eq!(entry.host_copy(&hook_changed), HostCopy::Edited);
    }

    /// R1: an entry written before M5 has no file hashes — it cannot vouch
    /// for the host copy either way, and it still reads and writes as before.
    #[test]
    fn an_entry_written_before_m5_is_unverified_not_edited() {
        let entry: ManifestEntry = serde_json::from_str(
            r#"{"hash":"h","files":["~/.claude/skills/s/SKILL.md"],"merges":[],"synced_at":1}"#,
        )
        .unwrap();
        assert!(entry.file_hashes.is_empty());
        let mut snap = HostSnapshot::default();
        snap.files
            .insert("~/.claude/skills/s/SKILL.md".into(), "anything".into());
        assert_eq!(entry.host_copy(&snap), HostCopy::Unverified);
        assert_eq!(
            ManifestEntry::default().host_copy(&snap),
            HostCopy::Unverified,
            "an entry that records nothing vouches for nothing"
        );
        assert!(!serde_json::to_string(&entry)
            .unwrap()
            .contains("file_hashes"));
    }

    /// R1: a `Set` merge is checked by its value hash; a `Subset` merge
    /// (a plugin's entry) is never hashed whole, so it cannot vouch.
    #[test]
    fn host_copy_reads_set_merges_and_cannot_read_subset_ones() {
        let value = json!({"url": "https://example/mcp"});
        let entry = ManifestEntry {
            merges: vec![ManifestMerge {
                file: "~/.claude.json".into(),
                json_path: vec!["mcpServers".into(), "fleet".into()],
                mode: MergeMode::Set,
                value_hash: value_hash(&value),
            }],
            ..Default::default()
        };
        let mut snap = HostSnapshot::default();
        snap.configs.insert(
            "~/.claude.json".into(),
            json!({"mcpServers": {"fleet": {"url": "https://example/mcp"}}}),
        );
        assert_eq!(entry.host_copy(&snap), HostCopy::Unchanged);
        snap.configs.insert(
            "~/.claude.json".into(),
            json!({"mcpServers": {"fleet": {"url": "https://elsewhere/mcp"}}}),
        );
        assert_eq!(entry.host_copy(&snap), HostCopy::Edited);

        let subset = ManifestEntry {
            merges: vec![ManifestMerge {
                file: "~/.claude/plugins/installed_plugins.json".into(),
                json_path: vec!["plugins".into(), "sp@mk".into()],
                mode: MergeMode::Subset,
                value_hash: "h".into(),
            }],
            ..Default::default()
        };
        assert_eq!(subset.host_copy(&snap), HostCopy::Unverified);
    }
```

If `ManifestMerge` is not yet imported in that test module, add `use super::super::super::harness::ManifestMerge;` next to the existing harness imports.

- [ ] **Step 2: Run them to verify they fail**

Run (on mercury via `mercury-run`): `cargo fleet-test -- service::catalog::sync::manifest`
Expected: FAIL to compile — `no field file_hashes`, `cannot find type HostCopy`, `no method host_copy`.

- [ ] **Step 3: Implement the signal in `manifest.rs`**

Change the import line at the top to:

```rust
use super::super::harness::{
    json_get, value_hash, HostSnapshot, ManifestMerge, MergeMode, RenderPlan,
};
use super::super::model::{sha256_hex, Kind};
```

Add the field at the end of `ManifestEntry` (after `catalog`):

```rust
    /// Assets M5 (Rulings R1): path → sha256 of the bytes this sync wrote
    /// there — the same hash a scan reports per file — so a later plan can
    /// tell a copy that is exactly as fleet left it from one a person
    /// edited. Absent on an entry written before M5, which then cannot
    /// vouch for its copy ([`HostCopy::Unverified`]).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_hashes: BTreeMap<String, String>,
```

In `impl Default for ManifestEntry`, add `file_hashes: BTreeMap::new(),`. In `Manifest::entry_for`, add to the struct literal:

```rust
            file_hashes: plan
                .files
                .iter()
                .map(|f| (f.path.clone(), sha256_hex(&f.bytes)))
                .collect(),
```

Append below `impl Default for ManifestEntry`:

```rust
/// Assets M5 (Rulings R1): whether a managed asset's copy on the host is
/// still what fleet wrote there — read from the manifest entry's recorded
/// hashes against a scan, so it needs no bytes from the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostCopy {
    /// Every location the entry records is exactly what fleet wrote.
    Unchanged,
    /// A recorded file or merge differs from what fleet wrote, or is gone:
    /// someone edited the host copy.
    Edited,
    /// The entry cannot say: written before M5 (no file hashes), a `Subset`
    /// merge (never hashed whole), or nothing recorded at all.
    Unverified,
}

impl ManifestEntry {
    /// [`HostCopy`] of this entry's asset on `snap`. Any differing or
    /// missing location is `Edited`, even when another one is unverifiable.
    pub fn host_copy(&self, snap: &HostSnapshot) -> HostCopy {
        let mut unverified = self.files.is_empty() && self.merges.is_empty();
        for path in &self.files {
            let Some(want) = self.file_hashes.get(path) else {
                unverified = true;
                continue;
            };
            if snap.files.get(path) != Some(want) {
                return HostCopy::Edited;
            }
        }
        for m in &self.merges {
            let have = snap
                .configs
                .get(&m.file)
                .and_then(|root| json_get(root, &m.json_path));
            let same = match m.mode {
                MergeMode::Set => have.is_some_and(|v| value_hash(v) == m.value_hash),
                MergeMode::AppendUnique => have
                    .and_then(|v| v.as_array())
                    .is_some_and(|arr| arr.iter().any(|e| value_hash(e) == m.value_hash)),
                MergeMode::Subset => {
                    unverified = true;
                    true
                }
            };
            if !same {
                return HostCopy::Edited;
            }
        }
        if unverified {
            HostCopy::Unverified
        } else {
            HostCopy::Unchanged
        }
    }
}
```

In `sync/apply.rs` `plugin_entry`, add `file_hashes: BTreeMap::new(),` to its `ManifestEntry { … }` literal (import `std::collections::BTreeMap` there if it is not already in scope). Every other `ManifestEntry { … }` in the crate ends in `..Default::default()`; `cargo fleet-check` names any that does not.

- [ ] **Step 4: Run the manifest tests**

Run: `cargo fleet-test -- service::catalog::sync::manifest`
Expected: PASS (the three new tests and every existing one; `entry_for_records_files_and_merge_hashes` is unchanged).

- [ ] **Step 5: Write the failing planner tests**

Add to `mod tests` in `sync/plan.rs`, next to `managed_and_stale_is_an_update_managed_and_edited_is_an_overwrite`:

```rust
    const SKILL_V2: &str = "kind: skill\nname: s\ndescription: d2\n";

    /// Assets M5 (R2): with the file hashes the entry recorded, a catalog
    /// change over an untouched copy is an `Update`, and over an edited copy
    /// an `Overwrite` — never an `Update` that silently drops the edit.
    #[test]
    fn a_catalog_change_updates_an_untouched_copy_and_overwrites_an_edited_one() {
        let old = substituted(&Claude, &asset(SKILL), &secrets_map());
        let mut manifest = Manifest {
            version: 1,
            ..Default::default()
        };
        manifest.assets.insert(
            "skill/s".into(),
            Manifest::entry_for(&old.hash(), &old, 0, "personal"),
        );
        let mut snap = HostSnapshot::default();
        satisfy(&mut snap, &old);
        let moved = catalog_of(&[SKILL_V2]);

        let hp = plan_for(&moved, &Claude, &snap, &manifest, &secrets_map());
        let a = act(&hp, "s");
        assert_eq!(
            (a.op, a.reason.as_deref(), a.host_copy),
            (ActionOp::Update, None, Some(HostCopy::Unchanged))
        );
        assert!(a.backup);

        snap.files
            .insert("~/.claude/skills/s/SKILL.md".into(), "edited".into());
        let hp = plan_for(&moved, &Claude, &snap, &manifest, &secrets_map());
        let a = act(&hp, "s");
        assert_eq!(a.op, ActionOp::Overwrite);
        assert_eq!(a.reason.as_deref(), Some(EDITED_AND_MOVED_REASON));
        assert_eq!(a.host_copy, Some(HostCopy::Edited));
        assert!(a.backup);

        // The catalog did not move: M4's reason, now with the signal.
        let hp = plan_for(&catalog_of(&[SKILL]), &Claude, &snap, &manifest, &secrets_map());
        let a = act(&hp, "s");
        assert_eq!(a.op, ActionOp::Overwrite);
        assert_eq!(a.reason.as_deref(), Some(EDITED_ON_HOST_REASON));
        assert_eq!(a.host_copy, Some(HostCopy::Edited));
    }

    /// R2: an entry written before M5 keeps M4's `Update`, but says it could
    /// not rule out a host edit — and carries `Unverified` for the filters.
    #[test]
    fn a_pre_m5_entry_still_updates_but_says_the_copy_is_unverified() {
        let old = substituted(&Claude, &asset(SKILL), &secrets_map());
        let mut entry = Manifest::entry_for(&old.hash(), &old, 0, "personal");
        entry.file_hashes.clear();
        let mut manifest = Manifest {
            version: 1,
            ..Default::default()
        };
        manifest.assets.insert("skill/s".into(), entry);
        let mut snap = HostSnapshot::default();
        satisfy(&mut snap, &old);
        let hp = plan_for(&catalog_of(&[SKILL_V2]), &Claude, &snap, &manifest, &secrets_map());
        let a = act(&hp, "s");
        assert_eq!(a.op, ActionOp::Update);
        assert_eq!(a.host_copy, Some(HostCopy::Unverified));
        assert_eq!(a.reason.as_deref(), Some(UNVERIFIED_UPDATE_REASON));
    }
```

- [ ] **Step 6: Run them to verify they fail**

Run: `cargo fleet-test -- service::catalog::sync::plan`
Expected: FAIL to compile — `no field host_copy on Action`, `cannot find value EDITED_AND_MOVED_REASON`.

- [ ] **Step 7: Implement R2 in `plan.rs`**

Change `use super::manifest::{Manifest, ManifestEntry};` to `use super::manifest::{HostCopy, Manifest, ManifestEntry};`.

Add the field at the end of `pub struct Action` (after `plugin`):

```rust
    /// Assets M5 (Rulings R2/R3): whether the host copy is still what fleet
    /// wrote, when the manifest names this asset. The applier's card
    /// filters read it: an Additive (Rollout, SB6) `Update` needs
    /// `Unchanged`.
    #[serde(skip)]
    pub host_copy: Option<HostCopy>,
```

Add `host_copy: None,` to `blank` in `action_for` (after `plugin: plugin.clone(),`). Next to the `MOVED_*` reasons add:

```rust
/// Rule 4, Assets M5 (Rulings R2): the host copy is not what fleet wrote,
/// and the catalog moved on as well.
pub(crate) const EDITED_AND_MOVED_REASON: &str = "edited on host, and the catalog changed too";
/// Rule 4: the host copy is not what fleet wrote; the catalog is unchanged.
pub(crate) const EDITED_ON_HOST_REASON: &str = "edited on host; the catalog has not changed";
/// Rule 4 (R2): the catalog moved on, and the manifest entry predates the
/// file hashes that would tell whether the host copy was edited too.
pub(crate) const UNVERIFIED_UPDATE_REASON: &str =
    "the catalog changed; the host copy predates fleet's file hashes, so a host edit cannot be ruled out";
```

Right after `let manifest_entry = manifest.assets.get(&Manifest::key(kind, &name));` (`:695`) add:

```rust
    // Assets M5 (R1/R2): which side moved, read once for rule 4 and the filters.
    let host_copy = manifest_entry.map(|e| e.host_copy(snap));
```

Replace the `else { match manifest_entry { … } }` branch of rule 4's `(op, reason)` (the present-and-differing case) with:

```rust
    } else {
        match manifest_entry {
            // F3c: the asset moved, and its new location already holds a
            // different copy the entry never listed — fleet did not write
            // it, so replacing it is an overwrite for a person to see, not
            // a catalog update.
            Some(entry)
                if has_stale_locations(entry, plan) && holds_unlisted_copy(entry, plan, snap) =>
            {
                (ActionOp::Overwrite, Some(MOVED_ONTO_FOREIGN_REASON.into()))
            }
            // Assets M5 (R2): the host copy is not what fleet wrote. Whatever
            // the catalog did, writing it loses that edit: an overwrite.
            Some(entry) if host_copy == Some(HostCopy::Edited) => {
                let why = if entry.hash == plan.hash() {
                    EDITED_ON_HOST_REASON
                } else {
                    EDITED_AND_MOVED_REASON
                };
                (ActionOp::Overwrite, Some(why.into()))
            }
            // Unverified (pre-M5 entry): M4's reading — same render means
            // the difference came from the host.
            Some(entry) if entry.hash == plan.hash() => {
                (ActionOp::Overwrite, Some(EDITED_ON_HOST_REASON.into()))
            }
            Some(_) => (
                ActionOp::Update,
                (host_copy == Some(HostCopy::Unverified))
                    .then(|| UNVERIFIED_UPDATE_REASON.to_string()),
            ),
            None => (
                ActionOp::Overwrite,
                Some("present but differs; not managed".into()),
            ),
        }
    };
```

In the final `Action { … }` of `action_for` (the one after rule 8, ending in `..blank()`), add `host_copy,` before `..blank()`. The plugin early-return `Action { … }` keeps `..blank()` (no host copy: plugins go through plugin ops, never Additive).

Update the rule-4 bullet of `compute_host_plan`'s doc comment: replace "`Update` when the manifest names it with a *different* hash (the catalog moved on), `Overwrite("edited on host")` when the manifest names it with the *same* hash (so the difference came from the host)" with "`Overwrite` when the entry's recorded hashes show the host copy was edited (`HostCopy::Edited`, Assets M5 — whether or not the catalog moved too); otherwise `Update` when the manifest names it with a *different* hash (the catalog moved on; reason `UNVERIFIED_UPDATE_REASON` when the entry predates the file hashes) and `Overwrite("edited on host")` when it names the *same* hash".

Add `host_copy: None,` after `plugin: None,` in every other full `Action { … }` literal `cargo fleet-check` reports (`sync/apply.rs` tests ×3, `sync/mod.rs`, `mcp/tools/tests_catalog_admin.rs`; `changesets/apply.rs` is Step 9).

- [ ] **Step 8: Run the planner tests**

Run: `cargo fleet-test -- service::catalog::sync::plan`
Expected: PASS, including the unchanged `managed_and_stale_is_an_update_managed_and_edited_is_an_overwrite` (its entries record no files, so `Unverified` → M4's ops).

- [ ] **Step 9: Write the failing filter test**

In `changesets/apply.rs` `mod filter_tests`: add `host_copy: None,` after `plugin: None,` in `fn action`, add `use crate::service::catalog::sync::manifest::HostCopy;`, and in `a_card_never_carries_an_overwrite_or_a_remove_to_a_host` replace `action("w4", ActionOp::Update, Some("personal")),` with:

```rust
                Action {
                    host_copy: Some(HostCopy::Unchanged),
                    ..action("w4", ActionOp::Update, Some("personal"))
                },
```

Then add:

```rust
    /// Assets M5 (R3): a Rollout or SB6 updates only a copy the planner
    /// verified untouched; a person's restore may update any copy.
    #[test]
    fn additive_updates_only_a_copy_the_planner_verified_untouched() {
        let mut a = action("w", ActionOp::Update, Some("personal"));
        for (copy, want) in [
            (Some(HostCopy::Unchanged), true),
            (Some(HostCopy::Edited), false),
            (Some(HostCopy::Unverified), false),
            (None, false),
        ] {
            a.host_copy = copy;
            assert_eq!(action_allowed(OpFilter::Additive, &a), want, "{copy:?}");
            assert!(action_allowed(OpFilter::Restore, &a), "restore: {copy:?}");
        }
        assert!(action_allowed(
            OpFilter::Additive,
            &action("w", ActionOp::Create, Some("personal"))
        ));
        assert!(!action_allowed(
            OpFilter::Additive,
            &action("w", ActionOp::Overwrite, Some("personal"))
        ));
    }
```

- [ ] **Step 10: Run it to verify it fails**

Run: `cargo fleet-test -- service::catalog::changesets::apply::filter_tests`
Expected: FAIL to compile — `cannot find function action_allowed`.

- [ ] **Step 11: Implement R3**

Below `op_allowed` in `changesets/apply.rs` add (and import `use crate::service::catalog::sync::manifest::HostCopy;` at the top):

```rust
/// Assets M5 (Rulings R3): whether `a` may go to a host under `f`. On top
/// of [`op_allowed`], an Additive `Update` (Rollout, SB6) needs the planner
/// to have verified the host copy untouched since fleet wrote it — never an
/// edited copy, nor one an entry from before M5 cannot vouch for. A
/// Restore (one asset, one host, a person's pick) may update any copy.
pub(crate) fn action_allowed(f: OpFilter, a: &Action) -> bool {
    op_allowed(f, a.op)
        && (f != OpFilter::Additive
            || a.op != ActionOp::Update
            || a.host_copy == Some(HostCopy::Unchanged))
}
```

In `narrow`, replace `op_allowed(f, a.op)` with `action_allowed(f, a)`. Update the module doc's sentence "A Rollout only creates, adopts or updates" to "A Rollout only creates, adopts or updates a copy the planner verified untouched (Assets M5)".

- [ ] **Step 12: Run the card tests**

Run: `cargo fleet-test -- service::catalog::changesets`
Expected: PASS. A rollout test whose fake host's manifest was hand-built without `file_hashes` and expected an `update` to be applied now sees it dropped — that is R3; rebuild that entry with `Manifest::entry_for(&plan.hash(), &plan, 0, "personal")` (which records the hashes) rather than loosening the filter.

- [ ] **Step 13: Checkpoint and commit**

Run: `cargo fleet-check`, `cargo fmt --all --check`, `cargo fleet-lint`.

```bash
git add crates/fleet-core/src/service/catalog/sync/manifest.rs crates/fleet-core/src/service/catalog/sync/plan.rs \
  crates/fleet-core/src/service/catalog/sync/apply.rs crates/fleet-core/src/service/catalog/sync/mod.rs \
  crates/fleet-core/src/service/catalog/changesets/apply.rs crates/fleet-core/src/mcp/tools/tests_catalog_admin.rs
git commit -m "feat(sync): the manifest records file hashes, so a plan tells an edited host copy from a stale one

A catalog change over a copy someone edited on the host is now an overwrite
(backed up, red in the plan) instead of an update that silently dropped the
edit; Rollout and SB6 update only a copy the planner verified untouched.
Entries written before this read as unverified and keep the old update."
```

---

### Task 2: Which side moved — `drift_side` in the inventory, the Drift rule, and SB6 (carry 1, R4–R6)

**Files:**
- Create: `crates/fleet-core/migrations/096_inventory_drift_side.sql`
- Modify: `crates/fleet-core/src/store/schema.rs` (`MIGRATIONS` after `:1028`, a guard next to `asset_inventory_has_catalog_id` `:399`, tests)
- Modify: `crates/fleet-core/src/store/rows.rs` (`AssetInventoryRow` `:1223`)
- Modify: `crates/fleet-core/src/store/catalog.rs` (`replace_host_inventory` `:338`, `list_inventory` `:357`)
- Modify: `crates/fleet-core/src/service/catalog/inventory.rs` (`compute_states` `:347-470`, tests)
- Modify: `crates/fleet-core/src/service/catalog/mod.rs` (`HostState` `:356`, `host_states` `:407`, every `HostState { … }` literal)
- Modify: `crates/fleet-core/src/service/catalog/changesets/rules.rs` (`DriftFacts` `:52`, `drift_cards` `:641`, tests)
- Modify: `crates/fleet-core/src/service/catalog/changesets/reconcile.rs` (`proposals` `:158-170`, tests)
- Modify: `crates/fleet-core/src/service/catalog/changesets/apply.rs` (`auto_additive` `:1880-1900`, tests)

**Interfaces:**
- Consumes: `ManifestEntry::host_copy`, `HostCopy` (Task 1).
- Produces:
  - `AssetInventoryRow.drift_side: Option<String>` (`"host" | "catalog"`, `#[serde(default, skip_serializing_if = "Option::is_none")]`).
  - `HostState.drift_side: Option<String>` (same attributes) — what the frontend reads (Task 6).
  - `DriftFacts.edited: bool`.
  - `fn drift_facts(rows: &[AssetInventoryRow]) -> Vec<DriftFacts>` (private, `reconcile`).
  - `pub(crate) fn sb6_due(r: &AssetInventoryRow) -> bool` (`changesets::apply`).

- [ ] **Step 1: Write the failing migration test**

In `store/schema.rs` `mod tests`, next to `migration_092_…`:

```rust
    /// Assets M5 (R4): `asset_inventory.drift_side`, NULL on every existing
    /// row, and a re-run is guarded (ADD COLUMN is not idempotent).
    #[test]
    fn migration_096_adds_drift_side_as_null_and_is_safe_to_rerun() {
        let s = store_at_version(95);
        s.conn
            .execute_batch(
                "INSERT INTO hosts (alias, reachable, provisioned) VALUES ('h', 1, 1); \
                 INSERT INTO asset_inventory (host_alias, harness, kind, name, state, scanned_at) \
                   VALUES ('h', 'claude', 'skill', 's', 'drifted', 1);",
            )
            .unwrap();
        assert!(!asset_inventory_has_drift_side(&s.conn).unwrap());
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert!(asset_inventory_has_drift_side(&s.conn).unwrap());
        let side: Option<String> = s
            .conn
            .query_row("SELECT drift_side FROM asset_inventory", [], |r| r.get(0))
            .unwrap();
        assert_eq!(side, None);
        s.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 96;")
            .unwrap();
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo fleet-test -- store::schema::tests::migration_096`
Expected: FAIL to compile — `cannot find function asset_inventory_has_drift_side`.

- [ ] **Step 3: Add the migration**

Create `crates/fleet-core/migrations/096_inventory_drift_side.sql`:

```sql
-- Assets M5 (Rulings R4): on a `drifted` managed inventory row, which side
-- moved. 'host' = the copy on the host is no longer what fleet wrote (a
-- person edited it); 'catalog' = the host copy is exactly what fleet wrote
-- and the catalog moved on, so a sync updates it safely; NULL = not
-- drifted, not managed, or the host's manifest entry predates the file
-- hashes that tell the two apart. ADD COLUMN is not idempotent: guarded in
-- schema.rs.
ALTER TABLE asset_inventory ADD COLUMN drift_side TEXT;

INSERT OR IGNORE INTO schema_version (version) VALUES (96);
```

In `store/schema.rs` add after the 95 entry of `MIGRATIONS`:

```rust
    // Assets M5: which side moved on a drifted managed row. ADD COLUMN is
    // not idempotent: the same guard 087 uses.
    Migration {
        version: 96,
        sql: include_str!("../../migrations/096_inventory_drift_side.sql"),
        already_applied: Some(asset_inventory_has_drift_side),
    },
```

and next to `asset_inventory_has_catalog_id`:

```rust
/// `already_applied` guard of migration 096 (`drift_side` on
/// `asset_inventory`, Assets M5).
fn asset_inventory_has_drift_side(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('asset_inventory') WHERE name = 'drift_side'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}
```

In `store/rows.rs`, at the end of `AssetInventoryRow`:

```rust
    /// Assets M5 (migration 096, Rulings R4): on a `drifted` managed row,
    /// `host` (edited there) or `catalog` (the host copy is as fleet wrote
    /// it; the catalog moved on). `None` otherwise, or when the manifest
    /// entry cannot tell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drift_side: Option<String>,
```

In `store/catalog.rs` `replace_host_inventory`, the INSERT becomes

```rust
                "INSERT INTO asset_inventory (host_alias, harness, kind, name, state, catalog_hash, host_hash, scanned_at, managed, secret_like, fleet_owned, catalog_id, drift_side)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, (SELECT id FROM catalogs WHERE id = ?12), ?13)",
```

with `r.drift_side,` appended to its `params!`; `list_inventory`'s SELECT appends `, drift_side` and its row closure `drift_side: row.get(12)?,`.

- [ ] **Step 4: Run the migration test and the store tests**

Run: `cargo fleet-test -- store::schema::tests::migration_096` then `cargo fleet-test -- store::catalog`
Expected: PASS.

- [ ] **Step 5: Write the failing inventory test**

In `inventory.rs` `mod tests` (add `use crate::service::catalog::model::sha256_hex;` and `use crate::service::catalog::sync::manifest::Manifest;` if absent):

```rust
    /// Assets M5 (R4): a drifted managed row says which side moved.
    #[test]
    fn a_drifted_managed_row_says_which_side_moved() {
        let skill = |desc: &str| {
            let mut a = Asset::from_yaml(
                None,
                &format!("kind: skill\nname: s\ndescription: {desc}\n"),
            )
            .unwrap();
            a.body = "body\n".into();
            a
        };
        let old = Claude.render(&skill("d")).unwrap();
        let mut manifest = Manifest {
            version: 1,
            ..Default::default()
        };
        manifest.assets.insert(
            "skill/s".into(),
            Manifest::entry_for(&old.hash(), &old, 0, "personal"),
        );
        let mut snap = HostSnapshot::default();
        for f in &old.files {
            snap.files.insert(f.path.clone(), sha256_hex(&f.bytes));
        }
        let moved = Catalog {
            assets: vec![skill("d2")],
            ..Default::default()
        };
        let side = |catalog: &Catalog, snap: &HostSnapshot, manifest: &Manifest| {
            let rows = compute_states(catalog, &Claude, "oci", snap, manifest, &empty(), 1);
            let r = rows.into_iter().find(|r| r.name == "s").unwrap();
            (r.state, r.drift_side)
        };
        assert_eq!(
            side(&moved, &snap, &manifest),
            ("drifted".to_string(), Some("catalog".to_string()))
        );
        let mut edited = snap.clone();
        edited
            .files
            .insert(old.files[0].path.clone(), sha256_hex(b"edited"));
        assert_eq!(
            side(&moved, &edited, &manifest),
            ("drifted".to_string(), Some("host".to_string()))
        );
        let mut pre_m5 = manifest.clone();
        pre_m5.assets.get_mut("skill/s").unwrap().file_hashes.clear();
        assert_eq!(side(&moved, &snap, &pre_m5), ("drifted".to_string(), None));
        let same = Catalog {
            assets: vec![skill("d")],
            ..Default::default()
        };
        assert_eq!(side(&same, &snap, &manifest), ("in_sync".to_string(), None));
    }
```

- [ ] **Step 6: Run it to verify it fails**

Run: `cargo fleet-test -- service::catalog::inventory`
Expected: FAIL — `left: ("drifted", None)` for the first assertion (the field exists from Step 3 but is never set).

- [ ] **Step 7: Compute `drift_side` in `compute_states`**

Import `use super::sync::manifest::HostCopy;` (next to the existing manifest import). In the catalog-asset loop, after `let host_hash = …;` and before `rows.push(AssetInventoryRow { … })`, add:

```rust
        // Assets M5 (R4): which side moved — only for a drifted copy fleet
        // manages, read from what its manifest entry recorded.
        let drift_side = (state == AssetState::Drifted)
            .then(|| manifest.assets.get(&Manifest::key(asset.kind(), &asset.header.name)))
            .flatten()
            .and_then(|entry| match entry.host_copy(snap) {
                HostCopy::Edited => Some("host"),
                HostCopy::Unchanged => Some("catalog"),
                HostCopy::Unverified => None,
            })
            .map(String::from);
```

and add `drift_side,` to that row's literal (before `..base`). If `AssetState` does not derive `PartialEq`, compare with `matches!(state, AssetState::Drifted)`.

- [ ] **Step 8: Carry it on `HostState`**

In `service/catalog/mod.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostState {
    pub host_alias: String,
    pub harness: String,
    pub state: String,
    /// Assets M5 (R4): `host` | `catalog` on a drifted managed copy, so a
    /// client that cannot read the inventory (an ungranted hub client) still
    /// sees which side moved. Absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drift_side: Option<String>,
}
```

In `host_states`, the map becomes `HostState { host_alias: r.host_alias.clone(), harness: r.harness.clone(), state: r.state.clone(), drift_side: r.drift_side.clone() }`. Add `drift_side: None,` to every other `HostState { … }` literal `cargo fleet-check` reports (e.g. `list_assets_lists_orphan_rows_as_unmanaged`).

- [ ] **Step 9: Run the inventory and catalog tests**

Run: `cargo fleet-test -- service::catalog::inventory` then `cargo fleet-test -- service::catalog::tests`
Expected: PASS.

- [ ] **Step 10: Write the failing rule tests**

In `rules.rs` `mod tests`: add `edited: false,` to the existing `DriftFacts { … }` literal in `drift_offers_take_or_restore_and_rollout_covers_a_new_layers_gaps`, then add:

```rust
    /// Assets M5 (R5): a copy a person edited says so in its card.
    #[test]
    fn a_drift_card_says_the_copy_was_edited_on_its_host() {
        let mut f = fleet(false, vec![]);
        f.bootstrapped = true;
        f.drifted = vec![DriftFacts {
            catalog_id: PERSONAL,
            kind: "skill".into(),
            name: "w".into(),
            host: "trn".into(),
            host_hash: Some("e".into()),
            edited: true,
        }];
        let cards = propose(&f.input());
        let drift = cards.iter().find(|c| c.kind == CardKind::Drift).unwrap();
        assert_eq!(drift.summary, "skill/w was edited on trn (catalog personal)");
        assert_eq!(drift.subject(), format!("drift:{PERSONAL}:skill/w@trn"));
    }
```

In `reconcile.rs` `mod tests` add:

```rust
    /// Assets M5 (R5): a copy that is only behind its catalog is not drift a
    /// person must decide — no card; an edited or unknown one is.
    #[test]
    fn only_edited_or_unverified_copies_become_drift_facts() {
        let row = |host: &str, side: Option<&str>| AssetInventoryRow {
            host_alias: host.into(),
            harness: "claude".into(),
            kind: "skill".into(),
            name: "w".into(),
            state: "drifted".into(),
            managed: true,
            catalog_id: Some(1),
            drift_side: side.map(String::from),
            ..Default::default()
        };
        let facts = drift_facts(&[
            row("oci", Some("catalog")),
            row("trn", Some("host")),
            row("htz", None),
        ]);
        let got: Vec<(&str, bool)> = facts.iter().map(|d| (d.host.as_str(), d.edited)).collect();
        assert_eq!(got, [("trn", true), ("htz", false)]);
    }
```

In `apply.rs` `mod filter_tests` add (import `crate::store::AssetInventoryRow`):

```rust
    /// Assets M5 (R6): SB6 brings a missing copy, adopts an identical one,
    /// and now updates a managed copy that is only behind its catalog —
    /// never an edited or unverified one.
    #[test]
    fn sb6_brings_a_copy_that_is_only_behind_and_leaves_an_edited_one() {
        let row = |state: &str, managed: bool, side: Option<&str>| AssetInventoryRow {
            state: state.into(),
            managed,
            drift_side: side.map(String::from),
            ..Default::default()
        };
        assert!(sb6_due(&row("missing", false, None)));
        assert!(sb6_due(&row("in_sync", false, None)));
        assert!(!sb6_due(&row("in_sync", true, None)));
        assert!(sb6_due(&row("drifted", true, Some("catalog"))));
        assert!(!sb6_due(&row("drifted", true, Some("host"))));
        assert!(!sb6_due(&row("drifted", true, None)));
        assert!(!sb6_due(&row("drifted", false, Some("catalog"))));
        assert!(!sb6_due(&row("orphan", true, None)));
    }
```

- [ ] **Step 11: Run them to verify they fail**

Run: `cargo fleet-test -- service::catalog::changesets`
Expected: FAIL to compile — `no field edited`, `cannot find function drift_facts`, `cannot find function sb6_due`.

- [ ] **Step 12: Implement R5 and R6**

`rules.rs` — add to `DriftFacts`:

```rust
    /// Assets M5 (R5): the host copy is no longer what fleet wrote
    /// (`drift_side = host`); `false` when the side is unknown.
    pub edited: bool,
```

and in `drift_cards` replace the `summary: format!(…)` with:

```rust
                summary: if d.edited {
                    format!("{}/{} was edited on {} (catalog {cat})", d.kind, d.name, d.host)
                } else {
                    format!("{}/{} differs on {} from catalog {cat}", d.kind, d.name, d.host)
                },
```

`reconcile.rs` — add above `proposals`:

```rust
/// Assets M5 (R5): the drifted managed Claude rows a Drift card is for. A
/// copy that is only behind its catalog (`drift_side = catalog`) is not one:
/// a Rollout, SB6 or a person's sync brings it up to date with no pick to
/// make, so a catalog-only change no longer opens a card per host (an open
/// one is withdrawn by the pass, its subject no longer produced).
fn drift_facts(rows: &[AssetInventoryRow]) -> Vec<DriftFacts> {
    rows.iter()
        .filter(|r| r.state == "drifted" && r.managed && r.harness == "claude")
        .filter(|r| r.drift_side.as_deref() != Some("catalog"))
        .filter_map(|r| {
            Some(DriftFacts {
                catalog_id: r.catalog_id?,
                kind: r.kind.clone(),
                name: r.name.clone(),
                host: r.host_alias.clone(),
                host_hash: r.host_hash.clone(),
                edited: r.drift_side.as_deref() == Some("host"),
            })
        })
        .collect()
}
```

and in `proposals` replace the `let mut drifted: Vec<DriftFacts> = rows.iter()….collect();` expression with `let mut drifted = drift_facts(&rows);` (the sort below stays).

`apply.rs` — add above `auto_additive`:

```rust
/// Assets M5 (R6): whether SB6 brings `r` up to date — missing; present,
/// identical and not yet managed (adopt); or managed and only behind its
/// catalog (`drift_side = catalog`). A copy a person edited, or one its
/// manifest entry cannot vouch for, waits for a person. The planner
/// re-checks the fresh snapshot (R2) and `action_allowed` (R3) drops any
/// update it cannot verify, so a copy edited after the scan is still safe.
pub(crate) fn sb6_due(r: &AssetInventoryRow) -> bool {
    match r.state.as_str() {
        "missing" => true,
        "in_sync" => !r.managed,
        "drifted" => r.managed && r.drift_side.as_deref() == Some("catalog"),
        _ => false,
    }
}
```

In `auto_additive`, replace the `.is_some_and(|r| match r.state.as_str() { … })` closure with `.is_some_and(sb6_due)`, and rewrite `auto_additive`'s doc sentence "A `drifted` copy — managed or not — is never due (final review I1) …" to "A `drifted` copy is due only when it is managed and only behind its catalog ([`sb6_due`], Assets M5); the planner turns an edited one into an overwrite, which SB6 never applies."

- [ ] **Step 13: Run the card tests**

Run: `cargo fleet-test -- service::catalog::changesets`
Expected: PASS, including `sb6_never_touches_a_managed_drifted_copy_on_a_rolled_out_layer` (its row has no `drift_side`).

- [ ] **Step 14: Checkpoint and commit**

Run: `cargo fleet-check`, `cargo fmt --all --check`, `cargo fleet-lint`.

```bash
git add crates/fleet-core/migrations/096_inventory_drift_side.sql crates/fleet-core/src/store \
  crates/fleet-core/src/service/catalog/inventory.rs crates/fleet-core/src/service/catalog/mod.rs \
  crates/fleet-core/src/service/catalog/changesets
git commit -m "feat(changesets): a drift card is only for a copy someone edited

The inventory records which side moved (migration 096: drift_side). A copy
that is only behind its catalog no longer opens a drift card per host; SB6
brings it up to date instead. An edited copy's card says where it was
edited, and HostState carries the side to clients."
```

---

### Task 3: The other M4 carries — slug collisions, decision time, per-tick cost and retention, person-first last sync (carries 2, 4, 5, 6; R7–R10)

**Files:**
- Create: `crates/fleet-core/migrations/097_changeset_item_decided_at.sql`
- Modify: `crates/fleet-core/src/store/schema.rs` (`MIGRATIONS`, a guard, a test)
- Modify: `crates/fleet-core/src/store/changesets.rs` (`ChangesetItemRow` `:33`, `ITEM_COLS`/`item_row` `:93-123`, `set_item_states` `:147`, `replace_changeset_items_keeping` `:282`, new `prune_withdrawn_changesets`, tests)
- Modify: `crates/fleet-core/src/store/catalog.rs` (new `last_person_sync_run`, a test)
- Modify: `crates/fleet-core/src/service/catalog/changesets/rules.rs` (`look_item` `:368`, `bootstrap_card` `:400`, tests)
- Modify: `crates/fleet-core/src/service/catalog/changesets/apply.rs` (`rejected_rollouts` `:2005`, `auto_additive` registry read `:1858`, tests)
- Modify: `crates/fleet-core/src/service/catalog/changesets/mod.rs` (`list` `:470`, new `undoable_ids`, tests)
- Modify: `crates/fleet-core/src/service/catalog/changesets/reconcile.rs` (`reconcile` `:63`, `gather` `:92-125`)
- Modify: `crates/fleet-core/src/service/catalog/sync/mod.rs` (`last_sync` `:792`, a test)

**Interfaces:**
- Consumes: `store::now_unix_ms`, `reconcile::WITHDRAWN` (`"withdrawn: no longer applies"`), `changes_catalog`, `applied_catalogs`.
- Produces:
  - `ChangesetItemRow.decided_at: Option<i64>` (Unix ms, `#[serde(default)]`).
  - `Store::prune_withdrawn_changesets(&self, created_before: i64) -> rusqlite::Result<usize>`.
  - `Store::last_person_sync_run(&self) -> rusqlite::Result<Option<SyncRunRow>>`.
  - `pub(super) fn rejected_from(cards: &[(ChangesetRow, Vec<ChangesetItemRow>)]) -> BTreeSet<(i64, String, String)>` (`changesets::apply`).
  - `pub(crate) fn undoable_ids(cards: &[(ChangesetRow, Vec<ChangesetItemRow>)]) -> BTreeSet<i64>` (`changesets`).
  - `pub const WITHDRAWN_RETENTION_SECS: i64 = 7 * 24 * 3600;` (`changesets::reconcile`).

- [ ] **Step 1: Write the failing slug-collision rule test (R7)**

In `rules.rs` `mod tests`:

```rust
    /// Assets M5 (R7): two names that import as one slug both need a look —
    /// the card still applies, instead of failing on the second import.
    #[test]
    fn names_that_import_as_one_slug_need_a_look_instead_of_failing_the_card() {
        let f = fleet(
            false,
            vec![
                row("local", "skill", "My_Skill", "h1"),
                row("local", "skill", "my-skill", "h2"),
                row("local", "skill", "other", "h3"),
            ],
        );
        let card = &propose(&f.input())[0];
        assert_eq!(card.kind, CardKind::Bootstrap);
        let looks: Vec<(&str, &str)> = card
            .items
            .iter()
            .filter(|i| i.grp == NEEDS_A_LOOK)
            .map(|i| (i.name.as_str(), i.params.reason.as_deref().unwrap_or("")))
            .collect();
        assert_eq!(
            looks,
            [
                ("My_Skill", "imports as my-skill, as my-skill does"),
                ("my-skill", "imports as my-skill, as My_Skill does"),
            ]
        );
        assert_eq!(imports(card).values().sum::<usize>(), 1, "`other` still imports");
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo fleet-test -- service::catalog::changesets::rules`
Expected: FAIL — `looks` is empty (both names are grouped for import).

- [ ] **Step 3: Implement R7 in `rules.rs`**

Replace `look_item` with a reason-taking pair:

```rust
/// An import a person must name to apply (R8), with why. Its catalog is
/// where it would go — an org catalog that failed to load included (Task 4
/// review M3), so apply refuses it until that catalog loads (R11) rather
/// than adopting it into personal.
fn look_item(id: &AssetIdentity, input: &RulesInput<'_>) -> ProposedItem {
    let reason = if id.class == IdentityClass::NeedsPerson {
        id.reason.clone().unwrap_or_else(|| "needs a person".into())
    } else {
        match &destination(id, input) {
            Err(e) => e.clone(),
            Ok(_) if source_host(id).is_none() => NO_CLAUDE_COPY.to_string(),
            Ok(_) => NO_LAYER.to_string(),
        }
    };
    look_item_because(id, input, reason)
}

/// [`look_item`] with the reason given.
fn look_item_because(id: &AssetIdentity, input: &RulesInput<'_>, reason: String) -> ProposedItem {
    let catalog_id = org_catalog(id, input)
        .map(|c| c.id)
        .or_else(|| personal_id(input));
    import_item(
        id,
        NEEDS_A_LOOK,
        catalog_id,
        source_host(id),
        None,
        Some(reason),
    )
}

/// Assets M5 (R7): the eligible normal identities whose slug another of a
/// different name shares (`My_Skill` and `my-skill` both import as
/// `my-skill`) → why. One import would create the slug and the other's
/// group would fail the whole card at apply, so both need a look.
fn slug_collisions(eligible: &[&AssetIdentity]) -> BTreeMap<(String, String), String> {
    let mut by_slug: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
    for id in eligible.iter().filter(|i| i.class == IdentityClass::Normal) {
        by_slug
            .entry((id.kind.clone(), slugify(&id.name)))
            .or_default()
            .insert(id.name.clone());
    }
    let mut out = BTreeMap::new();
    for ((kind, slug), names) in by_slug.into_iter().filter(|(_, n)| n.len() > 1) {
        for name in &names {
            let others: Vec<&str> = names
                .iter()
                .filter(|n| *n != name)
                .map(String::as_str)
                .collect();
            out.insert(
                (kind.clone(), name.clone()),
                format!("imports as {slug}, as {} does", others.join(", ")),
            );
        }
    }
    out
}
```

In `bootstrap_card`, before the `for &id in eligible` loop add `let collisions = slug_collisions(eligible);`, and inside the loop, right after the `if is_internal(id) { … continue; }` block:

```rust
        if let Some(why) = collisions.get(&(id.kind.clone(), id.name.clone())) {
            looks.push(look_item_because(id, input, why.clone()));
            continue;
        }
```

- [ ] **Step 4: Run the rule tests**

Run: `cargo fleet-test -- service::catalog::changesets::rules`
Expected: PASS (the live-fixture test included: its names are already slugs).

- [ ] **Step 5: Write the failing decision-time tests (R8)**

In `store/schema.rs` `mod tests`:

```rust
    /// Assets M5 (R8): `changeset_items.decided_at`, NULL on existing items,
    /// guarded on re-run.
    #[test]
    fn migration_097_adds_decided_at_as_null_and_is_safe_to_rerun() {
        let s = store_at_version(96);
        assert!(!changeset_items_has_decided_at(&s.conn).unwrap());
        s.migrate().unwrap();
        assert!(changeset_items_has_decided_at(&s.conn).unwrap());
        s.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 97;")
            .unwrap();
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    }
```

In `store/changesets.rs` `mod tests` (its `item(grp, action, catalog_id, params)` helper names items `<grp>-<action>`):

```rust
    /// Assets M5 (R8): an item's decision time is stamped when it leaves
    /// pending, kept when a refresh re-rejects it, cleared when it goes back.
    #[test]
    fn item_decision_times_are_stamped_kept_and_cleared() {
        let s = Store::open_in_memory().unwrap();
        s.set_catalog_config("/p", None).unwrap();
        let p = s.personal_catalog().unwrap().unwrap().id;
        let items = [
            item("core", "set_scope", Some(p), None),
            item("core", "assign_layer", Some(p), None),
        ];
        let card = s.insert_changeset("bootstrap", "Adopt 2", &items).unwrap();
        assert!(s
            .changeset_items(card.id)
            .unwrap()
            .iter()
            .all(|i| i.decided_at.is_none()));

        s.set_changeset_item_states(card.id, &[1], "rejected").unwrap();
        let at = s.changeset_items(card.id).unwrap()[1]
            .decided_at
            .expect("stamped when rejected");
        s.conn
            .execute(
                "UPDATE changeset_items SET decided_at = ?2 WHERE changeset_id = ?1 AND position = 1",
                rusqlite::params![card.id, at - 60_000],
            )
            .unwrap();
        assert!(s
            .replace_changeset_items_keeping(card.id, "Adopt 2", &items, |_| vec![1])
            .unwrap());
        assert_eq!(
            s.changeset_items(card.id).unwrap()[1].decided_at,
            Some(at - 60_000),
            "a refresh keeps when the person decided"
        );

        s.set_changeset_item_states(card.id, &[1], "pending").unwrap();
        assert_eq!(s.changeset_items(card.id).unwrap()[1].decided_at, None);
    }
```

In `changesets/apply.rs` `mod filter_tests` (import `crate::store::{ChangesetItemRow, ChangesetRow}`):

```rust
    /// Assets M5 (R8): the later DECISION wins per (catalog, layer, host),
    /// not the larger card id; an item decided before 097 falls back to its
    /// card's apply time.
    #[test]
    fn a_later_decision_wins_by_when_it_was_made_not_by_card_id() {
        let card = |id: i64, applied_at: Option<i64>| ChangesetRow {
            id,
            kind: "rollout".into(),
            summary: "Roll out core".into(),
            state: "applied".into(),
            created_at: 1,
            applied_at,
            commits: None,
            layers_snapshot: None,
            error: None,
        };
        let sync = |card: i64, state: &str, decided_at: Option<i64>| ChangesetItemRow {
            changeset_id: card,
            position: 0,
            grp: "core".into(),
            catalog_id: Some(1),
            kind: "host".into(),
            name: "oci".into(),
            action: "sync".into(),
            params: Some(r#"{"layer":"core","assets":["skill/w"]}"#.into()),
            decider: "person".into(),
            state: state.into(),
            decided_at,
        };
        let key = (1, "core".to_string(), "oci".to_string());
        // Card 9 rejected oci at t=100; card 5 (an OLDER id) applied it at t=200.
        let applied_later = vec![
            (card(5, Some(200)), vec![sync(5, "applied", Some(200))]),
            (card(9, None), vec![sync(9, "rejected", Some(100))]),
        ];
        assert!(!rejected_from(&applied_later).contains(&key));
        let rejected_later = vec![
            (card(5, Some(200)), vec![sync(5, "applied", Some(200))]),
            (card(9, None), vec![sync(9, "rejected", Some(300))]),
        ];
        assert!(rejected_from(&rejected_later).contains(&key));
        let pre_097 = vec![
            (card(5, Some(200)), vec![sync(5, "applied", None)]),
            (card(9, None), vec![sync(9, "rejected", Some(150))]),
        ];
        assert!(!rejected_from(&pre_097).contains(&key), "falls back to applied_at");
    }
```

Add `decided_at: None,` to every other `ChangesetItemRow { … }` literal `cargo fleet-check` reports (`changesets/mod.rs` test `stored`, and two more).

- [ ] **Step 6: Run them to verify they fail**

Run: `cargo fleet-test -- store::changesets` and `cargo fleet-test -- service::catalog::changesets::apply::filter_tests`
Expected: FAIL to compile — `no field decided_at`, `cannot find function changeset_items_has_decided_at`, `cannot find function rejected_from`.

- [ ] **Step 7: Implement R8**

Create `crates/fleet-core/migrations/097_changeset_item_decided_at.sql`:

```sql
-- Assets M5 (Rulings R8): when an apply or a person decided a changeset
-- item, Unix MILLISECONDS; NULL while pending and on every item decided
-- before this migration. `rejected_rollouts` orders decisions by it. ADD
-- COLUMN is not idempotent: guarded in schema.rs.
ALTER TABLE changeset_items ADD COLUMN decided_at INTEGER;

INSERT OR IGNORE INTO schema_version (version) VALUES (97);
```

`store/schema.rs`: after the 96 entry,

```rust
    // Assets M5: when a changeset item was decided. ADD COLUMN is not
    // idempotent: guarded.
    Migration {
        version: 97,
        sql: include_str!("../../migrations/097_changeset_item_decided_at.sql"),
        already_applied: Some(changeset_items_has_decided_at),
    },
```

and

```rust
/// `already_applied` guard of migration 097 (`decided_at` on
/// `changeset_items`, Assets M5).
fn changeset_items_has_decided_at(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('changeset_items') WHERE name = 'decided_at'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}
```

`store/changesets.rs`: add to `ChangesetItemRow`

```rust
    /// Assets M5 (migration 097, R8): when it left `pending`, Unix ms;
    /// `None` while pending and on items decided before 097.
    #[serde(default)]
    pub decided_at: Option<i64>,
```

`ITEM_COLS` gains `, decided_at`; `item_row` gains `decided_at: r.get(10)?,`. `set_item_states` becomes:

```rust
fn set_item_states(
    conn: &rusqlite::Connection,
    id: i64,
    positions: &[i64],
    state: &str,
) -> Result<()> {
    // R8: leaving `pending` is a decision, stamped; going back clears it.
    let mut stmt = conn.prepare(
        "UPDATE changeset_items SET state = ?3, \
           decided_at = CASE WHEN ?3 = 'pending' THEN NULL ELSE ?4 END \
         WHERE changeset_id = ?1 AND position = ?2",
    )?;
    let at = super::now_unix_ms();
    for p in positions {
        stmt.execute(rusqlite::params![id, p, state, at])?;
    }
    Ok(())
}
```

In `replace_changeset_items_keeping`, keep the person's time: after `let keep = rejected(&old);` add

```rust
        // R8: a re-rejected item keeps when the person decided it — matched
        // on what the item is, since positions change across a refresh.
        type Same = (String, String, String, String, Option<i64>);
        let decided: std::collections::BTreeMap<Same, i64> = old
            .iter()
            .filter(|i| i.state == "rejected")
            .filter_map(|i| {
                let at = i.decided_at?;
                Some(((i.grp.clone(), i.kind.clone(), i.name.clone(), i.action.clone(), i.catalog_id), at))
            })
            .collect();
        let now = super::now_unix_ms();
```

and replace the re-reject block with

```rust
        {
            let mut stmt = tx.prepare(
                "UPDATE changeset_items SET state = 'rejected', decided_at = ?3 \
                 WHERE changeset_id = ?1 AND position = ?2",
            )?;
            for p in keep {
                let at = items
                    .get(p as usize)
                    .and_then(|it| {
                        decided
                            .get(&(it.grp.clone(), it.kind.clone(), it.name.clone(), it.action.clone(), it.catalog_id))
                            .copied()
                    })
                    .unwrap_or(now);
                stmt.execute(rusqlite::params![id, p, at])?;
            }
        }
```

`changesets/apply.rs`: replace `rejected_rollouts` with

```rust
/// One rollout `sync` decision, for [`rejected_from`].
struct Decision {
    at: i64,
    card: i64,
    position: i64,
    key: (i64, String, String),
    rejected: bool,
}

/// Assets M5 (R8): the `(catalog, layer, host)` whose latest decision on a
/// rollout `sync` item was a rejection — "latest" by when it was decided:
/// the item's `decided_at`, else (decided before 097) its card's
/// `applied_at`, else the card's creation; then card id and position. Pure.
pub(super) fn rejected_from(
    cards: &[(ChangesetRow, Vec<ChangesetItemRow>)],
) -> BTreeSet<(i64, String, String)> {
    let mut decisions = Vec::new();
    for (card, items) in cards
        .iter()
        .filter(|(c, _)| c.kind == CardKind::Rollout.as_str())
    {
        for i in items.iter().filter(|i| i.action == ItemAction::Sync.as_str()) {
            let rejected = match i.state.as_str() {
                "rejected" => true,
                "applied" => false,
                _ => continue,
            };
            let (Some(cid), Some(layer)) =
                (i.catalog_id, ItemParams::parse(i.params.as_deref()).layer)
            else {
                continue;
            };
            decisions.push(Decision {
                at: i
                    .decided_at
                    .or(card.applied_at)
                    .unwrap_or(card.created_at.saturating_mul(1000)),
                card: card.id,
                position: i.position,
                key: (cid, layer, i.name.clone()),
                rejected,
            });
        }
    }
    decisions.sort_by_key(|d| (d.at, d.card, d.position));
    let mut last: BTreeMap<(i64, String, String), bool> = BTreeMap::new();
    for d in decisions {
        last.insert(d.key, d.rejected);
    }
    last.into_iter()
        .filter_map(|(k, rejected)| rejected.then_some(k))
        .collect()
}

/// [`rejected_from`] over the stored rollout cards.
pub(super) fn rejected_rollouts(s: &Store) -> Result<BTreeSet<(i64, String, String)>, IpcError> {
    let mut cards = Vec::new();
    for c in s
        .list_changesets()?
        .into_iter()
        .filter(|c| c.kind == CardKind::Rollout.as_str())
    {
        let items = s.changeset_items(c.id)?;
        cards.push((c, items));
    }
    Ok(rejected_from(&cards))
}
```

- [ ] **Step 8: Run the decision-time tests**

Run: `cargo fleet-test -- store::schema::tests::migration_097`, `cargo fleet-test -- store::changesets`, `cargo fleet-test -- service::catalog::changesets`
Expected: PASS.

- [ ] **Step 9: Write the failing retention and undoability tests (R9)**

In `store/changesets.rs` `mod tests`:

```rust
    /// Assets M5 (R9): only withdrawn cards that are old and that nobody
    /// ever decided an item of are pruned, with their items.
    #[test]
    fn only_old_untouched_withdrawn_cards_are_pruned() {
        let s = Store::open_in_memory().unwrap();
        s.set_catalog_config("/p", None).unwrap();
        let p = s.personal_catalog().unwrap().unwrap().id;
        let mk = || {
            s.insert_changeset("new", "New on oci", &[item("core", "import", Some(p), None)])
                .unwrap()
                .id
        };
        let (old, touched, open, fresh, by_person) = (mk(), mk(), mk(), mk(), mk());
        for id in [old, touched, fresh] {
            assert!(s.withdraw_changeset(id, "withdrawn: no longer applies").unwrap());
        }
        s.set_changeset_item_states(touched, &[0], "rejected").unwrap();
        s.reject_changeset_items(by_person, &[0], &[], true).unwrap();
        s.conn
            .execute("UPDATE changesets SET created_at = 1 WHERE id <> ?1", [fresh])
            .unwrap();
        assert_eq!(s.prune_withdrawn_changesets(100).unwrap(), 1);
        assert!(s.get_changeset(old).unwrap().is_none());
        assert!(s.changeset_items(old).unwrap().is_empty(), "items go with it");
        for id in [touched, open, fresh, by_person] {
            assert!(s.get_changeset(id).unwrap().is_some(), "card {id} stays");
        }
    }
```

In `changesets/mod.rs` `mod tests`:

```rust
    /// Assets M5 (R9): one linear pass answers what `later_card` answers per
    /// card — the latest applied catalog-changing card of every catalog it
    /// touched is undoable, and nothing else.
    #[test]
    fn undoable_is_the_latest_applied_card_in_each_of_its_catalogs() {
        let card = |id: i64, kind: &str, applied_at: Option<i64>| ChangesetRow {
            id,
            kind: kind.into(),
            summary: "s".into(),
            state: if applied_at.is_some() { "applied" } else { "proposed" }.into(),
            created_at: 1,
            applied_at,
            commits: None,
            layers_snapshot: None,
            error: None,
        };
        let import = |card: i64, catalog: i64| ChangesetItemRow {
            changeset_id: card,
            position: catalog,
            grp: "core".into(),
            catalog_id: Some(catalog),
            kind: "skill".into(),
            name: "w".into(),
            action: "import".into(),
            params: None,
            decider: "rule".into(),
            state: "applied".into(),
            decided_at: None,
        };
        let cards = vec![
            (card(1, "bootstrap", Some(10)), vec![import(1, 1)]),
            (card(2, "new", Some(20)), vec![import(2, 1)]),
            (card(3, "new", Some(15)), vec![import(3, 2)]),
            (card(4, "new", Some(30)), vec![import(4, 1), import(4, 2)]),
            (card(5, "rollout", Some(40)), vec![]),
            (card(6, "new", None), vec![import(6, 1)]),
        ];
        assert_eq!(undoable_ids(&cards), BTreeSet::from([4]));
        assert_eq!(undoable_ids(&cards[..3]), BTreeSet::from([2, 3]));
    }
```

- [ ] **Step 10: Run them to verify they fail**

Run: `cargo fleet-test -- store::changesets` and `cargo fleet-test -- service::catalog::changesets::tests`
Expected: FAIL to compile — `no method prune_withdrawn_changesets`, `cannot find function undoable_ids`.

- [ ] **Step 11: Implement R9**

`store/changesets.rs`, in `impl Store`:

```rust
    /// Assets M5 (R9): delete the cards the reconcile pass withdrew (R2:
    /// `dismissed` with `withdrawn: …`) that were created before
    /// `created_before` (Unix seconds) and of which no item ever left
    /// `pending` — no apply, no person's rejection, so nothing any rule,
    /// undo or SB6 reads. Items cascade. Answers how many went.
    pub fn prune_withdrawn_changesets(&self, created_before: i64) -> Result<usize> {
        self.conn.execute(
            "DELETE FROM changesets \
             WHERE state = 'dismissed' AND error LIKE 'withdrawn:%' AND created_at < ?1 \
               AND NOT EXISTS (SELECT 1 FROM changeset_items i \
                               WHERE i.changeset_id = changesets.id AND i.state <> 'pending')",
            [created_before],
        )
    }
```

`changesets/mod.rs`, next to `is_undoable`:

```rust
/// Assets M5 (R9): every card [`is_undoable`] answers true for, in one pass
/// over the cards: an applied catalog-changing card that is the latest —
/// by `(applied_at, id)` — in every catalog it touched. Equal to asking
/// [`later_card`] per card, without re-reading every card's items each time.
pub(crate) fn undoable_ids(cards: &[(ChangesetRow, Vec<ChangesetItemRow>)]) -> BTreeSet<i64> {
    let changing = || {
        cards
            .iter()
            .filter(|(c, items)| c.state == "applied" && changes_catalog(c, items))
    };
    let mut latest: BTreeMap<i64, (Option<i64>, i64)> = BTreeMap::new();
    for (card, items) in changing() {
        let at = (card.applied_at, card.id);
        for cid in applied_catalogs(items) {
            let l = latest.entry(cid).or_insert(at);
            if at > *l {
                *l = at;
            }
        }
    }
    changing()
        .filter(|(c, items)| {
            applied_catalogs(items)
                .iter()
                .all(|cid| latest.get(cid) == Some(&(c.applied_at, c.id)))
        })
        .map(|(c, _)| c.id)
        .collect()
}
```

and rewrite `list`:

```rust
/// Every open card and the [`RECENT_CLOSED`] most recent others, newest first.
pub fn list(store: &Mutex<Store>) -> Result<Vec<ChangesetSummary>, IpcError> {
    let s = lock(store)?;
    let mut cards = Vec::new();
    for card in s.list_changesets()? {
        let items = s.changeset_items(card.id)?;
        cards.push((card, items));
    }
    // R9: undoability once, linear, instead of `later_card` per card.
    let undoable = undoable_ids(&cards);
    let mut closed = 0;
    let mut out = Vec::new();
    for (card, items) in cards {
        if !is_open(&card.state) {
            if closed >= RECENT_CLOSED {
                continue;
            }
            closed += 1;
        }
        let mut groups: BTreeMap<String, usize> = BTreeMap::new();
        for i in &items {
            *groups.entry(i.grp.clone()).or_insert(0) += 1;
        }
        let pending = items.iter().filter(|i| i.state == "pending").count();
        out.push(ChangesetSummary {
            undoable: undoable.contains(&card.id),
            id: card.id,
            kind: card.kind,
            summary: card.summary,
            state: card.state,
            created_at: card.created_at,
            applied_at: card.applied_at,
            error: card.error,
            groups,
            pending,
        });
    }
    Ok(out)
}
```

`changesets/reconcile.rs`:

```rust
/// Assets M5 (R9): how long a card the pass withdrew, untouched, is kept.
pub const WITHDRAWN_RETENTION_SECS: i64 = 7 * 24 * 3600;

/// One pass. `auto` (`catalog.auto`): write `ignored` verdicts for
/// internals here instead of proposing `hide` items (R18). Then prune the
/// pass's own old, untouched withdrawn cards (Assets M5, R9).
pub fn reconcile(store: &Mutex<Store>, auto: bool) -> Result<ReconcileReport, IpcError> {
    let facts = gather(store)?;
    let (identities, proposed) = proposals(&facts, auto);
    let report = write(store, &facts, &identities, &proposed, auto)?;
    let pruned = lock(store)?.prune_withdrawn_changesets(now_unix() - WITHDRAWN_RETENTION_SECS)?;
    if pruned > 0 {
        tracing::debug!(pruned, "changesets: pruned withdrawn cards");
    }
    Ok(report)
}
```

In `gather`, replace `let snapshot = registry::snapshot()?;` and the two later uses of `&snapshot` (the `catalogs` map and the `layer_gaps` call) by one borrow, after `host_facts` is built:

```rust
    // Assets M5 (R9): borrow the loaded catalogs under the registry's read
    // lock instead of cloning every one (assets, bodies, resources) on every
    // pass. Registry → store is allowed (`layer_gaps` reads effective sets,
    // which take the store); nothing in here calls `registry::` again — a
    // second read while a writer waits would deadlock.
    let (catalogs, gaps) = registry::with_catalogs(|snapshot| {
        let catalogs: Vec<CatalogFacts> = configured
            .iter()
            .map(|row| catalog_facts(row, snapshot, &host_layers))
            .collect();
        let gaps = layer_gaps(store, snapshot, &configured, &host_layers, &rows, &rolled_out);
        Ok((catalogs, gaps))
    })?;
```

(`verdict_keys` stays where it is; the `PassFacts { … }` literal is unchanged.)

`changesets/apply.rs` `auto_additive`: replace `let snapshot = registry::snapshot()?;` and the host loop that fills `wants`/`catalogs` with the same loop inside a borrow:

```rust
    // Assets M5 (R9): borrow, do not clone, the registry; registry → store
    // is allowed, and nothing in here calls `registry::` again.
    let (wants, catalogs) = registry::with_catalogs(|snapshot| {
        let mut wants: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut catalogs: BTreeSet<String> = BTreeSet::new();
        for h in hosts.iter().filter(|h| {
            !h.hidden
                && (h.reachable || h.alias == "local")
                && !backoff.holds(&h.alias, scans.get(&h.alias).copied())
        }) {
            let Ok(eff) = effective::effective_for_host_in(store, &h.alias, snapshot) else {
                continue;
            };
            for (key, prov) in &eff.provenance {
                let Some(cid) = id_of.get(&prov.catalog) else {
                    continue;
                };
                if !rolled.contains(&(*cid, prov.introduced_by.clone()))
                    || rejected.contains(&(*cid, prov.introduced_by.clone(), h.alias.clone()))
                {
                    continue;
                }
                let Some((kind, name)) = key.split_once('/') else {
                    continue;
                };
                let due = rows
                    .iter()
                    .find(|r| {
                        r.host_alias == h.alias
                            && r.harness == "claude"
                            && r.kind == kind
                            && r.name == name
                    })
                    .is_some_and(sb6_due);
                if due {
                    wants.entry(h.alias.clone()).or_default().insert(key.clone());
                    catalogs.insert(prov.catalog.clone());
                }
            }
        }
        Ok((wants, catalogs))
    })?;
```

(`let id_of = super::catalog_ids_by_label(&configured);` moves above it; everything from `if wants.is_empty()` on is unchanged. The closure is synchronous; the `.await` on `sync_hosts` comes after the guard is gone.)

- [ ] **Step 12: Run the retention, undo and card tests**

Run: `cargo fleet-test -- store::changesets` and `cargo fleet-test -- service::catalog::changesets`
Expected: PASS — every existing reconcile, apply, undo and SB6 test included (the borrow changes no answer).

- [ ] **Step 13: Write the failing last-sync tests (R10)**

`store/catalog.rs` `mod tests`:

```rust
    /// Assets M5 (R10): the newest run a person made, past SB6's.
    #[test]
    fn last_person_sync_run_skips_runs_sb6_made() {
        let s = Store::open_in_memory().unwrap();
        assert!(s.last_person_sync_run().unwrap().is_none());
        s.record_sync_run(1, 2, r#"{"plan_id":"p1","started_at":1,"finished_at":2,"hosts":[]}"#)
            .unwrap();
        s.record_sync_run(3, 4, r#"{"plan_id":"p2","started_at":3,"finished_at":4,"hosts":[],"auto":true}"#)
            .unwrap();
        assert!(s.last_person_sync_run().unwrap().unwrap().summary_json.contains("p1"));
        assert!(s.last_sync_run().unwrap().unwrap().summary_json.contains("p2"));
    }
```

`sync/mod.rs` `mod tests`:

```rust
    /// Assets M5 (R10): the Assets footer shows a person's last sync; a fleet
    /// only SB6 has synced still shows SB6's, marked `auto`.
    #[test]
    fn last_sync_prefers_a_person_run_and_falls_back_to_sb6s() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let auto = r#"{"plan_id":"a","started_at":3,"finished_at":4,"hosts":[],"auto":true}"#;
        store.lock().unwrap().record_sync_run(3, 4, auto).unwrap();
        let only = last_sync(&store).unwrap().unwrap();
        assert!(only.auto);
        let person = r#"{"plan_id":"p","started_at":1,"finished_at":2,"hosts":[]}"#;
        store.lock().unwrap().record_sync_run(1, 2, person).unwrap();
        store.lock().unwrap().record_sync_run(5, 6, auto).unwrap();
        let got = last_sync(&store).unwrap().unwrap();
        assert_eq!((got.plan_id.as_str(), got.auto), ("p", false));
    }
```

- [ ] **Step 14: Run them to verify they fail**

Run: `cargo fleet-test -- store::catalog` and `cargo fleet-test -- service::catalog::sync::tests::last_sync`
Expected: FAIL to compile — `no method last_person_sync_run`.

- [ ] **Step 15: Implement R10**

`store/catalog.rs`, next to `last_person_sync_run_id`:

```rust
    /// The newest sync run a person made — one whose summary is not marked
    /// `"auto": true` (Assets M5, R10). A row whose JSON does not parse
    /// counts as a person's.
    pub fn last_person_sync_run(&self) -> Result<Option<SyncRunRow>, rusqlite::Error> {
        self.conn
            .prepare_cached(
                "SELECT id, started_at, finished_at, summary_json FROM sync_runs \
                 WHERE NOT (json_valid(summary_json) \
                            AND json_extract(summary_json, '$.auto') IS 1) \
                 ORDER BY id DESC LIMIT 1",
            )?
            .query_row([], |row| {
                Ok(SyncRunRow {
                    id: row.get(0)?,
                    started_at: row.get(1)?,
                    finished_at: row.get(2)?,
                    summary_json: row.get(3)?,
                })
            })
            .optional()
    }
```

`sync/mod.rs` `last_sync`:

```rust
/// The run the Assets footer shows (Assets M5, R10): the newest a person
/// made, else the newest SB6 made (marked `auto`), else none.
pub fn last_sync(store: &Mutex<Store>) -> Result<Option<SyncRunSummary>, IpcError> {
    let row = {
        let s = lock(store)?;
        match s.last_person_sync_run()? {
            Some(row) => Some(row),
            None => s.last_sync_run()?,
        }
    };
    match row {
        Some(row) => Ok(Some(serde_json::from_str(&row.summary_json).map_err(
            |e| IpcError::new(codes::E_PARSE, format!("stored sync summary: {e}")),
        )?)),
        None => Ok(None),
    }
}
```

- [ ] **Step 16: Run them, then checkpoint and commit**

Run: `cargo fleet-test -- store::catalog`, `cargo fleet-test -- service::catalog::sync`, `cargo fleet-check`, `cargo fmt --all --check`, `cargo fleet-lint`.
Expected: PASS.

```bash
git add crates/fleet-core/migrations/097_changeset_item_decided_at.sql crates/fleet-core/src/store \
  crates/fleet-core/src/service/catalog/changesets crates/fleet-core/src/service/catalog/sync/mod.rs
git commit -m "fix(changesets): slug collisions need a look; decisions order by time; the pass is cheaper

A Bootstrap card no longer fails because two names import as one slug —
both need a look. Rollout rejections are ordered by when they were decided
(migration 097: changeset_items.decided_at), not by card id. The reconcile
pass and SB6 borrow the registry instead of cloning every catalog, list
computes undoability in one pass, and untouched withdrawn cards older than a
week are pruned. The last sync shown is the newest a person made."
```

---

### Task 4: The read surface — every catalog in the listing, host states per catalog, asset history, and four read-only commands (carry 3; R11–R13, R28)

**Files:**
- Modify: `crates/fleet-core/src/service/catalog/mod.rs` (`AssetSummary` `:365`, `host_states` `:407`, `list_assets` `:427`, `get_asset_in` `:570`, tests)
- Modify: `crates/fleet-core/src/service/catalog/repo.rs` (new `CommitEntry`, `asset_log`, a test)
- Modify: `crates/fleet-core/src/service/catalog/author.rs` (new `asset_history_in`, `HISTORY_LIMIT`)
- Modify: `crates/fleet-core/src/service/catalog/admin.rs` (`admin_calls!` `:142`, `is_read` `:188`, `is_per_catalog` `:237`, `run` `:367`, `every_call` test)
- Modify: `crates/fleet-core/src/mcp/tools/assets.rs` (`list_assets` `:14`, new `listing_scope`)
- Modify: `crates/fleet-core/src/mcp/tools/params.rs` (`CatalogAdminParams::action` doc `:1053`)
- Modify: `crates/fleet-core/src/mcp/tools/tests_catalog_admin.rs` (a test), `crates/fleet-core/src/mcp/tools/tests.rs` (`BUDGET_BYTES` `:3453`)
- Modify: `src-tauri/src/commands/assets.rs` (four commands + `routed::` bodies + `in_catalog`), `src-tauri/src/lib.rs` (`:574`), `src-tauri/src/backend/verdicts.rs` (catalog section), `src-tauri/src/backend/tests_routing.rs` (`catalog_admin_cases` `:4463`)
- Regenerate: `src/lib/hub_verdicts.generated.json`, `docs/control-api-reference.md`

**Interfaces:**
- Consumes: `registry::{with_catalogs, in_order}`, `effective::label_of`, `catalogs::{list_catalogs, catalog_named, PERSONAL, CatalogStatus}`, `changesets::{list, ChangesetSummary}`, `author::{repo_status, repo_status_in}`, `CatalogTarget`.
- Produces (Rust):
  - `AssetSummary.catalog: String` (`#[serde(default = "personal_label")]`).
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum ListingScope { Every, Personal }`; `pub fn list_assets_in(store, scope) -> Result<AssetListing, IpcError>`; `list_assets(store)` = `Every`.
  - `#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)] pub struct CommitEntry { pub sha: String, pub at: i64, pub author: String, pub subject: String }` (`repo`); `pub fn asset_log(root, kind, name, limit) -> Result<Vec<CommitEntry>, IpcError>`.
  - `pub const HISTORY_LIMIT: usize = 50`; `pub fn asset_history_in(target, args: AssetRef, store) -> Result<Vec<repo::CommitEntry>, IpcError>` (`author`).
  - `AdminCall::AssetHistory(AssetRef)`, wire `"asset_history"`, a per-catalog read.
  - `pub(crate) fn listing_scope(caller: &Caller) -> ListingScope` (`mcp::tools::assets`).
- Produces (Tauri commands, the frontend's contract for Task 6):
  - `catalog_list_catalogs()` → `Vec<CatalogStatus>`
  - `catalog_list_changesets()` → `Vec<ChangesetSummary>`
  - `catalog_repo_status_in({ args: { name } })` → `RepoStatus`
  - `catalog_asset_history({ args: { kind, name, catalog } })` → `Vec<CommitEntry>` (`catalog: null` = personal)
  - `pub struct AssetHistoryArgs { pub kind: Kind, pub name: String, #[serde(default)] pub catalog: Option<String> }` in `commands::assets`.

- [ ] **Step 1: Write the failing listing test (R11, R12)**

In `service/catalog/mod.rs` `mod tests`:

```rust
    /// Assets M5 (R11, R12): the listing spans every loaded catalog, each
    /// asset naming its catalog and showing only its own catalog's host
    /// states; the personal-only listing is as before; `get_asset` reads
    /// its own catalog's hosts.
    #[test]
    fn list_assets_spans_catalogs_and_keeps_each_catalogs_host_states() {
        let _g = lock_registry_for_test();
        let root = repo_with_one_skill("m5-list");
        let store = Mutex::new(Store::open_in_memory().unwrap());
        configure(
            ConfigureArgs {
                repo_path: root.to_string_lossy().into(),
                remote_url: None,
            },
            &store,
        )
        .unwrap();
        load(false, &store).unwrap();
        let org = store.lock().unwrap().add_org("acme", None, false).unwrap();
        let acme = store
            .lock()
            .unwrap()
            .upsert_catalog(
                "acme",
                &repo_with_one_skill("m5-list-acme").to_string_lossy(),
                None,
                Some(org.id),
            )
            .unwrap();
        load_catalog(acme.id, false, &store).unwrap();
        let personal = store.lock().unwrap().personal_catalog().unwrap().unwrap().id;
        let row = |host: &str, state: &str, catalog_id: Option<i64>| crate::store::AssetInventoryRow {
            host_alias: host.into(),
            harness: "claude".into(),
            kind: "skill".into(),
            name: "s".into(),
            state: state.into(),
            scanned_at: 1,
            managed: true,
            catalog_id,
            drift_side: (state == "drifted").then(|| "host".to_string()),
            ..Default::default()
        };
        {
            let s = store.lock().unwrap();
            for h in ["local", "mefistos", "oci"] {
                s.upsert_host(h).unwrap();
            }
            s.replace_host_inventory("local", "claude", &[row("local", "in_sync", Some(acme.id))])
                .unwrap();
            s.replace_host_inventory("mefistos", "claude", &[row("mefistos", "drifted", Some(personal))])
                .unwrap();
            s.replace_host_inventory("oci", "claude", &[row("oci", "missing", None)])
                .unwrap();
        }

        let every = list_assets(&store).unwrap();
        let hosts_of = |catalog: &str| -> Vec<(String, String, Option<String>)> {
            every
                .assets
                .iter()
                .find(|a| a.catalog == catalog)
                .unwrap_or_else(|| panic!("no asset in {catalog}"))
                .hosts
                .iter()
                .map(|h| (h.host_alias.clone(), h.state.clone(), h.drift_side.clone()))
                .collect()
        };
        assert_eq!(every.assets.len(), 2, "one `s` per catalog");
        assert_eq!(
            hosts_of("personal"),
            [
                ("mefistos".to_string(), "drifted".to_string(), Some("host".to_string())),
                ("oci".to_string(), "missing".to_string(), None),
            ],
            "an unstamped row (scanned before M2) is personal's"
        );
        assert_eq!(
            hosts_of("acme"),
            [("local".to_string(), "in_sync".to_string(), None)]
        );

        let personal_only = list_assets_in(&store, ListingScope::Personal).unwrap();
        assert_eq!(
            personal_only.assets.iter().map(|a| a.catalog.as_str()).collect::<Vec<_>>(),
            ["personal"]
        );
        let detail = get_asset(Kind::Skill, "s", &store).unwrap();
        assert_eq!(
            detail.hosts.iter().map(|h| h.host_alias.as_str()).collect::<Vec<_>>(),
            ["mefistos", "oci"]
        );
        // An older hub's summary has no `catalog`: it reads as personal.
        let old: AssetSummary = serde_json::from_str(
            r#"{"kind":"skill","name":"s","version":"1","description":"d","tags":[],"hosts":[]}"#,
        )
        .unwrap();
        assert_eq!(old.catalog, "personal");
        registry::clear().unwrap();
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo fleet-test -- service::catalog::tests::list_assets_spans`
Expected: FAIL to compile — `no field catalog on AssetSummary`, `cannot find function list_assets_in`.

- [ ] **Step 3: Implement R11 and R12 in `service/catalog/mod.rs`**

Add to `AssetSummary` (after `scope`):

```rust
    /// Assets M5 (R11): the catalog this asset is in — `personal`, or an org
    /// catalog's name. A hub before M5 listed the personal catalog only, so
    /// an absent key reads as `personal`.
    #[serde(default = "personal_label")]
    pub catalog: String,
```

and below the struct:

```rust
fn personal_label() -> String {
    catalogs::PERSONAL.to_string()
}

/// Assets M5 (R11): which catalogs a listing spans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingScope {
    /// Every loaded catalog: the desktop, the master, an unbound full
    /// person device (the audience of `list_catalogs`, M3 PF15).
    Every,
    /// The personal catalog only, as before M5: per-host tokens, org-bound
    /// and readonly clients.
    Personal,
}
```

Replace `host_states` with:

```rust
/// Which hosts hold `cat`'s asset `kind/name`, and in what state. `unmanaged`
/// and `orphan` rows are excluded by name (they are what
/// `AssetListing::unmanaged` lists). Carry 3 (Assets M5, R12): a row is
/// `cat`'s when the scan stamped `cat`'s id on it, or — a managed row
/// scanned before Assets M2 stamped any — when it has none and `cat` is the
/// personal catalog. So `personal/x` and `acme/x` never show each other's
/// hosts.
fn host_states(
    rows: &[AssetInventoryRow],
    cat: &repo::Catalog,
    kind: Kind,
    name: &str,
) -> Vec<HostState> {
    rows.iter()
        .filter(|r| {
            r.kind == kind.as_str()
                && r.name == name
                && r.state != "unmanaged"
                && r.state != "orphan"
        })
        .filter(|r| match r.catalog_id {
            Some(id) => id == cat.id,
            None => cat.org_id.is_none(),
        })
        .map(|r| HostState {
            host_alias: r.host_alias.clone(),
            harness: r.harness.clone(),
            state: r.state.clone(),
            drift_side: r.drift_side.clone(),
        })
        .collect()
}
```

Replace `list_assets` with:

```rust
/// Every loaded catalog's assets (Assets M5, R11) with their per-host state
/// from the last scan, plus the unmanaged and orphan rows and the personal
/// catalog's problems.
pub fn list_assets(store: &Mutex<Store>) -> Result<AssetListing, IpcError> {
    list_assets_in(store, ListingScope::Every)
}

/// [`list_assets`] over `scope`'s catalogs. Store first (the inventory),
/// then the registry, never the other way round. `head`, `loaded_at` and
/// `problems` are the personal catalog's; an org catalog that failed to
/// load (a problem entry) lists nothing.
pub fn list_assets_in(store: &Mutex<Store>, scope: ListingScope) -> Result<AssetListing, IpcError> {
    require_config(store)?;
    let rows = inventory(store)?;
    let identities = identity::group_identities(&rows);
    registry::with_catalogs(|m| {
        let personal = m.values().find(|c| c.org_id.is_none()).ok_or_else(|| {
            IpcError::new(E_CATALOG_NOT_CONFIGURED, "catalog not loaded; call catalog_load")
        })?;
        let mut assets = Vec::new();
        for cat in registry::in_order(m) {
            if cat.load_error.is_some()
                || (scope == ListingScope::Personal && cat.org_id.is_some())
            {
                continue;
            }
            let label = effective::label_of(cat.org_id, &cat.name);
            assets.extend(cat.assets.iter().map(|a| AssetSummary {
                kind: a.kind().as_str().to_string(),
                name: a.header.name.clone(),
                version: a.header.version.clone(),
                description: a.header.description.clone(),
                tags: a.header.tags.clone(),
                hosts: host_states(&rows, cat, a.kind(), &a.header.name),
                install_as: a.header.install_as.clone(),
                scope: a.header.scope,
                catalog: label.clone(),
            }));
        }
        Ok(AssetListing {
            head: personal.head.clone(),
            loaded_at: personal.loaded_at,
            assets,
            // `unmanaged` is the wire name for "installed on a host but not
            // a catalog asset"; an `orphan` belongs in the same list (see
            // `list_assets_lists_orphan_rows_as_unmanaged`).
            unmanaged: rows
                .iter()
                .filter(|r| r.state == "unmanaged" || r.state == "orphan")
                .cloned()
                .collect(),
            problems: personal.problems.clone(),
            identities: Some(identities),
        })
    })
}
```

In `get_asset_in`, change `hosts: host_states(&rows, kind, name),` to `hosts: host_states(&rows, cat, kind, name),` (`cat` is the closure's catalog). Add `catalog: …` to every other `AssetSummary { … }` literal `cargo fleet-check` reports (`catalog: "personal".into()`). If `effective` is not yet imported in `mod.rs`, use `effective::label_of` via `super::effective` as the file already does elsewhere, or add `use effective;` — `cargo fleet-fast-check` says which.

- [ ] **Step 4: Run the catalog tests**

Run: `cargo fleet-test -- service::catalog::tests`
Expected: PASS (`list_assets_carries_scope_per_asset` and `list_assets_lists_orphan_rows_as_unmanaged` unchanged).

- [ ] **Step 5: Write the failing history test**

In `repo.rs` `mod tests`:

```rust
    /// Assets M5 (R19): the commits that touched one asset, newest first;
    /// a repo with no commit has none; `limit` caps them.
    #[test]
    fn asset_log_lists_the_commits_that_touched_one_asset_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]).unwrap();
        assert!(asset_log(root, Kind::Skill, "w", 10).unwrap().is_empty());
        std::fs::create_dir_all(root.join("skills/w")).unwrap();
        std::fs::write(root.join("skills/w/asset.yaml"), "kind: skill\nname: w\n").unwrap();
        stage_paths(root, &[]).unwrap();
        let first = commit(root, "add w").unwrap();
        std::fs::write(root.join("other.txt"), "x\n").unwrap();
        stage_paths(root, &[]).unwrap();
        commit(root, "unrelated").unwrap();
        std::fs::write(
            root.join("skills/w/asset.yaml"),
            "kind: skill\nname: w\ndescription: d\n",
        )
        .unwrap();
        stage_paths(root, &[]).unwrap();
        let second = commit(root, "edit w").unwrap();

        let log = asset_log(root, Kind::Skill, "w", 10).unwrap();
        assert_eq!(
            log.iter()
                .map(|c| (c.sha.as_str(), c.subject.as_str()))
                .collect::<Vec<_>>(),
            [(second.as_str(), "edit w"), (first.as_str(), "add w")]
        );
        assert!(log.iter().all(|c| c.at > 0 && !c.author.is_empty()));
        assert_eq!(asset_log(root, Kind::Skill, "w", 1).unwrap().len(), 1);
    }
```

- [ ] **Step 6: Run it to verify it fails**

Run: `cargo fleet-test -- service::catalog::repo::tests::asset_log`
Expected: FAIL to compile — `cannot find function asset_log`.

- [ ] **Step 7: Implement the history read**

`repo.rs`, next to `asset_rel_path`:

```rust
/// One commit that touched an asset (Assets M5, the Inspector's History).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitEntry {
    pub sha: String,
    /// Committer time, Unix seconds.
    pub at: i64,
    pub author: String,
    pub subject: String,
}

/// The newest `limit` commits that touched `kind/name` — its folder or its
/// file — newest first. A checkout with no commit yet has none.
pub fn asset_log(
    root: &Path,
    kind: Kind,
    name: &str,
    limit: usize,
) -> Result<Vec<CommitEntry>, IpcError> {
    if git(root, &["rev-parse", "--verify", "-q", "HEAD"]).is_err() {
        return Ok(Vec::new());
    }
    let rel = asset_rel_path(kind, name);
    let n = format!("-n{}", limit.max(1));
    let out = git(
        root,
        &["log", &n, "--format=%H%x1f%ct%x1f%an%x1f%s", "--", &rel],
    )?;
    Ok(out
        .lines()
        .filter_map(|line| {
            let mut p = line.splitn(4, '\u{1f}');
            Some(CommitEntry {
                sha: p.next()?.to_string(),
                at: p.next()?.parse().ok()?,
                author: p.next()?.to_string(),
                subject: p.next().unwrap_or("").to_string(),
            })
        })
        .collect())
}
```

`author.rs`, next to `repo_status_in`:

```rust
/// How many commits the History tab lists (Assets M5).
pub const HISTORY_LIMIT: usize = 50;

/// The Inspector's History (Assets M5): the newest [`HISTORY_LIMIT`]
/// commits that touched an asset in `target`'s checkout. Read only.
pub fn asset_history_in(
    target: CatalogTarget<'_>,
    args: AssetRef,
    store: &Mutex<Store>,
) -> Result<Vec<repo::CommitEntry>, IpcError> {
    check_name(&args.name)?;
    let root = target.root(store)?;
    repo::asset_log(&root, args.kind, &args.name, HISTORY_LIMIT)
}
```

`admin.rs`: append to `admin_calls!` after `"unadmit_catalog" => UnadmitCatalog(AdmitArgs),`:

```rust
    /// Assets M5: the commits that touched one asset (the Inspector's
    /// History), in the catalog the tool's `catalog` parameter names.
    "asset_history" => AssetHistory(AssetRef),
```

add `| AdminCall::AssetHistory(_)` to both `is_read` and `is_per_catalog`, add to `run`'s match (next to `AdminCall::RepoStatus`):

```rust
        AdminCall::AssetHistory(a) => json(author::asset_history_in(target, a, store)?),
```

and append `AdminCall::AssetHistory(skill("s")),` at the end of `every_call()` in `admin.rs`'s tests (declaration order).

`mcp/tools/params.rs`: the last line of `CatalogAdminParams::action`'s doc becomes `/// remove_catalog|admit_catalog|unadmit_catalog|asset_history`.

- [ ] **Step 8: Run the history and admin tests**

Run: `cargo fleet-test -- service::catalog::repo`, `cargo fleet-test -- service::catalog::admin`, `cargo fleet-test -- mcp::tools::tests_catalog_admin`
Expected: PASS (`the_action_param_names_every_admin_call` sees the new word).

- [ ] **Step 9: Write the failing caller-scope test**

In `mcp/tools/tests_catalog_admin.rs`:

```rust
/// Assets M5 (R11): `list_assets` spans every catalog only for the callers
/// that may list every catalog — the master and an unbound full device.
#[test]
fn list_assets_spans_every_catalog_only_for_the_master_and_unbound_full_devices() {
    use super::assets::listing_scope;
    use crate::service::catalog::ListingScope;
    assert_eq!(listing_scope(&Caller::master()), ListingScope::Every);
    assert_eq!(listing_scope(&client(1, TokenMode::Full, None)), ListingScope::Every);
    assert_eq!(listing_scope(&client(1, TokenMode::Readonly, None)), ListingScope::Personal);
    assert_eq!(listing_scope(&client(1, TokenMode::Full, Some(7))), ListingScope::Personal);
    assert_eq!(listing_scope(&host("oci")), ListingScope::Personal);
}
```

- [ ] **Step 10: Run it to verify it fails**

Run: `cargo fleet-test -- mcp::tools::tests_catalog_admin::list_assets_spans`
Expected: FAIL to compile — `cannot find function listing_scope`.

- [ ] **Step 11: Scope the MCP tool by caller**

In `mcp/tools/assets.rs`, the tool keeps its description and gains the caller:

```rust
    pub(super) async fn list_assets(
        &self,
        Extension(caller): Extension<Caller>,
    ) -> Result<CallToolResult, McpError> {
        audit("list_assets", &format!("caller={}", caller.label()));
        catalog::ensure_fresh(&self.store).map_err(to_mcp_err)?;
        ok_json_compact(
            &catalog::list_assets_in(&self.store, listing_scope(&caller)).map_err(to_mcp_err)?,
        )
    }
```

and, at module level (outside the `impl`):

```rust
/// Assets M5 (R11): every catalog's assets for the master and a person's own
/// unbound full device — the `list_catalogs` audience; the personal catalog
/// only, as before, for a per-host token and an org-bound or readonly client.
pub(crate) fn listing_scope(caller: &Caller) -> catalog::ListingScope {
    if caller.is_master() || (caller.is_person_device() && caller.mode == TokenMode::Full) {
        catalog::ListingScope::Every
    } else {
        catalog::ListingScope::Personal
    }
}
```

Every existing call site of the tool function in tests (`t.list_assets()`) gains `Extension(Caller::master())`.

- [ ] **Step 12: Run the MCP tests and re-measure the budget**

Run: `cargo fleet-test -- mcp::tools::tests_catalog_admin` then `cargo fleet-test -- mcp::tools::tests::the_served_definition_budget_stays_bounded`
Expected: the first PASS; the budget test prints `master surface measured at N bytes …`. If it fails, set `BUDGET_BYTES` in `mcp/tools/tests.rs` to N + 100 and re-run (PASS). Then `REGEN_DOCS=1 cargo fleet-test -- reference_is_current` and confirm `git diff docs/control-api-reference.md` adds only `asset_history`.

- [ ] **Step 13: Write the four Tauri commands' failing routing cases**

In `src-tauri/src/backend/tests_routing.rs` `catalog_admin_cases()` (add `use fleet_core::service::catalog::admin::CatalogNameArgs;` to its `use` block), append to the `vec![ … ]`:

```rust
        // Assets M5 (R13): the workspace's reads.
        (
            "catalog_list_catalogs",
            "catalog_admin",
            json!({ "action": "list_catalogs" }),
            "[]",
            Box::new(|b, s, _| block_on(r::catalog_list_catalogs(b, s)).map(|_| ())),
        ),
        (
            "catalog_list_changesets",
            "changesets",
            json!({ "action": "list" }),
            r#"[{"id":3,"kind":"new","summary":"New on oci: skill/w → core","state":"proposed","created_at":1,"groups":{"core":1},"pending":1,"undoable":false}]"#,
            Box::new(|b, s, _| block_on(r::catalog_list_changesets(b, s)).map(|_| ())),
        ),
        (
            "catalog_repo_status_in",
            "catalog_admin",
            json!({ "action": "repo_status", "catalog": "acme" }),
            STATUS,
            Box::new(|b, s, _| {
                block_on(r::catalog_repo_status_in(
                    b,
                    CatalogNameArgs { name: "acme".into() },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "catalog_asset_history",
            "catalog_admin",
            json!({ "action": "asset_history",
                    "args": { "kind": "skill", "name": "s" },
                    "catalog": "acme" }),
            r#"[{"sha":"abc","at":1,"author":"a","subject":"edit s"}]"#,
            Box::new(|b, s, _| {
                block_on(r::catalog_asset_history(
                    b,
                    commands::assets::AssetHistoryArgs {
                        kind: Kind::Skill,
                        name: "s".into(),
                        catalog: Some("acme".into()),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
```

- [ ] **Step 14: Run them to verify they fail**

Run: `cargo fleet-test -- backend::tests_routing`
Expected: FAIL to compile — `no function catalog_list_catalogs in routed`.

- [ ] **Step 15: Add the commands, their bodies, rows and registration**

`src-tauri/src/commands/assets.rs` — imports: add `admin::CatalogNameArgs` to the `admin::{…}` list, `catalogs::CatalogStatus`, `changesets::ChangesetSummary`, `repo::CommitEntry` and `model::Kind` to the `fleet_core::service::catalog::{…}` list. Commands (next to `catalog_repo_status`):

```rust
/// Assets M5 (R13): every catalog and its load state — the footer's chips.
#[tauri::command]
pub async fn catalog_list_catalogs(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<CatalogStatus>, IpcError> {
    routed::catalog_list_catalogs(&backend, &store).await
}

/// Assets M5 (R13, R14): the changeset cards, read only — the Inbox's
/// proposed cards. Apply / undo / dismiss are M6.
#[tauri::command]
pub async fn catalog_list_changesets(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<ChangesetSummary>, IpcError> {
    routed::catalog_list_changesets(&backend, &store).await
}

/// Assets M5 (R13, R24): one catalog's dirty / ahead / behind, by name.
#[tauri::command]
pub async fn catalog_repo_status_in(
    backend: State<'_, Arc<FleetBackend>>,
    args: CatalogNameArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<RepoStatus, IpcError> {
    routed::catalog_repo_status_in(&backend, args, &store).await
}

/// `catalog_asset_history`'s arguments: an asset, and its catalog (`None`
/// = personal).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AssetHistoryArgs {
    pub kind: Kind,
    pub name: String,
    #[serde(default)]
    pub catalog: Option<String>,
}

/// Assets M5 (R13, R19): the commits that touched one asset.
#[tauri::command]
pub async fn catalog_asset_history(
    backend: State<'_, Arc<FleetBackend>>,
    args: AssetHistoryArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<CommitEntry>, IpcError> {
    routed::catalog_asset_history(&backend, args, &store).await
}
```

In `mod routed`:

```rust
    /// `call` addressed to `catalog` through the tool's own top-level
    /// `catalog` parameter (Assets M5, R13) — never inside `args`, where a
    /// hub would ignore it and answer for personal.
    fn in_catalog(call: &AdminCall, catalog: &str) -> Result<serde_json::Value, IpcError> {
        let mut v = serde_json::to_value(call).map_err(|e| {
            IpcError::new(
                fleet_core::ipc_error::codes::E_INTERNAL,
                format!("{} could not be encoded for the hub: {e}", call.action()),
            )
        })?;
        v["catalog"] = serde_json::Value::String(catalog.to_string());
        Ok(v)
    }

    pub async fn catalog_list_catalogs(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<Vec<CatalogStatus>, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route("catalog_list_catalogs", &AdminCall::ListCatalogs)
                    .await
            }
            None => catalog::catalogs::list_catalogs(store),
        }
    }

    pub async fn catalog_list_changesets(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<Vec<ChangesetSummary>, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route(
                    "catalog_list_changesets",
                    &serde_json::json!({ "action": "list" }),
                )
                .await
            }
            None => catalog::changesets::list(store),
        }
    }

    pub async fn catalog_repo_status_in(
        backend: &FleetBackend,
        args: CatalogNameArgs,
        store: &Mutex<Store>,
    ) -> Result<RepoStatus, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route(
                    "catalog_repo_status_in",
                    &in_catalog(&AdminCall::RepoStatus, &args.name)?,
                )
                .await
            }
            None if args.name == catalog::catalogs::PERSONAL => author::repo_status(store),
            None => {
                let row = catalog::catalogs::catalog_named(&args.name, store)?;
                author::repo_status_in(catalog::CatalogTarget::Row(&row), store)
            }
        }
    }

    pub async fn catalog_asset_history(
        backend: &FleetBackend,
        args: AssetHistoryArgs,
        store: &Mutex<Store>,
    ) -> Result<Vec<CommitEntry>, IpcError> {
        check_name(&args.name)?;
        let asset = AssetRef {
            kind: args.kind,
            name: args.name,
        };
        let named = args
            .catalog
            .filter(|c| c.as_str() != catalog::catalogs::PERSONAL);
        match (backend.hub(), named) {
            (Some(hub), Some(c)) => {
                hub.route(
                    "catalog_asset_history",
                    &in_catalog(&AdminCall::AssetHistory(asset), &c)?,
                )
                .await
            }
            (Some(hub), None) => {
                hub.route("catalog_asset_history", &AdminCall::AssetHistory(asset))
                    .await
            }
            (None, Some(c)) => {
                let row = catalog::catalogs::catalog_named(&c, store)?;
                author::asset_history_in(catalog::CatalogTarget::Row(&row), asset, store)
            }
            (None, None) => {
                author::asset_history_in(catalog::CatalogTarget::Personal, asset, store)
            }
        }
    }
```

`src-tauri/src/backend/verdicts.rs`, at the end of the asset-catalog section (after `catalog_template`'s row):

```rust
    // Assets M5 (R13): the workspace's reads. The footer's catalog chips:
    // `catalog_admin { list_catalogs }`, the master's or an unbound full
    // client's — no grant (M3 PF15).
    (
        "catalog_list_catalogs",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    // The Inbox's cards, read only until M6: `changesets { list }`, the
    // master's or an unbound full client's.
    (
        "catalog_list_changesets",
        Verdict::Routed { tool: "changesets" },
    ),
    // One catalog's repo status by name: `catalog_admin { repo_status }`
    // with the tool's `catalog` parameter; needs a grant on that catalog.
    (
        "catalog_repo_status_in",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    // The Inspector's History: `catalog_admin { asset_history }`, per
    // catalog. A hub before M5 refuses the action with E_INVALID.
    (
        "catalog_asset_history",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
```

`src-tauri/src/lib.rs`, after `commands::assets::catalog_repo_status,`:

```rust
            commands::assets::catalog_repo_status_in,
            commands::assets::catalog_list_catalogs,
            commands::assets::catalog_list_changesets,
            commands::assets::catalog_asset_history,
```

- [ ] **Step 16: Run the routing tests and regenerate the verdicts**

Run: `cargo fleet-test -- backend::tests_routing` — Expected: PASS (including `every_command_has_a_verdict`, `every_commands_body_does_what_its_row_says`, `every_routed_row_is_driven_by_a_case`, `every_routed_tool_is_a_tool_the_hub_serves`, the unavailable-hub and skewed-contract sweeps).
Run: `REGEN_HUB_VERDICTS=1 cargo fleet-test -- verdict_gen`, then `cargo fleet-test -- verdict_gen` — Expected: PASS; `git diff src/lib/hub_verdicts.generated.json` shows the four `routed` rows; `docs/hub.md` unchanged (no `LocalOnly` row).
Run: `cargo fleet-test -- contract` — Expected: PASS with no golden change (no wire type in the golden changed; R13: no revision bump).

- [ ] **Step 17: Checkpoint and commit**

Run: `cargo fleet-check`, `cargo fmt --all --check`, `cargo fleet-lint`.

```bash
git add crates/fleet-core/src/service/catalog crates/fleet-core/src/mcp/tools \
  src-tauri/src/commands/assets.rs src-tauri/src/lib.rs src-tauri/src/backend/verdicts.rs \
  src-tauri/src/backend/tests_routing.rs src/lib/hub_verdicts.generated.json docs/control-api-reference.md
git commit -m "feat(assets): list every catalog's assets, per-catalog host states, asset history, workspace reads

list_assets spans every loaded catalog for the master and unbound full
devices (each asset names its catalog) and keeps personal-only for host
tokens and org-bound or readonly clients; host states are per catalog, so
personal/x and acme/x never show each other's hosts. catalog_admin gains a
read-only asset_history action. Four read-only desktop commands route to the
hub: list catalogs, list changesets, a catalog's repo status, asset history.

Budget: re-measured the master surface after adding asset_history to
CatalogAdminParams::action."
```

---

### Task 5: `Badge`, `HostStrip` states, and tokens instead of literal colours (R26, R27)

**Files:**
- Create: `src/lib/assets_visual.ts`, `src/lib/Badge.svelte`, `src/lib/Badge.test.ts`, `src/lib/assets_tokens.test.ts`
- Modify: `src/lib/HostStrip.svelte`, `src/lib/HostStrip.test.ts`
- Modify: `src/lib/AssetList.svelte` (state chips, needs-person and orphan badges → `Badge`; their CSS goes)
- Modify: `src/lib/SyncPlanDialog.svelte` (`.count-chip`, `.op-badge`, `.outcome` → `Badge`; hex → tokens)
- Modify: `src/lib/AssetDetail.svelte` (`<style>` `:270-287`), `src/lib/AssetEditor.svelte` (`:524-526`), `src/lib/AssetsPanel.svelte` (`:517`, `:528`, `.primary`)

**Interfaces:**
- Produces:
  - `export type BadgeTone = 'neutral' | 'ok' | 'warn' | 'crit' | 'accent' | 'muted'` (`assets_visual.ts`).
  - `export type DotState = 'present' | 'in_sync' | 'differs' | 'missing' | 'absent' | 'na' | 'stale' | 'blocked'`; `export const DOT_LABEL: Record<DotState, string>`; `export function opTone(op: string): BadgeTone`; `export function outcomeTone(outcome: string): BadgeTone`.
  - `Badge.svelte` props `{ label: string; tone?: BadgeTone; glyph?: string; dashed?: boolean; mono?: boolean; title?: string; testid?: string }`; renders `<span class="badge <tone>">`.
  - `HostStrip.svelte` props `{ order: string[]; present?: string[]; odd?: string[]; states?: Record<string, DotState> }` — `states` wins when given; a host missing from it is `na`.

- [ ] **Step 1: Write the failing component tests**

`src/lib/Badge.test.ts`:

```ts
import { render, screen } from '@testing-library/svelte';
import { describe, it, expect } from 'vitest';
import Badge from './Badge.svelte';

describe('Badge', () => {
  it('says its state in words, the glyph only reinforcing it', () => {
    render(Badge, { label: '2 drifted', tone: 'warn', glyph: '◐', testid: 'b' });
    const b = screen.getByTestId('b');
    expect(b.textContent).toBe('◐2 drifted');
    expect(b.className).toContain('badge');
    expect(b.className).toContain('warn');
    expect(b.querySelector('.glyph')?.getAttribute('aria-hidden')).toBe('true');
  });

  it('marks a private scope by its border shape, not by colour alone', () => {
    render(Badge, { label: 'private', dashed: true, title: 'Personal catalog, private', testid: 'b' });
    const b = screen.getByTestId('b');
    expect(b.className).toContain('dashed');
    expect(b.className).toContain('neutral');
    expect(b.getAttribute('title')).toBe('Personal catalog, private');
  });

  it('is information, never a control', () => {
    render(Badge, { label: 'orphan', tone: 'warn', testid: 'b' });
    const b = screen.getByTestId('b');
    expect(b.tagName).toBe('SPAN');
    expect(b.getAttribute('role')).toBeNull();
    expect(b.getAttribute('tabindex')).toBeNull();
  });
});
```

Append to `src/lib/HostStrip.test.ts` inside its `describe`:

```ts
  it('renders explicit per-host states with words for each', () => {
    const { container } = render(HostStrip, {
      order: ['local', 'mefistos', 'oci', 'trn', 'htz'],
      states: { local: 'in_sync', mefistos: 'differs', oci: 'missing', trn: 'stale' },
    });
    const dots = [...container.querySelectorAll('.dot')];
    expect(dots.map((d) => [...d.classList].find((c) => c !== 'dot' && !c.startsWith('svelte-')))).toEqual([
      'in_sync', 'differs', 'missing', 'stale', 'na',
    ]);
    expect(container.querySelector('.strip')?.getAttribute('aria-label')).toBe(
      'local: in sync, mefistos: differs, oci: missing, trn: stale scan, htz: not here',
    );
  });
```

`src/lib/assets_tokens.test.ts`:

```ts
import { readFileSync } from 'node:fs';
import { describe, it, expect } from 'vitest';

// Spec, Visuals: "the existing tokens and controls.css; the hard-coded hex
// colours in AssetDetail and SyncPlanDialog move to tokens". Held for every
// component of the Assets workspace, so a new one cannot bring one back.
// Each task that adds an Assets component adds its file here.
const GUARDED = [
  'AssetsPanel.svelte',
  'AssetList.svelte',
  'AssetDetail.svelte',
  'AssetEditor.svelte',
  'SyncPlanDialog.svelte',
  'HostStrip.svelte',
  'Badge.svelte',
];

const styleOf = (src: string) => src.match(/<style[^>]*>([\s\S]*?)<\/style>/)?.[1] ?? '';

describe('Assets components colour only through tokens', () => {
  for (const file of GUARDED) {
    it(file, () => {
      const css = styleOf(readFileSync(`src/lib/${file}`, 'utf8'));
      expect(css.match(/#[0-9a-fA-F]{3,8}\b/g) ?? []).toEqual([]);
      expect(css.match(/:\s*(white|black)\b/g) ?? []).toEqual([]);
    });
  }
});
```

- [ ] **Step 2: Run them to verify they fail**

Run: `pnpm exec vitest run src/lib/Badge.test.ts src/lib/HostStrip.test.ts src/lib/assets_tokens.test.ts`
Expected: FAIL — `Badge.svelte` does not exist; HostStrip ignores `states`; the guard lists the hex colours in `AssetsPanel`, `AssetDetail`, `AssetEditor`, `SyncPlanDialog` (and `white` in `AssetsPanel`).

- [ ] **Step 3: Write `assets_visual.ts` and `Badge.svelte`**

`src/lib/assets_visual.ts`:

```ts
// Assets M5: the visual vocabulary the workspace shares — badge tones and
// host-dot states (spec, Visuals; Rulings R26, R27). Colour reinforces a
// state; a word or a shape always carries it.

export type BadgeTone = 'neutral' | 'ok' | 'warn' | 'crit' | 'accent' | 'muted';

/** One host's dot in a `HostStrip`. */
export type DotState = 'present' | 'in_sync' | 'differs' | 'missing' | 'absent' | 'na' | 'stale' | 'blocked';

/** What each dot says to a screen reader and in its tooltip. */
export const DOT_LABEL: Record<DotState, string> = {
  present: 'present',
  in_sync: 'in sync',
  differs: 'differs',
  missing: 'missing',
  absent: 'absent',
  na: 'not here',
  stale: 'stale scan',
  blocked: 'blocked',
};

/** A sync op's badge tone, by what it does to a host. */
export function opTone(op: string): BadgeTone {
  if (op === 'overwrite' || op === 'remove') return 'crit';
  if (op === 'create' || op === 'adopt' || op === 'plugin_install') return 'ok';
  if (op === 'update' || op === 'plugin_update') return 'warn';
  return 'muted';
}

/** An applied action's outcome tone. */
export function outcomeTone(outcome: string): BadgeTone {
  if (outcome === 'done') return 'ok';
  if (outcome === 'skipped') return 'muted';
  return 'crit';
}
```

`src/lib/Badge.svelte`:

```svelte
<script lang="ts">
  import type { BadgeTone } from './assets_visual';

  /** One small label for a scope, a catalog, a count, an op or a state
   *  (spec, Visuals: one Badge replaces `.chip`, `.op-badge`, `.count-chip`).
   *  Information, never a control — a clickable one is a `.btn` holding a
   *  Badge. `tone` colours it; `label` (and `glyph`, `dashed`) carry the
   *  meaning, so it is never colour alone. */
  let {
    label,
    tone = 'neutral',
    glyph,
    dashed = false,
    mono = false,
    title,
    testid,
  }: {
    label: string;
    tone?: BadgeTone;
    glyph?: string;
    dashed?: boolean;
    mono?: boolean;
    title?: string;
    testid?: string;
  } = $props();
</script>

<span class="badge {tone}" class:dashed class:mono {title} data-testid={testid}
  >{#if glyph}<span class="glyph" aria-hidden="true">{glyph}</span>{/if}{label}</span
>

<style>
  .badge {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    box-sizing: border-box;
    height: 18px;
    padding: 0 6px;
    border: 1px solid var(--control-border);
    border-radius: var(--radius-sm);
    background: var(--bg);
    color: var(--control-fg-quiet);
    font-size: var(--control-font-sm);
    line-height: 1;
    white-space: nowrap;
  }
  .dashed { border-style: dashed; }
  .mono { font-family: var(--mono); }
  .ok { color: var(--usage-ok); border-color: color-mix(in srgb, var(--usage-ok) 45%, var(--control-border)); }
  .warn { color: var(--usage-warn); border-color: color-mix(in srgb, var(--usage-warn) 45%, var(--control-border)); }
  .crit { color: var(--usage-crit); border-color: color-mix(in srgb, var(--usage-crit) 45%, var(--control-border)); }
  .accent { color: var(--accent); border-color: color-mix(in srgb, var(--accent) 45%, var(--control-border)); }
  .muted { color: var(--fg-muted); background: var(--bg-pane); }
  .glyph { font-size: 10px; }
</style>
```

- [ ] **Step 4: Extend `HostStrip.svelte`**

```svelte
<script lang="ts">
  import { DOT_LABEL, type DotState } from './assets_visual';

  /** One dot per host in `order`, shape + colour + words (app.css: never
   *  colour alone). Identity rows pass `present`/`odd`: filled = present,
   *  half = differs, ring = absent. Asset rows pass `states` (Assets M5,
   *  R27): in sync, differs, missing (ring), not here (dash), stale scan
   *  (hatched), blocked (cross); a host `states` does not name is `na`. */
  let {
    order,
    present = [],
    odd = [],
    states,
  }: { order: string[]; present?: string[]; odd?: string[]; states?: Record<string, DotState> } = $props();

  const stateOf = (h: string): DotState =>
    states ? (states[h] ?? 'na') : !present.includes(h) ? 'absent' : odd.includes(h) ? 'differs' : 'present';
  const label = $derived(order.map((h) => `${h}: ${DOT_LABEL[stateOf(h)]}`).join(', '));
</script>

<span class="strip" role="img" aria-label={label} title={label}>
  {#each order as h (h)}<span class="dot {stateOf(h)}"></span>{/each}
</span>

<style>
  .strip { display: inline-flex; gap: 3px; align-items: center; }
  .dot { width: 9px; height: 9px; border-radius: 50%; box-sizing: border-box; position: relative; }
  .present, .in_sync { background: var(--usage-ok); }
  .differs { background: linear-gradient(90deg, var(--usage-warn) 50%, transparent 50%); box-shadow: inset 0 0 0 1.5px var(--usage-warn); }
  .absent, .missing { box-shadow: inset 0 0 0 1.5px var(--control-border-strong); }
  .na { width: 6px; height: 2px; margin: 0 1.5px; border-radius: 1px; background: var(--border); }
  .stale {
    background: repeating-linear-gradient(135deg, var(--control-border-strong) 0 1.5px, transparent 1.5px 3.5px);
    box-shadow: inset 0 0 0 1px var(--control-border-strong);
  }
  .blocked::before, .blocked::after {
    content: '';
    position: absolute;
    left: 3.5px;
    top: -1px;
    width: 2px;
    height: 11px;
    border-radius: 1px;
    background: var(--usage-crit);
    transform: rotate(45deg);
  }
  .blocked::after { transform: rotate(-45deg); }
</style>
```

- [ ] **Step 5: Put `Badge` in `AssetList` and `SyncPlanDialog`**

`AssetList.svelte`: `import Badge from './Badge.svelte';`; the `chips()` snippet becomes

```svelte
      {#snippet chips()}
        <span class="chips">
          {#if c.in_sync}<Badge tone="ok" glyph="●" label={`${c.in_sync} in sync`} />{/if}
          {#if c.drifted}<Badge tone="warn" glyph="◐" label={`${c.drifted} drifted`} />{/if}
          {#if c.missing}<Badge tone="muted" glyph="○" label={`${c.missing} missing`} />{/if}
          {#if c.unsupported}<Badge tone="muted" glyph="–" label={`${c.unsupported} unsupported`} />{/if}
        </span>
      {/snippet}
```

the needs-person badge `{#if i.class === 'needs_person'}<Badge tone="warn" label={i.reason ?? 'needs a person'} title={i.reason ?? ''} />{/if}`, the orphan badge `<Badge tone="warn" label="orphan" testid={`orphan-badge-${r.host_alias}-${r.harness}-${r.kind}-${r.name}`} />`, and the `.chip`, `.chip.ok|warn|muted`, `.badge.orphan`, `.badge.warn` rules leave its `<style>` (`.chips { display: flex; gap: 4px; }` stays).

`SyncPlanDialog.svelte`: `import Badge from './Badge.svelte'; import { opTone, outcomeTone } from './assets_visual';`; the counts become `{#each countsEntries as [op, n] (op)}<Badge tone={opTone(op)} label={`${op}: ${n}`} />{/each}`, the op `<span class={`op-badge op-${a.op}`}>…</span>` becomes `<Badge tone={opTone(a.op)} label={a.op} />`, and the outcome span becomes

```svelte
            {#if outcome}
              <Badge
                tone={outcomeTone(outcome)}
                label={outcome}
                testid={`plan-outcome-${h.host_alias}-${h.harness}-${a.kind}-${a.name}`}
              />
            {/if}
```

Its `<style>` loses `.count-chip`, `.op-badge`, `.op-*`, `.outcome`, `.outcome-*`, and the rest move to tokens:

```css
  .detail.warning { color: var(--usage-warn); }
  .backup { color: var(--usage-warn); } .secrets { color: var(--fg-muted); } .reason { color: var(--usage-crit); }
  .restart { color: var(--usage-warn); font-size: 12px; margin: 0; }
  .actions button.danger { color: var(--usage-crit); border-color: var(--usage-crit); }
  .error { color: var(--usage-crit); }
```

- [ ] **Step 6: Move the remaining literal colours to tokens**

`AssetDetail.svelte` `<style>`: `.commit` → `color: var(--usage-ok)`; `.sync-btn.danger` → `color: var(--usage-crit); border-color: var(--usage-crit)`; `.lint-error` → `var(--usage-crit)`; `.lint-warn` → `var(--usage-warn)`; `.state-in-sync` → `var(--usage-ok)`, `.state-drifted` → `var(--usage-warn)`; `.warn` → `var(--usage-warn)`; `.error` → `var(--usage-crit)`. (The matrix cells already say "in sync" / "drifted" in words.)
`AssetEditor.svelte`: `.lint-error` and `.error` → `var(--usage-crit)`, `.lint-warn` → `var(--usage-warn)`.
`AssetsPanel.svelte`: `.badge` → `color: var(--usage-warn)`, `.error` → `var(--usage-crit)`, `.primary`'s `color: white` → `color: var(--accent-fg)`.

- [ ] **Step 7: Run the frontend tests**

Run: `pnpm exec vitest run src/lib/Badge.test.ts src/lib/HostStrip.test.ts src/lib/assets_tokens.test.ts src/lib/AssetList.test.ts src/lib/SyncPlanDialog.test.ts src/lib/AssetDetail.test.ts src/lib/AssetEditor.test.ts src/lib/AssetsPanel.test.ts`
Expected: PASS — `AssetList.test.ts`'s `.badge.warn` selector still matches (`class="badge warn …"`), and `plan-outcome-…` still says `done`.
Run: `pnpm check` — Expected: 0 errors.

- [ ] **Step 8: Commit**

```bash
git add src/lib/assets_visual.ts src/lib/Badge.svelte src/lib/Badge.test.ts src/lib/assets_tokens.test.ts \
  src/lib/HostStrip.svelte src/lib/HostStrip.test.ts src/lib/AssetList.svelte src/lib/SyncPlanDialog.svelte \
  src/lib/AssetDetail.svelte src/lib/AssetEditor.svelte src/lib/AssetsPanel.svelte
git commit -m "feat(assets-ui): one Badge, host-dot states, and tokens instead of literal colours

Badge replaces the state chips, op badges, count chips and outcome labels in
the Assets components; HostStrip renders explicit per-host states (in sync,
differs, missing, not here, stale, blocked) with words; a test keeps hex
colours out of every Assets component's styles."
```

---

### Task 6: The workspace's data layer — wire types, stores, the query and the Inbox model (carry 6 type; R10, R14, R16, R21)

**Files:**
- Modify: `src/lib/assets.ts` (`AssetInventoryRow`, `HostState`, `AssetSummary`, `SyncRunSummary`)
- Create: `src/lib/assets_workspace.ts`, `src/lib/assets_workspace.test.ts`
- Create: `src/lib/assets_query.ts`, `src/lib/assets_query.test.ts`
- Create: `src/lib/assets_inbox.ts`, `src/lib/assets_inbox.test.ts`

**Interfaces:**
- Consumes: the four Tauri commands of Task 4; `catalog_list_layers` (existing); `BadgeTone`, `DotState` (Task 5).
- Produces (`assets.ts`): `export type DriftSide = 'host' | 'catalog'`; `AssetInventoryRow.drift_side?: DriftSide | null`, `AssetInventoryRow.catalog_id?: number | null`; `HostState.drift_side?: DriftSide | null`; `AssetSummary.catalog?: string`; `SyncRunSummary.auto?: boolean`.
- Produces (`assets_workspace.ts`): `PERSONAL`; types `CatalogStatus`, `CardKind`, `CardState`, `ChangesetSummary`, `CommitEntry`, `LayerDef`, `HostLayerRow`, `LayerListing`, `WorkspaceView = 'inbox' | 'library'`, `RAIL_VIEWS`, `Selection`, `BadgeSpec`, `WriteContext`; stores `catalogStatuses`, `changesetSummaries`, `layerListing` (`writable<… | null>`); `loadCatalogStatuses()`, `loadChangesets()`, `loadLayers()`, `repoStatusOf(catalog)`, `assetHistory(kind, name, catalog?)`; `isOpenCard(c)`, `keyOf(s)`, `parseKey(key)`, `scopeBadge(a)`, `canWrite(catalog, ctx)`, `summarizeRun(run)`, `ago(secs, now)`.
- Produces (`assets_query.ts`): `QUERY_KEYS`, `QueryKey`, `QueryToken`, `ParsedQuery`, `QueryRow`, `QueryVocab`, `STATES`, `SCOPES`, `parseQuery(raw)`, `matchesQuery(q, row)`, `completions(raw, vocab)`, `applyCompletion(raw, completion)`, `rowOfAsset(a, layers?)`, `rowOfIdentity(id)`, `rowOfOrphan(rows)`.
- Produces (`assets_inbox.ts`): `InboxSection`, `SECTION_LABEL`, `InboxRow`, `Inbox`, `InboxInput`, `hostOrderOf(aliases)`, `assetDots(a, order, stale)`, `buildInbox(input)`, `lastScanOf(listing, inventory)`, `sentence(inbox, ctx)`.

- [ ] **Step 1: Write the failing tests**

`src/lib/assets_query.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { parseQuery, matchesQuery, completions, applyCompletion, type QueryRow } from './assets_query';

const row = (over: Partial<QueryRow> = {}): QueryRow => ({
  kind: 'skill', name: 'infra-status', description: 'Health-check the infrastructure',
  catalog: 'personal', scope: 'shared', layers: ['core'],
  hosts: [
    { host_alias: 'local', state: 'drifted', drift_side: 'host' },
    { host_alias: 'oci', state: 'in_sync' },
    { host_alias: 'htz', state: 'missing' },
  ],
  ...over,
});

describe('parseQuery', () => {
  it('splits tokens from free text, case-insensitively, values by comma', () => {
    expect(parseQuery('Kind:Skill scope:shared,ORG infra')).toEqual({
      tokens: [{ key: 'kind', values: ['skill'] }, { key: 'scope', values: ['shared', 'org'] }],
      text: 'infra',
    });
  });
  it('an unknown key is free text; a key with no value yet matches everything', () => {
    expect(parseQuery('owner:me kind:')).toEqual({ tokens: [], text: 'owner:me' });
  });
});

describe('matchesQuery', () => {
  const m = (q: string, r: QueryRow = row()) => matchesQuery(parseQuery(q), r);
  it('host: is "present there", not missing', () => {
    expect(m('host:oci')).toBe(true);
    expect(m('host:local')).toBe(true);
    expect(m('host:htz')).toBe(false);
  });
  it('kind: takes aliases and dashes', () => {
    expect(m('kind:mcp', row({ kind: 'mcp_server' }))).toBe(true);
    expect(m('kind:mcp-server', row({ kind: 'mcp_server' }))).toBe(true);
    expect(m('kind:agent')).toBe(false);
  });
  it('state: reads any host, plus edited/behind by drift side', () => {
    expect(m('state:in-sync')).toBe(true);
    expect(m('state:edited')).toBe(true);
    expect(m('state:behind')).toBe(false);
    expect(m('state:orphan')).toBe(false);
  });
  it('layer:, catalog: and scope:', () => {
    expect(m('layer:CORE')).toBe(true);
    expect(m('catalog:papayapos')).toBe(false);
    expect(m('scope:org', row({ catalog: 'papayapos', scope: 'private' }))).toBe(true);
    expect(m('scope:private', row({ scope: undefined }))).toBe(true);
    expect(m('scope:managed', row({ managedElsewhere: true }))).toBe(true);
  });
  it('ORs values within a token and ANDs tokens and words', () => {
    expect(m('kind:agent,skill state:missing infra')).toBe(true);
    expect(m('kind:agent,skill state:orphan')).toBe(false);
    expect(m('health check')).toBe(true);
    expect(m('health nope')).toBe(false);
  });
});

describe('completions', () => {
  const vocab = { hosts: ['local', 'oci'], layers: ['core'], catalogs: ['personal', 'papayapos'] };
  it('offers keys, then that key’s values, continuing after a comma', () => {
    expect(completions('ho', vocab)).toEqual(['host:']);
    expect(completions('kind:sk', vocab)).toEqual(['kind:skill']);
    expect(completions('catalog:p', vocab)).toEqual(['catalog:personal', 'catalog:papayapos']);
    expect(completions('host:local,', vocab)).toEqual(['host:local,oci']);
    expect(completions('', vocab)).toEqual([]);
    expect(completions('nope:x', vocab)).toEqual([]);
  });
  it('replaces only the fragment being typed', () => {
    expect(applyCompletion('kind:skill ho', 'host:')).toBe('kind:skill host:');
  });
});
```

`src/lib/assets_inbox.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { buildInbox, hostOrderOf, lastScanOf, sentence } from './assets_inbox';
import type { AssetListing } from './assets';
import type { ChangesetSummary } from './assets_workspace';

const listing: AssetListing = {
  head: 'abc', loaded_at: 1, problems: [],
  assets: [
    { kind: 'skill', name: 'edited', version: '1', description: '', tags: [], catalog: 'personal', scope: 'shared',
      hosts: [{ host_alias: 'trn', harness: 'claude', state: 'drifted', drift_side: 'host' }] },
    { kind: 'skill', name: 'behind', version: '1', description: '', tags: [], catalog: 'papayapos',
      hosts: [{ host_alias: 'oci', harness: 'claude', state: 'drifted', drift_side: 'catalog' }] },
    { kind: 'skill', name: 'unknown', version: '1', description: '', tags: [],
      hosts: [{ host_alias: 'oci', harness: 'claude', state: 'drifted' }] },
    { kind: 'skill', name: 'fine', version: '1', description: '', tags: [],
      hosts: [{ host_alias: 'local', harness: 'claude', state: 'in_sync' }, { host_alias: 'oci', harness: 'claude', state: 'missing' }] },
  ],
  unmanaged: [
    { host_alias: 'mefistos', harness: 'claude', kind: 'skill', name: 'ghost', state: 'orphan', catalog_hash: null, host_hash: null, scanned_at: 50, managed: true },
  ],
  identities: [
    { kind: 'skill', name: 'fresh', hosts: [{ host_alias: 'oci', harness: 'claude', host_hash: 'h' }], signature: 'oci', variants: 1, class: 'normal', reason: null },
    { kind: 'mcp_server', name: 'jira', hosts: [{ host_alias: 'local', harness: 'claude', host_hash: 'h' }], signature: 'local', variants: 1, class: 'needs_person', reason: 'carries a secret' },
    { kind: 'hook', name: 'stop', hosts: [{ host_alias: 'local', harness: 'claude', host_hash: 'h' }], signature: 'local', variants: 1, class: 'fleet_internal', reason: null },
  ],
};
const cards: ChangesetSummary[] = [
  { id: 4, kind: 'bootstrap', summary: 'Adopt 3 as 1 layers', state: 'proposed', created_at: 1, groups: { core: 3 }, pending: 3 },
  { id: 3, kind: 'new', summary: 'old', state: 'applied', created_at: 1 },
];
const order = hostOrderOf(['trn', 'local', 'oci', 'mefistos']);

describe('buildInbox', () => {
  const inbox = buildInbox({ listing, cards, order, stale: new Set(['trn']) });
  const names = (s: keyof typeof inbox.sections) => inbox.sections[s].map((r) => r.name);

  it('puts every row in the section that says what to do next', () => {
    expect(names('cards')).toEqual(['Adopt 3 as 1 layers']);
    expect(names('needs')).toEqual(['jira', 'ghost']);
    expect(names('drifted')).toEqual(['edited', 'unknown']);
    expect(names('behind')).toEqual(['behind']);
    expect(names('fresh')).toEqual(['fresh']);
    expect(names('insync')).toEqual(['fine']);
    expect(inbox.hidden).toBe(1);
    expect(inbox.needCount).toBe(1 + 2 + 2);
  });

  it('says why, with the side that moved', () => {
    const why = (s: keyof typeof inbox.sections, n: string) => inbox.sections[s].find((r) => r.name === n)?.why;
    expect(why('drifted', 'edited')).toBe('Edited on trn');
    expect(why('drifted', 'unknown')).toBe('Differs on oci');
    expect(why('behind', 'behind')).toBe('Behind the catalog on oci');
    expect(why('needs', 'ghost')).toBe('Left on mefistos after the catalog dropped it');
    expect(why('needs', 'jira')).toBe('carries a secret');
    expect(why('fresh', 'fresh')).toBe('Found on oci');
  });

  it('keys rows by what they are, and draws a dot per host in order', () => {
    expect(order).toEqual(['local', 'mefistos', 'oci', 'trn']);
    expect(inbox.sections.behind[0].key).toBe('asset:papayapos:skill/behind');
    expect(inbox.sections.fresh[0].key).toBe('identity:skill/fresh');
    expect(inbox.sections.cards[0].key).toBe('card:4');
    expect(inbox.sections.insync[0].dots).toEqual({ local: 'in_sync', mefistos: 'na', oci: 'missing', trn: 'na' });
    expect(inbox.sections.drifted[0].dots.trn).toBe('stale');
  });
});

describe('sentence', () => {
  it('is a sentence about the fleet', () => {
    const inbox = buildInbox({ listing, cards, order, stale: new Set() });
    expect(sentence(inbox, { reachable: 4, total: 5, lastScan: 50, now: 50 + 180 })).toEqual({
      text: '5 need you · 1 behind the catalog · 1 new on hosts',
      sub: '4/5 hosts · scan 3 min ago',
    });
    const quiet = buildInbox({ listing: { ...listing, assets: [listing.assets[3]], unmanaged: [], identities: [] }, cards: [], order, stale: new Set() });
    expect(sentence(quiet, { reachable: 5, total: 5, lastScan: null, now: 1 }).text).toBe('Fleet converged');
  });
  it('takes the newest scan of the inventory and the listing', () => {
    expect(lastScanOf(listing, [])).toBe(50);
    expect(lastScanOf(null, [])).toBeNull();
  });
});
```

`src/lib/assets_workspace.test.ts`:

```ts
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import {
  keyOf, parseKey, scopeBadge, canWrite, summarizeRun, ago, assetHistory,
  loadChangesets, changesetSummaries, loadCatalogStatuses, catalogStatuses,
} from './assets_workspace';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;
beforeEach(() => { invoke.mockReset(); changesetSummaries.set(null); catalogStatuses.set(null); });

describe('selection keys', () => {
  it('round-trip every kind of row, names with colons included', () => {
    for (const s of [
      { type: 'asset' as const, catalog: 'papayapos', kind: 'skill', name: 'ppt-implement' },
      { type: 'identity' as const, kind: 'skill', name: 'superpowers:brainstorming' },
      { type: 'orphan' as const, kind: 'hook', name: 'stop' },
      { type: 'card' as const, id: 12 },
    ]) expect(parseKey(keyOf(s))).toEqual(s);
    expect(parseKey('nonsense')).toBeNull();
    expect(parseKey('card:x')).toBeNull();
  });
});

describe('scope and write rights', () => {
  it('names the scope in words, private by its dashed border', () => {
    expect(scopeBadge({ catalog: 'papayapos' })).toMatchObject({ label: 'papayapos', tone: 'accent', dashed: false });
    expect(scopeBadge({ catalog: 'personal', scope: 'shared' })).toMatchObject({ label: 'shared', dashed: false });
    expect(scopeBadge({})).toMatchObject({ label: 'private', dashed: true });
  });
  it('writes: never read-only; always standalone or personal; an org catalog only when granted', () => {
    const statuses = [{ id: 2, name: 'acme', org_id: 7, repo_path: '/a', remote_url: null, head_commit: null, last_loaded_at: null, state: 'loaded' as const, asset_count: 0, granted: ['desk'] }];
    expect(canWrite('personal', { readOnly: true, remote: true, clientName: 'desk', statuses })).toBe(false);
    expect(canWrite('acme', { readOnly: false, remote: false, clientName: null, statuses: null })).toBe(true);
    expect(canWrite('acme', { readOnly: false, remote: true, clientName: 'desk', statuses })).toBe(true);
    expect(canWrite('acme', { readOnly: false, remote: true, clientName: 'phone', statuses })).toBe(false);
    expect(canWrite(undefined, { readOnly: false, remote: true, clientName: null, statuses: null })).toBe(true);
  });
});

describe('words', () => {
  it('summarises a run and an age', () => {
    expect(summarizeRun({ plan_id: 'p', started_at: 1, finished_at: 2, hosts: [
      { host_alias: 'a', harness: 'claude', status: 'applied', detail: null, restart_required: false, actions: [] },
      { host_alias: 'b', harness: 'claude', status: 'applied', detail: null, restart_required: false, actions: [] },
    ] })).toContain('2 applied');
    expect(ago(100, 130)).toBe('just now');
    expect(ago(0, 180)).toBe('3 min ago');
    expect(ago(0, 7200)).toBe('2 h ago');
    expect(ago(0, 172800)).toBe('2 d ago');
  });
});

describe('reads', () => {
  it('loads the cards, and reads a refusal as "not available here"', async () => {
    invoke.mockResolvedValueOnce([{ id: 1, kind: 'new', summary: 's', state: 'proposed', created_at: 1 }]);
    await loadChangesets();
    expect(invoke).toHaveBeenCalledWith('catalog_list_changesets', undefined);
    expect(get(changesetSummaries)).toHaveLength(1);
    invoke.mockRejectedValueOnce({ code: 'E_FORBIDDEN', message: 'no' });
    await loadCatalogStatuses();
    expect(get(catalogStatuses)).toBeNull();
  });
  it('asks for History with the catalog only when it is not personal', async () => {
    invoke.mockResolvedValue([]);
    await assetHistory('skill', 'w', 'personal');
    expect(invoke).toHaveBeenLastCalledWith('catalog_asset_history', { args: { kind: 'skill', name: 'w', catalog: null } });
    await assetHistory('skill', 'w', 'acme');
    expect(invoke).toHaveBeenLastCalledWith('catalog_asset_history', { args: { kind: 'skill', name: 'w', catalog: 'acme' } });
  });
});
```

- [ ] **Step 2: Run them to verify they fail**

Run: `pnpm exec vitest run src/lib/assets_query.test.ts src/lib/assets_inbox.test.ts src/lib/assets_workspace.test.ts`
Expected: FAIL — `Failed to resolve import "./assets_query"` (and the other two modules).

- [ ] **Step 3: Extend the wire types in `assets.ts`**

```ts
/** Assets M5 (R4): on a drifted managed copy, which side moved. */
export type DriftSide = 'host' | 'catalog';
```

`AssetInventoryRow` gains `/** Migration 091: the asset's catalog. */ catalog_id?: number | null;` and `/** Assets M5 (R4). Absent from an older hub. */ drift_side?: DriftSide | null;`. `HostState` becomes `{ host_alias: string; harness: string; state: string; /** Assets M5 (R4). */ drift_side?: DriftSide | null }`. `AssetSummary` gains `/** Assets M5 (R11): the asset's catalog; absent (= personal) from an older hub. */ catalog?: string;`. `SyncRunSummary` gains `/** M4 I3 / M5 R10: SB6's automatic run, not a person's. */ auto?: boolean;`.

- [ ] **Step 4: Write `assets_workspace.ts`**

```ts
// Assets M5: the workspace's own reads — the catalogs behind the footer's
// chips, the cards behind the Inbox's Proposed rows, the layers behind
// `layer:`, a catalog's repo status, an asset's History — and the small pure
// helpers the views share. Mirrors service/catalog/{catalogs, changesets/mod,
// repo}.rs and the commands of commands/assets.rs (Rulings R13).
import { writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';
import type { AssetSummary, RepoStatus, SyncRunSummary } from './assets';
import type { BadgeTone } from './assets_visual';

export const PERSONAL = 'personal';

export interface CatalogStatus {
  id: number;
  name: string;
  org_id: number | null;
  org?: string | null;
  repo_path: string;
  remote_url: string | null;
  head_commit: string | null;
  last_loaded_at: number | null;
  state: 'loaded' | 'problem' | 'not_loaded';
  problem?: string | null;
  asset_count: number;
  admitted?: string[];
  granted?: string[];
}

export type CardKind = 'bootstrap' | 'new' | 'drift' | 'rollout';
export type CardState = 'proposed' | 'applied' | 'undone' | 'dismissed' | 'failed';
export interface ChangesetSummary {
  id: number;
  kind: CardKind;
  summary: string;
  state: CardState;
  created_at: number;
  applied_at?: number | null;
  error?: string | null;
  groups?: Record<string, number>;
  pending?: number;
  undoable?: boolean;
}

export interface CommitEntry { sha: string; at: number; author: string; subject: string }

export interface LayerDef { name: string; axis: 'role' | 'context'; description?: string; extends?: string; members?: string[] }
export interface HostLayerRow { host_alias: string; catalog_id?: number; layer_name: string; axis: string; position: number; active: boolean }
export interface LayerListing { layers: LayerDef[]; hosts: HostLayerRow[] }

/** The rail's views in M5 (R15); Layers and Hosts join in M6. */
export type WorkspaceView = 'inbox' | 'library';
export const RAIL_VIEWS: readonly { id: WorkspaceView; label: string }[] = [
  { id: 'inbox', label: 'Inbox' },
  { id: 'library', label: 'Library' },
];

/** `null` until loaded, and again when the read was refused (an ungranted,
 *  readonly or org-bound client): the views read it as "not here". */
export const catalogStatuses = writable<CatalogStatus[] | null>(null);
export const changesetSummaries = writable<ChangesetSummary[] | null>(null);
export const layerListing = writable<LayerListing | null>(null);

export async function loadCatalogStatuses(): Promise<Result<CatalogStatus[]>> {
  const r = await invokeCmd<CatalogStatus[]>('catalog_list_catalogs');
  catalogStatuses.set(r.ok ? r.value : null);
  return r;
}

export async function loadChangesets(): Promise<Result<ChangesetSummary[]>> {
  const r = await invokeCmd<ChangesetSummary[]>('catalog_list_changesets');
  changesetSummaries.set(r.ok ? r.value : null);
  return r;
}

export async function loadLayers(): Promise<Result<LayerListing>> {
  const r = await invokeCmd<LayerListing>('catalog_list_layers');
  layerListing.set(r.ok ? r.value : null);
  return r;
}

export function repoStatusOf(catalog: string): Promise<Result<RepoStatus>> {
  return invokeCmd<RepoStatus>('catalog_repo_status_in', { args: { name: catalog } });
}

export function assetHistory(kind: string, name: string, catalog?: string | null): Promise<Result<CommitEntry[]>> {
  return invokeCmd<CommitEntry[]>('catalog_asset_history', {
    args: { kind, name, catalog: catalog && catalog !== PERSONAL ? catalog : null },
  });
}

/** R14: a card the Inbox shows — still to apply, or failed and retryable. */
export function isOpenCard(c: ChangesetSummary): boolean {
  return c.state === 'proposed' || c.state === 'failed';
}

/** What a list row is — one string per row (`data-row-key`), shared by the
 *  views, the keyboard and the Inspector. */
export type Selection =
  | { type: 'asset'; catalog: string; kind: string; name: string }
  | { type: 'identity'; kind: string; name: string }
  | { type: 'orphan'; kind: string; name: string }
  | { type: 'card'; id: number };

export function keyOf(s: Selection): string {
  switch (s.type) {
    case 'asset':
      return `asset:${s.catalog}:${s.kind}/${s.name}`;
    case 'identity':
    case 'orphan':
      return `${s.type}:${s.kind}/${s.name}`;
    case 'card':
      return `card:${s.id}`;
  }
}

export function parseKey(key: string): Selection | null {
  const i = key.indexOf(':');
  if (i < 0) return null;
  const type = key.slice(0, i);
  let body = key.slice(i + 1);
  if (type === 'card') {
    const id = Number(body);
    return body !== '' && Number.isInteger(id) ? { type, id } : null;
  }
  let catalog = PERSONAL;
  if (type === 'asset') {
    const j = body.indexOf(':');
    if (j < 0) return null;
    catalog = body.slice(0, j);
    body = body.slice(j + 1);
  }
  const s = body.indexOf('/');
  if (s <= 0) return null;
  const kind = body.slice(0, s);
  const name = body.slice(s + 1);
  if (type === 'asset') return { type, catalog, kind, name };
  if (type === 'identity' || type === 'orphan') return { type, kind, name };
  return null;
}

export interface BadgeSpec { label: string; tone: BadgeTone; dashed: boolean; title: string }

/** The scope/catalog badge of a catalog asset (spec, Inbox rows: "a `Badge`
 *  for scope/catalog"): the org catalog's name, else shared or private. */
export function scopeBadge(a: Pick<AssetSummary, 'catalog' | 'scope'>): BadgeSpec {
  const catalog = a.catalog ?? PERSONAL;
  if (catalog !== PERSONAL) {
    return { label: catalog, tone: 'accent', dashed: false, title: `In catalog ${catalog}: only hosts that accept it receive it` };
  }
  if (a.scope === 'shared') {
    return { label: 'shared', tone: 'neutral', dashed: false, title: 'Personal catalog, shared: hosts of an org may receive it' };
  }
  return { label: 'private', tone: 'neutral', dashed: true, title: 'Personal catalog, private: never installed on a host of an org' };
}

export interface WriteContext { readOnly: boolean; remote: boolean; clientName: string | null; statuses: CatalogStatus[] | null }

/** R20: whether this window may author in `catalog`. Read-only: never.
 *  Standalone, or personal on a granted hub client: yes. Another catalog on
 *  a hub client: when `list_catalogs` names this client among its grantees. */
export function canWrite(catalog: string | undefined, ctx: WriteContext): boolean {
  if (ctx.readOnly) return false;
  const c = catalog ?? PERSONAL;
  if (!ctx.remote || c === PERSONAL) return true;
  const row = ctx.statuses?.find((s) => s.name === c);
  return !!row && !!ctx.clientName && (row.granted ?? []).includes(ctx.clientName);
}

/** The footer's last-sync line. */
export function summarizeRun(run: SyncRunSummary): string {
  const counts: Record<string, number> = {};
  for (const h of run.hosts) counts[h.status] = (counts[h.status] ?? 0) + 1;
  const parts = Object.entries(counts).map(([k, n]) => `${n} ${k}`);
  return `${new Date(run.finished_at * 1000).toLocaleString()} — ${parts.join(', ') || 'no hosts'}`;
}

/** "3 min ago" from Unix seconds. */
export function ago(secs: number, now: number): string {
  const d = Math.max(0, now - secs);
  if (d < 60) return 'just now';
  if (d < 3600) return `${Math.floor(d / 60)} min ago`;
  if (d < 86400) return `${Math.floor(d / 3600)} h ago`;
  return `${Math.floor(d / 86400)} d ago`;
}
```

- [ ] **Step 5: Write `assets_query.ts`**

```ts
// Assets M5: the token query (spec, Query; Rulings R21). `key:v1,v2` tokens
// AND together, values within one token OR; unknown keys and bare words are
// free text matched against name and description. Case-insensitive.
import { KIND_ORDER, type AssetIdentity, type AssetInventoryRow, type AssetSummary } from './assets';

export const QUERY_KEYS = ['host', 'kind', 'state', 'layer', 'catalog', 'scope'] as const;
export type QueryKey = (typeof QUERY_KEYS)[number];
export interface QueryToken { key: QueryKey; values: string[] }
export interface ParsedQuery { tokens: QueryToken[]; text: string }

/** What a list row offers the query. `catalog: null` = in no catalog (an
 *  identity, an orphan). */
export interface QueryRow {
  kind: string;
  name: string;
  description?: string;
  catalog?: string | null;
  scope?: string | null;
  hosts: { host_alias: string; state: string; drift_side?: string | null }[];
  layers?: string[];
  /** R20: an asset this window can only look at. */
  managedElsewhere?: boolean;
}

export interface QueryVocab { hosts: string[]; layers: string[]; catalogs: string[] }

export const STATES = ['in_sync', 'drifted', 'edited', 'behind', 'missing', 'unmanaged', 'orphan'];
export const SCOPES = ['private', 'shared', 'org', 'managed'];
const KIND_ALIASES: Record<string, string> = { mcp: 'mcp_server', plugin: 'plugin_ref', skills: 'skill', agents: 'agent', hooks: 'hook' };
const PRESENT = new Set(['in_sync', 'drifted', 'unmanaged', 'orphan']);
const isKey = (k: string): k is QueryKey => (QUERY_KEYS as readonly string[]).includes(k);
const snake = (v: string) => v.replace(/-/g, '_');

export function parseQuery(raw: string): ParsedQuery {
  const tokens: QueryToken[] = [];
  const text: string[] = [];
  for (const part of raw.split(/\s+/).filter(Boolean)) {
    const i = part.indexOf(':');
    const key = i > 0 ? part.slice(0, i).toLowerCase() : '';
    if (isKey(key)) {
      const values = part.slice(i + 1).split(',').map((v) => v.trim().toLowerCase()).filter(Boolean);
      if (values.length) tokens.push({ key, values });
      continue;
    }
    text.push(part.toLowerCase());
  }
  return { tokens, text: text.join(' ') };
}

function matchesToken(key: QueryKey, value: string, row: QueryRow): boolean {
  switch (key) {
    case 'host':
      return row.hosts.some((h) => h.host_alias.toLowerCase() === value && PRESENT.has(h.state));
    case 'kind':
      return row.kind === snake(KIND_ALIASES[value] ?? value);
    case 'state': {
      const s = snake(value);
      if (s === 'edited') return row.hosts.some((h) => h.state === 'drifted' && h.drift_side === 'host');
      if (s === 'behind') return row.hosts.some((h) => h.state === 'drifted' && h.drift_side === 'catalog');
      return row.hosts.some((h) => h.state === s);
    }
    case 'layer':
      return (row.layers ?? []).some((l) => l.toLowerCase() === value);
    case 'catalog':
      return (row.catalog ?? '').toLowerCase() === value;
    case 'scope':
      if (value === 'managed') return !!row.managedElsewhere;
      if (value === 'org') return !!row.catalog && row.catalog !== 'personal';
      return row.catalog === 'personal' && (row.scope ?? 'private') === value;
  }
}

export function matchesQuery(q: ParsedQuery, row: QueryRow): boolean {
  for (const t of q.tokens) if (!t.values.some((v) => matchesToken(t.key, v, row))) return false;
  if (!q.text) return true;
  const hay = `${row.name} ${row.description ?? ''}`.toLowerCase();
  return q.text.split(' ').every((w) => hay.includes(w));
}

function valuesFor(key: QueryKey, vocab: QueryVocab): string[] {
  switch (key) {
    case 'host': return vocab.hosts;
    case 'kind': return [...KIND_ORDER];
    case 'state': return STATES;
    case 'layer': return vocab.layers;
    case 'catalog': return vocab.catalogs;
    case 'scope': return SCOPES;
  }
}

/** Completions for the fragment at the end of `raw` — whole replacement
 *  fragments, at most 8; none for an empty fragment. */
export function completions(raw: string, vocab: QueryVocab): string[] {
  const frag = (/(\S*)$/.exec(raw)?.[1] ?? '').toLowerCase();
  if (!frag) return [];
  const i = frag.indexOf(':');
  if (i < 0) return QUERY_KEYS.filter((k) => k.startsWith(frag)).map((k) => `${k}:`);
  const key = frag.slice(0, i);
  if (!isKey(key)) return [];
  const done = frag.slice(i + 1).split(',');
  const partial = done.pop() ?? '';
  const head = `${key}:${done.map((v) => `${v},`).join('')}`;
  return valuesFor(key, vocab)
    .filter((v) => v.toLowerCase().startsWith(partial) && !done.includes(v.toLowerCase()))
    .slice(0, 8)
    .map((v) => `${head}${v}`);
}

/** `raw` with its last fragment replaced by `completion`. */
export function applyCompletion(raw: string, completion: string): string {
  return raw.replace(/\S*$/, completion);
}

export function rowOfAsset(a: AssetSummary, layers: string[] = []): QueryRow {
  return { kind: a.kind, name: a.name, description: a.description, catalog: a.catalog ?? 'personal', scope: a.scope ?? 'private', hosts: a.hosts, layers };
}

export function rowOfIdentity(id: AssetIdentity): QueryRow {
  return { kind: id.kind, name: id.name, catalog: null, scope: null, hosts: id.hosts.map((h) => ({ host_alias: h.host_alias, state: 'unmanaged' })) };
}

export function rowOfOrphan(rows: AssetInventoryRow[]): QueryRow {
  return { kind: rows[0].kind, name: rows[0].name, catalog: null, scope: null, hosts: rows.map((r) => ({ host_alias: r.host_alias, state: 'orphan' })) };
}
```

- [ ] **Step 6: Write `assets_inbox.ts`**

```ts
// Assets M5: the Inbox (spec, Workspace shell → Inbox; Rulings R16) and the
// sentence header — pure functions over the listing and the cards.
import { identitiesOf, type AssetIdentity, type AssetInventoryRow, type AssetListing, type AssetSummary, type HostState } from './assets';
import type { DotState } from './assets_visual';
import { ago, isOpenCard, keyOf, PERSONAL, type ChangesetSummary } from './assets_workspace';
import { rowOfAsset, rowOfIdentity, rowOfOrphan, type QueryRow } from './assets_query';

export type InboxSection = 'cards' | 'needs' | 'drifted' | 'behind' | 'fresh' | 'insync';
export const SECTION_LABEL: Record<InboxSection, string> = {
  cards: 'Proposed',
  needs: 'Needs you',
  drifted: 'Drifted',
  behind: 'Behind the catalog',
  fresh: 'New on hosts',
  insync: 'In sync',
};

export interface InboxRow {
  key: string;
  kind: string;
  /** The asset's name, or a card's sentence. */
  name: string;
  why: string;
  dots: Record<string, DotState>;
  query: QueryRow;
  asset?: AssetSummary;
  identity?: AssetIdentity;
  card?: ChangesetSummary;
}

export interface Inbox {
  sections: Record<InboxSection, InboxRow[]>;
  /** Fleet internals (`fleet_internal`, `harness_internal`): counted, not listed. */
  hidden: number;
  /** Cards + needs you + drifted: what the rail's Inbox count says. */
  needCount: number;
}

export interface InboxInput {
  listing: AssetListing;
  cards: ChangesetSummary[] | null;
  order: string[];
  /** Hosts the hosts store says are unreachable: their dots are stale. */
  stale: ReadonlySet<string>;
  /** A catalog asset's personal layers, for `layer:`. */
  layersOf?: (a: AssetSummary) => string[];
}

/** `local` first, then alphabetical — the fixed dot order. */
export function hostOrderOf(aliases: Iterable<string>): string[] {
  const all = new Set(aliases);
  const rest = [...all].filter((a) => a !== 'local').sort();
  return all.has('local') ? ['local', ...rest] : rest;
}

const words = (hosts: string[]) => hosts.join(', ');
const hostsWhere = (a: AssetSummary, pred: (s: HostState) => boolean) => [...new Set(a.hosts.filter(pred).map((s) => s.host_alias))];

/** One catalog asset's dots: differs over missing over in sync; a host with
 *  no row is `na`; a row on an unreachable host is `stale`. */
export function assetDots(a: AssetSummary, order: string[], stale: ReadonlySet<string>): Record<string, DotState> {
  const out: Record<string, DotState> = {};
  for (const h of order) {
    const states = a.hosts.filter((s) => s.host_alias === h).map((s) => s.state);
    let d: DotState = 'na';
    if (states.includes('drifted')) d = 'differs';
    else if (states.includes('missing')) d = 'missing';
    else if (states.includes('in_sync')) d = 'in_sync';
    if (states.length > 0 && stale.has(h)) d = 'stale';
    out[h] = d;
  }
  return out;
}

function identityDots(id: AssetIdentity, order: string[], stale: ReadonlySet<string>): Record<string, DotState> {
  const present = new Set(id.hosts.map((h) => h.host_alias));
  const odd = new Set(id.reason?.startsWith('copies differ on ') ? id.reason.slice(17).split(', ') : []);
  const out: Record<string, DotState> = {};
  for (const h of order) out[h] = !present.has(h) ? 'absent' : stale.has(h) ? 'stale' : odd.has(h) ? 'differs' : 'present';
  return out;
}

export function buildInbox(input: InboxInput): Inbox {
  const { listing, order, stale } = input;
  const sections: Record<InboxSection, InboxRow[]> = { cards: [], needs: [], drifted: [], behind: [], fresh: [], insync: [] };

  for (const c of (input.cards ?? []).filter(isOpenCard)) {
    sections.cards.push({
      key: keyOf({ type: 'card', id: c.id }), kind: c.kind, name: c.summary, why: c.error ?? '',
      dots: {}, query: { kind: c.kind, name: c.summary, hosts: [] }, card: c,
    });
  }

  let hidden = 0;
  for (const id of identitiesOf(listing)) {
    if (id.class === 'fleet_internal' || id.class === 'harness_internal') {
      hidden += 1;
      continue;
    }
    const hosts = [...new Set(id.hosts.map((h) => h.host_alias))];
    const row: InboxRow = {
      key: keyOf({ type: 'identity', kind: id.kind, name: id.name }), kind: id.kind, name: id.name,
      why: id.class === 'needs_person' ? (id.reason ?? 'needs a person') : `Found on ${words(hosts)}`,
      dots: identityDots(id, order, stale), query: rowOfIdentity(id), identity: id,
    };
    (id.class === 'needs_person' ? sections.needs : sections.fresh).push(row);
  }

  const orphans = new Map<string, AssetInventoryRow[]>();
  for (const r of listing.unmanaged.filter((r) => r.state === 'orphan')) {
    const k = `${r.kind}/${r.name}`;
    orphans.set(k, [...(orphans.get(k) ?? []), r]);
  }
  for (const rows of orphans.values()) {
    const { kind, name } = rows[0];
    const hosts = [...new Set(rows.map((r) => r.host_alias))];
    const dots: Record<string, DotState> = {};
    for (const h of order) dots[h] = hosts.includes(h) ? 'differs' : 'na';
    sections.needs.push({
      key: keyOf({ type: 'orphan', kind, name }), kind, name,
      why: `Left on ${words(hosts)} after the catalog dropped it`, dots, query: rowOfOrphan(rows),
    });
  }

  for (const a of listing.assets) {
    const edited = hostsWhere(a, (s) => s.state === 'drifted' && s.drift_side === 'host');
    const unknown = hostsWhere(a, (s) => s.state === 'drifted' && !s.drift_side);
    const behind = hostsWhere(a, (s) => s.state === 'drifted' && s.drift_side === 'catalog');
    const row: InboxRow = {
      key: keyOf({ type: 'asset', catalog: a.catalog ?? PERSONAL, kind: a.kind, name: a.name }),
      kind: a.kind, name: a.name, why: '', dots: assetDots(a, order, stale),
      query: rowOfAsset(a, input.layersOf?.(a) ?? []), asset: a,
    };
    if (edited.length || unknown.length) {
      row.why = [edited.length ? `Edited on ${words(edited)}` : '', unknown.length ? `Differs on ${words(unknown)}` : '']
        .filter(Boolean)
        .join(' · ');
      sections.drifted.push(row);
    } else if (behind.length) {
      row.why = `Behind the catalog on ${words(behind)}`;
      sections.behind.push(row);
    } else {
      sections.insync.push(row);
    }
  }

  return { sections, hidden, needCount: sections.cards.length + sections.needs.length + sections.drifted.length };
}

/** The newest scan the window knows of (Unix seconds), or null. */
export function lastScanOf(listing: AssetListing | null, inventory: AssetInventoryRow[]): number | null {
  let best: number | null = null;
  for (const r of [...inventory, ...(listing?.unmanaged ?? [])]) {
    if (best === null || r.scanned_at > best) best = r.scanned_at;
  }
  return best;
}

export interface Sentence { text: string; sub: string }

/** The sticky header (parent spec: "a sentence: `Fleet converged · 5/5
 *  hosts · last scan 3m`, or `9 need you`"). */
export function sentence(inbox: Inbox, ctx: { reachable: number; total: number; lastScan: number | null; now: number }): Sentence {
  const n = inbox.needCount;
  const behind = inbox.sections.behind.length;
  const fresh = inbox.sections.fresh.length;
  const scan = ctx.lastScan === null ? 'never scanned' : `scan ${ago(ctx.lastScan, ctx.now)}`;
  const parts: string[] = [];
  if (n) parts.push(`${n} need${n === 1 ? 's' : ''} you`);
  if (behind) parts.push(`${behind} behind the catalog`);
  if (fresh) parts.push(`${fresh} new on hosts`);
  return { text: parts.length ? parts.join(' · ') : 'Fleet converged', sub: `${ctx.reachable}/${ctx.total} hosts · ${scan}` };
}
```

- [ ] **Step 7: Run the tests**

Run: `pnpm exec vitest run src/lib/assets_query.test.ts src/lib/assets_inbox.test.ts src/lib/assets_workspace.test.ts src/lib/assets.test.ts`
Expected: PASS.
Run: `pnpm check` — Expected: 0 errors.

- [ ] **Step 8: Commit**

```bash
git add src/lib/assets.ts src/lib/assets_workspace.ts src/lib/assets_workspace.test.ts \
  src/lib/assets_query.ts src/lib/assets_query.test.ts src/lib/assets_inbox.ts src/lib/assets_inbox.test.ts
git commit -m "feat(assets-ui): the workspace's data layer — catalogs, cards, the token query, the Inbox model

Wire types gain catalog, drift_side and the auto flag; stores and loaders
for the catalog chips, the cards and the layers; selection keys; the token
query with completion; the Inbox's sections, dots and sentence header."
```

---

### Task 7: The Inbox view — sections, folded in-sync, proposed cards read-only (R14, R16)

**Files:**
- Create: `src/lib/AssetsInbox.svelte`, `src/lib/AssetsInbox.test.ts`
- Modify: `src/lib/assets_tokens.test.ts` (add `'AssetsInbox.svelte'` to `GUARDED`)

**Interfaces:**
- Consumes: `Inbox`, `InboxRow`, `InboxSection`, `SECTION_LABEL`, `buildInbox` (Task 6); `ParsedQuery`, `matchesQuery`, `parseQuery` (Task 6); `scopeBadge` (Task 6); `Badge`, `HostStrip` (Task 5).
- Produces: `AssetsInbox.svelte` props `{ inbox: Inbox; order: string[]; selectedKey: string | null; query: ParsedQuery; onselect: (key: string) => void }`. Every row is a `<button class="row" data-row-key={key} aria-current>`; section headers `data-testid="inbox-section-<section>"`; rows `data-testid="inbox-row-<key>"`; the fold `inbox-insync-toggle`; the empty line `inbox-quiet`.

- [ ] **Step 1: Write the failing test**

`src/lib/AssetsInbox.test.ts`:

```ts
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi } from 'vitest';
import AssetsInbox from './AssetsInbox.svelte';
import { buildInbox } from './assets_inbox';
import { parseQuery } from './assets_query';
import type { AssetListing } from './assets';

const listing: AssetListing = {
  head: 'abc', loaded_at: 1, problems: [], unmanaged: [],
  assets: [
    { kind: 'skill', name: 'edited', version: '1', description: '', tags: [], catalog: 'personal', scope: 'shared',
      hosts: [{ host_alias: 'oci', harness: 'claude', state: 'drifted', drift_side: 'host' }] },
    { kind: 'skill', name: 'quiet', version: '1', description: '', tags: [], catalog: 'papayapos',
      hosts: [{ host_alias: 'oci', harness: 'claude', state: 'in_sync' }] },
  ],
  identities: [
    { kind: 'skill', name: 'fresh', hosts: [{ host_alias: 'oci', harness: 'claude', host_hash: 'h' }], signature: 'oci', variants: 1, class: 'normal', reason: null },
  ],
};
const inbox = buildInbox({
  listing,
  cards: [{ id: 7, kind: 'bootstrap', summary: 'Adopt 1 as 1 layers', state: 'failed', created_at: 1, error: 'core: import failed', groups: { core: 1 } }],
  order: ['local', 'oci'],
  stale: new Set(),
});
const props = (over = {}) => ({ inbox, order: ['local', 'oci'], selectedKey: null, query: parseQuery(''), onselect: vi.fn(), ...over });

describe('AssetsInbox', () => {
  it('shows proposed cards first, then the sections, with in sync folded to one line', async () => {
    render(AssetsInbox, props());
    const headers = [...document.querySelectorAll('[data-testid^="inbox-section-"]')].map((h) => h.getAttribute('data-testid'));
    expect(headers).toEqual(['inbox-section-cards', 'inbox-section-drifted', 'inbox-section-fresh']);
    const card = screen.getByTestId('inbox-row-card:7');
    expect(card.textContent).toContain('Adopt 1 as 1 layers');
    expect(card.textContent).toContain('failed');
    expect(card.textContent).toContain('core: import failed');
    expect(card.querySelectorAll('button')).toHaveLength(0);
    expect(screen.queryByTestId('inbox-row-asset:papayapos:skill/quiet')).toBeNull();
    const fold = screen.getByTestId('inbox-insync-toggle');
    expect(fold.getAttribute('aria-expanded')).toBe('false');
    expect(fold.textContent).toContain('In sync');
    await fireEvent.click(fold);
    expect(screen.getByTestId('inbox-row-asset:papayapos:skill/quiet').textContent).toContain('papayapos');
  });

  it('says why, with the scope badge and one dot per host', () => {
    render(AssetsInbox, props());
    const row = screen.getByTestId('inbox-row-asset:personal:skill/edited');
    expect(row.textContent).toContain('Edited on oci');
    expect(row.textContent).toContain('shared');
    expect(row.querySelector('.strip')?.getAttribute('aria-label')).toBe('local: not here, oci: differs');
  });

  it('selects a row by its key, and marks the selected one', async () => {
    const onselect = vi.fn();
    render(AssetsInbox, props({ onselect, selectedKey: 'identity:skill/fresh' }));
    expect(screen.getByTestId('inbox-row-identity:skill/fresh').getAttribute('aria-current')).toBe('true');
    await fireEvent.click(screen.getByTestId('inbox-row-asset:personal:skill/edited'));
    expect(onselect).toHaveBeenCalledWith('asset:personal:skill/edited');
  });

  it('filters by the query; tokens leave the cards out', () => {
    render(AssetsInbox, props({ query: parseQuery('kind:skill fresh') }));
    expect(screen.getByTestId('inbox-row-identity:skill/fresh')).toBeTruthy();
    expect(screen.queryByTestId('inbox-row-asset:personal:skill/edited')).toBeNull();
    expect(screen.queryByTestId('inbox-row-card:7')).toBeNull();
  });

  it('is quiet when nothing needs you', () => {
    const calm = buildInbox({ listing: { ...listing, assets: [listing.assets[1]], identities: [] }, cards: [], order: ['oci'], stale: new Set() });
    render(AssetsInbox, props({ inbox: calm }));
    expect(screen.getByTestId('inbox-quiet').textContent).toBe('Nothing needs you.');
  });
});
```

- [ ] **Step 2: Run it to verify it fails**

Run: `pnpm exec vitest run src/lib/AssetsInbox.test.ts`
Expected: FAIL — `Failed to resolve import "./AssetsInbox.svelte"`.

- [ ] **Step 3: Write `AssetsInbox.svelte`**

```svelte
<script lang="ts">
  import Badge from './Badge.svelte';
  import HostStrip from './HostStrip.svelte';
  import { SECTION_LABEL, type Inbox, type InboxRow, type InboxSection } from './assets_inbox';
  import { matchesQuery, type ParsedQuery } from './assets_query';
  import { scopeBadge } from './assets_workspace';

  /** The Inbox (spec, Workspace shell; Rulings R14, R16): open cards first,
   *  read-only until M6; then Needs you, Drifted, Behind the catalog, New on
   *  hosts; In sync folds to one line. Every row is one asset identity — or
   *  one card — never one host copy. */
  let {
    inbox,
    order,
    selectedKey,
    query,
    onselect,
  }: {
    inbox: Inbox;
    order: string[];
    selectedKey: string | null;
    query: ParsedQuery;
    onselect: (key: string) => void;
  } = $props();

  const OPEN: InboxSection[] = ['needs', 'drifted', 'behind', 'fresh'];
  const KIND_LETTER: Record<string, string> = { skill: 'S', agent: 'A', hook: 'H', mcp_server: 'M', plugin_ref: 'P' };
  let insyncOpen = $state(false);

  // Cards have no hosts, scope or layer: any token leaves them out; free text
  // matches their sentence.
  const keep = (r: InboxRow) =>
    r.card
      ? query.tokens.length === 0 && (!query.text || r.card.summary.toLowerCase().includes(query.text))
      : matchesQuery(query, r.query);
  const shown = $derived(
    Object.fromEntries(
      (Object.keys(inbox.sections) as InboxSection[]).map((s) => [s, inbox.sections[s].filter(keep)]),
    ) as Record<InboxSection, InboxRow[]>,
  );
  const quiet = $derived(shown.cards.length + OPEN.reduce((n, s) => n + shown[s].length, 0) === 0);
</script>

{#snippet assetRow(r: InboxRow)}
  <button
    type="button"
    class="row"
    class:selected={selectedKey === r.key}
    aria-current={selectedKey === r.key ? 'true' : undefined}
    data-row-key={r.key}
    data-testid={`inbox-row-${r.key}`}
    onclick={() => onselect(r.key)}
  >
    <span class="kico" title={r.kind} aria-hidden="true">{KIND_LETTER[r.kind] ?? '?'}</span>
    <span class="nm">
      <b>{r.name}</b>
      {#if r.asset}
        {@const b = scopeBadge(r.asset)}
        <Badge tone={b.tone} dashed={b.dashed} label={b.label} title={b.title} />
      {/if}
      {#if r.why}<span class="why">{r.why}</span>{/if}
    </span>
    <HostStrip {order} states={r.dots} />
  </button>
{/snippet}

{#snippet cardRow(r: InboxRow)}
  {@const c = r.card}
  {#if c}
    <button
      type="button"
      class="row card"
      class:selected={selectedKey === r.key}
      aria-current={selectedKey === r.key ? 'true' : undefined}
      data-row-key={r.key}
      data-testid={`inbox-row-${r.key}`}
      onclick={() => onselect(r.key)}
    >
      <Badge tone={c.state === 'failed' ? 'crit' : 'accent'} label={c.state === 'failed' ? `${c.kind} · failed` : c.kind} />
      <span class="nm">
        <b class="sentence">{c.summary}</b>
        {#if c.error}<span class="why" title={c.error}>{c.error}</span>{/if}
      </span>
      <span class="groups">
        {#each Object.entries(c.groups ?? {}).slice(0, 3) as [g, n] (g)}<Badge label={`${g} ${n}`} />{/each}
      </span>
    </button>
  {/if}
{/snippet}

<div class="inbox" data-testid="assets-inbox">
  {#if shown.cards.length}
    <h3 class="grp" data-testid="inbox-section-cards">{SECTION_LABEL.cards} <span class="n">{shown.cards.length}</span></h3>
    {#each shown.cards as r (r.key)}{@render cardRow(r)}{/each}
  {/if}
  {#each OPEN as s (s)}
    {#if shown[s].length}
      <h3 class="grp" data-testid={`inbox-section-${s}`}>{SECTION_LABEL[s]} <span class="n">{shown[s].length}</span></h3>
      {#each shown[s] as r (r.key)}{@render assetRow(r)}{/each}
    {/if}
  {/each}
  {#if quiet}<p class="quiet" data-testid="inbox-quiet">Nothing needs you.</p>{/if}
  {#if shown.insync.length}
    <button
      type="button"
      class="grp fold"
      aria-expanded={insyncOpen}
      onclick={() => (insyncOpen = !insyncOpen)}
      data-testid="inbox-insync-toggle"
    ><span class="tri" aria-hidden="true">{insyncOpen ? '▾' : '▸'}</span>{SECTION_LABEL.insync} <span class="n">{shown.insync.length}</span></button>
    {#if insyncOpen}{#each shown.insync as r (r.key)}{@render assetRow(r)}{/each}{/if}
  {/if}
  {#if inbox.hidden}
    <p class="note">{inbox.hidden} fleet internal{inbox.hidden === 1 ? '' : 's'} hidden — fleet's own hooks, MCP entry and skills, and harness internals.</p>
  {/if}
</div>

<style>
  .inbox { display: flex; flex-direction: column; font-size: 13px; }
  .grp {
    display: flex; align-items: center; gap: 8px; margin: 0; padding: 12px 14px 6px;
    font-size: 11px; font-weight: 600; letter-spacing: 0.06em; text-transform: uppercase; color: var(--fg-muted);
  }
  .grp .n { font-variant-numeric: tabular-nums; letter-spacing: 0; }
  .fold { border: 0; background: none; cursor: pointer; text-align: left; font: inherit; font-size: 11px; font-weight: 600; letter-spacing: 0.06em; text-transform: uppercase; color: var(--fg-muted); }
  .fold:focus-visible, .row:focus-visible { outline: var(--ring-w) solid var(--ring); outline-offset: calc(-1 * var(--ring-w)); }
  .tri { display: inline-block; width: 8px; }
  .row {
    display: grid; grid-template-columns: 18px minmax(0, 1fr) auto; gap: 10px; align-items: center;
    width: 100%; min-height: 34px; padding: 0 14px; border: 0; border-bottom: 1px solid var(--border);
    background: none; color: var(--fg); font: inherit; text-align: left; cursor: pointer;
  }
  .row.card { grid-template-columns: auto minmax(0, 1fr) auto; }
  .row:hover { background: var(--bg-pane); }
  .row.selected { background: var(--accent-soft); box-shadow: inset 2px 0 0 var(--accent); }
  .nm { display: flex; align-items: center; gap: 8px; min-width: 0; }
  .nm b { font-weight: 560; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .nm b.sentence { font-weight: 600; }
  .why { color: var(--fg-muted); font-size: 12px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .kico {
    display: grid; place-items: center; width: 16px; height: 16px; border-radius: var(--radius-sm);
    background: var(--control-bg-active); color: var(--control-fg-quiet); font-family: var(--mono); font-size: 9.5px; font-weight: 700;
  }
  .groups { display: flex; gap: 4px; }
  .quiet, .note { margin: 0; padding: 10px 14px; color: var(--fg-muted); font-size: 12px; }
</style>
```

Add `'AssetsInbox.svelte'` to `GUARDED` in `src/lib/assets_tokens.test.ts`.

- [ ] **Step 4: Run the tests**

Run: `pnpm exec vitest run src/lib/AssetsInbox.test.ts src/lib/assets_tokens.test.ts` — Expected: PASS.
Run: `pnpm check` — Expected: 0 errors.

- [ ] **Step 5: Commit**

```bash
git add src/lib/AssetsInbox.svelte src/lib/AssetsInbox.test.ts src/lib/assets_tokens.test.ts
git commit -m "feat(assets-ui): the Inbox — open cards first, then what needs you, in sync folded

Proposed cards show read-only (kind, sentence, groups, failure); then Needs
you, Drifted, Behind the catalog and New on hosts, each row one asset with
its scope badge, why, and one dot per host; In sync folds to one line."
```

---

### Task 8: The Library — `AssetList` wrapped with badges and the query, and `QueryInput` (R17, R20, R21)

**Files:**
- Create: `src/lib/QueryInput.svelte`, `src/lib/QueryInput.test.ts`
- Modify: `src/lib/AssetList.svelte`, `src/lib/AssetList.test.ts`
- Modify: `src/lib/assets_tokens.test.ts` (add `'QueryInput.svelte'`)

**Interfaces:**
- Consumes: `completions`, `applyCompletion`, `parseQuery`, `QueryVocab`, `QueryRow`, `rowOfAsset`, `rowOfIdentity`, `rowOfOrphan` (Task 6); `scopeBadge`, `keyOf`, `PERSONAL` (Task 6); `Badge` (Task 5).
- Produces:
  - `QueryInput.svelte` props `{ value?: string ($bindable); vocab: QueryVocab; placeholder?: string; testid?: string; onescape?: () => void }`, instance method `focus()`. `data-testid="assets-query"` on the input, `assets-query-completions` on the list.
  - `AssetList.svelte` new optional props: `keep?: (row: QueryRow) => boolean`, `canWrite?: (a: AssetSummary) => boolean`, `onpick?: (key: string) => void`; `selected` may carry `catalog?: string`; `onselect(kind, name, catalog)`. Every row carries `data-row-key`; asset rows key by catalog (two catalogs may share a name).

- [ ] **Step 1: Write the failing tests**

`src/lib/QueryInput.test.ts`:

```ts
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi } from 'vitest';
import QueryInput from './QueryInput.svelte';

const vocab = { hosts: ['local', 'oci'], layers: ['core'], catalogs: ['personal', 'papayapos'] };

describe('QueryInput', () => {
  it('completes a key, then its value; Tab and Enter take the highlighted one', async () => {
    render(QueryInput, { vocab });
    const input = screen.getByTestId('assets-query') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'ki' } });
    expect(screen.getByTestId('assets-query-completions').textContent).toContain('kind:');
    expect(input.getAttribute('aria-expanded')).toBe('true');
    await fireEvent.keyDown(input, { key: 'Tab' });
    expect(input.value).toBe('kind:');
    await fireEvent.input(input, { target: { value: 'kind:sk' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    expect(input.value).toBe('kind:skill');
    expect(input.getAttribute('aria-expanded')).toBe('false');
  });

  it('moves the highlight with the arrows and names it for assistive tech', async () => {
    render(QueryInput, { vocab });
    const input = screen.getByTestId('assets-query') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'catalog:p' } });
    const first = input.getAttribute('aria-activedescendant');
    await fireEvent.keyDown(input, { key: 'ArrowDown' });
    const second = input.getAttribute('aria-activedescendant');
    expect(second).not.toBe(first);
    expect(document.getElementById(second!)?.getAttribute('aria-selected')).toBe('true');
    await fireEvent.keyDown(input, { key: 'Enter' });
    expect(input.value).toBe('catalog:papayapos');
  });

  it('Esc closes the list, then clears, then hands focus back', async () => {
    const onescape = vi.fn();
    render(QueryInput, { vocab, onescape });
    const input = screen.getByTestId('assets-query') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'ho' } });
    await fireEvent.keyDown(input, { key: 'Escape' });
    expect(screen.queryByTestId('assets-query-completions')).toBeNull();
    expect(input.value).toBe('ho');
    await fireEvent.keyDown(input, { key: 'Escape' });
    expect(input.value).toBe('');
    await fireEvent.keyDown(input, { key: 'Escape' });
    expect(onescape).toHaveBeenCalledOnce();
  });
});
```

Append to `src/lib/AssetList.test.ts`:

```ts
describe('AssetList — the Library (Assets M5)', () => {
  const two: AssetListing = {
    ...baseListing(),
    assets: [
      { kind: 'skill', name: 'w', version: '1', description: 'd', tags: [], hosts: [], catalog: 'personal' },
      { kind: 'skill', name: 'w', version: '1', description: 'd', tags: [], hosts: [], catalog: 'papayapos' },
      { kind: 'skill', name: 'other', version: '1', description: 'd', tags: [], hosts: [], catalog: 'personal', scope: 'shared' },
    ],
  };

  it('lists an asset once per catalog, with its scope or catalog badge and a row key', () => {
    const { container } = render(AssetList, { listing: two, selected: null, filter: '', onselect: () => {}, onimport: vi.fn() });
    const keys = [...container.querySelectorAll('[data-row-key]')].map((r) => r.getAttribute('data-row-key'));
    expect(keys).toEqual(['asset:personal:skill/w', 'asset:papayapos:skill/w', 'asset:personal:skill/other']);
    expect(container.querySelector('[data-row-key="asset:papayapos:skill/w"]')?.textContent).toContain('papayapos');
    expect(container.querySelector('[data-row-key="asset:personal:skill/w"]')?.textContent).toContain('private');
  });

  it('keeps only the rows the query keeps', () => {
    const { container } = render(AssetList, {
      listing: two, selected: null, filter: '', onselect: () => {}, onimport: vi.fn(),
      keep: (r: { catalog?: string | null }) => r.catalog === 'papayapos',
    });
    expect([...container.querySelectorAll('[data-row-key]')].map((r) => r.getAttribute('data-row-key'))).toEqual([
      'asset:papayapos:skill/w',
    ]);
  });

  it('shows an asset this window cannot write as managed and static, still selectable', async () => {
    const onselect = vi.fn();
    render(AssetList, {
      listing: two, selected: null, filter: '', onselect, onimport: vi.fn(),
      canWrite: (a: { catalog?: string }) => a.catalog !== 'papayapos',
    });
    const row = document.querySelector('[data-row-key="asset:papayapos:skill/w"]') as HTMLElement;
    expect(row.tagName).toBe('DIV');
    expect(row.textContent).toContain('managed');
    await fireEvent.click(row);
    expect(onselect).toHaveBeenCalledWith('skill', 'w', 'papayapos');
  });
});
```

(Add `fireEvent` to that file's `@testing-library/svelte` import.)

- [ ] **Step 2: Run them to verify they fail**

Run: `pnpm exec vitest run src/lib/QueryInput.test.ts src/lib/AssetList.test.ts`
Expected: FAIL — `QueryInput.svelte` missing; AssetList renders no `data-row-key`, keys the two `w`s alike (Svelte's duplicate-key error), ignores `keep`/`canWrite`.

- [ ] **Step 3: Write `QueryInput.svelte`**

```svelte
<script lang="ts">
  import Badge from './Badge.svelte';
  import { applyCompletion, completions, parseQuery, type QueryVocab } from './assets_query';

  /** The workspace's token filter (spec, Query; Rulings R21): `/` focuses
   *  it, typing completes keys and values, Tab or Enter takes the
   *  highlighted one, Esc closes the list, then clears, then leaves. */
  let {
    value = $bindable(''),
    vocab,
    placeholder = 'Filter · host: kind: state: layer: catalog: scope:',
    testid = 'assets-query',
    onescape,
  }: {
    value?: string;
    vocab: QueryVocab;
    placeholder?: string;
    testid?: string;
    /** Esc on an empty field with no list: give focus back to the list. */
    onescape?: () => void;
  } = $props();

  let input: HTMLInputElement | undefined = $state();
  let open = $state(false);
  let active = $state(0);
  const listId = `query-${Math.random().toString(36).slice(2, 9)}`;
  const options = $derived(open ? completions(value, vocab) : []);
  const tokens = $derived(parseQuery(value).tokens);

  export function focus() {
    input?.focus();
    input?.select();
  }

  function take(i: number): boolean {
    const c = options[i];
    if (!c) return false;
    value = applyCompletion(value, c);
    active = 0;
    // A key completion leaves the list open for that key's values.
    open = c.endsWith(':');
    return true;
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'ArrowDown' && options.length) {
      e.preventDefault();
      active = (active + 1) % options.length;
    } else if (e.key === 'ArrowUp' && options.length) {
      e.preventDefault();
      active = (active + options.length - 1) % options.length;
    } else if ((e.key === 'Tab' || e.key === 'Enter') && options.length) {
      if (take(active)) e.preventDefault();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      if (options.length) open = false;
      else if (value) value = '';
      else onescape?.();
    }
  }
</script>

<div class="query" role="search">
  <input
    bind:this={input}
    bind:value
    class="field"
    type="text"
    role="combobox"
    aria-label="Filter assets"
    aria-autocomplete="list"
    aria-expanded={options.length > 0}
    aria-controls={listId}
    aria-activedescendant={options.length ? `${listId}-${active}` : undefined}
    autocomplete="off"
    spellcheck="false"
    {placeholder}
    data-testid={testid}
    oninput={() => {
      open = true;
      active = 0;
    }}
    onblur={() => (open = false)}
    {onkeydown}
  />
  {#if tokens.length}
    <span class="tokens" aria-hidden="true">
      {#each tokens as t, i (i)}<Badge tone="accent" mono label={`${t.key}:${t.values.join(',')}`} />{/each}
    </span>
  {/if}
  {#if options.length}
    <ul class="list" id={listId} role="listbox" aria-label="Completions" data-testid={`${testid}-completions`}>
      {#each options as o, i (o)}
        <li
          id={`${listId}-${i}`}
          role="option"
          tabindex="-1"
          aria-selected={i === active}
          class:active={i === active}
          onmousedown={(e) => {
            e.preventDefault();
            take(i);
          }}
        >{o}</li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .query {
    position: relative; display: flex; align-items: center; gap: 6px; flex: 1; max-width: 520px;
    height: var(--control-h-lg); padding: 0 8px; border: 1px solid var(--control-border);
    border-radius: var(--radius-md); background: var(--control-bg);
  }
  .query:focus-within { outline: var(--ring-w) solid var(--ring); outline-offset: var(--ring-offset); }
  .field { flex: 1; min-width: 80px; border: 0; outline: 0; background: none; color: var(--fg); font: inherit; font-size: var(--control-font); }
  .tokens { display: flex; gap: 4px; }
  .list {
    position: absolute; top: calc(100% + 4px); left: 0; z-index: 5; min-width: 220px; margin: 0; padding: 4px 0;
    list-style: none; border: 1px solid var(--control-border); border-radius: var(--radius-md); background: var(--bg);
  }
  .list li { padding: 3px 10px; font-family: var(--mono); font-size: 11.5px; cursor: pointer; }
  .list li.active { background: var(--accent-soft); }
</style>
```

- [ ] **Step 4: Wrap `AssetList` for the Library**

Script additions (keep every existing prop):

```ts
  import Badge from './Badge.svelte';
  import { keyOf, PERSONAL, scopeBadge } from './assets_workspace';
  import { rowOfAsset, rowOfIdentity, rowOfOrphan, type QueryRow } from './assets_query';
  import type { AssetSummary } from './assets';
```

the prop list gains

```ts
    /** Assets M5 (R21): the query's tokens as a row predicate; `filter`
     *  stays the free text. */
    keep?: (row: QueryRow) => boolean;
    /** R20: false = this window may only look at the asset: a static row
     *  with a `managed` badge. */
    canWrite?: (a: AssetSummary) => boolean;
    /** An identity or orphan row was picked (the Inspector shows it). */
    onpick?: (key: string) => void;
```

`selected`'s type becomes `{ kind: string; name: string; catalog?: string } | null` and `onselect`'s `(kind: string, name: string, catalog?: string) => void`. Derivations:

```ts
  const catalogOf = (a: AssetSummary) => a.catalog ?? PERSONAL;
  const managed = (a: AssetSummary) => canWrite?.(a) === false;
  const keeps = (row: QueryRow) => keep?.(row) ?? true;
  const groups = $derived(
    groupByKind(listing).map((g) => ({
      ...g,
      assets: g.assets.filter(
        (a) =>
          (filter === '' || a.name.includes(filter) || a.description.toLowerCase().includes(filter.toLowerCase())) &&
          keeps({ ...rowOfAsset(a), managedElsewhere: managed(a) }),
      ),
    })).filter((g) => g.assets.length > 0),
  );
  const ids = $derived(
    identitiesOf(listing).filter((i) => (filter === '' || i.name.toLowerCase().includes(filter.toLowerCase())) && keeps(rowOfIdentity(i))),
  );
  const orphans = $derived(
    listing.unmanaged.filter(
      (r) => r.state === 'orphan' && (filter === '' || r.name.toLowerCase().includes(filter.toLowerCase())) && keeps(rowOfOrphan([r])),
    ),
  );
  const isSel = (a: AssetSummary) =>
    selected?.kind === a.kind && selected?.name === a.name && (selected?.catalog ?? PERSONAL) === catalogOf(a);
  const assetKey = (a: AssetSummary) => keyOf({ type: 'asset', catalog: catalogOf(a), kind: a.kind, name: a.name });
  function pickStatic(e: KeyboardEvent, a: AssetSummary) {
    if (e.key !== 'Enter' && e.key !== ' ') return;
    e.preventDefault();
    onselect(a.kind, a.name, catalogOf(a));
  }
```

(`ids`, `orphans` and `isSel` replace their previous definitions.) The catalog-asset rows become:

```svelte
    {#each g.assets as a (`${catalogOf(a)}:${a.name}`)}
      {@const c = stateCounts(a.hosts)}
      {@const b = scopeBadge(a)}
      {#snippet chips()}…unchanged from Task 5…{/snippet}
      {#if readonly || managed(a)}
        <div
          class="row static"
          class:selected={isSel(a)}
          role="button"
          tabindex="-1"
          title={hostsTitle(a.hosts)}
          data-row-key={assetKey(a)}
          data-testid={`asset-row-${a.kind}-${a.name}`}
          onclick={() => onselect(a.kind, a.name, catalogOf(a))}
          onkeydown={(e) => pickStatic(e, a)}
        >
          <span class="name">{a.name}</span>
          <Badge tone={b.tone} dashed={b.dashed} label={b.label} title={b.title} />
          {#if managed(a)}<Badge tone="muted" label="managed" title="This window can only look at it" />{/if}
          {#if a.version}<span class="meta">{a.version}</span>{/if}
          {@render chips()}
        </div>
      {:else}
        <button
          class="row"
          class:selected={isSel(a)}
          aria-current={isSel(a) ? 'true' : undefined}
          onclick={() => onselect(a.kind, a.name, catalogOf(a))}
          data-row-key={assetKey(a)}
          data-testid={`asset-row-${a.kind}-${a.name}`}
        >
          <span class="name">{a.name}</span>
          <Badge tone={b.tone} dashed={b.dashed} label={b.label} title={b.title} />
          {@render chips()}
        </button>
      {/if}
    {/each}
```

The identity row's `<div class="row unmanaged" …>` gains `data-row-key={keyOf({ type: 'identity', kind: i.kind, name: i.name })}`, `role="button"`, `tabindex="-1"`, `onclick={() => onpick?.(keyOf({ type: 'identity', kind: i.kind, name: i.name }))}` and an `onkeydown` doing the same on Enter/Space; its Import link gets `onclick={(e) => { e.stopPropagation(); onimport(i); }}`. The orphan row gains `data-row-key={keyOf({ type: 'orphan', kind: r.kind, name: r.name })}` and the same pick handlers. Style: `.row.static { cursor: pointer; }` and `.row:focus-visible { outline: var(--ring-w) solid var(--ring); outline-offset: calc(-1 * var(--ring-w)); }`.

Add `'QueryInput.svelte'` to `GUARDED` in `assets_tokens.test.ts`.

- [ ] **Step 5: Run the tests**

Run: `pnpm exec vitest run src/lib/QueryInput.test.ts src/lib/AssetList.test.ts src/lib/assets_tokens.test.ts src/lib/AssetsPanel.test.ts src/lib/hub_disabled.test.ts`
Expected: PASS (the panel still renders `AssetList` the old way until Task 11; the read-only overview's row is still a `DIV`).
Run: `pnpm check` — Expected: 0 errors.

- [ ] **Step 6: Commit**

```bash
git add src/lib/QueryInput.svelte src/lib/QueryInput.test.ts src/lib/AssetList.svelte src/lib/AssetList.test.ts src/lib/assets_tokens.test.ts
git commit -m "feat(assets-ui): the Library — every asset once per catalog, badges, the token query

AssetList keys rows by catalog, shows each asset's scope or catalog badge,
filters by the query's tokens, and shows assets this window cannot write as
managed. QueryInput completes host:, kind:, state:, layer:, catalog: and
scope: tokens as a combobox."
```

---

### Task 9: The Inspector — a tabbed pane over `AssetDetail`, `AssetEditor`, History and summaries (R19)

**Files:**
- Create: `src/lib/Inspector.svelte`, `src/lib/AssetInspector.svelte`, `src/lib/AssetInspector.test.ts`
- Modify: `src/lib/AssetDetail.svelte` (`section`, `onsection` props), `src/lib/AssetDetail.test.ts`
- Modify: `src/lib/assets_tokens.test.ts` (add `'Inspector.svelte'`, `'AssetInspector.svelte'`)

**Interfaces:**
- Consumes: `Selection`, `parseKey`, `assetHistory`, `CommitEntry`, `ChangesetSummary`, `scopeBadge`, `ago` (Task 6); `HostStrip`, `Badge` (Task 5); `AssetDetail`, `AssetEditor` (existing); `identitiesOf` (`assets.ts`).
- Produces:
  - `Inspector.svelte` props `{ eyebrow?: string; title: string; tabs: readonly { id: string; label: string }[]; active: string; onchange: (id: string) => void; children: Snippet; testid?: string }`; tabs `data-testid="inspector-tab-<id>"`.
  - `AssetDetail.svelte` props gain `section?: 'all' | 'overview' | 'hosts' | 'source'` (default `'all'`) and `onsection?: (s: 'source') => void`.
  - `AssetInspector.svelte` props `{ selectedKey: string | null; listing: AssetListing | null; cards: ChangesetSummary[] | null; hosts: HostRow[]; order: string[]; readOnly: boolean; canOpen: (a: AssetSummary) => boolean; autoEditKey: string; editNonce: number; onsync: (f: { hostAlias?: string; kind?: string; name?: string }) => void; ondeleted: () => void; onimport: (id: AssetIdentity) => void }`.

- [ ] **Step 1: Write the failing tests**

Append to `src/lib/AssetDetail.test.ts`:

```ts
describe('AssetDetail sections (Assets M5)', () => {
  const detail = {
    asset: { kind: 'skill', name: 'w', version: '1', description: 'Make one.', tags: [], body: '# b' },
    previews: [{ harness: 'claude', plan: { files: [{ path: '~/.claude/skills/w/SKILL.md', bytes: 'x' }], merges: [], placeholders: [], warnings: [] }, unsupported: null }],
    hosts: [{ host_alias: 'local', harness: 'claude', state: 'in_sync' }],
  };

  it('shows only the section it is asked for', async () => {
    byCmd({ catalog_get_asset: detail });
    const { rerender } = render(AssetDetail, { kind: 'skill', name: 'w', hosts: [], section: 'overview' });
    expect(await screen.findByTestId('asset-detail-title')).toBeTruthy();
    expect(screen.queryByTestId('preview-file-path')).toBeNull();
    await rerender({ kind: 'skill', name: 'w', hosts: [], section: 'source' });
    expect(screen.queryByTestId('asset-detail-title')).toBeNull();
    expect(screen.getByTestId('preview-file-path').textContent).toContain('SKILL.md');
    expect(invoke.mock.calls.filter((c) => c[0] === 'catalog_get_asset')).toHaveLength(1);
  });

  it('Edit asks for the Source section', async () => {
    byCmd({ catalog_get_asset: detail });
    const onsection = vi.fn();
    render(AssetDetail, { kind: 'skill', name: 'w', hosts: [], section: 'overview', onsection });
    (await screen.findByTestId('asset-edit')).click();
    expect(onsection).toHaveBeenCalledWith('source');
  });
});
```

`src/lib/AssetInspector.test.ts`:

```ts
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import AssetInspector from './AssetInspector.svelte';
import type { AssetListing } from './assets';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;
const listing: AssetListing = {
  head: 'h', loaded_at: 1, problems: [], unmanaged: [],
  assets: [
    { kind: 'skill', name: 'w', version: '1', description: 'Make one.', tags: [], catalog: 'personal',
      hosts: [{ host_alias: 'oci', harness: 'claude', state: 'drifted', drift_side: 'host' }] },
    { kind: 'skill', name: 'ppt', version: '2', description: 'Org skill.', tags: [], catalog: 'papayapos',
      hosts: [{ host_alias: 'trn', harness: 'claude', state: 'in_sync' }] },
  ],
  identities: [
    { kind: 'skill', name: 'fresh', hosts: [{ host_alias: 'oci', harness: 'claude', host_hash: 'abcdef1234' }], signature: 'oci', variants: 1, class: 'normal', reason: null },
  ],
};
const base = {
  listing, cards: [{ id: 7, kind: 'new' as const, summary: 'New on oci: skill/fresh → core', state: 'proposed' as const, created_at: 1, groups: { core: 1 } }],
  hosts: [], order: ['oci', 'trn'], readOnly: false, canOpen: (a: { catalog?: string }) => (a.catalog ?? 'personal') === 'personal',
  autoEditKey: '', editNonce: 0, onsync: vi.fn(), ondeleted: vi.fn(), onimport: vi.fn(),
};
beforeEach(() => {
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === 'catalog_get_asset') return { asset: { kind: 'skill', name: 'w', version: '1', description: 'Make one.', tags: [], body: '# b' }, previews: [], hosts: [] };
    if (cmd === 'catalog_asset_history') return [{ sha: 'abcdef1234567', at: 1, author: 'Martin', subject: 'edit w' }];
    throw { code: 'E_TEST', message: cmd };
  });
});

describe('AssetInspector', () => {
  it('says what to select when nothing is', () => {
    render(AssetInspector, { ...base, selectedKey: null });
    expect(screen.getByText('Select an asset.')).toBeTruthy();
  });

  it('a personal asset: Overview, Source, Hosts, History over one AssetDetail', async () => {
    render(AssetInspector, { ...base, selectedKey: 'asset:personal:skill/w' });
    expect([...document.querySelectorAll('[role="tab"]')].map((t) => t.textContent)).toEqual(['Overview', 'Source', 'Hosts', 'History']);
    expect(await screen.findByTestId('asset-detail-title')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('inspector-tab-history'));
    await waitFor(() => expect(screen.getByTestId('inspector-history').textContent).toContain('edit w'));
    expect(invoke).toHaveBeenCalledWith('catalog_asset_history', { args: { kind: 'skill', name: 'w', catalog: null } });
    expect(screen.getByTestId('inspector-history').textContent).toContain('abcdef1');
    expect(invoke.mock.calls.filter((c) => c[0] === 'catalog_get_asset')).toHaveLength(1);
  });

  it('Left/Right move between tabs', async () => {
    render(AssetInspector, { ...base, selectedKey: 'asset:personal:skill/w' });
    const overview = screen.getByTestId('inspector-tab-overview');
    await fireEvent.keyDown(overview, { key: 'ArrowRight' });
    expect(screen.getByTestId('inspector-tab-source').getAttribute('aria-selected')).toBe('true');
    await fireEvent.keyDown(screen.getByTestId('inspector-tab-source'), { key: 'ArrowLeft' });
    expect(overview.getAttribute('aria-selected')).toBe('true');
  });

  it('an asset this window cannot open: a summary, never a read of the asset', async () => {
    render(AssetInspector, { ...base, selectedKey: 'asset:papayapos:skill/ppt' });
    expect([...document.querySelectorAll('[role="tab"]')].map((t) => t.textContent)).toEqual(['Overview', 'Hosts', 'History']);
    expect(screen.getByTestId('inspector-summary').textContent).toContain('Org skill.');
    await fireEvent.click(screen.getByTestId('inspector-tab-hosts'));
    expect(screen.getByTestId('inspector-hosts').textContent).toContain('trn: in sync');
    expect(invoke.mock.calls.some((c) => c[0] === 'catalog_get_asset')).toBe(false);
  });

  it('read-only: no History, and Hosts says which side moved', async () => {
    render(AssetInspector, { ...base, readOnly: true, selectedKey: 'asset:personal:skill/w' });
    expect([...document.querySelectorAll('[role="tab"]')].map((t) => t.textContent)).toEqual(['Overview', 'Hosts']);
    await fireEvent.click(screen.getByTestId('inspector-tab-hosts'));
    expect(screen.getByTestId('inspector-hosts').textContent).toContain('oci: drifted — edited on the host');
  });

  it('an unmanaged identity: where it is, and Import', async () => {
    const onimport = vi.fn();
    render(AssetInspector, { ...base, onimport, selectedKey: 'identity:skill/fresh' });
    expect(screen.getByTestId('inspector-summary').textContent).toContain('oci');
    await fireEvent.click(screen.getByTestId('inspector-import'));
    expect(onimport).toHaveBeenCalledWith(expect.objectContaining({ name: 'fresh' }));
  });

  it('a card: its sentence and groups, read only', () => {
    render(AssetInspector, { ...base, selectedKey: 'card:7' });
    const s = screen.getByTestId('inspector-summary');
    expect(s.textContent).toContain('New on oci: skill/fresh → core');
    expect(s.textContent).toContain('core');
    expect(s.querySelectorAll('button')).toHaveLength(0);
  });

  it('opens a just-created asset straight into Source, editing', async () => {
    render(AssetInspector, { ...base, selectedKey: 'asset:personal:skill/w', autoEditKey: 'asset:personal:skill/w' });
    expect(screen.getByTestId('inspector-tab-source').getAttribute('aria-selected')).toBe('true');
    expect(await screen.findByTestId('editor-save')).toBeTruthy();
  });
});
```

- [ ] **Step 2: Run them to verify they fail**

Run: `pnpm exec vitest run src/lib/AssetDetail.test.ts src/lib/AssetInspector.test.ts`
Expected: FAIL — `AssetInspector.svelte` missing; `AssetDetail` ignores `section`.

- [ ] **Step 3: Give `AssetDetail` sections**

Props gain:

```ts
    /** Assets M5 (R19): which part the Inspector shows — `overview` (title,
     *  actions, lint, description, tags), `hosts` (the host × harness
     *  matrix), `source` (the editor or the rendered preview). `all` (the
     *  default) is the whole detail, as before. Switching it never refetches. */
    section = 'all',
    /** Edit lives in Source: ask the Inspector to show it. */
    onsection,
```

with types `section?: 'all' | 'overview' | 'hosts' | 'source'; onsection?: (s: 'source') => void;`, and in the script `const show = (s: 'overview' | 'hosts' | 'source') => section === 'all' || section === s;`. In the markup, wrap the title row, the lint report, description, install-as and tags in `{#if show('overview')} … {/if}`; the `<h4>Hosts</h4>` and the matrix in `{#if show('hosts')} … {/if}`; the `{#if editing} … {:else} … {/if}` editor/preview block in `{#if show('source')} … {/if}`. The Edit button becomes `onclick={() => { editing = true; onsection?.('source'); }}`.

- [ ] **Step 4: Write `Inspector.svelte` and `AssetInspector.svelte`**

`src/lib/Inspector.svelte`:

```svelte
<script lang="ts">
  import type { Snippet } from 'svelte';

  /** A pane, not a modal (spec, Inspector): a title, tabs, and what the
   *  active tab shows. Left/Right move between tabs. Shared by the asset
   *  Inspector and, in M6, Layers and Hosts. */
  let {
    eyebrow,
    title,
    tabs,
    active,
    onchange,
    children,
    testid = 'inspector',
  }: {
    eyebrow?: string;
    title: string;
    tabs: readonly { id: string; label: string }[];
    active: string;
    onchange: (id: string) => void;
    children: Snippet;
    testid?: string;
  } = $props();

  let tablist: HTMLElement | undefined = $state();

  function onkeydown(e: KeyboardEvent) {
    if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
    e.preventDefault();
    const i = tabs.findIndex((t) => t.id === active);
    const next = tabs[(i + (e.key === 'ArrowRight' ? 1 : tabs.length - 1)) % tabs.length];
    onchange(next.id);
    tablist?.querySelector<HTMLElement>(`[data-tab="${next.id}"]`)?.focus();
  }
</script>

<section class="insp" aria-label={title} data-testid={testid}>
  <header class="ih">
    {#if eyebrow}<span class="eyebrow">{eyebrow}</span>{/if}
    <h2 class="ititle">{title}</h2>
  </header>
  <div class="itabs" role="tablist" aria-label={`${title} views`} bind:this={tablist}>
    {#each tabs as t (t.id)}
      <button
        type="button"
        role="tab"
        id={`${testid}-tab-${t.id}`}
        data-tab={t.id}
        aria-selected={t.id === active}
        aria-controls={`${testid}-panel`}
        tabindex={t.id === active ? 0 : -1}
        class:on={t.id === active}
        onclick={() => onchange(t.id)}
        {onkeydown}
        data-testid={`inspector-tab-${t.id}`}
      >{t.label}</button>
    {/each}
  </div>
  <div class="ib" role="tabpanel" id={`${testid}-panel`} aria-labelledby={`${testid}-tab-${active}`}>
    {@render children()}
  </div>
</section>

<style>
  .insp { display: flex; flex-direction: column; min-height: 0; height: 100%; background: var(--bg); }
  .ih { display: grid; gap: 4px; padding: 12px 14px 0; }
  .eyebrow { font-size: 10.5px; font-weight: 600; letter-spacing: 0.07em; text-transform: uppercase; color: var(--fg-muted); }
  .ititle { margin: 0; font-size: 15px; font-weight: 650; letter-spacing: -0.01em; overflow-wrap: anywhere; }
  .itabs { display: flex; gap: 14px; margin-top: 8px; padding: 0 14px; border-bottom: 1px solid var(--border); }
  .itabs button {
    margin-bottom: -1px; padding: 7px 0; border: 0; border-bottom: 2px solid transparent; background: none;
    color: var(--fg-muted); font: inherit; font-size: 12px; cursor: pointer;
  }
  .itabs button.on { color: var(--fg); border-bottom-color: var(--accent); font-weight: 600; }
  .itabs button:focus-visible { outline: var(--ring-w) solid var(--ring); outline-offset: var(--ring-offset); }
  .ib { flex: 1; min-height: 0; overflow: auto; }
</style>
```

`src/lib/AssetInspector.svelte`:

```svelte
<script lang="ts">
  import { untrack } from 'svelte';
  import Inspector from './Inspector.svelte';
  import AssetDetail from './AssetDetail.svelte';
  import Badge from './Badge.svelte';
  import HostStrip from './HostStrip.svelte';
  import { identitiesOf, type AssetIdentity, type AssetListing, type AssetSummary } from './assets';
  import type { HostRow } from './hosts';
  import { ago, assetHistory, parseKey, scopeBadge, type ChangesetSummary, type CommitEntry } from './assets_workspace';

  /** What the Inspector shows for the selected row (Rulings R19). */
  let {
    selectedKey,
    listing,
    cards,
    hosts,
    order,
    readOnly,
    canOpen,
    autoEditKey,
    editNonce,
    onsync,
    ondeleted,
    onimport,
  }: {
    selectedKey: string | null;
    listing: AssetListing | null;
    cards: ChangesetSummary[] | null;
    hosts: HostRow[];
    order: string[];
    readOnly: boolean;
    /** A personal asset this window may write: the full detail and editor. */
    canOpen: (a: AssetSummary) => boolean;
    /** A just-created asset's key: open it in Source, editing. */
    autoEditKey: string;
    /** Bumped by the `e` key: open the selected asset in Source, editing. */
    editNonce: number;
    onsync: (f: { hostAlias?: string; kind?: string; name?: string }) => void;
    ondeleted: () => void;
    onimport: (id: AssetIdentity) => void;
  } = $props();

  type Tab = 'overview' | 'source' | 'hosts' | 'history';
  const LABEL: Record<Tab, string> = { overview: 'Overview', source: 'Source', hosts: 'Hosts', history: 'History' };

  const sel = $derived(selectedKey ? parseKey(selectedKey) : null);
  const asset = $derived.by((): AssetSummary | null => {
    if (sel?.type !== 'asset') return null;
    const found = listing?.assets.find(
      (a) => (a.catalog ?? 'personal') === sel.catalog && a.kind === sel.kind && a.name === sel.name,
    );
    if (found) return found;
    // Just created (the listing has not caught up) or chosen from Lint all:
    // a personal asset this window may write opens by name.
    return !readOnly && sel.catalog === 'personal'
      ? { kind: sel.kind, name: sel.name, version: '', description: '', tags: [], hosts: [], catalog: 'personal' }
      : null;
  });
  const identity = $derived(
    sel?.type === 'identity' && listing ? (identitiesOf(listing).find((i) => i.kind === sel.kind && i.name === sel.name) ?? null) : null,
  );
  const orphanRows = $derived(
    sel?.type === 'orphan' && listing ? listing.unmanaged.filter((r) => r.state === 'orphan' && r.kind === sel.kind && r.name === sel.name) : [],
  );
  const card = $derived(sel?.type === 'card' ? ((cards ?? []).find((c) => c.id === sel.id) ?? null) : null);
  const full = $derived(!!asset && !readOnly && canOpen(asset));
  const tabs = $derived.by((): Tab[] => {
    if (asset) return full ? ['overview', 'source', 'hosts', 'history'] : readOnly ? ['overview', 'hosts'] : ['overview', 'hosts', 'history'];
    if (identity) return ['overview', 'hosts'];
    return ['overview'];
  });

  let tab = $state<Tab>('overview');
  // Open in Source, editing: a just-created asset, or `e` (a new nonce).
  let editing = $state(false);
  let seenNonce = untrack(() => editNonce);
  // A new selection, or `e`, decides the tab once; the person's clicks
  // after that are theirs.
  $effect(() => {
    const key = selectedKey;
    const nonce = editNonce;
    untrack(() => {
      const edit = (key !== null && key === autoEditKey) || nonce !== seenNonce;
      seenNonce = nonce;
      editing = edit && full;
      tab = editing ? 'source' : 'overview';
    });
  });

  let history = $state<{ key: string; rows: CommitEntry[] | null; error: string | null } | null>(null);
  $effect(() => {
    if (tab !== 'history' || !asset || !selectedKey) return;
    const key = selectedKey;
    if (untrack(() => history?.key) === key) return;
    history = { key, rows: null, error: null };
    void assetHistory(asset.kind, asset.name, asset.catalog).then((r) => {
      if (history?.key !== key) return;
      history = r.ok
        ? { key, rows: r.value, error: null }
        : { key, rows: null, error: r.error.code === 'E_INVALID' ? 'This hub keeps no asset history yet; update the hub.' : r.error.message };
    });
  });

  const now = Math.floor(Date.now() / 1000);
  const sideWords = (side?: string | null) => (side === 'host' ? ' — edited on the host' : side === 'catalog' ? ' — behind the catalog' : '');
  const title = $derived(asset?.name ?? identity?.name ?? orphanRows[0]?.name ?? card?.summary ?? '');
  const eyebrow = $derived(
    asset ? `${asset.kind.replace('_', ' ')} · catalog ${asset.catalog ?? 'personal'}` : identity ? `${identity.kind.replace('_', ' ')} · on hosts, not in a catalog` : orphanRows.length ? `${orphanRows[0].kind} · orphan` : card ? `${card.kind} card · ${card.state}` : '',
  );
</script>

{#if !sel || (!asset && !identity && !orphanRows.length && !card)}
  <p class="empty" data-testid="inspector-empty">Select an asset.</p>
{:else}
  <Inspector {eyebrow} {title} tabs={tabs.map((t) => ({ id: t, label: LABEL[t] }))} active={tab} onchange={(id) => (tab = id as Tab)}>
    {#if asset && full}
      <!-- One instance for Overview, Source and Hosts — kept mounted (hidden)
           under History too, so going back never refetches or loses an edit. -->
      {#key `${selectedKey}::${editNonce}`}
        <div class="detail" hidden={tab === 'history'}>
          <AssetDetail
            kind={asset.kind}
            name={asset.name}
            {hosts}
            section={tab === 'source' ? 'source' : tab === 'hosts' ? 'hosts' : 'overview'}
            onsection={() => (tab = 'source')}
            {onsync}
            {ondeleted}
            startInEdit={editing}
          />
        </div>
      {/key}
    {/if}
    {#if asset && tab === 'history'}
      <div class="pad" data-testid="inspector-history">
        {#if !history || (history.rows === null && history.error === null)}<p class="muted">Loading…</p>
        {:else if history.error}<p class="error">{history.error}</p>
        {:else if history.rows && history.rows.length === 0}<p class="muted">No commits touch this asset yet.</p>
        {:else if history.rows}
          <ol class="commits">
            {#each history.rows as c (c.sha)}
              <li><span class="sha">{c.sha.slice(0, 7)}</span> <span class="subj">{c.subject}</span> <span class="muted">{c.author} · {ago(c.at, now)}</span></li>
            {/each}
          </ol>
        {/if}
      </div>
    {:else if asset && !full && tab === 'hosts'}
      <ul class="pad hosts" data-testid="inspector-hosts">
        {#each asset.hosts as h (`${h.host_alias}:${h.harness}`)}<li>{h.host_alias}: {h.state.replace('_', ' ')}{sideWords(h.drift_side)}{h.harness === 'claude' ? '' : ` (${h.harness})`}</li>{/each}
        {#if asset.hosts.length === 0}<li class="muted">Not scanned on any host.</li>{/if}
      </ul>
    {:else if asset && !full}
      {@const b = scopeBadge(asset)}
      <div class="pad" data-testid="inspector-summary">
        <p>{asset.description}</p>
        <dl class="kv">
          <dt>Scope</dt><dd><Badge tone={b.tone} dashed={b.dashed} label={b.label} title={b.title} /></dd>
          <dt>Version</dt><dd class="mono">{asset.version}</dd>
          <dt>Hosts</dt><dd><HostStrip {order} present={[...new Set(asset.hosts.map((h) => h.host_alias))]} /></dd>
        </dl>
        <p class="muted">
          {readOnly
            ? 'Read-only: this window has no grant on this catalog.'
            : `Opening assets of catalog ${asset.catalog} here is not supported yet; its History is.`}
        </p>
      </div>
    {:else if identity && tab === 'hosts'}
      <ul class="pad hosts" data-testid="inspector-hosts">
        {#each identity.hosts as h (`${h.host_alias}:${h.harness}`)}<li>{h.host_alias} ({h.harness}) <span class="mono">{h.host_hash?.slice(0, 7) ?? '—'}</span></li>{/each}
      </ul>
    {:else if identity}
      <div class="pad" data-testid="inspector-summary">
        <dl class="kv">
          <dt>Found on</dt><dd>{[...new Set(identity.hosts.map((h) => h.host_alias))].join(', ')}</dd>
          <dt>Copies</dt><dd>{identity.variants > 1 ? `${identity.variants} different` : 'identical'}</dd>
          {#if identity.reason}<dt>Needs</dt><dd>{identity.reason}</dd>{/if}
        </dl>
        {#if !readOnly}
          <button class="btn btn--quiet is-bounded" onclick={() => onimport(identity)} data-testid="inspector-import">Import…</button>
        {/if}
      </div>
    {:else if orphanRows.length}
      <div class="pad" data-testid="inspector-summary">
        <p>Fleet put it on {orphanRows.map((r) => r.host_alias).join(', ')}; the catalog no longer has it. The next sync of those hosts removes it, with a backup.</p>
      </div>
    {:else if card}
      <div class="pad" data-testid="inspector-summary">
        <p class="sentence">{card.summary}</p>
        <dl class="kv">
          <dt>State</dt><dd>{card.state}</dd>
          <dt>Proposed</dt><dd>{ago(card.created_at, now)}</dd>
          {#if card.error}<dt>Error</dt><dd class="error">{card.error}</dd>{/if}
        </dl>
        <ul class="groups">{#each Object.entries(card.groups ?? {}) as [g, n] (g)}<li><Badge label={`${g} ${n}`} /></li>{/each}</ul>
      </div>
    {/if}
  </Inspector>
{/if}

<style>
  .empty { padding: 14px; color: var(--fg-muted); }
  .pad { margin: 0; padding: 12px 14px; font-size: 13px; }
  .hosts { list-style: none; display: grid; gap: 4px; }
  .kv { display: grid; grid-template-columns: 92px 1fr; gap: 5px 10px; margin: 8px 0; font-size: 12px; }
  .kv dt { color: var(--fg-muted); }
  .kv dd { margin: 0; }
  .commits { margin: 0; padding: 0 0 0 18px; display: grid; gap: 4px; font-size: 12px; }
  .sha, .mono { font-family: var(--mono); font-size: 11.5px; }
  .groups { list-style: none; display: flex; gap: 4px; flex-wrap: wrap; margin: 0; padding: 0; }
  .sentence { font-weight: 600; }
  .muted { color: var(--fg-muted); }
  .error { color: var(--usage-crit); }
</style>
```

Add `'Inspector.svelte'` and `'AssetInspector.svelte'` to `GUARDED`.

- [ ] **Step 5: Run the tests**

Run: `pnpm exec vitest run src/lib/AssetDetail.test.ts src/lib/AssetInspector.test.ts src/lib/assets_tokens.test.ts src/lib/AssetsPanel.test.ts`
Expected: PASS (`AssetsPanel` still uses `AssetDetail` with the default `section="all"`).
Run: `pnpm check` — Expected: 0 errors.

- [ ] **Step 6: Commit**

```bash
git add src/lib/Inspector.svelte src/lib/AssetInspector.svelte src/lib/AssetInspector.test.ts \
  src/lib/AssetDetail.svelte src/lib/AssetDetail.test.ts src/lib/assets_tokens.test.ts
git commit -m "feat(assets-ui): the Inspector — a tabbed pane over the detail, the editor and History

One AssetDetail instance serves Overview, Source and Hosts (a section prop,
no refetch per tab); History lists the commits that touched the asset;
assets this window cannot open, unmanaged identities, orphans and cards get
summaries. Left/Right move between tabs."
```

---

### Task 10: The footer — catalog chips, `auto`, `JobChip`, and the last sync (carry 6 UI; R10, R24, R25)

**Files:**
- Create: `src/lib/JobChip.svelte`, `src/lib/JobChip.test.ts`, `src/lib/CatalogChip.svelte`, `src/lib/CatalogChip.test.ts`, `src/lib/AssetsFooter.svelte`, `src/lib/AssetsFooter.test.ts`
- Modify: `src/lib/assets_tokens.test.ts` (add the three files)

**Interfaces:**
- Consumes: `catalogStatuses`, `repoStatusOf`, `summarizeRun`, `PERSONAL` (Task 6); `repoStatusStore`, `lastSyncRun`, `syncProgress`, `catalogConfig`, `RepoStatus`, `AssetListing` (`assets.ts`); `fleetSettings`, `settingBool`, `SETTING_KEYS` (`fleet_settings.ts`); `Badge` (Task 5).
- Produces:
  - `JobChip.svelte` props `{ label: string; done?: number | null; total?: number | null; testid?: string }`.
  - `CatalogChip.svelte` props `{ name: string; head: string | null; state?: 'loaded' | 'problem' | 'not_loaded'; problem?: string | null; repo?: RepoStatus | null; writable?: boolean; busy?: boolean; onpull?: () => void; oncommit?: () => void; onpush?: () => void }` — testids `catalog-chip-<name>`; for `personal`: `assets-head` (the chip's text), `assets-repo-status`, `assets-pull`, `assets-commit-pending`, `assets-push` (in its popover).
  - `AssetsFooter.svelte` props `{ readOnly: boolean; listing: AssetListing | null; busy: string; onpull: () => void; oncommit: () => void; onpush: () => void }` — `assets-auto`, `assets-job`, `assets-last-sync`.

- [ ] **Step 1: Write the failing tests**

`src/lib/JobChip.test.ts`:

```ts
import { render, screen } from '@testing-library/svelte';
import { describe, it, expect } from 'vitest';
import JobChip from './JobChip.svelte';

describe('JobChip', () => {
  it('announces the work politely, without a bar when it does not count', () => {
    render(JobChip, { label: 'Scanning hosts' });
    const chip = screen.getByTestId('assets-job');
    expect(chip.getAttribute('role')).toBe('status');
    expect(chip.getAttribute('aria-live')).toBe('polite');
    expect(chip.textContent).toContain('Scanning hosts');
    expect(chip.querySelector('[role="progressbar"]')).toBeNull();
  });
  it('counts with a progress bar when it can', () => {
    render(JobChip, { label: 'Syncing', done: 2, total: 5 });
    const bar = screen.getByRole('progressbar');
    expect(screen.getByTestId('assets-job').textContent).toContain('Syncing 2/5');
    expect([bar.getAttribute('aria-valuenow'), bar.getAttribute('aria-valuemax')]).toEqual(['2', '5']);
  });
});
```

`src/lib/CatalogChip.test.ts`:

```ts
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi } from 'vitest';
import CatalogChip from './CatalogChip.svelte';

const repo = { head: 'abcdef1234567890', dirty: 2, ahead: 3, behind: 0, has_upstream: true };

describe('CatalogChip', () => {
  it('shows HEAD, ahead and dirty on the chip', () => {
    render(CatalogChip, { name: 'personal', head: null, repo });
    expect(screen.getByTestId('assets-head').textContent).toBe('personal @abcdef1 ↑3 ±2');
  });
  it('opens a popover with pull, commit and push for a catalog it may write', async () => {
    const onpush = vi.fn();
    const oncommit = vi.fn();
    render(CatalogChip, { name: 'personal', head: null, repo, writable: true, onpush, oncommit, onpull: vi.fn() });
    const chip = screen.getByTestId('catalog-chip-personal');
    expect(chip.getAttribute('aria-expanded')).toBe('false');
    await fireEvent.click(chip);
    expect(chip.getAttribute('aria-expanded')).toBe('true');
    expect(screen.getByTestId('assets-repo-status').textContent).toContain('2 dirty');
    await fireEvent.click(screen.getByTestId('assets-commit-pending'));
    expect(oncommit).toHaveBeenCalled();
    expect(screen.getByTestId('assets-push').textContent).toContain('↑3');
    await fireEvent.click(screen.getByTestId('assets-push'));
    expect(onpush).toHaveBeenCalled();
  });
  it('disables push without an upstream and hides commit when clean', async () => {
    render(CatalogChip, { name: 'personal', head: null, repo: { ...repo, dirty: 0, has_upstream: false, ahead: null }, writable: true, onpush: vi.fn() });
    await fireEvent.click(screen.getByTestId('catalog-chip-personal'));
    expect(screen.getByTestId('assets-push')).toBeDisabled();
    expect(screen.queryByTestId('assets-commit-pending')).toBeNull();
  });
  it('an org catalog: status only, and how it gets pushed', async () => {
    render(CatalogChip, { name: 'papayapos', head: '9f0e1d2aaaa', state: 'problem', problem: 'not a git repository' });
    const chip = screen.getByTestId('catalog-chip-papayapos');
    expect(chip.textContent).toContain('papayapos @9f0e1d2 ⚠');
    expect(chip.getAttribute('title')).toBe('not a git repository');
    await fireEvent.click(chip);
    expect(screen.queryByTestId('assets-push')).toBeNull();
    expect(screen.getByRole('dialog').textContent).toContain('catalog.auto_push');
  });
  it('Esc closes the popover', async () => {
    render(CatalogChip, { name: 'personal', head: 'abc', repo, writable: true });
    await fireEvent.click(screen.getByTestId('catalog-chip-personal'));
    await fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });
    expect(screen.queryByRole('dialog')).toBeNull();
  });
});
```

`src/lib/AssetsFooter.test.ts`:

```ts
import { render, screen } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import AssetsFooter from './AssetsFooter.svelte';
import { lastSyncRun, repoStatusStore, syncProgress, catalogConfig } from './assets';
import { catalogStatuses } from './assets_workspace';
import { fleetSettings, SETTING_DEFAULTS } from './fleet_settings';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;
const props = { readOnly: false, listing: null, busy: '', onpull: vi.fn(), oncommit: vi.fn(), onpush: vi.fn() };
const status = (name: string, org_id: number | null, head: string) => ({
  id: org_id ?? 1, name, org_id, repo_path: '/r', remote_url: null, head_commit: head, last_loaded_at: 1, state: 'loaded' as const, asset_count: 1,
});

beforeEach(() => {
  invoke.mockReset();
  invoke.mockRejectedValue({ code: 'E_FORBIDDEN', message: 'no grant' });
  lastSyncRun.set(null); repoStatusStore.set(null); syncProgress.set(null); catalogConfig.set(null); catalogStatuses.set(null);
  fleetSettings.set({ ...SETTING_DEFAULTS });
});

describe('AssetsFooter', () => {
  it('a chip per catalog, auto, and the last sync (an SB6 run marked auto)', () => {
    catalogStatuses.set([status('personal', null, 'a1b2c3d9'), status('papayapos', 7, '9f0e1d2a')]);
    lastSyncRun.set({ plan_id: 'p', started_at: 1, finished_at: 2, auto: true, hosts: [{ host_alias: 'oci', harness: 'claude', status: 'applied', detail: null, restart_required: false, actions: [] }] });
    render(AssetsFooter, props);
    expect(screen.getByTestId('catalog-chip-personal')).toBeTruthy();
    expect(screen.getByTestId('catalog-chip-papayapos').textContent).toContain('@9f0e1d2');
    expect(screen.getByTestId('assets-auto').textContent).toBe('auto: on');
    const last = screen.getByTestId('assets-last-sync');
    expect(last.textContent).toContain('1 applied');
    expect(last.textContent).toContain('auto');
  });
  it('with no catalog listing (refused), one personal chip from the repo status', () => {
    repoStatusStore.set({ head: 'abcdef1234', dirty: 0, ahead: 1, behind: 0, has_upstream: true });
    render(AssetsFooter, props);
    expect(screen.getByTestId('assets-head').textContent).toBe('personal @abcdef1 ↑1');
  });
  it('shows the work in progress instead of the last sync', () => {
    syncProgress.set({ plan_id: 'p', host_alias: 'oci', harness: 'claude', done: 2, total: 5 });
    render(AssetsFooter, { ...props, busy: 'apply' });
    expect(screen.getByTestId('assets-job').textContent).toContain('Syncing 2/5');
    expect(screen.queryByTestId('assets-last-sync')).toBeNull();
  });
  it('read-only: the chips are information, with no popover actions', async () => {
    render(AssetsFooter, { ...props, readOnly: true, listing: { head: 'feedface99', loaded_at: 1, assets: [], unmanaged: [], problems: [] } });
    expect(screen.getByTestId('assets-head').textContent).toBe('personal @feedfac');
    (screen.getByTestId('catalog-chip-personal') as HTMLButtonElement).click();
    expect(screen.queryByTestId('assets-push')).toBeNull();
  });
});
```

- [ ] **Step 2: Run them to verify they fail**

Run: `pnpm exec vitest run src/lib/JobChip.test.ts src/lib/CatalogChip.test.ts src/lib/AssetsFooter.test.ts`
Expected: FAIL — the three components do not exist.

- [ ] **Step 3: Write the components**

`src/lib/JobChip.svelte`:

```svelte
<script lang="ts">
  /** Background work in the footer (spec: "`JobChip` for scans and
   *  syncs"): non-modal, announced politely, with a bar when it counts. */
  let {
    label,
    done = null,
    total = null,
    testid = 'assets-job',
  }: { label: string; done?: number | null; total?: number | null; testid?: string } = $props();

  const counted = $derived(done !== null && total !== null && total > 0);
  const pct = $derived(counted ? Math.round(((done ?? 0) / (total ?? 1)) * 100) : 0);
</script>

<span class="job" role="status" aria-live="polite" data-testid={testid}>
  <span class="spin" aria-hidden="true">⟳</span>
  <span>{label}{#if counted} {done}/{total}{/if}</span>
  {#if counted}
    <span class="bar" role="progressbar" aria-label={label} aria-valuemin="0" aria-valuemax={total} aria-valuenow={done}
      ><i style:width={`${pct}%`}></i></span
    >
  {/if}
</span>

<style>
  .job { display: inline-flex; align-items: center; gap: 6px; color: var(--fg); }
  .spin { display: inline-block; animation: spin 1.2s linear infinite; }
  .bar { width: 60px; height: 4px; overflow: hidden; border-radius: 2px; background: var(--control-bg-active); }
  .bar i { display: block; height: 100%; background: var(--accent); }
  @keyframes spin { to { transform: rotate(360deg); } }
  @media (prefers-reduced-motion: reduce) { .spin { animation: none; } }
</style>
```

`src/lib/CatalogChip.svelte`:

```svelte
<script lang="ts">
  import type { RepoStatus } from './assets';

  /** One catalog in the footer (spec, Footer; Rulings R24): HEAD, how far
   *  ahead of its upstream, uncommitted files. A catalog this window may
   *  write gets pull, commit and push in a popover (SB4: pushing is a
   *  person's step); any other shows its status. */
  let {
    name,
    head,
    state = 'loaded',
    problem = null,
    repo = null,
    writable = false,
    busy = false,
    onpull,
    oncommit,
    onpush,
  }: {
    name: string;
    head: string | null;
    state?: 'loaded' | 'problem' | 'not_loaded';
    problem?: string | null;
    repo?: RepoStatus | null;
    writable?: boolean;
    busy?: boolean;
    onpull?: () => void;
    oncommit?: () => void;
    onpush?: () => void;
  } = $props();

  let open = $state(false);
  let root: HTMLElement | undefined = $state();
  const personal = $derived(name === 'personal');
  const short = $derived((repo?.head ?? head ?? '').slice(0, 7) || '—');
  const ahead = $derived(repo?.ahead ?? 0);
  const dirty = $derived(repo?.dirty ?? 0);
  const text = $derived(
    `${name} @${short}${ahead ? ` ↑${ahead}` : ''}${dirty ? ` ±${dirty}` : ''}${state === 'problem' ? ' ⚠' : ''}`,
  );

  $effect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => {
      if (root && !root.contains(e.target as Node)) open = false;
    };
    document.addEventListener('mousedown', away);
    return () => document.removeEventListener('mousedown', away);
  });

  function act(fn?: () => void) {
    open = false;
    fn?.();
  }
</script>

<span class="cchip" bind:this={root}>
  <button
    type="button"
    class="btn btn--quiet is-bounded chip"
    aria-haspopup="dialog"
    aria-expanded={open}
    title={problem ?? ''}
    onclick={() => (open = !open)}
    data-testid={`catalog-chip-${name}`}
  ><span class="mono" data-testid={personal ? 'assets-head' : undefined}>{text}</span></button>
  {#if open}
    <div
      class="pop"
      role="dialog"
      tabindex="-1"
      aria-label={`Catalog ${name}`}
      onkeydown={(e) => {
        if (e.key === 'Escape') {
          e.stopPropagation();
          open = false;
        }
      }}
    >
      {#if repo}
        <p class="status" data-testid={personal ? 'assets-repo-status' : `catalog-status-${name}`}>
          {repo.head.slice(0, 7)} · {repo.dirty} dirty
          {#if repo.has_upstream}· ↑{repo.ahead ?? 0} ↓{repo.behind ?? 0}{:else}· no upstream{/if}
        </p>
      {:else}
        <p class="status muted">{state === 'problem' ? problem : state === 'not_loaded' ? 'not loaded' : `@${short}`}</p>
      {/if}
      {#if writable && personal}
        <div class="acts">
          <button class="btn btn--quiet is-bounded" disabled={busy} onclick={() => act(onpull)} data-testid="assets-pull">Pull</button>
          {#if dirty > 0}
            <button class="btn btn--quiet is-bounded" disabled={busy} onclick={() => act(oncommit)} data-testid="assets-commit-pending">Commit pending</button>
          {/if}
          <button
            class="btn btn--quiet is-bounded"
            disabled={busy || !repo?.has_upstream}
            title={repo?.has_upstream ? '' : 'no upstream configured'}
            onclick={() => act(onpush)}
            data-testid="assets-push"
          >Push{ahead ? ` ↑${ahead}` : ''}</button>
        </div>
      {:else if !personal}
        <p class="muted">Cards commit here; with <code>catalog.auto_push</code> on, fleet pushes after each apply.</p>
      {/if}
    </div>
  {/if}
</span>

<style>
  .cchip { position: relative; display: inline-flex; }
  .chip { height: 18px; padding: 0 6px; }
  .mono { font-family: var(--mono); font-size: 11px; color: var(--fg); }
  .pop {
    position: absolute; bottom: calc(100% + 6px); left: 0; z-index: 10; display: grid; gap: 8px; min-width: 240px;
    padding: 10px 12px; border: 1px solid var(--control-border); border-radius: var(--radius-md); background: var(--bg); font-size: 12px;
  }
  .status { margin: 0; font-family: var(--mono); font-size: 11.5px; }
  .acts { display: flex; gap: 6px; flex-wrap: wrap; }
  .muted { margin: 0; color: var(--fg-muted); }
</style>
```

`src/lib/AssetsFooter.svelte`:

```svelte
<script lang="ts">
  import Badge from './Badge.svelte';
  import CatalogChip from './CatalogChip.svelte';
  import JobChip from './JobChip.svelte';
  import { catalogConfig, lastSyncRun, repoStatusStore, syncProgress, type AssetListing, type RepoStatus } from './assets';
  import { catalogStatuses, PERSONAL, repoStatusOf, summarizeRun } from './assets_workspace';
  import { fleetSettings, settingBool, SETTING_KEYS } from './fleet_settings';

  /** The workspace footer (spec, Footer): a chip per catalog, `auto`, and
   *  either the work in progress (`JobChip`) or the last sync (R10, R24, R25). */
  let {
    readOnly,
    listing,
    busy,
    onpull,
    oncommit,
    onpush,
  }: {
    readOnly: boolean;
    listing: AssetListing | null;
    busy: string;
    onpull: () => void;
    oncommit: () => void;
    onpush: () => void;
  } = $props();

  type Chip = { name: string; head: string | null; state: 'loaded' | 'problem' | 'not_loaded'; problem: string | null };
  const chips = $derived.by((): Chip[] => {
    const rows = $catalogStatuses;
    if (rows && rows.length) {
      return rows.map((c) => ({ name: c.name, head: c.head_commit, state: c.state, problem: c.problem ?? null }));
    }
    const head = $repoStatusStore?.head ?? $catalogConfig?.head_commit ?? listing?.head ?? null;
    return [{ name: PERSONAL, head, state: 'loaded', problem: null }];
  });

  // R24: an org catalog's dirty/ahead, read once per name; refused (no
  // grant) leaves the chip at its HEAD.
  let orgRepo = $state<Record<string, RepoStatus | null>>({});
  $effect(() => {
    if (readOnly) return;
    for (const c of chips) {
      if (c.name === PERSONAL || c.name in orgRepo) continue;
      orgRepo[c.name] = null;
      void repoStatusOf(c.name).then((r) => {
        if (r.ok) orgRepo[c.name] = r.value;
      });
    }
  });

  const JOB: Record<string, string> = {
    scan: 'Scanning hosts', plan: 'Planning a sync', apply: 'Syncing', pull: 'Pulling', commit: 'Committing', push: 'Pushing',
  };
  const auto = $derived(settingBool($fleetSettings, SETTING_KEYS.catalogAuto));
</script>

<footer class="foot" data-testid="assets-footer">
  {#each chips as c (c.name)}
    <CatalogChip
      name={c.name}
      head={c.head}
      state={c.state}
      problem={c.problem}
      repo={c.name === PERSONAL ? (readOnly ? null : $repoStatusStore) : (orgRepo[c.name] ?? null)}
      writable={!readOnly && c.name === PERSONAL}
      busy={busy !== ''}
      {onpull}
      {oncommit}
      {onpush}
    />
  {/each}
  <Badge
    tone={auto ? 'ok' : 'muted'}
    label={`auto: ${auto ? 'on' : 'off'}`}
    title="catalog.auto — Settings → Automation → Assets: hide internals, prepare cards, sync additively on rolled-out layers"
    testid="assets-auto"
  />
  <span class="grow"></span>
  {#if busy && JOB[busy]}
    <JobChip
      label={JOB[busy]}
      done={busy === 'apply' ? ($syncProgress?.done ?? null) : null}
      total={busy === 'apply' ? ($syncProgress?.total ?? null) : null}
    />
  {:else if $lastSyncRun}
    <span class="last" data-testid="assets-last-sync">
      {summarizeRun($lastSyncRun)}
      {#if $lastSyncRun.auto}<Badge tone="muted" label="auto" title="SB6 ran this sync, not a person" />{/if}
    </span>
  {/if}
</footer>

<style>
  .foot {
    display: flex; align-items: center; gap: 10px; min-height: 26px; padding: 0 12px;
    border-top: 1px solid var(--border); background: var(--bg-pane); color: var(--fg-muted); font-size: 11.5px;
  }
  .grow { flex: 1; }
  .last { display: inline-flex; align-items: center; gap: 6px; white-space: nowrap; }
</style>
```

Add `'JobChip.svelte'`, `'CatalogChip.svelte'`, `'AssetsFooter.svelte'` to `GUARDED`.

- [ ] **Step 4: Run the tests**

Run: `pnpm exec vitest run src/lib/JobChip.test.ts src/lib/CatalogChip.test.ts src/lib/AssetsFooter.test.ts src/lib/assets_tokens.test.ts`
Expected: PASS.
Run: `pnpm check` — Expected: 0 errors.

- [ ] **Step 5: Commit**

```bash
git add src/lib/JobChip.svelte src/lib/JobChip.test.ts src/lib/CatalogChip.svelte src/lib/CatalogChip.test.ts \
  src/lib/AssetsFooter.svelte src/lib/AssetsFooter.test.ts src/lib/assets_tokens.test.ts
git commit -m "feat(assets-ui): the footer — a chip per catalog, auto, JobChip and the last sync

Each catalog shows HEAD, ahead and dirty; the personal chip's popover pulls,
commits and pushes; an org catalog's shows its status. auto says whether
catalog.auto is on; JobChip shows scans and syncs in progress, otherwise the
last sync a person made (an SB6 run is marked auto)."
```

---

### Task 11: `AssetsWorkspace` — the shell, the rail, the sentence header, the read-only chip, inside `AssetsPanel` (R15, R17, R18, R20, R23)

**Files:**
- Create: `src/lib/AssetsRail.svelte`, `src/lib/AssetsWorkspace.svelte`, `src/lib/AssetsWorkspace.test.ts`
- Modify: `src/lib/Icon.svelte` (three icons)
- Modify: `src/lib/AssetsPanel.svelte` (the two toolbar layouts become `AssetsWorkspace`; selection by key; workspace loads in `refresh`/`loadOverview`)
- Modify: `src/lib/AssetsPanel.test.ts`, `src/lib/hub_disabled.test.ts` (moved controls)
- Modify: `src/lib/assets_tokens.test.ts` (add `'AssetsRail.svelte'`, `'AssetsWorkspace.svelte'`)

**Interfaces:**
- Consumes: everything of Tasks 5–10; `catalog`, `inventory`, `catalogConfig` (`assets.ts`); `hosts` (`hosts.ts`); `hubStatus`, `hubActionBlocked` (`hub.ts`); `hubConnection`.
- Produces:
  - `Icon.svelte` names `'inbox' | 'library' | 'key'`.
  - `AssetsRail.svelte` props `{ view: WorkspaceView; counts: Record<WorkspaceView, number>; readOnly: boolean; onview: (v: WorkspaceView) => void; onsecrets: () => void }`; testids `assets-rail-inbox`, `assets-rail-library`, `assets-secrets`.
  - `AssetsWorkspace.svelte` props:

    ```ts
    {
      readOnly?: boolean; readOnlyClient?: string | null; visible?: boolean;
      busy?: string; error?: string | null; scanResults?: HostScanResult[] | null;
      loading?: boolean; scanDisabled?: boolean; importBlocked?: string | null;
      failed?: Snippet;                         // shown in the list area instead of the list
      selectedKey?: string | null ($bindable);  autoEditKey?: string;
      onscan: () => void; onsync: (f: { hostAlias?: string; kind?: string; name?: string }) => void;
      onimport: (identity: AssetIdentity | null) => void; onsecrets?: () => void; onnew?: () => void;
      onlintall?: () => void; onpull?: () => void; oncommit?: () => void; onpush?: () => void;
      onrefresh?: () => void; ondeleted?: () => void;
    }
    ```

    testids `assets-workspace`, `assets-sentence`, `assets-readonly`, `assets-grant-cmd`, `assets-problems`, `assets-hub-refresh`, `assets-scan`, `assets-sync`, `assets-new`, `assets-import`, `assets-lint-all`, `assets-scan-result`, `assets-list`.

- [ ] **Step 1: Write the failing workspace test**

`src/lib/AssetsWorkspace.test.ts`:

```ts
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import AssetsWorkspace from './AssetsWorkspace.svelte';
import { catalog, inventory, lastSyncRun, repoStatusStore, type AssetListing } from './assets';
import { catalogStatuses, changesetSummaries, layerListing } from './assets_workspace';
import { hosts } from './hosts';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;
const listing: AssetListing = {
  head: 'abcdef1234567890', loaded_at: 1, problems: [{ path: 'hooks/bad.yaml', message: 'name' }],
  unmanaged: [{ host_alias: 'oci', harness: 'claude', kind: 'skill', name: 'fresh', state: 'unmanaged', catalog_hash: null, host_hash: 'h', scanned_at: 100, managed: false }],
  assets: [
    { kind: 'skill', name: 'edited', version: '1', description: '', tags: [], catalog: 'personal', hosts: [{ host_alias: 'oci', harness: 'claude', state: 'drifted', drift_side: 'host' }] },
    { kind: 'skill', name: 'fine', version: '1', description: '', tags: [], catalog: 'personal', hosts: [{ host_alias: 'local', harness: 'claude', state: 'in_sync' }] },
  ],
  identities: [{ kind: 'skill', name: 'fresh', hosts: [{ host_alias: 'oci', harness: 'claude', host_hash: 'h' }], signature: 'oci', variants: 1, class: 'normal', reason: null }],
};
const handlers = () => ({ onscan: vi.fn(), onsync: vi.fn(), onimport: vi.fn(), onsecrets: vi.fn(), onnew: vi.fn(), onlintall: vi.fn() });

beforeEach(() => {
  invoke.mockReset();
  invoke.mockRejectedValue({ code: 'E_TEST', message: 'not in this test' });
  catalog.set(listing); inventory.set([]); lastSyncRun.set(null); repoStatusStore.set(null);
  catalogStatuses.set(null); changesetSummaries.set(null); layerListing.set(null);
  hosts.set([
    { alias: 'local', ssh_alias: null, reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: null, account_uuid: null, provisioned: true, transport: 'ssh' },
    { alias: 'oci', ssh_alias: 'oci', reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: null, account_uuid: null, provisioned: true, transport: 'ssh' },
  ]);
});

describe('AssetsWorkspace', () => {
  it('opens on the Inbox with a sentence, the rail, the list, the Inspector and the footer', () => {
    render(AssetsWorkspace, handlers());
    expect(screen.getByTestId('assets-sentence').textContent).toBe('1 needs you · 1 new on hosts');
    expect(screen.getByTestId('assets-rail-inbox').getAttribute('aria-current')).toBe('page');
    expect(screen.getByTestId('assets-rail-inbox').textContent).toContain('1');
    expect(screen.getByTestId('assets-inbox')).toBeTruthy();
    expect(screen.getByTestId('inspector-empty')).toBeTruthy();
    expect(screen.getByTestId('assets-footer')).toBeTruthy();
    expect(screen.getByTestId('assets-problems').textContent).toContain('1 problems');
  });

  it('has one primary button, Sync fleet, and quiet controls otherwise', async () => {
    const h = handlers();
    render(AssetsWorkspace, h);
    const primaries = document.querySelectorAll('.btn--primary');
    expect(primaries).toHaveLength(1);
    expect(primaries[0].textContent).toContain('Sync fleet');
    await fireEvent.click(screen.getByTestId('assets-sync'));
    expect(h.onsync).toHaveBeenCalledWith({});
    await fireEvent.click(screen.getByTestId('assets-scan'));
    expect(h.onscan).toHaveBeenCalled();
    await fireEvent.click(screen.getByTestId('assets-secrets'));
    expect(h.onsecrets).toHaveBeenCalled();
  });

  it('the Library holds the authoring controls and every asset once', async () => {
    const h = handlers();
    render(AssetsWorkspace, h);
    expect(screen.queryByTestId('assets-new')).toBeNull();
    await fireEvent.click(screen.getByTestId('assets-rail-library'));
    expect(screen.getByTestId('assets-rail-library').getAttribute('aria-current')).toBe('page');
    expect(screen.getByTestId('asset-row-skill-edited')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('assets-new'));
    expect(h.onnew).toHaveBeenCalled();
    await fireEvent.click(screen.getByTestId('assets-import'));
    expect(h.onimport).toHaveBeenCalledWith(null);
  });

  it('selecting a row shows it in the Inspector', async () => {
    render(AssetsWorkspace, handlers());
    await fireEvent.click(screen.getByTestId('inbox-row-identity:skill/fresh'));
    expect(screen.getByTestId('inspector').textContent).toContain('fresh');
  });

  it('read-only: one scope chip, no mutating control, the grant command on demand', async () => {
    render(AssetsWorkspace, { ...handlers(), readOnly: true, readOnlyClient: 'desk', onrefresh: vi.fn() });
    const chip = screen.getByTestId('assets-readonly');
    expect(chip.textContent).toBe('read-only · ask the operator to grant assets on personal');
    for (const id of ['assets-sync', 'assets-secrets', 'assets-new', 'assets-import', 'assets-lint-all']) {
      expect(screen.queryByTestId(id), id).toBeNull();
    }
    expect(screen.getByTestId('assets-scan')).toBeTruthy();
    expect(screen.getByTestId('assets-hub-refresh')).toBeTruthy();
    await fireEvent.click(chip);
    expect(screen.getByTestId('assets-grant-cmd').textContent).toBe('fleet-hub client grant desk assets');
  });
});
```

- [ ] **Step 2: Run it to verify it fails**

Run: `pnpm exec vitest run src/lib/AssetsWorkspace.test.ts`
Expected: FAIL — `Failed to resolve import "./AssetsWorkspace.svelte"`.

- [ ] **Step 3: Add the rail icons and `AssetsRail.svelte`**

`Icon.svelte`: extend `IconName` with `| 'inbox' | 'library' | 'key'` and add before the closing `{/if}` (paths from the mockups):

```svelte
  {:else if name === 'inbox'}
    <path d="M3 12h5l2 3h4l2-3h5" /><path d="M5 5h14l2 7v6a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1v-6z" />
  {:else if name === 'library'}
    <rect x="3" y="3" width="7" height="7" rx="1" /><rect x="14" y="3" width="7" height="7" rx="1" />
    <rect x="3" y="14" width="7" height="7" rx="1" /><rect x="14" y="14" width="7" height="7" rx="1" />
  {:else if name === 'key'}
    <circle cx="8" cy="15" r="4" /><path d="m11 12 9-9M17 6l3 3" />
```

`src/lib/AssetsRail.svelte`:

```svelte
<script lang="ts">
  import Icon from './Icon.svelte';
  import { RAIL_VIEWS, type WorkspaceView } from './assets_workspace';

  /** The rail (spec: Inbox default, Library; Layers and Hosts join with
   *  their views in M6, R15). Secrets opens the existing panel. */
  let {
    view,
    counts,
    readOnly,
    onview,
    onsecrets,
  }: {
    view: WorkspaceView;
    counts: Record<WorkspaceView, number>;
    readOnly: boolean;
    onview: (v: WorkspaceView) => void;
    onsecrets: () => void;
  } = $props();
</script>

<nav class="rail" aria-label="Assets views">
  {#each RAIL_VIEWS as r (r.id)}
    <button
      type="button"
      class:on={view === r.id}
      aria-current={view === r.id ? 'page' : undefined}
      onclick={() => onview(r.id)}
      data-testid={`assets-rail-${r.id}`}
    >
      <Icon name={r.id} size={15} />
      <span class="lbl">{r.label}</span>
      {#if counts[r.id]}<span class="ct">{counts[r.id]}</span>{/if}
    </button>
  {/each}
  {#if !readOnly}
    <div class="sep" role="separator"></div>
    <button type="button" onclick={onsecrets} data-testid="assets-secrets">
      <Icon name="key" size={15} /><span class="lbl">Secrets</span>
    </button>
  {/if}
</nav>

<style>
  .rail { display: flex; flex-direction: column; gap: 2px; padding: 10px 8px; border-right: 1px solid var(--border); background: var(--bg-pane); }
  .rail button {
    display: flex; align-items: center; gap: 9px; height: 28px; padding: 0 8px; border: 0; border-radius: var(--radius-md);
    background: none; color: var(--control-fg-quiet); font: inherit; font-size: 12.5px; cursor: pointer; text-align: left;
  }
  .rail button:hover { background: var(--control-bg-hover); }
  .rail button.on { background: var(--accent-soft); color: var(--fg); font-weight: 600; }
  .rail button:focus-visible { outline: var(--ring-w) solid var(--ring); outline-offset: var(--ring-offset); }
  .ct { margin-left: auto; font-size: 11px; font-variant-numeric: tabular-nums; color: var(--fg-muted); font-weight: 400; }
  .on .ct { color: var(--accent); }
  .sep { height: 1px; margin: 8px 4px; background: var(--border); }
</style>
```

- [ ] **Step 4: Write `AssetsWorkspace.svelte`**

```svelte
<script lang="ts">
  import type { Snippet } from 'svelte';
  import AssetsRail from './AssetsRail.svelte';
  import AssetsInbox from './AssetsInbox.svelte';
  import AssetList from './AssetList.svelte';
  import AssetInspector from './AssetInspector.svelte';
  import AssetsFooter from './AssetsFooter.svelte';
  import QueryInput from './QueryInput.svelte';
  import Badge from './Badge.svelte';
  import { catalog, identitiesOf, inventory, type AssetIdentity, type AssetSummary, type HostScanResult } from './assets';
  import { hosts } from './hosts';
  import { hubStatus } from './hub';
  import {
    canWrite, catalogStatuses, changesetSummaries, keyOf, layerListing, parseKey, PERSONAL, type WorkspaceView,
  } from './assets_workspace';
  import { buildInbox, hostOrderOf, lastScanOf, sentence } from './assets_inbox';
  import { matchesQuery, parseQuery } from './assets_query';

  /** The Assets workspace (spec, Workspace shell): rail · list with a
   *  sentence header and the query · Inspector, over a footer. It owns the
   *  view, the query and the selection; `AssetsPanel` owns loading, probing
   *  and every dialog, and is told what the person asked for. */
  let {
    readOnly = false,
    readOnlyClient = null,
    visible = true,
    busy = '',
    error = null,
    scanResults = null,
    loading = false,
    scanDisabled = false,
    importBlocked = null,
    failed,
    selectedKey = $bindable(null),
    autoEditKey = '',
    onscan,
    onsync,
    onimport,
    onsecrets = () => {},
    onnew = () => {},
    onlintall = () => {},
    onpull = () => {},
    oncommit = () => {},
    onpush = () => {},
    onrefresh = () => {},
    ondeleted = () => {},
  }: {
    readOnly?: boolean;
    readOnlyClient?: string | null;
    visible?: boolean;
    busy?: string;
    error?: string | null;
    scanResults?: HostScanResult[] | null;
    loading?: boolean;
    scanDisabled?: boolean;
    importBlocked?: string | null;
    failed?: Snippet;
    selectedKey?: string | null;
    autoEditKey?: string;
    onscan: () => void;
    onsync: (f: { hostAlias?: string; kind?: string; name?: string }) => void;
    onimport: (identity: AssetIdentity | null) => void;
    onsecrets?: () => void;
    onnew?: () => void;
    onlintall?: () => void;
    onpull?: () => void;
    oncommit?: () => void;
    onpush?: () => void;
    onrefresh?: () => void;
    ondeleted?: () => void;
  } = $props();

  let view = $state<WorkspaceView>('inbox');
  let queryText = $state('');
  let showProblems = $state(false);
  let showGrant = $state(false);
  let editNonce = $state(0);
  let listEl: HTMLElement | undefined = $state();
  let queryEl: { focus: () => void } | undefined = $state();
  let now = $state(Math.floor(Date.now() / 1000));
  $effect(() => {
    const t = setInterval(() => (now = Math.floor(Date.now() / 1000)), 30_000);
    return () => clearInterval(t);
  });

  const listing = $derived($catalog);
  const shown = $derived($hosts.filter((h) => !h.hidden));
  const order = $derived(
    hostOrderOf([
      ...shown.map((h) => h.alias),
      ...(listing?.assets.flatMap((a) => a.hosts.map((s) => s.host_alias)) ?? []),
      ...(listing?.unmanaged.map((r) => r.host_alias) ?? []),
    ]),
  );
  const stale = $derived(new Set(shown.filter((h) => h.alias !== 'local' && !h.reachable).map((h) => h.alias)));
  const layersOf = (a: { kind: string; name: string; catalog?: string | null }) =>
    (a.catalog ?? PERSONAL) !== PERSONAL
      ? []
      : ($layerListing?.layers ?? []).filter((l) => (l.members ?? []).includes(`${a.kind}/${a.name}`)).map((l) => l.name);
  const inbox = $derived(listing ? buildInbox({ listing, cards: $changesetSummaries, order, stale, layersOf }) : null);
  const head = $derived(
    inbox
      ? sentence(inbox, {
          reachable: shown.filter((h) => h.alias === 'local' || h.reachable).length,
          total: shown.length,
          lastScan: lastScanOf(listing, $inventory),
          now,
        })
      : null,
  );
  const query = $derived(parseQuery(queryText));
  const vocab = $derived({
    hosts: order,
    layers: ($layerListing?.layers ?? []).map((l) => l.name),
    catalogs: $catalogStatuses?.map((c) => c.name) ?? [PERSONAL],
  });
  const counts = $derived({
    inbox: inbox?.needCount ?? 0,
    library: listing
      ? listing.assets.length + identitiesOf(listing).filter((i) => i.class === 'normal' || i.class === 'needs_person').length
      : 0,
  });
  const ctx = $derived({
    readOnly,
    remote: $hubStatus.remote,
    clientName: $hubStatus.client_name,
    statuses: $catalogStatuses,
  });
  const writable = (a: AssetSummary) => canWrite(a.catalog, ctx);
  // R19: the full detail and editor for a personal asset this window may
  // write; the desktop's authoring commands are personal until M6.
  const canOpen = (a: AssetSummary) => writable(a) && (a.catalog ?? PERSONAL) === PERSONAL;
  const selection = $derived(selectedKey ? parseKey(selectedKey) : null);
  const selectedAsset = $derived(
    selection?.type === 'asset' ? { kind: selection.kind, name: selection.name, catalog: selection.catalog } : null,
  );

  function select(key: string) {
    selectedKey = key;
  }
</script>

<div class="ws" data-testid="assets-workspace">
  <AssetsRail {view} {counts} {readOnly} onview={(v) => (view = v)} {onsecrets} />

  <div class="main">
    <header class="head">
      <div class="line">
        <span class="sentence" data-testid="assets-sentence">{head?.text ?? (loading ? 'Loading…' : 'Asset catalog')}</span>
        {#if head}<span class="sub">{head.sub}</span>{/if}
        {#if readOnly}
          <span class="ro">
            <button type="button" class="btn btn--chip" aria-expanded={showGrant} onclick={() => (showGrant = !showGrant)} data-testid="assets-readonly"
              >read-only · ask the operator to grant assets on personal</button
            >
            {#if showGrant}
              <span class="grant" role="note">On the hub's machine: <code data-testid="assets-grant-cmd">fleet-hub client grant {readOnlyClient ?? '<this client>'} assets</code></span>
            {/if}
          </span>
        {/if}
        <span class="grow"></span>
        {#if listing && listing.problems.length > 0}
          <button type="button" class="btn btn--quiet" aria-expanded={showProblems} onclick={() => (showProblems = !showProblems)} data-testid="assets-problems">
            <Badge tone="warn" glyph="!" label={`${listing.problems.length} problems`} />
          </button>
        {/if}
        {#if readOnly}
          <button type="button" class="btn btn--quiet" onclick={onrefresh} disabled={busy !== '' || loading} data-testid="assets-hub-refresh">{loading ? 'Loading…' : 'Refresh'}</button>
        {/if}
        <button type="button" class="btn btn--quiet" onclick={onscan} disabled={busy !== '' || scanDisabled} data-testid="assets-scan">{busy === 'scan' ? 'Scanning…' : 'Rescan'}</button>
        {#if !readOnly}
          <button type="button" class="btn btn--primary" onclick={() => onsync({})} disabled={busy !== ''} data-testid="assets-sync"
            >{busy === 'plan' ? 'Planning…' : 'Sync fleet'} <kbd>⌘↵</kbd></button
          >
        {/if}
      </div>
      <div class="line">
        <QueryInput bind:this={queryEl} bind:value={queryText} {vocab} onescape={() => listEl?.focus()} />
        {#if view === 'library' && !readOnly}
          <button type="button" class="btn btn--quiet" onclick={onnew} disabled={busy !== ''} data-testid="assets-new">New asset</button>
          <button type="button" class="btn btn--quiet" onclick={() => onimport(null)} disabled={busy !== '' || importBlocked !== null} title={importBlocked ?? ''} data-testid="assets-import">Import from host</button>
          <button type="button" class="btn btn--quiet" onclick={onlintall} disabled={busy !== ''} data-testid="assets-lint-all">Lint all</button>
        {/if}
      </div>
    </header>
    {#if error}<p class="error">{error}</p>{/if}
    {#if scanResults}
      <p class="scan-result" data-testid="assets-scan-result">{scanResults.map((r) => `${r.host}: ${r.status}${r.detail ? ` (${r.detail})` : ''}`).join(' · ')}</p>
    {/if}
    {#if showProblems && listing}
      <ul class="problems">{#each listing.problems as p (p.path)}<li><code>{p.path}</code> {p.message}</li>{/each}</ul>
    {/if}
    <div class="body" bind:this={listEl} tabindex="-1" data-testid="assets-list">
      {#if failed}
        {@render failed()}
      {:else if listing && view === 'inbox' && inbox}
        <AssetsInbox {inbox} {order} {selectedKey} {query} onselect={select} />
      {:else if listing}
        <AssetList
          {listing}
          selected={selectedAsset}
          filter={query.text}
          keep={(r) => matchesQuery({ tokens: query.tokens, text: '' }, { ...r, layers: r.catalog === PERSONAL ? layersOf(r) : [] })}
          canWrite={writable}
          readonly={readOnly}
          onselect={(kind, name, cat) => select(keyOf({ type: 'asset', catalog: cat ?? PERSONAL, kind, name }))}
          onpick={select}
          onimport={(i) => onimport(i)}
        />
      {:else}
        <p class="muted pad">Loading…</p>
      {/if}
    </div>
  </div>

  <div class="insp">
    <AssetInspector
      {selectedKey}
      {listing}
      cards={$changesetSummaries}
      hosts={$hosts}
      {order}
      {readOnly}
      {canOpen}
      {autoEditKey}
      {editNonce}
      {onsync}
      ondeleted={() => {
        selectedKey = null;
        ondeleted();
      }}
      onimport={(i) => onimport(i)}
    />
  </div>

  <div class="foot">
    <AssetsFooter {readOnly} {listing} {busy} {onpull} {oncommit} {onpush} />
  </div>
</div>

<style>
  .ws {
    display: grid; flex: 1; min-height: 0;
    grid-template-columns: 172px minmax(0, 1fr) minmax(300px, 392px);
    grid-template-rows: minmax(0, 1fr) auto;
    grid-template-areas: 'rail main insp' 'foot foot foot';
  }
  .ws > :global(.rail) { grid-area: rail; }
  .main { grid-area: main; display: flex; flex-direction: column; min-width: 0; min-height: 0; }
  .insp { grid-area: insp; min-height: 0; border-left: 1px solid var(--border); }
  .foot { grid-area: foot; }
  .head { display: grid; gap: 8px; padding: 10px 14px; border-bottom: 1px solid var(--border); }
  .line { display: flex; align-items: center; gap: 10px; min-width: 0; }
  .sentence { font-size: 14px; font-weight: 600; letter-spacing: -0.005em; }
  .sub { color: var(--fg-muted); font-size: 12px; white-space: nowrap; }
  .grow { flex: 1; }
  .ro { display: inline-flex; align-items: center; gap: 8px; }
  .grant { font-size: 12px; color: var(--fg-muted); }
  .grant code, .problems code { font-family: var(--mono); font-size: 11.5px; user-select: text; }
  kbd { font-family: var(--mono); font-size: 10.5px; padding: 0 4px; border-radius: 3px; border: 1px solid color-mix(in srgb, currentColor 35%, transparent); opacity: 0.85; }
  .body { flex: 1; min-height: 0; overflow: auto; outline: 0; }
  .error { margin: 0; padding: 4px 14px; color: var(--usage-crit); }
  .scan-result { margin: 0; padding: 4px 14px; font-size: 12px; color: var(--fg-muted); }
  .problems { margin: 0; padding: 4px 14px 4px 32px; font-size: 12px; }
  .muted { color: var(--fg-muted); }
  .pad { padding: 14px; }
</style>
```

Add `'AssetsRail.svelte'` and `'AssetsWorkspace.svelte'` to `GUARDED`.

- [ ] **Step 5: Run the workspace test**

Run: `pnpm exec vitest run src/lib/AssetsWorkspace.test.ts src/lib/assets_tokens.test.ts`
Expected: PASS.

- [ ] **Step 6: Render the workspace from `AssetsPanel`**

Script changes in `AssetsPanel.svelte`:
- imports: drop `AssetList`, `AssetDetail`; add `import AssetsWorkspace from './AssetsWorkspace.svelte';`, `import { keyOf, loadCatalogStatuses, loadChangesets, loadLayers } from './assets_workspace';`, `import { loadFleetSettings } from './fleet_settings';`; drop `type SyncRunSummary` only if now unused (it is still used by `onSyncApplied`).
- `let filter = $state('');` and `let selected = …` go; add `let selectedKey = $state<string | null>(null);`. `pendingAutoEdit` keeps its name and now holds a key.
- `summarizeRun` and `shortHead` go (the footer has them).
- `refresh()` becomes:

```ts
  async function refresh() {
    const [a, i] = await Promise.all([loadAssets(), loadInventory()]);
    if (!a.ok) error = a.error.message;
    if (!i.ok) error = i.error.message;
    // The workspace's own reads (R13): best effort — a refusal shows less, never an error.
    void loadCatalogStatuses();
    void loadChangesets();
    void loadLayers();
  }
```

- `loadOverview()` gains, after a successful `loadAssets()`: `void loadCatalogStatuses(); void loadChangesets();` (an ungranted client may list catalogs and cards when unbound and full — M3 PF15 — and is refused otherwise; never `catalog_last_sync` or `catalog_load`).
- `onMount` gains `void loadFleetSettings();` before its early return check (the footer's `auto`).
- `onAssetCreated`: `pendingAutoEdit = keyOf({ type: 'asset', catalog: 'personal', kind, name }); selectedKey = pendingAutoEdit;`.
- `onAssetDeleted`: `selectedKey = null;` (then `afterWrite()` as before).
- `onLintAllSelect`: `selectedKey = keyOf({ type: 'asset', catalog: 'personal', kind, name });`.
- add

```ts
  // The workspace's Import: the Library's button (no identity — the dialog's
  // own defaults) or an identity row, its Inspector, or the `a` key.
  function importFrom(identity: AssetIdentity | null) {
    if (identity) return onImportUnmanaged(identity);
    importPreset = null;
    showImport = true;
  }
```

Markup: the `{:else if hubOverview}` branch (the whole `<div class="hub-overview">…</div>`) becomes

```svelte
    {:else if hubOverview}
      {#snippet hubFailed()}
        <div class="load-failed pad" data-testid="assets-hub-failed">
          {#if overviewNotConfigured}
            <p class="muted">The hub has no asset catalog yet. It is a git checkout on the hub's machine, set there — then Refresh here:</p>
            <pre class="cmd" data-testid="assets-hub-setup-cmd">fleet-hub catalog set ~/agent-assets --remote git@github.com:you/agent-assets.git</pre>
            <p class="muted">The remote is cloned when the path is empty. In the Docker setup, prefix it with <code>docker compose exec fleet-hub</code> and keep the checkout on the data volume, e.g. <code>/var/lib/fleet-hub/agent-assets</code>.</p>
          {:else}
            <p class="muted">The hub's asset catalog could not be loaded.</p>
            {#if overviewError}<p class="error">{overviewError}</p>{/if}
          {/if}
        </div>
      {/snippet}
      {#if hubAdminError}<p class="error" data-testid="assets-grant-error">{hubAdminError}</p>{/if}
      <AssetsWorkspace
        readOnly
        readOnlyClient={$hubStatus.client_name}
        {visible}
        {busy}
        {error}
        {scanResults}
        loading={overviewLoad === 'loading'}
        scanDisabled={overviewNotConfigured}
        failed={overviewLoad === 'failed' ? hubFailed : undefined}
        bind:selectedKey
        onscan={scanOnHub}
        onsync={() => {}}
        onimport={() => {}}
        onrefresh={() => void loadOverview()}
      />
```

and the final `{:else}` branch (the granted / standalone toolbar, repo-status strip, error, scan result, problems and the `.body` grid) becomes

```svelte
  {:else}
    {#snippet loadFailed()}
      <!-- `catalog` is only ever set on success; this keys off the load's
           OWN state, not the shared per-action `error`. -->
      <div class="load-failed pad" data-testid="assets-load-failed">
        <p class="muted">The asset catalog could not be loaded.</p>
        {#if catalogLoadError}<p class="error" data-testid="assets-load-error">{catalogLoadError}</p>{/if}
        <button class="btn" onclick={() => void reload(false)} data-testid="assets-retry">Retry</button>
      </div>
    {/snippet}
    <AssetsWorkspace
      {visible}
      {busy}
      {error}
      {scanResults}
      loading={catalogLoad === 'loading'}
      importBlocked={importBlocked}
      failed={!$catalog && catalogLoad === 'failed' ? loadFailed : undefined}
      bind:selectedKey
      autoEditKey={pendingAutoEdit}
      onscan={scan}
      onsync={requestSync}
      onimport={importFrom}
      onsecrets={() => (showSecrets = true)}
      onnew={() => (showNewAsset = true)}
      onlintall={() => (showLintAll = true)}
      onpull={pull}
      oncommit={() => (showCommitPrompt = true)}
      onpush={doPush}
      ondeleted={onAssetDeleted}
    />
  {/if}
```

The dialogs below stay exactly as they are. In `<style>`, drop `.toolbar`, `.path`, `.head`, `.badge`, `.last-sync`, `.repo-status`, `.filter`, `.hub-overview`, `.hub-overview .note`, `.overview-list`, `.body`, `.left`, `.right`, `.scan-result`, `.problems`, `.empty`; keep `.assets-panel`, `.setup`, `.load-failed`, `.cmd`, `.muted`, `.error`, `.primary`; add `.pad { padding: 14px; }`.

- [ ] **Step 7: Move the panel's tests to the new homes**

In `src/lib/AssetsPanel.test.ts`:

- add after `byCmd`:

```ts
/** The Library rail entry: where the catalog's rows, New asset, Import and
 *  Lint all live since Assets M5 (Rulings R17). */
async function openLibrary() {
  await fireEvent.click(await screen.findByTestId('assets-rail-library'));
}
/** The personal catalog chip's popover: Pull, Commit pending, Push and the
 *  repo status line (R17, R24). */
async function openPersonalChip() {
  await fireEvent.click(await screen.findByTestId('catalog-chip-personal'));
}
```

- `lists assets grouped by kind…`: after `render(AssetsPanel);` add `await openLibrary();` (the rest is unchanged: `assets-problems` is in the header, `assets-head` on the footer chip).
- `selecting an asset loads the detail…`: `await openLibrary();` before clicking the row; after `findByTestId('asset-detail-title')`, the matrix assertions follow `await fireEvent.click(screen.getByTestId('inspector-tab-hosts'));`, and the preview ones follow `await fireEvent.click(screen.getByTestId('inspector-tab-source'));`.
- `renders the detail title when catalog_get_asset omits the tags key entirely`, `Import next to an unmanaged identity…`, `an orphan row…`, `the filter matches an orphan row…`, `hides fleet internals…`, `Delete asks for confirmation…`, `Lint shows the inline report…`, `reloads the catalog when the panel regains visibility…`, `New asset creates via the dialog…`, `Lint all opens the dialog…`: add `await openLibrary();` right after `render(…)`.
- `New asset creates via the dialog…`: a just-created asset opens in Source, where `AssetDetail`'s title row (an Overview part) is not shown; its wait becomes `await waitFor(() => expect(screen.getByTestId('inspector').textContent).toContain('my-new-skill'));` (the Inspector's title), then `editor-save` as before.
- `import completion reloads the catalog…`: `await openLibrary();` then `await fireEvent.click(screen.getByTestId('assets-import'));` replaces `findByText('Import from host')`/`getByText` (the label is unchanged).
- `Import next to an unmanaged identity…`: the second open after Close also uses `screen.getByTestId('assets-import')`.
- `the filter matches an orphan row…`: `screen.getByPlaceholderText('filter')` becomes `screen.getByTestId('assets-query')`.
- `shows the repo status strip and gates Commit pending / Push…`, `Push is disabled without an upstream`, `Commit pending prompts…`, `Push calls catalog_push and refreshes`, `a push failure renders the git stderr…`: `await openPersonalChip();` before the first `assets-repo-status` / `assets-commit-pending` / `assets-push` lookup. In `Push calls catalog_push and refreshes`, re-open the chip before the final assertion (`await openPersonalChip();` then the `waitFor` on `assets-repo-status`), since acting closes the popover.
- `Delete asks for confirmation…` ends with `expect(screen.getByText('Select an asset.')).toBeTruthy();` unchanged (the Inspector's empty line).
- `does not reload on a visibility flip…`: `await screen.findByTestId('assets-new');` becomes `await screen.findByTestId('assets-sync');`.

In `src/lib/hub_disabled.test.ts` (`the asset catalog on a hub client`):

- `asks the hub once…`: `findByTestId('assets-remote-note')` → `findByTestId('assets-readonly')`.
- `refused: the read-only overview…` becomes:

```ts
  it('refused: the read-only overview, one scope chip with how to get the grant, none of the controls', async () => {
    hubStatus.set(remote);
    refusing();
    render(AssetsPanel, { props: { visible: true } });
    const chip = await screen.findByTestId('assets-readonly');
    expect(chip.textContent).toContain('read-only');
    await fireEvent.click(chip);
    expect(screen.getByTestId('assets-grant-cmd').textContent).toContain('fleet-hub client grant');
    // E_FORBIDDEN is the ordinary answer, not an error to show.
    expect(screen.queryByTestId('assets-grant-error')).toBeNull();
    expect(screen.queryByTestId('assets-sync')).toBeNull();
    expect(screen.queryByTestId('assets-secrets')).toBeNull();
    expect(screen.queryByTestId('assets-setup')).toBeNull();
  });
```

- `shows the hub's catalog read-only…` and `scans the hosts through the hub…`: after `render(…)` add `await fireEvent.click(await screen.findByTestId('assets-rail-library'));` (the row is still a `DIV`; its `title` still names `mac: drifted`; `assets-head` is the footer chip).
- `granted: the full panel onto the hub's catalog…`: `expect(screen.queryByTestId('assets-remote-note')).toBeNull();` → `expect(screen.queryByTestId('assets-readonly')).toBeNull();`, and open the Library before `screen.getByTestId('assets-new')` / `assets-import`.
- Import `fireEvent` in that file if it is not imported yet.

Add `'AssetsPanel.svelte'` is already in `GUARDED`.

- [ ] **Step 8: Run the whole frontend suite**

Run: `pnpm test` — Expected: PASS (all files; `AssetsPanel.test.ts` and `hub_disabled.test.ts` with the moves above).
Run: `pnpm check` — Expected: 0 errors.

- [ ] **Step 9: Commit**

```bash
git add src/lib/Icon.svelte src/lib/AssetsRail.svelte src/lib/AssetsWorkspace.svelte src/lib/AssetsWorkspace.test.ts \
  src/lib/AssetsPanel.svelte src/lib/AssetsPanel.test.ts src/lib/hub_disabled.test.ts src/lib/assets_tokens.test.ts
git commit -m "feat(assets-ui): the Assets workspace replaces the twelve-button toolbar

A rail (Inbox, Library, Secrets), a sentence header with the token query, a
tabbed Inspector and a footer of catalog chips. Sync fleet is the one
primary button; Rescan, New asset, Import and Lint all are quiet; pull,
commit and push moved to the personal catalog chip. An ungranted hub client
gets the same views read-only with one scope chip instead of the paragraph."
```

---

### Task 12: The keyboard, then verification and docs (R22, R29)

**Files:**
- Modify: `src/lib/AssetsWorkspace.svelte` (a keydown handler), `src/lib/AssetsWorkspace.test.ts`
- Modify: `CLAUDE.md` (an *Assets M5* paragraph after *Assets M4*)

**Interfaces:**
- Consumes: `isEditable` (`terminal_keys.ts`); `QueryInput.focus()` (Task 8); `editNonce` / `AssetInspector` (Task 9).
- Produces: the keymap of R22 on the workspace root.

- [ ] **Step 1: Write the failing keyboard tests**

Append to `src/lib/AssetsWorkspace.test.ts`:

```ts
describe('AssetsWorkspace keyboard', () => {
  const rowKey = () => (document.activeElement as HTMLElement | null)?.getAttribute('data-row-key');

  it('j/k move focus through the rows in display order; Enter selects', async () => {
    render(AssetsWorkspace, handlers());
    const list = screen.getByTestId('assets-list');
    list.focus();
    await fireEvent.keyDown(list, { key: 'j' });
    expect(rowKey()).toBe('asset:personal:skill/edited');
    await fireEvent.keyDown(document.activeElement!, { key: 'j' });
    expect(rowKey()).toBe('identity:skill/fresh');
    await fireEvent.keyDown(document.activeElement!, { key: 'k' });
    expect(rowKey()).toBe('asset:personal:skill/edited');
    await fireEvent.click(document.activeElement!);
    expect(screen.getByTestId('inbox-row-asset:personal:skill/edited').getAttribute('aria-current')).toBe('true');
  });

  it('s syncs the focused asset, a adopts the focused identity, ⌘↵ runs the primary', async () => {
    const h = handlers();
    render(AssetsWorkspace, h);
    const list = screen.getByTestId('assets-list');
    list.focus();
    await fireEvent.keyDown(list, { key: 'j' });
    await fireEvent.keyDown(document.activeElement!, { key: 's' });
    expect(h.onsync).toHaveBeenCalledWith({ kind: 'skill', name: 'edited' });
    await fireEvent.keyDown(document.activeElement!, { key: 'j' });
    await fireEvent.keyDown(document.activeElement!, { key: 'a' });
    expect(h.onimport).toHaveBeenCalledWith(expect.objectContaining({ kind: 'skill', name: 'fresh' }));
    await fireEvent.keyDown(document.activeElement!, { key: 'Enter', metaKey: true });
    expect(h.onsync).toHaveBeenLastCalledWith({});
  });

  it('/ focuses the query; keys typed there are the field’s', async () => {
    const h = handlers();
    render(AssetsWorkspace, h);
    const list = screen.getByTestId('assets-list');
    list.focus();
    await fireEvent.keyDown(list, { key: '/' });
    const q = screen.getByTestId('assets-query');
    expect(document.activeElement).toBe(q);
    await fireEvent.keyDown(q, { key: 's' });
    expect(h.onsync).not.toHaveBeenCalled();
  });

  it('read-only: moving and selecting only', async () => {
    const h = handlers();
    render(AssetsWorkspace, { ...h, readOnly: true });
    const list = screen.getByTestId('assets-list');
    list.focus();
    await fireEvent.keyDown(list, { key: 'j' });
    expect(rowKey()).toBe('asset:personal:skill/edited');
    await fireEvent.keyDown(document.activeElement!, { key: 's' });
    await fireEvent.keyDown(document.activeElement!, { key: 'Enter', ctrlKey: true });
    expect(h.onsync).not.toHaveBeenCalled();
  });
});
```

- [ ] **Step 2: Run them to verify they fail**

Run: `pnpm exec vitest run src/lib/AssetsWorkspace.test.ts`
Expected: FAIL — `rowKey()` is `null` (no handler moves focus).

- [ ] **Step 3: Add the handler**

In `AssetsWorkspace.svelte`'s script: `import { isEditable } from './terminal_keys';` and

```ts
  // Rulings R22. Focus moves between rows (the DOM order is the display
  // order, folded rows excluded); Space/Enter select natively (a static
  // read-only row handles both itself).
  function rows(): HTMLElement[] {
    return listEl ? [...listEl.querySelectorAll<HTMLElement>('[data-row-key]')] : [];
  }
  function focusedKey(): string | null {
    const el = document.activeElement as HTMLElement | null;
    return el && listEl?.contains(el) ? (el.dataset.rowKey ?? null) : null;
  }
  function move(delta: 1 | -1) {
    const all = rows();
    if (!all.length) return;
    const at = all.findIndex((r) => r.dataset.rowKey === (focusedKey() ?? selectedKey));
    const next = at < 0 ? (delta > 0 ? 0 : all.length - 1) : Math.min(all.length - 1, Math.max(0, at + delta));
    all[next].focus();
    all[next].scrollIntoView?.({ block: 'nearest' });
  }
  function onKeydown(e: KeyboardEvent) {
    const target = e.target as HTMLElement | null;
    if (e.defaultPrevented || target?.closest?.('dialog')) return;
    if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
      if (!readOnly && busy === '' && !isEditable(target)) {
        e.preventDefault();
        onsync({});
      }
      return;
    }
    if (e.metaKey || e.ctrlKey || e.altKey || isEditable(target)) return;
    const key = focusedKey() ?? selectedKey;
    const sel = key ? parseKey(key) : null;
    switch (e.key) {
      case 'j':
      case 'ArrowDown':
        e.preventDefault();
        move(1);
        break;
      case 'k':
      case 'ArrowUp':
        e.preventDefault();
        move(-1);
        break;
      case '/':
        e.preventDefault();
        queryEl?.focus();
        break;
      case 'a': {
        if (readOnly || sel?.type !== 'identity' || !listing) break;
        const id = identitiesOf(listing).find((i) => i.kind === sel.kind && i.name === sel.name);
        if (id) {
          e.preventDefault();
          onimport(id);
        }
        break;
      }
      case 's':
        if (readOnly || sel?.type !== 'asset' || busy !== '') break;
        e.preventDefault();
        onsync({ kind: sel.kind, name: sel.name });
        break;
      case 'e':
        if (readOnly || sel?.type !== 'asset' || sel.catalog !== PERSONAL || !key) break;
        e.preventDefault();
        selectedKey = key;
        editNonce += 1;
        break;
      // `i` (ignore) is a card verb — reject_item — bound with the cards in M6.
    }
  }
```

and put `onkeydown={onKeydown}` plus `role="group" aria-label="Assets workspace"` on the root `<div class="ws">`, with `<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->` on the line above it (the handler only delegates keys from the rows and fields inside; nothing on the group itself is a control).

- [ ] **Step 4: Run the tests**

Run: `pnpm exec vitest run src/lib/AssetsWorkspace.test.ts` — Expected: PASS.
Run: `pnpm test` and `pnpm check` — Expected: PASS, 0 errors.

- [ ] **Step 5: Document M5 in `CLAUDE.md`**

After the *Assets M4 — changesets* bullet of *Architecture*, add:

```markdown
- **Assets M5 — the workspace shell** (plan
  `docs/superpowers/plans/2026-10-04-assets-m5-workspace.md`): a manifest
  entry records the sha256 of every file it wrote (`file_hashes`), so
  `ManifestEntry::host_copy` tells a copy fleet left untouched from one a
  person edited; rule 4 of the planner turns an edited copy into an
  `overwrite` (never an `update`), and Rollout/SB6 (`OpFilter::Additive`,
  `action_allowed`) update only a verified-untouched copy. Migration 096
  stores `asset_inventory.drift_side` (`host` | `catalog`; also on
  `HostState`): a copy only behind its catalog opens no Drift card and SB6
  brings it up (`sb6_due`). Migration 097 stamps
  `changeset_items.decided_at`; `rejected_rollouts` orders by it. Slug
  collisions in a Bootstrap card need a look; the reconcile pass and SB6
  borrow the registry instead of cloning it, `changesets::list` computes
  undoability once (`undoable_ids`), untouched withdrawn cards older than a
  week are pruned; `last_sync` prefers a person's run. `list_assets` spans
  every loaded catalog (`AssetSummary.catalog`, host states per catalog) for
  the desktop, the master and unbound full devices, personal-only for
  everyone else; `catalog_admin { asset_history }` lists an asset's commits.
  Four read-only desktop commands route to the hub (`catalog_list_catalogs`,
  `catalog_list_changesets`, `catalog_repo_status_in`,
  `catalog_asset_history`); card verbs stay M6. Frontend: `AssetsPanel`
  keeps loading, probing and every dialog and renders `AssetsWorkspace`
  (rail Inbox/Library/Secrets, a sentence header, `QueryInput`, the
  Inbox's sections with open cards read-only, `AssetList` as the Library,
  the tabbed `AssetInspector` over `AssetDetail`'s `section`s and History,
  a footer of `CatalogChip`s, `auto` and `JobChip`); one `Badge`, `HostStrip`
  states, no hex colour in Assets components (`assets_tokens.test.ts`);
  keyboard `j/k`, `/`, `a`, `s`, `e`, `⌘↵`.
```

- [ ] **Step 6: Full verification (on mercury for Rust)**

Run, in order (Rust through `mercury-run`):
- `cargo fmt --all --check` — clean.
- `cargo fleet-lint` — no warnings.
- `cargo test --workspace` — PASS (re-run any known flake alone).
- `REGEN_HUB_VERDICTS=1 cargo fleet-test -- verdict_gen` and `REGEN_DOCS=1 cargo fleet-test -- reference_is_current` — no further diff.
- `cargo fleet-test -- mcp::tools::tests::the_served_definition_budget_stays_bounded` — PASS.
- `pnpm install --frozen-lockfile && pnpm test && pnpm check` — PASS, 0 errors.
- `scripts/ci-local.sh` — PASS.

Manual smoke on a standalone desktop (`pnpm tauri dev` on a machine with a display, never in a cloud session): open Assets → the Inbox shows the sentence and sections; `j`/`k` walk the rows; `/` then `kind:sk` completes; select an asset → Overview / Source / Hosts / History; the personal chip's popover pulls/pushes; Sync fleet opens the plan dialog; with a paired ungranted desktop, the read-only chip shows the grant command.

- [ ] **Step 7: Commit**

```bash
git add src/lib/AssetsWorkspace.svelte src/lib/AssetsWorkspace.test.ts CLAUDE.md
git commit -m "feat(assets-ui): keyboard for the workspace; document Assets M5

j/k move between rows, Space/Enter select, / opens the query, a adopts an
unmanaged identity, s syncs an asset, e opens it in Source, ⌘↵ runs Sync
fleet. Keys typed in a field or a dialog stay theirs; read-only windows get
movement and selection only."
```

---

## Self-review against the spec (M5)

| Spec item | Task |
|---|---|
| M5: `AssetsWorkspace` replaces the toolbar layout; the twelve buttons go | 11 (R17 table) |
| The only primary button is contextual (`Sync fleet` in M5) | 11 (R18), 12 (`⌘↵`) |
| Rail: Inbox (default), Layers, Hosts, Library, Secrets | 11 — Inbox, Library, Secrets; Layers and Hosts with their M6 views (R15) |
| Sentence header ("Fleet converged · 5/5 hosts · scan 3m") | 6 (`sentence`), 11 |
| Inbox: proposed cards on top, then sections; in-sync folds to one line | 6 (`buildInbox`), 7 (cards read-only, R14; sections R16) |
| Rows reuse identity rows and `HostStrip`, plus a `Badge` for scope/catalog | 5 (`HostStrip` states, `Badge`), 6 (`scopeBadge`), 7, 8 |
| Library: every asset once, grouped by kind; managed-elsewhere read-only | 4 (every catalog listed, R11), 8 (`AssetList` per catalog, `managed`, R20) |
| Inspector: a pane; `AssetDetail` → Overview + Hosts; `AssetEditor` in Source; History lists commits | 4 (`asset_history`), 9 (R19) |
| Inspector: Drift `DiffView` with Take / Restore | M6 (out of scope; the Drift section and the edited/behind wording land in 2, 6, 7) |
| Footer: a chip per catalog (HEAD, ahead; popover pull, push, commit), `auto: on|off`, `JobChip` | 4 (`catalog_list_catalogs`, `catalog_repo_status_in`), 10 (R24, R25) |
| `SyncPlanDialog` becomes the Rollout card; "Plan anyway" host-scoped | dialog kept modal with its host-scoped Plan anyway (R18); the Rollout card is M6 |
| Query: `QueryInput` on `/`, tokens `host:` `kind:` `state:` `layer:` `catalog:` `scope:`, case-insensitive, completion | 6 (`assets_query`), 8 (`QueryInput`), 12 (`/`) |
| Keyboard: `j/k`, `space`, `a`, `i`, `s`, `e`, `⌘↵` | 12 (R22; `i` is a card verb, bound in M6) |
| QuickSwitcher `asset` kind + commands | M6 (spec row) |
| Hub client without a grant: same views, no mutating controls, one scope chip | 11 (R23), 9 (read-only Inspector), 10 (read-only chips), 12 (read-only keys) |
| Visuals: tokens and `controls.css`; hex in `AssetDetail`/`SyncPlanDialog` → tokens; one `Badge` replaces `.chip`, `.op-badge`, `.count-chip`; never colour alone | 5 (+ the guard test extended by 7–11) |
| Accessibility: keyboard-operable, visible focus ring, dot `aria-label`s, no colour alone | 5 (dot labels), 7–11 (`:focus-visible`, roles: combobox/listbox, tablist/tab/tabpanel, status/progressbar, `aria-current`, `aria-expanded`), 12 |
| States: sync or scan running → `JobChip`, panel stays usable | 10 (R25) |
| States: host unreachable → hatched stale dot, nothing inferred removed | 5 (`stale`), 6 (`stale` set), 11 (from the hosts store) |
| Design coverage: Scope / catalog badge on rows | 5, 6, 7, 8 |
| Design coverage: Screen 2 Inbox sections | 6, 7 (diff and cards' actions M6) |
| Design coverage: Screen 4 Library | 8 |
| Testing (Vitest): `AssetsWorkspace` navigation, `QueryInput` tokens, `Badge`, the read-only chip | 11, 12 (`AssetsWorkspace.test.ts`), 8 (`QueryInput.test.ts`), 5 (`Badge.test.ts`), 11 (read-only chip); `ChangesetCard` and the Rollout card's Plan anyway are M6 |
| Hub CLI and MCP: "The Tauri commands route to the hub through `AdminCall` as today; the verdict table gains the new actions" | 4 (four read-only commands, rows, cases, regenerated verdicts; R13) |
| Milestone end: `cargo test --workspace`, `pnpm test`, `pnpm check` green | 12 (Step 6) |
| Carry 1: host edited vs host stale — signal, Drift rule, Update vs Overwrite | 1 (R1–R3), 2 (R4–R6) |
| Carry 2: slug collisions → needs a look | 3 (R7) |
| Carry 3: `get_asset_in` host states per catalog | 4 (R12) |
| Carry 4: `rejected_rollouts` by decision time | 3 (R8) |
| Carry 5: retention / per-tick cost | 3 (R9) |
| Carry 6: `SyncRunSummary.auto` in TS; last sync prefers person runs | 3 (R10), 6 (type), 10 (footer) |
| M4 safety invariants (nothing removes; only a person's restore overwrites; SB6 only on rolled-out layers; person verdicts stick) | 1 (`action_allowed` only narrows), 2 (`sb6_due` widens to verified-behind copies only), 3 (prune keeps every decided card) |
| Out of scope (M6/S3–S5): Layers/Hosts views, admissions UI, `ChangesetCard`, `DiffView`, Settings → Catalogs, QuickSwitcher, drafts, tests, Jev | not touched |
