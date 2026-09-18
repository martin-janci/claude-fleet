# Asset layers: composable profiles for the catalog

**Date:** 2026-09-17
**Status:** Design — not yet implemented
**Scope:** Sub-project 1 of 4 (layers → AI categorisation → UX → AI-assisted
setup). This one is the backbone; the other three are shaped by it and get
their own specs.

## Problem

The catalog is all-or-nothing per host. `compute_host_plan`
(`src-tauri/src/service/catalog/sync/plan.rs:296`) loops
`for asset in &catalog.assets` and plans **every** asset onto **every** host.
`PlanFilter` carries a `host_alias`, but its `matches()` only tests `kind` and
`name` — the alias selects which hosts to *visit*, never which assets belong
there. Nothing in the model expresses "this asset belongs on these machines".

The one axis of variation that does exist is `targets.<harness>`
(`enabled: false`, per-harness field overrides, `extra`). It is well shaped —
it is simply keyed on the wrong dimension for profiles. This design reuses that
shape on a new axis rather than inventing a second override mechanism.

`Header::tags` is parsed, stored, serialised and displayed, but the catalog
spec calls it "informational in v1" and nothing filters or groups on it.

## What the fleet actually looks like

Measured on 2026-09-17 from `list_assets` after the first `plan_sync`
(520 unmanaged rows across 5 hosts, `claude` harness):

| kind | distinct names | local↔oci | local↔trn | local↔mefistos | local↔htz |
|---|---|---|---|---|---|
| skill | **99** | 93 % | 85 % | 94 % | 9 % |
| agent | **18** | 100 % | 100 % | 50 % | — |
| plugin_ref | **32** | 19 % | 19 % | 19 % | 19 % |

The 520 rows are **99 skills, 18 agents and 32 plugin refs counted five
times**. Only 10 skill names are unique to a single host. Agents are identical
on three hosts. The one genuine axis of divergence is `plugin_ref`: `local`
carries 25 that exist nowhere else — a workstation/server split, not five
configurations.

**This is why layers must compose.** Five independent profiles would duplicate
82 skills five times and re-drift within a month. The model has to be a shared
base plus small overlays.

## Decisions taken during design

| Question | Decision |
|---|---|
| What selects a profile? | **Both axes**: a per-host *role* (base) and switchable *contexts* layered on top. Acknowledged as the largest option; chosen deliberately. |
| How is membership expressed? | **The layer lists its members** (not the asset listing its layers, not a tag query). The UI writes these files, so verbosity is free and explicitness is the point. |
| What happens when a context is switched off? | Its files are **removed**; `plugin_ref` entries are reported as orphans but **never auto-uninstalled**. |

## The layer file

`layers/<name>.yaml` in the catalog repo:

```yaml
kind: layer
name: server
axis: role                    # role | context
version: "1"
description: Headless build/CI boxes
extends: core                 # 0..1 parent; cycles are a load error
members:                      # added to the parent's set
  - skill/argocd
  - agent/pm-backend
exclude:                      # removed from the parent's set
  - skill/airbnb-invoices
overrides:                    # field changes, deep-merged over the parent's
  skill/argocd:
    targets:
      claude:
        extra: { disable-model-invocation: true }
```

**Keys are `<kind>/<name>`** — deliberately the same convention
`Manifest::key` / `Manifest::split_key` already use
(`src-tauri/src/service/catalog/sync/manifest.rs:44`), so one key spelling runs
through layers, the manifest, the plan and the UI. `kind` is one of `skill`,
`agent`, `hook`, `mcp_server`, `plugin_ref` (`Kind::as_str`).

`members`, `exclude` and `overrides` are three separate keys on purpose:
"not a member" and "a member that is turned off" must never be expressible as
the same thing.

Layers live under `layers/` and are loaded alongside the five asset kinds.
`Kind::ALL` is **not** extended — a layer is not an asset and must never reach
`compute_host_plan` as one. It is a sixth, separate directory read by its own
loader pass.

### Authoring

Layer files go through the existing authoring path (`author.rs`): create from a
template, lint, auto-commit as `catalog: create|update|delete layer/<name>`.
Lint rules for layers:

- a `members` / `exclude` / `overrides` key naming an asset not in the catalog
  → **warning**, reported as a `Problem` at load, never a hard failure (matches
  `load_dir`'s existing tolerance);
- an `extends` cycle, or `extends` naming a missing layer → **error**;
- `axis` outside `role` | `context` → **error**;
- a key listed in both `members` and `exclude` of the **same** layer →
  **error**. Across layers this is meaningful (a later layer re-adds what an
  earlier one dropped); within one layer it is always a mistake, and picking a
  winner silently would hide it;
- a `role` layer that `extends` a `context` layer → **error** (an axis may only
  extend its own).

## Two axes, one format

`axis: role` — a host has exactly one; it is the base.
`axis: context` — zero or more, switched on and off.

Nothing else differs, so there is one file format and one loader.

## Resolution

One pure function, and it is the whole seam:

```rust
pub fn resolve(
    catalog: &Catalog,
    role_chain: &[&Layer],      // root → leaf, already flattened from `extends`
    contexts: &[&Layer],        // in configured order
) -> Resolution;

pub struct Resolution {
    /// The effective catalog: member assets only, overrides applied.
    pub catalog: Catalog,
    /// `<kind>/<name>` → where it came from. Drives the UI's provenance view.
    pub provenance: BTreeMap<String, Provenance>,
    /// `<kind>/<name>` → the layer that excluded it, for "why is this missing?".
    pub excluded: BTreeMap<String, String>,
}

pub struct Provenance {
    pub introduced_by: String,
    pub overridden_by: Vec<String>,   // in application order
}
```

The chain is flattened **at load**, not by `resolve`: the loader walks
`extends`, detects cycles and missing parents, and stores each layer's
root → leaf chain. `resolve` therefore receives an already-valid chain and
cannot loop.

Order of application:

1. the role chain, root → leaf (each `extends` parent before its child);
2. then the contexts, in their configured order.

Within each step: `members` add, `exclude` remove, `overrides` deep-merge
field-by-field with the later layer winning. A key may be excluded by one layer
and re-added by a later one; last write wins, and `provenance` records it.

`targets.<harness>` keeps its current meaning and is applied by the renderer
**after** resolution — a layer override may *set* `targets.<harness>` fields,
but it does not replace the harness mechanism. The two compose: layer overrides
decide the IR, harness targets decide how that IR is rendered per harness.

**Why this is the right seam:** `resolve` returns a plain `Catalog`, so
`compute_host_plan`, the applier and the manifest run over it **unchanged**.
And because it is pure, every rule above is testable without a host, an ssh
connection or a running app.

## Storage: content in git, assignment in the DB

| | where | why |
|---|---|---|
| layer definitions | catalog repo `layers/*.yaml` | portable, shareable, reviewable in a PR |
| host → role + active contexts | fleet DB | that is per-installation state, not catalog content |

### Migration `0NN_asset_layers.sql`

```sql
CREATE TABLE host_layers (
  host_alias TEXT    NOT NULL REFERENCES hosts(alias),
  layer_name TEXT    NOT NULL,
  axis       TEXT    NOT NULL,             -- 'role' | 'context'
  position   INTEGER NOT NULL DEFAULT 0,   -- context application order
  active     INTEGER NOT NULL DEFAULT 1,
  PRIMARY KEY (host_alias, layer_name)
);

-- At most one active role per host, enforced by the schema rather than by
-- application code.
CREATE UNIQUE INDEX idx_host_active_role
  ON host_layers(host_alias) WHERE axis = 'role' AND active = 1;
```

Register it in the `MIGRATIONS` table in `src-tauri/src/store/schema.rs`.

**Numbering:** the last migration on disk is `031_asset_sync.sql`. The
host-reboot spec (`2026-09-17-host-reboot-session-survival-design.md`) also
claims `032`. Whichever lands first takes it; the second must renumber. Do not
assume `032` when implementing this.

## Sync integration

`plan_sync` resolves before it plans: for each (host, harness) it reads the
host's role + active contexts, calls `resolve`, and hands the resulting
`Catalog` to `compute_host_plan`. No change to `compute_host_plan` itself.

**Removal falls out for free.** `Manifest::orphans` asks
`catalog.find(kind, &name).is_none()`
(`src-tauri/src/service/catalog/sync/manifest.rs:99`). An asset that leaves the
effective set is therefore already an orphan and already planned as `Remove`.
Switching a context off needs no new removal machinery.

**One new rule:** a `plugin_ref` orphan is reported but not scheduled for
removal. `plugin_op` yields a `Noop` with a reason naming the layer that
dropped it, and the UI offers explicit removal. Rationale: uninstalling is slow
and network-bound, and a context switch must not fail because the box is
offline.

## Backward compatibility

A host with no row in `host_layers` resolves to **the whole catalog** — today's
behaviour exactly. Layers are opt-in per host, and a catalog with no `layers/`
directory behaves as it does now. No existing installation changes until the
user assigns a layer.

## Bootstrapping from what is already installed

A one-shot `propose_layers` (read-only, returns a proposal; writes nothing)
that reads the last scan and partitions by cross-host overlap:

- group every asset by its **exact host-set signature** (the set of hosts it is
  installed on);
- the **largest** group becomes the `core` role layer;
- every other multi-host group becomes an overlay named after its host set
  (`workstation`, `minimal`);
- assets on exactly one host are left out of every layer and listed separately,
  so the user decides whether each is a context or a mistake.

Grouping by signature — rather than intersecting across all hosts — is what
makes this work on the real data. A strict all-host intersection would produce
a `core` of only **7** skills, because `htz` is a near-empty outlier (11 skills
against 82–90 elsewhere) and would drag the intersection down with it. By
signature, the `local`/`oci`/`trn`/`mefistos` group is the largest and becomes
`core`, and `htz` gets its own `minimal` layer instead of impoverishing
everyone else.

Deterministic set arithmetic, no heuristics, no AI. On the measured data it
yields `core` (~82 skills + 9 agents + 6 plugin refs), `workstation` (25
plugin refs), `minimal` (htz), and ~10 singletons to triage.

**This is the hand-off point for sub-project 2.** AI categorisation replaces
this counting with something that groups by meaning and proposes `tags`; the
call signature and the review step stay the same, so swapping the proposer in
does not disturb anything downstream.

## Surface

New Tauri commands, each mirrored as an MCP tool:

| tool | kind | notes |
|---|---|---|
| `list_layers` | read-only | layer definitions + per-host assignment |
| `resolve_preview { host_alias }` | read-only | effective set **with provenance**; the "what will actually land here" view |
| `propose_layers` | read-only | returns a proposal, writes nothing |
| `set_host_layers { host_alias, role?, contexts? }` | mutating | assignment only; never edits catalog files |

`list_layers`, `resolve_preview` and `propose_layers` go in
`guard::READONLY_TOOLS`; `set_host_layers` does not.

These are new Tauri commands as well as MCP tools, so
`docs/control-api-reference.md` must be regenerated or CI fails:

```bash
REGEN_DOCS=1 cargo test --manifest-path src-tauri/Cargo.toml reference_is_current
```

## Testing

Pure, no host required, over `resolve`:

- an `extends` chain applies root → leaf, and a child's override beats its
  parent's;
- contexts apply after the whole role chain, in `position` order;
- `exclude` removes an inherited member; a later layer re-adding it wins;
- overrides deep-merge field-by-field rather than replacing the header;
- a layer override setting `targets.<harness>` still renders correctly per
  harness;
- `provenance` names the introducing layer and every overriding layer in order;
- `excluded` names the layer responsible, so the UI can answer "why is this
  missing?";
- an `extends` cycle is a load error, not a hang, and is caught at load so
  `resolve` never sees one;
- a key in both `members` and `exclude` of the same layer is a lint error;
- `propose_layers` groups by host-set signature: on a fixture with one
  near-empty outlier host, `core` is the large shared group and the outlier
  gets its own layer, rather than `core` collapsing to the all-host
  intersection;
- a member naming an unknown asset is a `Problem`, and the rest of the layer
  still resolves.

Over the plan:

- a host with no `host_layers` row plans the whole catalog (backward compat);
- an asset dropped from the effective set is planned as `Remove` via the
  existing orphan path;
- a dropped `plugin_ref` is reported but **not** scheduled for removal;
- the partial unique index rejects a second active role for one host.

Over `propose_layers`:

- on the measured fixture it yields `core` / `workstation` / `minimal` and
  leaves single-host assets out;
- it writes nothing.

## Out of scope

- **The Assets UI** — sub-project 3. This design only commits to producing what
  that UI needs (`resolve_preview` with provenance, and a host×layer view), on
  the grounds that provenance is very hard to retrofit.
- **AI categorisation and AI-assisted setup** — sub-projects 2 and 4.
  `propose_layers` is deliberately dumb set arithmetic so it can be replaced.
- **Per-project / per-repo asset sets.** Fleet sync writes to `~/.claude`,
  which is per-machine; a per-repo axis is a different mechanism.
- **Tag-driven layers.** Rejected for v1: a mistagged asset would silently
  appear or disappear. Revisit once sub-project 2 has made tags trustworthy.
