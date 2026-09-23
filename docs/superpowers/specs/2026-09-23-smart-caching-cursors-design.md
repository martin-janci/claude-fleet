# Smart caching — remembered read cursors

Date: 2026-09-23. Base `cbeede20`. Cycle 2 of three.

Goal, in the operator's words: *a fetch already made must be repeatable so that
it returns **only what is new***.

> **As built (2026-09-24).** This spec was written before implementation, and
> eight of its statements turned out wrong or were changed during the build.
> Each is corrected in place below and marked *As built*, naming the decision
> that changed it. The decisions are recorded in the SDD ledger,
> `.superpowers/sdd/2026-09-23-smart-caching-cursors/progress.md`: the plan's
> five stated corrections to this spec (generation column, `ahead_of_head` from
> id reuse, no retention window, `reader_unknown`, the unit-test no-SSH proof)
> and Rulings 8, 10–14 and 16–19. The caller-facing contract is
> `docs/control-api.md` → *Remembered read cursors*; where this spec and that
> section disagree, that section describes the build.

| Cycle | Scope | Status |
|---|---|---|
| 1 — addressing and delivery | durable participant identity, fleet addresses, two-way `/hook`, `wait_for_reply`, tombstone retention | **landed**, PR #245 |
| **2 — smart caching** | remembered read cursors on the tools where payload size hurts | **this spec** |
| 3 — hub↔hub federation | hub identity, peer links, routing a foreign address, loop prevention | not started; builds on cycle 1's addressing |

## A dependency that turned out not to exist

Cycle 1's design asserted that cycle 2 "must build on phase 4 item 12" of
`docs/specs/2026-09-21-device-communication-analysis.md` — event-bus `seq`, SSE
`id:`, a replay ring, `Last-Event-ID`.

**That item is still unimplemented** (phases 1 and 2a landed; 2b, 3, 4 and 5 are
open), and the asserted dependency was wrong anyway: item 12 is delta on the
**push** stream for SSE clients; this cycle is delta on **pull** responses for
MCP tool callers. Different layers, different consumers. They should share the
*concept* of a monotonic watermark and nothing else. Building a fleet-wide `seq`
first would couple the tool layer to the event bus for a benefit no caller can
perceive.

So this cycle stands alone, and cycle 1's claim is retracted here.

## The constraint that decides the shape

The served tool-description budget is **62,640 bytes** (*as built:* stale —
see Section 5) (`BUDGET_BYTES`, `mcp/tools/tests.rs`), and its comment history argues raises in
*tens* of bytes — one entry records 24 bytes of headroom and the note that "the
next clause to land here has to pay for itself". There are ~80 tools. Every
optional param pays for its name, type, description and JSON-schema `default`
wrapper, and that structural cost is one no wording can argue away.

So this cycle is **deliberately not applied to the whole fetch surface.** Only
the tools whose payloads actually hurt get it. **One new param per tool, five in total.**

## Two mechanisms, because there are two kinds of resource

| kind | tools | what "new" means |
|---|---|---|
| **append-only stream** | `session_transcript`, `session_history`, `inbox` | a genuine suffix — a cursor saves real context |
| **snapshot** | `repo_diff`, `list_sessions` | nothing; a diff between two refs has no tail, a row set has no tail. The honest win is a cheap `unchanged` |

Pretending a diff has a suffix would be an API that lies about half its
surface. Streams get a watermark and a delta; snapshots get a server-side
content hash and a cheap `unchanged`. One param drives both (see *One param, not
two*), so the caller does not have to know which kind it is asking about.

## Section 1 — The cursor table

**Migration 044**:

*As built* — the table gained three columns and a trigger (the draft had only
`watermark` and `content_hash`; see `crates/fleet-core/migrations/044_read_cursors.sql`
for the full comments):

```sql
CREATE TABLE IF NOT EXISTS read_cursors (
  id INTEGER PRIMARY KEY,
  reader_session_id INTEGER NOT NULL,
  tool TEXT NOT NULL,
  resource_key TEXT NOT NULL,
  target_session_id INTEGER, -- the session a cursor is ABOUT (NULL for list_sessions)
  watermark INTEGER,         -- streams
  generation INTEGER,        -- session_transcript: latest conversation-boundary event id
  anchor TEXT,               -- session_transcript: {"at","fingerprint"} of the last served turn
  content_hash TEXT,         -- snapshots
  updated_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_read_cursors_key
  ON read_cursors(reader_session_id, tool, resource_key);
CREATE INDEX IF NOT EXISTS idx_read_cursors_target
  ON read_cursors(target_session_id) WHERE target_session_id IS NOT NULL;
CREATE TRIGGER IF NOT EXISTS trg_read_cursors_on_session_delete
AFTER DELETE ON sessions
BEGIN
  DELETE FROM read_cursors
   WHERE reader_session_id = old.id OR target_session_id = old.id;
END;
```

`generation` is the plan's correction (`turn_seq` keeps counting across `/clear`
while the transcript file changes); `anchor` is Ruling 8; `target_session_id`
lets a cursor die with the session it is about; the trigger is Ruling 16.

A row uses `watermark` (+ `generation`, `anchor`) **or** `content_hash`, never
both — the two mechanisms do not mix on one row.

`resource_key` is the target's identity as text. *As built:* the target
`session_id` for `session_transcript` and `session_history`;
`<session_id>:<unread_only>` for `inbox` (Ruling 10a — a `true` and a `false`
read watch different sequences, and sharing a cursor let one skip rows the
other had not returned); `<session_id>:<path>` for `repo_diff` (the draft said
`<host>:<path>@<ref>`; the tool is addressed by session and path, not by ref);
a hash of the filter fingerprint for `list_sessions`.

### Why the reader passes its own id

The obvious design — "the server remembers per caller" — **cannot work on this
auth model**, and this is the single most important thing in this spec.

A caller label is `host:<alias>`, `client:<name>` or `master`
(`mcp/auth.rs:108-114`). Every Claude session on one host authenticates with
that host's token, so **they are all the same caller.** And the server cannot
identify the calling session at all: `whoami` requires the caller to *pass its
own `tmux_name`* as a parameter.

A cursor keyed on the caller would therefore be shared by every session on a
host, and they would consume each other's deltas — not as a rare race, but as
the default. So the reader identifies itself, in the `fresh_for` param that
names its own session id (the table's `reader_session_id` column). This matches
the convention the codebase already uses — `send_message` takes
`from_session_id`, `wait_for_reply` takes `session_id`.

A reader can pass another session's id and consume its cursor. That is the
existing trust boundary, not a new one: `send_message{from_session_id}` is
already unproven within a host's token.

### One param, not two

The whole mechanism is a single optional field on each of the five tools:

```
fresh_for: Option<i64>   // the READER's own session id
```

Absent → the full payload, and no cursor is read or written. Present → the
server looks up `(fresh_for, tool, resource_key)` and answers a delta, an
`unchanged`, or a full payload with `cursor_reset`.

This started as two fields (`fresh_only: bool` + `reader_session_id`) and
collapsed to one during review: a reader id is only ever supplied *because* the
caller wants the cached behaviour, so a separate boolean carries no information.
It also removes `if_unchanged` from the snapshot tools entirely — since the
server remembers the hash, the caller never carries one. Five fields instead of
nine, and nothing for the agent to keep in context.

### Retention

A reader is a session, and a session can be killed or moved. *As built:* there
is **no retention window** (the draft reused cycle 1's mail window). A cursor
has no one left to inform once its reader or target is gone, and waiting is
actively harmful: `sessions.id` has no `AUTOINCREMENT`, so a deleted id is
handed to the next session created, which would inherit the dead session's
"already read" and answer its first read `unchanged` (Ruling 16). Cleanup is the
`trg_read_cursors_on_session_delete` trigger, which drops a session's cursors —
as reader and as target — in the same statement that deletes the row, on every
delete path. The GC sweep (`Store::sweep_orphan_read_cursors`, every tick, not
gated on `gc.enabled`) is kept as a backstop for rows naming an id no session
ever had.

## Section 2 — Per-tool watermarks

| tool | watermark | the "changed?" test |
|---|---|---|
| `session_transcript` | `turn_seq` (+ `generation`) as a change detector; the delta is positioned by `anchor` | compare to `row.turn_seq` — **a local row read, no SSH** |
| `session_history` | last `session_events.id` | `MAX(id)` for the session |
| `inbox` | last `session_messages.id` | `MAX(id)` for the target session's participant |

The transcript is where this pays most, and for a reason worth stating: a
transcript read is an SSH `tail` of up to 4 MiB (the device-communication
analysis flags the conversation panel re-reading that every 5 s). `turn_seq`
lives on the session row, so answering "nothing new" costs **one SQLite read and
no remote call**.

*As built:* `turn_seq` is bumped by the **`Stop` hook only** (the draft also
named `UserPromptSubmit`), and it is a **change detector, not a position**
(Ruling 8). An in-progress turn, an interrupt, a slash command or a queued prompt
each add a turn to the transcript file with no `Stop` behind it, so "the last
`turn_seq − watermark` turns" does not name the turns a caller has not seen. The
delta is positioned by the `anchor` instead: the last served turn's
opening-prompt timestamp plus a fingerprint of its rendered text (Rulings 8,
12, 13). Consequently `unchanged` means precisely **no completed turn since your
last read** — an interrupted turn can be answered `unchanged` while the session
sits idle, and is served with the next completed turn (Ruling 18).

### `since_turn` is not already a delta path

Cycle 2 was nearly specified on the belief that `session_transcript{since_turn}`
already computed a delta. It does not. `support.rs:1051-1053` computes
`turns = row.turn_seq - t` and then reads *the last N turns* — a turns-back
**count**, not a positional cursor. `.max(1)` means it always returns at least
one turn, so `since_turn == turn_seq` still returns content: **there is no way
to say "nothing new" today.**

Consequence: `fresh_for` is a new path, not a rename of an old one.
`since_turn: None` keeps returning the last turn exactly as today, so no
existing caller shifts.

## Section 3 — When a cursor is not trustworthy

**The rule: a cursor that cannot be trusted returns the full payload and says
so. Never a silent skip.** Silent skipping is the whole failure mode this cycle
exists to prevent, and cycle 1 shipped that class of bug twice before review
caught it.

*As built:* `cursor_reset` is a **reason string** (the draft had `true` plus a
separate reason), `null` when there is no reset. There is **no `compacted`
reason**: a compaction is one of the conversation-boundary events
(`conversation_started`, `conversation_ended`, `compact_done`) that move the
cursor's `generation`, and so is reported as `conversation_changed`. The four
reasons are `service::fresh::ResetReason`:

- **No cursor** (first call) → the first-read answer, `cursor_reset: null`,
  cursor established.
- `conversation_changed` — a conversation boundary (`/clear`, `/resume`,
  compaction, a new conversation) moved the target's `generation` since the
  last read.
- `ahead_of_head` — `watermark > current head`, or a cursor on an empty stream.
  It comes from **id reuse** (a new session reusing a deleted `sessions.id`
  starts its counters at zero), **not `/clear`**, which `generation` catches.
  Since Ruling 16's trigger drops a deleted session's cursors, this is a
  defensive backstop.
- `reader_unknown` — `fresh_for` names no session. The tool still answers (it is
  a read) and no cursor is stored. For `session_history`/`inbox` it answers the
  default newest-first page with `more: false` (Ruling 17), because a cursorless
  oldest-first page would repeat forever for a caller following `more`.
- `too_far_behind` — `session_transcript` only: the stored anchor cannot be
  located in the tail window read (or was never recorded), or a page could not
  end on an anchorable turn. Answered with the default window, never a guess.

What a reset returns (*as built*): for `session_history`/`inbox`, the oldest
page from the start of the stream (paged, `more` set), except `reader_unknown`
above; for `session_transcript`, the default window (the last turn with
content) under a `[cursor reset: <reason> …]` banner; for the snapshot tools,
the full payload.

`cursor_reset` is a field on the response, not an error. A caller that ignores
it gets correct data — just more of it than it expected.

## Section 4 — Snapshots

`repo_diff` and `list_sessions` take the same `fresh_for` and answer either
`unchanged` or the payload. The hash lives in the cursor row, so the caller
neither sends nor stores one.

*As built:* both answer the envelope `{unchanged, cursor_reset, more, data}`
that `session_history` and `inbox` also use (`more` is always `false` for a
snapshot; `data` is `null` when `unchanged`). `session_transcript` alone
answers plain text with banner lines. The stored hash is over the **exact
`data` value** placed in the envelope, serialized canonically (key-sorted), so
"the hash matched" and "the bytes you would receive are the same" are one claim
(Ruling 14). `list_sessions` rows on the `fresh_for` path are **ordered by
session id** (after filters and `limit`), because the default
`last_activity_at DESC` order reshuffles on every reconcile tick and would
defeat `unchanged` on a busy fleet (Ruling 14).

**What this does not save:** for `repo_diff` the hash is computed over the diff
output, so the server does the same work either way. The saving is the agent's
context and the transfer, not server time. Stated plainly rather than implied,
because a reader could reasonably assume otherwise.

## Section 5 — Budget

One `fresh_for: Option<i64>` on each of five tools. At roughly 120-180 bytes
per field that is 600-900 bytes — measure it, pay it once, and document the
raise in the established comment style naming this cycle. Never a silent bump.

*As built:* the budget figure above (62,640) was already stale when the build
started. The raise was measured and paid once, in Task 4; the current value is
`BUDGET_BYTES` in `crates/fleet-core/src/mcp/tools/tests.rs`, with its comment
naming this cycle. Cite that constant rather than a number here, which drifts.

Collapsing two fields into one was worth more than any wording could buy: it cut
the projected cost from nine fields to five before a byte was spent.

Each description gets one clause. The prose belongs in the control skill and in
`docs/control-api.md`, per the precedent the budget's own history sets.

## Section 6 — Testing

- **Pure, property-tested:** the watermark comparison and the four trust rules.
  Given a stored watermark and a current head, which of {delta, full+reset,
  unchanged} results — for every ordering including equal and ahead.
- **Store-level:** cursor upsert is idempotent per `(reader, tool, resource)`;
  two different readers of the same resource keep independent cursors (the
  regression test for the caller-keying defect this spec exists to avoid); a
  swept reader's cursors go with it.
- *As built:* the claim that `unchanged` on `session_transcript` performs **no
  SSH call** is proven by a **unit test**,
  `an_unchanged_transcript_read_touches_no_transcript_at_all`
  (`mcp/tools/tests.rs`), not only by an e2e — the draft's belief that the unit
  fixture could not see it was wrong. The test points the target at a host with
  no reachable SSH and no transcript path, so any read attempt would error; the
  `unchanged` answer succeeding proves no read happened.
- A test that asserts the *presence* of a delta is weaker than one asserting the
  *absence* of the skipped content. Prefer the latter.

## Out of scope, deliberately

- The whole fetch surface. Five tools, chosen by payload size.
- A fleet-wide `seq` / phase 4 item 12. Independent work on a different layer.
- `repo_diff` returning a true diff-between-two-SHAs. That is a different diff,
  not a suffix, and git would have to compute it — worth its own decision later.
- Cycle 1's deferred `REDELIVER_AFTER_TURNS`. It is a delivery concern, not a
  caching one, and putting it here would be scope drift wearing a cursor's
  clothes.
