# Chat forms: an agent asks, a person fills a form, the answers come back

Date: 2026-10-07
Status: built (part 1), see docs/forms.md.
Repos: `claude-fleet` (this spec), `fleet-mobile` (its renderer, own plan)

This is part 1 of two. Part 2 puts the same form format into guide steps
(layout L9), where the answers start a session, go to a running session,
run a page action or feed later steps. Part 2 gets its own spec; this one
fixes the format it will reuse.

## 1. What this adds

A Claude session running in tmux on any host calls a new control-API tool,
`ask`, with a form. The form appears as a card in the desktop's
Conversation panel (and on the phone, through the hub). A person fills it
in, step by step, and the answers return to the agent **as the tool's
result**. A secret typed into the form never reaches the model: it is
written to a 0600 file on the session's host and the agent receives the
path.

Why not what exists:

- `AnswerPrompt.svelte` answers one AskUserQuestion menu scraped from the
  pane: one question, numbered choices, delivered as a key press. No text,
  no several fields, no steps.
- Flows (`pages/flows.rs`) are wizards with real fields, but compiled into
  Rust (`tracker.connect` is the only one) and local-only on a hub.
- Guides take only registered settings as fields (`validate.rs`, "is not a
  registered setting").
- Native MCP elicitation (`elicitation/create`) would draw in Claude Code's
  terminal, not in fleet; it has no steps, and our streamable HTTP
  transport is stateless (`with_stateful_mode(false)`), so the server
  cannot send a request to the client.

## 2. The form spec, `fleet.form/1`

Data only, like a page spec. No code, markup, styles or expressions.

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
| `secret` | — (`value` refused) | written to the host; see §5 |

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

## 3. The `ask` tool

One tool, both sides. Its description stays a short clause (the
definition budget test, `the_served_definition_budget_stays_bounded`,
raised to the measured figure); the prose lives in `docs/forms.md` and
the `claude-fleet-control` skill.

### 3.1 The agent's side

| Call | Does |
|---|---|
| `ask { form, why?, timeout_s? }` | Validates, stores a request for the **calling session**, waits |
| `ask { wait: form_id, timeout_s? }` | Waits again on a pending form |
| `ask { cancel: form_id }` | Withdraws it; the card closes |

- **The calling session.** Taken from the caller's proven session: a
  per-host token and an `X-Fleet-Pane` that matches a session row
  (`view_scope` → `proven_session`, `auth.rs`). Without that proof:
  `E_NOT_A_SESSION`. There is no `session_id` parameter; an agent cannot
  open a form in another session's chat.
- **Waiting.** `timeout_s` defaults to 600 and is capped at 600
  (`MAX_WAIT_SECS`); the tool's policy row is `LongPoll` (660 s call cap)
  and the wait takes a long-poll permit (8 per caller). The wait loops on
  the store's `form_notify()` with a 500 ms safety re-read and re-checks
  access on each wake, as `wait_for_reply` does.
- **One open form per session.** A second `ask` while one is pending
  answers `E_CONFLICT` with the open `form_id`.
- **Invalid spec.** `E_INVALID` with every problem (§2.4); nothing stored.

### 3.2 Results

```json
{ "status": "answered", "form_id": "f_7Kq2…",
  "answers": { "name": "my-app", "kind": "web", "db": true, "engine": "pg" },
  "secrets": { "db_pass": "/home/u/.cache/claude-fleet/forms/f_7Kq2…/db_pass" },
  "answered_by": "phone (device)",
  "note": "Delete each secret file once you have used it." }
```

| `status` | When |
|---|---|
| `answered` | As above. Hidden fields are absent; values are typed per §2.2 |
| `pending` | The wait ran out. The person can still answer; call `ask { wait }` |
| `declined` | The person pressed Decline; `note` holds their reason, if any |
| `cancelled` | The agent withdrew it |
| `expired` | Nobody answered within 24 h |

A finished form answers `ask { wait }` with its final result at once, any
number of times, until its row is deleted (7 days); then `E_NOTFOUND`.

### 3.3 The person's side

| Call | Does |
|---|---|
| `ask { list: { session_id?, state? } }` | Forms the caller may see, newest first |
| `ask { get: form_id }` | One form: spec, state, non-secret answers |
| `ask { answer: form_id, values }` | Submits; `values` keyed by field name |
| `ask { decline: form_id, note? }` | Declines |

- **Who may answer or decline:** a caller with `drive` reach on the
  session (the same gate `send_prompt`, and so the AnswerPrompt card,
  uses): the master token, or a person's device that owns the session or
  holds a `drive` grant. **A per-host token never may**, nor a `readonly`
  token: `E_FORBIDDEN`. This keeps any agent from filling its own or
  another session's form.
  `answered_by` is `<client> (device)` for a paired device, `you (desktop)`
  for a standalone desktop and `the control API` for the master token.
  A `readonly` token cannot call `ask` at all (it is not on the read-only
  allow-list), not even `list` or `get`.
- **Who may list / get:** whoever may read the session.
- **`answer` validates again** on the server: required, types, option
  values, min/max/integer, `max_len`, hidden fields dropped. Errors come
  back per field (`E_INVALID` with `{field, problem}` rows); the form stays
  pending. `values` holds every visible field the person filled; a missing
  optional field is absent from the answers.
- An `answer` or `decline` on a form that is no longer pending answers
  `E_CONFLICT` with its current state (it was answered on the phone,
  withdrawn, expired).

## 4. Storage and lifecycle

A new migration (number taken from `origin/main` when it is written;
`scripts/check-migration-numbers.sh` holds it) creates `form_requests`:

| Column | |
|---|---|
| `id` | integer key |
| `form_id` | unguessable text id (`f_` + 16 random base62), unique |
| `session_id`, `host_alias` | the asking session |
| `spec` | the validated spec, JSON |
| `why` | text, ≤ 500 |
| `state` | `pending` / `answered` / `declined` / `cancelled` / `expired` |
| `answers` | JSON, non-secret values and secret **paths** only |
| `note` | the decline reason |
| `answered_by` | the actor, in words |
| `secrets_on_host` | 1 while a secret directory for it may exist on the host (§5) |
| `created_at`, `decided_at` | unix seconds |

- A partial index keeps "one pending per session" cheap to check.
- The reconcile tick marks a pending form older than 24 h `expired` and
  deletes rows decided more than 7 days ago.
- A killed session's pending form becomes `cancelled` (`record_kill`).
  A deleted session row is handled by an `AFTER DELETE` trigger, not a
  cascade: a form with `secrets_on_host = 1` survives under the negated
  session id (`-id`, so a recycled id never inherits it), its answers and
  `why` cleared and a pending one cancelled, until the sweep has removed
  its directory (§5); every other form is deleted with the session.
- An unfinished draft lives only in the UI component; nothing is stored
  until `answer`.
- Every change goes through the store, wakes `form_notify()`, bumps the
  session's `row_version` and emits `session:updated` (§6).

## 5. Secrets on the host

On `answer`, before the row changes state:

1. Each visible `secret` value is written on the session's host with
   `provision::write_host_file_secret`, the helper that already writes the
   MCP token into `~/.claude.json`: a `umask 077` script creates the
   directory and an empty 0600 temp file, the value goes over
   `upload_file` (stdin into `cat` over ssh; an `Upload` frame keeping the
   0600 mode through fleet-agent), and a rename puts it in place. The
   value is never in argv or a command string. Path:
   `~/.cache/claude-fleet/forms/<form_id>/<field>`.
2. If any write fails, the answer is refused with that host's error
   (`E_HOST_WRITE` with the field), the form stays pending, nothing is
   stored, and the files written so far are removed best-effort.
3. On success the row stores the path; the value is in no table, log,
   event or audit line, and is dropped from memory after the write.

The reconcile tick removes a form's directory from its host once the form
no longer needs it: its session is gone (ghost or deleted) or it was
decided more than 7 days ago. A row with secrets on a host carries
`secrets_on_host = 1` until that removal succeeds, so an unreachable host
is retried on a later tick. The retry is per host and in memory: after a
failure a host is skipped for 10 minutes, doubling per consecutive failure
up to 6 hours, so a down host does not cost its timeout on every tick; a
form whose host row is gone is marked swept at once (nothing to reach). The
result's `note` asks the agent to delete each file after use.

## 6. Events, rows and attention

- **No new event kind.** The session row gains
  `pending_form: {form_id, title} | null` (`#[serde(default)]`), read by a
  subselect on `form_requests` in `SESSION_COLUMNS`. Every form change
  bumps the session's `row_version` and emits `session:updated`, so the
  card opens and closes on the event every client already receives and
  fences. `list_sessions { view: "phone" }` keeps the field.
- **Contract.** The golden is regenerated (`REGEN_HUB_CONTRACT=1`). The
  new field alone would not move `CONTRACT_REVISION`, but the desktop now
  routes to a hub tool that did not exist before (`ask`), which the rule
  in `wire_contract.rs` lists as a bump: `CONTRACT_REVISION`,
  `MIN_HUB_CONTRACT` and `MAX_HUB_CONTRACT` move from 8 to 9 together, so
  this desktop and its hub ship in the same release.
- **Attention.** `needs_attention_with` answers `Reason::Waiting` for a
  session with a pending form, although its `claude_status` is `working`
  while the tool call runs. The form therefore shows in the waiting
  triage bucket, the sidebar and the phone's attention filter.

## 7. Hub and paired desktop

On a hub the pending call, the row and the notify live in the hub process
(the host's MCP entry points at `https://<domain>/mcp`). The desktop gets
four Tauri commands:

| Command | Standalone | Paired |
|---|---|---|
| `list_forms` | local service | `Routed { ask }` |
| `get_form` | local service | `Routed { ask }` |
| `answer_form` | local service | `Routed { ask }` |
| `decline_form` | local service | `Routed { ask }` |

Each gets its `verdicts.rs` row (then `REGEN_HUB_VERDICTS=1`), its
`tests_routing.rs` row with non-default args, `Serialize` on its args
struct, and a line in `control-api-reference.md` (`REGEN_DOCS=1`). A
device without `drive` on the session can list and get, not answer: the
card says so with the reason. The phone uses the same `ask` actions through its
hub client.

## 8. The desktop UI

New files under `src/lib/forms/`:

- **`form_model.ts`**, pure: evaluate `when`, the visible steps,
  *Step n of m*, coerce input to typed values, validate like the server.
  It runs the shared `docs/form-examples`.
- **`FormWizard.svelte`** draws one step's fields and Back / Next /
  Submit. Back keeps the values. Next and Submit stay disabled until the
  step's required fields are valid. A secret input is cleared once sent.
  It borrows FlowView's field markup; FlowView itself is not changed
  (part 2 may merge the two).
- **`FormCard.svelte`** is the chat frame: the asking session's name, the
  title, `why`, the wizard, a **Decline** button with an optional reason,
  and the server's per-field errors.

Placement:

- **Conversation panel:** the card at the bottom, where `AnswerPrompt`
  sits (`ConversationPanel.svelte`), while the session has
  `pending_form`. The two do not compete: while `ask` waits, the pane
  shows no dialog.
- **Transcript:** the tool line of `mcp__claude-fleet__ask` leads with the
  verb "Form". Its existing detail view shows the tool's result, which is
  the status and the answers; a secret appears only as its path.
- **Sidebar row:** a compact "Form waiting" chip (the row's own click
  opens the conversation); the session sits in the `waiting` triage
  bucket.
- **No write access** (no `drive` on the session, a hub offline): the card
  is read-only with the reason, through `writeBlocked` /
  `hubActionBlocked`.

Errors: per-field server errors next to their fields; a host write
failure at the top of the secret's step; a form finished elsewhere
closes the card when `session:updated` clears `pending_form`, with a line
saying what happened (from `get_form`); a lost hub keeps
the typed values in the component for a second submit.

## 9. The phone

This repo delivers the format (`docs/form-spec.schema.json`, the
examples), the `ask` actions on the hub and the `pending_form` row field.
fleet-mobile's renderer and its card are a separate plan in that repo,
shipped with `scripts/release-mobile.sh` after the claude-fleet release
that carries this.

## 10. Documentation

- `docs/forms.md`: the format, the limits, the lifecycle, secrets, who
  may answer.
- `skills/claude-fleet-control/SKILL.md`: a short section on when an
  agent should `ask` rather than use AskUserQuestion: the person may not
  be at the terminal, or the input has several fields or steps. And on
  re-waiting after `pending`.
- `docs/control-api.md`: the `ask` tool.
- `docs/status.md`: the feature's line.

## 11. Testing

Rust (`cargo fleet-test`):

- The validator: each rule in §2, with its location string; the shared
  examples.
- The lifecycle: `ask` then `answer` wakes the waiter with the typed
  answers; a timeout answers `pending`; `wait` after that; `cancel`;
  `decline` with a note; expiry by the tick; one pending per session
  (`E_CONFLICT`); a killed session cancels its form.
- Access: a per-host token's `answer` and the asking session's own
  `answer` are `E_FORBIDDEN`; a caller with no proven session gets
  `E_NOT_A_SESSION` from `ask { form }`.
- Values: required, types, options, bounds, hidden fields dropped.
- Secrets: through a fake `SshExec`, the value arrives on stdin, never in
  argv or the script; the row, the event and the audit hold the path, not
  the value; a failed write leaves the form pending.
- Events and attention: a form change emits `session:updated` with the
  new `pending_form` and a higher `row_version`; `needs_attention` is
  `Waiting`.
- Routing and contract: `every_command_has_a_verdict`, the routing rows,
  the contract golden, the definition budget.

Vitest: `form_model` (conditions, visible steps, coercion, the shared
examples); `FormCard` (steps, Back keeps values, required blocks Next,
Decline with a reason, read-only when blocked, final state on an event);
`ConversationPanel` shows the card for a session with `pending_form`.

hub-e2e (`scripts/hub-e2e.sh`): one scenario. A session on an agent host
calls `ask`; a paired client answers through `answer_form`; the agent's
call returns the answers, and the secret's file exists on the host with
mode 0600.

## 12. Out of scope

- Forms in guide steps and their four outcomes (part 2).
- Syncing an unfinished draft between devices.
- A file-upload field.
- The fleet-mobile renderer (its own plan).
