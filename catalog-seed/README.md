# catalog-seed

Assets claude-fleet ships for your asset catalog (see
`docs/superpowers/specs/2026-09-14-asset-catalog-design.md`). This directory
is a valid catalog root, so it can be loaded as-is in a test. The fleet
agents in `fleet-agents/` are the definition gallery: fleet-core embeds them
(`service::fleet_agents::gallery`, every directory listed in `SEEDS`, a test
checks it), and Automation › Agents › Gallery adds one to your catalog with
*Add to catalog* (`agents { action: install, definition }`). Installing never
overwrites a definition the catalog has; the copy is yours to edit in Assets.
To use any other asset, copy its folder into your catalog's checkout, commit,
and reload (`fleet-hub catalog reload`, or Assets → Reload on the desktop).
The next sync installs it on your hosts.

| Asset | What it is for |
|---|---|
| `fleet-agents/support` | The support agent as a definition: on `Tech help!` or `/analyze` in a Jira ticket's comment, investigates with the bound logs, tenant registry and knowledge base, fingerprints the error it names (`run.fingerprint`), links the ticket to the bug already filed for that error or (with a person) to a matching open bug, and otherwise files a high-confidence bug that later runs of the same error link to. The built-in support agent stays beside it. |
| `fleet-agents/pr-reviewer` | The PR reviewer as a definition: reviews each new head of an open pull request of the instance's repo in a worktree of that head, from its filtered diff. Line comments land on the diff, a re-review settles the previous review's findings, a dismissed review is made again, and the description is written when the PR has none. Never an unattended REQUEST_CHANGES, nor an unattended APPROVE unless `limits.auto_approve`. |
| `fleet-agents/pr-test-debate` | The reviewer's test debate: when a pull request's author replies to a missing-test finding arguing that no test is needed, judges the argument with read-only access to the head and answers in the thread (at most two rounds). A concession resolves the thread and is kept as a test exemption. It shares that store of accepted exemptions with `pr-reviewer`, which then does not flag the location again. |
| `fleet-agents/issue-triage` | Triage on request: someone comments `/triage` (the trigger phrases are an input) on an issue of a tracker (Jira, GitHub, Linear or Asana); the agent reads the project's code and comments the issue's kind, severity, where in the code it likely lives and the next step, or asks for what is missing. |
| `fleet-agents/pr-questions` | Answers on GitHub: someone comments `/ask …` on an issue or pull request of the instance's repo; the agent answers from the project's code (read-only) as a reply comment, with the files it read and how sure it is. |
| `fleet-agents/daily-digest` | A workday-evening digest (17:00): reads the day's runs, sessions, failures and host health through `fleet-diagnostics` and keeps what got done, what went wrong and what needs a person tomorrow in a fleet store; the job's result is the digest. |
| `fleet-agents/self-debug` | The self debug agent: a manual fleet agent whose run reads the fleet's own logs through `fleet-diagnostics` (hub log, error reports, runs, host health, host journals and OOM kills, fleet-agent and updater logs, sessions) and answers with findings and their evidence. Grant an instance in Automation › Agents, then *Run now* with what to look into and where. It remembers what it found in a fleet store; each finding may carry one next step (a Work task, a fix session, a setting change or a session recover), and with an issue tracker and project set it can propose an issue; a person approves each. |
| `fleet-agents/proposer` | The proposer: a daily fleet agent (07:00, or *Run now* / Control's `/propose <focus>`) that reads what the other agents concluded and proposed lately (`fleet-diagnostics` `analyses` and `proposals`), merges repeats by fingerprint, drops what is pending, done or rejected in the last 14 days, and proposes at most five next steps of the same kinds. A person approves each in the Inbox. |
| `fleet-agents/simplify` | The simplify agent: a manual fleet agent that reads one project's code (read-only checkout) and finds what can be simpler (dead code, duplicates, needless layers, long functions). Each finding may propose a Work task or, with the instance's `repo` set, a fix session; a person approves each. |
| `fleet-agents/refactor` | The refactor agent: like simplify, for structure (leaking module boundaries, god files, coupling, missing test seams), with each refactor split into steps that leave the project green. |
| `skills/fleet-guides` | Lets a Claude session on a host write a step-by-step guide for Settings → Guides and propose it through the `claude-fleet` MCP `guide` tool; a person approves it (`docs/pages.md` → *Guides*). |

`service::guides` tests load this catalog with the catalog's own reader,
lint it, and render the skill as sync installs it.
