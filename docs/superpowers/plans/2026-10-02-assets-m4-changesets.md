# Assets M4: changesets, triage verdicts and per-catalog authoring — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the last scan into proposed changeset cards (Bootstrap, New on host, Drift, Rollout) built by rules after every scan-tick pass, and let the master — or a client granted every catalog a card touches — apply a card (one commit per catalog, nothing committed on failure), undo the latest applied card per catalog (`git revert` + the stored `host_layers` snapshot), dismiss it or reject items, with triage verdicts so a decided subject is not proposed again until its content changes, and `catalog.auto`'s additive sync on layers already rolled out once.

**Architecture:** Migration 094 adds the spec's `changesets`, `changeset_items` and `asset_triage_verdicts` (rows in `store/changesets.rs`). A new `service/catalog/changesets/` module holds the card model (`mod.rs`: kinds, actions, typed item params, views, the process-wide `APPLY_LOCK`), the rules (`rules.rs`, pure functions over identities, hosts, catalogs, drift and layer gaps), the reconcile pass (`reconcile.rs`: gathers store rows then the registry, asks the rules, inserts/refreshes/withdraws cards, writes automatic `ignored` verdicts; called from the scan tick after each pass and by `changesets { propose }`), the apply engine (`apply.rs`: catalog cards — per-catalog import, layer files, scope, `host_layers`, one commit per catalog, reset on failure; host cards — Rollout and Drift restore through `plan_sync` + `apply_sync` narrowed to additive ops; SB6's automatic additive sync) and undo (`undo.rs`: revert, snapshot restore, dismiss, reject_item). Authoring becomes per catalog through `CatalogTarget` (carry 1), a catalog's load problems hold their keys instead of planning removals (carry 2), `catalog_admin` resolves a named catalog once (M-d) and `fleet-hub catalog list` stops loading (M-e). One new MCP tool, `changesets { list | propose | apply | undo | dismiss | reject_item }`, checks a grant per touched catalog.

**Tech Stack:** Rust (fleet-core: rusqlite, rmcp, tokio; fleet-hub: clap), one settings page JSON, one TypeScript settings mirror. No Svelte (M5/M6).

**Spec:** `docs/superpowers/specs/2026-09-30-assets-s1b-s2-design.md` — milestone **M4** (Milestones row: "`changesets`, `changeset_items`, `asset_triage_verdicts`"), with *Decisions taken during design* (SB4, SB5, SB6), *Data model* (the three tables' DDL), *Changesets (the cards)* (triggers, Apply steps 1–5, Rollout apply, Undo, Automatic), *Hub CLI and MCP* (the `changesets` tool, grants per touched catalog) and *Testing* (changesets, store, authorization). Builds on M1 (#414), M2 (#416) and M3 (merged, migration 093). The UI (M5/M6), Jev and haiku (S5) are out of scope.

## Global Constraints

- Migration number **094** (`094_changesets.sql`). On 2026-10-02 `git ls-tree origin/main crates/fleet-core/migrations/` ends at `093_catalog_access.sql`. If `main` has gained a migration by the time this lands, renumber (file, `MIGRATIONS` entry, `schema_version` insert, test names) and re-check.
- The DDL is the spec's, verbatim (Rulings R1):
  `changesets (id INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT NOT NULL, summary TEXT NOT NULL, state TEXT NOT NULL, created_at INTEGER NOT NULL, applied_at INTEGER, commits TEXT, layers_snapshot TEXT, error TEXT)`;
  `changeset_items (changeset_id INTEGER NOT NULL REFERENCES changesets(id) ON DELETE CASCADE, position INTEGER NOT NULL, grp TEXT NOT NULL, catalog_id INTEGER REFERENCES catalogs(id), kind TEXT NOT NULL, name TEXT NOT NULL, action TEXT NOT NULL, params TEXT, decider TEXT NOT NULL, state TEXT NOT NULL, PRIMARY KEY (changeset_id, position))`;
  `asset_triage_verdicts (catalog_id INTEGER REFERENCES catalogs(id) ON DELETE CASCADE, kind TEXT NOT NULL, name TEXT NOT NULL, content_hash TEXT NOT NULL, verdict TEXT NOT NULL, decider TEXT NOT NULL, decided_at INTEGER NOT NULL, PRIMARY KEY (kind, name, content_hash))`.
- Vocabularies, from the DDL comments: card `kind` bootstrap | new | drift | rollout; card `state` proposed | applied | undone | dismissed | failed; item `action` import | assign_layer | set_scope | hide | take_host | restore | sync; `decider` rule | jev | haiku | person; item `state` pending | applied | skipped | rejected; `verdict` ignored | rejected | host_local.
- SB4: "Not automatic; the footer chip pushes. `catalog.auto_push` exists, off by default."
- SB5: "`git revert` of the card's commits plus the stored `host_layers` snapshot; only the latest applied card per catalog (a stack)."
- SB6: "Only additive ops (adopt, create, backed-up update) on layers that have been rolled out at least once. A layer's first rollout is always a card."
- Spec, Changesets: "A reconcile pass runs after each scan-tick pass and on demand (`changesets { action: propose }`) … Rules never re-propose a subject with a matching `asset_triage_verdicts` row until its content hash changes; an agent never overturns a person's verdict."
- Spec, Apply: "1. Snapshot `host_layers`. 2. For each catalog the card touches: import each item from the host holding the most common copy (S1a remote import, `only`), write `layers/*.yaml`, set `scope`. 3. Update `host_layers`. 4. One commit per touched catalog (`fleet: <card summary>`); store the SHAs. 5. Mark items `applied`; the card `applied`. A failure before step 4 leaves the working trees reset to their HEAD, the card `failed` with the error on the failing group, and nothing committed."
- Spec: "**Rollout apply** = `plan_sync` (hosts of the card) + `apply_sync`. `overwrite` and `remove` never go through a card." / "**Undo** … allowed only for the latest applied card in each catalog it touched. It never touches hosts; a follow-up Rollout card restores them." / "**Automatic (no card)** — `catalog.auto` (default on): hide internals, group, prepare cards; and additive sync ops on layers that have already been rolled out once (SB6)."
- Spec, MCP: "A new tool `changesets { list | propose | apply | undo | dismiss | reject_item }`. Every mutating action checks the caller's grant **for the catalogs it touches** (`may_admin_catalog(caller, catalog_id)`); per-host tokens never pass. `list_assets` and the inventory stay readable as today."
- Spec, Out of scope: "Automatic push by default; automatic overwrite / remove under any mode." Nothing in M4 removes an asset from a host.
- Jev and haiku are S5: M4 writes `decider` `rule` or `person` only, and keeps the column.
- Lock rule: **registry → store is allowed; store → registry never.** Read store rows under one guard, drop it, then take the registry. Never call `lock(store)` inside a `registry::with_*` closure.
- Never hold the `Store` guard across an `.await`. The new `APPLY_LOCK` is a `tokio::sync::Mutex<()>`, not the store; holding *it* across awaits is the point (R27).
- New serialized fields are `#[serde(default)]`; internal bookkeeping is `#[serde(skip)]`.
- A fleet with only `personal`, no org-bound hosts and no applied cards plans, scans, syncs and previews exactly as before, and `catalog_admin` answers the master and a granted client exactly as before. The tick adds rows (cards, `ignored` verdicts) and syncs nothing until a Rollout card has been applied (R16, R17).
- Shell-quoting only through `crate::shell::quote`; child processes only through `fleet_core::proc::command` / `std_command` (test `git` runs use `crate::proc::std_command`).
- `cargo test` takes ONE name filter per command. Known unrelated failures: `rewind::tests::the_removal_script_leaves_a_tree_a_live_pane_is_in` (socket path); `work::scale_tests::*` (perf under load); two `tests_upgrade` timing-budget tests under load; `service::add_project` tests on a busy box; the intermittent `trackers::sync` poison-view test. Re-run a suspected flake alone before touching anything.
- Tests that touch the catalog registry or `HOME` take `crate::service::catalog::lock_registry_for_test()`.
- Generators: `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current` after any `#[tool(...)]` description or params doc change; `REGEN_SETTINGS_DOCS=1 cargo test -p fleet-core settings_docs_are_current` and `REGEN_PAGE_DOCS=1 cargo test -p fleet-core page_docs_are_current` after the settings change; `REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen` (expected: no change, R25).
- The served-definition budget (`tools::tests::the_served_definition_budget_stays_bounded`, `BUDGET_BYTES`): any task that changes the MCP surface re-measures and sets the constant to the printed measurement + 100, naming what was measured in the commit message.
- Git: branch `feat/assets-m4-changesets` (checked out, tracking `origin/main`); commit per task; never rebase, never force-push; merge `origin/main` for conflicts.

## Pre-flight (verify before Task 1)

Every task below leans on these facts of the merged code. Check each; if one differs, stop and re-rule before coding.

| Fact | Where (2026-10-02) |
|---|---|
| Latest migration is 093; `MIGRATIONS` ends with `Migration::plain(93, …093_catalog_access.sql)` | `crates/fleet-core/src/store/schema.rs:1022` |
| `EXPECTED_TABLES` lacks `host_layers`, `host_catalogs`, `client_catalog_grants` | `store/schema.rs:1331` |
| `store_at_version(v)` test helper | `store/schema.rs:1696` |
| `Kind` derives `Ord` + `Hash`; `Kind::from_dir` exists (`#[allow(dead_code)]`) | `service/catalog/model.rs:32`, `:62` |
| `load_dir` records a parse failure at `<dir>/<stem>/asset.yaml` or `<dir>/<stem>.yaml`, an unreadable kind dir at `<dir>`; `load_one(root, kind, yaml_path, stem)` is private | `service/catalog/repo.rs:509`, `:483` |
| `repo::git(dir, args) -> Result<String, IpcError>` (private), `commit`, `stage_paths`, `has_staged`, `head`, `push`, `remove_asset`, `write_asset` | `service/catalog/repo.rs` |
| `EffectiveSet` is built in exactly one place; `KeepRules { … }` struct literals at `sync/mod.rs:402` and `sync/plan.rs:1859` | `effective.rs:559` |
| `held_noop(kind, name, catalog, reason)` is `pub(crate)` | `sync/plan.rs:506` |
| `registry::entry_for` is `pub(crate)`; `registry::snapshot()`; `registry::with_catalog_row` | `registry.rs:217`, `:176`, `:230` |
| `effective::effective_for_host_in(store, host, &snapshot)` | `effective.rs` |
| `sync::plan_sync(PlanArgs, store, ssh)`; `PlanArgs: Default`; `sync::apply_sync_with(ApplyArgs, store, ssh, CancellationToken)`; `plan::registry_put` (pub), `plan::registry_take_with_expiry` (`pub(crate)`) | `sync/mod.rs:189`, `:46`, `:634`; `sync/plan.rs:893`, `:952` |
| `HostSyncResult.status` is `applied` \| `partial` \| `skipped` \| `failed` | `sync/apply.rs:121` |
| `import_host(args, store, ssh, fleet_token)` resolves `require_config` → personal checkout; `import_claude_only` never overwrites and reports `created: Vec<(kind, name)>` | `service/catalog/mod.rs:473`, `import.rs:994` |
| `import::slugify` is `pub`; `only` keys are slugified by `normalize_only` | `import.rs:78`, `:168` |
| `author.rs` writes through `repo_root(store)` / `catalog_asset` / `lint_in_repo` / `commit_and_reload` (personal only) | `service/catalog/author.rs:612-700` |
| `admin::run(call, catalog: Option<&str>, …)`; `AdminCall::is_authoring`; the M4 refusal text in `touches` | `service/catalog/admin.rs:285-300`, `:390` |
| `catalog_admin` resolves the catalog name in `may_admin_catalog` and again in `run` (`catalogs::catalog_named`) — M-d | `mcp/tools/assets.rs:206-255`, `admin.rs:398` |
| `fleet-hub catalog list` calls `catalog::ensure_fresh` (may clone and restamp) — M-e | `crates/fleet-hub/src/catalog.rs:167-185` |
| The scan tick `continue`s while no personal catalog is loaded, then scans `due`, then sets `seen` | `service/catalog/scan_tick.rs:128-205` |
| `crate::rt::try_spawn` exists | `crates/fleet-core/src/rt.rs:46` |
| `tmux::fake_exec::{write_exec, PROBE_GUARD}`; `SshClient::with_ssh_binary` | `tmux.rs:1512`, `ssh.rs:259` |
| `confirm_gate("apply_sync", nonce, summary, caller)` is how `catalog_admin` gates its `apply_sync` | `mcp/tools/assets.rs:568`, `support.rs:1320` |
| Settings: `Spec::new(KEY, default, Kind::Bool, label, help)`; the Assets section of `pages/settings.automation.json`; `src/lib/fleet_settings.ts` `SETTING_KEYS` + `SETTING_DEFAULTS` (`every_spec_is_mirrored_in_fleet_settings_ts`) | `service/settings.rs:929-945`, `pages/settings.automation.json:97-108` |

## Rulings

Where the spec is silent or the carry list asked for a decision. Each ruling states its cost if it turns out wrong.

- **R1 — DDL verbatim, vocabularies in Rust.** The three tables exactly as the spec writes them, no extra column, index or CHECK. The vocabularies are Rust enums in the service (`CardKind`, `ItemAction`, `Decider`) and strings in the store. *Cost if wrong:* a bad string can only come from this crate's own code.
- **R2 — A card's subject is derived, not stored.** The DDL has no subject column, so `rules::subject_of(kind, items)` derives it: `bootstrap` (one at a time); `new:<kind>/<name>`; `drift:<catalog_id>:<kind>/<name>@<host>`; `rollout:<catalog_id>/<layer>,…`. The pass refreshes an open card with the same subject in place (same id, items replaced); it withdraws an open bootstrap/new/drift card whose subject it no longer produces (`dismissed`, error `withdrawn: no longer applies`, no verdict); it never withdraws a rollout card. *Cost if wrong:* a stored column later is additive.
- **R3 — Open = proposed | failed; a card applies once.** A failed card can be applied again and is refreshed by the pass. Any successful apply closes the card: the items it applied are `applied`, every other pending item `skipped`. *Cost if wrong:* applying part of a card twice means letting the pass propose the rest again.
- **R4 — Bootstrap or New.** Eligible = unmanaged identities on non-hidden hosts with no verdict on `(kind, name, identity hash)`. A Bootstrap card when a Bootstrap card is open, or ≥ 20 eligible `normal` identities exist (`BOOTSTRAP_MIN`, the spec's number), or nothing is bootstrapped yet and ≥ 1 `normal` exists — "a catalog is empty" read as *not bootstrapped*: no applied (not undone) Bootstrap card and an empty personal catalog. Otherwise one New card per identity. *Cost if wrong:* an empty org catalog next to a non-empty personal gets New cards, not a Bootstrap.
- **R5 — Destination and source.** Destination = org X's catalog when every host holding the identity is bound to org X and X has a catalog (one that failed to load → "needs a look"), else `personal`. Source = among the identity's Claude copies, the content hash most hosts hold (tie: the smaller hash), and among its holders `local` first, then alphabetical. An identity with no Claude copy needs a look (the S1a import reads Claude config only). *Cost if wrong:* a Codex-only asset needs a person.
- **R6 — Layer names (SB3).** Per destination: one group per host-set signature; members of a signature whose slug shares a prefix (up to the first `-`) with ≥ 3 members form their own layer named by the prefix (`PREFIX_FAMILY_MIN`). The rest, largest first: `everywhere` when the signature is every host that accepts the catalog (all non-hidden hosts for personal, the org's hosts for an org catalog), `<host>-only` for one host, `core` for the first remaining group, else the sorted aliases joined by `-`. A name the catalog or the card already uses gets `-2`, `-3`, …. Every proposed layer is a **context** layer. *Cost if wrong:* names are starting points; M6 renames.
- **R7 — Assign only where it cannot shrink a host.** A Bootstrap assigns a layer to a host only when the destination catalog has no assets yet or the host already has a layer in it. Layering a host that today takes the whole non-empty catalog would drop the rest of that catalog from its effective set, and a later manual sync would plan removals. *Cost if wrong:* such a host stays unlayered — it still takes the new imports, since unlayered means everything.
- **R8 — Items and params.** One typed `ItemParams` (all fields optional, serialized as the `params` JSON): `from_host`, `layer`, `member` (`<kind>/<slug>`), `host`, `axis`, `scope`, `hash`, `reason`, `assets`. Group names: the layer name, `needs a look`, `hidden`, `drift`, `update` (a take_host follow-up). A `needs a look` item is an import that a default apply skips; named in `positions` it imports into the catalog with no layer. *Cost if wrong:* none — params are additive JSON.
- **R9 — Verdict subjects and hashes.** An identity's hash is its one known host hash, else `sha256` of its sorted distinct hashes joined by `\n`, else `-`; a drift subject is `(kind, name, the host copy's hash)`; a rollout subject is `("layer", "<catalog_id>/<layer>", sha256 of its sorted host:asset gaps)`. `reject_item` writes `rejected` for import, hide and sync items; for a drift card only once both of its items are rejected; set_scope and assign_layer carry no subject. `dismiss` = reject every pending item. `ignored` is written by the pass (auto on) or by applying a hide item. `host_local` stays reserved for M6's "keep on that host". *Cost if wrong:* dismissing a Bootstrap silences its identities until they change; M6 can add "reopen".
- **R10 — Who decides.** Every explicit `changesets` call records `person`; the pass and applied hide items record `rule`. The store never replaces a `person` verdict with another decider's ("an agent never overturns a person's verdict"). *Cost if wrong:* an AI driving the master token is recorded as `person` — the wire carries no agent identity until S5.
- **R11 — Apply preconditions.** The card is open; every catalog it touches is loaded (not a problem entry) and its checkout clean (`git status --porcelain` empty), or `E_INVALID_STATE` before anything is written — not a card failure, the state stays. The snapshot is the touched catalogs' `host_layers` rows only; a whole-table snapshot would let an undo clobber other catalogs. *Cost if wrong:* a person with uncommitted authoring edits commits them first (`catalog_admin commit_pending`).
- **R12 — Failure discipline.** Any error in steps 2–4 (imports, files, `host_layers`, commits) resets every touched checkout to its pre-apply HEAD (`git reset --hard` + `git clean -fd`, safe because the tree was clean), restores the snapshot and marks the card `failed` with `<group>: <error>`. A failed second commit therefore also resets the first catalog's commit: "nothing committed" holds literally. Reload and push failures after all commits are warnings in the card's `error`; the card stays applied. *Cost if wrong:* none — the reset only ever drops this apply's own unpushed commit.
- **R13 — Imports and take_host.** Imports are grouped per (catalog, source host): one remote read per pair, with `only`. Each item must come back in `created` as `(kind, slugify(name))` or its group fails. `take_host` removes the catalog copy, imports the host's copy and keeps the old `scope`. *Cost if wrong:* other header fields (tags, description) come from the host copy.
- **R14 — Follow-up Rollout.** An applied card that added layer members proposes one Rollout card with an item per (host assigned the layer, layer) carrying the members it added; a take_host proposes a Rollout of that asset to every other host that manages it (group `update`). Undo proposes a Rollout only for undone take_host items. *Cost if wrong:* undoing a Bootstrap leaves its adopted copies on hosts as orphans — undo never removes them, but their manifest entries stay, so the next ordinary sync would remove them with a backup (final review I4: the undo answer warns, naming the assets and hosts; a refusal or a "release" of adopted entries is M5 work).
- **R15 — What a card may write on a host.** Rollout and SB6: only `create`, `adopt` and `update` (the applier backs up every file it replaces), only for the card's assets on the card's hosts and from the card's catalogs; `overwrite`, `remove` and plugin ops are dropped from the plan before apply. Drift `restore` (one host, one asset, a person picked it): `update` or `overwrite`, with backup — the spec's Drift row ("restore (sync the catalog copy, backup)") read as the one explicit exception to "overwrite never through a card", which governs Rollout; `remove` never. A host card that partly applies answers its view with state `failed` and the failing hosts; its successful hosts' items stay `applied`. *Cost if wrong:* restore of a host-edited copy refuses — one line in `op_allowed`.
- **R16 — "A new layer exists" (Rollout trigger).** A layer assigned to a host, never rolled out, whose members (by the host's provenance) are `missing` there gets one Rollout card, unless an open rollout card names it or a verdict holds its gap hash. "Rolled out" = an applied `sync` item naming the layer (on an applied or a failed card). *Cost if wrong:* a pre-M4 layer counts as never rolled out until one Rollout card is applied — so SB6 never syncs a pre-M4 fleet on its own.
- **R17 — SB6.** With `catalog.auto` on, after each pass a detached task (try-lock; skipped while any apply runs) plans the hosts whose rolled-out layers have a member `missing`, `drifted` or present-but-unmanaged, and applies only `create`/`adopt`/`update` for assets those layers introduced. Plugin ops are excluded. *Cost if wrong:* one extra scan of such a host per pass until it converges.
- **R18 — The settings.** `catalog.auto` (Bool, default `true`) and `catalog.auto_push` (Bool, default `false`), on Settings → Automation → Assets. Auto off: the tick runs neither the reconcile pass nor SB6; `changesets { propose }` still builds cards, with `hide` items instead of automatic verdicts. Auto-push on: push each catalog a card committed to (or an undo reverted in); a push failure is a warning on the card. *Cost if wrong:* none.
- **R19 — Scan-tick integration (carry 4).** `reconcile::after_scan_pass(&store, &ssh)` runs after the pass's scan loop with the tick's own `Arc`s, only on passes that got past the tick's "personal loaded" check, takes `APPLY_LOCK` with `try_lock` (never waits), never reads or writes `owed`/`seen`, and only logs its errors. The reconcile pass is synchronous (store + registry, no SSH); SB6 is spawned detached (`rt::try_spawn`). *Cost if wrong:* a pass's reconcile is skipped while an apply runs.
- **R20 — Undo.** Only an applied bootstrap, new, or drift-take_host card. Refused (`E_INVALID_STATE`, "undo #N first") while a later applied catalog-changing card touches any of its catalogs; the touched checkouts must be clean; a revert conflict aborts (`git revert --abort`), resets any catalog already reverted in this undo and changes nothing. Then the snapshot rows of the touched catalogs are restored (hosts deleted since are skipped), the card is `undone`, the catalogs reload. *Cost if wrong:* a manual layer change made after the card, in that catalog, is reverted too.
- **R21 — Authoring per catalog (carry 1).** Every authoring action but `configure` honours `catalog`: `get_asset`, `template`, `create_asset`, `update_asset`, `delete_asset`, `add_resource_bytes`, `remove_resource`, `lint_asset`, `lint_all`, `commit_pending`, `push`, `repo_status`, `layer_template`, `write_layer`, `delete_layer`, `import_host`. They take a `CatalogTarget::{Personal, Row}`; `Personal` keeps each pre-M4 path exactly (`require_config`, `registry::with_personal`, `load`), so the desktop's commands are unchanged. `configure` stays personal (M3 M-g). The standalone `import_assets` tool stays personal; `catalog_admin { action: import_host, catalog }` is per catalog. *Cost if wrong:* none for personal.
- **R22 — M-d.** `catalog_admin` reads a named catalog's row once; the gate (`may_admin_catalog_row`) and `run(call, Option<&CatalogRow>, …)` use that row. An unknown name is `E_NOTFOUND` for the master — never a fall-through to personal — and `E_FORBIDDEN` for a client, as before.
- **R23 — M-e.** `fleet-hub catalog list` reads only: each checkout is probed with `load_dir` (never `ensure_repo`, so never a clone), nothing is recorded in the store, the registry is untouched. `reload` loads. MCP `list_catalogs` keeps its best-effort `ensure_fresh`: the hub process's registry is what it reports. *Cost if wrong:* a never-loaded catalog shows `not_loaded` until a `reload`.
- **R24 — Problem holds (carry 2).** A key whose own file is a load problem (`<dir>/<name>/asset.yaml`, `<dir>/<name>.yaml`), or whose whole kind directory is one (`<dir>`), is held in that catalog: its manifest entries get a `Noop` "catalog X could not read it (…); its copy is kept, not removed", never a `Remove`. Layer and catalog-file problems hold nothing. The inventory keeps calling such entries `orphan` (M3 R8). *Cost if wrong:* an asset really deleted while a broken leftover file remains is kept until the file is fixed or removed.
- **R25 — The tool's gate and the desktop.** `changesets` is `Access::Client` and not in `NOT_FOR_HOST_TOKENS`: `list` is open to every caller that reaches it, per-host tokens included (spec, Testing: "… but can list", the same as `list_assets`); readonly clients are refused by the mode gate (the tool mutates). `propose` needs the personal grant (fleet-wide, like `plan_sync`). `apply`, `undo`, `dismiss`, `reject_item` need a grant on every catalog the card's items name (none named: personal); applying a rollout or a restore also needs personal and passes the `apply_sync` confirm gate. No Tauri command, no verdict row and no `CONTRACT_REVISION` bump in M4 — M3 R14's precedent: `changesets` is a new tool, not an `AdminCall` action, so the AdminCall verdict table does not change; M6 adds the commands, their rows and the bump. *Cost if wrong:* M6 adds them.
- **R26 — Removing a catalog that has cards.** `changeset_items.catalog_id` has no `ON DELETE` in the spec's DDL, so `Store::remove_catalog` first withdraws the open cards that name the catalog and sets those items' `catalog_id` to NULL; applied cards keep their history but can no longer be undone. `CatalogRemoval` gains `cards` (`#[serde(default)]`). *Cost if wrong:* none.
- **R27 — One apply at a time.** A process-wide `tokio::sync::Mutex<()>` (`APPLY_LOCK`) serialises apply, undo, dismiss, reject_item, on-demand propose, the tick's reconcile and SB6. Two processes do not share it, and the `fleet-hub` CLI has no changeset verbs (the spec gives none). A card's host sync runs under its own `CancellationToken`, so `cancel_task` cannot stop it. *Cost if wrong:* a cancel handle later.
- **R28 — No retention and no row events in M4.** Cards accumulate one per new subject; M6's UI adds events and pruning. *Cost if wrong:* a few rows a week.
- **R29 — Carry 3d ("Task 3 M5/M6 doc sentences").** Read as the forward references M3 Task 3 left: R6 "until a person removes them (M4/M6 cards)" and R8 "M5 can add a distinct state". M4 settles them in the docs: cards never propose a removal, so a kept entry stays until a person syncs a plan that removes it; the distinct inventory state stays with M5's badge work. *Cost if the parked note meant something else:* one doc edit.
- **R30 — Carry 3c (M3 Task 1 coverage).** A test of 093's backfill over ineligible holders (revoked, readonly, org-bound: rows exist, grant nothing, never listed) and `EXPECTED_TABLES` gaining `host_layers`, `host_catalogs`, `client_catalog_grants` and the three M4 tables.

## Carry list → where it lands

| # | Carry | Lands in |
|---|---|---|
| 1 | R10 from M3: authoring actions personal-only until M4 | Task 3 (R21) |
| 2 | A catalog with parse/read Problems still speaks → its keys plan `Remove` | Task 2 (R24) |
| 3a | M-d: catalog name resolved twice (gate vs run) | Task 3 (R22) |
| 3b | M-e: `fleet-hub catalog list` side effects | Task 10 (R23) |
| 3c | Task 1 coverage: 092→093 backfill over ineligible holders; `EXPECTED_TABLES` | Task 1 (R30) |
| 3d | Task 3 M5/M6 doc sentences | Task 10 (R29) |
| 4 | Reconcile after each scan-tick pass; owed-rescan untouched; never block the tick | Task 5 (R19), Task 7 (R17) |

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/fleet-core/migrations/094_changesets.sql` (new) | the three tables |
| `crates/fleet-core/src/store/changesets.rs` (new) | card, item and verdict rows; `rolled_out_layers` |
| `crates/fleet-core/src/store/{mod,schema,layers,catalog,rows}.rs` | registration and tests; `restore_host_layers`; `remove_catalog` and cards; `CatalogRemoval.cards` |
| `crates/fleet-core/src/service/catalog/repo.rs` | `ProblemHolds`; `read_asset`, `is_clean`, `reset_hard`, `revert` |
| `crates/fleet-core/src/service/catalog/effective.rs` | `EffectiveSet.problem_held` |
| `crates/fleet-core/src/service/catalog/sync/{plan,mod}.rs` | `KeepRules.problem_held`, the held `Noop` |
| `crates/fleet-core/src/service/catalog/mod.rs` | `CatalogTarget`, `get_asset_in`, `import_host_into`; `pub mod changesets` |
| `crates/fleet-core/src/service/catalog/author.rs` | `*_in(target, …)` + personal wrappers |
| `crates/fleet-core/src/service/catalog/admin.rs` | authoring per catalog; `run(call, Option<&CatalogRow>, …)` |
| `crates/fleet-core/src/service/catalog/catalogs.rs` | `probe_catalogs` (read-only listing) |
| `crates/fleet-core/src/service/catalog/changesets/mod.rs` (new) | model, views, `APPLY_LOCK`, `list`/`get`/`card`/`propose`, undoability |
| `crates/fleet-core/src/service/catalog/changesets/rules.rs` (new) | pure rules: subjects, hashes, Bootstrap/New/Drift/Rollout |
| `crates/fleet-core/src/service/catalog/changesets/reconcile.rs` (new) | the pass; `after_scan_pass` |
| `crates/fleet-core/src/service/catalog/changesets/apply.rs` (new) | catalog apply, host apply, SB6 |
| `crates/fleet-core/src/service/catalog/changesets/undo.rs` (new) | undo, dismiss, reject_item |
| `crates/fleet-core/src/service/catalog/changesets/testkit.rs` (new, test-only) | git checkouts, a fleet store, a fake `ssh` with its own `HOME` |
| `crates/fleet-core/src/service/catalog/scan_tick.rs` | the hook after each pass |
| `crates/fleet-core/src/service/settings.rs`, `crates/fleet-core/pages/settings.automation.json`, `src/lib/fleet_settings.ts` | `catalog.auto`, `catalog.auto_push` |
| `crates/fleet-core/src/mcp/tools/{assets,params,mod}.rs`, `mcp/tools/tests_changesets.rs` (new), `mcp/tools/tests_catalog_admin.rs`, `mcp/guard.rs`, `mcp/tools/tests.rs` | the `changesets` tool; M-d; the policy row; the budget |
| `crates/fleet-hub/src/catalog.rs` | read-only `catalog list` |
| `CLAUDE.md`, `docs/hub.md`, `docs/control-api.md`, generated docs | docs |

---

### Task 1: Changesets and verdicts in the store (migration 094)

**Files:**
- Create: `crates/fleet-core/migrations/094_changesets.sql`
- Create: `crates/fleet-core/src/store/changesets.rs`
- Modify: `crates/fleet-core/src/store/mod.rs` (module + re-exports)
- Modify: `crates/fleet-core/src/store/schema.rs` (`MIGRATIONS` entry; `EXPECTED_TABLES`; tests)
- Modify: `crates/fleet-core/src/store/layers.rs` (`restore_host_layers` + test)
- Modify: `crates/fleet-core/src/store/catalog.rs` (`remove_catalog` and cards)
- Modify: `crates/fleet-core/src/store/rows.rs` (`CatalogRemoval.cards`)

**Interfaces:**
- Consumes: `Store::personal_catalog`, `Store::upsert_catalog`, `Store::add_org`, `Store::remove_catalog`, `now_unix`.
- Produces:

```rust
// store/changesets.rs (re-exported from store)
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChangesetRow { pub id: i64, pub kind: String, pub summary: String, pub state: String,
    pub created_at: i64, pub applied_at: Option<i64>, pub commits: Option<String>,
    pub layers_snapshot: Option<String>, pub error: Option<String> }
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChangesetItemRow { pub changeset_id: i64, pub position: i64, pub grp: String,
    pub catalog_id: Option<i64>, pub kind: String, pub name: String, pub action: String,
    pub params: Option<String>, pub decider: String, pub state: String }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewChangesetItem { pub grp: String, pub catalog_id: Option<i64>, pub kind: String,
    pub name: String, pub action: String, pub params: Option<String>, pub decider: String }
impl From<&ChangesetItemRow> for NewChangesetItem;
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TriageVerdictRow { pub catalog_id: Option<i64>, pub kind: String, pub name: String,
    pub content_hash: String, pub verdict: String, pub decider: String, pub decided_at: i64 }

impl Store {
    pub fn insert_changeset(&self, kind: &str, summary: &str, items: &[NewChangesetItem]) -> rusqlite::Result<ChangesetRow>;
    pub fn replace_changeset_items(&self, id: i64, summary: &str, items: &[NewChangesetItem]) -> rusqlite::Result<bool>; // false: not open
    pub fn get_changeset(&self, id: i64) -> rusqlite::Result<Option<ChangesetRow>>;
    pub fn list_changesets(&self) -> rusqlite::Result<Vec<ChangesetRow>>;          // newest first
    pub fn changeset_items(&self, id: i64) -> rusqlite::Result<Vec<ChangesetItemRow>>; // by position
    pub fn set_changeset_state(&self, id: i64, state: &str, error: Option<&str>) -> rusqlite::Result<()>;
    pub fn mark_changeset_applied(&self, id: i64, applied_at: i64, commits: &str, layers_snapshot: &str, error: Option<&str>) -> rusqlite::Result<()>;
    pub fn set_changeset_item_states(&self, id: i64, positions: &[i64], state: &str) -> rusqlite::Result<()>;
    pub fn upsert_triage_verdict(&self, v: &TriageVerdictRow) -> rusqlite::Result<()>;
    pub fn triage_verdicts(&self) -> rusqlite::Result<Vec<TriageVerdictRow>>;
    pub fn rolled_out_layers(&self) -> rusqlite::Result<std::collections::BTreeSet<(i64, String)>>;
}
// store/layers.rs
impl Store { pub fn restore_host_layers(&self, catalog_id: i64, rows: &[HostLayerRow]) -> rusqlite::Result<usize>; }
// store/rows.rs — CatalogRemoval gains:
#[serde(default)] pub cards: usize,
```

- [ ] **Step 1: Write the failing tests**

`store/schema.rs` tests module — add `"host_layers", "host_catalogs", "client_catalog_grants", "changesets", "changeset_items", "asset_triage_verdicts"` to `EXPECTED_TABLES`, and add next to the 093 tests:

```rust
    /// 094 on a database stopped at 093 creates the spec's three tables,
    /// column for column; re-running it is a no-op.
    #[test]
    fn migration_94_creates_changesets_items_and_verdicts() {
        let old = store_at_version(93);
        old.migrate().expect("094");
        let cols = |t: &str| -> Vec<String> {
            old.conn
                .prepare(&format!("SELECT name FROM pragma_table_info('{t}') ORDER BY cid"))
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert_eq!(
            cols("changesets"),
            ["id", "kind", "summary", "state", "created_at", "applied_at", "commits", "layers_snapshot", "error"]
        );
        assert_eq!(
            cols("changeset_items"),
            ["changeset_id", "position", "grp", "catalog_id", "kind", "name", "action", "params", "decider", "state"]
        );
        assert_eq!(
            cols("asset_triage_verdicts"),
            ["catalog_id", "kind", "name", "content_hash", "verdict", "decider", "decided_at"]
        );
        old.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 94;")
            .unwrap();
        old.migrate().expect("re-running 094 is safe");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    }

    /// Carry 3c (Rulings R30): 093 backfills a grant for every
    /// `assets_admin_at` holder, eligible or not — revoked, readonly and
    /// org-bound ones too — and those rows grant nothing: the live predicate
    /// refuses them and `catalog_grantees` never lists them.
    #[test]
    fn migration_93_backfills_ineligible_holders_but_they_grant_nothing() {
        let old = store_at_version(92);
        old.conn
            .execute_batch(
                "INSERT INTO catalogs (id, name, repo_path, org_id, created_at) VALUES (1, 'personal', '/p', NULL, 0);\
                 INSERT INTO orgs (id, name, created_at) VALUES (10, 'acme', 0);\
                 INSERT INTO client_tokens (name, token_sha256, mode, created_at, assets_admin_at, revoked_at) \
                   VALUES ('gone', 'h1', 'full', 1, 5, 9);\
                 INSERT INTO client_tokens (name, token_sha256, mode, created_at, assets_admin_at) \
                   VALUES ('kiosk', 'h2', 'readonly', 1, 5);\
                 INSERT INTO client_tokens (name, token_sha256, mode, created_at, assets_admin_at, org_id) \
                   VALUES ('contractor', 'h3', 'full', 1, 5, 10);\
                 INSERT INTO client_tokens (name, token_sha256, mode, created_at, assets_admin_at) \
                   VALUES ('desk', 'h4', 'full', 1, 5);",
            )
            .unwrap();
        old.migrate().expect("093 and 094");
        let n: i64 = old
            .conn
            .query_row("SELECT COUNT(*) FROM client_catalog_grants", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 4, "the backfill copies every holder");
        let id = |name: &str| -> i64 {
            old.conn
                .query_row("SELECT id FROM client_tokens WHERE name = ?1", [name], |r| r.get(0))
                .unwrap()
        };
        for name in ["gone", "kiosk", "contractor"] {
            assert!(!old.client_may_admin_catalog(id(name), 1).unwrap(), "{name} is not eligible");
        }
        assert!(old.client_may_admin_catalog(id("desk"), 1).unwrap());
        assert_eq!(old.catalog_grantees(1).unwrap(), vec!["desk".to_string()]);
    }
```

`store/changesets.rs` (new file — the tests module now, the implementation in Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use std::collections::BTreeSet;

    fn item(grp: &str, action: &str, catalog_id: Option<i64>, params: Option<&str>) -> NewChangesetItem {
        NewChangesetItem {
            grp: grp.into(),
            catalog_id,
            kind: "skill".into(),
            name: format!("{grp}-{action}"),
            action: action.into(),
            params: params.map(String::from),
            decider: "rule".into(),
        }
    }

    fn store() -> (Store, i64) {
        let s = Store::open_in_memory().unwrap();
        s.set_catalog_config("/p", None).unwrap();
        let personal = s.personal_catalog().unwrap().unwrap().id;
        (s, personal)
    }

    #[test]
    fn a_card_round_trips_and_only_an_open_card_is_refreshed() {
        let (s, p) = store();
        let card = s
            .insert_changeset(
                "bootstrap",
                "Adopt 2 as 1 layers",
                &[
                    item("core", "import", Some(p), Some(r#"{"from_host":"oci"}"#)),
                    item("core", "assign_layer", Some(p), None),
                ],
            )
            .unwrap();
        assert_eq!(
            (card.kind.as_str(), card.state.as_str(), card.applied_at),
            ("bootstrap", "proposed", None)
        );
        let items = s.changeset_items(card.id).unwrap();
        assert_eq!(items.iter().map(|i| i.position).collect::<Vec<_>>(), [0, 1]);
        assert!(items.iter().all(|i| i.state == "pending"));
        assert_eq!(items[0].params.as_deref(), Some(r#"{"from_host":"oci"}"#));
        assert_eq!(
            NewChangesetItem::from(&items[1]),
            item("core", "assign_layer", Some(p), None)
        );

        assert!(s
            .replace_changeset_items(card.id, "Adopt 1 as 1 layers", &[item("core", "import", Some(p), None)])
            .unwrap());
        assert_eq!(s.changeset_items(card.id).unwrap().len(), 1);
        assert_eq!(s.get_changeset(card.id).unwrap().unwrap().summary, "Adopt 1 as 1 layers");

        s.set_changeset_item_states(card.id, &[0], "applied").unwrap();
        s.mark_changeset_applied(card.id, 50, r#"{"1":"abc"}"#, "[]", None)
            .unwrap();
        let applied = s.get_changeset(card.id).unwrap().unwrap();
        assert_eq!(
            (applied.state.as_str(), applied.applied_at, applied.commits.as_deref()),
            ("applied", Some(50), Some(r#"{"1":"abc"}"#))
        );
        assert!(
            !s.replace_changeset_items(card.id, "x", &[]).unwrap(),
            "an applied card is history"
        );
        assert_eq!(s.changeset_items(card.id).unwrap()[0].state, "applied");
        let newer = s.insert_changeset("new", "New", &[]).unwrap();
        assert_eq!(
            s.list_changesets().unwrap().iter().map(|c| c.id).collect::<Vec<_>>(),
            [newer.id, card.id],
            "newest first"
        );
        s.set_changeset_state(card.id, "undone", None).unwrap();
        assert_eq!(s.get_changeset(card.id).unwrap().unwrap().state, "undone");
    }

    /// Spec, Testing (store): a verdict holds by content hash; a person's
    /// verdict is never replaced by another decider (Rulings R10).
    #[test]
    fn verdicts_hold_by_content_hash_and_a_persons_verdict_stays() {
        let (s, _) = store();
        let v = |hash: &str, verdict: &str, decider: &str| TriageVerdictRow {
            catalog_id: None,
            kind: "skill".into(),
            name: "w".into(),
            content_hash: hash.into(),
            verdict: verdict.into(),
            decider: decider.into(),
            decided_at: 1,
        };
        s.upsert_triage_verdict(&v("h1", "rejected", "person")).unwrap();
        s.upsert_triage_verdict(&v("h1", "ignored", "rule")).unwrap();
        s.upsert_triage_verdict(&v("h2", "ignored", "rule")).unwrap();
        let all = s.triage_verdicts().unwrap();
        assert_eq!(all.len(), 2, "one row per (kind, name, content_hash)");
        let h1 = all.iter().find(|r| r.content_hash == "h1").unwrap();
        assert_eq!(
            (h1.verdict.as_str(), h1.decider.as_str()),
            ("rejected", "person"),
            "a rule never overturns a person"
        );
        s.upsert_triage_verdict(&v("h2", "rejected", "person")).unwrap();
        let h2 = s
            .triage_verdicts()
            .unwrap()
            .into_iter()
            .find(|r| r.content_hash == "h2")
            .unwrap();
        assert_eq!(h2.decider, "person", "a person may overturn a rule");
    }

    /// Rulings R16: a layer is rolled out once an applied `sync` item names
    /// it — on a failed card too (its other hosts failed, not this one).
    #[test]
    fn a_layer_is_rolled_out_once_an_applied_sync_item_names_it() {
        let (s, p) = store();
        let sync = |layer: &str| NewChangesetItem {
            grp: layer.into(),
            catalog_id: Some(p),
            kind: "host".into(),
            name: "oci".into(),
            action: "sync".into(),
            params: Some(format!(r#"{{"layer":"{layer}","assets":["skill/w"]}}"#)),
            decider: "rule".into(),
        };
        let a = s
            .insert_changeset("rollout", "Roll out core to oci", &[sync("core"), sync("extra")])
            .unwrap();
        assert!(s.rolled_out_layers().unwrap().is_empty(), "proposed is not rolled out");
        s.set_changeset_item_states(a.id, &[0], "applied").unwrap();
        s.set_changeset_state(a.id, "failed", Some("trn: unreachable")).unwrap();
        assert_eq!(
            s.rolled_out_layers().unwrap(),
            BTreeSet::from([(p, "core".to_string())])
        );
    }

    /// Rulings R26: removing a catalog withdraws the open cards that name it
    /// and clears its items' catalog; applied history stays.
    #[test]
    fn removing_a_catalog_clears_its_items_and_withdraws_open_cards() {
        let (s, p) = store();
        let org = s.add_org("acme", None, false).unwrap();
        let acme = s.upsert_catalog("acme", "/a", None, Some(org.id)).unwrap().id;
        let open = s
            .insert_changeset("new", "New", &[item("core", "import", Some(acme), None)])
            .unwrap();
        let done = s
            .insert_changeset(
                "new",
                "Done",
                &[item("core", "import", Some(acme), None), item("core", "import", Some(p), None)],
            )
            .unwrap();
        s.mark_changeset_applied(done.id, 5, "{}", "[]", None).unwrap();

        let gone = s.remove_catalog("acme").unwrap();
        assert_eq!(gone.cards, 1, "one open card withdrawn");
        let open = s.get_changeset(open.id).unwrap().unwrap();
        assert_eq!(open.state, "dismissed");
        assert!(open.error.as_deref().unwrap().contains("acme was removed"));
        assert_eq!(s.get_changeset(done.id).unwrap().unwrap().state, "applied", "history stays");
        let items = s.changeset_items(done.id).unwrap();
        assert_eq!((items[0].catalog_id, items[1].catalog_id), (None, Some(p)));
    }
}
```

`store/layers.rs` tests module:

```rust
    /// Assets M4 (undo, failed apply): one catalog's rows go back exactly as
    /// they were; a row whose host was deleted since is skipped.
    #[test]
    fn restore_host_layers_replaces_one_catalog_and_skips_hosts_gone_since() {
        let s = store_with_local();
        s.upsert_host("oci").unwrap();
        let p = s.personal_catalog().unwrap().unwrap().id;
        s.set_host_layers("local", Some("core"), &["a"]).unwrap();
        s.set_host_layers("oci", None, &["b"]).unwrap();
        let before = s.list_all_host_layers().unwrap();
        s.set_host_layers("local", None, &["changed"]).unwrap();
        s.set_host_layers("oci", None, &[]).unwrap();
        s.delete_host("oci").unwrap();
        assert_eq!(s.restore_host_layers(p, &before).unwrap(), 2, "oci's row is skipped");
        let local: Vec<_> = before.into_iter().filter(|r| r.host_alias == "local").collect();
        assert_eq!(s.list_all_host_layers().unwrap(), local);
    }
```

`store/mod.rs`: add `mod changesets;` (alphabetical, after `mod catalog;`) and `pub use changesets::{ChangesetItemRow, ChangesetRow, NewChangesetItem, TriageVerdictRow};` next to the other `pub use` lines, so the tests compile against the names.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p fleet-core migration_94` — Expected: FAIL (no migration 94; `changesets` missing).
Run: `cargo test -p fleet-core store::changesets` — Expected: compile error: `insert_changeset` not found.

- [ ] **Step 3: Write the implementation**

`crates/fleet-core/migrations/094_changesets.sql`:

```sql
-- Assets S1b+S2 (M4): changeset cards, their items, and triage verdicts.
-- The DDL is the spec's verbatim (docs/superpowers/specs/
-- 2026-09-30-assets-s1b-s2-design.md, Data model). Vocabularies live in
-- `service::catalog::changesets`. CREATE IF NOT EXISTS: safe to re-run.
CREATE TABLE IF NOT EXISTS changesets (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  kind        TEXT NOT NULL,        -- bootstrap | new | drift | rollout
  summary     TEXT NOT NULL,        -- the card's sentence
  state       TEXT NOT NULL,        -- proposed | applied | undone | dismissed | failed
  created_at  INTEGER NOT NULL,
  applied_at  INTEGER,
  commits     TEXT,                 -- JSON {catalog_id: sha}
  layers_snapshot TEXT,             -- JSON host_layers rows before apply
  error       TEXT
);

CREATE TABLE IF NOT EXISTS changeset_items (
  changeset_id INTEGER NOT NULL REFERENCES changesets(id) ON DELETE CASCADE,
  position     INTEGER NOT NULL,
  grp          TEXT    NOT NULL,    -- the card group (a layer name, "needs a look", ...)
  catalog_id   INTEGER REFERENCES catalogs(id),
  kind         TEXT NOT NULL,
  name         TEXT NOT NULL,
  action       TEXT NOT NULL,       -- import | assign_layer | set_scope | hide | take_host | restore | sync
  params       TEXT,                -- JSON
  decider      TEXT NOT NULL,       -- rule | jev | haiku | person
  state        TEXT NOT NULL,       -- pending | applied | skipped | rejected
  PRIMARY KEY (changeset_id, position)
);

CREATE TABLE IF NOT EXISTS asset_triage_verdicts (
  catalog_id   INTEGER REFERENCES catalogs(id) ON DELETE CASCADE,
  kind         TEXT NOT NULL,
  name         TEXT NOT NULL,
  content_hash TEXT NOT NULL,
  verdict      TEXT NOT NULL,       -- ignored | rejected | host_local
  decider      TEXT NOT NULL,
  decided_at   INTEGER NOT NULL,
  PRIMARY KEY (kind, name, content_hash)
);

INSERT OR IGNORE INTO schema_version (version) VALUES (94);
```

`store/schema.rs` — after the 93 entry in `MIGRATIONS`:

```rust
    // Assets S1b+S2 M4: changeset cards, their items, triage verdicts (the
    // spec's DDL verbatim). CREATE IF NOT EXISTS: safe to re-run.
    Migration::plain(94, include_str!("../../migrations/094_changesets.sql")),
```

`store/changesets.rs` (above the tests module):

```rust
//! Changeset cards, their items and triage verdicts (Assets M4, migration
//! 094). The rules — which cards exist, what applying one does — live in
//! `service::catalog::changesets`; this is the rows.

use super::{now_unix, Store};
use rusqlite::{OptionalExtension, Result};
use std::collections::BTreeSet;

/// One card (`changesets` row).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChangesetRow {
    pub id: i64,
    /// bootstrap | new | drift | rollout
    pub kind: String,
    pub summary: String,
    /// proposed | applied | undone | dismissed | failed
    pub state: String,
    pub created_at: i64,
    pub applied_at: Option<i64>,
    /// JSON `{catalog_id: sha}`: the commits an apply made.
    pub commits: Option<String>,
    /// JSON `[HostLayerRow]`: the touched catalogs' `host_layers` before apply.
    pub layers_snapshot: Option<String>,
    pub error: Option<String>,
}

/// One item of a card (`changeset_items` row).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChangesetItemRow {
    pub changeset_id: i64,
    pub position: i64,
    pub grp: String,
    pub catalog_id: Option<i64>,
    pub kind: String,
    pub name: String,
    /// import | assign_layer | set_scope | hide | take_host | restore | sync
    pub action: String,
    pub params: Option<String>,
    /// rule | jev | haiku | person
    pub decider: String,
    /// pending | applied | skipped | rejected
    pub state: String,
}

/// An item to insert; its position is its index. `state` is always
/// `pending` on insert, so it is not part of the value — two proposals with
/// the same items compare equal whatever their stored items' states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewChangesetItem {
    pub grp: String,
    pub catalog_id: Option<i64>,
    pub kind: String,
    pub name: String,
    pub action: String,
    pub params: Option<String>,
    pub decider: String,
}

impl From<&ChangesetItemRow> for NewChangesetItem {
    fn from(r: &ChangesetItemRow) -> Self {
        NewChangesetItem {
            grp: r.grp.clone(),
            catalog_id: r.catalog_id,
            kind: r.kind.clone(),
            name: r.name.clone(),
            action: r.action.clone(),
            params: r.params.clone(),
            decider: r.decider.clone(),
        }
    }
}

/// One `asset_triage_verdicts` row.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TriageVerdictRow {
    pub catalog_id: Option<i64>,
    pub kind: String,
    pub name: String,
    pub content_hash: String,
    /// ignored | rejected | host_local
    pub verdict: String,
    pub decider: String,
    pub decided_at: i64,
}

const CARD_COLS: &str =
    "id, kind, summary, state, created_at, applied_at, commits, layers_snapshot, error";
const ITEM_COLS: &str =
    "changeset_id, position, grp, catalog_id, kind, name, action, params, decider, state";

fn card_row(r: &rusqlite::Row<'_>) -> Result<ChangesetRow> {
    Ok(ChangesetRow {
        id: r.get(0)?,
        kind: r.get(1)?,
        summary: r.get(2)?,
        state: r.get(3)?,
        created_at: r.get(4)?,
        applied_at: r.get(5)?,
        commits: r.get(6)?,
        layers_snapshot: r.get(7)?,
        error: r.get(8)?,
    })
}

fn item_row(r: &rusqlite::Row<'_>) -> Result<ChangesetItemRow> {
    Ok(ChangesetItemRow {
        changeset_id: r.get(0)?,
        position: r.get(1)?,
        grp: r.get(2)?,
        catalog_id: r.get(3)?,
        kind: r.get(4)?,
        name: r.get(5)?,
        action: r.get(6)?,
        params: r.get(7)?,
        decider: r.get(8)?,
        state: r.get(9)?,
    })
}

fn insert_items(conn: &rusqlite::Connection, id: i64, items: &[NewChangesetItem]) -> Result<()> {
    let mut stmt = conn.prepare(
        "INSERT INTO changeset_items \
           (changeset_id, position, grp, catalog_id, kind, name, action, params, decider, state) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'pending')",
    )?;
    for (i, it) in items.iter().enumerate() {
        stmt.execute(rusqlite::params![
            id,
            i as i64,
            it.grp,
            it.catalog_id,
            it.kind,
            it.name,
            it.action,
            it.params,
            it.decider
        ])?;
    }
    Ok(())
}

impl Store {
    /// A new `proposed` card with its items, in one transaction.
    pub fn insert_changeset(
        &self,
        kind: &str,
        summary: &str,
        items: &[NewChangesetItem],
    ) -> Result<ChangesetRow> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO changesets (kind, summary, state, created_at) VALUES (?1, ?2, 'proposed', ?3)",
            rusqlite::params![kind, summary, now_unix()],
        )?;
        let id = tx.last_insert_rowid();
        insert_items(&tx, id, items)?;
        tx.commit()?;
        Ok(self.get_changeset(id)?.expect("just inserted"))
    }

    /// Refresh an OPEN card (proposed or failed) in place: new summary, new
    /// items, every item pending. `false` (and nothing written) when the
    /// card is not open — applied, undone and dismissed cards are history.
    pub fn replace_changeset_items(
        &self,
        id: i64,
        summary: &str,
        items: &[NewChangesetItem],
    ) -> Result<bool> {
        let tx = self.conn.unchecked_transaction()?;
        let n = tx.execute(
            "UPDATE changesets SET summary = ?2 WHERE id = ?1 AND state IN ('proposed', 'failed')",
            rusqlite::params![id, summary],
        )?;
        if n == 0 {
            return Ok(false);
        }
        tx.execute("DELETE FROM changeset_items WHERE changeset_id = ?1", [id])?;
        insert_items(&tx, id, items)?;
        tx.commit()?;
        Ok(true)
    }

    pub fn get_changeset(&self, id: i64) -> Result<Option<ChangesetRow>> {
        self.conn
            .query_row(
                &format!("SELECT {CARD_COLS} FROM changesets WHERE id = ?1"),
                [id],
                card_row,
            )
            .optional()
    }

    /// Every card, newest first.
    pub fn list_changesets(&self) -> Result<Vec<ChangesetRow>> {
        self.conn
            .prepare(&format!("SELECT {CARD_COLS} FROM changesets ORDER BY id DESC"))?
            .query_map([], card_row)?
            .collect()
    }

    /// A card's items, by position.
    pub fn changeset_items(&self, id: i64) -> Result<Vec<ChangesetItemRow>> {
        self.conn
            .prepare(&format!(
                "SELECT {ITEM_COLS} FROM changeset_items WHERE changeset_id = ?1 ORDER BY position"
            ))?
            .query_map([id], item_row)?
            .collect()
    }

    pub fn set_changeset_state(&self, id: i64, state: &str, error: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE changesets SET state = ?2, error = ?3 WHERE id = ?1",
            rusqlite::params![id, state, error],
        )?;
        Ok(())
    }

    /// The card applied: when, its commits and its `host_layers` snapshot
    /// (both JSON), and any warning (a reload or push that failed after the
    /// commits, Rulings R12).
    pub fn mark_changeset_applied(
        &self,
        id: i64,
        applied_at: i64,
        commits: &str,
        layers_snapshot: &str,
        error: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE changesets SET state = 'applied', applied_at = ?2, commits = ?3, \
             layers_snapshot = ?4, error = ?5 WHERE id = ?1",
            rusqlite::params![id, applied_at, commits, layers_snapshot, error],
        )?;
        Ok(())
    }

    pub fn set_changeset_item_states(&self, id: i64, positions: &[i64], state: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for p in positions {
            tx.execute(
                "UPDATE changeset_items SET state = ?3 WHERE changeset_id = ?1 AND position = ?2",
                rusqlite::params![id, p, state],
            )?;
        }
        tx.commit()
    }

    /// Record a verdict on `(kind, name, content_hash)`. A `person` verdict
    /// is never replaced by another decider's (spec: "an agent never
    /// overturns a person's verdict", Rulings R10); a person may replace
    /// anything.
    pub fn upsert_triage_verdict(&self, v: &TriageVerdictRow) -> Result<()> {
        self.conn.execute(
            "INSERT INTO asset_triage_verdicts \
               (catalog_id, kind, name, content_hash, verdict, decider, decided_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
             ON CONFLICT (kind, name, content_hash) DO UPDATE SET \
               catalog_id = excluded.catalog_id, verdict = excluded.verdict, \
               decider = excluded.decider, decided_at = excluded.decided_at \
             WHERE asset_triage_verdicts.decider != 'person' OR excluded.decider = 'person'",
            rusqlite::params![
                v.catalog_id,
                v.kind,
                v.name,
                v.content_hash,
                v.verdict,
                v.decider,
                v.decided_at
            ],
        )?;
        Ok(())
    }

    pub fn triage_verdicts(&self) -> Result<Vec<TriageVerdictRow>> {
        self.conn
            .prepare(
                "SELECT catalog_id, kind, name, content_hash, verdict, decider, decided_at \
                 FROM asset_triage_verdicts ORDER BY kind, name, content_hash",
            )?
            .query_map([], |r| {
                Ok(TriageVerdictRow {
                    catalog_id: r.get(0)?,
                    kind: r.get(1)?,
                    name: r.get(2)?,
                    content_hash: r.get(3)?,
                    verdict: r.get(4)?,
                    decider: r.get(5)?,
                    decided_at: r.get(6)?,
                })
            })?
            .collect()
    }

    /// `(catalog_id, layer)` of every layer some rollout card has applied a
    /// `sync` item for (Rulings R16) — on an applied or a failed card.
    pub fn rolled_out_layers(&self) -> Result<BTreeSet<(i64, String)>> {
        self.conn
            .prepare(
                "SELECT DISTINCT i.catalog_id, json_extract(i.params, '$.layer') \
                 FROM changeset_items i JOIN changesets c ON c.id = i.changeset_id \
                 WHERE c.kind = 'rollout' AND i.action = 'sync' AND i.state = 'applied' \
                   AND i.catalog_id IS NOT NULL AND json_extract(i.params, '$.layer') IS NOT NULL",
            )?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect()
    }
}
```

`store/layers.rs` — in `impl Store`:

```rust
    /// Put one catalog's `host_layers` back exactly as `rows` had them
    /// (Assets M4: undo and a failed apply, Rulings R12/R20). Rows of other
    /// catalogs in `rows` are ignored; a row whose host has been deleted
    /// since is skipped. Answers how many rows were written.
    pub fn restore_host_layers(
        &self,
        catalog_id: i64,
        rows: &[HostLayerRow],
    ) -> Result<usize, rusqlite::Error> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM host_layers WHERE catalog_id = ?1", [catalog_id])?;
        let mut n = 0;
        for r in rows.iter().filter(|r| r.catalog_id == catalog_id) {
            n += tx.execute(
                "INSERT INTO host_layers (host_alias, catalog_id, layer_name, axis, position, active) \
                 SELECT ?1, ?2, ?3, ?4, ?5, ?6 WHERE EXISTS (SELECT 1 FROM hosts WHERE alias = ?1)",
                rusqlite::params![
                    r.host_alias,
                    catalog_id,
                    r.layer_name,
                    r.axis,
                    r.position,
                    r.active as i64
                ],
            )?;
        }
        tx.commit()?;
        Ok(n)
    }
```

`store/rows.rs` — `CatalogRemoval` gains, after `grants`:

```rust
    /// Open changeset cards that named the catalog, withdrawn with it
    /// (Assets M4, Rulings R26).
    #[serde(default)]
    pub cards: usize,
```

`store/catalog.rs` — in `remove_catalog`, after the `grants` count line of the `CatalogRemoval { … }` literal add `cards: count("SELECT COUNT(DISTINCT c.id) FROM changesets c JOIN changeset_items i ON i.changeset_id = c.id WHERE i.catalog_id = ?1 AND c.state IN ('proposed', 'failed')")?,`, and before `tx.execute("DELETE FROM catalogs WHERE id = ?1", [row.id])?;` insert:

```rust
        // Assets M4 (R26): `changeset_items.catalog_id` has no ON DELETE
        // (the spec's DDL), so the cards that name this catalog let go of it
        // first: open ones are withdrawn, applied ones keep their history.
        tx.execute(
            "UPDATE changesets SET state = 'dismissed', error = ?2 \
             WHERE state IN ('proposed', 'failed') \
               AND id IN (SELECT changeset_id FROM changeset_items WHERE catalog_id = ?1)",
            rusqlite::params![row.id, format!("withdrawn: catalog {} was removed", row.name)],
        )?;
        tx.execute(
            "UPDATE changeset_items SET catalog_id = NULL WHERE catalog_id = ?1",
            [row.id],
        )?;
```

Also extend the `remove_catalog` doc comment with "Open changeset cards naming it are withdrawn and their items let go of it (Assets M4)."

- [ ] **Step 4: Run the tests to verify they pass**

Run, each on its own: `cargo test -p fleet-core migration_94`; `cargo test -p fleet-core migration_93`; `cargo test -p fleet-core store::changesets`; `cargo test -p fleet-core restore_host_layers`; `cargo test -p fleet-core open_in_memory_creates_all_tables`; `cargo test -p fleet-core removing_a_catalog`.
Expected: PASS (M3's `removing_a_catalog_drops_its_assignments_admissions_and_grants_but_never_personal` still passes: it destructures fields by name).

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/migrations/094_changesets.sql crates/fleet-core/src/store/
git commit -m "feat(store): changesets, changeset items and triage verdicts (migration 094)"
```

---

### Task 2: A catalog's load problems hold their keys (carry 2)

**Files:**
- Modify: `crates/fleet-core/src/service/catalog/repo.rs` (`ProblemHolds` + tests)
- Modify: `crates/fleet-core/src/service/catalog/model.rs` (drop `#[allow(dead_code)]` and its comment on `Kind::from_dir` — now used)
- Modify: `crates/fleet-core/src/service/catalog/effective.rs` (`EffectiveSet.problem_held` + test)
- Modify: `crates/fleet-core/src/service/catalog/sync/plan.rs` (`KeepRules.problem_held`, the held `Noop`, test; the literal at `:1859` gains `..Default::default()`)
- Modify: `crates/fleet-core/src/service/catalog/sync/mod.rs` (`KeepRules` literal at `:402`)

**Interfaces:**
- Consumes: `model::{Kind, Problem}`, `Kind::from_dir`, `plan::held_noop`.
- Produces:

```rust
// repo.rs
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProblemHolds { pub kinds: BTreeMap<Kind, String>, pub assets: BTreeMap<(Kind, String), String> }
impl ProblemHolds {
    pub fn from_problems(problems: &[Problem]) -> ProblemHolds;
    pub fn reason(&self, kind: Kind, name: &str) -> Option<&str>;
    pub fn is_empty(&self) -> bool;
}
// effective.rs — EffectiveSet gains (never serialized):
#[serde(skip)] pub problem_held: BTreeMap<String, ProblemHolds>,
// sync/plan.rs — KeepRules gains:
pub problem_held: BTreeMap<String, ProblemHolds>,
```

- [ ] **Step 1: Write the failing tests**

`repo.rs` tests module:

```rust
    /// Carry 2 (Rulings R24): a problem holds the asset its path names, or a
    /// whole kind when the kind's directory could not be read; layer,
    /// catalog-file and absolute (problem-entry) paths hold nothing.
    #[test]
    fn problem_holds_name_the_asset_or_the_kind_a_problem_is_about() {
        let p = |path: &str| Problem {
            path: path.into(),
            message: format!("bad {path}"),
        };
        let h = ProblemHolds::from_problems(&[
            p("skills/broken/asset.yaml"),
            p("hooks/stop.yaml"),
            p("agents"),
            p("layers/core.yaml"),
            p("layers"),
            p("/abs/repo"),
        ]);
        assert_eq!(h.reason(Kind::Skill, "broken"), Some("bad skills/broken/asset.yaml"));
        assert_eq!(h.reason(Kind::Hook, "stop"), Some("bad hooks/stop.yaml"));
        assert_eq!(h.reason(Kind::Agent, "anything"), Some("bad agents"));
        assert_eq!(h.reason(Kind::Skill, "fine"), None);
        assert_eq!(h.assets.len() + h.kinds.len(), 3);
    }

    /// The loader's own paths: a skill whose asset.yaml does not parse.
    #[test]
    fn load_dir_problems_become_holds() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("catalog.yaml"), "schema_version: 1\n").unwrap();
        std::fs::create_dir_all(root.path().join("skills/broken")).unwrap();
        std::fs::write(root.path().join("skills/broken/asset.yaml"), "kind: skill\nname: [\n").unwrap();
        let cat = load_dir(root.path()).unwrap();
        let h = ProblemHolds::from_problems(&cat.problems);
        assert!(h.reason(Kind::Skill, "broken").is_some(), "{:?}", cat.problems);
    }
```

`effective.rs` tests module (uses the module's `seeded_store`, `personal_cat`, `skill` helpers):

```rust
    /// Carry 2: a speaking catalog's load problems are reported per key, so
    /// the planner can keep a broken asset's copies (Rulings R24).
    #[test]
    fn a_speaking_catalogs_load_problems_are_held_per_key() {
        let _g = crate::service::catalog::lock_registry_for_test();
        let (store, personal, _acme) = seeded_store();
        let mut cat = personal_cat(personal, vec![skill("a", "private")], LayerSet::default());
        cat.problems.push(crate::service::catalog::model::Problem {
            path: "skills/broken/asset.yaml".into(),
            message: "bad yaml".into(),
        });
        registry::install_personal(cat).unwrap();
        let e = effective_for_host(&store, "h").unwrap();
        let holds = e.problem_held.get("personal").expect("held");
        assert_eq!(holds.reason(Kind::Skill, "broken"), Some("bad yaml"));
        assert_eq!(holds.reason(Kind::Skill, "a"), None);
    }
```

`sync/plan.rs` tests module (add `use crate::service::catalog::model::Problem; use crate::service::catalog::repo::ProblemHolds;` at its top):

```rust
    /// Carry 2 (Rulings R24): an entry whose catalog speaks but whose own
    /// file is a load problem is kept with a `Noop`, never removed; so is
    /// every entry of a kind whose directory could not be read. A sibling
    /// the catalog really dropped is still removed.
    #[test]
    fn an_asset_the_catalog_could_not_read_is_kept_not_removed() {
        let mut manifest = Manifest::default();
        for name in ["broken", "gone"] {
            manifest.assets.insert(
                format!("skill/{name}"),
                ManifestEntry {
                    files: vec![format!("~/.claude/skills/{name}/SKILL.md")],
                    ..Default::default()
                },
            );
        }
        manifest.assets.insert("agent/x".into(), ManifestEntry::default());
        let holds = ProblemHolds::from_problems(&[
            Problem {
                path: "skills/broken/asset.yaml".into(),
                message: "bad yaml".into(),
            },
            Problem {
                path: "agents".into(),
                message: "permission denied".into(),
            },
        ]);
        let keep = KeepRules {
            speaks_for: Some(BTreeSet::from(["personal".to_string()])),
            problem_held: BTreeMap::from([("personal".to_string(), holds)]),
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
        let broken = act(&hp, "broken");
        assert_eq!(broken.op, ActionOp::Noop);
        assert!(broken.reason.as_deref().unwrap().contains("bad yaml"), "{:?}", broken.reason);
        assert!(broken.remove_entry.is_none());
        assert_eq!(act(&hp, "x").op, ActionOp::Noop, "an unreadable kind dir holds its entries");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p fleet-core problem_holds` — Expected: compile error, `ProblemHolds` not found.

- [ ] **Step 3: Write the implementation**

`repo.rs` (after `Catalog`'s `impl`):

```rust
/// What a catalog's load problems put in doubt (Assets M4, carry 2,
/// Rulings R24). `load_dir` records a file that did not parse at
/// `<kind dir>/<name>/asset.yaml` (skills, agents) or `<kind dir>/<name>.yaml`
/// (the rest), and a kind directory it could not read at `<kind dir>`. The
/// first holds that one asset, the second every asset of the kind. Layer
/// and catalog-file problems hold nothing. A sync must never read a held
/// asset's absence as "the catalog dropped it" (`sync::plan::KeepRules`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProblemHolds {
    /// Kind → why its whole directory could not be read.
    pub kinds: BTreeMap<Kind, String>,
    /// `(kind, name)` → why its file did not load.
    pub assets: BTreeMap<(Kind, String), String>,
}

impl ProblemHolds {
    pub fn from_problems(problems: &[Problem]) -> ProblemHolds {
        let mut out = ProblemHolds::default();
        for p in problems {
            let parts: Vec<&str> = p.path.split(['/', '\\']).collect();
            let Some(kind) = parts.first().and_then(|d| Kind::from_dir(d)) else {
                continue;
            };
            match parts.as_slice() {
                [_] => {
                    out.kinds.insert(kind, p.message.clone());
                }
                [_, name, "asset.yaml"] if kind.is_folder() => {
                    out.assets.insert((kind, (*name).to_string()), p.message.clone());
                }
                [_, file] if !kind.is_folder() => {
                    if let Some(name) = file.strip_suffix(".yaml") {
                        out.assets.insert((kind, name.to_string()), p.message.clone());
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// Why `kind/name` is held, if it is.
    pub fn reason(&self, kind: Kind, name: &str) -> Option<&str> {
        self.assets
            .get(&(kind, name.to_string()))
            .or_else(|| self.kinds.get(&kind))
            .map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty() && self.assets.is_empty()
    }
}
```

`model.rs`: remove the two comment lines and `#[allow(dead_code)]` above `pub fn from_dir`, leaving the doc `/// The kind whose catalog directory is \`dir\`.`

`effective.rs`:
- `use crate::service::catalog::repo::{Catalog, CatalogRef, ProblemHolds};`
- `EffectiveSet` gains, after `not_accepted`:

```rust
    /// Assets M4 (carry 2, R24): catalog → the keys its own load problems
    /// put in doubt, for every catalog in `speaks_for` that has any. A held
    /// key's manifest entries are kept, never removed as orphans. Never
    /// serialized (problem messages can name paths on the hub's machine).
    #[serde(skip)]
    pub problem_held: BTreeMap<String, ProblemHolds>,
```

- In `compose`, declare `let mut problem_held: BTreeMap<String, ProblemHolds> = BTreeMap::new();` next to `held_back`, replace `speaks_for.insert(label);` with:

```rust
        let holds = ProblemHolds::from_problems(&cat.problems);
        if !holds.is_empty() {
            problem_held.insert(label.clone(), holds);
        }
        speaks_for.insert(label);
```

- and add `problem_held,` to the `Ok(EffectiveSet { … })` literal.

`sync/plan.rs`:
- `use crate::service::catalog::repo::ProblemHolds;` (with the other imports at the top).
- `KeepRules` gains, after `held_back`:

```rust
    /// Catalog → the keys its load problems hold (Assets M4, carry 2,
    /// `EffectiveSet::problem_held`): an entry of that catalog whose key is
    /// held gets a `Noop` saying why, never a `Remove`.
    pub problem_held: BTreeMap<String, ProblemHolds>,
```

- In `compute_host_plan`'s orphan loop, right after `if !filter.matches(kind, &name) { continue; }`, insert:

```rust
        if let Some(why) = keep
            .problem_held
            .get(&entry.catalog)
            .and_then(|h| h.reason(kind, &name))
        {
            actions.push(held_noop(
                kind,
                &name,
                &entry.catalog,
                format!(
                    "catalog {} could not read it ({why}); its copy is kept, not removed",
                    entry.catalog
                ),
            ));
            continue;
        }
```

- Rule 6 of the `compute_host_plan` doc gains: "An entry whose own catalog speaks but whose key that catalog could not read (`keep.problem_held`, Assets M4) is kept the same way."
- The test literal at `:1859` gains `..Default::default()` as its last line.

`sync/mod.rs` — the `plan::KeepRules { … }` literal gains `problem_held: eff.problem_held.clone(),`.

- [ ] **Step 4: Run the tests to verify they pass**

Run, each on its own: `cargo test -p fleet-core problem_holds`; `cargo test -p fleet-core load_dir_problems_become_holds`; `cargo test -p fleet-core a_speaking_catalogs_load_problems`; `cargo test -p fleet-core an_asset_the_catalog_could_not_read`; `cargo test -p fleet-core sync::plan`; `cargo test -p fleet-core effective`.
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/service/catalog/
git commit -m "fix(sync): an asset its catalog could not read is kept, not removed"
```

---

### Task 3: Authoring per catalog, and one catalog lookup per call (carry 1, M-d)

**Files:**
- Modify: `crates/fleet-core/src/service/catalog/mod.rs` (`CatalogTarget`, `get_asset_in`, `import_host_into`)
- Modify: `crates/fleet-core/src/service/catalog/author.rs` (`*_in` functions; personal wrappers; test)
- Modify: `crates/fleet-core/src/service/catalog/admin.rs` (`is_per_catalog`, `touches`, `run`; tests)
- Modify: `crates/fleet-core/src/mcp/tools/assets.rs` (`catalog_admin` resolves once; `may_admin_catalog_row`, `lookup_catalog`)
- Modify: `crates/fleet-core/src/mcp/tools/params.rs` (`CatalogAdminParams.catalog` doc)
- Modify: `crates/fleet-core/src/mcp/tools/tests_catalog_admin.rs`
- Modify: `crates/fleet-core/src/mcp/tools/tests.rs` (`BUDGET_BYTES`, measured)
- Modify: `docs/control-api-reference.md` (generated)

**Interfaces:**
- Consumes: `registry::{with_personal, with_catalog_row}`, `load`, `load_catalog`, `require_config`, `Store::get_catalog_by_name`, `Store::client_may_admin_catalog`, `Store::client_is_assets_admin`.
- Produces:

```rust
// service/catalog/mod.rs
#[derive(Debug, Clone, Copy)]
pub enum CatalogTarget<'a> { Personal, Row(&'a CatalogRow) }
impl CatalogTarget<'_> {
    pub(crate) fn root(self, store: &Mutex<Store>) -> Result<std::path::PathBuf, IpcError>;
    pub(crate) fn with<T>(self, f: impl FnOnce(&repo::Catalog) -> Result<T, IpcError>) -> Result<T, IpcError>;
    pub(crate) fn reload(self, store: &Mutex<Store>) -> Result<CatalogSummary, IpcError>;
}
pub fn get_asset_in(target: CatalogTarget<'_>, kind: Kind, name: &str, store: &Mutex<Store>) -> Result<AssetDetail, IpcError>;
pub async fn import_host_into(target: CatalogTarget<'_>, args: ImportArgs, store: &Mutex<Store>, ssh: &Arc<SshClient>, fleet_token: Option<&str>) -> Result<import::ImportReport, IpcError>;
// service/catalog/author.rs — each gains an `_in` twin taking `target: CatalogTarget<'_>` first:
// create_in, update_in, delete_asset_in, write_layer_in, delete_layer_in, add_resource_bytes_in,
// remove_resource_in, commit_pending_in, push_in, repo_status_in, lint_asset_in, lint_everything_in.
// service/catalog/admin.rs
pub async fn run(call: AdminCall, catalog: Option<&CatalogRow>, store: &Mutex<Store>, ssh: &Arc<SshClient>, reg: &Arc<CancellationRegistry>) -> Result<serde_json::Value, IpcError>;
// mcp/tools/assets.rs
fn may_admin_catalog_row(caller: &Caller, store: &Mutex<Store>, name: &str, row: Option<&CatalogRow>) -> Result<bool, McpError>;
fn may_admin_catalog(caller: &Caller, store: &Mutex<Store>, name: &str) -> Result<bool, McpError>; // unchanged signature, now a wrapper
fn lookup_catalog(store: &Mutex<Store>, name: &str) -> Result<Option<CatalogRow>, McpError>;
```

- [ ] **Step 1: Write the failing tests**

`author.rs` tests module (uses its `init_repo`, `configured_store`, `git`, `subjects` helpers):

```rust
    /// Carry 1 (Rulings R21): an authoring call aimed at an org catalog
    /// writes, commits and reloads that catalog's checkout — personal's
    /// HEAD never moves.
    #[test]
    fn authoring_writes_the_named_catalogs_checkout_only() {
        let _g = lock_registry_for_test();
        let personal = init_repo("m4-personal");
        let acme_root = init_repo("m4-acme");
        let store = configured_store(&personal);
        let acme = {
            let s = store.lock().unwrap();
            let org = s.add_org("acme", None, false).unwrap();
            s.upsert_catalog("acme", &acme_root.to_string_lossy(), None, Some(org.id))
                .unwrap()
        };
        crate::service::catalog::load(false, &store).unwrap();
        crate::service::catalog::load_catalog(acme.id, false, &store).unwrap();
        let personal_head = git(&personal, &["rev-parse", "HEAD"]);
        let target = CatalogTarget::Row(&acme);

        create_in(
            target,
            CreateArgs {
                kind: Kind::Skill,
                name: "ops".into(),
                duplicate_from: None,
            },
            &store,
        )
        .unwrap();
        assert!(acme_root.join("skills/ops/asset.yaml").is_file());
        assert!(!personal.join("skills/ops").exists());
        assert_eq!(subjects(&acme_root)[0], "catalog: create skill/ops");
        assert_eq!(git(&personal, &["rev-parse", "HEAD"]), personal_head, "personal untouched");
        assert!(
            registry::with_catalog_row(&acme, |c| Ok(c.find(Kind::Skill, "ops").is_some())).unwrap(),
            "acme reloaded, not personal"
        );

        write_layer_in(target, &layer_template("ops-core", Axis::Context), &store).unwrap();
        assert!(acme_root.join("layers/ops-core.yaml").is_file());
        assert_eq!(repo_status_in(target, &store).unwrap().dirty, 0);
        let ops = AssetRef {
            kind: Kind::Skill,
            name: "ops".into(),
        };
        assert!(lint_asset_in(target, ops.clone(), &store).is_ok());
        delete_asset_in(target, ops, &store).unwrap();
        assert!(!acme_root.join("skills/ops").exists());
        assert_eq!(git(&personal, &["rev-parse", "HEAD"]), personal_head);
    }
```

(`AssetRef` derives `Clone`; `registry` stays imported in the tests module through `use super::*` only if author.rs still imports it — add `use crate::service::catalog::registry;` to the tests module.)

`admin.rs` tests — in `touches_names_the_catalog_a_grant_is_checked_on`, replace

```rust
        let e = AdminCall::Push.touches(Some("acme")).unwrap_err();
        assert!(e.message.contains("M4"), "{}", e.message);
```

with

```rust
        assert_eq!(
            AdminCall::Push.touches(Some("acme")).unwrap(),
            Touches::Catalog("acme".into()),
            "authoring is per catalog since M4 (R21)"
        );
        let configure = AdminCall::Configure(ConfigureArgs {
            repo_path: "/x".into(),
            remote_url: None,
        });
        let e = configure.touches(Some("acme")).unwrap_err();
        assert!(e.message.contains("add_catalog"), "configure stays personal: {}", e.message);
```

`tests_catalog_admin.rs` — in `each_action_needs_a_grant_on_the_catalog_it_touches`, replace the PF16 block's `create_asset` assertions (from `let create = json!(…)` through the `"authoring is personal-only until M4"` assert) with:

```rust
    // R21: authoring is per catalog — the grant decides, not the action.
    let r = call_on(&t, &desk, "repo_status", None, Some("acme")).await;
    assert_eq!(code_of(&r), "E_FORBIDDEN");
    let r = call_on(&t, &ops, "repo_status", None, Some("acme")).await;
    assert_ne!(code_of(&r), "E_FORBIDDEN", "{:?}", r.err());
    assert_ne!(code_of(&r), "E_INVALID", "past the gate, into acme's checkout");
```

and add:

```rust
/// R22 (M-d): the master naming a catalog that does not exist is told so at
/// the gate — the call never falls through to personal.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn the_master_naming_an_unknown_catalog_gets_not_found() {
    let _g = crate::service::catalog::lock_registry_for_test();
    let (s, ..) = two_catalog_store();
    let t = tools(s);
    let r = call_on(&t, &Caller::master(), "repo_status", None, Some("ghost")).await;
    assert_eq!(code_of(&r), "E_NOTFOUND");
    assert!(message_of(r).contains("no catalog named ghost"));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p fleet-core authoring_writes_the_named_catalogs_checkout_only` — Expected: compile error, `create_in` / `CatalogTarget` not found.

- [ ] **Step 3: Write the implementation**

`service/catalog/mod.rs` — after `require_config`:

```rust
/// Which catalog an authoring call reads and writes (Assets M4, Rulings
/// R21). `Personal` keeps every pre-M4 path exactly — `require_config` for
/// the checkout, `registry::with_personal` for the loaded catalog, `load`
/// to reload — so the desktop's commands behave as before; `Row` is a
/// catalog the caller resolved (`catalog_admin`'s gate, R22).
#[derive(Debug, Clone, Copy)]
pub enum CatalogTarget<'a> {
    Personal,
    Row(&'a CatalogRow),
}

impl CatalogTarget<'_> {
    /// The checkout this target writes.
    pub(crate) fn root(self, store: &Mutex<Store>) -> Result<std::path::PathBuf, IpcError> {
        match self {
            CatalogTarget::Personal => Ok(std::path::PathBuf::from(require_config(store)?.repo_path)),
            CatalogTarget::Row(r) => Ok(std::path::PathBuf::from(&r.repo_path)),
        }
    }

    /// Borrow its loaded catalog. Same lock rule as `registry::with_*`: `f`
    /// must never take the store.
    pub(crate) fn with<T>(
        self,
        f: impl FnOnce(&repo::Catalog) -> Result<T, IpcError>,
    ) -> Result<T, IpcError> {
        match self {
            CatalogTarget::Personal => registry::with_personal(f),
            CatalogTarget::Row(r) => registry::with_catalog_row(r, f),
        }
    }

    /// Reload it after a write.
    pub(crate) fn reload(self, store: &Mutex<Store>) -> Result<CatalogSummary, IpcError> {
        match self {
            CatalogTarget::Personal => load(false, store),
            CatalogTarget::Row(r) => load_catalog(r.id, false, store),
        }
    }
}
```

Replace `import_host`'s body and add its twin:

```rust
pub async fn import_host(
    args: ImportArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    fleet_token: Option<&str>,
) -> Result<import::ImportReport, IpcError> {
    import_host_into(CatalogTarget::Personal, args, store, ssh, fleet_token).await
}

/// [`import_host`] into `target`'s checkout (Assets M4: `catalog_admin
/// import_host` with a `catalog`, and a card's apply).
pub async fn import_host_into(
    target: CatalogTarget<'_>,
    args: ImportArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    fleet_token: Option<&str>,
) -> Result<import::ImportReport, IpcError> {
    let repo = target.root(store)?;
    if args.host_alias == "local" {
        crate::service::hub::ensure_local_allowed(&args.host_alias)?;
        let src = import::ImportSources::for_local()?;
        return import::import_claude_only(&src, &repo, "local", fleet_token, args.dry_run, &args.only);
    }
    require_dialable_host(store, &args.host_alias)?;
    let script = import::REMOTE_SOURCES_SCRIPT;
    let out = inventory::run_host_script(ssh, &args.host_alias, script).await?;
    let tmp = tempfile::tempdir()
        .map_err(|e| IpcError::new(codes::E_IO, format!("remote import: {e}")))?;
    let src = import::parse_remote_dump(&out, tmp.path())?;
    import::scrub_fleet_entries(&src)?;
    import::import_claude_only(&src, &repo, &args.host_alias, fleet_token, args.dry_run, &args.only)
}
```

(the doc comment of `import_host` stays on `import_host`). Replace `get_asset` with:

```rust
pub fn get_asset(kind: Kind, name: &str, store: &Mutex<Store>) -> Result<AssetDetail, IpcError> {
    get_asset_in(CatalogTarget::Personal, kind, name, store)
}

/// [`get_asset`] out of `target`'s loaded catalog (Assets M4).
pub fn get_asset_in(
    target: CatalogTarget<'_>,
    kind: Kind,
    name: &str,
    store: &Mutex<Store>,
) -> Result<AssetDetail, IpcError> {
    let rows = inventory(store)?;
    target.with(|cat| {
        let asset = cat.find(kind, name).ok_or_else(|| {
            IpcError::new(
                E_ASSET_NOT_FOUND,
                format!("{} {name} is not in the catalog", kind.as_str()),
            )
        })?;
        let previews = harness::all()
            .iter()
            .map(|h| match h.render(asset) {
                Ok(plan) => Preview {
                    harness: h.id().into(),
                    plan: Some(plan),
                    unsupported: None,
                },
                Err(u) => Preview {
                    harness: h.id().into(),
                    plan: None,
                    unsupported: Some(u.into_ipc().message),
                },
            })
            .collect();
        Ok(AssetDetail {
            asset: asset.clone(),
            previews,
            hosts: host_states(&rows, kind, name),
        })
    })
}
```

`author.rs` — `use super::CatalogTarget;`; delete `repo_root`; drop `use super::registry;` once nothing in the file but its tests uses it. Replace the four helpers:

```rust
/// A clone of the named asset out of `target`'s loaded catalog, borrowed
/// (only the one `Asset` is cloned). Not loaded and not present are the same
/// answer, as before: `E_ASSET_NOT_FOUND`.
fn catalog_asset(target: CatalogTarget<'_>, kind: Kind, name: &str) -> Result<Asset, IpcError> {
    let not_found = || {
        IpcError::new(
            E_ASSET_NOT_FOUND,
            format!("{} {name} is not in the catalog", kind.as_str()),
        )
    };
    match target.with(|c| Ok(c.find(kind, name).cloned())) {
        Ok(found) => found.ok_or_else(not_found),
        Err(e) if e.code == super::E_CATALOG_NOT_CONFIGURED => Err(not_found()),
        Err(e) => Err(e),
    }
}

/// Lint against `target` as loaded (not loaded: an empty catalog). Borrows;
/// never clones the catalog (M1 carry, R16).
fn lint_in_repo(target: CatalogTarget<'_>, asset: &Asset, root: &Path) -> LintReport {
    let names = secrets_example_names(root);
    let exists = root.join(SECRETS_EXAMPLE).exists();
    target
        .with(|c| Ok(lint(asset, c, &names, exists)))
        .unwrap_or_else(|_| lint(asset, &Catalog::default(), &names, exists))
}

/// Stage `rel_paths`, commit them (skipped when nothing changed), reload
/// `target` and answer the HEAD — unchanged behaviour, per catalog.
fn commit_and_reload(
    target: CatalogTarget<'_>,
    root: &Path,
    rel_paths: &[String],
    message: &str,
    store: &Mutex<Store>,
) -> Result<String, IpcError> {
    repo::stage_paths(root, rel_paths)?;
    let commit = if repo::has_staged(root, rel_paths)? {
        repo::commit(root, message)?
    } else {
        repo::head(root)?
    };
    target.reload(store)?;
    Ok(commit)
}

fn write_commit_reload(
    target: CatalogTarget<'_>,
    root: &Path,
    asset: &Asset,
    overwrite: bool,
    message: &str,
    store: &Mutex<Store>,
) -> Result<String, IpcError> {
    repo::write_asset(root, asset, overwrite)?;
    let rel = repo::asset_rel_dir(asset.kind(), &asset.header.name);
    commit_and_reload(target, root, &[rel], message, store)
}
```

Then each public operation becomes an `_in` function with the same body except that `let root = repo_root(store)?;` becomes `let root = target.root(store)?;`, `catalog_asset(` gains `target, ` as its first argument, `lint_in_repo(` gains `target, `, `commit_and_reload(` and `write_commit_reload(` gain `target, `; and the old name becomes a one-line personal wrapper. In full:

```rust
pub fn create(args: CreateArgs, store: &Mutex<Store>) -> Result<WriteResult, IpcError> {
    create_in(CatalogTarget::Personal, args, store)
}

/// [`create`] in `target` (Assets M4, R21).
pub fn create_in(
    target: CatalogTarget<'_>,
    args: CreateArgs,
    store: &Mutex<Store>,
) -> Result<WriteResult, IpcError> {
    check_name(&args.name)?;
    if let Some(from) = args.duplicate_from.as_deref() {
        check_name(from)?;
    }
    let root = target.root(store)?;
    let asset = match args.duplicate_from.as_deref() {
        Some(from) => {
            let mut a = catalog_asset(target, args.kind, from)?;
            a.header.name = args.name.clone();
            a.header.source = None;
            a.header.scope = Scope::Private; // R15: a copy is private until marked
            a
        }
        None => template(args.kind, &args.name),
    };
    let message = format!("catalog: create {}/{}", args.kind.as_str(), args.name);
    let commit = write_commit_reload(target, &root, &asset, false, &message, store)?;
    Ok(WriteResult {
        commit,
        lint: lint_in_repo(target, &asset, &root),
    })
}

pub fn update(args: UpdateArgs, store: &Mutex<Store>) -> Result<WriteResult, IpcError> {
    update_in(CatalogTarget::Personal, args, store)
}

/// [`update`] in `target`.
pub fn update_in(
    target: CatalogTarget<'_>,
    args: UpdateArgs,
    store: &Mutex<Store>,
) -> Result<WriteResult, IpcError> {
    let asset = args.asset;
    let kind = asset.kind();
    check_name(&asset.header.name)?;
    for r in &asset.resources {
        check_resource_path(&r.rel_path)?;
    }
    let root = target.root(store)?;
    if !repo::asset_path(&root, kind, &asset.header.name).exists() {
        return Err(IpcError::new(
            E_ASSET_NOT_FOUND,
            format!(
                "{} {} is not in the catalog repo",
                kind.as_str(),
                asset.header.name
            ),
        ));
    }
    let report = lint_in_repo(target, &asset, &root);
    if !report.errors.is_empty() {
        let details = serde_json::to_value(&report)
            .map_err(|e| IpcError::new(E_SERIALIZE, format!("lint report: {e}")))?;
        return Err(
            IpcError::new(E_LINT, "the asset has lint errors and was not saved")
                .with_details(details),
        );
    }
    let message = format!("catalog: update {}/{}", kind.as_str(), asset.header.name);
    let commit = write_commit_reload(target, &root, &asset, true, &message, store)?;
    Ok(WriteResult {
        commit,
        lint: report,
    })
}

pub fn delete_asset(args: AssetRef, store: &Mutex<Store>) -> Result<String, IpcError> {
    delete_asset_in(CatalogTarget::Personal, args, store)
}

/// [`delete_asset`] in `target`.
pub fn delete_asset_in(
    target: CatalogTarget<'_>,
    args: AssetRef,
    store: &Mutex<Store>,
) -> Result<String, IpcError> {
    check_name(&args.name)?;
    let root = target.root(store)?;
    repo::remove_asset(&root, args.kind, &args.name)?;
    let rel = repo::asset_rel_dir(args.kind, &args.name);
    let message = format!("catalog: delete {}/{}", args.kind.as_str(), args.name);
    commit_and_reload(target, &root, &[rel], &message, store)
}

pub fn write_layer(layer: &Layer, store: &Mutex<Store>) -> Result<String, IpcError> {
    write_layer_in(CatalogTarget::Personal, layer, store)
}

/// [`write_layer`] in `target`.
pub fn write_layer_in(
    target: CatalogTarget<'_>,
    layer: &Layer,
    store: &Mutex<Store>,
) -> Result<String, IpcError> {
    check_layer_name(&layer.name)?;
    let root = target.root(store)?;
    if let Err(e) = layer.validate() {
        let details = serde_json::to_value(&e)
            .map_err(|e| IpcError::new(E_SERIALIZE, format!("lint report: {e}")))?;
        return Err(
            IpcError::new(E_LINT, "the layer has lint errors and was not saved")
                .with_details(details),
        );
    }
    let rel = layer_rel_path(&layer.name);
    let path = root.join(&rel);
    let verb = if path.exists() { "update" } else { "create" };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, layer.to_yaml())?;
    let message = layer_commit_message(verb, &layer.name);
    commit_and_reload(target, &root, &[rel], &message, store)
}

pub fn delete_layer(name: &str, store: &Mutex<Store>) -> Result<String, IpcError> {
    delete_layer_in(CatalogTarget::Personal, name, store)
}

/// [`delete_layer`] in `target`.
pub fn delete_layer_in(
    target: CatalogTarget<'_>,
    name: &str,
    store: &Mutex<Store>,
) -> Result<String, IpcError> {
    check_layer_name(name)?;
    let root = target.root(store)?;
    let rel = layer_rel_path(name);
    let path = root.join(&rel);
    if !path.is_file() {
        return Err(IpcError::new(
            E_ASSET_NOT_FOUND,
            format!("layer {name} not found in the catalog"),
        ));
    }
    std::fs::remove_file(&path)?;
    let message = layer_commit_message("delete", name);
    commit_and_reload(target, &root, &[rel], &message, store)
}

pub fn add_resource_bytes(
    args: AddResourceBytesArgs,
    store: &Mutex<Store>,
) -> Result<WriteResult, IpcError> {
    add_resource_bytes_in(CatalogTarget::Personal, args, store)
}

/// [`add_resource_bytes`] in `target`.
pub fn add_resource_bytes_in(
    target: CatalogTarget<'_>,
    args: AddResourceBytesArgs,
    store: &Mutex<Store>,
) -> Result<WriteResult, IpcError> {
    check_name(&args.name)?;
    check_has_resources(args.kind)?;
    check_resource_path(&args.rel_path)?;
    let root = target.root(store)?;
    let len = args.bytes.len() as u64;
    if len > MAX_RESOURCE_BYTES {
        return Err(too_big(&args.rel_path, len));
    }
    let rel_path = args.rel_path;
    let mut asset = catalog_asset(target, args.kind, &args.name)?;
    asset.resources.retain(|r| r.rel_path != rel_path);
    asset.resources.push(super::model::Resource {
        rel_path: rel_path.clone(),
        bytes: args.bytes,
    });
    asset.resources.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    let message = resource_message(args.kind, &args.name);
    let commit = write_commit_reload(target, &root, &asset, true, &message, store)?;
    Ok(WriteResult {
        commit,
        lint: lint_in_repo(target, &asset, &root),
    })
}

pub fn remove_resource(
    args: RemoveResourceArgs,
    store: &Mutex<Store>,
) -> Result<WriteResult, IpcError> {
    remove_resource_in(CatalogTarget::Personal, args, store)
}

/// [`remove_resource`] in `target`.
pub fn remove_resource_in(
    target: CatalogTarget<'_>,
    args: RemoveResourceArgs,
    store: &Mutex<Store>,
) -> Result<WriteResult, IpcError> {
    check_name(&args.name)?;
    check_has_resources(args.kind)?;
    check_resource_path(&args.rel_path)?;
    let root = target.root(store)?;
    let mut asset = catalog_asset(target, args.kind, &args.name)?;
    let before = asset.resources.len();
    asset.resources.retain(|r| r.rel_path != args.rel_path);
    if asset.resources.len() == before {
        return Err(IpcError::new(
            E_ASSET_NOT_FOUND,
            format!("{} has no resource {}", args.name, args.rel_path),
        ));
    }
    let message = resource_message(args.kind, &args.name);
    let commit = write_commit_reload(target, &root, &asset, true, &message, store)?;
    Ok(WriteResult {
        commit,
        lint: lint_in_repo(target, &asset, &root),
    })
}

pub fn commit_pending(args: CommitPendingArgs, store: &Mutex<Store>) -> Result<String, IpcError> {
    commit_pending_in(CatalogTarget::Personal, args, store)
}

/// [`commit_pending`] in `target`.
pub fn commit_pending_in(
    target: CatalogTarget<'_>,
    args: CommitPendingArgs,
    store: &Mutex<Store>,
) -> Result<String, IpcError> {
    let root = target.root(store)?;
    let message = args
        .message
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .unwrap_or("catalog: commit pending changes")
        .to_string();
    if repo::git_status(&root)?.dirty == 0 {
        return Err(IpcError::new(E_CATALOG_GIT, "nothing to commit"));
    }
    commit_and_reload(target, &root, &[], &message, store)
}

pub fn push(store: &Mutex<Store>) -> Result<RepoStatus, IpcError> {
    push_in(CatalogTarget::Personal, store)
}

/// [`push`] `target`'s checkout.
pub fn push_in(target: CatalogTarget<'_>, store: &Mutex<Store>) -> Result<RepoStatus, IpcError> {
    let root = target.root(store)?;
    repo::push(&root)?;
    repo::git_status(&root)
}

pub fn repo_status(store: &Mutex<Store>) -> Result<RepoStatus, IpcError> {
    repo_status_in(CatalogTarget::Personal, store)
}

/// [`repo_status`] of `target`'s checkout.
pub fn repo_status_in(target: CatalogTarget<'_>, store: &Mutex<Store>) -> Result<RepoStatus, IpcError> {
    repo::git_status(&target.root(store)?)
}

pub fn lint_asset(args: AssetRef, store: &Mutex<Store>) -> Result<LintReport, IpcError> {
    lint_asset_in(CatalogTarget::Personal, args, store)
}

/// [`lint_asset`] in `target`.
pub fn lint_asset_in(
    target: CatalogTarget<'_>,
    args: AssetRef,
    store: &Mutex<Store>,
) -> Result<LintReport, IpcError> {
    check_name(&args.name)?;
    let root = target.root(store)?;
    let asset = catalog_asset(target, args.kind, &args.name)?;
    Ok(lint_in_repo(target, &asset, &root))
}

pub fn lint_everything(store: &Mutex<Store>) -> Result<LintAll, IpcError> {
    lint_everything_in(CatalogTarget::Personal, store)
}

/// [`lint_everything`] over `target`, borrowed (R16).
pub fn lint_everything_in(
    target: CatalogTarget<'_>,
    store: &Mutex<Store>,
) -> Result<LintAll, IpcError> {
    let root = target.root(store)?;
    let names = secrets_example_names(&root);
    let exists = root.join(SECRETS_EXAMPLE).exists();
    match target.with(|c| Ok(lint_all_with(c, &names, exists))) {
        Ok(all) => Ok(all),
        Err(e) if e.code == super::E_CATALOG_NOT_CONFIGURED => {
            Ok(lint_all_with(&Catalog::default(), &names, exists))
        }
        Err(e) => Err(e),
    }
}
```

Keep each original doc comment on the personal wrapper. `add_resource` (a desktop path → bytes) keeps calling `add_resource_bytes`.

`admin.rs`:
- `use super::CatalogTarget; use crate::store::CatalogRow;`
- `is_per_catalog` matches `Config | Load(_) | ListLayers | SetHostLayers(_)` plus every authoring call but `Configure`: `GetAsset(_) | Template(_) | CreateAsset(_) | UpdateAsset(_) | DeleteAsset(_) | AddResourceBytes(_) | RemoveResource(_) | LintAsset(_) | LintAll | CommitPending(_) | Push | RepoStatus | LayerTemplate(_) | WriteLayer(_) | DeleteLayer(_) | ImportHost(_)`; its doc: "The calls the tool's `catalog` parameter addresses (M3 R10; the authoring calls since M4, R21)."
- delete `fn is_authoring` and, in `touches`, the `Some(other) if c.is_authoring() => …` arm (the `Configure` arm above it stays).
- replace `run`'s signature and head:

```rust
/// Run one call against this process's catalogs and answer the same value
/// the matching desktop command returns, as JSON. `catalog` is the row the
/// tool's gate resolved for a named catalog (R22: read once, so the gate and
/// the run cannot disagree); `None` is personal. [`AdminCall::touches`]
/// refuses a parameter the call cannot honour before anything runs.
pub async fn run(
    call: AdminCall,
    catalog: Option<&CatalogRow>,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    reg: &Arc<CancellationRegistry>,
) -> Result<serde_json::Value, IpcError> {
    call.touches(catalog.map(|r| r.name.as_str()))?;
    // A per-catalog call naming a catalog other than personal (R10, R21).
    let named = catalog.filter(|r| r.org_id.is_some() && call.is_per_catalog());
    let target = match named {
        Some(row) => CatalogTarget::Row(row),
        None => CatalogTarget::Personal,
    };
```

- in the match, every `&named` becomes `named` (it is now `Option<&CatalogRow>`), and the authoring arms become:

```rust
        AdminCall::GetAsset(a) => {
            check_name(&a.name)?;
            json(super::get_asset_in(target, a.kind, &a.name, store)?)
        }
        AdminCall::WriteLayer(a) => json(author::write_layer_in(target, &a.layer, store)?),
        AdminCall::DeleteLayer(a) => json(author::delete_layer_in(target, &a.name, store)?),
        AdminCall::ImportHost(a) => {
            let token = lock(store)?.get_setting(crate::mcp::SETTING_TOKEN)?;
            json(super::import_host_into(target, a, store, ssh, token.as_deref()).await?)
        }
        AdminCall::CreateAsset(a) => json(author::create_in(target, a, store)?),
        AdminCall::UpdateAsset(a) => json(author::update_in(target, *a, store)?),
        AdminCall::DeleteAsset(a) => json(author::delete_asset_in(target, a, store)?),
        AdminCall::AddResourceBytes(a) => json(author::add_resource_bytes_in(target, a, store)?),
        AdminCall::RemoveResource(a) => json(author::remove_resource_in(target, a, store)?),
        AdminCall::LintAsset(a) => json(author::lint_asset_in(target, a, store)?),
        AdminCall::LintAll => json(author::lint_everything_in(target, store)?),
        AdminCall::CommitPending(a) => json(author::commit_pending_in(target, a, store)?),
        AdminCall::Push => json(author::push_in(target, store)?),
        AdminCall::RepoStatus => json(author::repo_status_in(target, store)?),
```

(`Template` and `LayerTemplate` stay as they are: they read nothing.) The `catalogs::catalog_named` lookup at the top of the old `run` is gone. Update the module doc's last paragraph: "… a paired client the operator granted the catalog each call touches (`AdminCall::touches`); the authoring calls address any catalog since Assets M4."

`mcp/tools/assets.rs` — in `catalog_admin`, between `let (mut call, touches) = parsed?;` and the `allowed` match, insert:

```rust
        // R22 (M-d): a named catalog's row is read once, here; the gate and
        // `run` both use it. The master naming an unknown catalog is told so
        // now — it must never fall through to personal; a client gets the
        // ungranted refusal below, as before.
        let target: Option<crate::store::CatalogRow> = match &touches {
            Touches::Catalog(name) if name != catalog::catalogs::PERSONAL => {
                let row = lookup_catalog(&self.store, name)?;
                if row.is_none() && caller.is_master() {
                    return Err(mcp_err(
                        codes::E_NOTFOUND,
                        format!("no catalog named {name}; list them with list_catalogs"),
                        None,
                    ));
                }
                row
            }
            _ => None,
        };
```

change the `Touches::Catalog(name)` arm of `allowed` to `Touches::Catalog(name) => may_admin_catalog_row(&caller, &self.store, name, target.as_ref())?,` and the `run` call to `catalog::admin::run(call, target.as_ref(), &self.store, &self.ssh, &self.reg)`. Replace `may_admin_catalog` with:

```rust
/// True when `caller` may touch the catalog `name` whose row the caller has
/// already read (`row`; `None` for personal or an unknown name) — spec:
/// `may_admin_catalog(caller, catalog_id)`. The master, or a live `full`
/// paired client, bound to no org, holding a grant on it (personal: the
/// assets grant, R2). Read live; a per-host token never may; an unknown name
/// is "no" for a client, so the refusal does not tell it which catalogs
/// exist.
fn may_admin_catalog_row(
    caller: &Caller,
    store: &std::sync::Mutex<Store>,
    name: &str,
    row: Option<&crate::store::CatalogRow>,
) -> Result<bool, McpError> {
    if caller.is_master() {
        return Ok(true);
    }
    let (None, Some(c)) = (&caller.host_alias, &caller.client) else {
        return Ok(false);
    };
    let s = store
        .lock()
        .map_err(|_| mcp_err(codes::E_LOCK, "store mutex poisoned", None))?;
    let ok = if name == catalog::catalogs::PERSONAL {
        s.client_is_assets_admin(c.id)
    } else {
        match row {
            Some(r) => s.client_may_admin_catalog(c.id, r.id),
            None => Ok(false),
        }
    };
    ok.map_err(|e| to_mcp_err(e.into()))
}

/// [`may_admin_catalog_row`] by name, for a caller that has not read the row.
fn may_admin_catalog(
    caller: &Caller,
    store: &std::sync::Mutex<Store>,
    name: &str,
) -> Result<bool, McpError> {
    if caller.is_master() || name == catalog::catalogs::PERSONAL {
        return may_admin_catalog_row(caller, store, name, None);
    }
    let row = lookup_catalog(store, name)?;
    may_admin_catalog_row(caller, store, name, row.as_ref())
}

/// The catalog row named `name`, if any.
fn lookup_catalog(
    store: &std::sync::Mutex<Store>,
    name: &str,
) -> Result<Option<crate::store::CatalogRow>, McpError> {
    let s = store
        .lock()
        .map_err(|_| mcp_err(codes::E_LOCK, "store mutex poisoned", None))?;
    s.get_catalog_by_name(name).map_err(|e| to_mcp_err(e.into()))
}
```

`mcp/tools/params.rs` — `CatalogAdminParams.catalog`'s doc becomes:

```rust
    /// Catalog name (default personal) for config|load|list_layers|
    /// set_host_layers and the authoring actions; configure is personal
    /// only. remove/admit/unadmit_catalog: only the name in args.
    /// add/remove_catalog: master only. Fleet-wide actions refuse it.
```

- [ ] **Step 4: Run the tests and regenerate**

Run, each on its own: `cargo test -p fleet-core authoring_writes_the_named_catalogs_checkout_only`; `cargo test -p fleet-core service::catalog::author`; `cargo test -p fleet-core service::catalog::admin`; `cargo test -p fleet-core tests_catalog_admin`; `cargo test -p claude-fleet --lib commands::assets` (the desktop's wrappers still compile and pass).
Expected: PASS.
Then: `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`; `cargo test -p fleet-core the_served_definition_budget_stays_bounded -- --nocapture` — if it fails, set `BUDGET_BYTES` to the printed measurement + 100 and re-run (expected: PASS).

- [ ] **Step 5: Commit** (N = the measurement the budget test printed)

```bash
git add crates/fleet-core/src/service/catalog/ crates/fleet-core/src/mcp/tools/ docs/control-api-reference.md
git commit -m "feat(catalog_admin): authoring per catalog; one catalog lookup per call (M-d)

Budget: measured N bytes after CatalogAdminParams.catalog's doc names the authoring actions."
```

---

### Task 4: The card model and the rules

**Files:**
- Create: `crates/fleet-core/src/service/catalog/changesets/mod.rs` (model)
- Create: `crates/fleet-core/src/service/catalog/changesets/rules.rs` (rules + tests)
- Modify: `crates/fleet-core/src/service/catalog/mod.rs` (`pub mod changesets;`)

**Interfaces:**
- Consumes: `identity::{AssetIdentity, IdentityClass, IdentityHost, group_identities}`, `import::slugify`, `model::sha256_hex`, Task 1's `NewChangesetItem`.
- Produces:

```rust
// changesets/mod.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)] #[serde(rename_all = "snake_case")]
pub enum CardKind { Bootstrap, New, Drift, Rollout }       // as_str, parse
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "snake_case")]
pub enum ItemAction { Import, AssignLayer, SetScope, Hide, TakeHost, Restore, Sync }  // as_str
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "snake_case")]
pub enum Decider { Rule, Jev, Haiku, Person }              // as_str
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemParams { from_host, layer, member, host, axis, scope, hash, reason: Option<String>, assets: Vec<String> } // all pub, serde(default)
impl ItemParams { pub fn parse(raw: Option<&str>) -> ItemParams; pub(crate) fn to_json(&self) -> Option<String>; }
pub struct ProposedItem { pub grp: String, pub catalog_id: Option<i64>, pub kind: String, pub name: String, pub action: ItemAction, pub params: ItemParams, pub decider: Decider }
impl ProposedItem { pub fn to_new(&self) -> NewChangesetItem; }
pub struct ProposedCard { pub kind: CardKind, pub summary: String, pub items: Vec<ProposedItem> }
impl ProposedCard { pub fn subject(&self) -> String; }
// changesets/rules.rs
pub const BOOTSTRAP_MIN: usize = 20; pub const PREFIX_FAMILY_MIN: usize = 3;
pub const NEEDS_A_LOOK: &str = "needs a look"; pub const HIDDEN: &str = "hidden"; pub const DRIFT: &str = "drift"; pub const UPDATE: &str = "update";
pub struct HostFacts { pub alias: String, pub org_id: Option<i64> }
pub struct LayerFacts { pub name: String, pub hosts: BTreeSet<String> }
pub struct CatalogFacts { pub id: i64, pub name: String, pub org_id: Option<i64>, pub loaded: bool, pub asset_count: usize, pub layers: Vec<LayerFacts> }
pub struct DriftFacts { pub catalog_id: i64, pub kind: String, pub name: String, pub host: String, pub host_hash: Option<String> }
pub struct LayerGap { pub catalog_id: i64, pub layer: String, pub host: String, pub assets: Vec<String> }
pub struct RulesInput<'a> { identities, hosts, catalogs, verdicts: &BTreeSet<(String, String, String)>, drifted, gaps, rollout_open: &BTreeSet<(i64, String)>, bootstrapped, bootstrap_open, auto }
pub struct SubjectItem<'a> { pub grp: &'a str, pub catalog_id: Option<i64>, pub kind: &'a str, pub name: &'a str, pub host: Option<&'a str> }
pub fn subject_of<'a>(kind: CardKind, items: impl IntoIterator<Item = SubjectItem<'a>>) -> String;
pub fn identity_hash(id: &AssetIdentity) -> String;
pub fn gap_hash(gaps: &[&LayerGap]) -> String;
pub fn propose(input: &RulesInput<'_>) -> Vec<ProposedCard>;
```

- [ ] **Step 1: Write the failing tests**

`changesets/rules.rs` — the tests module (the code above it follows in Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::catalog::identity::group_identities;
    use crate::store::AssetInventoryRow;

    const PERSONAL: i64 = 1;
    const PAPAYA: i64 = 2;
    const ORG: i64 = 7;

    /// Owns everything a `RulesInput` borrows.
    struct Facts {
        identities: Vec<AssetIdentity>,
        hosts: Vec<HostFacts>,
        catalogs: Vec<CatalogFacts>,
        verdicts: BTreeSet<(String, String, String)>,
        drifted: Vec<DriftFacts>,
        gaps: Vec<LayerGap>,
        rollout_open: BTreeSet<(i64, String)>,
        bootstrapped: bool,
        bootstrap_open: bool,
        auto: bool,
    }

    impl Facts {
        fn input(&self) -> RulesInput<'_> {
            RulesInput {
                identities: &self.identities,
                hosts: &self.hosts,
                catalogs: &self.catalogs,
                verdicts: &self.verdicts,
                drifted: &self.drifted,
                gaps: &self.gaps,
                rollout_open: &self.rollout_open,
                bootstrapped: self.bootstrapped,
                bootstrap_open: self.bootstrap_open,
                auto: self.auto,
            }
        }
    }

    fn row(host: &str, kind: &str, name: &str, hash: &str) -> AssetInventoryRow {
        AssetInventoryRow {
            host_alias: host.into(),
            harness: "claude".into(),
            kind: kind.into(),
            name: name.into(),
            state: "unmanaged".into(),
            host_hash: Some(hash.into()),
            scanned_at: 1,
            ..Default::default()
        }
    }

    /// The live fleet's hosts (trn bound to org 7), an empty personal
    /// catalog and, optionally, org 7's empty `papayapos` catalog.
    fn fleet(with_org_catalog: bool, rows: Vec<AssetInventoryRow>) -> Facts {
        let hosts = ["local", "mefistos", "oci", "trn", "htz"]
            .iter()
            .map(|h| HostFacts {
                alias: h.to_string(),
                org_id: (*h == "trn").then_some(ORG),
            })
            .collect();
        let mut catalogs = vec![CatalogFacts {
            id: PERSONAL,
            name: "personal".into(),
            org_id: None,
            loaded: true,
            asset_count: 0,
            layers: vec![],
        }];
        if with_org_catalog {
            catalogs.push(CatalogFacts {
                id: PAPAYA,
                name: "papayapos".into(),
                org_id: Some(ORG),
                loaded: true,
                asset_count: 0,
                layers: vec![],
            });
        }
        Facts {
            identities: group_identities(&rows),
            hosts,
            catalogs,
            verdicts: BTreeSet::new(),
            drifted: vec![],
            gaps: vec![],
            rollout_open: BTreeSet::new(),
            bootstrapped: false,
            bootstrap_open: false,
            auto: true,
        }
    }

    /// The live fleet's shape (2026-09-29, `identity.rs`): 164 identities in
    /// 8 host-set signatures. Synthetic names, real distribution.
    fn live_shape() -> Vec<AssetInventoryRow> {
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
                    rows.push(row(h, "skill", &format!("s{n}"), "same"));
                }
            }
        }
        rows
    }

    /// Import items per (catalog, group), "needs a look" left out.
    fn imports(card: &ProposedCard) -> BTreeMap<(Option<i64>, String), usize> {
        let mut out = BTreeMap::new();
        for i in card
            .items
            .iter()
            .filter(|i| i.action == ItemAction::Import && i.grp != NEEDS_A_LOOK)
        {
            *out.entry((i.catalog_id, i.grp.clone())).or_insert(0) += 1;
        }
        out
    }

    /// Spec, Testing: "bootstrap on the live fixture shape (164 identities,
    /// 8 signatures) produces the expected groups and the `shared`
    /// proposals for assets present on an org host".
    #[test]
    fn bootstrap_on_the_live_fixture_shape_groups_by_signature_and_shares_what_trn_has() {
        let f = fleet(true, live_shape());
        assert_eq!(f.identities.len(), 164);
        let cards = propose(&f.input());
        assert_eq!(cards.len(), 1, "{:?}", cards.iter().map(|c| &c.summary).collect::<Vec<_>>());
        let card = &cards[0];
        assert_eq!(card.kind, CardKind::Bootstrap);
        assert_eq!(card.summary, "Adopt 164 as 8 layers");
        let expected: BTreeMap<(Option<i64>, String), usize> = [
            (PERSONAL, "core", 82),
            (PERSONAL, "local-only", 30),
            (PERSONAL, "everywhere", 17),
            (PERSONAL, "local-oci-trn", 11),
            (PERSONAL, "local-mefistos", 9),
            (PERSONAL, "htz-oci", 5),
            (PERSONAL, "htz-only", 1),
            (PAPAYA, "trn-only", 9),
        ]
        .into_iter()
        .map(|(c, l, n)| ((Some(c), l.to_string()), n))
        .collect();
        assert_eq!(imports(card), expected);
        let shared: Vec<&ProposedItem> = card
            .items
            .iter()
            .filter(|i| i.action == ItemAction::SetScope)
            .collect();
        assert_eq!(shared.len(), 82 + 17 + 11, "every personal asset trn already has");
        assert!(shared
            .iter()
            .all(|i| i.catalog_id == Some(PERSONAL) && i.params.scope.as_deref() == Some("shared")));
        let assigned: BTreeSet<(String, String)> = card
            .items
            .iter()
            .filter(|i| i.action == ItemAction::AssignLayer)
            .map(|i| (i.name.clone(), i.params.host.clone().unwrap()))
            .collect();
        assert_eq!(assigned.len(), 4 + 1 + 5 + 3 + 2 + 2 + 1 + 1);
        assert!(assigned.contains(&("trn-only".to_string(), "trn".to_string())));
        assert!(card
            .items
            .iter()
            .filter(|i| i.action == ItemAction::AssignLayer)
            .all(|i| i.params.axis.as_deref() == Some("context")));
        for i in card.items.iter().filter(|i| i.action == ItemAction::Import) {
            assert!(i.params.from_host.is_some(), "{i:?}");
            assert_eq!(i.params.member.as_deref(), Some(format!("skill/{}", i.name).as_str()));
            assert_eq!(i.params.layer.as_deref(), Some(i.grp.as_str()));
        }
    }

    /// R5: with no catalog for trn's org, its assets go to personal — and,
    /// being on an org host, are proposed shared.
    #[test]
    fn without_an_org_catalog_trn_only_assets_go_to_personal_and_are_shared() {
        let f = fleet(false, live_shape());
        let card = &propose(&f.input())[0];
        assert_eq!(imports(card).get(&(Some(PERSONAL), "trn-only".to_string())), Some(&9));
        assert_eq!(
            card.items.iter().filter(|i| i.action == ItemAction::SetScope).count(),
            82 + 17 + 11 + 9
        );
    }

    /// R6, R8: a prefix family is its own layer; differing copies need a
    /// look; internals are `hide` items only with `catalog.auto` off.
    #[test]
    fn prefix_families_get_their_own_layer_and_odd_ones_need_a_look() {
        let mut rows = Vec::new();
        for n in ["author-draft", "author-beta", "author-critic", "jira", "worklog"] {
            for h in ["local", "oci"] {
                rows.push(row(h, "skill", n, "x"));
            }
        }
        rows.push(row("local", "skill", "copy", "a"));
        rows.push(row("oci", "skill", "copy", "a"));
        rows.push(row("mefistos", "skill", "copy", "b"));
        let mut fleet_hook = row("local", "hook", "stop", "f");
        fleet_hook.fleet_owned = true;
        rows.push(fleet_hook);
        let mut f = fleet(false, rows);
        f.auto = false;
        let card = &propose(&f.input())[0];
        let by_layer = imports(card);
        assert_eq!(by_layer.get(&(Some(PERSONAL), "author".to_string())), Some(&3));
        assert_eq!(by_layer.get(&(Some(PERSONAL), "core".to_string())), Some(&2));
        let look = card.items.iter().find(|i| i.grp == NEEDS_A_LOOK).unwrap();
        assert_eq!(
            (look.name.as_str(), look.params.reason.as_deref()),
            ("copy", Some("copies differ on mefistos"))
        );
        let hide = card.items.iter().find(|i| i.action == ItemAction::Hide).unwrap();
        assert_eq!((hide.name.as_str(), hide.grp.as_str()), ("stop", HIDDEN));
        assert!(card.summary.ends_with("; 1 need a look"), "{}", card.summary);
        f.auto = true;
        assert!(
            propose(&f.input())[0].items.iter().all(|i| i.action != ItemAction::Hide),
            "with catalog.auto the pass hides internals itself"
        );
    }

    /// Spec, New on host: after bootstrap a new identity joins the layer
    /// whose hosts it shares; else it needs a look.
    #[test]
    fn after_bootstrap_a_new_identity_joins_the_layer_sharing_its_hosts_or_needs_a_look() {
        let rows = vec![
            row("local", "skill", "fresh", "h"),
            row("oci", "skill", "fresh", "h"),
            row("htz", "skill", "lonely", "h"),
        ];
        let mut f = fleet(false, rows);
        f.bootstrapped = true;
        f.catalogs[0].asset_count = 10;
        f.catalogs[0].layers = vec![LayerFacts {
            name: "core".into(),
            hosts: BTreeSet::from(["local".to_string(), "oci".to_string()]),
        }];
        let cards = propose(&f.input());
        assert_eq!(cards.len(), 2);
        let fresh = cards.iter().find(|c| c.summary.contains("fresh")).unwrap();
        assert_eq!(fresh.kind, CardKind::New);
        assert_eq!(fresh.summary, "New on local, oci: skill/fresh → core");
        assert_eq!(fresh.items[0].params.layer.as_deref(), Some("core"));
        assert_eq!(fresh.subject(), "new:skill/fresh");
        let lonely = cards.iter().find(|c| c.summary.contains("lonely")).unwrap();
        assert_eq!(lonely.items[0].grp, NEEDS_A_LOOK);
        assert!(lonely.summary.ends_with("needs a look"));
    }

    /// Spec: "Rules never re-propose a subject with a matching verdict until
    /// its content hash changes."
    #[test]
    fn a_verdict_holds_a_subject_until_its_content_changes() {
        let mut f = fleet(false, vec![row("local", "skill", "w", "h1")]);
        f.bootstrapped = true;
        f.verdicts.insert(("skill".into(), "w".into(), "h1".into()));
        assert!(propose(&f.input()).is_empty());
        f.identities = group_identities(&[row("local", "skill", "w", "h2")]);
        assert_eq!(propose(&f.input()).len(), 1, "a new copy is a new subject");
    }

    /// Drift offers take or restore; a never-rolled-out layer's gaps become
    /// one Rollout card; an open rollout and a verdict hold each.
    #[test]
    fn drift_offers_take_or_restore_and_rollout_covers_a_new_layers_gaps() {
        let mut f = fleet(false, vec![]);
        f.bootstrapped = true;
        f.drifted = vec![DriftFacts {
            catalog_id: PERSONAL,
            kind: "skill".into(),
            name: "w".into(),
            host: "trn".into(),
            host_hash: Some("e".into()),
        }];
        f.gaps = ["oci", "htz"]
            .iter()
            .map(|h| LayerGap {
                catalog_id: PERSONAL,
                layer: "core".into(),
                host: h.to_string(),
                assets: vec!["skill/w".into()],
            })
            .collect();
        let cards = propose(&f.input());
        let drift = cards.iter().find(|c| c.kind == CardKind::Drift).unwrap();
        assert_eq!(
            drift.items.iter().map(|i| i.action).collect::<Vec<_>>(),
            [ItemAction::TakeHost, ItemAction::Restore]
        );
        assert_eq!(drift.subject(), format!("drift:{PERSONAL}:skill/w@trn"));
        let rollout = cards.iter().find(|c| c.kind == CardKind::Rollout).unwrap();
        assert_eq!(rollout.items.len(), 2);
        assert_eq!(rollout.subject(), format!("rollout:{PERSONAL}/core"));
        assert!(rollout.items.iter().all(|i| i.params.layer.as_deref() == Some("core")));
        f.rollout_open.insert((PERSONAL, "core".into()));
        f.verdicts.insert(("skill".into(), "w".into(), "e".into()));
        assert!(propose(&f.input()).is_empty(), "an open rollout and a verdict hold both");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p fleet-core changesets::rules` — Expected: compile error, module `changesets` not found.

- [ ] **Step 3: Write the implementation**

`service/catalog/mod.rs`: add `pub mod changesets;` (alphabetical, after `pub mod catalogs;`).

`changesets/mod.rs`:

```rust
//! Assets M4: changeset cards — proposed by rules (`rules`), built and
//! refreshed by the reconcile pass (`reconcile`), applied (`apply`) and
//! undone (`undo`). Spec: docs/superpowers/specs/2026-09-30-assets-s1b-s2-design.md,
//! *Changesets (the cards)*. The rows are `store::changesets`.

pub mod rules;

use crate::store::NewChangesetItem;
use serde::{Deserialize, Serialize};

/// What a card is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardKind {
    Bootstrap,
    New,
    Drift,
    Rollout,
}

impl CardKind {
    pub const ALL: [CardKind; 4] = [CardKind::Bootstrap, CardKind::New, CardKind::Drift, CardKind::Rollout];

    pub fn as_str(self) -> &'static str {
        match self {
            CardKind::Bootstrap => "bootstrap",
            CardKind::New => "new",
            CardKind::Drift => "drift",
            CardKind::Rollout => "rollout",
        }
    }

    pub fn parse(s: &str) -> Option<CardKind> {
        CardKind::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// What applying one item does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemAction {
    Import,
    AssignLayer,
    SetScope,
    Hide,
    TakeHost,
    Restore,
    Sync,
}

impl ItemAction {
    pub fn as_str(self) -> &'static str {
        match self {
            ItemAction::Import => "import",
            ItemAction::AssignLayer => "assign_layer",
            ItemAction::SetScope => "set_scope",
            ItemAction::Hide => "hide",
            ItemAction::TakeHost => "take_host",
            ItemAction::Restore => "restore",
            ItemAction::Sync => "sync",
        }
    }
}

/// Who decided an item or a verdict. M4 writes `Rule` and `Person` only;
/// `Jev` and `Haiku` are S5's (Rulings R10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decider {
    Rule,
    Jev,
    Haiku,
    Person,
}

impl Decider {
    pub fn as_str(self) -> &'static str {
        match self {
            Decider::Rule => "rule",
            Decider::Jev => "jev",
            Decider::Haiku => "haiku",
            Decider::Person => "person",
        }
    }
}

/// An item's `params` JSON (Rulings R8). Every field optional; each action
/// reads the ones it needs: import `from_host`, `layer`, `member`, `hash`,
/// `reason`; assign_layer `host`, `layer`, `axis`; set_scope `scope`,
/// `member`; hide `hash`, `reason`; take_host/restore `host`, `hash`; sync
/// `layer`, `assets`, `hash`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
    /// `<kind>/<catalog name>` — the layer member key the import becomes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub axis: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// The subject's content hash a verdict on this item records (R9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<String>,
}

impl ItemParams {
    /// A stored `params`; anything unreadable is "no params".
    pub fn parse(raw: Option<&str>) -> ItemParams {
        raw.and_then(|r| serde_json::from_str(r).ok()).unwrap_or_default()
    }

    /// The stored form: `None` when every field is empty.
    pub(crate) fn to_json(&self) -> Option<String> {
        if *self == ItemParams::default() {
            None
        } else {
            serde_json::to_string(self).ok()
        }
    }
}

/// One item a rule proposes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedItem {
    pub grp: String,
    pub catalog_id: Option<i64>,
    pub kind: String,
    pub name: String,
    pub action: ItemAction,
    pub params: ItemParams,
    pub decider: Decider,
}

impl ProposedItem {
    pub fn to_new(&self) -> NewChangesetItem {
        NewChangesetItem {
            grp: self.grp.clone(),
            catalog_id: self.catalog_id,
            kind: self.kind.clone(),
            name: self.name.clone(),
            action: self.action.as_str().to_string(),
            params: self.params.to_json(),
            decider: self.decider.as_str().to_string(),
        }
    }
}

/// One card a rule proposes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedCard {
    pub kind: CardKind,
    pub summary: String,
    pub items: Vec<ProposedItem>,
}

impl ProposedCard {
    /// Its subject (Rulings R2).
    pub fn subject(&self) -> String {
        rules::subject_of(
            self.kind,
            self.items.iter().map(|i| rules::SubjectItem {
                grp: &i.grp,
                catalog_id: i.catalog_id,
                kind: &i.kind,
                name: &i.name,
                host: i.params.host.as_deref(),
            }),
        )
    }
}
```

`changesets/rules.rs` (above its tests module):

```rust
//! Assets M4: the rules that turn the last scan into proposed cards (spec,
//! *Changesets (the cards)*). Pure — no store, registry or I/O: `reconcile`
//! gathers the facts into a [`RulesInput`].

use super::{CardKind, Decider, ItemAction, ItemParams, ProposedCard, ProposedItem};
use crate::service::catalog::identity::{AssetIdentity, IdentityClass, IdentityHost};
use crate::service::catalog::import::slugify;
use crate::service::catalog::model::sha256_hex;
use std::collections::{BTreeMap, BTreeSet};

/// Spec: a Bootstrap card once "≥ 20 unmanaged `normal` identities exist".
pub const BOOTSTRAP_MIN: usize = 20;
/// R6: how many members sharing a name prefix make their own layer.
pub const PREFIX_FAMILY_MIN: usize = 3;
pub const NEEDS_A_LOOK: &str = "needs a look";
pub const HIDDEN: &str = "hidden";
pub const DRIFT: &str = "drift";
/// The group of a take_host's follow-up Rollout items (R14).
pub const UPDATE: &str = "update";

const NO_CLAUDE_COPY: &str = "found only outside Claude; import reads Claude config";
const NO_LAYER: &str = "no layer shares its hosts or name prefix";

/// A non-hidden host and its org.
#[derive(Debug, Clone)]
pub struct HostFacts {
    pub alias: String,
    pub org_id: Option<i64>,
}

/// One layer of a catalog and the hosts it is (actively) assigned to.
#[derive(Debug, Clone)]
pub struct LayerFacts {
    pub name: String,
    pub hosts: BTreeSet<String>,
}

/// A configured catalog as the rules see it.
#[derive(Debug, Clone)]
pub struct CatalogFacts {
    pub id: i64,
    pub name: String,
    pub org_id: Option<i64>,
    /// In the registry and not a problem entry.
    pub loaded: bool,
    pub asset_count: usize,
    pub layers: Vec<LayerFacts>,
}

/// A managed asset whose copy on a host differs from its catalog's.
#[derive(Debug, Clone)]
pub struct DriftFacts {
    pub catalog_id: i64,
    pub kind: String,
    pub name: String,
    pub host: String,
    pub host_hash: Option<String>,
}

/// Members a never-rolled-out layer should have put on a host, and has not
/// (R16).
#[derive(Debug, Clone)]
pub struct LayerGap {
    pub catalog_id: i64,
    pub layer: String,
    pub host: String,
    pub assets: Vec<String>,
}

/// Everything the rules read.
pub struct RulesInput<'a> {
    pub identities: &'a [AssetIdentity],
    pub hosts: &'a [HostFacts],
    pub catalogs: &'a [CatalogFacts],
    /// `(kind, name, content_hash)` of every verdict.
    pub verdicts: &'a BTreeSet<(String, String, String)>,
    pub drifted: &'a [DriftFacts],
    pub gaps: &'a [LayerGap],
    /// `(catalog_id, layer)` an open rollout card already names.
    pub rollout_open: &'a BTreeSet<(i64, String)>,
    /// R4: an applied Bootstrap exists, or personal already has assets.
    pub bootstrapped: bool,
    pub bootstrap_open: bool,
    /// `catalog.auto`: the pass hides internals itself, so no `hide` items.
    pub auto: bool,
}

/// One item's share of a card's subject (R2).
pub struct SubjectItem<'a> {
    pub grp: &'a str,
    pub catalog_id: Option<i64>,
    pub kind: &'a str,
    pub name: &'a str,
    pub host: Option<&'a str>,
}

/// A card's subject (R2): what the pass matches an open card by.
pub fn subject_of<'a>(kind: CardKind, items: impl IntoIterator<Item = SubjectItem<'a>>) -> String {
    let mut items = items.into_iter();
    match kind {
        CardKind::Bootstrap => "bootstrap".to_string(),
        CardKind::New => match items.next() {
            Some(i) => format!("new:{}/{}", i.kind, i.name),
            None => "new:".to_string(),
        },
        CardKind::Drift => match items.next() {
            Some(i) => format!(
                "drift:{}:{}/{}@{}",
                i.catalog_id.unwrap_or(0),
                i.kind,
                i.name,
                i.host.unwrap_or("")
            ),
            None => "drift:".to_string(),
        },
        CardKind::Rollout => {
            let groups: BTreeSet<String> = items
                .map(|i| format!("{}/{}", i.catalog_id.unwrap_or(0), i.grp))
                .collect();
            format!("rollout:{}", groups.into_iter().collect::<Vec<_>>().join(","))
        }
    }
}

/// The content hash a verdict on this identity holds by (R9).
pub fn identity_hash(id: &AssetIdentity) -> String {
    let hashes: BTreeSet<&str> = id.hosts.iter().filter_map(|h| h.host_hash.as_deref()).collect();
    match hashes.len() {
        0 => "-".to_string(),
        1 => hashes.into_iter().next().unwrap_or("-").to_string(),
        _ => sha256_hex(hashes.into_iter().collect::<Vec<_>>().join("\n").as_bytes()),
    }
}

/// The hash a Rollout card's verdict holds by (R9).
pub fn gap_hash(gaps: &[&LayerGap]) -> String {
    let lines: BTreeSet<String> = gaps
        .iter()
        .flat_map(|g| g.assets.iter().map(move |a| format!("{}:{a}", g.host)))
        .collect();
    sha256_hex(lines.into_iter().collect::<Vec<_>>().join("\n").as_bytes())
}

/// Every card the facts call for.
pub fn propose(input: &RulesInput<'_>) -> Vec<ProposedCard> {
    let held = |kind: &str, name: &str, hash: &str| {
        input
            .verdicts
            .contains(&(kind.to_string(), name.to_string(), hash.to_string()))
    };
    let eligible: Vec<&AssetIdentity> = input
        .identities
        .iter()
        .filter(|i| !held(&i.kind, &i.name, &identity_hash(i)))
        .collect();
    let normal = eligible
        .iter()
        .filter(|i| i.class == IdentityClass::Normal)
        .count();
    let mut cards = Vec::new();
    if input.bootstrap_open || normal >= BOOTSTRAP_MIN || (!input.bootstrapped && normal > 0) {
        cards.extend(bootstrap_card(&eligible, input));
    } else {
        for id in &eligible {
            if is_internal(id) && input.auto {
                continue;
            }
            cards.push(new_card(id, input));
        }
    }
    cards.extend(drift_cards(input, &held));
    cards.extend(rollout_cards(input, &held));
    cards
}

fn is_internal(id: &AssetIdentity) -> bool {
    matches!(id.class, IdentityClass::FleetInternal | IdentityClass::HarnessInternal)
}

fn class_label(c: &IdentityClass) -> &'static str {
    match c {
        IdentityClass::Normal => "normal",
        IdentityClass::FleetInternal => "fleet_internal",
        IdentityClass::HarnessInternal => "harness_internal",
        IdentityClass::NeedsPerson => "needs_person",
    }
}

fn hosts_of(id: &AssetIdentity) -> BTreeSet<String> {
    id.hosts.iter().map(|h| h.host_alias.clone()).collect()
}

fn org_of(input: &RulesInput<'_>, alias: &str) -> Option<i64> {
    input.hosts.iter().find(|h| h.alias == alias).and_then(|h| h.org_id)
}

fn on_org_host(id: &AssetIdentity, input: &RulesInput<'_>) -> bool {
    id.hosts.iter().any(|h| org_of(input, &h.host_alias).is_some())
}

fn catalog<'a>(input: &'a RulesInput<'_>, id: i64) -> Option<&'a CatalogFacts> {
    input.catalogs.iter().find(|c| c.id == id)
}

fn personal_id(input: &RulesInput<'_>) -> Option<i64> {
    input.catalogs.iter().find(|c| c.org_id.is_none()).map(|c| c.id)
}

fn member_of(id: &AssetIdentity) -> String {
    format!("{}/{}", id.kind, slugify(&id.name))
}

/// R6: the slug's part before its first `-`, when there is a rest.
fn prefix_of(name: &str) -> Option<String> {
    let slug = slugify(name);
    let (p, rest) = slug.split_once('-')?;
    (!p.is_empty() && !rest.is_empty()).then(|| p.to_string())
}

/// R5: org X's catalog when every holder is bound to X and X has one, else
/// personal; `Err(why)` when it needs a look.
fn destination(id: &AssetIdentity, input: &RulesInput<'_>) -> Result<i64, String> {
    let orgs: Vec<Option<i64>> = id
        .hosts
        .iter()
        .map(|h| org_of(input, &h.host_alias))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if let [Some(org)] = orgs.as_slice() {
        if let Some(c) = input.catalogs.iter().find(|c| c.org_id == Some(*org)) {
            return if c.loaded {
                Ok(c.id)
            } else {
                Err(format!("catalog {} failed to load", c.name))
            };
        }
    }
    personal_id(input).ok_or_else(|| "no personal catalog".to_string())
}

/// R5: the Claude copy most hosts hold (tie: the smaller hash), `local`
/// first among its holders, then alphabetical.
fn source_host(id: &AssetIdentity) -> Option<String> {
    let claude: Vec<&IdentityHost> = id.hosts.iter().filter(|h| h.harness == "claude").collect();
    let mut counts: BTreeMap<Option<&str>, usize> = BTreeMap::new();
    for h in &claude {
        *counts.entry(h.host_hash.as_deref()).or_default() += 1;
    }
    let top = counts.values().copied().max()?;
    let common = counts.iter().find(|(_, n)| **n == top).map(|(h, _)| *h)?;
    let mut holders: Vec<&str> = claude
        .iter()
        .filter(|h| h.host_hash.as_deref() == common)
        .map(|h| h.host_alias.as_str())
        .collect();
    holders.sort_by_key(|a| (*a != "local", *a));
    holders.first().map(|s| s.to_string())
}

fn import_item(
    id: &AssetIdentity,
    grp: &str,
    catalog_id: Option<i64>,
    from_host: Option<String>,
    layer: Option<&str>,
    reason: Option<String>,
) -> ProposedItem {
    ProposedItem {
        grp: grp.to_string(),
        catalog_id,
        kind: id.kind.clone(),
        name: id.name.clone(),
        action: ItemAction::Import,
        params: ItemParams {
            from_host,
            layer: layer.map(String::from),
            member: Some(member_of(id)),
            hash: Some(identity_hash(id)),
            reason,
            ..Default::default()
        },
        decider: Decider::Rule,
    }
}

fn share_item(id: &AssetIdentity, grp: &str, catalog_id: i64) -> ProposedItem {
    ProposedItem {
        grp: grp.to_string(),
        catalog_id: Some(catalog_id),
        kind: id.kind.clone(),
        name: id.name.clone(),
        action: ItemAction::SetScope,
        params: ItemParams {
            scope: Some("shared".into()),
            member: Some(member_of(id)),
            hash: Some(identity_hash(id)),
            ..Default::default()
        },
        decider: Decider::Rule,
    }
}

fn assign_item(layer: &str, catalog_id: i64, host: &str) -> ProposedItem {
    ProposedItem {
        grp: layer.to_string(),
        catalog_id: Some(catalog_id),
        kind: "layer".into(),
        name: layer.to_string(),
        action: ItemAction::AssignLayer,
        params: ItemParams {
            host: Some(host.to_string()),
            layer: Some(layer.to_string()),
            axis: Some("context".into()),
            ..Default::default()
        },
        decider: Decider::Rule,
    }
}

fn hide_item(id: &AssetIdentity) -> ProposedItem {
    ProposedItem {
        grp: HIDDEN.into(),
        catalog_id: None,
        kind: id.kind.clone(),
        name: id.name.clone(),
        action: ItemAction::Hide,
        params: ItemParams {
            hash: Some(identity_hash(id)),
            reason: Some(class_label(&id.class).into()),
            ..Default::default()
        },
        decider: Decider::Rule,
    }
}

/// An import a person must name to apply (R8), with why.
fn look_item(id: &AssetIdentity, input: &RulesInput<'_>) -> ProposedItem {
    let dest = destination(id, input);
    let reason = if id.class == IdentityClass::NeedsPerson {
        id.reason.clone().unwrap_or_else(|| "needs a person".into())
    } else {
        match &dest {
            Err(e) => e.clone(),
            Ok(_) if source_host(id).is_none() => NO_CLAUDE_COPY.to_string(),
            Ok(_) => NO_LAYER.to_string(),
        }
    };
    import_item(
        id,
        NEEDS_A_LOOK,
        dest.ok().or_else(|| personal_id(input)),
        source_host(id),
        None,
        Some(reason),
    )
}

/// One proposed layer.
struct Group<'a> {
    catalog_id: i64,
    hosts: BTreeSet<String>,
    layer: String,
    members: Vec<(&'a AssetIdentity, String)>,
}

fn bootstrap_card(eligible: &[&AssetIdentity], input: &RulesInput<'_>) -> Option<ProposedCard> {
    let mut looks = Vec::new();
    let mut hides = Vec::new();
    let mut by_sig: BTreeMap<(i64, BTreeSet<String>), Vec<(&AssetIdentity, String)>> = BTreeMap::new();
    for &id in eligible {
        if is_internal(id) {
            if !input.auto {
                hides.push(hide_item(id));
            }
            continue;
        }
        match (id.class == IdentityClass::Normal, destination(id, input), source_host(id)) {
            (true, Ok(dest), Some(src)) => by_sig.entry((dest, hosts_of(id))).or_default().push((id, src)),
            _ => looks.push(look_item(id, input)),
        }
    }
    let groups = name_groups(by_sig, input);
    let mut items = Vec::new();
    let mut imports = 0;
    for g in &groups {
        let personal_dest = catalog(input, g.catalog_id).is_some_and(|c| c.org_id.is_none());
        for (id, src) in &g.members {
            items.push(import_item(id, &g.layer, Some(g.catalog_id), Some(src.clone()), Some(&g.layer), None));
            imports += 1;
            if personal_dest && on_org_host(id, input) {
                items.push(share_item(id, &g.layer, g.catalog_id));
            }
        }
        // R7: only where layering cannot shrink what the host takes today.
        let cat = catalog(input, g.catalog_id);
        for host in &g.hosts {
            let safe = cat.is_some_and(|c| {
                c.asset_count == 0 || c.layers.iter().any(|l| l.hosts.contains(host))
            });
            if safe {
                items.push(assign_item(&g.layer, g.catalog_id, host));
            }
        }
    }
    let looks_n = looks.len();
    items.extend(looks);
    items.extend(hides);
    if items.is_empty() {
        return None;
    }
    let mut summary = format!("Adopt {imports} as {} layers", groups.len());
    if looks_n > 0 {
        summary.push_str(&format!("; {looks_n} need a look"));
    }
    Some(ProposedCard {
        kind: CardKind::Bootstrap,
        summary,
        items,
    })
}

/// R6: split prefix families out of each signature group, then name every
/// group, largest first.
fn name_groups<'a>(
    by_sig: BTreeMap<(i64, BTreeSet<String>), Vec<(&'a AssetIdentity, String)>>,
    input: &RulesInput<'_>,
) -> Vec<Group<'a>> {
    struct Raw<'a> {
        catalog_id: i64,
        hosts: BTreeSet<String>,
        family: Option<String>,
        members: Vec<(&'a AssetIdentity, String)>,
    }
    let mut raws: Vec<Raw<'a>> = Vec::new();
    for ((catalog_id, hosts), members) in by_sig {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for (id, _) in &members {
            if let Some(p) = prefix_of(&id.name) {
                *counts.entry(p).or_default() += 1;
            }
        }
        let mut families: BTreeMap<String, Vec<(&'a AssetIdentity, String)>> = BTreeMap::new();
        let mut rest = Vec::new();
        for m in members {
            match prefix_of(&m.0.name) {
                Some(p) if counts.get(&p).copied().unwrap_or(0) >= PREFIX_FAMILY_MIN => {
                    families.entry(p).or_default().push(m)
                }
                _ => rest.push(m),
            }
        }
        for (p, ms) in families {
            raws.push(Raw {
                catalog_id,
                hosts: hosts.clone(),
                family: Some(p),
                members: ms,
            });
        }
        if !rest.is_empty() {
            raws.push(Raw {
                catalog_id,
                hosts,
                family: None,
                members: rest,
            });
        }
    }
    raws.sort_by(|a, b| {
        b.members
            .len()
            .cmp(&a.members.len())
            .then_with(|| a.catalog_id.cmp(&b.catalog_id))
            .then_with(|| a.hosts.cmp(&b.hosts))
            .then_with(|| a.family.cmp(&b.family))
    });
    let mut used: BTreeMap<i64, BTreeSet<String>> = input
        .catalogs
        .iter()
        .map(|c| (c.id, c.layers.iter().map(|l| l.name.clone()).collect()))
        .collect();
    let mut core_given: BTreeSet<i64> = BTreeSet::new();
    let mut out = Vec::new();
    for r in raws {
        let base = if let Some(p) = &r.family {
            p.clone()
        } else if r.hosts.len() == 1 {
            format!("{}-only", slugify(r.hosts.iter().next().map(String::as_str).unwrap_or("host")))
        } else if r.hosts == accepting_hosts(input, r.catalog_id) {
            "everywhere".to_string()
        } else if core_given.insert(r.catalog_id) {
            "core".to_string()
        } else {
            slugify(&r.hosts.iter().cloned().collect::<Vec<_>>().join("-"))
        };
        let taken = used.entry(r.catalog_id).or_default();
        let layer = free_name(&base, taken);
        taken.insert(layer.clone());
        out.push(Group {
            catalog_id: r.catalog_id,
            hosts: r.hosts,
            layer,
            members: r.members,
        });
    }
    out
}

/// Every host that accepts the catalog: all of them for personal, the
/// org's for an org catalog.
fn accepting_hosts(input: &RulesInput<'_>, catalog_id: i64) -> BTreeSet<String> {
    match catalog(input, catalog_id).and_then(|c| c.org_id) {
        None => input.hosts.iter().map(|h| h.alias.clone()).collect(),
        Some(org) => input
            .hosts
            .iter()
            .filter(|h| h.org_id == Some(org))
            .map(|h| h.alias.clone())
            .collect(),
    }
}

fn free_name(base: &str, taken: &BTreeSet<String>) -> String {
    if !taken.contains(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|n| !taken.contains(n))
        .expect("an unbounded range finds a free name")
}

/// New on host: the layer whose assigned hosts are exactly the identity's,
/// else one named after its prefix family covering its hosts.
fn layer_for(id: &AssetIdentity, dest: i64, input: &RulesInput<'_>) -> Option<String> {
    let cat = catalog(input, dest)?;
    let hosts = hosts_of(id);
    if let Some(l) = cat.layers.iter().find(|l| l.hosts == hosts) {
        return Some(l.name.clone());
    }
    let prefix = prefix_of(&id.name)?;
    cat.layers
        .iter()
        .find(|l| l.name == prefix && l.hosts.is_superset(&hosts))
        .map(|l| l.name.clone())
}

fn new_card(id: &AssetIdentity, input: &RulesInput<'_>) -> ProposedCard {
    let on = id.signature.replace(',', ", ");
    if is_internal(id) {
        return ProposedCard {
            kind: CardKind::New,
            summary: format!("Hide {}/{} on {on}", id.kind, id.name),
            items: vec![hide_item(id)],
        };
    }
    if id.class == IdentityClass::Normal {
        if let (Ok(dest), Some(src)) = (destination(id, input), source_host(id)) {
            if let Some(layer) = layer_for(id, dest, input) {
                let mut items = vec![import_item(id, &layer, Some(dest), Some(src), Some(&layer), None)];
                if catalog(input, dest).is_some_and(|c| c.org_id.is_none()) && on_org_host(id, input) {
                    items.push(share_item(id, &layer, dest));
                }
                return ProposedCard {
                    kind: CardKind::New,
                    summary: format!("New on {on}: {}/{} → {layer}", id.kind, id.name),
                    items,
                };
            }
        }
    }
    ProposedCard {
        kind: CardKind::New,
        summary: format!("New on {on}: {}/{} needs a look", id.kind, id.name),
        items: vec![look_item(id, input)],
    }
}

fn drift_cards(input: &RulesInput<'_>, held: &dyn Fn(&str, &str, &str) -> bool) -> Vec<ProposedCard> {
    input
        .drifted
        .iter()
        .filter(|d| !held(&d.kind, &d.name, d.host_hash.as_deref().unwrap_or("-")))
        .map(|d| {
            let cat = catalog(input, d.catalog_id).map_or("?", |c| c.name.as_str());
            let params = ItemParams {
                host: Some(d.host.clone()),
                hash: Some(d.host_hash.clone().unwrap_or_else(|| "-".into())),
                ..Default::default()
            };
            let item = |action| ProposedItem {
                grp: DRIFT.into(),
                catalog_id: Some(d.catalog_id),
                kind: d.kind.clone(),
                name: d.name.clone(),
                action,
                params: params.clone(),
                decider: Decider::Rule,
            };
            ProposedCard {
                kind: CardKind::Drift,
                summary: format!("{}/{} differs on {} from catalog {cat}", d.kind, d.name, d.host),
                items: vec![item(ItemAction::TakeHost), item(ItemAction::Restore)],
            }
        })
        .collect()
}

fn rollout_cards(input: &RulesInput<'_>, held: &dyn Fn(&str, &str, &str) -> bool) -> Vec<ProposedCard> {
    let mut by_layer: BTreeMap<(i64, String), Vec<&LayerGap>> = BTreeMap::new();
    for g in input.gaps {
        if !input.rollout_open.contains(&(g.catalog_id, g.layer.clone())) {
            by_layer.entry((g.catalog_id, g.layer.clone())).or_default().push(g);
        }
    }
    by_layer
        .into_iter()
        .filter_map(|((cid, layer), gaps)| {
            let hash = gap_hash(&gaps);
            if held("layer", &format!("{cid}/{layer}"), &hash) {
                return None;
            }
            let hosts: Vec<&str> = gaps.iter().map(|g| g.host.as_str()).collect();
            let items = gaps
                .iter()
                .map(|g| ProposedItem {
                    grp: layer.clone(),
                    catalog_id: Some(cid),
                    kind: "host".into(),
                    name: g.host.clone(),
                    action: ItemAction::Sync,
                    params: ItemParams {
                        layer: Some(layer.clone()),
                        assets: g.assets.clone(),
                        hash: Some(hash.clone()),
                        ..Default::default()
                    },
                    decider: Decider::Rule,
                })
                .collect();
            Some(ProposedCard {
                kind: CardKind::Rollout,
                summary: format!("Roll out {layer} to {}", hosts.join(", ")),
                items,
            })
        })
        .collect()
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p fleet-core changesets::rules` — Expected: PASS (7 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/service/catalog/mod.rs crates/fleet-core/src/service/catalog/changesets/
git commit -m "feat(changesets): card model and rules (bootstrap, new, drift, rollout)"
```

---

### Task 5: The reconcile pass, the settings, and the scan-tick hook

**Files:**
- Modify: `crates/fleet-core/src/service/catalog/changesets/mod.rs` (views, `APPLY_LOCK`, `card`/`get`/`list`/`propose`, undoability helpers)
- Create: `crates/fleet-core/src/service/catalog/changesets/reconcile.rs` (+ tests)
- Modify: `crates/fleet-core/src/service/catalog/scan_tick.rs` (the hook)
- Modify: `crates/fleet-core/src/service/settings.rs` (`CATALOG_AUTO`, `CATALOG_AUTO_PUSH`)
- Modify: `crates/fleet-core/pages/settings.automation.json` (two fields in *Assets*)
- Modify: `src/lib/fleet_settings.ts` (`SETTING_KEYS`, `SETTING_DEFAULTS`)
- Modify (generated): `docs/settings-reference.md` and the other `REGEN_SETTINGS_DOCS` / `REGEN_PAGE_DOCS` outputs

**Interfaces:**
- Consumes: Task 1's store API; Task 4's `rules::*`, `ProposedCard`, `ItemParams`, `CardKind`, `Decider`; `identity::group_identities`; `registry::{snapshot, entry_for}`; `effective::effective_for_host_in`; `settings::get_bool`.
- Produces:

```rust
// changesets/mod.rs
pub(crate) static APPLY_LOCK: tokio::sync::Mutex<()>;
pub const RECENT_CLOSED: usize = 20;
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct ItemView { pub position: i64, pub grp: String, pub catalog: Option<String>, pub kind: String, pub name: String, pub action: String, pub params: ItemParams, pub decider: String, pub state: String }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct ChangesetView { pub id: i64, pub kind: String, pub summary: String, pub state: String, pub created_at: i64, pub applied_at: Option<i64>, pub error: Option<String>, pub commits: BTreeMap<String, String>, pub undoable: bool, pub items: Vec<ItemView> }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct ChangesetSummary { pub id: i64, pub kind: String, pub summary: String, pub state: String, pub created_at: i64, pub applied_at: Option<i64>, pub error: Option<String>, pub groups: BTreeMap<String, usize>, pub pending: usize, pub undoable: bool }
pub fn is_open(state: &str) -> bool;
pub fn card(id: i64, store: &Mutex<Store>) -> Result<(ChangesetRow, Vec<ChangesetItemRow>), IpcError>;
pub fn get(id: i64, store: &Mutex<Store>) -> Result<ChangesetView, IpcError>;
pub fn list(store: &Mutex<Store>) -> Result<Vec<ChangesetSummary>, IpcError>;
pub async fn propose(store: &Mutex<Store>) -> Result<Vec<ChangesetSummary>, IpcError>;
pub fn writes_hosts(card: &ChangesetRow, items: &[ChangesetItemRow], positions: Option<&[i64]>) -> bool;
pub(crate) fn changes_catalog(card: &ChangesetRow, items: &[ChangesetItemRow]) -> bool;
pub(crate) fn applied_catalogs(items: &[ChangesetItemRow]) -> BTreeSet<i64>;
pub(crate) fn later_card(card: &ChangesetRow, items: &[ChangesetItemRow], s: &Store) -> Result<Option<(i64, String)>, IpcError>;
// changesets/reconcile.rs
#[derive(Debug, Clone, Default, PartialEq, Eq)] pub struct ReconcileReport { pub inserted: usize, pub refreshed: usize, pub withdrawn: usize, pub hidden: usize }
pub const WITHDRAWN: &str = "withdrawn: no longer applies";
pub fn reconcile(store: &Mutex<Store>, auto: bool) -> Result<ReconcileReport, IpcError>;
pub fn after_scan_pass(store: &Arc<Mutex<Store>>);   // Task 7 adds `ssh: &Arc<SshClient>`
// service/settings.rs
pub const CATALOG_AUTO: &str = "catalog.auto";
pub const CATALOG_AUTO_PUSH: &str = "catalog.auto_push";
```

Every test that can reach `APPLY_LOCK` (this task's, and Tasks 6–9's) takes `lock_registry_for_test()` first: that serialises them, so no test sees another's apply holding the lock.

- [ ] **Step 1: Write the failing tests**

`changesets/reconcile.rs` — the tests module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::catalog::layer::{Axis, LayerSet};
    use crate::service::catalog::model::{Asset, Kind};
    use crate::service::catalog::repo::Catalog;
    use crate::service::catalog::{author, lock_registry_for_test, registry};

    fn skill(name: &str) -> Asset {
        Asset::from_yaml(
            Some(Kind::Skill),
            &format!("kind: skill\nname: {name}\ndescription: d\n"),
        )
        .unwrap()
    }

    /// `personal` configured and installed holding `assets`; hosts `oci`
    /// and `trn` (org 7). Returns `(store, personal_id)`.
    fn fleet_store(assets: Vec<Asset>) -> (Arc<Mutex<Store>>, i64) {
        let s = Store::open_in_memory().unwrap();
        s.set_catalog_config("/p", None).unwrap();
        for h in ["oci", "trn"] {
            s.upsert_host(h).unwrap();
        }
        s.conn_ref()
            .execute("INSERT INTO orgs (id, name, created_at) VALUES (7, 'papayapos', 0)", [])
            .unwrap();
        s.set_host_org("trn", Some(7)).unwrap();
        let p = s.personal_catalog().unwrap().unwrap().id;
        registry::install_personal(Catalog {
            id: p,
            name: "personal".into(),
            assets,
            ..Default::default()
        })
        .unwrap();
        (Arc::new(Mutex::new(s)), p)
    }

    fn unmanaged(host: &str, name: &str) -> AssetInventoryRow {
        AssetInventoryRow {
            host_alias: host.into(),
            harness: "claude".into(),
            kind: "skill".into(),
            name: name.into(),
            state: "unmanaged".into(),
            host_hash: Some(format!("h-{name}")),
            scanned_at: 1,
            ..Default::default()
        }
    }

    fn fleet_hook(host: &str) -> AssetInventoryRow {
        let mut r = unmanaged(host, "stop");
        r.kind = "hook".into();
        r.fleet_owned = true;
        r
    }

    fn put(store: &Mutex<Store>, host: &str, rows: Vec<AssetInventoryRow>) {
        store
            .lock()
            .unwrap()
            .replace_host_inventory(host, "claude", &rows)
            .unwrap();
    }

    #[test]
    fn the_pass_proposes_a_bootstrap_hides_internals_and_refreshes_in_place() {
        let _g = lock_registry_for_test();
        let (store, _) = fleet_store(vec![]);
        let mut rows: Vec<_> = (0..25).map(|n| unmanaged("oci", &format!("s{n}"))).collect();
        rows.push(fleet_hook("oci"));
        put(&store, "oci", rows.clone());

        let r = reconcile(&store, true).unwrap();
        assert_eq!((r.inserted, r.hidden), (1, 1));
        let cards = store.lock().unwrap().list_changesets().unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].kind, "bootstrap");
        let verdicts = store.lock().unwrap().triage_verdicts().unwrap();
        assert_eq!(
            (verdicts[0].name.as_str(), verdicts[0].verdict.as_str(), verdicts[0].decider.as_str()),
            ("stop", "ignored", "rule")
        );

        let again = reconcile(&store, true).unwrap();
        assert_eq!(again, ReconcileReport::default(), "nothing changed, nothing written");

        rows.push(unmanaged("oci", "s99"));
        put(&store, "oci", rows);
        let more = reconcile(&store, true).unwrap();
        assert_eq!((more.inserted, more.refreshed), (0, 1));
        let after = store.lock().unwrap().list_changesets().unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].id, cards[0].id, "the same card, refreshed in place");
    }

    /// R2: an open card whose subject the rules no longer produce is
    /// withdrawn; a rollout card never is.
    #[test]
    fn a_card_whose_subject_disappears_is_withdrawn_but_a_rollout_stays() {
        let _g = lock_registry_for_test();
        let (store, p) = fleet_store(vec![skill("kept")]);
        put(&store, "oci", vec![unmanaged("oci", "w")]);
        reconcile(&store, true).unwrap();
        let card = store.lock().unwrap().list_changesets().unwrap()[0].clone();
        assert_eq!(card.kind, "new", "personal is not empty: bootstrapped");
        let rollout = store
            .lock()
            .unwrap()
            .insert_changeset(
                "rollout",
                "Roll out core to oci",
                &[NewChangesetItem {
                    grp: "core".into(),
                    catalog_id: Some(p),
                    kind: "host".into(),
                    name: "oci".into(),
                    action: "sync".into(),
                    params: Some(r#"{"layer":"core","assets":["skill/kept"]}"#.into()),
                    decider: "rule".into(),
                }],
            )
            .unwrap();
        put(&store, "oci", vec![]);
        let r = reconcile(&store, true).unwrap();
        assert_eq!(r.withdrawn, 1);
        let s = store.lock().unwrap();
        let gone = s.get_changeset(card.id).unwrap().unwrap();
        assert_eq!((gone.state.as_str(), gone.error.as_deref()), ("dismissed", Some(WITHDRAWN)));
        assert_eq!(s.get_changeset(rollout.id).unwrap().unwrap().state, "proposed");
    }

    /// R16: a layer never rolled out whose member is `missing` on a host it
    /// is assigned to gets a Rollout card for that host.
    #[test]
    fn a_layer_never_rolled_out_with_missing_members_gets_a_rollout_card() {
        let _g = lock_registry_for_test();
        let (store, p) = fleet_store(vec![skill("w")]);
        let mut core = author::layer_template("core", Axis::Context);
        core.members.push("skill/w".into());
        let (layers, errors) = LayerSet::from_layers(vec![core]);
        assert!(errors.is_empty(), "{errors:?}");
        registry::install_personal(Catalog {
            id: p,
            name: "personal".into(),
            assets: vec![skill("w")],
            layers,
            ..Default::default()
        })
        .unwrap();
        store
            .lock()
            .unwrap()
            .set_host_layers_for("oci", p, None, &["core"])
            .unwrap();
        let mut missing = unmanaged("oci", "w");
        missing.state = "missing".into();
        missing.catalog_id = Some(p);
        put(&store, "oci", vec![missing]);

        reconcile(&store, true).unwrap();
        let s = store.lock().unwrap();
        let cards = s.list_changesets().unwrap();
        let rollout = cards.iter().find(|c| c.kind == "rollout").expect("a rollout card");
        assert_eq!(rollout.summary, "Roll out core to oci");
        let items = s.changeset_items(rollout.id).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "oci");
        assert_eq!(ItemParams::parse(items[0].params.as_deref()).assets, ["skill/w"]);
    }

    /// R18: with `catalog.auto` off the tick's pass writes nothing; an
    /// on-demand propose still builds cards, with `hide` items instead of
    /// automatic verdicts.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn with_auto_off_the_tick_writes_nothing_and_propose_brings_hide_items() {
        let _g = lock_registry_for_test();
        let (store, _) = fleet_store(vec![]);
        settings::set(&store.lock().unwrap(), settings::CATALOG_AUTO, "false").unwrap();
        put(&store, "oci", vec![unmanaged("oci", "w"), fleet_hook("oci")]);
        after_scan_pass(&store);
        assert!(store.lock().unwrap().list_changesets().unwrap().is_empty());

        let cards = super::super::propose(&store).await.unwrap();
        assert_eq!(cards.len(), 1);
        let (_, items) = super::super::card(cards[0].id, &store).unwrap();
        assert!(items.iter().any(|i| i.action == "hide" && i.name == "stop"));
        assert!(store.lock().unwrap().triage_verdicts().unwrap().is_empty());
    }

    /// R19: the tick's pass never waits for an apply; it skips and the next
    /// pass catches up.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn the_tick_pass_never_waits_for_an_apply() {
        let _g = lock_registry_for_test();
        let (store, _) = fleet_store(vec![]);
        put(&store, "oci", (0..3).map(|n| unmanaged("oci", &format!("s{n}"))).collect());
        let busy = super::super::APPLY_LOCK.lock().await;
        after_scan_pass(&store);
        assert!(store.lock().unwrap().list_changesets().unwrap().is_empty(), "skipped, not waited");
        drop(busy);
        after_scan_pass(&store);
        assert_eq!(store.lock().unwrap().list_changesets().unwrap().len(), 1);
    }
}
```

`changesets/mod.rs` gets the test of the views at the end of the file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// `list` shows every open card and the most recent closed ones, with
    /// item counts per group; `get` resolves catalog names.
    #[test]
    fn list_and_get_describe_cards() {
        let s = Store::open_in_memory().unwrap();
        s.set_catalog_config("/p", None).unwrap();
        let p = s.personal_catalog().unwrap().unwrap().id;
        let it = |grp: &str| NewChangesetItem {
            grp: grp.into(),
            catalog_id: Some(p),
            kind: "skill".into(),
            name: "w".into(),
            action: "import".into(),
            params: Some(r#"{"from_host":"oci"}"#.into()),
            decider: "rule".into(),
        };
        let card = s.insert_changeset("new", "New on oci: skill/w → core", &[it("core"), it("core")]).unwrap();
        let store = Mutex::new(s);
        let all = list(&store).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!((all[0].pending, all[0].groups.get("core")), (2, Some(&2)));
        assert!(!all[0].undoable);
        let v = get(card.id, &store).unwrap();
        assert_eq!(v.items[0].catalog.as_deref(), Some("personal"));
        assert_eq!(v.items[0].params.from_host.as_deref(), Some("oci"));
        assert_eq!(get(9999, &store).unwrap_err().code, codes::E_NOTFOUND);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p fleet-core changesets::reconcile` — Expected: compile error, `reconcile` not found.

- [ ] **Step 3: Write the implementation**

`service/settings.rs` — next to the scan-tick keys:

```rust
// ── asset changesets (Assets M4; `service::catalog::changesets`) ──
/// SB6 / spec *Automatic (no card)*: after every scan-tick pass, hide
/// internals, build changeset cards, and apply additive sync ops on layers
/// already rolled out once. On by default.
pub const CATALOG_AUTO: &str = "catalog.auto";
/// SB4: push each catalog a changeset card commits to, right after it
/// applies (or is undone). Off by default.
pub const CATALOG_AUTO_PUSH: &str = "catalog.auto_push";
```

and in `SPECS`, right after the `CATALOG_SCAN_MAX_AGE_SECS` row:

```rust
    Spec::new(
        CATALOG_AUTO,
        "true",
        Kind::Bool,
        "Asset cards and safe sync",
        "After each asset scan, hide fleet's own and Claude's internal assets, propose changeset cards, and install or update assets on hosts for layers already rolled out once. Never overwrites or removes.",
    ),
    Spec::new(
        CATALOG_AUTO_PUSH,
        "false",
        Kind::Bool,
        "Push applied cards",
        "Push the catalog repo right after a changeset card commits to it or is undone. Off: push it yourself.",
    ),
```

`crates/fleet-core/pages/settings.automation.json` — the *Assets* section's `items` gains, after `catalog.scan_max_age_secs`:

```json
        {
          "type": "field",
          "key": "catalog.auto"
        },
        {
          "type": "field",
          "key": "catalog.auto_push"
        }
```

`src/lib/fleet_settings.ts` — `SETTING_KEYS` gains `catalogAuto: 'catalog.auto',` and `catalogAutoPush: 'catalog.auto_push',` after `catalogScanMaxAgeSecs`; `SETTING_DEFAULTS` gains `'catalog.auto': 'true',` and `'catalog.auto_push': 'false',` after `'catalog.scan_max_age_secs'`.

`changesets/mod.rs` — after the model types:

```rust
pub mod reconcile;

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::settings;
use crate::store::{ChangesetItemRow, ChangesetRow, Store};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

/// One apply at a time (Rulings R27): apply, undo, dismiss, reject_item,
/// on-demand propose, the tick's reconcile and SB6 all take it. A tokio
/// mutex, not the store: it is held across the awaits of an apply, while
/// every store guard inside stays scoped. The tick only ever `try_lock`s it.
pub(crate) static APPLY_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Closed cards `list` shows next to every open one.
pub const RECENT_CLOSED: usize = 20;

/// One item as the tool shows it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemView {
    pub position: i64,
    pub grp: String,
    /// The catalog's name; `None` for an item that names none (hide).
    #[serde(default)]
    pub catalog: Option<String>,
    pub kind: String,
    pub name: String,
    pub action: String,
    #[serde(default)]
    pub params: ItemParams,
    pub decider: String,
    pub state: String,
}

/// One card in full (`changesets { list, id }`, and every mutating action's
/// answer).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangesetView {
    pub id: i64,
    pub kind: String,
    pub summary: String,
    pub state: String,
    pub created_at: i64,
    #[serde(default)]
    pub applied_at: Option<i64>,
    #[serde(default)]
    pub error: Option<String>,
    /// Catalog name → the commit the apply made there.
    #[serde(default)]
    pub commits: BTreeMap<String, String>,
    #[serde(default)]
    pub undoable: bool,
    pub items: Vec<ItemView>,
}

/// One card in a list (`changesets { list }`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangesetSummary {
    pub id: i64,
    pub kind: String,
    pub summary: String,
    pub state: String,
    pub created_at: i64,
    #[serde(default)]
    pub applied_at: Option<i64>,
    #[serde(default)]
    pub error: Option<String>,
    /// Group → item count.
    #[serde(default)]
    pub groups: BTreeMap<String, usize>,
    #[serde(default)]
    pub pending: usize,
    #[serde(default)]
    pub undoable: bool,
}

/// R3: a card that can still be applied (and that the pass refreshes).
pub fn is_open(state: &str) -> bool {
    matches!(state, "proposed" | "failed")
}

/// A card and its items, or `E_NOTFOUND`.
pub fn card(id: i64, store: &Mutex<Store>) -> Result<(ChangesetRow, Vec<ChangesetItemRow>), IpcError> {
    let s = lock(store)?;
    let row = s
        .get_changeset(id)?
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no changeset card {id}")))?;
    let items = s.changeset_items(id)?;
    Ok((row, items))
}

/// Whether applying `positions` of this card writes to hosts: every rollout,
/// and a drift card's restore (R15, R25).
pub fn writes_hosts(card: &ChangesetRow, items: &[ChangesetItemRow], positions: Option<&[i64]>) -> bool {
    match card.kind.as_str() {
        "rollout" => true,
        "drift" => positions.unwrap_or(&[]).iter().any(|p| {
            items
                .iter()
                .any(|i| i.position == *p && i.action == ItemAction::Restore.as_str())
        }),
        _ => false,
    }
}

/// A card that changed a catalog, so can be undone (R20): bootstrap, new,
/// and a drift applied as take_host.
pub(crate) fn changes_catalog(card: &ChangesetRow, items: &[ChangesetItemRow]) -> bool {
    match card.kind.as_str() {
        "bootstrap" | "new" => true,
        "drift" => items
            .iter()
            .any(|i| i.action == ItemAction::TakeHost.as_str() && i.state == "applied"),
        _ => false,
    }
}

/// The catalogs a card's applied items touched.
pub(crate) fn applied_catalogs(items: &[ChangesetItemRow]) -> BTreeSet<i64> {
    items
        .iter()
        .filter(|i| i.state == "applied")
        .filter_map(|i| i.catalog_id)
        .collect()
}

/// R20: a later applied catalog-changing card sharing a catalog with `card`
/// — `(its id, the shared catalog's name)` — which must be undone first.
pub(crate) fn later_card(
    card: &ChangesetRow,
    items: &[ChangesetItemRow],
    s: &Store,
) -> Result<Option<(i64, String)>, IpcError> {
    let mine = applied_catalogs(items);
    for other in s.list_changesets()? {
        if other.id == card.id
            || other.state != "applied"
            || (other.applied_at, other.id) <= (card.applied_at, card.id)
        {
            continue;
        }
        let its = s.changeset_items(other.id)?;
        if !changes_catalog(&other, &its) {
            continue;
        }
        if let Some(cid) = applied_catalogs(&its).intersection(&mine).next() {
            let name = s
                .get_catalog(*cid)?
                .map(|r| r.name)
                .unwrap_or_else(|| cid.to_string());
            return Ok(Some((other.id, name)));
        }
    }
    Ok(None)
}

fn undoable(card: &ChangesetRow, items: &[ChangesetItemRow], s: &Store) -> Result<bool, IpcError> {
    Ok(card.state == "applied" && changes_catalog(card, items) && later_card(card, items, s)?.is_none())
}

fn view(card: ChangesetRow, items: Vec<ChangesetItemRow>, s: &Store) -> Result<ChangesetView, IpcError> {
    let names: BTreeMap<i64, String> = s.list_catalogs()?.into_iter().map(|r| (r.id, r.name)).collect();
    let undoable = undoable(&card, &items, s)?;
    let commits = card
        .commits
        .as_deref()
        .and_then(|c| serde_json::from_str::<BTreeMap<String, String>>(c).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|(id, sha)| {
            let name = id.parse::<i64>().ok().and_then(|i| names.get(&i).cloned());
            (name.unwrap_or(id), sha)
        })
        .collect();
    Ok(ChangesetView {
        id: card.id,
        kind: card.kind,
        summary: card.summary,
        state: card.state,
        created_at: card.created_at,
        applied_at: card.applied_at,
        error: card.error,
        commits,
        undoable,
        items: items
            .into_iter()
            .map(|i| ItemView {
                position: i.position,
                grp: i.grp,
                catalog: i.catalog_id.and_then(|c| names.get(&c).cloned()),
                kind: i.kind,
                name: i.name,
                action: i.action,
                params: ItemParams::parse(i.params.as_deref()),
                decider: i.decider,
                state: i.state,
            })
            .collect(),
    })
}

/// One card in full.
pub fn get(id: i64, store: &Mutex<Store>) -> Result<ChangesetView, IpcError> {
    let s = lock(store)?;
    let card = s
        .get_changeset(id)?
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no changeset card {id}")))?;
    let items = s.changeset_items(id)?;
    view(card, items, &s)
}

/// Every open card and the [`RECENT_CLOSED`] most recent others, newest first.
pub fn list(store: &Mutex<Store>) -> Result<Vec<ChangesetSummary>, IpcError> {
    let s = lock(store)?;
    let mut closed = 0;
    let mut out = Vec::new();
    for card in s.list_changesets()? {
        if !is_open(&card.state) {
            if closed >= RECENT_CLOSED {
                continue;
            }
            closed += 1;
        }
        let items = s.changeset_items(card.id)?;
        let mut groups: BTreeMap<String, usize> = BTreeMap::new();
        for i in &items {
            *groups.entry(i.grp.clone()).or_insert(0) += 1;
        }
        let pending = items.iter().filter(|i| i.state == "pending").count();
        let undoable = undoable(&card, &items, &s)?;
        out.push(ChangesetSummary {
            id: card.id,
            kind: card.kind,
            summary: card.summary,
            state: card.state,
            created_at: card.created_at,
            applied_at: card.applied_at,
            error: card.error,
            groups,
            pending,
            undoable,
        });
    }
    Ok(out)
}

/// `changesets { propose }`: run the pass now (waiting for an apply in
/// flight), with `hide` items instead of automatic verdicts when
/// `catalog.auto` is off (R18), then list.
pub async fn propose(store: &Mutex<Store>) -> Result<Vec<ChangesetSummary>, IpcError> {
    let _busy = APPLY_LOCK.lock().await;
    let auto = settings::get_bool(&*lock(store)?, settings::CATALOG_AUTO);
    reconcile::reconcile(store, auto)?;
    list(store)
}
```

(The `use crate::store::NewChangesetItem;` at the top stays; the tests module also needs it — `use super::*` brings it.)

`changesets/reconcile.rs` (above its tests):

```rust
//! Assets M4: the reconcile pass — gather the facts, ask the rules, write
//! the cards (spec: "A reconcile pass runs after each scan-tick pass and on
//! demand"). Synchronous: store, then registry, never SSH. Store rows are
//! read under one guard that is dropped before the registry is read, and the
//! writes take a fresh guard (store → registry is never allowed).

use super::rules::{self, CatalogFacts, DriftFacts, HostFacts, LayerFacts, LayerGap, RulesInput, SubjectItem};
use super::{is_open, CardKind, Decider, ItemParams, ProposedItem, APPLY_LOCK};
use crate::ipc_error::{lock, IpcError};
use crate::service::catalog::identity::{self, IdentityClass};
use crate::service::catalog::repo::Catalog;
use crate::service::catalog::{effective, registry};
use crate::service::settings;
use crate::store::{
    now_unix, AssetInventoryRow, CatalogRow, ChangesetItemRow, ChangesetRow, HostLayerRow,
    NewChangesetItem, Store, TriageVerdictRow,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

/// What one pass wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    pub inserted: usize,
    pub refreshed: usize,
    pub withdrawn: usize,
    pub hidden: usize,
}

/// The error a withdrawn card carries (R2).
pub const WITHDRAWN: &str = "withdrawn: no longer applies";

type OpenCard = (ChangesetRow, Vec<ChangesetItemRow>);

/// One pass. `auto` (`catalog.auto`): write `ignored` verdicts for
/// internals here instead of proposing `hide` items (R18).
pub fn reconcile(store: &Mutex<Store>, auto: bool) -> Result<ReconcileReport, IpcError> {
    // 1. Store rows, under one guard.
    let (rows, hosts, configured, host_layers, verdicts, open, bootstrap_applied, rolled_out) = {
        let s = lock(store)?;
        let all = s.list_changesets()?;
        let mut open: Vec<OpenCard> = Vec::new();
        for c in all.iter().filter(|c| is_open(&c.state)) {
            open.push((c.clone(), s.changeset_items(c.id)?));
        }
        (
            s.list_inventory()?,
            s.list_hosts()?,
            s.list_catalogs()?,
            s.list_all_host_layers()?,
            s.triage_verdicts()?,
            open,
            all.iter().any(|c| c.kind == "bootstrap" && c.state == "applied"),
            s.rolled_out_layers()?,
        )
    };
    // 2. The registry, cloned out (no lock held after).
    let snapshot = registry::snapshot()?;
    let visible: BTreeSet<&str> = hosts.iter().filter(|h| !h.hidden).map(|h| h.alias.as_str()).collect();
    let rows: Vec<AssetInventoryRow> = rows
        .into_iter()
        .filter(|r| visible.contains(r.host_alias.as_str()))
        .collect();
    let identities = identity::group_identities(&rows);
    let host_facts: Vec<HostFacts> = hosts
        .iter()
        .filter(|h| !h.hidden)
        .map(|h| HostFacts {
            alias: h.alias.clone(),
            org_id: h.org_id,
        })
        .collect();
    let catalogs: Vec<CatalogFacts> = configured
        .iter()
        .map(|row| catalog_facts(row, &snapshot, &host_layers))
        .collect();
    let verdict_keys: BTreeSet<(String, String, String)> = verdicts
        .iter()
        .map(|v| (v.kind.clone(), v.name.clone(), v.content_hash.clone()))
        .collect();
    let drifted: Vec<DriftFacts> = rows
        .iter()
        .filter(|r| r.state == "drifted" && r.managed && r.harness == "claude")
        .filter_map(|r| {
            Some(DriftFacts {
                catalog_id: r.catalog_id?,
                kind: r.kind.clone(),
                name: r.name.clone(),
                host: r.host_alias.clone(),
                host_hash: r.host_hash.clone(),
            })
        })
        .collect();
    let gaps = layer_gaps(store, &snapshot, &configured, &host_layers, &rows, &rolled_out);
    let rollout_open: BTreeSet<(i64, String)> = open
        .iter()
        .filter(|(c, _)| c.kind == "rollout")
        .flat_map(|(_, items)| items.iter().filter_map(|i| Some((i.catalog_id?, i.grp.clone()))))
        .collect();
    let personal_assets = catalogs.iter().find(|c| c.org_id.is_none()).map_or(0, |c| c.asset_count);
    let input = RulesInput {
        identities: &identities,
        hosts: &host_facts,
        catalogs: &catalogs,
        verdicts: &verdict_keys,
        drifted: &drifted,
        gaps: &gaps,
        rollout_open: &rollout_open,
        bootstrapped: bootstrap_applied || personal_assets > 0,
        bootstrap_open: open.iter().any(|(c, _)| c.kind == "bootstrap"),
        auto,
    };
    let proposed = rules::propose(&input);

    // 3. Writes, under one fresh guard.
    let s = lock(store)?;
    let mut report = ReconcileReport::default();
    if auto {
        let now = now_unix();
        for id in identities.iter().filter(|i| {
            matches!(i.class, IdentityClass::FleetInternal | IdentityClass::HarnessInternal)
        }) {
            let hash = rules::identity_hash(id);
            if verdict_keys.contains(&(id.kind.clone(), id.name.clone(), hash.clone())) {
                continue;
            }
            s.upsert_triage_verdict(&TriageVerdictRow {
                catalog_id: None,
                kind: id.kind.clone(),
                name: id.name.clone(),
                content_hash: hash,
                verdict: "ignored".into(),
                decider: Decider::Rule.as_str().into(),
                decided_at: now,
            })?;
            report.hidden += 1;
        }
    }
    let by_subject: BTreeMap<String, &OpenCard> = open.iter().map(|o| (subject_of_row(&o.0, &o.1), o)).collect();
    let mut produced: BTreeSet<String> = BTreeSet::new();
    for card in &proposed {
        let subject = card.subject();
        let items: Vec<NewChangesetItem> = card.items.iter().map(ProposedItem::to_new).collect();
        match by_subject.get(&subject) {
            Some(o) => {
                let (row, existing) = (&o.0, &o.1);
                let same = row.summary == card.summary
                    && existing.iter().map(NewChangesetItem::from).collect::<Vec<_>>() == items;
                if !same && s.replace_changeset_items(row.id, &card.summary, &items)? {
                    report.refreshed += 1;
                }
            }
            None => {
                s.insert_changeset(card.kind.as_str(), &card.summary, &items)?;
                report.inserted += 1;
            }
        }
        produced.insert(subject);
    }
    for (subject, o) in &by_subject {
        let (row, items) = (&o.0, &o.1);
        if row.kind == CardKind::Rollout.as_str()
            || produced.contains(subject)
            || items.iter().any(|i| i.state == "applied")
        {
            continue;
        }
        s.set_changeset_state(row.id, "dismissed", Some(WITHDRAWN))?;
        report.withdrawn += 1;
    }
    Ok(report)
}

fn catalog_facts(row: &CatalogRow, snapshot: &BTreeMap<i64, Catalog>, host_layers: &[HostLayerRow]) -> CatalogFacts {
    let entry = registry::entry_for(snapshot, row);
    CatalogFacts {
        id: row.id,
        name: row.name.clone(),
        org_id: row.org_id,
        loaded: entry.is_some_and(|c| c.load_error.is_none()),
        asset_count: entry.map_or(0, |c| c.assets.len()),
        layers: entry
            .map(|c| {
                c.layers
                    .iter()
                    .map(|l| LayerFacts {
                        name: l.name.clone(),
                        hosts: host_layers
                            .iter()
                            .filter(|r| r.active && r.catalog_id == row.id && r.layer_name == l.name)
                            .map(|r| r.host_alias.clone())
                            .collect(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// R16: per (catalog, layer, host), the members a never-rolled-out layer
/// introduced on a host that the scan finds `missing` there. Composes each
/// layered host's effective set against `snapshot`
/// (`effective_for_host_in` takes the store guard itself; none is held here).
fn layer_gaps(
    store: &Mutex<Store>,
    snapshot: &BTreeMap<i64, Catalog>,
    configured: &[CatalogRow],
    host_layers: &[HostLayerRow],
    rows: &[AssetInventoryRow],
    rolled_out: &BTreeSet<(i64, String)>,
) -> Vec<LayerGap> {
    let id_of: BTreeMap<String, i64> = configured
        .iter()
        .map(|r| {
            let label = if r.org_id.is_none() { "personal".to_string() } else { r.name.clone() };
            (label, r.id)
        })
        .collect();
    let layered: BTreeSet<&str> = host_layers.iter().filter(|r| r.active).map(|r| r.host_alias.as_str()).collect();
    let mut out: BTreeMap<(i64, String, String), Vec<String>> = BTreeMap::new();
    for host in layered {
        let Ok(eff) = effective::effective_for_host_in(store, host, snapshot) else {
            continue;
        };
        for (key, prov) in &eff.provenance {
            let Some(cid) = id_of.get(&prov.catalog) else { continue };
            if rolled_out.contains(&(*cid, prov.introduced_by.clone())) {
                continue;
            }
            let Some((kind, name)) = key.split_once('/') else { continue };
            let missing = rows.iter().any(|r| {
                r.host_alias == host && r.harness == "claude" && r.kind == kind && r.name == name && r.state == "missing"
            });
            if missing {
                out.entry((*cid, prov.introduced_by.clone(), host.to_string()))
                    .or_default()
                    .push(key.clone());
            }
        }
    }
    out.into_iter()
        .map(|((catalog_id, layer, host), assets)| LayerGap { catalog_id, layer, host, assets })
        .collect()
}

/// A stored card's subject (R2), the same function the proposals use.
fn subject_of_row(card: &ChangesetRow, items: &[ChangesetItemRow]) -> String {
    let kind = CardKind::parse(&card.kind).unwrap_or(CardKind::Rollout);
    let params: Vec<ItemParams> = items.iter().map(|i| ItemParams::parse(i.params.as_deref())).collect();
    rules::subject_of(
        kind,
        items.iter().zip(&params).map(|(i, p)| SubjectItem {
            grp: &i.grp,
            catalog_id: i.catalog_id,
            kind: &i.kind,
            name: &i.name,
            host: p.host.as_deref(),
        }),
    )
}

/// The scan tick's hook (R19, carry 4): with `catalog.auto` on, one pass —
/// unless an apply holds `APPLY_LOCK`, in which case this pass is skipped
/// (the tick never waits). Errors are logged, never returned: the tick's own
/// bookkeeping (`owed`, `seen`) is not this pass's business.
pub fn after_scan_pass(store: &Arc<Mutex<Store>>) {
    let auto = store
        .lock()
        .map(|s| settings::get_bool(&s, settings::CATALOG_AUTO))
        .unwrap_or(false);
    if !auto {
        return;
    }
    let Ok(_busy) = APPLY_LOCK.try_lock() else {
        tracing::debug!("changesets: an apply is running; this pass's reconcile is skipped");
        return;
    };
    match reconcile(store, true) {
        Ok(r) if r != ReconcileReport::default() => tracing::info!(
            inserted = r.inserted,
            refreshed = r.refreshed,
            withdrawn = r.withdrawn,
            hidden = r.hidden,
            "changesets: reconciled"
        ),
        Ok(_) => {}
        Err(e) => tracing::warn!("changesets: reconcile failed: {}", e.message),
    }
}
```

`scan_tick.rs` — between the `for alias in due { … }` loop and `seen = Some(now_key);`:

```rust
            // Assets M4 (R19): the reconcile pass — cards, automatic hides —
            // after every pass that got this far (a personal catalog is
            // loaded). It never waits: an apply in flight skips it, and it
            // never touches `owed` or `seen`.
            super::changesets::reconcile::after_scan_pass(&store);
```

and the module doc's first paragraph gains: "After each pass it runs the changeset reconcile pass (Assets M4)."

- [ ] **Step 4: Run the tests and regenerate**

Run, each on its own: `cargo test -p fleet-core changesets::reconcile`; `cargo test -p fleet-core changesets::tests`; `cargo test -p fleet-core scan_tick`; `cargo test -p fleet-core every_spec_is_mirrored_in_fleet_settings_ts`; `cargo test -p fleet-core every_setting_has_one_home`; `cargo test -p fleet-core every_spec_has_consistent_metadata`; `pnpm test -- fleet_settings`.
Expected: PASS.
Then: `REGEN_SETTINGS_DOCS=1 cargo test -p fleet-core settings_docs_are_current`; `REGEN_PAGE_DOCS=1 cargo test -p fleet-core page_docs_are_current`; run both again without the variable — Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/service/catalog/ crates/fleet-core/src/service/settings.rs crates/fleet-core/pages/settings.automation.json src/lib/fleet_settings.ts docs/ src/lib/pages/
git commit -m "feat(changesets): reconcile pass after each scan tick; catalog.auto and catalog.auto_push"
```

---

### Task 6: Applying a catalog card (bootstrap, new, drift take_host)

**Files:**
- Modify: `crates/fleet-core/src/service/catalog/repo.rs` (`read_asset`, `is_clean`, `reset_hard`, `revert` + tests)
- Create: `crates/fleet-core/src/service/catalog/changesets/apply.rs` (+ tests)
- Create: `crates/fleet-core/src/service/catalog/changesets/testkit.rs` (test-only)
- Modify: `crates/fleet-core/src/service/catalog/changesets/mod.rs` (`pub mod apply;`, `#[cfg(test)] pub(crate) mod testkit;`)

**Interfaces:**
- Consumes: Task 1 (`insert_changeset`, `mark_changeset_applied`, `set_changeset_item_states`, `upsert_triage_verdict`, `restore_host_layers`, `set_host_layers_for`, `get_host_layers_for`, `list_all_host_layers`); Task 3 (`CatalogTarget::Row`, `import_host_into`); Task 4/5 (`ItemParams`, `ItemAction`, `CardKind`, `APPLY_LOCK`, `card`, `get`, `is_open`, `writes_hosts`, `rules::{NEEDS_A_LOOK, UPDATE}`); `author::layer_template`; `load_catalog`; `settings::CATALOG_AUTO_PUSH`.
- Produces:

```rust
// repo.rs
pub fn read_asset(root: &Path, kind: Kind, name: &str) -> Result<Asset, IpcError>;
pub fn is_clean(root: &Path) -> Result<bool, IpcError>;
pub fn reset_hard(root: &Path, rev: &str) -> Result<(), IpcError>;
pub fn revert(root: &Path, sha: &str) -> Result<String, IpcError>;
// changesets/apply.rs
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApplyArgs { pub id: i64, #[serde(default)] pub positions: Option<Vec<i64>> }
pub async fn apply(args: ApplyArgs, store: &Mutex<Store>, ssh: &Arc<SshClient>) -> Result<ChangesetView, IpcError>;
pub(crate) fn follow_up_rollout(selected: &[&ChangesetItemRow], s: &Store) -> Result<Option<(String, Vec<NewChangesetItem>)>, IpcError>;
// changesets/testkit.rs (cfg(test))
pub(crate) fn git(dir: &Path, args: &[&str]) -> String; pub(crate) fn head(root: &Path) -> String; pub(crate) fn subjects(root: &Path) -> Vec<String>;
pub(crate) fn init_catalog(root: &Path) -> PathBuf; pub(crate) fn host_skill(home: &Path, name: &str, description: &str);
#[cfg(unix)] pub(crate) fn ssh_with_home(bin_dir: &Path, home: &Path) -> Arc<SshClient>;
pub(crate) struct Fleet { pub store: Mutex<Store>, pub personal: CatalogRow, pub personal_root: PathBuf, .. }
impl Fleet { pub(crate) fn new(hosts: &[&str]) -> Fleet; pub(crate) fn add_org_catalog(&mut self, name: &str) -> (CatalogRow, PathBuf); pub(crate) fn commit_files(&self, root: &Path, id: i64, files: &[(&str, &str)]); }
pub(crate) fn item(grp: &str, catalog_id: Option<i64>, kind: &str, name: &str, action: ItemAction, params: ItemParams) -> NewChangesetItem;
```

- [ ] **Step 1: Write the failing tests**

`repo.rs` tests module:

```rust
    /// Assets M4: the apply engine's git steps — a clean check that counts
    /// untracked files, a reset that drops this apply's commit and strays,
    /// and a revert that keeps history and takes only a hex sha.
    #[test]
    fn is_clean_reset_hard_and_revert() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]).unwrap();
        std::fs::write(root.join("catalog.yaml"), "schema_version: 1\n").unwrap();
        stage_paths(root, &[]).unwrap();
        let base = commit(root, "init").unwrap();
        assert!(is_clean(root).unwrap());
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        assert!(!is_clean(root).unwrap(), "an untracked file is not clean");
        stage_paths(root, &[]).unwrap();
        commit(root, "add a").unwrap();
        std::fs::write(root.join("stray.txt"), "x\n").unwrap();
        reset_hard(root, &base).unwrap();
        assert_eq!(head(root).unwrap(), base);
        assert!(is_clean(root).unwrap());
        assert!(!root.join("a.txt").exists() && !root.join("stray.txt").exists());

        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        stage_paths(root, &[]).unwrap();
        let added = commit(root, "add a").unwrap();
        let reverted = revert(root, &added).unwrap();
        assert_ne!(reverted, added);
        assert!(!root.join("a.txt").exists());
        assert!(revert(root, "--help").is_err(), "only a hex sha reaches git");
    }

    #[test]
    fn read_asset_reads_one_asset_back() {
        let dir = tempfile::tempdir().unwrap();
        let a = Asset::from_yaml(None, "kind: skill\nname: w\ndescription: d\n").unwrap();
        write_asset(dir.path(), &a, false).unwrap();
        assert_eq!(read_asset(dir.path(), Kind::Skill, "w").unwrap().header.name, "w");
        assert!(read_asset(dir.path(), Kind::Skill, "nope").is_err());
    }
```

`changesets/testkit.rs`:

```rust
//! Test fixtures for the changeset engine (Tasks 6–9): real git checkouts,
//! a store with reachable hosts, and a fake `ssh` that runs the remote
//! script locally under a chosen `HOME` — no process-wide env change.

use super::{ItemAction, ItemParams};
use crate::ssh::SshClient;
use crate::store::{CatalogRow, NewChangesetItem, Store};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub(crate) fn git(dir: &Path, args: &[&str]) -> String {
    let out = crate::proc::std_command("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A catalog checkout at `root` with one commit.
pub(crate) fn init_catalog(root: &Path) -> PathBuf {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(root.join("catalog.yaml"), "schema_version: 1\n").unwrap();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["config", "user.email", "t@t"]);
    git(root, &["config", "user.name", "t"]);
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "init"]);
    root.to_path_buf()
}

pub(crate) fn head(root: &Path) -> String {
    git(root, &["rev-parse", "HEAD"])
}

pub(crate) fn subjects(root: &Path) -> Vec<String> {
    git(root, &["log", "--format=%s"])
        .lines()
        .map(str::to_string)
        .collect()
}

/// `~/.claude/skills/<name>/SKILL.md` under `home`.
pub(crate) fn host_skill(home: &Path, name: &str, description: &str) {
    let dir = home.join(".claude/skills").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\nSteps for {name}.\n"),
    )
    .unwrap();
}

/// A fake `ssh` (M3 R20's pattern): everything up to `-- <host>` is dropped
/// and the rest runs under `sh -c` with `HOME` set to `home`, as the remote
/// login shell would run it.
#[cfg(unix)]
pub(crate) fn ssh_with_home(bin_dir: &Path, home: &Path) -> Arc<SshClient> {
    use crate::tmux::fake_exec::{write_exec, PROBE_GUARD};
    let bin = write_exec(
        bin_dir,
        "ssh",
        &format!(
            "#!/bin/sh\n{PROBE_GUARD}\
             case \"$*\" in *'-O check'*|*'-O exit'*) exit 0;; esac\n\
             while [ \"$#\" -gt 0 ] && [ \"$1\" != \"--\" ]; do shift; done\n\
             shift 2\n\
             HOME='{home}' exec sh -c \"$*\"\n",
            home = home.display()
        ),
    );
    Arc::new(SshClient::with_ssh_binary(bin))
}

/// A store with `personal` (a fresh checkout, loaded) and reachable hosts.
pub(crate) struct Fleet {
    pub store: Mutex<Store>,
    pub personal: CatalogRow,
    pub personal_root: PathBuf,
    dirs: Vec<tempfile::TempDir>,
}

impl Fleet {
    pub(crate) fn new(hosts: &[&str]) -> Fleet {
        let dir = tempfile::tempdir().unwrap();
        let personal_root = init_catalog(&dir.path().join("personal"));
        let s = Store::open_in_memory().unwrap();
        s.set_catalog_config(&personal_root.to_string_lossy(), None)
            .unwrap();
        for h in hosts {
            s.insert_host(h, Some(h)).unwrap();
            s.update_host_probe(h, true, None, None, 1).unwrap();
        }
        let personal = s.personal_catalog().unwrap().unwrap();
        let store = Mutex::new(s);
        crate::service::catalog::load_catalog(personal.id, false, &store).unwrap();
        Fleet {
            store,
            personal,
            personal_root,
            dirs: vec![dir],
        }
    }

    /// An org named `name` owning a catalog of the same name, loaded.
    pub(crate) fn add_org_catalog(&mut self, name: &str) -> (CatalogRow, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = init_catalog(&dir.path().join(name));
        let row = {
            let s = self.store.lock().unwrap();
            let org = s.add_org(name, None, false).unwrap();
            s.upsert_catalog(name, &root.to_string_lossy(), None, Some(org.id))
                .unwrap()
        };
        crate::service::catalog::load_catalog(row.id, false, &self.store).unwrap();
        self.dirs.push(dir);
        (row, root)
    }

    /// Write and commit `files` (relative path, content) in `root`, then
    /// reload catalog `id`.
    pub(crate) fn commit_files(&self, root: &Path, id: i64, files: &[(&str, &str)]) {
        for (rel, body) in files {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "seed"]);
        crate::service::catalog::load_catalog(id, false, &self.store).unwrap();
    }
}

pub(crate) fn item(
    grp: &str,
    catalog_id: Option<i64>,
    kind: &str,
    name: &str,
    action: ItemAction,
    params: ItemParams,
) -> NewChangesetItem {
    NewChangesetItem {
        grp: grp.into(),
        catalog_id,
        kind: kind.into(),
        name: name.into(),
        action: action.as_str().into(),
        params: params.to_json(),
        decider: "rule".into(),
    }
}
```

`changesets/apply.rs` — the tests module:

```rust
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::service::catalog::changesets::rules::NEEDS_A_LOOK;
    use crate::service::catalog::changesets::testkit::*;
    use crate::service::catalog::lock_registry_for_test;

    const DESC: &str = "A reasonably long description here.";

    fn import(layer: &str, cid: i64, name: &str, from: &str) -> NewChangesetItem {
        item(
            layer,
            Some(cid),
            "skill",
            name,
            ItemAction::Import,
            ItemParams {
                from_host: Some(from.into()),
                layer: Some(layer.into()),
                member: Some(format!("skill/{name}")),
                hash: Some(format!("h-{name}")),
                ..Default::default()
            },
        )
    }

    fn assign(layer: &str, cid: i64, host: &str) -> NewChangesetItem {
        item(
            layer,
            Some(cid),
            "layer",
            layer,
            ItemAction::AssignLayer,
            ItemParams {
                host: Some(host.into()),
                layer: Some(layer.into()),
                axis: Some("context".into()),
                ..Default::default()
            },
        )
    }

    fn contexts(f: &Fleet, host: &str, cid: i64) -> Vec<String> {
        f.store
            .lock()
            .unwrap()
            .get_host_layers_for(host, cid)
            .unwrap()
            .into_iter()
            .map(|r| r.layer_name)
            .collect()
    }

    /// Spec, Apply 1–5: imports, the layer file, the scope and the host's
    /// layers land in one commit; "needs a look" is skipped unless named;
    /// the catalog reloads; a follow-up Rollout card is proposed (R14).
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn applying_a_bootstrap_commits_once_and_assigns_layers() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        let home = tempfile::tempdir().unwrap();
        host_skill(home.path(), "w", DESC);
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let p = f.personal.id;
        let card = f
            .store
            .lock()
            .unwrap()
            .insert_changeset(
                "bootstrap",
                "Adopt 1 as 1 layers",
                &[
                    import("core", p, "w", "oci"),
                    item(
                        "core",
                        Some(p),
                        "skill",
                        "w",
                        ItemAction::SetScope,
                        ItemParams {
                            scope: Some("shared".into()),
                            member: Some("skill/w".into()),
                            ..Default::default()
                        },
                    ),
                    assign("core", p, "oci"),
                    item(
                        NEEDS_A_LOOK,
                        Some(p),
                        "skill",
                        "odd",
                        ItemAction::Import,
                        ItemParams {
                            from_host: Some("oci".into()),
                            reason: Some("carries a secret".into()),
                            ..Default::default()
                        },
                    ),
                ],
            )
            .unwrap();
        let before = head(&f.personal_root);

        let v = apply(ApplyArgs { id: card.id, positions: None }, &f.store, &ssh)
            .await
            .unwrap();
        assert_eq!(v.state, "applied", "{:?}", v.error);
        assert_eq!(subjects(&f.personal_root)[0], "fleet: Adopt 1 as 1 layers");
        assert_eq!(
            git(&f.personal_root, &["rev-list", "--count", &format!("{before}..HEAD")]),
            "1",
            "one commit per catalog"
        );
        assert_eq!(v.commits.get("personal"), Some(&head(&f.personal_root)));
        let w = repo::read_asset(&f.personal_root, Kind::Skill, "w").unwrap();
        assert_eq!(w.header.scope, Scope::Shared);
        let layer = Layer::from_yaml(
            &std::fs::read_to_string(f.personal_root.join("layers/core.yaml")).unwrap(),
        )
        .unwrap();
        assert_eq!((layer.axis, layer.members.clone()), (Axis::Context, vec!["skill/w".to_string()]));
        assert_eq!(contexts(&f, "oci", p), ["core"]);
        let states: Vec<&str> = v.items.iter().map(|i| i.state.as_str()).collect();
        assert_eq!(states, ["applied", "applied", "applied", "skipped"]);
        assert!(
            registry::with_catalog_row(&f.personal, |c| Ok(c.find(Kind::Skill, "w").is_some())).unwrap(),
            "reloaded"
        );
        let cards = f.store.lock().unwrap().list_changesets().unwrap();
        let rollout = cards.iter().find(|c| c.kind == "rollout").expect("a follow-up rollout");
        assert_eq!(rollout.state, "proposed");
        assert!(rollout.summary.starts_with("Roll out core to oci"), "{}", rollout.summary);
    }

    /// Spec, Testing: "apply makes one commit per catalog".
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_card_touching_two_catalogs_commits_once_in_each() {
        let _g = lock_registry_for_test();
        let mut f = Fleet::new(&["oci"]);
        let (acme, acme_root) = f.add_org_catalog("acme");
        let home = tempfile::tempdir().unwrap();
        host_skill(home.path(), "w", DESC);
        host_skill(home.path(), "v", DESC);
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let card = f
            .store
            .lock()
            .unwrap()
            .insert_changeset(
                "bootstrap",
                "Adopt 2 as 2 layers",
                &[import("core", f.personal.id, "w", "oci"), import("ops", acme.id, "v", "oci")],
            )
            .unwrap();
        let (p0, a0) = (head(&f.personal_root), head(&acme_root));
        let v = apply(ApplyArgs { id: card.id, positions: None }, &f.store, &ssh)
            .await
            .unwrap();
        assert_eq!(v.state, "applied", "{:?}", v.error);
        for (root, before) in [(&f.personal_root, p0), (&acme_root, a0)] {
            assert_eq!(git(root, &["rev-list", "--count", &format!("{before}..HEAD")]), "1");
        }
        assert_eq!(v.commits.keys().map(String::as_str).collect::<Vec<_>>(), ["acme", "personal"]);
        assert!(acme_root.join("skills/v/asset.yaml").is_file());
        assert!(!f.personal_root.join("skills/v").exists());
    }

    /// Spec, Testing: "a failed apply commits nothing" — an import that
    /// brings nothing back fails its group; the tree is reset.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_failed_import_commits_nothing_and_leaves_the_card_failed() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        let home = tempfile::tempdir().unwrap();
        host_skill(home.path(), "w", DESC);
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let p = f.personal.id;
        let card = f
            .store
            .lock()
            .unwrap()
            .insert_changeset(
                "bootstrap",
                "Adopt 2 as 2 layers",
                &[import("core", p, "w", "oci"), import("extra", p, "absent", "oci")],
            )
            .unwrap();
        let before = head(&f.personal_root);
        let err = apply(ApplyArgs { id: card.id, positions: None }, &f.store, &ssh)
            .await
            .unwrap_err();
        assert!(err.message.starts_with("extra: "), "{}", err.message);
        assert_eq!(head(&f.personal_root), before, "nothing committed");
        assert!(git(&f.personal_root, &["status", "--porcelain"]).is_empty(), "nothing left behind");
        let v = super::super::get(card.id, &f.store).unwrap();
        assert_eq!(v.state, "failed");
        assert_eq!(v.error.as_deref(), Some(err.message.as_str()));
        assert!(v.items.iter().all(|i| i.state == "pending"), "a failed card can be applied again");
    }

    /// R12: a commit that fails in the second catalog resets the first one's
    /// commit too and restores `host_layers`.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_failed_commit_resets_every_catalog_and_restores_host_layers() {
        use crate::tmux::fake_exec::{write_exec, PROBE_GUARD};
        let _g = lock_registry_for_test();
        let mut f = Fleet::new(&["oci"]);
        let (acme, acme_root) = f.add_org_catalog("acme");
        write_exec(&acme_root.join(".git/hooks"), "pre-commit", &format!("#!/bin/sh\n{PROBE_GUARD}exit 1\n"));
        let home = tempfile::tempdir().unwrap();
        host_skill(home.path(), "w", DESC);
        host_skill(home.path(), "v", DESC);
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let p = f.personal.id;
        f.store
            .lock()
            .unwrap()
            .set_host_layers_for("oci", p, None, &["base"])
            .unwrap();
        let card = f
            .store
            .lock()
            .unwrap()
            .insert_changeset(
                "bootstrap",
                "Adopt 2 as 2 layers",
                &[import("core", p, "w", "oci"), assign("core", p, "oci"), import("ops", acme.id, "v", "oci")],
            )
            .unwrap();
        let (p0, a0) = (head(&f.personal_root), head(&acme_root));
        let err = apply(ApplyArgs { id: card.id, positions: None }, &f.store, &ssh)
            .await
            .unwrap_err();
        assert!(err.message.starts_with("commit: catalog acme"), "{}", err.message);
        assert_eq!((head(&f.personal_root), head(&acme_root)), (p0, a0), "nothing committed anywhere");
        assert!(!f.personal_root.join("skills/w").exists());
        assert_eq!(contexts(&f, "oci", p), ["base"], "host_layers restored");
    }

    /// R11: a dirty checkout refuses before anything is written; the card
    /// stays proposed.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_dirty_catalog_refuses_before_anything_changes() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        std::fs::write(f.personal_root.join("pending.txt"), "x\n").unwrap();
        let bin = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let card = f
            .store
            .lock()
            .unwrap()
            .insert_changeset("new", "New", &[import("core", f.personal.id, "w", "oci")])
            .unwrap();
        let err = apply(ApplyArgs { id: card.id, positions: None }, &f.store, &ssh)
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID_STATE);
        assert!(err.message.contains("uncommitted"), "{}", err.message);
        assert_eq!(super::super::get(card.id, &f.store).unwrap().state, "proposed");
    }

    /// Spec, Drift: one item at a time; take_host imports the host's copy
    /// and keeps the catalog copy's scope (R13).
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_drift_card_takes_the_host_copy_one_item_at_a_time() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        let p = f.personal.id;
        f.commit_files(
            &f.personal_root,
            p,
            &[
                ("skills/w/asset.yaml", "kind: skill\nname: w\ndescription: The catalog's own long description.\nscope: shared\n"),
                ("skills/w/body.md", "Old steps.\n"),
            ],
        );
        let home = tempfile::tempdir().unwrap();
        host_skill(home.path(), "w", "Edited on the host, long enough.");
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let drift = |action| {
            item(
                "drift",
                Some(p),
                "skill",
                "w",
                action,
                ItemParams {
                    host: Some("oci".into()),
                    hash: Some("e".into()),
                    ..Default::default()
                },
            )
        };
        let card = f
            .store
            .lock()
            .unwrap()
            .insert_changeset(
                "drift",
                "skill/w differs on oci from catalog personal",
                &[drift(ItemAction::TakeHost), drift(ItemAction::Restore)],
            )
            .unwrap();
        let err = apply(ApplyArgs { id: card.id, positions: None }, &f.store, &ssh)
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
        assert!(err.message.contains("one item at a time"), "{}", err.message);

        let v = apply(ApplyArgs { id: card.id, positions: Some(vec![0]) }, &f.store, &ssh)
            .await
            .unwrap();
        assert_eq!(v.state, "applied", "{:?}", v.error);
        let w = repo::read_asset(&f.personal_root, Kind::Skill, "w").unwrap();
        assert_eq!(w.header.description, "Edited on the host, long enough.");
        assert_eq!(w.header.scope, Scope::Shared, "the catalog copy's scope is kept");
        assert_eq!(v.items.iter().map(|i| i.state.as_str()).collect::<Vec<_>>(), ["applied", "skipped"]);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p fleet-core is_clean_reset_hard_and_revert` — Expected: compile error, `is_clean` not found.
Run: `cargo test -p fleet-core changesets::apply` — Expected: compile error, module `apply` not found.

- [ ] **Step 3: Write the implementation**

`repo.rs` (after `push`):

```rust
/// One asset straight from the checkout (Assets M4: a card's scope edit and
/// take_host read the file just written, before any reload).
pub fn read_asset(root: &Path, kind: Kind, name: &str) -> Result<Asset, IpcError> {
    let yaml = asset_path(root, kind, name);
    load_one(root, kind, &yaml, name)
        .map_err(|m| IpcError::new(E_CATALOG_PARSE, format!("{}: {m}", rel(root, &yaml))))
}

/// Whether the working tree has nothing to commit — untracked files count
/// (Rulings R11: what `reset_hard` would delete must not be someone's work).
pub fn is_clean(root: &Path) -> Result<bool, IpcError> {
    Ok(git(root, &["status", "--porcelain"])?.trim().is_empty())
}

/// Put the tree back at `rev`, deleting what is untracked (Rulings R12:
/// only after a failed apply that started from a clean tree).
pub fn reset_hard(root: &Path, rev: &str) -> Result<(), IpcError> {
    git(root, &["reset", "-q", "--hard", rev])?;
    git(root, &["clean", "-q", "-fd"])?;
    Ok(())
}

/// `git revert` one commit (Assets M4 undo, SB5), with the synthetic
/// identity `commit` falls back to. A conflict aborts the revert, leaving
/// the tree as it was, and is the error. Answers the new HEAD.
pub fn revert(root: &Path, sha: &str) -> Result<String, IpcError> {
    if sha.is_empty() || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(IpcError::new(E_INVALID, format!("not a commit id: {sha}")));
    }
    let mut args: Vec<&str> = Vec::new();
    if !has_identity(root) {
        args.extend(["-c", "user.name=claude-fleet", "-c", "user.email=fleet@localhost"]);
    }
    args.extend(["revert", "--no-edit", sha]);
    if let Err(e) = git(root, &args) {
        let _ = git(root, &["revert", "--abort"]);
        return Err(e);
    }
    head(root)
}
```

`changesets/mod.rs`: `pub mod apply;` and `#[cfg(test)] pub(crate) mod testkit;` next to `pub mod reconcile;`.

`changesets/apply.rs` (above its tests):

```rust
//! Assets M4: applying a card (spec, *Changesets* → Apply). A catalog card
//! (bootstrap, new, drift take_host) changes catalogs: its touched catalogs
//! are snapshotted, imported into, their layer files and scopes written,
//! `host_layers` updated, and one commit made per catalog — or, on any
//! failure, every checkout is reset and nothing is committed (R11, R12).

use super::rules::{NEEDS_A_LOOK, UPDATE};
use super::{CardKind, ChangesetView, Decider, ItemAction, ItemParams, APPLY_LOCK};
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::catalog::import::slugify;
use crate::service::catalog::layer::{split_key, Axis, Layer};
use crate::service::catalog::model::{Kind, Scope};
use crate::service::catalog::validate::check_layer_name;
use crate::service::catalog::{author, registry, repo, CatalogTarget, ImportArgs, E_CATALOG_PARSE};
use crate::service::settings;
use crate::ssh::SshClient;
use crate::store::{
    now_unix, CatalogRow, ChangesetItemRow, ChangesetRow, HostLayerRow, NewChangesetItem, Store,
    TriageVerdictRow,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// `changesets { apply }`'s arguments.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApplyArgs {
    pub id: i64,
    /// The items to apply; `None` = every pending item but "needs a look"
    /// (R8). A drift card needs exactly one.
    #[serde(default)]
    pub positions: Option<Vec<i64>>,
}

/// Apply card `args.id` and answer it as it now stands.
pub async fn apply(
    args: ApplyArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<ChangesetView, IpcError> {
    let _busy = APPLY_LOCK.lock().await;
    let (card, items) = super::card(args.id, store)?;
    if !super::is_open(&card.state) {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!("card {} is {}; only a proposed or failed card applies", card.id, card.state),
        ));
    }
    let selected = select_items(&card, &items, args.positions.as_deref())?;
    if super::writes_hosts(&card, &items, args.positions.as_deref()) {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("card {} writes hosts; this build applies catalog cards only", card.id),
        ));
    }
    apply_catalog(&card, &items, &selected, store, ssh).await?;
    super::get(args.id, store)
}

/// R3, R8: the items this apply runs.
fn select_items<'a>(
    card: &ChangesetRow,
    items: &'a [ChangesetItemRow],
    positions: Option<&[i64]>,
) -> Result<Vec<&'a ChangesetItemRow>, IpcError> {
    let chosen: Vec<&ChangesetItemRow> = match positions {
        Some(ps) => {
            let mut out = Vec::new();
            for p in ps {
                let item = items.iter().find(|i| i.position == *p).ok_or_else(|| {
                    IpcError::new(codes::E_NOTFOUND, format!("card {} has no item {p}", card.id))
                })?;
                if item.state != "pending" {
                    return Err(IpcError::new(
                        codes::E_INVALID_STATE,
                        format!("item {p} of card {} is {}", card.id, item.state),
                    ));
                }
                out.push(item);
            }
            out
        }
        None => items
            .iter()
            .filter(|i| i.state == "pending" && i.grp != NEEDS_A_LOOK)
            .collect(),
    };
    if card.kind == CardKind::Drift.as_str() && chosen.len() != 1 {
        return Err(IpcError::new(
            codes::E_INVALID,
            "a drift card applies one item at a time: name take_host or restore in positions",
        ));
    }
    if chosen.is_empty() {
        return Err(IpcError::new(codes::E_INVALID, format!("card {} has nothing to apply", card.id)));
    }
    for i in &chosen {
        if i.action != ItemAction::Hide.as_str() && i.catalog_id.is_none() {
            return Err(IpcError::new(
                codes::E_INVALID_STATE,
                format!("item {} names no catalog (removed since?); dismiss this card", i.position),
            ));
        }
    }
    Ok(chosen)
}

/// A failed step and the card group it failed in (spec: "the error on the
/// failing group").
struct Failure {
    group: String,
    error: IpcError,
}

fn fail(item: &ChangesetItemRow, error: IpcError) -> Failure {
    Failure {
        group: item.grp.clone(),
        error,
    }
}

fn fail_in(group: &str, error: IpcError) -> Failure {
    Failure {
        group: group.to_string(),
        error,
    }
}

fn kind_of(item: &ChangesetItemRow) -> Result<Kind, IpcError> {
    serde_json::from_value(serde_json::Value::String(item.kind.clone())).map_err(|_| {
        IpcError::new(
            codes::E_INVALID,
            format!("item {} names no asset kind ({})", item.position, item.kind),
        )
    })
}

async fn apply_catalog(
    card: &ChangesetRow,
    items: &[ChangesetItemRow],
    selected: &[&ChangesetItemRow],
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<(), IpcError> {
    let touched: BTreeSet<i64> = selected
        .iter()
        .filter(|i| i.action != ItemAction::Hide.as_str())
        .filter_map(|i| i.catalog_id)
        .collect();
    let (rows, token) = {
        let s = lock(store)?;
        let mut rows = Vec::new();
        for id in &touched {
            rows.push(s.get_catalog(*id)?.ok_or_else(|| {
                IpcError::new(codes::E_NOTFOUND, format!("catalog {id} no longer exists; dismiss this card"))
            })?);
        }
        (rows, s.get_setting(crate::mcp::SETTING_TOKEN)?)
    };
    // R11: every touched catalog loaded and clean, or nothing happens.
    let mut pre: BTreeMap<i64, String> = BTreeMap::new();
    for row in &rows {
        registry::with_catalog_row(row, |_| Ok(()))?;
        let root = Path::new(&row.repo_path);
        if !repo::is_clean(root)? {
            return Err(IpcError::new(
                codes::E_INVALID_STATE,
                format!(
                    "catalog {} has uncommitted changes; commit them (catalog_admin commit_pending) \
                     or discard them before applying a card",
                    row.name
                ),
            ));
        }
        pre.insert(row.id, repo::head(root)?);
    }
    // 1. Snapshot the touched catalogs' host_layers.
    let snapshot: Vec<HostLayerRow> = lock(store)?
        .list_all_host_layers()?
        .into_iter()
        .filter(|r| touched.contains(&r.catalog_id))
        .collect();
    match run_steps(card, selected, &rows, token.as_deref(), store, ssh).await {
        Ok(commits) => finish_catalog_card(card, items, selected, &rows, &commits, &snapshot, store),
        Err(failure) => {
            // R12: every checkout back at its HEAD, host_layers back, the
            // failing group named.
            for row in &rows {
                if let Err(e) = repo::reset_hard(Path::new(&row.repo_path), &pre[&row.id]) {
                    tracing::error!(catalog = %row.name, "card {}: reset after a failed apply: {}", card.id, e.message);
                }
            }
            let msg = format!("{}: {}", failure.group, failure.error.message);
            let s = lock(store)?;
            for cid in &touched {
                s.restore_host_layers(*cid, &snapshot)?;
            }
            s.set_changeset_state(card.id, "failed", Some(&msg))?;
            Err(IpcError::new(&failure.error.code, msg))
        }
    }
}

/// Steps 2–4. Every error is a [`Failure`]; the caller resets.
async fn run_steps(
    card: &ChangesetRow,
    selected: &[&ChangesetItemRow],
    rows: &[CatalogRow],
    token: Option<&str>,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<BTreeMap<i64, String>, Failure> {
    let row_of = |id: i64| rows.iter().find(|r| r.id == id).expect("touched catalogs are resolved");

    // 2a. Imports — one remote read per (catalog, source host) (R13).
    let mut imports: BTreeMap<(i64, String), Vec<&ChangesetItemRow>> = BTreeMap::new();
    for &item in selected.iter().filter(|i| {
        i.action == ItemAction::Import.as_str() || i.action == ItemAction::TakeHost.as_str()
    }) {
        let p = ItemParams::parse(item.params.as_deref());
        let host = if item.action == ItemAction::TakeHost.as_str() { p.host } else { p.from_host };
        let (Some(host), Some(cid)) = (host, item.catalog_id) else {
            return Err(fail(
                item,
                IpcError::new(codes::E_INVALID, format!("{}/{} names no source host", item.kind, item.name)),
            ));
        };
        imports.entry((cid, host)).or_default().push(item);
    }
    let mut kept_scope: Vec<(&CatalogRow, Kind, String, Scope, &ChangesetItemRow)> = Vec::new();
    for ((cid, host), group) in &imports {
        let row = row_of(*cid);
        let root = Path::new(&row.repo_path);
        for &item in group.iter().filter(|i| i.action == ItemAction::TakeHost.as_str()) {
            let kind = kind_of(item).map_err(|e| fail(item, e))?;
            let old = repo::read_asset(root, kind, &item.name).map_err(|e| fail(item, e))?;
            kept_scope.push((row, kind, item.name.clone(), old.header.scope, item));
            repo::remove_asset(root, kind, &item.name).map_err(|e| fail(item, e))?;
        }
        let only: Vec<String> = group.iter().map(|i| format!("{}:{}", i.kind, i.name)).collect();
        let args = ImportArgs {
            host_alias: host.clone(),
            dry_run: false,
            only,
        };
        let report = crate::service::catalog::import_host_into(CatalogTarget::Row(row), args, store, ssh, token)
            .await
            .map_err(|e| fail(group[0], e))?;
        for &item in group {
            if !report.created.contains(&(item.kind.clone(), slugify(&item.name))) {
                let why: Vec<&str> = report.problems.iter().map(|p| p.message.as_str()).take(3).collect();
                let tail = if why.is_empty() { String::new() } else { format!(": {}", why.join("; ")) };
                return Err(fail(
                    item,
                    IpcError::new(
                        codes::E_INVALID,
                        format!("{}/{} was not imported from {host}{tail}", item.kind, item.name),
                    ),
                ));
            }
        }
    }
    for (row, kind, name, scope, item) in &kept_scope {
        set_scope(Path::new(&row.repo_path), *kind, name, *scope).map_err(|e| fail(item, e))?;
    }

    // 2b. Layer files: the members this card adds.
    let mut members: BTreeMap<(i64, String), BTreeSet<String>> = BTreeMap::new();
    for &item in selected.iter().filter(|i| i.action == ItemAction::Import.as_str()) {
        let p = ItemParams::parse(item.params.as_deref());
        if let (Some(cid), Some(layer), Some(member)) = (item.catalog_id, p.layer, p.member) {
            members.entry((cid, layer)).or_default().insert(member);
        }
    }
    for ((cid, layer), add) in &members {
        add_layer_members(Path::new(&row_of(*cid).repo_path), layer, add).map_err(|e| fail_in(layer, e))?;
    }

    // 2c. Scope.
    for &item in selected.iter().filter(|i| i.action == ItemAction::SetScope.as_str()) {
        let p = ItemParams::parse(item.params.as_deref());
        let row = row_of(item.catalog_id.unwrap_or_default());
        let member = p
            .member
            .clone()
            .unwrap_or_else(|| format!("{}/{}", item.kind, slugify(&item.name)));
        let (kind, name) = split_key(&member)
            .ok_or_else(|| fail(item, IpcError::new(codes::E_INVALID, format!("bad member {member}"))))?;
        let scope = match p.scope.as_deref() {
            Some("shared") => Scope::Shared,
            Some("private") => Scope::Private,
            other => {
                return Err(fail(item, IpcError::new(codes::E_INVALID, format!("bad scope {other:?}"))))
            }
        };
        set_scope(Path::new(&row.repo_path), kind, &name, scope).map_err(|e| fail(item, e))?;
    }

    // 3. host_layers: append this card's contexts, keeping each host's role
    //    and the order of what it had.
    {
        let s = lock(store).map_err(|e| fail_in("host layers", e))?;
        let mut adds: BTreeMap<(String, i64), Vec<String>> = BTreeMap::new();
        for &item in selected.iter().filter(|i| i.action == ItemAction::AssignLayer.as_str()) {
            let p = ItemParams::parse(item.params.as_deref());
            let (Some(host), Some(cid)) = (p.host, item.catalog_id) else {
                return Err(fail(item, IpcError::new(codes::E_INVALID, "assign_layer names no host")));
            };
            adds.entry((host, cid)).or_default().push(item.name.clone());
        }
        for ((host, cid), layers) in adds {
            let group = layers[0].clone();
            let current = s
                .get_host_layers_for(&host, cid)
                .map_err(|e| fail_in(&group, e.into()))?;
            let role = current.iter().find(|r| r.axis == "role").map(|r| r.layer_name.clone());
            let mut contexts: Vec<String> = current
                .iter()
                .filter(|r| r.axis == "context")
                .map(|r| r.layer_name.clone())
                .collect();
            for l in layers {
                if role.as_ref() != Some(&l) && !contexts.contains(&l) {
                    contexts.push(l);
                }
            }
            let ctx: Vec<&str> = contexts.iter().map(String::as_str).collect();
            s.set_host_layers_for(&host, cid, role.as_deref(), &ctx)
                .map_err(|e| fail_in(&group, e.into()))?;
        }
    }

    // 4. One commit per touched catalog (`fleet: <card summary>`).
    let mut commits = BTreeMap::new();
    for row in rows {
        let sha = commit_all(Path::new(&row.repo_path), &format!("fleet: {}", card.summary)).map_err(|e| {
            fail_in("commit", IpcError::new(&e.code, format!("catalog {}: {}", row.name, e.message)))
        })?;
        if let Some(sha) = sha {
            commits.insert(row.id, sha);
        }
    }
    Ok(commits)
}

/// Stage everything and commit it; `None` when nothing changed.
fn commit_all(root: &Path, message: &str) -> Result<Option<String>, IpcError> {
    repo::stage_paths(root, &[])?;
    if repo::has_staged(root, &[])? {
        Ok(Some(repo::commit(root, message)?))
    } else {
        Ok(None)
    }
}

fn set_scope(root: &Path, kind: Kind, name: &str, scope: Scope) -> Result<(), IpcError> {
    let mut a = repo::read_asset(root, kind, name)?;
    if a.header.scope != scope {
        a.header.scope = scope;
        repo::write_asset(root, &a, true)?;
    }
    Ok(())
}

/// Add `add` to `layers/<name>.yaml`, creating a context layer when there is
/// none (R6). Never removes a member.
fn add_layer_members(root: &Path, name: &str, add: &BTreeSet<String>) -> Result<(), IpcError> {
    check_layer_name(name)?;
    let path = root.join("layers").join(format!("{name}.yaml"));
    let mut layer = if path.is_file() {
        Layer::from_yaml(&std::fs::read_to_string(&path)?)
            .map_err(|e| IpcError::new(E_CATALOG_PARSE, format!("layers/{name}.yaml: {e}")))?
    } else {
        author::layer_template(name, Axis::Context)
    };
    for m in add {
        if !layer.members.contains(m) {
            layer.members.push(m.clone());
        }
    }
    layer.validate().map_err(|e| IpcError::new(codes::E_INVALID, e))?;
    std::fs::create_dir_all(root.join("layers"))?;
    std::fs::write(&path, layer.to_yaml())?;
    Ok(())
}

/// Step 5, after every commit landed: reload, push when `catalog.auto_push`
/// (R18), record verdicts for hide items, mark the items and the card, and
/// propose the follow-up Rollout (R14). Reload and push failures are
/// warnings on the card (R12).
fn finish_catalog_card(
    card: &ChangesetRow,
    items: &[ChangesetItemRow],
    selected: &[&ChangesetItemRow],
    rows: &[CatalogRow],
    commits: &BTreeMap<i64, String>,
    snapshot: &[HostLayerRow],
    store: &Mutex<Store>,
) -> Result<(), IpcError> {
    let mut warnings = Vec::new();
    for row in rows {
        if let Err(e) = crate::service::catalog::load_catalog(row.id, false, store) {
            warnings.push(format!("reload {}: {}", row.name, e.message));
        }
    }
    let auto_push = settings::get_bool(&*lock(store)?, settings::CATALOG_AUTO_PUSH);
    if auto_push {
        for row in rows.iter().filter(|r| commits.contains_key(&r.id)) {
            if let Err(e) = repo::push(Path::new(&row.repo_path)) {
                warnings.push(format!("push {}: {}", row.name, e.message));
            }
        }
    }
    let s = lock(store)?;
    let now = now_unix();
    for &item in selected.iter().filter(|i| i.action == ItemAction::Hide.as_str()) {
        let p = ItemParams::parse(item.params.as_deref());
        s.upsert_triage_verdict(&TriageVerdictRow {
            catalog_id: None,
            kind: item.kind.clone(),
            name: item.name.clone(),
            content_hash: p.hash.unwrap_or_else(|| "-".into()),
            verdict: "ignored".into(),
            decider: item.decider.clone(),
            decided_at: now,
        })?;
    }
    let applied: Vec<i64> = selected.iter().map(|i| i.position).collect();
    let rest: Vec<i64> = items
        .iter()
        .filter(|i| i.state == "pending" && !applied.contains(&i.position))
        .map(|i| i.position)
        .collect();
    s.set_changeset_item_states(card.id, &applied, "applied")?;
    s.set_changeset_item_states(card.id, &rest, "skipped")?;
    let commits_json: BTreeMap<String, String> =
        commits.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
    let encode = |e: serde_json::Error| IpcError::new(codes::E_SERIALIZE, e.to_string());
    let warning = (!warnings.is_empty()).then(|| warnings.join("; "));
    s.mark_changeset_applied(
        card.id,
        now,
        &serde_json::to_string(&commits_json).map_err(encode)?,
        &serde_json::to_string(snapshot).map_err(encode)?,
        warning.as_deref(),
    )?;
    if let Some((summary, follow)) = follow_up_rollout(selected, &s)? {
        s.insert_changeset(CardKind::Rollout.as_str(), &summary, &follow)?;
    }
    Ok(())
}

/// R14: the Rollout an applied catalog card calls for — per (host assigned
/// the layer, layer) the members it added; per other host managing a
/// take_host asset, that asset. `None` when no host needs anything.
pub(crate) fn follow_up_rollout(
    selected: &[&ChangesetItemRow],
    s: &Store,
) -> Result<Option<(String, Vec<NewChangesetItem>)>, IpcError> {
    let mut added: BTreeMap<(i64, String), BTreeSet<String>> = BTreeMap::new();
    let mut taken: BTreeMap<(i64, String), String> = BTreeMap::new();
    for &item in selected {
        let p = ItemParams::parse(item.params.as_deref());
        let Some(cid) = item.catalog_id else { continue };
        if item.action == ItemAction::Import.as_str() {
            if let (Some(layer), Some(member)) = (p.layer, p.member) {
                added.entry((cid, layer)).or_default().insert(member);
            }
        } else if item.action == ItemAction::TakeHost.as_str() {
            taken.insert((cid, format!("{}/{}", item.kind, item.name)), p.host.unwrap_or_default());
        }
    }
    let layers = s.list_all_host_layers()?;
    let inventory = if taken.is_empty() { Vec::new() } else { s.list_inventory()? };
    let sync = |grp: &str, cid: i64, host: String, params: ItemParams| NewChangesetItem {
        grp: grp.to_string(),
        catalog_id: Some(cid),
        kind: "host".into(),
        name: host,
        action: ItemAction::Sync.as_str().into(),
        params: params.to_json(),
        decider: Decider::Rule.as_str().into(),
    };
    let mut items = Vec::new();
    let mut hosts: BTreeSet<String> = BTreeSet::new();
    for ((cid, layer), assets) in &added {
        let assigned: BTreeSet<String> = layers
            .iter()
            .filter(|r| r.active && r.catalog_id == *cid && &r.layer_name == layer)
            .map(|r| r.host_alias.clone())
            .collect();
        for host in assigned {
            hosts.insert(host.clone());
            let params = ItemParams {
                layer: Some(layer.clone()),
                assets: assets.iter().cloned().collect(),
                ..Default::default()
            };
            items.push(sync(layer, *cid, host, params));
        }
    }
    for ((cid, key), source) in &taken {
        let (kind, name) = key.split_once('/').unwrap_or_default();
        let holders: BTreeSet<String> = inventory
            .iter()
            .filter(|r| r.managed && r.kind == kind && r.name == name && &r.host_alias != source)
            .map(|r| r.host_alias.clone())
            .collect();
        for host in holders {
            hosts.insert(host.clone());
            let params = ItemParams {
                assets: vec![key.clone()],
                ..Default::default()
            };
            items.push(sync(UPDATE, *cid, host, params));
        }
    }
    if items.is_empty() {
        return Ok(None);
    }
    let groups: BTreeSet<&str> = items.iter().map(|i| i.grp.as_str()).collect();
    let summary = format!(
        "Roll out {} to {}",
        groups.into_iter().collect::<Vec<_>>().join(", "),
        hosts.into_iter().collect::<Vec<_>>().join(", ")
    );
    Ok(Some((summary, items)))
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run, each on its own: `cargo test -p fleet-core is_clean_reset_hard_and_revert`; `cargo test -p fleet-core read_asset_reads_one_asset_back`; `cargo test -p fleet-core changesets::apply`.
Expected: PASS (6 apply tests). If the fake `ssh` cannot run `bash` on the test box, the import tests fail with `E_SSH` — that is an environment gap, not a code error; run them on mercury.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/service/catalog/repo.rs crates/fleet-core/src/service/catalog/changesets/
git commit -m "feat(changesets): apply catalog cards — one commit per catalog, nothing committed on failure"
```

---

### Task 7: Host cards — Rollout and Drift restore — and SB6's automatic additive sync

**Files:**
- Modify: `crates/fleet-core/src/service/catalog/changesets/apply.rs` (`OpFilter`, `op_allowed`, `narrow`, `sync_hosts`, `apply_rollout`, `apply_restore`, `finish_host_card`, `auto_additive`; `apply`'s dispatch; tests)
- Modify: `crates/fleet-core/src/service/catalog/changesets/reconcile.rs` (`after_scan_pass` takes `ssh` and spawns SB6; its two test calls)
- Modify: `crates/fleet-core/src/service/catalog/scan_tick.rs` (the hook passes `&ssh`)

**Interfaces:**
- Consumes: `sync::{plan_sync, PlanArgs, apply_sync_with, ApplyArgs}`, `sync::plan::{registry_put, registry_take_with_expiry, ActionOp, HostPlan, SyncPlan}`, `effective::effective_for_host_in`, `registry::snapshot`, `Store::rolled_out_layers`, `crate::rt::try_spawn`, Task 6's `apply`, `select_items`.
- Produces:

```rust
// changesets/apply.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub(crate) enum OpFilter { Additive, Restore }
pub(crate) fn op_allowed(f: OpFilter, op: ActionOp) -> bool;
pub(crate) fn narrow(hp: &mut HostPlan, f: OpFilter, assets: &BTreeSet<String>, catalogs: &BTreeSet<String>);
pub async fn auto_additive(store: &Mutex<Store>, ssh: &Arc<SshClient>) -> Result<usize, IpcError>; // hosts synced
// changesets/reconcile.rs
pub fn after_scan_pass(store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>);
```

- [ ] **Step 1: Write the failing tests**

`changesets/apply.rs` — a pure test module next to the unix one:

```rust
#[cfg(test)]
mod filter_tests {
    use super::*;
    use crate::service::catalog::sync::plan::Action;

    fn action(name: &str, op: ActionOp, catalog: Option<&str>) -> Action {
        let kind = if op == ActionOp::PluginInstall { "plugin_ref" } else { "skill" };
        Action {
            kind: kind.into(),
            name: name.into(),
            op,
            catalog: catalog.map(String::from),
            reason: None,
            files: vec![],
            merges: vec![],
            backup: false,
            secrets: vec![],
            missing_secrets: vec![],
            plan: None,
            expected: Default::default(),
            secret_files: Default::default(),
            remove_entry: None,
            plugin: None,
        }
    }

    /// R15: a Rollout keeps create, adopt and update for its own assets and
    /// catalogs only — never an overwrite, a remove or a plugin op; restore
    /// may overwrite, never remove.
    #[test]
    fn a_card_never_carries_an_overwrite_or_a_remove_to_a_host() {
        let mut hp = HostPlan {
            host_alias: "oci".into(),
            harness: "claude".into(),
            status: "planned".into(),
            detail: None,
            actions: vec![
                action("w", ActionOp::Create, Some("personal")),
                action("w2", ActionOp::Overwrite, Some("personal")),
                action("w3", ActionOp::Remove, None),
                action("w4", ActionOp::Update, Some("personal")),
                action("w5", ActionOp::Adopt, Some("acme")),
                action("other", ActionOp::Create, Some("personal")),
                action("p", ActionOp::PluginInstall, Some("personal")),
            ],
            snapshot: Default::default(),
            manifest: Default::default(),
        };
        let assets = BTreeSet::from(
            ["skill/w", "skill/w2", "skill/w3", "skill/w4", "skill/w5", "plugin_ref/p"].map(String::from),
        );
        narrow(&mut hp, OpFilter::Additive, &assets, &BTreeSet::from(["personal".to_string()]));
        assert_eq!(hp.actions.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["w", "w4"]);
        assert!(op_allowed(OpFilter::Restore, ActionOp::Overwrite));
        assert!(op_allowed(OpFilter::Restore, ActionOp::Update));
        assert!(!op_allowed(OpFilter::Restore, ActionOp::Remove));
        assert!(!op_allowed(OpFilter::Additive, ActionOp::Overwrite));
        assert!(!op_allowed(OpFilter::Additive, ActionOp::PluginInstall));
    }
}
```

and in the unix `tests` module:

```rust
    /// Spec, Rollout apply: plan_sync for the card's hosts + apply_sync —
    /// the skill lands on the host, the layer counts as rolled out, and SB6
    /// then puts a member back by itself (R16, R17).
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_rollout_installs_its_layer_and_sb6_keeps_it_there() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        let p = f.personal.id;
        let asset_yaml = format!("kind: skill\nname: w\ndescription: {DESC}\n");
        f.commit_files(
            &f.personal_root,
            p,
            &[
                ("skills/w/asset.yaml", asset_yaml.as_str()),
                ("skills/w/body.md", "Steps.\n"),
                ("layers/core.yaml", "kind: layer\nname: core\naxis: context\nmembers:\n- skill/w\n"),
            ],
        );
        f.store
            .lock()
            .unwrap()
            .set_host_layers_for("oci", p, None, &["core"])
            .unwrap();
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        assert_eq!(auto_additive(&f.store, &ssh).await.unwrap(), 0, "nothing rolled out yet: SB6 waits");

        let card = f
            .store
            .lock()
            .unwrap()
            .insert_changeset(
                "rollout",
                "Roll out core to oci",
                &[item(
                    "core",
                    Some(p),
                    "host",
                    "oci",
                    ItemAction::Sync,
                    ItemParams {
                        layer: Some("core".into()),
                        assets: vec!["skill/w".into()],
                        ..Default::default()
                    },
                )],
            )
            .unwrap();
        let v = apply(ApplyArgs { id: card.id, positions: None }, &f.store, &ssh)
            .await
            .unwrap();
        assert_eq!(v.state, "applied", "{:?}", v.error);
        let installed = home.path().join(".claude/skills/w/SKILL.md");
        assert!(installed.is_file(), "the rollout installed the skill");
        assert_eq!(
            f.store.lock().unwrap().rolled_out_layers().unwrap(),
            BTreeSet::from([(p, "core".to_string())])
        );

        std::fs::remove_dir_all(home.path().join(".claude/skills/w")).unwrap();
        let mut missing = f.store.lock().unwrap().list_inventory().unwrap();
        missing.retain(|r| r.host_alias == "oci" && r.harness == "claude");
        for r in &mut missing {
            if r.name == "w" {
                r.state = "missing".into();
            }
        }
        f.store
            .lock()
            .unwrap()
            .replace_host_inventory("oci", "claude", &missing)
            .unwrap();
        assert_eq!(auto_additive(&f.store, &ssh).await.unwrap(), 1);
        assert!(installed.is_file(), "SB6 put the member back");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p fleet-core a_card_never_carries_an_overwrite_or_a_remove_to_a_host` — Expected: compile error, `narrow` not found.

- [ ] **Step 3: Write the implementation**

`changesets/apply.rs` — imports gain:

```rust
use crate::service::catalog::effective;
use crate::service::catalog::sync::plan::{ActionOp, HostPlan, SyncPlan};
use crate::service::catalog::sync::{self, ApplyArgs as SyncApplyArgs, PlanArgs};
use tokio_util::sync::CancellationToken;
```

`apply`'s body, from `let selected = …` on, becomes:

```rust
    let selected = select_items(&card, &items, args.positions.as_deref())?;
    if card.kind == CardKind::Rollout.as_str() {
        apply_rollout(&card, &items, &selected, store, ssh).await?;
    } else if super::writes_hosts(&card, &items, args.positions.as_deref()) {
        apply_restore(&card, &items, selected[0], store, ssh).await?;
    } else {
        apply_catalog(&card, &items, &selected, store, ssh).await?;
    }
    super::get(args.id, store)
```

and the module doc gains: "A host card (rollout, drift restore) plans the card's hosts and applies only what R15 allows; SB6's automatic additive sync shares that path (`auto_additive`)."

Add:

```rust
/// R15: which sync ops a card may carry to a host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpFilter {
    /// Rollout and SB6: create, adopt, update (the applier backs up every
    /// file it replaces).
    Additive,
    /// Drift restore, one asset a person picked: update or overwrite.
    Restore,
}

pub(crate) fn op_allowed(f: OpFilter, op: ActionOp) -> bool {
    match f {
        OpFilter::Additive => matches!(op, ActionOp::Create | ActionOp::Adopt | ActionOp::Update),
        OpFilter::Restore => matches!(op, ActionOp::Update | ActionOp::Overwrite),
    }
}

/// Keep only what the card may apply on this host: an allowed op, for one
/// of `assets` (`<kind>/<name>`), from one of `catalogs`.
pub(crate) fn narrow(hp: &mut HostPlan, f: OpFilter, assets: &BTreeSet<String>, catalogs: &BTreeSet<String>) {
    hp.actions.retain(|a| {
        op_allowed(f, a.op)
            && assets.contains(&format!("{}/{}", a.kind, a.name))
            && a.catalog.as_ref().is_some_and(|c| catalogs.contains(c))
    });
}

/// Plan each host in `wants`, narrow to what the card may write, park the
/// result as one plan and apply it under its own token (R27). Per host:
/// `Ok` when every (host, harness) pair applied, else why.
async fn sync_hosts(
    wants: &BTreeMap<String, BTreeSet<String>>,
    catalogs: &BTreeSet<String>,
    filter: OpFilter,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<BTreeMap<String, Result<(), String>>, IpcError> {
    let mut plans: Vec<HostPlan> = Vec::new();
    for (host, assets) in wants {
        let args = PlanArgs {
            host_alias: Some(host.clone()),
            ..Default::default()
        };
        let planned = sync::plan_sync(args, store, ssh).await?;
        let Some((_, parked)) = sync::plan::registry_take_with_expiry(&planned.id) else {
            continue;
        };
        for mut hp in parked.hosts {
            narrow(&mut hp, filter, assets, catalogs);
            plans.push(hp);
        }
    }
    let plan_id = sync::plan::registry_put(SyncPlan::new(plans));
    let args = SyncApplyArgs {
        plan_id,
        force_partial: false,
        call_id: None,
    };
    let run = sync::apply_sync_with(args, store, ssh, CancellationToken::new()).await?;
    let mut out: BTreeMap<String, Result<(), String>> =
        wants.keys().map(|h| (h.clone(), Ok(()))).collect();
    for r in &run.hosts {
        if r.status != "applied" {
            let why = r.detail.clone().unwrap_or_else(|| r.status.clone());
            out.insert(r.host_alias.clone(), Err(format!("{} ({}): {why}", r.host_alias, r.harness)));
        }
    }
    Ok(out)
}

/// The catalog labels (`personal` for personal) the items name.
fn catalog_labels(items: &[&ChangesetItemRow], store: &Mutex<Store>) -> Result<BTreeSet<String>, IpcError> {
    let s = lock(store)?;
    let mut out = BTreeSet::new();
    for id in items.iter().filter_map(|i| i.catalog_id).collect::<BTreeSet<_>>() {
        if let Some(r) = s.get_catalog(id)? {
            out.insert(if r.org_id.is_none() { "personal".to_string() } else { r.name });
        }
    }
    Ok(out)
}

/// Spec, Rollout apply: `plan_sync` (the card's hosts) + `apply_sync`,
/// additive only (R15).
async fn apply_rollout(
    card: &ChangesetRow,
    items: &[ChangesetItemRow],
    selected: &[&ChangesetItemRow],
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<(), IpcError> {
    let catalogs = catalog_labels(selected, store)?;
    let mut wants: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for &item in selected {
        wants
            .entry(item.name.clone())
            .or_default()
            .extend(ItemParams::parse(item.params.as_deref()).assets);
    }
    let outcome = match sync_hosts(&wants, &catalogs, OpFilter::Additive, store, ssh).await {
        Ok(o) => o,
        Err(e) => {
            lock(store)?.set_changeset_state(card.id, "failed", Some(&e.message))?;
            return Err(e);
        }
    };
    finish_host_card(card, items, selected, &outcome, |i| i.name.clone(), store)
}

/// Drift restore: the catalog copy back onto one host, with backup (R15).
async fn apply_restore(
    card: &ChangesetRow,
    items: &[ChangesetItemRow],
    item: &ChangesetItemRow,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<(), IpcError> {
    let host = ItemParams::parse(item.params.as_deref())
        .host
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "restore names no host"))?;
    let catalogs = catalog_labels(&[item], store)?;
    let wants = BTreeMap::from([(host.clone(), BTreeSet::from([format!("{}/{}", item.kind, item.name)]))]);
    let outcome = match sync_hosts(&wants, &catalogs, OpFilter::Restore, store, ssh).await {
        Ok(o) => o,
        Err(e) => {
            lock(store)?.set_changeset_state(card.id, "failed", Some(&e.message))?;
            return Err(e);
        }
    };
    finish_host_card(card, items, &[item], &outcome, move |_| host.clone(), store)
}

/// R15: items of hosts that applied are `applied`; the card is `applied`
/// when every host did (the rest of its pending items `skipped`), else
/// `failed` naming the hosts that did not — answered as the card's view,
/// not as an error, since some hosts did change.
fn finish_host_card(
    card: &ChangesetRow,
    items: &[ChangesetItemRow],
    selected: &[&ChangesetItemRow],
    outcome: &BTreeMap<String, Result<(), String>>,
    host_of: impl Fn(&ChangesetItemRow) -> String,
    store: &Mutex<Store>,
) -> Result<(), IpcError> {
    let done: Vec<i64> = selected
        .iter()
        .filter(|i| matches!(outcome.get(&host_of(i)), Some(Ok(()))))
        .map(|i| i.position)
        .collect();
    let failures: Vec<String> = outcome.values().filter_map(|r| r.as_ref().err().cloned()).collect();
    let s = lock(store)?;
    s.set_changeset_item_states(card.id, &done, "applied")?;
    if failures.is_empty() {
        let rest: Vec<i64> = items
            .iter()
            .filter(|i| i.state == "pending" && !done.contains(&i.position))
            .map(|i| i.position)
            .collect();
        s.set_changeset_item_states(card.id, &rest, "skipped")?;
        s.mark_changeset_applied(card.id, now_unix(), "{}", "[]", None)?;
    } else {
        s.set_changeset_state(card.id, "failed", Some(&failures.join("; ")))?;
    }
    Ok(())
}

/// SB6 (R17): with no card, sync additively what a rolled-out layer
/// introduced and a host lacks (`missing`), has differently (`drifted`) or
/// has unmanaged. Answers how many hosts applied cleanly. A fleet with no
/// applied Rollout answers 0 without planning anything.
pub async fn auto_additive(store: &Mutex<Store>, ssh: &Arc<SshClient>) -> Result<usize, IpcError> {
    let (rolled, rows, hosts, configured) = {
        let s = lock(store)?;
        (s.rolled_out_layers()?, s.list_inventory()?, s.list_hosts()?, s.list_catalogs()?)
    };
    if rolled.is_empty() {
        return Ok(0);
    }
    let snapshot = registry::snapshot()?;
    let id_of: BTreeMap<String, i64> = configured
        .iter()
        .map(|r| {
            let label = if r.org_id.is_none() { "personal".to_string() } else { r.name.clone() };
            (label, r.id)
        })
        .collect();
    let mut wants: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut catalogs: BTreeSet<String> = BTreeSet::new();
    for h in hosts.iter().filter(|h| !h.hidden && (h.reachable || h.alias == "local")) {
        let Ok(eff) = effective::effective_for_host_in(store, &h.alias, &snapshot) else {
            continue;
        };
        for (key, prov) in &eff.provenance {
            let Some(cid) = id_of.get(&prov.catalog) else { continue };
            if !rolled.contains(&(*cid, prov.introduced_by.clone())) {
                continue;
            }
            let Some((kind, name)) = key.split_once('/') else { continue };
            let due = rows
                .iter()
                .find(|r| r.host_alias == h.alias && r.harness == "claude" && r.kind == kind && r.name == name)
                .is_some_and(|r| {
                    matches!(r.state.as_str(), "missing" | "drifted") || (r.state == "in_sync" && !r.managed)
                });
            if due {
                wants.entry(h.alias.clone()).or_default().insert(key.clone());
                catalogs.insert(prov.catalog.clone());
            }
        }
    }
    if wants.is_empty() {
        return Ok(0);
    }
    let outcome = sync_hosts(&wants, &catalogs, OpFilter::Additive, store, ssh).await?;
    Ok(outcome.values().filter(|r| r.is_ok()).count())
}
```

`changesets/reconcile.rs` — `after_scan_pass` becomes:

```rust
/// The scan tick's hook (R19, carry 4): with `catalog.auto` on, one pass —
/// skipped, never awaited, while an apply holds `APPLY_LOCK` — then SB6's
/// additive sync, detached (`rt::try_spawn`) so the tick never waits for
/// SSH, and skipped likewise when the lock is taken by then (R17). Errors
/// are logged; the tick's `owed`/`seen` are never touched.
pub fn after_scan_pass(store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>) {
    let auto = store
        .lock()
        .map(|s| settings::get_bool(&s, settings::CATALOG_AUTO))
        .unwrap_or(false);
    if !auto {
        return;
    }
    {
        let Ok(_busy) = APPLY_LOCK.try_lock() else {
            tracing::debug!("changesets: an apply is running; this pass's reconcile is skipped");
            return;
        };
        match reconcile(store, true) {
            Ok(r) if r != ReconcileReport::default() => tracing::info!(
                inserted = r.inserted,
                refreshed = r.refreshed,
                withdrawn = r.withdrawn,
                hidden = r.hidden,
                "changesets: reconciled"
            ),
            Ok(_) => {}
            Err(e) => tracing::warn!("changesets: reconcile failed: {}", e.message),
        }
    }
    let (store, ssh) = (Arc::clone(store), Arc::clone(ssh));
    crate::rt::try_spawn(async move {
        let Ok(_busy) = APPLY_LOCK.try_lock() else {
            return;
        };
        match super::apply::auto_additive(&store, &ssh).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(hosts = n, "catalog.auto: additive sync applied"),
            Err(e) => tracing::warn!("catalog.auto: additive sync failed: {}", e.message),
        }
    });
}
```

with `use crate::ssh::SshClient;` added; in its tests, both `after_scan_pass(&store)` calls become `after_scan_pass(&store, &Arc::new(crate::ssh::SshClient::new()))` (no rollout is applied there, so SB6 answers 0 without planning).

`scan_tick.rs` — the hook line becomes `super::changesets::reconcile::after_scan_pass(&store, &ssh);`.

- [ ] **Step 4: Run the tests to verify they pass**

Run, each on its own: `cargo test -p fleet-core a_card_never_carries_an_overwrite_or_a_remove_to_a_host`; `cargo test -p fleet-core a_rollout_installs_its_layer_and_sb6_keeps_it_there`; `cargo test -p fleet-core changesets::`; `cargo test -p fleet-core scan_tick`.
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/service/catalog/changesets/ crates/fleet-core/src/service/catalog/scan_tick.rs
git commit -m "feat(changesets): rollout and restore cards; catalog.auto's additive sync on rolled-out layers"
```

---

### Task 8: Undo, dismiss and reject_item

**Files:**
- Create: `crates/fleet-core/src/service/catalog/changesets/undo.rs` (+ tests)
- Modify: `crates/fleet-core/src/service/catalog/changesets/mod.rs` (`pub mod undo;`)

**Interfaces:**
- Consumes: Task 5's `card`, `get`, `is_open`, `changes_catalog`, `applied_catalogs`, `later_card`, `APPLY_LOCK`; Task 6's `follow_up_rollout`, `repo::{is_clean, head, revert, reset_hard, push}`; `Store::restore_host_layers`; `load_catalog`.
- Produces:

```rust
pub async fn undo(id: i64, store: &Mutex<Store>) -> Result<ChangesetView, IpcError>;
pub async fn dismiss(id: i64, store: &Mutex<Store>) -> Result<ChangesetView, IpcError>;
pub async fn reject_items(id: i64, positions: &[i64], store: &Mutex<Store>) -> Result<ChangesetView, IpcError>;
```

- [ ] **Step 1: Write the failing tests**

`changesets/undo.rs` — the tests module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::catalog::changesets::testkit::*;
    use crate::service::catalog::lock_registry_for_test;
    use std::sync::Arc;

    fn store_with_personal() -> (Mutex<Store>, i64) {
        let s = Store::open_in_memory().unwrap();
        s.set_catalog_config("/p", None).unwrap();
        let p = s.personal_catalog().unwrap().unwrap().id;
        (Mutex::new(s), p)
    }

    /// R9: dismiss rejects every pending item and holds its subjects by
    /// content hash — set_scope carries none; a person's verdict.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn dismiss_rejects_every_pending_item_and_holds_its_subjects() {
        let _g = lock_registry_for_test();
        let (store, p) = store_with_personal();
        let card = store
            .lock()
            .unwrap()
            .insert_changeset(
                "new",
                "New on oci: skill/w → core",
                &[
                    item("core", Some(p), "skill", "w", ItemAction::Import, ItemParams {
                        from_host: Some("oci".into()),
                        hash: Some("h1".into()),
                        ..Default::default()
                    }),
                    item("core", Some(p), "skill", "w", ItemAction::SetScope, ItemParams {
                        scope: Some("shared".into()),
                        ..Default::default()
                    }),
                ],
            )
            .unwrap();
        let v = dismiss(card.id, &store).await.unwrap();
        assert_eq!(v.state, "dismissed");
        assert!(v.items.iter().all(|i| i.state == "rejected"));
        let verdicts = store.lock().unwrap().triage_verdicts().unwrap();
        assert_eq!(verdicts.len(), 1, "set_scope carries no subject");
        assert_eq!(
            (verdicts[0].content_hash.as_str(), verdicts[0].verdict.as_str(), verdicts[0].decider.as_str()),
            ("h1", "rejected", "person")
        );
        assert_eq!(dismiss(card.id, &store).await.unwrap_err().code, codes::E_INVALID_STATE);
    }

    /// R9: rejecting one drift item leaves the other to apply; the subject
    /// is held only once both are rejected.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_drift_item_rejected_alone_holds_nothing_until_both_are() {
        let _g = lock_registry_for_test();
        let (store, p) = store_with_personal();
        let drift = |action| {
            item("drift", Some(p), "skill", "w", action, ItemParams {
                host: Some("trn".into()),
                hash: Some("e".into()),
                ..Default::default()
            })
        };
        let card = store
            .lock()
            .unwrap()
            .insert_changeset("drift", "skill/w differs on trn", &[drift(ItemAction::TakeHost), drift(ItemAction::Restore)])
            .unwrap();
        let v = reject_items(card.id, &[0], &store).await.unwrap();
        assert_eq!((v.state.as_str(), v.items[0].state.as_str()), ("proposed", "rejected"));
        assert!(store.lock().unwrap().triage_verdicts().unwrap().is_empty());
        let v = reject_items(card.id, &[1], &store).await.unwrap();
        assert_eq!(v.state, "dismissed");
        let verdicts = store.lock().unwrap().triage_verdicts().unwrap();
        assert_eq!((verdicts.len(), verdicts[0].content_hash.as_str()), (1, "e"));
    }

    #[cfg(unix)]
    const DESC: &str = "A reasonably long description here.";

    /// Apply a card importing `name` from oci into personal's `core` and
    /// assigning `core` to oci; answer its id.
    #[cfg(unix)]
    async fn applied_card(f: &Fleet, ssh: &Arc<crate::ssh::SshClient>, name: &str) -> i64 {
        let p = f.personal.id;
        let card = f
            .store
            .lock()
            .unwrap()
            .insert_changeset(
                "new",
                &format!("Adopt {name}"),
                &[
                    item("core", Some(p), "skill", name, ItemAction::Import, ItemParams {
                        from_host: Some("oci".into()),
                        layer: Some("core".into()),
                        member: Some(format!("skill/{name}")),
                        ..Default::default()
                    }),
                    item("core", Some(p), "layer", "core", ItemAction::AssignLayer, ItemParams {
                        host: Some("oci".into()),
                        layer: Some("core".into()),
                        axis: Some("context".into()),
                        ..Default::default()
                    }),
                ],
            )
            .unwrap();
        let args = super::super::apply::ApplyArgs { id: card.id, positions: None };
        let v = super::super::apply::apply(args, &f.store, ssh).await.unwrap();
        assert_eq!(v.state, "applied", "{:?}", v.error);
        card.id
    }

    #[cfg(unix)]
    fn contexts(f: &Fleet) -> Vec<String> {
        f.store
            .lock()
            .unwrap()
            .get_host_layers_for("oci", f.personal.id)
            .unwrap()
            .into_iter()
            .map(|r| r.layer_name)
            .collect()
    }

    /// Spec, Testing: "undo reverts them and restores `host_layers`".
    #[cfg(unix)]
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn undo_reverts_the_commit_and_restores_host_layers() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        host_skill(home.path(), "w", DESC);
        let ssh = ssh_with_home(bin.path(), home.path());
        let id = applied_card(&f, &ssh, "w").await;
        assert!(super::super::get(id, &f.store).unwrap().undoable);
        assert_eq!(contexts(&f), ["core"]);

        let v = undo(id, &f.store).await.unwrap();
        assert_eq!(v.state, "undone", "{:?}", v.error);
        assert!(subjects(&f.personal_root)[0].starts_with("Revert \"fleet: Adopt w\""));
        assert!(!f.personal_root.join("skills/w").exists());
        assert!(!f.personal_root.join("layers/core.yaml").exists());
        assert!(contexts(&f).is_empty(), "host_layers back to the snapshot");
        assert!(crate::service::catalog::registry::with_catalog_row(&f.personal, |c| {
            Ok(c.find(crate::service::catalog::model::Kind::Skill, "w").is_none())
        })
        .unwrap());
        assert_eq!(undo(id, &f.store).await.unwrap_err().code, codes::E_INVALID_STATE);
    }

    /// SB5: only the latest applied card per catalog.
    #[cfg(unix)]
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn only_the_latest_applied_card_per_catalog_can_be_undone() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        host_skill(home.path(), "w", DESC);
        host_skill(home.path(), "v", DESC);
        let ssh = ssh_with_home(bin.path(), home.path());
        let a = applied_card(&f, &ssh, "w").await;
        let b = applied_card(&f, &ssh, "v").await;
        assert!(!super::super::get(a, &f.store).unwrap().undoable);
        let err = undo(a, &f.store).await.unwrap_err();
        assert_eq!(err.code, codes::E_INVALID_STATE);
        assert!(err.message.contains(&format!("undo #{b} first")), "{}", err.message);
        undo(b, &f.store).await.unwrap();
        assert_eq!(contexts(&f), ["core"], "b's snapshot: core was already there");
        undo(a, &f.store).await.unwrap();
        assert!(contexts(&f).is_empty());
    }

    /// R20: a revert that conflicts with a later hand-made commit changes
    /// nothing — HEAD, tree, host_layers and the card stay.
    #[cfg(unix)]
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_revert_conflict_changes_nothing() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        host_skill(home.path(), "w", DESC);
        let ssh = ssh_with_home(bin.path(), home.path());
        let id = applied_card(&f, &ssh, "w").await;
        std::fs::write(f.personal_root.join("skills/w/body.md"), "Edited by hand.\n").unwrap();
        git(&f.personal_root, &["commit", "-qam", "hand edit"]);
        let before = head(&f.personal_root);

        let err = undo(id, &f.store).await.unwrap_err();
        assert!(err.message.contains("nothing was changed"), "{}", err.message);
        assert_eq!(head(&f.personal_root), before);
        assert!(git(&f.personal_root, &["status", "--porcelain"]).is_empty());
        assert_eq!(super::super::get(id, &f.store).unwrap().state, "applied");
        assert_eq!(contexts(&f), ["core"]);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p fleet-core changesets::undo` — Expected: compile error, module `undo` not found.

- [ ] **Step 3: Write the implementation**

`changesets/mod.rs`: `pub mod undo;` next to `pub mod apply;`.

`changesets/undo.rs` (above its tests):

```rust
//! Assets M4: undo (SB5) — `git revert` of a card's commits plus its
//! `host_layers` snapshot, only for the latest applied card in each catalog
//! it touched (R20) — and the person's no: dismiss and reject_item, which
//! hold their subjects by content hash (R9, R10).

use super::apply::follow_up_rollout;
use super::{
    applied_catalogs, changes_catalog, is_open, later_card, CardKind, ChangesetView, Decider,
    ItemAction, ItemParams, APPLY_LOCK,
};
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::catalog::repo;
use crate::service::settings;
use crate::store::{now_unix, ChangesetItemRow, ChangesetRow, HostLayerRow, Store, TriageVerdictRow};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

/// Undo card `id`. Never touches hosts (spec); an undone take_host proposes
/// the Rollout that puts the restored copy back (R14).
pub async fn undo(id: i64, store: &Mutex<Store>) -> Result<ChangesetView, IpcError> {
    let _busy = APPLY_LOCK.lock().await;
    let (card, items) = super::card(id, store)?;
    if card.state != "applied" || !changes_catalog(&card, &items) {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!(
                "card {id} is a {} card in state {}; only an applied bootstrap, new or take_host card can be undone",
                card.kind, card.state
            ),
        ));
    }
    let touched = applied_catalogs(&items);
    let rows = {
        let s = lock(store)?;
        if let Some((later, catalog)) = later_card(&card, &items, &s)? {
            return Err(IpcError::new(
                codes::E_INVALID_STATE,
                format!("card #{later} was applied after it in catalog {catalog}; undo #{later} first"),
            ));
        }
        let mut rows = Vec::new();
        for cid in &touched {
            rows.push(s.get_catalog(*cid)?.ok_or_else(|| {
                IpcError::new(codes::E_NOTFOUND, format!("catalog {cid} no longer exists; card {id} cannot be undone"))
            })?);
        }
        rows
    };
    let commits: BTreeMap<String, String> = card
        .commits
        .as_deref()
        .and_then(|c| serde_json::from_str(c).ok())
        .unwrap_or_default();
    let mut pre: BTreeMap<i64, String> = BTreeMap::new();
    for row in &rows {
        let root = Path::new(&row.repo_path);
        if !repo::is_clean(root)? {
            return Err(IpcError::new(
                codes::E_INVALID_STATE,
                format!("catalog {} has uncommitted changes; commit or discard them before undoing a card", row.name),
            ));
        }
        pre.insert(row.id, repo::head(root)?);
    }
    let mut reverted = Vec::new();
    for row in &rows {
        let Some(sha) = commits.get(&row.id.to_string()) else { continue };
        if let Err(e) = repo::revert(Path::new(&row.repo_path), sha) {
            for done in &reverted {
                if let Err(r) = repo::reset_hard(Path::new(&done.repo_path), &pre[&done.id]) {
                    tracing::error!(catalog = %done.name, "undo card {id}: reset after a failed revert: {}", r.message);
                }
            }
            return Err(IpcError::new(
                &e.code,
                format!("undo card {id}: catalog {}: {}; nothing was changed", row.name, e.message),
            ));
        }
        reverted.push(row);
    }
    let snapshot: Vec<HostLayerRow> = serde_json::from_str(card.layers_snapshot.as_deref().unwrap_or("[]"))
        .map_err(|e| IpcError::new(codes::E_PARSE, format!("card {id}'s layer snapshot: {e}")))?;
    {
        let s = lock(store)?;
        for cid in &touched {
            s.restore_host_layers(*cid, &snapshot)?;
        }
        s.set_changeset_state(id, "undone", None)?;
    }
    let mut warnings = Vec::new();
    for row in &rows {
        if let Err(e) = crate::service::catalog::load_catalog(row.id, false, store) {
            warnings.push(format!("reload {}: {}", row.name, e.message));
        }
    }
    if settings::get_bool(&*lock(store)?, settings::CATALOG_AUTO_PUSH) {
        for row in &reverted {
            if let Err(e) = repo::push(Path::new(&row.repo_path)) {
                warnings.push(format!("push {}: {}", row.name, e.message));
            }
        }
    }
    let taken: Vec<&ChangesetItemRow> = items
        .iter()
        .filter(|i| i.state == "applied" && i.action == ItemAction::TakeHost.as_str())
        .collect();
    {
        let s = lock(store)?;
        if !warnings.is_empty() {
            s.set_changeset_state(id, "undone", Some(&warnings.join("; ")))?;
        }
        if let Some((summary, follow)) = follow_up_rollout(&taken, &s)? {
            s.insert_changeset(CardKind::Rollout.as_str(), &summary, &follow)?;
        }
    }
    super::get(id, store)
}

/// A person's no to the whole card: every pending item rejected.
pub async fn dismiss(id: i64, store: &Mutex<Store>) -> Result<ChangesetView, IpcError> {
    let _busy = APPLY_LOCK.lock().await;
    let (card, items) = super::card(id, store)?;
    let pending: Vec<i64> = items.iter().filter(|i| i.state == "pending").map(|i| i.position).collect();
    reject_locked(&card, &items, &pending, store)?;
    super::get(id, store)
}

/// A person's no to some items; the card closes when nothing is pending.
pub async fn reject_items(id: i64, positions: &[i64], store: &Mutex<Store>) -> Result<ChangesetView, IpcError> {
    let _busy = APPLY_LOCK.lock().await;
    let (card, items) = super::card(id, store)?;
    for p in positions {
        match items.iter().find(|i| i.position == *p) {
            None => return Err(IpcError::new(codes::E_NOTFOUND, format!("card {id} has no item {p}"))),
            Some(i) if i.state != "pending" => {
                return Err(IpcError::new(codes::E_INVALID_STATE, format!("item {p} of card {id} is {}", i.state)))
            }
            Some(_) => {}
        }
    }
    reject_locked(&card, &items, positions, store)?;
    super::get(id, store)
}

fn reject_locked(
    card: &ChangesetRow,
    items: &[ChangesetItemRow],
    positions: &[i64],
    store: &Mutex<Store>,
) -> Result<(), IpcError> {
    if !is_open(&card.state) {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!("card {} is {}; only a proposed or failed card can be dismissed", card.id, card.state),
        ));
    }
    let closing = !items
        .iter()
        .any(|i| i.state == "pending" && !positions.contains(&i.position));
    let s = lock(store)?;
    let now = now_unix();
    for item in items.iter().filter(|i| positions.contains(&i.position)) {
        if let Some(v) = verdict_for(item, closing, now) {
            s.upsert_triage_verdict(&v)?;
        }
    }
    s.set_changeset_item_states(card.id, positions, "rejected")?;
    if closing {
        s.set_changeset_state(card.id, "dismissed", None)?;
    }
    Ok(())
}

/// R9: the verdict a rejected item leaves, if its action has a subject.
fn verdict_for(item: &ChangesetItemRow, closing: bool, now: i64) -> Option<TriageVerdictRow> {
    let (kind, name) = match item.action.as_str() {
        "import" | "hide" => (item.kind.clone(), item.name.clone()),
        "take_host" | "restore" if closing => (item.kind.clone(), item.name.clone()),
        "sync" => ("layer".to_string(), format!("{}/{}", item.catalog_id.unwrap_or(0), item.grp)),
        _ => return None,
    };
    Some(TriageVerdictRow {
        catalog_id: item.catalog_id,
        kind,
        name,
        content_hash: ItemParams::parse(item.params.as_deref())
            .hash
            .unwrap_or_else(|| "-".into()),
        verdict: "rejected".into(),
        decider: Decider::Person.as_str().into(),
        decided_at: now,
    })
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p fleet-core changesets::undo` — Expected: PASS (5 tests; the 3 git ones are `cfg(unix)`).

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/service/catalog/changesets/
git commit -m "feat(changesets): undo the latest applied card per catalog; dismiss and reject_item hold their subjects"
```

---

### Task 9: The MCP `changesets` tool and its grants

**Files:**
- Modify: `crates/fleet-core/src/mcp/tools/assets.rs` (the tool; `check_card_grants`; `changesets_forbidden`)
- Modify: `crates/fleet-core/src/mcp/tools/params.rs` (`ChangesetsParams`)
- Modify: `crates/fleet-core/src/mcp/guard.rs` (the policy row)
- Create: `crates/fleet-core/src/mcp/tools/tests_changesets.rs`
- Modify: `crates/fleet-core/src/mcp/tools/mod.rs` (`#[cfg(test)] mod tests_changesets;`)
- Modify: `crates/fleet-core/src/mcp/tools/tests.rs` (`BUDGET_BYTES`, measured)
- Modify: `docs/control-api.md`; `docs/control-api-reference.md` (generated)

**Interfaces:**
- Consumes: Task 3's `may_admin_catalog`, `may_admin_catalog_row`; Tasks 5–8's `changesets::{list, get, card, propose, writes_hosts}`, `apply::{apply, ApplyArgs}`, `undo::{undo, dismiss, reject_items}`; `confirm_gate`.
- Produces:

```rust
// mcp/tools/params.rs
#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ChangesetsParams { pub action: String, #[serde(default)] pub id: Option<i64>,
    #[serde(default)] pub positions: Option<Vec<i64>>, #[serde(default)] pub confirm_nonce: Option<String> }
// mcp/tools/assets.rs (FleetTools)
pub(super) async fn changesets(&self, Extension(caller): Extension<Caller>, Parameters(p): Parameters<ChangesetsParams>) -> Result<CallToolResult, McpError>;
fn check_card_grants(&self, caller: &Caller, action: &str, items: &[ChangesetItemRow], writes_hosts: bool) -> Result<(), McpError>;
```

- [ ] **Step 1: Write the failing tests**

`mcp/tools/tests_changesets.rs`:

```rust
//! `changesets` driven the way `call_tool` drives it — spec, Testing
//! (authorization): "an ungranted client and a per-host token cannot apply,
//! undo or admit in a catalog they have no grant for, but can list" (R25).

use super::*;
use crate::store::NewChangesetItem;

fn tools(s: Store) -> FleetTools {
    FleetTools::new(
        Arc::new(Mutex::new(s)),
        Arc::new(SshClient::new()),
        CancellationRegistry::new(),
        Arc::new(crate::service::tunnel::TunnelSupervisor::new()),
        McpGuards::new(Arc::new(|_: &guard::ConfirmRequest| {})),
    )
}

fn client(id: i64, mode: TokenMode) -> Caller {
    Caller {
        host_alias: None,
        client: Some(crate::mcp::auth::ClientRef {
            id,
            name: format!("client-{id}"),
            trusted: false,
            org_id: None,
        }),
        mode,
    }
}

fn host(alias: &str) -> Caller {
    Caller {
        host_alias: Some(alias.into()),
        client: None,
        mode: TokenMode::Full,
    }
}

async fn call(
    t: &FleetTools,
    caller: &Caller,
    action: &str,
    id: Option<i64>,
    positions: Option<Vec<i64>>,
) -> Result<CallToolResult, McpError> {
    enforce_mode(caller, "changesets")?;
    enforce_admin(caller, "changesets")?;
    t.changesets(
        Extension(caller.clone()),
        Parameters(ChangesetsParams {
            action: action.into(),
            id,
            positions,
            confirm_nonce: None,
        }),
    )
    .await
}

fn code_of(r: &Result<CallToolResult, McpError>) -> String {
    match r {
        Ok(_) => "OK".into(),
        Err(e) => e.message.split(':').next().unwrap_or_default().to_string(),
    }
}

fn message_of(r: Result<CallToolResult, McpError>) -> String {
    r.err().map(|e| e.message.to_string()).unwrap_or_default()
}

/// `personal` and `acme` (neither loadable); `desk` holds personal, `ops`
/// acme, `plain` nothing; one open New card importing into acme. Returns
/// `(store, desk, ops, plain, acme_id, card_id)`.
fn seeded() -> (Store, i64, i64, i64, i64, i64) {
    let s = Store::open_in_memory().unwrap();
    s.set_catalog_config("/nonexistent/m4-personal", None).unwrap();
    let org = s.add_org("acme", None, false).unwrap();
    let acme = s
        .upsert_catalog("acme", "/nonexistent/m4-acme", None, Some(org.id))
        .unwrap();
    let desk = s.insert_client_token("desk", "aa11", "full").unwrap();
    s.set_client_assets_admin("desk", true).unwrap();
    let ops = s.insert_client_token("ops", "bb22", "full").unwrap();
    s.set_client_catalog_grant("ops", acme.id, true).unwrap();
    let plain = s.insert_client_token("plain", "cc33", "full").unwrap();
    let card = s
        .insert_changeset(
            "new",
            "New on oci: skill/w → core",
            &[NewChangesetItem {
                grp: "core".into(),
                catalog_id: Some(acme.id),
                kind: "skill".into(),
                name: "w".into(),
                action: "import".into(),
                params: Some(r#"{"from_host":"oci","layer":"core","member":"skill/w","hash":"h"}"#.into()),
                decider: "rule".into(),
            }],
        )
        .unwrap();
    (s, desk.id, ops.id, plain.id, acme.id, card.id)
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn every_caller_can_list_but_only_a_grant_on_the_cards_catalog_may_act() {
    let _g = crate::service::catalog::lock_registry_for_test();
    let (s, desk, ops, plain, _acme, card) = seeded();
    let t = tools(s);
    for caller in [
        Caller::master(),
        client(desk, TokenMode::Full),
        client(plain, TokenMode::Full),
        host("h1"),
    ] {
        assert_eq!(code_of(&call(&t, &caller, "list", None, None).await), "OK");
        assert_eq!(code_of(&call(&t, &caller, "list", Some(card), None).await), "OK");
    }
    for caller in [client(desk, TokenMode::Full), client(plain, TokenMode::Full), host("h1")] {
        for action in ["apply", "undo", "dismiss"] {
            let r = call(&t, &caller, action, Some(card), None).await;
            assert_eq!(code_of(&r), "E_FORBIDDEN", "{action}: {:?}", r.err());
        }
        let r = call(&t, &caller, "reject_item", Some(card), Some(vec![0])).await;
        assert_eq!(code_of(&r), "E_FORBIDDEN");
        assert!(message_of(r).contains("--catalog acme"), "names the remedy");
    }
    assert_eq!(
        t.store.lock().unwrap().get_changeset(card).unwrap().unwrap().state,
        "proposed",
        "nothing changed"
    );
    let r = call(&t, &client(ops, TokenMode::Full), "dismiss", Some(card), None).await;
    assert_eq!(code_of(&r), "OK", "{:?}", r.err());
    assert_eq!(t.store.lock().unwrap().get_changeset(card).unwrap().unwrap().state, "dismissed");
}

/// R25: propose is fleet-wide (the personal grant); a readonly client is
/// refused by the mode gate, list included.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn propose_needs_the_personal_grant_and_a_readonly_client_is_refused() {
    let _g = crate::service::catalog::lock_registry_for_test();
    let (s, _desk, ops, _plain, _acme, _card) = seeded();
    let kiosk = s.insert_client_token("kiosk", "dd44", "readonly").unwrap();
    let t = tools(s);
    let r = call(&t, &client(ops, TokenMode::Full), "propose", None, None).await;
    assert_eq!(code_of(&r), "E_FORBIDDEN");
    assert!(message_of(r).contains("catalog personal"));
    let r = call(&t, &client(kiosk.id, TokenMode::Readonly), "list", None, None).await;
    assert_eq!(code_of(&r), "E_FORBIDDEN");
    let r = call(&t, &Caller::master(), "bogus", None, None).await;
    assert_eq!(code_of(&r), "E_INVALID");
}

/// R25: applying a rollout writes hosts — the personal grant too, and the
/// `apply_sync` confirm gate.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn applying_a_rollout_needs_personal_too_and_passes_the_confirm_gate() {
    let _g = crate::service::catalog::lock_registry_for_test();
    let (s, _desk, ops, _plain, acme, _card) = seeded();
    s.set_setting(guard::SETTING_CONFIRM_DESTRUCTIVE, "true").unwrap();
    let rollout = s
        .insert_changeset(
            "rollout",
            "Roll out ops to h",
            &[NewChangesetItem {
                grp: "ops".into(),
                catalog_id: Some(acme),
                kind: "host".into(),
                name: "h".into(),
                action: "sync".into(),
                params: Some(r#"{"layer":"ops","assets":["skill/w"]}"#.into()),
                decider: "rule".into(),
            }],
        )
        .unwrap();
    let t = tools(s);
    let r = call(&t, &client(ops, TokenMode::Full), "apply", Some(rollout.id), None).await;
    assert_eq!(code_of(&r), "E_FORBIDDEN", "a host-writing apply also needs personal");
    assert!(message_of(r).contains("catalog personal"));
    let asked = call(&t, &Caller::master(), "apply", Some(rollout.id), None)
        .await
        .unwrap_err();
    assert!(asked.message.starts_with(codes::E_CONFIRM_REQUIRED), "{}", asked.message);
}
```

`mcp/tools/mod.rs`: add `#[cfg(test)] mod tests_changesets;` next to `mod tests_catalog_admin;`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p fleet-core tests_changesets` — Expected: compile error, `ChangesetsParams` / `changesets` not found.

- [ ] **Step 3: Write the implementation**

`mcp/tools/params.rs`:

```rust
#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ChangesetsParams {
    /// list|propose|apply|undo|dismiss|reject_item
    pub action: String,
    /// The card: list shows it in full; apply|undo|dismiss|reject_item need it.
    #[serde(default)]
    pub id: Option<i64>,
    /// apply: items (default all pending but "needs a look"; drift: one).
    /// reject_item: required.
    #[serde(default)]
    pub positions: Option<Vec<i64>>,
    /// apply of a rollout or restore: nonce of an approved E_CONFIRM_REQUIRED.
    #[serde(default)]
    pub confirm_nonce: Option<String>,
}
```

`mcp/guard.rs` — in `TOOL_POLICIES`, after the `catalog_admin` row:

```rust
    // Assets M4: changeset cards. `Client` and NOT in `NOT_FOR_HOST_TOKENS`:
    // `list` is open to every caller that reaches it, per-host tokens
    // included, as `list_assets` (spec, Testing: "… but can list"); every
    // other action checks a grant on each catalog the card names
    // (`may_admin_catalog_row`), which a per-host token never holds. Not
    // confirm-gated as a whole; applying a rollout or a restore passes the
    // `apply_sync` confirm gate.
    ToolPolicy {
        name: "changesets",
        access: Access::Client,
        readonly: false,
        confirm: false,
        deadline: Deadline::Lifecycle,
    },
```

`mcp/tools/assets.rs` — in the `#[tool_router]` impl, after `catalog_admin`:

```rust
    #[tool(description = "Changeset cards that adopt, sync and fix assets: \
        list (one in full with id), propose (rebuild from the last scan), \
        apply (positions picks items; a drift card takes one), undo (the \
        latest applied card per catalog), dismiss, reject_item. Mutating \
        actions need a grant on every catalog the card names.")]
    pub(super) async fn changesets(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<ChangesetsParams>,
    ) -> Result<CallToolResult, McpError> {
        use catalog::changesets as cs;
        audit(
            "changesets",
            &format!(
                "action={} id={} caller={}",
                p.action,
                p.id.map_or_else(|| "-".to_string(), |i| i.to_string()),
                caller.label()
            ),
        );
        match p.action.as_str() {
            "list" => match p.id {
                Some(id) => ok_json(&cs::get(id, &self.store).map_err(to_mcp_err)?),
                None => ok_json_compact(&cs::list(&self.store).map_err(to_mcp_err)?),
            },
            "propose" => {
                if !may_admin_catalog(&caller, &self.store, catalog::catalogs::PERSONAL)? {
                    return Err(changesets_forbidden("propose", catalog::catalogs::PERSONAL, &caller));
                }
                catalog::ensure_fresh(&self.store).map_err(to_mcp_err)?;
                ok_json_compact(&cs::propose(&self.store).await.map_err(to_mcp_err)?)
            }
            "apply" | "undo" | "dismiss" | "reject_item" => {
                let id = p.id.ok_or_else(|| {
                    mcp_err(codes::E_INVALID, format!("changesets {} needs an id", p.action), None)
                })?;
                let (card, items) = cs::card(id, &self.store).map_err(to_mcp_err)?;
                let writes_hosts =
                    p.action == "apply" && cs::writes_hosts(&card, &items, p.positions.as_deref());
                self.check_card_grants(&caller, &p.action, &items, writes_hosts)?;
                match p.action.as_str() {
                    "apply" => {
                        if writes_hosts {
                            self.confirm_gate(
                                "apply_sync",
                                p.confirm_nonce.as_deref(),
                                &format!("changeset={id}"),
                                &caller,
                            )?;
                        }
                        catalog::ensure_fresh(&self.store).map_err(to_mcp_err)?;
                        let args = cs::apply::ApplyArgs {
                            id,
                            positions: p.positions,
                        };
                        ok_json(&cs::apply::apply(args, &self.store, &self.ssh).await.map_err(to_mcp_err)?)
                    }
                    "undo" => ok_json(&cs::undo::undo(id, &self.store).await.map_err(to_mcp_err)?),
                    "dismiss" => ok_json(&cs::undo::dismiss(id, &self.store).await.map_err(to_mcp_err)?),
                    _ => {
                        let positions = p.positions.ok_or_else(|| {
                            mcp_err(codes::E_INVALID, "reject_item needs positions", None)
                        })?;
                        ok_json(
                            &cs::undo::reject_items(id, &positions, &self.store)
                                .await
                                .map_err(to_mcp_err)?,
                        )
                    }
                }
            }
            other => Err(mcp_err(
                codes::E_INVALID,
                format!("unknown changesets action {other}: list|propose|apply|undo|dismiss|reject_item"),
                None,
            )),
        }
    }
```

In the plain `impl FleetTools` block (next to `prepare_admin_call`):

```rust
    /// R25: a grant on every catalog the card's items name (none named:
    /// personal), and personal too for an apply that writes hosts. Per-host
    /// tokens never pass (`may_admin_catalog_row`).
    fn check_card_grants(
        &self,
        caller: &Caller,
        action: &str,
        items: &[crate::store::ChangesetItemRow],
        writes_hosts: bool,
    ) -> Result<(), McpError> {
        let ids: std::collections::BTreeSet<i64> = items.iter().filter_map(|i| i.catalog_id).collect();
        let rows: Vec<crate::store::CatalogRow> = {
            let s = self
                .store
                .lock()
                .map_err(|_| mcp_err(codes::E_LOCK, "store mutex poisoned", None))?;
            ids.iter()
                .filter_map(|id| s.get_catalog(*id).transpose())
                .collect::<Result<_, _>>()
                .map_err(|e| to_mcp_err(e.into()))?
        };
        let personal = catalog::catalogs::PERSONAL;
        if (writes_hosts || rows.is_empty()) && !may_admin_catalog(caller, &self.store, personal)? {
            return Err(changesets_forbidden(action, personal, caller));
        }
        for row in &rows {
            let name = if row.org_id.is_none() { personal } else { row.name.as_str() };
            if !may_admin_catalog_row(caller, &self.store, name, Some(row))? {
                return Err(changesets_forbidden(action, name, caller));
            }
        }
        Ok(())
    }
```

and a free function next to `forbidden`:

```rust
/// The `E_FORBIDDEN` for a `changesets` action without the grant it needs.
fn changesets_forbidden(action: &str, catalog: &str, caller: &Caller) -> McpError {
    let grant = if catalog == catalog::catalogs::PERSONAL {
        "fleet-hub client grant <name> assets".to_string()
    } else {
        format!("fleet-hub client grant <name> assets --catalog {catalog}")
    };
    mcp_err(
        "E_FORBIDDEN",
        format!(
            "changesets {action} needs the master token or a paired client granted catalog \
             {catalog} ({} refused); on the hub: {grant}",
            caller.label()
        ),
        None,
    )
}
```

`docs/control-api.md` — after the *Asset catalog layers* bullet:

```markdown
- **Asset changesets (Assets M4)** — `changesets { action: list | propose
  | apply | undo | dismiss | reject_item }`: cards proposed from the last
  scan (Bootstrap, New on host, Drift, Rollout). `list` (one card in full
  with `id`) is open to every caller; `propose` needs the personal grant;
  `apply` (`positions` picks items; a drift card applies one), `undo` (the
  latest applied card per catalog: `git revert` plus the stored layer
  assignments, never touching hosts), `dismiss` and `reject_item` need a
  grant on every catalog the card names; applying a rollout or a restore
  also needs the personal grant and passes the `apply_sync` confirm gate. A
  failed apply commits nothing; a card never removes anything from a host.
  Dismissing or rejecting records a verdict, so the same content is not
  proposed again.
```

- [ ] **Step 4: Run the tests and regenerate**

Run, each on its own: `cargo test -p fleet-core tests_changesets`; `cargo test -p fleet-core every_router_tool_has_exactly_one_policy_row`; `cargo test -p fleet-core mcp::guard`; `cargo test -p fleet-core tests_catalog_admin`.
Expected: PASS.
Then: `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`; `cargo test -p fleet-core the_served_definition_budget_stays_bounded -- --nocapture` — set `BUDGET_BYTES` to the printed measurement + 100 and re-run (expected: PASS).

- [ ] **Step 5: Commit** (N = the measurement the budget test printed)

```bash
git add crates/fleet-core/src/mcp/ docs/control-api.md docs/control-api-reference.md
git commit -m "feat(mcp): changesets tool — list for all, a grant per touched catalog for the rest

Budget: measured N bytes after the changesets tool and its params."
```

---

### Task 10: Read-only `catalog list` (M-e), verification, docs

**Files:**
- Modify: `crates/fleet-core/src/service/catalog/catalogs.rs` (`catalog_rows`, `probe_catalogs`)
- Modify: `crates/fleet-hub/src/catalog.rs` (`list_rows` reads only; `List`'s doc; tests)
- Modify: `CLAUDE.md` (M3 paragraph clause; an M4 paragraph)
- Modify: `docs/hub.md` (*Asset catalog*)
- Possibly modify: generated files the four generators rewrite

**Interfaces:**
- Consumes: `repo::load_dir`, `Store::{list_catalogs, list_orgs, catalog_admissions, catalog_grantees}`.
- Produces:

```rust
// catalogs.rs
pub fn probe_catalogs(store: &Mutex<Store>) -> Result<Vec<CatalogStatus>, IpcError>;
// fleet-hub catalog.rs
fn list_rows(store: &Mutex<Store>) -> Result<Vec<catalogs::CatalogStatus>, String>;
```

- [ ] **Step 1: Write the failing test**

`crates/fleet-hub/src/catalog.rs` tests module:

```rust
    /// R23 (M-e): `catalog list` reads only — an org catalog whose checkout
    /// is missing is not cloned, and the store's load record is untouched.
    #[test]
    fn list_never_clones_or_records_a_load() {
        let _registry = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let opts = HubOptions {
            data_dir: Some(dir.path().to_path_buf()),
            ..HubOptions::default()
        };
        let env = HashMap::new();
        drop(
            Store::open_with_bus(
                &dir.path().join("state.db"),
                Arc::new(fleet_core::events::NoopEventBus),
            )
            .unwrap(),
        );
        let personal = git_repo(dir.path(), "personal-assets");
        let set = CatalogCmd::Set {
            path: personal.to_string_lossy().into(),
            remote: None,
        };
        run(set, &opts, &env).unwrap();
        let remote = git_repo(dir.path(), "acme-remote");
        let target = dir.path().join("acme-checkout");
        {
            let s = serve::open_store(&opts, &env).unwrap();
            let org = s.add_org("acme", None, false).unwrap();
            s.upsert_catalog(
                "acme",
                &target.to_string_lossy(),
                Some(&remote.to_string_lossy()),
                Some(org.id),
            )
            .unwrap();
        }
        let store = Mutex::new(serve::open_store(&opts, &env).unwrap());
        let before = store.lock().unwrap().get_catalog_by_name("personal").unwrap().unwrap();

        let rows = list_rows(&store).unwrap();
        let acme = rows.iter().find(|c| c.name == "acme").expect("listed");
        assert_eq!(acme.state, "not_loaded");
        assert!(acme.problem.as_deref().unwrap().contains("reload --catalog acme"));
        assert!(!target.exists(), "list never clones");
        let p = rows.iter().find(|c| c.name == "personal").unwrap();
        assert_eq!((p.state.as_str(), p.asset_count), ("loaded", 1));
        let after = store.lock().unwrap().get_catalog_by_name("personal").unwrap().unwrap();
        assert_eq!(
            (after.head_commit, after.last_loaded_at),
            (before.head_commit, before.last_loaded_at),
            "nothing recorded"
        );
    }
```

and in `list_puts_a_personal_load_error_on_the_personal_row`, replace `let (rows, unplaced) = list_rows(&store).unwrap(); assert_eq!(unplaced, None);` with `let rows = list_rows(&store).unwrap();`.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p fleet-hub list_never_clones_or_records_a_load` — Expected: FAIL (the old `list_rows` returns a tuple; once adapted, `ensure_fresh` clones `acme-checkout`).

- [ ] **Step 3: Write the implementation**

`catalogs.rs` — factor `list_catalogs`'s store block into a helper and add the probe:

```rust
/// One catalog's store record: its row, owner name, admissions, grantees.
type CatalogRecord = (CatalogRow, Option<String>, Vec<String>, Vec<String>);

/// Every catalog's store record, under one guard (released before any
/// registry or disk read).
fn catalog_rows(store: &Mutex<Store>) -> Result<Vec<CatalogRecord>, IpcError> {
    let s = lock(store)?;
    let orgs = s.list_orgs()?;
    let mut rows = Vec::new();
    for r in s.list_catalogs()? {
        let org = r
            .org_id
            .and_then(|id| orgs.iter().find(|o| o.id == id).map(|o| o.name.clone()));
        let admitted = s.catalog_admissions(r.id)?;
        let granted = s.catalog_grantees(r.id)?;
        rows.push((r, org, admitted, granted));
    }
    Ok(rows)
}

/// `fleet-hub catalog list` (Assets M4, R23 — M-e): every catalog as its
/// checkout stands, read only — parsed in place (`repo::load_dir`), never
/// cloned (`ensure_repo` is not called), never recorded in the store, never
/// put in the registry. `head_commit`/`last_loaded_at` stay the store's
/// record of the last load.
pub fn probe_catalogs(store: &Mutex<Store>) -> Result<Vec<CatalogStatus>, IpcError> {
    Ok(catalog_rows(store)?
        .into_iter()
        .map(|(r, org, admitted, granted)| {
            let reload = if r.org_id.is_none() {
                "fleet-hub catalog reload".to_string()
            } else {
                format!("fleet-hub catalog reload --catalog {}", r.name)
            };
            let root = std::path::Path::new(&r.repo_path);
            let (state, problem, asset_count) = match root.join(".git").try_exists() {
                Ok(false) => (
                    "not_loaded",
                    Some(format!("no checkout at {} yet; `{reload}` clones or loads it", r.repo_path)),
                    0,
                ),
                Err(e) => ("problem", Some(format!("catalog checkout {}: {e}", r.repo_path)), 0),
                Ok(true) => match repo::load_dir(root) {
                    Ok(c) => ("loaded", None, c.assets.len()),
                    Err(e) => ("problem", Some(e.message), 0),
                },
            };
            CatalogStatus {
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
            }
        })
        .collect())
}
```

and `list_catalogs`'s opening block becomes `let rows = catalog_rows(store)?;` (the registry half is unchanged).

`crates/fleet-hub/src/catalog.rs`:
- `List`'s doc: `/// Print every catalog as its checkout stands — owner, state, HEAD of the last load, path, admissions, grants. Read-only: never clones, pulls or records a load (\`reload\` does).`
- replace `list_rows` with:

```rust
/// `catalog list`'s rows (Rulings R23): read-only, see
/// `catalogs::probe_catalogs` — a personal checkout that does not parse
/// shows on its own row as `problem`, as before.
fn list_rows(store: &Mutex<Store>) -> Result<Vec<catalogs::CatalogStatus>, String> {
    catalogs::probe_catalogs(store).map_err(|e| e.message)
}
```

- in `list`, `let (all, unplaced) = list_rows(store)?;` and its `if let Some(e) = unplaced { … }` become `let all = list_rows(store)?;`; its doc comment: "`catalog list`: reads each checkout where it is (`list_rows`) and prints one line per catalog (NAME, OWNER, STATE, HEAD, PATH) with its admissions, grants and any problem."

- [ ] **Step 4: Run the tests**

Run, each on its own: `cargo test -p fleet-hub list_never_clones_or_records_a_load`; `cargo test -p fleet-hub catalog`; `cargo test -p fleet-core service::catalog::catalogs`.
Expected: PASS.

- [ ] **Step 5: Docs**

`CLAUDE.md` — in the M3 paragraph replace "(config, load, list_layers, set_host_layers; the authoring actions stay personal-only until M4)" with "(config, load, list_layers, set_host_layers, and the authoring actions since M4)". Add after the M3 paragraph:

```markdown
- **Assets M4 — changesets** (plan
  `docs/superpowers/plans/2026-10-02-assets-m4-changesets.md`): migration 094
  adds `changesets`, `changeset_items` and `asset_triage_verdicts` (the
  spec's DDL verbatim). `service/catalog/changesets/` proposes cards by rule
  (`rules.rs`, pure): Bootstrap when nothing is bootstrapped yet or ≥ 20
  unmanaged normal identities exist — grouped by host-set signature and name
  prefix into context layers (`everywhere`, `<host>-only`, `core`,
  `<prefix>`), `set_scope shared` for personal assets an org host has,
  `hide` for internals; New on host per identity; Drift (take the host copy
  or restore); Rollout for a never-rolled-out layer with `missing` members.
  The reconcile pass (`reconcile.rs`) refreshes them after every scan-tick
  pass (`after_scan_pass`, `try_lock` — it never blocks the tick or touches
  its owed set) and on `changesets { propose }`; a card's subject is derived
  from its items; a verdict on `(kind, name, content_hash)` holds a subject
  until its content changes, and a `person` verdict is never replaced by
  another decider. Apply (`apply.rs`) needs loaded, clean checkouts,
  snapshots the touched catalogs' `host_layers`, imports per (catalog, source
  host), writes layer files and scopes, appends contexts, and commits once
  per catalog (`fleet: <summary>`); any failure resets every touched checkout
  and the snapshot and commits nothing; success proposes a follow-up
  Rollout. Rollout and Drift restore run `plan_sync` + `apply_sync` narrowed
  to create/adopt/update (restore: update/overwrite) — never remove. Undo
  (`undo.rs`) reverts the card's commits and restores the snapshot, only for
  the latest applied card per catalog; dismiss and reject_item write
  `rejected` verdicts. `catalog.auto` (on) runs the pass on the tick and
  SB6's additive sync on layers a Rollout card has applied;
  `catalog.auto_push` (off) pushes after apply and undo. One tokio
  `APPLY_LOCK` serialises all of it. MCP `changesets { list | propose |
  apply | undo | dismiss | reject_item }`: list is open (per-host tokens
  too); every other action needs a grant on each catalog the card names
  (rollout and restore also personal, plus the `apply_sync` confirm gate).
  Also in M4: the authoring `catalog_admin` actions take `catalog`
  (`CatalogTarget`; `configure` stays personal) and a named catalog is
  resolved once per call; a catalog's load problems hold their keys
  (`ProblemHolds` → a `Noop`, never a `Remove`); `fleet-hub catalog list`
  reads only. No Tauri command or verdict row yet (M6).
```

`docs/hub.md`, *Asset catalog*:
- replace the paragraph starting "A catalog whose checkout cannot be loaded is shown as a problem" with:

```markdown
A catalog whose checkout cannot be loaded is shown as a problem (`catalog
list`) while the others load; it is retried at its next `reload`. Sync never
removes what a catalog installed because that catalog went away — not
loaded, failed, unadmitted, the host changed org, or removed — nor an asset
whose own file in a loaded catalog does not parse (or whose kind's directory
cannot be read): it reports those assets as `Noop` "kept, not removed" and
leaves them to you. Changeset cards never propose a removal either: a kept
copy stays until you sync a plan that removes it.

**Changeset cards.** After each asset scan the hub proposes cards from what
the hosts have: a Bootstrap card that adopts what is already installed into
the catalogs as layers (grouped by which hosts have each asset; personal
assets an org-bound host already has are proposed `shared`), New-on-host
cards, Drift cards (take the host's copy, or restore the catalog's) and
Rollout cards (sync a layer to its hosts). Nothing is applied until someone
applies a card through the MCP tool `changesets` (`list`, `propose`,
`apply`, `undo`, `dismiss`, `reject_item`). Applying commits once in each
catalog it changes and commits nothing if any step fails; `undo` reverts
the latest applied card in each catalog and puts the hosts' layer
assignments back, without touching hosts. A card never overwrites or
removes on a host, except a Drift card's restore, which a person picks for
one asset on one host and which backs up what it replaces. Once a Rollout
card for a layer has been applied, `catalog.auto` (Settings → Automation →
Assets, on by default) also installs and updates that layer's assets on its
hosts by itself; turn it off to make every change a card.
`catalog.auto_push` (off by default) pushes the catalog right after a card
commits. Applying, undoing, dismissing or rejecting needs the master token
or a client granted every catalog the card names (`fleet-hub client grant
<name> assets [--catalog NAME]`); listing does not. A catalog with
uncommitted changes refuses a card until they are committed.
```

- in the sentence "`catalog list` shows each catalog's owner, load state, HEAD, admissions and grants;" add after "grants": " — read-only: it parses each checkout where it is and never clones, pulls or records a load (`reload` does)".

- [ ] **Step 6: Full verification and generators**

Run, each on its own: `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace --no-fail-fast`; `pnpm test`; `pnpm check`. Expected: green except the known environmental failures named in Global Constraints.
Run: `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`; `REGEN_SETTINGS_DOCS=1 cargo test -p fleet-core settings_docs_are_current`; `REGEN_PAGE_DOCS=1 cargo test -p fleet-core page_docs_are_current`; `REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen` (it fails once on purpose when it rewrites; run it again). Expected: no diff beyond what Tasks 3, 5 and 9 committed — in particular no verdict change (R25). `git status` clean after committing anything a generator rewrote, naming the generator in the message.
Smoke on a hub with a personal catalog only and no cards applied: `fleet-hub catalog list` prints the same rows as before and creates nothing; a `plan_sync` for a host is identical to `main`'s (Global Constraints: a personal-only fleet behaves as before).

- [ ] **Step 7: Commit**

```bash
git add crates/fleet-core/src/service/catalog/catalogs.rs crates/fleet-hub/src/catalog.rs CLAUDE.md docs/hub.md
git commit -m "fix(fleet-hub): catalog list reads only (M-e); docs: changesets (M4)"
```

---

## Self-review against the spec (M4)

| Spec item | Task |
|---|---|
| `changesets` table (DDL as specified) | 1 |
| `changeset_items` table (DDL as specified) | 1 (R1; `catalog_id` without ON DELETE handled by R26) |
| `asset_triage_verdicts` table (DDL as specified) | 1 |
| SB4: no automatic push; `catalog.auto_push` exists, off by default | 5 (setting), 6 and 8 (push after apply / undo when on) |
| SB5: undo = `git revert` + stored `host_layers` snapshot; only the latest applied card per catalog | 8 (R20), snapshot written in 6 |
| SB6: automatic apply only additive ops on layers rolled out at least once; first rollout always a card | 7 (`auto_additive`, R16, R17) |
| Reconcile pass after each scan-tick pass and on demand (`propose`) | 5 (`after_scan_pass`, `propose`; R19) |
| Never re-propose a subject with a matching verdict until its hash changes | 4 (rules), 1 (store), 8 (verdicts from dismiss/reject) |
| An agent never overturns a person's verdict | 1 (store-enforced upsert, R10) |
| Bootstrap trigger: a catalog is empty, or ≥ 20 unmanaged normal identities | 4 (R4) |
| Bootstrap items: groups by signature + prefix family → layers; destination org X else personal; `set_scope shared` for personal destinations on an org host; `hide` internals; needs_person as "needs a look" | 4 (R5, R6, R8), 5 (hide verdicts with auto) |
| New on host: adopt into the layer sharing host set or prefix; else needs a look | 4 |
| Drift: per host take_host or restore, one item at a time | 4 (card), 6 (take_host, one item), 7 (restore) |
| Rollout: a card changed a layer, or a new layer exists; `plan_sync` limited to affected hosts and assets | 6 (follow-up, R14), 5 (gaps, R16), 7 (apply) |
| Apply steps 1–5 (snapshot, per-catalog import/layers/scope, host_layers, one commit per catalog with SHAs, mark applied) | 6 |
| A failure before step 4 resets trees, card `failed` with the error on the failing group, nothing committed | 6 (R12, incl. a commit failure) |
| Rollout apply = `plan_sync` + `apply_sync`; overwrite and remove never through a card | 7 (R15) |
| Undo never touches hosts; a follow-up Rollout card restores them | 8 (R14) |
| Automatic (`catalog.auto`, default on): hide internals, group, prepare cards, additive sync | 5, 7 (R18) |
| MCP `changesets { list \| propose \| apply \| undo \| dismiss \| reject_item }` | 9 |
| Grants per touched catalog (`may_admin_catalog`); per-host tokens never pass; listing stays readable | 9 (R25), 3 (`may_admin_catalog_row`) |
| Tauri AdminCall verdict table gains the new actions | no new AdminCall actions in M4 (`changesets` is its own tool); no command, row or contract bump until M6 (R25); `REGEN_HUB_VERDICTS` in 10 shows no change |
| Jev/haiku are S5; keep the `decider` column | 1 (column), 4 (`Decider` enum; M4 writes rule/person, R10) |
| Testing — changesets: bootstrap on the live fixture shape (164 identities, 8 signatures), expected groups and `shared` proposals | 4 |
| Testing — apply makes one commit per catalog | 6 |
| Testing — undo reverts and restores `host_layers` | 8 |
| Testing — a failed apply commits nothing | 6 |
| Testing — store: verdict holds by content hash | 1, 4 |
| Testing — authorization: ungranted client and per-host token cannot apply/undo in a catalog without a grant, but can list | 9 |
| Out of scope: automatic push by default; automatic overwrite / remove under any mode | 5 (`auto_push` off), 7 (Additive filter), 2 (problem holds never remove) |
| UI (M5/M6), Jev/haiku (S5) | not touched |
| Carry 1–4 | see the carry table above |
