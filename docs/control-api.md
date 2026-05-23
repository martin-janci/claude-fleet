# claude-fleet Control API (MCP)

claude-fleet embeds a [Model Context Protocol](https://modelcontextprotocol.io)
server. With it enabled, an AI assistant can drive the fleet directly — list
sessions, spawn new ones, send prompts, manage hosts — by calling tools over a
localhost HTTP connection.

The design rationale is in `docs/specs/2026-05-21-control-api-mcp-design.md`.

## Enabling it

The control API is **off by default**. To turn it on:

1. Open **Settings** → **Control API (MCP)**.
2. Tick **Enable control API**. The server starts immediately (no restart);
   the indicator flips to **running**.
3. Note the **URL** (`http://127.0.0.1:<port>/mcp`, default port `4180`) and
   the **token**. Use **Show** / **Copy** to read the token.

The token is a 256-bit secret generated on first use. Every request must carry
it as `Authorization: Bearer <token>`. The server binds `127.0.0.1` only — it
is never reachable from another machine.

Changing the port or regenerating the token restarts the server. **Regenerate**
invalidates any client still using the old token.

## Connecting a client

The Settings panel has a **MCP client config** block — copy it straight into a
client that reads `mcpServers` JSON (e.g. Claude Desktop):

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

| Tool | What it does |
|---|---|
| `fleet_health` | App version, schema version, DB readiness. |
| `list_hosts` / `discover_hosts` | Registered hosts; SSH-config candidates. |
| `add_host` / `remove_host` / `probe_host` / `hide_host` | Host management. |
| `list_accounts` | Cached Claude accounts across hosts. |
| `list_projects` / `refresh_projects` | Projects + worktrees; rescan. |
| `list_sessions` | Reconcile + list all tmux sessions (primary fleet view). |
| `related_sessions` | Sessions sharing a project + worktree. |
| `new_session` / `kill_session` / `rename_session` / `restart_session` | Session lifecycle. |
| `send_prompt` | Deliver a prompt to a running session's Claude REPL. |
| `spawn_review` | Spawn a review session in another session's worktree. |

**Read session output**

| Tool | What it does |
|---|---|
| `capture_session` | Capture a session's terminal output — the visible tmux pane, or include scrollback history. |
| `peek_session` | Peek at a session's background Claude logs. |

**Session lifecycle**

| Tool | What it does |
|---|---|
| `recreate_session` | Recreate a session: kill its tmux session and rebuild it fresh in the same worktree, resuming the same Claude conversation. |
| `dismiss_ghost_session` | Dismiss a ghost session (lost from tmux): permanently delete its row. |
| `new_bg_session` | Launch a supervised headless (background) Claude session on a host with an initial prompt. |

**Peer-to-peer messaging**

| Tool | What it does |
|---|---|
| `send_message` | Send a message from one session's id to another's. Persisted to the recipient's inbox; set `deliver: true` to ALSO type it into the recipient's pane with a `[msg #id from name@host]:` header. |
| `inbox` | Read the caller's inbox. Returns messages addressed to `session_id`, newest-first; `unread_only` filters and `mark_read` (default true) consumes them. |
| `peer_status` | What is a peer doing right now? Returns its `claude_status`, `current_activity`, `stuck_kind`, and `context_pct` — no need to capture and parse the pane. |

**Files & git (read-only)**

| Tool | What it does |
|---|---|
| `repo_changes` | List a session's changed files (git status) in its worktree. |
| `repo_tree` | List a session's worktree files (tracked + untracked, gitignore respected). |
| `repo_file` | Read one worktree file's contents (capped). |
| `repo_diff` | Unified diff for one worktree file vs HEAD (untracked files render as all-added). |
| `repo_log` | Commit log (branch graph) for a session's worktree. |
| `repo_branches` | List local + remote branches for a session's worktree with ahead/behind. |
| `repo_commit` | One commit's metadata + changed files. |
| `repo_commit_diff` | Diff of one file within a commit. |

**Host provisioning**

| Tool | What it does |
|---|---|
| `provision_hosts` | Install the fleet-control skill + MCP server entry on every reachable host and start reverse SSH tunnels for remote hosts. |

A typical loop: `list_sessions` to see state → `new_session` to spawn one →
`send_prompt` to steer it.

## Provisioning hosts

`provision_hosts` (also reachable via Settings → Control API → **Provision hosts**) makes a Claude on every managed host able to drive the fleet. For each non-hidden, reachable host it performs four steps:

1. **Skills** — writes both `~/.claude/skills/claude-fleet-control/SKILL.md` and `~/.claude/skills/fleet-friendly-name/SKILL.md` on that host. Claude picks up skills from this directory live, without a restart. The fleet-friendly-name skill is the path agents use to set the session's sidebar label via the `set_friendly_name` MCP tool.
2. **`~/.claude/CLAUDE.md` managed block** — appends (or refreshes in place) a sentinel-delimited block telling Claude to invoke fleet-friendly-name at every task start. Content outside the sentinels is the user's own and is preserved verbatim; the block is idempotent and only re-written when its body drifts.
3. **`~/.claude.json` entry** — reads the host's `~/.claude.json`, merges an `mcpServers.claude-fleet` entry (preserving all sibling keys), backs the original up to `~/.claude.json.fleet-bak`, then writes the updated file. The entry added is:
   ```json
   {
     "type": "http",
     "url": "http://127.0.0.1:<port>/mcp",
     "headers": { "Authorization": "Bearer <token>" }
   }
   ```
4. **Reverse SSH tunnel** (remote hosts only) — starts an `ssh -R` tunnel so the remote host's `127.0.0.1:<port>` is forwarded to the central machine's MCP server. The server stays bound to `127.0.0.1` on the central machine; remote hosts reach it only through this authenticated tunnel.

**After provisioning, each host must restart Claude** to load the MCP server (skill files and CLAUDE.md are picked up live, but the MCP server entry requires a restart).

### Host-alias mismatch (set_friendly_name returns `E_NOTFOUND`)

The fleet-friendly-name skill discovers its session identity by running `tmux display-message -p '#S'` and `hostname -s` in the tmux session, then calls `set_friendly_name` with that `(tmux_name, host_alias)` pair. If both `hostname -s` and the full `hostname` return `E_NOTFOUND`, the claude-fleet alias for this machine does not match either value. To fix it:

1. Open the claude-fleet app and find the offending row in **Settings → Hosts** (or the host picker).
2. Either rename the host (the alias) to match `hostname -s` on that machine, or change the machine's hostname to match the alias. Aliases are arbitrary identifiers — pick whatever is least disruptive.
3. The skill stops after the second `E_NOTFOUND` and emits a short notice to the user; it never retries blindly. Once the alias is fixed, the next task pickup on that host succeeds without further action.

### Per-host results

Each call returns a status for every non-hidden host:

| Status | Meaning |
|---|---|
| `provisioned` | All three steps succeeded; tunnel established (remote hosts). |
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
  `fetch` (which cannot read the token).
- **DNS-rebinding defense.** Requests carrying a non-loopback `Origin` or
  `Host` header are rejected with `403` before the token is even checked — a
  remote page cannot reach the server by rebinding its domain to `127.0.0.1`.
- **Off by default.** No listener exists until you enable it in Settings.
- **Same trust as the UI.** Tools call the same validated, shell-quoted code
  paths the desktop UI uses — the API adds no new SSH-command surface.
- **Audited.** Every tool call is logged to the app's stderr (tool name +
  identifying arguments; prompt bodies are never logged).

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
