# Smart caching — remembered read cursors

Date: 2026-09-23. Base `cbeede20`. Cycle 2 of three.

Goal, in the operator's words: *a fetch already made must be repeatable so that
it returns **only what is new***.

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

The served tool-description budget is **62,640 bytes**
(`BUDGET_BYTES`, `mcp/tools/tests.rs`), and its comment history argues raises in
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

```sql
CREATE TABLE IF NOT EXISTS read_cursors (
  id INTEGER PRIMARY KEY,
  reader_session_id INTEGER NOT NULL,
  tool TEXT NOT NULL,
  resource_key TEXT NOT NULL,
  watermark INTEGER,      -- streams
  content_hash TEXT,      -- snapshots
  updated_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_read_cursors_key
  ON read_cursors(reader_session_id, tool, resource_key);
```

A row uses `watermark` **or** `content_hash`, never both — the two mechanisms do
not mix on one row.

`resource_key` is the target's identity as text: the target `session_id` for the
stream tools, `<host>:<path>@<ref>` for `repo_diff`, a filter fingerprint for
`list_sessions`.

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

A reader is a session, and a session can be killed or moved. Cursor rows for a
reader that no longer exists are swept by the **same** GC pass cycle 1 added for
retired participants, with the same window — not a second retention path.

## Section 2 — Per-tool watermarks

| tool | watermark | the "changed?" test |
|---|---|---|
| `session_transcript` | `turn_seq` | compare to `row.turn_seq` — **a local row read, no SSH** |
| `session_history` | last `session_events.id` | `MAX(id)` for the session |
| `inbox` | last `session_messages.id` | `MAX(id)` for the reader's participant |

The transcript is where this pays most, and for a reason worth stating: a
transcript read is an SSH `tail` of up to 4 MiB (the device-communication
analysis flags the conversation panel re-reading that every 5 s). `turn_seq`
lives on the session row and is bumped by the `Stop` / `UserPromptSubmit` hooks,
so answering "nothing new" costs **one SQLite read and no remote call**.

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

Four cases:

- **No cursor** (first call) → full payload, cursor established.
- **Cursor ahead of the head** (`watermark > current`) → full payload plus
  `cursor_reset: true`. Reachable: `/clear` and `/resume` rebind a session's
  conversation (migration 037's conversation tracking), and `turn_seq` can
  restart.
- **The transcript was compacted** between reads → full payload,
  `cursor_reset: true`, reason `compacted`. Not a guess: `PreCompact` /
  `PostCompact` hooks are installed, so a compaction is a recorded event, and
  turns before it are gone from the JSONL. A watermark pointing into the
  compacted region cannot yield a suffix.
- **The reader no longer exists** → the tool still answers (it is a read); the
  cursor simply is not stored.

`cursor_reset` is a field on the response, not an error. A caller that ignores
it gets correct data — just more of it than it expected.

## Section 4 — Snapshots

`repo_diff` and `list_sessions` take the same `fresh_for` and answer either
`{ unchanged: true }` or the payload. The hash lives in the cursor row, so the
caller neither sends nor stores one.

**What this does not save:** for `repo_diff` the hash is computed over the diff
output, so the server does the same work either way. The saving is the agent's
context and the transfer, not server time. Stated plainly rather than implied,
because a reader could reasonably assume otherwise.

## Section 5 — Budget

One `fresh_for: Option<i64>` on each of five tools. At roughly 120-180 bytes
per field that is 600-900 bytes — measure it, pay it once, and document the
raise in the established comment style naming this cycle. Never a silent bump.

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
- **Only the e2e can prove** that `unchanged` on `session_transcript` performs
  **no SSH call**. That is the central performance claim, and it is invisible to
  a unit test because the unit fixture has no SSH. It belongs in
  `scripts/hub-e2e.sh`, asserted by the absence of a remote read.
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
