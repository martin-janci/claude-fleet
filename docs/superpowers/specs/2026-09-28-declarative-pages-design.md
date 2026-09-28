# Declarative pages and forms: research and design

**Status:** accepted 2026-09-28. The owner took every recommendation in §9 and
widened D-P1 to a general page engine. P1–P4 are landed; P5 is next.

**Goal.** A small framework in claude-fleet that renders pages and forms from a
declarative spec: settings, and also pages with dynamic setup, prefilled data,
charts, tables and editable lists. Pages can have sub-pages, tabs, collapsible
sections and list editors, drawn from a few prepared layouts. An AI agent adds
a page by writing a spec, not Svelte. The AI can also operate the forms: it
proposes values, a person reviews them, and the change is applied with a record
of where it came from.

The design combines two research passes: a survey of the codebase (§1) and a
survey of prior art in schema-driven forms and AI-native generative UI (§2).

---

## 1. Where we are today

### The backend already has half a registry

`crates/fleet-core/src/service/settings.rs` holds **56** settings in
`SPECS: &[Spec]`, where `Spec { key, default, kind }`.

- `Kind` is a closed enum: `Bool`, `Secs`, `SecsMin`, `Int{min,max}`, `Choice`,
  `ChoiceSet`, `PathMap`, `IdSet`, `PriceMap`.
- `validate()` refuses unknown keys and malformed values with `E_INVALID`.
- `set()` normalises values before storing them.
- The order of `SPECS` is already documented as the display order.

What is **missing** is everything a UI needs besides the type: label, help,
unit, group, restart flag, danger level, visibility, and whether the setting is
secret. That text exists only in three places:

1. rustdoc comments on the key constants, which are not available at runtime;
2. `hook-desc` spans in `SettingsDialog.svelte`;
3. the doc tables in `docs/work-graph.md` and `docs/decisions.md`. The tests
   check only the Key and Default columns, so Range and "What it does" can
   drift.

Some settings are **not in the registry**: `hub.*` (`service/hub.rs`), `mcp.*`
(`mcp/settings.rs`), `operator.*`, `ui.quick_replies`, and per-tracker
`trackers.settings` JSON. Those keys are never checked by `validate()`, and
nothing can describe them.

### The frontend repeats itself

| File | Lines | What it holds |
|---|---|---|
| `src/lib/SettingsDialog.svelte` | 1607 | One long modal of 12 hand-written `<section>` blocks: 53 field rows, 16 toggles, 37 number inputs, 5 selects |
| `src/lib/fleet_settings.ts` | 355 | A hand mirror of the registry: `SETTING_KEYS`, `SETTING_DEFAULTS`, choice lists, bounds, parsers |
| `WorkSettings.svelte`, `OrgSettings.svelte`, `McpSettings.svelte` | 699 / 395 / 367 | Separate, bespoke pages with different patterns again |

The dialog repeats the same trio three times: `applySetting`/`automationError`,
`applyLimit`/`limitsError` and `applyDecide`/`decideError`. It has seven
near-identical input adapters (`onHoursChange`, `onLimitIntChange`, and so on),
and its min/max values are written into the markup, where they can drift from
`Kind::Int{min,max}`.

The UI has no tabs primitive (tablists are written inline four times), no
accordion (raw `<details>`), and no field, toggle or select component. Settings
also use their own CSS classes (`.mcp-field`, `.hook-btn`, `.port`) instead of
`controls.css`.

### Constraints the design must respect

- **No settings change event.** `RowChange` has no settings variant. A change
  made through MCP `set_setting` or from another window stays invisible until
  the dialog remounts.
- **Hub client.** `get_fleet_settings` and `set_fleet_setting` are `LocalOnly`
  in `verdicts.rs`, so a paired desktop shows prose instead of settings. The
  audit `docs/ux/2026-09-21-audit/iterations/03-hub-parity-settings.md` already
  calls that a dead end.
- **MCP tool-description budget.** `the_served_definition_budget_stays_bounded`
  caps it at 62,355 bytes. Settings metadata cannot go into a tool
  description; it has to come back as a tool **result**.
- **Generated files are not imported by the frontend.** `verdict_gen.rs`
  explicitly keeps `hub.ts` hand-written and uses the generated JSON only in
  tests. `src/lib/names.json` is the one counter-example: the same JSON is read
  by Rust (`include_str!`) and by TS. The catalog should come to the frontend at
  **runtime**, through a command, so that both local and hub-client desktops
  get it without a build step.
- **A test to rewrite.** `every_spec_has_a_settings_dialog_row` greps
  `SettingsDialog.svelte` for `SETTING_KEYS.<name>`, which stops making sense
  once rows are rendered. It becomes the "every key is placed" check in §5.
- **Stores.** Module state is `svelte/store` `writable`, not `.svelte.ts` runes.
  The renderer should follow that; it is not the place to change the pattern.

---

## 2. Prior art

| System | What we take | What we leave |
|---|---|---|
| **VS Code `contributes.configuration`** | The model for a *settings* UI. Typed properties with metadata (description, enum labels, order, tags, scope, deprecation) are rendered into one UI that has search, `@modified` filters, a modified bar, reset, "copy setting ID/URL", and scope tabs | Weak list and object editing: it falls back to "edit in settings.json" |
| **Home Assistant config flows** | Server-driven wizard steps (`form / menu / progress / external / abort / done`), collapsible sections, errors keyed by field plus `base`, and **`suggested_value` kept separate from `default`**, which is exactly the slot an AI suggestion needs | Python/voluptuous |
| **JSON Forms** | The split between data and layout, and rule effects (`SHOW/HIDE/ENABLE/DISABLE`) as data | Two coordinated schemas; JSON Pointer scopes that LLMs get wrong |
| **JetBrains** | Search that highlights the matching *control* in a page; Apply only on risky pages | Hand-built pages |
| **Raycast** | Proof that a tiny closed type set covers most preferences; `required` before use | Too small for collections |
| **Grafana options builder** | Categories as accordions; defaults → overrides display | `showIf` as a function |
| **Sanity / Payload** | Arrays of typed rows as the collection primitive; access control per field | TS functions for conditions |
| **Backstage scaffolder** | Named, registered domain fields; a preview route for specs | The rjsf underneath shows through |
| **Google A2UI (v0.9)** | Specs are inert data; the **client owns a component catalog** referenced by id; the data model is separate from layout; actions go back to the owner as events | Streaming and flat adjacency lists, which we don't need because our specs are authored and reviewed |
| **Vercel json-render** | Catalog → validated spec → renderer, and it has a Svelte 5 renderer worth studying | `$computed`, `$template` and `watch`, which are too expressive for settings |
| **MCP elicitation (2025-11-25)** | A flat, primitive JSON Schema for asking a human for a value; URL mode for secrets so they never pass through the model | — |
| **AG-UI / CopilotKit** | RFC 6902 JSON Patch as the shape of a proposed change; the tool boundary as the approval gate | — |
| **MCP Apps, OpenAI Apps SDK, Thesys C1** | Only as a later option: fleet's renderer inside Claude Desktop | They ship HTML and JS, or a UI invented per turn, which is the opposite of a stable settings surface |
| **rjsf / svelte-jsonschema-form, SurveyJS, Form.io, Retool** | — | Rejected. They are form-centric with no scopes or modified state, allow arbitrary expressions or JS, or bring their own design system |

**What the field agrees on:** the model composes and the app owns the
vocabulary. Every serious 2026 generative-UI system constrains output to a
catalog the client owns and treats the description as data. Our case is
easier than theirs, because a page spec is written once, validated in CI and
reviewed in a PR; it is not invented on each user turn.

---

## 3. Recommended architecture: three layers, one fact each

```
┌──────────────────────────┐   describe_settings / settings_catalog (runtime)
│ 1. Registry (Rust)       │──────────────────────────────┐
│   fleet-core settings.rs │   types, defaults, validation,│
│   + resources            │   scope, danger, verdict, AI  │
└──────────────────────────┘                               ▼
┌──────────────────────────┐                     ┌────────────────────────┐
│ 2. Page specs (JSON)     │── validated in ────▶│ 3. Renderer (Svelte)   │
│   fleet-core/ui/pages/   │   Rust (CI + MCP)   │   ~12 widgets, 6       │
│   where + how, nothing   │                     │   layouts, closed      │
│   else                   │                     │   catalog              │
└──────────────────────────┘                     └────────────────────────┘
```

1. **The registry** is the one authority for *what a setting is*.
   `Spec` grows metadata:

   ```rust
   pub struct Spec {
       pub key: &'static str,
       pub default: &'static str,
       pub kind: Kind,            // + Secret, Text{max}, Ref(Resource)
       pub label: &'static str,
       pub help: &'static str,    // one paragraph, markdown subset
       pub unit: Unit,            // None | Hours | Days | Ms | Usd …: display only
       pub tags: &'static [Tag],  // Advanced, Experimental, Network, Ai
       pub scopes: &'static [Scope], // Global | Org | Host | Tracker
       pub danger: Danger,        // None | Confirm(&str) | TypeToConfirm
       pub restart: Restart,      // None | Hub | App
       pub ai: AiPolicy,          // Suggest | Fill | Never
   }
   ```

   The unregistered keys (`hub.*`, `mcp.*`, `operator.*`) join it, even if
   they stay read-only in the UI.

   **Resources** are collections of entities: trackers, orgs, peers, clients
   and hosts. Each one gets the same treatment:
   `ResourceType { id, fields, actions, title_field, badges, variant_by }`.
   `variant_by` is the single, explicit tagged union. A tracker's fields depend
   on its provider, and that is modelled as a closed set of variants, never as
   generic `oneOf`.

2. **Page specs** say only *where* a key appears and *how* it is shown. A spec
   **cannot** declare a type, a default, a label or a validation rule; it can
   only reference keys, resources and actions that the registry exports. Specs
   live in `fleet-core` and are compiled in with `include_str!`. The hub can
   therefore serve the same pages to a paired desktop or a phone, and a single
   Rust validator checks them in CI and in the MCP tool.

   Specs are written in JSON because both sides already parse it; YAML would add
   a dependency on each side. That choice is decision D-P3.

3. **The renderer** is a closed Svelte catalog. It has no HTML, no expressions
   beyond a tiny condition grammar, and no components inside specs. A new
   widget is a normal code change.

JSON Schema is an **export**, not the authoring format. The registry generates:

- the schema for the page DSL, used by CI and for an LLM's structured output;
- MCP elicitation schemas for individual primitive fields;
- the doc tables. All columns become generated, which removes the drift in
  Range and "What it does".

This follows the repo's existing `REGEN_*` pattern.

### Page DSL: ten node types, one way to do each thing

> **As built (P2):** every item is an object tagged by `type` (`field`,
> `stat`, `record`, `table`, `chart`, `notice`, `link`), rather than the
> per-kind arrays (`fields: [...]`, `derived: [...]`) used in the sketches
> below and in §7. One shape per purpose is easier for an agent to produce
> and easier for a schema to check. `action` and `collection` arrive with the
> resource registry in P4. The authoring guide is `docs/pages.md`, and the
> model is `crates/fleet-core/src/pages/model.rs`.

| Node | Purpose |
|---|---|
| `page` | A route: `id` (which is also the deep link `settings/<id>#<key>`), `title`, `icon`, `parent`, `layout` (one of six), and an optional `scope` picker |
| `tabs` | Page level only, never nested |
| `section` | A titled group. `collapsible: true` makes it an accordion; `advanced` sections collapse on their own |
| `field` | `key`, plus an optional `widget` override from the catalog and an optional `hint`. Label, help, validation and default all come from the registry |
| `collection` | Bound to a resource: list, detail, add flow, row actions, empty state |
| `action` | A button bound to a declared `ActionSpec`, never a free command |
| `when` | `{key, eq / in / truthy / not}` combined with `all / any`. Nothing else |
| `derived` | A read-only value from a named backend provider, e.g. `sync_metrics` |
| `notice` | A static info, warning or danger callout |
| `link` | To another page id or a doc anchor, checked to exist |

The DSL deliberately leaves out computed properties, templates, loops, styling,
grid spans, custom components and per-spec validation. When a page needs logic,
the logic becomes a backend `derived` provider or a new catalog widget.

### Widget catalog, about twelve widgets, each declaring which `Kind`s it accepts

`switch`, `tristate` (Inherit/On/Off), `number`, `duration` (stores seconds and
displays the registry unit), `select`, `multiselect`, `text`, `textarea`,
`secret` (write-only), `key_value_table` (only for real maps such as
`PathMap`/`PriceMap`), `ref_picker` (host/org/project/tracker), and `readonly`.
Two primitives the app lacks anyway come with this: `Tabs` and `Disclosure`.
Both are also useful outside settings.

### Scopes and inheritance

Values resolve through `default → hub → org → host → item`. When a page has a
scope picker, each field renders either **inherited** (a greyed value with a
source chip such as "from hub" or "default") or **overridden here** (a modified
bar and "Revert to inherited").

`orgs.auto_tidy`'s hand-rolled on/off/inherit becomes the generic rule: any
`Bool` shown at a non-global scope is a `tristate`.

---

### Beyond settings: sources, tables, charts and editable lists (D-P1)

The owner widened the scope on 2026-09-28. The same engine also drives pages
that are not settings: dynamic setup, prefilled data, charts, tables, editable
lists and forms. The guardrails stay the same. What changes is that a spec can
bind to three kinds of thing, all declared in Rust, instead of one:

| Binding | Declared as | Read | Write |
|---|---|---|---|
| `key` | a settings `Spec` | `read_all` | `settings::set` |
| `resource` | a `ResourceType` (fields, actions, `variant_by`) | its list/get provider | its `ActionSpec`s |
| `source` | a named **`DataSource`**: an id, typed params, a result shape (`Scalar`, `Record{fields}`, `Rows{columns}`, `Series{x, y[]}`), the verdict and `OrgScope` rule, and a live-update event | a service call | none; a source is read-only, and writes are always actions |

A `DataSource` is the "named backend provider" of `derived`, promoted so that
any widget can use one. Examples: `usage.by_model` (`Series`), `sessions.list`
(`Rows`), `work.today` (`Record`), and `tracker.sync_metrics` (`Record`).
Params come only from the route, the page's scope picker, or another field on
the same page (`{"param": "org_id", "from": "scope.org"}`). They are typed and
validated like settings.

**Prefilled forms.** A form in a `flow` or `object_editor` can take its initial
values from a source (`"prefill": {"source": "project.defaults", "params": {…}}`).
Prefilled values appear as defaults the user can see and override, never as
already-saved values. An agent's prefill is a *suggested value*, per §5.

**New widgets for data**, each declaring which result shapes it accepts:

| Widget | Accepts | Notes |
|---|---|---|
| `stat` | `Scalar` | Value, unit, and an optional delta against a second source |
| `table` | `Rows` | Columns come from the source's shape. Sort, filter and select come from the column types. Row actions are `ActionSpec`s. It is virtualised above about 200 rows |
| `list_editor` | `Rows` + a resource | Add, edit inline, reorder, remove. Each operation is an action, and a row's editor is an `object_editor` |
| `chart` | `Series` | `line`, `bar`, `stacked_bar`, `sparkline`. Charts are hand-rolled SVG so they need no dependency, since the app has no chart library. Colours come from the `app.css` tokens |
| `record` | `Record` | A read-only key → value list with typed formatting |
| `form` | a resource action's params | An inline form bound to one action (e.g. "add a quick reply") |

A **seventh layout**, `L7 data_page`, covers dashboards and reports that mix
these: a filter bar (fields bound to params) above a grid of `stat`, `chart`
and `table` blocks. The layout places the blocks; there are still no spans or
styles in specs.

**Where the engine stops.** It serves screens whose logic is "read named data,
show it, and run named actions on it". It does not serve screens with their
own interaction model: the terminal, the Work tree's drag-and-drop, and the
Transfer sheet's live steps. Those stay hand-built. A new need goes into a new
catalog widget or a new source; it never becomes an expression in a spec.

## 4. Prepared layouts

Every page chooses exactly one layout. The layout, not the individual field,
decides whether changes **auto-save** or go through a **draft with Apply**.

| Layout | Use for | Anatomy | Saving |
|---|---|---|---|
| **L1 `category`** | Scalar knobs: `gc.*`, `work.*`, `decide.*`, limits | Header with a modified count and optional scope picker; sections; an automatic "Advanced" section; a restart banner | Auto-save (debounced), with an inline tick and an 8 s undo toast |
| **L2 `master_detail`** | Trackers, orgs, peers, clients, hosts | List (title, badges, search) and a detail pane (an L4 editor); "Add" opens an L3 flow; row actions come from the resource | Detail uses a draft |
| **L3 `flow`** | Connect a tracker, pair a phone, add a peer, first run | Steps of kind `form / menu / progress / external / done / abort`; the **backend** may choose the next step (for example, a tracker URL implies the provider) | Draft, committed at the end |
| **L4 `object_editor`** | One tracker, one org, the hub's network config | Sections and a sticky Apply/Discard footer with a dirty count; danger confirms on Apply | Draft |
| **L5 `cards`** | The Settings root, integration health | Cards of `derived` values, a status dot, a link or action. No editable fields | — |
| **L6 `review_apply`** | AI proposals, imports, copying an org, resetting a page, bulk changes | A grouped diff (before → after, source); accept or reject per row; danger rows unchecked by default; "Apply selected" | Explicit |

L6 matters most for making AI part of the UI. Everything an agent proposes lands
there.

On a phone, the same spec degrades by rule: tabs become a list of sub-pages,
master-detail becomes list → push navigation, and sections become cards.
`LocalOnly` fields render read-only with the reason from `verdicts.rs`, which is
*parity or refusal*, rendered.

---

## 5. How AI fits in

### An agent authors a page

1. The agent writes a spec (`fleet-core/ui/pages/<id>.json`).
2. `settings_page_validate` checks it. This is the same Rust code CI runs, and
   it checks that:
   - the spec matches the DSL schema, with closed enums and no unknown fields;
   - every key, resource and action exists;
   - each widget is compatible with the field's `Kind`;
   - every `when` resolves;
   - **every registry key is placed exactly once or marked `unlisted`**, which
     replaces `every_spec_has_a_settings_dialog_row`;
   - `LocalOnly` controls are read-only on a hub client.
3. A preview route (`settings/_preview`) and one Vitest smoke test per layout.
4. A PR.

The vocabulary is small and every enum is closed, so a validator error is
precise enough for the model to fix in one retry.

### An agent operates the forms

- **An MCP tool, `settings`,** with `describe | get | propose | apply`.
  `describe` returns the catalog as a tool *result*, so it stays within the
  description budget. `propose` takes a JSON Patch over `{scope → key → value}`
  and returns a proposal id. **It never writes.**
- **Suggested values.** A proposal appears inline as a ghosted value with an
  "✦ suggested by Claude" chip, ✓/✗ buttons and a *Why?* popover. It also
  appears collected in an L6 review. This is the work graph's own rule R11
  applied to settings: an agent's answer is only ever a pre-selected
  suggestion.
- **`AiPolicy`:**
  - `Suggest` is the default.
  - `Fill` allows the agent to apply reversible, non-dangerous values inside a
    flow a person started.
  - `Never` covers secrets, `TypeToConfirm`, and consent flags such as
    `orgs.jev_allowed`.

  A per-host token proposes only within its `OrgScope`.
- **A natural-language palette.** "Set tidy idle to 3 days for Acme" is matched
  against labels, keys and help from `describe`. It produces an inline confirm
  row (`work.tidy_idle_unlinked_days: 7 → 3 (org Acme)`), and Enter applies.
  The model can only choose among declared keys.
- **Explain this setting** shows the registry help, the provenance chain and a
  doc link, and works with no model at all.
- **Audit.** Every write records who made it (a person, an agent with its
  session, or a hub client), the scope, before → after (secrets redacted) and
  the proposal id. A per-field "History" shows it. Secrets travel only through
  the `secret` widget or MCP elicitation URL mode, never through the model.

### Security invariants, each backed by a test

1. Specs are data. No string is evaluated as code, HTML or CSS, and help text
   is a markdown subset.
2. An unknown widget, key or action causes the spec to be **rejected**. There is
   no fallback rendering.
3. Bindings reach only registry keys and resource fields, and a spec cannot read
   a secret.
4. Actions run through the same service layer, verdicts and `OrgScope` as the
   Tauri command or MCP tool. The UI adds no authority.
5. Every write passes `settings::validate`, whatever its source.
6. Agent writes are proposals unless `AiPolicy::Fill` holds and the danger level
   is `None`.

---

## 6. Settings UX rules the renderer gives every page

- **Search** across label, key, help and tags, including keys no page places
  yet. Filters: `@modified`, `@tag:advanced`, `@scope:org`, `@restart`, `@ai`.
  A hit **scrolls to and highlights the control**.
- **Modified from default**: a left bar, "N modified" in the page header, and
  a gear menu with Reset, Copy key, Copy link and Explain.
- **Provenance chip** on every value: `default`, `hub`, `org: Acme`,
  `suggested by agent`.
- **Validation** comes from one authority. The backend checks on blur and on
  save, the range is shown ("1–365 days") rather than just "invalid", and
  form-level errors go in a `base` slot.
- **Danger, restart and secret** behaviour comes from the registry, not from
  the page.
- **Keyboard**: `⌘,` opens settings, `/` searches, arrow keys move in the
  page tree, and `Esc` reverts a draft field.
- **Accessibility**: `<label for>`, `aria-describedby` and a live region for
  errors. Tristate is a radio group, and state is never shown by colour alone.
- **Live updates**: a new `RowChange::Setting { key }` event, so an MCP or agent
  write shows up in an open page.

---

## 7. Worked example: Work → Trackers

This is P4's target, in the sketch syntax. The built DSL tags each item (see §3).

```json
{
  "spec": "fleet.page/1",
  "id": "work.trackers",
  "title": "Trackers",
  "parent": "work",
  "layout": "master_detail",
  "collection": {
    "resource": "tracker",
    "badges": ["state", "provider"],
    "columns": ["name", "provider", "last_sync"],
    "empty": {
      "title": "No trackers connected",
      "body": "Paste any ticket or issue URL and fleet works out the rest.",
      "primary": { "action": "tracker.add" }
    },
    "add": { "flow": "tracker.connect" },
    "row_actions": ["tracker.test", "tracker.sync_now", "tracker.remove"],
    "detail": {
      "tabs": [
        { "title": "Connection", "sections": [
          { "title": "Site", "fields": ["name", "site", "transport"] },
          { "title": "Credentials",
            "fields": ["credential",
                       { "key": "extra_ca", "when": { "key": "provider", "eq": "jira_dc" } }],
            "actions": ["tracker.test"] } ] },
        { "title": "Sync", "sections": [
          { "title": "Health", "derived": ["sync_metrics"] } ] },
        { "title": "Status map", "when": { "key": "provider", "eq": "asana" },
          "sections": [
            { "title": "Sections",
              "fields": [{ "key": "settings.section_map", "widget": "key_value_table" }] },
            { "title": "Proposals", "derived": ["status_map_proposals"],
              "actions": ["status_map.review"] } ] },
        { "title": "Write-back", "when": { "key": "provider", "eq": "jira_cloud" },
          "sections": [ { "title": "Pull requests",
            "fields": ["settings.write_back.pr_remote_link"] } ] }
      ]
    }
  }
}
```

The spec never mentions the hub. On a hub client, the verdicts for the tracker
actions and `work_admin` make the same page render read-only with the reason,
as `WorkSettings.svelte` does by hand today.

---

## 8. Build plan (slices)

Each slice deletes the hand-written UI it replaces, so the old and new paths
never coexist for long.

| Slice | Content | Replaces |
|---|---|---|
| **P1** ✅ | `Spec` metadata for all 56 keys (label, help, unit, `zero`, tags, danger, restart, ai), held by `every_spec_has_consistent_metadata`; `settings::describe()`, served by `get_settings { describe: true }` and the `describe_fleet_settings` command (`LocalOnly`, like `get_fleet_settings`); `docs/settings-reference.md` and the `work.*` / `decide.*` guide tables generated (`REGEN_SETTINGS_DOCS=1`). **Moved out:** `RowChange::Setting` goes to P3, where a page first listens for it and the wire-contract bump is justified. Registering `hub.*` / `mcp.*` / `operator.*` goes to P2 | Rustdoc-only help; the hand-kept doc tables |
| **P2** ✅ | `fleet-core::pages`, built:<br>• the `fleet.page/1` model, with unknown fields refused<br>• the catalog (a widget per setting kind, items per layout)<br>• data sources `usage.total`, `usage.by_day`, `usage.by_host` and `usage.by_model`, whose shapes are held by their readers<br>• the validator, which names the page and place of each problem<br>• seven pages in `crates/fleet-core/pages/` that give all 56 settings one home each (`every_setting_has_one_home`), plus a usage `data_page`<br>• generated `docs/page-spec.schema.json` and `docs/page-catalog.json` (`REGEN_PAGE_DOCS=1`)<br>• the guide `docs/pages.md`<br>**Moved out:**<br>• the preview route and the transport to the UI (`list_pages`, `fetch_source`) go to P3, with the renderer that consumes them<br>• `work.today` waits for an org-scoped source<br>• registering `hub.*` / `mcp.*` / `operator.*` is D-P7 | `every_spec_has_a_settings_dialog_row`, once P3's renderer replaces the dialog rows |
| **P3** ✅ | **Renderer.** `src/lib/pages/`: `PageView`, `FieldRow` (switch, number, duration, select, radio, multiselect, text, textarea, key_value_table, id_list, readonly), `DataItem` and a dependency-free `Chart`, `Tabs`, `Disclosure`, and `SettingsNav` with search (`@modified`, `@tag:`).<br>**Settings dialog.** It is now a wide dialog with a page tree beside the content. The hand-written Automation, Limits and Decisions sections are gone (the dialog is 1,607 → 904 lines). Two `custom` items carry what the catalog cannot express yet: `work_retention` and `auto_tidy_preview`.<br>**Transport.** `list_pages` (`SameInBoth`) and `fetch_page_source` (`LocalOnly`).<br>**Live updates.** `settings:changed` (kind `settings`, hidden from host-bound and org-bound streams); an open page re-reads on it.<br>**Registry additions.** D-P7: ten read-only `hub.*` / `mcp.*` specs, with pages Hub daemon and Control API. Option labels for choices.<br>**Tests.** The frontend runs against `registry.generated.json`.<br>**Still open.** Projects keeps its draft-and-rescan section until L4 (P4). On a hub client, pages show the hub reason until P6 | ~700 lines of `SettingsDialog.svelte`; `every_spec_has_a_settings_dialog_row` became `every_spec_is_mirrored_in_fleet_settings_ts` |
| **P4a** ✅ | **Resource registry** (`pages/resources.rs`): fields (text, color, bool, inherit, items), badges, confirms, and actions that name existing desktop commands with argument bindings, so hub verdicts and routing stay as they are.<br>**Layouts.** L2 `master_detail` and L4 (the record editor: a draft, Apply sends only what changed, confirms, list add/remove, delete always confirmed).<br>**Validator.** Every resource field is placed once; `when` conditions can name record fields; new `list_items`.<br>**Tests.** `resource_commands_exist` (src-tauri) and a hub reason for every update.<br>**Organisations.** Now a generated page, read-only with the hub reason on a paired desktop. `org_suggestions` is the third, and last allowed, custom item | `OrgSettings.svelte` (395 lines) and its tests, now `ResourcePage.test.ts` |
| **P4b** ✅ | **L3 `flow`** (`pages/flows.rs`): a server-driven wizard — `flow_start` / `flow_submit` / `flow_back` / `flow_cancel` (`LocalOnly`), steps with text / secret / textarea / bool / select fields, a secret never stored, a failed check answered as the same step with its error; `FlowView.svelte` only draws it.<br>**`tracker.connect`.** `trackers::infer` ports the frontend's URL inference to Rust (GitHub Enterprise included); the flow adds or updates the tracker with the settled site (fixing a port-in-URL refusal the old form had), stores the credential and tests it.<br>**Tracker resource.** New field kinds `choice` and `time`, a `label` badge, `merge(arg, path)` for a setting inside a JSON argument (PR write-back), record `actions` with `secret` params, `variants` by provider and `report` results, `create_flow`.<br>**Trackers page.** `settings.trackers` (master_detail) with state notices keyed on state and provider; the footer and Reconnect items open it. `tracker_extras` (sync metrics, the Asana section map, Jev proposals) is the fourth custom item; the cap is four until P5 pays it back | Most of `WorkSettings.svelte` (the tracker list, connect form and extras): what is left is the blurb and Usage |
| **P4b** | L7 `data_page` and the data widgets (`stat`, `table`, `chart`, `record`, `list_editor`, `form`). First page: usage by model and host | Hand-built usage views, where a spec covers them |
| **P5** | L6, `settings { propose | apply }`, suggested values, audit, the NL palette | — |
| **P6** | Settings routed through the hub (master token or an org-scoped `full` client), and the phone renderer over the same catalog | The `LocalOnly` prose on paired desktops |

One escape hatch is allowed: a `custom: <Component>` slot registered in code,
with CI capping it at three uses. `McpSettings` is the likely first user until
L4 covers it.

---

## 9. Decisions for the owner

| # | Question | Decision (accepted 2026-09-28) |
|---|---|---|
| D-P1 | Is the framework scoped to **settings** (keys and resources), or is it a general page engine for any screen? | **Decided 2026-09-28: general.** It covers settings plus pages with dynamic setup, prefilled data, charts, tables, editable lists and forms, bound to named `DataSource`s (§3, "Beyond settings"). Screens with their own interaction model stay hand-built |
| D-P2 | Where do page specs live? | `fleet-core`, compiled in, so the hub can serve them to clients and a phone |
| D-P3 | Spec format: JSON or YAML? | JSON, because it adds no dependency on either side |
| D-P4 | Do agents write settings at runtime (P5), or only author pages? | Both. Runtime writes go only through `propose` → review, with `AiPolicy` gates |
| D-P5 | Should Settings stay a modal, or become a full view with a page tree? | A full view with a left page tree and search. A 640 px modal cannot host master-detail |
| D-P6 | Should settings be routed on a hub client (P6)? | Yes, which revives audit iteration 03 |
| D-P7 | *Decided 2026-09-28: yes, read-only (built in P3).* Should `hub.*`, `mcp.*` and `operator.*` join the registry? They are deliberately not writable through `set_setting` (its tests refuse `mcp.confirm_destructive` and `hub.allow_plaintext`), and some are security posture. | Proposed: register them as **read-only** specs, so they get metadata and a place on a page (e.g. Settings → Hub, shown but not editable), while `set` keeps refusing them. Writing any of them from a page is a separate yes |

---

## Sources

VS Code [contribution points](https://code.visualstudio.com/api/references/contribution-points) ·
[settings editor](https://code.visualstudio.com/docs/configure/settings) ·
Home Assistant [data entry flow](https://developers.home-assistant.io/docs/data_entry_flow_index/) ·
JSON Forms [layouts](https://jsonforms.io/docs/uischema/layouts) ·
[svelte-jsonschema-form](https://github.com/x0k/svelte-jsonschema-form) ·
[Grafana option editors](https://grafana.com/developers/plugin-tools/how-to-guides/panel-plugins/custom-panel-option-editors) ·
[Raycast manifest](https://developers.raycast.com/information/manifest) ·
[Backstage field extensions](https://backstage.io/docs/features/software-templates/writing-custom-field-extensions/) ·
A2UI [announcement](https://developers.googleblog.com/introducing-a2ui-an-open-project-for-agent-driven-interfaces/), [a2ui.org](https://a2ui.org/) ·
[json-render](https://github.com/vercel-labs/json-render/blob/main/README.md) ·
[MCP Apps SEP-1865](https://modelcontextprotocol.io/seps/1865-mcp-apps-interactive-user-interfaces-for-mcp) ·
[MCP elicitation](https://modelcontextprotocol.io/specification/2025-11-25/client/elicitation) ·
[AG-UI state](https://docs.ag-ui.com/concepts/state) ·
[OpenAI Apps SDK state](https://developers.openai.com/apps-sdk/build/state-management) ·
[Thesys OpenUI](https://www.thesys.dev/blogs/openui) ·
[Form.io comparison](https://form.io/json-schema-forms-formio-rjsf-jsonforms-surveyjs-compared/)
