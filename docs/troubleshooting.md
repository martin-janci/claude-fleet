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
| **MCP bind error** — server enabled but not listening | Port 4180 (or configured port) already in use | Change the port in **Settings → Control API**. The `bind_error` field in `McpStatus` shows the exact OS error. |
| **No projects found** | Scan path is empty or the base directory does not exist | Place repos under `~/projects/github.com/<owner>/<repo>`, or set `CLAUDE_FLEET_PROJECTS_BASE` to your projects root. (`E_FLEET_PROJECTS_BASE`) |
| Session **won't attach** / appears as a ghost | The underlying tmux session has been destroyed | Use **Recreate** to replace the session, or **Dismiss** to remove the ghost entry. |
| *(Developers)* `localStorage is undefined` in frontend tests | Pre-existing test-environment limitation noted in `CLAUDE.md` | This is not caused by app code. Run `npx vitest run` and compare failures against `main` before attributing them to a change. |

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

---

## See also

- [Getting started](getting-started.md)
- [Concepts](concepts.md)
