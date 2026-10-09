# Chat blocks

An agent's reply in the Conversation view is Markdown. Two kinds of text in
it are drawn as cards instead:

- **A task report.** A `FLEET_TASK_DONE_<nonce>` line followed by one JSON
  object (fenced ```` ```json ```` or bare), the report a run's prompt asks
  for (`run_instruction` in `crates/fleet-core/src/service/work/report.rs`).
  The card shows the outcome, the confidence, the summary, then blockers,
  warnings, tests run and follow-ups. A follow-up's *Ask* button puts it in
  the composer. The JSON is still there under *Reported after FLEET_TASK_DONE_…*.
- **A `fleet.ui/1` block.** A fenced ```` ```fleet-ui ```` block (or a
  ```` ```json ```` block whose object has `"spec": "fleet.ui/1"`) holding one
  block: a tutorial, a guide, a callout, facts, choices, a form, a report,
  a job's progress, results, an error or a settings change.

A card is a view of the reply. Nothing is stored and nothing is sent from
one, with two exceptions: a `setting` card applies a settings change an
agent proposed, after the person confirms it, and a `wizard` card runs one of
the app's wizards when the person presses its last button. A card that acts (choices, a form, a follow-up) only fills the
session's composer, and the person presses Enter. It acts only when the
composer is on screen for the conversation shown. An earlier conversation
or a row with no REPL draws the card with its actions turned off.

A block that does not check out is drawn as the code block it was written
as, with what is wrong under it (`Not shown as a card: …`). A fence that has
not closed yet, because the reply is still being written, stays code until
it closes.

## When to use which

| You want the person to… | Use |
|---|---|
| answer now, while you wait | `ask` (`docs/forms.md`), not a block |
| follow a procedure, ticking steps off | `steps` |
| read reference material in parts, or walk a guide fleet has | `guide` |
| notice one thing (a risk, a tip) | `callout` |
| see a few labelled values at a glance | `facts` |
| pick what you do next | `choices` |
| fill in several values for your next turn | `form` |
| see a run's result | `report` (or the done marker) |
| follow a long job as it moves on | `progress` |
| read numbers, a chart or a table you measured | `results` |
| see what failed and pick what to do about it | `error` |
| apply a settings change you proposed | `setting` |
| run one of the app's wizards (add a project, a host, a session, …) | `wizard` |

A block's form is not `ask`: you do not wait for it, and its answers arrive
as the person's next prompt. Use it when the answers can wait for the next
turn. Use `ask` when you need them before you go on, or when a field is a
secret. A block refuses secret fields, since their answers would land in
the transcript.

## Format

Every block is one JSON object with `"spec": "fleet.ui/1"` and a `kind`.
Text marked *Markdown* is rendered as Markdown. Every other string is
shown as plain text. Unknown keys are ignored. A block is at most 32 KiB.

````
```fleet-ui
{ "spec": "fleet.ui/1", "kind": "callout", "tone": "warning",
  "title": "Production", "body": "This restarts the **live** hub." }
```
````

### `steps`: a tutorial

| Key | Required | Meaning |
|---|---|---|
| `title` | yes | ≤ 120 chars |
| `intro` | no | Markdown, ≤ 2000 |
| `steps` | yes | 1–30 of `{ title, body?, code?, lang? }` |

`body` is Markdown (≤ 4000); `code` (≤ 8000) is drawn as a code block in
`lang`. Each step has a checkbox and the header counts `N of M done`. The
ticks last while the window is open.

```json
{ "spec": "fleet.ui/1", "kind": "steps", "title": "Run the hub locally",
  "steps": [
    { "title": "Build it", "code": "cargo build -p fleet-hub --locked", "lang": "sh" },
    { "title": "Start it", "body": "Point it at a **fresh** data dir.", "code": "fleet-hub --data /tmp/hub", "lang": "sh" }
  ] }
```

### `guide`: sections

| Key | Required | Meaning |
|---|---|---|
| `title` | yes | ≤ 120 chars |
| `intro` | no | Markdown, ≤ 2000 |
| `sections` | yes | 1–20 of `{ title, body }`, `body` Markdown ≤ 8000 |

Each section folds. The first one starts open.

**A guide fleet already has.** With `page` (a page id, letters, digits and
`. _ : -`, ≤ 64), the card is that guide: a compiled-in or approved
`fleet.page/1` page of layout `guide` (Settings › Guides, `guide { list }`
over the control API). It is drawn by the same view as in Settings: the
same steps with Back, Next and Done, and its fields are the live settings,
with their own guards (a paired desktop that may not write them shows
them read-only). *Open in Settings* opens it there; Done folds the card.
The other keys are ignored: `{"spec": "fleet.ui/1", "kind": "guide",
"page": "guide.cleanup"}`. A page this fleet does not have says so and
links to Settings › Guides.

### `callout`

`tone`: `info` (default), `tip`, `success`, `warning` or `danger`.
`title` is optional (≤ 120). `body` is required Markdown (≤ 4000).

### `facts`

`title` is optional. `items` holds 1–40 `[label, value]` pairs. A value
may be a string, a number or a bool.

### `choices`

| Key | Required | Meaning |
|---|---|---|
| `title` | no | ≤ 120 chars |
| `question` | no | Markdown, ≤ 500 |
| `options` | yes | 1–8 of `{ label, prompt, hint? }` |

A button shows `label` (≤ 80) and `hint` (≤ 200). A click puts `prompt`
(≤ 4000) in the composer. Write `prompt` as the person's instruction to
you, for example `"Open a draft PR"`.

### `form`

`{ "kind": "form", "form": <fleet.form/1> }`. The form format is the one in
`docs/forms.md` (steps, field types, `when` conditions, defaults), with no
`secret` fields. Submitting fills the composer with:

````
Answers to the form "<title>":

```json
{ "env": "stg", "replicas": 2 }
```
````

Only the fields on visible steps are in it. The check in the app is
looser than `ask`'s: it makes sure the form can be drawn, and leaves
validating the answers to you.

### `report`

The task report's keys (`summary`, `outcome`, `tests_run`, `warnings`,
`blockers`, `followups`, `confidence`) plus an optional `title`, read the
way the backend reads a run's report. An unknown `outcome` is `partial`,
and each list holds at most 20 entries of at most 500 chars.

### `progress`: a long job, updated in place

| Key | Required | Meaning |
|---|---|---|
| `id` | yes | The job: letters, digits and `. _ : -`, ≤ 64 |
| `title` | yes | ≤ 120 chars |
| `state` | no | `running` (default), `waiting` (on the person), `done` or `failed` |
| `done` | no | Whole number ≥ 0 |
| `total` | no | Whole number ≥ 1, at least `done`; leave it out while the size is unknown |
| `unit` | no | ≤ 20 chars, after the count: `3 of 7 hosts` |
| `steps` | no | 1–20 of `{ title, state? }`; a step's `state` is `pending` (default), `running`, `done`, `failed` or `skipped` |
| `note` | no | Markdown, ≤ 2000 |

Write the block again with the same `id` each time the job moves on. In one
conversation the **first** card of an id shows the newest block, with
*N updates below*; every later block of that id draws as one line pointing
up to it. A `total` draws a meter; without one the card counts
(`120 rows so far`). Nothing animates: a job waiting on the person is
`waiting`, never a spinner.

```json
{ "spec": "fleet.ui/1", "kind": "progress", "id": "deploy-42",
  "title": "Deploying to staging", "done": 3, "total": 7, "unit": "hosts",
  "steps": [ { "title": "Build", "state": "done" }, { "title": "Push", "state": "running" }, { "title": "Restart" } ] }
```

### `results`: numbers, a chart, a table

| Key | Required | Meaning |
|---|---|---|
| `title` | no | ≤ 120 chars |
| `summary` | no | Markdown, ≤ 2000 |
| `items` | yes | 1–12 items, each with a `type` |

The items are the page widgets, fed from the block instead of a data source:

- `{ "type": "stat", "label", "value", "ty"?, "hint"? }`: one number (or
  text ≤ 80) in large type; `hint` (≤ 200) goes after it.
- `{ "type": "chart", "chart", "title", "x", "y", "points" }`: one series;
  `chart` is `line`, `bar` or `sparkline`; `x` and `y` are
  `{ "label", "ty"? }`; `points` holds 1–200 `[x, number]` pairs, `x` text
  or a number. *Table* shows the same numbers as text.
- `{ "type": "table", "title"?, "columns", "rows" }`: 1–12 columns of
  `{ "label", "ty"? }`, up to 200 rows of one cell per column (text, a
  number, a bool or null).

`ty` is a page column type and formats the value the way a page does:
`text`, `int` (`12,500`), `tokens` (`1.2M`), `usd_micros` (`$1.83` from
1830000), `day` or `time` (seconds since the epoch, shown as `5 min ago`).
A stat or a numeric cell without one is an `int`.

### `error`: what failed and what next

| Key | Required | Meaning |
|---|---|---|
| `code` | yes | Letters, digits and `. _ : -`, ≤ 64: `E_SSH`, `build.failed` |
| `title` | yes | ≤ 120 chars, what failed in plain words |
| `body` | no | Markdown, ≤ 4000 |
| `detail` | no | A log excerpt, ≤ 8000, drawn as code under *Details* |
| `next` | no | 1–4 of `{ label, prompt, hint? }`, as `choices` |

A next step fills the composer, as a choice does; the person presses Enter.

### `setting`: a settings change to apply

| Key | Required | Meaning |
|---|---|---|
| `proposal` | yes | The id `set_setting` with `propose: true` answered (a whole number ≥ 1) |
| `note` | no | ≤ 500 chars, what the change does in plain words |

Propose first, over the control API: `set_setting { key, value, why,
propose: true }`. It validates the value and answers the proposal with its
`id`. Then write the block with that id. The card does not take the key or
the value from the block: it reads both, and the value now, from the
proposal, so the person always sees what Apply would write.

Apply asks first, with the setting's own warning when it has one, then
applies that one proposal as Settings › Review does
(`decide_setting_proposals`, which only the desktop or a trusted device may
call). *Not now* writes nothing: the proposal still waits in Settings ›
Review. After Apply, *Undo in Settings* opens the setting where it lives.
A proposal that was already decided shows as such; a device that may not
change settings shows the change with no Apply.

### `wizard`: one of the app's wizards

| Key | Required | Meaning |
|---|---|---|
| `wizard` | yes | `add_host`, `add_project`, `get_started`, `new_session` or `pair_device` |
| `why` | no | ≤ 500 chars, why you open it, in one sentence |

`{"spec": "fleet.ui/1", "kind": "wizard", "wizard": "add_project", "why":
"You asked for the receipts repo on mercury."}` opens the app's own Add
project form at that point of the reply (`docs/forms.md` → *Wizards are
forms too*). The spec is the app's (`src/lib/forms/wizards/<id>.json`),
never yours: the block only names it. The card builds while it reads what
it offers (your hosts, your SSH config), then the person fills it in;
nothing runs until they press its last button, which does what the
wizard's own screen does (adds the project, starts the session). The card
then shrinks to one line, and that line goes into your session's composer,
unsent: `Done in the "Add project" form: acme/pos on mercury`, or the
person's note when they declined. On an earlier conversation, or a row with
no composer on screen, the card is a line saying it opens in the live
conversation.

## Checking a block

`docs/chat-block.schema.json` is the JSON Schema, generated from the Rust
model (`REGEN_FORM_DOCS=1 cargo fleet-test -- form_docs_are_current`), for
editors, LLM structured output and fleet-mobile. The app's check is
`checkUiBlock` in `src/lib/rich_blocks.ts`. Its Rust twin,
`crates/fleet-core/src/pages/chat_blocks.rs`, is the model the schema is
generated from and the same check, meant for blocks fleet writes or relays
itself; no production path calls it yet. Both run
`docs/chat-block-examples/blocks.json` and report the same problems, in
the same words and order: `item 1 › point 3: must be [x, number]`. At
most 20 problems are reported.

## Where it lives

`src/lib/rich_blocks.ts` splits a text block and checks a block. It also
draws a work handover (the text between `WORK_HANDOVER_BEGIN_<nonce>` and
`WORK_HANDOVER_END_<nonce>`, which fleet asks a session for) as a card;
`src/lib/handover.ts` reads its sections. That one is not a `fleet.ui/1`
block: it is prose, and a section the parser cannot name stays Markdown.
`src/lib/RichText.svelte` draws the segments, with the cards in
`src/lib/rich/`; `rich/progress_board.ts` pairs the progress cards of one
id. The transcript already carries the text, so the hub contract is
unchanged. A phone or another client that does not know a block shows its
fenced JSON.
