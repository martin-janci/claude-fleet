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
- If the agent is aborted while it waits, its form stays pending: it expires
  after 24 h, or is cancelled when the session is killed.

### Drafting a long form

    ask { draft: "<the form's JSON so far>", why: "what you are reading" }

Before a long form is whole, the agent may show it building: each `draft`
call replaces the session's draft (at most 16 KiB, not validated, migration
153), the session row carries it as `form_draft`, and the Conversation panel
draws it in (`ChatForm` in its Building state: a small Atom, "Building a form
· reading …", the title and a skeleton for each field once its name, type
and label are whole; `partial_spec.ts`). It answers at once,
`{status: "drafting", bytes}`. The `ask { form }` that follows replaces the
draft with the form card; `ask { draft: "" }` drops it; the tick drops one
not written to for 10 minutes. A draft is refused (`E_CONFLICT`) while the
session already waits on a form. Same rules as `form`: only from inside the
asking session.

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

### Top level

| Key | Required | Meaning |
|---|---|---|
| `spec` | yes | `"fleet.form/1"` |
| `title` | yes | ≤ 120 chars |
| `intro` | no | ≤ 500 chars |
| `submit` | no | The last step's button; default "Submit" |
| `save_later` | no | Offer "Save and finish later" (hub contract 15) |
| `steps` | yes | 1–12 steps |

A step has `title` (required, unique within the form), `name` (≤ 24
chars, the word on its step chip; default the title), `intro`, `when`
and `fields` (≥ 1). One step is a plain form; the wizard chrome (step
chips, Back / Next, *Step 2 of 4*) appears from two visible steps on.

A step with `"kind": "review"` has no fields: it summarises every visible
step before it, each with an **Edit** link back to that step (a secret
reads "set, never shown to the agent", never its value). It is the last
step and never the first.

### Fields

Every field has `name` (`[a-z][a-z0-9_]*`, ≤ 40 chars, unique across the
**whole** form), `type`, `label` (≤ 200 chars), and optionally `help`
(≤ 500), `required` (default false), `value` (the default) and `when`.

| `type` | Extra keys | Answer value |
|---|---|---|
| `text` | `placeholder`, `max_len` (≤ 2000, default 500) | string |
| `textarea` | `placeholder`, `max_len` (≤ 20000, default 5000) | string |
| `number` | `min`, `max`, `integer` (bool) | number |
| `bool` | — | bool |
| `select` | `options`: 1–50, see below; `other` (bool) | string, one of the values (any text with `other`) |
| `multiselect` | `options` as above | array of values, in option order |
| `secret` | `secret_note` (≤ 500); `value` refused | written to the host; see Secrets below |

An option is the pair `[value, label]` or an object
`{ "value", "label", "detail"?, "proposed"?: { "by", "reason" } }`; the
two shapes mix in one list. `detail` (≤ 200) is a line under the label
("2 idle"). `proposed` marks the likely choice of a `select` (at most one
per field; `by` is `rule`, `jev` or `llm`): it is shown first and chosen
while the field is empty, with "Proposed by Jev · reason · Change", unless
the field asks something AI never decides or the option is risky.

`other: true` on a `select` adds **Another…**, a free entry: the answer may
then be any text up to 500 characters.

Any field but a secret may carry:

- `disabled_reason` (≤ 500): the field is shown, greyed, with the reason
  under it, and never answered (its value is dropped like a hidden one's).
  A disabled field cannot be `required`.
- `drafted: { "by", "from" }`: its `value` was drafted by an AI ("haiku on
  mercury", "the Jira epic PD-3012"). It needs a `value`, and shows the
  Drafted label until the person changes it.

With `save_later`, **Save and finish later** folds the card to one line
with Resume; what was typed (never a secret, never a disabled field) is
kept on that device until the form is answered or declined. The app's
own wizards take the same key (Get started does): in the chat the card folds
the same way, and in a dialog the dialog closes without asking to discard,
opening next time from what was kept. A kept choice the form no longer
offers (a host since removed) is dropped when it opens. The hub's
`form_drafts` table (migration 153) is the agent's spec while it is being
written, purged after 10 minutes, so it does not hold answers.

A `required` bool means "must be on" (a consent box). A `required`
multiselect needs at least one value. A `value` must be valid for its
field (an option value, inside min/max, …).
The agent never decides for the person: on the desktop and the phone a
`required` bool starts unticked whatever its `value`, and a bool, or a
select or multiselect option, whose label names a risky step (the quick-answer
`RISKY_WORDS`: push, approve, allow, deploy, production, …) does not start
chosen.

### Conditions

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

### Limits and validation

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
(0600); the result's `secrets` maps the field to that path. A secret is at
most 2000 characters. The control API's audit row for an `ask { answer,
values }` call records only the number of fields (`values=<N fields>`),
never a name or a value; the MCP transport's own debug-level request logging
(rmcp, off by default) would show call arguments, as it would for every
other secret-carrying call, so leave it off. Read it, use
it, delete it: the agent cleans up its own files. Fleet removes the
directory too, once the session is a ghost or deleted, or a week after the
answer.

The answer records, durably and before the first file is written, that
secrets may be on the host (`secrets_on_host`). The tick sweep removes the
directory of every such form whose session is a ghost or deleted, or whose
answer is a week old, and retries a failed removal on a later tick. A host
that does not answer is skipped for 10 minutes after its first failure, then
20, 40 and so on up to 6 hours (in memory: a restart tries every host once
more), so an unreachable host does not cost a timeout on every tick; a form
whose host has been removed is marked swept, there being nothing to reach.
Per pass at most 20 directories are removed, so a slow fleet cannot stretch
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

## Wizards are forms too

The app's own wizards are the same format (redesign step 10.12): each one
is a fleet.form/1 spec in `src/lib/forms/wizards/<id>.json`, listed in
`src/lib/forms/wizards.ts` with its "Sending…" words and its loader. The one
spec renders two ways:

- **As a dialog**, `WizardDialog.svelte`: the spec's title and intro, the
  steps, Cancel. Settings › Hub's *Link to a hub…* is `link_hub`;
  Settings › Devices' *Pair a device* is `pair_device`, and Get started's
  *Start your first session* is `get_started`.
- **In the chat**, `ChatForm.svelte`, as a card at the end of the
  conversation (in a session's Conversation panel and in Control's chat,
  which is the same panel). While the spec is still being written it draws
  in (a small Atom, what is being read, each field once its name, type and
  label are whole; `partial_spec.ts`); once whole it is the wizard. Three
  ways put one there:
  - An agent (Control's operator or any session) writes a `wizard` block,
    `{"spec": "fleet.ui/1", "kind": "wizard", "wizard": "add_project",
    "why": "…"}` (`docs/chat-blocks.md`). The card is the app's spec, never
    the block's; once it ran (or was declined) a line saying so goes into
    the session's composer, unsent, for the agent.
  - The app opens one (`forms/chat_wizards.ts`): Control's *Add project*
    chip puts the Add project form at the end of Control's chat.
  - An agent's own `ask { draft }` then `ask { form }` (above): the draft
    draws in, the form card replaces it.

  `WizardChatCard.svelte` shows the Building state while it reads the
  choices only known now (your hosts, your SSH config, your orgs), then the
  wizard. The wizards a chat can run are `CHAT_WIZARD_IDS`
  (`forms/chat_wizard_ids.ts`, the same list as `CHAT_WIZARDS` in
  `pages/chat_blocks.rs`): `add_host`, `add_project`, `get_started`,
  `new_session`, `pair_device`. `link_hub` stays in Settings › Hub.

Choices only known when a wizard opens (the fleet's hosts, an owner's
repositories) replace the file's example options through `withChoices`; a
choice left with none leaves the form with the option that led to it.
`add_project` runs in the chat only: 6.11's dialog keeps its GitHub browser,
which a form cannot hold, so in the chat it has no *From GitHub* (no list of
an owner's repositories to offer). Its run (`add_project_wizard.ts`) adds
each repository in turn, stops at the first failure and says what was added,
and never creates a repository on GitHub (that needs the dialog's
confirmation).

Either way nothing runs until the last step's button is pressed: in the
chat the button carries a Comet while it runs, the card then shrinks to one
line, and a Pulse says what is starting. Never a modal over the chat or a
full-screen loader. What the button does belongs to the screen that opens
the wizard (`chat_wizard_runs.ts` for the chat); the spec is data only.
fleet-core's `every_wizard_spec_is_valid` validates every file there, and
`every_chat_wizard_has_a_spec_and_the_app_lists_the_same` the chat's list.

`add_host` is the guided Add host wizard (4.9) in one step, for the chat;
the Hosts page keeps 4.9's drafts and per-step checks.
Its run (`add_host_wizard.ts`) offers the hosts in `~/.ssh/config`, runs
4.9's checks in order (Sonar while each waits), refuses a host SSH cannot
reach, then adds it. `pair_device` (`pair_device_wizard.ts`) offers the
hub's orgs and calls `pair_device`; on the Devices page its code and QR are
PairingResult's, with its Halo; in the chat its answered line carries the
code and the link.

`new_session` is ⌘N's start as a form: project, host and agent, then the
worktree (a new one with its branch and base, or one the opener offers),
then the label and, for Claude Code, model, effort and login profile (a
shell asks what to run instead). Its run (`new_session_wizard.ts`) makes
the same `new_session` call the dialog makes, so the answered card carries
the Pulse while the agent comes up. ⌘N keeps its dialog, which holds what a
form cannot hold: the duplicate check, the account headroom ask, a ticket
start and Cancel creation.

`get_started` is 10.5's Get started as one form: a host the fleet has, a
project (one in the fleet, a repository to clone or a folder on the host)
and the agent. Its run (`get_started_wizard.ts`) adds the project unless one
in the fleet was picked, then starts the first session in a new worktree
with the same `new_session` call; the dialog shows the Galaxy while it
builds (10.10) and selects the session it started. A new machine joins from
Hosts, which checks it first; the form only picks one.

## When to use it instead of AskUserQuestion

When the person may not be at the terminal, or the input is several
fields or steps. A single yes/no in front of the terminal stays a
question.
