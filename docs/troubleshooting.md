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
| **No projects found** | Scan path is empty or the base directory does not exist | Place repos under `~/projects/github.com/<owner>/<repo>`, or set `CLAUDE_FLEET_PROJECTS_BASE` to your projects root. (`E_FLEET_PROJECTS_BASE`) |
| Session **won't attach** / appears as a ghost | The underlying tmux session has been destroyed | Use **Recreate** to replace the session, or **Dismiss** to remove the ghost entry. |
| **Every** session on one host turned into a ghost | The host rebooted or its tmux server restarted | See [tmux server restarted](#a-hosts-tmux-server-restarted-all-sessions-become-ghosts). Recreate before the next pass removes the rows. |
| Hosts **offline** right after the laptop wakes | SSH ControlMasters went stale during sleep | Wait one or two reconcile passes, or click **Re-probe**. See [after sleep / wake](#after-laptop-sleep--wake). |
| Sidebar looks **stale** | Cache-first `list_sessions` inside the reconcile interval | Click **Refresh** (forced pass). See [reconcile tick](#the-reconcile-tick-reconcileinterval_secs-and-refresh). |
| Need logs / reporting a bug | n/a | **Settings → Diagnostics → Copy diagnostics**; logs under `<app data>/logs/`. See [Logs](#logs-where-they-live-and-how-to-raise-verbosity). |
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

The project scanner expects repos at
`$CLAUDE_FLEET_PROJECTS_BASE/<owner>/<repo>`. The default base is
`~/projects/github.com`. If your repos live elsewhere, set the environment
variable before launching the app (e.g. add
`export CLAUDE_FLEET_PROJECTS_BASE=~/code` to your shell profile and relaunch).

### Session won't attach / ghost session

A "ghost" session is a database row whose tmux session no longer exists on the
remote host (the host was rebooted, tmux was killed, etc.). Use **Recreate** to
spawn a fresh tmux session in the same window, or **Dismiss** to remove the
database entry. If Recreate fails with `E_TMUX`, confirm tmux is still running
on the host with `ssh <alias> tmux ls`.

### Logs: where they live and how to raise verbosity

claude-fleet writes a daily-rotated log file into its app data directory and
keeps the newest five files:

| OS | Log folder |
|---|---|
| Linux | `~/.local/share/claude-fleet/logs/` |
| macOS | `~/Library/Application Support/sk.rlt.claude-fleet/logs/` |

Files are named `claude-fleet.YYYY-MM-DD.log`. **Settings → Diagnostics → Open
log folder** opens the folder (the path is also shown there, with a copy
button). Bearer tokens, `?token=` query values and 64-character hex strings
are masked as `[REDACTED]` before a line is written.

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
or `launchctl setenv RUST_LOG debug` and relaunch. Debug builds (`pnpm tauri
dev`) also log to stderr; set `CLAUDE_FLEET_LOG_STDERR=1` to get that in a
release build.

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
- **Refresh (forced):** the sidebar **Refresh** button (and `list_sessions`
  with `force: true` over MCP) always starts a pass, ignoring freshness. If a
  pass is already running it waits for that one instead of starting another.

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
