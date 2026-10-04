# CLAUDE.md

Orientation for Claude Code working in this repository.

## What this is

`claude-fleet` — a Tauri 2 desktop app (Rust backend + Svelte 5 frontend) for
managing long-lived Claude Code sessions running in tmux across multiple
machines over SSH. ~143,000 LOC Rust, ~55,000 LOC frontend.

The Rust side is a workspace: `crates/fleet-core` (Tauri-free service/store/SSH/MCP),
`crates/fleet-hub` (headless daemon, see `docs/hub.md`), `crates/fleet-proto` (the
hub/agent frame types, shared by both ends), `crates/fleet-agent` (the agent binary
for hosts the hub cannot reach — depends on `fleet-proto` only, never `fleet-core`),
`crates/fleet-agent-e2e` (tests only: the hub against the real agent over a socket,
through fleet-core's `testkit` feature), `src-tauri` (the desktop app).

## Build & test

### Validation ladder (use this, in this order)

One canonical way to validate Rust changes. Every command below selects the
whole workspace. All but the first build test targets, so they share one set
of compiled dependencies in `target/debug/`; `fleet-fast-check` builds none
and keeps its own set in `target/fast-check/` (a cargo profile), so the two
never evict each other. Mixing in other selections (`-p <crate>`, plain
`cargo build`, `cargo check` without `--all-targets`, `pnpm tauri …`) makes
cargo compile another copy of fleet-core and its dependency graph: 1.5–3 min
the first time, then every edit paid once per copy
(RUST-BUILD-PERFORMANCE-AUDIT.md §10.3). The aliases live in
`.cargo/config.toml`. Times are for a fleet-core edit on 4 cores.

```bash
# 1. while you work, after every edit (≈ 13 s; libraries and binaries only)
cargo fleet-fast-check                  # check --workspace --profile fast-check
pnpm check                              # frontend edits: svelte-check
# 2. at a checkpoint: before committing, and after any change to an API that
#    tests use (≈ 19 s; = rust-analyzer's own check)
cargo fleet-check                       # check --workspace --all-targets
# 3. the tests of what you touched (module path filter; ≈ 30 s build + the run)
cargo fleet-test -- service::health     # test --workspace --lib --bins -- <filter>
pnpm exec vitest run src/lib/foo.test.ts
# 4. before committing (also what .githooks/pre-commit runs)
cargo fmt --all --check
cargo fleet-lint                        # clippy --workspace --all-targets -- -D warnings
# 5. before pushing / marking a PR ready (≈ 2.5 min warm)
cargo test --workspace                  # full suite, what CI runs
scripts/ci-local.sh                     # everything in CI order; --rust-only / --frontend-only / --hub-e2e
```

`fleet-fast-check` does not type-check test code: a signature change that
breaks a test passes it and fails `fleet-check`. rust-analyzer stays on the
full check. `target/fast-check/` costs ~2 GB once and is never cleaned
automatically.

Rules:

- Do not run `cargo build` to see whether something compiles; `cargo
  fleet-fast-check` / `cargo fleet-check` answer that 2–6× faster. Build
  only when you need a binary.
- Do not narrow with `-p <crate>` in the inner loop; narrow with a test filter.
  Keep `-p` for the cases below that need a binary or a different feature set.
- A test that checks a file outside its crate (a `src/lib/*.ts` mirror,
  `src-tauri/src/lib.rs`, a `docs/*.md` guide) reads it when it runs
  (`repo_files::read` in fleet-core), never with `include_str!`: a compiled-in
  copy makes every edit to that file recompile the whole test target (~26 s
  for fleet-core's, against ~0.4 s). Likewise fleet-core takes no
  dev-dependency on a workspace crate it does not already depend on; a test
  that needs one lives in a crate of its own, as `crates/fleet-agent-e2e`
  does. `src/lib/names.json`,
  `tools/ag/**` and two `skills/*/SKILL.md` are embedded in fleet-core itself,
  so editing them does recompile it.
- `pnpm tauri dev` / `pnpm tauri build` and `cargo build -p fleet-hub` use other
  feature sets. Run them when you need them; in a cloud session (no display,
  ~30 GB disk) do not run the Tauri ones at all.

Other commands:

```bash
pnpm install --frozen-lockfile
pnpm test                       # frontend (Vitest), all of it
cargo deny check                # licenses + advisories (cargo install cargo-deny --locked)
cargo build -p fleet-hub --locked   # headless hub (no Tauri libs needed)
scripts/hub-e2e.sh               # real fleet-hub/-agent e2e; needs tmux, opt-in via ci-local.sh --hub-e2e
```

The Rust toolchain is pinned in `rust-toolchain.toml` (bump it in its own PR,
with the CI and Dockerfile pins; `scripts/check-version-consistency.sh`
enforces it).

`devtools` is an off-by-default cargo feature: `cargo tauri build --features
devtools` enables the Web Inspector in a release bundle; dev builds have it
automatically.

**Caveat:** `cargo` builds need the Tauri system libraries (dbus, gtk/atk,
pkg-config). On a headless box without them, `cargo build`/`cargo test` fail in
a build script — that is an environment gap, not a code error. Frontend
(`pnpm test`) builds anywhere.

After pulling, run `pnpm install --frozen-lockfile` before testing. Stale
`node_modules` cause `Failed to resolve import "@tauri-apps/plugin-clipboard-manager"`
in `App.test.ts` and `clipboard_native.test.ts` — that is a dependency gap, not a
code error. (`localStorage` is polyfilled in `vitest.setup.ts`; there are no
known pre-existing frontend test failures.)

Known Rust flakes — timing-sensitive, so they fail on a loaded box; re-run
alone before blaming your change: `rewind::tests::the_removal_script_leaves_a_tree_a_live_pane_is_in`,
`work::scale_tests::*`, the `CHAIN_BUDGET` migration tests in
`store/schema/tests_upgrade.rs`, and `service::add_project`. Not a flake:
`cargo test -p fleet-core --lib -- --test-threads=1` takes 23–25 minutes
(4.5k tests; measured on mercury, 2026-10-02), so a `timeout 600` wrapper
kills it mid-run and the last `test … ...` line names whichever test was in
flight — a slow run, not a hang. Run it in parallel (the default), or give a
sequential run a 30-minute budget.

## Releasing

Releases are cut manually with `scripts/release.sh <new-version>` — it bumps
the six version files (+ `Cargo.lock`), prefills a `CHANGELOG.md` section
from the Conventional Commits since the last tag, commits, and creates the
`vX.Y.Z` tag. Never edit the version fields by hand; run the script from a
clean `main`. See `docs/RELEASING.md`. Once the release is pushed,
`scripts/release-mobile.sh <same-version>` tags fleet-mobile's `main` so its
own workflow builds the signed APK under the same version.

`docs/control-api-reference.md` is generated from the MCP tool router. After
editing any `#[tool(...)]` description or the `generate_handler!` list,
regenerate it or CI fails:

```bash
REGEN_DOCS=1 cargo fleet-test -- reference_is_current
```

`src/lib/hub_verdicts.generated.json` and the refusal table in `docs/hub.md`
are generated from `src-tauri/src/backend/verdicts.rs`. After editing any row,
regenerate them or CI fails:

```bash
REGEN_HUB_VERDICTS=1 cargo fleet-test -- verdict_gen
```

## Architecture

- **Frontend stores** (`src/lib/*.ts`) hold app state as Svelte 5 runes. Backend
  mutations emit row events (`events.rs` → `events.ts` `subscribeToRowEvents`);
  the frontend patches stores in place (`mergeOne`/`removeOne`) instead of
  re-fetching. Mutation wrappers also do an optimistic patch from the command's
  return value.
- **Backend** (`crates/fleet-core/src/`, thin Tauri handlers in
  `src-tauri/src/commands/`): the handlers wrap the transport-agnostic logic in
  `service/`; SSH multiplexing in `ssh.rs` (per-host `ControlMaster`, async
  `tokio::process`); tmux command construction in `tmux.rs`; SQLite in `store/`
  (migrations are registered in the `MIGRATIONS` table there — add a new
  `NNN_<topic>.sql` plus an entry); the event bus in `events.rs`; cancellation
  registry in `cancel.rs`. The single global PTY (`pty.rs`) stays in
  `src-tauri`, since it is desktop-only.
- **Session listing** is cache-first: `service::sessions::list_sessions` serves
  stored rows and only runs a reconcile pass when the last one is stale;
  `refresh_sessions` is the forced path for an explicit user refresh.
- **Settings metadata** (`service/settings.rs`): every `SPECS` row carries
  label, help, unit, what `0` means, tags, danger, restart and AI policy
  next to its kind and default (`every_spec_has_consistent_metadata`).
  `describe()` serves it to `get_settings { describe: true }` and
  `describe_fleet_settings`; `docs/settings-reference.md` and the settings
  tables in `docs/work-graph.md` / `docs/decisions.md` are generated from
  it — after editing a spec run
  `REGEN_SETTINGS_DOCS=1 cargo fleet-test -- settings_docs_are_current`.
  This is P1 of the declarative pages framework
  (`docs/superpowers/specs/2026-09-28-declarative-pages-design.md`).
- **Declarative pages** (`crates/fleet-core/src/pages/`, P2): pages are JSON
  specs in `crates/fleet-core/pages/<id>.json`, listed in `PAGE_FILES`,
  that NAME registered settings, data sources (`pages/sources.rs`) and
  catalog widgets and layouts; `pages::validate` refuses anything else, and
  every setting has exactly one home (`every_setting_has_one_home`: a new
  `SPECS` row needs a `field` on a page). Authoring guide `docs/pages.md`;
  regenerate `docs/page-spec.schema.json` / `docs/page-catalog.json` and
  the frontend fixture `src/lib/pages/registry.generated.json` with
  `REGEN_PAGE_DOCS=1 cargo fleet-test -- page_docs_are_current`.
  P3's renderer (`src/lib/pages/`) shows them in Settings beside
  "General" (`list_pages`, `fetch_page_source`); `hub.*` / `mcp.*` are
  read-only specs (`owned_by`, D-P7), and `settings::set` emits
  `settings:changed` (kind `settings`, never on host/org-bound streams).
  P4a: resources (`pages/resources.rs`) back `master_detail` pages — an
  action names an existing desktop command and binds its args, never code,
  so verdicts and hub routing are unchanged (`resource_commands_exist`);
  Settings → Organisations is one (OrgSettings.svelte is gone). P4b:
  flows (`pages/flows.rs`, `flow_start|submit|back|cancel`, `LocalOnly`)
  are server-driven wizards — the backend decides each step, a secret is
  never stored; `tracker.connect` backs Settings → Trackers
  (`settings.trackers`), which replaced WorkSettings' tracker list.
  P4c: a `data_page`'s `filters` set a source parameter by name on every
  item that takes it (Usage's window and host); Usage → Work graph usage
  (`usage.work`, source `work.usage` over `UsageSummary::rows`) replaced
  the hand-built WorkUsage panel and the `work_usage` command.
  P4d: layout L8 `embed` places catalog items in the desktop's own
  screens at a closed `Slot`; account usage (`account_usage { view }`,
  live source `accounts.usage` over `list_account_usage`) is drawn that
  way in Host detail, the Hosts list, the New-session chips and the
  footer, and on Usage → Claude accounts. Embed pages are not in
  `list_pages`: the desktop reads `src/lib/pages/embeds.generated.json`
  (REGEN_PAGE_DOCS); the views are `src/lib/pages/usage/`.
  P5: `set_setting { propose: true, why }` leaves a proposal, never a
  write (`service/settings_review.rs`, migration 083); every registered
  write is audited through `settings::set_by` with its `Actor`; layout L6
  `review_apply` (Settings → Proposed changes, `fleet-hub settings`), a
  field's inline suggestion and History, search as a plain-words command
  (`settings_nl.ts`), and page actions (`pages/actions.rs`) — custom
  items are capped at 3.
  P6: the settings route to the hub. `guard::Access::Person` (the master
  or a paired client bound to no org) reaches `get_settings` /
  `set_setting`; writes need a trusted `full` device (`settings_writer` in
  `mcp/tools/fleet.rs`); `Access::PersonDevice` tools (`setting_proposals`,
  `setting_history`, `decide_setting_proposals`, `list_pages`) are not
  served to the master. A paired desktop's pages show the hub's settings
  (`remote` hides data items, page actions and custom components).
  Guides: layout L9 `guide` (a section per step, Back / Next / Done; a
  guide's fields are never a setting's home), stored at runtime too
  (`service/guides.rs`, migration 088): the control API's `guide` tool
  (`Access::Client`, so a host's own token reaches it) serves `catalog` /
  `validate` / `propose` / `list`, and `decide` / `remove` need the
  master or a trusted device; a person approves in Settings → Guides or
  `fleet-hub guides`. A host's session writes one with the `fleet-guides`
  skill, shipped as a catalog asset in `catalog-seed/` (its example and
  limits are held to the tool by `service::guides` tests).
- **Status vocabulary** (`claude_status`, `stuck_kind`) lives in the enums in
  `service/pane_intel.rs`; the MCP tool descriptions and the generated
  reference derive from them, so add values there, not in prose.
- **Control API** (`mcp/`): an embedded MCP server (off by default, localhost +
  bearer token) lets an AI assistant drive the fleet. Its tools call the same
  `service/` layer as the Tauri commands. See `docs/control-api.md`.
- **Client access** (`mcp/pairing.rs`, `mcp/events_route.rs`,
  `store/clients.rs`): a phone or browser pairs through a single-use code
  (`pair_client` → `POST /pair`) for a named, revocable client token
  (`full`/`readonly`) that is never the master and never reaches fleet admin
  — except the asset catalog, when the operator grants it per client
  (`fleet-hub client grant <name> assets`, migration 074; the hub's
  `catalog_admin` tool, `service/catalog/admin.rs`, reads the grant live),
  and the fleet's settings, which a trusted device bound to no org writes
  (declarative pages P6) —
  and follows `GET /events` instead of polling. Hub-only; `fleet-hub
  pair|client` is the operator's side. See `docs/hub.md` → *Pair a phone*.
- **Assets S1a — unmanaged inventory** (plan
  `docs/superpowers/plans/2026-09-29-assets-s1a-foundation.md`, spec
  `docs/superpowers/specs/2026-09-29-assets-workspace-design.md`): an
  unmanaged row keeps its `host_hash` (content hash) and
  `secret_like`/`fleet_owned` flags (migration 087); `list_assets` returns
  `identities`, folding every host's copies of a `(kind, name)` into one
  `AssetIdentity` by host-set signature and classifying it
  (`service/catalog/identity.rs`).
  `service/catalog/scan_tick.rs` rescans a host whose inventory is older
  than `catalog.scan_max_age_secs` on a `catalog.scan_check_secs` timer,
  rescans every host after the catalog HEAD or a sync changes, and keeps a
  host whose rescan failed owed until it succeeds. `import_assets` (needs
  the `assets` grant) reads any registered host over SSH through
  `REMOTE_SOURCES_SCRIPT`, scrubbing fleet-owned hook entries
  (`hooks_install::is_fleet_owned_hook`), confined to top-level symlinks
  under a 64 MiB cap. `plan_sync` skips a non-`local`, unlayered,
  non-empty-catalog host — never scanning it over SSH — unless
  `allow_unlayered` is set, since syncing it as-is would otherwise install
  the whole catalog there.
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
- **Multi-harness F3c** (plan
  `docs/superpowers/plans/2026-10-01-f3c-codex-skills-agents-dir.md`):
  Codex skills render to `~/.agents/skills/<install name>/`
  (`CODEX_SKILLS_DIR`); `~/.codex/skills` (`CODEX_LEGACY_SKILLS_DIR`, minus
  Codex's `.system`) is still hashed so a pre-F3c manifest entry's old copy
  can be deleted under compare-and-swap — the planner's existing rule-8
  `remove_entry` path does the move, and a moved asset whose new location
  holds a copy fleet did not write is an `overwrite`, not an `update`. The
  Codex scan prints `##LINK <path>`, then the target on the next line
  (`HostSnapshot::links`), for a symlinked `~/.agents`, `~/.agents/skills`,
  entry in it, or
  `~/.codex/skills`; `compute_host_plan` blocks every write/adopt/delete
  under one (rule 9, `Harness::symlink_reason`), and any action whose
  planned file is absent at its exact path but present differing only in
  ASCII case (`skill.md` vs `SKILL.md`) is `Blocked` too (rule 10,
  `block_case_variants`, every harness).
  `plan::block_cross_harness_collisions`, called once per host in
  `plan_sync`, blocks any action whose file another harness's plan on that
  host also touches.
- **Assets M1 — catalogs table** (plan
  `docs/superpowers/plans/2026-09-30-assets-m1-catalogs.md`, spec
  `docs/superpowers/specs/2026-09-30-assets-s1b-s2-design.md`): a catalog is
  now a row in the `catalogs` table (migration 090, `org_id IS NULL` for the
  personal one), not the old singleton `catalog_config` — which stays in the
  schema but is no longer read. The existing config API
  (`get_catalog_config`/`set_catalog_config`/`set_catalog_head`) and
  `fleet-hub catalog set` keep their old shape and now address the
  `personal` row. Loaded catalogs live in `service/catalog/registry.rs`
  (`personal()`, `with_personal`, `get(id)`), which replaces the single
  process-global `Option<Catalog>`; registry then store is allowed (the
  `resolve_preview` path); store then registry is never allowed — read
  store rows first, release the guard, then take the registry. Assets
  carry `scope: private |
  shared` (private by default, omitted from `asset.yaml` when private,
  always present in the JSON API and on `AssetSummary`) — meaningful only in
  the personal catalog, since an org catalog's assets are org-scoped
  regardless.
- **Assets M2 — sync across catalogs** (plan
  `docs/superpowers/plans/2026-09-30-assets-m2-sync.md`): migration 091 puts
  `catalog_id` on `host_layers` and `asset_inventory` (an unknown id reads
  as `NULL`), so a host's layer assignments and scanned inventory are each
  pinned to one catalog. `service/catalog/effective.rs` decides, per host,
  what it should end up with: `acceptance(host_org, catalog_id, catalog_org,
  admitted)` returns `No` / `SharedOnly` / `All` (an org-bound host gets only
  the `shared` slice of `personal`; a host with no org also takes every org
  catalog it admits, M3), and `effective_for_host(store, host)` composes an
  `EffectiveSet` — reading every store row under one guard first, then the
  registry, since store → registry is never allowed. Within that set, the
  scope boundary and a `(kind, name)`/`(kind, install_name)` collision
  between catalogs are never silent destructive removals: each becomes a
  per-asset `Blocked` action carrying a reason (collision:
  `conflict: <catA>/<name> vs <catB>/<name> — use install_as or move one`),
  and a refused asset never gets an orphan `Remove`. An org-bound host that
  already has a private personal asset keeps it — `withheld` reports it as
  a `Noop` ("private; withheld from org host, not removed") instead of
  dropping it. `Action.catalog` and `ManifestEntry.catalog` record which
  catalog an asset came from (an old manifest with no field reads as
  `"personal"`); `compute_states` stamps `asset_inventory.catalog_id` from
  the same source. Scans compose every loaded catalog via
  `registry::union_all`, and the unlayered guard (`refuse_unlayered`) checks
  against that same union, not just `personal`. `apply_override` rejects a
  layer that tries to change an asset's `scope`, since scope is what decides
  who may receive it.
- **Assets M3 — admissions, grants per catalog, loading every catalog** (plan
  `docs/superpowers/plans/2026-10-01-assets-m3-admissions.md`): migration 093
  adds `host_catalogs` (a host with no org admits an org catalog; `admit`
  refuses an org-bound host and `personal`) and `client_catalog_grants` (a
  grant names one catalog; the personal grant is also mirrored into
  `client_tokens.assets_admin_at`, which is read only while no personal
  catalog exists yet). `load_catalog(id)` loads any catalog; `ensure_fresh`
  walks every `catalogs` row — an org catalog that cannot load becomes a
  registry *problem entry* (`Catalog.load_error`, retried once its row's load
  record, `repo_path` or `remote_url` changes), a removed one is evicted, a
  personal failure is still the error. `effective_for_host` reads admissions
  and reports `speaks_for` / `held_back`: a manifest entry is an orphan
  (`Remove`) only when its own catalog speaks for the host; one whose catalog
  is not loaded, failed, no longer accepted (unadmit, org change) or not
  configured gets a `Noop` saying why — never a remove. `plan_sync` reads one
  `registry::snapshot()` for scan and plan. `service/catalog/catalogs.rs`
  adds / lists / removes catalogs (removal is config only and cascades their
  layer rows, admissions and grants) and admits hosts; `add_catalog` refuses
  moving an existing catalog to another org (`Store::check_catalog_owner`).
  `catalog_admin` takes an optional `catalog` (config, load, list_layers,
  set_host_layers, and the authoring actions since M4) and
  five actions (`list_catalogs`, `add_catalog`, `remove_catalog`,
  `admit_catalog`, `unadmit_catalog`); every action checks a grant on the
  catalog it touches (`AdminCall::touches` → `may_admin_catalog`) — the
  fleet-wide actions (`plan_sync`, `apply_sync`, `inventory`, secrets,
  `resolve_preview`…) touch `personal`, so an org-only grant covers only that
  catalog's config/load/layers/admissions —
  `list_catalogs` is master or an unbound full client only (an org-bound
  client is refused), `add_catalog` and `remove_catalog` are master-only
  (a removal cascades every other client's grant), and `apply_sync` fails
  closed for a non-master caller when its parked plan is gone and otherwise
  needs a grant on the catalog of every manifest entry an Update/Overwrite
  replaces, not only the catalogs its actions come from. Operator side:
  `fleet-hub catalog add|list|remove|admit|unadmit`, `catalog reload
  --catalog`, `client grant|ungrant <name> assets --catalog`.
- **Assets M4 — changesets** (plan
  `docs/superpowers/plans/2026-10-02-assets-m4-changesets.md`): migration 094
  adds `changesets`, `changeset_items` and `asset_triage_verdicts` (the
  spec's DDL verbatim; `applied_at` is Unix milliseconds, `created_at` and
  `decided_at` seconds). `service/catalog/changesets/` proposes cards by
  rule (`rules.rs`, pure): Bootstrap when nothing is bootstrapped yet or
  ≥ 20 unmanaged normal identities exist — grouped by host-set signature and
  name prefix into context layers (`everywhere`, `<host>-only`, `core`,
  `<prefix>`), `set_scope shared` for personal assets an org host has,
  `hide` for internals; New on host per identity; Drift (take the host copy
  or restore); Rollout for a never-rolled-out layer with `missing` members.
  The reconcile pass (`reconcile.rs`) refreshes them after every scan-tick
  pass while `catalog.auto` is on (`after_scan_pass`, `try_lock` — it never
  blocks the tick or touches its owed set) and on `changesets { propose }`;
  a card's subject is derived from its items; a verdict on `(kind, name,
  content_hash)` holds a subject until its content changes, and a `person`
  verdict is never replaced by another decider. Apply (`apply.rs`) needs
  clean checkouts (every untracked file counts), snapshots the touched
  catalogs' `host_layers`, imports per (catalog, source host), writes layer
  files and scopes, appends contexts, and commits only the exact files it
  wrote, once per catalog (`fleet: <summary>`); on any failure it puts back
  only its own files and the snapshot and commits nothing — and where
  anything foreign is in its way it resets nothing and the card says
  "manual cleanup needed"; success proposes a follow-up Rollout. Rollout
  runs `plan_sync` + `apply_sync` narrowed to create/adopt/update; a Drift
  restore (one asset, one host, its harness, a person's pick) may also
  overwrite, with a backup; nothing ever removes from a host. Undo
  (`undo.rs`) reverts the card's commits and restores the snapshot —
  replacing every `host_layers` row of each touched catalog — only for the
  latest applied card per catalog, on clean checkouts, and refuses when a
  path its revert touches sits on disk untracked; it does not un-hide, and
  a rolled-out layer stays rolled out (P27). Dismiss and reject_item write
  `rejected` verdicts. `catalog.auto` (on) hides internals, prepares cards
  and runs SB6's additive sync on layers a Rollout card has applied
  (missing assets and identical copies only — never a `drifted` one —
  skipping a host whose rollout a person rejected); `catalog.auto_push`
  (off) pushes after apply and undo. One tokio `APPLY_LOCK` serialises all
  of it and every authoring write. MCP `changesets { list | propose | apply
  | undo | dismiss | reject_item }` is never served to per-host tokens and
  is the master's or an unbound full person device's (org-bound, readonly
  and peer callers are refused before any card is read): `list` needs no
  more, `propose` the personal grant, `apply` a grant on every catalog its
  selected items name, undo/dismiss/reject_item one on every catalog the
  card names — plus personal when an item names no catalog (hide) or the
  apply writes hosts (rollout, restore), which also passes the `apply_sync`
  confirm gate. Also in M4: the authoring `catalog_admin` actions take
  `catalog` (`CatalogTarget`; `configure` stays personal) and a named
  catalog is resolved once per call; an asset whose own catalog file has a
  load problem is held (`ProblemHolds` → a `Noop`, never a `Remove`) — in
  `personal` too, the one deliberate change for a personal-only fleet;
  `fleet-hub catalog list` reads only (`probe_catalogs`: never clones or
  records a load) and `catalog remove` reports the open cards it withdrew.
  No Tauri command or verdict row yet (M6).
- **File downloads** (spec
  `docs/superpowers/specs/2026-10-03-file-downloads-design.md`, migration
  095, contract revision 7): `send_file { session_id, path }` (a host's
  Claude from its own host, or a person) stats the file, keeps the
  `downloads.*` budget and inserts a `fetching` row; `service::downloads`
  copies it in carry chunks into `<data dir>/downloads/<id>` (set by
  `downloads::init` in `fleet-hub serve` and the desktop's setup). Clients
  read `list_downloads` (not served to host tokens), re-read on
  `download:changed` (ids only, hidden from scoped streams) and fetch the
  bytes from `GET /downloads/<id>` (`mcp/downloads_route.rs`), never through
  a tool result. The GC sweep drops rows past `downloads.keep_secs`. Desktop:
  the footer's ⤓ Downloads sheet and the file viewer's *Send to downloads*;
  `save_download` picks the destination in its own save dialog.
- **Terminal** is a hand-rolled ANSI screen buffer (`src/lib/ansi.ts` +
  `TerminalView.svelte`), *not* xterm.js — xterm's renderer failed to repaint in
  the WKWebView setup. Only one PTY is attached at a time.
- **Hub daemon** (`crates/fleet-hub`): the same core headless; `hub.*`
  settings, `HubBase` in `service/hub.rs`.
- **Hub client mode** (`src-tauri/src/backend/`): a desktop paired with a hub
  (Settings → Hub) resolves once at startup to a window onto that hub; every
  command routes to a hub tool, refuses with `E_LOCAL_ONLY`, or is the same in
  both modes, under the rule *parity or refusal* in `docs/hub.md`. That
  verdict is written down once, in `backend/verdicts.rs`, for all 220
  commands; `backend/tests_routing.rs` reads the handler list from `lib.rs`, each command's
  body, and every routed call and refusal to it, and `backend/verdict_gen.rs`
  publishes it to `src/lib/hub_verdicts.generated.json` and the refusal table
  in `docs/hub.md`. Adding a command means: a row, then `route`/
  `refuse_local_only` **by command name** (never a second tool literal or a
  pasted sentence), then `REGEN_HUB_VERDICTS=1`, then — for a `LocalOnly`
  command the UI can reach — a `REASONS` entry or an allowlist line in
  `src/lib/hub_verdicts.test.ts`.

## Conventions

- Backend errors flow as `IpcError` (`ipc_error.rs`) with `E_*` codes; the
  frontend unwraps a `Result` type (`src/lib/result.ts`).
- Shell-quoting has **one** canonical implementation: `crate::shell::quote`
  (alias `shq`) in `crates/fleet-core/src/shell.rs`. Every value interpolated into an
  SSH/bash command string MUST be quoted with it. The former duplicate copies
  (`shell_quote`/`shell_quote_str`/`shell_escape`) were consolidated — do not
  reintroduce them.
- Every child process is built by `fleet_core::proc::command` /
  `std_command`, never `Command::new`: on Windows they set `CREATE_NO_WINDOW`,
  without which each `ssh.exe` a probe spawns flashes a console window.
  `no_eprintln_tests::production_code_spawns_through_proc` enforces it.
- SQLite access goes through `Store` behind a `std::sync::Mutex`. Never hold the
  guard across an `.await`.
- In tests, `Store::open_in_memory()` / `open_with_bus_in_memory` hand out a
  copy of a database migrated once per test process (`migrated_template_copy`);
  a test about the migrations themselves builds its database with
  `store::testgen` / `migrations_through` instead. The bundled SQLite is built
  with `SQLITE_DEFAULT_MEMSTATUS=0` (`.cargo/config.toml`), so it takes no
  process-wide lock per allocation.
- No blocking I/O under `Mutex<PtyState>` and none on a sync Tauri command (a
  sync command runs on the macOS main thread). PTY input goes to the writer
  thread through its bounded channel — `E_PTY_BUSY` when it is full,
  `E_PTY_CLOSED` when the thread is gone; kill / reap / fd teardown runs on the
  `PtyParts` taken out under the lock, after the guard is released.

## Status & known issues

Iterations 1–4a are landed (multi-host, accounts, cross-host sessions, prompt
transfer, async/events rework), plus the MCP control API, background sessions,
the background reconcile tick, fleet_health roll-up, and the persistent session
event timeline (session_history). Handoff from the original spec is replaced by
`move_session` (Move to host…) and Freeze is descoped, per
`docs/adr/0001-descope-freeze-ship-move.md`. `move_session` now CARRIES
uncommitted and unpushed work plus small git-ignored files to the target
instead of refusing a dirty or unpushed source (`strict: true` restores the
ADR 0001 refusals), and the session's Claude directory and project memory
(slice 2 spec `docs/superpowers/specs/2026-09-20-move-carry-claude-state-design.md`),
per `docs/adr/0002-move-carries-work-as-is.md` and
`docs/superpowers/specs/2026-09-19-move-carry-engine-design.md`. The move's UI
is the Transfer sheet (terminal-header chip + `moves.ts`, live steps from the
`move:progress` event), per
`docs/superpowers/specs/2026-09-20-transfer-sheet-design.md`. A full
hardening review is in
`docs/specs/2026-05-21-hardening-review.md` — consult it before touching SSH
command construction, the PTY, migrations, or the optimistic-merge / event-bus
paths.

The headless `fleet-hub` daemon, `fleet-agent` for hosts the hub cannot reach
over SSH, paired-client access for phones/browsers, and hub-client mode
(pairing the desktop itself to a hub) are landed; see `docs/hub.md`. Since contract revision 5 a hub client adds
projects through the hub (`add_project` / `list_github_repos` tools), per
`docs/superpowers/specs/2026-09-27-hub-add-project-design.md`. Host
reboot handling is landed in both halves, per
`docs/superpowers/specs/2026-09-17-host-reboot-session-survival-design.md` and
`docs/superpowers/plans/2026-09-19-host-reboot-recovery.md`: **survival**
(sessions are recovered by boot identity rather than declared lost on a
restart, migration 036 + `lost_reason`) and **recovery** —
`restore_host_sessions` batch-resumes a host's lost sessions over
`recreate_session`, `discover_lost_sessions` scans a host's Claude transcripts
for conversations fleet has no row for, and `new_session` takes a
`resume_claude_session_id`; the UI for both is in `HostDetail`. Recovery
reached `main` only on 2026-09-22: PR #161 was merged into the stacked branch
`feat/host-reboot-survival`, which was never re-merged after PR 1/2 (#135)
landed on its own, so for three days this paragraph described half a feature.
When a stacked PR says MERGED, check what it was merged INTO.

The work graph's M0–M3 are landed (work links anchored on participants,
group-by-work, and M2's work journal, carry rules, handover brief and resume
— `work` / `work_link` actions, `service/work/`; M3's trackers: Jira Cloud
read-only over `fleet-core::net`, the sync tick, `work_admin`, tickets /
lookup / start — `service/trackers/`, `store/trackers.rs`,
`store/tracker_items.rs`); read
`docs/superpowers/2026-09-24-work-graph-roadmap.md` before touching them.
The user guide is `docs/work-graph.md`: update it with any change a user
sees. Its `work.*` settings table is generated (see *Settings metadata*
below).
Tracker secrets are read ONLY by `Store::resolve_tracker_credential`.
Work graph M4 (detection) is landed: one recogniser in Rust and TS over a
shared fixture (`service/work/recognize.rs`, `src/lib/work_keys.ts`), the
pure resolver (`service/work/resolve.rs`, rules R1–R9 and R3u; state signals are
current, rejections are final), `detect.rs` wiring the prompt / Stop / PR
probe / sync triggers, migration 049, `SessionRow.work_suggested` (a guess
never groups a session), and the chip / popover / batch review UI. The
SessionStart context (M4.5) is built but OFF behind
`work.session_start_context` (decision D5; the remote-host numbers are the
user's to take with `scripts/measure-session-start.sh`). M4.6, the opt-in classification
nudge, is landed OFF behind `work.classify_nudge`: after three turns with no
link and 1–5 candidates in the host's scope, one UserPromptSubmit per
conversation carries a ≤400-char note after the mail
(`service/work/nudge.rs`, migration 056 `conversations.classify_nudged_at`; migration 057 re-issues the read-cursor delete trigger that a rewrite of 044 left out of some databases);
Claude's answer, `work_link { source: agent_inferred }`, is only ever a
pre-selected suggestion (rule R11, strength `inferred`).
Work graph M5 (organisations) is landed: migration 050 (`orgs`, text-keyed
`org_rules`, `hosts.org_id`, `work_links.snap_org_id`), `SessionRow.org_id`
(SQL, `session_org_sql!`, held equal to `store::org_of_session`), and the
org BOUNDARY for per-host tokens — `service::orgs::OrgScope`, made only by
`Caller::org_scope`, filters every work read in the service layer, and
`call_tool` redacts session rows' work in everything a host receives. M3's
per-host ticket fence is kept (composed with the org). Cross-org links need
`force_cross_org` for every caller; `isolate_sessions` (D7) is per org, off.
Any new `work` / `work_link` / `work_admin` action needs a row in the
isolation matrix (`mcp/tools/tests_isolation.rs`), which fails otherwise.
Work graph M6 (more providers) is landed: GitHub Issues (through `gh` on a
host, `transport = via_cli:<host>`, no token in fleet), Asana (`asana:<gid>`
keys, events-API sync tokens, the section map), Linear, and Jira Data Center
(admin-fenced site: exact host, resolve-then-refuse loopback / link-local,
optional `extra_ca`), all behind the `TrackerProvider` trait and the
`HttpTransport` seam (`net/via_host.rs` has `gh` and host-side `curl`, the
credential only ever on stdin), migration 051 (`tracker_views.sync_mark`,
`trackers.settings`). A new provider must pass the conformance suite
(`service/trackers/conformance.rs`, `conformance_suite!`) and
`tests_isolation_providers.rs`.
Work graph M7 (self-cleaning lifecycle) is landed: the pure planner
`service/gc/tidy.rs` (reasons, hard-coded protections), migrations 052
(UI-only archive, snooze / never per link, `sessions.last_touch_at`,
`work_items.reopened_at`) and 053 (`orgs.auto_tidy`), `work { tidy | reopened }` and
`work_link { archive | unarchive | snooze | never | dismiss | tidy_apply }`,
and auto-tidy in the GC sweep behind `work.auto_tidy` (OFF; safe kill only),
overridable per org. Tidy-up suggests; it never kills a dirty tree except
through safe kill, and a per-host token sees only its host's and org's
candidates.
Work graph M9.1 / M9.2 are landed: the Today view (`work { action: today }`,
Details' empty state and ⌘⇧T, a plain-text Copy standup) and the ticket
context card (`work { action: card }`, acceptance criteria from the cache,
Insert into composer with the hub-fenced `composer_text` — never sent).
M9.7 (the operator's starts and kills always confirmed; refused on a hub),
M9.3 (agent-written handover on demand, `work_link { action: handover }`)
and M9.6 (multi-repo start, `work_link start { project_ids }`) are landed
too. Write-back (D3), dead-session summaries (D10) and webhooks (D13) were
decided against; the user said yes to D3, D10 and D20 on 2026-09-27, as
M13.4e, M13.4c and M13.4a of M13. D13 stays no: its build (M13.4f) reached
`main` and was removed again (migration 062 stays, 064 drops its table;
build notes:
`docs/superpowers/plans/2026-09-26-work-graph-m13-decided-yes.md`).
M13.4c (D10) is landed (#327, fixed in #331):
`work_link { action: summarize }`, on demand only, one tool-less
print-mode fork on the session's own host (`service/work/summary.rs`,
`work.summary_model`), stored as a journal `summary` and fenced in the
brief.
M13.4e (D3) is landed too (#327, fixed in #332; tests #334): the PR
remote link on Jira only, per tracker `settings.write_back.pr_remote_link` (off), queued by the PR probe
for `manual` / `started` confirmed links in the tracker's own org, through
the outbox `tracker_writes` (migration 061) drained by the sync pass
(`service/trackers/write_back.rs`, `TrackerProvider::write`).
Work graph M10 (`docs/superpowers/plans/2026-09-25-work-graph-m10-settle.md`):
M10.4 is landed — Today's Stale opens Tidy-up narrowed to those sessions,
and the M5.5 filters (tracker / status / mine / has-session / archived)
have chips under the sidebar's "⚑ work" pill (`work_filters.ts`, through
`rowMatches`). The rest of M10 is landed too: the M9 review leftovers, the
work graph end to end in `scripts/hub-e2e.sh` against a loopback fake
tracker (the `e2e` feature; CI builds it as `WBIN` and fails without it),
the replay-ring numbers, and the phone's Today / ticket card; the written
acceptance run, `docs/work-graph-acceptance.md`, waits on the owner.
Work graph M11 (the long tail) is landed: "Name this work…" (`work_link
{ action: name }`, `work { local_items }`), resume probes the transcript,
tidy reason `idle_unlinked` (`work.tidy_idle_unlinked_days`, never
auto-tidied), GitHub Enterprise and per-tracker `SyncMetrics`
(`work_admin { status }`), and the tool budget paid back.
Work graph M12 (ship and operate) is landed: the upgrade test and
downgrade guard (`store::testgen`), the scale fixture and budget tests
(`service/work/scale_tests.rs`, migration 058), the `work.retention.*`
windows (`store/work_retention.rs`), trackers in `fleet_health` with a
Reconnect Attention item, and the review of the decided-against list
(`docs/superpowers/reviews/2026-09-26-work-graph-decisions-revisited.md`).
M13 (live use, `docs/superpowers/plans/2026-09-26-work-graph-m13-live-use.md`)
is closed (M13.5, #337): M13.1 (partial sync failures, #320), M13.2
(`work_admin { usage }`, #323 / #324), M13.4c and M13.4e above are on
`main`; M13.4a (D20) and M13.4d (D15, multi-start) are on fleet-mobile
(#51, #50). The work graph is *operating* (D26): new work is issues and
small plans. Two items stay open, waiting on the owner: the acceptance run
and its triage (M13.3), and D5 (M13.4b). Open decisions are the roadmap's table, and a decision-gated
feature starts only on the user's "yes".
Work graph M14 (the Work view: org → group → task → every session, and a
phone paired to one org) is the one milestone after it (roadmap D36): plan
`docs/superpowers/plans/2026-09-27-work-graph-m14-work-view.md`, design
`docs/superpowers/specs/2026-09-27-work-view-design.md`. M14.1a–d (the
backend: `work { tree | task | session_tasks | review | rules | … }` in
`service/work/view.rs`, `work_link { set_primary | place | assign_org | … }`
in `service/work/structure.rs`, migrations 066–067, org-bound clients as
`OrgScope::Org`, compare-and-set with `E_CONFLICT`, the desktop commands
and `work:changed`) and M14.2–M14.4 (the desktop Work view — `WorkTree`,
`WorkTaskDetail`, `WorkReview`, the rules / place / org dialogs, state in
`src/lib/work_view.ts` — and fleet-mobile's *My work*) are landed; the
desktop re-reads on `onWorkChanged` and the `workChanged` tick in
`work.ts`. `scripts/hub-e2e.sh` hub W section 10 runs the contract on a
real hub. M14.2 / M14.3 landed in #349 (fixes #357, #359, #361, #365) and
M14.4 as one PR, fleet-mobile#54; M14.5's docs are on `main`, so only the
owner's Part R run is open, and *Assign org…* / *Make a rule…* stay
desktop-only (owner, 2026-09-28; M14's D31–D36 and Jev's D31–D47 share
numbers, so write "M14-D3x" / "Jev-D3x").

The Jev evaluation (TypeSafe's decision model as an optional reader for
closed-set decisions) has started with a local language census: `fleet-hub
census languages` over `service::nl` (cargo feature `nl-detect`, lingua, ON
only in fleet-hub — the models add ~45 MB, kept there by D47). The decision
envelope is built and OFF (Jev spec D35–D37; the roadmap's D31–D36 are other decisions): `service::decide` (`gate` / `decide`,
`DecisionBackend`, `jev.rs` fenced to api.typesafe.ai), `decide.*` settings,
per-org consent `orgs.jev_allowed` (migration 068), the record
`decision_runs` + key `decision_secrets` (069; the key is read ONLY by
`Store::resolve_decision_credential`, never raw text in a run), `fleet-hub
decide`; guide `docs/decisions.md` (its `decide.*` settings table is generated). The first use case, J3 `status_map`, is built (shadow / assist
only, off): the Asana probe keeps `config.unmapped_sections` /
`project_sections`, `service/decide/status_map.rs` asks one Choice per
unclassified section after a clean sync (`StatusMapTrigger`, daily), and
`fleet-hub decide proposals` lists what a person applies with `fleet-hub
tracker section-map`; follow-ups are recorded in `work_admin update`.
Assist is usable one proposal at a time (`status_map::decide_proposal`,
by run id: apply / apply_as through `work_admin update`, reject → the
follow-up only, hidden until a new answer): Settings → Trackers on a
standalone desktop (`status_map_proposals` / `decide_status_map_proposal`,
`LocalOnly` when paired) and `fleet-hub decide proposals apply|reject`.
Phase 0 (offline) is built for J1 `work_link` and J3: `fleet-hub decide bench
work-link | status-map` (`service/decide/bench/`: BM25, leakage guard, time
split, calibration, the test map's acceptance lines, D39 `--export-unlinked`
/ `--labels`) with the `claude -p haiku` baseline (D33,
`service/decide/haiku.rs`: a named host of the SAME org only, prompt on
stdin). J1 has no live adapter: it waits on its acceptance lines.
Their diagnostics are built too (evidence, never an acceptance line):
`--perturb` (dataset C, `bench/perturb.rs`; J3 in `status_map_robust.rs`,
J1 in `work_link_robust.rs`), J3's paired languages (dataset B, `pair` ids,
`--paired-fixture`), `--floor-sweep` and `--question-set` (drafts in
`service/testdata/decide/questions/`, dev only), and `fleet_health.decide`
(`service::decide::health`, *degraded* per test map §7; the desktop's *Jev
degraded* Attention item). Label
hygiene (D34) is built: an agent never overturns a person's rejection,
`store::Decider` records `agent` / `agent_started` vs `manual` / `started`
(`PERSON_SOURCES` gate write-back, auto-trust and person counts), and a
person's Clear work holds against the unchanged branch / PR (R9u, migration
070 `work_unlinks`). The test map is
`docs/superpowers/specs/2026-09-27-jev-test-map.md`.
Decisions D31–D47 and what is still open
are in `docs/superpowers/specs/2026-09-27-jev-language-census-design.md`.

Reply actions are landed (#338): Copy, Quote, Retry, Fork here and Rewind
here under each reply; Fork, Rewind and Retry are one operation,
`rewind_conversation` (`service/rewind.rs`), which copies the transcript up
to the anchor into a new conversation and never changes the original.
Retry (the client's rewind + `send_prompt`) is offered only when
`ConvTurn.prompt_partial` is false. A rewind is refused unless the session
is quiet (live pane probe first) and without an anchor; a failed restart
reverts the binding (`Store::revert_rebind`) and removes the copy. Fork into
a NEW worktree (`new_worktree`, the Fork sheet's default) creates the
worktree first — a fresh branch at the source's HEAD, uncommitted changes
not carried — then writes the copy under its `pwd -P`, then starts in it;
a failure after the worktree removes the copy, the tree, the branch and
the row. Spec
`docs/superpowers/specs/2026-09-26-reply-actions-design.md`.

Session state machine hardening (plan A, #343) is landed: a `working` row
with no activity for `reconcile.stale_working_secs` turns `idle` with
`stale_working_at` (migration 065); a StopFailure reads as failed; one
threshold, `health.context_red_pct`, drives `context_full`; the `oom`
playbook is capped by `playbooks.oom_max_attempts`; `gc.external_lost_ttl_secs`
ages out lost external rows. Attention reasons `stop_failed`,
`context_full`, `stale_working`, `ci_failing`. Plan
`docs/superpowers/plans/2026-09-27-session-state-machine.md`.

The stale-working acknowledgement (#381) is landed, per
`docs/superpowers/plans/2026-09-28-stale-working-acknowledge.md`: the tick
runs `Store::expire_stale_working`, which lifts the `stale_working_at`
stamp once the row is working / blocked again or older than
`reconcile.stale_working_ttl_secs`; an attach (`touch_session`) clears it
too. Migration 080 adds `stale_demoted_at`, the reconcile veto's own
memory: cleared by a hook, a pane that shows a live turn, or the row being
`working` / `blocked` again — never by an attach or the TTL — and while it
is set a turn-over check asks the pane (`store::trusted_status`). It is a
`#[serde(skip)]` `SessionRow` field: off the wire, and a change to it alone
emits nothing. Migration 081 adds `pane_working_at`, so a long tool call
whose spinner is on screen is not stale. The sweep judges only rows a
reconcile pass observed within the window (`last_reconciled_at`), so an
unreachable or unprobed host's `working` rows are never demoted.

Hub ops and accounting (plan D, #344) is landed: `fleet_health.hub`
(uptime, reconcile timing), process gauges on `/metrics`, a transcript's
first read booked as `backfill` apart from the day's live cost (migration
071 re-keys `usage_daily` by `(day, host_alias, backfill)`), and
`deploy/hub/backup.sh` / `upgrade.sh` and the `behind-proxy` compose; see
`docs/hub.md` → *Backups* / *Upgrade with the script*, plan
`docs/superpowers/plans/2026-09-27-hub-ops-accounting.md`.

Host identity and health (#354) is landed, per
`docs/superpowers/plans/2026-09-27-host-identity-health.md`: migrations
076 (`hosts.claude_version_at`), 077 (the health sample — disk / load /
mem / uptime, `health_at`, `last_hook_at`, `agent_version`) and 078
(`provision_fingerprint` / `provisioned_at`). The reconcile probe reads
versions every `VERSIONS_REFRESH_SECS` (6 h) and the health sample every
pass, which rides `host:pinged`; `fleet_health.hosts[]` (`disk_low` /
`claude_behind` / `agent_behind` / `hooks_silent`) is judged against
`health.version_max_age_secs`, `health.disk_low_pct`,
`health.claude_max_behind` and `health.hooks_silent_secs`. One rule,
`service::hosts::active_hosts`, picks the hosts of every host loop: a
hidden host is skipped by reconcile, not reaped. `merge_host` /
`fleet-hub host merge <from> <into>` retires a renamed alias (its
worktrees, sessions, usage, asset inventory, org rules and org move to the
target); a provisioning records its content fingerprint, so an older one
reads `provision_stale` (`fleet-hub provision --host <alias>
--content-only` refreshes it); `forget_project` drops a project row, and
`refresh_projects` drops rows that vanished.

Conversation event tracking is landed end to end (migration 037
`conversations` table; `SessionStart`/`PreCompact`/`PostCompact` hooks;
`/clear`, `/resume` and compaction tracked as conversation switches;
`session_conversations` API; the Conversations UI panel), per
`docs/superpowers/specs/2026-09-18-conversation-events-design.md`.

The desktop builds for Windows as a **client** (plan
`docs/superpowers/plans/2026-09-27-windows-desktop.md`, user guide
`docs/windows.md`): no `local` host (`retire_local_host`, as on a hub with
`hub.local_host=false`), no ssh multiplexing (`ssh::mux_supported`, off
there), the home/cache dirs through `fleet_core::home` only, and the hub
token in Credential Manager. Unix-only code and tests stay `#[cfg(unix)]`
(for a test module: a `#[cfg(unix)]` line above a bare `#[cfg(test)]`, the
form `no_eprintln_tests` recognises); in CI, `rust-windows` keeps the tests
and the Windows leg of `clippy` keeps the lints green there. `fleet-agent`
and `fleet-hub` stay Unix-only.
On Windows a WSL distribution is a host (`fleet_core::wsl`, alias
`wsl-<name>`): `SshClient::remote_command` and the PTY attach run it through
`wsl.exe … sh -c` instead of `ssh`, and it gets no reverse tunnel. The `ssh`
program is `ssh::default_ssh_binary()` everywhere (probes, PTY, tunnels):
`CLAUDE_FLEET_SSH`, else the Windows OpenSSH, else PATH. The Windows
bundle ships Microsoft's ConPTY (`conpty.dll`/`OpenConsole.exe`, which
portable-pty prefers to the built-in one) via `scripts/fetch-conpty.sh`
(pinned version + SHA-256) and `--config src-tauri/tauri.conpty.conf.json`
in release.yml and ci.yml; plain dev builds use the system ConPTY.

Hub↔hub federation (cycle 3) is landed: two `fleet-hub` daemons link with
`fleet-hub pair --mode peer` / `peer add|list|remove`, a dialer supervisor
and a `peer_exchange` listener carry messages both ways by fleet address,
and `fleet_health.peer_links_down` reports a link in trouble, per
`docs/superpowers/specs/2026-09-24-hub-federation-design.md`.

Application updates: design
`docs/superpowers/specs/2026-09-28-update-channel-design.md` (with
fleet-mobile's `docs/superpowers/specs/2026-09-28-mobile-update-adapter.md`).
The Hub is the policy authority and the release key (minisign) the content
authority. The manifest is two signed documents, a per-release manifest plus
a per-track channel doc on the `update-channels` branch. The `/update` wire is
frozen and exempt from `E_HUB_CONTRACT`. `fleet-updater` rolls the hub
container back, including the DB restore. **S1 is landed:**
`crates/fleet-update` (Tauri-free, no fleet-core dependency; version-exempt
like fleet-core) holds the manifest / channel types, `verify` (`verify_target`
is the one check before any install), the pure `decide()` over the shared
fixture `tests/decide_cases.json`, `UpdatePhase`, and `UpdateChannel` with
`GitUpdateChannel` / `HubUpdateChannel`. **S5 is landed too:**
- `fleet-hub backup [--prefix|--to] --json` (`store::backup`, a
  read-only `VACUUM INTO`, never migrates);
- `fleet-hub healthcheck --ready --json`, which reads the readiness file
  `serve` rewrites every 5 s (`fleet-hub/src/ready.rs`, `<data
  dir>/run/ready.json`), so `/healthz` stays unversioned;
- the build identity from `crates/fleet-hub/build.rs` (`FLEET_GIT_SHA` /
  `FLEET_BUILD_ID`, passed by `release.yml` and the `hub-image.yml` build
  args).

**S4a (the hub side) is landed:**
- migration 079 (`update_desired`, `update_observed`, `update_events`, and
  `update_docs`, the signed-document cache, re-verified on every read);
- `service/update/` (`check` / `report` / `status` / `pin` / `refresh`,
  plus the refresh tick in `fleet-hub serve`, which records `hub:self`);
- `POST /update/check` and `POST /update/report` (`mcp/update_route.rs`,
  behind `authorize`; the caller's identity comes from its token);
- `TokenMode::Updater` (`fleet-hub pair --mode updater`): `/update/*` only,
  refused by every tool, `/events` and `/report`;
- the tools `update_status` (client, read-only; a scoped caller sees only
  itself) and `update_admin` (master only);
- the `update.*` settings, with their Settings → Updates rows, and the user
  guide `docs/updates.md` (every `update.*` setting must be in its table).

**S2 (publishing) is landed:** release.yml's `manifest` job signs
`release-manifest.json` from 0.4.1 (`scripts/release-manifest.sh`, the
windows from the shipped `fleet-hub compat`; `verify-release` requires it
through the `manifest` leg of `release-assets.sh`), its `channel` job and
`update-channels.yml` write the signed `stable` / `beta` channels on the
orphan branch `update-channels` (`scripts/update-channels.sh`, the
`fleet-release` bin of fleet-update), and every document is checked
against `keys.rs` before it leaves the runner
(`scripts/release-update-scripts-test.sh`, CI hub-headless). See
`docs/RELEASING.md` → *Update manifest and channels*.

**Half of S4b is landed:** `X-Fleet-Client` (`fleet_update::client_header`)
recorded into `update_observed` on `last_seen_at`'s once-a-minute beat in
`authorize`; the `update:changed` row event (kind `update`, ids only, in
`HOST_BOUND_HIDDEN_KINDS`); `fleet_health.updates` (`service::update::health`:
`update_required`, `update_failed`, `update_rolled_back`, `rollback_failed`,
`channel_stale`); and `update_status { target }`, the design's
`update_check_for`. Left: the per-target `update:decision` push, hub-e2e
section U, rollouts (S9).

**S3 is landed:** `service::update::git_check` (Git mode: a `GitCheck`
from the hub's own settings, pin and last-seen sequence) and `fleet-hub
update check [--track] [--json]`, which reads the published channel and
prints what this build should run, verified; it installs nothing.

Trusted keys are `fleet_update::keys::RELEASE_KEYS`: the owner's release
key (made on the owner's machine only, `scripts/release-key.sh`) is trusted
since a3033c2 / #384 (v0.4.1); the secret half is only the
`RELEASE_SIGNING_KEY` repository secret and the owner's backup. Nothing is
offered until 0.4.1 publishes the first channel. `FLEET_UPDATE_E2E_KEYS`
(read by `e2e` builds only) is reserved for S4b's hub-e2e section U;
nothing uses it yet. `update.track` offers `stable` / `beta` only until S2b
publishes `nightly` (a stored `nightly` resolves to `stable`). S2b
(nightly), the rest of S4b and S6–S9 are not built; the
other §13 questions wait on the owner.
