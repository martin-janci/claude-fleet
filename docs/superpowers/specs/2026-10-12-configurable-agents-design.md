# Configurable agents: design

Status: steps 1 to 5 built, and step 6's GitHub sources, PR comments and
labels, and writes to other trackers, 2026-10-14 (the phone's part in
martin-janci/fleet-mobile); polling other trackers and the gallery,
2026-10-18; `pr_review`, 2026-10-20. Restating the built-in agents under
way (*Migration*), 2026-10-23.
Asked for by the owner after the built-in reviewer and support agent landed
(`2026-10-11-review-and-support-agents-design.md`, PRs
martin-janci/claude-fleet#871, martin-janci/fleet-mobile#217,
martin-janci/support-agent#1): make an agent something a person **configures**
rather than something fleet **compiles**. Its triggers, the MCP servers it
reads, its instructions, the shape of its answer and the writes it may
propose are all set in a form. Several instances of one agent run in parallel,
each with its own variables and its own **stores**.

## The problem

The runtime from the previous spec (rules, jobs, actions, the locked-down
`claude -p`, the Inbox) is generic. The two agents on it are not:

- **The trigger is code.** The reviewer polls `gh pr list`. The support agent
  polls Jira for phrases and assignments, plus an optional Graylog scan. A
  third kind of agent means a new poller in Rust.
- **The answer is code.** `review/output.rs` and `support/output.rs` each
  validate one JSON schema.
- **The writes are code.** `post_review`, `jira_comment`,
  `jira_create_bug`, `kb_learn`, … each have their own payload, plan function
  and executor branch.
- **Stores are host-global.** A rule names MCP servers by install name, and
  `support/mod.rs` reads them from the host's `~/.claude.json`. Each name exists
  once per host, with one set of secrets. So two support rules on one host
  cannot read two Graylogs (prod and staging, or two customers' clusters), two
  knowledge bases, or two tenant registries. Fleet's own state is fixed tables
  (`review_prs`, `support_fingerprints`, `review_test_exemptions`), each owned
  by one agent.
- **Configuration is raw JSON.** `fleet-hub agents grant … --config '{…}'`,
  checked only by `SupportConfig::parse`. There is no form, so neither the
  phone nor a chat agent can set one up.

What the owner wants is to describe a new agent (an incident triager, a
release-notes writer, a dependency watcher, an L2 agent for a second
product) as data: pick its sources and stores in a form, and run several
instances side by side.

## What it does

- A **definition** (`kind: fleet_agent` in the asset catalog) declares an
  agent:
  - its **inputs**: typed variables, rendered as a fleet form;
  - its **store roles**: what kind of data source it needs, e.g. `logs`,
    `kb`, `tenants`, `state`;
  - its **trigger**: one of a fixed set of sources, configured by inputs;
  - the run: skill, read-only tools, the MCP servers from its store bindings,
    the answer's JSON schema;
  - its **actions**: what the answer may turn into, each with a condition, a
    template, an allow-list, and whether `act` runs it alone.
- An **instance** is a rule: one definition, the inputs' values, **store
  bindings** (role → a concrete store), a host, optionally a project, and a
  level. Two instances of one definition with different bindings run in
  parallel and share nothing but the definition.
- A **store** is either:
  - an **MCP store**: a catalog `mcp_server` with per-binding environment.
    Fleet renders it for each run with the binding's secrets and sends it on
    stdin. It is never installed into `~/.claude.json`, so a host can hold any
    number of them.
  - a **fleet store**: a namespaced record table in the hub's SQLite, private
    to one instance or shared by name between instances.
- **The model proposes, fleet acts** stays the rule. The run gets read-only
  tools and the *read* tools of its MCP stores. A write is always an action
  that fleet carries out:
  - a built-in write (a tracker comment, an issue, a link, a PR review);
  - `mcp_call`: one named write tool on one bound store, with arguments the
    model proposed and the definition's allow-list and schema checked;
  - `record`: a fleet-store write;
  - fleet's own writes, for agents that read the fleet itself: `work_task`,
    `fix_session`, `setting`, `session_recover` (see the action table).
- The reviewer and the support agent stay as they are. Each can later be
  restated as a definition (*Migration*). The generic runtime does not need
  them to move.

## A. Definitions

A definition is a catalog asset, so it is versioned in git, synced, scoped
(private or shared, S1b), edited in the Assets tab, and imported like a skill.
Directory `fleet-agents/<name>/`, with `asset.yaml` and `skill.md` as the body,
so the instructions live next to the definition. The kind is `fleet_agent`, not
`agent`: `agent` is already the catalog's Claude Code subagent.

```yaml
kind: fleet_agent
name: l2-support
description: L2 investigation of support tickets
version: "1"
inputs:
  - { name: jira_tracker, type: tracker, label: Jira, required: true }
  - { name: project, type: text, label: Jira project key, required: true }
  - { name: phrases, type: text, label: Trigger phrases, value: "Tech help!, /analyze" }
  - { name: bug_project, type: text, label: Bug project }
stores:
  - { role: logs, kind: mcp, label: Logs, required: true, read_tools: [search_logs] }
  - { role: tenants, kind: mcp, label: Tenant registry, read_tools: [resolve_tenant, lookup_client] }
  - { role: kb, kind: mcp, label: Knowledge base,
      read_tools: [search_known_errors, search_docs], write_tools: [add_known_error] }
  - { role: memory, kind: fleet, label: Fingerprints, shared_by_default: false }
trigger:
  source: tracker_comments          # see B
  tracker: "{{jira_tracker}}"
  query: 'project = "{{project}}"'
  match: { any_phrase: "{{phrases}}", not_from_bot: true }
run:
  workspace: project                # project | home
  tools: [Read, Grep, Glob]
  max_turns: 40
  timeout_secs: 900
  output_schema:                    # JSON Schema subset, see C
    type: object
    required: [rootCause, confidence, severity]
    properties:
      rootCause: { type: string }
      confidence: { enum: [high, medium, low] }
      severity: { enum: [critical, major, minor] }
      exceptionClass: { type: string }
      needsMoreInfo: { type: string }
actions:
  - id: ask
    when: { field: needsMoreInfo, truthy: true }
    do: tracker_comment
    target: "{{subject.key}}"
    body: "❓ {{answer.needsMoreInfo}}"
    auto: true
    stop: true                      # no further actions after this one
  - id: comment
    do: tracker_comment
    target: "{{subject.key}}"
    body_template: investigation    # a named template in skill.md's front matter
    auto: true
  - id: learn
    when: { all: [ { field: confidence, eq: high }, { field: exceptionClass, truthy: true } ] }
    do: mcp_call
    store: kb
    tool: add_known_error
    arguments: { exception: "{{answer.exceptionClass}}", rootCause: "{{answer.rootCause}}" }
  - id: remember
    when: { field: exceptionClass, truthy: true }
    do: record
    store: memory
    key: "{{answer.exceptionClass}}"
    value: { ticket: "{{subject.key}}", seen: "{{now}}" }
    auto: true
limits:
  max_body_chars: 5000
  write_projects: ["{{project}}", "{{bug_project}}"]
```

**Validation** (`fleet_agents/definition.rs`, PURE). The pages' style reports
every problem with where it is (`actions › learn: tool add_known_error is not a
write_tool of store kb`):

- Inputs are form fields (`docs/forms.md` → *Fields*) plus three
  fleet-reference types:
  - `tracker`: a configured tracker, rendered as a `select` of the
    trackers;
  - `project`: a project;
  - `host`: a host.

  `secret` is refused: secrets belong to store bindings, never to inputs.
- Templates use one syntax, `{{path}}`, over four roots: `inputs.*`,
  `subject.*`, `answer.*` and `now`. A path is checked against what exists:
  inputs by name, `answer.*` against `output_schema`, `subject.*` against
  the trigger source's subject shape. Templates have no expressions, loops or
  filters.
- Conditions are the form conditions (`field` / `eq` / `in` / `truthy` /
  `all` / `any` / `not`) over the answer's fields. No other language.
- Every `mcp_call` names a `store` role of `kind: mcp` and a tool in that
  role's `write_tools`. Every read the run may make is in `read_tools`; both
  lists are explicit, never `*`.
- Limits: at most 30 inputs, 8 stores, 12 actions, 64 KiB of YAML;
  `max_turns` and `timeout_secs` stay within the vehicle's bounds.

## B. Triggers

Trigger sources are a **fixed set in Rust**. A definition picks one and
configures it; it never supplies code.

| `source` | polls | subject | job key |
|---|---|---|---|
| `tracker_comments` | issues updated since the cursor, through `TrackerProvider::issue_threads` | `{key, summary, description, status, assignee, url}` + the comment | `<tracker>:<KEY>#c<id>` |
| `tracker_assigned` | the same, assignee = `match.assignee` | issue | `<tracker>:<KEY>#assigned` |
| `tracker_query` | a query's results that are new since the cursor | issue | `<tracker>:<KEY>` |
| `github_prs` | open PRs, new heads (the reviewer's `plan.rs` gating) | `{repo, number, title, head_sha, …}` | `gh:o/r#N@sha` |
| `github_comments` | issue/PR comments matching `match` | comment + PR | `gh:o/r#N@c<id>` |
| `github_review_replies` | a person's reply in a fleet agent's review thread | thread + PR | `gh:o/r#N@t<root>:c<id>` |
| `mcp_scan` | a read tool of a bound MCP store, every `every_mins` (the support scanner, generalised) | one entry of the answer, keyed by `key_template` | `scan:<hash of key>` |
| `schedule` | a cron expression (routines' parser) | `{at}` | `cron:<rule>:<slot>` |
| `manual` | nothing: a person or a chat agent starts it with inputs | the inputs | `manual:<uuid>` |

`issue_threads` is Jira's. On GitHub, Linear or Asana `tracker_comments` and
`tracker_assigned` read the hub's synced copy of the tracker instead (built
2026-10-18, see *Steps*), so `query`, which is JQL, is optional on these two
sources and refused on a tracker that is not Jira.

The **loop guard** is generic: a comment the instance's own writes made
(fleet records the remote id of every write it sends) or a comment carrying
the instance's marker never triggers. This replaces the support agent's
`marker` and `bot_account_id` checks.

## C. The run and its answer

The run vehicle is the current one (`service/agents/vehicle.rs`), unchanged:
`claude -p --output-format json`, `--tools` from `run.tools` (read-only only:
the validator refuses `Bash`, `Edit`, `Write` and every tool in
`NEVER_TOOLS`), and the MCP config and system prompt on stdin. Only the MCP
config is assembled differently (*D. Stores*). `--allowedTools` lists
`mcp__<binding>__<tool>` for exactly the bound stores' `read_tools`.

The prompt is the skill (`skill.md`) as the system prompt, plus a user message
the runtime builds:

- the subject as JSON, labelled as data and never as instructions (as both
  agents do now);
- the inputs' values;
- what the bound stores are (role, label, tools);
- the output schema, with "answer with ONLY a JSON object matching it".

**The answer** is read in the three tiers both agents use (whole text,
```` ```json ```` fence, the object ending at the last `}`) and validated
against `output_schema`. The schema is a subset: `object` / `string` /
`number` / `integer` / `boolean` / `array` / `enum` / `required` /
`maxLength`. An answer that does not validate fails its job (Permanent), as
today. The validated answer is the job's `result`.

## D. Stores

### Store bindings

A rule binds each role of its definition to a store. A role is either an
`mcp` store or a `fleet` store.

**MCP store binding.** The binding names:
- a catalog `mcp_server` asset;
- a binding name, unique per rule, e.g. `logs-prod`;
- env overrides that map the server's `${NAME}` placeholders to secret names.

```json
{ "role": "logs", "server": "graylog", "name": "logs-prod",
  "env": { "GRAYLOG_BASE_URL": "${GRAYLOG_PROD_URL}", "GRAYLOG_API_TOKEN": "${GRAYLOG_PROD_TOKEN}" } }
```

For each run the hub renders the binding:
1. It takes the asset's command, args and env.
2. It applies the overrides.
3. It substitutes the secrets with the catalog's own resolver
   (`catalog/sync/secrets.rs::resolve`: host override, then global, then
   built-ins).
4. It writes the result into the run's MCP config under the binding's name.

That config goes on stdin into the run's private temp dir, as the support
agent's does today. The same server asset can therefore be bound any number of
times, on one host and in parallel: `logs-prod`, `logs-staging`,
`logs-customer-b`. Nothing is installed in the host's `~/.claude.json`, so
nothing there drifts, and a revoked rule leaves nothing behind.

The command must exist on the host. The asset says how it is installed (today
`node ${SUPPORT_AGENT_DIR}/dist/…`). A grant checks the binding:
1. render it;
2. run `initialize` and `tools/list` over stdio, with the scanner's call
   script, `support/scan.rs::call_script`;
3. confirm the role's `read_tools` and `write_tools` are all listed.

A store that is not there fails the grant with the reason, not the first job.

**Fleet store binding.** The binding names:
- a namespace: by default `rule:<id>`, private to the rule;
- or a shared name such as `l2-fingerprints`. Two instances binding the same
  shared name read and write the same records, e.g. two support rules sharing
  one fingerprint memory.

```json
{ "role": "memory", "namespace": "shared:l2-fingerprints" }
```

### The fleet store

There is one new table, `agent_records`, with:
- the key `(namespace, kind, key)`;
- `value` (JSON, ≤ 64 KiB);
- `created_at`, `updated_at`;
- `expires_at`, optional;
- `written_by_job`.

The run reads it two ways:
- **Before the run**, the definition's `run.context` can name lookups whose
  results go into the prompt, e.g.
  `{ store: memory, key: "{{subject.key}}" }` or
  `{ store: memory, recent: 20 }`.
- **During the run**, a built-in MCP server `fleet-records`, read-only, is
  scoped to the rule's fleet bindings. It offers `get` and `search` over its
  namespaces only, and the hub serves it through its existing control API with
  a run-scoped token.

Writes are `record` actions only. A namespace a rule does not bind is
invisible to it.

The reviewer's and support agent's tables stay. If they move (*Migration*),
`support_fingerprints` becomes a `memory` namespace.

**Paralelism and isolation.** Instances share nothing but what they bind:
- Jobs are keyed per rule.
- Cursors are per rule.
- MCP bindings are rendered per run with their own secrets.
- Fleet namespaces are private unless explicitly shared.

The existing runtime keeps one running job per subject and global concurrency
(`agents.max_concurrent`). A definition may lower the concurrency for its own
instances with `run.max_parallel`.

## E. Actions

An action turns the validated answer into one `agent_actions` row, queued the
way the reviewer's and the support agent's are: kept at `watch`, waiting for a
person at `propose`, run at `act` when `auto` is true.

| `do` | payload | executor |
|---|---|---|
| `tracker_comment` | `{tracker, key, body}` (ADF / wiki rendered as now) | `WriteOp::RichComment` |
| `tracker_issue` | `{tracker, project, type, summary, body, labels, link?}` | `WriteOp::CreateIssue` (+ `LinkIssues`) |
| `tracker_link` | `{tracker, from, to, type}` | `WriteOp::LinkIssues` |
| `pr_review` | `{repo, number, sha, event, body, findings[]}` | the reviewer's `gh` calls: the diff, then the review (`ReviewRequest`) |
| `pr_comment`, `pr_labels` | as the reviewer's | gh |
| `pr_description` | `{repo, number, sha, body, overwrite, first}` | gh, only onto an empty description unless `overwrite` and `first` |
| `pr_reply` | `{repo, number, root_id, body, resolve}` | gh: a reply in the thread, then resolving it with `resolve` |
| `mcp_call` | `{binding, tool, arguments}` | calls the tool over stdio on the host with the binding rendered, as the scanner does |
| `record` | `{namespace, key, value, expires_at?}` | `agent_records` upsert |
| `work_task` | `{title, notes, steps[]}` from `summary`, `body`, `steps` | a Work task, its steps proposed as subtasks (`fleet_ops::work_task`) |
| `fix_session` | `{project, host?, title, prompt}` from `project`, `host`, `summary`, `body` | a Work task and a Claude session started on it (`start_work`) |
| `setting` | `{key, value, was, why}` from `key`, `value`, `body` | a settings-review proposal, applied as the person's (`settings_review`) |
| `session_recover` | `{session, host?, why}` from `target`, `host`, `body` | a lost session restored, a live one restarted |

The four fleet-own writes (`work_task`, `fix_session`, `setting`,
`session_recover`) and `tracker_issue` also take `fingerprint` (a template):
what the finding is, kept on the payload so the `fleet-diagnostics`
`proposals` tool can tell a later run it was proposed, or rejected, before.
Only `work_task` may be `auto`; `fix_session`, `setting` and
`session_recover` **never** run without a person, and a definition that
says `auto: true` on one is refused. A `setting` is planned only for a key
an agent may propose (`ai` not `never`, not owned by another subsystem) and
a value the key accepts; queued, it records the value it had (`was`), and
one already at that value is skipped.

**`each`.** An action with `each: <array field of the answer>` is planned
once per item (at most `MAX_EACH`, 10), its templates reading the item as
`{{item.…}}` and its `when` tested on the item. The self debug agent's
`findings` and the proposer's `proposals` use it.

**`run.diagnostics: true`** gives the run `fleet-diagnostics`, the hub's
read-only MCP server over the fleet's own logs, error reports, runs, host
health, sessions, the agents' analyses and their proposals. Every answer is
masked with every secret the store knows before the run sees it.

**Guardrails**, checked by the executor at send time (not only at plan
time):
- the rule still exists and is above `watch`;
- the target is inside `limits` (`write_projects`, repos, …);
- the text is within `max_body_chars`;
- for `mcp_call`:
  - the tool is in the bound role's `write_tools`;
  - the arguments validate against that tool's `inputSchema`, as `tools/list`
    returned it at grant time and stored with the rule;
  - the arguments are within 16 KiB.
- An `mcp_call` never runs at `act` unless the action says `auto: true` **and**
  the definition's store role marks the tool `auto_ok: true`. A person must
  have said, in the definition, that this write is safe to run unattended.

The **body** of every action, the text a person sees under Show before
approving, is what will be sent:
- the rendered template;
- for `mcp_call`, the tool name and its arguments as JSON.

**Templates for bodies.** A named template is a front-matter block of
`skill.md` with `{{…}}` paths and the same shortening rules the support
comment uses, so a template fits `max_body_chars` by dropping its longest
optional sections first.

## F. Configuring an instance

**Forms.** A definition's `inputs` and `stores` render as a fleet form
(`docs/forms.md`, `fleet.form/1`), generated by
`fleet_agents/form.rs::grant_form(definition)`:
- Step 1, *What*: the inputs.
- Step 2, *Where*: host and project.
- Step 3, *Stores*: each role is a `select` of compatible servers or
  namespaces. A new MCP binding's secrets are `secret` fields: their values go
  to the hub's secret store under the names the form shows, never into the
  rule.
- Step 4, *How much*: the level, and `expires`.

The same spec renders three ways:
- **As the Agents tab's *New instance* dialog** (`WizardDialog.svelte`).
- **On the phone.** fleet-mobile already renders forms.
- **In a chat.** An agent asked "put the L2 agent on project L2S with the
  staging Graylog" drafts the form with `ask { draft }` and fills what it can
  (`drafted`). The person sends it, and the answer becomes the grant.

An agent never grants anything itself (*G*).

**The Agents tab** lists definitions with their instances under them. Each
instance shows its bindings, its level, its newest jobs and its waiting
actions (the current rows, generalised). *Edit* reopens the form with the
instance's values. *Pause* and *Revoke* are as today.

**fleet-hub** gains `agents define` (validate a definition file) and
`agents grant <definition> --values <file.json>`. The latter is the form's
answer as a file, for scripts.

## G. Security

The configurable part is exactly where an agent could be talked into
something, so:

1. **Only a person grants.** A rule is created only from a form answered by a
   person with the owner's device or the master token, as `agents` is today
   (`ToolPolicy::PersonDevice`). A definition is catalog data and grants
   nothing by existing.
2. **No write tool in the run.** `run.tools` refuses everything but
   read-only tools. `--allowedTools` names only the bound stores'
   `read_tools`, and a `write_tools` entry is never offered to the model. The
   model can only *propose* a write by putting it in its answer, where an
   action template picks it up.
3. **Every write is checked when sent**, against the rule as it is then.
4. **Secrets** live in the hub's secret store and are rendered into a run's
   stdin. They never go into a rule, a form answer, a job's result or a log.
   A binding's env override can only reference secret names, never literal
   values.
5. **Definitions from a shared catalog** can be granted only if the catalog
   is the person's own or their org's (S1b scope). An imported definition
   shows its `write_tools` and `auto` actions in the grant form, in red.
6. **Data stays data.** The subject, store answers and fleet records reach the
   model inside JSON blocks labelled as data, as now.

## Migration

The built-in agents do not move in v1. Later, each one is restated as a
definition and deleted from Rust, step by step:

- **The support agent's read side** fits the definition above almost
  directly. Its trigger is `tracker_comments` + `tracker_assigned`. Its stores
  are `logs`, `tenants` and `kb` (MCP) plus `memory` (fleet, for
  fingerprints). The fingerprint and similarity steps stay in Rust as a
  built-in `fingerprint` helper a definition can call from `run.context`:
  hashing is not the model's job.
- **The reviewer** keeps its Rust-only parts as built-in helpers:
  - diff filtering and line placement;
  - verification of posted reviews;
  - the debate.

  A definition with `source: github_prs` and `do: pr_review` covers the rest.

The owner's decision (2026-10-23): the built-in agents stay while the
definitions are brought to parity and ship in the gallery beside them;
deleting the Rust agents and converting their rules and tables waits until
the definitions have run on real tickets and PRs. An unattended `pr_review`
may APPROVE only when the definition sets `limits.auto_approve`, as the
built-in reviewer does at `act`; it never sends REQUEST_CHANGES unattended,
which a later COMMENT from the agent could not lift.

The gaps, in the order they close (each its own PR):

1. **Support basics** (built 2026-10-23): `run.cooldown_secs` (a subject
   this instance finished less than that ago waits until it has passed,
   deferred rather than skipped so a PR head or comment is not lost; the support
   agent's `cooldown_secs`, per instance:
   `Store::last_agent_rule_subject_run`); `run.branch` (the project
   worktree's branch, a template over the inputs, checked with the
   workspace's `branch_ok`; default origin's default branch, its `HEAD`); `mcp_scan`'s `window_mins`
   (the tool gets `from` / `to`, unix seconds, over the last that many
   minutes, as the support scanner does) and `max_jobs` (1-10);
   `limits.auto_approve`. The `l2-support` example's `remember` now keys
   its record by the ticket, as its context lookup reads it.
2. **Fingerprints** (built 2026-10-24, `fleet_agents/fingerprints.rs`):
   `run.fingerprint` names a fleet store and, as templates over the
   answer, the error's `class`, `message`, `stack` and `component`, with
   `app_frame_prefix` over the inputs, `window_mins` (default 120) and an
   optional `bugs: {tracker?, project, issue_type?}`. It runs after the
   answer is checked and before the actions are planned, not from
   `run.context`: a comment ticket's error is known only once the model
   has read the logs. The support agent's own `fingerprint` (sha256 of the
   class, the message without numbers or ids, the first five app frames)
   is counted in a record of kind `fingerprint`, and five fields join the
   answer: `fingerprint`, `seenCount`, `knownBug` (the bug filed for it,
   while it was seen inside the window), and, with `bugs` and no known bug,
   `similarBug` / `relatedBug` (the open bug the support agent's JQL finds
   whose summary scores at least 80 / 60 on class, component and message).
   The validator adds the five to the schema the actions see and refuses
   an `output_schema` that has them. A `tracker_issue` whose payload's
   `fingerprint` is one (`fingerprint: "{{answer.fingerprint}}"`) keeps
   the bug it filed against it, so the next run of the same error links to
   it: incident aggregation. The gallery's `support` definition is the
   support agent's comment trigger restated: ask, comment, link to the
   known bug unattended, link to a similar one with a person, else file a
   high-confidence bug. Not restated yet: the assignment trigger and the
   log scanner (each a definition of its own, one trigger per definition)
   and the KB write-back (a git commit, not an MCP call).
3. **Reviewer inputs** (built 2026-10-25): `run.workspace: pr_head` (a
   worktree of the project at the job's pull request head, through the
   reviewer's `prepare`; the head moving on skips the run); `run.diff:
   {exclude_paths, respect_claudeignore (default on), max_lines (default
   3000, at most 20000)}`, the diff filtered with the reviewer's
   `exclude_patterns` / `filter_diff` and put in the prompt as data, a
   diff left empty or too long skipping the run (`empty_diff`,
   `diff_too_large`); `github_prs`'s `skip_wip` (the reviewer's `is_wip`)
   and `debounce_secs` (0-3600: a head's job is due that long after it is
   first seen). A `github_prs` job whose PR is no longer open at its head
   is skipped when it runs, so a burst of pushes is read once, at its last
   head. A `github_comments` comment on an open pull request carries its
   `head_sha`, `base_ref` and `head_ref`, so `/review` in a comment reads
   and reviews that head (a comment's job is not skipped when the head
   moves: it asks whatever the head).
4. **Reviewer answer** (built 2026-10-25): `pr_review`'s `min_confidence`
   (0-100: a finding whose `confidence` is below it is dropped, one without
   counts as 100) and a finding's `blocking`; a sent review is remembered
   in the rule's subjects namespace (kind `review`, key `owner/repo#N`:
   `{sha, event, findings[{path, line, blocking}]}`), and the next run's
   GitHub subject carries it as `previous`; `pr_review`'s `resolutions`
   (an answer array of `{path, line, resolution}`) adjusts the verdict as
   the reviewer's `adjust_verdict` does: REQUEST_CHANGES while a previous
   blocking finding is unresolved or `open`, a COMMENT with nothing
   blocking left an APPROVE, then the unattended rules of `review_event`.
   The subject carries `paths` (the files of the diff the run reads), and a
   `pr_labels` item may be `{label, paths: [globs]}`, a label only when a
   changed file matches (the reviewer's label rules). A `pr_description`
   action (`target`, `body` / `body_template`, `overwrite`) writes the
   pull request's description when it has none, or with `overwrite` on the
   instance's first review of it.
5. **Reviewer state** (built 2026-10-25): `github_prs`' `reverify_mins`
   (1-1440): each open PR's remembered review (its GitHub id kept with it)
   is looked up when due, at most 5 a poll, and one GitHub deleted or
   dismissed is a `reverify` job at the same head, keyed apart from the
   head's own. The `github_review_replies` source (`repo`, `match`,
   `max_rounds` 1-5, default 2): a job per person's reply in a review thread
   whose first comment is a fleet agent's (and carries `match` when set),
   the last word being a person's and this instance having answered fewer
   than `max_rounds` times; the subject carries the PR, `comment` and
   `thread` (`root_id`, `path`, `line`, `diff_hunk`, `finding`, `rounds`,
   `replies[{user, body, mine}]`, the markers stripped). The `pr_reply`
   action (`target` the thread's root, `body`, `resolve`) answers in the
   thread with the instance's marker, resolving it after. The gallery's
   `pr-reviewer` (the reviewer's skill, `pr_head`, the filtered diff,
   confidence 50, resolutions, the description) and `pr-test-debate`
   (concede or hold; a concession resolves the thread and is recorded in
   the `exemptions` store both bind shared, which the reviewer reads as
   context).

## Steps

1. **Definitions** (built): the `fleet_agent` catalog kind (load, render nothing on
   hosts, lint in the Assets tab), the PURE validator, and templates and
   conditions with their tests. `fleet-hub agents define`.
   `service/fleet_agents/`: `definition.rs` (the typed model, strict on
   unknown keys, and `problems`: inputs through the form validator, which
   keys each trigger source and action kind take, template paths against
   the inputs, the source's subject fields and `output_schema`, read-only
   run tools, `mcp_call` against the role's `write_tools` and `auto_ok`),
   `template.rs` (`{{path}}` parse and render), `answer.rs` (the schema
   subset, the answer check, conditions over the answer). The catalog's
   `Kind::FleetAgent` loads `fleet-agents/<name>/` (validated after its
   `skill.md` is read; an invalid one is a load problem) and every harness
   renders it as nothing. The Assets tab lists it as *Fleet agents* and can
   create one from a template; its definition is edited as YAML.
   `fleet-hub agents define <dir|asset.yaml>`. The example
   `docs/fleet-agent-examples/l2-support/` is loaded by the tests.
2. **Store bindings** (built): rendering an MCP binding per run (asset + env
   overrides + secrets), the grant-time `tools/list` check, migration
   `agent_records` and the `fleet-records` read server. Rules gain `definition`
   and `bindings`, and `agents grant` takes them.
   Migration 185 rebuilds `agent_rules` and `agent_jobs` (their `agent`
   CHECK admits `fleet`; rules gain `definition`, `bindings`) and adds
   `agent_records`. A fleet rule's `scope` is its instance name, its
   `config` the input values. `service/fleet_agents/`: `bindings.rs`
   (the binding model; `check`, PURE, against the definition and the
   catalog: every required role bound, kinds matching, unique binding names,
   an override only of a placeholder the server has and only ever
   `${SECRET}`; `render_entry` and `mcp_config` for a run: read tools only,
   plus `fleet-records`), `grant.rs` (the grant in lock order: the
   definition and servers out of the catalog, the host's secrets, `prepare`
   under the store lock, which fills input defaults and checks the values
   with the form validator and fleet's references, then `check_stores`
   with no lock held, then the write), `probe.rs` (`initialize` +
   `tools/list` on the host: a stdio server's script, secrets as `export`s,
   travels on stdin; an http one is asked with `curl -K`, headers in a
   config file), `records.rs` (`fleet-records`: `get` and `find` over the
   rule's stores, named by role). One change from *D*: the read server is
   not a control API tool. It is `/agent-records` beside `/pair`, outside
   `authorize`, and its only credential is a run token held in the hub's
   memory as a hash (`records::issue` / `revoke`), so no hub token and no
   change to the control API's callers is involved. The admin path
   (`admin::validate_grant`) refuses `agent: fleet`; the `agents` tool and
   `fleet-hub agents grant fleet` route it to `fleet_agents::grant`.
3. **Generic runner** (built): trigger sources `tracker_comments`, `tracker_assigned`,
   `mcp_scan`, `schedule`, `manual`; prompt assembly; answer validation
   against `output_schema`.
   `service/fleet_agents/runner.rs` is the agent runtime's `fleet` agent
   (`Registry::live`). Its poller reads a rule's instance from the catalog
   on every poll; the first poll only sets the clock (a JSON cursor:
   `since`, and the last `scan`). `tracker_comments` reads the issues the
   trigger's query finds updated since the last poll and makes a job per
   new comment carrying a phrase (comma separated, any case) or matching
   the regex on a line; `tracker_assigned` one per issue assigned to the
   account. `mcp_scan` calls the store's tool every `every_mins` (the
   rendered binding, the script on stdin as the grant's check:
   `probe::call_script`), takes the answer's list of entries and makes a
   job per entry whose `key_template` key is new. `schedule` fires the
   newest cron slot since the last poll (missed slots do not pile up).
   `manual` jobs come from a person: `agents { action: run, id, request? }`
   or `fleet-hub agents run <id> --request …` (a `schedule` rule can be run
   now the same way). Job keys start with `r<rule id>:`, so two instances
   never share a job. A scan entry, a schedule slot and a request are kept
   as the job's subject in the namespace `job:rule:<id>` of
   `agent_records`, which no binding can name. The runner reads an issue
   afresh (the asking comment by its id), runs the `run.context` lookups,
   renders the bindings with the host's secrets, issues a `fleet-records`
   token for the run and revokes it after, prepares the workspace (a
   project's checkout at `main`, or the home), and runs `claude -p` with
   the skill as its system prompt and only the read tools allowed. The
   prompt carries the subject and the context records as JSON data, the
   stores the run may read and the `output_schema`. The answer is the
   first JSON object of the reply that matches the schema, trimmed to it;
   none is a permanent failure that names the mismatch. The job's result is
   `{subject, answer}`. Not yet: `run.max_parallel` (the runtime's global
   concurrency applies), `tracker_query` and the GitHub sources (step 6),
   and every write: until step 4 a `propose` or `act` rule behaves as
   `watch`.
4. **Actions** (built): `tracker_*`, `record` and `mcp_call` with their guardrails;
   the generic loop guard.
   `service/fleet_agents/actions.rs`. `plan` (PURE) renders each action
   whose `when` holds over the answer into its payload: `tracker_comment`
   (`{action, tracker, key, body, adf}`; the body ends with the instance's
   marker line `[fleet-agent:<instance>]` and is cut to fit
   `max_body_chars`), `tracker_issue` (issue type `Task` unless set; an
   optional `link` to another issue), `tracker_link`, `record` (into the
   rule's namespace for the role) and `mcp_call` (`{binding, tool,
   arguments}`, its body the call as a person reads it). The tracker is the
   action's, else the trigger's, else the first `tracker` input's. A taken
   `stop` action ends the list; an action whose target, key or body renders
   empty is skipped with the reason. The runner queues them after a checked
   answer: kept in the job's result at `watch`, `pending` at `propose`,
   approved at once at `act` when `auto` (an `mcp_call` only when its store
   marks the tool `auto_ok`). The job's result is `{subject, answer,
   actions}` with what became of each. The `fleet` executor checks again
   when it sends: the rule still there, active and above `watch`; the
   definition still asking for that action; the tracker the instance's; an
   issue or link inside `limits.write_projects` (a comment or a link may
   also land on the job's own subject); the body within `max_body_chars`; a
   record into the rule's own namespace; an `mcp_call`'s tool among the
   role's `write_tools`, its arguments within 16 KiB and valid against the
   tool's `inputSchema`. One change from *E*: the schema is the one the
   server lists **when the action is sent** (a `tools/list` just before the
   `tools/call`), not one stored at grant time, so a server upgraded since
   the grant is checked against what it takes now. The check is the schema's
   object shape: required properties, `additionalProperties: false`, and
   each given property's declared type. A new issue is linked after it is
   created; a failed link says the issue was made. The loop guard: a
   tracker poll never turns a comment carrying a fleet agent's marker into
   a job, and the runner skips a subject that already ran five times in a
   day. The Inbox names the new kinds (`actionTitle`). Not yet: the
   shortening of a template by its longest optional sections (a body is
   cut at the end), and the `pr_*` actions (step 6).
5. **Forms and UI**: `grant_form`, *New instance* / *Edit* in Automation ›
   Agents, the phone's Inbox and instance list, the chat draft path.
   Built: `service/fleet_agents/form.rs`. `grant_form(definition,
   FormContext, rule?)` is PURE: the steps *What* (`f_instance` and the
   inputs; a `tracker`, `project` or `host` input is a select of what fleet
   has), *Where* (`f_host`, and `f_project` for a project workspace),
   *Stores* (per role `f_store_<role>`: a select of the catalog's MCP
   servers with an optional `f_env_<role>` textarea of `NAME=${SECRET}`
   lines, or for a fleet role `rule` / the shared namespaces in use / a
   typed `shared:<name>`; an optional role left empty is not bound) and *How
   much* (`f_level`, `f_hours`, under a line naming every write and which
   run unattended at act). With a rule it is the Edit form, prefilled.
   `answers_to_args` checks the answers against the same form
   (`forms::check_answers`) and returns the `agents` grant. The grant form
   asks for no secret value: an override names a secret of the hub's store,
   so nothing secret is ever in an answer (a change from *F*, which put new
   secrets in `secret` fields). Inputs may not be named `f_…`, and a role is
   at most 30 characters (`f_store_<role>` is a field name). The `agents`
   tool gains `definitions`, `form {definition, id?}` and a fleet `grant`
   with `answers`; the desktop command `agents_fleet` (routed to `agents`)
   carries definitions, form, grant, revoke and run. Automation › Agents
   lists the catalog's fleet agents (`automation/FleetAgents.svelte`), each
   with *New instance* (disabled with the problems when it does not load),
   its instances under it with *Edit*, *Revoke* and, for a manual or
   scheduled one, *Run now*; the form is a `WizardDialog`, and a store that
   fails the grant's check is the dialog's error. The chat path is a
   `wizard` block with `wizard: fleet_agent` and the `definition`
   (`docs/chat-blocks.md`), not `ask { draft }`: an agent drafts the
   answers (`values`), the person's device asks the hub for the form when
   the card opens and its last button grants from that device, so the
   answer becomes the grant without an agent ever holding the grant. The
   phone (fleet-mobile) lists the rules under Automation › Agents, a fleet
   agent's instance by its definition, with Revoke, and its Inbox names a
   fleet agent's actions; a new instance and Edit stay with the desktop's
   form or a chat card.
6. **Later**:
   - `github_*` sources and `pr_*` actions for generic agents;
   - `tracker_*` on GitHub, Linear and Asana;
   - restating the built-in agents as definitions (*Migration*);
   - a definition gallery.

   Built (`service/fleet_agents/github.rs`, `actions.rs`): `github_prs`
   makes a job per open, non-draft pull request at each new head (the key
   carries the head's sha); `github_comments` a job per new issue or PR
   comment carrying a phrase or matching the regex, never a bot's or a
   fleet agent's own (its marker). Both go through the PR reviewer's `gh`
   calls on the rule's host and keep the subject when they find it
   (`repo, number, title, head_sha, base_ref, head_ref, author, url`, and a
   comment's `comment`). `pr_comment` (`target` is the number) posts the
   body with the instance's marker; `pr_labels` takes `labels` with `-name`
   for a removal. Both are sent only to the trigger's repo or one of
   `limits.repos`. On a tracker that is not Jira (GitHub, Linear, Asana)
   `tracker_comment` sends the same text plain (`WriteOp::Comment`) and
   `tracker_issue` creates an item in the project (`WriteOp::CreateItem`);
   an issue link is Jira's, so a link, or an issue with one, is refused
   there. The built-in agents are restated as gallery definitions
   (*Migration* above).

   `pr_review`, built 2026-10-20: a review of the subject's head with line
   comments, on a `github_prs` trigger only and never with `each`. It takes
   `target` (the number), `body` or `body_template` (the review's text),
   `verdict` (a template: APPROVE, REQUEST_CHANGES or COMMENT, anything else
   COMMENT) and `findings`, an array field of the answer whose items have
   `path`, `line` and `body` (the validator checks the schema). Planning
   keeps the head the run read (`sha`), at most 30 findings, and the event;
   an `auto` review never approves unless the definition sets
   `limits.auto_approve`, and never requests changes (*Migration*). Sending checks the repo as `pr_comment` does, expires the
   action when the PR is no longer open or its head moved on, fetches the
   diff and places each finding on it with the PR reviewer's
   `commentable_lines` / `snap_line` (within three lines); a finding that
   does not land is listed in the body under *Outside the diff*. The body
   and each line comment end with the instance's marker. An APPROVE or
   REQUEST_CHANGES GitHub refuses on one's own PR is sent as COMMENT, as
   the reviewer does. The reviewer's confidence filter, its verdict
   adjustment against the previous review, the debate and the re-review of
   a dismissed review came later (*Migration* steps 4 and 5): fleet keeps
   the instance's last review in its own records, never the reviewer's
   `review_prs` table.

   The gallery, built 2026-10-18 (`service/fleet_agents/gallery.rs`): the
   definitions in `catalog-seed/fleet-agents/`, embedded in fleet-core
   (`SEEDS`; a test checks it lists the directory and that each loads
   clean). The `agents` tool's `gallery` lists them, each marked whether the
   catalog has one of that name; `install {definition}` adds one to the
   personal catalog as a commit (`catalog::author::install_in`), private and
   with no source, and refuses a name the catalog has, so an edited copy is
   never overwritten. Automation › Agents shows the ones not installed under
   *Gallery* with *Add to catalog*. Next to the self debug agent, the
   proposer, simplify and refactor, it ships three for everyday work:
   `issue-triage` (`tracker_comments` on `/triage`: a triage comment, or a
   question when the issue is too vague), `pr-questions` (`github_comments`
   on `/ask`: an answer from the code as a `pr_comment`) and `daily-digest`
   (a workday `schedule` reading `fleet-diagnostics`, kept in a fleet
   store).

   Polling a tracker that is not Jira, built 2026-10-18: no provider grows
   `issue_threads`. The poll pages through the items the hub's tracker sync
   wrote (`updated_at`, the local write, which a sync moves only on a real
   change; not the tracker's own update time, so a sync that runs late
   loses nothing), oldest first, from where the last poll stopped
   (`Cursor::items`, `(updated_at, id)`), at most 25 a poll (`ITEM_POLL_MAX`,
   `Store::tracker_items_written_since`): a burst bigger than a page is read
   over the next polls, and an unchanged item is not read again. For
   `tracker_comments` it reads each one's newest 20 comments through its
   provider's `comments`; `tracker_assigned` makes no comments call. A rate
   limit or an outage stops the page at the item before, for the next poll.
   A comment counts from the rule's first poll (`Cursor::start`), and no
   further back than a kept job key lives (30 days). With the tracker sync
   off (`work.sync_interval_secs = 0`) the poll fails and says so. The job
   keys are the Jira poll's (`<tracker>:<key>#c<id>`, `#assigned:<who>`), so
   an item read again finds nothing twice. The sync holds what its views
   read: on GitHub and Linear only the items assigned to the tracker's own
   account (`assignee:@me`, `isMe`), on Asana those or a `project:` view's.
   So `tracker_comments` there sees comments on those items only, and a
   `tracker_assigned` whose `match.assignee` is anyone else never matches. The subject is kept when the job is found, the item's own
   URL with it, and the run reads it back rather than asking the tracker
   again. `match.assignee` is the account id as the sync stores it (a GitHub
   login, a Linear or Asana user id). A comment there has no author id, so
   the loop guard is the marker alone. `query` is JQL: it is optional on
   `tracker_comments` and `tracker_assigned`, and a poll of a tracker that is
   not Jira refuses one that is set.

## Not in scope

- Code in a definition: no scripts, no expressions beyond the form
  conditions, no custom triggers.
- Write tools offered to the model.
- Webhooks: polling stays (D13).
- Outbound Slack/Teams: it stays the owner's decision from the previous spec.
  A Slack MCP store with a `write_tools` entry would technically make it an
  `mcp_call` action, which is one more reason for guardrail 5.

## Open questions

1. **Where a store's server runs.**
   - v1: on the rule's host, as every run does now.
   - A store reachable only from another host (a Graylog inside one customer's
     network) would need the run, or just that store, on that host.
   - Should a binding name its own host, the call relayed through the hub?
2. **Secret naming** (decided 2026-10-26): both. A binding's `env` names
   the secrets a person picks, so one can serve several instances; with no
   override the server reads the hub's secrets under its own `${NAME}`s.
   The grant form's *this instance's own secrets* switch per MCP role fills
   every override it leaves out with a generated name,
   `AGENT_<INSTANCE>_<ROLE>_<NAME>` (`form::own_secret`, upper case, any
   other character `_`), and its help lists the secrets each catalog server
   reads (`FormContext::server_secrets`). The grant still starts each
   binding and names any secret missing from the hub's store; *Edit* shows
   the switch on and only the person's own lines.
3. **Shared fleet namespaces across orgs.**
   - v1: a shared namespace is per hub.
   - Should it be per org, so one org's instances cannot read another's
     records?
4. **Cost** (decided 2026-10-25). A definition's `limits.daily_budget_usd`
   (more than 0, at most 1000) caps what one instance's runs cost in a UTC
   day: the jobs a rule finished since the day began are summed
   (`Store::agent_rule_cost_since`), and once that reaches the budget a job
   is deferred to the next day, not dropped. `limits.budget_input` names a
   `number` input whose value lowers the budget for one instance; a value
   above the definition's is held to it, so an instance may spend less than
   its definition allows, never more. Unset is no cap. The gallery's
   `support` and `pr-reviewer` cap at $25 a day, `pr-test-debate` at $5,
   each with a `daily_budget` input to lower it.
