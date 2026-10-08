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
  block: a tutorial, a guide, a callout, facts, choices, a form or a report.

A card is a view of the reply. Nothing is stored and nothing is sent from
one. A card that acts (choices, a form, a follow-up) only fills the
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
| read reference material in parts | `guide` |
| notice one thing (a risk, a tip) | `callout` |
| see a few labelled values at a glance | `facts` |
| pick what you do next | `choices` |
| fill in several values for your next turn | `form` |
| see a run's result | `report` (or the done marker) |

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

## Where it lives

`src/lib/rich_blocks.ts` splits a text block and checks a block.
`src/lib/RichText.svelte` draws the segments, with the cards in
`src/lib/rich/`. Both are frontend only: the transcript already carries
the text, so the hub contract and the backend are unchanged. A phone or
another client that does not know a block shows its fenced JSON.
