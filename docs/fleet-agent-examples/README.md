# Configurable agent examples

Each directory is a `kind: fleet_agent` catalog asset (`asset.yaml` and its `skill.md`), as it would
sit in a catalog repo under `fleet-agents/<name>/`. Design:
`docs/superpowers/specs/2026-10-12-configurable-agents-design.md`. The fleet-core tests load and
validate every example here (`service::fleet_agents::tests`); check one by hand with
`fleet-hub agents define docs/fleet-agent-examples/<name>`.

An instance of one is granted with its input values and store bindings (spec *D. Stores*), e.g.
for `l2-support`:

```bash
cat > values.json <<'JSON'
{ "jira_tracker": "1", "project": "L2S", "bug_project": "BUG" }
JSON
cat > bindings.json <<'JSON'
[
  { "role": "logs", "server": "graylog", "name": "logs-prod",
    "env": { "GRAYLOG_BASE_URL": "${GRAYLOG_PROD_URL}", "GRAYLOG_API_TOKEN": "${GRAYLOG_PROD_TOKEN}" } },
  { "role": "kb", "server": "known-errors" },
  { "role": "memory", "namespace": "shared:l2-fingerprints" }
]
JSON
fleet-hub agents grant fleet l2-prod --definition l2-support --host mercury \
  --project acme/pos --level propose --values values.json --bindings bindings.json
```

An `env` value names a secret (`fleet-hub` / the Assets tab's secrets), never a value. Each MCP
binding is started on the host and asked for its tools before the rule is written.

The gallery's restated built-in agents (`support`, `pr-reviewer`, `pr-test-debate`) are rolled
out next to the built-in ones as `docs/fleet-agents-rollout.md` describes.
