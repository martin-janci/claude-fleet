# Replay-ring pressure under tracker sync (work graph M10.6)

**Date:** 2026-09-25 · **Plan:** `docs/superpowers/plans/2026-09-25-work-graph-m10-settle.md` §M10.6
· **Roadmap risk:** "The replay ring (512 frames) is shared by every event kind."

## Summary

- **Ring reach is fine.** At the default sync interval (300 s) with 5–10 % of a
  200-item board changing per pass on each of two trackers, the ring still
  reaches back **10–11.7 minutes right after a sync burst**. The worst case
  measured was **602 s**. The plan's threshold is 5 minutes, so there is no
  per-kind ring, no coalescing layer, and no change to `REPLAY_RING`.
- **Item frames are clean.** No pass emitted a `work:item` for an item that
  did not change. That held for incremental passes, their 120 s overlap
  re-lists, and the hourly whole listings. No item got more than one frame in
  a pass.
- **Session frames were not clean, and that is now fixed.** When a linked
  item changed only in ways the session row does not show (description,
  assignee, or a comment bumping `updated`), the sync still bumped
  `row_version` and sent `session:updated` for every session whose primary
  work was that item. The row was otherwise identical.
  - That was **60 % (12 of 20) and 80 % (20 of 25)** of the sync's session
    frames in the two default-interval scenarios.
  - It is now **0**. `session:updated` is sent only when the item's key,
    title, status, url or availability moves.
- **Something for you to decide:** the **first sync of a newly added
  tracker** writes one `work:item` per item (400 here). Right after it, the
  ring reaches back only **155 s**. This happens once per tracker added; see
  *Recommendations*.

## Method

The measurement is a normal `cargo test` in `fleet-core`:
`crates/fleet-core/src/service/trackers/sync/tests_ring_pressure.rs`, a child
module of `sync.rs`. It is deterministic, with a seeded xorshift and a
simulated clock. It prints its numbers with:

```bash
cargo test -p fleet-core --lib ring_pressure -- --nocapture --test-threads=1
```

The M10.2 loopback fake tracker does not exist yet, so the test brings its
own.

- **Fake Jira Cloud (`Board`, an `HttpTransport`).**
  - Two sites, `acme` (`ABC-*`) and `beta` (`XYZ-*`), with **200 items
    each**. That is 400 items in total, a pessimistic reading of "a 200-item
    board".
  - It answers `POST /rest/api/3/search/jql` (paged by 100) and
    `POST /rest/api/3/issue/bulkfetch` in the shapes the real adapter parses.
  - The requests go through the real `JiraCloud` provider and the real
    `TrackerSync::run_pass`, with `with_clock` driving the sync's clock.
  - An incremental listing returns the items updated since the watermark the
    fake last served, minus `OVERLAP_SECS`. That is the window the sync asks
    for. The fake derives it from its own served watermark because the
    adapter's JQL `-Nm` is computed from the wall clock. The overlap therefore
    re-lists recently changed items every pass, which is the no-op path under
    test.
  - Every 12th pass lists each view whole (`FULL_EVERY_SECS`), so hourly
    whole listings are included.
- **Churn.** Between passes, N distinct items per tracker change, each at a
  random moment inside the interval. The mix is:
  - 35 % status move;
  - 10 % title;
  - 15 % description;
  - 15 % assignee;
  - 25 % "touch" (only `updated` moves, like a comment or a field fleet does
    not read).
- **Sessions.** 20 sessions: 10 linked, 10 idle.
  - Of the 10 linked sessions, three share `ABC-7` (a pair and a reviewer)
    and seven have one item each, across both trackers.
  - Links are `manual` (confirmed, primary), with a `claude_session_id` so
    status moves are journaled.
- **Baseline.** Session frames at **0.64 frames/s**, the busy-fleet rate the
  `REPLAY_RING` comment in `events.rs` gives. That rate was taken as given,
  not re-measured. The frames are spread evenly over each interval,
  round-robin over the 20 sessions, and each one is a real row change
  (`last_activity_at`).
- **The ring.**
  - A tee bus forwards every emit into a real `BroadcastEventBus`, with a
    subscriber held so it records, and logs it with the simulated time.
  - Ring reach is `now − time(oldest frame replay_after can still return)`,
    read through `replay_after` itself.
  - The test checks that the ring and the log agree frame for frame.
  - It measures both right after each sync burst (the worst moment for a
    phone that resumes) and just before each pass.
- **Counting.**
  - Frames per kind, per pass and per minute.
  - Bytes per pass (payload with nulls stripped, as the ring stores it).
  - A `work:item` for an item the board did not change counts as a no-op.
  - A sync `session:updated` whose row, apart from `row_version`, equals the
    last frame for that session also counts as a no-op.
  - It also records the most frames sent for one item, and for one session,
    in one pass.
- **Steady state.** 20 minutes of churn run unmeasured first, so the first
  sync has left the ring. The measurement then covers 36 passes (3 h) at
  300 s, and 60 passes (1 h) at 60 s.

## Numbers

Measured on 2026-09-25 at `origin/main` plus this branch. "Before" is the
same harness run against the unmodified `store/tracker_items.rs`.

### Default interval (300 s), 5 % churn (10 items per tracker per pass)

| | before | after |
|---|---|---|
| sync frames per pass (mean / max) | 20.6 / 25 | 20.2 / 22 |
| `work:item` per pass | 20.0 | 20.0 |
| sync `session:updated` per pass (mean / max) | 0.56 / 5 | 0.22 / 2 |
| sync `session:updated` with an unchanged row | 12 of 20 | **0 of 8** |
| bytes per pass (mean / max) | 8,229 / 10,887 | 8,032 / 9,131 |
| frames/min: baseline · `work:item` · sync `session:updated` | 38.40 · 4.00 · 0.11 | 38.40 · 4.00 · 0.04 |
| 512 slots last (average) | 12.0 min | 12.1 min |
| ring reach right after a burst (min / mean) | 694 s / 702 s | 700 s / 704 s |
| ring reach before a pass (mean) | 734 s | 735 s |
| no-op `work:item` | 0 | 0 |
| most frames for one item / one session per pass | 1 / 1 | 1 / 1 |

### Default interval (300 s), 10 % churn (20 items per tracker per pass)

| | before | after |
|---|---|---|
| sync frames per pass (mean / max) | 40.7 / 45 | 40.1 / 42 |
| `work:item` per pass | 40.0 | 40.0 |
| sync `session:updated` per pass (mean / max) | 0.69 / 5 | 0.14 / 2 |
| sync `session:updated` with an unchanged row | 20 of 25 | **0 of 5** |
| bytes per pass (mean / max) | 16,235 / 18,774 | 15,906 / 17,039 |
| frames/min: baseline · `work:item` · sync `session:updated` | 38.40 · 8.00 · 0.14 | 38.40 · 8.00 · 0.03 |
| 512 slots last (average) | 11.0 min | 11.0 min |
| ring reach right after a burst (min / mean) | 602 s / 608 s | 608 s / 610 s |
| ring reach before a pass (mean) | 672 s | 673 s |
| no-op `work:item` | 0 | 0 |
| most frames for one item / one session per pass | 1 / 1 | 1 / 1 |

### Stress: minimum interval (60 s), 10 % churn *per pass*

This is five times the change rate above, about 40 item changes a minute. It
is not realistic; it shows where the margin ends.

| | before | after |
|---|---|---|
| sync frames per pass (mean / max) | 40.8 / 45 | 40.4 / 45 |
| sync `session:updated` with an unchanged row | 28 of 49 | **0 of 21** |
| frames/min total | 78.8 | 78.4 |
| 512 slots last (average) | 6.5 min | 6.5 min |
| ring reach right after a burst (min / mean) | 360 s / 360 s | 360 s / 363 s |

### One-off: the first sync of a new tracker

- The first pass over two fresh 200-item trackers emits **402 frames**: 400
  `work:item` plus 2 `work:tracker`. Sessions whose keys were typed before
  the trackers were added also get one bound `session:updated` each.
- With an hour of baseline already in the ring, the ring right after that
  pass reaches back **155 s**.

### Fan-out of one change (`ring_pressure_one_change_fans_out_once_and_quiet_passes_are_silent`)

- A status move of `ABC-7`, the primary work of three sessions, emits
  exactly `work:item` followed by three `session:updated`.
- No link frame is sent; links have no frame kind of their own. Their state
  reaches clients inside the session row.
- The next pass re-lists `ABC-7` through the overlap and is silent. So is an
  unchanged incremental pass, and so is an unchanged hourly whole listing.

## Findings

1. **Sync traffic is small next to the baseline.** At the default interval a
   realistic board adds 4–8 `work:item` frames a minute to about 38 session
   frames a minute. The sync burst itself is 20–45 frames, under 9 % of the
   ring.
2. **No no-op item frames.** `upsert_tracker_item` already compares the row
   it would write with the stored one. An unchanged item writes only
   `fetched_at` and emits nothing. The overlap and the hourly whole listing
   cost no frames.
3. **No need to coalesce per item.**
   - Within a pass, the sync de-duplicates on `(external_id, updated)` across
     views, the by-id refresh and the by-key lookups.
   - An item therefore gets at most one `work:item` per pass, and a session
     at most one frame per pass, because a session has one primary link.
   - A coalescing layer would have nothing to merge.
4. **No-op session frames: fixed at the source.** `emit_work_item` bumped
   `row_version` and re-sent the whole session row for every change
   to the item. The session row's `work` / `work_suggested` summary
   (`SESSION_COLUMNS` in `store/rows.rs`) reads only these item fields:
   - `key`;
   - `title`;
   - `status_name` and `status_category`;
   - `url`;
   - whether `unavailable_at` is set (and the tracker, which an update never
     changes).

   Any other change produced a frame identical to the last one apart from
   `row_version`.

## The fix

`crates/fleet-core/src/store/tracker_items.rs`:

- `upsert_tracker_item` computes, from the before/after comparison it
  already makes, whether the fields the session row shows moved
  (`Visible::session_view`). `emit_work_item(id, sessions)` always emits
  `work:item`. It bumps `row_version` and emits `session:updated` only when
  `sessions` is true.
- `mark_tracker_item_unavailable` notifies sessions only on the transition
  from available to unavailable. The row shows the flag, not the reason, so
  a reason-only change no longer does.
- A new item keeps the old behaviour, and no live link can point at it yet.
  A re-sighting of an unavailable item still notifies, because it counts as
  a visible change.

### Why this is safe for clients

This was checked in the code before relying on it.

- **Desktop sessions (`src/lib/sessions.ts`).** `createRowStore` keys rows
  by `id` and replaces them whole. `sessionIsStale` rejects only a payload
  with a *lower* `row_version`. A frame we no longer send carried a row equal
  to the one the client holds except for `row_version`, and the server no
  longer bumps `row_version` either. The client's copy and the server's
  stay equal, and a later list or frame compares the same way it did before.
- **Desktop `work:item` (`src/lib/events.ts` → `applyWorkEvents` in
  `src/lib/trackers.ts`).** The desktop reads only `work:tracker` /
  `work:tracker_removed` from the batch and holds no store of `work:item`
  rows. `work:item` frames are unchanged anyway.
- **Phone.** Frame kinds, names and fields are unchanged, so
  `MAX_HUB_CONTRACT = 4` is unaffected. There is no new tool, no
  `CONTRACT_REVISION` bump and no migration.
  - The phone repository is not in this session, so its reducer was not
    read. It receives a subset of the same full-row `session:updated` frames
    the desktop does.

### The guard

The pressure tests assert, for every scenario:

- no no-op `work:item`;
- no unchanged-row `session:updated`;
- at most one frame per item and per session per pass;
- a pass's frames ≤ changed items + linked sessions;
- ring reach right after every steady-state burst ≥ 5 minutes.

The fan-out test pins the exact frames of one change and the silence of
unchanged passes. A unit test in `tracker_items.rs` pins the per-field
behaviour: a description, assignee or `updated` change moves no session;
unavailable notifies once; a re-sighting notifies.

## Decision

Under the plan's rule (*coalesce only if sessions fall out of the ring in
under 5 minutes during sync*), the numbers say **leave the ring alone**:

- The worst realistic reach right after a burst is 602 s.
- Even the stress case keeps 360 s.

The only code change is emitting only real changes. The no-op
`session:updated` frames are removed at the source. They were a correctness
nit (a spurious `row_version` bump and a re-sent row), not a capacity
problem: at the default interval they were under 0.2 frames/min.

## Recommendations that need you

1. **The first sync of a new tracker floods the ring (reach 155 s).**
   - Adding a tracker is a rare, operator-initiated action. A phone that
     resumes across it degrades to the existing behaviour: a `lagged` frame
     and a full re-list of about 61 KB.
   - **Recommendation: accept it.**
   - The alternative is a design change: suppress per-item frames on a
     tracker's *first* pass and rely on the `work:tracker` first-sync frame,
     which the desktop already uses to announce a first sync
     (`onWorkEvents` in `App.svelte`). That changes which frames a client
     can count on, so it is your call, and the phone's reducer should be
     checked first.
2. **Per-kind rings or a larger `REPLAY_RING`: not needed on these
   numbers.** Revisit only if either of these happens:
   - a hub runs high-churn boards at the minimum 60 s interval (the stress
     case, ~6 min of reach);
   - the baseline session rate grows well past 0.64 frames/s. The ring is
     dominated by session frames, not by sync.
3. **A stale-summary gap outside M10.6's scope (not fixed).**
   - `emit_work_item` notifies only sessions whose **live primary link** is
     the item.
   - `work_suggested` on the session row also shows an item's title and
     status. A session whose *suggestion* points at an item that changes
     keeps the old title and status on clients until its next row change.
   - Fixing it adds frames rather than removing them, so it is left for a
     decision. Suggested owner: M10.1's leftovers or M10.4.

## Not done, and why

- **The baseline rate (0.64 frames/s) was taken from the `events.rs`
  comment, not re-measured on a live hub.** This session has no hub.
- **Real Jira was not used.** The fake reproduces the request and response
  shapes and the watermark and overlap mechanics, not Jira's own search
  latency or consistency.
- **Other providers (GitHub, Asana, Linear, Jira DC) were not driven.** They
  share `upsert_tracker_item` and `emit_work_item`, where both the
  measurement's findings and the fix live. Asana's sync-token path emits
  through the same upsert.
