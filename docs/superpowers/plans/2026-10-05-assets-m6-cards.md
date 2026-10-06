# Assets M6: cards you can act on, Layers and Hosts, DiffView, Settings → Catalogs, QuickSwitcher — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the M5 workspace act. Cards in the Inbox apply, undo, dismiss and reject items: Bootstrap, New, Drift with a `DiffView` and Take / Restore, Rollout with per-host "sync it yourself" lines, and new person-made layer cards. Add the Layers and Hosts rail views, with admissions as toggles and "why is it on oci?" provenance. Add Settings → Catalogs, and give QuickSwitcher an `asset` kind plus the Rescan / Sync fleet / Propose commands. Land the M5 carry list along the way.

**Architecture:**
- **Backend first** (Tasks 1–7, `fleet-core` + `src-tauri`):
  - Structured per-host outcomes on card items (migration 098 `changeset_items.outcome`).
  - `catalogs` on cards, and `withdrawn_at` (migration 099).
  - New-card slug collisions.
  - A person-made `layer` card: create / rename / move a member, applied and undone by the existing catalog-card machinery.
  - `catalog_admin { drift_diff }`, which returns the catalog and host text of one drifted asset's files.
  - Desktop commands for every card verb, the catalog set, per-catalog layers, host provenance and the diff. Each new command gets its verdict row and routing case.
  - A `catalog` page resource.
- **Frontend after that** (Tasks 8–15, Svelte 5):
  - The data layer, then `ChangesetCard`.
  - The Drift panel over a tokenised `DiffView` with a small line-diff helper.
  - The Rollout review and a non-modal sync plan view in place of `SyncPlanDialog`.
  - The Layers and Hosts views, and QuickSwitcher.
  - The UI carries, verification and docs.

**Tech Stack:** Rust (fleet-core: rusqlite, rmcp, serde; src-tauri: Tauri 2 commands, `backend::verdicts`, page resources), SQLite migrations 098/099, Svelte 5 (runes), TypeScript, Vitest + @testing-library/svelte, `controls.css` tokens.

**Spec:** `docs/superpowers/specs/2026-09-30-assets-s1b-s2-design.md`. It is binding in these parts:
- The milestone **M6** row: "Layers and Hosts views, admissions UI, `ChangesetCard` (Bootstrap, New, Drift, Rollout), `DiffView`, Settings → Catalogs, QuickSwitcher".
- The *Design coverage* rows marked M6.
- *Changesets (the cards)*, *Hub CLI and MCP*, and *Workspace shell*.

Also binding:
- The parent `docs/superpowers/specs/2026-09-29-assets-workspace-design.md`.
- The mockups `docs/superpowers/specs/2026-09-29-assets-workspace-mockups.html`. Screen 1 (First run: Bootstrap and Rollout cards), screen 2 (Inbox: drift diff with Take / Restore, new-asset suggestion) and screen 3 (Layers and hosts) are the visual reference.
- The M5 plan `docs/superpowers/plans/2026-10-04-assets-m5-workspace.md`, whose rulings R14, R15, R18, R22 and R24 defer work to M6.
- The M5 final review's carry list, in `.superpowers/sdd/2026-10-04-assets-m5-workspace/final-review.md` of the M5 worktree, quoted under *Carry list*.

## Global Constraints

**Spec text (quoted)**
- Spec, M6 row: "Layers and Hosts views, admissions UI, `ChangesetCard` (Bootstrap, New, Drift, Rollout), `DiffView`, Settings → Catalogs, QuickSwitcher".
- Spec, Changesets: "**Drift** … per host: `take_host` (import that copy into its catalog) or `restore` (sync the catalog copy, backup) — always one item at a time". "**Rollout apply** = `plan_sync` (hosts of the card) + `apply_sync`. `overwrite` and `remove` never go through a card." "**Undo** = `git revert` of the card's commits (one per catalog) + restore the `host_layers` snapshot; allowed only for the latest applied card in each catalog it touched. It never touches hosts."
- Spec, Workspace shell:
  - "**Layers:** layers grouped by catalog with members and footprint; create / rename / move produce cards, never direct commits."
  - "**Hosts (inside Assets):** per host its org, role per catalog, accepted catalogs (toggle = admission), effective set with provenance ("on oci via layer core from personal")."
  - "Drift shows a `DiffView` with Take / Restore."
  - "The modal `SyncPlanDialog` becomes the Rollout card; its "Plan anyway" stays host-scoped."
  - "**Keyboard:** `j/k`, `space`, `a` adopt, `i` ignore, `s` sync, `e` edit, `⌘↵` primary. `QuickSwitcher` gains an `asset` kind and the commands Rescan, Sync fleet, Propose."
  - "**Settings → Catalogs:** a declarative `master_detail` page with a `catalog` resource, like Organisations: add (name, path, remote, org), the deploy-key hint (GitHub SSO orgs need an admin to allow it), grants."
  - "**Hub client without a grant:** the same views without mutating controls, and one scope chip."
  - "State is never colour alone."
- Spec, Hub CLI and MCP: "Every mutating action checks the caller's grant **for the catalogs it touches** (`may_admin_catalog(caller, catalog_id)`); per-host tokens never pass." "The Tauri commands route to the hub through `AdminCall` as today; the verdict table gains the new actions."
- Mockup notes: "Drift shows a diff with two choices — Take the host version or restore the catalog one. Nothing is overwritten silently." "Ignore sticks until the file's content changes." "Hosts accept catalogs explicitly … Private assets never reach an org host." "Why is it here? Provenance for every asset on every host: role, context, layer."

**Safety invariants carried from M4 and M5** (never weakened)
- Nothing ever removes from a host through a card or SB6.
- An `overwrite` happens only through a Drift restore a person picked, or through the person's own Sync.
- A Rollout or SB6 updates a copy only when the planner verified it `Unchanged`. The same holds for any action that deletes a moved asset's old location.
- A `person` verdict is never replaced by another decider.
- A card's apply writes and commits only the paths it claimed (`Progress::claim`) and resets only those on failure.
- Per-host tokens never reach `changesets` or `catalog_admin`.

**Migrations**
- **098** `098_changeset_item_outcome.sql` and **099** `099_changeset_withdrawn_at.sql`.
- On 2026-10-05 `crates/fleet-core/migrations/` ends at `097_changeset_item_decided_at.sql`. If `main` gains a migration first, renumber everything that carries the number: the file, its `MIGRATIONS` entry, its `schema_version` insert, its guard's name and its test names.
- Both migrations are `ALTER TABLE … ADD COLUMN`, so both carry an `already_applied` guard, exactly like 097 (`changeset_items_has_decided_at`).

**Rust rules**
- Lock rule: **registry → store is allowed; store → registry never.** Read store rows under one guard, drop it, then take the registry. Nothing inside a `registry::with_catalogs` closure calls `registry::` again.
- Never hold the `Store` guard across an `.await`. Card writes take `changesets::authoring_lock()` (APPLY_LOCK) first, as every authoring write does.
- New serialized fields are `#[serde(default)]`, plus `skip_serializing_if` where an absent field must read as before. An M5 desktop talking to an M6 hub, or the reverse, must not fail to decode.
- **Contract:** no bump. `changesets` and `catalog_admin` have been served since contract revision 7. A new action (`propose_layer`, `drift_diff`) sent to an older hub is refused with `E_INVALID`, the R13 precedent. The desktop turns that into "the hub is older than this desktop — update it" (Task 8, `olderHubWords`).
- The served-definition budget (`mcp::tools::tests::the_served_definition_budget_stays_bounded`, `BUDGET_BYTES`, currently **72_927**):
  - Tasks 3 and 4 change parameter docs. Each re-measures and sets the constant to the printed measurement + 100, and the commit message names what was measured.
  - Each also regenerates `docs/control-api-reference.md` with `REGEN_DOCS=1 cargo fleet-test -- reference_is_current`.
- Hub routing: a new Tauri command means all of these:
  - a `VERDICTS` row;
  - a `routed::` body that routes **by its own command name**;
  - a case in `tests_routing.rs`;
  - registration in `src-tauri/src/lib.rs`;
  - `REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen` (it fails once on purpose, then passes);
  - for a mutating command, its name in `ROUTED_ACTIONS` in `src/lib/hub.ts`.
- Page resources:
  - Regenerate with `REGEN_PAGE_DOCS=1 cargo fleet-test -- page_docs_are_current` (→ `src/lib/pages/registry.generated.json`, `docs/page-spec.schema.json`, `docs/page-catalog.json`).
  - Then run `REGEN_SETTINGS_DOCS=1 cargo fleet-test -- settings_docs_are_current` if it reports drift.
- Tests that touch the catalog registry or `HOME` take `crate::service::catalog::lock_registry_for_test()`.
- Shell-quoting only through `crate::shell::quote`. Child processes only through `fleet_core::proc::command` / `std_command`. Remote scripts only through `inventory::run_host_script`.

**Frontend rules**
- Svelte 5 runes only (`$props`, `$state`, `$derived`, `$effect`, `$bindable`). Stores stay `svelte/store` writables in `src/lib/*.ts`.
- Controls use the `controls.css` classes: `.btn`, `.btn--primary`, `.btn--quiet`, `.btn--chip`, `.is-bounded`.
- Colours only through tokens (`--usage-ok|warn|crit`, `--accent`, `--accent-soft`, `--control-*`, `--fg-muted`, `--border`, `--bg-pane`). No hex, `rgb(` or `hsl(` literal in any guarded Assets component's `<style>`; `src/lib/assets_tokens.test.ts` holds it, and Task 15 widens the regex.
- `prefers-reduced-motion` is respected for every animation.
- The workspace has exactly **one** `.btn--primary` in view at a time (M5 test `AssetsWorkspace.test.ts:49-61`). A card's own primary lives inside the card and counts only for the selected card. Task 9 rewrites that test to "one primary per region: header or selected card".
- Every new component gets its own `*.test.ts`. `pnpm test` and `pnpm check` are green at the end of every task that touches the frontend.

**Validation, platform and git**
- Validation follows the ladder in `CLAUDE.md`:
  - `cargo fleet-fast-check` while editing;
  - `cargo fleet-check` at checkpoints;
  - `cargo fleet-test -- <filter>` for touched tests;
  - `cargo fmt --all --check` and `cargo fleet-lint` before each commit;
  - `cargo test --workspace`, `pnpm test` and `pnpm check` before the PR.
- **Rust builds and tests run on mercury through the `mercury-run` skill**, because the local Mac cargo is unreliable. Never run more than two mercury builds at once. `pnpm test` and `pnpm check` run locally.
- `cargo test` takes ONE name filter per command.
- Known unrelated flakes: `rewind::tests::the_removal_script_leaves_a_tree_a_live_pane_is_in`, `work::scale_tests::*`, the `CHAIN_BUDGET` tests in `store/schema/tests_upgrade.rs`, `service::add_project`, `trackers::sync …poison_view…`. Re-run a suspected flake alone before touching anything.
- Windows CI: a test helper used only by `#[cfg(unix)]` tests is itself `#[cfg(unix)]`. M2 hit this: clippy `dead_code` on Windows.
- Git:
  - Branch `feat/assets-m6-cards` from `origin/main` (worktree `cf-m6`, cut at `31d3c515`).
  - One commit per task.
  - Never rebase, never force-push; merge `origin/main` for conflicts.
  - No attribution lines in commits.

## Pre-flight (verify before Task 1)

Every task leans on these facts of the merged code, read on 2026-10-05 at `31d3c515`. Check each one. If a fact differs, stop and re-rule before coding.

| Fact | Where |
|---|---|
| Latest migration is 097 with guard `changeset_items_has_decided_at`; test pattern `migration_097_adds_decided_at_as_null_and_is_safe_to_rerun` with `store_at_version(96)` | `crates/fleet-core/src/store/schema.rs:419`, `:1062`, `:4531` |
| `ITEM_COLS` / `item_row` / `insert_items`; `CARD_COLS` / `card_row`; `withdraw_changeset(id, error)`; `prune_withdrawn_changesets(created_before)` keyed on `created_at` | `crates/fleet-core/src/store/changesets.rs:123-160`, `:440`, `:457` |
| `CardKind {Bootstrap, New, Drift, Rollout}` with `ALL`, `as_str`, `parse`; `ItemAction` 7 variants; `ItemParams` all-optional; `ItemView`, `ChangesetView`, `ChangesetSummary` (no `catalogs`) | `crates/fleet-core/src/service/catalog/changesets/mod.rs:47-325` |
| `changes_catalog` matches `"bootstrap" \| "new"` and drift-with-take_host; `undoable_ids`, `view`, `list` | `changesets/mod.rs:374-555` |
| `apply_held` dispatch: restore alone → `apply_restore`; rollout → `apply_rollout`; else `apply_catalog` → `run_steps` (2a imports, 2b layer files `layer_with_members`, 2c scope, 3 host_layers, 4 commit per catalog) | `changesets/apply.rs:77-104`, `:323-420`, `:542-990`, `:1054` |
| `held_by_host_copy` returns `Vec<String>`; drops an `Overwrite` whose `host_copy` is `Unverified` without a line (the T1 carry); `HostOutcome {applied, nothing, held, failed, skipped}`; `finish_host_card` writes `skipped: …` into `error` | `changesets/apply.rs:1476-1560`, `:1853-1912` |
| `Action.host_copy` is `#[serde(skip)]` | `crates/fleet-core/src/service/catalog/sync/plan.rs:141-142` |
| Reconcile withdraws every open card whose subject the pass did not produce, except Rollout cards and cards with an applied item | `changesets/reconcile.rs:312-326` |
| `rules::subject_of(kind, items)` matches all four kinds; `slug_collisions` is used only by the Bootstrap card; `new_card` has no collision check | `changesets/rules.rs:93-125`, `:414-442`, `:654-692` |
| `ChangesetsParams {action, id, positions, confirm_nonce}`; handler `changesets` with the `list\|propose\|apply\|undo\|dismiss\|reject_item` match and `check_card_grants` | `crates/fleet-core/src/mcp/tools/params.rs:1082-1096`; `mcp/tools/assets.rs:314-450`, `:689-734` |
| `AdminCall` via `admin_calls!`; `AdminCall::touches` (`Touches::{Nothing, MasterOnly, Catalog}`); `CatalogAdminParams {action, args, confirm_nonce, catalog}` | `crates/fleet-core/src/service/catalog/admin.rs:116-188`, `:271-322`; `params.rs:1058-1080` |
| `import_host_into` reads a remote host with `inventory::run_host_script(ssh, host, script)`; `harness.render(asset)` gives a `RenderPlan { files: [{path, bytes}], … }` | `crates/fleet-core/src/service/catalog/mod.rs:601-640`; `sync/plan.rs:681` |
| MCP `resolve_preview` projects a `Resolution` to `{provenance, excluded, refused, withheld, held_back, assets:[{kind,name,version}]}` | `mcp/tools/assets.rs:504-538` |
| `catalogs::{add_catalog(AddCatalogArgs), remove_catalog(&name), admit(&host,&catalog), unadmit(..), list_catalogs}` | `crates/fleet-core/src/service/catalog/catalogs.rs:13-206` |
| Tauri: `catalog_list_changesets` pattern (command + `routed::` + verdict row + routing case `catalog_admin_cases()`); `in_catalog(call, name)` adds a top-level `catalog` | `src-tauri/src/commands/assets.rs:342`, `:942-980`; `src-tauri/src/backend/verdicts.rs:1274-1278`; `src-tauri/src/backend/tests_routing.rs:4927` |
| `RESOURCES = &[ORG, TRACKER]`; `OptionSource {Hosts, Trackers}`; `ResourceType`, `FieldSpec`, `ActionSpec`, `Bind` | `crates/fleet-core/src/pages/resources.rs:29`, `:46`, `:350-380`, `:565` |
| Frontend: `WorkspaceView = 'inbox' \| 'library'`, `RAIL_VIEWS`, stale `ChangesetItem` | `src/lib/assets_workspace.ts:46-76` |
| Frontend: card rows are read-only buttons; test asserts no inner buttons | `src/lib/AssetsInbox.svelte:90-109`; `src/lib/AssetsInbox.test.ts:33` |
| Frontend: keyboard switch; `i` unbound with a test asserting it | `src/lib/AssetsWorkspace.svelte:198-258`; `src/lib/AssetsWorkspace.test.ts:415-428` |
| Frontend: `SyncPlanDialog` props and `planAnyway`; opened from `AssetsPanel.requestSync` | `src/lib/SyncPlanDialog.svelte:18-65`; `src/lib/AssetsPanel.svelte:309-322`, `:434-444` |
| Frontend: `DiffView` takes `{diff: string}` and exports `parseUnifiedDiff`; hex at 114–123 | `src/lib/DiffView.svelte` |
| Frontend: QuickSwitcher kinds `'session'\|'project'\|'host'\|'ticket'\|'lookup'`; `pick` dispatch; `hostsViewRequest` precedent | `src/lib/quick_switcher.ts:35`; `src/lib/QuickSwitcher.svelte:194-218`; `src/lib/app_views.ts:16-23` |
| Frontend: `ROUTED_ACTIONS`; `hubNextStep` words `E_CONFIRM_REQUIRED` | `src/lib/hub.ts:297`, `:405-433` |

## Rulings

These cover where the spec is silent, where the carry list asked for a decision, or where the code forced one. Each ruling states what it costs if it turns out wrong.

**R1 — Held lines become data.** Migration 098 adds `changeset_items.outcome TEXT`: JSON `ItemOutcome { held: [HeldLine {kind, name, why}], note: Option<String> }`, written per host item by `finish_host_card`.
- `why` is one of:
  - `edited` (`HostCopy::Edited`);
  - `unverified` (`HostCopy::Unverified`, a pre-M5 entry);
  - `differs` (no host-copy verdict).
- `note` carries a skipped host's or a failed host's line.
- **Every** `Overwrite` of a card's own asset under the Additive filter is held. This closes the T1 carry: a pre-M5 copy that differs while the catalog is unchanged used to drop silently and count as applied.
- The card's `error` text stays as it is, for the MCP and mobile readers.
- `Action.host_copy` is now serialized (`#[serde(default, skip_serializing_if = "Option::is_none")]`, snake_case), so the Rollout review in TS can mirror `action_allowed`.
- Cost: one more column. An M5 hub sends no `outcome`, so its cards show the old `error` text instead.

**R2 — `catalogs` on cards.** `ChangesetSummary.catalogs` and `ChangesetView.catalogs` list the sorted, unique names of every item's `catalog_id` (a `hide`-only card lists none). The `catalog:` query token filters cards by it. Cost: one join per list, over rows already read.

**R3 — Withdrawal time.**
- Migration 099 adds `changesets.withdrawn_at INTEGER` (unix seconds). `withdraw_changeset` stamps it.
- `prune_withdrawn_changesets(before)` deletes on `COALESCE(withdrawn_at, created_at) < before`. The fallback covers cards withdrawn before 099.
- `ChangesetSummary.withdrawn: bool` (`dismissed` and an error starting `withdrawn:`), so the UI stops parsing the error.
- Cost: none beyond the column.

**R4 — New-card slug collisions.** `rules::new_cards` demotes a New card to "needs a look" with the reason `imports as {slug}, as {other} does` in two cases:
- its `(destination catalog, kind, slugify(name))` equals another New candidate's in the same pass;
- the destination catalog already holds an asset with that slug under another name.

`CatalogFacts` gains `slugs: BTreeSet<(String, String)>` (kind, slug), filled from the registry by `reconcile::gather`. Cost: one set per catalog per pass.

**R5 — Layer cards.** The spec's "create / rename / move produce cards, never direct commits".
- **The kind.** A new `CardKind::Layer` (`"layer"`), decider `person`, with three new `ItemAction`s:
  - `create_layer` (params `axis`, `description`, `members`);
  - `rename_layer` (params `to`);
  - `move_member` (params `member`, `layer` = from, `to`).
- **Proposing.** A person proposes one with `changesets { action: "propose_layer", change: LayerChange }`. That makes one proposed card of one item, validated at propose time with `E_INVALID` naming the problem. The person then applies it like any card. The UI offers "Apply now" on the card it just made.
- **Apply.** Apply runs through `apply_catalog` (a new step 2d), claiming every path it writes or deletes. Rename rewrites `layers/<old>.yaml` → `layers/<new>.yaml`, rewrites every `extends: <old>` in that catalog, and renames that catalog's `host_layers` rows in step 3, under the existing snapshot.
- **Undo.** Undo is the existing revert plus snapshot. `changes_catalog` gains `"layer"`.
- **Reconcile.** A layer card is never withdrawn by reconcile; it has no rule subject. A person dismisses it.
- **Grants.** The card's item carries the catalog, so `check_card_grants` asks for a grant on it.
- **Older hubs.** One refuses `propose_layer` with `E_INVALID`, which Task 8 words.
- Cost: three apply branches, each tested for claim, commit and undo.

**R6 — Out of scope in M6:**
- moving a layer to another catalog;
- splitting a layer;
- renaming a group of a still-proposed Bootstrap card;
- the mockup's "Dry run" button.

The proposed Bootstrap card's Inspector offers **Skip this layer** (reject that group's pending items) and per-item reject. **Edit layers** opens the Layers view. "Dry run" is the card itself: it lists everything, and nothing happens until Apply.

Cost: someone who dislikes a proposed layer's name applies it, then renames it — one more card and commit.

**R7 — The drift diff is data, not a diff.** `catalog_admin { drift_diff, args: {host_alias, kind, name, harness?} }`, with the tool's `catalog`, returns `DriftDiff { host_alias, harness, files: [DriftFile {path, catalog: Option<String>, host: Option<String>, binary, truncated}] }`.
- **Catalog side.** The asset rendered for that harness **without** secret substitution, so `${NAME}` placeholders stay.
- **Host side.** The same paths read over SSH with a script that base64-dumps each named path, or locally for `local`. Each side is capped at 256 KiB; non-UTF-8 is reported as `binary`.
- **Never read.** Only `RenderPlan.files` are read. Config merges (`.claude.json` MCP entries and the like) are never read, so no host secret value travels. An asset that renders only merges answers `files: []`, and the UI says the diff of a config entry is not shown.
- **The diff itself.** TS computes the unified diff (`src/lib/line_diff.ts`, Myers over lines) and feeds the tokenised `DiffView`.
- **Gate.** `Touches::Catalog(name)`: a grant on that catalog. Per-host tokens never reach it.
- **Why.** This adds no Rust diff dependency, and the backend stays a reader.
- Cost: the client diff is O((N+M)·D), fine at 256 KiB.

**R8 — Desktop card verbs.**
- **Commands:** `catalog_get_changeset {id}`, `catalog_apply_changeset {id, positions?}`, `catalog_undo_changeset {id}`, `catalog_dismiss_changeset {id}`, `catalog_reject_changeset_items {id, positions}`, `catalog_propose_changesets`, `catalog_propose_layer_change {change}`. Each is a verdict row `Routed{tool:"changesets"}`.
- **Local path.** The service functions, with no grant check and no confirm gate. The desktop owns its store, as every local catalog command does.
- **Confirm gate.** `confirm_nonce` is never sent, as with `catalog_apply_sync`. A hub with `mcp.confirm_destructive` answers `E_CONFIRM_REQUIRED` for a Rollout or Restore apply, and the UI shows `hubNextStep`'s words.
- Cost: on such hubs a person approves on the hub.

**R9 — Desktop catalog commands.**
- **Commands:** `catalog_add_catalog`, `catalog_remove_catalog`, `catalog_admit_catalog`, `catalog_unadmit_catalog`, `catalog_list_layers_in {name}`, `catalog_host_provenance {host_alias}`, `catalog_drift_diff {host_alias, kind, name, harness?, catalog?}`.
- **Provenance.** `catalog_host_provenance` routes to the MCP `resolve_preview` tool (already a slim projection). Locally it calls the same projection, extracted as `resolve::ResolutionView::of(&Resolution)`.
- **The others** route to `catalog_admin`.
- Cost: none; every one is an existing service function.

**R10 — Grants stay on the hub CLI.**
- The Catalogs page shows `granted` read-only. Its help gives the exact command: `fleet-hub client grant <client> assets --catalog <name>`.
- No grant command is added. Granting is a master-only security change, and a standalone desktop has no paired clients.
- Cost: one CLI step for the operator.

**R11 — Settings → Catalogs.** A `CATALOG` resource:
- **list:** `catalog_list_catalogs`.
- **create:** `catalog.add` → `catalog_add_catalog`. Params: name, repo path, remote URL, and org (a new `OptionSource::Orgs`). The org is required: `personal` already exists and is re-pointed with `fleet-hub catalog set`, so every catalog added here is an org's.
- **delete:** `catalog.remove` → `catalog_remove_catalog`. Confirm text: "Removes the catalog from fleet's config. Its checkout stays on disk; open cards on it are withdrawn."
- **fields:** `state` (a read-only `Choice` badge, the tracker pattern); `admitted` as `Items` (add with `OptionSource::Hosts` → `catalog_admit_catalog`, remove → `catalog_unadmit_catalog`); `granted` as `Items` with neither add nor remove.
- **help:** the deploy-key hint ("A GitHub org with SSO must allow the deploy key; an org admin does that once in the org's settings").
- No `update` action: re-adding a name re-points it.
- **On a hub client.** The page is read-only, with the verdict's refusal. `add` and `remove` are master-only on the hub (`Touches::MasterOnly`).
- Cost: no inline edit of a catalog's path.

**R12 — Cards in the Inbox and the Inspector.**
- **The Inbox.** Each open card renders as a `ChangesetCard` at the top of the Inbox:
  - the summary sentence and a decider badge;
  - for Bootstrap, New and Layer, a group table (group, count, decider, scope);
  - for Bootstrap and New, "needs a look" chips;
  - for Rollout, a host table of per-host held lines;
  - a footer of the primary verb, the secondary verbs and a note ("2 commits: personal, papayapos. One Undo. No host is touched.").
- **Selection.** Selecting a card (`card:<id>`) shows its full `ChangesetView` in the Inspector, with these tabs:
  - **Items**: every item, its state, a per-item ✕ reject, and Skip-this-group;
  - **Hosts** (Rollout only): per-host lines;
  - **Diff** (Drift only).
- **Applied cards.** An applied, undoable card shows as an "applied" banner row with **Undo** at the top of the Inbox while it stays among the recent cards. An Undo toast also follows every apply.
- Cost: cards take more vertical space than the M5 rows.

**R13 — Primary verbs.**

| Card | Primary verb |
|---|---|
| Bootstrap | `Adopt {n} as {m} layers` |
| New | `Adopt into {layer}`, or `Review` for a "needs a look" card (selects it) |
| Drift | none in the list: `Review diff` selects it; Take / Restore live in the Inspector |
| Rollout | `Roll out to {n} hosts` |
| Layer | `Apply` |

`⌘↵` runs the selected card's primary. With no card selected it runs `Sync fleet`, as in M5.

**R14 — Confirmation.**
- **Restore** asks first, in a `ConfirmDialog`: "Restore the catalog version of {kind}/{name} on {host}? The host copy is overwritten; a .fleet-bak copy is kept."
- **Rollout, Bootstrap, New, Layer and Take** apply without a dialog. Rollout is additive, and the other four are catalog-only and undoable.
- Cost: one click more for Restore, which is the only overwrite a card makes.

**R15 — Rollout review.**
- **Review plan** on a Rollout card plans each of the card's hosts **host-scoped** (`planSync({hostAlias})`, read-only) and narrows each to the card's assets in TS (`cardOwns`). It marks every action that `cardMayApply` refuses as "held — sync it yourself". `cardMayApply` mirrors R1/R3 `action_allowed` via the now-serialized `host_copy`.
- **Plan anyway** stays host-scoped.
- The review never applies; **Roll out** does, through the card.
- Cost: N plan calls for N hosts.

**R16 — `SyncPlanDialog` becomes `SyncPlanView`.**
- It is the same body without the `Modal`. While a plan is open it takes the place of the main list, with a "Back" button and `Esc`.
- It hosts both the person's Sync (overwrites with a backup are allowed, as today) and the Rollout review (read-only, `mode: 'review'`).
- `SyncPlanDialog.svelte` is deleted; its tests move to `SyncPlanView.test.ts`.
- Cost: a full-width view instead of a modal.

**R17 — Layers view.**
- **List.** Layers grouped by catalog, from `catalog_list_layers_in` for each catalog `catalogStatuses` lists as loaded (personal included).
- **Footprint.** The hosts a layer reaches: an active row naming it, or a host whose active role layer `extends` it, transitively. Computed by `layerFootprint` in TS and drawn as a `HostStrip`.
- **Inspector.** Tabs Members and Hosts. Hosts carries the "Why is it on {host}?" chain: role → extends → … → the layer.
- **Header verbs:** New layer, Propose again (`catalog_propose_changesets`).
- **Inspector verbs:** Rename, Move member. Each creates an R5 card and jumps to it in the Inbox.
- Cost: none.

**R18 — Hosts view.**
- **Rows.** Each non-hidden host shows:
  - its org badge, or "no org";
  - its role per catalog, from that catalog's layer listing;
  - its accepted catalogs as toggles.
- **The toggles** follow `effective::acceptance`:
  - **personal**: always on and locked; "shared only" on an org host;
  - **the host's own org catalog**: on and locked ("via org");
  - **other org catalogs**: toggles that admit or unadmit, on org-less hosts only. On an org host they are disabled, and the title explains why.
- **Inspector.** "On {host}": `catalog_host_provenance` grouped by catalog, one line per asset ("via layer core from personal"), then refused and held-back lines.
- Read-only clients see no toggles.
- Cost: one provenance call per selected host.

**R19 — QuickSwitcher.**
- **Asset rows.** Kind `asset`: one row per asset in `$catalog` (label `kind/name`, meta `Assets`, description the catalog). Picking one opens Assets with it selected.
- **Command rows.** Kind `command`: Rescan assets, Sync fleet, Propose cards. Picking one opens Assets and runs it there.
- **The channel.** `requestAssetsView({ select?, command? })` in `app_views.ts`; App opens Assets; `AssetsPanel` consumes the request. A read-only client gets the command ignored, with the read-only chip already shown.
- Cost: none.

**R20 — UI carries:**
- **Tokens:** hex out of `ImportDialog`, `NewAssetDialog`, `SecretsPanel`, `AuthorSessionDialog`, `LintAllDialog` and `DiffView`. The guard covers them and catches `rgb(` / `hsl(` too.
- **Narrow layout:**
  - ≤ 1100 px: the rail collapses to icons (56 px).
  - ≤ 860 px: the Inspector stacks under the list.
- **Scoped keys:** `s` / `a` / `e` / `i` act only when focus is in the list or the Inspector.
- **Live region:** `JobChip` gets a persistent footer `role=status` region, empty when idle, so starts and ends are announced.
- **Secrets key:** `blockedSecretKeys` is keyed `<catalog>:<kind>/<name>`.
- Cost: none.

**R21 — Deferred.**
- **SB6 perf:** SB6 re-plans a held moved asset on every pass. The cost is bounded by the scan cadence, so it waits for a post-M7 follow-up, recorded in the ledger.
- **M7:** the spec's `docs/hub.md` catalog section.

## Carry list → where it lands

| Carry (M5 final review, "M6" rank) | Task |
|---|---|
| T1: a pre-M5 `Overwrite` is dropped without a held line | 1 (R1) |
| T7: `ChangesetSummary.catalogs` | 1 (R2) |
| T3 M2: retention counted from withdrawal time | 2 (R3) |
| T3 M8: New-card slug collisions across cards | 2 (R4) |
| Final review advice 5: a Rollout card shows pre-M5 and moved-asset holds as per-host "sync it yourself" lines | 1 (R1) + 11 |
| T5: hex in Import/NewAsset/Secrets/AuthorSession/LintAll; guard regex breadth | 15 (R20) |
| T10: persistent live region for JobChip | 11 (R20) |
| T10: `blockedSecretKeys` keyed without catalog | 15 (R20) |
| T11: narrow layout | 15 (R20) |
| T12: scope `s`/`a`/`e` (and `i`) to list/inspector focus | 15 (R20) |
| SB6 re-plans a held moved asset each pass (perf) | deferred (R21) |

## File Structure

**Backend (fleet-core).** All paths are under `crates/fleet-core/src`.

| File | Change | Task |
|---|---|---|
| `crates/fleet-core/migrations/098_changeset_item_outcome.sql` | new: `changeset_items.outcome` | 1 |
| `crates/fleet-core/migrations/099_changeset_withdrawn_at.sql` | new: `changesets.withdrawn_at` | 2 |
| `store/schema.rs` | MIGRATIONS 098/099, guards, migration tests | 1, 2 |
| `store/changesets.rs` | `outcome` on items (`ITEM_COLS`, `item_row`, `set_changeset_item_outcomes`); `withdrawn_at` on cards; prune by it | 1, 2 |
| `service/catalog/sync/plan.rs` | serialize `Action.host_copy` | 1 |
| `service/catalog/changesets/mod.rs` | `ItemOutcome`, `HeldLine`; `catalogs` + `withdrawn` on summary/view; `outcome` on `ItemView`; `CardKind::Layer`; three `ItemAction`s; `LayerChange`; `propose_layer` | 1, 2, 3 |
| `service/catalog/changesets/apply.rs` | structured held lines; Overwrite held; outcomes per host item; step 2d layer edits; rename in step 3 | 1, 3 |
| `service/catalog/changesets/rules.rs` | `CatalogFacts.slugs`; New-card collisions; `subject_of` for `Layer` | 2, 3 |
| `service/catalog/changesets/reconcile.rs` | fill `slugs`; never withdraw a `layer` card | 2, 3 |
| `service/catalog/changesets/layers.rs` | new: validation and the file edits of a layer card | 3 |
| `service/catalog/drift_diff.rs` | new: `drift_diff` | 4 |
| `service/catalog/admin.rs` | `AdminCall::DriftDiff`; `touches` | 4 |
| `service/catalog/resolve.rs` | `ResolutionView::of` (projection shared with MCP) | 6 |
| `mcp/tools/params.rs`, `mcp/tools/assets.rs` | `ChangesetsParams.change`; `propose_layer`; `resolve_preview` uses `ResolutionView` | 3, 6 |
| `mcp/tools/tests.rs` | `BUDGET_BYTES` | 3, 4 |
| `pages/resources.rs`, `crates/fleet-core/pages/settings.catalogs.json`, `crates/fleet-core/pages/settings.json`, `pages/mod.rs` | `CATALOG` resource, `OptionSource::Orgs`, the page and its link | 7 |

**Backend (src-tauri).**

| File | Change | Task |
|---|---|---|
| `src-tauri/src/commands/assets.rs` | 14 new commands + `routed::` bodies | 5, 6 |
| `src-tauri/src/lib.rs` | registration | 5, 6 |
| `src-tauri/src/backend/verdicts.rs` | 14 rows | 5, 6 |
| `src-tauri/src/backend/tests_routing.rs` | cases | 5, 6 |

**Frontend (src/lib).**

| File | Change | Task |
|---|---|---|
| `assets_workspace.ts` | wire types (`ChangesetView`, `ItemView`, `ItemOutcome`, `LayerChange`, `ResolutionView`, `DriftDiff`), wrappers, stores, `WorkspaceView` += layers/hosts | 8 |
| `assets_cards.ts` | new, pure: card primary/verbs/labels, `cardOwns`, `cardMayApply`, `olderHubWords` | 8 |
| `line_diff.ts` | new, pure: Myers line diff → unified text | 8 |
| `assets_layers.ts` | new, pure: `layerFootprint`, `whyChain`, `rolesByCatalog`, `acceptanceOf` | 8 |
| `ChangesetCard.svelte` | new | 9 |
| `ChangesetDetail.svelte` | new: the Inspector's card tabs | 9 |
| `AssetsInbox.svelte`, `AssetInspector.svelte`, `AssetsWorkspace.svelte`, `AssetsPanel.svelte` | cards wired; `i`; ⌘↵ | 9 |
| `DiffView.svelte` | tokens | 10 |
| `DriftPanel.svelte` | new: diff + Take / Restore | 10 |
| `SyncPlanView.svelte` | new, from `SyncPlanDialog.svelte` (deleted) | 11 |
| `AssetsFooter.svelte`, `JobChip.svelte` | persistent live region | 11 |
| `AssetsLayers.svelte`, `LayerInspector.svelte`, `LayerChangeForm.svelte` | new | 12 |
| `AssetsHosts.svelte`, `HostInspector.svelte` | new | 13 |
| `AssetsRail.svelte`, `Icon.svelte` | layers/hosts entries | 12, 13 |
| `quick_switcher.ts`, `QuickSwitcher.svelte`, `app_views.ts`, `App.svelte` | asset/command kinds; `requestAssetsView` | 14 |
| `pages/resources.ts`, `pages/ResourcePage.svelte` | `OptionSource 'orgs'`; reloaders | 7 |
| `ImportDialog.svelte`, `NewAssetDialog.svelte`, `SecretsPanel.svelte`, `AuthorSessionDialog.svelte`, `LintAllDialog.svelte` | tokens | 15 |
| `assets_tokens.test.ts` | widened guard | 15 |
| `CLAUDE.md` | M6 paragraph | 15 |

---

### Task 1: Held lines as data, `catalogs` on cards, and the pre-M5 Overwrite hold (R1, R2; carries T1, T7)

**Files:**
- Create: `crates/fleet-core/migrations/098_changeset_item_outcome.sql`
- Modify: `crates/fleet-core/src/store/schema.rs` (guard after `changeset_items_has_decided_at` ~:421; MIGRATIONS entry after 097 ~:1062; test after `migration_097_…` ~:4531; the `tests_upgrade` column list that names `decided_at` ~:2716-2729 gains `outcome`)
- Modify: `crates/fleet-core/src/store/changesets.rs` (`ChangesetItemRow.outcome`, `ITEM_COLS`, `item_row`, new `set_changeset_item_outcomes`)
- Modify: `crates/fleet-core/src/service/catalog/sync/plan.rs:141-142` (serialize `host_copy`), `sync/manifest.rs:71-81` (`HostCopy` gains `Serialize, Deserialize` + `#[serde(rename_all = "snake_case")]`)
- Modify: `crates/fleet-core/src/service/catalog/changesets/mod.rs` (`HeldWhy`, `HeldLine`, `ItemOutcome`; `ItemView.outcome`; `catalogs` on summary + view)
- Modify: `crates/fleet-core/src/service/catalog/changesets/apply.rs` (`held_by_host_copy` → `Vec<HeldLine>`; Overwrite always held; `HostOutcome.held_lines`; `finish_host_card` writes outcomes)
- Test: `store/schema.rs`, `store/changesets.rs`, `changesets/apply.rs`, `changesets/mod.rs` test modules

**Interfaces:**
- Consumes: `HostCopy {Unchanged, Edited, Unverified}` (`sync/manifest.rs`), `Action.host_copy: Option<HostCopy>`, `op_allowed`, `action_allowed`, `card_owns`, `finish_host_card`, `record_changeset_applied`, `set_changeset_item_states`.
- Produces:
  - `pub enum HeldWhy { Edited, Unverified, Differs }` (serde snake_case);
  - `pub struct HeldLine { pub kind: String, pub name: String, pub why: HeldWhy }`;
  - `pub struct ItemOutcome { #[serde(default, skip_serializing_if = "Vec::is_empty")] pub held: Vec<HeldLine>, #[serde(default, skip_serializing_if = "Option::is_none")] pub note: Option<String> }`, all in `changesets/mod.rs`;
  - `ItemView.outcome: Option<ItemOutcome>` (`#[serde(default, skip_serializing_if = "Option::is_none")]`);
  - `ChangesetSummary.catalogs: Vec<String>` and `ChangesetView.catalogs: Vec<String>` (`#[serde(default)]`);
  - `ChangesetItemRow.outcome: Option<String>` (`#[serde(default, skip_serializing_if = "Option::is_none")]`);
  - `Store::set_changeset_item_outcomes(&self, id: i64, outcomes: &[(i64, String)]) -> Result<()>` (position → JSON);
  - `Action.host_copy` on the wire as `"unchanged" | "edited" | "unverified"` when known.

- [ ] **Step 1: Write the migration test (fails: no 098)**

In `store/schema.rs` tests, after `migration_097_adds_decided_at_as_null_and_is_safe_to_rerun`:

```rust
    /// Assets M6 (R1): `changeset_items.outcome`, NULL on existing items,
    /// guarded on re-run.
    #[test]
    fn migration_098_adds_item_outcome_as_null_and_is_safe_to_rerun() {
        let s = store_at_version(97);
        s.conn
            .execute_batch(
                "INSERT INTO changesets (id, kind, summary, state, created_at) \
                   VALUES (1, 'rollout', 'Roll out core to oci', 'applied', 1); \
                 INSERT INTO changeset_items \
                   (changeset_id, position, grp, kind, name, action, decider, state) \
                   VALUES (1, 0, 'core', 'host', 'oci', 'sync', 'rule', 'skipped');",
            )
            .unwrap();
        assert!(!changeset_items_has_outcome(&s.conn).unwrap());
        s.migrate().unwrap();
        assert!(changeset_items_has_outcome(&s.conn).unwrap());
        let o: Option<String> = s
            .conn
            .query_row("SELECT outcome FROM changeset_items", [], |r| r.get(0))
            .unwrap();
        assert_eq!(o, None, "an item recorded before 098 has no outcome");
        s.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 98;")
            .unwrap();
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    }
```

- [ ] **Step 2: Run it to see it fail**

Run (mercury): `cargo test -p fleet-core --lib migration_098`
Expected: compile error, `changeset_items_has_outcome` not found.

- [ ] **Step 3: The migration, its guard and its entry**

`crates/fleet-core/migrations/098_changeset_item_outcome.sql`:

```sql
-- Assets M6 (Rulings R1): what a host-writing card left undone on one
-- item's host, as JSON {held: [{kind, name, why}], note?}: the copies it
-- held back ("sync it yourself") and a skipped or failed host's line. NULL
-- on items applied cleanly and on every item recorded before this
-- migration. ADD COLUMN is not idempotent: guarded in schema.rs.
ALTER TABLE changeset_items ADD COLUMN outcome TEXT;

INSERT OR IGNORE INTO schema_version (version) VALUES (98);
```

In `schema.rs`, next to `changeset_items_has_decided_at`:

```rust
/// `already_applied` guard of migration 098 (`outcome` on
/// `changeset_items`, Assets M6).
fn changeset_items_has_outcome(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('changeset_items') WHERE name = 'outcome'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}
```

and after the 097 entry in `MIGRATIONS`:

```rust
    Migration {
        version: 98,
        sql: include_str!("../../migrations/098_changeset_item_outcome.sql"),
        already_applied: Some(changeset_items_has_outcome),
    },
```

Update the `tests_upgrade` expectation that lists `changeset_items` columns. It is at ~:2716-2729 and today ends at `"decided_at"`. Append `"outcome"` there in the same form.

- [ ] **Step 4: Run the migration test**

Run (mercury): `cargo test -p fleet-core --lib migration_098`
Expected: PASS. Also run `cargo test -p fleet-core --lib tests_upgrade` and expect PASS; the CHAIN_BUDGET timing tests are known flakes, so re-run those alone if they fail.

- [ ] **Step 5: The row field and the outcome writer — test first**

In `store/changesets.rs` tests, add:

```rust
    #[test]
    fn item_outcomes_are_written_by_position_and_read_back() {
        let s = Store::open_in_memory().unwrap();
        let id = s
            .insert_changeset(
                "rollout",
                "Roll out core to oci, htz",
                &[sync_item("core", "oci"), sync_item("core", "htz")],
            )
            .unwrap();
        s.set_changeset_item_outcomes(id, &[(1, r#"{"note":"htz: unreachable"}"#.into())])
            .unwrap();
        let items = s.changeset_items(id).unwrap();
        assert_eq!(items[0].outcome, None);
        assert_eq!(items[1].outcome.as_deref(), Some(r#"{"note":"htz: unreachable"}"#));
    }
```

Use the module's existing `NewChangesetItem` constructor helper if there is one, and name it in place of `sync_item`. Otherwise add this local helper:

```rust
    fn sync_item(layer: &str, host: &str) -> NewChangesetItem {
        NewChangesetItem {
            grp: layer.into(),
            catalog_id: None,
            kind: "host".into(),
            name: host.into(),
            action: "sync".into(),
            params: None,
            decider: "rule".into(),
        }
    }
```

Before writing it, match the real field list of `NewChangesetItem` at `store/changesets.rs:~60`.

Then implement:
- `ChangesetItemRow` gains, after `decided_at`:

```rust
    /// Assets M6 (migration 098, R1): JSON `ItemOutcome` — what a
    /// host-writing card left undone on this item's host; `None` when it
    /// applied cleanly, and on items recorded before 098 (absent on the
    /// wire then, as before).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
```

- `ITEM_COLS` ends `…, state, decided_at, outcome`.
- `item_row` reads `outcome: r.get(11)?`.
- Every struct-literal of `ChangesetItemRow` in tests gains `outcome: None`. Find them with `grep -rn 'decided_at: None' crates/fleet-core/src`.
- The writer:

```rust
    /// Assets M6 (R1): each `(position, outcome JSON)` onto card `id`'s
    /// item, in one transaction. A position the card does not have is a
    /// no-op.
    pub fn set_changeset_item_outcomes(&self, id: i64, outcomes: &[(i64, String)]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for (position, json) in outcomes {
            tx.execute(
                "UPDATE changeset_items SET outcome = ?3 WHERE changeset_id = ?1 AND position = ?2",
                rusqlite::params![id, position, json],
            )?;
        }
        tx.commit()
    }
```

Run (mercury): `cargo test -p fleet-core --lib item_outcomes_are_written`. Expected: PASS.

- [ ] **Step 6: Serialize `host_copy` — test first**

In `sync/plan.rs` tests:

```rust
    #[test]
    fn an_actions_host_copy_is_on_the_wire_when_known() {
        let mut a = blank();
        a.host_copy = Some(HostCopy::Unverified);
        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(v["host_copy"], "unverified");
        a.host_copy = None;
        let v = serde_json::to_value(&a).unwrap();
        assert!(v.get("host_copy").is_none(), "absent when unknown, as before");
    }
```

`blank()` is the existing test constructor at `plan.rs`; if it has another name, use it. Implement it like this:
- In `sync/manifest.rs`, `HostCopy` derives `serde::Serialize, serde::Deserialize` with `#[serde(rename_all = "snake_case")]`.
- In `sync/plan.rs:141-142`, replace `#[serde(skip)]` on `host_copy` with:

```rust
    /// Assets M6 (R1, R15): whether the host copy is what fleet wrote, when
    /// the planner could tell — the desktop's Rollout review mirrors
    /// `action_allowed` with it. Absent when unknown, as before M6.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_copy: Option<HostCopy>,
```

The plan registry parks `SyncPlan` values in memory (no serde round-trip), so nothing else changes. Run `cargo test -p fleet-core --lib an_actions_host_copy`. Expected: PASS.

- [ ] **Step 7: The pre-M5 Overwrite is held, and held lines are structured — failing tests**

In `changesets/apply.rs` tests, next to `a_rollout_over_a_copy_fleet_cannot_vouch_for_skips_its_host`:

```rust
    /// Strip the file hashes from every manifest entry under `home`: the
    /// entries now read as written before M5.
    fn make_entries_pre_m5(home: &Path) {
        let path = home.join(".claude/.fleet-assets.json");
        let mut m: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        for e in m["assets"].as_object_mut().unwrap().values_mut() {
            e.as_object_mut().unwrap().remove("file_hashes");
        }
        std::fs::write(&path, m.to_string()).unwrap();
    }

    /// Assets M6 (R1, carry T1): a pre-M5 copy edited on the host while the
    /// catalog did not change plans as an Overwrite. The Rollout never
    /// applies it, and now says so — the host's item is skipped with a
    /// held line, not counted done.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_rollout_holds_a_pre_m5_copy_that_differs_with_a_line() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let p = f.personal.id;
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        person_syncs_oci(&f, &ssh).await;
        make_entries_pre_m5(home.path());
        let skill = home.path().join(".claude/skills/w/SKILL.md");
        std::fs::write(&skill, "edited on the host\n").unwrap();

        let card = rollout_card(&f, &[sync_item("core", p, "oci", &["skill/w"])]);
        let v = apply_all(&f, card, &ssh).await.unwrap();
        assert_eq!(item_states(&v), ["skipped"], "{:?}", v.error);
        let o = v.items[0].outcome.clone().expect("an outcome");
        assert_eq!(
            o.held,
            vec![HeldLine {
                kind: "skill".into(),
                name: "w".into(),
                why: HeldWhy::Unverified,
            }]
        );
        assert_eq!(std::fs::read_to_string(&skill).unwrap(), "edited on the host\n");
    }

    /// R1: the existing pre-M5 hold (catalog changed) is the same line as
    /// data, and the card's `error` text is unchanged for MCP readers.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_held_update_is_an_outcome_and_still_a_note() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let p = f.personal.id;
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        person_syncs_oci(&f, &ssh).await;
        make_entries_pre_m5(home.path());
        f.commit_files(&f.personal_root, p, &[("skills/w/body.md", "Steps, revised.\n")]);
        let card = rollout_card(&f, &[sync_item("core", p, "oci", &["skill/w"])]);
        let v = apply_all(&f, card, &ssh).await.unwrap();
        let o = v.items[0].outcome.clone().expect("an outcome");
        assert_eq!(o.held[0].why, HeldWhy::Unverified);
        assert!(v
            .error
            .unwrap_or_default()
            .contains("skill/w on oci: host copy predates fleet's file hashes — sync it yourself"));
    }

    /// R1: an edited copy (hashes present) is held as `edited`.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn an_edited_copy_is_held_as_edited() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let p = f.personal.id;
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        person_syncs_oci(&f, &ssh).await;
        std::fs::write(home.path().join(".claude/skills/w/SKILL.md"), "mine\n").unwrap();
        let card = rollout_card(&f, &[sync_item("core", p, "oci", &["skill/w"])]);
        let v = apply_all(&f, card, &ssh).await.unwrap();
        assert_eq!(item_states(&v), ["skipped"]);
        assert_eq!(v.items[0].outcome.as_ref().unwrap().held[0].why, HeldWhy::Edited);
    }
```

Then simplify `a_rollout_over_a_copy_fleet_cannot_vouch_for_skips_its_host` to use `make_entries_pre_m5`. Keep its `assert!(… .remove("file_hashes").is_some())` intent by asserting the file contained `file_hashes` before the call.

Run (mercury): `cargo test -p fleet-core --lib a_rollout_holds_a_pre_m5`. Expected: FAIL. Either `outcome` does not exist on `ItemView` (compile error), or the item is `applied`.

- [ ] **Step 8: Implement the structured lines**

In `changesets/mod.rs`, after `ItemParams`:

```rust
/// Why a host-writing card left one of its assets on a host to a person
/// (Assets M6, R1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeldWhy {
    /// The host copy is not what fleet wrote.
    Edited,
    /// Its manifest entry predates fleet's file hashes, so an edit cannot
    /// be ruled out.
    Unverified,
    /// The planner had no host-copy verdict, and the action is one a card
    /// never applies (an overwrite).
    Differs,
}

impl HeldWhy {
    /// The words the card's `error` note has used since M5.
    pub fn words(self) -> &'static str {
        match self {
            HeldWhy::Edited => "host copy edited",
            HeldWhy::Unverified => "host copy predates fleet's file hashes",
            HeldWhy::Differs => "host copy differs",
        }
    }
}

/// One asset a card held back on a host (R1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldLine {
    pub kind: String,
    pub name: String,
    pub why: HeldWhy,
}

/// What a host-writing card left undone on one item's host (migration 098).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemOutcome {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub held: Vec<HeldLine>,
    /// A skipped or failed host's line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl ItemOutcome {
    pub fn parse(json: Option<&str>) -> Option<ItemOutcome> {
        json.and_then(|j| serde_json::from_str(j).ok())
    }
}
```

`ItemView` gains:

```rust
    /// Assets M6 (R1): what the card left undone on this item's host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<ItemOutcome>,
```

`view()` sets `outcome: ItemOutcome::parse(i.outcome.as_deref())`.

In `apply.rs`, rewrite `held_by_host_copy`:

```rust
/// Assets M5 fix round 1, M6 R1: the card's own actions that `narrow` drops
/// under `f` only because fleet cannot treat the host copy as untouched —
/// an `Update` over a copy whose entry predates the file hashes, a moved
/// asset's `Create` that would delete such an old copy (final review I1),
/// and EVERY `Overwrite` of the card's asset (an edited copy, or a pre-M5
/// one that differs while the catalog did not change — carry T1). Such a
/// host is left to a person, never counted done.
fn held_by_host_copy(
    hp: &HostPlan,
    f: OpFilter,
    assets: &BTreeSet<String>,
    catalogs: &BTreeSet<String>,
) -> Vec<HeldLine> {
    hp.actions
        .iter()
        .filter(|a| card_owns(a, assets, catalogs) && !action_allowed(f, a))
        .filter(|a| op_allowed(f, a.op) || a.op == ActionOp::Overwrite)
        .map(|a| HeldLine {
            kind: a.kind.to_string(),
            name: a.name.clone(),
            why: match a.host_copy {
                Some(HostCopy::Edited) => HeldWhy::Edited,
                Some(HostCopy::Unverified) => HeldWhy::Unverified,
                _ => HeldWhy::Differs,
            },
        })
        .collect()
}
```

`a.kind` may be a `Kind` enum or a `String`. Use `.to_string()` or `.as_str().to_string()` to match. The old version formatted `a.kind` with `{}`, so `Display` exists.

`HostOutcome` gains `held_lines: Vec<HeldLine>`. In `sync_hosts`:

```rust
            let held = if hp.status == "planned" {
                held_by_host_copy(&hp, filter, assets, catalogs)
            } else {
                Vec::new()
            };
            narrow(&mut hp, filter, assets, catalogs);
            if !held.is_empty() {
                o.held = true;
                o.skipped.extend(held.iter().map(|h| {
                    format!("{}/{} on {host}: {} — sync it yourself", h.kind, h.name, h.why.words())
                }));
                o.held_lines.extend(held);
            }
```

The text of the `skipped` line is identical to M5's, so the existing tests on `error` still pass.

`finish_host_card`: after computing `done`, build per-item outcomes for every selected item whose host has held lines, a skipped line or a failure:

```rust
    let outcomes: Vec<(i64, String)> = selected
        .iter()
        .filter_map(|i| {
            let o = outcome.get(&host_of(i))?;
            let note = (!o.failed.is_empty())
                .then(|| o.failed.join("; "))
                .or_else(|| {
                    let plain: Vec<&String> = o
                        .skipped
                        .iter()
                        .filter(|l| !l.ends_with("— sync it yourself"))
                        .collect();
                    (!plain.is_empty()).then(|| {
                        plain.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("; ")
                    })
                });
            let out = ItemOutcome {
                held: o.held_lines.clone(),
                note,
            };
            (out != ItemOutcome::default())
                .then(|| serde_json::to_string(&out).ok().map(|j| (i.position, j)))
                .flatten()
        })
        .collect();
```

Write them with `s.set_changeset_item_outcomes(card.id, &outcomes)?` under the same `s` guard, in both arms (applied and failed), before the state writes. A restore card's single item gets the restore's failure as `note`, which is harmless and informative.

- [ ] **Step 9: `catalogs` on cards — test first**

In `changesets/mod.rs` tests:

```rust
    #[test]
    fn a_card_lists_the_catalogs_its_items_name() {
        let f = testkit::store_with_catalogs(&["personal", "acme"]);
        let (p, a) = (f.id("personal"), f.id("acme"));
        let id = f
            .store
            .lock()
            .unwrap()
            .insert_changeset(
                "bootstrap",
                "Adopt 2 as 2 layers",
                &[
                    testkit::import_item("core", Some(a), "x"),
                    testkit::import_item("core", Some(p), "y"),
                    testkit::hide_item("z"),
                ],
            )
            .unwrap();
        let summary = list(&f.store).unwrap().into_iter().find(|c| c.id == id).unwrap();
        assert_eq!(summary.catalogs, vec!["acme".to_string(), "personal".to_string()]);
        assert_eq!(get(id, &f.store).unwrap().catalogs, summary.catalogs);
    }
```

First check `changesets/testkit.rs` for the existing builders (`store_with_catalogs`, `import_item`, `hide_item` or their equivalents). Use whatever exists. If a builder is missing, add it to `testkit.rs` in the module's style.

Implement:
- Both structs gain `#[serde(default)] pub catalogs: Vec<String>`.
- A helper:

```rust
/// R2: the sorted, unique names of the catalogs a card's items name.
fn catalog_names(items: &[ChangesetItemRow], names: &BTreeMap<i64, String>) -> Vec<String> {
    items
        .iter()
        .filter_map(|i| i.catalog_id.and_then(|c| names.get(&c).cloned()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
```

- `list` reads `names` once (as `view` does) and sets `catalogs: catalog_names(&items, &names)`. `view` does the same.

- [ ] **Step 10: Run the card tests and the whole changesets module**

Run (mercury), one at a time:
- `cargo test -p fleet-core --lib changesets` → PASS
- `cargo test -p fleet-core --lib sync::plan` → PASS
- `cargo test -p fleet-core --lib store::changesets` → PASS

Expected: all PASS. Pre-existing tests asserting the exact `error` text still pass, because the words are unchanged.

- [ ] **Step 11: Format, lint, commit**

```bash
cargo fmt --all --check && cargo fleet-lint
git add crates/fleet-core
git commit -m "feat(assets): held lines as card item outcomes, catalogs on cards, hold pre-M5 overwrites

Migration 098 adds changeset_items.outcome. A Rollout records per host the
assets it held back (edited / unverified / differs) and a skipped or failed
host's line; every Overwrite of a card's own asset is now held, so a pre-M5
copy that differs no longer counts as applied (M5 carry T1). Cards list the
catalogs their items name (carry T7). Action.host_copy is on the wire."
```

---

### Task 2: Withdrawal time and New-card slug collisions (R3, R4; carries T3 M2, T3 M8)

**Files:**
- Create: `crates/fleet-core/migrations/099_changeset_withdrawn_at.sql`
- Modify: `crates/fleet-core/src/store/schema.rs` (guard `changesets_has_withdrawn_at`, MIGRATIONS 099, test)
- Modify: `crates/fleet-core/src/store/changesets.rs` (`ChangesetRow.withdrawn_at`, `CARD_COLS`, `card_row`, `withdraw_changeset` stamps it, `prune_withdrawn_changesets(before)` on `COALESCE(withdrawn_at, created_at)`)
- Modify: `crates/fleet-core/src/service/catalog/changesets/mod.rs` (`ChangesetSummary.withdrawn`, `WITHDRAWN_PREFIX`)
- Modify: `crates/fleet-core/src/service/catalog/changesets/reconcile.rs` (`WITHDRAWN_RETENTION_SECS` doc: "after withdrawal"; fill `CatalogFacts.slugs`)
- Modify: `crates/fleet-core/src/service/catalog/changesets/rules.rs` (`CatalogFacts.slugs`; `new_cards` collision demotion)
- Modify: `CLAUDE.md` (the M5 sentence that says withdrawn cards are pruned "7 days after creation" now says "7 days after withdrawal")

**Interfaces:**
- Consumes: `withdraw_changeset`, `prune_withdrawn_changesets`, `slug_collisions`, `importable`, `new_card`, `look_item`, `CatalogFacts` construction at `reconcile.rs:372`, `registry::snapshot()` already read in `reconcile::gather`.
- Produces:
  - `ChangesetRow.withdrawn_at: Option<i64>` (unix secs);
  - `ChangesetSummary.withdrawn: bool` (`#[serde(default)]`);
  - `pub const WITHDRAWN_PREFIX: &str = "withdrawn:"` in `changesets/mod.rs`, used by `ChangesetSummary.withdrawn` and the store's GLOB;
  - `CatalogFacts.slugs: BTreeSet<(String, String)>` (kind, slug).

- [ ] **Step 1: Migration test (fails)**

```rust
    /// Assets M6 (R3): `changesets.withdrawn_at`, NULL on existing cards,
    /// guarded on re-run.
    #[test]
    fn migration_099_adds_withdrawn_at_as_null_and_is_safe_to_rerun() {
        let s = store_at_version(98);
        s.conn
            .execute_batch(
                "INSERT INTO changesets (id, kind, summary, state, created_at, error) \
                   VALUES (1, 'new', 'New on oci', 'dismissed', 1, 'withdrawn: no longer applies');",
            )
            .unwrap();
        assert!(!changesets_has_withdrawn_at(&s.conn).unwrap());
        s.migrate().unwrap();
        assert!(changesets_has_withdrawn_at(&s.conn).unwrap());
        let at: Option<i64> = s
            .conn
            .query_row("SELECT withdrawn_at FROM changesets", [], |r| r.get(0))
            .unwrap();
        assert_eq!(at, None);
        s.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 99;")
            .unwrap();
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    }
```

Run (mercury): `cargo test -p fleet-core --lib migration_099` → FAIL (no guard).

- [ ] **Step 2: Migration, guard, entry**

```sql
-- Assets M6 (Rulings R3): when the system withdrew a card (Unix SECONDS),
-- so a withdrawn card is pruned a week after its withdrawal, not after its
-- creation. NULL on every other card and on cards withdrawn before this
-- migration (pruning falls back to created_at for those). ADD COLUMN is
-- not idempotent: guarded in schema.rs.
ALTER TABLE changesets ADD COLUMN withdrawn_at INTEGER;

INSERT OR IGNORE INTO schema_version (version) VALUES (99);
```

The guard `changesets_has_withdrawn_at` checks `pragma_table_info('changesets')` for `withdrawn_at`, in the same form as Task 1's guard. The `MIGRATIONS` entry is `version: 99` with `already_applied: Some(changesets_has_withdrawn_at)`. If a `tests_upgrade` list names the `changesets` columns, add `withdrawn_at` there. Run the test → PASS.

- [ ] **Step 3: Prune from withdrawal — failing store test**

In `store/changesets.rs` tests:

```rust
    #[test]
    fn a_withdrawn_card_is_pruned_a_week_after_its_withdrawal_not_its_creation() {
        let s = Store::open_in_memory().unwrap();
        let id = s.insert_changeset("new", "New on oci", &[sync_item("core", "oci")]).unwrap();
        // Created long ago, withdrawn just now.
        s.conn
            .execute("UPDATE changesets SET created_at = 1 WHERE id = ?1", [id])
            .unwrap();
        assert!(s.withdraw_changeset(id, "withdrawn: no longer applies").unwrap());
        let row = s.get_changeset(id).unwrap().unwrap();
        assert!(row.withdrawn_at.is_some_and(|t| t > 1));
        let week_ago = now_unix() - 7 * 24 * 3600;
        assert_eq!(s.prune_withdrawn_changesets(week_ago).unwrap(), 0, "withdrawn today");
        assert_eq!(s.prune_withdrawn_changesets(now_unix() + 1).unwrap(), 1);
    }

    #[test]
    fn a_card_withdrawn_before_099_is_pruned_by_its_creation() {
        let s = Store::open_in_memory().unwrap();
        let id = s.insert_changeset("new", "New on oci", &[sync_item("core", "oci")]).unwrap();
        s.conn
            .execute(
                "UPDATE changesets SET created_at = 1, state = 'dismissed', \
                 error = 'withdrawn: no longer applies', withdrawn_at = NULL WHERE id = ?1",
                [id],
            )
            .unwrap();
        assert_eq!(s.prune_withdrawn_changesets(100).unwrap(), 1);
    }
```

The first test's `sync_item` is Task 1's helper. Run → FAIL (`withdrawn_at` unknown).

- [ ] **Step 4: Implement**

- `ChangesetRow` gains:

```rust
    /// Assets M6 (migration 099, R3): when the system withdrew the card,
    /// Unix seconds; `None` otherwise and for cards withdrawn before 099.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub withdrawn_at: Option<i64>,
```

- `CARD_COLS` ends `…, error, withdrawn_at`, and `card_row` reads index 9.
- Fix every `ChangesetRow { … }` literal with `withdrawn_at: None`. Find them with `grep -rn "layers_snapshot:" crates/fleet-core/src`.
- `withdraw_changeset`:

```rust
            "UPDATE changesets SET state = 'dismissed', error = ?2, withdrawn_at = ?3 \
             WHERE id = ?1 AND state IN ('proposed', 'failed')",
            rusqlite::params![id, error, now_unix()],
```

- `prune_withdrawn_changesets(&self, withdrawn_before: i64)` (rename the parameter and its doc), with `PRUNABLE`'s time test becoming `COALESCE(c.withdrawn_at, c.created_at) < ?1`.
- `reconcile.rs:74` keeps calling it with `now_unix() - WITHDRAWN_RETENTION_SECS`. Update the const's doc to "a week after it was withdrawn".
- `changesets/mod.rs`:

```rust
/// The start of every `error` the system writes when it withdraws a card.
pub const WITHDRAWN_PREFIX: &str = "withdrawn:";
```

  Then use it: replace the string literals `"withdrawn:` (`reconcile.rs:43`, `undo.rs:~600`, `catalogs.rs` remove) with `format!("{WITHDRAWN_PREFIX} …")` where it reads naturally. Leave a literal const like `WITHDRAWN` as it is if it already starts with the prefix, and add `debug_assert!(WITHDRAWN.starts_with(WITHDRAWN_PREFIX))` in a test.
- `ChangesetSummary` gains:

```rust
    /// Assets M6 (R3): the system withdrew it (dismissed, error `withdrawn:…`).
    #[serde(default)]
    pub withdrawn: bool,
```

  `list` sets it to `card.state == "dismissed" && card.error.as_deref().is_some_and(|e| e.starts_with(WITHDRAWN_PREFIX))`.

Run (mercury): `cargo test -p fleet-core --lib store::changesets` and `cargo test -p fleet-core --lib changesets`. Expected: PASS.

- [ ] **Step 5: New-card slug collisions — failing rules tests**

In `rules.rs` tests (use the module's existing `RulesInput` builders and identity factory, e.g. `ident(kind, name, hosts)`, and the bootstrapped flag the New-card tests already set):

```rust
    /// Assets M6 (R4, carry T3 M8): two new identities that import as the
    /// same slug into the same catalog both need a look — never two New
    /// cards, the second of which would fail at apply.
    #[test]
    fn two_new_identities_sharing_a_slug_both_need_a_look() {
        let ids = [ident("skill", "My_Skill", &["oci"]), ident("skill", "my-skill", &["oci"])];
        let cards = propose(&bootstrapped_input(&ids));
        let news: Vec<_> = cards.iter().filter(|c| c.kind == CardKind::New).collect();
        assert_eq!(news.len(), 2);
        for c in news {
            assert!(c.summary.ends_with("needs a look"), "{}", c.summary);
            assert_eq!(c.items[0].grp, NEEDS_A_LOOK);
            assert!(c.items[0]
                .params
                .reason
                .as_deref()
                .unwrap()
                .starts_with("imports as my-skill, as "));
        }
    }

    /// R4: a new identity whose slug the destination catalog already holds
    /// under another name needs a look.
    #[test]
    fn a_new_identity_whose_slug_the_catalog_holds_needs_a_look() {
        let ids = [ident("skill", "My_Skill", &["oci"])];
        let mut input = bootstrapped_input(&ids);
        input.catalogs[0].slugs.insert(("skill".into(), "my-skill".into()));
        let cards = propose(&input);
        let c = cards.iter().find(|c| c.kind == CardKind::New).unwrap();
        assert_eq!(
            c.items[0].params.reason.as_deref(),
            Some("imports as my-skill, which the catalog already holds")
        );
    }
```

`bootstrapped_input` is whatever helper the existing New-card tests use to build a bootstrapped `RulesInput` with a `personal` catalog that has a layer covering `oci`. Name it after the real one. If the inputs are borrowed slices, build `catalogs` as a local `Vec` and mutate that before building the input. Run → FAIL (`slugs` unknown).

- [ ] **Step 6: Implement collisions**

- `CatalogFacts` gains:

```rust
    /// Assets M6 (R4): `(kind, slug)` of every asset the catalog holds.
    pub slugs: BTreeSet<(String, String)>,
```

  Fill it in `reconcile.rs:372`'s builder from the loaded catalog's assets: `(kind.as_str().to_string(), slugify(&asset.name))`. Use the registry snapshot `gather` already holds — no second registry call inside a registry closure. Fix every test literal of `CatalogFacts` with `slugs: BTreeSet::new()`.

- Generalise `slug_collisions(eligible, input)` so the New-card pass can call it too; it already keys on the destination. In `propose`, where New cards are built for the post-bootstrap identities (the call site of `new_card`), compute it once:

```rust
    let new_eligible: Vec<&AssetIdentity> = /* the identities new_card is called for */;
    let collides = slug_collisions(&new_eligible, input);
```

  Pass `&collides` into `new_card(id, input, &collides)`. In `new_card`, before the layer branch:

```rust
    if id.class == IdentityClass::Normal {
        if let Ok(dest) = destination(id, input) {
            let slug = slugify(&id.name);
            let why = collides.get(&(id.kind.clone(), id.name.clone())).cloned().or_else(|| {
                catalog(input, dest)
                    .filter(|c| c.slugs.contains(&(id.kind.clone(), slug.clone())))
                    .filter(|_| slug != id.name)
                    .map(|_| format!("imports as {slug}, which the catalog already holds"))
            });
            if let Some(why) = why {
                let mut item = look_item(id, input);
                item.params.reason = Some(why);
                return ProposedCard {
                    kind: CardKind::New,
                    summary: format!("New on {on}: {}/{} needs a look", id.kind, id.name),
                    items: vec![item],
                };
            }
        }
    }
```

  A name already equal to its slug is the same asset (managed), and is never an identity here. The `slug != id.name` filter keeps that impossible case quiet.

Run (mercury): `cargo test -p fleet-core --lib changesets::rules` → PASS; then `cargo test -p fleet-core --lib changesets` → PASS.

- [ ] **Step 7: Format, lint, commit**

```bash
cargo fmt --all --check && cargo fleet-lint
git add crates/fleet-core CLAUDE.md
git commit -m "feat(assets): prune withdrawn cards a week after withdrawal; New-card slug collisions need a look

Migration 099 adds changesets.withdrawn_at (M5 carry T3 M2); cards carry
withdrawn. A New card whose slug another candidate or the catalog already
holds is proposed as needs-a-look instead of failing at apply (T3 M8)."
```

---

### Task 3: Layer cards — create, rename, move a member (R5, R6)

**Files:**
- Create: `crates/fleet-core/src/service/catalog/changesets/layers.rs`
- Modify: `crates/fleet-core/src/service/catalog/changesets/mod.rs` (`mod layers; pub use layers::{LayerChange, propose_layer};`; `CardKind::Layer`; three `ItemAction`s; `ItemParams.to`, `ItemParams.members`, `ItemParams.description`; `changes_catalog` gains `"layer"`)
- Modify: `crates/fleet-core/src/service/catalog/changesets/apply.rs` (`run_steps`: step 2d layer edits; step 3 renames host_layers rows)
- Modify: `crates/fleet-core/src/service/catalog/changesets/rules.rs` (`subject_of` handles `Layer`)
- Modify: `crates/fleet-core/src/service/catalog/changesets/reconcile.rs` (layer cards never enter `by_subject`; never withdrawn)
- Modify: `crates/fleet-core/src/service/catalog/changesets/undo.rs` (only if it lists kinds explicitly; otherwise `changes_catalog` covers it)
- Modify: `crates/fleet-core/src/mcp/tools/params.rs` (`ChangesetsParams.change`), `crates/fleet-core/src/mcp/tools/assets.rs` (`propose_layer`)
- Modify: `crates/fleet-core/src/mcp/tools/tests.rs` (`BUDGET_BYTES`), `docs/control-api-reference.md` (regen)
- Test: `changesets/layers.rs` tests, `changesets/apply.rs` tests, `changesets/reconcile.rs` tests, `mcp/tools/tests_catalog_admin.rs` (or the changesets MCP test module — find with `grep -rln '"reject_item"' crates/fleet-core/src/mcp`)

**Interfaces:**
- Consumes: `Layer {name, axis, version, description, extends, members, exclude, overrides}`, `Layer::from_yaml`, `Layer::to_yaml`, `Layer::validate`, `author::layer_template(name, Axis)`, `validate::check_layer_name`, `Progress::{claim, write_file}`, `Store::{get_host_layers_for, set_host_layers_for, list_all_host_layers, insert_changeset}`, `registry::with_catalog_row`, `authoring_lock()`, `check_card_grants` (the MCP handler), `may_admin_catalog_row`.
- Produces:

```rust
/// A person's change to a catalog's layers (Assets M6, R5): one card of one
/// item, decider `person`. `catalog` is the catalog's name (default
/// `personal`); `members` and layer names follow the catalog's rules
/// (`<kind>/<name>`, `check_layer_name`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum LayerChange {
    Create {
        #[serde(default)]
        catalog: Option<String>,
        layer: String,
        /// `role` or `context` (default `context`).
        #[serde(default)]
        axis: Option<String>,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        members: Vec<String>,
    },
    Rename {
        #[serde(default)]
        catalog: Option<String>,
        layer: String,
        to: String,
    },
    Move {
        #[serde(default)]
        catalog: Option<String>,
        member: String,
        /// The layer it leaves.
        layer: String,
        to: String,
    },
}

impl LayerChange {
    pub fn catalog(&self) -> &str; // the name, `personal` when None
}

/// Validate `change` against the loaded catalog and record it as a proposed
/// `layer` card; answers the card. Takes APPLY_LOCK.
pub async fn propose_layer(change: LayerChange, store: &Mutex<Store>) -> Result<ChangesetView, IpcError>;
```

- `CardKind::Layer` → `"layer"`. `ItemAction::{CreateLayer, RenameLayer, MoveMember}` → `"create_layer" | "rename_layer" | "move_member"`.
- Item shapes:

| Action | `grp` | `kind` | `name` | `params` |
|---|---|---|---|---|
| `create_layer` | the layer | `"layer"` | the layer | `{axis, description, members}` |
| `rename_layer` | the old name | `"layer"` | the old name | `{to}` |
| `move_member` | the from-layer | the member's kind | its name | `{member, layer: from, to}` |

- Card summaries: `New layer {layer} in {catalog}`, `Rename layer {layer} to {to} in {catalog}`, `Move {member} from {layer} to {to} in {catalog}`.

- [ ] **Step 1: Failing tests for propose-time validation**

In `changesets/layers.rs` (new file, with `#[cfg(test)] mod tests`). Use `testkit`/the `Fleet` fixture from `apply.rs` tests. If `Fleet` is private to `apply.rs`'s test module, move `Fleet`, `fleet_with_core`, `apply_all`, `new_card` and `skill_yaml` into `changesets/testkit.rs` as `pub(crate)` first. That move is mechanical, and both test modules then import them.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::catalog::changesets::testkit::{fleet_with_core, Fleet};
    use crate::service::catalog::lock_registry_for_test;

    fn create(layer: &str) -> LayerChange {
        LayerChange::Create {
            catalog: None,
            layer: layer.into(),
            axis: None,
            description: None,
            members: vec![],
        }
    }

    #[tokio::test]
    async fn creating_a_layer_that_exists_is_refused_at_propose() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let e = propose_layer(create("core"), &f.store).await.unwrap_err();
        assert_eq!(e.code, codes::E_INVALID);
        assert!(e.message.contains("catalog personal already has a layer core"), "{}", e.message);
    }

    #[tokio::test]
    async fn renaming_a_missing_layer_or_onto_an_existing_one_is_refused() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let missing = LayerChange::Rename { catalog: None, layer: "nope".into(), to: "x".into() };
        assert!(propose_layer(missing, &f.store).await.unwrap_err().message.contains("no layer nope"));
        let onto = LayerChange::Rename { catalog: None, layer: "core".into(), to: "core".into() };
        assert!(propose_layer(onto, &f.store).await.unwrap_err().message.contains("already has a layer core"));
    }

    #[tokio::test]
    async fn moving_a_member_the_layer_does_not_hold_is_refused() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        f.add_layer("extra", &[]);
        let m = LayerChange::Move {
            catalog: None,
            member: "skill/other".into(),
            layer: "core".into(),
            to: "extra".into(),
        };
        let e = propose_layer(m, &f.store).await.unwrap_err();
        assert!(e.message.contains("layer core has no member skill/other"), "{}", e.message);
    }

    #[tokio::test]
    async fn a_valid_change_is_one_proposed_person_card() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let v = propose_layer(create("servers"), &f.store).await.unwrap();
        assert_eq!((v.kind.as_str(), v.state.as_str()), ("layer", "proposed"));
        assert_eq!(v.summary, "New layer servers in personal");
        assert_eq!(v.items.len(), 1);
        assert_eq!(v.items[0].action, "create_layer");
        assert_eq!(v.items[0].decider, "person");
        assert_eq!(v.catalogs, vec!["personal".to_string()]);
    }
}
```

`Fleet::add_layer(name, members)` is a new test helper. It commits `layers/<name>.yaml` (`kind: layer\nname: <name>\naxis: context\nmembers: [...]`) with `commit_files` and reloads the catalog the way `fleet_with_core` does. Add it to `testkit.rs`.

Run (mercury): `cargo test -p fleet-core --lib changesets::layers` → FAIL (module missing).

- [ ] **Step 2: Implement the types and `propose_layer`**

In `mod.rs`, add `Layer` to `CardKind`. `ALL` becomes `[CardKind; 5]` and gains `CardKind::Layer`, with `as_str` `"layer"`. Add the three actions to `ItemAction` with their `as_str`. Add to `ItemParams` (skip-if-empty, like its siblings):

```rust
    /// Assets M6 (R5): rename_layer / move_member — the target layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// create_layer — its first members (`<kind>/<name>`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<String>,
    /// create_layer — its description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
```

`changes_catalog`: `"bootstrap" | "new" | "layer" => true`.

`rules::subject_of`: add an arm. `reconcile` never matches a layer card by subject (Step 5), so any stable string will do:

```rust
        CardKind::Layer => match items.next() {
            Some(i) => format!("layer:{}:{}/{}", i.catalog_id.unwrap_or(0), i.grp, i.name),
            None => "layer:".to_string(),
        },
```

`layers.rs`:

```rust
//! Assets M6 (Rulings R5): a person's layer changes as cards — create a
//! layer, rename one, move a member between two layers of one catalog.
//! Proposed here (validated against the loaded catalog), applied by
//! `apply::run_steps` step 2d (the files) and step 3 (a rename's
//! host_layers rows), undone like every catalog card.

use super::{
    lock, CardKind, ChangesetView, Decider, ItemAction, ItemParams, APPLY_LOCK,
};
use crate::ipc::{codes, IpcError};
use crate::service::catalog::layer::{Axis, Layer};
use crate::service::catalog::registry;
use crate::service::catalog::validate::check_layer_name;
use crate::store::changesets::NewChangesetItem;
use crate::store::Store;
use std::sync::Mutex;

// (LayerChange as in Interfaces)

impl LayerChange {
    pub fn catalog(&self) -> &str {
        let c = match self {
            LayerChange::Create { catalog, .. }
            | LayerChange::Rename { catalog, .. }
            | LayerChange::Move { catalog, .. } => catalog,
        };
        c.as_deref().unwrap_or("personal")
    }
}

fn invalid(msg: String) -> IpcError {
    IpcError::new(codes::E_INVALID, msg)
}

fn parse_axis(axis: Option<&str>) -> Result<Axis, IpcError> {
    match axis.unwrap_or("context") {
        "context" => Ok(Axis::Context),
        "role" => Ok(Axis::Role),
        other => Err(invalid(format!("axis must be role or context, not {other}"))),
    }
}

pub async fn propose_layer(
    change: LayerChange,
    store: &Mutex<Store>,
) -> Result<ChangesetView, IpcError> {
    let _busy = APPLY_LOCK.lock().await;
    let name = change.catalog().to_string();
    let row = lock(store)?
        .get_catalog_by_name(&name)?
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no catalog {name}")))?;
    // Store guard dropped; the registry next (lock rule).
    let layers: Vec<Layer> = registry::with_catalog_row(&row, |c| Ok(c.layers.iter().cloned().collect()))?;
    let has = |l: &str| layers.iter().any(|x| x.name == l);
    let (summary, item) = match &change {
        LayerChange::Create { layer, axis, description, members, .. } => {
            check_layer_name(layer)?;
            if has(layer) {
                return Err(invalid(format!("catalog {name} already has a layer {layer}")));
            }
            let axis = parse_axis(axis.as_deref())?;
            (
                format!("New layer {layer} in {name}"),
                NewChangesetItem {
                    grp: layer.clone(),
                    catalog_id: Some(row.id),
                    kind: "layer".into(),
                    name: layer.clone(),
                    action: ItemAction::CreateLayer.as_str().into(),
                    params: ItemParams {
                        axis: Some(axis.as_str().into()),
                        description: description.clone(),
                        members: members.clone(),
                        ..Default::default()
                    }
                    .to_json(),
                    decider: Decider::Person.as_str().into(),
                },
            )
        }
        LayerChange::Rename { layer, to, .. } => {
            if !has(layer) {
                return Err(invalid(format!("catalog {name} has no layer {layer}")));
            }
            check_layer_name(to)?;
            if has(to) {
                return Err(invalid(format!("catalog {name} already has a layer {to}")));
            }
            (
                format!("Rename layer {layer} to {to} in {name}"),
                NewChangesetItem {
                    grp: layer.clone(),
                    catalog_id: Some(row.id),
                    kind: "layer".into(),
                    name: layer.clone(),
                    action: ItemAction::RenameLayer.as_str().into(),
                    params: ItemParams { to: Some(to.clone()), ..Default::default() }.to_json(),
                    decider: Decider::Person.as_str().into(),
                },
            )
        }
        LayerChange::Move { member, layer, to, .. } => {
            let from = layers.iter().find(|l| &l.name == layer).ok_or_else(|| {
                invalid(format!("catalog {name} has no layer {layer}"))
            })?;
            if !from.members.contains(member) {
                return Err(invalid(format!("layer {layer} has no member {member}")));
            }
            let dest = layers.iter().find(|l| &l.name == to).ok_or_else(|| {
                invalid(format!("catalog {name} has no layer {to}"))
            })?;
            if dest.members.contains(member) {
                return Err(invalid(format!("layer {to} already has {member}")));
            }
            let (kind, asset) = member
                .split_once('/')
                .ok_or_else(|| invalid(format!("bad member {member}: use <kind>/<name>")))?;
            (
                format!("Move {member} from {layer} to {to} in {name}"),
                NewChangesetItem {
                    grp: layer.clone(),
                    catalog_id: Some(row.id),
                    kind: kind.into(),
                    name: asset.into(),
                    action: ItemAction::MoveMember.as_str().into(),
                    params: ItemParams {
                        member: Some(member.clone()),
                        layer: Some(layer.clone()),
                        to: Some(to.clone()),
                        ..Default::default()
                    }
                    .to_json(),
                    decider: Decider::Person.as_str().into(),
                },
            )
        }
    };
    let id = lock(store)?.insert_changeset(CardKind::Layer.as_str(), &summary, &[item])?;
    super::get(id, store)
}
```

Adapt these names to the real API:
- the catalog lookup by name (`get_catalog_by_name`, or filter `list_catalogs()`);
- the registry accessor for a catalog's layers (the `Catalog` value's layer set; see `registry::with_catalog_row` and `LayerSet::iter`);
- `ItemParams`' JSON encoder (`to_json`, or `serde_json::to_string(&p).ok()`, matching `ProposedItem::to_new`);
- the `NewChangesetItem` field list.

Keep every message exactly as written: the tests assert them.

Run → the four tests PASS.

- [ ] **Step 3: Failing apply tests — create, rename, move, undo**

In `changesets/apply.rs` tests:

```rust
    async fn propose_and_apply(f: &Fleet, change: LayerChange, ssh: &Arc<SshClient>) -> ChangesetView {
        let v = propose_layer(change, &f.store).await.unwrap();
        apply_all(f, v.id, ssh).await.unwrap()
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_create_layer_card_commits_the_layer_file_and_undoes() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let ssh = ssh_with_home(bin.path(), home.path());
        let v = propose_and_apply(
            &f,
            LayerChange::Create {
                catalog: None,
                layer: "servers".into(),
                axis: None,
                description: Some("On the servers".into()),
                members: vec!["skill/w".into()],
            },
            &ssh,
        )
        .await;
        assert_eq!(v.state, "applied", "{:?}", v.error);
        assert!(v.undoable);
        let file = f.personal_root.join("layers/servers.yaml");
        let l = Layer::from_yaml(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!((l.axis, l.members.clone()), (Axis::Context, vec!["skill/w".to_string()]));
        assert_eq!(f.head_subject(&f.personal_root), "fleet: New layer servers in personal");
        let u = crate::service::catalog::changesets::undo::undo(v.id, &f.store).await.unwrap();
        assert_eq!(u.state, "undone");
        assert!(!file.exists());
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_rename_layer_card_renames_the_file_its_children_and_host_layers() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let p = f.personal.id;
        f.commit_files(
            &f.personal_root,
            p,
            &[("layers/server.yaml", "kind: layer\nname: server\naxis: role\n")],
        );
        f.commit_files(
            &f.personal_root,
            p,
            &[("layers/edge.yaml", "kind: layer\nname: edge\naxis: role\nextends: server\n")],
        );
        f.reload_personal();
        f.store.lock().unwrap().set_host_layers_for("oci", p, Some("server"), &["core"]).unwrap();
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let ssh = ssh_with_home(bin.path(), home.path());
        let v = propose_and_apply(
            &f,
            LayerChange::Rename { catalog: None, layer: "server".into(), to: "servers".into() },
            &ssh,
        )
        .await;
        assert_eq!(v.state, "applied", "{:?}", v.error);
        assert!(!f.personal_root.join("layers/server.yaml").exists());
        let renamed = std::fs::read_to_string(f.personal_root.join("layers/servers.yaml")).unwrap();
        assert_eq!(Layer::from_yaml(&renamed).unwrap().name, "servers");
        let edge = std::fs::read_to_string(f.personal_root.join("layers/edge.yaml")).unwrap();
        assert_eq!(Layer::from_yaml(&edge).unwrap().extends.as_deref(), Some("servers"));
        let rows = f.store.lock().unwrap().get_host_layers_for("oci", p).unwrap();
        assert!(rows.iter().any(|r| r.axis == "role" && r.layer_name == "servers"));
        // Undo puts back the files and the host's role.
        let u = crate::service::catalog::changesets::undo::undo(v.id, &f.store).await.unwrap();
        assert_eq!(u.state, "undone");
        assert!(f.personal_root.join("layers/server.yaml").exists());
        let rows = f.store.lock().unwrap().get_host_layers_for("oci", p).unwrap();
        assert!(rows.iter().any(|r| r.axis == "role" && r.layer_name == "server"));
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_move_member_card_moves_it_in_one_commit() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        f.add_layer("extra", &[]);
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let ssh = ssh_with_home(bin.path(), home.path());
        let v = propose_and_apply(
            &f,
            LayerChange::Move {
                catalog: None,
                member: "skill/w".into(),
                layer: "core".into(),
                to: "extra".into(),
            },
            &ssh,
        )
        .await;
        assert_eq!(v.state, "applied", "{:?}", v.error);
        let read = |n: &str| {
            Layer::from_yaml(&std::fs::read_to_string(f.personal_root.join(format!("layers/{n}.yaml"))).unwrap())
                .unwrap()
                .members
        };
        assert!(read("core").is_empty());
        assert_eq!(read("extra"), vec!["skill/w".to_string()]);
        assert_eq!(v.commits.len(), 1);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_layer_card_never_deletes_a_file_someone_changed() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let ssh = ssh_with_home(bin.path(), home.path());
        let v = propose_layer(
            LayerChange::Rename { catalog: None, layer: "core".into(), to: "base".into() },
            &f.store,
        )
        .await
        .unwrap();
        // An uncommitted edit makes the checkout dirty: refused before anything changes.
        std::fs::write(f.personal_root.join("layers/core.yaml"), "kind: layer\nname: core\naxis: context\n# mine\n").unwrap();
        let e = apply_all(&f, v.id, &ssh).await.unwrap_err();
        assert_eq!(e.code, codes::E_INVALID_STATE);
        assert!(std::fs::read_to_string(f.personal_root.join("layers/core.yaml")).unwrap().contains("# mine"));
    }
```

`Fleet::head_subject(root)` (`git log -1 --format=%s`) and `Fleet::reload_personal()` are new `testkit` helpers if no equivalent exists. Check first for an existing "last commit subject" helper in the module, e.g. used by `applying_a_bootstrap_commits_once…`.

Run (mercury): `cargo test -p fleet-core --lib a_create_layer_card` → FAIL. The new actions fall through `run_steps` untouched, so no file is written.

- [ ] **Step 4: Implement step 2d and the rename in step 3**

In `layers.rs`, the file edits, called from `run_steps` after 2c and before 3:

```rust
/// A rename step 3 applies to host_layers: (catalog id, old, new).
pub(super) type Rename = (i64, String, String);

fn read_layer(root: &Path, name: &str) -> Result<Layer, IpcError> {
    let p = root.join("layers").join(format!("{name}.yaml"));
    let text = std::fs::read_to_string(&p)
        .map_err(|_| invalid(format!("no layer {name} in this catalog")))?;
    Layer::from_yaml(&text).map_err(|e| IpcError::new(E_CATALOG_PARSE, format!("layers/{name}.yaml: {e}")))
}

/// Step 2d: each selected layer item's files, through `progress` (every
/// path claimed before it is written or deleted). Answers the renames.
pub(super) fn apply_layer_items(
    selected: &[&ChangesetItemRow],
    rows: &[CatalogRow],
    progress: &mut Progress,
) -> Result<Vec<Rename>, Failure> {
    let mut renames = Vec::new();
    for &item in selected.iter().filter(|i| {
        matches!(
            i.action.as_str(),
            "create_layer" | "rename_layer" | "move_member"
        )
    }) {
        let row = item
            .catalog_id
            .and_then(|id| rows.iter().find(|r| r.id == id))
            .ok_or_else(|| fail(item, invalid(format!("item {} names no catalog", item.position))))?;
        let root = Path::new(&row.repo_path);
        let p = ItemParams::parse(item.params.as_deref());
        let rel = |n: &str| format!("layers/{n}.yaml");
        match item.action.as_str() {
            "create_layer" => {
                if root.join(rel(&item.name)).exists() {
                    return Err(fail(item, IpcError::new(codes::E_INVALID_STATE,
                        format!("catalog {} already has a layer {}", row.name, item.name))));
                }
                let mut l = author::layer_template(&item.name, parse_axis(p.axis.as_deref()).map_err(|e| fail(item, e))?);
                if let Some(d) = p.description { l.description = d; }
                l.members = p.members;
                l.validate().map_err(|e| fail(item, invalid(e)))?;
                progress.write_file(row.id, root, &rel(&item.name), l.to_yaml().as_bytes()).map_err(|e| fail(item, e))?;
            }
            "rename_layer" => {
                let to = p.to.ok_or_else(|| fail(item, invalid("rename names no target".into())))?;
                check_layer_name(&to).map_err(|e| fail(item, e))?;
                if root.join(rel(&to)).exists() {
                    return Err(fail(item, IpcError::new(codes::E_INVALID_STATE,
                        format!("catalog {} already has a layer {to}", row.name))));
                }
                let mut l = read_layer(root, &item.name).map_err(|e| fail(item, e))?;
                l.name = to.clone();
                progress.write_file(row.id, root, &rel(&to), l.to_yaml().as_bytes()).map_err(|e| fail(item, e))?;
                progress.claim(row.id, root, &rel(&item.name)).map_err(|e| fail(item, e))?;
                std::fs::remove_file(root.join(rel(&item.name))).map_err(|e| fail(item, e.into()))?;
                // Children that extend it, in the same catalog.
                for entry in std::fs::read_dir(root.join("layers")).map_err(|e| fail(item, e.into()))? {
                    let path = entry.map_err(|e| fail(item, e.into()))?.path();
                    let Some(stem) = path.file_stem().and_then(|s| s.to_str()).map(str::to_string) else { continue };
                    if path.extension().and_then(|e| e.to_str()) != Some("yaml") || stem == to { continue; }
                    let mut child = read_layer(root, &stem).map_err(|e| fail(item, e))?;
                    if child.extends.as_deref() == Some(item.name.as_str()) {
                        child.extends = Some(to.clone());
                        progress.write_file(row.id, root, &rel(&stem), child.to_yaml().as_bytes()).map_err(|e| fail(item, e))?;
                    }
                }
                renames.push((row.id, item.name.clone(), to));
            }
            "move_member" => {
                let (member, from, to) = match (p.member, p.layer, p.to) {
                    (Some(m), Some(f), Some(t)) => (m, f, t),
                    _ => return Err(fail(item, invalid("move names no member, layer or target".into()))),
                };
                let mut a = read_layer(root, &from).map_err(|e| fail(item, e))?;
                let mut b = read_layer(root, &to).map_err(|e| fail(item, e))?;
                if !a.members.contains(&member) {
                    return Err(fail(item, IpcError::new(codes::E_INVALID_STATE,
                        format!("layer {from} no longer has {member}"))));
                }
                a.members.retain(|m| m != &member);
                if !b.members.contains(&member) { b.members.push(member); }
                progress.write_file(row.id, root, &rel(&from), a.to_yaml().as_bytes()).map_err(|e| fail(item, e))?;
                progress.write_file(row.id, root, &rel(&to), b.to_yaml().as_bytes()).map_err(|e| fail(item, e))?;
            }
            _ => unreachable!(),
        }
    }
    Ok(renames)
}
```

- `Progress`, `Failure` and `fail` live in `apply.rs`. Make them `pub(super)` or move `apply_layer_items` into `apply.rs`, whichever keeps visibility minimal. Moving the function is fine; then `layers.rs` keeps only `LayerChange`, `propose_layer` and the parsers.
- `layer_with_members`' rule for children: a rename that leaves a child with a dangling `extends` would fail the reload in `after_commits` with a warning. Walking every layer file covers it.

In `run_steps`, after 2c:

```rust
    // 2d. Layer cards (Assets M6, R5): create / rename / move, every path
    //     claimed before it is written or deleted.
    let renames = apply_layer_items(selected, rows, progress)?;
```

In step 3, after the `adds` loop and under the same `s` guard, rename that catalog's rows:

```rust
        for (cid, old, new) in &renames {
            let hosts: BTreeSet<String> = s
                .list_all_host_layers()
                .map_err(|e| fail_in(old, e.into()))?
                .into_iter()
                .filter(|r| r.catalog_id == *cid && &r.layer_name == old)
                .map(|r| r.host_alias)
                .collect();
            for host in hosts {
                let current = s.get_host_layers_for(&host, *cid).map_err(|e| fail_in(old, e.into()))?;
                let swap = |n: &str| if n == old { new.clone() } else { n.to_string() };
                let role = current.iter().find(|r| r.axis == "role").map(|r| swap(&r.layer_name));
                let ctx: Vec<String> = current.iter().filter(|r| r.axis == "context").map(|r| swap(&r.layer_name)).collect();
                let ctx: Vec<&str> = ctx.iter().map(String::as_str).collect();
                s.set_host_layers_for(&host, *cid, role.as_deref(), &ctx).map_err(|e| fail_in(old, e.into()))?;
            }
        }
```

`get_host_layers_for` may return only ACTIVE rows (`list_layers_for` does). If so, an inactive row naming `old` stays as it is, harmless since inactive. Note this in a comment. The snapshot taken in step 1 already covers these catalogs (`touched` includes the item's catalog), so a failure or an undo restores them.

`after_commits` → `propose_follow_up` reads Import/TakeHost only, so a layer card proposes no follow-up Rollout. That is correct:
- a moved member reaches the target layer's hosts through SB6 when that layer was rolled out;
- for a never-rolled-out layer, the next reconcile pass's gap rule proposes a Rollout card.

Run (mercury): the four tests from Step 3 one by one (`a_create_layer_card`, `a_rename_layer_card`, `a_move_member_card`, `a_layer_card_never_deletes`) → PASS.

- [ ] **Step 5: Reconcile never withdraws a layer card — test then fix**

In `reconcile.rs` tests:

```rust
    #[tokio::test]
    async fn the_pass_never_withdraws_a_persons_layer_card() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let v = propose_layer(
            LayerChange::Create { catalog: None, layer: "servers".into(), axis: None, description: None, members: vec![] },
            &f.store,
        )
        .await
        .unwrap();
        reconcile(&f.store, true).unwrap();
        reconcile(&f.store, true).unwrap();
        let after = crate::service::catalog::changesets::get(v.id, &f.store).unwrap();
        assert_eq!(after.state, "proposed");
    }
```

Fix: in `reconcile`, build `by_subject` from `f.open` filtered by `o.0.kind != CardKind::Layer.as_str()`, with a comment: "a person's layer card (R5) has no rule subject; only a person dismisses it". Run → PASS.

- [ ] **Step 6: MCP `propose_layer` — failing tool tests**

Find the module that tests the `changesets` tool (`grep -rln 'reject_item' crates/fleet-core/src/mcp/tools/`). Add tests in its style, with its caller builders (master, granted client, ungranted client, per-host token):

```rust
    #[tokio::test]
    async fn propose_layer_makes_a_card_for_a_granted_caller() {
        // master proposes a create in personal → Ok, card kind "layer".
    }

    #[tokio::test]
    async fn propose_layer_in_a_catalog_the_client_has_no_grant_for_is_forbidden() {
        // unbound full client granted personal only → propose_layer in "acme" → E_FORBIDDEN,
        // and no card was inserted.
    }

    #[tokio::test]
    async fn propose_layer_without_a_change_is_invalid() {
        // {action:"propose_layer"} → E_INVALID "propose_layer needs a change".
    }
```

Write the three bodies concretely with the module's own `call(&tools, caller, json!({...}))` helper and its catalog fixtures. Mirror the existing `apply`/`reject_item` grant tests line for line, changing the action and arguments. A per-host token is already refused by the central gate, and the existing test covers that for every action.

Implement:
- `ChangesetsParams` gains:

```rust
    /// propose_layer: {op: create|rename|move, catalog?, layer, to?, member?, axis?, description?, members?}.
    #[serde(default)]
    pub change: Option<LayerChange>,
```

- The `action` doc becomes `list|propose|propose_layer|apply|undo|dismiss|reject_item`. So does the unknown-action error text.
- In the handler, before the `id`-requiring branch:

```rust
        "propose_layer" => {
            let change = p.change.ok_or_else(|| IpcError::new(codes::E_INVALID, "propose_layer needs a change"))?;
            let name = change.catalog().to_string();
            let row = lock(store)?.get_catalog_by_name(&name)?;
            if !may_admin_catalog_row(&caller, store, &name, row.as_ref())? {
                return Err(changesets_forbidden("propose_layer", Some(&name), &caller));
            }
            let v = changesets::propose_layer(change, store).await?;
            ok_json(&v)
        }
```

  Match `may_admin_catalog_row`'s real signature and the forbidden helper (`assets.rs:857-881`, `changesets_forbidden`).

Run (mercury): the three tests → PASS.

- [ ] **Step 7: Budget and reference docs**

Run (mercury): `cargo test -p fleet-core --lib the_served_definition_budget_stays_bounded`. It prints the measurement. Set `BUDGET_BYTES` to measurement + 100, and append a history line in the comment above it: "M6 Task 3: changesets propose_layer + change (measured N)". Then run `REGEN_DOCS=1 cargo fleet-test -- reference_is_current` and `cargo fleet-test -- reference_is_current` → PASS.

- [ ] **Step 8: The whole changesets module, format, lint, commit**

Run (mercury): `cargo test -p fleet-core --lib changesets` → PASS. Run `cargo test -p fleet-core --lib mcp::tools` → PASS.

```bash
cargo fmt --all --check && cargo fleet-lint
git add crates/fleet-core docs/control-api-reference.md
git commit -m "feat(assets): layer cards — create, rename, move a member (changesets propose_layer)

A person's layer change is a proposed 'layer' card, validated at propose,
applied by the catalog-card machinery (every path claimed; a rename moves
the file, its children's extends and the host_layers rows) and undone by
revert + snapshot. Reconcile never withdraws one. BUDGET_BYTES re-measured."
```

---

### Task 4: `catalog_admin { drift_diff }` — the two texts of a drifted asset (R7)

**Files:**
- Create: `crates/fleet-core/src/service/catalog/drift_diff.rs`
- Modify: `crates/fleet-core/src/service/catalog/mod.rs` (`pub mod drift_diff;`)
- Modify: `crates/fleet-core/src/service/catalog/admin.rs` (`AdminCall::DriftDiff(DriftDiffArgs)` → `drift_diff`; `touches` → `Touches::Catalog(name)`; the dispatcher arm)
- Modify: `crates/fleet-core/src/mcp/tools/params.rs` (`CatalogAdminParams::action` doc names `drift_diff`; the test `the_action_param_names_every_admin_call` holds it)
- Modify: `crates/fleet-core/src/mcp/tools/tests.rs` (`BUDGET_BYTES`), `docs/control-api-reference.md` (regen)
- Test: `drift_diff.rs` tests; `mcp/tools/tests_catalog_admin.rs` (`every_call()` gains one `DriftDiff`)

**Interfaces:**
- Consumes:
  - the per-host effective set the planner renders from: `effective_for_host(store, host)`, or whatever `compute_host_plan` receives — read `sync/mod.rs::plan_sync` to see how it gets each host's assets and harnesses;
  - `harness::by_id(id)` and `Harness::render(asset) -> RenderPlan { files: Vec<{path, bytes}> , merges, … }`;
  - the harness's root on a host (the same base `sync/apply.rs` writes `files[].path` under — look for how apply resolves a file path on the host);
  - `inventory::run_host_script(ssh, host, script)`, `crate::shell::quote`, `require_dialable_host`, `crate::service::hub::ensure_local_allowed`.
- Produces:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriftDiffArgs {
    pub host_alias: String,
    pub kind: Kind,
    pub name: String,
    /// default `claude`
    #[serde(default)]
    pub harness: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriftFile {
    /// Relative to the harness root on the host (as the plan names it).
    pub path: String,
    /// The catalog's rendered text; `None` when the catalog renders no such file.
    #[serde(default)]
    pub catalog: Option<String>,
    /// The host's text; `None` when the file is missing on the host.
    #[serde(default)]
    pub host: Option<String>,
    /// Either side is not UTF-8 (then both texts are `None`).
    #[serde(default)]
    pub binary: bool,
    /// Either side was cut at `MAX_SIDE_BYTES`.
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriftDiff {
    pub host_alias: String,
    pub harness: String,
    pub files: Vec<DriftFile>,
    /// The asset renders only into config files (an MCP entry): its diff is
    /// not shown, and no config file was read (R7).
    #[serde(default)]
    pub merges_only: bool,
}

pub const MAX_SIDE_BYTES: usize = 256 * 1024;

pub async fn drift_diff(
    target: CatalogTarget<'_>,
    args: DriftDiffArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<DriftDiff, IpcError>;
```

- [ ] **Step 1: Failing tests**

`ssh_with_home(bin, home)` (the `apply.rs` test helper) runs a remote script locally against `home`. Reuse it, moving it to `testkit` if it is private.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::catalog::changesets::testkit::{fleet_with_core, person_syncs_oci, ssh_with_home};
    use crate::service::catalog::lock_registry_for_test;

    fn args(name: &str) -> DriftDiffArgs {
        DriftDiffArgs { host_alias: "oci".into(), kind: Kind::Skill, name: name.into(), harness: None }
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn the_diff_holds_the_catalog_and_the_host_text_of_each_file() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let ssh = ssh_with_home(bin.path(), home.path());
        person_syncs_oci(&f, &ssh).await;
        std::fs::write(home.path().join(".claude/skills/w/SKILL.md"), "edited on the host\n").unwrap();
        let d = drift_diff(CatalogTarget::Personal, args("w"), &f.store, &ssh).await.unwrap();
        assert_eq!((d.host_alias.as_str(), d.harness.as_str()), ("oci", "claude"));
        let skill = d.files.iter().find(|x| x.path.ends_with("skills/w/SKILL.md")).unwrap();
        assert!(skill.catalog.as_deref().unwrap().contains("Steps."));
        assert_eq!(skill.host.as_deref(), Some("edited on the host\n"));
        assert!(!skill.binary && !skill.truncated);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_file_missing_on_the_host_has_no_host_text() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let ssh = ssh_with_home(bin.path(), home.path());
        person_syncs_oci(&f, &ssh).await;
        std::fs::remove_file(home.path().join(".claude/skills/w/SKILL.md")).unwrap();
        let d = drift_diff(CatalogTarget::Personal, args("w"), &f.store, &ssh).await.unwrap();
        let skill = d.files.iter().find(|x| x.path.ends_with("SKILL.md")).unwrap();
        assert_eq!(skill.host, None);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_large_host_file_is_cut_and_marked() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let ssh = ssh_with_home(bin.path(), home.path());
        person_syncs_oci(&f, &ssh).await;
        std::fs::write(home.path().join(".claude/skills/w/SKILL.md"), "x".repeat(MAX_SIDE_BYTES + 10)).unwrap();
        let d = drift_diff(CatalogTarget::Personal, args("w"), &f.store, &ssh).await.unwrap();
        let skill = d.files.iter().find(|x| x.path.ends_with("SKILL.md")).unwrap();
        assert!(skill.truncated);
        assert_eq!(skill.host.as_ref().unwrap().len(), MAX_SIDE_BYTES);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn an_asset_not_planned_for_the_host_is_refused() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let ssh = ssh_with_home(bin.path(), home.path());
        let e = drift_diff(CatalogTarget::Personal, args("nope"), &f.store, &ssh).await.unwrap_err();
        assert_eq!(e.code, codes::E_NOTFOUND);
        assert!(e.message.contains("skill/nope is not planned for oci"), "{}", e.message);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn an_mcp_entry_is_merges_only_and_reads_no_config_file() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        f.add_mcp_server_to_core("jira"); // testkit helper: commits mcp_servers/jira/asset.yaml and adds it to core
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let ssh = ssh_with_home(bin.path(), home.path());
        std::fs::write(home.path().join(".claude.json"), r#"{"mcpServers":{"jira":{"env":{"TOKEN":"secret-value"}}}}"#).unwrap();
        let d = drift_diff(
            CatalogTarget::Personal,
            DriftDiffArgs { host_alias: "oci".into(), kind: Kind::McpServer, name: "jira".into(), harness: None },
            &f.store,
            &ssh,
        )
        .await
        .unwrap();
        assert!(d.merges_only);
        assert!(d.files.is_empty());
        assert!(!serde_json::to_string(&d).unwrap().contains("secret-value"));
    }
}
```

Write `add_mcp_server_to_core` using an existing MCP-server asset fixture. Search the catalog tests for `kind: mcp_server` and copy a minimal valid `asset.yaml`. Run (mercury): `cargo test -p fleet-core --lib drift_diff` → FAIL (module missing).

- [ ] **Step 2: Implement**

```rust
//! Assets M6 (Rulings R7): the catalog's and the host's text of one asset's
//! files, for the Drift card's DiffView. Only the files the asset renders
//! are read — never a config file a merge writes into — and the catalog
//! side is rendered WITHOUT secret substitution, so no secret value is
//! ever in an answer. The desktop computes the diff.

/// One remote read: each named path, relative to the harness root, as
/// `==FILE <path> <size>` + base64 of at most MAX+1 bytes, or `==MISSING <path>`.
fn read_script(root: &str, paths: &[String]) -> String {
    let mut s = format!(
        "cd {} 2>/dev/null || exit 0\n",
        crate::shell::quote(root)
    );
    for p in paths {
        let q = crate::shell::quote(p);
        s.push_str(&format!(
            "if [ -f {q} ]; then printf '==FILE %s\\n' {q}; head -c {} {q} | base64; printf '\\n==END\\n'; \
             else printf '==MISSING %s\\n' {q}; fi\n",
            MAX_SIDE_BYTES + 1
        ));
    }
    s
}
```

Use `crate::shell::quote`'s real form. `root` is a home-relative path such as `$HOME/.claude`: build it the way `sync/apply.rs` does for the harness. If apply passes a `~`-relative path, the script uses `cd "$HOME"/<rel>`, quoting the relative part only.

`parse_read(stdout) -> BTreeMap<String, Option<Vec<u8>>>`: path → bytes or None. It decodes base64 with the crate's base64 engine (find the one `import.rs::parse_remote_dump` uses). A path that is not in the request is an error.

`drift_diff` itself:
1. **Validate.** `args.host_alias` is a registered host (`require_dialable_host`), or `local` (`ensure_local_allowed`).
2. **Find the asset.** The host's effective set, as the planner builds it, with catalog = `target`'s label. Find `(kind, name)` in it, else `E_NOTFOUND "{kind}/{name} is not planned for {host}"`.
3. **Render.** `harness::by_id(harness)` (else `E_INVALID`), then `render(asset)`. No `secrets::substitute`.
4. **Merges only.** If `plan.files` is empty and the plan has merges, answer `merges_only: true, files: []`.
5. **Check paths.** Every `plan.files[].path` must be relative with no `..` component (`E_INVALID` otherwise, defensive).
6. **Read the host.** Locally, read `home_dir()/<root>/<path>` directly with the same cap. Remotely, use `run_host_script(ssh, host, read_script(...))`.
7. **Build each `DriftFile`.** Cut a side longer than `MAX_SIDE_BYTES` to that length and mark it `truncated`. A side that is not UTF-8 makes `binary: true`, and both texts become `None`. If the catalog bytes exceed the cap, cut them the same way.

`admin.rs`:
- add `drift_diff(DriftDiffArgs)` to `admin_calls!` and the dispatcher, calling `drift_diff::drift_diff(target, args, store, ssh).await`;
- in `touches`, map it like the other per-catalog reads, `Touches::Catalog(<the call's catalog or personal>)` (follow `AssetHistory`'s arm);
- add one `AdminCall::DriftDiff(..)` to `every_call()` in declaration order.

`CatalogAdminParams::action`'s doc lists every action. Insert `drift_diff` where `AdminCall::ACTIONS` puts it; the test `the_action_param_names_every_admin_call` fails until it matches.

Run (mercury): `cargo test -p fleet-core --lib drift_diff` → PASS. Run `cargo test -p fleet-core --lib the_action_param_names_every_admin_call` → PASS.

- [ ] **Step 3: Budget, reference docs, commit**

Same as Task 3, Step 7: re-measure `BUDGET_BYTES` (+100, with a history line "M6 Task 4: catalog_admin drift_diff"), then `REGEN_DOCS=1 cargo fleet-test -- reference_is_current`.

```bash
cargo fmt --all --check && cargo fleet-lint
git add crates/fleet-core docs/control-api-reference.md
git commit -m "feat(assets): catalog_admin drift_diff — the catalog's and the host's text of a drifted asset

Only the files the asset renders are read (never a config file), the
catalog side keeps its \${NAME} placeholders, each side is capped at
256 KiB, and non-UTF-8 is reported as binary. The desktop computes the
diff (R7). Gated by a grant on the asset's catalog."
```

---

### Task 5: Desktop commands for the card verbs (R8)

**Files:**
- Modify: `src-tauri/src/commands/assets.rs` (7 commands + `routed::` bodies + arg structs)
- Modify: `src-tauri/src/lib.rs` (register them next to `catalog_list_changesets`)
- Modify: `src-tauri/src/backend/verdicts.rs` (7 rows `Routed{tool:"changesets"}`)
- Modify: `src-tauri/src/backend/tests_routing.rs` (7 cases in the M5 block of `catalog_admin_cases()`)
- Modify: `src/lib/hub.ts` (`ROUTED_ACTIONS` += the six mutating ones), regenerated `src/lib/hub_verdicts.generated.json` and `docs/hub.md`

**Interfaces:**
- Consumes:
  - `changesets::{get, list, propose, propose_layer, LayerChange, ChangesetView, ChangesetSummary}`;
  - `changesets::apply::{apply, ApplyArgs}`;
  - `changesets::undo::{undo, dismiss, reject_items}`;
  - `HubBackend::route`;
  - the `SshClient` state the local `catalog_apply_sync` body uses (copy its parameter list).
- Produces: the Tauri commands and their `args`:

| Command | `args` | Hub tool arguments | Answers |
|---|---|---|---|
| `catalog_get_changeset` | `{id}` | `{"action":"list","id":id}` | `ChangesetView` |
| `catalog_apply_changeset` | `{id, positions?}` | `{"action":"apply","id":id,"positions":[..]}`, `positions` omitted when null | `ChangesetView` |
| `catalog_undo_changeset` | `{id}` | `{"action":"undo","id":id}` | `ChangesetView` |
| `catalog_dismiss_changeset` | `{id}` | `{"action":"dismiss","id":id}` | `ChangesetView` |
| `catalog_reject_changeset_items` | `{id, positions}` | `{"action":"reject_item","id":id,"positions":[..]}` | `ChangesetView` |
| `catalog_propose_changesets` | none | `{"action":"propose"}` | `Vec<ChangesetSummary>` |
| `catalog_propose_layer_change` | `{change: LayerChange}` | `{"action":"propose_layer","change":{…}}` | `ChangesetView` |

- [ ] **Step 1: Failing routing cases**

In `tests_routing.rs`, after the `catalog_list_changesets` case, add one case per command in the same tuple form. The canned reply is a minimal view: `const VIEW: &str = r#"{"id":3,"kind":"new","summary":"s","state":"applied","created_at":1,"items":[]}"#;` with a summary list for `propose`. Two of the seven:

```rust
        (
            "catalog_apply_changeset",
            "changesets",
            json!({ "action": "apply", "id": 3, "positions": [0, 2] }),
            VIEW,
            Box::new(|b, s, h| {
                block_on(r::catalog_apply_changeset(
                    b,
                    commands::assets::ApplyChangesetArgs { id: 3, positions: Some(vec![0, 2]) },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "catalog_apply_changeset",
            "changesets",
            json!({ "action": "apply", "id": 3 }),
            VIEW,
            Box::new(|b, s, h| {
                block_on(r::catalog_apply_changeset(
                    b,
                    commands::assets::ApplyChangesetArgs { id: 3, positions: None },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
```

The third closure argument stands for the SSH/AppHandle state the local arm needs. Match how the existing `catalog_apply_sync` case passes it. If cases cannot pass it, give the routed body the state the closure has.

Write the other five cases the same way:
- `get` → `{"action":"list","id":3}`;
- `undo` and `dismiss` → `{action, id}`;
- `reject` → `{"action":"reject_item","id":3,"positions":[1]}`;
- `propose` → `{"action":"propose"}` answering `"[]"`;
- `propose_layer_change` → `{"action":"propose_layer","change":{"op":"rename","layer":"core","to":"base"}}`, built from `LayerChange::Rename { catalog: None, layer: "core".into(), to: "base".into() }`. That serializes `catalog` as absent only if the enum's `catalog` fields carry `skip_serializing_if = "Option::is_none"`; add it in Task 3's types if this case shows `"catalog":null`.

Run (mercury): `cargo test -p claude-fleet --lib tests_routing` → FAIL (commands missing).

- [ ] **Step 2: Implement the commands**

In `commands/assets.rs`, next to `catalog_list_changesets`:

```rust
/// `catalog_get_changeset` / `_undo_` / `_dismiss_`'s arguments.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChangesetIdArgs {
    pub id: i64,
}

/// `catalog_apply_changeset`'s arguments: the card and, optionally, the
/// items to run (default: every pending item but "needs a look"; a drift
/// card needs exactly one).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ApplyChangesetArgs {
    pub id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub positions: Option<Vec<i64>>,
}

/// `catalog_reject_changeset_items`' arguments.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RejectItemsArgs {
    pub id: i64,
    pub positions: Vec<i64>,
}

/// `catalog_propose_layer_change`'s arguments.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LayerChangeArgs {
    pub change: LayerChange,
}

/// Assets M6 (R8): one card in full — the Inspector's Items / Hosts / Diff.
#[tauri::command]
pub async fn catalog_get_changeset(
    backend: State<'_, Arc<FleetBackend>>,
    args: ChangesetIdArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<ChangesetView, IpcError> {
    routed::catalog_get_changeset(&backend, args, &store).await
}
```

Write the other six commands the same way. Each has a doc line naming its ruling, R8 or R5. For example, `catalog_apply_changeset`'s doc: "Apply a card (or the named items) — on a hub, its `changesets { apply }`, which checks the caller's grant on every catalog the items touch and, for a rollout or restore, the hub's confirm gate (R8)."

`routed::` bodies, for example:

```rust
    pub async fn catalog_apply_changeset(
        backend: &FleetBackend,
        args: ApplyChangesetArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<ChangesetView, IpcError> {
        match backend.hub() {
            Some(hub) => {
                let mut body = serde_json::json!({ "action": "apply", "id": args.id });
                if let Some(p) = &args.positions {
                    body["positions"] = serde_json::json!(p);
                }
                hub.route("catalog_apply_changeset", &body).await
            }
            None => {
                catalog::changesets::apply::apply(
                    catalog::changesets::apply::ApplyArgs { id: args.id, positions: args.positions },
                    store,
                    ssh,
                )
                .await
            }
        }
    }
```

The local arms:
- `get` → `catalog::changesets::get(args.id, store)`;
- `undo` → `catalog::changesets::undo::undo(args.id, store).await`;
- `dismiss` → `undo::dismiss(args.id, store).await`;
- `reject` → `undo::reject_items(args.id, &args.positions, store).await`;
- `propose` → `catalog::changesets::propose(store).await`;
- `propose_layer_change` → `catalog::changesets::propose_layer(args.change, store).await`.

Match each function's real sync/async-ness. Each takes APPLY_LOCK itself.

`verdicts.rs`: seven rows after `catalog_list_changesets`, each `Verdict::Routed { tool: "changesets" }`, with one comment above the block:

```rust
    // Assets M6 (R8): the card verbs — `changesets { list(id) | apply |
    // undo | dismiss | reject_item | propose | propose_layer }`. The hub
    // checks the caller's grant per catalog the card touches and runs its
    // confirm gate for a rollout or restore apply; the desktop never sends
    // a nonce (as `catalog_apply_sync`). A hub before M6 refuses
    // `propose_layer` with E_INVALID (R13 precedent) — no contract bump.
```

`lib.rs`: register the seven next to `catalog_list_changesets`.

`hub.ts`: add to `ROUTED_ACTIONS`, with a comment "Assets M6: the card verbs":
- `catalog_apply_changeset`
- `catalog_undo_changeset`
- `catalog_dismiss_changeset`
- `catalog_reject_changeset_items`
- `catalog_propose_changesets`
- `catalog_propose_layer_change`

- [ ] **Step 3: Regenerate, run, commit**

Run (mercury):
1. `REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen` — fails once on purpose, writing the files;
2. `cargo test -p claude-fleet --lib verdict_gen` → PASS;
3. `cargo test -p claude-fleet --lib tests_routing` → PASS.

Locally: `pnpm vitest run src/lib/hub_verdicts.test.ts` → PASS.

```bash
cargo fmt --all --check && cargo fleet-lint
git add src-tauri src/lib/hub.ts src/lib/hub_verdicts.generated.json docs/hub.md
git commit -m "feat(assets): desktop commands for the card verbs, routed to the hub's changesets tool"
```

---

### Task 6: Desktop commands for catalogs, per-catalog layers, host provenance and the diff (R9)

**Files:**
- Modify: `crates/fleet-core/src/service/catalog/resolve.rs` (`ResolutionView`, `ResolutionView::of`)
- Modify: `crates/fleet-core/src/mcp/tools/assets.rs:504-538` (`resolve_preview` answers `ResolutionView::of(&r)`)
- Modify: `src-tauri/src/commands/assets.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/backend/verdicts.rs`, `src-tauri/src/backend/tests_routing.rs`
- Modify: `src/lib/hub.ts` (`ROUTED_ACTIONS` += add/remove/admit/unadmit), regenerated verdict files

**Interfaces:**
- Consumes: `catalogs::{add_catalog, AddCatalogArgs, remove_catalog, admit, unadmit}`, `admin::{CatalogNameArgs, AdmitArgs}`, `list_layers_for(row)` (or the `catalog_admin list_layers` + `catalog` service path), `resolve::resolve_preview(host)` → `Resolution`, `drift_diff::{drift_diff, DriftDiffArgs, DriftDiff}`, the `in_catalog(call, name)` helper.
- Produces:
  - `pub struct ResolutionView { provenance, excluded, refused, withheld, held_back, assets: Vec<ResolvedAsset { kind, name, version }> }`, exactly the MCP projection's JSON shape. Move the projection out of `assets.rs` into `resolve.rs` unchanged.
  - The commands:

| Command | `args` | Routes to | Answers |
|---|---|---|---|
| `catalog_add_catalog` | `AddCatalogArgs {name, repo_path, remote_url?, org?}` | `catalog_admin {add_catalog}` | `CatalogStatus` |
| `catalog_remove_catalog` | `CatalogNameArgs {name}` | `catalog_admin {remove_catalog}` | `CatalogRemoval` |
| `catalog_admit_catalog` | `AdmitArgs {host_alias, catalog}` | `catalog_admin {admit_catalog}` | `Vec<String>` |
| `catalog_unadmit_catalog` | `AdmitArgs` | `catalog_admin {unadmit_catalog}` | `Vec<String>` |
| `catalog_list_layers_in` | `CatalogNameArgs {name}` | `catalog_admin {list_layers}` + top-level `catalog` (personal: none) | `LayerListing` |
| `catalog_host_provenance` | `{host_alias}` | the MCP **`resolve_preview`** tool (`{"host_alias": …}`) | `ResolutionView` |
| `catalog_drift_diff` | `{host_alias, kind, name, harness?, catalog?}` | `catalog_admin {drift_diff}` + top-level `catalog` | `DriftDiff` |

- [ ] **Step 1: `ResolutionView` — move with a guard test**

In `resolve.rs` tests:

```rust
    #[test]
    fn the_view_drops_asset_bodies_and_keeps_provenance() {
        let r = sample_resolution(); // existing test builder, or build one with one asset and one provenance entry
        let v = ResolutionView::of(&r);
        let json = serde_json::to_value(&v).unwrap();
        assert!(json["provenance"].as_object().unwrap().contains_key("skill/w"));
        assert_eq!(json["assets"][0], serde_json::json!({"kind":"skill","name":"w","version":"1"}));
        assert!(json["assets"][0].get("body").is_none());
    }
```

Move the projection code from `mcp/tools/assets.rs:504-538` into `ResolutionView::of`, and have the tool call it. The existing MCP `resolve_preview` tests must pass unchanged; they pin the wire shape. Run (mercury): `cargo test -p fleet-core --lib resolve` and `cargo test -p fleet-core --lib resolve_preview` → PASS.

- [ ] **Step 2: Failing routing cases (seven)**

The same form as Task 5, for example:

```rust
        (
            "catalog_admit_catalog",
            "catalog_admin",
            json!({ "action": "admit_catalog", "args": { "host_alias": "mefistos", "catalog": "acme" } }),
            r#"["acme"]"#,
            Box::new(|b, s, _| {
                block_on(r::catalog_admit_catalog(
                    b,
                    AdmitArgs { host_alias: "mefistos".into(), catalog: "acme".into() },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "catalog_list_layers_in",
            "catalog_admin",
            json!({ "action": "list_layers", "catalog": "acme" }),
            r#"{"layers":[],"hosts":[]}"#,
            Box::new(|b, s, _| {
                block_on(r::catalog_list_layers_in(b, CatalogNameArgs { name: "acme".into() }, s)).map(|_| ())
            }),
        ),
        (
            "catalog_list_layers_in",
            "catalog_admin",
            json!({ "action": "list_layers" }),
            r#"{"layers":[],"hosts":[]}"#,
            Box::new(|b, s, _| {
                block_on(r::catalog_list_layers_in(b, CatalogNameArgs { name: "personal".into() }, s)).map(|_| ())
            }),
        ),
        (
            "catalog_host_provenance",
            "resolve_preview",
            json!({ "host_alias": "oci" }),
            r#"{"provenance":{},"excluded":{},"refused":[],"withheld":[],"held_back":{},"assets":[]}"#,
            Box::new(|b, s, _| {
                block_on(r::catalog_host_provenance(b, HostAliasArgs { host_alias: "oci".into() }, s)).map(|_| ())
            }),
        ),
        (
            "catalog_drift_diff",
            "catalog_admin",
            json!({ "action": "drift_diff",
                    "args": { "host_alias": "oci", "kind": "skill", "name": "w" },
                    "catalog": "acme" }),
            r#"{"host_alias":"oci","harness":"claude","files":[]}"#,
            Box::new(|b, s, h| {
                block_on(r::catalog_drift_diff(
                    b,
                    DriftDiffCmdArgs { host_alias: "oci".into(), kind: Kind::Skill, name: "w".into(), harness: None, catalog: Some("acme".into()) },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
```

Write the remaining cases in the same form:
- `add_catalog`: args `{"name":"acme","repo_path":"/r","remote_url":null,"org":"Acme"}`, or omitting null fields if `AddCatalogArgs` skips them; check its serde;
- `remove_catalog`: args `{"name":"acme"}`;
- `unadmit_catalog`.

Also check the provenance reply literal's `withheld` shape against `ResolutionView`'s serde (a list, or a map keyed by `kind/name`).

`every_routed_tool_is_a_tool_the_hub_serves` holds `resolve_preview` to the served tools. It is served and is `Access::Client`, so a per-host token may call it. That is acceptable for provenance: it is the existing read of one host's resolved set.

Run (mercury) → FAIL.

- [ ] **Step 3: Implement**

The commands:
- `HostAliasArgs { host_alias: String }`, unless an equivalent exists; reuse `ResolvePreviewArgs` if it is just `{host_alias}`.
- `DriftDiffCmdArgs { host_alias, kind, name, harness: Option<String>, catalog: Option<String> }`.

The routed bodies:
- add / remove / admit / unadmit send `AdminCall::X(args)` with no top-level `catalog`. These actions carry the name in `args`, and `touches` reads it there.
- `catalog_list_layers_in` sends `in_catalog(AdminCall::ListLayers, &name)`. The helper drops `catalog` for personal; check `in_catalog`, and if it does not drop it, do so here, as the personal case asserts.
- `catalog_host_provenance` sends `hub.route("catalog_host_provenance", &json!({"host_alias": a}))`. Its verdict row names tool `resolve_preview`.
- `catalog_drift_diff` sends `in_catalog(AdminCall::DriftDiff(DriftDiffArgs{…}), catalog)`.

The local arms:
- `catalogs::add_catalog(args, store)` and the others;
- `list_layers_for(&row)` after looking the row up by name (`E_NOTFOUND "no catalog {name}"`);
- `ResolutionView::of(&resolve::resolve_preview(…)?)` for provenance;
- `drift_diff::drift_diff(CatalogTarget::by_name(catalog), …)` for the diff. Use however `catalog_asset_history`'s local arm turns a name into a `CatalogTarget`.

Verdict rows: four `Routed{tool:"catalog_admin"}` (add, remove, admit, unadmit); `catalog_list_layers_in` and `catalog_drift_diff` → `catalog_admin`; `catalog_host_provenance` → `resolve_preview`. One comment block:

```rust
    // Assets M6 (R9): the catalog set (add / remove: the master only on a
    // hub; admit / unadmit: a grant on that catalog), one catalog's layers,
    // one host's provenance (the MCP resolve_preview projection) and a
    // drifted asset's two texts (catalog_admin drift_diff — a hub before M6
    // refuses the action with E_INVALID; no contract bump).
```

`hub.ts` `ROUTED_ACTIONS` += `catalog_add_catalog`, `catalog_remove_catalog`, `catalog_admit_catalog`, `catalog_unadmit_catalog`. These are the mutating ones; the reads are not listed.

- [ ] **Step 4: Regenerate, run, commit**

As in Task 5 Step 3, then:

```bash
cargo fmt --all --check && cargo fleet-lint
git add crates/fleet-core src-tauri src/lib/hub.ts src/lib/hub_verdicts.generated.json docs/hub.md
git commit -m "feat(assets): desktop commands for the catalog set, per-catalog layers, host provenance and the drift diff"
```

---

### Task 7: Settings → Catalogs (R10, R11)

**Files:**
- Modify: `crates/fleet-core/src/pages/resources.rs` (`OptionSource::Orgs`; `CATALOG` resource; `RESOURCES` gains it)
- Create: `crates/fleet-core/pages/settings.catalogs.json`
- Modify: `crates/fleet-core/pages/settings.json` (a link in the "Work" section after `settings.orgs`)
- Modify: `crates/fleet-core/src/pages/mod.rs` (`PAGE_FILES` gains `settings.catalogs.json`)
- Modify: `src/lib/pages/resources.ts` (`OptionSource` += `'orgs'`; `RESOURCE_RELOADERS.catalog = [loadCatalogStatuses]`)
- Modify: `src/lib/pages/ResourcePage.svelte` (options for `orgs` from the `orgs` store, plus a "none (personal)" option is NOT offered: personal exists already; org is required for a new catalog — see R11 note below)
- Regenerate: `src/lib/pages/registry.generated.json`, `docs/page-spec.schema.json`, `docs/page-catalog.json`
- Test: `crates/fleet-core/src/pages/tests.rs` (well-formedness runs over every resource automatically), `src/lib/pages/CatalogsPage.test.ts` (new)

**R11 note (decided here):** `add_catalog` for a name other than `personal` requires an org; `personal` already exists and is re-pointed with `fleet-hub catalog set`. So the Add form's `org` is a required `Options{source: Orgs}` param.

**Interfaces:**
- Consumes: `ResourceType`, `FieldSpec::new(..).edit(..)`, `FieldKind::{Choice, Items, Text}`, `ActionSpec::new(id, label, command, bind).params(..).confirm(..)`, `ParamSpec`, `ParamKind::{Text, Options}`, `Bind::{Record, Item, Param}`, `ItemLabel::Plain`. Copy the forms of `ORG` (:384-500) and `TRACKER` (:499 `TRACKER_STATES`).
- Produces:
  - resource id `catalog`;
  - page id `settings.catalogs`;
  - `OptionSource::Orgs`, which resolves to the org names from `list_orgs`; in TS, `'orgs'` from the `orgs` store, `value: name`.

- [ ] **Step 1: The resource (Rust)**

```rust
const CATALOG_STATES: &[(&str, &str)] = &[
    ("loaded", "Loaded"),
    ("problem", "Could not load"),
    ("not_loaded", "Not loaded"),
];

const CATALOG: ResourceType = ResourceType {
    id: "catalog",
    label: "Catalog",
    plural: "Catalogs",
    help: "Git repos of assets fleet syncs to hosts. `personal` is yours; an org's catalog \
           reaches that org's hosts and the org-less hosts that admit it. A GitHub org with \
           SSO must allow the catalog's deploy key — an org admin does that once in the \
           org's settings. Grants are given on the hub: \
           `fleet-hub client grant <client> assets --catalog <name>`.",
    list: "catalog_list_catalogs",
    id_field: "name",
    title_field: "name",
    color_field: None,
    empty: "No catalogs yet. Add an org's catalog by its checkout path.",
    fields: &[
        FieldSpec::new("state", "State", "", FieldKind::Choice { options: CATALOG_STATES }),
        FieldSpec::new("repo_path", "Checkout", "Where the catalog's git repo is on this machine.", FieldKind::Text { max: 512 }),
        FieldSpec::new("remote_url", "Remote", "Its git remote, if any.", FieldKind::Text { max: 512 }),
        FieldSpec::new("org", "Org", "The org whose hosts receive it; none for personal.", FieldKind::Text { max: 128 }),
        FieldSpec::new(
            "admitted",
            "Admitted by",
            "Hosts with no org that receive this catalog. Hosts of its org always do.",
            FieldKind::Items {
                item_label: ItemLabel::Plain,
                remove: Some(ActionSpec::new(
                    "catalog.unadmit",
                    "Remove",
                    "catalog_unadmit_catalog",
                    &[("host_alias", Bind::Item), ("catalog", Bind::Record("name"))],
                )),
                add: &[ActionSpec::new(
                    "catalog.admit",
                    "Admit a host",
                    "catalog_admit_catalog",
                    &[("host_alias", Bind::Param("host")), ("catalog", Bind::Record("name"))],
                )
                .params(&[ParamSpec::new("host", "Host", ParamKind::Options { source: OptionSource::Hosts })])],
            },
        ),
        FieldSpec::new(
            "granted",
            "Granted to",
            "Paired desktops that may change this catalog. Granted on the hub (see above).",
            FieldKind::Items { item_label: ItemLabel::Plain, remove: None, add: &[] },
        ),
    ],
    create: Some(
        ActionSpec::new(
            "catalog.add",
            "Add catalog",
            "catalog_add_catalog",
            &[
                ("name", Bind::Param("name")),
                ("repo_path", Bind::Param("repo_path")),
                ("remote_url", Bind::Param("remote_url")),
                ("org", Bind::Param("org")),
            ],
        )
        .params(&[
            ParamSpec::new("name", "Name", ParamKind::Text { max: 64, placeholder: "papayapos" }),
            ParamSpec::new("repo_path", "Checkout path", ParamKind::Text { max: 512, placeholder: "~/catalogs/papayapos" }),
            ParamSpec::new("remote_url", "Remote URL (optional)", ParamKind::Text { max: 512, placeholder: "git@github.com:org/catalog.git" }),
            ParamSpec::new("org", "Org", ParamKind::Options { source: OptionSource::Orgs }),
        ]),
    ),
    update: None,
    delete: Some(
        ActionSpec::new("catalog.remove", "Remove", "catalog_remove_catalog", &[("name", Bind::Record("name"))])
            .confirm("Removes the catalog from fleet's config. Its checkout stays on disk; open cards on it are withdrawn."),
    ),
    actions: &[],
    create_flow: None,
    variant_by: None,
};
```

Adapt the constructor names and the optional-param mechanism (an empty `remote_url` must send `null`). Find how `ORG`'s optional params bind (`Bind::Param` of an empty text → `null`, or a `.optional()`). Keep every user-facing string.

`OptionSource::Orgs`: add the variant, its serde name `orgs`, and its doc. `RESOURCES = &[ORG, TRACKER, CATALOG]`.

`settings.catalogs.json`:

```json
{
  "id": "settings.catalogs",
  "title": "Catalogs",
  "parent": "settings",
  "layout": "master_detail",
  "resource": "catalog",
  "sections": [
    { "title": "Where it lives", "items": [
      { "type": "field", "key": "state" },
      { "type": "field", "key": "repo_path" },
      { "type": "field", "key": "remote_url" },
      { "type": "field", "key": "org" }
    ]},
    { "title": "Who receives and changes it", "items": [
      { "type": "field", "key": "admitted" },
      { "type": "field", "key": "granted" }
    ]}
  ]
}
```

Match `settings.orgs.json`'s exact key names for title, sections and items. Add `{"type":"link","page":"settings.catalogs"}` after the `settings.orgs` link in `settings.json`. Add the file to `PAGE_FILES`.

Run (mercury): `cargo test -p fleet-core --lib pages` → expect the well-formedness tests and `page_docs_are_current` to fail on drift only. Then `REGEN_PAGE_DOCS=1 cargo fleet-test -- page_docs_are_current`, and re-run `cargo test -p fleet-core --lib pages` → PASS. Run `cargo test -p claude-fleet --lib resource_commands_exist` → PASS: every command it names was registered in Tasks 5 and 6.

- [ ] **Step 2: The frontend option source and reloader — failing page test**

`src/lib/pages/CatalogsPage.test.ts`. Copy `ResourcePage.test.ts`'s setup: `vi.mock` core, `route()`, `bundle.pages.find(p => p.id === 'settings.catalogs')`.

```ts
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import ResourcePage from './ResourcePage.svelte';
import { bundle } from './testing';
import { orgs } from '../orgs';
import { hosts } from '../hosts';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;
const page = bundle.pages.find((p) => p.id === 'settings.catalogs')!;
const resource = bundle.resources.find((r) => r.id === 'catalog')!;
const ROW = {
  id: 2, name: 'acme', org_id: 1, org: 'Acme', repo_path: '/r/acme', remote_url: null,
  head_commit: 'abc', last_loaded_at: 1, state: 'loaded', asset_count: 3,
  admitted: ['mefistos'], granted: ['laptop'],
};

beforeEach(() => {
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === 'catalog_list_catalogs') return [ROW];
    if (cmd === 'catalog_unadmit_catalog') return [];
    if (cmd === 'catalog_add_catalog') return ROW;
    throw { code: 'E_TEST', message: cmd };
  });
  orgs.set([{ id: 1, name: 'Acme' } as never]);
  hosts.set([{ alias: 'mefistos' } as never, { alias: 'oci' } as never]);
});

describe('Settings → Catalogs', () => {
  it('lists catalogs with who admits and who is granted', async () => {
    render(ResourcePage, { page, resource });
    await screen.findByText('acme');
    await fireEvent.click(screen.getByText('acme'));
    expect(await screen.findByText('mefistos')).toBeInTheDocument();
    expect(screen.getByText('laptop')).toBeInTheDocument();
  });

  it('unadmits a host with the catalog name', async () => {
    render(ResourcePage, { page, resource });
    await fireEvent.click(await screen.findByText('acme'));
    await fireEvent.click(await screen.findByRole('button', { name: /Remove mefistos/i }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('catalog_unadmit_catalog', { args: { host_alias: 'mefistos', catalog: 'acme' } }),
    );
  });

  it('offers the orgs as the new catalog org', async () => {
    render(ResourcePage, { page, resource });
    await fireEvent.click(await screen.findByTestId('resource-add'));
    expect(await screen.findByRole('option', { name: 'Acme' })).toBeInTheDocument();
  });

  it('shows granted clients read-only', async () => {
    render(ResourcePage, { page, resource });
    await fireEvent.click(await screen.findByText('acme'));
    expect(screen.queryByRole('button', { name: /Remove laptop/i })).toBeNull();
  });
});
```

Adapt these selectors to ResourcePage's real accessible names: the Items remove button's label, how a record is opened, and the `bundle` export name in `testing.ts`. Keep the four behaviours. Run locally: `pnpm vitest run src/lib/pages/CatalogsPage.test.ts` → FAIL, since `'orgs'` has no options.

- [ ] **Step 3: Implement the TS side**

- `resources.ts`: `export type OptionSource = 'hosts' | 'trackers' | 'orgs';` and `RESOURCE_RELOADERS.catalog = [loadCatalogStatuses]` (import it from `../assets_workspace`).
- `ResourcePage.svelte` (around :111-121, where options resolve): add `orgs: $orgs.map((o) => ({ value: o.name, label: o.name }))`, and make sure the component loads orgs on mount when the page uses that source, the way trackers load (`loadOrgs` from `../orgs`).

Run: `pnpm vitest run src/lib/pages` → PASS; `pnpm check` → 0 errors.

- [ ] **Step 4: Commit**

```bash
cargo fmt --all --check && cargo fleet-lint
git add crates/fleet-core src/lib/pages docs/page-spec.schema.json docs/page-catalog.json
git commit -m "feat(settings): Settings → Catalogs — add, remove, admit hosts; grants shown with the hub command"
```

---

### Task 8: The data layer — wire types, wrappers, stores, and three pure modules (R1–R5, R7, R12–R19)

**Files:**
- Modify: `src/lib/assets_workspace.ts` (types, wrappers, stores, `WorkspaceView`, `Selection`)
- Modify: `src/lib/assets.ts` (`SyncAction.catalog?`, `SyncAction.host_copy?`)
- Create: `src/lib/assets_cards.ts`, `src/lib/line_diff.ts`, `src/lib/assets_layers.ts`
- Test: `src/lib/assets_workspace.test.ts` (extend), `src/lib/assets_cards.test.ts`, `src/lib/line_diff.test.ts`, `src/lib/assets_layers.test.ts`

**Interfaces:**
- Consumes: Tasks 1–6 wire shapes; `invokeCmd`, `Result`, `IpcError` (`./result`); `HostRow` (`./hosts`); `SyncAction`, `SyncPlan`, `HostPlan` (`./assets`).
- Produces (TS, exported from `assets_workspace.ts` unless stated otherwise):

```ts
export type CardKind = 'bootstrap' | 'new' | 'drift' | 'rollout' | 'layer';
export interface ChangesetSummary { /* M5 fields */ catalogs?: string[]; withdrawn?: boolean }
export type HeldWhy = 'edited' | 'unverified' | 'differs';
export interface HeldLine { kind: string; name: string; why: HeldWhy }
export interface ItemOutcome { held?: HeldLine[]; note?: string | null }
export interface ItemParams {
  from_host?: string; layer?: string; member?: string; host?: string; axis?: string;
  scope?: string; hash?: string; reason?: string; assets?: string[]; harness?: string;
  to?: string; members?: string[]; description?: string;
}
/** One card item as `changesets { list, id }` answers it (replaces M5's stale `ChangesetItem`). */
export interface ItemView {
  position: number; grp: string; catalog?: string | null; kind: string; name: string;
  action: 'import' | 'assign_layer' | 'set_scope' | 'hide' | 'take_host' | 'restore' | 'sync'
        | 'create_layer' | 'rename_layer' | 'move_member';
  params: ItemParams; decider: 'rule' | 'jev' | 'haiku' | 'person';
  state: 'pending' | 'applied' | 'skipped' | 'rejected'; outcome?: ItemOutcome | null;
}
export interface ChangesetView {
  id: number; kind: CardKind; summary: string; state: CardState; created_at: number;
  applied_at?: number | null; error?: string | null; commits: Record<string, string>;
  undoable: boolean; catalogs?: string[]; items: ItemView[];
}
export type LayerChange =
  | { op: 'create'; catalog?: string; layer: string; axis?: 'role' | 'context'; description?: string; members?: string[] }
  | { op: 'rename'; catalog?: string; layer: string; to: string }
  | { op: 'move'; catalog?: string; member: string; layer: string; to: string };
export interface Provenance { introduced_by: string; overridden_by?: string[]; catalog: string }
export interface ResolutionView {
  provenance: Record<string, Provenance>; excluded: Record<string, string>;
  refused: { kind: string; name: string; reason: string; catalog?: string | null }[];
  withheld: unknown; held_back: Record<string, string>;
  assets: { kind: string; name: string; version: string }[];
}
export interface DriftFile { path: string; catalog?: string | null; host?: string | null; binary?: boolean; truncated?: boolean }
export interface DriftDiff { host_alias: string; harness: string; files: DriftFile[]; merges_only?: boolean }

export type WorkspaceView = 'inbox' | 'layers' | 'hosts' | 'library';
export type Selection = /* M5 */ | { type: 'layer'; catalog: string; name: string } | { type: 'host'; alias: string };

export const cardViews: Writable<Record<number, ChangesetView>>;          // open cards in full
export const layersByCatalog: Writable<Record<string, LayerListing> | null>;
export function getChangeset(id: number): Promise<Result<ChangesetView>>;
export function applyChangeset(id: number, positions?: number[] | null): Promise<Result<ChangesetView>>;
export function undoChangeset(id: number): Promise<Result<ChangesetView>>;
export function dismissChangeset(id: number): Promise<Result<ChangesetView>>;
export function rejectItems(id: number, positions: number[]): Promise<Result<ChangesetView>>;
export function proposeChangesets(): Promise<Result<ChangesetSummary[]>>;
export function proposeLayerChange(change: LayerChange): Promise<Result<ChangesetView>>;
export function admitCatalog(host_alias: string, catalog: string): Promise<Result<string[]>>;
export function unadmitCatalog(host_alias: string, catalog: string): Promise<Result<string[]>>;
export function listLayersIn(name: string): Promise<Result<LayerListing>>;
export function hostProvenance(host_alias: string): Promise<Result<ResolutionView>>;
export function driftDiff(a: { host_alias: string; kind: string; name: string; harness?: string | null; catalog?: string | null }): Promise<Result<DriftDiff>>;
export async function loadOpenCardViews(cards: ChangesetSummary[] | null): Promise<void>;
export async function loadAllLayers(statuses: CatalogStatus[] | null): Promise<void>;
```

and `assets_cards.ts`, `line_diff.ts`, `assets_layers.ts` as written below.

- [ ] **Step 1: `line_diff.ts` — failing tests**

`src/lib/line_diff.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { diffLines, unifiedDiff } from './line_diff';
import { parseUnifiedDiff } from './DiffView.svelte';

describe('diffLines', () => {
  it('keeps equal lines and marks the edit', () => {
    const ops = diffLines(['a', 'b', 'c'], ['a', 'x', 'c']);
    expect(ops.map((o) => o.t + o.line)).toEqual([' a', '-b', '+x', ' c']);
  });
  it('handles empty sides', () => {
    expect(diffLines([], ['a']).map((o) => o.t + o.line)).toEqual(['+a']);
    expect(diffLines(['a'], []).map((o) => o.t + o.line)).toEqual(['-a']);
    expect(diffLines([], [])).toEqual([]);
  });
  it('is minimal on a moved block', () => {
    const ops = diffLines(['1', '2', '3', '4'], ['1', '3', '4', '2']);
    expect(ops.filter((o) => o.t !== ' ')).toHaveLength(2);
  });
  it('falls back to replace-all past the edit budget', () => {
    const a = Array.from({ length: 50 }, (_, i) => `a${i}`);
    const b = Array.from({ length: 50 }, (_, i) => `b${i}`);
    const ops = diffLines(a, b, 10);
    expect(ops.filter((o) => o.t === '-')).toHaveLength(50);
    expect(ops.filter((o) => o.t === '+')).toHaveLength(50);
  });
});

describe('unifiedDiff', () => {
  it('emits hunks with three lines of context that DiffView parses', () => {
    const a = 'l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\n';
    const b = 'l1\nl2\nl3\nl4\nX\nl6\nl7\nl8\n';
    const u = unifiedDiff(a, b, 'catalog/SKILL.md', 'oci/SKILL.md');
    expect(u).toContain('--- catalog/SKILL.md');
    expect(u).toContain('+++ oci/SKILL.md');
    expect(u).toContain('@@ -2,7 +2,7 @@');
    const rows = parseUnifiedDiff(u);
    expect(rows.filter((r) => r.kind === 'del').map((r) => r.text)).toEqual(['l5']);
    expect(rows.filter((r) => r.kind === 'add').map((r) => r.text)).toEqual(['X']);
  });
  it('is empty when both sides are equal', () => {
    expect(unifiedDiff('a\n', 'a\n', 'x', 'y')).toBe('');
  });
  it('treats a missing side as empty', () => {
    expect(unifiedDiff(null, 'a\n', 'c', 'h')).toContain('@@ -0,0 +1,1 @@');
  });
});
```

Run: `pnpm vitest run src/lib/line_diff.test.ts` → FAIL (module missing).

- [ ] **Step 2: `line_diff.ts`**

```ts
// Assets M6 (Rulings R7): the drift diff is computed here, from the two
// texts `catalog_drift_diff` answers. Myers' O((N+M)·D) line diff; past
// `maxD` edits it gives up and replaces the whole file (a drift that large
// is read as "different", not line by line). Output is the unified format
// `DiffView`'s `parseUnifiedDiff` reads.

export interface DiffOp { t: ' ' | '-' | '+'; line: string }

export function splitLines(text: string | null | undefined): string[] {
  if (!text) return [];
  const lines = text.split('\n');
  if (lines[lines.length - 1] === '') lines.pop();
  return lines;
}

export function diffLines(a: string[], b: string[], maxD = 2000): DiffOp[] {
  const n = a.length;
  const m = b.length;
  const max = n + m;
  if (max === 0) return [];
  const off = max + 1;
  const v = new Int32Array(2 * max + 3);
  const trace: Int32Array[] = [];
  for (let d = 0; d <= max; d++) {
    if (d > maxD) return [...a.map((line) => ({ t: '-' as const, line })), ...b.map((line) => ({ t: '+' as const, line }))];
    trace.push(v.slice(off - d - 1, off + d + 2));
    for (let k = -d; k <= d; k += 2) {
      let x = k === -d || (k !== d && v[off + k - 1] < v[off + k + 1]) ? v[off + k + 1] : v[off + k - 1] + 1;
      let y = x - k;
      while (x < n && y < m && a[x] === b[y]) { x++; y++; }
      v[off + k] = x;
      if (x >= n && y >= m) return backtrack(trace, a, b);
    }
  }
  return [];
}

function backtrack(trace: Int32Array[], a: string[], b: string[]): DiffOp[] {
  const out: DiffOp[] = [];
  let x = a.length;
  let y = b.length;
  for (let d = trace.length - 1; d >= 0; d--) {
    const v = trace[d];
    const at = (k: number) => v[k + d + 1];
    const k = x - y;
    const prevK = k === -d || (k !== d && at(k - 1) < at(k + 1)) ? k + 1 : k - 1;
    const prevX = d === 0 ? 0 : at(prevK);
    const prevY = prevX - prevK;
    while (x > prevX && y > prevY) { out.push({ t: ' ', line: a[x - 1] }); x--; y--; }
    if (d > 0) {
      if (x === prevX) { out.push({ t: '+', line: b[y - 1] }); y--; }
      else { out.push({ t: '-', line: a[x - 1] }); x--; }
    }
  }
  return out.reverse();
}

/** Unified diff of two texts (a `null` side is empty), `ctx` lines of context. '' when equal. */
export function unifiedDiff(a: string | null | undefined, b: string | null | undefined, aName: string, bName: string, ctx = 3): string {
  const ops = diffLines(splitLines(a), splitLines(b));
  if (!ops.some((o) => o.t !== ' ')) return '';
  const lines = [`--- ${aName}`, `+++ ${bName}`];
  // Positions of each op in a and b (1-based line numbers before the op).
  let ai = 0;
  let bi = 0;
  const pos = ops.map((o) => {
    const p = { a: ai, b: bi };
    if (o.t !== '+') ai++;
    if (o.t !== '-') bi++;
    return p;
  });
  let i = 0;
  while (i < ops.length) {
    if (ops[i].t === ' ') { i++; continue; }
    const start = Math.max(0, i - ctx);
    let end = i;
    // Extend while the next change is within 2·ctx of the last one.
    let j = i;
    while (j < ops.length) {
      if (ops[j].t !== ' ') { end = j; j++; continue; }
      let k = j;
      while (k < ops.length && ops[k].t === ' ') k++;
      if (k < ops.length && k - j <= 2 * ctx) { j = k; continue; }
      break;
    }
    const stop = Math.min(ops.length, end + ctx + 1);
    const hunk = ops.slice(start, stop);
    const aLen = hunk.filter((o) => o.t !== '+').length;
    const bLen = hunk.filter((o) => o.t !== '-').length;
    const aStart = aLen === 0 ? pos[start].a : pos[start].a + 1;
    const bStart = bLen === 0 ? pos[start].b : pos[start].b + 1;
    lines.push(`@@ -${aStart},${aLen} +${bStart},${bLen} @@`);
    for (const o of hunk) lines.push(o.t + o.line);
    i = stop;
  }
  return lines.join('\n') + '\n';
}
```

Run → PASS. If the `@@ -2,7 +2,7 @@` assertion is off by one, the hunk-start arithmetic is wrong. Fix it to match the unified-diff convention: start is 1-based, or the line before when the length is 0. Never change the test to fit.

- [ ] **Step 3: `assets_cards.ts` — failing tests**

`src/lib/assets_cards.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { primaryVerb, cardOwns, cardMayApply, heldWords, olderHubWords, cardNote, coveringNewCard, mergePlans, isUndoBanner } from './assets_cards';
import type { ChangesetSummary, ChangesetView, ItemView } from './assets_workspace';
import type { SyncAction, SyncPlan } from './assets';

const item = (over: Partial<ItemView>): ItemView => ({
  position: 0, grp: 'core', catalog: 'personal', kind: 'skill', name: 'w', action: 'import',
  params: {}, decider: 'rule', state: 'pending', ...over,
});
const view = (over: Partial<ChangesetView>): ChangesetView => ({
  id: 1, kind: 'new', summary: 's', state: 'proposed', created_at: 1, commits: {}, undoable: false, items: [], ...over,
});
const summary = (over: Partial<ChangesetSummary>): ChangesetSummary => ({ id: 1, kind: 'new', summary: 's', state: 'proposed', created_at: 1, ...over });

describe('primaryVerb', () => {
  it('bootstrap: adopt n as m layers, skipping needs a look', () => {
    const v = view({ kind: 'bootstrap', items: [
      item({ position: 0, grp: 'core' }), item({ position: 1, grp: 'core', name: 'x' }),
      item({ position: 2, grp: 'authoring', name: 'y' }), item({ position: 3, grp: 'needs a look', name: 'z' }),
      item({ position: 4, grp: 'core', action: 'assign_layer', kind: 'layer', name: 'core' }),
    ] });
    expect(primaryVerb(summary({ kind: 'bootstrap' }), v)).toEqual({ label: 'Adopt 3 as 2 layers', apply: true });
  });
  it('new into a layer, or review for a look card', () => {
    expect(primaryVerb(summary({}), view({ items: [item({})] }))).toEqual({ label: 'Adopt into core', apply: true });
    expect(primaryVerb(summary({}), view({ items: [item({ grp: 'needs a look' })] }))).toEqual({ label: 'Review', apply: false });
  });
  it('rollout to n hosts; drift reviews; layer applies', () => {
    const r = view({ kind: 'rollout', items: [item({ action: 'sync', kind: 'host', name: 'oci' }), item({ position: 1, action: 'sync', kind: 'host', name: 'htz' })] });
    expect(primaryVerb(summary({ kind: 'rollout' }), r)).toEqual({ label: 'Roll out to 2 hosts', apply: true });
    expect(primaryVerb(summary({ kind: 'drift' }), view({ kind: 'drift' }))).toEqual({ label: 'Review diff', apply: false });
    expect(primaryVerb(summary({ kind: 'layer' }), view({ kind: 'layer' }))).toEqual({ label: 'Apply', apply: true });
  });
  it('a closed card has no primary', () => {
    expect(primaryVerb(summary({ state: 'applied' }), null)).toBeNull();
  });
  it('without the full card, falls back to the summary groups', () => {
    expect(primaryVerb(summary({ kind: 'bootstrap', groups: { core: 4, 'needs a look': 2 } }), null)).toEqual({ label: 'Adopt', apply: true });
  });
});

describe('cardMayApply mirrors the backend Additive filter', () => {
  const a = (op: SyncAction['op'], host_copy?: 'unchanged' | 'edited' | 'unverified'): SyncAction => ({
    kind: 'skill', name: 'w', op, reason: null, files: [], merges: [], backup: false, secrets: [], missing_secrets: [], catalog: 'personal', host_copy,
  });
  it('creates and adopts; updates only a verified copy; never overwrites or removes', () => {
    expect(cardMayApply(a('create'))).toBe(true);
    expect(cardMayApply(a('adopt'))).toBe(true);
    expect(cardMayApply(a('update', 'unchanged'))).toBe(true);
    expect(cardMayApply(a('update', 'unverified'))).toBe(false);
    expect(cardMayApply(a('update'))).toBe(false);
    expect(cardMayApply(a('overwrite', 'edited'))).toBe(false);
    expect(cardMayApply(a('remove'))).toBe(false);
  });
  it('owns only the card assets from its catalogs', () => {
    expect(cardOwns(a('create'), new Set(['skill/w']), new Set(['personal']))).toBe(true);
    expect(cardOwns(a('create'), new Set(['skill/x']), new Set(['personal']))).toBe(false);
    expect(cardOwns({ ...a('create'), catalog: 'acme' }, new Set(['skill/w']), new Set(['personal']))).toBe(false);
  });
});

describe('words', () => {
  it('says why a copy was held', () => {
    expect(heldWords('edited')).toBe('edited on the host');
    expect(heldWords('unverified')).toBe('synced before fleet recorded file hashes');
    expect(heldWords('differs')).toBe('differs from the catalog');
  });
  it('names an older hub only for an unknown action', () => {
    expect(olderHubWords({ code: 'E_INVALID', message: 'unknown variant `drift_diff`' }, 'show this diff')).toBe(
      'The hub is older than this desktop and cannot show this diff yet — update the hub.',
    );
    expect(olderHubWords({ code: 'E_INVALID', message: 'unknown changesets action propose_layer: list|…' }, 'propose layer changes')).toMatch(/^The hub is older/);
    expect(olderHubWords({ code: 'E_INVALID', message: 'bad scope' }, 'x')).toBeNull();
  });
  it('notes what applying a catalog card does', () => {
    expect(cardNote(view({ kind: 'bootstrap', catalogs: ['personal', 'papayapos'] }))).toBe('2 commits: personal, papayapos. One Undo. No host is touched.');
    expect(cardNote(view({ kind: 'rollout' }))).toBe('Additive only: creates and adopts, updates a copy fleet wrote. Nothing is overwritten or removed.');
  });
});

describe('coveringNewCard', () => {
  it('finds the open New card that imports an identity', () => {
    const views = { 4: view({ id: 4, items: [item({ kind: 'skill', name: 'fresh' })] }) };
    const cards = [summary({ id: 4 })];
    expect(coveringNewCard(cards, views, 'skill', 'fresh')?.id).toBe(4);
    expect(coveringNewCard(cards, views, 'skill', 'other')).toBeNull();
  });
});

describe('mergePlans', () => {
  it('joins host plans for a read-only review', () => {
    const p = (host: string): SyncPlan => ({ id: host, computed_at: 1, hosts: [{ host_alias: host, harness: 'claude', status: 'planned', detail: null, actions: [] }], counts: { create: 1 } });
    const m = mergePlans([p('oci'), p('htz')]);
    expect(m.hosts.map((h) => h.host_alias)).toEqual(['oci', 'htz']);
    expect(m.counts).toEqual({ create: 2 });
  });
});

describe('isUndoBanner', () => {
  it('is an applied, undoable card', () => {
    expect(isUndoBanner(summary({ state: 'applied', undoable: true }))).toBe(true);
    expect(isUndoBanner(summary({ state: 'applied', undoable: false }))).toBe(false);
  });
});
```

Run → FAIL.

- [ ] **Step 4: `assets_cards.ts`**

```ts
// Assets M6: what a card says and offers — its primary verb (R13), its note,
// why a copy was held (R1), and the Rollout review's mirror of the
// backend's Additive filter (R15: `action_allowed` in changesets/apply.rs).
import type { ChangesetSummary, ChangesetView, HeldWhy } from './assets_workspace';
import { isOpenCard } from './assets_workspace';
import type { IpcError } from './result';
import type { SyncAction, SyncPlan } from './assets';

export const NEEDS_A_LOOK = 'needs a look';
const CATALOG_ACTIONS = new Set(['import', 'take_host', 'create_layer', 'rename_layer', 'move_member']);

export interface Verb { label: string; apply: boolean }

export function primaryVerb(card: ChangesetSummary, view: ChangesetView | null | undefined): Verb | null {
  if (!isOpenCard(card)) return null;
  switch (card.kind) {
    case 'bootstrap': {
      if (!view) return { label: 'Adopt', apply: true };
      const imports = view.items.filter((i) => i.action === 'import' && i.grp !== NEEDS_A_LOOK && i.state === 'pending');
      const layers = new Set(imports.map((i) => i.grp));
      return { label: `Adopt ${imports.length} as ${layers.size} layer${layers.size === 1 ? '' : 's'}`, apply: true };
    }
    case 'new': {
      const first = view?.items[0];
      if (!first) return { label: 'Adopt', apply: true };
      if (first.grp === NEEDS_A_LOOK) return { label: 'Review', apply: false };
      if (first.action === 'hide') return { label: 'Hide', apply: true };
      return { label: `Adopt into ${first.grp}`, apply: true };
    }
    case 'rollout': {
      const hosts = new Set((view?.items ?? []).filter((i) => i.state === 'pending').map((i) => i.name));
      return { label: view ? `Roll out to ${hosts.size} host${hosts.size === 1 ? '' : 's'}` : 'Roll out', apply: true };
    }
    case 'drift':
      return { label: 'Review diff', apply: false };
    case 'layer':
      return { label: 'Apply', apply: true };
  }
}

export function cardNote(view: ChangesetView): string {
  if (view.kind === 'rollout') return 'Additive only: creates and adopts, updates a copy fleet wrote. Nothing is overwritten or removed.';
  if (view.kind === 'drift') return 'Taking makes a commit you can undo. Restoring keeps a .fleet-bak copy on the host.';
  const cats = view.catalogs ?? [];
  const n = cats.length;
  if (n === 0) return 'Records verdicts only. No host is touched.';
  return `${n} commit${n === 1 ? '' : 's'}: ${cats.join(', ')}. One Undo. No host is touched.`;
}

export function heldWords(why: HeldWhy): string {
  switch (why) {
    case 'edited': return 'edited on the host';
    case 'unverified': return 'synced before fleet recorded file hashes';
    case 'differs': return 'differs from the catalog';
  }
}

/** A hub before M6 refuses a new action with E_INVALID naming it unknown (Global Constraints, contract). */
export function olderHubWords(e: IpcError, what: string): string | null {
  if (e.code !== 'E_INVALID') return null;
  if (!/unknown (variant|changesets action)/.test(e.message)) return null;
  return `The hub is older than this desktop and cannot ${what} yet — update the hub.`;
}

export function cardOwns(a: SyncAction, assets: Set<string>, catalogs: Set<string>): boolean {
  return assets.has(`${a.kind}/${a.name}`) && !!a.catalog && catalogs.has(a.catalog);
}

/** R15: `action_allowed(OpFilter::Additive, a)`. A moved asset's Create over an
 *  unverified old copy is held by the backend but not visible here; the
 *  card's outcome after apply is authoritative. */
export function cardMayApply(a: SyncAction): boolean {
  if (a.op === 'create' || a.op === 'adopt') return true;
  if (a.op === 'update') return a.host_copy === 'unchanged';
  return false;
}

export function coveringNewCard(
  cards: ChangesetSummary[] | null,
  views: Record<number, ChangesetView>,
  kind: string,
  name: string,
): ChangesetSummary | null {
  for (const c of cards ?? []) {
    if (c.kind !== 'new' || !isOpenCard(c)) continue;
    const v = views[c.id];
    if (v?.items.some((i) => i.kind === kind && i.name === name && i.state === 'pending')) return c;
  }
  return null;
}

/** R15: host plans joined for a read-only review (its id is never applied). */
export function mergePlans(plans: SyncPlan[]): SyncPlan {
  const counts: Record<string, number> = {};
  for (const p of plans) for (const [k, n] of Object.entries(p.counts)) counts[k] = (counts[k] ?? 0) + n;
  return { id: 'review', computed_at: Math.max(0, ...plans.map((p) => p.computed_at)), hosts: plans.flatMap((p) => p.hosts), counts };
}

export function isUndoBanner(c: ChangesetSummary): boolean {
  return c.state === 'applied' && !!c.undoable;
}

/** Positions a card's primary applies: every pending item but "needs a look" — the backend's own default, sent as null. */
export function catalogChanging(view: ChangesetView): boolean {
  return view.items.some((i) => CATALOG_ACTIONS.has(i.action));
}
```

Run → PASS.

- [ ] **Step 5: `assets_layers.ts` — failing tests**

```ts
import { describe, it, expect } from 'vitest';
import { layerFootprint, whyChain, roleIn, acceptanceOf } from './assets_layers';
import type { LayerListing, CatalogStatus } from './assets_workspace';

const L: LayerListing = {
  layers: [
    { name: 'base', axis: 'role' },
    { name: 'server', axis: 'role', extends: 'base' },
    { name: 'core', axis: 'context', members: ['skill/w'] },
  ],
  hosts: [
    { host_alias: 'oci', catalog_id: 1, layer_name: 'server', axis: 'role', position: 0, active: true },
    { host_alias: 'oci', catalog_id: 1, layer_name: 'core', axis: 'context', position: 0, active: true },
    { host_alias: 'htz', catalog_id: 1, layer_name: 'core', axis: 'context', position: 0, active: false },
  ],
};

describe('layerFootprint', () => {
  it('counts active rows and roles that extend a layer', () => {
    const f = layerFootprint(L);
    expect([...f.get('server')!]).toEqual(['oci']);
    expect([...f.get('base')!]).toEqual(['oci']);
    expect([...f.get('core')!]).toEqual(['oci']);
  });
});

describe('whyChain', () => {
  it('walks role → extends to the layer', () => {
    expect(whyChain(L, 'oci', 'base')).toEqual(['role server', 'extends base']);
    expect(whyChain(L, 'oci', 'core')).toEqual(['context core']);
    expect(whyChain(L, 'htz', 'core')).toEqual([]);
  });
});

describe('roleIn', () => {
  it('names the host role in a catalog', () => {
    expect(roleIn(L, 'oci')).toBe('server');
    expect(roleIn(L, 'htz')).toBeNull();
  });
});

describe('acceptanceOf', () => {
  const cat = (over: Partial<CatalogStatus>): CatalogStatus => ({
    id: 2, name: 'acme', org_id: 7, repo_path: '/r', remote_url: null, head_commit: null, last_loaded_at: null,
    state: 'loaded', asset_count: 0, admitted: [], ...over,
  });
  it('personal: all for an org-less host, shared only for an org host, locked', () => {
    const p = cat({ id: 1, name: 'personal', org_id: null });
    expect(acceptanceOf({ alias: 'oci', org_id: null }, p)).toEqual({ state: 'all', locked: true, why: 'personal reaches every host' });
    expect(acceptanceOf({ alias: 'trn', org_id: 7 }, p)).toEqual({ state: 'shared', locked: true, why: 'an org host receives only shared assets' });
  });
  it('own org: all, locked; other org host: none, locked', () => {
    expect(acceptanceOf({ alias: 'trn', org_id: 7 }, cat({}))).toEqual({ state: 'all', locked: true, why: 'via its org' });
    expect(acceptanceOf({ alias: 'trn', org_id: 9 }, cat({}))).toEqual({ state: 'none', locked: true, why: 'a host of another org never receives it' });
  });
  it('org-less host: admitted or not, a toggle', () => {
    expect(acceptanceOf({ alias: 'mef', org_id: null }, cat({ admitted: ['mef'] }))).toEqual({ state: 'all', locked: false, why: 'admitted' });
    expect(acceptanceOf({ alias: 'mef', org_id: null }, cat({}))).toEqual({ state: 'none', locked: false, why: 'not admitted' });
  });
});
```

Run → FAIL.

- [ ] **Step 6: `assets_layers.ts`**

```ts
// Assets M6 (R17, R18): layer footprints, "why is it on <host>?", a host's
// role per catalog and which catalogs a host accepts — mirrors of
// effective.rs `acceptance` and the resolver's role → extends chain.
import type { CatalogStatus, LayerListing } from './assets_workspace';

function parentOf(l: LayerListing, name: string): string | null {
  return l.layers.find((x) => x.name === name)?.extends ?? null;
}

export function layerFootprint(l: LayerListing): Map<string, Set<string>> {
  const out = new Map<string, Set<string>>(l.layers.map((x) => [x.name, new Set<string>()]));
  for (const r of l.hosts) {
    if (!r.active) continue;
    let name: string | null = r.layer_name;
    const seen = new Set<string>();
    while (name && !seen.has(name)) {
      seen.add(name);
      if (!out.has(name)) out.set(name, new Set());
      out.get(name)!.add(r.host_alias);
      name = r.axis === 'role' ? parentOf(l, name) : null;
    }
  }
  return out;
}

export function whyChain(l: LayerListing, host: string, layer: string): string[] {
  for (const r of l.hosts.filter((x) => x.host_alias === host && x.active)) {
    if (r.axis === 'context' && r.layer_name === layer) return [`context ${layer}`];
    if (r.axis === 'role') {
      const chain = [`role ${r.layer_name}`];
      let name: string | null = r.layer_name;
      const seen = new Set<string>();
      while (name && !seen.has(name)) {
        if (name === layer) return chain;
        seen.add(name);
        name = parentOf(l, name);
        if (name) chain.push(`extends ${name}`);
      }
    }
  }
  return [];
}

export function roleIn(l: LayerListing, host: string): string | null {
  return l.hosts.find((r) => r.host_alias === host && r.axis === 'role' && r.active)?.layer_name ?? null;
}

export interface Acceptance { state: 'all' | 'shared' | 'none'; locked: boolean; why: string }

export function acceptanceOf(host: { alias: string; org_id?: number | null }, c: CatalogStatus): Acceptance {
  const org = host.org_id ?? null;
  if (c.org_id === null) {
    return org === null
      ? { state: 'all', locked: true, why: 'personal reaches every host' }
      : { state: 'shared', locked: true, why: 'an org host receives only shared assets' };
  }
  if (org !== null) {
    return org === c.org_id
      ? { state: 'all', locked: true, why: 'via its org' }
      : { state: 'none', locked: true, why: 'a host of another org never receives it' };
  }
  return (c.admitted ?? []).includes(host.alias)
    ? { state: 'all', locked: false, why: 'admitted' }
    : { state: 'none', locked: false, why: 'not admitted' };
}
```

Run → PASS.

- [ ] **Step 7: Wire types, wrappers, stores — failing tests in `assets_workspace.test.ts`**

Replace the M5 `ChangesetItem` literal test (~:110) with `ItemView`, and add:

```ts
describe('card verbs', () => {
  beforeEach(() => { invoke.mockReset(); });
  it('applies with positions only when given', async () => {
    invoke.mockResolvedValue({ id: 3 });
    await applyChangeset(3);
    expect(invoke).toHaveBeenLastCalledWith('catalog_apply_changeset', { args: { id: 3 } });
    await applyChangeset(3, [1]);
    expect(invoke).toHaveBeenLastCalledWith('catalog_apply_changeset', { args: { id: 3, positions: [1] } });
  });
  it('rejects, undoes, dismisses, gets, proposes', async () => {
    invoke.mockResolvedValue({});
    await rejectItems(3, [0, 1]);
    expect(invoke).toHaveBeenLastCalledWith('catalog_reject_changeset_items', { args: { id: 3, positions: [0, 1] } });
    await undoChangeset(3);
    expect(invoke).toHaveBeenLastCalledWith('catalog_undo_changeset', { args: { id: 3 } });
    await dismissChangeset(3);
    expect(invoke).toHaveBeenLastCalledWith('catalog_dismiss_changeset', { args: { id: 3 } });
    await getChangeset(3);
    expect(invoke).toHaveBeenLastCalledWith('catalog_get_changeset', { args: { id: 3 } });
    await proposeChangesets();
    expect(invoke).toHaveBeenLastCalledWith('catalog_propose_changesets', undefined);
    await proposeLayerChange({ op: 'rename', layer: 'core', to: 'base' });
    expect(invoke).toHaveBeenLastCalledWith('catalog_propose_layer_change', { args: { change: { op: 'rename', layer: 'core', to: 'base' } } });
  });
  it('loads the open cards in full', async () => {
    invoke.mockImplementation(async (_c: string, a: { args: { id: number } }) => ({ id: a.args.id, items: [] }));
    await loadOpenCardViews([
      { id: 1, kind: 'new', summary: '', state: 'proposed', created_at: 1 },
      { id: 2, kind: 'new', summary: '', state: 'applied', created_at: 1 },
    ]);
    expect(Object.keys(get(cardViews))).toEqual(['1']);
  });
  it('loads layers for every loaded catalog by name', async () => {
    invoke.mockResolvedValue({ layers: [], hosts: [] });
    await loadAllLayers([
      { id: 1, name: 'personal', org_id: null, repo_path: '', remote_url: null, head_commit: null, last_loaded_at: null, state: 'loaded', asset_count: 0 },
      { id: 2, name: 'acme', org_id: 7, repo_path: '', remote_url: null, head_commit: null, last_loaded_at: null, state: 'problem', asset_count: 0 },
    ]);
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith('catalog_list_layers_in', { args: { name: 'personal' } });
    expect(Object.keys(get(layersByCatalog)!)).toEqual(['personal']);
  });
  it('round-trips the new selection keys', () => {
    for (const s of [{ type: 'layer', catalog: 'acme', name: 'core' }, { type: 'host', alias: 'oci' }] as const) {
      expect(parseKey(keyOf(s))).toEqual(s);
    }
  });
});
```

`invokeCmd(cmd)` with no args calls `invoke(cmd, undefined)`; check `result.ts:11` and match it, or assert with `toHaveBeenLastCalledWith('catalog_propose_changesets')` if it omits the argument. Run → FAIL.

- [ ] **Step 8: Implement in `assets_workspace.ts` and `assets.ts`**

- Types as in Interfaces. Delete `ChangesetItem`, since only its test used it.
- `WorkspaceView` and `RAIL_VIEWS`:

```ts
export type WorkspaceView = 'inbox' | 'layers' | 'hosts' | 'library';
export const RAIL_VIEWS: readonly { id: WorkspaceView; label: string }[] = [
  { id: 'inbox', label: 'Inbox' },
  { id: 'layers', label: 'Layers' },
  { id: 'hosts', label: 'Hosts' },
  { id: 'library', label: 'Library' },
];
```

  `AssetsRail.svelte` and `AssetsWorkspace.svelte` index `counts` by `WorkspaceView`, so `pnpm check` now fails until Task 12 adds the views. To keep this task green, give the workspace's `counts` the two new keys (`layers`: the number of layers across catalogs, `hosts`: the shown hosts). Also extend `Icon.svelte`'s `IconName` union with `'layers' | 'hosts'` and add two simple SVG branches: layers = three stacked rhombi, hosts = a server rack of two rectangles, in the style of the existing branches. `AssetsRail.test.ts:15-21` asserts there is no layers/hosts entry: change it to assert the four entries in order. Until Tasks 12–13, the workspace renders the Library for the two new views, exactly as M5 does for every non-inbox view (`AssetsWorkspace.svelte:357`). Tasks 12 and 13 replace that.
- `Selection` gains `layer` and `host`:
  - `keyOf`: `layer:<catalog>:<name>`, `host:<alias>`;
  - `parseKey`: `host` takes the whole body; `layer` splits on the first `:`.
- Wrappers, all `invokeCmd` with `{ args: {...} }`:

```ts
export function getChangeset(id: number) { return invokeCmd<ChangesetView>('catalog_get_changeset', { args: { id } }); }
export function applyChangeset(id: number, positions?: number[] | null) {
  return invokeCmd<ChangesetView>('catalog_apply_changeset', { args: positions ? { id, positions } : { id } });
}
export function undoChangeset(id: number) { return invokeCmd<ChangesetView>('catalog_undo_changeset', { args: { id } }); }
export function dismissChangeset(id: number) { return invokeCmd<ChangesetView>('catalog_dismiss_changeset', { args: { id } }); }
export function rejectItems(id: number, positions: number[]) {
  return invokeCmd<ChangesetView>('catalog_reject_changeset_items', { args: { id, positions } });
}
export function proposeChangesets() { return invokeCmd<ChangesetSummary[]>('catalog_propose_changesets'); }
export function proposeLayerChange(change: LayerChange) {
  return invokeCmd<ChangesetView>('catalog_propose_layer_change', { args: { change } });
}
export function admitCatalog(host_alias: string, catalog: string) {
  return invokeCmd<string[]>('catalog_admit_catalog', { args: { host_alias, catalog } });
}
export function unadmitCatalog(host_alias: string, catalog: string) {
  return invokeCmd<string[]>('catalog_unadmit_catalog', { args: { host_alias, catalog } });
}
export function listLayersIn(name: string) { return invokeCmd<LayerListing>('catalog_list_layers_in', { args: { name } }); }
export function hostProvenance(host_alias: string) {
  return invokeCmd<ResolutionView>('catalog_host_provenance', { args: { host_alias } });
}
export function driftDiff(a: { host_alias: string; kind: string; name: string; harness?: string | null; catalog?: string | null }) {
  return invokeCmd<DriftDiff>('catalog_drift_diff', {
    args: { ...a, harness: a.harness ?? null, catalog: a.catalog && a.catalog !== PERSONAL ? a.catalog : null },
  });
}

export const cardViews = writable<Record<number, ChangesetView>>({});
export const layersByCatalog = writable<Record<string, LayerListing> | null>(null);

/** R12: every open card in full (cards are few; the views need items). */
export async function loadOpenCardViews(cards: ChangesetSummary[] | null): Promise<void> {
  const open = (cards ?? []).filter(isOpenCard);
  const got = await Promise.all(open.map((c) => getChangeset(c.id)));
  const next: Record<number, ChangesetView> = {};
  got.forEach((r, i) => { if (r.ok) next[open[i].id] = r.value; });
  cardViews.set(next);
}

/** R17: each loaded catalog's layers, by name. */
export async function loadAllLayers(statuses: CatalogStatus[] | null): Promise<void> {
  const loaded = (statuses ?? []).filter((s) => s.state === 'loaded');
  const got = await Promise.all(loaded.map((s) => listLayersIn(s.name)));
  const next: Record<string, LayerListing> = {};
  got.forEach((r, i) => { if (r.ok) next[loaded[i].name] = r.value; });
  layersByCatalog.set(statuses ? next : null);
}
```

- `loadChangesets()` also calls `void loadOpenCardViews(r.ok ? r.value : null)` after setting the store.
- `ChangesetSummary` gains `catalogs?: string[]; withdrawn?: boolean`. `CardKind` gains `'layer'`.
- In `assets.ts`, `SyncAction` gains `catalog?: string | null; host_copy?: 'unchanged' | 'edited' | 'unverified';`.

Run: `pnpm vitest run src/lib/assets_workspace.test.ts src/lib/assets_cards.test.ts src/lib/line_diff.test.ts src/lib/assets_layers.test.ts src/lib/AssetsRail.test.ts` → PASS. `pnpm test` → PASS. `pnpm check` → 0 errors.

- [ ] **Step 9: Commit**

```bash
git add src/lib
git commit -m "feat(assets): M6 data layer — card views and verbs, layer changes, provenance, drift diff; line diff and layer helpers"
```

---

### Task 9: `ChangesetCard` and the card verbs — Inbox, Inspector, `i`, `⌘↵` (R12, R13, R14 for non-restore verbs)

**Files:**
- Create: `src/lib/ChangesetCard.svelte`, `src/lib/ChangesetCard.test.ts`
- Create: `src/lib/ChangesetDetail.svelte`, `src/lib/ChangesetDetail.test.ts`
- Create: `src/lib/card_actions.ts`, `src/lib/card_actions.test.ts` (the verb runner: busy, toasts with Undo, reloads)
- Modify: `src/lib/AssetsInbox.svelte` (+ test): cards render as `ChangesetCard`; recently applied, undoable cards as a banner row
- Modify: `src/lib/AssetInspector.svelte` (+ test): a selected card shows `ChangesetDetail`
- Modify: `src/lib/AssetsWorkspace.svelte` (+ test): verbs wired, `i`, `a` over a covering New card, `⌘↵` = the selected card's primary
- Modify: `src/lib/assets_inbox.ts` (+ test): `cards` section keeps open cards; a new `applied` list for undo banners; `card:` rows filter by `catalog:` (R2)
- Modify: `src/lib/assets_tokens.test.ts` (`GUARDED` += `ChangesetCard.svelte`, `ChangesetDetail.svelte`)

**Interfaces:**
- Consumes (Task 8):
  - `ChangesetSummary`, `ChangesetView`, `ItemView`, `cardViews`;
  - `applyChangeset`, `undoChangeset`, `dismissChangeset`, `rejectItems`, `loadChangesets`, `loadAllLayers`, `catalogStatuses`;
  - `primaryVerb`, `cardNote`, `heldWords`, `coveringNewCard`, `olderHubWords`, `isUndoBanner`, `NEEDS_A_LOOK`;
  - `push`, `pushError` (`./toasts`), `Badge`, `RowName`, `Inspector`.
- Produces:
  - `ChangesetCard` props: `{ card: ChangesetSummary; view: ChangesetView | null; selected: boolean; readOnly: boolean; busy: boolean; onselect(): void; onapply(positions?: number[] | null): void; ondismiss(): void; onundo(): void; onsynchost?(host: string): void }`.
  - `ChangesetDetail` props: `{ view: ChangesetView; readOnly: boolean; busy: boolean; onreject(positions: number[]): void; onapply(positions?: number[] | null): void; onundo(): void; onsynchost?(host: string): void; diff?: Snippet }`. Task 10 fills the `diff` snippet.
  - `card_actions.ts`: `export async function runCardVerb(verb: 'apply' | 'undo' | 'dismiss' | 'reject', id: number, opts: { positions?: number[] | null; setBusy(b: string): void; onchanged(): Promise<void> | void }): Promise<ChangesetView | null>`.
  - testids:
    - card: `card-{id}`, `card-primary-{id}`, `card-dismiss-{id}`, `card-undo-{id}`, `card-look-{id}`, `card-host-{id}-{host}`;
    - detail: `card-item-{id}-{pos}`, `card-reject-{id}-{pos}`, `card-skip-{id}-{grp}`, `card-commits-{id}`.

- [ ] **Step 1: `card_actions.ts` — failing test**

```ts
import { describe, it, expect, vi, beforeEach } from 'vitest';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { get } from 'svelte/store';
import { runCardVerb } from './card_actions';
import { toasts } from './toasts';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;
const VIEW = { id: 3, kind: 'new', summary: 'New on oci: skill/w → core', state: 'applied', created_at: 1, commits: { personal: 'abc' }, undoable: true, items: [] };

describe('runCardVerb', () => {
  beforeEach(() => { invoke.mockReset(); toasts.set([]); });
  it('applies, reloads, and offers Undo in the toast', async () => {
    invoke.mockResolvedValue(VIEW);
    const setBusy = vi.fn();
    const onchanged = vi.fn();
    const v = await runCardVerb('apply', 3, { setBusy, onchanged });
    expect(v?.id).toBe(3);
    expect(setBusy.mock.calls).toEqual([['card'], ['']]);
    expect(onchanged).toHaveBeenCalled();
    const t = get(toasts).at(-1)!;
    expect(t.message).toBe('Applied: New on oci: skill/w → core');
    expect(t.action?.label).toBe('Undo');
  });
  it('words a failed card from its error, not as success', async () => {
    invoke.mockResolvedValue({ ...VIEW, state: 'failed', undoable: false, error: 'oci: unreachable' });
    await runCardVerb('apply', 3, { setBusy: () => {}, onchanged: () => {} });
    expect(get(toasts).at(-1)!.message).toBe('Not applied: oci: unreachable');
  });
  it('words an older hub', async () => {
    invoke.mockRejectedValue({ code: 'E_INVALID', message: 'unknown changesets action propose_layer: list|…' });
    await runCardVerb('apply', 3, { setBusy: () => {}, onchanged: () => {} });
    expect(get(toasts).at(-1)!.message).toMatch(/^The hub is older than this desktop/);
  });
});
```

Use the real toast store's export name: `toasts`, or whatever `toasts.ts` exports for the list. Run → FAIL.

- [ ] **Step 2: `card_actions.ts`**

```ts
// Assets M6 (R12, R13): one place that runs a card verb — busy while it
// runs, the reload after, and the toast: what happened, with Undo when the
// card can be undone. A failed card (a host sync that failed) comes back as
// a view, not an error, and is worded from its error.
import { applyChangeset, dismissChangeset, rejectItems, undoChangeset, type ChangesetView } from './assets_workspace';
import { olderHubWords } from './assets_cards';
import { push, pushError } from './toasts';
import type { Result } from './result';

type Verb = 'apply' | 'undo' | 'dismiss' | 'reject';
const DONE: Record<Verb, string> = { apply: 'Applied', undo: 'Undone', dismiss: 'Dismissed', reject: 'Ignored' };

export async function runCardVerb(
  verb: Verb,
  id: number,
  opts: { positions?: number[] | null; setBusy: (b: string) => void; onchanged: () => Promise<void> | void },
): Promise<ChangesetView | null> {
  opts.setBusy('card');
  let r: Result<ChangesetView>;
  try {
    r =
      verb === 'apply' ? await applyChangeset(id, opts.positions)
      : verb === 'undo' ? await undoChangeset(id)
      : verb === 'dismiss' ? await dismissChangeset(id)
      : await rejectItems(id, opts.positions ?? []);
  } finally {
    opts.setBusy('');
  }
  await opts.onchanged();
  if (!r.ok) {
    const older = olderHubWords(r.error, 'do this');
    if (older) push({ kind: 'error', message: older });
    else pushError(r.error, `${DONE[verb].replace(/ed$/, '')} card ${id}`);
    return null;
  }
  const v = r.value;
  if (verb === 'apply' && v.state === 'failed') {
    push({ kind: 'error', message: `Not applied: ${v.error ?? 'see the card'}` });
  } else {
    push({
      kind: 'info',
      message: `${DONE[verb]}: ${v.summary}`,
      action: verb === 'apply' && v.undoable ? { label: 'Undo', run: () => void runCardVerb('undo', v.id, opts) } : undefined,
    });
  }
  return v;
}
```

The context words for `pushError` are "Apply card 3", "Undo card 3", "Dismiss card 3" and "Ignore card 3". Map them explicitly with a small record rather than the regex if the regex reads oddly. Run → PASS.

- [ ] **Step 3: `ChangesetCard` — failing tests**

`src/lib/ChangesetCard.test.ts`:

```ts
import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';
import ChangesetCard from './ChangesetCard.svelte';
import type { ChangesetSummary, ChangesetView, ItemView } from './assets_workspace';

const item = (o: Partial<ItemView>): ItemView => ({ position: 0, grp: 'core', catalog: 'personal', kind: 'skill', name: 'w', action: 'import', params: {}, decider: 'rule', state: 'pending', ...o });
const boot: ChangesetSummary = { id: 7, kind: 'bootstrap', summary: 'Adopt 3 as 2 layers; 1 need a look', state: 'proposed', created_at: 1, groups: { core: 2, authoring: 1, 'needs a look': 1 }, pending: 4, catalogs: ['personal', 'papayapos'] };
const bootView: ChangesetView = { id: 7, kind: 'bootstrap', summary: boot.summary, state: 'proposed', created_at: 1, commits: {}, undoable: false, catalogs: ['personal', 'papayapos'], items: [
  item({ position: 0 }), item({ position: 1, name: 'x', catalog: 'papayapos' }), item({ position: 2, grp: 'authoring', name: 'y', decider: 'person' }),
  item({ position: 3, grp: 'needs a look', name: 'z', params: { reason: 'copy on oci differs' } }),
] };
const props = (o = {}) => ({ card: boot, view: bootView, selected: false, readOnly: false, busy: false, onselect: vi.fn(), onapply: vi.fn(), ondismiss: vi.fn(), onundo: vi.fn(), ...o });

describe('ChangesetCard', () => {
  it('shows the sentence, one row per group with its count and decider, and the looks as chips', () => {
    render(ChangesetCard, props());
    expect(screen.getByTestId('card-7')).toHaveTextContent('Adopt 3 as 2 layers');
    expect(screen.getByText('core')).toBeInTheDocument();
    expect(screen.getByText('rule')).toBeInTheDocument();
    expect(screen.getByText('rule + person')).toBeInTheDocument();
    expect(screen.getByTestId('card-look-7')).toHaveTextContent('z · copy on oci differs');
  });
  it('applies with its primary and says what applying does', async () => {
    const p = props();
    render(ChangesetCard, p);
    const primary = screen.getByTestId('card-primary-7');
    expect(primary).toHaveTextContent('Adopt 3 as 2 layers');
    expect(primary).toHaveClass('btn--primary');
    await fireEvent.click(primary);
    expect(p.onapply).toHaveBeenCalledWith(null);
    expect(screen.getByText('2 commits: personal, papayapos. One Undo. No host is touched.')).toBeInTheDocument();
  });
  it('dismisses', async () => {
    const p = props();
    render(ChangesetCard, p);
    await fireEvent.click(screen.getByTestId('card-dismiss-7'));
    expect(p.ondismiss).toHaveBeenCalled();
  });
  it('read-only: no verbs', () => {
    render(ChangesetCard, props({ readOnly: true }));
    expect(screen.queryByTestId('card-primary-7')).toBeNull();
    expect(screen.queryByTestId('card-dismiss-7')).toBeNull();
  });
  it('busy: verbs disabled', () => {
    render(ChangesetCard, props({ busy: true }));
    expect(screen.getByTestId('card-primary-7')).toBeDisabled();
  });
  it('a failed card says so in words and keeps its retry', () => {
    render(ChangesetCard, props({ card: { ...boot, state: 'failed', error: 'core: E_IO' }, view: { ...bootView, state: 'failed', error: 'core: E_IO' } }));
    expect(screen.getByRole('alert')).toHaveTextContent('Failed: core: E_IO');
    expect(screen.getByTestId('card-primary-7')).toBeInTheDocument();
  });
  it('a rollout lists its hosts with the copies it held back', () => {
    const r: ChangesetSummary = { id: 9, kind: 'rollout', summary: 'Roll out core to oci, htz', state: 'proposed', created_at: 1, groups: { core: 2 } };
    const rv: ChangesetView = { id: 9, kind: 'rollout', summary: r.summary, state: 'proposed', created_at: 1, commits: {}, undoable: false, items: [
      item({ position: 0, kind: 'host', name: 'oci', action: 'sync', state: 'skipped', outcome: { held: [{ kind: 'skill', name: 'w', why: 'edited' }] } }),
      item({ position: 1, kind: 'host', name: 'htz', action: 'sync' }),
    ] };
    const onsynchost = vi.fn();
    render(ChangesetCard, props({ card: r, view: rv, onsynchost }));
    expect(screen.getByTestId('card-primary-9')).toHaveTextContent('Roll out to 1 host');
    const oci = screen.getByTestId('card-host-9-oci');
    expect(oci).toHaveTextContent('skill/w — edited on the host · sync it yourself');
    fireEvent.click(oci.querySelector('button')!);
    expect(onsynchost).toHaveBeenCalledWith('oci');
  });
  it('an applied, undoable card is a banner with Undo', async () => {
    const p = props({ card: { ...boot, state: 'applied', undoable: true, applied_at: 1 }, view: { ...bootView, state: 'applied', undoable: true, commits: { personal: 'a1b2c3d4', papayapos: '9f0e1d2' } } });
    render(ChangesetCard, p);
    expect(screen.getByTestId('card-7')).toHaveTextContent('personal a1b2c3d');
    await fireEvent.click(screen.getByTestId('card-undo-7'));
    expect(p.onundo).toHaveBeenCalled();
  });
});
```

The "one primary per region" rule is held at workspace level in Step 9. Run → FAIL.

- [ ] **Step 4: `ChangesetCard.svelte`**

```svelte
<script lang="ts">
  import Badge from './Badge.svelte';
  import RowName from './RowName.svelte';
  import type { ChangesetSummary, ChangesetView, ItemView } from './assets_workspace';
  import { cardNote, heldWords, isUndoBanner, NEEDS_A_LOOK, primaryVerb } from './assets_cards';

  /** One card (spec, Changesets; Rulings R12, R13; mockups screen 1): its
   *  sentence; a group table (Bootstrap, New, Layer) or a host table
   *  (Rollout); "needs a look" chips; one primary verb, Dismiss, and a note
   *  on what applying does. An applied, undoable card is a one-line banner
   *  with Undo. State is in words and glyphs, never colour alone. */
  let {
    card,
    view,
    selected,
    readOnly,
    busy,
    onselect,
    onapply,
    ondismiss,
    onundo,
    onsynchost,
  }: {
    card: ChangesetSummary;
    view: ChangesetView | null;
    selected: boolean;
    readOnly: boolean;
    busy: boolean;
    onselect: () => void;
    onapply: (positions?: number[] | null) => void;
    ondismiss: () => void;
    onundo: () => void;
    onsynchost?: (host: string) => void;
  } = $props();

  const key = $derived(`card:${card.id}`);
  const verb = $derived(primaryVerb(card, view));
  const failed = $derived(card.state === 'failed');
  const banner = $derived(isUndoBanner(card));
  const items = $derived(view?.items ?? []);
  const looks = $derived(items.filter((i) => i.grp === NEEDS_A_LOOK && i.state === 'pending'));

  /** Group → {count, deciders, catalogs}, in first-seen order, looks excluded. */
  const groups = $derived.by(() => {
    const out = new Map<string, { n: number; deciders: Set<string>; catalogs: Set<string> }>();
    if (!view) {
      for (const [g, n] of Object.entries(card.groups ?? {})) if (g !== NEEDS_A_LOOK) out.set(g, { n, deciders: new Set(), catalogs: new Set() });
      return out;
    }
    for (const i of items) {
      if (i.grp === NEEDS_A_LOOK || i.action === 'assign_layer') continue;
      const g = out.get(i.grp) ?? { n: 0, deciders: new Set<string>(), catalogs: new Set<string>() };
      g.n += 1;
      g.deciders.add(i.decider);
      if (i.catalog) g.catalogs.add(i.catalog);
      out.set(i.grp, g);
    }
    return out;
  });
  const hostRows = $derived(card.kind === 'rollout' ? items : ([] as ItemView[]));
  const short = (sha: string) => sha.slice(0, 7);
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<section
  class="card"
  class:selected
  class:failed
  class:banner
  aria-label={card.summary}
  data-row-key={key}
  data-testid={`card-${card.id}`}
  tabindex="0"
  onclick={(e) => { if (!(e.target as HTMLElement).closest('button')) onselect(); }}
  onkeydown={(e) => { if ((e.key === 'Enter' || e.key === ' ') && e.target === e.currentTarget) { e.preventDefault(); onselect(); } }}
>
  {#if banner}
    <div class="line">
      <span class="tick" aria-hidden="true">✓</span>
      <RowName name={`Applied: ${card.summary}`} strong />
      <span class="commits">
        {#each Object.entries(view?.commits ?? {}) as [cat, sha] (cat)}<Badge mono label={`${cat} ${short(sha)}`} />{/each}
      </span>
      {#if !readOnly}
        <button type="button" class="btn" data-testid={`card-undo-${card.id}`} disabled={busy} onclick={onundo}>Undo</button>
      {/if}
    </div>
  {:else}
    <header class="line">
      <Badge tone={failed ? 'crit' : 'accent'} glyph={failed ? '✗' : undefined} label={failed ? `${card.kind} · failed` : card.kind} />
      <RowName name={card.summary} strong />
    </header>
    {#if failed}
      <p class="err" role="alert">Failed: {card.error ?? 'unknown error'}</p>
    {/if}
    {#if card.kind === 'rollout'}
      <ul class="table" aria-label="Hosts">
        {#each hostRows as h (h.position)}
          <li class="row" data-testid={`card-host-${card.id}-${h.name}`}>
            <b>{h.name}</b>
            <Badge label={h.grp} />
            <Badge tone={h.state === 'applied' ? 'ok' : h.state === 'skipped' ? 'warn' : 'neutral'} glyph={h.state === 'applied' ? '✓' : h.state === 'skipped' ? '◐' : undefined} label={h.state} />
            {#each h.outcome?.held ?? [] as l (`${l.kind}/${l.name}`)}
              <span class="held">{l.kind}/{l.name} — {heldWords(l.why)} · sync it yourself</span>
            {/each}
            {#if h.outcome?.note}<span class="held">{h.outcome.note}</span>{/if}
            {#if (h.outcome?.held?.length ?? 0) > 0 && onsynchost && !readOnly}
              <button type="button" class="btn btn--quiet" disabled={busy} onclick={() => onsynchost(h.name)}>Sync {h.name}</button>
            {/if}
          </li>
        {/each}
      </ul>
    {:else if card.kind !== 'drift'}
      <ul class="table" aria-label="Groups">
        {#each [...groups] as [g, info] (g)}
          <li class="row">
            <b>{g}</b>
            <span class="num">{info.n}</span>
            {#if info.catalogs.size}<Badge label={[...info.catalogs].join(', ')} />{/if}
            {#if info.deciders.size}<Badge tone="muted" label={[...info.deciders].join(' + ')} />{/if}
          </li>
        {/each}
      </ul>
    {/if}
    {#if looks.length}
      <div class="looks" data-testid={`card-look-${card.id}`}>
        <span class="lbl">{looks.length} need a look</span>
        {#each looks as l (l.position)}<Badge title={l.params.reason ?? ''} label={`${l.name} · ${l.params.reason ?? 'needs a person'}`} />{/each}
      </div>
    {/if}
    {#if !readOnly}
      <footer class="line">
        {#if verb}
          <button
            type="button"
            class="btn btn--primary"
            data-testid={`card-primary-${card.id}`}
            disabled={busy}
            onclick={() => (verb.apply ? onapply(null) : onselect())}
          >{verb.label}</button>
        {/if}
        <button type="button" class="btn btn--quiet" data-testid={`card-dismiss-${card.id}`} disabled={busy} onclick={ondismiss}>Dismiss</button>
        {#if view}<span class="note">{cardNote(view)}</span>{/if}
      </footer>
    {/if}
  {/if}
</section>

<style>
  .card { display: grid; gap: 8px; margin: 8px 12px; padding: 10px 12px; border: 1px solid var(--border); border-radius: var(--radius-md, 8px); background: var(--bg-pane); }
  .card.selected { border-color: var(--accent); box-shadow: 0 0 0 1px var(--accent); }
  .card:focus-visible { outline: var(--ring-w, 2px) solid var(--ring, var(--accent)); outline-offset: var(--ring-offset, 2px); }
  .card.banner { padding: 6px 12px; }
  .line { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; min-width: 0; }
  .table { list-style: none; margin: 0; padding: 0; display: grid; gap: 2px; }
  .row { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; font-size: 12.5px; min-height: 22px; }
  .num { font-variant-numeric: tabular-nums; color: var(--fg-muted); }
  .looks { display: flex; flex-wrap: wrap; gap: 4px; align-items: center; }
  .lbl { font-size: 12px; font-weight: 600; }
  .held { font-size: 12px; color: var(--usage-warn); }
  .err { margin: 0; color: var(--usage-crit); font-size: 12.5px; }
  .note { font-size: 12px; color: var(--fg-muted); }
  .tick { color: var(--usage-ok); }
  .commits { display: inline-flex; gap: 4px; }
</style>
```

Notes:
- The rollout's held-host button is `.btn--quiet`, and the "Sync {host}" text names the host.
- The card is a `section` with `tabindex=0` and `data-row-key`, so the M5 `rows()` helper (`[data-row-key]` that match `button, [tabindex]`) walks it with `j`/`k`.

Run → PASS. If `primaryVerb` gives "Roll out to 1 host" while the test expects 1, it counts pending hosts only; oci is skipped, so that is right.

- [ ] **Step 5: `ChangesetDetail` — failing tests**

```ts
import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';
import ChangesetDetail from './ChangesetDetail.svelte';
import type { ChangesetView, ItemView } from './assets_workspace';

const item = (o: Partial<ItemView>): ItemView => ({ position: 0, grp: 'core', catalog: 'personal', kind: 'skill', name: 'w', action: 'import', params: {}, decider: 'rule', state: 'pending', ...o });
const v: ChangesetView = { id: 7, kind: 'bootstrap', summary: 'Adopt', state: 'proposed', created_at: 1, commits: {}, undoable: false, items: [
  item({ position: 0 }), item({ position: 1, name: 'x' }), item({ position: 2, grp: 'authoring', name: 'y', state: 'rejected' }),
] };
const props = (o = {}) => ({ view: v, readOnly: false, busy: false, onreject: vi.fn(), onapply: vi.fn(), onundo: vi.fn(), ...o });

describe('ChangesetDetail', () => {
  it('lists every item by group with its action, decider and state', () => {
    render(ChangesetDetail, props());
    expect(screen.getByTestId('card-item-7-0')).toHaveTextContent('skill/w');
    expect(screen.getByTestId('card-item-7-0')).toHaveTextContent('import');
    expect(screen.getByTestId('card-item-7-2')).toHaveTextContent('rejected');
  });
  it('rejects one pending item, or skips a whole group', async () => {
    const p = props();
    render(ChangesetDetail, p);
    await fireEvent.click(screen.getByTestId('card-reject-7-1'));
    expect(p.onreject).toHaveBeenLastCalledWith([1]);
    await fireEvent.click(screen.getByTestId('card-skip-7-core'));
    expect(p.onreject).toHaveBeenLastCalledWith([0, 1]);
    expect(screen.queryByTestId('card-reject-7-2')).toBeNull();
  });
  it('an applied card lists its commits and offers Undo when undoable', async () => {
    const p = props({ view: { ...v, state: 'applied', undoable: true, commits: { personal: 'abcdef1234' } } });
    render(ChangesetDetail, p);
    expect(screen.getByTestId('card-commits-7')).toHaveTextContent('personal abcdef1');
    await fireEvent.click(screen.getByRole('button', { name: 'Undo' }));
    expect(p.onundo).toHaveBeenCalled();
  });
  it('read-only: no reject, skip or undo', () => {
    render(ChangesetDetail, props({ readOnly: true }));
    expect(screen.queryByTestId('card-reject-7-0')).toBeNull();
    expect(screen.queryByTestId('card-skip-7-core')).toBeNull();
  });
});
```

Run → FAIL.

- [ ] **Step 6: `ChangesetDetail.svelte`**

```svelte
<script lang="ts">
  import type { Snippet } from 'svelte';
  import Badge from './Badge.svelte';
  import type { ChangesetView, ItemView } from './assets_workspace';
  import { heldWords } from './assets_cards';

  /** A card in the Inspector (R12): every item by group, with per-item ✕
   *  (reject — a person's verdict: it sticks until the content changes) and
   *  "Skip this group"; an applied card's commits and Undo; a Drift card's
   *  diff (the `diff` snippet, Task 10). */
  let {
    view,
    readOnly,
    busy,
    onreject,
    onapply,
    onundo,
    onsynchost,
    diff,
  }: {
    view: ChangesetView;
    readOnly: boolean;
    busy: boolean;
    onreject: (positions: number[]) => void;
    onapply: (positions?: number[] | null) => void;
    onundo: () => void;
    onsynchost?: (host: string) => void;
    diff?: Snippet;
  } = $props();

  const open = $derived(view.state === 'proposed' || view.state === 'failed');
  const byGroup = $derived.by(() => {
    const m = new Map<string, ItemView[]>();
    for (const i of view.items) m.set(i.grp, [...(m.get(i.grp) ?? []), i]);
    return m;
  });
  const label = (i: ItemView) => (i.kind === 'host' ? i.name : `${i.kind}/${i.name}`);
</script>

<div class="detail">
  {#if view.kind === 'drift' && diff}{@render diff()}{/if}
  {#each [...byGroup] as [g, items] (g)}
    {@const pending = items.filter((i) => i.state === 'pending').map((i) => i.position)}
    <section class="grp">
      <header>
        <span class="sec-t">{g}</span>
        {#if open && !readOnly && pending.length && view.kind !== 'drift'}
          <button type="button" class="btn btn--quiet" data-testid={`card-skip-${view.id}-${g}`} disabled={busy} onclick={() => onreject(pending)}>Skip this group</button>
        {/if}
      </header>
      <ul>
        {#each items as i (i.position)}
          <li data-testid={`card-item-${view.id}-${i.position}`}>
            <span class="nm">{label(i)}</span>
            <Badge label={i.action.replace('_', ' ')} />
            <Badge tone="muted" label={i.decider} />
            <Badge tone={i.state === 'applied' ? 'ok' : i.state === 'rejected' ? 'muted' : i.state === 'skipped' ? 'warn' : 'neutral'} label={i.state} />
            {#if i.params.reason}<span class="why">{i.params.reason}</span>{/if}
            {#each i.outcome?.held ?? [] as l (`${l.kind}/${l.name}`)}<span class="why">{l.kind}/{l.name} — {heldWords(l.why)} · sync it yourself</span>{/each}
            {#if i.outcome?.note}<span class="why">{i.outcome.note}</span>{/if}
            {#if open && !readOnly && i.state === 'pending' && view.kind !== 'drift'}
              <button type="button" class="btn btn--quiet x" aria-label={`Reject ${label(i)}`} data-testid={`card-reject-${view.id}-${i.position}`} disabled={busy} onclick={() => onreject([i.position])}>✕</button>
            {/if}
          </li>
        {/each}
      </ul>
    </section>
  {/each}
  {#if Object.keys(view.commits).length}
    <p class="commits" data-testid={`card-commits-${view.id}`}>
      {#each Object.entries(view.commits) as [c, sha] (c)}<Badge mono label={`${c} ${sha.slice(0, 7)}`} />{/each}
    </p>
  {/if}
  {#if view.state === 'applied' && view.undoable && !readOnly}
    <button type="button" class="btn" disabled={busy} onclick={onundo}>Undo</button>
  {/if}
</div>

<style>
  .detail { display: grid; gap: 12px; }
  .grp header { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
  .sec-t { font-size: 11px; text-transform: uppercase; letter-spacing: 0.04em; color: var(--fg-muted); }
  ul { list-style: none; margin: 4px 0 0; padding: 0; display: grid; gap: 2px; }
  li { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; font-size: 12.5px; min-height: 22px; }
  .nm { font-weight: 560; }
  .why { color: var(--fg-muted); font-size: 12px; }
  .x { margin-left: auto; }
  .commits { display: flex; gap: 4px; margin: 0; }
</style>
```

`onapply` and `onsynchost` are accepted for Task 10's drift buttons and the Hosts lines. Leave them unused here, and do not destructure unused props if svelte-check warns: use `$props()` with a rest for them, or reference them in Task 10. Run → PASS.

- [ ] **Step 7: Inbox renders cards — update its tests first**

In `AssetsInbox.test.ts`:
- Replace the M5 test "card row holds no buttons" (:33) with: "an open card renders as a ChangesetCard with its verbs", asserting `screen.getByTestId('card-3')` and `screen.getByTestId('card-primary-3')`.
- Add "an applied undoable card is listed under Recently applied", with `inbox-section-applied` and `card-undo-4`.
- Add "a catalog: token hides a card of another catalog" (`catalog:acme` hides a card whose `catalogs` is `['personal']`).

`AssetsInbox` gains these props: `views: Record<number, ChangesetView>`, `busy: boolean`, `oncard: { apply(id: number, positions?: number[] | null): void; dismiss(id: number): void; undo(id: number): void; synchost(host: string): void }`.

`assets_inbox.ts`:
- `InboxSection` gains `'applied'`; `SECTION_LABEL.applied = 'Recently applied'`.
- `buildInbox` puts `isUndoBanner` cards into `applied`, in the same row shape as cards (`key: card:<id>`).
- A card row's `query` becomes `{ catalog: c.catalogs ?? [] }`. Extend `keep`'s card handling: `keepCard` keeps the free-text rule and adds `catalog:` matching against `catalogs`. The other tokens still cannot exclude a card (no host, scope or state of its own).
- Update `assets_inbox.test.ts` for both.

In `AssetsInbox.svelte`, replace `cardRow`'s body with:

```svelte
    <ChangesetCard
      card={c}
      view={views[c.id] ?? null}
      selected={selectedKey === r.key}
      readOnly={readonly}
      {busy}
      onselect={() => onselect(r.key)}
      onapply={(p) => oncard.apply(c.id, p)}
      ondismiss={() => oncard.dismiss(c.id)}
      onundo={() => oncard.undo(c.id)}
      onsynchost={oncard.synchost}
    />
```

Render the `applied` section after the cards section, with the same snippet. Run `pnpm vitest run src/lib/AssetsInbox.test.ts src/lib/assets_inbox.test.ts` → PASS.

- [ ] **Step 8: The Inspector shows a card in full**

In `AssetInspector.test.ts`, add: "a selected card shows its items and the reject ✕". Set `cardViews` with one view, render with `selectedKey: 'card:7'`, and expect `card-item-7-0`. In `AssetInspector.svelte`:
- the card branch (:254-265 `inspector-summary`) renders `ChangesetDetail` when `$cardViews[id]` exists, else keeps the M5 summary;
- tabs for a card: `['items']`, or `['items', 'hosts']` for rollout (Hosts shows the same host lines), or `['diff', 'items']` for drift (Task 10 fills Diff);
- props pass through: `oncard` like the Inbox, plus `onreject(id, positions)`.

Run → PASS.

- [ ] **Step 9: The workspace wires verbs and keys — tests first**

In `AssetsWorkspace.test.ts`:
- Rewrite "exactly one primary" (:49-61) as: "one primary in the header, and one more inside the selected card only". Selecting a card makes the header's Sync button `.btn` (not primary), so the selected card's primary is the region's one primary. Implement it with `headerPrimary = !selectedCard` in the workspace header markup.
- Replace "i is not bound in M5" (:415-428) with:
  - "`i` on a card rejects its pending items": a card selected → `catalog_reject_changeset_items` with every pending position;
  - "`i` on an identity a New card covers rejects that card": `cardViews` holds a New card importing `skill/fresh`; select `identity:skill/fresh`; press `i`; expect the reject call for that card;
  - "`i` on anything else does nothing".
- Add "`a` on an identity a New card covers applies that card instead of opening Import": expect `catalog_apply_changeset {id}` and no `onimport`.
- Add "⌘↵ with a card selected runs its primary": `catalog_apply_changeset`, and `onsync` not called.
- Add "⌘↵ with a drift card selected does nothing" (its primary only selects).

Implement in `AssetsWorkspace.svelte`:

```ts
  import { cardViews, loadAllLayers, loadChangesets, catalogStatuses } from './assets_workspace';
  import { coveringNewCard, primaryVerb } from './assets_cards';
  import { runCardVerb } from './card_actions';

  let cardBusy = $state('');
  const anyBusy = $derived(busy !== '' || cardBusy !== '');
  async function changed() {
    await loadChangesets();
    void loadAllLayers($catalogStatuses);
    onrefresh();
  }
  const card = {
    apply: (id: number, positions?: number[] | null) => void runCardVerb('apply', id, { positions, setBusy: (b) => (cardBusy = b), onchanged: changed }),
    dismiss: (id: number) => void runCardVerb('dismiss', id, { setBusy: (b) => (cardBusy = b), onchanged: changed }),
    undo: (id: number) => void runCardVerb('undo', id, { setBusy: (b) => (cardBusy = b), onchanged: changed }),
    reject: (id: number, positions: number[]) => void runCardVerb('reject', id, { positions, setBusy: (b) => (cardBusy = b), onchanged: changed }),
    synchost: (host: string) => onsync({ hostAlias: host }),
  };
  const selectedCard = $derived(selection?.type === 'card' ? ($changesetSummaries ?? []).find((c) => c.id === selection.id) ?? null : null);
  function pendingOf(id: number): number[] {
    return ($cardViews[id]?.items ?? []).filter((i) => i.state === 'pending').map((i) => i.position);
  }
```

In `onKeydown`:
- `⌘↵`: if `selectedCard`, take `const v = primaryVerb(selectedCard, $cardViews[selectedCard.id] ?? null)`. When `v?.apply && !readOnly && !anyBusy`, call `card.apply(selectedCard.id, null)`; otherwise do nothing, and do not fall back to Sync. With no card selected, Sync as before.
- `a`: before the Import branch, `const c = coveringNewCard($changesetSummaries, $cardViews, sel.kind, sel.name); if (c) { e.preventDefault(); card.apply(c.id, null); break; }`.
- the new case:

```ts
      case 'i': {
        if (readOnly || anyBusy) break;
        const id = sel?.type === 'card' ? sel.id
          : sel?.type === 'identity' ? coveringNewCard($changesetSummaries, $cardViews, sel.kind, sel.name)?.id ?? null
          : null;
        const positions = id === null ? [] : pendingOf(id);
        if (id === null || !positions.length) break;
        e.preventDefault();
        card.reject(id, positions);
        break;
      }
```

Pass `views={$cardViews}`, `busy={anyBusy}` and `oncard={card}` to `AssetsInbox`, and `oncard` to `AssetInspector`. The `busy` given to the footer and rail stays the panel's `busy`; the card's own busy shows in the cards.

Run: `pnpm vitest run src/lib/AssetsWorkspace.test.ts` → PASS; `pnpm test` → PASS; `pnpm check` → 0.

- [ ] **Step 10: Guard list, commit**

Add `ChangesetCard.svelte` and `ChangesetDetail.svelte` to `GUARDED` in `assets_tokens.test.ts`. Run `pnpm test` and `pnpm check`.

```bash
git add src/lib
git commit -m "feat(assets): ChangesetCard — apply, dismiss, reject items, undo from the Inbox and the Inspector; i and ⌘↵ on cards"
```

---

### Task 10: `DiffView` on tokens, and the Drift panel with Take / Restore (R7, R14)

**Files:**
- Modify: `src/lib/DiffView.svelte` (tokens; optional `testid` prop)
- Create: `src/lib/DriftPanel.svelte`, `src/lib/DriftPanel.test.ts`
- Modify: `src/lib/AssetInspector.svelte` (+ test): a drift card's Diff tab renders `DriftPanel`; a drifted host cell links to its card; a behind-the-catalog cell says what will happen
- Modify: `src/lib/assets_tokens.test.ts` (`GUARDED` += `DiffView.svelte`, `DriftPanel.svelte`)

**Interfaces:**
- Consumes: `driftDiff`, `DriftDiff`, `ChangesetView` (Task 8); `unifiedDiff` (Task 8); `olderHubWords`; `ConfirmDialog` (`title, message, confirmLabel, danger, busy, onconfirm, oncancel, confirmTestId`); `fleetSettings`/`settingBool`/`SETTING_KEYS.catalogAuto`.
- Produces:
  - `DriftPanel` props: `{ view: ChangesetView; readOnly: boolean; busy: boolean; onapply(positions: number[]): void }`.
  - testids: `drift-panel`, `drift-file-{path}`, `drift-take`, `drift-restore`, `drift-note`, plus `confirm-restore` on the confirm button.

- [ ] **Step 1: Tokens in `DiffView`**

Replace the four hex uses:

```css
  .row.add { background: color-mix(in srgb, var(--usage-ok) 16%, transparent); }
  .row.add .marker { color: var(--usage-ok); }
  .row.del { background: color-mix(in srgb, var(--usage-crit) 16%, transparent); }
  .row.del .marker { color: var(--usage-crit); }
```

Add `DiffView.svelte` to `GUARDED`. Run `pnpm vitest run src/lib/assets_tokens.test.ts src/lib/files_view.test.ts` → PASS.

- [ ] **Step 2: `DriftPanel` — failing tests**

```ts
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import DriftPanel from './DriftPanel.svelte';
import type { ChangesetView } from './assets_workspace';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;
const view: ChangesetView = {
  id: 12, kind: 'drift', summary: 'skill/w was edited on oci (catalog personal)', state: 'proposed', created_at: 1, commits: {}, undoable: false, catalogs: ['personal'],
  items: [
    { position: 0, grp: 'drift', catalog: 'personal', kind: 'skill', name: 'w', action: 'take_host', params: { host: 'oci', harness: 'claude' }, decider: 'rule', state: 'pending' },
    { position: 1, grp: 'drift', catalog: 'personal', kind: 'skill', name: 'w', action: 'restore', params: { host: 'oci', harness: 'claude' }, decider: 'rule', state: 'pending' },
  ],
};
const props = (o = {}) => ({ view, readOnly: false, busy: false, onapply: vi.fn(), ...o });

beforeEach(() => {
  invoke.mockReset();
  invoke.mockResolvedValue({ host_alias: 'oci', harness: 'claude', files: [{ path: 'skills/w/SKILL.md', catalog: 'a\nb\n', host: 'a\nX\n' }] });
});

describe('DriftPanel', () => {
  it('reads the two texts for the card host and shows their diff', async () => {
    render(DriftPanel, props());
    await screen.findByTestId('drift-file-skills/w/SKILL.md');
    expect(invoke).toHaveBeenCalledWith('catalog_drift_diff', { args: { host_alias: 'oci', kind: 'skill', name: 'w', harness: 'claude', catalog: null } });
    expect(screen.getByTestId('drift-file-skills/w/SKILL.md')).toHaveTextContent('X');
  });
  it('takes the host version with the take position', async () => {
    const p = props();
    render(DriftPanel, p);
    await fireEvent.click(await screen.findByTestId('drift-take'));
    expect(p.onapply).toHaveBeenCalledWith([0]);
    expect(screen.getByTestId('drift-take')).toHaveTextContent("Take oci's version into personal");
  });
  it('restores only after a confirm that names the backup', async () => {
    const p = props();
    render(DriftPanel, p);
    await fireEvent.click(await screen.findByTestId('drift-restore'));
    expect(p.onapply).not.toHaveBeenCalled();
    expect(screen.getByTestId('confirm-dialog')).toHaveTextContent('a .fleet-bak copy is kept');
    await fireEvent.click(screen.getByTestId('confirm-restore'));
    expect(p.onapply).toHaveBeenCalledWith([1]);
  });
  it('an MCP entry says its diff is not shown', async () => {
    invoke.mockResolvedValue({ host_alias: 'oci', harness: 'claude', files: [], merges_only: true });
    render(DriftPanel, props());
    expect(await screen.findByTestId('drift-note')).toHaveTextContent('lives in a config file');
  });
  it('an older hub is named', async () => {
    invoke.mockRejectedValue({ code: 'E_INVALID', message: 'unknown variant `drift_diff`' });
    render(DriftPanel, props());
    expect(await screen.findByTestId('drift-note')).toHaveTextContent('The hub is older than this desktop and cannot show this diff yet');
  });
  it('read-only: the diff without the buttons', async () => {
    render(DriftPanel, props({ readOnly: true }));
    await screen.findByTestId('drift-file-skills/w/SKILL.md');
    expect(screen.queryByTestId('drift-take')).toBeNull();
    expect(screen.queryByTestId('drift-restore')).toBeNull();
  });
  it('a rejected take leaves Restore only', async () => {
    const v = { ...view, items: [{ ...view.items[0], state: 'rejected' as const }, view.items[1]] };
    render(DriftPanel, props({ view: v }));
    await waitFor(() => expect(screen.queryByTestId('drift-take')).toBeNull());
    expect(screen.getByTestId('drift-restore')).toBeInTheDocument();
  });
});
```

Run → FAIL.

- [ ] **Step 3: `DriftPanel.svelte`**

```svelte
<script lang="ts">
  import ConfirmDialog from './ConfirmDialog.svelte';
  import DiffView from './DiffView.svelte';
  import { driftDiff, type ChangesetView, type DriftDiff } from './assets_workspace';
  import { olderHubWords } from './assets_cards';
  import { unifiedDiff } from './line_diff';

  /** A Drift card's diff (spec: "Drift shows a DiffView with Take /
   *  Restore"; mockups screen 2; R7, R14): the catalog's and the host's text
   *  of each file, diffed here; Take imports the host copy into the catalog
   *  (a commit, undoable); Restore puts the catalog copy back on the host
   *  after a confirm — the only overwrite a card makes, with a backup. */
  let { view, readOnly, busy, onapply }: { view: ChangesetView; readOnly: boolean; busy: boolean; onapply: (positions: number[]) => void } = $props();

  const take = $derived(view.items.find((i) => i.action === 'take_host' && i.state === 'pending') ?? null);
  const restore = $derived(view.items.find((i) => i.action === 'restore' && i.state === 'pending') ?? null);
  const subject = $derived(view.items[0]);
  const host = $derived(subject?.params.host ?? '');
  const catalog = $derived(subject?.catalog ?? 'personal');

  let diff = $state<DriftDiff | null>(null);
  let note = $state<string | null>(null);
  let confirming = $state(false);

  $effect(() => {
    const s = subject;
    if (!s) return;
    diff = null;
    note = null;
    void driftDiff({ host_alias: s.params.host ?? '', kind: s.kind, name: s.name, harness: s.params.harness ?? null, catalog: s.catalog ?? null }).then((r) => {
      if (r.ok) {
        diff = r.value;
        if (r.value.merges_only) note = 'This asset lives in a config file (an MCP entry); its diff is not shown, so no secret leaves the host.';
      } else {
        note = olderHubWords(r.error, 'show this diff') ?? `Could not read the two copies: ${r.error.message}`;
      }
    });
  });
</script>

<div class="drift" data-testid="drift-panel">
  {#if note}<p class="note" data-testid="drift-note">{note}</p>{/if}
  {#each diff?.files ?? [] as f (f.path)}
    <section class="file" data-testid={`drift-file-${f.path}`}>
      <header><span class="mono">{f.path}</span><span class="muted">catalog → {host}</span></header>
      {#if f.binary}
        <p class="muted">Binary file: the copies differ.</p>
      {:else}
        {@const u = unifiedDiff(f.catalog, f.host, `catalog/${f.path}`, `${host}/${f.path}`)}
        {#if u}<DiffView diff={u} />{:else}<p class="muted">Identical.</p>{/if}
        {#if f.truncated}<p class="muted">Only the first 256 KiB of each side is compared.</p>{/if}
      {/if}
    </section>
  {/each}
  {#if !readOnly}
    <div class="verbs">
      {#if take}
        <button type="button" class="btn btn--primary" data-testid="drift-take" disabled={busy} onclick={() => onapply([take.position])}>Take {host}'s version into {catalog}</button>
      {/if}
      {#if restore}
        <button type="button" class="btn" data-testid="drift-restore" disabled={busy} onclick={() => (confirming = true)}>Restore catalog version on {host} (backs up)</button>
      {/if}
      <span class="muted small">Taking it makes a commit you can undo. Restoring keeps a .fleet-bak copy on {host}.</span>
    </div>
  {/if}
</div>

{#if confirming && restore}
  <ConfirmDialog
    title="Restore the catalog version?"
    message={`Restore the catalog version of ${subject.kind}/${subject.name} on ${host}? The host copy is overwritten; a .fleet-bak copy is kept.`}
    confirmLabel="Restore"
    danger
    {busy}
    confirmTestId="confirm-restore"
    onconfirm={() => { confirming = false; onapply([restore.position]); }}
    oncancel={() => (confirming = false)}
  />
{/if}

<style>
  .drift { display: grid; gap: 10px; }
  .file { border: 1px solid var(--border); border-radius: var(--radius-sm, 6px); overflow: hidden; }
  .file header { display: flex; justify-content: space-between; gap: 8px; padding: 4px 8px; font-size: 12px; background: var(--bg-pane); border-bottom: 1px solid var(--border); }
  .file :global([data-testid='diff-view']) { max-height: 320px; height: auto; }
  .verbs { display: grid; gap: 6px; justify-items: start; }
  .muted { color: var(--fg-muted); font-size: 12px; margin: 0; }
  .small { font-size: 11.5px; }
  .note { margin: 0; font-size: 12.5px; }
  .mono { font-family: var(--mono, ui-monospace, monospace); }
</style>
```

Run → PASS.

- [ ] **Step 4: The Inspector — failing tests, then wiring**

In `AssetInspector.test.ts`, add three tests:
- "a drift card's Diff tab shows the DriftPanel and applies Take through oncard": select `card:12` with `cardViews[12] = view`; expect `drift-panel`; click `drift-take`; expect `oncard.apply` called with `(12, [0])`.
- "a host copy edited on the host links to its drift card": an asset whose host state is `drifted` with `drift_side: 'host'` on `oci`, and an open drift card whose items name it and `oci`. The Hosts tab shows `inspector-drift-link-oci`, and clicking it calls `onselect('card:12')`.
- "a copy behind the catalog says the next sync brings it up to date": with `catalog.auto` on, the text is "Behind the catalog — fleet updates it automatically"; with it off, "Behind the catalog — your next Sync updates it".

Wiring:
- The drift branch of the card Inspector renders `ChangesetDetail` with `diff` = a snippet that renders `DriftPanel {view} {readOnly} busy onapply={(p) => oncard.apply(view.id, p)}`.
- Its tabs are `['diff', 'items']`, opening on Diff.
- `AssetInspector` gains `onselect(key: string)` for the drift link; the workspace passes `select`.
- The Hosts tab of the summary path (`AssetInspector.svelte`, the `drift_side` words from M5) and `AssetDetail.svelte`'s matrix cell title (M5 final-review minor 2) both use `driftSideWords` plus the link or the behind sentence.

Run: `pnpm vitest run src/lib/AssetInspector.test.ts src/lib/DriftPanel.test.ts` → PASS; `pnpm test`; `pnpm check`.

- [ ] **Step 5: Commit**

```bash
git add src/lib
git commit -m "feat(assets): DiffView on tokens; Drift panel with Take and a confirmed Restore"
```

---

### Task 11: `SyncPlanView` (non-modal), the Rollout review, and the footer's live region (R15, R16; carries T10, advice 5)

**Files:**
- Create: `src/lib/SyncPlanView.svelte` (from `SyncPlanDialog.svelte`'s body), `src/lib/SyncPlanView.test.ts` (from `SyncPlanDialog.test.ts`, plus review tests)
- Delete: `src/lib/SyncPlanDialog.svelte`, `src/lib/SyncPlanDialog.test.ts`
- Modify: `src/lib/AssetsPanel.svelte` (+ test): a plan opens the view inside the workspace instead of the modal
- Modify: `src/lib/AssetsWorkspace.svelte` (+ test): `plan` / `review` props; while one is open the main column shows `SyncPlanView` with Back and `Esc`
- Modify: `src/lib/ChangesetDetail.svelte` (+ test): a Rollout card's Hosts tab has **Review plan**
- Modify: `src/lib/AssetsFooter.svelte`, `src/lib/JobChip.svelte` (+ tests): a persistent `role=status` region
- Modify: `src/lib/assets_tokens.test.ts` (`GUARDED`: `SyncPlanDialog` → `SyncPlanView`)

**Interfaces:**
- Consumes: `planSync`, `applySync`, `isDestructive`, `syncProgress` (`assets.ts`); `mergePlans`, `cardOwns`, `cardMayApply` (Task 8).
- Produces:
  - `SyncPlanView` props: `{ plan: SyncPlan; filter?: {hostAlias?, kind?, name?}; mode?: 'sync' | 'review'; owned?: { assets: Set<string>; catalogs: Set<string> } | null; onclose(): void; onapplied?(s: SyncRunSummary): void; onopensecrets?(): void; onapplying?(b: boolean): void; onreplanned?(p: SyncPlan): void }`.
  - Every M5 testid stays the same (`plan-counts`, `plan-anyway-{host}-{harness}`, `plan-apply` …) on root `sync-plan-view`, plus `plan-back`, `plan-held-{host}-{harness}-{kind}-{name}`, `plan-review-note`.
  - `ChangesetDetail` gains `onreview?(): void`.
  - The workspace gains props `plan: SyncPlan | null`, `planFilter`, `review: { plan: SyncPlan; owned } | null`, `onplanclose()`, `onreview(cardId)`.

- [ ] **Step 1: Move the tests and add the review ones**

`git mv src/lib/SyncPlanDialog.test.ts src/lib/SyncPlanView.test.ts`, then:
- replace `SyncPlanDialog` with `SyncPlanView` and `sync-plan-dialog` with `sync-plan-view`;
- keep every existing test, including Plan anyway host-scoped at :171 and :200;
- add:

```ts
describe('review mode (R15)', () => {
  it('has no Apply, marks what the card would hold, and says so', () => {
    const p = plan([hostPlan('oci', [
      action('w', 'update', { host_copy: 'unverified', catalog: 'personal' }),
      action('v', 'create', { catalog: 'personal' }),
      action('other', 'create', { catalog: 'personal' }),
    ])]);
    render(SyncPlanView, { plan: p, mode: 'review', owned: { assets: new Set(['skill/w', 'skill/v']), catalogs: new Set(['personal']) }, onclose: vi.fn() });
    expect(screen.queryByTestId('plan-apply')).toBeNull();
    expect(screen.getByTestId('plan-held-oci-claude-skill-w')).toHaveTextContent('held — sync it yourself');
    expect(screen.queryByTestId('plan-action-oci-claude-skill-other')).toBeNull();
    expect(screen.getByTestId('plan-review-note')).toHaveTextContent('Roll out applies only');
  });
  it('Back closes it', async () => {
    const onclose = vi.fn();
    render(SyncPlanView, { plan: plan([]), onclose });
    await fireEvent.click(screen.getByTestId('plan-back'));
    expect(onclose).toHaveBeenCalled();
  });
});
```

Extend the `action()` factory to take an optional overrides object. Run → FAIL.

- [ ] **Step 2: `SyncPlanView.svelte`**

1. `git mv src/lib/SyncPlanDialog.svelte src/lib/SyncPlanView.svelte`.
2. Replace the `<Modal …>` wrapper with `<section class="plan-view" aria-label="Sync plan" data-testid="sync-plan-view">`, headed by:

```svelte
  <header class="line">
    <button type="button" class="btn btn--quiet" data-testid="plan-back" disabled={applying} onclick={onclose}>← Back</button>
    <h2>{mode === 'review' ? 'Roll-out review' : 'Sync plan'}</h2>
  </header>
```

3. In review mode:
   - filter each host's actions to `owned ? cardOwns(a, owned.assets, owned.catalogs) : true`;
   - for an owned action with `!cardMayApply(a)`, render its row with `data-testid="plan-held-{host}-{harness}-{kind}-{name}"` and a `Badge tone="warn" glyph="◐" label="held — sync it yourself"`;
   - hide the Apply, force-partial and Cancel controls;
   - show `<p class="muted" data-testid="plan-review-note">Roll out applies only creates, adopts and updates of copies fleet wrote; a held copy waits for your own Sync of that host.</p>`.
4. Keep sync mode exactly as the dialog behaved: Apply, forcePartial, cancel, outcomes, and the restart strip.
5. Styles move unchanged. The view is full-height inside the main column (`overflow:auto`).

- [ ] **Step 3: Panel and workspace — tests then wiring**

`AssetsPanel.test.ts`: where it expected `sync-plan-dialog`, it now expects `sync-plan-view` inside `assets-workspace`; the list is hidden while the plan is open.

`AssetsWorkspace.test.ts`:
- "while a plan is open the list is replaced and Esc closes it": render with `plan`, press `Escape` on the root, expect `onplanclose`;
- "Review plan on a Rollout card plans each of its hosts host-scoped": `ChangesetDetail`'s Review button → `catalog_plan_sync` called once per pending host with `{ host_alias: 'oci', … }` and `{ host_alias: 'htz', … }`, then `sync-plan-view` in review mode.

Wiring:
- **The panel.** `AssetsPanel` stops rendering `{#if syncPlan}<SyncPlanDialog …/>`. It passes `plan={syncPlan}`, `planFilter={syncFilter}` and `onplanclose={() => (syncPlan = null)}` to the workspace, with the same `onapplied`, `onopensecrets`, `onapplying` and `onreplanned` as before.
- **The workspace.** Where `listing && view === …` renders the body, put `{#if review}<SyncPlanView mode="review" … />{:else if plan}<SyncPlanView … />{:else}…the views…{/if}`.
  - **Esc.** In `onKeydown`, `Escape` with a plan or review open (and not in a field) calls `preventDefault()` and `stopPropagation()`, then closes it. This keeps the App from closing the Assets overlay.
  - **The review handler.**

```ts
  let review = $state<{ plan: SyncPlan; owned: { assets: Set<string>; catalogs: Set<string> } } | null>(null);
  async function reviewRollout(id: number) {
    const v = $cardViews[id];
    if (!v) return;
    const pending = v.items.filter((i) => i.state === 'pending');
    const plans = await Promise.all(pending.map((i) => planSync({ hostAlias: i.name })));
    const ok = plans.flatMap((r) => (r.ok ? [r.value] : []));
    plans.forEach((r) => { if (!r.ok) pushError(r.error, 'Plan'); });
    review = {
      plan: mergePlans(ok),
      owned: { assets: new Set(pending.flatMap((i) => i.params.assets ?? [])), catalogs: new Set(v.catalogs ?? []) },
    };
  }
```

`ChangesetDetail` shows `<button class="btn" onclick={onreview}>Review plan</button>` for a rollout card that is open. The workspace passes `onreview={() => reviewRollout(view.id)}` through `AssetInspector`.

- [ ] **Step 4: The footer live region (carry T10) — test then implement**

`AssetsFooter.test.ts`:
- "the footer always holds a status region, empty when idle, that names the job": with `busy=''`, `screen.getByTestId('assets-live')` exists and is empty; with `busy='apply'` and `syncProgress` at 2/5, it reads "Syncing 2 of 5 hosts…"; when busy returns to `''` after a job, it reads "Done." until the next job.

In `AssetsFooter.svelte`, add `<span class="sr-only" role="status" aria-live="polite" data-testid="assets-live">{liveText}</span>`, rendered always:
- `liveText` comes from `JOB[busy]` plus the progress;
- it changes to "Done." on the busy → idle edge (tracked with `$effect`), and clears on the next job.

`JobChip` drops its own `role="status"`/`aria-live`, keeping it visual only, so nothing is announced twice. Update `JobChip.test.ts` to match. `.sr-only` is the app's visually-hidden class; if none exists, define it locally (`position:absolute; width:1px; height:1px; overflow:hidden; clip-path: inset(50%); white-space:nowrap`).

Run: `pnpm test` → PASS; `pnpm check` → 0.

- [ ] **Step 5: Commit**

```bash
git add -A src/lib
git commit -m "feat(assets): the sync plan becomes a view in the workspace; Rollout review per host; a persistent footer live region"
```

---

### Task 12: The Layers view (R17, R5, R6)

**Files:**
- Create: `src/lib/AssetsLayers.svelte`, `src/lib/AssetsLayers.test.ts`
- Create: `src/lib/LayerInspector.svelte`, `src/lib/LayerInspector.test.ts`
- Create: `src/lib/LayerChangeForm.svelte`, `src/lib/LayerChangeForm.test.ts`
- Modify: `src/lib/AssetsWorkspace.svelte` (+ test): the `layers` view and its Inspector; "Apply now" after a proposal
- Modify: `src/lib/AssetsPanel.svelte`: `loadAllLayers($catalogStatuses)` in `refresh()`, next to `loadLayers()`
- Modify: `src/lib/assets_tokens.test.ts` (`GUARDED` += the three)

**Interfaces:**
- Consumes: `layersByCatalog`, `catalogStatuses`, `proposeLayerChange`, `proposeChangesets`, `loadChangesets`, `LayerChange`, `keyOf`/`parseKey` (layer keys), `layerFootprint`, `whyChain`, `HostStrip`, `Inspector`, `canWrite`, `olderHubWords`, `push`/`pushError`.
- Produces:
  - `AssetsLayers` props: `{ layers: Record<string, LayerListing> | null; order: string[]; selectedKey: string | null; readOnly: boolean; busy: boolean; onselect(key): void; onnew(): void; onpropose(): void }`. testids `layers-view`, `layers-catalog-{catalog}`, `layer-row-{catalog}-{name}`, `layers-new`, `layers-propose`.
  - `LayerInspector` props: `{ catalog: string; layer: LayerDef; listing: LayerListing; order: string[]; writable: boolean; onchange(c: LayerChange): void }`. Tabs `members`, `hosts`; testids `layer-member-{member}`, `layer-move-{member}`, `layer-rename`, `layer-why-{host}`.
  - `LayerChangeForm` props: `{ mode: 'create' | 'rename' | 'move'; catalogs: string[]; catalog: string; layer?: string; member?: string; layers?: string[]; onsubmit(c: LayerChange): void; oncancel(): void }`. testids `layer-form`, `layer-form-name`, `layer-form-axis`, `layer-form-to`, `layer-form-submit`.

- [ ] **Step 1: Failing tests — `AssetsLayers`**

```ts
import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';
import AssetsLayers from './AssetsLayers.svelte';
import type { LayerListing } from './assets_workspace';

const personal: LayerListing = {
  layers: [{ name: 'core', axis: 'context', members: ['skill/w', 'skill/v'] }, { name: 'server', axis: 'role' }],
  hosts: [{ host_alias: 'oci', catalog_id: 1, layer_name: 'core', axis: 'context', position: 0, active: true }],
};
const acme: LayerListing = { layers: [{ name: 'acme-ops', axis: 'context', members: ['skill/ppt'] }], hosts: [] };
const props = (o = {}) => ({ layers: { personal, acme }, order: ['local', 'oci'], selectedKey: null, readOnly: false, busy: false, onselect: vi.fn(), onnew: vi.fn(), onpropose: vi.fn(), ...o });

describe('AssetsLayers', () => {
  it('groups layers by catalog, personal first, with members and footprint', () => {
    render(AssetsLayers, props());
    const groups = screen.getAllByTestId(/^layers-catalog-/).map((e) => e.dataset.testid);
    expect(groups).toEqual(['layers-catalog-personal', 'layers-catalog-acme']);
    const core = screen.getByTestId('layer-row-personal-core');
    expect(core).toHaveTextContent('2');
    expect(core.querySelector('[role="img"]')?.getAttribute('aria-label')).toContain('oci');
  });
  it('selects a layer by key', async () => {
    const p = props();
    render(AssetsLayers, p);
    await fireEvent.click(screen.getByTestId('layer-row-acme-acme-ops'));
    expect(p.onselect).toHaveBeenCalledWith('layer:acme:acme-ops');
  });
  it('offers New layer and Propose again, not when read-only', async () => {
    const p = props();
    const { unmount } = render(AssetsLayers, p);
    await fireEvent.click(screen.getByTestId('layers-new'));
    await fireEvent.click(screen.getByTestId('layers-propose'));
    expect(p.onnew).toHaveBeenCalled();
    expect(p.onpropose).toHaveBeenCalled();
    unmount();
    render(AssetsLayers, props({ readOnly: true }));
    expect(screen.queryByTestId('layers-new')).toBeNull();
  });
  it('says when nothing is loaded', () => {
    render(AssetsLayers, props({ layers: {} }));
    expect(screen.getByTestId('layers-view')).toHaveTextContent('No layers yet');
  });
});
```

- [ ] **Step 2: `AssetsLayers.svelte`**

```svelte
<script lang="ts">
  import Badge from './Badge.svelte';
  import HostStrip from './HostStrip.svelte';
  import { PERSONAL, keyOf, type LayerListing } from './assets_workspace';
  import { layerFootprint } from './assets_layers';

  /** Layers (spec, Workspace shell; mockups screen 3; R17): every loaded
   *  catalog's layers, grouped by catalog, each with its member count and
   *  footprint (the hosts it reaches). Create / rename / move produce cards
   *  (R5), never direct commits. */
  let { layers, order, selectedKey, readOnly, busy, onselect, onnew, onpropose }: {
    layers: Record<string, LayerListing> | null;
    order: string[];
    selectedKey: string | null;
    readOnly: boolean;
    busy: boolean;
    onselect: (key: string) => void;
    onnew: () => void;
    onpropose: () => void;
  } = $props();

  const catalogs = $derived(Object.keys(layers ?? {}).sort((a, b) => (a === PERSONAL ? -1 : b === PERSONAL ? 1 : a.localeCompare(b))));
  const total = $derived(catalogs.reduce((n, c) => n + (layers?.[c]?.layers.length ?? 0), 0));
</script>

<div class="layers" data-testid="layers-view">
  <div class="head">
    <span class="sentence">{total} layer{total === 1 ? '' : 's'} across {catalogs.length} catalog{catalogs.length === 1 ? '' : 's'}</span>
    {#if !readOnly}
      <button type="button" class="btn" data-testid="layers-new" disabled={busy} onclick={onnew}>New layer</button>
      <button type="button" class="btn btn--quiet" data-testid="layers-propose" disabled={busy} onclick={onpropose}>Propose again</button>
    {/if}
  </div>
  {#if total === 0}
    <p class="quiet">No layers yet. Adopt assets from the Inbox, or make one with New layer.</p>
  {/if}
  {#each catalogs as cat (cat)}
    {@const l = layers![cat]}
    {@const foot = layerFootprint(l)}
    <section data-testid={`layers-catalog-${cat}`}>
      <h3 class="grp">{cat} <span class="n">{l.layers.length}</span></h3>
      {#each l.layers as layer (layer.name)}
        {@const key = keyOf({ type: 'layer', catalog: cat, name: layer.name })}
        <button
          type="button"
          class="row"
          class:selected={selectedKey === key}
          aria-current={selectedKey === key ? 'true' : undefined}
          data-row-key={key}
          data-testid={`layer-row-${cat}-${layer.name}`}
          onclick={() => onselect(key)}
        >
          <b>{layer.name}</b>
          <Badge tone="muted" label={layer.axis} />
          <span class="num">{layer.members?.length ?? 0}</span>
          <HostStrip {order} present={[...(foot.get(layer.name) ?? [])]} />
        </button>
      {/each}
    </section>
  {/each}
</div>

<style>
  .layers { display: grid; align-content: start; }
  .head { display: flex; align-items: center; gap: 8px; padding: 8px 12px; }
  .sentence { flex: 1; font-weight: 600; }
  .grp { margin: 0; padding: 8px 12px 4px; font-size: 11px; text-transform: uppercase; letter-spacing: 0.04em; color: var(--fg-muted); }
  .n { font-variant-numeric: tabular-nums; }
  .row { display: grid; grid-template-columns: minmax(0, 1fr) auto 44px auto; gap: 8px; align-items: center; width: 100%; padding: 4px 12px; border: 0; background: none; text-align: left; font: inherit; }
  .row.selected { background: var(--accent-soft); }
  .row:focus-visible { outline: var(--ring-w, 2px) solid var(--ring, var(--accent)); outline-offset: -2px; }
  .num { text-align: right; font-variant-numeric: tabular-nums; color: var(--fg-muted); }
  .quiet { padding: 12px; color: var(--fg-muted); }
</style>
```

`HostStrip`'s `present` gives filled dots, and the others draw absent. Its `role="img"` `aria-label` names the hosts, which the test reads. Run → PASS.

- [ ] **Step 3: `LayerChangeForm` — tests then component**

```ts
import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';
import LayerChangeForm from './LayerChangeForm.svelte';

describe('LayerChangeForm', () => {
  it('creates a context layer in the chosen catalog', async () => {
    const onsubmit = vi.fn();
    render(LayerChangeForm, { mode: 'create', catalogs: ['personal', 'acme'], catalog: 'personal', onsubmit, oncancel: vi.fn() });
    await fireEvent.input(screen.getByTestId('layer-form-name'), { target: { value: 'servers' } });
    await fireEvent.click(screen.getByTestId('layer-form-submit'));
    expect(onsubmit).toHaveBeenCalledWith({ op: 'create', catalog: 'personal', layer: 'servers', axis: 'context' });
  });
  it('refuses an invalid name before submitting', async () => {
    const onsubmit = vi.fn();
    render(LayerChangeForm, { mode: 'create', catalogs: ['personal'], catalog: 'personal', onsubmit, oncancel: vi.fn() });
    await fireEvent.input(screen.getByTestId('layer-form-name'), { target: { value: 'Bad Name' } });
    expect(screen.getByTestId('layer-form-submit')).toBeDisabled();
    expect(screen.getByTestId('layer-form')).toHaveTextContent('lowercase letters, digits and dashes');
  });
  it('renames and moves', async () => {
    const onsubmit = vi.fn();
    const { unmount } = render(LayerChangeForm, { mode: 'rename', catalogs: ['personal'], catalog: 'personal', layer: 'core', onsubmit, oncancel: vi.fn() });
    await fireEvent.input(screen.getByTestId('layer-form-name'), { target: { value: 'base' } });
    await fireEvent.click(screen.getByTestId('layer-form-submit'));
    expect(onsubmit).toHaveBeenLastCalledWith({ op: 'rename', catalog: 'personal', layer: 'core', to: 'base' });
    unmount();
    render(LayerChangeForm, { mode: 'move', catalogs: ['personal'], catalog: 'personal', layer: 'core', member: 'skill/w', layers: ['core', 'extra'], onsubmit, oncancel: vi.fn() });
    await fireEvent.change(screen.getByTestId('layer-form-to'), { target: { value: 'extra' } });
    await fireEvent.click(screen.getByTestId('layer-form-submit'));
    expect(onsubmit).toHaveBeenLastCalledWith({ op: 'move', catalog: 'personal', member: 'skill/w', layer: 'core', to: 'extra' });
  });
});
```

Component:
- an inline `<form data-testid="layer-form">` (not a modal) with `onsubmit` preventDefault;
- fields by mode: create has catalog select, name, axis select (context/role); rename has name; move has a `to` select of `layers` minus the current one;
- the name check mirrors `check_layer_name`. Check `validate.rs:26` and copy its rule, e.g. `/^[a-z0-9][a-z0-9-]*$/`. The error text is "Use lowercase letters, digits and dashes.";
- submit is disabled while invalid;
- `.btn--primary` labelled "Propose", plus a Cancel `.btn--quiet`;
- `catalog` is omitted from the emitted change when it is `personal`? No — always send it. The tests above include `catalog: 'personal'`, and the backend treats it the same.

Run → PASS.

- [ ] **Step 4: `LayerInspector` — tests then component**

Tests:
- Members tab lists `layer-member-skill/w`, and clicking `layer-move-skill/w` shows the move form;
- Hosts tab shows `layer-why-oci` with "role server → extends base", using `whyChain` joined with " → ";
- `layer-rename` opens the rename form, and its submit calls `onchange` with the change;
- not writable: no rename and no move.

Component:
- uses `Inspector` (eyebrow `Layer · catalog {catalog}`, title = name, tabs `members` / `hosts`);
- Members: the members, each with a Move… `.btn--quiet` when `writable`;
- Hosts: `HostStrip` plus one line per host in the footprint, "Why is it on {host}?" with the chain in a `<code>` block (mockup screen 3);
- a header Rename button;
- a form slot shown inline under the header, with `LayerChangeForm`.

- [ ] **Step 5: The workspace — tests then wiring**

`AssetsWorkspace.test.ts`:
- "the Layers rail entry shows the Layers view": `layersByCatalog` set; click `assets-rail-layers`; expect `layers-view`;
- "proposing a layer change selects its card in the Inbox and offers Apply now":
  - mock `catalog_propose_layer_change` → a view `{id: 21, kind: 'layer', …}`;
  - the change goes through the layer form, opened with `layers-new`;
  - the view becomes `inbox` and `selectedKey` becomes `card:21`;
  - a toast "Card ready: New layer servers in personal" appears with action "Apply now", whose `run` calls `catalog_apply_changeset {id: 21}`;
- "an older hub refusing propose_layer is worded": the toast matches `/^The hub is older/`.

Wiring:
- The body adds `{:else if view === 'layers'}<AssetsLayers … />`.
- With a `layer:` selection, the Inspector column renders `LayerInspector`; pass `writable={canWrite(catalog, ctx)}`.
- "New layer" toggles a `creating` state that renders `LayerChangeForm mode="create"` at the top of the Layers view. Its `catalogs` are the loaded catalogs this window may write.
- `onpropose` → `proposeChangesets()`, then `loadChangesets()` and a toast "Proposed again: {n} open cards".
- The shared handler:

```ts
  async function proposeChange(c: LayerChange) {
    cardBusy = 'card';
    const r = await proposeLayerChange(c);
    cardBusy = '';
    if (!r.ok) {
      const older = olderHubWords(r.error, 'propose layer changes');
      if (older) push({ kind: 'error', message: older });
      else pushError(r.error, 'Propose');
      return;
    }
    await loadChangesets();
    view = 'inbox';
    selectedKey = `card:${r.value.id}`;
    push({ kind: 'info', message: `Card ready: ${r.value.summary}`, action: { label: 'Apply now', run: () => card.apply(r.value.id, null) } });
  }
```

`counts.layers` = the total number of layers.

Run: `pnpm test` → PASS; `pnpm check` → 0.

- [ ] **Step 6: Commit**

```bash
git add src/lib
git commit -m "feat(assets): the Layers view — layers by catalog with footprints, why-is-it-here, and layer changes as cards"
```

---

### Task 13: The Hosts view inside Assets — org, role per catalog, admissions, provenance (R18)

**Files:**
- Create: `src/lib/AssetsHosts.svelte`, `src/lib/AssetsHosts.test.ts`
- Create: `src/lib/HostInspector.svelte`, `src/lib/HostInspector.test.ts`
- Modify: `src/lib/AssetsWorkspace.svelte` (+ test): the `hosts` view and its Inspector
- Modify: `src/lib/assets_tokens.test.ts`

**Interfaces:**
- Consumes: `hosts` (`HostRow[]`, with `org_id`, `hidden`?), `orgs` (`./orgs`: names by id), `catalogStatuses`, `layersByCatalog`, `admitCatalog`, `unadmitCatalog`, `loadCatalogStatuses`, `hostProvenance`, `acceptanceOf`, `roleIn`.
- Produces:
  - `AssetsHosts` props: `{ hosts: HostRow[]; statuses: CatalogStatus[] | null; layers: Record<string, LayerListing> | null; orgName(id: number | null): string | null; selectedKey: string | null; readOnly: boolean; busy: boolean; onselect(key): void; ontoggle(host: string, catalog: string, on: boolean): void }`. testids `hosts-view`, `host-row-{alias}`, `host-org-{alias}`, `host-role-{alias}-{catalog}`, `host-accept-{alias}-{catalog}`.
  - `HostInspector` props: `{ alias: string }`. It loads its provenance itself. testids `host-prov-{catalog}`, `host-prov-line-{kind}/{name}`, `host-refused`, `host-held-back`.

- [ ] **Step 1: Failing tests — `AssetsHosts`**

```ts
import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';
import AssetsHosts from './AssetsHosts.svelte';
import type { CatalogStatus, LayerListing } from './assets_workspace';

const cat = (o: Partial<CatalogStatus>): CatalogStatus => ({ id: 1, name: 'personal', org_id: null, repo_path: '', remote_url: null, head_commit: null, last_loaded_at: null, state: 'loaded', asset_count: 0, admitted: [], ...o });
const statuses = [cat({}), cat({ id: 2, name: 'papayapos', org_id: 7, admitted: ['local'] })];
const layers: Record<string, LayerListing> = {
  personal: { layers: [], hosts: [{ host_alias: 'oci', catalog_id: 1, layer_name: 'server', axis: 'role', position: 0, active: true }] },
};
const hosts = [{ alias: 'local', org_id: null }, { alias: 'oci', org_id: null }, { alias: 'trn', org_id: 7 }] as never[];
const props = (o = {}) => ({ hosts, statuses, layers, orgName: (id: number | null) => (id === 7 ? 'papayapos' : null), selectedKey: null, readOnly: false, busy: false, onselect: vi.fn(), ontoggle: vi.fn(), ...o });

describe('AssetsHosts', () => {
  it('shows each host org and its role per catalog', () => {
    render(AssetsHosts, props());
    expect(screen.getByTestId('host-org-trn')).toHaveTextContent('papayapos');
    expect(screen.getByTestId('host-org-oci')).toHaveTextContent('no org');
    expect(screen.getByTestId('host-role-oci-personal')).toHaveTextContent('role server');
  });
  it('admission toggles follow acceptance: locked for personal and own org, a toggle for an org-less host', async () => {
    const p = props();
    render(AssetsHosts, p);
    const personal = screen.getByTestId('host-accept-oci-personal');
    expect(personal).toBeDisabled();
    expect(personal).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByTestId('host-accept-trn-papayapos')).toBeDisabled();
    expect(screen.getByTestId('host-accept-trn-personal')).toHaveTextContent('shared only');
    const local = screen.getByTestId('host-accept-local-papayapos');
    expect(local).toHaveAttribute('aria-pressed', 'true');
    await fireEvent.click(local);
    expect(p.ontoggle).toHaveBeenCalledWith('local', 'papayapos', false);
    await fireEvent.click(screen.getByTestId('host-accept-oci-papayapos'));
    expect(p.ontoggle).toHaveBeenLastCalledWith('oci', 'papayapos', true);
  });
  it('read-only: no toggles, the state in words', () => {
    render(AssetsHosts, props({ readOnly: true }));
    expect(screen.getByTestId('host-accept-local-papayapos')).toBeDisabled();
  });
});
```

Run → FAIL.

- [ ] **Step 2: `AssetsHosts.svelte`**

Rows are `button.row` with `data-row-key="host:<alias>"`. Each row holds:
- its alias;
- an org `Badge`: the org name, or "no org" in muted tone;
- one `host-role-{alias}-{catalog}` line per catalog that has a role row (`role {name}`);
- a toggle per catalog: `<button type="button" class="btn btn--chip" aria-pressed={state !== 'none'} disabled={locked || readOnly || busy} title={why}>`.

The toggle's label is the catalog name, plus " · shared only" when `state === 'shared'`; a glyph `●`/`○` carries the state, so it is not colour alone. Clicking it calls `ontoggle(alias, catalog, state === 'none')`. Hosts with `hidden` are left out, as everywhere in Assets (`shown` in the workspace). The toggle is a `button` nested inside the row button: that is invalid HTML. So the row is a `div role="button" tabindex="0"`, the `AssetList` static-row pattern, with its own Enter/Space handler, and the toggles are real buttons inside it.

Run → PASS.

- [ ] **Step 3: `HostInspector` — tests then component**

Tests (mock `catalog_host_provenance`):
- "groups the effective set by catalog with the layer that brought each asset": provenance `{ 'skill/w': { introduced_by: 'core', catalog: 'personal' }, 'skill/ppt': { introduced_by: 'acme-ops', catalog: 'papayapos' } }` → `host-prov-personal` holds `host-prov-line-skill/w` with the text "skill/w — via layer core from personal";
- "lists refused and held-back catalogs": `refused: [{ kind: 'skill', name: 'x', reason: 'private asset; oci belongs to an org' }]` → `host-refused` has that text; `held_back: { papayapos: 'failed to load' }` → `host-held-back` reads "papayapos: failed to load — its assets are left as they are";
- "an older hub or a refusal is said in words".

Component: `Inspector`, with eyebrow `Host`, title = alias, and one tab `effective`. An `$effect` on `alias` loads `hostProvenance(alias)`. Group `provenance` entries by `.catalog`, sorted with personal first. Each line is `{key} — via layer {introduced_by} from {catalog}`, plus ` (overridden by …)` when `overridden_by` is non-empty.

- [ ] **Step 4: The workspace — tests then wiring**

`AssetsWorkspace.test.ts`:
- "the Hosts rail entry shows the Hosts view and a host selection shows its provenance";
- "toggling an admission admits, reloads the catalogs, and says so": `catalog_admit_catalog {host_alias:'oci', catalog:'papayapos'}`, then `catalog_list_catalogs` is called again, and a toast "oci now receives papayapos";
- "unadmitting says the host keeps what it has": "local no longer receives papayapos; what is installed stays until you remove it".

`ontoggle` calls `admitCatalog`/`unadmitCatalog` under `cardBusy`, then `loadCatalogStatuses()` and `loadAllLayers(...)`, with the toast words above; on error, `pushError(e, 'Admission')`. `counts.hosts` = `shown.length`. `orgName` comes from `$orgs`; make sure `loadOrgs()` runs in `AssetsPanel.refresh()` if the orgs store is not already loaded app-wide (`App.svelte` loads it; check).

Run: `pnpm test` → PASS; `pnpm check` → 0.

- [ ] **Step 5: Commit**

```bash
git add src/lib
git commit -m "feat(assets): the Hosts view — org, role per catalog, catalog admissions as toggles, and per-host provenance"
```

---

### Task 14: QuickSwitcher — `asset` and `command` kinds (R19)

**Files:**
- Modify: `src/lib/quick_switcher.ts` (+ `quick_switcher.test.ts`): kinds, `assetEntries`, `commandEntries`, ranking
- Modify: `src/lib/QuickSwitcher.svelte` (+ `QuickSwitcher.test.ts`): groups, `pick`
- Modify: `src/lib/app_views.ts` (+ test): `assetsViewRequest`, `requestAssetsView`
- Modify: `src/lib/App.svelte`: open Assets on a request
- Modify: `src/lib/AssetsPanel.svelte` (+ test): consume the request (select, rescan, sync, propose)

**Interfaces:**
- Consumes: `catalog` store (`assets.ts`), `keyOf`, `catalogOf`, `proposeChangesets`, the panel's `onscan`/`requestSync`.
- Produces:
  - `SwitcherEntry.kind` += `'asset' | 'command'`;
  - `SwitcherEntry.asset?: { key: string }`, `SwitcherEntry.command?: AssetsCommand`;
  - `export type AssetsCommand = 'rescan' | 'sync' | 'propose'`;
  - `export function assetEntries(listing: AssetListing | null): SwitcherEntry[]`, with keys `asset:<catalog>:<kind>/<name>`;
  - `export function commandEntries(): SwitcherEntry[]`, with keys `command:rescan|sync|propose` and labels "Rescan assets", "Sync fleet", "Propose cards";
  - `app_views.ts`: `export interface AssetsViewRequest { select?: string; command?: AssetsCommand; at: number }`, `export const assetsViewRequest: Writable<AssetsViewRequest | null>`, `export function requestAssetsView(r: Omit<AssetsViewRequest, 'at'>): void`;
  - testids `switcher-asset`, `switcher-command`.

- [ ] **Step 1: Failing pure tests (`quick_switcher.test.ts`)**

```ts
describe('asset and command entries', () => {
  const listing = { head: null, loaded_at: null, unmanaged: [], problems: [], assets: [
    { kind: 'skill', name: 'infra-status', version: '1', description: 'Check the infra', tags: [], hosts: [], catalog: 'personal' },
    { kind: 'skill', name: 'ppt-implement', version: '1', description: '', tags: [], hosts: [], catalog: 'papayapos' },
  ] } as never;
  it('one row per asset, keyed like the workspace selection', () => {
    const e = assetEntries(listing);
    expect(e.map((x) => x.key)).toEqual(['asset:personal:skill/infra-status', 'asset:papayapos:skill/ppt-implement']);
    expect(e[0]).toMatchObject({ kind: 'asset', label: 'skill/infra-status', meta: 'Assets', description: 'personal · Check the infra' });
  });
  it('three commands', () => {
    expect(commandEntries().map((c) => c.label)).toEqual(['Rescan assets', 'Sync fleet', 'Propose cards']);
  });
  it('an asset name match ranks after sessions, a command after assets', () => {
    const entries = [...buildEntries([sess({ name: 'infra work' })], [], []), ...assetEntries(listing), ...commandEntries()];
    const ranked = rankEntries(entries, 'infra', []);
    expect(ranked.map((r) => r.kind)).toEqual(['session', 'asset']);
    expect(rankEntries(entries, 'sync', []).map((r) => r.label)).toContain('Sync fleet');
  });
});
```

`sess()` is the existing factory in the test file; if it lives only in `QuickSwitcher.test.ts`, inline a minimal one. Run → FAIL.

- [ ] **Step 2: Implement `quick_switcher.ts`**

- Add the kinds and fields.
- `assetEntries`: `fields: [label, a.description, catalogOf(a)]`, `meta: 'Assets'`, `description: `${catalogOf(a)} · ${a.description}`` (trim it when the description is empty: `personal`).
- `commandEntries`: `meta: 'Commands'`, `fields: [label, 'assets', …synonyms]`, with synonyms `scan`, `sync`, `rollout`, `propose`, `cards`, `layers`.
- `rankBase`: after the session/project partition, order `asset` after `host` and `command` last. Partition them like `isTicket` at :218 (an `isTail` predicate), and merge them after the hosts.

Run → PASS.

- [ ] **Step 3: The channel, the switcher and the panel — failing tests**

`app_views.test.ts` (or the existing test for `requestHostsView`): `requestAssetsView({ select: 'asset:personal:skill/w' })` sets the store with an `at` timestamp.

`QuickSwitcher.test.ts`:
- "typing an asset name lists it under Assets, and picking it requests the Assets view with it selected": spy on `requestAssetsView`, or read `get(assetsViewRequest)`;
- "picking Sync fleet requests the command".

`AssetsPanel.test.ts`:
- "a select request selects the row": set `assetsViewRequest` before render, expect the workspace's `selectedKey`, observable as `inspector` showing the asset title;
- "a rescan request runs the scan once": `assets_scan_hosts` is called once, and the request is cleared;
- "a sync request on a read-only client does nothing".

Implement:
- `QuickSwitcher.svelte`:
  - entries = `[...buildEntries(...), ...assetEntries($catalog), ...commandEntries(), ...tickets, ...lookup]`;
  - group headings `Assets` and `Commands` (:133-142);
  - `pick`: `asset` → `requestAssetsView({ select: e.key })`; `command` → `requestAssetsView({ command: e.command })`; then close.
- `App.svelte`: next to the `$hostsViewRequest` effect, `$effect(() => { if ($assetsViewRequest) showAssets(); })`, where `showAssets` (:589) opens the overlay.
- `AssetsPanel.svelte`, an `$effect` on `$assetsViewRequest`:
  - `select` → `selectedKey = r.select`, view `library`;
  - `rescan` → `onscan()` when not `readOnly` and not busy;
  - `sync` → `requestSync({})` under the same guard;
  - `propose` → `proposeChangesets()`, then `loadChangesets()` and view `inbox`;
  - then `assetsViewRequest.set(null)`.

  Give `AssetsWorkspace` a bindable `view` so the panel can set it.

Run: `pnpm test` → PASS; `pnpm check` → 0.

- [ ] **Step 4: Commit**

```bash
git add src/lib
git commit -m "feat(switcher): assets and the Rescan / Sync fleet / Propose commands in the QuickSwitcher"
```

---

### Task 15: The UI carries, verification and docs (R20; carries T5, T10-keys, T11, T12)

**Files:**
- Modify: `src/lib/ImportDialog.svelte:62`, `src/lib/NewAssetDialog.svelte:77`, `src/lib/SecretsPanel.svelte:134`, `src/lib/AuthorSessionDialog.svelte:96`, `src/lib/LintAllDialog.svelte:67,73,75` (tokens)
- Modify: `src/lib/assets_tokens.test.ts` (`GUARDED` += those five and every new M6 component not yet listed; regex covers `rgb(`/`rgba(`/`hsl(`)
- Modify: `src/lib/AssetsWorkspace.svelte` (+ test): narrow layout; `s`/`a`/`e`/`i` scoped to list or Inspector focus
- Modify: `src/lib/assets_workspace.ts` (+ test): `blockedSecretKeys` keyed with the catalog
- Modify: `src/lib/assets_inbox.ts` and its caller: the blocked predicate takes the catalog
- Modify: `CLAUDE.md` (an M6 paragraph after M5's)

- [ ] **Step 1: The token guard — widen and fail**

In `assets_tokens.test.ts`:

```ts
const COLOR_LITERAL = /#[0-9a-fA-F]{3,8}\b|\b(?:rgb|rgba|hsl|hsla)\(/g;
// …
expect(css.match(COLOR_LITERAL) ?? []).toEqual([]);
```

Add to `GUARDED` every file that is not yet listed:
- the five dialogs;
- every new M6 component: `ChangesetCard`, `ChangesetDetail`, `DriftPanel`, `DiffView`, `SyncPlanView`, `AssetsLayers`, `LayerInspector`, `LayerChangeForm`, `AssetsHosts`, `HostInspector`.

Run → FAIL on the five dialogs.

- [ ] **Step 2: Tokens**

| Before | After |
|---|---|
| `#dc2626` | `var(--usage-crit)` |
| `#d97706` | `var(--usage-warn)` |

Run → PASS.

- [ ] **Step 3: `blockedSecretKeys` with the catalog (carry T10) — test then fix**

```ts
  it('keys blocked assets by catalog too', () => {
    const run = { plan_id: 'p', started_at: 1, finished_at: 2, hosts: [{ host_alias: 'oci', harness: 'claude', status: 'partial', detail: null, restart_required: false,
      actions: [{ kind: 'skill', name: 'w', op: 'blocked', outcome: 'blocked', detail: 'missing secrets: TOKEN', catalog: 'acme' }] }] } as never;
    expect(blockedSecretKeys(run)).toEqual(['acme:skill/w']);
    const blocked = blockedOnSecrets(run);
    expect(blocked({ kind: 'skill', name: 'w', catalog: 'acme' })).toBe(true);
    expect(blocked({ kind: 'skill', name: 'w', catalog: 'personal' })).toBe(false);
  });
```

`ActionResult` must carry `catalog`. Check `sync/apply.rs:107-129` (`ActionResult {kind, name, op, outcome, detail}`). If it has no `catalog`, add one there:
- Rust: `ActionResult.catalog: Option<String>` with `#[serde(default, skip_serializing_if = "Option::is_none")]`, filled from the planned `Action.catalog` where results are built;
- one assertion in an existing apply test;
- TS: `ActionResult.catalog?: string | null`.

A result without a catalog keys as `personal:` (pre-M6 hubs). Change `blockedOnSecrets` to `(a: {kind, name, catalog?}) => keys.has(`${a.catalog ?? PERSONAL}:${a.kind}/${a.name}`)`. Run → PASS.

- [ ] **Step 4: Scoped keys and the narrow layout (carries T11, T12) — tests then implement**

`AssetsWorkspace.test.ts`:
- "`s`, `a`, `e` and `i` act only from the list or the Inspector": focus the footer's catalog chip button and press `s` with an own asset selected; `onsync` is not called. Focus the list row and press `s`; it is called.
- "the layout narrows": read the component's `<style>`, as the M5 source-guard tests do, and assert it has `@media (max-width: 1100px)` setting the rail column to `56px`, and `@media (max-width: 860px)` stacking `'main' 'insp'`.

Implement:
- **Scoped keys.** In `onKeydown`, compute `const inScope = !!target && (listEl?.contains(target) || inspEl?.contains(target) || target === rootEl)`, and return early for `a`, `s`, `e` and `i` when `!inScope`. Bind `inspEl` on the Inspector column.
- **Narrow layout.**

```css
  @media (max-width: 1100px) {
    .ws { grid-template-columns: 56px minmax(0, 1fr) minmax(280px, 340px); }
  }
  @media (max-width: 860px) {
    .ws {
      grid-template-columns: 56px minmax(0, 1fr);
      grid-template-rows: minmax(0, 1fr) minmax(0, 45%) auto;
      grid-template-areas: 'rail main' 'rail insp' 'foot foot';
    }
    .insp { border-left: 0; border-top: 1px solid var(--border); }
  }
```

  `AssetsRail.svelte` hides its labels below 1100 px and keeps them as `title`/`aria-label`. Its buttons already carry `aria-label` from the label text. Make sure they keep it when the text hides: use `.lbl { display: none }` at that width, with an `aria-label` on the button.

Run: `pnpm test` → PASS; `pnpm check` → 0.

- [ ] **Step 5: CLAUDE.md**

After the M5 paragraph, add one paragraph in the same voice. It covers:
- card verbs on the desktop, and their routing;
- layer cards;
- `drift_diff`'s rule (rendered files only, placeholders kept, 256 KiB);
- held lines in `changeset_items.outcome`;
- Overwrite always held under Additive;
- `withdrawn_at` pruning;
- the Layers and Hosts views and Settings → Catalogs (grants via the hub CLI);
- the QuickSwitcher kinds;
- the `SyncPlanView` replacing the modal.

Also correct the M5 sentence about pruning, if Task 2 left it.

- [ ] **Step 6: Full verification**

Run (mercury), one filter or package at a time, never more than two at once:
- `cargo fmt --all --check`
- `cargo fleet-lint`
- `cargo test -p fleet-core` — expect only the known flakes; re-run each alone
- `cargo test -p fleet-hub`
- `cargo test -p claude-fleet --lib`
- the generators, each must report no change:
  - `cargo fleet-test -- reference_is_current`
  - `cargo fleet-test -- page_docs_are_current`
  - `cargo fleet-test -- settings_docs_are_current`
  - `cargo test -p claude-fleet --lib verdict_gen`

Locally: `pnpm test` and `pnpm check` (0 errors, 0 warnings).

- [ ] **Step 7: Commit**

```bash
git add -A src/lib crates CLAUDE.md
git commit -m "chore(assets): M6 carries — tokens in the remaining dialogs, wider colour guard, secrets keyed by catalog, scoped keys, narrow layout; CLAUDE.md"
```

---

## Self-review against the spec (M6)

| Spec requirement (M6) | Task |
|---|---|
| `ChangesetCard` (Bootstrap, New, Drift, Rollout) with apply / undo / dismiss / reject | 5, 8, 9 |
| Screen 1: groups, decider badges, needs-a-look chips, hidden internals, Adopt, commits + Undo, then the Rollout card | 9 (Dry run, Edit layers → R6) |
| Screen 2: drift diff with Take / Restore; new-asset suggestion (Adopt into, Ignore) | 4, 6, 10, 9 (`a`, `i`, primary) |
| Rollout = plan_sync (card hosts) + apply_sync; "Plan anyway" host-scoped; `SyncPlanDialog` becomes the Rollout card | 1, 11 |
| Screen 3: layers by catalog with footprint; host rows with org and role; accepted catalogs; "why is it on oci?" | 6, 12, 13 |
| Layers: create / rename / move produce cards | 3, 12 |
| Hosts: admission toggles | 6, 13 |
| Settings → Catalogs: add (name, path, remote, org), deploy-key hint, grants | 6, 7 |
| QuickSwitcher `asset` kind + Rescan, Sync fleet, Propose | 14 |
| Keyboard `i` ignore, `⌘↵` primary | 9, 15 |
| Hub client without a grant: same views without mutating controls | 9–13 (every verb hidden or disabled under `readOnly`) |
| Authorization: grant per catalog; per-host tokens never | 3 (MCP), 5–6 (routing to the existing gates) |
| State never colour alone; tokens | 9–13, 15 |
| Vitest: `ChangesetCard` apply/undo states; Rollout card host-scoped "Plan anyway" | 9, 11 |
