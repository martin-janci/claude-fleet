# Chat forms

An agent in a fleet session can ask the person a form instead of a
question in the terminal. The form appears as a card in the session's
Conversation panel on the desktop (and on a paired desktop or the phone
through the hub). The person fills it in step by step; the answers come
back as the result of the agent's tool call. The design is
`docs/superpowers/specs/2026-10-07-chat-forms-design.md`.

## Ask

    ask { form: <fleet.form/1>, why: "one sentence", timeout_s: 600 }

- Only from inside a fleet session: the per-host token and `X-Fleet-Pane`
  identify which session asks (see *What a token can see*); anything else is
  `E_NOT_A_SESSION`. A
  `readonly` token cannot call `ask` at all.
- One open form per session (`E_CONFLICT` names the open one).
- The call waits up to 600 s. `{status: "pending", form_id}` means the
  person has not answered yet: call `ask { wait: form_id }` again.
- `ask { cancel: form_id }` withdraws it. Only the asking session or the
  master token cancels.

Results: `answered` (with `answers`, `secrets`, `answered_by`), `pending`,
`declined` (with the person's `note`), `cancelled`, `expired` (24 h).

An `answered` result that carries secrets also says: "Delete each secret
file once you have used it." A decided form's row is kept 7 days, then
purged (once its secret directory is gone).

## The format

The JSON Schema is `docs/form-spec.schema.json`; the validator is
`crates/fleet-core/src/pages/forms.rs`.

```json
{
  "spec": "fleet.form/1",
  "title": "New project",
  "intro": "One plain sentence under the title.",
  "submit": "Create",
  "steps": [
    { "title": "Basics", "fields": [
      { "name": "name", "type": "text", "label": "Name", "required": true, "placeholder": "my-app" },
      { "name": "kind", "type": "select", "label": "Kind", "options": [["web", "Web app"], ["cli", "CLI"]] },
      { "name": "db", "type": "bool", "label": "Needs a database", "value": false }
    ] },
    { "title": "Database", "when": { "field": "db", "truthy": true }, "fields": [
      { "name": "engine", "type": "select", "label": "Engine", "options": [["pg", "Postgres"], ["sqlite", "SQLite"]] },
      { "name": "db_pass", "type": "secret", "label": "Password" }
    ] }
  ]
}
```

### 2.1 Top level

| Key | Required | Meaning |
|---|---|---|
| `spec` | yes | `"fleet.form/1"` |
| `title` | yes | ≤ 120 chars |
| `intro` | no | ≤ 500 chars |
| `submit` | no | The last step's button; default "Submit" |
| `steps` | yes | 1–12 steps |

A step has `title` (required, unique within the form), `intro`, `when`
and `fields` (≥ 1). One step is a plain form; the wizard chrome (Back /
Next, *Step 2 of 4*) appears from two visible steps on.

### 2.2 Fields

Every field has `name` (`[a-z][a-z0-9_]*`, ≤ 40 chars, unique across the
**whole** form), `type`, `label` (≤ 200 chars), and optionally `help`
(≤ 500), `required` (default false), `value` (the default) and `when`.

| `type` | Extra keys | Answer value |
|---|---|---|
| `text` | `placeholder`, `max_len` (≤ 2000, default 500) | string |
| `textarea` | `placeholder`, `max_len` (≤ 20000, default 5000) | string |
| `number` | `min`, `max`, `integer` (bool) | number |
| `bool` | — | bool |
| `select` | `options`: `[[value, label], …]`, 1–50 | string, one of the values |
| `multiselect` | `options` as above | array of values, in option order |
| `secret` | — (`value` refused) | written to the host; see Secrets below |

A `required` bool means "must be on" (a consent box). A `required`
multiselect needs at least one value. A `value` must be valid for its
field (an option value, inside min/max, …).

### 2.3 Conditions

`when` on a step or a field uses the page condition forms with `field` in
place of `key`:

- `{ "field": "<name>", "eq": <value> }`
- `{ "field": "<name>", "in": [ … ] }`
- `{ "field": "<name>", "truthy": true }`
- `{ "all": [ … ] }`, `{ "any": [ … ] }`, `{ "not": { … } }`

A condition names only a field that comes **earlier** (an earlier step,
or earlier in the same step), never a `secret`. `eq` / `in` values must
be values that field can take. A hidden step or field is not asked, not
validated and not in the answers.

### 2.4 Limits and validation

At most 12 steps, 40 fields, 50 options per select, 16 KiB of JSON.
Unknown keys are refused. The validator is `crates/fleet-core/src/pages/forms.rs`
and reports every problem with where it is, in the pages' style:
`step 2 › field 1 (engine): option value "pg" appears twice`.

`docs/form-spec.schema.json` is generated from the Rust model
(`REGEN_FORM_DOCS=1 cargo fleet-test -- form_docs_are_current`), for
editors, LLM structured output and fleet-mobile. `docs/form-examples/*.json`
holds valid and invalid examples with the expected problems; the Rust and
the TypeScript validators both run them (the Rust test reads them with
`repo_files::read`).

## Secrets

A `secret` field's value never reaches the agent or any log. It is written
to `~/.cache/claude-fleet/forms/<form_id>/<field>` on the session's host
(0600); the result's `secrets` maps the field to that path. Read it, use
it, delete it: the agent cleans up its own files. Fleet removes the
directory too, once the session is a ghost or deleted, or a week after the
answer.

The answer records, durably and before the first file is written, that
secrets may be on the host (`secrets_on_host`). The tick sweep removes the
directory of every such form whose session is a ghost or deleted, or whose
answer is a week old, and retries a failed removal on a later tick. Per pass it skips a
host after one failure (the rest of that host's forms wait for the next
pass) and removes at most 20 directories, so a slow fleet cannot stretch
the tick. A form with secrets outlives its session's deletion (a trigger
cancels it if it was pending, clears its answers and keeps the row under
the negated session id, so a recycled session id never inherits it); a
form without secrets goes with its session.

## Who answers

A person with `drive` on the session: the master token, the owner's
device, a `drive` grantee. Never a per-host token: an agent cannot fill a
form. On the desktop: the card in the Conversation panel; the commands are
`list_forms`, `get_form`, `answer_form`, `decline_form`, routed to the
hub's `ask` when paired.

One answer at a time: while one answer is being processed (the secrets are
being written to the host), a second answerer of the same form gets
`E_CONFLICT` "being answered right now"; it can look again once the first
has finished.

## What a token can see

Reading a form (`list`, `get`, `wait`) needs read reach on the form's
session, which is the same read gate as the rest of the control API. So a
per-host token on the same host can read another unclaimed session's forms:
the answers and the secret file **paths**, never the secret values (those
are only on the host's disk). And `X-Fleet-Pane` is a claim, not a proof: an
agent holding a host token can name another pane on its host, as everywhere
else in the control API. The file mode (0600, in the session's own home) is
the protection for the values, not the token.

## When to use it instead of AskUserQuestion

When the person may not be at the terminal, or the input is several
fields or steps. A single yes/no in front of the terminal stays a
question.
