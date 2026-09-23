# Fleet mesh — addressing and delivery

Date: 2026-09-22. Base `332284c9`. Cycle 1 of three.

Goal, in the operator's words: it must be possible for **everyone to talk to
everyone**; where two fleets each have a hub, via a **hub↔hub** relay;
communication must be **fluid and very fast**; and a fetch already made must
be repeatable so that it returns **only what is new**.

That is not one system. It is three, and this spec is the first:

| Cycle | Scope | Status |
|---|---|---|
| **1 — addressing and delivery** | a durable identity for every endpoint, a string address, two-way `/hook`, blocking wait, idle wake-up | **this spec** |
| 2 — smart caching | server-remembered cursors on the fetch tools, built on phase 4 item 12 of the device-communication roadmap | not started |
| 3 — hub↔hub federation | hub identity, peer links, routing a foreign address, loop prevention | not started; needs cycle 1's addressing |

## Relation to prior work

`docs/specs/2026-09-21-device-communication-analysis.md` is an audit of this
same layer with the same goal ("fast and flawless"), carrying a five-phase
roadmap. **Phases 1 and 2a are merged.** This spec is additive to that
roadmap, not a replacement, and three interactions are load-bearing:

1. **Phase 1 already rebuilt the send primitive.** Bodies go to stdin via
   `tmux load-buffer` + `paste-buffer -p -d` against `tmux_pane_id`, control
   bytes are rejected, a `blocked` row is gated with `E_INVALID_STATE`, the
   ack is read from the `UserPromptSubmit` stamp, and `client_msg_id`
   dedupes. Since `d5084346`, `send_message{deliver}` routes through
   `sessions::send_system_prompt` (`label: false`), so it no longer renames
   the recipient from the message body. The keystroke path is therefore *not*
   the weak link this spec replaces — it has exactly one hole left, an idle
   session nobody prompts, and this spec closes that one.
2. **Phase 4 item 12 is half of cycle 2.** `seq: u64` on `EventMessage`, SSE
   `id:`, a 2048-entry replay ring and `Last-Event-ID` replay are the
   delta mechanism at the event-bus level. Cycle 2 must build on it, not
   erect a second parallel cursor scheme.
3. **Phase 2b item 8 collides with this spec and must carve out an
   exception.** It proposes cutting hook timeouts to 1–2 s and marking
   non-blocking events `async: true`. An `async` hook's response body is
   discarded. Delivery here rides the response body of `UserPromptSubmit`
   and `Stop`, so **those two hooks must stay synchronous and keep the 5 s
   `HOOK_TIMEOUT_SECS`**. Written here because phase 2b would otherwise
   break delivery without noticing.

## What the CLI actually permits

The repo knows nothing about hook response bodies — `/hook` answers `204` in
every case (`mcp/hooks.rs`). The following was read out of the installed
Claude Code binary (2.1.278) and is **evidence from one installed version,
not a contract**:

- `execCommand` and `execHttp` sit in one dispatch table, so an HTTP hook's
  response `body` reaches the same output consumer as a command hook's
  stdout. `execHttp` returns `{ok, statusCode, body}`; `ok` is 2xx.
- The output sanitizer has a `case "UserPromptSubmit"` and a `case "Stop"`,
  both carrying `additionalContext`.
- Hard caps, applied by truncation with a `report.truncated` note:
  **`additionalContext` 8000 characters / 200 lines**; `reason` 2000 / 20;
  `systemMessage` 4000 / 20.
- `allowedHttpHookUrls`: when that setting is defined, a hook URL that does
  not match a pattern is **blocked entirely**. Fleet's provisioning does not
  know about it today. Harmless while hooks are telemetry; after this spec
  delivery depends on them, so it belongs in diagnostics.

Because this is evidence and not a contract, every part of the design
degrades: when a CLI ignores the response body, the message stays in the
inbox and nothing is lost. Delivery is an acceleration, not the only path.
A spike settles it before implementation — see *Testing*.

## Section 1 — Addressing and participants

Today the only addressable thing is a `sessions` row, as a local `i64`. The
convention in `mcp/tools/params.rs` is `session_id` **or** `host_alias` +
`tmux_name`. A phone (`client_tokens`) is not addressable at all; nor is a
hub.

**Migration 043** adds:

- `fleet_id` — a UUID minted once per store, exposed by `whoami` and
  `/healthz`. New: fleets have no identity today. **As built it is minted
  lazily on first use that needs it (an address comparison), not by a read:
  `whoami` reports `null` until then, because `whoami` is callable by a
  readonly token and a read must never cause a write.** Clients must treat the
  field as nullable.
- table `participants` — one row per addressable endpoint:
  `id INTEGER PRIMARY KEY`, `kind TEXT` (`session` | `client` | `hub`),
  `session_id INTEGER` (set for `kind='session'`, re-pointed on a move),
  `client_id INTEGER` (set for `kind='client'`, → `client_tokens`),
  `retired_at INTEGER` (tombstone), and a unique index on
  `(kind, session_id, client_id)`. Backfilled with one row per existing
  session.
- `session_messages.from_participant_id` / `to_participant_id`, backfilled
  from `from_session_id` / `to_session_id`.
- `session_messages.delivered_at` — distinct from `read_at`; see section 2.

The address is **one string**, parsed by one function:

```
<fleet>/session/<host_alias>/<tmux_name>
<fleet>/client/<client_name>
<fleet>/hub
```

An empty `<fleet>` means "this fleet", so today's calls are unchanged and
cycle 3 does not touch the schema again.

**The string is a resolution key, never a stored foreign key.** Messages
reference `to_participant_id`. This is not a stylistic preference — see
section 3, where a session move changes both the host alias and the row id.

Two costs, stated as costs:

- **`to_session_id` cannot simply be abandoned.** Deletion paths hang off it
  in four places: `store/reconcile.rs:632`, `store/sessions.rs:874`,
  `store/projects.rs:548`, `store/hosts_accounts.rs:424`. They are rewritten
  onto `participants`. Keeping the old column as a denormalised copy was
  considered and rejected: a move would desynchronise the copy from the truth
  exactly when it matters.
- **The tool-description budget is tight.** `the_served_definition_budget_stays_bounded`
  holds 59,400 bytes, and the constant's history is a list of deliberate,
  justified raises. New address fields on `send_message` and `inbox` fit
  inside it, or the constant moves with a written reason — never by a silent
  bump.

## Section 2 — Delivery

### The main path: the hook response

`/hook` stops answering a bare `204` for `UserPromptSubmit` and `Stop`.
After `apply_hook` — which already resolves *which* `SessionRow` fired, via
the three steps of `resolve_hook_row` — the handler reads that row's
undelivered messages and answers `200` with
`hookSpecificOutput.additionalContext`.

The packer fills the 8000-character / 200-line budget with **whole messages
only**; a body is never cut mid-way. A one-line tail names how many remain
in the inbox. This is where cycle 2's cursors attach naturally.

The two hooks differ in kind, not degree:

- **`UserPromptSubmit`** — the message rides along with the prompt the
  operator just sent. Safe, non-invasive.
- **`Stop`** — `additionalContext` here only *adds context*; the turn still
  ends. Making Claude act on the message requires the legacy
  `decision: "block"` + `reason` (2000 characters).

So: `block` only for messages that genuinely require an answer —
`kind == "question"`, or a `wait_for_reply` pending on the other side;
everything else `additionalContext`.

**As built, the block path packs independently against `reason`'s own
2000-character / 20-line budget** — it does not truncate the 8000-char
`additionalContext` batch. Truncating it was the original instruction and it
was wrong: it cut bodies mid-sentence and cut the "N more waiting" tail first,
since that tail sits last. Only the messages actually carried in the block are
stamped `delivered_at`. A question too large for 2000 characters rides the
block as a stub naming its id and pointing at `inbox`, so it stays actionable
rather than silently absent. **With a hard cap**: one message may
block a `Stop` at most once, and at most **3** consecutive `Stop` blocks per
session regardless of how many senders are queued
(`STOP_BLOCK_STREAK_MAX = 3`). Without the cap a session can be shut inside a
never-ending turn, which is worse than slow delivery, and in a full mesh it is
not theoretical (section 3.5).

### There is no ack, and the design does not pretend otherwise

We know the body was written into the response. We do not know Claude
ingested it — the turn may have been interrupted. Hence `delivered_at`,
separate from `read_at`, meaning exactly "handed to a hook response".

The remainder is a genuine fork, decided rather than hidden: a message whose
`delivered_at` is set but whose `read_at` is still null is **re-delivered
exactly once**, on the second turn boundary after the first attempt
(`REDELIVER_AFTER_TURNS = 2`). Rationale: a duplicate is visible to the agent,
a silent loss is not. Bounded at one, so it cannot become a loop.

### The blocking wait: `wait_for_reply`

A sibling of `wait_for_session`: `long_poll_permit`
(`MAX_LONG_POLLS_PER_CALLER = 8`), `timeout_s` default 120 / max 600.

Today's wait primitives poll SQLite every 500 ms
(`service/tasks.rs:POLL_INTERVAL`). `wait_for_reply` is **event-driven** —
subscribe to the bus, wake on the message event, with the 500 ms poll as a
safety floor. This is a new pattern in this codebase and, after the hook, the
second-riskiest item in the cycle.

### Waking an idle session

A hook only fires when the session does something, so an idle, unprompted
session never speaks. Delivery therefore depends on the recipient's state:

| recipient state | path |
|---|---|
| `working` | hook on `Stop` |
| `idle` | wake via `sessions::send_system_prompt` (phase 1 primitive, `label: false`) |
| `blocked` / stuck | **never write to the pane** — Enter would pick a dialog option. The message stays in the inbox and `send_message` reports it. |

The third row is a lesson phase 1 already paid for (the `E_INVALID_STATE`
gate) and one that would be easy to lose here. This wake-up is the **only**
remaining use of the keystroke path.

## Section 3 — Error states

### 3.1 A session move, the sharpest case

`move_session` does not move a row, it creates a new one:
`move_session/finalise.rs:173-174` carries `from_session_id: source_row_id`
and `to_session_id: target_row_id`. The source is killed, and
`delete_session` runs `DELETE FROM session_messages WHERE to_session_id=?1`.

**Today a move destroys the session's undelivered inbox** — silently, with no
event, and with no way for the sender to find out. That is a pre-existing
defect, not one introduced here, but this cycle builds delivery on top of it
and so makes it visible.

It is also the direct tax on the string address: the address embeds
`host_alias`, so a move changes it. Therefore:

- the `participants` row is the durable identity; the string is a resolution
  key;
- `finalise` re-points the participant from `source_row_id` to
  `target_row_id` instead of letting the delete cascade eat the inbox;
- `delete_session` stops deleting by `to_session_id` — it deletes only when a
  participant is genuinely retired, never when it is being re-pointed.

### 3.2 The hook does not complete (hub down, tunnel blip, sleeping desktop)

Today: 5 s timeout, no retry, no spool — theme A of the analysis ("10 s added
per turn, every `Stop` in the window is lost").

After this cycle the cost takes a tolerable shape: delivery does not happen
that turn, the message stays in the inbox, **nothing is lost**. The condition
is that the handler never does remote work — one indexed `SELECT` over
`(to_participant_id, delivered_at, sent_at)` and return. No SSH, no hub
round-trip inside the hook.

One interaction to record: phase 2b's spool is for *outbound* hook bodies. A
spooled hook has no live response, so delivery cannot work for spooled events
by construction. Unwritten, phase 2b would believe it covers this.

### 3.3 The recipient is gone for good

A killed session, or one pruned by reconcile (`store/reconcile.rs:632` deletes
messages in bulk). Today the sender learns nothing and undelivered messages
vanish.

Instead: the participant is **tombstoned**, not deleted; undelivered messages
survive for **7 days** after `retired_at` and are then swept by the existing
`service/gc.rs` pass; and a `message_undeliverable` timeline event is written
on the sender. An agent hanging in `wait_for_reply` then gets a real answer
instead of a 120 s timeout — which is the difference between "fast" and "fast
when nothing goes wrong".

### 3.4 Duplicates

Phase 1 gave `send_prompt` a `client_msg_id` with a 10-minute
`(caller, id) → result` dedupe. `send_message` uses **that same table**, not a
second one of its own: the analysis asks for idempotency on both, and phase 1
delivered it for one.

The other duplicate source is the bounded re-delivery of section 2.

### 3.5 Runaway `Stop` block

The `STOP_BLOCK_STREAK_MAX` cap from section 2 needs a visible failure: once
hit, blocking stops, falls back to `additionalContext`, and records a
timeline event. **A session
must never be shuttable inside a turn by a remote sender** — that is a denial
of service against one's own fleet, and in a full mesh where everyone can
address everyone, it is a reachable state rather than a hypothetical one.

### 3.6 Error codes and mandatory bookkeeping

New: `E_PARTICIPANT_UNKNOWN` (the address does not resolve),
`E_PARTICIPANT_RETIRED` (it resolves but is gone). Existing `E_INVALID_STATE`
and `E_SELF_TARGET` are reused.

Three things CI enforces, so they are design, not afterthought:

- every new tool or command → `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`
  (`control-api-reference.md` lists **all** frontend commands, not only MCP
  tools);
- hub-client mode → a row in `backend/verdicts.rs` (route or refuse) plus
  `backend/tests_routing.rs` entries, then
  `REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen`;
- a new field on a hub-routed argument needs `Serialize` and a non-default row
  in `tests_routing.rs`; **a new wire field without `serde(default)` is a
  shipped outage against an older hub**, so it ships with a default and a
  regenerated contract golden.

## Section 4 — Testing

### What unit tests can prove (in-memory `Store`, no tmux, no Claude)

- **Address resolution** — string → participant, and each failure:
  `E_PARTICIPANT_UNKNOWN`, `E_PARTICIPANT_RETIRED`, self-target, a foreign
  fleet with no link (refused for now; links arrive in cycle 3).
- **The `additionalContext` packer** as a pure function: N messages against
  the 8000-character / 200-line budget → which *whole* messages fit, the
  "N more" tail, and never a split body. This earns a property test; the repo
  already has that pattern for `shell::quote`.
- **The `Stop` block cap**: `STOP_BLOCK_STREAK_MAX` consecutive blocks, then
  fallback plus a timeline event; and that one message blocks at most once.
- **A move**: a `RecordingEventBus` test that `move_session` re-points the
  participant and the **undelivered inbox survives**; and the converse, that a
  real kill tombstones and writes `message_undeliverable`. Plus a test that
  `delete_session` no longer wipes a re-pointed participant's inbox.
- **Dedupe**: the same `client_msg_id` twice → one row, the same result.

### What only the e2e can prove (real tmux, real hub process, real HTTP)

`scripts/hub-e2e.sh:221` currently asserts
`hook with master token, unknown session -> 204`. That check splits: an
unknown session still answers `204` with no body, and a **known session with a
pending message answers `200` with the JSON body**. That is the single
assertion pinning the contract on the wire, through the real axum stack.

The wake-up path has a proven shape to copy — lines 421-423 already do
`send_prompt` → `capture_session | grep` over the agent. Plus the refusal: a
`blocked` row must never have text inserted.

### What no test in this repo can prove

**That Claude Code actually ingests the `additionalContext` we return.** The
e2e can prove fleet returns the right body; it cannot prove the CLI consumes
it, because the harness has no real Claude process — it drives a shell in
tmux. The evidence above is strings from 2.1.278: good evidence, not a
contract.

So: **one manual spike before implementation.** A local HTTP server returning
a fixed `additionalContext`, one `UserPromptSubmit` http hook pointed at it,
one prompt in a throwaway session, and observe whether the context arrives.
Cheap, and it settles the assumption the whole of section 2 rests on.

If the spike fails, cycle 1 does **not** die — it degrades to `participants` +
`wait_for_reply` + wake-up. Still worth shipping, just without free delivery.

### Environment traps known in advance

- `hub-e2e` runs **only on ubuntu-24.04** (the macOS runners have no tmux), so
  the wire assertions live there and the macOS `rust` job verifies nothing
  tmux-shaped. Locally `--hub-e2e` is opt-in.
- In a worktree `cargo` is a shell function forcing a per-root target dir, but
  **scripts bypass it** and inherit the main checkout's shared target — so
  `ci-local.sh` here can compile against another worktree's crates. Export
  `CARGO_TARGET_DIR` explicitly.
- Judge CI unpiped, never through `| tail`; run the full suite per task rather
  than filtered tests — a filtered green has hidden a red test in this repo
  before.
- Order is TDD: RED first, with verified RED evidence per task rather than a
  claim of it.

## Summary of cycle 1

Migration 043 (`participants`, `fleet_id`, `from`/`to_participant_id`,
`delivered_at`); a string address used as a resolution key; a two-way `/hook`
on `UserPromptSubmit` and `Stop` within an 8000/200 budget; an event-driven
`wait_for_reply`; a wake-up via `send_system_prompt` for idle sessions only;
participant re-pointing on a move; tombstone plus `message_undeliverable` on a
kill; and an exception recorded for phase 2b, that `UserPromptSubmit` and
`Stop` stay synchronous.
