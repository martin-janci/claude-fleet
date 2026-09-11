# Troubleshooting

This guide covers the most common problems encountered when running
**claude-fleet** (the Tauri 2 desktop app for managing Claude Code tmux
sessions over SSH). Each row in the table below maps a visible symptom to its
most likely cause and the fix. Extra detail follows the table where a row needs
it.

---

## Quick reference

| Symptom | Likely cause | Fix |
|---|---|---|
| Host shows **offline** / probe fails | SSH host unreachable or key auth not configured | Fix `~/.ssh/config`; verify `ssh <alias>` works in a terminal. (`E_HOST_OFFLINE`) |
| `claude` or `tmux` **missing** on a host | Binary not installed or not on `PATH` for non-interactive SSH sessions | Install the missing tool on the host; confirm it is on the default `PATH`. (`E_CLAUDE_CLI`) |
| **Provisioning failed** | Cannot write `~/.claude.json`, `~/.claude/CLAUDE.md`, `~/.tmux.conf`, or the skills directory | Read the `detail` string in the per-host result; fix the permissions or path involved. (`E_PROVISION`) |
| Tunnel shows **"down — retrying"** | Control API is disabled, or the host `sshd` blocks remote port forwarding | Enable the Control API in Settings; check `AllowTcpForwarding` / `GatewayPorts` in the host's `sshd_config`. |
| **MCP bind error** — server enabled but not listening | Port 4180 (or configured port) already in use | Change the port in **Settings → Control API (MCP)**. The `bind_error` field in `McpStatus` shows the exact OS error. |
| **No projects found** | The projects base is empty, does not exist, or uses a different layout | Set this machine's projects base and layout in **Settings → Projects**, then click **Save & rescan**. With no base set, `CLAUDE_FLEET_PROJECTS_BASE` and then `~/projects/github.com` are used. (`E_FLEET_PROJECTS_BASE`) |
| Session **won't attach** / appears as a ghost | The underlying tmux session has been destroyed | Use **Recreate** to replace the session, or **Dismiss** to remove the ghost entry. |
| **Every** session on one host turned into a ghost | The host rebooted or its tmux server restarted | See [tmux server restarted](#a-hosts-tmux-server-restarted-all-sessions-become-ghosts). Recreate before the next pass removes the rows. |
| Hosts **offline** right after the laptop wakes | SSH ControlMasters went stale during sleep | Wait one or two reconcile passes, or click **Re-probe**. See [after sleep / wake](#after-laptop-sleep--wake). |
| Sidebar looks **stale** | Cache-first `list_sessions` inside the reconcile interval | Click **Refresh** (forced pass). See [reconcile tick](#the-reconcile-tick-reconcileinterval_secs-and-refresh). |
| Need logs / reporting a bug | n/a | **Settings → Diagnostics → Copy diagnostics**; logs under `<app data>/logs/`. See [Logs](#logs-where-they-live-and-how-to-raise-verbosity). |
| Session's **worktree directory vanished** (git errors in the pane, `cd: no such directory`, new panes fail) | The worktree was deleted, pruned, or moved on disk while the fleet row (and possibly the tmux session) survived | New session, Restart, Recreate and opening the terminal re-create only what is confirmed missing; anything more (a stale git entry, a moved checkout, a deleted branch, a pane in a removed directory) needs **Repair workspace** in the session details (or the `repair_session` tool). See [Repairing a session whose directory vanished](#repairing-a-session-whose-directory-vanished). (`E_REPAIR_REQUIRED`, `E_REPO_MISSING`, `E_BRANCH_CHECKED_OUT`, `E_WORKSPACE_LOCKED`, `E_REPAIR_FAILED`) |
| *(Developers)* `Failed to resolve import "@tauri-apps/plugin-clipboard-manager"` in `App.test.ts` / `clipboard_native.test.ts` | Stale `node_modules` after pulling | Run `pnpm install --frozen-lockfile` (pnpm 10; `corepack pnpm@10 install --frozen-lockfile` if your pnpm is older), then re-run `pnpm test`. `localStorage` is polyfilled in `vitest.setup.ts`, so a missing-`localStorage` failure is not expected. |

---

## Detail

### Host shows offline / probe fails (`E_HOST_OFFLINE`)

claude-fleet probes each host at startup and on a background tick using the
SSH alias stored in the database. If the probe fails, the host is marked
offline. Common causes:

- The SSH alias is wrong or the entry is missing from `~/.ssh/config`.
- The host is firewalled or shut down.
- Key-based authentication is not set up (password prompts are invisible to
  the probe, so it times out silently).

Run `ssh <ssh_alias>` in a terminal on the machine running claude-fleet and
confirm it logs in without a password prompt. Once that works, use **Re-probe**
in the Hosts panel to refresh the status.

### `claude` or `tmux` missing on a host (`E_CLAUDE_CLI`)

The probe runs a non-interactive SSH command, so only the `PATH` configured in
the remote shell's non-interactive startup files (e.g. `~/.bashrc`, not
`~/.bash_profile`) is visible. If `claude` or `tmux` was installed via a
version manager (nvm, rbenv, mise, etc.), ensure those managers initialise in
`~/.bashrc` (or the equivalent for the remote shell), or create a symlink in a
standard `PATH` directory such as `/usr/local/bin`.

### Provisioning failed (`E_PROVISION`)

Provisioning writes four things to the remote host: the fleet-control skill
(`~/.claude/skills/claude-fleet-control/SKILL.md`), the fleet-friendly-name
skill, a managed block in `~/.claude/CLAUDE.md`, and a merged
`~/.claude.json` with the MCP server entry. A backup of the original
`~/.claude.json` is written to `~/.claude.json.fleet-bak` before any changes.

If provisioning fails, the per-host result contains a `detail` field describing
which step failed. Typical causes:

- The `~/.claude` directory or `~/.claude/skills` directory is not writable.
- `~/.claude.json` contains invalid JSON that cannot be parsed (check with
  `cat ~/.claude.json | python3 -m json.tool` on the remote host).
- The connection dropped mid-transfer (retry usually succeeds).

### Tunnel shows "down — retrying"

The reverse SSH tunnel (`-R`) that makes the local Control API reachable from
remote hosts requires the remote `sshd` to allow `AllowTcpForwarding yes` (or
at minimum `AllowTcpForwarding local`). If `GatewayPorts` is also needed,
enable it. After changing `sshd_config`, reload sshd (`systemctl reload sshd`)
and click **Re-provision** in the Hosts panel to restore the tunnel.

### MCP bind error

If `enabled` is `true` but `running` is `false` in `McpStatus`, the server
failed to bind its port. The `bind_error` field contains the OS-level message
(e.g. `Address already in use (os error 98)`). Find and stop the conflicting
process with `lsof -i :<port>`, or choose a different port in **Settings →
Control API**.

### No projects found (`E_FLEET_PROJECTS_BASE`)

The project scanner looks under this machine's projects base, in the
configured layout: `github` scans `<base>/<owner>/<repo>`, `flat` scans
`<base>/<repo>`. Set both in **Settings → Projects** and click **Save &
rescan**. The line under each field previews where projects are expected.

With no base set for this machine, the scanner uses the
`CLAUDE_FLEET_PROJECTS_BASE` environment variable (trimmed, with `~/`
expanded), then `~/projects/github.com` (`~/projects` for `flat`). Remote hosts
use their own Settings entry or that default; the environment variable only
applies to the machine running the app.

### Session won't attach / ghost session

A "ghost" session is a database row whose tmux session no longer exists on the
remote host (the host was rebooted, tmux was killed, etc.). Use **Recreate** to
spawn a fresh tmux session in the same window, or **Dismiss** to remove the
database entry. If Recreate fails with `E_TMUX`, confirm tmux is still running
on the host with `ssh <alias> tmux ls`.

### Logs: where they live and how to raise verbosity

claude-fleet writes an hourly-rotated log file into its app data directory
and keeps the newest 72 files, which is three days:

| OS | Log folder |
|---|---|
| Linux | `~/.local/share/claude-fleet/logs/` |
| macOS | `~/Library/Application Support/sk.rlt.claude-fleet/logs/` |

Files are named `claude-fleet.YYYY-MM-DD-HH.log`, where the hour is in UTC.
Older builds wrote one `claude-fleet.YYYY-MM-DD.log` per day. Those files are
still read, and they are deleted first when the folder is pruned.
**Settings → Diagnostics → Open log folder** opens the folder. The path is
also shown there, with a copy button.

Rotation is hourly because the logging library has no per-file size cap. With
daily rotation, a `RUST_LOG=debug` run could grow one day's file without
limit. With hourly rotation, one file holds at most an hour of output. Total
disk use still grows with the level you choose, since up to 72 hours of it are
kept, so unset `RUST_LOG` once you have the log you need.

Before a line is written, these are masked as `[REDACTED]`:

- a token-shaped value after `Bearer`: one that contains a digit or is at
  least 24 characters long, so prose such as "Bearer authentication" is kept;
- `?token=` / `&token=` query values;
- runs of exactly 64 hex digits, including one glued to other letters
  (`tok_<64 hex>`). A longer hex run, such as a SHA-512 digest, is kept.

The default level is `info` for claude-fleet itself and `warn` for its
libraries. Override it with the standard `RUST_LOG` syntax, then relaunch:

```bash
RUST_LOG=debug claude-fleet                        # everything at debug
RUST_LOG=warn,claude_fleet_lib=debug claude-fleet  # only the app at debug
RUST_LOG=warn,claude_fleet_lib=info,rmcp=debug     # plus the MCP library
```

A Finder-launched macOS app does not see your shell's environment. Run the
binary from a terminal instead
(`RUST_LOG=debug /Applications/claude-fleet.app/Contents/MacOS/claude-fleet`),
or `launchctl setenv RUST_LOG debug` and relaunch.

**Release builds do not log to stderr.** Debug builds (`pnpm tauri dev`) write
every log line to stderr as well as to the file. A release build writes only
to the file, unless you launch it with `CLAUDE_FLEET_LOG_STDERR=1` (or
`true`). Some lines used to be printed straight to stderr. These are the
`[startup]` instance-reaper and friendly-name backfill lines, the
`[reconcile-tick]` lines, and the `[mcp]` tunnel and start-failure errors. They
are now log lines, without those bracketed prefixes. A release build started
from a terminal no longer shows them unless you set the variable, so read
them in the log file instead.

The reconcile tick's `a reconcile pass is already running; skipping tick`
line is logged at `debug`, so the default level hides it. To see skipped
ticks, run with `RUST_LOG=warn,claude_fleet_lib=debug`.

Not every subsystem logs to the file yet. Older code in the SSH client, PTY,
reconcile and MCP handlers still prints to stderr only, and moving it to the
file logger is a follow-up. Until that lands, run the app from a terminal to
see those lines.

### The reconcile tick, `reconcile.interval_secs`, and Refresh

A background **reconcile tick** keeps the sidebar current without the UI
polling. Each pass probes every host over SSH, lists its tmux sessions and
Claude state, updates the session rows, and marks sessions that disappeared as
ghosts (see below). The stuck-session playbooks and the session GC sweep run
on the same tick, right after the pass, if you enabled them.

- **Interval:** `reconcile.interval_secs` in **Settings → Automation**. The
  default is `20`. `0` turns the tick off, so reconcile only runs when
  something asks for it. The tick reads the value at startup, so restart the
  app after you change it.
- **Cache-first `list_sessions`:** the sidebar, window focus and the MCP
  `list_sessions` tool serve the stored rows as long as the last completed
  pass is younger than the interval (or 20 s when the tick is off). They
  never stack a second fleet-wide probe on top of one that is already running.
- **Refresh (forced):** the sidebar **Refresh** button (it also rescans the
  projects, like the `refresh_projects` tool) and `list_sessions` with
  `force: true` over MCP start a pass right away, ignoring freshness. If a pass
  is already running they neither start a second one nor wait for it: they
  return the stored rows at once, and the running pass updates the sidebar as
  it writes its results. A forced MCP call in that case gets the rows from
  before the pass; call `list_sessions` again a moment later for fresh ones.

If the sidebar looks stale, click Refresh first. If Refresh does not change
anything, check the log for `reconcile failed` lines and probe the host from
Settings.

### After laptop sleep / wake

When the laptop sleeps, the SSH ControlMaster connections (one per host,
sockets in `~/.cache/claude-fleet/cm-<host>.sock`) go silent. On wake:

1. Keepalives (`ServerAliveInterval=5`, `ServerAliveCountMax=2`) make a dead
   master exit about 10 s after its peer stopped answering. The next command
   opens a fresh connection.
2. Every SSH command also has a **wall-clock timeout**: three times its connect
   timeout, and never less than 30 s. A command that hangs on a wedged
   connection fails with `E_SSH_TIMEOUT` instead of blocking forever.
3. After such a timeout the client resets that host's master (`ssh -O exit`),
   but only if no other command is using it and `ssh -O check` gets no answer.
   A slow command on a healthy connection keeps the master.

Expect hosts to show **offline** for a pass or two after wake, then recover
without any action. Reverse tunnels restart on their own (backoff up to
30 s). If a host stays offline after a minute, click **Re-probe** in
Settings. To remove a master by hand:
`ssh -O exit -o ControlPath=~/.cache/claude-fleet/cm-<host>.sock <host>`.
Re-attach the terminal view if it froze during sleep.

### A host's tmux server restarted (all sessions become ghosts)

If a host reboots or its tmux server is killed (`tmux kill-server`, OOM),
every session on it disappears from `tmux ls`. On the next reconcile pass
claude-fleet marks each of those rows `ghost` and stamps `lost_at`; the
sidebar shows them as ghosts. A ghost that is **still** missing on the following pass is removed
from the database. Worktrees, files and Claude's own transcripts on the host
are not touched either way.

To recover:

- **While the row is still a ghost:** use **Recreate**. It starts a new tmux
  session with the same name in the same worktree and resumes the recorded
  Claude conversation (`claude --resume <id>`) when the row has one.
  **Dismiss** removes a ghost you do not want back.
- **After the row is gone:** start a new session on the same project/worktree
  and resume inside Claude with `/resume` (or `claude --resume <id>`).
- If Recreate fails with `E_TMUX`, check that tmux works on the host
  (`ssh <alias> tmux ls`; "no server running" is fine, since Recreate starts
  one).

To keep ghosts around longer before removal, raise
`reconcile.interval_secs`. Removal happens on the pass after the ghosting
pass.

### Producing a diagnostics bundle for a bug report

Open **Settings → Diagnostics** and click **Copy diagnostics**. A toast
confirms when it is on the clipboard. Paste it into the issue. The bundle is
plain text and contains:

- app version, OS, `schema_version`, data and log directories, `RUST_LOG`;
- the automation settings (reconcile interval, playbooks, GC);
- the control API: enabled, bound, port, last bind error, and whether a
  master token is set;
- every host: reachability, last probe time, tmux/claude versions,
  provisioned, tunnel state, token **mode**;
- session counts by host and status;
- the last 200 log lines.

Tokens are never included. The master token and every per-host token are
masked even when they show up in a log line or error message. Hostnames, SSH
aliases and file paths **are** included, so read the bundle before you post
it publicly.
### Repairing a session whose directory vanished

A session row can outlive its directory: someone ran `git worktree remove`
or `rm -rf` on the worktree, a cleanup job pruned it, the checkout was moved
to the other layout (`.worktrees/` vs `.claude/worktrees/`), or the branch was
deleted after a merge. The tmux pane then sits in a deleted inode — git
commands fail, `cd` errors, new windows cannot start — and a plain Recreate
or Restart would rebuild the session at the repo root.

One probe script inspects the host (paths are compared in their resolved
form, so a project under a symlinked directory is fine); a healthy workspace
is a no-op. What happens next depends on who asked.

**Automatically** — new session on an existing worktree, Restart, Recreate,
and opening the terminal — fleet only *creates* what is confirmed missing:

- `git worktree add` for the session's branch when that branch still exists
  locally or as `origin/<branch>`, and the target directory is absent and not
  registered with git;
- `tmux new-session -c <dir>` (terminal open only) when `tmux has-session`
  confirms the session is gone.

Anything else is reported, never done: New session / Restart / Recreate
stop with `E_REPAIR_REQUIRED` and say what an explicit repair would do;
opening the terminal shows a notice and attaches anyway. A live pane is never
restarted just because you opened it.

**Explicitly** — **Repair workspace** in the session details (or the
`repair_session` control-API tool, which is behind the desktop confirmation
when that is on) — fleet may also:

1. `git worktree remove --force -- <path>` for this worktree's own stale
   entry (never a repo-wide prune, so other worktrees' stale entries — for
   example ones on an unmounted volume — are left alone);
2. recreate a branch that exists nowhere locally: it asks origin first
   (`git ls-remote`); if origin has it, it is fetched and tracked; only when
   origin confirms it is gone is a new branch made from the project's base
   branch (recorded as `branch_from_base:<start>` on the timeline and in the
   notice). A fetch or connection error stops the repair;
3. `git worktree repair` when the directory exists but its `.git` link is
   stale;
4. adopt the existing checkout when the branch is checked out in another
   linked worktree (the row's path is corrected) — only after that checkout
   verifies as healthy, and never when another fleet worktree or session uses
   it;
5. `tmux respawn-pane -k -c <dir>` for a live pane whose reported directory
   no longer exists (the pane's process restarts; Claude resumes its
   conversation).

Each run that changed something appends a `workspace_repaired` event (with
the actions and the branch source) to the session timeline; a refused or
failed repair appends `workspace_repair_failed`. The repair never deletes
files and never ghosts or removes the session row.

Cases that are reported rather than fixed:

- `E_REPAIR_REQUIRED` — an automatic check found something only **Repair
  workspace** may fix (see above).

- `E_REPO_MISSING` — the project's main checkout is missing or is not a git
  repository. It is never faked with `mkdir`; restore or re-clone it (a new
  session on a remote host clones automatically).
- `E_BRANCH_CHECKED_OUT` — the worktree's branch is checked out in the main
  checkout. Switch the main checkout to another branch, then repair again.
- `E_WORKSPACE_LOCKED` — the worktree is `git worktree lock`ed and its
  directory is gone. Run `git worktree unlock -- <path>` if the lock is stale.
- `E_REPAIR_FAILED` — a git step failed (permissions, disk full: the message
  carries git's stderr), origin could not be asked about a missing branch, the
  result did not verify, or the directory exists, is not empty and is not a
  worktree (move it aside; it is never deleted). If the connection dropped
  while the repair was being applied, the message says it may be partially
  applied — run **Repair workspace** again.
- `E_HOST_OFFLINE` — the host could not be reached before anything ran;
  nothing was changed.

### Releases and tags

`origin` currently has no `v0.2.x` tags even though the version fields moved
past 0.2.4, so `git describe --tags` and the changelog prefill in
`scripts/release.sh` see a stale baseline. Going forward the owner should cut
releases with `scripts/release.sh <version>`, which bumps the version files,
commits, and creates the `vX.Y.Z` tag; pushing that tag
(`git push origin main --follow-tags`) is what triggers the release workflow.
See [docs/RELEASING.md](RELEASING.md).

---

## See also

- [Getting started](getting-started.md)
- [Concepts](concepts.md)
