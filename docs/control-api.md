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

On a `fleet-hub` daemon the API is always on and reachable at the hub's
public URL; see [`hub.md`](hub.md).

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

Each host's token has a **mode**, shown and changed under **Integration** in
the host's detail in the **Hosts** view (⌘I):

- `full` (default) — whole-fleet **session** control: every tool except the
  fleet-admin set. Cross-host `send_prompt`, `kill_session`, `new_session`
  etc. remain allowed by design.
- `readonly` — only tools that observe the fleet (`list_*`, `capture_session`,
  `session_history`, `inbox`, `peer_status`, `session_transcript`,
  `peek_session` (deprecated), `repo_*`, `get_clipboard`, `wait_for_session`,
  `wait_for_task`, `list_tasks`, …). Anything that sends, kills, deletes,
  provisions, dispatches, writes the clipboard, or writes a session row
  (including `set_friendly_name`, so an agent on a `readonly` host cannot
  set its sidebar label) returns `E_FORBIDDEN`.

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

The server speaks MCP **2025-11-25** over **stateless streamable HTTP**: every
`POST /mcp` is a self-contained JSON-RPC exchange, no `Mcp-Session-Id` is
issued or required, and `GET /mcp` is not served (`405`). An app restart, a
port or token change, or a reverse-tunnel bounce therefore needs no reconnect
on the client side — the next call simply works. Responses are SSE-framed so a
long poll keeps receiving a keep-alive every 15 s.

## Tools

The authoritative per-tool documentation — description and parameter list for
every tool, straight from the tool router — is the generated
[`control-api-reference.md`](control-api-reference.md). It is regenerated with
`REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`
and CI fails when it is stale. The workflows that tie the tools together
(steering, recovery, safe-kill, self-identification) live in the
`claude-fleet-control` skill (`skills/claude-fleet-control/SKILL.md`), which
`provision_hosts` installs on every host.

Index by area (names only; see the reference for details):

- **Fleet & hosts** — `fleet_health`, `usage_report` (estimated token
  usage and cost per session, host and day), `list_hosts`, `discover_hosts`,
  `add_host`, `remove_host`, `probe_host`, `hide_host`, `provision_hosts`,
  `list_accounts`.
- **Projects & worktrees** — `list_projects`, `refresh_projects`,
  `list_worktrees`, `delete_worktree`.
- **Sessions** — `list_sessions`, `related_sessions`, `new_session`,
  `new_shell_session`, `new_bg_session`, `spawn_review`, `rename_session`,
  `set_friendly_name`, `register_self`, `whoami`.
- **Steering & observing** — `send_prompt`, `broadcast_prompt`,
  `capture_session`, `session_transcript` (the conversation of any session,
  including pane-less `bg:<uuid>` rows — track background runs with it),
  `peek_session` (`peek_session` is deprecated: use `session_transcript`),
  `peer_status`, `session_history`, `send_message`, `inbox`. Rows with
  `kind: external` are interactive Claude sessions running outside tmux:
  fleet can read them (`session_transcript`) but not control them.
- **Lifecycle & recovery** — `restart_session`, `recreate_session`,
  `repair_session` (explicit repair, same as the Repair workspace button:
  may unregister this worktree's stale entry, adopt a moved checkout,
  recreate the branch and respawn the pane; behind the desktop confirmation
  when `mcp.confirm_destructive` is on), `kill_session`, `safe_kill_session`,
  `dismiss_ghost_session`, `move_session` (continue a work session on another
  host: clean + pushed worktree required, transcript copied, `--resume` on
  the target, source killed once the target runs; master token only, since
  the caller must be allowed on both hosts).
- **Worktree files & git (read-only)** — `repo_changes`, `repo_tree`,
  `repo_file`, `repo_diff`, `repo_log`, `repo_branches`, `repo_commit`,
  `repo_commit_diff`.
- **Host clipboard** — `get_clipboard`, `set_clipboard`.
- **Asset catalog** — `list_assets` (catalog assets with per-host drift
  state, unmanaged assets and parse problems), `scan_assets` (re-scan hosts,
  read-only on the hosts, and recompute asset states), `import_assets`
  (import the controller's `~/.claude` into the catalog working tree;
  `dry_run` supported), `plan_sync` (compute a fleet-wide sync plan with
  per-host actions; mutating classification to update inventory), `apply_sync`
  (apply a `plan_sync` plan; master token only, behind desktop confirmation
  when `mcp.confirm_destructive` is enabled, and needs a `confirm_nonce` for
  ANY apply — not only one whose plan includes an overwrite or remove —
  while the setting is on), `set_secret` (store a `${NAME}`
  placeholder value; master token only; the value is never returned, logged, or
  audited). Authoring (create, edit, delete assets) is desktop-only via the
  Assets tab, which auto-commits every save; no MCP tools. Sessions may edit
  the catalog repo directly and commit with `catalog:` prefixed messages,
  which the app picks up on its next catalog load.
- **Orchestration** — `wait_for_session`, `session_transcript`,
  `session_conversation`, `run_prompt`, `dispatch_task`, `wait_for_task`,
  `list_tasks`, `cancel_task`, `set_session_tags`.
- **Paired clients** — `pair_client` (mint a single-use pairing code and the
  URL to show as a QR; master token only), `list_clients` (the paired devices
  and what each one's token may do — the stored token digest is never
  returned), `revoke_client` (revoke one by name; master token only). A
  paired client is never the master, so every tool in this group — like the
  rest of fleet admin — stays out of a phone's reach. The `fleet-hub pair`,
  `fleet-hub client list` and `fleet-hub client revoke` commands are thin
  wrappers around these three.

A typical loop: `list_sessions` to see state → `new_session` to spawn one →
`run_prompt` to steer it and get the reply back (or `send_prompt` →
`wait_for_session` → `session_transcript` step by step; `capture_session`
for the raw screen).

### Status vocabulary

### Errors and limits

A tool that fails in fleet's service layer answers with a **tool result**
carrying `isError: true` (what the MCP spec prescribes for tool-execution
failures, so the model sees it as the tool's output and can correct the call),
not a JSON-RPC error. The text block is the documented `E_CODE: message` line;
`structuredContent` carries `{ code, message, details }` (for example the
candidate rows of `E_AMBIGUOUS` or the `confirm_nonce` of
`E_CONFIRM_REQUIRED`). JSON-RPC errors are reserved for protocol failures:
an unknown tool name or arguments that do not match the schema.

Every call runs under a wall clock: 60 s for reads and single round trips,
300 s for session lifecycle, provisioning and host probes, 660 s for the
self-bounded long polls (`wait_for_session`, `wait_for_task`, `run_prompt`,
whose own `timeout_s` maxes at 600). On elapse the result is
`E_TIMEOUT: <tool> exceeded its <n> s limit; the call may have partially
completed` with `structuredContent { code: "E_TIMEOUT", tool, limit_secs }`.

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
commits by default (`limit`, `skip`); `session_history`, `inbox` and
`list_tasks` default to 50 rows; `session_transcript` / `run_prompt` return at
most `max_chars` characters (default 8000, max 64000).

### Orchestration

**Completion signal.** Every session row carries `turn_seq` (completed turns)
and `last_stop_at`. Two Claude Code hooks maintain them: `Stop` marks the
session `idle`, bumps `turn_seq` and stamps `last_stop_at`; `UserPromptSubmit`
marks it `working`, so "idle because never started" and "idle after a turn"
are distinguishable from "busy". A turn that ends in an API error fires
`StopFailure` instead of `Stop`; fleet ends the turn the same way (idle,
`turn_seq` bump) and records a `stop_failure` timeline event with the error
type, so `run_prompt` / `wait_for_session` return and `session_history` shows
why. A hook-stamped status that is newer than a reconcile pass's pane
observation is never overwritten by the pane heuristic.
`send_prompt` returns `{ delivered, session_id, turn_seq_before }`;
`wait_for_session { session_id, until: "idle" | "turn_gt", turn?, timeout_s? }`
is a bounded long-poll (500 ms polls, default 120 s, max 600 s) returning
`{ status: satisfied | timeout, claude_status, turn_seq, last_stop_at,
stuck_kind }`. Each caller may hold at most 8 concurrent bounded waits (`wait_for_session`, `wait_for_task`, `run_prompt`); a ninth returns `E_RATE_LIMITED`. Sessions on hosts provisioned before this hook set exist keep
working through reconcile alone; re-provision to get the `UserPromptSubmit`
hook (see *Provisioning hosts*).

**Transcript.** `session_transcript { session_id, since_turn?, max_chars? }`
reads the session's Claude Code JSONL transcript
(`~/.claude/projects/<cwd with every non-alphanumeric char replaced by
"-">/<claude_session_id>.jsonl`) on its host and returns the last assistant
turn (or every turn after `since_turn`) as plain text: text blocks verbatim,
one `[tool_use] Name(...)` line per tool call, no thinking. The file is found through the `transcript_path` Claude Code reports in every hook when fleet has one, else under the session's physical cwd (symlinks resolved on the host with `pwd -P`), else by the session id under `~/.claude/projects/*/` (which also covers Claude truncating encoded directory names longer than 200 characters). `E_INVALID_STATE`
when the row has no `claude_session_id` yet, `E_NO_TRANSCRIPT` when the file
does not exist. `session_conversation { session_id, turns? }` reads the same
transcript but returns it as structured turns — `{ turns: [{ prompt, at,
ended_at, items: [{ kind: "text", text } | { kind: "tool", summary, error }] }],
truncated }` — the same shape the desktop's Conversation tab renders, for a
client that wants the exchange's structure rather than one flat blob.
`turns` defaults to 10 and is capped at 100; the character budget scales with
it. Same errors as `session_transcript`. `run_prompt { session_id, prompt, timeout_s?, max_chars?,
raw? }` composes the three: deliver, wait for `turn_seq` to grow, return
`{ turn_seq, status, transcript }`. It refuses (`E_INVALID_STATE`) a session that is not between turns (`claude_status` idle, completed or stopped): mid-turn, the previous turn's `Stop` would satisfy the wait and return the old reply.

**Tasks.** `dispatch_task { worker_session_id | new_worker { host_alias,
project_id, name? }, prompt, requester_session_id?, raw? }` creates a task
row (states `queued → running → done | failed | cancelled`), spawns the worker
when asked (recording `requester_session_id` as the worker's
`parent_session_id`), and delivers the prompt with an appended instruction:
*"When finished, print exactly `FLEET_TASK_DONE_<nonce>` on its own line
followed by a one-paragraph result."* The nonce is per task and never sent to
the UI. On the worker's next `Stop` fleet reads its last transcript turn (pane
capture as fallback), looks for the marker on its own line — which the prompt
echo, where it is followed by more text, never satisfies — and flips the task
to `done` with the paragraph as `result`; the result is also delivered to the
requester's inbox as `kind: task_result`. A `Stop` without the marker leaves
the task `running`. Open tasks are failed when their worker session is killed or lost, when it is recreated onto a new Claude conversation, or after `tasks.max_age_secs` (default 86400, `0` = off); the check runs on the reconcile tick and on every `list_tasks` / `wait_for_task` call. The fleet instruction is appended after an `[claude-fleet: end of untrusted input]` line, outside the marked prompt, and the result is prefixed with the untrusted-content marker wherever it is returned (inbox, `wait_for_task`, `list_tasks`). `wait_for_task { task_id, timeout_s? }` long-polls for a
terminal state; `list_tasks { requester_session_id?, state?, limit? }` lists;
`cancel_task { task_id }` marks a task cancelled (`E_TASK_TERMINAL` if it
already finished; the worker keeps running) and is confirm-gated like
`kill_session`. A per-host token only sees, waits on and cancels tasks it
requested from its host or whose worker is on its host (`E_FORBIDDEN`). The
desktop shows the same rows in the Tasks panel (per session in the details
pane, fleet-wide from the sidebar header).

**Threads and tags.** `send_message` accepts `reply_to` (an inbox message id
the sender took part in; `E_NOTFOUND` / `E_INVALID` otherwise) and `inbox`
rows carry it back. `set_session_tags { session_id, tags }` replaces a
session's labels (up to 16 of 1–32 chars from `[A-Za-z0-9_.:-]`) and
`list_sessions { tag }` filters on them; summary rows include `tags`.

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
   The URL is `http://127.0.0.1:<port>` on the desktop (the reverse tunnel's
   loopback end); on a `fleet-hub` daemon with a public URL configured it is
   that public URL instead (e.g. `https://fleet.example.com`), since every
   host can already reach it directly.
4. **`~/.tmux.conf` clipboard passthrough** — ensures `set -g set-clipboard on` is present (appended if missing, file created if absent) so OSC 52 clipboard writes from inside tmux reach the host clipboard.
5. **Hooks in `~/.claude/settings.json`** — merges fleet's `Stop`, `UserPromptSubmit`, `PostToolUse(EnterWorktree|ExitWorktree)`, `SessionEnd(logout|prompt_input_exit|other)`, `StopFailure` and `Notification(permission_prompt|elicitation_dialog|elicitation_url_dialog|quota_auto_resume_stale|quota_auto_resume_disabled|quota_auto_resume_fired)` hooks as Claude Code `type: "http"` hooks, leaving the user's own hooks alone. Each entry POSTs the hook payload to `http://127.0.0.1:<port>/hook` with `"headers": { "Authorization": "Bearer <host-token>" }` and a 5 s timeout — the token never appears in a process argv. On a remote host the URL's `127.0.0.1:<port>` is the reverse tunnel's loopback end (step 6); on a `fleet-hub` daemon with a public URL, the URL is that public URL's `/hook` instead (e.g. `https://fleet.example.com/hook`) and no tunnel is used. Any older fleet entry for the same port (including the pre-0.3 `curl … /hook?token=` command form) is replaced, so re-running upgrades in place; the file is written `0600`. Required for `safe_kill_session` to finalize, for real-time `idle` / `working` status, `turn_seq` and task completion on every host that runs Claude Code. **Settings → Install Hook (local)** performs only this step for the `local` host, using the `local` host token. **Hosts provisioned before the `UserPromptSubmit` hook existed must be re-provisioned** (no rotate needed) to get the busy signal; until then their status only flips to `working` on the next reconcile pass. **Hosts provisioned before the `SessionEnd` / `StopFailure` / `Notification` hooks existed must be re-provisioned** to get `stopped`, API-error turn completion and hook-driven `blocked` (see *Hook contract*).
6. **Reverse SSH tunnel** (remote hosts only, loopback hubs only) — starts an `ssh -R` tunnel so the remote host's `127.0.0.1:<port>` is forwarded to the central machine's MCP server. The server stays bound to `127.0.0.1` on the central machine; remote hosts reach it only through this authenticated tunnel. A `fleet-hub` daemon configured with a public URL skips this step entirely — every host already reaches the hub's public address directly.

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

## Hook contract

Claude Code on each host POSTs its hook events to `http://127.0.0.1:<port>/hook`
(the reverse tunnel's loopback end on a remote host) as `type: "http"` hooks with
`Authorization: Bearer <host-token>` and a 5 s timeout. The body is Claude Code's
hook input JSON; fleet reads only the fields below and ignores the rest. Answers:
`204` applied (or a no-op for an unknown session), `400` a body that fails
validation (`E_VALIDATE` / `E_INVALID`, e.g. a worktree path outside a project),
`403` a host token reporting about another host's session, `401` no valid token,
`500` a store failure. A non-2xx answer is a non-blocking error on the Claude side:
the session continues.

| Event | Matcher | Fields read | Effect on the session row | Timeline |
|---|---|---|---|---|
| `UserPromptSubmit` | all | `session_id`, `transcript_path` | `claude_status = working`, `idle_since` cleared | — |
| `Stop` | all | `session_id`, `transcript_path`, `cwd` | `idle`, `turn_seq + 1`, `last_stop_at`; triggers safe-kill and task-marker checks | — |
| `StopFailure` | all | `session_id`, `error`, `error_details` | same as `Stop` | `stop_failure` — `<error>[: <details>]` |
| `SessionEnd` | `logout\|prompt_input_exit\|other` | `session_id`, `reason` | `stopped`, `idle_since` started, stuck cleared | `session_end` — reason |
| `Notification` | `permission_prompt\|elicitation_dialog\|elicitation_url_dialog` | `session_id`, `notification_type` | `blocked` | `notification` — type |
| `Notification` | `quota_auto_resume_stale` | same | `blocked`, `stuck_kind = press_enter` | `notification` — type |
| `Notification` | `quota_auto_resume_disabled` | same | `blocked` | `notification` — type |
| `Notification` | `quota_auto_resume_fired` | same | `working`, stuck cleared | `notification` — type |
| `PostToolUse` | `EnterWorktree\|ExitWorktree` | `tool_name`, `tool_input`, `tool_response` | worktree row registered / removed | — |

Every hook write stamps `last_hook_at`; a reconcile pass that started before that
stamp never overwrites the hook's status with its pane heuristic. `SessionEnd`
with reason `clear` or `resume` is not installed (the process continues under a
new session id). `SessionStart` is not used: Claude Code accepts only `command` /
`mcp_tool` hooks there. Fleet never writes `allowedHttpHookUrls` — defining it
at user level would block every other http hook on the host.

## Security

- **Localhost only (desktop).** The desktop binds `127.0.0.1` and this is
  not configurable there. A `fleet-hub` daemon binds the configured address
  and adds its public host to the Host/Origin allowlist; plaintext on a
  routable bind is refused unless explicitly allowed. See
  [`hub.md`](hub.md) → *Configuration*.
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
  `/hook` sits behind the same layer as `/mcp`, and an `EnterWorktree` hook
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
  `broadcast_prompt`, `kill_session`, `delete_worktree`, `set_clipboard`,
  `repair_session`, `cancel_task` and `move_session` return `E_CONFIRM_REQUIRED` with a one-time `confirm_nonce`;
  approve the request in the desktop dialog, then retry the call with that
  nonce. The nonce is bound to the call's arguments — for `set_clipboard` and
  `broadcast_prompt` including a digest of the content / prompt — so an
  approval cannot be replayed with different text.
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
- **`E_TIMEOUT` from a lifecycle tool** — the host answered too slowly for
  the call's wall clock (see *Errors and limits*). Check the host with
  `probe_host`, then `list_sessions { force: true }`: a `new_session` that
  timed out may still have created the tmux session.
