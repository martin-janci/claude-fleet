# Assets workspace: an inbox over layers, with authoring and skill tests

**Date:** 2026-09-29
**Status:** Design — not yet implemented
**Scope:** Umbrella design for sub-projects 2–4 of the layers spec
(`2026-09-17-asset-layers-and-profiles-design.md`: layers → AI categorisation
→ UX → AI-assisted setup), widened to multiple catalogs, authoring and skill
testing. It fixes the information architecture, the automation model, the
ownership model and the visual system once, then splits delivery into five
sub-projects (S1–S5), each of which gets its own spec and plan.
**Inputs:** a UX/UI audit and an agent-flows/Jev analysis, both run
read-only against `origin/main` @ `d1e29ed1` and the owner's live hub
(`list_assets`, 2026-09-29).

## Problem

The owner opens Assets and is lost. On the live hub the catalog holds **0
managed assets** and the list shows **520 unmanaged rows**.

The rows are the wrong object. `compute_states` emits one row per
host × harness × kind × name (`service/catalog/inventory.rs:505-519`), and
`AssetList.svelte:62-77` renders them flat and unsorted. The 520 rows are
**164 kind+name identities**, and those fall into only **8 distinct host-set
signatures**:

| host set | identities |
|---|---|
| {local, mefistos, oci, trn} | 82 |
| {local} | 30 |
| all five | 17 |
| {local, oci, trn} | 11 |
| four smaller sets | 24 |

About 41 rows are fleet's own provisioning (six hooks from
`hooks_install.rs` carrying the host token, the `claude-fleet` MCP server,
`claude-fleet-control`, `fleet-friendly-name`) or harness internals
(`.system`, codex). The scan is 11.8 days old: `scan_hosts`
(`inventory.rs:156`) has no tick, only the Scan button and `scan_assets`.

Beyond the list, the path from "found on hosts" to "managed, synced fleet" is
unsafe or impossible in places:

1. **Sync without layers ships everything everywhere.** An empty role chain
   and empty contexts return the whole catalog (`resolve.rs:97-99`). Import
   local (152 assets), press Sync, and all of them land on all five hosts.
   Nothing warns.
2. **Import reads `local` only** (`ImportDialog.svelte:29`,
   `import.rs:36`). 14 names exist only on remote hosts and cannot be
   imported. The per-row Import link ignores its row (`AssetsPanel.svelte:201`).
3. **Identical copies are indistinguishable from variants.** Unmanaged rows
   carry `host_hash: None` (`inventory.rs:460,512`) although the snapshot
   hashes every file (`harness/claude.rs:393`).
4. **Layers have no UI.** `propose_layers` and `set_host_layers` exist only as
   MCP / Tauri commands.
5. **Authoring has no draft.** `commit_and_reload` / `write_commit_reload`
   (`author.rs:597,613`) commit every save to the checked-out branch; the next
   Sync ships a half-finished edit. Resource add/remove commit immediately
   while header and body wait for Save. Switching rows discards the draft.
   "Open in session" edits the same working tree with no reviewable diff.
6. **No testing concept exists** beyond static `lint` (`author.rs:325`).
7. **No ownership.** The catalog is one repo with no notion of owner.
   Orgs exist (`050_orgs.sql`: `orgs`, `hosts.org_id`, `org_rules`), but sync
   ignores them, so an org's skills can reach a personal host and vice versa.
8. **Twelve equal-weight toolbar buttons**, git plumbing above fleet intent,
   hard-coded hex colours in `AssetDetail` / `SyncPlanDialog`, three chip
   styles, and a modal sync that blocks the panel.

## Goals

The workspace serves four jobs in one IA, not four apps:

1. **Curate** — decide what belongs in the catalog.
2. **Set up hosts** — layers decide what goes where; sync is safe by
   construction.
3. **Author** — write and change assets without shipping drafts.
4. **Test** — lint, trigger, eval and smoke, with regressions surfaced.

Plus: an automatic experience (the list is short because the system did the
sorting), and a minimal review surface on mobile.

## Decisions taken during design

| # | Question | Decision |
|---|---|---|
| AW1 | Primary job | Curate + host setup, with authoring and testing inside the same workspace. |
| AW2 | Information architecture | **Inbox over layers** (option C of three: deduplicated library; host-centric profiles; fleet-state inbox). Library and Hosts remain as drill-downs. |
| AW3 | Autonomy | **Safe operations automatic, everything else batched** into changeset cards; destructive items one at a time. |
| AW4 | Surfaces | Desktop (standalone and hub client) + a **review-only mobile inbox** in fleet-mobile. |
| AW5 | Test levels | All four: static lint, triggering, behaviour evals, per-host smoke. |
| AW6 | Ownership | Every asset has a **scope**: private, org, shared, or managed (read-only). |
| AW7 | Where org assets live | **One catalog per org** (e.g. `papayapos/agent-assets`) beside the personal catalog. Org IP stays in the org's GitHub. |
| AW8 | Jev | Not needed for the automatic feeling. Used, in shadow then assist, only for two closed-set choices and one fleet-wide pre-screen (see *Jev*). |

## Information architecture

```
┌ Rail ──────┬ List (grouped, sticky sentence header) ──┬ Inspector ─────────────┐
│ ◉ Inbox 9  │ Fleet converged · 5/5 hosts · scan 3m    │ skill · author-draft   │
│ ▤ Layers   │ ▾ Needs you (9)                          │ Overview Source Tests  │
│ ▣ Hosts    │ ▾ Drifted (2)                            │ Hosts History          │
│ ▦ Library  │ ▾ New on hosts (4)                       │                        │
│ ✓ Tests    │ ▸ In sync (148)                          │                        │
│ ⚿ Secrets  │                                          │                        │
├────────────┴──────────────────────────────────────────┴────────────────────────┤
│ personal @a1b2c3 · papayapos @9f0e1d ↑1 · auto: on               ⟳ syncing 2/5 │
└────────────────────────────────────────────────────────────────────────────────┘
```

**Rail views.**

- **Inbox** (default) — exceptions and changeset cards only. Everything in
  sync collapses to one line. The sticky header is a sentence:
  `Fleet converged · 5/5 hosts · last scan 3m`, or `9 need you`.
- **Layers** — each layer with its members and host footprint; host roles
  and contexts. This is the UI the layers backend never got.
- **Hosts** — each host's effective set, with provenance for every asset
  ("on oci via layer `core`", from `resolve_preview`).
- **Library** — every asset, one row per identity, grouped by kind or
  category.
- **Tests** — fleet-wide results, regressions, and the overlap view.
- **Secrets** — unchanged.

**The row** is one identity, never one host copy:

- name, kind, a **scope badge** (🔒 private, 🏢 `papayapos`, shared, managed);
- a **host strip**: one dot per host in fixed order, encoded by shape *and*
  colour — `●` in sync, `◐` differs, `○` missing, `✕` blocked, hatched =
  stale scan; an org band above the dots groups hosts by org;
- a lifecycle pill: `draft`, `canary · oci`, `published`, `⎇ 2 drafts`;
- a test glyph: `✓ 12`, `◐ 1` flaky, `✗ 2`, or a muted `○` for untested (the
  ~160 imported skills must not flood the list red).

**The inspector** is a pane, not a modal, for whatever is selected (asset,
layer or host). Asset tabs: **Overview** (description, footprint, provenance,
scope), **Source**, **Tests**, **Hosts** (the host × harness matrix that
exists today), **History** (commits and a test sparkline).

**Focus mode** (`⌘E`, or clicking into Source): the list collapses to a
48 px rail and the inspector takes the full width as a split
**Source | Tests**. `Esc` returns; the draft survives.

**Toolbar.** The twelve buttons go. Git state moves to a footer chip per
catalog with a popover (pull, push, commits). The only primary button is
contextual: `Adopt 110 as 3 layers`, `Roll out to core`, `Sync fleet`.

**Keyboard.** `j/k` move, `space` select, `a` adopt, `i` ignore, `s` sync
selection, `e` edit, `⌘↵` primary action. `/` opens a token query
(`host:oci kind:skill state:drifted layer:core catalog:papayapos scope:org`),
case-insensitive, with completion. An `asset` kind joins `QuickSwitcher`
(`⌘K`) with commands: Rescan, Sync fleet, Propose layers, Run trigger tests.

**Hub read-only client.** The same screens without mutating controls, and one
scope chip ("read-only · ask the operator to grant `assets` on `personal`")
replacing the CLI paragraph at `AssetsPanel.svelte:308-315`.

## Ownership: scopes and multiple catalogs

| Scope | Example | May be installed on | Source |
|---|---|---|---|
| **private** | `airbnb-invoices`, `paycheck`, the author suite | hosts with no org | personal catalog |
| **org** | `ppt-*`, `openmarket-*`, `papayapos-*` | hosts whose `org_id` is that org | that org's catalog |
| **shared** | `superpowers`, `worktree`, `code-review` | wherever a layer puts it | any catalog |
| **managed** | `anthropic-skills:*` (claude.ai account), marketplace plugin skills, a repo's own `.claude/skills` | not synced; shown read-only | outside fleet |

**A catalog is a source with an owner.** `personal` →
`martin-janci/agent-assets` (private and shared scopes); `papayapos` →
`papayapos/agent-assets`, bound to that org. On a hub each catalog is its own
checkout under `<data-dir>/catalogs/<name>` with its own remote and deploy
key. A repository in a GitHub org with SSO needs an org admin to allow the
deploy key; onboarding says so.

**Rules.**

- **Scope is a hard boundary above layers.** Sync never plans an org asset
  onto a host of another org, nor a private asset onto an org host. This is
  enforced in planning, the same place `hosts.org_id` bounds tokens, and a
  layer that would violate it fails validation rather than being silently
  filtered.
- Layers belong to a catalog. A host composes layers only from catalogs valid
  for it (personal + its own org's).
- An asset found on an org host that is not shared is proposed into that
  org's catalog, never into the personal one.
- **Name collision across catalogs** (Claude Code skill names are global) is
  always a needs-a-person item: `install_as`, or move the asset.
- A changeset makes one commit per affected catalog; Undo reverts the group.
- The client grant `assets` becomes per catalog (`074_client_assets_admin.sql`
  extends to a catalog column).
- Jev and test runs follow the asset's org (see below).

## Automation model

**Triggers.** A reconcile pass runs on: the daily scan tick; a host
reconnecting or being added; a catalog HEAD change; the end of any sync; and
opening the Assets tab when the last scan is older than an hour. Inventory
change events (`store/catalog.rs:264`) already exist to hang this on.

**Pipeline.**

| Step | Decided by | Output |
|---|---|---|
| 1. Scan | automatic | inventory with content hashes kept |
| 2. Collapse | rule | 520 rows → ~164 identities; identical on N / differs on some |
| 3. Classify | rules | fleet-infra and `.`-names hidden; same name + hash = one asset; same name, different hash = variant conflict; secrets → needs a person; scope from where it was found |
| 4. Structure | rules | host-set signature (`propose_from_installed`) + name-prefix families → proposed layers |
| 5. Naming (bootstrap only) | `claude -p` haiku on a same-org host with headroom | human layer names (`authoring`, `papayapos-ops`, `workstation-plugins`) |
| 6. Residual closed-set choices | Jev, shadow → assist | layer and category for a new asset with no siblings; else needs a look |
| 7. Package | automatic | a changeset card |

**Autonomy (AW3).**

- **Automatic:** scan, hashing, classification, hiding infra, lint, trigger
  tests on save, smoke after every sync, and additive sync operations
  (`adopt`, `create`, backed-up `update`) on hosts already in a layer.
- **Batched into a card:** import into a catalog, creating or changing
  layers, assigning hosts, a layer's first rollout, taking a host edit into
  the catalog, canary promotion when evals changed.
- **One at a time, always asks:** `overwrite` of a host edit, `remove` and
  orphan removal, plugin uninstall, anything with secrets, anything crossing
  an org boundary.

**Changeset cards.** One sentence ("Bootstrap ready: import 118 into 6
layers · assign 5 hosts · 9 need a look"), expanding into groups with Apply,
Edit and Skip. Apply is **one commit per affected catalog**; **Undo** is a
revert of that group. A rollout that syncs hosts is always its own card,
never folded into a catalog change. Cards appear in the Inbox, the desktop
Attention list and mobile.

**Audit.** A changeset row records each item's decider — rule, jev, haiku,
person — mirroring `store::Decider` (`store/work.rs:47`). Jev items also write
`decision_runs`.

**Rejections stick.** A new table in the shape of `work_unlinks` (migration
070): `asset_triage_verdicts(catalog, kind, name, content_hash, verdict,
decider)`. A rejected or ignored subject is not proposed again until its
content hash changes. A rule or agent never overturns a person
(`E_FORBIDDEN`, as `work/mod.rs:704`).

**Settings.** `catalog.auto` (rule-only, reversible steps) defaults **on**.
Anything a model decides uses the existing `decide.*` modes
(`off | shadow | assist`); `auto` is not offered, matching the rest of the
app.

**Failure handling.** An unreachable host keeps its last scan, marked stale;
nothing is inferred as removed. Jev or haiku down → the affected items go to
needs a look, and the footer notes "assist offline". A failed apply leaves
the commit unmade and the card open, with the error on the failing group.

## Jev

Rules deliver the automatic feeling: content hash, the fleet-infra list,
host-set signature and prefix families settle about 90 % of subjects. Jev is
used only where the question is genuinely closed-set and a rule is weak.

| Decision | Verdict |
|---|---|
| Unmanaged asset → adopt / ignore / host-local / infra / duplicate | **Rule, then a person.** Not Jev: the residue is the owner's intent, which no text carries. |
| New asset without siblings → one of the existing layers | **Jev, shadow → assist**, `min_confidence` 0.7 (above status_map's 0.5, because membership causes installs). |
| Category tag from a closed vocabulary | **Jev, assist.** Low stakes; display grouping only. |
| Near-duplicate (`code-review` skill vs plugin) | Hypothesis; name-similarity pre-filter first. Low volume. |
| Drift → pull / overwrite / keep | **Never Jev.** The input is file content, which can carry tokens. A person with a diff; `claude -p` may summarise it. |
| Which hosts get an asset | Not a decision — layers and scope decide. |
| Fleet-wide trigger collision pre-screen | **Jev in shadow against real `claude -p` results.** Jev does not route the way Claude Code does (the session model routes, with CLAUDE.md and context), so it is only a pre-screen if agreement is high. ~1,600 prompts ≈ 2.4 M tokens ≈ $0.10 per night; above the default 2 M `daily_token_budget`. |

**Constraints.** A new `Feature::AssetTriage` and `decide.jev.asset_triage`
(`decide/mod.rs`, `settings.decisions.json`). Only name and description are
sent, through `redact_state`; never bodies, hooks or MCP entries. Assets in
the personal catalog follow `decide.jev.unassigned`; org assets require that
org's `jev_allowed`; the strictest applicable consent wins. Volume is ~160
questions once, then a handful a week, so Jev's speed and price barely
matter; its value is calibrated confidence and audit fit.

## Authoring

**A draft is a git branch.** `draft/<kind>-<name>` in a fleet-managed
worktree of the owning catalog. The form editor and the "Ask Claude" session
write to the same draft. Save is an autosaved commit on the draft branch,
never on `main`. The Source tab shows the draft's diff against the published
version, whichever writer produced it. Switching rows never discards a draft.
Resource add/remove joins the same transaction as header and body.

**New asset** starts from blank, from a host copy (any host), or from an
intent in plain words that Claude turns into a draft. Scope and catalog are
chosen at creation. Rename is allowed until first publish. Version bumps are
tied to publish, not free text.

**The loop is a stepper** in the focus-mode header. Each step is a gate; the
primary button names the next step.

```
Edit ─ Lint ─ Trigger ─ Eval ─ Canary ─ Smoke ─ Roll out
 ✓      ✓      12/12    3/4 ◐   oci ✓    ✓      [Roll out to core ▸]
```

Canary syncs the draft ref to one host (a designated canary by default).
Smoke runs on it. If smoke passes and evals show no regression, a card
proposes "Roll out to N hosts"; applying merges the draft into `main` and
syncs only the hosts of the asset's layer.

**Backend gap:** planning a sync from a git ref other than HEAD.

## Skill testing

| Level | When | How | Graded by |
|---|---|---|---|
| **Lint** | while typing (debounced), anchored to the field | today's `lint` + trigger phrasing in the description + cross-catalog name collision + secret detection | deterministic |
| **Trigger** | quick on save (5 prompts, haiku); full at rollout (20) | real `claude -p --output-format stream-json` in a temp dir holding the target host's resolved skill set plus the candidate, with **only the Skill tool** allowed (`isolation_flags`' `--tools ''` removes it, `claude_print.rs:25`, so a variant is needed) | a `Skill` tool_use naming the asset for a positive, not naming it for a negative |
| **Eval** | on demand and at rollout, cost estimated first | `claude -p` on a realistic task in a scratch worktree with a fixture, hooks and MCP off | assertions first (files, commands, tool calls from stream-json), then a haiku rubric judge |
| **Smoke** | automatically after every sync | re-scan reads `in_sync`; MCP server answers `initialize` in time; hook command exists and parses (never executed); http hooks answer HEAD; plugin listed | pass / fail per host × harness |

**Cases live in the catalog** beside the asset, versioned and reviewable:
`skills/<name>/tests/triggers.yaml` (`prompt`, `expect: trigger |
no_trigger`, optional `over: [<competitor>]`) and
`skills/<name>/tests/evals/<case>.yaml` (`task`, `fixture`, `rubric`,
`budget`, `models`). Smoke needs no authoring; it is derived per kind.

**Suggest cases** generates 5 positives from the description and 5 near-miss
negatives taken from the 3–5 nearest neighbours by BM25 (reusing
`decide/bench`). A case a person edits is pinned; the generator never
rewrites it.

**Where and on whose account.** On a host whose account has headroom
(`account_usage`) and whose org matches the asset's. Caps: per run (prompts,
turns), per account per day. Credentials are never copied between hosts.

**Results.** A matrix of cases × {model × harness}; hosts × harness for
smoke. Flaky (`◐ 3/5`) is a pass rate strictly between 0 and 1. Each cell
carries cost, latency and a transcript link. A **regression** is a case that
passed at the last published commit and fails now; it pins to the top and
posts to the Inbox.

**Overlap** (`Tests ▸ Overlap`). Clusters of similar descriptions as a cheap
signal (`author-*`, `paperclip-*`, `ppt-*`); the truth is a confusion matrix
of which skill actually fired on another's positives. Clusters are ranked by
misfire rate. Actions: "Sharpen descriptions" (an agent drafts description
diffs on draft branches) and "Add as negative case".

**Gating.** A layer does not accept an asset with lint errors or failing
trigger tests. Rollout goes canary → smoke → card.

## Visual system

Direction: a professional tool in the Linear / Raycast register — dense,
calm, keyboard-first. Colour carries state only, and never alone (shape or
glyph always accompanies it, per `app.css:61-63`). Monospace for hashes,
paths and git only. Motion only where it carries information (a row settling
from pending to done, a card collapsing after Apply). Light and dark through
the existing tokens; `prefers-reduced-motion` respected.

**Reused:** tokens `--usage-ok/warn/crit`, `--accent-soft`, `--control-*`,
`--radius-pill`, `--mono` (`app.css`); `.btn`, `.btn--primary`,
`.btn--quiet`, `.btn--chip` (`controls.css`); `SegmentedControl`,
`FilterChipGroup`, `HostChips`, `ConfirmDialog`, `QuickSwitcher`; `Icon`'s
`circle`, `circle-half`, `circle-check`. The hard-coded hex colours in
`AssetDetail.svelte` and `SyncPlanDialog.svelte` move to tokens.

**New shared components:**

| Component | Purpose |
|---|---|
| `HostStrip` | dots in fixed order, shape + colour + tooltip, org band |
| `Badge` | one style replacing `.chip`, `.op-badge`, `.count-chip`: scope, lifecycle, test, count |
| `ChangesetCard` | sentence, groups, Apply / Edit / Skip, Undo, decider per item |
| `Inspector` | tabbed pane shell, shared with Hosts |
| `Stepper` | Edit → Roll out gates |
| `TestMatrix`, `Sparkline` | results and history |
| `DiffView` | drift and draft-vs-published |
| `QueryInput` | token filter with completion |
| `JobChip` | non-modal background progress in the footer |
| `BulkBar` | sticky bar for multi-select (the select-mode pattern from the 2026-09-21 audit, iteration 10) |

## States

| State | What the user sees |
|---|---|
| No catalog | Onboarding: connect the personal catalog (repo, deploy key); optionally org catalogs |
| Empty catalog, first scan | The Bootstrap card: "164 assets on 5 hosts → 6 layers · 9 need a look" |
| Steady | "Fleet converged · 5/5"; the Inbox is empty and quiet |
| Drift | `◐` row; inspector diff with "Take host version" / "Restore catalog version (backup)" |
| Variant conflict | side-by-side copies; pick the canonical one, the rest become drift |
| Cross-catalog name collision | needs-a-person item: `install_as` or move |
| Sync or tests running | `JobChip` in the footer; rows update in place; the panel stays usable |
| Host unreachable | hatched stale dot with scan age; nothing inferred as removed |
| Jev / haiku down | items to needs a look; footer "assist offline" |
| Hub read-only client | same screens, no mutating controls, one scope chip |
| Apply failed | card stays open, error on the failing group |

## Mobile

fleet-mobile (Compose Multiplatform) gets changeset cards in its existing
*Needs attention* screen beside `BlockedCard`: the sentence, the groups, and
Apply / Reject per card or per group. One-at-a-time items (overwrite,
remove, secrets) are listed read-only with "decide on desktop".
Notifications on test regressions and smoke failures. No editing or test
runs on mobile. It uses the same MCP verbs as the desktop; no mobile-only
API.

## Accessibility

Fully keyboard-operable with a visible focus ring. Host dots carry
`aria-label` ("oci: differs"). Contrast follows the tokens. No state is
conveyed by colour alone.

## Delivery: sub-projects

Each gets its own spec and plan. S1 is the prerequisite for the rest.

| | Sub-project | Contents |
|---|---|---|
| **S1** | Foundation | persist `host_hash` on unmanaged rows; group by identity in `list_assets` and the list; rule classifier (infra, `.`-names, variants, secrets); scan tick + reconnect/HEAD/after-sync triggers; import from any host (SSH read of scanned files); **multiple catalogs, scopes and the scope boundary in planning**; a guard on Sync when no layers exist |
| **S2** | Inbox and layers | rail + Inbox + Layers + Hosts views; changesets (store, one commit per catalog, Undo); `asset_triage_verdicts`; the Bootstrap card; footer git chips; token query; QuickSwitcher kind; hub read-only chip; mobile cards |
| **S3** | Authoring | drafts as branches in managed worktrees; focus mode; Source diff; new-asset sources; the Ask Claude session writing to the draft; sync from a non-HEAD ref (canary) |
| **S4** | Testing | extended lint; trigger runner (Skill-only `claude -p` variant, account/org selection, caps); smoke after sync; evals with assertions + judge; results, history, regressions; Overlap |
| **S5** | Jev | `Feature::AssetTriage` in shadow then assist (layer, category); the trigger pre-screen in shadow against S4's ground truth |

S2 and S3 can proceed in parallel after S1. S4 needs S3's drafts for the
stepper but its smoke level only needs S1. S5 needs S2 (cards to assist) and
S4 (ground truth).

## Testing this work

- Store tests: identity collapse, the rule classifier, the scope boundary
  (an org asset is never planned onto another org's host; a layer that would
  do so fails validation), rejection holds by content hash.
- A fixture from the live `list_assets` (520 rows) asserting 164 identities,
  8 signatures and the expected bootstrap card.
- Vitest component tests for `HostStrip`, `ChangesetCard`, `QueryInput`,
  `Stepper`, `TestMatrix`.
- End to end: bootstrap → Undo → re-scan yields an empty changeset.
- Trigger runner: a golden stream-json transcript per outcome
  (fired / not fired / competitor fired).

## Out of scope

- Editing or running tests from mobile.
- Automatic `overwrite`, `remove` or plugin uninstall under any mode.
- An `auto` mode for any model-decided step.
- Publishing assets to public marketplaces.
- Managing claude.ai account skills or a repo's own `.claude/skills` (shown
  read-only only).
