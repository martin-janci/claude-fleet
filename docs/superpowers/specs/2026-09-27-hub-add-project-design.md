# Add project from a hub client

Date: 2026-09-27. Status: approved for implementation. Plan:
`docs/superpowers/plans/2026-09-27-hub-add-project.md`.

## Problem

A desktop paired with a hub cannot add a project. `add_project` and
`list_github_repos` are `LocalOnly` in `src-tauri/src/backend/verdicts.rs`, so
the sidebar's "＋ Add project…" is disabled with the sentence "Do it on the
hub". Nothing on the hub can do it:

- The hub serves no `add_project` / `list_github_repos` tool and `fleet-hub`
  has no project subcommand.
- The only other production writer of `projects` rows is
  `service::projects::refresh_projects`, which scans the hub machine's own
  projects root (refused outright with `hub.local_host` off). It never sees
  `mac`, `mefistos` or an agent host.
- Unpairing does not help: a standalone desktop writes its own `state.db`,
  which the hub never reads, so no hub session can be started in that project.

The `LocalOnly` reason ("uses this machine's SSH and GitHub credentials") is
also not true of the service: `add_project_with` runs `git clone`, `gh repo
list` and `gh repo create` **on the target host** through `SshExec`. The
credentials are the host's. The hub already owns an `SshClient` with a route to
every host (SSH or `fleet-agent`), so the same service call works there.

## Decisions

| # | Decision |
|---|----------|
| D1 | `add_project` and `list_github_repos` become hub tools in `mcp/tools/repo.rs`, calling the existing `service::add_project` functions. No new service logic. |
| D2 | Both commands become `Verdict::Routed` on the desktop; the `REASONS` entries and the `refuse_local_only` calls go away. |
| D3 | No hub-side cancel. The dialog's Cancel in remote mode aborts the desktop's HTTP call only; the hub run completes (or hits its deadline) and its row arrives as a `project` event. This is the semantics a remote host already has in standalone mode ("Stopped waiting — `host` may still finish"), and the service doc says even a local cancel cannot stop a remote clone. `call_id` is never sent to the hub, exactly like `new_session`. |
| D4 | Tool policy: `add_project` is `Access::Client`, not readonly, no `confirm` (it has its own single-use `create_remote` confirm token, which round-trips through the tool error's `details.confirm`), deadline class `LongPoll` (660 s) because a clone's wall clock is 600 s and `Lifecycle` is 300 s. `list_github_repos` is `Access::Client`, readonly, `Quick` (its `gh` wall clock is 30 s). |
| D5 | The dialog hides the **Existing folder** source on a hub client. It forces host `local` and opens this machine's folder picker; under a hub `local` is the hub's machine. The other three sources (clone URL, GitHub browse, new project) stay. |
| D6 | On a hub client every host counts as remote for the dialog's "what stopping cannot undo" note, including the hub's `local`. |
| D7 | `GithubRepo` becomes a wire type: it joins the contract golden, `CONTRACT_REVISION` goes 4 → 5, and `MIN_HUB_CONTRACT` = `MAX_HUB_CONTRACT` = 5, following the revision-4 precedent (a hub without the tool would otherwise show an enabled button that fails with an unknown-tool error). |
| D8 | `add_project` joins `ROUTED_ACTIONS` in `src/lib/hub.ts`, so the opener is disabled with the offline sentence while the live link is down, through `hubActionBlocked`. |

## Hub tools

```
add_project { host_alias, source }        → ProjectTreeRow   (repo.rs)
list_github_repos { host_alias }          → Vec<GithubRepo>  (repo.rs)
```

`source` is the existing tagged enum `AddProjectSource` (`kind`: `clone { url }`,
`folder { path }`, `new { owner, repo, create_remote, confirm }`). `AddProjectArgs`
and `AddProjectSource` derive `rmcp::schemars::JsonSchema` the way
`hosts::AddHostArgs` does; `call_id` is `#[schemars(skip)]` so it is not part of
the served schema. Every field and variant gets a one-line doc comment
(`every_tool_parameter_is_documented`). Descriptions are one sentence each; the
definition budget test (`the_served_definition_budget_stays_bounded`) is raised
by the measured amount, with the numbers in its doc comment as before.

The tool body is `add_project::add_project(args, &self.store, &*self.ssh,
&self.reg)` with `call_id: None`, so the registry mints an anonymous token and
the `CancelGuard` releases it. `folder` on a non-`local` host is refused by the
service already (`E_INVALID`); the hub's own `local` keeps working for an
operator calling the tool directly.

Per-host tokens: `Access::Client` is the same class as `new_session`, so a host
token may add a project on another host, matching "whole-fleet session
control" as documented on `Access`. A `readonly` client gets
`list_github_repos` only.

Audit lines: `add_project host=<alias> kind=<clone|folder|new> target=<url|path|owner/repo>`;
`list_github_repos host=<alias>`. No token or path body beyond that.

## Desktop routing

`commands/projects.rs` gains a `routed::add_project` and
`routed::list_github_repos` following `routed::probe_host`. The hub branch
spells the arguments out as `json!({ "host_alias", "source" })` so `call_id`
stays local (`AddProjectSource` needs `Serialize` for that). Both verdict rows
become `Routed { tool: <same name> }`, `tests_routing.rs` gets a row each with
non-default arguments (a `new` source with `create_remote: true` and a
`confirm`, to prove the tagged enum crosses the wire), and the module doc
comment that says neither has a hub tool is rewritten.

`remote.rs`'s `call_timeout` needs no change: `tool_deadline("add_project")`
is the `LongPoll` cap plus the margin.

## Frontend

- `hub.ts`: remove `add_project` from `REASONS`; add `add_project` to
  `ROUTED_ACTIONS`.
- `Sidebar.svelte`: `addProjectBlocked` uses `hubActionBlocked('add_project',
  $hubStatus, $hubConnection)`.
- `hub_verdicts.test.ts`: drop `gatedByAddProjectDialog` (its one entry,
  `list_github_repos`, is routed now).
- `AddProjectDialog.svelte`: the mode list omits `folder` when
  `$hubStatus.remote`; `remote` for the stop note is `inflight.host !== 'local'
  || $hubStatus.remote`.
- `projects.ts` needs no change: `addProject` and `listGithubRepos` already go
  through `invokeCmd*`, and the hub error's `details.confirm` reaches
  `confirmTokenOf` unchanged (`remote.rs::tool_error` forwards `details`).

## Wire contract

`GithubRepo` is registered in `backend/contract.rs` as a report type
(`Deserialize`, no serde defaults, per the contract test's rules). Regenerate
with `REGEN_HUB_CONTRACT=1`, bump `CONTRACT_REVISION` to 5 and both bounds to
5, and extend the doc comment on `MIN_HUB_CONTRACT` with the reason: a
revision-4 hub does not serve `add_project`.

## Generated artifacts and docs

After the code changes, in this order:

```bash
REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen
REGEN_DOCS=1 cargo test -p fleet-core reference_is_current
REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract
```

`docs/control-api.md`'s "Projects & worktrees" bullet names the two tools.
`docs/hub.md`'s refusal table loses both rows through the regen; its prose
gains one sentence under the hub-client section: a paired desktop adds
projects through the hub, which clones on the target host.

## Testing

- `mcp/tools/tests.rs`: `add_project` over a `FakeSsh` registers a row and
  returns it; a `create_remote` call without `confirm` answers
  `E_CONFIRM_REQUIRED` (or the service's code) with `details.confirm` in the
  structured content; `list_github_repos` parses `gh`'s JSON; a readonly caller
  is refused `add_project` and allowed `list_github_repos`; the policy
  exhaustiveness and parameter-doc tests pass.
- `tests_routing.rs`: the two new Routed rows, arguments as sent.
- `hub_verdicts.test.ts`, `Sidebar.test.ts`, `AddProjectDialog.test.ts`:
  the opener is enabled on a connected hub client and disabled with the
  offline sentence while reconnecting; the folder mode is absent on a hub
  client and present standalone; the stop note says the host may still finish.
- Full suites per `scripts/ci-local.sh` before the PR (both halves).

## Rollout

1. Merge; cut a `claude-fleet` release (the hub and the desktop share the
   version and the contract).
2. Deploy the hub image to the NAS first (`fleet-hub-deploy-nas`), then install
   the desktop. A desktop at revision 5 refuses a revision-4 hub with the skew
   banner, which is the intended failure mode.
3. `scripts/release-mobile.sh` as for every release; the mobile client is not
   affected by the new tools.

## Out of scope

- A hub-side cancel tool (D3).
- Scanning remote hosts' project roots from the hub (a different feature:
  discovery, not adding).
- `purge_project`, which stays `LocalOnly` for its own reasons.
