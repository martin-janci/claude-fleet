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
machine cannot impersonate another. The same binding applies to the tools that
change a session (`recreate_session`, `dismiss_ghost_session`, …) and to the
pane / transcript reads (`capture_session`, `session_transcript`), checked
against the stored row's host.

Each host's token has a **mode**, shown and changed under **Integration** in
the host's detail in the **Hosts** view (⌘I):

- `full` (default) — **session** control on its own host: every tool except
  the fleet-admin set. Session-addressed tools (`send_prompt`,
  `kill_session`, `new_session`, …) refuse another host's sessions with
  `E_FORBIDDEN`; fleet-wide listings (`list_sessions`, …) still see every
  host.
- `readonly` — only tools that observe the fleet (`list_*`, `capture_session`,
  `session_history`, `session_conversations`, `inbox`, `peer_status`,
  `session_transcript`, `repo_*`,
  `get_clipboard`, `wait_for_session`, `wait_for_reply`, `wait_for_task`, `list_tasks`, …).
  Anything that sends, kills, deletes, provisions, dispatches, writes the
  clipboard, or writes a session row (including `set_friendly_name`, so an
  agent on a `readonly` host cannot set its sidebar label) returns
  `E_FORBIDDEN`.

Since multi-user M1 a host token's reach inside its own host is narrower
still: it addresses a session only when that session is `unclaimed`, or when
the request proves it is standing in that session's pane (the
`X-Fleet-Pane` header the provisioned MCP entry carries). Another person's
private session on the same machine answers `E_PANE_UNPROVEN` — its own
code, because the ordinary cause is an agent in a second split of the same
tmux window rather than an attack, and it says nothing about whose the
session is. A session on another host still answers `E_FORBIDDEN`, and one
outside the caller's reach entirely answers `E_NOTFOUND`, exactly as an id
that does not exist.

The fleet-admin tools — `provision_hosts`, `add_host`, `remove_host`,
`merge_host`, `hide_host`, `forget_project` — are **master-token only** in either mode: a token lifted from
one host must not be able to rotate, re-provision or remove the others.

**Rotate** next to a host mints a fresh token and re-provisions that host with
it (the new token is only persisted once the host's files were rewritten, so an
unreachable host keeps its old one). **Rotate all tokens** does the same for
every host; the master token is unaffected.

Settings → Control API lists every token in one **Control API tokens** table:
each host's (per-host), with when it was last **used** (the last request it
authenticated, stamped at most once a minute), when it was created and, once
rotated, when; and beside them the paired devices' tokens, with when each was
last seen. A host row has **Rotate**, a device row **Revoke** (as Settings →
Devices does; the device you are using has none). A rotation keeps the host's
created time, records the rotation and starts "used" over; stamping use never
invalidates the hub's token cache (`auth_epoch`), a rotation does.

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

The SSE mount advertises `tools.listChanged`. With no `GET /mcp` stream to
push on, `notifications/tools/list_changed` rides a `tools/call`'s own
response, one frame ahead of the result. A caller is told on its next call
when the tool list it may see differs from the one it last listed (or was
last told about), and on its first call after a fleet restart if it has not
listed since, because the new process cannot know what the client cached.
In practice that is a client left connected across a fleet upgrade: it
re-lists and sees the new tools without a manual reconnect. Each change is sent once per token, so two clients sharing
one token share one notice. Every token is told except a peer hub's, which
calls one tool; that includes a paired client whose mode flips between `full`
and `readonly`. `/mcp/json` neither advertises nor sends it, because its
answer is the handler's first message. A reader that takes the last `data:`
frame of an SSE body (as `wire::last_event_payload` does), or the first frame
carrying `result` or `error` (as fleet-mobile does), is unaffected.

### Endpoints

| Path | Auth | What it is |
|---|---|---|
| `POST /mcp` | bearer | The MCP transport. `GET` is not served (`405`). |
| `POST /hook` | bearer (per-host) | Claude Code's hook events — see *Hook contract*. |
| `GET /healthz` | none | Liveness: `fleet-hub ok`. Outside the Host allowlist. |
| `GET` / `POST /pair` | **none, by design** | The pairing exchange (below). Outside the Host allowlist. |
| `GET /events` | bearer | The row-change stream (below). |
| `GET /agent` | bearer (per-host, `full`, agent host) | The WebSocket a `fleet-agent` dials in on (below). |
| `POST /mcp/json` | bearer | The same tools with an unframed JSON body — see *`/mcp/json`* in [`hub.md`](hub.md). |
| `GET /metrics` | bearer | What each caller costs the hub — see *`/metrics`* in [`hub.md`](hub.md). |
| `POST /report`, `GET /reports` | bearer | Error reports from the hub's participants — see *Error reports* in [`hub.md`](hub.md). |
| `GET /downloads/<id>` | bearer | A file's bytes for the downloads tools (not a tool result). |
| `GET /voice/capture`, `GET /voice/source` | bearer | The voice relay (off by default; Settings → Limits → Voice). |
| `POST /update/check`, `POST /update/report` | bearer | The updater wire — see [`updates.md`](updates.md). |

### `/pair` — how a client gets its first credential

`pair_client` (master token only) mints a single-use **code** — eight
Crockford-base32 characters from the CSPRNG, held in the server's memory, not
in `state.db` — and returns
`{ url, code, expires_in_s, name, mode }`. The `url` is
`<public-url>/pair#<code>`: the code rides in the URL **fragment**, which a
browser never sends, so a QR of that URL can be photographed off a terminal
without a proxy or an access log ever seeing the secret. `GET /pair` is the
inert page a camera scan lands on — no JavaScript, no auto-redeem — and
`POST /pair` with `{"code":"…"}` is the exchange itself, answering
`{ hub, mode, name, token }` with `Cache-Control: no-store`. That response is
the only place the client token ever exists in plaintext; the store keeps its
SHA-256.

It is the one route besides `/healthz` that carries no bearer token — a
device being paired has none yet — and it is outside the `Host`/`Origin`
allowlist too, since a phone may reach the hub by a name nobody listed. What
stands in for the token is the code: single-use (a replay answers `404`),
minutes-long (`ttl_s`, clamped 30 s…1 h, default 10 min), compared in constant
time, voided wholesale by a restart, and rate-limited to **one attempt per
source address every six seconds**, an IPv6 address counting as its /64, and
to **thirty attempts a minute hub-wide** (`429` with `Retry-After` otherwise). The
address is the request's peer, or the last parseable `X-Forwarded-For` hop
when the peer is a loopback/private address — i.e. plausibly the reverse proxy
in front.

Nothing token-shaped is logged anywhere on this path: not the presented code,
not the minted token, not its hash.

The desktop app embeds the **same** router, so it serves `/pair` too, and that
route is not merely decorative there: `pair_client` is one of the shared tools,
so a desktop master token can mint a code and redeem it against the desktop's
own loopback `/pair`, creating a real row in the desktop's `client_tokens`.
That is not an escalation — minting is master-token-only and the desktop binds
`127.0.0.1`, so the exchange never leaves the machine — but it is worth saying
plainly rather than assuming the desktop's `/pair` can only answer `404`. The
desktop simply has no UI for it; client access is a `fleet-hub` feature, and
`fleet-hub pair` is how you are meant to reach it.

### `/events` — the row-change stream

`GET /events` is a server-sent-event stream of every row change the store
emits after the connection opened — the same
events the desktop UI repaints from. It sits **behind** the bearer token (a
change stream names sessions, hosts, projects and prompts, so it needs what
`/mcp` needs), and a client that is revoked mid-stream has its stream ended at
the next heartbeat.

- Each frame is `event: <name>` + `data: <json>` where `<name>` is the event
  (`session:created`, `session:updated`, `session:killed`, `host:probed`,
  `task:updated`, `account_usage:updated`, `asset_inventory:updated`,
  `catalog:loaded`, `sync:progress`, `move:progress`, `start:progress`, …) and the payload is
  the same JSON the desktop frontend receives.
- The first frame is `ready`, carrying `{ version, now, kinds }` — the kinds
  this stream will actually deliver.
- `?kinds=session,host` filters on the part of the name before the `:`. An
  unrecognised value is dropped and logged (and is absent from `ready`'s
  `kinds`), so a typo is visible rather than producing a silent stream.
- A comment line every 15 s keeps the connection alive through a phone's NAT,
  a reverse tunnel and any proxy in between.
- A caller may hold **8** concurrent streams (`429` beyond that, on its own
  budget — parking phone streams never costs an agent its `wait_for_session`
  slots), and a subscriber that falls more than 256 events behind gets one
  `lagged` frame and is disconnected: reconnect and re-list rather than
  assume continuity.
- The desktop app has no event source to stream from, so `/events` there
  answers `503 events are not enabled on this server`. The `fleet-hub` daemon
  serves it.

### `/agent` — where a `fleet-agent` dials in

`GET /agent` upgrades to the WebSocket a host's `fleet-agent` keeps open when
the hub cannot reach that host over SSH. Setup, rotation and the operator
side are in `hub.md` → *A host that cannot be reached*; this is the contract.

- **Who may connect.** The upgrade sits behind the same bearer check as
  `/mcp`, and then:
  - only a **per-host** token may connect; the master and paired clients get
    `403`;
  - its mode must be `full` (`403`, with the reason in the body);
  - its host must be on the `agent` transport (`403`);
  - at most 2 connections per host and 64 in all (`429`).

  The host alias comes from the token, never from the request.
- **Staying connected.** A live connection's token is re-checked against the
  store on every heartbeat and before every call routed to it. Rotating the
  token, narrowing it to `readonly`, removing the host or moving it back to
  SSH ends the connection. A second connection for the same host replaces
  the first.
- **Frames.** Each frame is one JSON object in a WebSocket text message, one
  request and one response per `id`. The hub sends `exec`, `upload`, `cancel`
  and `ping`; the agent sends `hello` (first), `result` and `pong`. The table
  is in `docs/superpowers/specs/2026-09-18-host-agent-design.md`, and
  `crates/fleet-proto` is the one definition both sides compile.
- **Heartbeat.** The hub pings every 30 s and drops a connection that misses
  two beats in a row.
- **Size.** One frame is at most about 267 MiB (a 200 MiB transcript after
  base64). Each answer is decoded against the budget of the requests in
  flight, which is the full ceiling whenever an uncapped call is among them.
- **Not enabled.** On the desktop, which routes nothing to agents, `/agent`
  answers `503`.

Tools see an agent host through the same calls as an SSH host. `agent_status`
reports which agent hosts are connected. `add_host { transport: "agent" }`
registers one without an SSH probe. `install_agent` does the whole *Set it
up* of `docs/hub.md` for a host the hub already reaches over SSH.

## Tools

The authoritative per-tool documentation — description and parameter list for
every tool, straight from the tool router — is the generated
[`control-api-reference.md`](control-api-reference.md). It is regenerated with
`REGEN_DOCS=1 cargo fleet-test -- reference_is_current`
and CI fails when it is stale. The workflows that tie the tools together
(steering, recovery, safe-kill, self-identification) live in the
`claude-fleet-control` skill (`skills/claude-fleet-control/SKILL.md`), which
`provision_hosts` installs on every host.

Index by area (names only; see the reference for details):

- **Fleet & hosts** — `fleet_health` (with `trackers`: each tracker's sync
  health and the detection backlog, from cached sync state; a per-host token
  sees its own org's trackers; and `org_budgets`, the orgs at or over a
  daily or monthly budget, for a caller that sees every session),
  `usage_report` (estimated token
  usage and cost per session, host and day), `list_hosts`, `discover_hosts`,
  `add_host`, `remove_host`, `merge_host` (fold a renamed alias into another),
  `probe_host`, `hide_host`, `provision_hosts`,
  `list_accounts`, `account_usage` (each account's latest plan usage: the
  5-hour and weekly windows with their reset times, status and when it was
  fetched, as the hub's usage poll last answered; never fetches; hub
  contract 11), `check_account_headroom` (whether starting or switching a
  session on a host under a login crosses `accounts.pause_at`, and the
  login there with the most headroom; reads the same usage, never fetches;
  `new_session` asks the same question itself: a Claude start on a login at
  or past the line fails `E_ACCOUNT_LIMIT`, naming the account and the
  login with headroom, unless `over_limit_ok: true` says the person chose
  it; `agent: "agy"` fails `E_UNSUPPORTED` until its adapter is validated),
  `agent_status` (which agent hosts have a `fleet-agent`
  connected; see *`/agent`* above), `install_agent` (install `fleet-agent`
  on a host the hub reaches over SSH and move the host onto it; a job read
  with `agent_installs`).
- **Projects & worktrees** — `list_projects`, `refresh_projects`,
  `forget_project` (drop a row a local-less hub cannot rescan away),
  `add_project` (clone, adopt or create a repository on a host — `git` and
  `gh` run there; a per-host token acts on its own host only, and
  `create_remote` — publishing on GitHub — needs a person's approval from
  any caller but a paired client, refused outright on a hub with no
  approver), `list_github_repos` (what `gh` on a host can see),
  `project_picks` / `set_project_pick` (a person's New session picker
  choices per project — pinned, hide or keep, group; a paired device only),
  `list_worktrees`, `list_host_worktrees` (one host scanned over SSH, for the
  worktrees fleet's own rows do not cover), `delete_worktree`.
- **Sessions** — `list_sessions`, `related_sessions`, `new_session`,
  `new_shell_session`, `new_bg_session`, `spawn_review`, `rename_session`,
  `set_friendly_name`, `touch_session_viewed` (the turns it has finished
  read as seen; drive reach), `shell_terminals` (list, open or close the
  session's shell terminals, tmux sessions `<name>--sh<N>` beside the agent
  that no session list shows; own reach), `register_self`, `whoami`, `ensure_operator` (the UX
  agent's own session, idempotent), `operator_status` (why it cannot work,
  if it cannot).
- **Sharing & ownership** (multi-user M1) — `session_share` (give one
  person `watch`, `answer` or `drive` on a session you own — or, with `org` instead of
  `person`, an org you are a member of: its members and admins as of now,
  never a later joiner or a viewer), `session_unshare` (take it
  back), `session_narrow` (`drive` or `answer` → `watch`; there is deliberately no tool
  that raises a grant — widen by revoking and sharing again),
  `session_access` (who holds a live grant on your session), `my_grants`
  (who *you* are on this fleet and every live grant to you, which is what a
  client derives access from together with each row's `owner_person_id` and
  `visibility`), and `session_claim` (give an `unclaimed` session to a
  person). The first five are a person's device and the master; a per-host
  token is refused all of them, because it proves no person. `session_claim`
  is the mirror image: a **per-host token only**, and only for the session
  whose active pane its `X-Fleet-Pane` header names — the operator's own
  claim is `fleet-hub session claim <id> --person <name>`, beside
  `fleet-hub session unclaimed`. Sharing never confers a terminal: a
  terminal is this machine's own SSH, which no revoke could reach.
  `answer` (Orbit Fleet 11.7) sits between the two: it reads like `watch`
  and may also answer the dialog on the pane — `send_prompt` with an empty
  `prompt` and one of `keys` Enter, Escape, Tab, Up, Down or a digit the
  dialog numbers, accepted only while a fresh read of the pane shows a
  dialog. Any prompt text, `C-c`, the Ctrl letters, Left, Right and BTab
  still need `drive`.
- **Presence** (redesign 11.7b) — `session_presence` (`session_id`,
  `leaving?`): say you have a session open, again every `heartbeat_secs`
  (20), and once with `leaving: true` when you close it; the answer is who
  has it open now, `{ person_id, name, device?, since, you? }` per device.
  Watch reach. The owner sees everyone; anyone else sees the owner and
  themselves, never another grantee. The hub keeps it in memory only, so a
  report lapses 45 s after its last heartbeat and a restart forgets it. A
  per-host token is refused it.
- **Composer** — `quick_replies` (the fleet's shared chip row: the prompt
  presets the desktop and the phone both draw above their text box, in list
  order, each with `auto_send`: a tap sends at once rather than filling the
  box — call it with no arguments to read, with `set` to replace the list; a
  `set` entry without `auto_send` keeps the stored chip's flag; `expected`,
  the list last read, turns a lost race into `E_CONFLICT`; `set` is refused
  to a per-host token and the operator).
- **Steering & observing** — `send_prompt`, `broadcast_prompt`,
  `queue_prompt` and `queued_prompts` (a prompt typed as a new turn once the
  session is idle; list or take back what still waits),
  `capture_session`, `session_transcript` (the conversation of any session,
  including pane-less `bg:<uuid>` rows — track background runs with it),
  `peer_status`, `session_history`, `session_conversations` (the Claude
  conversations a session has run — `/clear`, `/resume`, compaction; pass a
  `claude_session_id` from it to `session_conversation` to read an earlier
  one), `send_message`, `inbox`. Rows with
  `kind: external` are interactive Claude sessions running outside tmux:
  fleet can read them (`session_transcript`) but not control them.
- **Lifecycle & recovery** — `restart_session`, `rewind_conversation`
  (truncate a session's Claude transcript into a new conversation: mode
  `fork` starts a new session from that point — in this worktree, or with
  `new_worktree: "<name>"` in a new worktree and branch cut from the
  session's HEAD (uncommitted changes stay behind; an existing name is
  `E_CONFLICT`) — mode `rewind` restarts this one there; the original
  transcript is never changed), `recreate_session`,
  `repair_session` (explicit repair, same as the Repair workspace button:
  may unregister this worktree's stale entry, adopt a moved checkout,
  recreate the branch and respawn the pane; behind the desktop confirmation
  when `mcp.confirm_destructive` is on), `kill_session`, `safe_kill_session`,
  `dismiss_ghost_session`, `adopt_session` (a live tmux session fleet did
  not start, `started_at` null, becomes fleet's: `started_at` is set and the
  caller owns it when nobody did; the pane is untouched; `project_id` puts it
  in a project), `lost_target` (read-only: the project a Lost and found
  entry would go into, to prefill Adopt or Restore: a directory inside a
  project answers `rule`; otherwise Jev may propose one, `decide.jev.adopt_target`
  and `decide.jev.restore_target`, both off by default), `place_transcript`
  (Restore into a project: copies a found conversation under the directory
  Claude Code keys the project's root by, so `new_session { resume_claude_session_id }`
  resumes it there; never moves or overwrites a transcript), `move_session` (continue a work session on another
  host: the transcript is copied and the work travels as it is — unpushed
  commits, uncommitted files and small git-ignored ones, nothing pushed or
  committed for you (`strict: true` restores the old clean + pushed
  refusals) — then `--resume` on the target and the source killed once the
  target runs; master token only, since the caller must be allowed on both
  hosts; `dry_run: true` previews instead — writes nothing to either host,
  needs no confirmation, and answers what would travel plus `unknowns`, or
  the same refusal the real move would raise; the result is tagged
  `kind: "moved"` (the move report) or `kind: "preview"`; a live work link
  that the target host's org would put across the org boundary is
  `E_FORBIDDEN` with details `cross_org: true` — the same shape as a refused
  link — unless `force_cross_org: true`, which carries it as it is and names
  the crossing in the report's `warnings`),
  `resolve_move` (finish or undo a partial move left with both sessions
  alive),
  `restore_host_sessions` (batch resume a host's sessions lost to a reboot or
  tmux server restart over `recreate_session`; call with `dry_run: true` first
  for the plan — no ssh, no writes — then without it to run; paced by
  `restore.batch_size` / `restore.stagger_ms`, one failure never stops the
  rest), `discover_lost_sessions` (read-only: scan a host's
  `~/.claude/projects` for Claude conversations fleet has no row for — e.g.
  right after a reboot, before `restore_host_sessions` has anything to work
  with — rank them against the host's boot, and enrich each with
  `project_id`/`worktree_id`/`existing_session_id`/`derived_tmux_name` where
  inferable; restore a candidate with `new_session`'s
  `resume_claude_session_id`).
- **Worktree files & git (read-only)** — `repo_changes`, `repo_tree`,
  `repo_file`, `repo_diff`, `repo_blame`, `repo_log`, `repo_branches`,
  `repo_commit`, `repo_commit_diff`, `repo_branch_diff`, `repo_range_diff`.
- **Host clipboard** — `get_clipboard`, `set_clipboard`.
- **Asset catalog** — `list_assets` (catalog assets with per-host drift
  state, unmanaged assets and parse problems; the personal catalog only
  unless `all_catalogs` is set, which lists every catalog for the master
  or an unbound full client and still personal only for anyone else), `scan_assets` (re-scan hosts,
  read-only on the hosts, and recompute asset states), `import_assets`
  (import the controller's `~/.claude` into the catalog working tree;
  `dry_run` supported), `plan_sync` (compute a fleet-wide sync plan with
  per-host actions, including `plugin_update` once a pinned plugin's catalog
  version changes; mutating classification to update inventory), `apply_sync`
  (apply a `plan_sync` plan; master token only, behind desktop confirmation
  when `mcp.confirm_destructive` is enabled, and needs a `confirm_nonce` for
  ANY apply — not only one whose plan includes an overwrite or remove —
  while the setting is on), `set_secret` (store a `${NAME}`
  placeholder value; master token only; the value is never returned, logged, or
  audited). `catalog_admin` is everything else the desktop's Assets tab does
  — configure and load the checkout, create / edit / delete assets and
  their resources, lint, commit and push, plan and apply Sync, secrets and
  layers — as one tool whose `action` names the operation and whose `args`
  is that operation's own object. It answers the master and a paired client
  the operator granted (`fleet-hub client grant <name> assets`), refuses
  every other caller with `E_FORBIDDEN`, and is not listed to a per-host
  token; its `apply_sync` action passes the same confirm gate as
  `apply_sync`. It is what a hub-client desktop's Assets tab routes to.
  Sessions may also edit the catalog repo directly and commit with
  `catalog:` prefixed messages, which the app picks up on its next catalog
  load.
- **Asset catalog layers** — `list_layers` (the catalog's layer definitions
  plus every host's stored role + contexts), `resolve_preview` (compute one
  host's effective asset set with provenance, without writing anything),
  `propose_layers` (group the last scan's installed assets by host-set
  signature into a starting layer split, for triage), `set_host_layers`
  (replace a host's role + ordered contexts; validates the host and every
  named layer against the loaded catalog on the right axis, and rejects a
  role/context name collision; edits fleet state only, never catalog
  files; master token only — a host's layer assignment decides what the
  next `apply_sync` writes to its filesystem, the same reasoning as
  `apply_sync` and `set_secret`).
- **Asset changesets (Assets M4)** — `changesets` (`action`: `list` |
  `propose` | `apply` | `undo` | `dismiss` | `reject_item`): cards
  proposed from the last scan (Bootstrap, New on host, Drift, Rollout). It
  answers only the master and a full paired client bound to no org, which
  may `list` (one card in full with `id`); an org-bound client is refused
  every action before any card is read, and a per-host token is never
  served the tool (its "can list" is `list_assets` and the inventory).
  `propose` needs the personal grant; `apply` (`positions` picks items; a
  drift card applies one), `undo` (the latest applied card per catalog:
  `git revert` plus the stored layer assignments, never touching hosts),
  `dismiss` and `reject_item` need a grant on every catalog the card names
  (`fleet-hub client grant <name> assets [--catalog NAME]`); for `apply`
  the grants and the gate are read from exactly the items it runs (with no
  `positions`, every pending item but "needs a look"): applying a rollout
  or a restore also needs the personal grant and passes the `apply_sync`
  confirm gate, whose request names the card, its kind and each item with
  a short hash of its content, so an approval covers only that content (a
  card refreshed to new content, even for the same hosts and assets, asks
  again). A client naming a card that does
  not exist gets the same `E_FORBIDDEN` as an ungranted one, and a client
  holding no grant at all gets one refusal for every card; the master gets
  `E_NOTFOUND`. A failed apply commits nothing; a card never removes
  anything from a host. Dismissing or rejecting records a verdict, so the
  same content is not proposed again. One card action runs at a time; a
  card's host sync is not a task, so `cancel_task` cannot stop it.
- **Multi-harness set (F3a)** — `set_host_harnesses` (choose which
  harnesses the catalog syncs on one host: `null` = auto — Claude, plus
  Codex where a scan finds it or fleet already manages it there —
  otherwise an explicit list that must include `claude`; edits fleet state
  only, master token only, same reasoning as `set_host_layers`).
  `catalog_admin`'s `set_host_harnesses` action is the same call for a
  granted desktop.
- **Orchestration** — `wait_for_session`, `session_transcript`,
  `session_conversation`, `session_tool_detail`, `session_summary_since`, `run_prompt`,
  `dispatch_task`, `wait_for_task`, `list_tasks`, `cancel_task`,
  `set_session_tags`, `decide_related_session` (Link or Not related on a
  session's `related_session` proposal; a linked partner stays on the row
  with `linked: true`).
- **Work** — `work` (read: `{session_id}` → that session's live work links,
  primary first; `{key}` → ended links to the key, each with the snapshot of
  the session that did it), `work_link` (`{session_id, action}`: `link` a key
  or `item_id` — it becomes the session's primary work, `source` `manual` by
  default or `agent` from the in-session agent; `reject` — a sticky "not
  this"; `unlink` a `link_id` — a person's unlink (the master, a paired
  person's client) also keeps the unchanged branch / PR that named the key
  from linking it again, rule R9u; a per-host token's or the operator's is a
  plain unlink). Returns the updated row; a session row's
  `work` carries its primary link. A per-host token reads and decides only
  its own host's sessions. With neither `session_id` nor `key`, `work`
  lists the links that ended within `work.recent_days`.
  Resume and work memory (roadmap M2): `work { action: "context", key }` is
  the full handover context of a key — its sessions, conversations, the
  last progress line and compaction summary Claude wrote (from the work
  journal, which outlives the sessions), plus a live read-only git probe;
  third-party text is fenced as untrusted. `work { action: "resume_plan",
  key, link_id?, host_alias?, with_brief? }` says what a resume would do:
  live sessions (Jump, never a second one), the ended candidates, where it
  lands, and per mode (`last` | `brief` | `fresh`) whether it is possible
  and why not. `work_link { action: "resume", key, mode, link_id?,
  host_alias?, brief? }` starts it: `last` continues the conversation in its
  worktree (recreated from the branch when it is gone), `brief` starts fresh
  with the handover brief delivered through the first hook's
  `additionalContext` (never typed into the pane; a short start prompt is
  typed only into a ready REPL, never into the trust dialog), `fresh` starts
  clean. `work { action: "purge_impact", project_id, host_aliases }` names
  the keys a purge would leave without resumable conversations. A per-host
  token reads context and plans only for work that ran on its host, and
  resumes only onto it.
  Detection and explanations (roadmap M4): fleet proposes links by itself
  from the session's current branch, its PR (head branch, closing issues,
  keys in the title / body and in commit trailers) and the references in
  submitted prompts (keys, Jira / Linear / Asana / GitHub ticket URLs,
  `#n` against the session's own repo). Each link carries `state`
  (`confirmed` | `suggested` | `rejected`), `strength` (`explicit` |
  `strong` | `weak`), `rule` (the resolver rule, R2–R8 and R3u of design §0.3.1) and
  `evidence` (what was seen: signal, matched text, a ±40-character redacted
  prompt snippet unless `work.evidence_snippets` is off, when, which
  conversation) — `work { session_id }` returns it all. A session row's
  `work` stays the primary CONFIRMED link; `work_suggested` is its top
  suggestion (with `suggestions`, the count), kept apart so a guess never
  groups a session. A branch or PR change ends the automatic link it made
  (`end_reason` `branch_changed` | `pr_changed`, snapshotted as past work);
  manual, `started`, `agent` and `agent_started` links are never ended by it, and a rejected
  (session, target) pair is never proposed again. Decide with `work_link
  { session_id, action: "confirm", link_id }` or `{ action: "reject",
  link_id }`; `work_link { action: "trust_project", project_id, on }` lets a
  sole branch key in that project link by itself (master or client token;
  refused to a per-host token). Prompts are never stored — only matches.
  `work_link { action: "link", source: "agent_inferred", key }` is Claude's
  answer to the opt-in classification nudge (`work.classify_nudge`, M4.6):
  not a decision but a pre-selected suggestion (rule R11, strength
  `inferred`) that a person confirms or rejects; a rejected pair stays
  rejected. `source: "agent"` remains a confirmed declaration.
  Trackers (roadmap M3): `work_admin` (master token only — fleet admin, so
  on a paired desktop the Settings → Trackers page says "configure on the
  hub") manages them: `list`, `add { site_url, provider?, transport?,
  settings? }` (the site, or any ticket / issue URL on it; the provider —
  `jira`, `github`, `asana`, `linear`, `jira_dc` — is inferred from the URL
  unless given, and each provider's site is fenced: `*.atlassian.net`,
  `github.com[/<owner>]`, `app.asana.com[/<workspace>]`,
  `linear.app/<workspace>`, one exact Data Center host), `update
  { tracker_id, name?, transport?, settings? }`, `set_credential { tracker_id,
  auth_kind?, username?, secret | credential_ref }` (`env:NAME` or
  `file:/path`, read by the hub at use; without `username` the token is the
  whole credential; refused for a `via_cli` tracker),
  `test { tracker_id }` (probe the site: account, key prefixes, sprint
  projects, views; the tracker's `state` becomes `ok` or says why not) and
  `remove { tracker_id }` (confirm-gated; its items stay, marked
  unavailable). No answer, event, log line or error ever carries the secret:
  a tracker row has only `has_credential` and a `…abcd` hint. The hub's
  operator has the same over loopback: `fleet-hub tracker
  list|add|set-credential|test|remove`, which reads the token from stdin or
  `--from-env`, never from argv.
  More providers (roadmap M6): `transport` is `direct` (HTTPS from the hub),
  `via_host:<host>` (`curl` on that host, the credential piped on stdin,
  never in argv) or `via_cli:<host>` (a trusted CLI there with its own login
  — GitHub's `gh`; fleet then holds no credential, and GitHub accepts only
  this). `settings` is the provider's admin object: GitHub `repos`
  (`owner/repo` list narrowing the owner scope), Asana `section_map`
  (section → `todo` | `in_progress` | `done`) and `section_map_confirmed`,
  Jira Data Center `extra_ca` (PEM) and `allow_private_network` (the site may
  resolve to a loopback / link-local address, refused otherwise), and for
  Jira (Cloud and Data Center) `write_back: { pr_remote_link }` (work graph
  M13.4e, D3: off by default; with it on, a PR seen on a session a person
  linked to one of the tracker's items is added to that item once as a
  remote link, `globalId` `fleet:pr:<url>`, through an outbox drained by the
  sync pass; `fleet_health.trackers[].write_failures` counts the writes
  given up). `update` replaces the whole `settings` object. Keys are
  the tracker's own: `ABC-123` (Jira, Linear team keys), `owner/repo#42`
  (GitHub), `asana:<task gid>` (Asana, which has no human keys — detection is
  by URL); `lookup` and `start` take any of them, or the item's URL.
  Reading tickets (M3.4): `work { action: "tickets", tracker_id?, view?,
  query?, limit? }` serves the sync's **cache** (never a live call); `view`
  is `mine` (assigned to the tracker account, not done), `sprint` (in an
  active sprint), `recent` (updated within 14 days) or `filter:<id>` (a
  favourite filter), each row with the live sessions already on it.
  `work { action: "lookup", key | url }` answers one ticket from the cache,
  or fetches it once and caches it (a URL names its site; an unknown site is
  `E_NOTFOUND` with the `site_url` to add). `work { action: "trackers" }`
  lists the trackers (no secrets). `work_link { action: "start", key | url |
  item_id, project_id?, host_alias?, with_brief?, brief? }` starts work on a
  ticket in one call: a live session on the key is `E_EXISTS` (details name
  it — jump, do not start a second); the project defaults to where that key
  prefix last ran (else `E_AMBIGUOUS` with candidates), the host likewise;
  the worktree is `slug(key + title)`, the session's name `KEY title`, and it
  is linked `started` (`agent_started` for a per-host token or the operator).
  With a brief, the ticket's context (its description
  fenced as untrusted) rides the first hook's `additionalContext` and a short
  start prompt is typed only into a ready REPL. A per-host token reads,
  looks up and starts only tickets linked to sessions on its own host, and
  starts only there (`E_FORBIDDEN` says why); it never receives `work:*`
  frames on `/events`. Events: `work:item`, `work:tracker`,
  `work:tracker_removed` — emitted only when something a reader sees
  changed; a session's `work` carries its item's `kind` (`tracker` | `local`
  | `ref`), `status_category`, `status_name`, `url` and `unavailable`.
  `status_category` is the tracker's own status — `null` for a local item or
  a bare key, by design, since a paired phone tells a local item apart from
  a ticket by that absence (native item status task 4). `effective_status`
  is the live answer instead: the stored value with a person's override, a
  merged-PR's stamped `done`, or a currently-working session's live
  `in_progress` applied — present for a local item too, and the field a
  status display should read. `work:changed` (work graph M14)
  carries ids only — `{ what: placement | rule | view | org, task_id?,
  rule_id?, view_id? }` — after a Work view structure write; a client
  re-reads what it shows. Like every `work:*` frame it never reaches a
  per-host token or an org-bound client.
  Organisations (roadmap M5): `work { action: "scopes" }` lists the scope
  selector's entries (named orgs, then GitHub owners no org covers, then
  the unassigned rest, each with `session_count` and `needs_you`), `work
  { action: "orgs" }` the orgs with their rules, hosts and trackers, and
  `work { action: "org_suggestions" }` proposed orgs (from owners of live
  sessions and tracker sites; never applied, empty for a per-host token).
  `work_admin` adds `list_orgs`, `add_org { name, color?, isolate_sessions? }`,
  `update_org { org_id, … }`, `remove_org { org_id }` (confirm-gated;
  refused while a tracker belongs to it, naming it), `add_rule { org_id,
  owner?, repo?, path_prefix?, host_alias? }`, `remove_rule { rule_id }`,
  `assign_host` / `unassign_host { host_alias, org_id }` and `assign_tracker
  { tracker_id, org_id? }` — master only, so a host can never move itself.
  Retention (M12.3): `work_admin { action: "status" }` answers `{ trackers,
  retention }`. `trackers` is M11.4's sync metrics. `retention` gives each
  swept table's rows, its window and a dry-run `would_delete`, plus the
  last sweep. `sweep_now` runs one bounded sweep; the GC tick is the only
  other trigger (`docs/hub.md`, *Work retention*).
  Rows gain `org_id` (session, host, tracker, link, `work` summaries). For a
  **per-host token** the host's org is a boundary on everything above: it
  reads links, tickets, trackers, context and briefs only inside its org or
  unassigned (a host in no org: unassigned only), an id outside answers as
  an unknown id, a key outside links as the bare key it typed, and every
  session row it receives has other orgs' work taken out. Linking,
  confirming, starting or resuming work of one org on a session of another
  — or moving a session (`move_session`) to a host whose org its live links
  are not in — is `E_FORBIDDEN` for every caller (details `cross_org: true`)
  unless `force_cross_org: true`. An org with `isolate_sessions` also hides its
  sessions from other orgs' hosts (lists, `whoami`, `peer_status`,
  `session_history`, repo reads, messages, `session:*` frames). See
  [hub.md](hub.md) → *Organisations and isolation*.
  Lifecycle (roadmap M7): `work { action: "tidy" }` returns the tidy-up
  candidates — each with `session_id`, `link_id`, a primary `reason`
  (`done_idle` | `pr_merged_idle` | `not_planned` | `duplicate_worktree` |
  `same_work` | `ghost_expiring` | `idle_unlinked`; `same_work` carries
  `same_as`, the session kept), `secondary` reasons, the preselected `action`
  (`safe_kill` | `kill` | `archive` | `resume_or_expire`), a preview (host,
  branch, key, item status, PR, idle time), `auto` (auto-tidy would act
  on it) and, on a `safe_kill` row, `worktree_path` (the tree a clean up
  removes) and `worktree_kb` (its size as the host probe last measured it;
  absent until measured) — plus the policy (`auto_tidy`, `auto_reasons`, `done_days`,
  `idle_hours`). `work { action: "reopened" }` lists work moved out of done
  that has past sessions. `work_link { action: "tidy_apply", items: [{
  session_id, action, link_id?, days? }] }` applies a batch (`safe_kill`,
  `kill`, `archive`, `snooze`, `never`) and reports every item: one failing
  never stops the rest, a protected session is refused per item, a kill of a
  session's own worktree always inspects it first (dirty or unpushed ⇒ the
  safe-kill path), and a worktree another live session shares is only
  plain-killed. With `mcp.confirm_destructive` on, a batch containing a kill
  needs the desktop's confirmation (`confirm_nonce`), like `kill_session`.
  `work_link { session_id, action: "archive" | "unarchive" }` collapses a
  live session into its group's Done (UI only; tmux keeps running) or brings
  it back — a prompt or an attach does too; `{ action: "snooze", days? }`
  (default 7) and `{ action: "never" }` flag the session's primary link (or
  `link_id`); `{ action: "dismiss", item_id }` clears a reopened entry
  (refused to a per-host token). A per-host token sees and applies only its
  own host's candidates of its org (a session outside answers as an unknown
  one), and reads reopened work only when its newest past session ran there
  and its item is in the token's org. `work_admin { add_org | update_org,
  auto_tidy: "on" | "off" | "inherit" }` overrides `work.auto_tidy` for one
  org (master only).
  Work graph M9: `work { action: "today", since? }` is the Today view's
  digest — live sessions grouped by primary work into `waiting` (someone
  is needed), `stale` (idle three days, or the ticket is done while a
  session runs) and `in_progress`, plus what `shipped` since `since` (unix
  seconds; default the last 24 h): tickets that moved to done and work that
  ended with a PR. `work { action: "card", key }` is a ticket's context
  card from the tracker cache (never a fetch): title, status, url, the
  `acceptance` criteria parsed from the description (else an `excerpt`),
  and `composer_text` — the ticket text fenced as untrusted, for inserting
  into a prompt. A per-host token reads only its own host's day and cards
  for its own work, and gets `composer_text` without the plain fields.
  `work_link { action: "handover", session_id }` (M9.3, on demand only)
  asks a live, idle Claude session linked to work to write the hand-off the
  next session will need: fleet types one prompt asking for it between two
  nonce-tagged marker lines, and the next Stop hook keeps the text between
  them as a work-journal `note` from the `agent`. The resume brief and
  `work { action: "context" }` show the newest one first, fenced as
  untrusted. Refused while the session is busy, waiting on a dialog or
  stuck, without work, when a request is already pending (30 min), and for
  the operator's own session. Timeline: `handover_requested`,
  `handover_written`, `handover_missing`, `handover_send_failed`.
  `work_link { action: "summarize", key, link_id }` (M13.4c, on demand only)
  is the dead-session counterpart: a Claude-written summary of past work
  `link_id` (an ended link of `key`, from `work { links }` or the resume
  plan). One `claude -p --resume <id> --fork-session
  --no-session-persistence --tools '' --strict-mcp-config` run on the
  session's own host, in the directory its transcript recorded, with the
  model `work.summary_model` names and fleet's hooks off. The answer is
  redacted, capped at 4,000 characters, stored as that conversation's one
  journal `summary` (asking again replaces it) and returned fenced as
  untrusted in `summary`; the resume brief shows it after the agent
  handover. `E_NO_TRANSCRIPT` when the transcript is gone or was purged,
  `E_NOTFOUND` when its directory is gone, `E_CLAUDE_CLI` when `claude` is
  missing or fails, `E_TIMEOUT` past 170 s, `E_EXISTS` while another
  summary runs on that host, and `E_INVALID` when a live session still
  holds the conversation (ask it for a `handover`). A per-host token may
  summarise only its own host's past work in its org; the operator's
  request waits for confirmation, and is refused on a hub.
  `work_link { action: "start", …, project_ids: [..] }` (M9.6) starts one
  ticket in several repositories at once — up to 8 — one sibling session
  per project, all on the same branch name (`slug(key + title)`, or the
  `worktree` given), each linked `started`, each brief naming the others.
  A repository where the key already runs is skipped (naming the session)
  rather than refusing the whole start; the reply is `{ key, started,
  skipped, failed }`. `project_id` and `project_ids` are exclusive.
  Local work (M11.1, "Name this work…"): `work_link { action: "name",
  session_id, title, key? }` creates a **new** local work item — work with a
  title and no ticket — and links the session to it (manual, confirmed; it
  becomes the session's primary work only when the session has none). The
  title is trimmed, 1–120 characters, no control characters; the key goes
  through the usual canonical spelling. A key a tracker item the caller can
  see already carries (by key or alias) is `E_EXISTS` (details `item_id`,
  `tracker: true`): that work has a ticket — link it with `{ action:
  "link", key }`. A ticket of an org the caller cannot see is no collision:
  the key names new work, exactly as an unknown key does. A key a local item
  already carries is `E_EXISTS` for every caller (local keys are one
  fleet-wide namespace; details `item_id` only for a caller that sees that
  item). `work_link { action: "name", item_id, title }` renames a local
  item (a ticket is `E_INVALID`) and returns the item; both emit
  `session:updated` for the rows that show it and `work:item`.
  `work_link { action: "set_status", item_id, status }` sets a local item's
  status to `todo`, `in_progress` or `done` and returns the item; a ticket is
  `E_INVALID` too, naming it — its status belongs to its tracker, and the
  next sync would otherwise overwrite it here. The setting is final: fleet
  never derives a status back over what a person set.
  `work_link { action: "edit", item_id, title?, notes?, assignees? }` edits a
  local item's title, notes (`""` clears) and assignees (display names, `[]`
  clears; trimmed, each once, at most 10 of 80 characters) and returns the
  item; a field left out stays as it is. A ticket is `E_INVALID`, naming it,
  and a delegated job's notes (its dispatch prompt) are `E_INVALID` too. The
  person gate is `set_status`'s.
  **Sprints and releases** (design `2026-09-28-sprints-releases-epics-design.md`):
  `work { action: "buckets", kind? }` lists the sprints and releases in the
  caller's scope with their roll-up (`total`, `done`) and the tracker sprints
  or versions each adopts from; `work { action: "bucket", bucket_id }` answers
  one with its members, past ones too (`removed_at`: carried over, or taken
  out). `work_link { action: "bucket_add" | "bucket_remove", bucket_id,
  item_id }` plans work into one: an item is in at most one current sprint
  (a second is `E_CONFLICT`, naming the first) and any number of releases; a
  closed sprint takes nothing; an item of another organisation than the
  bucket's is `E_FORBIDDEN` (`cross_org: true`); a per-host token is
  `E_FORBIDDEN` — a session does not plan. Creating and changing them is the
  master's: `work_admin { action: "bucket_create", kind, name, org_id?,
  starts_at?, ends_at?, goal? }`, `bucket_update { bucket_id, …, state:
  active | released, shipped_ref?, expected_version? }` (a sprint goes
  planned → active, a release planned → released, stamping `shipped_at`; an
  active sprint beside another active one answers with a `warning`, never a
  refusal), `bucket_close { bucket_id, carry_to?, carry? }` (ends every
  membership; carries the unfinished — all of them unless `carry` names the
  confirmed list — into `carry_to`), `bucket_delete` (confirmed), and
  `bucket_adopt | bucket_unadopt { bucket_id, tracker_id, external_id }`: a
  Jira sprint or fixVersion, a Linear cycle or project milestone, a GitHub
  milestone (by name) whose synced items join the bucket as `adopted` and
  leave it when the tracker stops reporting them. A person's membership is
  never touched by a sync. A provider without sprints (GitHub, Asana) or
  versions (Asana) only cannot be adopted from (`E_INVALID`).
  `work { action: "local_items" }` lists local items (`id`, `key`, `title`,
  `created_at`, `updated_at`, `live_sessions`), newest change first.
  Readonly tokens cannot name or rename. A per-host token names work only on
  its own host's sessions inside its org — any other session answers as an
  unknown one — and sees (lists, renames) a local item only through a live
  link on its host's sessions or a past one whose session ran there, inside
  its org; the count is of its host's sessions. The phone names and
  renames work with a full token (D20, fleet-mobile M13.4a).
- **Paired clients** — `pair_client` (mint a single-use pairing code and the
  URL to show as a QR; master token only), `list_clients` (the paired devices
  and what each one's token may do — the stored token digest is never
  returned; a read, but master token only, since it enumerates every paired
  device), `revoke_client` (revoke one by name; master token only),
  `set_client_trust` (grant or withdraw trust in one by name — a trusted
  client's prompts are delivered without the untrusted-content marker; master
  token only). A paired client is never the master, so every tool in this
  group — like the rest of fleet admin — stays out of a phone's reach. The
  `fleet-hub pair`, `fleet-hub client list`, `fleet-hub client revoke` and
  `fleet-hub client trust|untrust` commands are thin wrappers around these
  four.
- **Hub links** — `peer_exchange` (hub-to-hub federation: a linked hub's
  `peer` token long-polls it to trade messages and acknowledgements; the one
  tool a peer token reaches, and no other token reaches it, so it is listed
  to peer tokens only), `list_peer_links` (this hub's links to other fleets'
  hubs — fleet, role, state, pending count, last exchange and error, the
  last round trip and the messages carried today and in all, never a token;
  a read for the master and the hub owner's own device, since it names other
  fleets), `link_peer` / `unlink_peer` (link a hub with the one-time code its
  owner minted, or remove a link and fail what waits for it; the master or
  the owner's trusted full device). Settings → Federation shows and drives
  them from a paired desktop; the `fleet-hub peer add|list|remove` commands
  drive the same links straight on `state.db`.
- **Updates** — `update_status` (the fleet's application updates: the
  verified release channel, each target's reported version and phase with
  what the hub would tell it now, per-component counts and the operator's
  pins; a read any client may make, but a per-host or org-bound token sees
  its own row only) and `update_admin` (master token only: `pin` a version
  for a component or one target, where a pin below installed is a rollback;
  `unpin`; `update_now` to install at once; `refresh` to re-read the signed
  channel now), and `update_policy` (a person's device: `list`, `set` or
  `clear` the update policy of the org it administers — an org admin's own
  org, or any org from the hub owner's device). `update_status {
  target }` answers for one target (`client:<id>`, `agent:<alias>`,
  `hub:self`) with its whole decision — status, reason code, the release it
  would be offered and whether it is mandatory — the dashboard's "why"; a
  scoped token may ask about itself only. `fleet_health.updates` names what
  needs a person: `update_required`, `update_failed`,
  `update_rolled_back`, `rollback_failed` per target, `channel_stale`, and
  `rollout_paused` per paused rollout (`rollout_start` / `rollout_pause` /
  `rollout_resume` / `rollout_abort` stage a release in waves, and
  `set_policy` / `clear_policy` override the policy for one org's targets;
  see `docs/updates.md`). Events: `update:changed` carries ids only — `{ what:
  observed | pin | channel, target? }` — when a target's reported build or
  phase, a pin or the verified channel changes; a client re-reads
  `update_status`. It never reaches a per-host token or an org-bound
  client. The update wire itself, `POST /update/check` and
  `/update/report` (and `GET /update/artifact/<sha256>`, the mirror), is
  not a tool: see `docs/updates.md`.
- **File downloads** — `send_file` (`{ session_id, path, note? }`) copies a
  file from a session's host to the machine that owns the fleet (the hub),
  for the person's phone and desktop: absolute or relative to the session's
  worktree root, ≤ `downloads.max_file_mb`, folders refused (zip them). It
  answers the row in state `fetching` at once; the copy runs in the
  background. A per-host token sends from its OWN host only — that is how a
  session's Claude hands over what it made. `list_downloads` (a read) and
  `remove_download` are a person's, never served to a per-host token; the
  bytes are `GET /downloads/<id>` (bearer, not a tool result). Events:
  `download:changed { id }`, ids only, never on a host- or org-bound
  stream. See `docs/hub.md` → *File downloads*.
- **Library** — `library`, by `action` (Orbit Fleet 9.7): the files a
  person put on a host, by Control's Library Upload… or as a prompt's
  attachment (`library_items`). `list { session_id?, host_alias?, limit? }`
  answers `{ items }`, newest first; `add { kind: upload | attachment,
  session_id, files: [{ path, name?, size? }] }` records files already on
  the session's host (the desktop puts them there over its own ssh), taking
  the host, session name and org from the session's row; `remove { id }`
  drops a row and never the file. A row names a path on the owner's host,
  so it is the downloads' `own` tier: a grantee or another person sees an
  empty page and `add` answers `E_NOTFOUND`. Never served to a per-host
  token. The Library lists downloads and repos beside these from
  `list_downloads` and the sessions it already has.
- **Routines** — `routines`, by `action` (Orbit Fleet 8.5): a person's
  saved prompt that starts a session on a cron schedule (`trigger: cron`,
  five fields read at the `utc_offset_min` the device had when it was
  saved), on a session timeline event (`trigger: event`, one of `turn_done`,
  `stop_failure`, `stuck`, `lost`, `task_done`, `task_failed`,
  `session_restore_failed`, `workspace_repair_failed`, written for a
  session its owner owns and not started by a routine), on a pull
  request's review, checks or merge (`pr_review` when it gains an approval
  or a request for changes, `pr_ci_failed`, `pr_ci_passed`, `pr_merged`:
  written when reconcile's `pull_requests` row changes, on the timeline of
  the session that opened it; M15 G2.4), or only when a person presses
  `run_now` (`trigger: manual`). A pull request routine may narrow to
  `event_repo` (`owner/name`, or `name` of any owner) and widen with
  `event_author: anyone` from its owner's PRs to those of any session on a
  host of its org, never one a routine's session opened; its run's prompt
  ends with a line naming the PR. Any event routine may set
  `event_rate_secs` (up to a week): at most one run per PR, or per
  session, in that window; a fire inside it is dropped. `list`; `get {
  routine_id }` with its last runs; `runs { routine_id, limit? }`; `save {
  routine, routine_id? }` writes the whole routine (host, project,
  `profile` = the account it bills, prompt, `budget_run_micros`,
  `budget_day_micros`, `overlap: skip | parallel`); `preview { routine,
  routine_id? }` is the editor's dry run (gap plan G2.3): it writes
  nothing and answers `problem` (what `save` would refuse, absent when it
  would save), `next_runs` (the schedule's next five fires at
  `utc_offset_min`, which a daylight-saving change does not move), the
  `account` its login bills and the host's `logins` for the Account
  picker; `delete`; `set_enabled
  { enabled }`; `skip_next { skip? }`; `run_now`. Each run's session has
  origin `routine` and is the routine's owner's; the prompt is its
  handover. A run is `done` when its session's first turn finishes,
  `failed` on an error, a lost or removed session, six quiet hours, or a
  session past the run budget (which also turns the routine off with
  `paused_reason`), and `skipped` when the last run is still going under
  `overlap: skip`, today's budget is spent, the account its login bills
  is at or past `accounts.pause_at` (Orbit Fleet 8.7, read from the stored
  usage readings), or a person skipped it.
  Guards (gap plan G3.8, migration 159), all optional in `save`:
  `time_zone` (the IANA zone shown with the schedule; `utc_offset_min`
  still drives it), `run_max_secs` (60 to 21600: a run still going after
  it has its turn stopped with Escape and fails), `fallback_host` (a host
  of the same org that takes a run when its host is unreachable, its login
  is past `accounts.pause_at`, or its start failed; the run's `host_alias`
  says where it ran), `retry_once` (a failed run starts again once on the
  next pass, `trigger_ref` `retry:<run id>`, never while paused) and
  `autonomy` (0 report only, 1 ask before push, 2 or absent push: a line
  under the prompt, not a sandbox). `automation.daily_budget` (whole USD,
  0 none) skips every routine's runs, `run_now` included, once all of them
  spent it in the UTC day; `budget` answers `{ spent_micros,
  budget_micros?, since }`. A failed run carries `error_code`, and `get`'s
  `fixes` and `failing`'s `fix` name its fix `{ run_id, code, label,
  action: edit | host | accounts | session | retry, host? }`.
  A finished run also carries an `outcome` (8.10): `did_work`, `nothing`,
  `failed` or `needs_person`, with `outcome_source` `exit` (a failed run,
  which nothing overrides), `rule` (an open question or a wedged session is
  `needs_person`, a pull request `did_work`) or `jev`; a run no rule can
  read has none until Jev answers. A `nothing` run marks its session seen,
  so it stays out of the Inbox. `automation.paused` (Pause all) stops the
  schedule and event fires,
  never `run_now`. Read and changed by the owner and the org's admins, read
  by the org's members, never served to a per-host token; the routine's
  org is its host's.
- **Start rules** — `start_rules`, by `action` (Orbit Fleet 8.11): a task
  key pattern (`PD-*`, `*` for any run of characters, any case) that names
  the project, and optionally the host, a start of a matching task lands
  in. An active rule decides before the key's history and before Jev K1,
  so a rule match records no decision run; a project or host the caller
  names still wins. The most specific pattern decides. After a person
  starts tasks of one `PREFIX-N` prefix in the same project five times in
  a row, fleet offers the rule: the start preview carries it as
  `rule_offer`, and `list` shows it first. `list` (offers, active and
  dismissed rules, each with `project` and `may_change`); `save { rule,
  rule_id? }` writes the whole rule (`pattern`, `project_id`,
  `host_alias?`, `org_id?` on a new one) as active; `accept { rule_id }`
  makes an offer the caller's active rule and replaces the pattern's other
  active rule; `dismiss { rule_id }` (an offer is never made again);
  `delete { rule_id }`. A rule decides for its org's tasks only; read by
  the org's members, added and changed by its admins and the owner, never
  served to a per-host token. A plan a rule decided carries `rule_id`.
- **Debug devices** — `debug_devices`, by `action`. Android phones and
  emulators, iOS simulators and devices attached to any fleet host,
  inventoried by a scan of that host (`list { refresh? }`, `scan { host? }`,
  which also answers the emulators and simulators `boot { host, name }`
  could start). Every command runs on the device's own host over the
  fleet's usual SSH, so a session on one host drives a phone plugged into
  another: `run { device, args, timeout_s? }` takes one `adb` (`shell`,
  `uninstall`, `reboot`, `forward`, `reverse`, `emu`…), `simctl` (`launch`,
  `terminate`, `openurl`, `privacy`, `ui`, `status_bar`, `location`…) or
  `devicectl device` (`info`, `process`…) command, every argument quoted
  and nothing that reads or writes a host path; `install { device, path }`
  copies the app from the caller's host (a per-host token: its own host
  only) to the device's host first; `logs` (`logcat -d`, a simulator's
  `log show`), `screenshot` (an image content block), `boot`, `shutdown`.
  `claim { device, claim_s?, note? }` keeps every other caller off the
  device (`E_CONFLICT`) until it lapses or is released; each use by its
  holder extends it. A per-host token sees its own host's devices and the
  ones a person marked `shared` on hosts of its own org; `configure {
  label, shared }` and `forget` are a person's. See `docs/debug-devices.md`.
- **Pull requests** — `prs`, `action: list { state?: open|merged|closed|all,
  project_id?, limit? }` (redesign 6.4). Every PR a session's branch has had,
  newest change first: repo and number, title, head branch, state, draft,
  CI, review decision, merge state, merged time and the session that opened
  it. Reconcile records them from the `gh pr view` probe that already fills
  a session's `pr_url` / `ci_status`, and keeps them after the session is
  gone. A row is served to whoever may see the session that opened it; a PR
  whose session is gone only to the hub's own reader or the person of a
  one-person hub. A read: nothing here merges or closes a PR.
- **Runs** — `runs`, by `action` (`list` today; Orbit Fleet 8.3). One
  newest-first list of everything that ran on the fleet's behalf: dispatched
  tasks, a mission's steps and brakes, Jev's decisions, fleet's own
  `claude -p` runs (the planner, a summary) and routine fires. `list { since?, until?, kind?,
  outcome?, org_id?, mission_id?, session_id?, routine_id?, limit? (≤ 200,
  50), offset? }`
  answers `{ runs, total }`; each run carries `id` (`<source>:<rowid>`),
  `kind` (`operator` | `task` | `mission` | `jev` | `routine`, or a fleet
  `claude -p` origin: `planner` | `summary` | `commit_message` |
  `release_note` | `morning_brief` | `brief` | `watch_summary` | `triage`),
  `owner`, `started_at`, `ended_at?`, `duration_ms?`, `outcome` (`ok` |
  `failed` | `needs_person` | `nothing_to_do` | `running`), `error?`,
  `cost_micros?`, `model?`, `host?`, `org_id?`, `mission_id?`,
  `session_ids`, `summary?` and `routine_id?`. Jev rows are only the
  decisions that called the provider. The list is cut to the caller's view scope
  (`service::runs`): a task needs every session it names to be visible, a
  mission's rows need the mission, a routine's fires the routine, and runs that belong to no session or
  mission need whole-fleet spend. A person's, never served to a per-host
  token.
- **PR shepherd** — `pr_shepherd`, the standing rules that let fleet look
  after a project's PRs (`docs/superpowers/specs/2026-10-08-pr-shepherd-design.md`):
  `status` (rules, recent episodes and merges), `grant { project_id, level:
  watch|nudge|merge, hours?, recipes? }`, `revoke { project_id }`,
  `pause_all`. Served only to the hub owner's own paired device, never to
  the master an agent holds (the operator has `fleet-hub shepherd`); a write
  needs a trusted full device.
- **Chat forms** — `ask`. Chat forms: open a `fleet.form/1` form in your own session's chat and wait for a person's answers; the person's side lists, gets, answers and declines. See `docs/forms.md`.
- **Operator settings** — `get_settings` (every registered key of the
  settings registry, `service/settings.rs`, with its effective value; a
  read, but master token only, since the values name hosts and their
  projects roots) and `set_setting` (change one: validated against the
  key's shape, `E_INVALID` for an unknown or derived key or a bad value;
  returns the whole object). They reach the same keys as the desktop's
  Settings dialog and no others. `hub.*` and `mcp.*` are registered
  read-only: `get_settings` shows them, and `set_setting` refuses them,
  naming the flag or command that changes each one. `controller.*` and the
  tokens are not registered at all. `get_settings { describe: true }`
  returns every key's metadata instead of a plain map, in display order:
  label, help, kind with bounds and options, unit, what `0` means, default,
  value, `modified`, tags, danger, restart, AI policy, `owned_by` and option
  labels. This is the list `docs/settings-reference.md` is generated from.
  A write emits `settings:changed { key }`. Like `work:*`, it never reaches
  a per-host token or an org-bound client. Every write of a registered key
  is audited (who: `person`, `agent` for this API, or `system`; before →
  after; the proposal it applied), and the desktop shows it as each
  field's **History** (declarative pages P5).
  `set_setting { propose: true, why? }` writes nothing: it validates the
  value like a write and leaves a **proposal** a person applies or rejects
  (Settings → Proposed changes on a standalone desktop, `fleet-hub
  settings proposals | apply | reject` on a hub). Use it when a person
  should decide — the work graph's rule R11 applied to settings. A key
  whose AI policy is `never` (every change that needs confirming, and
  every read-only key) cannot be proposed: `E_FORBIDDEN`. A value the key
  already has is `E_INVALID`; a newer proposal for a key replaces its
  pending one, and at most 50 wait (`E_RATE_LIMITED`). `why` is at most
  500 characters and shown to the person as written.
  **Who** (declarative pages P6): the master token, and the HUB OWNER's own
  paired device — a client bound to no org whose person is this hub's owner
  (multi-user M1), such as the desktop paired with a hub or a phone — reach
  `get_settings` and `set_setting`; a per-host token, an org-bound client and
  a second person's device never do (the settings are the whole fleet's). A
  device of either mode reads; a `full` device proposes; only a device the
  operator **trusts** (`fleet-hub client trust <name>`) writes directly and
  decides proposals, and its writes are audited as `person` with `client
  <name>`. The UX agent's operator client is an agent: it proposes only.
  Four more tools are served to a paired device and not to the master (who
  has `fleet-hub settings` on the hub machine, and whose tool list is
  budgeted), on the same terms — the owner's own device, never a
  colleague's: `setting_proposals` (pending, each with the key's value now,
  and `can_write` for this device), `setting_history` (`key`, `limit`),
  `decide_setting_proposals` (`accept`, `reject`; trusted only) and
  `list_pages` (the page specs, data source shapes, resources and page
  actions a device renders).

  The ticks and sweeps read their settings every pass, so a change takes
  effect on the next one. On a hub, this is how `reports.*` and `work.*`,
  which have no flag, are set.
- **`org_admin`** — the company's administration from the hub owner's
  own device (org administration phase B,
  `docs/superpowers/specs/2026-10-06-org-administration-design.md`). Served
  to that device and not to the master, who has `fleet-hub org|client|person`
  and `work_admin`. Orgs: `work_admin`'s org actions under the same names
  (`list_orgs`, `add_org`, `update_org`, `remove_org`, `add_rule`,
  `remove_rule`, `assign_host`, `unassign_host`, `assign_tracker`), with an
  org named by `org_id` or `org`, and `set_org_setting { org, key, value }`
  (an org's own value of a per-org setting; no `value` inherits the
  fleet's). Devices: `list_devices` (name, mode,
  trust, org, person, catalog grants, `this_device`; never a peer link or an
  updater token), `pair_device { device, mode: full|readonly, trusted, org,
  person, ttl_s }` (a one-time code, its URL and the URL's QR as rows of
  `1` / `0`), `revoke_device`, `set_device_trust`, `rename_device { device,
  name }` (its grants, catalogs and person follow it), `set_device_mode {
  device, mode: full|readonly }`, `bind_device { device,
  org }` (no org unbinds), `set_device_person { device, person }` (created
  when new) and `grant_catalog { device, catalog, on }`. People:
  `list_people`, `rename_person { person_id, name, display_name }` and
  `disable_person` (revokes their devices and the shares made to them;
  never the owner). Any of the owner's `full` devices lists (a readonly
  device is refused the tool, which is not readonly); a change needs a
  **trusted `full`** device, and none may revoke, untrust, make read-only,
  bind, hand over or take a grant from the device the call comes through.

  **Phase D — members and an org admin.** The row is `Access::Device` (a
  person's device, bound to an org or not), and the tool decides the
  authority: the hub owner's unbound device administers the fleet, as
  above; a device fenced to an org its person administers acts on that org
  only, and is refused everything else (`E_FORBIDDEN`) — rules, tracker
  routing, `bound_sees_unassigned`, other orgs, people's names and
  disabling, and the two hub-owner switches stay the hub owner's; routing a
  host (`assign_host` / `unassign_host`) is a host administrator's (the hub
  owner, or an admin of the org that owns the hub). Its lists name its own
  org's devices and members only, and `pair_device` pairs for a member of
  the org (`person` required), fenced to it. Members: `list_members { org }`,
  `set_member { org, person | person_id, role: admin|member|viewer }` (adds,
  or changes a role; a new name becomes a person), `remove_member { org,
  person, keep_grants }` (revokes what was shared with them on the org's
  sessions unless `keep_grants`), `member_grants` (`{ watch, answer, drive }`
  counts), `revoke_member_grants` and `narrow_member_grants` — downward
  only. An org admin never changes their own membership or the hub owner's.
  Hub owner only: `set_hub_org { org }` (no org: none), and `update_org`'s
  `owns_hub` / `admins_see_unclaimed`.
- **`guide`** — step-by-step guides in Settings → Guides (declarative
  pages, layout `guide`; `docs/pages.md` → *Guides*). Any token, a host's
  own included, calls `catalog` (the rules, every setting's key, label and
  kind — never a value — the pages, page actions and read-only sources a
  guide may name, and a working example), `validate { spec }` (`ok` and
  every problem with where it is) and `propose { spec, why }`: nothing is
  shown until a person approves. `list` returns the live guides, what
  waits, and `can_write`. `decide { id, approve }` and `remove { page_id }`
  are a person's: the master or a trusted device, as for settings. A host's
  session writes one with the `fleet-guides` catalog skill.

A typical loop: `list_sessions` to see state → `new_session` to spawn one →
`run_prompt` to steer it and get the reply back (or `send_prompt` →
`wait_for_session` → `session_transcript` step by step; `capture_session`
for the raw screen).

### Status vocabulary

The values below live in enums (`service/pane_intel.rs`,
`service/attention.rs`); the tool descriptions and the generated reference
derive from them.

- **`claude_status`**: `working | blocked | completed | failed | stopped |
  idle`. `idle`, `completed`, `stopped` and `failed` mean the turn is over
  (what `wait_for_session { until: "idle" }` and `run_prompt` wait for);
  `blocked` is a dialog inside a turn. A row the tick demoted from `working`
  after `reconcile.stale_working_secs` with no sign of life (judged only while
  its host is being reconciled) carries `stale_working_at` and reads `idle`,
  but that is only a guess (one long tool call looks the same): `run_prompt`,
  `move_session` and `wait_for_session` ask its pane first and count it as
  mid-turn unless the pane shows the idle prompt.
- **`stuck_kind`**: `auth_menu | reconnect | trust_prompt | oom |
  press_enter`.
- **`needs_attention.reason`** (on session rows and `/events` frames), most
  urgent first: `waiting` (blocked on a dialog), `stuck` (`stuck_kind` says
  which), `host_down` (the session's host was pinged and did not answer),
  `account_limit` (its account's 5-hour or weekly window is used up and the
  session is not working), `no_credentials` (its account's login has
  expired or its token was rejected, and the session is not working), `stop_failed` (the
  last turn ended in an API error; re-prompt),
  `failed` (a pane-less agent reported failure), `context_full` (context at or
  past `health.context_red_pct`), `stale_working` (the demotion above),
  `ci_failing` (idle with failing PR checks), `probably_waiting` (contract
  15: Jev read the end of a silent turn as a question; a proposal, never
  counted as Needs you) and `lifecycle` (a failed or
  pending safe kill, a ghost, a lost row). `since` is when the session
  entered that state. `state` (contract 11) is the attention state the
  reason puts it in: `action_required`, `failed`, `blocked` (`host_down`,
  `account_limit`, `no_credentials`), `proposed` (`probably_waiting`) or
  `paused` (`lifecycle`). A `needs_attention=true` listing includes
  `probably_waiting` rows; the Needs you count is the first three states.
- **`waiting_on`** (on a mission row, contract 15): why an active mission
  waits on a person: `question` (an open `ask` card), `sign_grant` (it asks
  for autonomy and no live grant covers its plan) or `confirm` (open cards
  in its confirm queue), with `since` and `open_cards`.
- **`finish`** (on `work { mission }` of a finished mission, G3.7; optional,
  absent while it runs): `sessions`, the live sessions on its member items
  that the caller may see (`session_id`, `item_id`, `host_alias`,
  `tmux_name`, `kind`, `worktree_kb?` from the host probe), and `prs`, the
  pull requests its work opened, as `prs { list }` rows behind the same
  fence. A finished mission reopens with `work_link { mission_state,
  status: paused }`; it does not go straight back to active.

### Errors and limits

A tool that fails in fleet's service layer answers with a **tool result**
carrying `isError: true` (what the MCP spec prescribes for tool-execution
failures, so the model sees it as the tool's output and can correct the call),
not a JSON-RPC error. The text block is the documented `E_CODE: message` line;
`structuredContent` carries `{ code, message, details }` (for example the
candidate rows of `E_AMBIGUOUS` or the `confirm_nonce` of
`E_CONFIRM_REQUIRED`). JSON-RPC errors are reserved for protocol failures:
an unknown tool name or arguments that do not match the schema.

Three codes are specific to agent hosts:
- `E_AGENT_OFFLINE`: no `fleet-agent` is connected for the host. It is
  returned at once and means what `E_SSH` means for an SSH host.
- `E_AGENT_PROTOCOL`: the agent answered with something the protocol does
  not allow.
- `E_AGENT_REINSTALL`: `provision_hosts { rotate: true }` saved a new token
  for an agent host and sent it nothing. Install the token on the host out
  of band (`fleet-hub agent-token <host>`, then
  `fleet-agent install --token-file -`), then provision again.

Two codes belong to chat forms (`ask`):
- `E_NOT_A_SESSION`: `ask { form }` from a caller that is not a proven
  session. A form opens in the asking session's own chat, so the call must
  come from a per-host token whose `X-Fleet-Pane` matches a session row.
- `E_HOST_WRITE`: a chat form's secret could not be written to its session's
  host; `details.field` names it. The form stays pending.

A timed-out agent call reports `E_SSH_TIMEOUT`, the same code as SSH, so
nothing downstream mistakes a timeout for "nothing ran".

Every call runs under a wall clock: 60 s for reads and single round trips,
300 s for session lifecycle, provisioning and host probes, 660 s for the
self-bounded long polls (`wait_for_session`, `wait_for_task`, `run_prompt`,
`ask`, whose own `timeout_s` maxes at 600). On elapse the result is
`E_TIMEOUT: <tool> exceeded its <n> s limit; the call may have partially
completed` with `structuredContent { code: "E_TIMEOUT", tool, limit_secs }`.

Session rows carry `claude_status` (one of `working`, `blocked`, `completed`,
`failed`, `stopped`, `idle`, or null when unknown) and `stuck_kind` (one of
`auth_menu`, `reconnect`, `trust_prompt`, `oom`, `press_enter`, or null when
not stuck). The enums in `crates/fleet-core/src/service/pane_intel.rs` are the single
source of truth; the tool descriptions, server instructions and the control
skill quote them, and a test fails if any of those drift.

A full row also carries `pending_input`: the permission/question dialog a
blocked pane is showing, as `{kind, question, options[{n,label,selected}]}`
(`kind` is `permission` | `input`), or null when the pane shows no such
dialog — derived alongside `current_activity` on the same reconcile pass. A
multi-select question adds `multi: true` and `checked: true` on each ticked
option (both absent otherwise).

### Response caps

Responses are sized for MCP token limits: `list_sessions` and `list_projects`
return slim summary rows by default and accept `limit` (`list_sessions` also
takes `view: "phone"`, a named projection to the columns the phone app reads, and `list_projects` takes `has_sessions: true`, which keeps only
the projects a live session can name — see *Asking for fewer columns* in
`docs/hub.md`); `list_worktrees`
answers `{total, worktrees}` with slim rows, at most 100 of them (`limit`,
0 = no cap — what the desktop asks for in hub-client mode), filtered by
`project_id` / `host_alias`; `capture_session` returns plain text
capped to the last 200 lines (`max_lines`, 0 = no cap), and reads at most
20 000 rows of scrollback however large `scrollback_lines` is; `repo_log` returns 50
commits by default (`limit`, `skip`); `session_history`, `inbox` and
`list_tasks` default to 50 rows; `session_transcript` / `run_prompt` return at
most `max_chars` characters (default 8000, max 64000).

Every result is compact JSON (no pretty-printing), and the list/report tools
drop `null` fields — an absent field reads the same as a null one to a model,
and the indentation and `"field": null` repetitions measured ~25% of those
payloads.

### Remembered read cursors

Five fetch tools — `list_sessions`, `session_history`, `session_transcript`,
`inbox`, `repo_diff` — accept an optional `fresh_for: <session id>` and, when
given, remember what that reader last saw so a re-ask returns only what is
new.

**`fresh_for` is your own session id, not the caller's.** A caller label is
`host:<alias>`, so every Claude session running on a host shares one caller;
a cursor keyed by the caller would let those sessions silently consume each
other's deltas. Pass the id `list_sessions`/`whoami` gave the session about
itself, never a token or host identifier. An id that names no session gets a
full read with `cursor_reset: "reader_unknown"` — nothing is stored, so the
next call with the same bad id behaves identically rather than compounding.
The reader is fenced like a target: a per-host token may only name a session
on its own host, else `E_FORBIDDEN` before any read and no cursor is written
— a token on host A can never advance a host-B session's watermark and blind
it to its deltas. Every caller is then held to what it may SEE: a reader the
caller cannot see (another person's private session, a row outside its org,
an `unclaimed` row a host token's request proves no pane for) is answered
exactly like an id that names nothing — a full read with
`cursor_reset: "reader_unknown"` and no cursor written. The two are
deliberately indistinguishable, so the call is no existence oracle, and
nobody can write a cursor under somebody else's session id.

**Two answer shapes.** `session_history`, `inbox`, `repo_diff` and
`list_sessions` answer with a JSON envelope `{unchanged, cursor_reset, more,
data}`. `session_transcript` answers with **plain text**, as it does without
`fresh_for`, and states the same facts as banner lines (below).

Each envelope call answers one of three ways:
- **new data** — `unchanged: false`, `cursor_reset: null`, `data` holding
  only what is new since the stored cursor (for a snapshot tool, the whole
  current payload, since it changed);
- **`unchanged`** — `unchanged: true`, `data: null` (an empty array for
  `session_history`/`inbox`); nothing changed since the last read, so
  nothing is sent;
- **a reset** — `unchanged: false`, `cursor_reset` set to why the stored
  cursor could not be trusted for a delta, and `data` depending on the tool:
  - `session_history` / `inbox`: the **oldest page from the start of the
    stream** (row id 0, oldest-first, `limit` rows, `more: true` when more
    follow) — the reader re-walks the stream from the beginning. The one
    exception is `reader_unknown`, which gets the **default newest-first
    page** (exactly what the call returns with no `fresh_for`) and
    `more: false`: with no cursor to store, an oldest-first page would come
    back identical on every call and a caller following `more` would never
    stop.
  - `list_sessions` / `repo_diff`: the full payload, as with no
    `fresh_for`. Their only reset reason is `reader_unknown`.

A reader's **first** read (no cursor yet) is answered the same as a reset,
with `cursor_reset: null`. For `session_history` and `inbox` that means a
first read starts from the **oldest** row, so reaching the present on a long
stream takes several paged calls.

`session_transcript`'s text answers:
- `(unchanged since your last read at turn N)` — the whole answer, when
  nothing changed. **`unchanged` means no completed turn since your last
  read**: it is keyed on the session's `turn_seq`, which only a `Stop` hook
  moves. An interrupted turn or a slash command adds to the transcript with
  no `Stop` behind it, so it can be answered `unchanged` while the session
  sits idle. Nothing is lost — it is served with the next completed turn —
  but a caller polling an interrupted, idle session waits for that turn.
- the new turns, oldest first, separated by `---` lines;
- `[cursor reset: <reason> — earlier turns may not be shown; see
  session_conversations]` as the FIRST line, when the stored cursor could
  not be trusted. What follows is the default window — the last turn with
  content, as a first read gets — not the whole transcript.
- `[more: additional new turns follow — call session_transcript again with
  the same fresh_for to continue]` as the LAST line, when `max_chars` cut
  the page short;
- `[session_transcript: N chars dropped from the start — raise max_chars to
  see more]` above a single turn larger than the whole `max_chars` budget,
  served truncated from the front (the cursor still moves past it);
- `(no assistant text in the requested turns)` when the new turns render to
  nothing.

A first `session_transcript` read (no cursor yet) gets the default window —
the last turn with content — with no banner, and positions the cursor there.
`since_turn` is **ignored** whenever `fresh_for` is present: the cursor, not
`since_turn`, decides what is new.

`cursor_reset` is one of the four reasons in
`service::fresh::ResetReason::as_str`:
- `conversation_changed` — a conversation boundary (`/clear`, compaction, a
  new conversation) happened on the target session since the last read.
- `ahead_of_head` — the stored cursor is past the current head; the case it
  was written for is a reused `sessions.id` (the table has no
  `AUTOINCREMENT`) starting a new session's counters over from zero while an
  old cursor still named a much later point. Deleting a session now drops its
  cursors at once (see *Retention*), so this is a defensive backstop.
- `reader_unknown` — `fresh_for` names no session, as above.
- `too_far_behind` — `session_transcript` only: the stored positional anchor
  named a spot this read could not locate (outside the tail window it
  fetched, or no anchor was ever recorded), or a page could not be ended on
  a turn the anchor can name (a turn with no timestamp). `turn_seq` said a
  delta existed, but the delta could not be positioned, so it is answered
  with the default window rather than guessed.

**Streaming tools page oldest-first.** `session_history`, `inbox` and
`session_transcript`'s delta path return the oldest new rows first (not the
usual newest-first order of a plain read) and set `more: true` when the page
was truncated by `limit` (or, for `session_transcript`, `max_chars`). The
stored cursor only advances to what was actually returned in that page —
never past it — so a truncated catch-up is safe to repeat with the same
`fresh_for` until `more` is false.

`session_transcript` answers `unchanged` straight from the session's
`turn_seq` (plus its conversation generation) — **no transcript file is
read** for that answer. When a delta IS due, it is positioned by a stored
anchor (the last-served turn's opening-prompt timestamp plus a fingerprint
of its rendered text), not by arithmetic on `turn_seq`: an in-progress turn,
an interrupt, a slash command, or a queued prompt each add a turn to the
transcript file with no `Stop` behind it, so "`turn_seq` − watermark turns
from the end" cannot reliably name the same turns a caller already saw. A
turn that grew since it was served (its fingerprint changed) is re-served
whole. A delta with no anchor on record resets as `too_far_behind` instead
of guessing.

`inbox`'s cursor is keyed by `session_id` **and** `unread_only` together — a
`true`-filtered read and a `false`-filtered read of the same inbox watch two
independent sequences, so one can never skip rows the other has not yet
returned. **`fresh_for` does not change `mark_read`.** `mark_read` still
defaults to `true` and still flips the rows this call returned to read,
exactly as without `fresh_for`; pass `mark_read: false` to peek without
consuming. The cursor itself is per reader and never reads or writes
`read_at`, so with `unread_only: false` another reader's `mark_read` cannot
hide rows from your `fresh_for` delta (with `unread_only: true` a row read
by anyone is, by definition, filtered out).

`repo_diff`'s `unchanged` still computes (and hashes) the full diff on every
call — the tool has no cheaper "did it change" signal than the diff itself,
so the saving from `fresh_for` here is in what crosses back to the caller
and what it has to process, not in server-side git work.

For the two snapshot tools, the stored hash is over the exact `data` value
the envelope carries (its canonical, key-sorted JSON), so "the hash
matched" and "the bytes you would have received are the same" are one
claim. `list_sessions` with `fresh_for` returns rows sorted by session id
rather than the default's `last_activity_at DESC`, so a reconcile pass
bumping one session's activity can never reorder the array and change its
hash with no field the reader would recognize actually differing.
`unchanged` fires usefully in the default slim shape, which already drops
`last_activity_at` and `current_activity` — the two fields a reconcile tick
churns constantly — before hashing; a `summary: false` caller hashes those
two churny fields as well, so it will see `unchanged` far less often.

**Retention.** A session's cursors are deleted **in the same statement that
deletes the session row** — as the reader, and as the target a cursor is
about — by the `trg_read_cursors_on_session_delete` trigger (migration 044).
It has to be immediate: `sessions.id` has no `AUTOINCREMENT`, so a deleted
id is handed to the next session created, which must not inherit a dead
session's "already read". There is no retention window. The GC sweep
(`Store::sweep_orphan_read_cursors`) is kept as a backstop for rows naming
an id no session ever had; it runs on every GC tick alongside the
mail-retention sweep and, like it, is not gated on `gc.enabled`.

### The served tool surface

`tools/list` is scoped to the caller: the list is filtered by the same
predicates that gate the call (`readonly` mode, fleet-admin access), so a
token is never offered a tool it would be refused. The master token sees
every tool; a per-host `full` token and a `readonly` token see progressively
fewer (the budget test below prints the current counts and bytes). Definitions are also slimmed on the way out —
`$schema`, `title`, numeric `format`s and `"default": null` carry no meaning
for a caller — and each tool carries the MCP hints from its policy row
(`readOnlyHint` on reads, `destructiveHint` on the confirmation-gated
mutations). `mcp::tools::tests::the_served_definition_budget_stays_bounded`
holds the surface to a byte budget so a new tool or a grown description shows
up as a deliberate change.

For the measured audit behind these defaults, see
[`specs/2026-09-20-mcp-token-efficiency.md`](specs/2026-09-20-mcp-token-efficiency.md).

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
`send_prompt` returns `{ delivered, session_id, turn_seq_before, queued, acked }`.
It refuses a `blocked` or stuck session with `E_INVALID_STATE` (Enter would
answer its dialog) unless `force: true`; to a `working` session the prompt is
queued behind the running turn (`queued: true`, and `turn_seq_before` already
points past that turn). `acked` is `true` once the session's `UserPromptSubmit`
hook confirmed the prompt, `false` when it did not within 1.5 s, and `null`
when it cannot be known — nothing was submitted, no hook has ever reached the
row, or the prompt was QUEUED (the hook fires when the queued prompt starts,
which is whenever the running turn ends, so there is nothing to wait for).

`queue_prompt` (`{ session_id, prompt }`) is the patient form, what the
desktop's Send prompt dialog uses: an idle session gets the prompt at once
(`delivered: true`); a working, blocked or stuck one keeps it in the hub
(`queued_id`) and gets it as a new turn when its Stop hook reports the turn
over, with the reconcile tick as the backstop. It is never typed into a
dialog, prompts for one session go out in order, one per idle moment, and a
typing that fails three times is kept with its `error` rather than retried
forever. `queued_prompts { session_id }` lists what still waits (and what
failed); `cancel: <id>` takes one back. Both are `drive`, like `send_prompt`.

`acked: false` is **not** "the send failed": the text is in the pane either
way, and a slow hook, a busy host and a REPL that took the paste without
firing all look identical from the outside. Read the pane with
`capture_session` when you need certainty, or `session_activity` for just
the status/spinner slice of the same pane. There is deliberately no automatic
Enter retry — the only evidence available is a 1.5 s non-answer, and between
that and a retry the session may have opened a permission dialog, into which
Enter would select the highlighted answer. **To press Enter yourself, send an
EMPTY prompt**: an empty body is a bare Enter, it skips the blocked/stuck
refusal (pressing Enter into a stuck session is the point of it), and it
returns `queued: false, acked: null` with `turn_seq_before` unchanged.

Pass a `client_msg_id` to make a retry return the first
result instead of delivering twice (10-minute memory). The key is reserved
before delivery, so a retry sent while the first call is still running is
refused with `E_IN_FLIGHT` rather than delivered a second time; a send that
failed releases its key, so retrying after an error does deliver. The key is
`(caller, client_msg_id)` and does not include the target session — reusing an
id for a different session returns the earlier result without delivering.
Bodies are limited to
64 KiB; `\r\n` is folded to `\n` and any other control character is refused
(`E_VALIDATE`). The text lands in the pane reconcile last saw Claude in, or
the session's active pane when that is unknown. An empty prompt with
`submit: false` is refused (`E_VALIDATE`); an empty prompt with `submit: true`
is a bare Enter that bypasses the blocked-session check.
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
it. Same errors as `session_transcript`. `session_tool_detail { session_id,
tool_use_id, claude_session_id? }` returns what a tool item leaves out —
one call's input and result, `{ id, name, input, edit, command, result,
is_error }` (`edit` is `{ file_path, old, new }` for Edit / MultiEdit /
Write), each text capped at 8 000 chars; the phone and a hub-paired desktop
read it when a tool row is expanded. Readonly, like `session_conversation`. `run_prompt { session_id, prompt, timeout_s?, max_chars?,
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

**Addressing and reply waits.** `send_message` also accepts `to_addr` (a
fleet address, `<fleet>/session/<host>/<name>`) as an alternative to
`to_session_id` — `to_addr` wins when both are set — and `wake: true` to
nudge an idle recipient's pane the same way `deliver` does (a working
session gets it from its own Stop hook; a blocked one is never typed into —
Enter there would answer whatever dialog is on screen). Repeat
`client_msg_id` to retry a send without delivering twice, the same
discipline as `send_prompt`. `wait_for_reply` (`{ session_id,
after_message_id?, timeout_s? }`) long-polls the inbox for the next message
newer than `after_message_id`, the same bounded-wait budget as
`wait_for_session`.

**Across a hub link.** When `to_addr` names another fleet
(`<fleet>/session/<host>/<name>`) and this hub has a live peer link to it,
`send_message` queues the message on the link's outbox and returns at once —
delivery happens on the dialer's next exchange, typically within a few
seconds. `deliver: true` is refused (`E_UNSUPPORTED`: "deliver types into a
pane; a hub never types into another fleet's panes") — there is no pane on
the other side of a link to type into. So is `kind: "question"`
(`E_VALIDATE`: a message from another fleet cannot hold a session's stop), a
recipient address over 256 bytes, and a `reply_to` whose parent came from a
third fleet (`E_INVALID`). The reply, once the remote session
sends one, arrives through the normal `wait_for_reply` / `inbox` path like
any other message, with `from_addr` set to the sender's
`<fleet>/session/<host>/<name>` and marked as untrusted input. A link going
`refused` (a revoked token, a fleet-id mismatch) does not by itself fail a
waiting message — its pending rows stay attached for up to 7 days so a
re-pair within that window still delivers them. Only removing the link
(`fleet-hub peer remove`) or the message outliving that 7-day retention
turns it into a `message_undeliverable` event on the sender's session
timeline instead of a reply. See `docs/hub.md` → *Link two hubs*.

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
5. **Hooks in `~/.claude/settings.json`** — merges fleet's `Stop`, `UserPromptSubmit`, `PostToolUse(EnterWorktree|ExitWorktree|TaskCreate|TaskUpdate|TodoWrite)`, `SessionEnd(logout|prompt_input_exit|other|clear|resume)`, `StopFailure`, `Notification(permission_prompt|elicitation_dialog|elicitation_url_dialog|quota_auto_resume_stale|quota_auto_resume_disabled|quota_auto_resume_fired)`, `PreCompact` and `PostCompact` hooks as Claude Code `type: "http"` hooks, leaving the user's own hooks alone. Each entry POSTs the hook payload to `http://127.0.0.1:<port>/hook` with `"headers": { "Authorization": "Bearer <host-token>" }` and a 5 s timeout — the token never appears in a process argv. `SessionStart` is the one event Claude Code refuses to fire as `type: "http"`, so it is instead installed as an async `curl` command hook that reads its bearer header from `~/.claude/fleet-hook.headers` (mode `0600`) rather than embedding it in the command string (see *Hook contract*). On a remote host the URL's `127.0.0.1:<port>` is the reverse tunnel's loopback end (step 6); on a `fleet-hub` daemon with a public URL, the URL is that public URL's `/hook` instead (e.g. `https://fleet.example.com/hook`) and no tunnel is used. Any older fleet entry for the same port (including the pre-0.3 `curl … /hook?token=` command form) is replaced, so re-running upgrades in place; the file and the headers file are written `0600`. Required for `safe_kill_session` to finalize, for real-time `idle` / `working` status, `turn_seq`, task completion and conversation tracking (`session_conversations`) on every host that runs Claude Code. **Settings → Install Hook (local)** performs only this step for the `local` host, using the `local` host token. **Hosts provisioned before the `UserPromptSubmit` hook existed must be re-provisioned** (no rotate needed) to get the busy signal; until then their status only flips to `working` on the next reconcile pass. **Hosts provisioned before the `SessionEnd` / `StopFailure` / `Notification` hooks existed must be re-provisioned** to get `stopped`, API-error turn completion and hook-driven `blocked`. **Hosts provisioned before the `SessionStart` / `PreCompact` / `PostCompact` hooks existed must be re-provisioned** to get conversation tracking across `/clear`, `/resume` and compaction (see *Hook contract*). **Hosts provisioned before the task-tool matcher (`TaskCreate|TaskUpdate|TodoWrite`) existed must be re-provisioned** to capture agent steps live; until then (the host reads `provision_stale`) the Stop hook backfills them from the transcript tail. The worker guard (orchestration §7.2) is a `PreToolUse(Bash)` command hook: a `case` prefilter posts the hook body synchronously (1 s connect, 3 s cap, `|| true`) only for a command that mentions `gh pr`, `git … push` or a tracker CLI, and prints the hub's answer — `permissionDecision: "deny"` for a mission's worker running a person's step, nothing otherwise. **Hosts provisioned before it existed must be re-provisioned** to get it.
6. **Reverse SSH tunnel** (remote hosts only, loopback hubs only) — starts an `ssh -R` tunnel so the remote host's `127.0.0.1:<port>` is forwarded to the central machine's MCP server. The server stays bound to `127.0.0.1` on the central machine; remote hosts reach it only through this authenticated tunnel. A `fleet-hub` daemon configured with a public URL skips this step entirely — every host already reaches the hub's public address directly.

**After provisioning, each host must restart Claude** to load the MCP server (skill files and CLAUDE.md are picked up live, but the MCP server entry requires a restart).

`provision_hosts` takes `host` (one alias; every active host when omitted) and `content_only` (steps 1, 2 and 5 only — skills, the CLAUDE.md block and hooks, with the host's existing token; no token minted, no `~/.claude.json` rewrite, no tunnel, no restart needed; a host without a token answers `E_NO_TOKEN`). Each provisioning records a fingerprint of the content it shipped, and `list_hosts` reports `provisioned_at` and `provision_stale` (provisioned with content other than this build's). Each managed skill dir carries a `.fleet-managed` marker; a git work tree that tracks files in those dirs (a dotfiles checkout) is refused (`E_INVALID`) unless `provision.force_git_tree` is on; untracked or ignored dirs pass.

Unless `provision.install_ag` is off, provisioning (full and `content_only`) also installs fleet's `ag` launcher: the embedded `tools/ag` tree is staged under `~/.local/share/fleet/ag-src` and its installer puts `ag` in `~/.local/share/ag` (linked from `~/.local/bin/ag`, never over a foreign `ag`) and adds a `cl` alias (`claude --yolo`) with its shim in `~/.local/bin` unless the host's ag config already defines `cl`. This step is optional: a failure is appended to the host result's `detail` (full and `content_only` runs alike; the hub's unattended refresh logs it) and provisioning continues. The installer never adds `cl` over a command the host already has (a `cl` elsewhere on `PATH`, or a `~/.local/bin/cl` ag did not write), and never edits a symlinked ag config. Panes then launch through the host's own `cl` if it has one, else `~/.local/share/ag/ag claude --yolo`, else `claude --dangerously-skip-permissions`.

**Upgrade heads-up:** the `ag` launcher is part of the provisioning fingerprint, so upgrading to this build makes every provisioned host `provision_stale`, and the hub's unattended content refresh installs `ag` and (where the host has no `cl`) a `cl` shim on every reachable host within about a minute of start. To opt out, set `provision.install_ag=false` right after upgrading.

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

### Host health

Every reconcile pass reads one health sample from each reachable host in the
same batched probe script (`df -Pk` of `$HOME` and `${TMPDIR:-/tmp}`, the
1-minute load, `MemAvailable` on Linux, the uptime) and, every 6 h, `tmux -V`
and `claude --version`. `list_hosts` carries the sample on each row
(`disk_home_free_kb`, `disk_home_total_kb`, `disk_tmp_free_kb`, `load_1m`,
`mem_avail_kb`, `uptime_secs`, `health_at`), the versions stamp
(`claude_version_at`), the last accepted hook from the host's own token
(`last_hook_at`, rewritten at most once a minute) and, for an agent host, the `agent_version` its last hello
reported. `fleet_health.hosts[]` judges them per host:

| Field | Meaning |
|---|---|
| `disk_home_pct` | Used percent of `$HOME`'s filesystem, when sampled. |
| `disk_low` | `disk_home_pct >= health.disk_low_pct` (default 90). |
| `claude_behind` | More than `health.claude_max_behind` (default 30) patch releases behind the fleet's newest version among stamps younger than 24 h. |
| `agent_behind` | An agent host whose agent is not the hub's version (the live registry outranks the stored hello). |
| `hooks_silent` | Reachable, with a live non-external session, and no hook from its token within `health.hooks_silent_secs` (default 1 h). |

The desktop's "older than the fleet" mark trusts a `claude_version_at`
younger than `health.version_max_age_secs` (default 24 h). `move_session`
refuses (`E_INVALID`, `reason: target_disk_low`) a target whose last sample
cannot take the source worktree plus 1 GiB of headroom; an unsampled target
proceeds.

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
| `UserPromptSubmit` | all | `session_id`, `transcript_path` | `claude_status = working`, `idle_since` cleared; a non-current id rebinds the row (fallback for hosts missing `SessionStart`) | — |
| `Stop` | all | `session_id`, `transcript_path`, `cwd` | `idle`, `turn_seq + 1`, `last_stop_at`; triggers safe-kill and task-marker checks | `turn_done` |
| `StopFailure` | all | `session_id`, `error`, `error_details` | same as `Stop` | `stop_failure` — `<error>[: <details>]` |
| `SessionEnd` | `logout\|prompt_input_exit\|other\|clear\|resume` | `session_id`, `reason` | `clear`/`resume`: conversation closed, row marked awaiting a rebind, status untouched (the process lives on under a new id); other reasons: `stopped`, `idle_since` started, stuck cleared | `conversation_ended` (clear/resume) or `session_end` (other) — reason |
| `SessionStart` | all (command hook) | `session_id`, `source`, `model` | `source = compact`: counts the compaction only, no rebind; otherwise the row rebinds onto the new conversation, which becomes current | `conversation_started` — source |
| `PreCompact` | all | `session_id`, `trigger` | `current_activity = compacting` on the current conversation | `compact_started` — trigger |
| `PostCompact` | all | `session_id`, `trigger` | counts the compaction (deduped against `SessionStart(compact)`), clears `compacting`, marks context stale for the current conversation | `compact_done` — trigger |
| `Notification` | `permission_prompt\|elicitation_dialog\|elicitation_url_dialog` | `session_id`, `notification_type` | `blocked` | `notification` — type |
| `Notification` | `quota_auto_resume_stale` | same | `blocked`, `stuck_kind = press_enter` | `notification` — type |
| `Notification` | `quota_auto_resume_disabled` | same | `blocked` | `notification` — type |
| `Notification` | `quota_auto_resume_fired` | same | `working`, stuck cleared | `notification` — type |
| `PostToolUse` | `EnterWorktree\|ExitWorktree` | `tool_name`, `tool_input`, `tool_response` | worktree row registered / removed | — |
| `PreToolUse` | `Bash` (command hook, prefiltered) | `tool_name`, `tool_input.command` | none; answers `deny` for a mission's worker running a person's step (orchestration §7.2) | — |

Every hook write stamps `last_hook_at`; a reconcile pass that started before that
stamp never overwrites the hook's status with its pane heuristic. Fleet never
writes `allowedHttpHookUrls` — defining it at user level would block every other
http hook on the host.

**Conversations.** Each session row tracks the sequence of Claude Code
conversations (`claude_session_id`s) it has run: `/clear`, `/resume` and
compaction each replace the transcript's session id while the underlying
process and tmux pane continue, and fleet keeps one row per id —
`session_conversations` lists them, `session_conversation { claude_session_id
}` reads an earlier one (see *Steering & observing* above). `SessionStart` is
the event that opens a new conversation, but it is also the one event Claude
Code refuses to fire as `type: "http"`, so fleet installs it as an async
`curl` command hook instead of the `http` hooks used everywhere else. That
command POSTs the same JSON body to `/hook` with the bearer header supplied
via `curl -H @"$HOME/.claude/fleet-hook.headers"` (a single `Authorization:
Bearer <host-token>` line, mode `0600`, written by the same provisioning /
local-install code path as `settings.json`) so the token never appears in the
command string itself, plus `-H "X-Fleet-Pane: ${TMUX_PANE:-}"` so `resolve_hook_row`
can match the row by pane directly. With `work.session_start_context` on,
the hooks are instead installed as the synchronous form, which also sends
`X-Fleet-Sync: 1`: only a SessionStart carrying that header can have its
answer read by Claude, so only then does fleet include a pending handover
brief in it and stamp the brief delivered — the async form gets the work
context alone and the brief waits for the next `UserPromptSubmit`. Hosts pick up the `SessionStart` /
`PreCompact` / `PostCompact` entries only once re-provisioned —
`provision_hosts` refreshes them on its next run; the local host installs them
automatically on app start.

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
- **Paired clients.** A phone or browser holds a named, revocable client
  token (`full` or `readonly`) obtained through `/pair`; only its SHA-256 is
  stored. A client is never the master and never reaches fleet admin, so it
  cannot pair another device or revoke the operator's own client, and it
  cannot ask for an unmarked prompt (`raw: true` is the master token's
  alone — text typed on a phone reaches an agent marked, naming the client it
  came from, unless the operator has *trusted* that client with
  `set_client_trust` or `pair_client { trusted: true }`, in which case its
  text goes through unmarked). `revoke_client` takes effect on the next
  request, and
  an open `/events` stream ends at the next heartbeat. See `hub.md` →
  *Clients*.
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
  master token may pass `raw: true` to skip it, and a paired client the
  operator has trusted (`set_client_trust`) is delivered without it.
  `send_prompt`'s `keys` presses one named key without text — Enter, Escape,
  Tab, BTab (Shift-Tab), the four arrows (Up, Down, Left, Right), C-c, a Ctrl
  letter from a closed list (C-a/b/d/e/f/g/h/k/l/n/o/p/r/t/u/v/w/x/y: never
  C-z, C-s, C-q, or Tab and Enter under another name) or a digit 1–9; it is
  never marked. The argument's schema enumerates every key, so a client can
  tell a hub that takes the arrows from an older one (the phone's key bar,
  redesign 14.14). The Settings toggle **"Ask me
  before agents broadcast, kill sessions, delete worktrees or write the
  clipboard"** (`mcp.confirm_destructive`, off by default) makes
  `broadcast_prompt`, `kill_session`, `delete_worktree`, `set_clipboard`,
  `repair_session`, `cancel_task` and `move_session` (not its `dry_run`) return `E_CONFIRM_REQUIRED` with a one-time `confirm_nonce`;
  approve the request in the desktop dialog, then retry the call with that
  nonce. The nonce is bound to the call's tool and to EVERY argument — free
  text as a readable prefix plus a digest of the whole, a prompt or brief as
  a digest only — so an approval cannot be replayed with different
  arguments, and is single use. **The operator** (the UX agent's own client
  token, `ux-agent`) is gated whatever the toggle says (work graph M9.7,
  decision D12): its session starts and restarts — `new_session`,
  `new_shell_session`, `new_bg_session`, `spawn_review`, `dispatch_task`
  with `new_worker`, `restore_host_sessions` (not its `dry_run`),
  `recreate_session`, `restart_session`, `work_link` `start` / `resume` —
  its `safe_kill_session`, and every tool above return
  `E_CONFIRM_REQUIRED` until a person approves them: on the desktop in its
  dialog or Control's cards, and on a hub from the owner's paired device,
  which lists the waiting calls with `mcp_confirms` (each with `nonce`,
  `tool`, `summary`, `caller`, `operator`, `asked_at`) and answers one with
  `answer_mcp_confirm` (`nonce`, `approved`; `false` when it was already
  answered or expired). `control_route` (redesign step 9.9, Jev K2,
  `decide.jev.control_route`) tells the owner's device where a message just
  sent in Control goes: `propose {text}` answers `{outcome: proposed | ask |
  none, target?, proposal?, targets, run_id?}` over the active missions and
  running sessions, and `follow {run_id, chosen}` records the person's pick. Both are the owner's own device only, never the
  operator; each change sends an empty `confirm:changed` event. Every
  other caller is unaffected: for them these tools are not gated.
- **Handoff receipts.** Each successful call of the operator that hands
  work on leaves a receipt (redesign 9.3): a prompt or task to a session
  (`send_prompt`, `queue_prompt`, `run_prompt`, `dispatch_task`), a new
  session, a new or started mission, a created task, a proposed tree of
  subtasks (`work_link` `create` / `propose_tree` / `mission_*`). Control
  draws them as chips and cards. `control_handoffs` (`limit?`, default 50)
  lists them newest first, each with its target's state now (a mission's
  name and state, a task's or the tree's items with their status and
  proposal state); like `mcp_confirms` it answers the owner's own device
  only, since a receipt quotes the operator's prompt. Each new receipt sends
  an empty `handoff:changed` event. No other caller's calls leave one.
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
