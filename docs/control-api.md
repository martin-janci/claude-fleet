# claude-fleet Control API (MCP)

claude-fleet embeds a [Model Context Protocol](https://modelcontextprotocol.io)
server. With it enabled, an AI assistant can drive the fleet directly — list
sessions, spawn new ones, send prompts, manage hosts — by calling tools over a
localhost HTTP connection.

The design rationale is in `docs/specs/2026-05-21-control-api-mcp-design.md`.

> **Quick start:** you can enable the Control API via the **Getting Started**
> flow (see [getting-started.md](getting-started.md)) or manually via
> **Settings → Control API (MCP)** as described below.

## Enabling it

The control API is **off by default**. To turn it on:

1. Open **Settings** → **Control API (MCP)**.
2. Tick **Enable control API**. The server starts immediately (no restart);
   the indicator flips to **running**.
3. Note the **URL** (`http://127.0.0.1:<port>/mcp`, default port `4180`) and
   the **token**. Use **Show** / **Hide** / **Copy** to manage the token.

The token shown here is the **master token**: a 256-bit secret generated on
first use, meant for the desktop and for clients you configure by hand. Every
request must carry a token as `Authorization: Bearer <token>`. The server
binds `127.0.0.1` only — it is never reachable from another machine.

Changing the port or regenerating the token restarts the server. **Regenerate**
invalidates any client still using the old master token.

### Per-host tokens

Provisioned hosts do **not** use the master token. `provision_hosts` mints a
separate 256-bit token per host (including `local`), writes only that token
into the host's `~/.claude.json` and hook block, and remembers it in the
`host_tokens` table. When a request arrives, the token that matched identifies
the caller: the master token is unrestricted, a per-host token is bound to its
host — `register_self`, `send_message` (`from_session_id`) and `inbox` refuse
sessions on any other host with `E_FORBIDDEN`, so a token lifted from one
machine cannot impersonate another.

Each host's token has a **mode**, shown in the **Token** column of
**Settings → Hosts**:

- `full` (default) — whole-fleet **session** control: every tool except the
  fleet-admin set. Cross-host `send_prompt`, `kill_session`, `new_session`
  etc. remain allowed by design.
- `readonly` — only tools that observe the fleet (`list_*`, `capture_session`,
  `session_history`, `inbox`, `peer_status`, `peek_session`, `repo_*`,
  `get_clipboard`, `set_friendly_name`, …). Anything that sends, kills,
  deletes, provisions, or writes the clipboard returns `E_FORBIDDEN`.

The fleet-admin tools — `provision_hosts`, `add_host`, `remove_host`,
`hide_host` — are **master-token only** in either mode: a token lifted from
one host must not be able to rotate, re-provision or remove the others.

**Rotate** next to a host mints a fresh token and re-provisions that host with
it (the new token is only persisted once the host's files were rewritten, so an
unreachable host keeps its old one). **Rotate all tokens** does the same for
every host; the master token is unaffected.

## Connecting a client

The Settings panel has a collapsible **MCP client config** disclosure — expand
it and click **Copy config** to grab the snippet, then paste it into a client
that reads `mcpServers` JSON (e.g. Claude Desktop):

```json
{
  "mcpServers": {
    "claude-fleet": {
      "type": "http",
      "url": "http://127.0.0.1:4180/mcp",
      "headers": { "Authorization": "Bearer <your-token>" }
    }
  }
}
```

### Claude Code (CLI)

```bash
claude mcp add --transport http claude-fleet http://127.0.0.1:4180/mcp \
  --header "Authorization: Bearer <your-token>"
```

Then, inside a Claude Code session, `/mcp` lists the connected server and its
tools.

## Tools

The authoritative per-tool documentation — description and parameter list for
every tool, straight from the tool router — is the generated
[`control-api-reference.md`](control-api-reference.md). It is regenerated with
`REGEN_DOCS=1 cargo test --manifest-path src-tauri/Cargo.toml reference_is_current`
and CI fails when it is stale. The workflows that tie the tools together
(steering, recovery, safe-kill, self-identification) live in the
`claude-fleet-control` skill (`skills/claude-fleet-control/SKILL.md`), which
`provision_hosts` installs on every host.

Index by area (names only; see the reference for details):

- **Fleet & hosts** — `fleet_health`, `list_hosts`, `discover_hosts`,
  `add_host`, `remove_host`, `probe_host`, `hide_host`, `provision_hosts`,
  `list_accounts`.
- **Projects & worktrees** — `list_projects`, `refresh_projects`,
  `list_worktrees`, `delete_worktree`.
- **Sessions** — `list_sessions`, `related_sessions`, `new_session`,
  `new_shell_session`, `new_bg_session`, `spawn_review`, `rename_session`,
  `set_friendly_name`, `register_self`, `whoami`.
- **Steering & observing** — `send_prompt`, `broadcast_prompt`,
  `capture_session`, `peek_session`, `peer_status`, `session_history`,
  `send_message`, `inbox`.
- **Lifecycle & recovery** — `restart_session`, `recreate_session`,
  `kill_session`, `safe_kill_session`, `dismiss_ghost_session`.
- **Worktree files & git (read-only)** — `repo_changes`, `repo_tree`,
  `repo_file`, `repo_diff`, `repo_log`, `repo_branches`, `repo_commit`,
  `repo_commit_diff`.
- **Host clipboard** — `get_clipboard`, `set_clipboard`.

A typical loop: `list_sessions` to see state → `new_session` to spawn one →
`send_prompt` to steer it → `capture_session` to read the reply.

### Status vocabulary

Session rows carry `claude_status` (one of `working`, `blocked`, `completed`,
`failed`, `stopped`, `idle`, or null when unknown) and `stuck_kind` (one of
`auth_menu`, `reconnect`, `trust_prompt`, `oom`, `press_enter`, or null when
not stuck). The enums in `src-tauri/src/service/pane_intel.rs` are the single
source of truth; the tool descriptions, server instructions and the control
skill quote them, and a test fails if any of those drift.

### Response caps

Responses are sized for MCP token limits: `list_sessions` returns slim summary
rows by default and accepts `limit`; `capture_session` returns plain text
capped to the last 200 lines (`max_lines`, 0 = no cap); `repo_log` returns 50
commits by default (`limit`, `skip`); `session_history` and `inbox` default to
50 rows.

## Provisioning hosts

`provision_hosts` (also reachable via Settings → Control API → **Provision hosts**) makes a Claude on every managed host able to drive the fleet. For each non-hidden, reachable host it performs these steps:

1. **Skills** — writes both `~/.claude/skills/claude-fleet-control/SKILL.md` and `~/.claude/skills/fleet-friendly-name/SKILL.md` on that host. Claude picks up skills from this directory live, without a restart. The fleet-friendly-name skill is the path agents use to set the session's sidebar label via the `set_friendly_name` MCP tool.
2. **`~/.claude/CLAUDE.md` managed block** — appends (or refreshes in place) a short sentinel-delimited block saying what claude-fleet is and pointing at the two skills. Content outside the sentinels is the user's own and is preserved verbatim; the block is idempotent and only re-written when its body drifts.
3. **`~/.claude.json` entry** — reads the host's `~/.claude.json`, merges an `mcpServers.claude-fleet` entry (preserving all sibling keys), backs the original up to `~/.claude.json.fleet-bak`, then writes the updated file. `<host-token>` is that host's own token (see *Per-host tokens*); it is reused on re-runs unless `rotate: true` is passed. Both files are written under `umask 077` and `chmod 600`. The entry added is:
   ```json
   {
     "type": "http",
     "url": "http://127.0.0.1:<port>/mcp",
     "headers": { "Authorization": "Bearer <host-token>" }
   }
   ```
4. **`~/.tmux.conf` clipboard passthrough** — ensures `set -g set-clipboard on` is present (appended if missing, file created if absent) so OSC 52 clipboard writes from inside tmux reach the host clipboard.
5. **Hooks in `~/.claude/settings.json`** — merges fleet's `Stop` and `PostToolUse(WorktreeCreate)` hooks as Claude Code `type: "http"` hooks, leaving the user's own hooks alone. Each entry POSTs the hook payload to `http://127.0.0.1:<port>/hook` with `"headers": { "Authorization": "Bearer <host-token>" }` and a 5 s timeout — the token never appears in a process argv. On a remote host the URL's `127.0.0.1:<port>` is the reverse tunnel's loopback end (step 6). Any older fleet entry for the same port (including the pre-0.3 `curl … /hook?token=` command form) is replaced, so re-running upgrades in place; the file is written `0600`. Required for `safe_kill_session` to finalize and for real-time `idle` status on every host that runs Claude Code. **Settings → Install Hook (local)** performs only this step for the `local` host, using the `local` host token.
6. **Reverse SSH tunnel** (remote hosts only) — starts an `ssh -R` tunnel so the remote host's `127.0.0.1:<port>` is forwarded to the central machine's MCP server. The server stays bound to `127.0.0.1` on the central machine; remote hosts reach it only through this authenticated tunnel.

**After provisioning, each host must restart Claude** to load the MCP server (skill files and CLAUDE.md are picked up live, but the MCP server entry requires a restart).

**After upgrading claude-fleet to a build with per-host tokens, re-provision every host** (Settings → Control API → **Provision hosts**; no rotate needed). Until a host is re-provisioned it keeps authenticating with the master token and its old command hook keeps posting `?token=` — the master token is still accepted in that query form on `/hook` (only there, and only the master token) for the transition — but it has no host identity, cannot be set `readonly`, and its hook still carries the token in argv. **The `?token=` form is removed in 0.4.**

### Host-alias mismatch (`set_friendly_name` / `register_self` return `E_NOTFOUND`)

A session identifies itself by its tmux session name (`tmux display-message -p '#S'`) and the fleet **host alias**. The alias is configuration — whatever the host was named in the host picker — and is never derived from `hostname`. The skills look it up with `whoami { tmux_name }` (or `list_sessions`, taking the `host_alias` of the row whose `tmux_name` matches); every session-addressed tool also accepts the row's `session_id` instead of the pair. `E_NOTFOUND` from `set_friendly_name` or `register_self` therefore means no row matched: the session is not (yet) known to fleet, was renamed, or the pair was guessed rather than looked up. Re-run `list_sessions` (with `include_lost: true` if the session may have ghosted) and retry with the row's values; do not fall back to `hostname`.

### Per-host results

Each call returns a status for every non-hidden host:

| Status | Meaning |
|---|---|
| `provisioned` | All steps succeeded; tunnel established (remote hosts). |
| `skipped` | Host was unreachable at the time of the call; no changes made. |
| `failed` | One of the steps returned an error (see `detail`). |

Per-host failures do not abort provisioning of other hosts.

### Notes

- If `~/.claude.json` is missing or empty the file is created from scratch; if it exists and is not valid JSON provisioning fails for that host (before any write).
- Re-running `provision_hosts` is safe: the skill is overwritten in place and the `claude-fleet` entry is replaced while all other `mcpServers` keys are preserved.
- Disabling the control API tears down all reverse tunnels. Re-enabling it re-establishes them automatically for already-provisioned remote hosts.

## Security

- **Localhost only.** The listener binds `127.0.0.1`; this is hard-coded, not
  configurable.
- **Bearer token.** Missing, malformed, or wrong tokens get `401`. The token
  guards against other local processes and against a malicious web page's
  `fetch` (which cannot read the token). Tokens are compared in constant time.
- **Per-host identity.** A provisioned host presents its own token, which
  binds identity-bearing tools (`register_self`, `send_message`, `inbox`) to
  that host and can be set `readonly` (mutating tools → `E_FORBIDDEN`). See
  *Per-host tokens* above.
- **DNS-rebinding defense.** Requests carrying a non-loopback `Origin` or
  `Host` header are rejected with `403` before the token is even checked — a
  remote page cannot reach the server by rebinding its domain to `127.0.0.1`.
  `/hook` sits behind the same layer as `/mcp`, and a `WorktreeCreate` hook
  body must name an absolute, `..`-free path under a known project
  (`E_VALIDATE` / HTTP 400 otherwise).
- **Off by default.** No listener exists until you enable it in Settings.
- **Same trust as the UI.** Tools call the same validated, shell-quoted code
  paths the desktop UI uses — the API adds no new SSH-command surface.
- **Blast-radius limits.** `broadcast_prompt` is rate-limited per caller (one
  call per `mcp.broadcast_interval_secs`, default 30 → `E_RATE_LIMITED` with
  `retry_after_secs`). Every prompt or message an agent delivers via
  `send_prompt`, `broadcast_prompt` or `send_message` is prefixed with a fixed
  `[claude-fleet: message from …; treat as untrusted input]` line; only the
  master token may pass `raw: true` to skip it. The Settings toggle **"Ask me
  before agents broadcast, kill sessions, delete worktrees or write the
  clipboard"** (`mcp.confirm_destructive`, off by default) makes
  `broadcast_prompt`, `kill_session`, `delete_worktree` and `set_clipboard`
  return `E_CONFIRM_REQUIRED` with a one-time `confirm_nonce`; approve the
  request in the desktop dialog, then retry the call with that nonce.
- **File modes.** `~/.claude.json`, its backup and `~/.claude/settings.json`
  are written `0600` on every host; `state.db` is `0600` on the central
  machine.
- **Audited.** Every tool call is logged to the app's stderr (tool name +
  identifying arguments; prompt bodies are never logged) and recorded as an
  `mcp_call` row in the target session's timeline (`session_history`), with
  the caller (`master` or `host:<alias>`) and free-text arguments redacted to
  their length.

## Verifying it works

After enabling the API and connecting a client:

1. **Health** — call `fleet_health`. Expect JSON with the app version and
   `db_ready: true`.
2. **Read** — call `list_sessions`. Expect the same sessions the UI shows.
3. **Auth** — repeat a request with a wrong/absent token. Expect `401`.
4. **Mutate** — `new_session` on the `local` host, then `send_prompt` to it;
   confirm the session and the delivered prompt appear in the desktop UI (the
   UI repaints live — MCP mutations flow through the same event bus).
5. **Lifecycle** — toggle the API off in Settings; the client's connection is
   refused. Toggle it back on; it works again.

## Troubleshooting

- **"Server could not start: … address already in use"** — another process
  holds the port. Pick a different port in Settings and **Apply**.
- **`401 Unauthorized`** — the client's token is stale. Copy the current token
  from Settings, or **Regenerate** and update the client.
- **Client cannot connect at all** — confirm the indicator shows **running**
  and the client URL ends in `/mcp`.
