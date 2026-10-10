# Architecture

How claude-fleet is put together, by subsystem: where each piece lives, the
invariants it keeps, and the `REGEN_*` command for anything generated from it.
Moved out of `CLAUDE.md` (which every session loads) on 2026-10-06; read the
bullet for the area you are about to change.

- **Frontend stores** (`src/lib/*.ts`) hold app state in `svelte/store`
  stores; row lists are built on `createRowStore` (`src/lib/row_store.ts`).
  Backend mutations emit row events (`events.rs` → `events.ts`
  `subscribeToRowEvents`); the frontend patches stores in place (each
  store's own `mergeSession`/`removeSession`, `mergeTask`, `mergeProject`,
  …) instead of re-fetching. Mutation wrappers also do an optimistic patch
  from the command's return value.
- **Backend** (`crates/fleet-core/src/`, thin Tauri handlers in
  `src-tauri/src/commands/`): the handlers wrap the transport-agnostic logic in
  `service/`; SSH multiplexing in `ssh.rs` (per-host `ControlMaster`, async
  `tokio::process`); tmux command construction in `tmux.rs`; SQLite in `store/`
  (migrations are registered in the `MIGRATIONS` table there — add a new
  `NNN_<topic>.sql` plus an entry); the event bus in `events.rs`; cancellation
  registry in `cancel.rs`. The PTY map (`pty.rs`) stays in `src-tauri`,
  since it is desktop-only.
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
- **Background loops** (`service/loops.rs`, redesign 8.1): every periodic
  job (reconcile and the jobs riding its tick, missions, tracker / catalog /
  local syncs, peers, updates, host refresh) reports its last run, next run
  and result to one registry that `fleet_health.loops` lists; every job
  that acts on the person's behalf (`loops::acts`) asks `loops::gate` first
  and stands still while `automation.paused` is on, and the rest
  (`loops::keeps`) carry the reason they keep running, which the Automation
  view shows. A new loop needs a `LOOPS` row, a `loops::report` call and,
  when it acts, a gate (`every_loop_reports_and_every_pausable_one_is_gated`)
  and a `pause_all_stops_*` behaviour test next to its pass.
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
  A table row may carry `transfer: { column, percent }` (not a column):
  `DataItem.svelte` draws the transfer loader beside that cell, the
  Progress ring for a known `percent`, Data rain for `null`
  (`transfer_loader.ts`, step 10.10); Settings › Updates' rows carry it
  while an update is in flight (`src-tauri/src/commands/updates.rs`).
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
  Its Tauri commands and verdict rows arrived with M6.
- **Assets M5 — the workspace shell** (plan
  `docs/superpowers/plans/2026-10-04-assets-m5-workspace.md`): a manifest
  entry records the sha256 of every file it wrote (`file_hashes`), so
  `ManifestEntry::host_copy_for` tells a copy fleet left untouched
  (`Unchanged`) from one a person edited (`Edited`) or one an entry from
  before M5 cannot vouch for (`Unverified`); rule 4 of the planner turns an
  edited copy into an `overwrite` (never an `update`), and Rollout/SB6
  (`OpFilter::Additive`, `action_allowed`) apply an `update`, or delete a
  moved asset's old location, only over a verified `Unchanged` copy.
  Migration 096 stores
  `asset_inventory.drift_side` (`host` | `catalog`; also on `HostState`): a
  copy only behind its catalog opens no Drift card, shows under *Behind the
  catalog* in the Inbox whatever `catalog.auto` says, and SB6 brings it up
  (`sb6_due`). What SB6 writes shows as a card (redesign 8.7,
  `record_auto_card`): an applied Rollout, one `sync` item per host and
  layer, carrying `AUTO_SYNC_NOTE`, which `ChangesetSummary.auto` reads and
  the Inbox keeps under *Recently applied*. It has no Undo: a host write is
  not a catalog commit, so it is changed by syncing the host. Migration 097 stamps `changeset_items.decided_at`;
  `rejected_rollouts` orders by it. Slug collisions in a Bootstrap card
  (per destination catalog, kind and slug) need a look; `changesets::list`
  computes undoability once (`undoable_ids`); the pass prunes an untouched
  withdrawn card a week after its withdrawal (`WITHDRAWN_RETENTION_SECS`;
  migration 103 stamps `changesets.withdrawn_at`, cards withdrawn earlier
  fall back to `created_at`); a New card whose slug another candidate or
  the catalog holds needs a look; `last_sync`
  prefers a person's run. `list_assets` takes an opt-in `all_catalogs`
  (default personal-only, as before, for every caller; with it, every
  loaded catalog — `AssetSummary.catalog`, host states per catalog — for
  the master and unbound full person devices, personal for everyone
  else); this desktop asks for it locally and through the hub.
  `catalog_admin { asset_history }` lists an asset's commits. Four read-only desktop
  commands route to the hub (`catalog_list_catalogs`,
  `catalog_list_changesets`, `catalog_repo_status_in`,
  `catalog_asset_history`); the card verbs came with M6. Frontend: `AssetsPanel`
  keeps loading, probing and every dialog and renders `AssetsWorkspace`
  (rail Inbox/Library/Secrets, a sentence header, `QueryInput`, the Inbox's
  sections with open cards read-only, `AssetList` as the Library, the
  tabbed `AssetInspector` over `AssetDetail`'s `section`s and History, a
  footer of `CatalogChip`s, `auto` and `JobChip`; Sync fleet the one
  primary; a hub client without a grant gets one read-only scope chip); one
  `Badge`, `HostStrip` states, no hex colour in Assets components
  (`assets_tokens.test.ts`). Keyboard (R22, on the workspace while shown):
  `j`/`k` (arrows in the list) move between rows, Space/Enter select, `/`
  focuses the query (Esc there closes, clears, then returns to the list),
  `a` adopts an identity, `s` syncs and `e` edits a personal asset, `⌘↵`
  runs Sync fleet; never inside a field or a dialog; read-only windows move
  and select only; `i` (M6) rejects a card's pending items.
- **Assets M6 — cards on the desktop** (plan
  `docs/superpowers/plans/2026-10-05-assets-m6-cards.md`): the card verbs are
  desktop commands (`catalog_get_changeset`, `catalog_apply_changeset`,
  `catalog_undo_changeset`, `catalog_dismiss_changeset`,
  `catalog_reject_changeset_items`, `catalog_propose_changesets`,
  `catalog_propose_layer_change`) that route by their own name to the hub's
  `changesets` tool (standalone: the fleet-core functions); the hub checks the
  grant per catalog the card touches and runs its confirm gate for a rollout
  or restore apply, and a hub older than M6 refuses the new actions with
  `E_INVALID`, which the desktop words as "the hub is older than this
  desktop" (`olderHubWords`; no contract bump). Layer cards: `changesets {
  propose_layer }` takes a `LayerChange` (create, rename, move a member)
  and writes a `layer` card the person applies and may undo; reconcile never
  refreshes or withdraws one. `catalog_admin { drift_diff }` (read) answers
  a drifted asset's two texts for the Drift card's Take / Restore: the
  rendered files only (`plan.files`, never a config merge), placeholders
  kept (the catalog side is never secret-substituted), 256 KiB a side
  (`truncated`, `binary`, `merges_only`). A held line (a copy a Rollout
  would not touch) is the item's `changeset_items.outcome` (migration 102,
  JSON): every Overwrite of the card's own asset is held under Additive,
  as is a copy fleet cannot vouch for; an untouched withdrawn card is pruned
  a week after `withdrawn_at` (migration 103). Frontend: the Inbox's cards
  are `ChangesetCard`s (apply, undo, dismiss, ✕ per item, one primary for the
  selected card; `⌘↵` runs it, else Sync fleet), the Inspector's Diff tab is
  `DriftPanel` over `DiffView`, and `SyncPlanView` replaces the modal
  `SyncPlanDialog` (a plan or a Rollout card's read-only review takes the
  main column; "Plan anyway" stays host-scoped; Esc closes only it). The rail
  gains **Layers** (by catalog, with footprints; create / rename / move make
  cards) and **Hosts** (org, role per catalog, admission toggles — a hub
  client may toggle only a catalog it holds a grant on — and the Inspector's
  "on oci via layer core from personal" provenance, `resolve_preview`'s
  `ResolutionView`). **Settings → Catalogs** is a `catalog` page resource
  (add name/path/remote/org, remove, admit hosts); grants are shown as the
  hub command `fleet-hub client grant CLIENT assets --catalog NAME`, never
  edited here. The quick switcher lists assets and the commands Rescan
  assets / Sync fleet / Propose cards after everything else; App fills the
  `catalog` store once at launch (`primeCatalog`) so ⌘K works before Assets
  is opened. A sync run's `ActionResult.catalog` keys a secret-blocked asset
  as `catalog:kind/name` (a result without one is personal's). `a` / `s` /
  `e` / `i` act from the list or the Inspector only, and the layout narrows
  (rail to icons under 1100 px, the Inspector under the list under 860 px).
- **Chat forms** (spec `docs/superpowers/specs/2026-10-07-chat-forms-design.md`,
  guide `docs/forms.md`; `pages/forms.rs`, `store/forms.rs`,
  `service/forms.rs`, the `ask` tool in `mcp/tools/forms.rs`, migration 119,
  `src/lib/forms/`): an agent opens a `fleet.form/1` form in its session's
  chat; a person answers; secret fields go to the host as files, never into
  the database, a log or the audit row (`ask`'s `values` is rendered
  `<N fields>`). Regenerate `docs/form-spec.schema.json` with
  `REGEN_FORM_DOCS=1 cargo fleet-test -- form_docs_are_current`.
- **Runs** (Orbit Fleet 8.3; `store/runs.rs`, `service/runs.rs`,
  `mcp/tools/runs.rs`, migration 141 (indexes only), desktop `list_runs` →
  hub `runs`): one `UNION ALL` over `tasks`, `orchestration_events` (only
  `MISSION_RUN_EVENTS`), `decision_runs` (never `bench`), `aux_usage` and
  `routine_runs`, newest first. Each branch carries its own time, filter and reach clauses
  on its own indexed columns (`the_union_walks_its_indexes` pins the plan),
  so `total` and the page agree. The column mapping per source is written
  on its branch constant. `store/` takes the reach as ids
  (`RunsReach`); `service::runs::reach` derives them from the `ViewScope`
  (sessions it sees, missions and routines it may read, whole-fleet
  spend).
- **Debug devices** (guide `docs/debug-devices.md`; `service/debug_devices/`,
  `store/debug_devices.rs`, migration 120, `mcp/tools/devices.rs`, contract
  revision 10): a scan runs `scripts::scan_script` on a host (adb,
  emulator, `xcrun simctl` / `devicectl`) and `debug_devices_apply_scan`
  upserts by `(host_alias, dev_key)`; an unseen device turns `missing`,
  keeping a person's label and `shared`. `list` is cache-first and rescans
  hosts older than `STALE_SECS` in the background. Every operation runs on
  the device's host through `run_shell_bounded`; `run` takes a closed verb
  set per tool (`ADB_VERBS`, `SIMCTL_VERBS`, `DEVICECTL_VERBS`), never a
  host path; `install` relays an app between hosts with `carry::chunk_script`
  and `run_with_stdin`. Reach: a person by org scope; a per-host token its
  own host's devices plus `shared` ones on hosts of its org; `configure` /
  `forget` are a person's. Claims are advisory leases in the row. The
  desktop's Debug devices page is the `debug_device` resource; its seven
  commands route to the hub's `debug_devices` tool.
- **Chat blocks** (guide `docs/chat-blocks.md`; `src/lib/rich_blocks.ts`,
  `src/lib/RichText.svelte`, `src/lib/rich/`): frontend only. An assistant
  text block is split into Markdown and cards: a `FLEET_TASK_DONE_<nonce>`
  line and its JSON (normalised as `service/work/report.rs`
  `report_from_value` does), a ```` ```fleet-ui ```` `fleet.ui/1` block, or
  a work handover between `WORK_HANDOVER_BEGIN_<nonce>` /
  `WORK_HANDOVER_END_<nonce>` lines (`service/work/agent_handover.rs`
  asks for it; `src/lib/handover.ts` reads it into sections by the headings
  that prompt names, and a hand-off with none it knows stays one body).
  A card acts only by `insertIntoComposer`, never by sending. A reply form
  refuses secret fields; `ask` is the path for those. Two cards act through
  the store, each only after a person's press: `setting` applies one
  settings proposal (`decide_setting_proposals`, after a confirm), and
  `guide` with `page` draws a guide fleet has with the Settings `PageView`.
- **States kit** (`src/lib/states/`): the shared loading, empty,
  no-results and offline-host states. `Skeleton` shows only after
  `LOADING_DELAY_MS` (400 ms); `EmptyState` says what happened and offers
  the next step (`kind: 'none'` always has a way out); `HostOffline` sits in
  the host's own group or pane, never window-wide. The hub banner
  (`HubConnectionBanner.svelte`) counts the backoff down live, and *Retry
  now* calls `hub_retry_now`, which wakes the event bridge's wait
  (`HubConnectionStatus::retry_now`).
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
  a tool result. The GC sweep drops rows past `downloads.keep_secs`. A row
  being copied carries `fetched_bytes` (in memory, `download:changed` after
  each 8 MiB slice; never stored). Desktop: the footer's ⤓ Downloads sheet
  (progress with the time left, *Retry* = `send_file` again for the same
  session and path, *Show in Finder* for a file saved in this window, *Clear
  finished*) and the file viewer's *Send to downloads*; `save_download` picks
  the destination in its own save dialog. The same sheet's Notifications tab
  is the notification centre: every toast this window showed
  (`src/lib/notifications.ts`), its button offered only while the toast is up.
- **Voice relay F1** (spec `docs/superpowers/specs/2026-10-05-voice-relay-design.md`, plan
  `docs/superpowers/plans/2026-10-05-voice-relay-f1.md`, guide `docs/voice.md`):
  `service/voice` `VoiceRegistry` (one claim per session, process-global),
  `mcp/voice_route.rs` (`/voice/capture` host token only; `/voice/source`
  websocket = a client's claim), host stand-in `tools/voice/arecord`
  provisioned to `~/.claude-fleet/voice/bin` and put first on `claude`'s PATH
  by `tmux::VOICE_PATH_PREFIX`; desktop `src-tauri/src/voice/` (cpal, macOS /
  Windows only) and the 🎤 `MicToggle`. `voice.enabled` off by default. Audio is
  never stored.
- **Terminal** is a hand-rolled ANSI screen buffer (`src/lib/ansi.ts` +
  `TerminalView.svelte`), *not* xterm.js — xterm's renderer failed to repaint in
  the WKWebView setup. PTYs live in an id-keyed map (`PtyState` in `pty.rs`,
  at most `MAX_PTYS` open): each id has its own child, reader and writer
  threads and 1 MiB output cap, and every `pty_*` command names its id. The
  agent pane uses `agent`; opening an id replaces only that id's PTY.
- **Local workspace sync** (`service/local_sync/`, `store/local_workspaces.rs`,
  migration 109; spec
  `docs/superpowers/specs/2026-10-07-local-workspace-sync-design.md`): one
  link per worktree (host, owner/repo, worktree key), never per session. A
  pass scans both sides, hashes (local) or downloads (remote) only what moved
  since the BASE, asks the pure `plan::decide`, and writes under guards on
  both sides — a target that moved is left for the next pass, never
  overwritten. The host side is three bash scripts over
  `SshExec::run_with_stdin` (GNU and BSD userlands); `.gitignore` is git's own
  answer there and the `ignore` crate's here, plus `excludes::DEFAULT_EXCLUDES`
  and the link's patterns. The tick (`spawn_local_sync_tick`) is started by
  `bootstrap::tasks` in both modes: the folders and the SSH are this
  machine's, so every command is `SameInBoth`, and a paired desktop reads the
  session and project from the hub (`commands/local_workspaces.rs`). Row event `local_workspace:changed` carries the id only.
  Phases 2 and 3 (migration 112; spec
  `docs/superpowers/specs/2026-10-07-local-workspace-handoff-design.md`):
  each pass records which side carried each path
  (`local_workspace_activity`, nothing on the first pass); `local_sync::git`
  runs status / diff / commit / discard / compare in the worktree by its
  path, so no session is needed; `local_sync::handoff` composes the Ask AI
  and driver prompts in fleet-core and `src-tauri` delivers them through
  `sessions::send_prompt`; `local_sync::open` builds the Open in IDE command
  per OS.
- **Hub daemon** (`crates/fleet-hub`): the same core headless; `hub.*`
  settings, `HubBase` in `service/hub.rs`.
- **Hub client mode** (`src-tauri/src/backend/`): a desktop paired with a hub
  (Settings → Hub & sync) resolves once at startup to a window onto that hub; every
  command routes to a hub tool, refuses with `E_LOCAL_ONLY`, or is the same in
  both modes, under the rule *parity or refusal* in `docs/hub.md`. That
  verdict is written down once, in `backend/verdicts.rs`, for every
  command; `backend/tests_routing.rs` reads the handler list from `lib.rs`, each command's
  body, and every routed call and refusal to it, and `backend/verdict_gen.rs`
  publishes it to `src/lib/hub_verdicts.generated.json` and the refusal table
  in `docs/hub.md`. Adding a command means: a row, then `route`/
  `refuse_local_only` **by command name** (never a second tool literal or a
  pasted sentence), then `REGEN_HUB_VERDICTS=1`, then — for a `LocalOnly`
  command the UI can reach — a `REASONS` entry in `src/lib/hub.ts` or an
  allowlist line in `src/lib/hub_verdicts.test.ts`.
