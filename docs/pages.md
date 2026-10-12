# Writing a page

A fleet page is a JSON file, not Svelte. It says which settings and data go on
the page and in what order. Everything else comes from the registries in
`crates/fleet-core`: each setting's label, help, bounds, danger and default,
and each data source's shape. A renderer turns the file into the page. The
design is `docs/superpowers/specs/2026-09-28-declarative-pages-design.md`.

In the app, every page appears in Settings. The left-hand list is the one
Settings tree (`src/lib/settings_tree.ts`, redesign step 7.1): groups such as
General, Sessions, Work and System, each leaf a hand-written panel, a page,
or one section of a page (Voice is the Voice section of Limits). A page the
tree does not name still shows: nested under its parent's leaf, or in a
"More" group at the end. The search box above it finds
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
3. Run `cargo fleet-test -- pages::`. Each problem is reported with the
   page and where in it, for example
   `settings.work › tab 1 › section 2 › item 3: `work.nope` is not a registered setting`.
4. Give it a leaf in `SETTINGS_TREE` (`src/lib/settings_tree.ts`) when it
   should not just sit under its parent. `settings_tree.test.ts` fails a
   page that no leaf reaches, and a leaf naming a section the page lacks.

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
`REGEN_PAGE_DOCS=1 cargo fleet-test -- page_docs_are_current`.

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
| `category` | Settings in sections, saved as you change them | `field`, `stat`, `record`, `table`, `action`, `notice`, `link`, `custom` |
| `cards` | An overview: numbers and links | `stat`, `record`, `notice`, `link` |
| `data_page` | Stats, charts and tables, with an optional filter bar (see *Data pages and filters*) | `stat`, `record`, `table`, `chart`, `account_usage` (`block`), `action`, `notice`, `link` |
| `embed` | Items placed inside one of the desktop's own screens at a named `slot` (see *Embed pages*); never in the page tree | `account_usage`, in the views its slot takes |
| `master_detail` | A list of a resource's records beside one record's editor | `field` (naming a field of the resource), `notice`, `custom`; `list_items` above the list: `notice`, `custom` |
| `flow` | A server-driven wizard (see *Flows*); not a page of its own yet, launched by a resource's `create_flow` | — |
| `review_apply` | Reviewing proposed changes (see *Proposals*); names its `review` (`settings`, or `guides` for Settings → Guides) | `notice`, `link`; sections are optional |
| `guide` | A task walked through one step at a time (see *Guides*); compiled in, or proposed by an agent at runtime | `field`, `stat`, `record`, `action`, `notice`, `link` |
| `object_editor` | One object on its own page | Not yet |

### Items

Every item is an object tagged by `type`:

| `type` | Fields | Notes |
|---|---|---|
| `field` | `key`, optional `widget`, `hint`, `when` | A registered setting. It gets exactly one home across all pages. `widget` must accept the setting's kind (see the catalog). `hint` is one extra line, 120 characters at most |
| `stat` | `source`, then `field` for a record source, optional `label` | One number |
| `record` | `source` | A key → value list of a record source |
| `table` | `source`, optional `columns`, `copy` | A rows source; `columns` picks and orders columns. `copy: true` adds *Copy as text*: the source's label with the filters, then one line per row (`first: rest, …`) |
| `chart` | `source`, `chart` (`line` / `bar` / `stacked_bar` / `sparkline`), optional `title` | A series source |
| `account_usage` | `source` (an `account_usage` source: `accounts.usage`), `view` | Claude accounts' plan headroom, with the hosts-view design's wording, staleness and severity in every view. On a page, `block` for every account; in a slot, the view the slot takes for the slot's host or account |
| `notice` | `tone` (`info` / `warn` / `danger`), `text` | Plain text, 300 characters at most |
| `action` | `action` | A button that runs a page action (see *Page actions*), then re-reads the page's data items. Hidden on a read-only page |
| `link` | `page`, optional `label` | Another page's id |
| `custom` | `component` (`auto_tidy_preview` / `org_suggestions` / `tracker_extras`) | A hand-written component registered in code: the one escape hatch, capped at three uses across all pages (`MAX_CUSTOM`). Replace one with a data source and an action when the catalog can express it, as P5 did for work retention |

A `source` is `{ "id": "usage.by_day", "params": { "days": 30 } }`. Parameters
are literals, checked against the source's declared parameters.

### Data pages and filters

A `data_page` may have a filter bar: `filters`, a list of controls, each
bound by name to a source parameter. A filter sets that parameter on every
data item whose source declares it, and the items re-read when it changes;
a source without the parameter ignores the filter.

```json
"filters": [
  { "param": "days", "label": "Window", "choices": [7, 30, 90], "default": 30 },
  { "param": "host", "label": "Host" }
]
```

The control follows the parameter's type. A `days` parameter is a select
over `choices` (ascending, each within every declaring source's bounds;
`default` is one of them, else the first). A `host` parameter is a select
over the registered hosts, starting at *All hosts*, which sends no host;
it takes no `choices` or `default`. A data item may not also set a
filtered parameter in its own `params`. `usage` and `usage.work` are the
examples.

### Embed pages

An `embed` page (layout L8) places items inside a screen the desktop draws
by hand, at a named `slot`, instead of on a page of its own. The screen
owns the slot and hands it a context; the spec decides what goes there.
Each slot has at most one page, an embed page has no `parent` and no tabs,
and nothing links to it. `list_pages` never carries embed pages: the
desktop reads them from `src/lib/pages/embeds.generated.json`, generated
from the same specs, so a slot draws on the first frame (and a phone never
sees them).

| `slot` | Where | Context | Views |
|---|---|---|---|
| `host_detail` | Host detail, under the account | the host, its account and snapshot, the other hosts on the account, the refresh | `block` |
| `hosts_group_title` | A Hosts-list account group's title line | the account and snapshot | `freshness` |
| `hosts_group` | A Hosts-list account group's header | the account and snapshot | `bars` |
| `new_session_chip` | Each New-session host chip | the host, its account and snapshot | `chip` |
| `new_session_host` | Under the New-session chips | the selected host, every host, its account and snapshot | `line`, `warning` |
| `status_footer` | The status footer's right end | every host, account and snapshot; open the Hosts view | `footer` |

```json
{ "spec": "fleet.page/1", "id": "embed.new_session_host", "title": "New session: selected host",
  "layout": "embed", "slot": "new_session_host",
  "sections": [{ "title": "Usage", "items": [
    { "type": "account_usage", "source": { "id": "accounts.usage" }, "view": "line" },
    { "type": "account_usage", "source": { "id": "accounts.usage" }, "view": "warning" }
  ] }] }
```

A new slot is a code change: a `Slot` variant and its views in
`catalog::slot_views`, the context in `src/lib/pages/usage/context.ts`,
the owner's `<EmbedSlot slot=…>`, and the slot in `RENDERED_SLOTS`
(`embeds.test.ts` holds the filled slots, the rendered ones and their
owners equal).

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

A section with `"matrix": true` shows its fields as one grid
(`MatrixSection.svelte`): a row per option, a column per field, a checkbox
in each cell, as Settings → Notifications does with Desktop, Phone and
Sound. Every field must be a `choice_set` setting over the same options,
with no `widget`, and there must be at least two of them. A resource page
has no matrix.

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
  - `count`, a whole number the backend counts, shown and never edited (an
    absent value reads `0`);
  - `money`, micro-USD shown as dollars, and `money_series`, a
    `[{ day, cost_micros }]` list shown as a bar chart with a table toggle;
    an absent value leaves either out (the caller may not see spend);
  - `sync`, a transfer in progress (`{ done, total, both_ways, since? }`,
    a hub link's): a Constellation with the real count ("412 of 1 280
    messages · 18 s") while `total` is above `done`, a Counter-orbit while
    both ends trade with nothing queued; absent while idle, and left out;
  - `items`: a list changed through its own `remove` and `add` actions,
    shown by a closed formatter (`plain`, `field`, `org_rule`, `device`,
    `member`, or `admin_need`, which lists each need on its own line with
    why or when).
    A record that carries no such key at all (an older hub, or a list only
    the operator is shown) leaves the field out rather than showing it
    empty;
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
  `report` action shows the command's `{ ok, error }` as a toast, and a
  `busy` action shows that loader beside its form while the command runs
  (`counter-orbit` when Link a hub talks to the other hub).

Every action names an **existing desktop command** and binds each argument
to one of: a record field, the sub-item or its field, a form param, or
`null`. It never names code. The command's hub verdict therefore applies
unchanged. On a paired desktop the list routes to the hub, and the page is
read-only with that command's reason.

A `master_detail` page names its resource and lays out one record's fields
in sections, each field exactly once. A section with `"tiles": true` holds
only `count` and `money` fields and shows them as tiles: the label, the
value, and the field's `sub` line (`budget`: "82% of $750" from a budget in
whole USD, with a Meter that warns from 80% and is critical once reached;
`count`: "2 need you"), as the org overview does. The record's fields can be
laid out in `tabs` instead of `sections` (the org's Overview, Members,
Devices, Spend and Settings): the editor keeps one draft across them, and a
tab whose sections hold exactly one list shows its count. A field label may
name the record with `{title}`, its one placeholder ("Allow Jev (decision
model) for {title}'s work" reads "… for Acme's work").

A page may also draw its records as a `graph` above the list — the centre
node joined to each record, the line solid while the record's `state`
(a `choice` field) is one of `up` and dashed otherwise, with up to three
plain `facts` (text, count or time fields) under each name. The list stays
the keyboard path, and the figure says the same in its label and legend.
Federation's linked hubs use it:

```json
"graph": { "center": "This hub", "state": "state", "up": ["connected"],
           "facts": ["latency", "messages_today"] }
```

The org overview, shortened:

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

### Page actions

A page action (`crates/fleet-core/src/pages/actions.rs`, `PAGE_ACTIONS`) is
a button that runs one existing desktop command with no arguments: an
`id`, a `label`, the `command`, one sentence of `help`, and an optional
`confirm`. A page places it with `{ "type": "action", "action":
"work.retention_sweep" }`; after it runs, the page's data items read their
sources again. Like a resource's actions it names a command, never code
(`resource_commands_exist` checks it), so the command's hub verdict applies
and a read-only page shows no button. Work retention on the Work page is
the first: the `work.retention` table, the `work.retention_last` record
and *Sweep now*, where a hand-written component was before.

### Proposals (`review_apply`) and history

An agent proposes a settings change with `set_setting { propose: true,
why }` (control API); nothing is written until a person decides
(`service/settings_review.rs`, migration 083). A `review_apply` page names
what it reviews — `"review": "settings"` is the one source — and lists the
pending proposals grouped by the page each setting lives on, as now →
proposed with who and why. A row whose value moved since it was proposed,
or whose setting needs confirming, starts unticked; **Apply selected** and
**Reject selected** decide the ticked ones, each on its own. Every page
also shows a field's own proposal inline (✓ Apply, ✗ Not this, Why?), and
every field has a **History** of its writes (`setting_history`: who,
before → after, the proposal). A key whose AI policy is `never` — every
confirmed change, every read-only key — cannot be proposed.

### Search as a command

The Settings search takes a command in plain words: `set recent work to 3
days`, `change keep lost sessions to 2 days`, `turn on press enter`,
`disable collect token usage` (`src/lib/pages/settings_nl.ts`). The words
are matched against the registry's labels and keys and the value is parsed
by the setting's own kind (on / off, a number in the unit the field shows
or with its own unit, an option's name or label), so only a declared key
and a value it can hold come out. It shows one row, now → new; Enter or
**Apply** writes it as the person, and a setting that needs confirming asks
first. When the words fit several settings about as well, it lists them
instead of guessing. No model is involved.

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

### On a paired desktop (P6)

A desktop paired with a hub shows the **hub's** settings on the same pages:
`describe_fleet_settings`, `get_fleet_settings`, `set_fleet_setting`, the
proposals and a field's History all route to the hub. One line at the top
of each page says so. A device the hub's operator trusts (`fleet-hub client
trust <name>`) edits the fields and decides proposals; an untrusted one
sees them read-only, with that command. Data items, page actions and
custom components read or run on this app's own store, so a paired desktop
shows none of them (a section of data items only is left out, and a
`data_page` says where its data is instead); a resource page (Trackers, Organisations) stays
read-only with the hub's reason. A hub that serves no settings to the
device (an older hub, or a device bound to one org) leaves the page on its
reason and the hub's answer.

### Guides

A guide (layout L9 `guide`) walks a person through one task, one step at a
time. Its `sections` are the steps, in order, shown with Back / Next / Done
and a *Step 2 of 4* line; a step's `when` drops it until it applies, so the
count follows the answers so far. A step holds settings `field`s (saved as
they change, like a `category` page), `notice`s, `link`s, page `action`s and
`stat` / `record` values. A guide passes through settings whose home is
another page: its fields never count as a setting's one home, and a key
appears at most once per guide. At most 12 steps, each titled differently,
none collapsible or advanced.

```json
{ "spec": "fleet.page/1", "id": "guide.cleanup", "title": "Let fleet tidy up idle sessions",
  "parent": "guides", "layout": "guide",
  "sections": [
    { "title": "What it does", "items": [ { "type": "notice", "tone": "info", "text": "…" } ] },
    { "title": "Turn it on", "items": [ { "type": "field", "key": "gc.enabled" } ] },
    { "title": "When it acts", "when": { "key": "gc.enabled", "truthy": true },
      "items": [ { "type": "field", "key": "gc.bg_idle_secs", "hint": "A day suits most fleets." } ] } ] }
```

**Guides an agent writes.** A guide can also be stored at runtime
(`service::guides`, migration 088), so a Claude session on a host — which
has no checkout of this repository — can write one. It uses the control
API's `guide` tool (`docs/control-api.md`), usually through the
`fleet-guides` skill that ships in `catalog-seed/` for your asset catalog:

1. `guide { action: catalog }`: the rules, every setting's key, label,
   help, kind, unit, default, what `0` means and home page (never a value),
   the pages, page actions and read-only sources a guide may name, and a
   working example.
2. `guide { action: validate, spec }` answers `{ ok, problems }`, each
   problem with where it is — the same `pages::validate` as every page,
   against this build's registries — plus: id `guide.<name>` and none of
   the app's pages, parent `guides`, 16 KiB at most.
3. `guide { action: propose, spec, why }` stores a proposal. **Nothing is
   shown until a person approves it**: in Settings → Guides (each proposal
   with who, why and its steps in words, then Approve / Reject), or with
   `fleet-hub guides list | show <id> | approve <id> | reject <id> |
   remove <guide id>` on a hub. Deciding and removing need the master or a
   trusted device; a host's token only proposes. 20 wait at most, 50 are
   live; a newer proposal for an id replaces the pending one, and a live
   guide stays until its revision is approved.

Approved guides join the page tree under Settings → Guides. A live guide
is checked again whenever the pages are read, so one that names a setting
a later build removed is left out rather than drawn broken. On a paired
desktop the guides are the hub's (`list_guides`, `decide_guide`,
`remove_guide` route to its `guide` tool).

## Rules the validator enforces

- Every registered setting is on exactly one page (`every_setting_has_one_home`),
  or it is in `pages::UNLISTED` with a sentence saying why. A guide's
  fields are not homes.
- Keys, sources, source fields and columns, widgets, parents and link targets
  must all exist.
- A `slot` is on an `embed` page only, once per slot, and an `account_usage`
  item takes only the views its slot (or, on a page, `block`) allows.
- Filters are on a `data_page` only, each names a parameter some source on
  the page declares (with one type across them), and no item sets a
  filtered parameter itself.
- Titles are at most 60 characters. Text is plain, with no `<` or `>`.
- Parents form a tree.
- A `graph` is on a `master_detail` page; its `state` is a `choice` field,
  `up` names some of its values (not none, not all), and its facts are at
  most three plain fields.
- Unknown fields and item types are refused when the file is parsed.

## Add a setting

1. Add a `Spec` row in `crates/fleet-core/src/service/settings.rs` with its
   label, help, unit and the rest (see `every_spec_has_consistent_metadata`).
2. Place the setting as a `field` on the right page.
3. Regenerate the settings docs:
   `REGEN_SETTINGS_DOCS=1 cargo fleet-test -- settings_docs_are_current`.
4. Regenerate the page docs (see above).

## Add a data source

1. Declare it in `SOURCES` in `crates/fleet-core/src/pages/sources.rs`: an id,
   a label, help, its shape (`scalar`, `record`, `rows` or `series`, with typed
   columns) and its parameters.
2. Add its reader to `fetch`.

`every_source_has_a_reader_that_returns_its_shape` holds the reader to the
declared shape. A **live** source (`live: { command, event }`, today
`accounts.usage` and `updates.targets`) has no reader: the app loads it
with `command` and keeps it current from the `event` row kind, so `fetch_page_source` refuses it and
`resource_commands_exist` holds `command` to the handler list. A source is read-only; changes are made through actions
(design P4). Every source reads the whole fleet, so it is only for whatever
owns the fleet. An org-scoped view is a new source, not a parameter.
