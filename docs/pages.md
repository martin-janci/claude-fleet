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
| `master_detail`, `flow`, `object_editor`, `review_apply` | Lists of resources, wizards, one object with Apply, reviewing proposed changes | Not yet: these wait on the resource registry (design P4) |

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
| `custom` | `component` (`work_retention` / `auto_tidy_preview`) | A hand-written component registered in code: the one escape hatch, capped at three uses across all pages. Replace one with a data source and an action when the catalog can express it |

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
