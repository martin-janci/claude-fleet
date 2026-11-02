# Rolling out the restated agents

The support agent and the PR reviewer exist twice now: as the built-in agents (`agent: support`,
`agent: review`) and as fleet agent definitions in the gallery (`support`, `pr-reviewer`,
`pr-test-debate`). The built-in ones stay until the definitions have run for real (spec
*Migration*, `docs/superpowers/specs/2026-10-12-configurable-agents-design.md`). This guide takes
a definition from the gallery to `act` next to its built-in twin, and says what to compare
before the twin is retired.

## 1. Add the definitions to the catalog

Automation › Agents › **Gallery** › *Add to catalog* on each one you roll out (or the control
API: `agents { action: install, definition: "pr-reviewer" }`). The definition becomes a
`fleet-agents/<name>/` asset of your catalog repo: edit it there like any asset, and check an
edited copy with `fleet-hub agents define <dir>` before it loads.

What to edit before the first instance, if anything:

| Definition | Worth a look |
|---|---|
| `support` | the `bug` action's `when` (files a bug only at `confidence: high`), `run.cooldown_secs` |
| `pr-reviewer` | `run.diff.exclude_paths`, `run.diff.max_lines`, the `review` action's `min_confidence` (50), `trigger.debounce_secs` |
| `pr-test-debate` | `trigger.max_rounds` (2), the `phrase` input if you change pr-reviewer's `🧪 Missing test:` |
| all three | `limits.daily_budget_usd`: $25 a day for `support` and `pr-reviewer`, $5 for `pr-test-debate` |

## 2. Grant an instance at `watch`

*New instance* on the definition opens the grant form: inputs, store bindings, host, project and
level. From a shell on the hub, `fleet-hub agents grant fleet <instance> --definition <name>
--host <host> [--project owner/repo] --level watch --values values.json --bindings bindings.json`.

At `watch` an instance runs and keeps its answer and the actions it would take, and sends
nothing; the built-in twin keeps doing the real work meanwhile. It still spends: its
`daily_budget` input lowers the definition's cap for this instance (never raises it), and
once a UTC day's runs have cost that much, the next job waits for the next day
(`fleet-hub agents job <id>` says `budget: …`).

### `pr-reviewer` and `pr-test-debate`

Both need a project (`--project owner/repo`): they run in a worktree of the pull request's head.
Bind their `exemptions` store to the **same** shared namespace, so a concession the debate keeps
is what the reviewer reads:

```bash
echo '{ "repo": "acme/pos" }' > values.json
echo '[{ "role": "exemptions", "namespace": "shared:acme-pos-test-exemptions" }]' > bindings.json
fleet-hub agents grant fleet acme-pos-review --definition pr-reviewer \
  --host mercury --project acme/pos --level watch --values values.json --bindings bindings.json
fleet-hub agents grant fleet acme-pos-debate --definition pr-test-debate \
  --host mercury --project acme/pos --level watch --values values.json --bindings bindings.json
```

The definitions and the built-in reviewer do not answer each other's threads: the built-in one
marks its comments `<!-- fleet-agent:review -->`, an instance `[fleet-agent:<instance>]`, and
each debate only takes threads of its own kind.

### `support`

The inputs are the Jira tracker, the ticket project, the trigger phrases, the bug project and the
app frame prefix. The stores bind the catalog's MCP servers and a fleet namespace for the
fingerprints:

```json
[
  { "role": "logs", "server": "graylog", "name": "logs-prod",
    "env": { "GRAYLOG_API_TOKEN": "${GRAYLOG_PROD_TOKEN}" } },
  { "role": "tenants", "server": "tenant-registry" },
  { "role": "kb", "server": "known-errors" },
  { "role": "memory", "namespace": "shared:l2-fingerprints" }
]
```

An `env` value names a secret, never a value. With no override the server reads the hub's
secrets under its own names (`GRAYLOG_API_TOKEN`), shared by every instance; the grant form's
*this instance's own secrets* switch names each one `AGENT_<INSTANCE>_<ROLE>_<NAME>` instead
(`AGENT_L2_PROD_LOGS_GRAYLOG_API_TOKEN`), so add those under Assets › Secrets first. Each MCP binding is started on the host and asked
for its tools before the rule is written, so a wrong server or secret fails the grant, not a run.

Give the instance its own trigger phrase while it watches (e.g. `/analyze-next`), or both agents
answer the same ticket comment.

## 3. Compare

For a week or two of real traffic, per job: `fleet-hub agents status` (the rules, newest jobs,
waiting actions), `fleet-hub agents job <id>` (one job whole: its answer and the actions it
planned), `fleet-hub agents explain` (why a rule does or does not poll). The same is under
Automation › Agents.

| | Compare with the built-in twin |
|---|---|
| pr-reviewer | the verdict on the same head; which findings it raises and where they land on the diff; the re-review verdict after a push (its `resolutions`); a skip for size or WIP; a dismissed review made again (`reverify_mins`) |
| pr-test-debate | concede or hold on the same reply; the exemption recorded on a concession; the round limit |
| support | the fingerprint (same error, same fingerprint across tickets); the bug it links or would file; the comment's content |

A definition's behavior that differs is either a skill to tune (edit `skill.md` in the catalog)
or a gap in fleet: note which.

## 4. Move up a level

- **`propose`**: each write waits for a person (Automation › Agents › the action, or
  `fleet-hub agents approve <id>` / `reject <id>`). Run the built-in twin at `watch` from here
  (revoke its rule and grant it again at `watch`), or a pull request gets two reviews.
- **`act`**: the actions marked `auto` are sent with no person. An unattended review never
  requests changes and never approves unless the definition's `limits.auto_approve` says so;
  pr-test-debate's replies and the support agent's comments are `auto`, its bug filing and
  bug links to a similar bug are not.

## 5. Retire the built-in twin

Once a definition has done its twin's work at `propose` or `act` without surprises: `fleet-hub
agents revoke <id>` the built-in rule. Deleting the built-in agents' code and converting their
stored rules and tables is a separate change, made once every rule has moved.
