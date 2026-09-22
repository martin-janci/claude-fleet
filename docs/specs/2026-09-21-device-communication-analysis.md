# Device communication analysis — fast and flawless

Date: 2026-09-21. HEAD `5fa119f7` (v0.2.31). Six independent read-only
expert reviews, one per link, merged here. Every finding was verified
against the file and line it cites; a spot-check of the twelve most
consequential claims was repeated by hand before this document was written.

Goal, in the operator's words: communication between devices must be
**fast** and **flawless**.

## Verdict

The transport foundations are sound: one audited shell quoter, ControlMaster
with BatchMode and keepalives, kill-and-reap on every SSH child, a shared
jittered backoff for all three dial-again loops, TLS verified everywhere
with no skip-verify escape hatch, emit-after-commit on the event bus, and a
deterministic hub/agent handshake with version negotiation. Nothing in the
communication layer is unsafe.

It is not yet fast, and it is not yet flawless. The gaps cluster into six
themes that recur across links:

| # | Theme | Where it bites | Effect today |
|---|---|---|---|
| 1 | **No ack, no idempotency** | send_prompt, every desktop→hub mutation, hooks | prompts silently mis-delivered or doubled; sessions created twice |
| 2 | **Liveness measured in whole frames / whole calls** | hub↔agent, SSH probe budget | big transfers killed as "silence"; busy hosts flap to unreachable |
| 3 | **One process / one connection per operation** | SSH (`bash -lc` per call), desktop→hub (TCP+TLS per call) | O(N) round-trips per tick; 90–120 ms floor per click over WAN |
| 4 | **Gaps are healed by re-listing, not by replay** | event bus (no seq / `Last-Event-ID`), subscribe-after-list | every blip costs 4 serial list calls; changes in the window are lost |
| 5 | **Failure is noticed late and silently** | agent link (60–120 s), SSH master death, probe errors discarded | 20–50 s of stale UI; nothing in logs |
| 6 | **Polling survives where push exists** | conversation panel (5 s / 15 s / 2 s) in hub mode | dominant hub traffic, a TLS handshake + SSH tail per poll |

## Findings by link

Severity: C = Critical, H = High, M = Medium, L = Low. Paths are relative
to the repo root; `core/` = `crates/fleet-core/src/`.

### A. Prompt delivery and status (the last mile)

| Sev | Finding | Evidence |
|---|---|---|
| C | Prompt bodies go through `send-keys -l` with no normalisation. `\r` from a CRLF client submits mid-body (three submissions for one prompt); an ESC or Ctrl-C byte interrupts or kills the recipient. Reachable from paired clients via `send_message{deliver}` and `broadcast_prompt` without any confirm gate. | `core/service/sessions/prompt.rs:20-30`; `validate.rs:39` (`no_control`) is never applied to a body |
| H | No busy/blocked guard. Enter sent to a `blocked` session selects the highlighted dialog option ("Yes"). To a `working` session the prompt is queued but `turn_seq_before` names the current turn, so `wait_for_session{turn_gt}` reads the wrong reply. | `core/mcp/tools/messaging.rs:19-42`, `support.rs:842-862`; only `run_prompt` checks readiness (`support.rs:258-274`) |
| H | Wrong-pane delivery: target is `=<session>:` (active pane), never `tmux_pane_id` which the row already has. A split window types the prompt into the shell. | `core/tmux.rs:24-26`; `tmux_pane_id` unused in `prompt.rs`, `messages.rs`, `tasks.rs` |
| H | `delivered: true` means "send-keys exited 0". The Enter race is a 150 ms guess; a large paste can absorb the newline and sit staged. The `UserPromptSubmit` hook already is the perfect ack and is not correlated. | `prompt.rs:26-27`; `support.rs:857-861`; `hooks.rs:586-617` |
| M | Body rides the command line: tmux IPC and Linux `MAX_ARG_STRLEN` (128 KB) cap it with an opaque `E_TMUX`. | `prompt.rs:23,64,73-78` |
| M | Status flaps: `capture-pane -S -8` without `-E` returns the whole visible screen plus 8 lines; `Working` is a bare substring match for `esc to interrupt` anywhere in it, and every pass overrides a hook stamp unless the pass started before the hook. A session quoting that string flips `idle`→`working` every 20 s. | `core/service/sessions/reconcile.rs:14,416,574-576`; `pane_intel.rs:572-608`; `store/reconcile.rs:277-280` |
| M | Hooks are synchronous HTTP with a 5 s timeout, no retry, no spool. Desktop asleep / hub restarting: 10 s added per turn and every `Stop` in the window is lost, so `turn_seq` never bumps and waiters time out. | `core/service/hooks_install.rs:23,33-42,117-127`; only `SessionStart` is `async` (`:159`) |
| M | No idempotency key on `send_prompt` / `send_message`; a retried call delivers twice. | `messaging.rs:19-42`, `messages.rs:90-198` |
| M | `blocked` clears only on the next pass (≤20 s); no `PostToolUse` hook reports "dialog answered". | `hooks.rs:712-757` |
| L | `send_message{deliver}` records `prompt_sent`, overwrites `last_prompt`, and can rename an unnamed recipient from the marker line. | `messages.rs:176-186`, `prompt.rs:110-127` |
| L | `capture_pane` returns `Ok("")` on a non-existent pane; `capture_session` and the panel show empty for a dead session. | `tmux.rs:280-285,523-527` |
| L | Stop payload's `last_assistant_message` is ignored for task completion; one extra SSH transcript read per Stop. | `hooks.rs:551-556`, `tasks.rs:596-646` |

### B. SSH and tmux transport

| Sev | Finding | Evidence |
|---|---|---|
| H | Per-host probe is **5–6 + N** serial `ssh … bash -lc` executions (identity, list, `claude agents`, oauth, mtimes, then one capture per session). `new_session`/`kill_session` end with the same chain. 10 sessions ⇒ 15–16 execs ⇒ 1–8 s per tick per host. | `reconcile.rs:1186-1216,409-429`; `tmux.rs:381-390` |
| H | `HOST_PROBE_TIMEOUT` (30 s) equals the per-call wall clock (`max(3×connect, 30 s)`), so the outer timeout always fires first: `maybe_reset_master` is never reached from the tick, and a busy host or a slow `claude agents` cold start is written `reachable=false`. | `reconcile.rs:22-27,1226,1238`; `ssh.rs:33,234-236`; `tmux.rs:387` |
| H | `bash -lc` per call on a zsh/macOS host misses the login PATH (`tmux`, `claude`, `cl` not found); `cl` is a user alias never provisioned or checked, so a remote `new_session` silently falls through to a bare shell. Remote `new-session` forwards only `COLORTERM/TERM/LANG`, local forwards `PATH`. | `tmux.rs:381-390,419-424,721-729,770-776`; `hosts.rs:375-403` |
| H | A failed or timed-out `claude agents --json` is `[]`, and `reconcile_agent_rows` prunes every bg/external row not in it. One exit-255 ghosts every background session for a cycle; two in a row delete them. | `tmux.rs:544-552`; `reconcile.rs:1010-1087`; `store/sessions.rs:194-197` |
| M | No retry after the ControlMaster dies (sleep/wake, roaming): ~10 s of exit-255 failures become "unreachable", `E_TMUX` on send, aborted `new_session`. | `ssh.rs:220-223,796-798`; no retry in `ssh.rs`/`prompt.rs`/`reconcile.rs` |
| M | `ServerAliveInterval=5 × CountMax=2` on a master shared with the user's PTY: a 10 s Wi-Fi stall drops the terminal and every probe together. | `ssh.rs:220-223`; `src-tauri/src/pty.rs:323-368` |
| M | Reverse tunnel argv has no `ConnectTimeout`/`BatchMode`; a black-holed connect waits the OS default (75 s–2 min) before backoff, then `HEALTHY_AFTER` adds 60 s. Hooks ride this tunnel. | `core/service/tunnel.rs:34,170-197` |
| M | `ensure_remote_project` clones into the final path under a 360 s wall clock; a cancel leaves a partial `.git` that the `[ ! -d .git ]` guard then respects forever. | `lifecycle.rs:119-126,163-171` |
| M | Conversation panel re-reads up to 4 MiB of transcript every 5 s per open panel, uncompressed. | `transcript.rs:34,102`; `conversation.ts:73,793` |
| M | tmux calls read output uncapped; `scrollback_lines` from MCP is passed straight to `capture-pane -S -<n>`. | `ssh.rs:706-709`; `prompt.rs:414-441`; `session_ops.rs:272` |
| L | Local tmux calls have no timeout or `kill_on_drop`; ControlPath can exceed the macOS 104-byte socket limit. | `tmux.rs:275-279,604-611`; `ssh.rs:180-194` |

### C. Hub ↔ fleet-agent WebSocket

| Sev | Finding | Evidence |
|---|---|---|
| C | Liveness counts complete frames; each message is one WS frame (up to ~267 MiB) on a FIFO writer that pings and pongs share. Any transfer longer than 60–90 s (a 150 MiB `move_session` upload on a 20 Mbit/s uplink) is declared silence: connection dropped, every in-flight command SIGKILLed on the agent, move failed. The 300 s `UPLOAD_WALL_CLOCK`/`SEND_TIMEOUT` can never be reached. | `core/agent/ws.rs:96,461-462,705-738,819-851`; `crates/fleet-agent/src/conn.rs:47,520-521,591-598,689-728`; `fleet-proto/src/lib.rs:230,242` |
| H | Any disconnect kills all in-flight work: agent SIGKILLs children, hub fails every waiter `E_AGENT_OFFLINE`, nothing is retried or resumed. A hub deploy or a tunnel blip kills every command on every agent host. | `conn.rs:603-605`; `registry.rs:85-88,166-179,295-296` |
| H | Dead peer noticed in 60–90 s (hub) / 90–120 s (agent); no `SO_KEEPALIVE` anywhere; no `TCP_NODELAY` on the hub side. During the blind window every routed call burns its full wall clock. | `ws.rs:89-96`; `mcp/listener.rs:105-106`; `mcp/mod.rs:593-599`; `registry.rs:284-314` |
| M | 200 MiB payloads are read, base64'd, JSON-encoded and parsed synchronously on tokio workers with 3–4 in-memory copies. | `transport.rs:338-368`; `ws.rs:722,863` |
| M | Unbounded outbound queues; the agent releases its concurrency permit before the result is sent, so a stalled hub reader can OOM the agent. | `registry.rs:66`; `ws.rs:525-526`; `conn.rs:487,796-811` |
| M | Two agents holding one token replace each other at 1 Hz forever, each round aborting all pending calls. | `registry.rs:166-168`; `conn.rs:1026-1028` |
| M | Small requests queue behind large ones on the single FIFO writer. | `ws.rs:705-713` |
| L | Two SQLite reads under the global store mutex per routed request; `Instant + Duration` overflow on an absurd `timeout_ms`; 60 s backoff cap after a hub outage; agent writer has no per-send timeout. | `router.rs:87-105`; `ws.rs:982`; `exec.rs:100`; `conn.rs:52,692` |

### D. Desktop (hub-client mode) ↔ hub

| Sev | Finding | Evidence |
|---|---|---|
| H | Client `CALL_TIMEOUT` is 30 s; the hub's own tool deadlines are 60 s (quick) and 300 s (lifecycle). `new_session`, `kill_session`, `restart_session`, `repo_commit` … are reported failed while the hub completes them; the user retries and gets duplicates. Only `move_session` is handled (`moves.ts:100`). | `src-tauri/src/backend/remote.rs:1046-1063`; `core/mcp/tools/support.rs:943,948` |
| H | One TCP + TLS handshake per command with `Connection: close`, response framed by EOF; no pool, no `TCP_NODELAY`, no compression. ≥3 RTT + handshake floor per click; startup is 6 handshakes. | `remote.rs:858,1016-1039,1080,1123` |
| H | Hub mode still polls: the conversation panel calls the routed `session_conversation` (an SSH transcript read on the hub) every 5 s working / 15 s quiet. This is the dominant hub traffic. | `src/lib/conversation.ts:73,793,803-805`; `ConversationPanel.svelte:453-470` |
| M | No connect timeout separate from the call bound; no circuit breaker while the bridge already reports `Offline`. Alt-tab into the window with the hub down = two 30 s hangs. | `remote.rs:317,1016-1039`; `connection.rs:57-62`; `App.svelte:255-266` |
| M | Cancellation stops at the process boundary: `cancel_command` is `SameInBoth` but a routed call has no token and the hub never learns the caller left. | `backend/verdicts.rs:944-949`; `remote.rs:804-806,1067` |
| M | Dead stream detected only by the 37.5 s idle timeout; nothing nudges the bridge on wake/focus/network change. | `backend/events.rs:155-162,779` |
| M | Resync is four serial list calls run inside `pump` with the socket unread, so the hub's broadcast lags, which triggers another resync (the code documents the loop). `Connected` is reported before the resync; a failed list is never retried. | `backend/events.rs:213-239,410-437,690-770` |
| M | SSE partial-line buffer is unbounded; `Dechunker` is O(n²) on multi-MB bodies. | `core/mcp/wire.rs:68-83`; `backend/http1.rs:94,110` |
| L | `401` on `GET /events` is retried forever as "Cannot reach the hub"; `Seen` mutex `.expect` can kill the bridge task on poison. | `backend/events.rs:335-339,670,877-887` |

### E. Event bus and fan-out

| Sev | Finding | Evidence |
|---|---|---|
| H | The optimistic-merge guard `isStale` keys on `last_activity_at`, which only reconcile writes. Two writes within one tick compare equal and last-arrival wins. Concrete regression: `new_session` calls `set_started_at` after reading the row and returns `started_at: null`; reconcile's diff never re-emits it, so the sidebar's "started" stays blank until a full re-list. | `src/lib/sessions.ts:158-161`; `store/reconcile.rs:363,453-455`; `lifecycle.rs:618-688` |
| M | `BROADCAST_CAPACITY = 256`, no sequence number, no `id:` on SSE frames, no `Last-Event-ID`, nothing replayed. Every gap costs a full re-list; a reconcile burst over 256 events lags every subscriber every tick. | `core/events.rs:453-470`; `mcp/events_route.rs:265-268,385-394`; `wire.rs:119-121` |
| M | App subscribes to row events **after** the initial lists resolve (19 `listen` round-trips); Tauri drops emits with no listener. A `Stop` landing in that window is lost until the row changes again or the 30 s focus refetch. Tasks are already done in the right order. | `src/App.svelte:191-236`; `app_events.rs:32-33` |
| M | The documented client protocol ("list, then follow the stream") is backwards; `emit` returns early with zero receivers, so a phone that lists then subscribes loses the window with no `lagged`. | `events_route.rs:3-5`; `docs/hub.md:623-625`; `events.rs:482-485` |
| M | Frontend `flush` has no per-event fault isolation; one throwing handler drops the whole 16 ms batch. `payload_fits` exists only on the hub bridge. | `src/lib/events.ts:110-205`; `backend/events.rs:531-570` |
| L | `host:probed` fires for every host every pass (`last_pinged_at` defeats the diff); cross-kind order is lost inside one flush; `AppHandleEventBus` uses an unbounded `std::sync::mpsc`. | `store/reconcile.rs:241-243`; `events.ts:197-204`; `app_events.rs:23-28` |

### F. Cross-cutting resilience

The full inventory of 62 timeouts, backoffs and buffer capacities is in the
resilience review; the points that matter:

- Every remote await found is bounded by some timeout; all three reconnect
  loops share one jittered backoff; no unjittered retry loop exists.
- Four unbounded channels (`ws.rs:525-526`, `conn.rs:487`, `app_events.rs:28`)
  and one unbounded buffer (`wire.rs:69`).
- Silence thresholds differ per link (hub→agent 60 s, agent→hub 90 s,
  desktop `/events` 37.5 s); connect timeouts to hosts vary per call site
  (5 s or 10 s) with no named constant.
- Agent link transitions are not structured: `[agent] disconnected` carries
  neither reason nor lifetime; `reconcile.rs:806` discards the probe error
  (`Err(_e)`) with no log line; `fleet_health` has no `agents_connected`.
- `authorize` does two SQLite reads and a write attempt under the global
  store mutex on every request, including every hook POST.
- A cancelled multi-step remote script (`new_session`: mkdir + worktree +
  tmux) keeps running on the host after ssh dies; nothing sends a remote
  kill, so a cancel can leave a worktree with no session and no row.

## What is already done well (do not "fix" into something worse)

- **Handshake and versioning** on the agent link: `welcome` before anything,
  version window, private close code 4001, `force_max` backoff, replacement
  generations, cancel semantics with process-group kill, bounded `SeenIds`
  replay defence, log-injection hardening of every peer string.
- **SSH child discipline**: biased `select!` cancel > deadline > exit,
  explicit kill + reap, `in_flight` counter and `-O check` before a master
  reset, one `shell::quote` with property tests, parallel host fan-out with
  per-host store lock windows and transactional `apply_host_reconcile`.
- **Event bus**: emit-after-commit is real (`Store::atomically`, pinned by
  `reconcile_batch_rolls_back_and_emits_nothing_on_error`), the BE-11 diff
  suppresses no-op events at the source, the 16 ms frontend batch turns a
  burst into one notification, `send_prompt` records only after the tmux
  send succeeded.
- **Hub-client bridge**: contract gate before any row is trusted, name
  allowlist, `payload_fits`, one resync per connection, backoff not reset on
  `lagged`, split-frame and UTF-8-boundary decoding, token redaction at every
  error surface, ~60 tests.
- **Hooks**: `Stop`/`UserPromptSubmit` bump `turn_seq` and emit; the
  in-flight guard stops a slow pass clobbering a fresh hook; the token never
  appears in argv; paired clients are refused on `/hook`.
- **Transport security**: TLS verified with platform trust everywhere, the
  loopback rule written once in `fleet-proto/net.rs`, constant-time token
  compare, per-caller stream caps, graceful ordered shutdown on the hub.

## Roadmap

Ordered by impact on the goal per unit of change. Each phase is independently
shippable and testable; none depends on a wire-format bump except phase 4.

### Phase 1 — Make delivery correct (flawless, last mile)

Landed 2026-09-21 on branch feature/device-communication-fa2aec (plan: docs/superpowers/plans/2026-09-21-device-communication-phase-1.md). Deferred from item 4: idempotency keys on every hub-routed mutation — needs a wire change per mutating tool; send_prompt has client_msg_id.

1. **Replace the send primitive** (`prompt.rs:20-30` + a new
   `SshClient::run_with_stdin`): normalise `\r\n`/`\r` to `\n`, reject
   control bytes other than `\n`/`\t` (`E_VALIDATE`), ship the body on stdin
   via `tmux load-buffer -b fleet-<nonce> -` + `paste-buffer -p -d -b … -t
   '%<pane_id>'` (bracketed paste, pane-id target from `tmux_pane_id`,
   `=name:` as fallback), then Enter. Removes the CR split, the control-byte
   injection, the wrong-pane delivery, the argv size caps and the Enter race
   in one place; every caller inherits it.
2. **Gate and ack in `deliver_prompt`** (`support.rs:842-862`): refuse
   `blocked`/stuck rows with `E_INVALID_STATE` unless `force: true`; for
   `working` return `queued: true` and `turn_seq_before = turn_seq + 1`;
   wait ≤1.5 s for the `UserPromptSubmit` stamp and return `acked`; optional
   `client_msg_id` with a 10-minute `(caller, id) → result` dedupe table.
   Apply the same filter in `select_targets` and `send_message{deliver}`.
3. **Fix the merge guard**: migration adding `row_version` to `sessions`
   and `hosts` with an `AFTER UPDATE` trigger, `isStale` on `row_version`,
   every service function re-reads the row after its last write, and a
   `RecordingEventBus` test that the returned row equals the last emitted
   one. Subscribe to row events **before** the initial lists in
   `App.svelte`, and turn `sessions.set` into a merge so an in-flight list
   cannot clobber an event.
4. **Client timeouts must dominate hub deadlines**: derive the desktop's
   per-tool bound from `guard::TOOL_POLICIES` (+10 s), add a 5 s connect
   timeout, a distinct `E_HUB_TIMEOUT` ("may still complete on the hub"),
   and a breaker that refuses instantly while the bridge is `Offline`. Add
   an idempotency key on hub-routed mutations and map the timeout on a
   mutation to "outcome unknown, refreshing" rather than a retryable error.

### Phase 2 — Make the SSH path O(1) per tick (fast, hosts)

Items 5–7 landed 2026-09-22 on branch `feature/device-communication-phase-2`
(plan: `docs/superpowers/plans/2026-09-22-device-communication-phase-2a.md`).
Item 8 is Phase 2b. Deviations: no `E_CL_MISSING` — main's `cl` fallback
(331d49f8) covers a missing `cl`; the toolchain is resolved from the user's
interactive login shell and cached for the process lifetime, not persisted.

5. **Batch the probe** into one delimited script per host (identity, list,
   all pane tails via `| tail -n 8`, oauth), parsed in Rust; `claude
   agents --json` on a slower cadence and returning `Option` so a failure
   never prunes bg rows. Derive the probe budget from the work and make
   `maybe_reset_master` reachable from the probe-timeout arm.
6. **Resolve the toolchain once per host** (`$HOME`, login `$PATH`,
   absolute `tmux`/`claude`/`cl`) from the user's real login shell at
   `add_host`/first contact, cache it beside `homes`, run tmux by absolute
   path under `sh -c`, forward `PATH` on remote `new-session`, and refuse
   `new_session` with `E_CL_MISSING` when `cl` is not executable.
7. **Second chances**: retry once after `reset_master` on a mux-failure
   exit 255; keepalive 15 s × 3 (or a separate ControlPath for the PTY);
   `ConnectTimeout=10 BatchMode=yes` on the tunnel; clone into a temp dir
   and `mv` on success; clamp `scrollback_lines` and cap tmux output.
8. **Status heuristic subordinate to hooks**: analyse only the bottom N
   lines, derive `Working` from `spinner_line()`, let a pass override a
   hook stamp younger than 30 s only with `blocked`/stuck evidence. Cut
   http hook timeouts to 1–2 s, mark non-blocking events `async`, spool
   failed hook POSTs to `~/.claude/fleet-hook.spool` and drain the spool in
   the batched probe script. Install `PostToolUse` to clear `blocked`.

### Phase 3 — Make the desktop↔hub link live (fast, hub mode)

9. **Persistent connections**: a small keep-alive pool (or hyper-util's
   pooled client over the existing rustls connector), responses framed by
   `Content-Length`/0-chunk, `TCP_NODELAY` on both ends (`tap_io` on both
   listener branches), gzip on `/mcp` excluding `/events`.
10. **Push, not poll**: gate the transcript poll on `turn_seq` from
    `session:updated` with a 60 s safety poll; later, a
    `session:transcript` delta event from the hook path.
11. **Resync that does not feed itself**: `tokio::join!` the four lists,
    run them concurrently with a reader that buffers frames, retry a
    failed list once, report `Reconnecting` until the backfill lands; a
    `hub_reconnect_now` nudge from focus/wake; classify 401/403 on
    `/events` as `Unauthorized` with `force_max`.

### Phase 4 — Replay and chunking (flawless under load; wire changes)

12. **Sequence numbers on the bus**: `seq: u64` on `EventMessage`, sent as
    SSE `id:`, a 2048-entry replay ring, `Last-Event-ID` replay on `GET
    /events`, `lagged` only when the cursor is older than the ring; the
    bridge re-lists only on a true hole. Raise `BROADCAST_CAPACITY` to
    ≥2048. Docs and the mobile spec: "open `/events`, wait for `ready`, then
    list". Emit `host:probed` only on a real change.
13. **Chunked agent protocol (proto v2, `MIN_SUPPORTED_PROTO` stays 1)**:
    `upload_begin/chunk/end` and `result_begin/chunk/end` at 1 MiB, control
    frames first and round-robin between bulk streams in the writer, socket
    limits back to 16 MiB. `HEARTBEAT` 10 s / 3 misses, TCP keepalive on
    both sockets, on-demand ping when a request goes to a quiet connection.
14. **Survive reconnects**: children outlive the socket on the agent with a
    bounded unclaimed-results queue flushed after the next `welcome`; a
    per-alias pending table with a reconnect grace on the hub; refuse a
    duplicate-host newcomer with close code 4002 instead of flapping; bound
    the outbound queues (`mpsc::channel(64/256)`, permit held through the
    send, `E_AGENT_BUSY` on full).

### Phase 5 — Observability and cancellation

15. Emit a host row event from `ws.rs` on agent connect/disconnect/replace;
    `agents_connected` in `fleet_health`; log the `End` reason and lifetime
    on disconnect; log the probe error at `reconcile.rs:806` and store
    `last_probe_error` on the host row.
16. Race routed calls against the caller's `CancellationToken` and add a
    hub-side cancel keyed on the operation id; run multi-step remote scripts
    under `setsid` with a `HUP` trap so a dropped ssh does not leave a
    half-created session.

## Tests to add (the gaps every reviewer named)

- Delivery against real tmux in `scripts/hub-e2e.sh`: multi-line body with
  the marker, CRLF body, diacritics, a 64 KB body, an ESC byte, a two-pane
  window, a `blocked` row, and exactly one `UserPromptSubmit` per send.
- A `RecordingEventBus` test that every command's returned row equals its
  last emitted row; an `isStale` test; a startup-ordering test that an
  event between `listen` and `loadSessions` wins; a throwing handler inside
  `flush`.
- A transfer whose wire time exceeds a heartbeat over a throttled proxy;
  a black-holed socket (`Proxy::blackhole`) asserting failure within
  `MISSED_HEARTBEATS × HEARTBEAT`; two agents on one token; results piling
  behind a stalled hub reader.
- Client timeout > hub deadline for every routed tool (intersect `VERDICTS`
  with `TOOL_POLICIES`); a peer that keeps the socket open after a complete
  body; a slow/failing resync while frames arrive; 401 on `/events`.
- A failed `claude agents --json` keeps bg rows; a probe budget that
  survives N sessions × per-call latency; a real ControlMaster death
  mid-call; the login-shell PATH mismatch; a missing `cl`.
