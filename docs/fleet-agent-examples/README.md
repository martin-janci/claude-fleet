# Configurable agent examples

Each directory is a `kind: fleet_agent` catalog asset (`asset.yaml` and its `skill.md`), as it would
sit in a catalog repo under `fleet-agents/<name>/`. Design:
`docs/superpowers/specs/2026-10-12-configurable-agents-design.md`. The fleet-core tests load and
validate every example here (`service::fleet_agents::tests`); check one by hand with
`fleet-hub agents define docs/fleet-agent-examples/<name>`.
