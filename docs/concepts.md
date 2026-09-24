# claude-fleet — core concepts

claude-fleet manages long-lived Claude Code processes running in tmux across multiple machines, presenting them as a unified fleet from a single Tauri 2 desktop app.

## Sessions

A session is a tmux window on some host that runs a Claude Code process; claude-fleet attaches to it, streams its output, and can send prompts or keystrokes. Sessions that should run unattended are created as **background sessions** (`kind: 'bg'`): they are headless, supervised by the reconcile tick, and never attach a terminal. Work sessions and background sessions share the same lifecycle model — create, peek, kill — but only work sessions get an interactive terminal view.

## Hosts & accounts

Hosts are discovered from `~/.ssh/config` plus a built-in `local` entry for the machine running the app. SSH connections to each remote host are multiplexed through a per-host `ControlMaster` socket so subsequent commands reuse the same authenticated channel without re-negotiating. The app probes each host's `~/.claude.json` to surface the signed-in account (email, org, tier); it reads this file for display purposes only — no credentials are extracted or stored by claude-fleet.

## Projects & worktrees

Each host has a **projects base**, the directory that holds its repositories, set in Settings → Projects. The layout under that base is either `github` (`<base>/<owner>/<repo>`) or `flat` (`<base>/<repo>`). When a host has no base configured, the default is `~/projects/github.com` (`~/projects` for `flat`). On the local machine the `CLAUDE_FLEET_PROJECTS_BASE` environment variable is checked before that default. The app scans the local base for repositories and their git worktrees. For a remote host it derives the project directory from that host's base and clones the repository on first use. Sessions are associated with the project whose working directory they were started from, so the UI can group and filter sessions by repository and branch.

## Control API & tunnels

An embedded MCP server (disabled by default, bound to `localhost`, protected by a bearer token, default port 4180) exposes the full fleet API so an AI assistant can drive sessions programmatically — creating sessions, sending prompts, reading output. When the control API is enabled, **reverse SSH tunnels** (`ssh -R`) forward that localhost port to each remote host's localhost, allowing remote agents to call back to the central server. Each tunnel is supervised: if the `ssh` process exits it restarts with capped exponential backoff. The same fleet can instead be run headless as the `fleet-hub` daemon, with the control API always on and bound to a configurable address; a hub given a public URL is reached directly by every host, and reverse tunnels exist only for a hub left on loopback. The desktop can also be paired with a hub as an ordinary client (Settings → Hub), and it then becomes a live window onto the hub's fleet rather than a second owner of it, decided once at startup. See [control-api.md](control-api.md) for the full tool reference and [hub.md](hub.md) for the daemon and for pointing a desktop at it.

## Work

A session can say **what work** it is doing: a ticket key (`ABC-123`) or a
free-form workstream, linked to the session's identity so it survives moves
and restarts. Keys found in branch names group sessions in the sidebar with
no setup; an explicit link (from the UI, or the in-session agent) wins over
recognition, and "Not this" is sticky. When a session ends its link ends
with a snapshot, and the work journal keeps what its conversations did, so
the work can be resumed later — continued, or started fresh with a handover
brief. A **tracker** (Jira Cloud today, configured on the hub) only enriches
this: titles and status on the chips, tickets in ⌘K, and starting a session
from a ticket in one step. Trackers are polled, read-only, and never gate
anything. See [control-api.md](control-api.md) (`work`, `work_link`,
`work_admin`) and [hub.md](hub.md) → *Trackers*.

## Lifecycle

Fleet keeps the sidebar about current work without destroying anything
useful. It **suggests** cleaning up — "Tidy up · n" in the attention strip,
shown only when there is something to tidy — and a person confirms in one
sheet; nothing is killed automatically unless `work.auto_tidy` is turned on
(off by default, see [hub.md](hub.md) → *Tidy-up and auto-tidy*). A
suggestion has a reason: the linked ticket is done (for
`work.tidy_done_days`) and the session idle (for `work.tidy_idle_hours`), its
PR is merged, the ticket was closed as won't-do or duplicate, two sessions
work in one worktree, or a lost session is a day from being reaped. Work that
comes back — a ticket moved out of done — shows as **Reopened** with its past
sessions and Resume.

A "session" is six things, and fleet may touch only the first three, only on
a confirm (or opt-in auto-tidy):

| Layer | Fleet may… | Fleet never… |
|---|---|---|
| work link | end it (with its snapshot) when the session is killed; **archive** a live session, UI-only | delete it |
| sidebar visibility | collapse archived or done work into the group's *Done* | hide a session that needs you |
| tmux session | kill it through **safe kill** (Claude commits and pushes first); plain-kill a session that shares its worktree with another | kill a dirty or unpushed worktree any other way |
| Claude conversation / transcript | — | delete it |
| worktree / branch | — (safe kill removes the worktree only after the push succeeded) | delete a branch with unpushed commits |
| journal / history | — (retention is `work.journal_days`) | delete it |

**Archive** collapses a live session into its work group's Done; tmux keeps
running, and the next prompt or attach brings it back. **Snooze 7 d** and
**Never for this work** are per link. No setting overrides the protections:
a session that is working, blocked, stuck or waiting on a dialog, one linked
to an in-progress ticket, the controller and the operator, a session
prompted or attached to within the hour, and a background agent with open
tasks are never suggested.

## Asset catalog

Skills, subagents, hooks, MCP servers and plugin references can be kept in a
git repo in a harness-neutral format and managed from the **Assets** tab.
Fleet loads the repo on the controller, renders every asset the way each
harness expects it (Claude Code fully; Codex CLI for skills and MCP servers),
scans hosts read-only for what is actually installed, and shows each asset
as in sync, drifted, missing or unsupported per host. Assets found on a host
but not in the catalog are listed as unmanaged and can be imported.

Skills, agents and MCP servers have an optional `install_as` field naming the
identifier a harness installs them under, when it differs from the catalog
name (the Claude Code skill/agent directory or `mcpServers.<key>`; the Codex
skill directory or `mcp_servers.<key>`) — hooks and plugin references derive
their host key from other fields and cannot set it. Importing a host sets
`install_as` whenever slugifying its identifier into a kebab-case catalog
name changes it (`~/.claude/skills/foo_bar` becomes catalog `skill/foo-bar`
with `install_as: foo_bar`), so the rendered asset keeps installing under the
original identifier and that host copy reads as in sync rather than
unmanaged; when the original identifier is not itself a valid install name
(a space, a slash, or exactly `.` or `..`), the import proceeds under the slug without
`install_as` and the report lists it as a warning instead. The asset editor
exposes an "Installs as" field for the kinds that support it, and the detail
view shows "installs as `<name>`" when one is set.

**Sync** is plan-first: `plan_sync` scans the selected hosts and computes
which assets to create, update, overwrite, adopt, or remove, returning a plan
valid for 10 minutes. `apply_sync` applies the plan using compare-and-swap on
every file against the scan-time hash, and creates `.fleet-bak-<time>-<pid>`
backups before overwriting or removing files and keeps the three newest
backups of each file. Config merges (JSON for Claude
Code, TOML for Codex) are applied on the controller and written through the
secure 0600 path; plugins are installed via `claude plugin install` on the
host. A pinned plugin is updated through the harness CLI only when the
catalog's pin changes; `latest` refs are never updated automatically. A
per-harness managed manifest (`~/.claude/.fleet-assets.json` and
`~/.codex/.fleet-assets.json`) records what fleet installed, so only managed
assets are ever removed. Secrets referenced as `${NAME}` in assets are resolved
at apply time from the fleet database (global with per-host override) and never
leave the controller. Secret values are stored in the fleet SQLite database in
plaintext, the same as host tokens — there is no at-rest encryption layer.
Codex support is experimental; TOML comments are not
preserved during config merges, and config files containing TOML datetimes are
rejected. The format and implementation are specified in
`docs/superpowers/specs/2026-09-14-asset-catalog-design.md` and
`docs/superpowers/specs/2026-09-14-asset-sync-design.md`.

The **Assets** tab provides a graphical editor for authoring: create assets from templates, edit them in a form with a text editor for the body, lint before saving (errors block save, warnings do not), and every save auto-commits with a `catalog: create|update|delete <kind>/<name>` message; push to the upstream is explicit. **Open in session** hands an asset to an interactive fleet session whose working directory is the catalog repo; that session, like any other, edits the repo directly and commits with `catalog:` messages, which the app picks up on its next catalog load.

## The terminal

The in-app terminal is a hand-rolled ANSI screen-buffer renderer (`src/lib/ansi.ts` + `TerminalView.svelte`), not xterm.js. xterm.js was tried first but its renderer silently no-ops after the first write in the Tauri 2 + macOS WKWebView environment, producing a blank terminal. The custom renderer covers the escape-sequence surface area that tmux and Claude's TUI actually emit — SGR colors, cursor positioning, clear-screen/line, basic scrolling — and renders into a plain DOM node where repaint is reliable. The trade-off is fewer features: no mouse tracking, no application keypad, no scrollback beyond the visible window. Only one PTY is attached at a time.

---

Going deeper: see [../CLAUDE.md](../CLAUDE.md) for architecture details and conventions, and [specs/](specs/) for in-progress design documents.
