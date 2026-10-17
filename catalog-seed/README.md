# catalog-seed

Assets claude-fleet ships for your asset catalog (see
`docs/superpowers/specs/2026-09-14-asset-catalog-design.md`). This directory
is a valid catalog root, so it can be loaded as-is in a test; to use an
asset, copy its folder into your catalog's checkout, commit, and reload
(`fleet-hub catalog reload`, or Assets → Reload on the desktop). The next
sync installs it on your hosts.

| Asset | What it is for |
|---|---|
| `fleet-agents/self-debug` | The self debug agent: a manual fleet agent whose run reads the fleet's own logs through `fleet-diagnostics` (hub log, error reports, runs, host health, host journals and OOM kills, fleet-agent and updater logs, sessions) and answers with findings and their evidence. Grant an instance in Automation › Agents, then *Run now* with what to look into and where. It remembers what it found in a fleet store; each finding may carry one next step (a Work task, a fix session, a setting change or a session recover), and with an issue tracker and project set it can propose an issue; a person approves each. |
| `fleet-agents/proposer` | The proposer: a daily fleet agent (07:00, or *Run now* / Control's `/propose <focus>`) that reads what the other agents concluded and proposed lately (`fleet-diagnostics` `analyses` and `proposals`), merges repeats by fingerprint, drops what is pending, done or rejected in the last 14 days, and proposes at most five next steps of the same kinds. A person approves each in the Inbox. |
| `fleet-agents/simplify` | The simplify agent: a manual fleet agent that reads one project's code (read-only checkout) and finds what can be simpler (dead code, duplicates, needless layers, long functions). Each finding may propose a Work task or, with the instance's `repo` set, a fix session; a person approves each. |
| `fleet-agents/refactor` | The refactor agent: like simplify, for structure (leaking module boundaries, god files, coupling, missing test seams), with each refactor split into steps that leave the project green. |
| `skills/fleet-guides` | Lets a Claude session on a host write a step-by-step guide for Settings → Guides and propose it through the `claude-fleet` MCP `guide` tool; a person approves it (`docs/pages.md` → *Guides*). |

`service::guides` tests load this catalog with the catalog's own reader,
lint it, and render the skill as sync installs it.
