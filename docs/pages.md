# Writing a page

A fleet page is a JSON file, not Svelte. It says which settings and data go on
the page and in what order. Everything else comes from the registries in
`crates/fleet-core`: each setting's label, help, bounds, danger and default,
and each data source's shape. A renderer turns the file into the page. The
design is `docs/superpowers/specs/2026-09-28-declarative-pages-design.md`.

In the app, every page appears in Settings. The left-hand list shows
"General" (the hand-written panels), the Settings overview and its pages,
then any other top-level page such as Usage. The search box above it finds
any setting by its label, key, help text or tags. `@modified` lists the
settings that differ from their default, and `@tag:experimental` (or
another tag) filters by tag. Choosing a result opens its page and tab and
highlights the setting.

This guide is for people and for agents. A spec is data. It cannot hold code,
markup, styles or validation rules, and the validator refuses anything the
registries don't know.

## Add a page

1. Write `crates/fleet-core/pages/<id>.json`. The file is named after the
   page's `id`.
2. List the file in `PAGE_FILES` in `crates/fleet-core/src/pages/mod.rs`.
3. Run `cargo test -p fleet-core pages::`. Each problem is reported with the
   page and where in it, for example
   `settings.work › tab 1 › section 2 › item 3: `work.nope` is not a registered setting`.

The two generated files are what you write a spec against:

- `docs/page-spec.schema.json` is the JSON Schema for a spec. Use it in an
  editor, or as an LLM's structured-output schema.
- `docs/page-catalog.json` lists what a spec can name:
  - every layout and the items it holds;
  - every widget and the setting kinds it takes;
  - every setting with its default widget and allowed widgets;
  - every data source with its shape and parameters.

Regenerate both, together with the frontend tests' fixture, after changing
the model, the catalog, a setting or a source:
`REGEN_PAGE_DOCS=1 cargo test -p fleet-core page_docs_are_current`.

## Shape

```json
{
  "spec": "fleet.page/1",
  "id": "settings.automation",
  "title": "Automation",
  "parent": "settings",
  "intro": "One plain sentence under the title.",
  "layout": "category",
  "sections": [
    {
      "title": "Garbage collection",
      "items": [
        { "type": "field", "key": "gc.enabled" },
        { "type": "field", "key": "gc.bg_idle_secs",
          "when": { "key": "gc.enabled", "truthy": true } }
      ]
    },
    { "title": "Reconcile tick", "advanced": true, "collapsible": true,
      "items": [ { "type": "field", "key": "reconcile.interval_secs" } ] }
  ]
}
```

A page has either `sections` or `tabs` (each tab has a `title`, an optional
`when` and its own `sections`), never both. `parent` places the page in the
page tree. `id` is dotted lowercase and is also the page's deep link.

### Layouts

| Layout | For | Items it holds |
|---|---|---|
| `category` | Settings in sections, saved as you change them | `field`, `stat`, `record`, `notice`, `link`, `custom` |
| `cards` | An overview: numbers and links | `stat`, `record`, `notice`, `link` |
| `data_page` | Stats, charts and tables | `stat`, `record`, `table`, `chart`, `notice`, `link` |
| `master_detail` | A list of a resource's records beside one record's editor | `field` (naming a field of the resource), `notice`, `custom`; `list_items` above the list: `notice`, `custom` |
| `flow` | A server-driven wizard (see *Flows*); not a page of its own yet, launched by a resource's `create_flow` | — |
| `object_editor`, `review_apply` | One object on its own page, reviewing proposed changes | Not yet (design P5) |

### Items

Every item is an object tagged by `type`:

| `type` | Fields | Notes |
|---|---|---|
| `field` | `key`, optional `widget`, `hint`, `when` | A registered setting. It gets exactly one home across all pages. `widget` must accept the setting's kind (see the catalog). `hint` is one extra line, 120 characters at most |
| `stat` | `source`, then `field` for a record source, optional `label` | One number |
| `record` | `source` | A key → value list of a record source |
| `table` | `source`, optional `columns` | A rows source; `columns` picks and orders columns |
| `chart` | `source`, `chart` (`line` / `bar` / `stacked_bar` / `sparkline`), optional `title` | A series source |
| `notice` | `tone` (`info` / `warn` / `danger`), `text` | Plain text, 300 characters at most |
| `link` | `page`, optional `label` | Another page's id |
| `custom` | `component` (`work_retention` / `auto_tidy_preview` / `org_suggestions` / `tracker_extras`) | A hand-written component registered in code: the one escape hatch, capped at four uses across all pages (`MAX_CUSTOM`; design P5 pays it back). Replace one with a data source and an action when the catalog can express it |

A `source` is `{ "id": "usage.by_day", "params": { "days": 30 } }`. Parameters
are literals, checked against the source's declared parameters.

### Conditions (`when`)

Each condition uses exactly one form:

- `{ "key": "<setting>", "eq": "<value>" }`
- `{ "key": "<setting>", "in": ["a", "b"] }`
- `{ "key": "<on/off setting>", "truthy": true }`
- `{ "all": [ … ] }`, `{ "any": [ … ] }`, `{ "not": { … } }`

The values in `eq` and `in` must be values the setting can actually take.
There are no expressions. If a page needs more than these forms, add a data
source or a widget in code.

### What the renderer does with a field

Everything below comes from the registry. The page spec never says any of it:

- The widget: the setting kind's default, or the page's `widget` if it names one.
- The label, the help text, and the range with what `0` means.
- The unit. Seconds are shown in hours or days when the registry says so,
  and converted back when written.
- The option labels of a choice.
- A blue bar and a **Reset** button when the value differs from the default.
- A confirmation before a change that carries `danger`.
- An "applies after a restart" tag.
- For a key another subsystem owns (`owned_by`), the value shown read-only,
  with where to change it.

Changes are saved as they are made, through `set_fleet_setting`, and the
backend's `E_INVALID` message appears under the field. A `settings:changed`
event from another window, or from an agent over the control API, refreshes
an open page.

The renderer lives in `src/lib/pages/`:

- `PageView.svelte`, the page itself;
- `FieldRow.svelte`, one setting;
- `DataItem.svelte` and `Chart.svelte`, the data items;
- `Tabs.svelte` and `Disclosure.svelte`, tabs and collapsible sections;
- `SettingsNav.svelte`, the page list and search.

Its tests run against `src/lib/pages/registry.generated.json`. That file holds
exactly what `list_pages` and `describe_fleet_settings` answer on a fresh
store, so it is never a hand copy. It is regenerated with the other page docs.

### Resources and `master_detail` pages

A resource (`crates/fleet-core/src/pages/resources.rs`) is a collection of
things, such as organisations, that a person adds, edits and removes. Its
type declares:

- `list`: the desktop command that returns the records;
- the fields of a record, each of these kinds:
  - `text`;
  - `color`;
  - `bool`, where `on_off` writes `"on"` / `"off"`;
  - `inherit`, which is on / off / inherit;
  - `choice`, a closed set of values with labels (shown read-only, or as a
    `label` badge);
  - `time`, a unix time shown as "5 min ago";
  - `items`: a list changed through its own `remove` and `add` actions,
    shown by a closed formatter (`plain`, `field`, `org_rule`);
- which fields are editable (`edit` names the update argument), with a
  `badge` shown in the list and a `confirm` asked before Apply. A field kept
  inside a JSON argument (a tracker's `settings.write_back.pr_remote_link`)
  declares `merge(arg, path)`: Apply sends the whole argument with only that
  path changed, so the other settings survive;
- `create`, `update` and `delete`, or `create_flow` naming a flow (below);
- record `actions` shown in the editor's header. An action's params are
  `text`, `secret` (a password input, never kept), `color` or `options`;
  `variants` limits an action to records whose `variant_by` field holds one
  of the listed values (Jira asks an email and a token, Asana a token); a
  `report` action shows the command's `{ ok, error }` as a toast.

Every action names an **existing desktop command** and binds each argument
to one of: a record field, the sub-item or its field, a form param, or
`null`. It never names code. The command's hub verdict therefore applies
unchanged. On a paired desktop the list routes to the hub, and the page is
read-only with that command's reason.

A `master_detail` page names its resource and lays out one record's fields
in sections, each field exactly once:

```json
{ "spec": "fleet.page/1", "id": "settings.orgs", "title": "Organisations",
  "parent": "settings", "layout": "master_detail", "resource": "org",
  "list_items": [ { "type": "custom", "component": "org_suggestions" } ],
  "sections": [
    { "title": "Organisation", "items": [ { "type": "field", "key": "name" },
                                           { "type": "field", "key": "color" } ] } ] }
```

The renderer (`src/lib/pages/ResourcePage.svelte`, `RecordEditor.svelte`,
`ActionForm.svelte`) edits scalar fields as a draft. **Apply** sends the
record's id and only the fields that changed, after asking any `confirm`.
Discard drops the draft. A list changes item by item through its actions,
and removing a record is always confirmed. A failed command becomes a toast,
and the list is re-read either way.

### Flows

A flow (`crates/fleet-core/src/pages/flows.rs`, listed in `FLOWS`) is a
wizard whose steps the **backend** decides: `flow_start { flow, prefill }`
answers the first step, `flow_submit { flowId, values }` the next step or
`{ state: done, message, record_id }`, and `flow_back` / `flow_cancel` do
what they say. A step is a title, an intro, fields (`text`, `secret`,
`textarea`, `bool`, `select`) and an optional error; the renderer
(`FlowView.svelte`) only draws it. A `secret` field is never stored in the
flow and is cleared on screen once sent. A failed check (a tracker's Test)
answers the same step again with the tracker's own error. Flows live in
memory for 30 minutes, at most 32 at once.

The one flow is `tracker.connect`: paste a ticket URL (`trackers::infer`
guesses the provider, the site and GitHub Enterprise's hostname), then give
what that provider needs; it adds or updates the tracker, stores the
credential and tests it, through the same service calls as `work_admin`.
All four commands are `LocalOnly`: on a hub, trackers are fleet
administration (`fleet-hub tracker add <ticket-url>`).

To add a resource:

1. Declare it in `RESOURCES`. `resources_are_well_formed` checks the
   bindings, namespaces and sentences.
2. Name only commands `lib.rs` registers (`resource_commands_exist`).
3. Give its update command a hub reason (`pages.test.ts`).
4. Add its store reloaders to `RESOURCE_RELOADERS` in
   `src/lib/pages/resources.ts` when other views keep a copy.

## Rules the validator enforces

- Every registered setting is on exactly one page (`every_setting_has_one_home`),
  or it is in `pages::UNLISTED` with a sentence saying why.
- Keys, sources, source fields and columns, widgets, parents and link targets
  must all exist.
- Titles are at most 60 characters. Text is plain, with no `<` or `>`.
- Parents form a tree.
- Unknown fields and item types are refused when the file is parsed.

## Add a setting

1. Add a `Spec` row in `crates/fleet-core/src/service/settings.rs` with its
   label, help, unit and the rest (see `every_spec_has_consistent_metadata`).
2. Place the setting as a `field` on the right page.
3. Regenerate the settings docs:
   `REGEN_SETTINGS_DOCS=1 cargo test -p fleet-core settings_docs_are_current`.
4. Regenerate the page docs (see above).

## Add a data source

1. Declare it in `SOURCES` in `crates/fleet-core/src/pages/sources.rs`: an id,
   a label, help, its shape (`scalar`, `record`, `rows` or `series`, with typed
   columns) and its parameters.
2. Add its reader to `fetch`.

`every_source_has_a_reader_that_returns_its_shape` holds the reader to the
declared shape. A source is read-only; changes are made through actions
(design P4). Every source reads the whole fleet, so it is only for whatever
owns the fleet. An org-scoped view is a new source, not a parameter.
