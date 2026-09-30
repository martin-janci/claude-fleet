# Assets S1b + S2: catalogs, scopes, changesets and the workspace shell

**Date:** 2026-09-30
**Status:** Design — not yet implemented
**Parent:** `2026-09-29-assets-workspace-design.md` (the umbrella; its
decisions AW1–AW8 bind here) and its mockups
`2026-09-29-assets-workspace-mockups.html`, which are the visual reference
for everything in *Workspace shell*.
**Builds on:** S1a (PR #410): unmanaged rows with `host_hash` /
`secret_like` / `fleet_owned` (migration 087), `list_assets.identities`,
`scan_tick.rs`, import from any registered host, the unlayered-sync guard.
**Scope:** S1b (multiple catalogs and scopes) and S2 (Inbox, Layers, Hosts,
changesets, the shell) delivered together, on one branch, in seven green
milestones. Out of scope: S3 drafts and focus mode, S4 skill tests, S5 Jev,
and the fleet-mobile cards (their own PR in that repo, using the MCP verbs
defined here).

## Decisions taken during design

| # | Question | Decision |
|---|---|---|
| SB1 | Default scope of an asset in the personal catalog | **private**. After migration an org host (trn) receives nothing from `personal` until an asset is marked `shared`; the Bootstrap card proposes which. |
| SB2 | How much UI | **The full S2 shell now**, folded together with S1b. |
| SB3 | Layer names | From rules (prefix family, `core`, `everywhere`, `<host>-only`). haiku naming moves to S5. |
| SB4 | Push after a card is applied | Not automatic; the footer chip pushes. `catalog.auto_push` exists, off by default. |
| SB5 | Undo | `git revert` of the card's commits plus the stored `host_layers` snapshot; only the latest applied card per catalog (a stack). |
| SB6 | Automatic apply without a card | Only additive ops (adopt, create, backed-up update) on layers that have been rolled out at least once. A layer's first rollout is always a card. |

## Design coverage

Every element of the mockups and where it lands. Nothing in screens 1–4 is
left for later.

| Mockup element | State after S1a | Milestone |
|---|---|---|
| Rail (Inbox, Layers, Hosts, Library, Secrets) | missing | M5 |
| Sentence header ("Fleet converged · 5/5 hosts · scan 3m") | missing | M5 |
| Identity rows + `HostStrip` | done (S1a) | — (reused) |
| Scope / catalog badge on rows | missing | M5 |
| Lifecycle pill (`draft`, `canary`) | missing | S3 |
| Test glyph (`✓12`, `◐1`, `○`) | missing | S4 |
| Inspector pane with tabs (Overview, Source, Hosts, History) | missing | M5 |
| Footer: catalog chips, `auto`, `JobChip` | missing | M5 |
| Token query (`/`, `host:` `kind:` `state:` `layer:` `catalog:` `scope:`) | missing | M5 |
| Keyboard (`j/k`, `space`, `a`, `i`, `s`, `e`, `⌘↵`) | missing | M5 |
| QuickSwitcher `asset` kind + commands | missing | M6 |
| **Screen 1** First run: Bootstrap card (groups, decider badges, needs-a-look chips, hidden internals, Adopt / Dry run / Edit layers, commits + Undo, then Rollout card) | missing | M4 (engine), M6 (UI) |
| **Screen 2** Inbox: Needs you / Drifted / New on hosts / In sync sections, drift diff with Take / Restore, new-asset suggestion | missing | M5 (sections), M6 (diff, cards) |
| Jev confidence badge on a suggestion | missing | S5 (shown as `rule` until then) |
| **Screen 3** Layers and hosts: layers by catalog with footprint, host rows with org and role, accepted catalogs, "why is it on oci?" provenance | missing | M6 |
| **Screen 4** Library: all assets once, grouped by kind, managed assets read-only | partial | M5 |
| Settings → Catalogs | missing | M6 |
| Hub read-only scope chip | missing | M5 |
| **Screens 5–7** author, test results, overlap | missing | S3, S4 |
| **Screen 8** mobile cards | missing | fleet-mobile PR |

## Data model

One migration (the next free number at implementation time, guarded like
087 wherever it adds columns).

**New tables**

```sql
CREATE TABLE catalogs (
  id             INTEGER PRIMARY KEY AUTOINCREMENT,
  name           TEXT    NOT NULL UNIQUE,         -- 'personal', 'papayapos'
  repo_path      TEXT    NOT NULL,
  remote_url     TEXT,
  org_id         INTEGER REFERENCES orgs(id) ON DELETE RESTRICT,  -- NULL = personal
  head_commit    TEXT,
  last_loaded_at INTEGER,
  created_at     INTEGER NOT NULL
);
CREATE UNIQUE INDEX idx_catalogs_personal ON catalogs((org_id IS NULL)) WHERE org_id IS NULL;

CREATE TABLE host_catalogs (          -- explicit admissions (hosts with no org)
  host_alias TEXT    NOT NULL REFERENCES hosts(alias) ON DELETE CASCADE,
  catalog_id INTEGER NOT NULL REFERENCES catalogs(id) ON DELETE CASCADE,
  admitted_at INTEGER NOT NULL,
  PRIMARY KEY (host_alias, catalog_id)
);

CREATE TABLE client_catalog_grants (
  client_id  INTEGER NOT NULL REFERENCES client_tokens(id) ON DELETE CASCADE,
  catalog_id INTEGER NOT NULL REFERENCES catalogs(id) ON DELETE CASCADE,
  granted_at INTEGER NOT NULL,
  PRIMARY KEY (client_id, catalog_id)
);

CREATE TABLE changesets (
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
CREATE TABLE changeset_items (
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

CREATE TABLE asset_triage_verdicts (
  catalog_id   INTEGER REFERENCES catalogs(id) ON DELETE CASCADE,
  kind         TEXT NOT NULL,
  name         TEXT NOT NULL,
  content_hash TEXT NOT NULL,
  verdict      TEXT NOT NULL,       -- ignored | rejected | host_local
  decider      TEXT NOT NULL,
  decided_at   INTEGER NOT NULL,
  PRIMARY KEY (kind, name, content_hash)
);
```

**Changed**

- `host_layers` gains `catalog_id` (NOT NULL after backfill). Primary key
  `(host_alias, catalog_id, layer_name)`; the one-active-role index becomes
  per `(host_alias, catalog_id)`.
- `asset_inventory` gains `catalog_id` for managed rows (NULL for
  unmanaged / orphan). The key stays `(host_alias, harness, kind, name)`:
  installed names are global on a host, so two catalogs can never both be
  installed under one name — that is a collision (below), not two rows.
- `asset.yaml` header gains a typed `scope: private | shared`
  (`#[serde(default)]` = `private`). It is only meaningful in the personal
  catalog; an org catalog's assets are org-scoped regardless. Typed, because
  `model.rs` drops unknown keys on save.
- The host's fleet manifest entries gain `catalog` (serde default
  `personal`), so an orphan is "a manifest entry whose catalog no longer
  has it".

**Migration**

1. The `catalog_config` row, if any, becomes catalog `personal`
   (`org_id NULL`). `catalog_config` stays but nothing reads it.
2. Every `host_layers` row gets `catalog_id` = `personal`.
3. Every client with `assets_admin_at` gets a `client_catalog_grants` row on
   `personal`. `assets_admin_at` stays but nothing reads it.
4. Every existing asset is `private` (SB1). No file is rewritten: the
   default is the absence of the key.

**Which catalogs a host accepts**

- A host with `org_id = X`: catalog(s) of org X, plus the `shared` assets
  of `personal`.
- A host with no org: `personal` (all scopes) plus every catalog in its
  `host_catalogs` admissions.
- `local` on a hub follows the same rule.

**Runtime.** `service::catalog::CATALOG: RwLock<Option<Catalog>>` becomes
`CATALOGS: RwLock<BTreeMap<CatalogId, Catalog>>`. `with_catalog(id, f)` and
`effective_for_host(store, host) -> EffectiveSet` replace the ~30 current
call sites. A catalog whose repo cannot be loaded is kept as a problem entry;
the others load.

## Sync across catalogs

- **Effective set of a host** = ⋃ over accepted catalogs of
  `resolve(catalog, host's role + contexts in that catalog)`, then filtered
  by the scope rule above. A private asset is never planned onto an
  org-bound host; an org asset never onto a host that does not accept its
  catalog. A layer that would do either fails validation with the reason.
- **Unlayered guard (from S1a)** applies per host: a remote host with no
  layers in *any* accepted catalog is skipped unless `allow_unlayered`.
- **Collision:** two catalogs render the same install target (kind +
  install name) for one host → both actions are `blocked` with
  `conflict: personal/x vs papayapos/x — use install_as or move one`, and a
  "needs a person" item appears. Neither side wins silently.
- Planning and inventory report `catalog` on every managed action and row.

## Changesets (the cards)

A reconcile pass runs after each scan-tick pass and on demand
(`changesets { action: propose }`), and builds or refreshes proposed cards
from rules. Rules never re-propose a subject with a matching
`asset_triage_verdicts` row until its content hash changes; an agent never
overturns a person's verdict.

| Card | Trigger | Items |
|---|---|---|
| **Bootstrap** | a catalog is empty, or ≥ 20 unmanaged `normal` identities exist | group identities by host-set signature and name-prefix family → proposed layers; destination catalog: found only on hosts of org X that has a catalog → X, else `personal`; **`set_scope shared` for personal destinations that are also present on an org-bound host** (so trn keeps what it has); `hide` for `fleet_internal` / `harness_internal`; `needs_person` identities listed as "needs a look" |
| **New on host** | a new unmanaged identity after bootstrap | adopt into the layer whose members share its host set or prefix; else "needs a look" |
| **Drift** | a managed asset `drifted` on a host | per host: `take_host` (import that copy into its catalog) or `restore` (sync the catalog copy, backup) — always one item at a time |
| **Rollout** | a card changed a layer, or a new layer exists | a `plan_sync` limited to the affected hosts and assets |

**Apply** (per card, on the hub or standalone desktop):
1. Snapshot `host_layers`.
2. For each catalog the card touches: import each item from the host holding
   the most common copy (S1a remote import, `only`), write
   `layers/*.yaml`, set `scope`.
3. Update `host_layers`.
4. One commit per touched catalog (`fleet: <card summary>`); store the SHAs.
5. Mark items `applied`; the card `applied`. A failure before step 4 leaves
   the working trees reset to their HEAD, the card `failed` with the error
   on the failing group, and nothing committed.

**Rollout apply** = `plan_sync` (hosts of the card) + `apply_sync`.
`overwrite` and `remove` never go through a card.

**Undo** = `git revert` of the card's commits (one per catalog) + restore the
`host_layers` snapshot; allowed only for the latest applied card in each
catalog it touched. It never touches hosts; a follow-up Rollout card
restores them.

**Automatic (no card)** — `catalog.auto` (default on): hide internals,
group, prepare cards; and additive sync ops on layers that have already been
rolled out once (SB6).

## Hub CLI and MCP

- `fleet-hub catalog add <name> <path> [--remote URL] [--org NAME]`,
  `catalog list`, `catalog remove <name>` (config only; never deletes the
  repo), `catalog admit|unadmit <host> <catalog>`. `catalog set` stays as the
  alias for `personal`.
- `fleet-hub client grant <name> assets [--catalog NAME]` (default
  `personal`) and `client revoke … --catalog`.
- MCP `catalog_admin` actions take an optional `catalog` (default
  `personal`); new actions `list_catalogs`, `add_catalog`,
  `remove_catalog`, `admit_catalog`, `unadmit_catalog`. A new tool
  `changesets { list | propose | apply | undo | dismiss | reject_item }`.
  Every mutating action checks the caller's grant **for the catalogs it
  touches** (`may_admin_catalog(caller, catalog_id)`); per-host tokens never
  pass. `list_assets` and the inventory stay readable as today.
- The Tauri commands route to the hub through `AdminCall` as today; the
  verdict table gains the new actions.

## Workspace shell

The layout, rows and cards follow the mockups. Existing components are kept
and wrapped, not rewritten.

```
┌ Rail ─────────┬ List (sentence header) ────────┬ Inspector ─────────────┐
│ Inbox 9       │ 6 need you · rest in sync      │ Overview Source Hosts  │
│ Layers        │ Needs you / Drifted / New /    │ History                │
│ Hosts         │ In sync (148, folded)          │                        │
│ Library       │                                │                        │
│ Secrets       │                                │                        │
├───────────────┴────────────────────────────────┴────────────────────────┤
│ personal @a1b2 · papayapos @9f0e ↑1 · auto: on            ⟳ syncing 2/5  │
└──────────────────────────────────────────────────────────────────────────┘
```

- **`AssetsWorkspace.svelte`** (new) replaces the toolbar layout of
  `AssetsPanel.svelte`; the twelve buttons go. The only primary button is
  contextual (`Adopt 110 as 6 layers`, `Roll out to core`, `Sync fleet`).
- **Rail:** Inbox (default), Layers, Hosts, Library, Secrets. A Tests entry
  appears only when S4 lands.
- **Inbox:** proposed cards on top (`ChangesetCard`), then sections; the
  in-sync block folds to one line. Rows reuse S1a's identity rows and
  `HostStrip`, plus a `Badge` for scope/catalog.
- **Layers:** layers grouped by catalog with members and footprint;
  create / rename / move produce cards, never direct commits.
- **Hosts (inside Assets):** per host its org, role per catalog, accepted
  catalogs (toggle = admission), effective set with provenance
  ("on oci via layer core from personal").
- **Library:** every asset once, grouped by kind; managed-elsewhere assets
  read-only.
- **Inspector:** a pane. `AssetDetail` becomes Overview + Hosts;
  `AssetEditor` stays in Source (drafts arrive in S3); History lists
  commits. Drift shows a `DiffView` with Take / Restore.
- **Footer:** a chip per catalog (HEAD, ahead count; popover with pull,
  push, commit), `auto: on|off`, and `JobChip` for scans and syncs. The modal
  `SyncPlanDialog` becomes the Rollout card; its "Plan anyway" stays
  host-scoped.
- **Query:** `QueryInput` on `/` with tokens `host:` `kind:` `state:`
  `layer:` `catalog:` `scope:`, case-insensitive, with completion.
- **Keyboard:** `j/k`, `space`, `a` adopt, `i` ignore, `s` sync, `e` edit,
  `⌘↵` primary. `QuickSwitcher` gains an `asset` kind and the commands
  Rescan, Sync fleet, Propose.
- **Settings → Catalogs:** a declarative `master_detail` page with a
  `catalog` resource, like Organisations: add (name, path, remote, org), the
  deploy-key hint (GitHub SSO orgs need an admin to allow it), grants.
- **Hub client without a grant:** the same views without mutating
  controls, and one scope chip.
- **Visuals:** the existing tokens and `controls.css`; the hard-coded hex
  colours in `AssetDetail` and `SyncPlanDialog` move to tokens; one `Badge`
  replaces `.chip`, `.op-badge`, `.count-chip`. State is never colour alone.

## Milestones

Each ends with `cargo test --workspace`, `pnpm test`, `pnpm check` green.

| M | Contents |
|---|---|
| M1 | migration; `CATALOGS` registry and `with_catalog` / `effective_for_host`; `scope` in `asset.yaml`; `personal` backfill |
| M2 | sync across catalogs: scope boundary, collisions, manifest `catalog`, validation errors |
| M3 | hub CLI, MCP `catalog` parameter and new actions, per-catalog grants, admissions, verdict table |
| M4 | changeset engine: store, Bootstrap / New / Drift / Rollout rules, apply / undo / dismiss / reject, `catalog.auto`, `catalog.auto_push` |
| M5 | shell: `AssetsWorkspace`, rail, Inbox sections, Library, Inspector, footer chips, `JobChip`, `QueryInput`, keyboard, `Badge`, read-only chip |
| M6 | Layers and Hosts views, admissions UI, `ChangesetCard` (Bootstrap, New, Drift, Rollout), `DiffView`, Settings → Catalogs, QuickSwitcher |
| M7 | full verification, generated files, `docs/hub.md` catalog section, `CLAUDE.md` |

## Testing

- Store: the migration (with and without an existing `catalog_config`),
  admissions, grants per catalog, verdict holds by content hash.
- Planning: an org host never receives a private asset; a no-org host
  receives an org asset only when admitted; a collision blocks both sides;
  per-host unlayered guard across catalogs.
- Changesets: bootstrap on the live fixture shape (164 identities, 8
  signatures) produces the expected groups and the `shared` proposals for
  assets present on an org host; apply makes one commit per catalog; undo
  reverts them and restores `host_layers`; a failed apply commits nothing.
- Authorization: an ungranted client and a per-host token cannot apply,
  undo or admit in a catalog they have no grant for, but can list.
- Vitest: `AssetsWorkspace` navigation, `ChangesetCard` apply/undo states,
  `QueryInput` tokens, `Badge`, the read-only chip, the Rollout card's
  host-scoped "Plan anyway".

## Out of scope

- Drafts, focus mode, the stepper (S3); skill tests and the Tests view (S4);
  Jev and haiku naming (S5).
- fleet-mobile cards (their own PR).
- Automatic push by default; automatic overwrite / remove under any mode.
- Rewriting literal secrets to `${NAME}` on import, and excluding `.git` /
  `.env` from skill resources: tracked follow-ups from the S1a review.
