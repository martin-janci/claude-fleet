# Hub store latency — plan

**Goal:** cut the latency of MCP reads on the production hub (fleet-hub in
docker on a NAS with HDD storage) by removing store-mutex contention.

**Spec:** none separate. This plan is the spec. It comes from a measured
investigation on 2026-09-25 (PR #272, which carries it):

- Transport overhead is ~0.2 s (MCP `ping`, `tools/list`).
- `whoami` and `list_hosts` take 0.4–2 s.
- `list_sessions` and `list_worktrees` sometimes take ~15 s.
- The hub process wrote 4.7 GB in 53 minutes against a 3 MB `state.db`.
- A rollback-journal autocommit write cost 208 ms on the NAS.

Commit `2d7415fd` switched the store to WAL + `synchronous=NORMAL`. A write now
costs ~0.1 ms of fsync, but every write still holds the one
`std::sync::Mutex<Store>`, and every reader queues behind it.

Line numbers below come from the analysis at `2d7415fd`. Treat them as
pointers and re-locate the code before editing.

## Global Constraints

These bind every task:

- **SQLite access** goes through `Store` behind a `std::sync::Mutex`. Never
  hold the guard across an `.await`.
- **No blocking I/O** (SSH, process spawn, network) while a store guard is
  held.
- **Nested transactions.** `Store::atomically` / `with_transaction` cannot
  nest. Anything that may already run inside a transaction must use a
  SAVEPOINT or an `_in_tx` variant taking `&rusqlite::Transaction` /
  `&Connection`. Never start a second `BEGIN`.
- **Event emission.** Events emitted inside `Store::atomically` are held until
  commit. Keep that property: no `RowChange` may announce a write that rolled
  back.
- **Observable behaviour is unchanged unless the task says otherwise.** That
  covers the rows, events and `row_version` bumps the frontend relies on.
  - A no-op guard (`AND col IS NOT ?`) must still emit an event when the value
    changes.
  - It may skip the event when the value does not change, if and only if the
    write is skipped too.
- **No new MCP tool, wire field or tool-description change.** If one becomes
  unavoidable:
  - regenerate `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`;
  - regenerate `REGEN_HUB_CONTRACT=1` goldens;
  - add `#[serde(default)]` to any new wire field.
- **New migrations** follow the registered pattern:
  - add a `crates/fleet-core/migrations/NNN_<topic>.sql` file;
  - add an entry in the `MIGRATIONS` table in
    `crates/fleet-core/src/store/schema.rs`;
  - add a test in the same style as `migration_055_*`.
- **Tests first.** Every behaviour change starts with a failing test, and the
  report must quote the RED output.
- **Gates**, each run as a separate foreground command from the worktree root:
  - `cargo fmt --all --check`
  - `cargo clippy -p fleet-core -p fleet-hub --all-targets -- -D warnings`
  - `cargo test -p fleet-core -p fleet-hub`
  - Add `-p claude-fleet` to clippy and test when `src-tauri` is touched.
- **Suite result.** The full `fleet-core` suite has 3239+ tests and takes
  ~4 minutes. Judge the result by the `test result:` lines, never through
  `| tail` alone.

## Task 1: `list_sessions` never probes the fleet inline; agents cadence; GC off the tick

**Files:**
- `crates/fleet-core/src/service/sessions/reconcile.rs`
- `crates/fleet-core/src/service/tick.rs`
- `crates/fleet-core/src/service/gc*` only if a single-flight spawn helper is
  needed there

### Problem

**(a) `list_sessions` can run a whole pass inside the request.**
- `list_sessions_with` (reconcile.rs ~1519-1533) runs
  `run_full_reconcile(...).await` when `force || !gate.is_fresh(window)`.
  That is a probe of every host over SSH (up to `HOST_PROBE_TIMEOUT` = 65 s
  per host) plus all the writes.
- The window is the tick interval (20 s), measured from when the last pass
  completed.
- The tick uses `MissedTickBehavior::Skip`. So once a tick body runs over
  20 s, callers find stale data with no pass running, and the request itself
  pays for the pass.
- This is the ~15 s `list_sessions`.

**(b) The agents cadence never applies in production.**
- `ReconcileDeps::real` (~256-273) builds `last_agents: DashMap::new()` on
  every call.
- It is called once per pass (lines ~1588, ~1606, ~1624, ~1644 and
  `lifecycle.rs:975`), so `agents_due` (~348) always returns true.
- As a result, `claude agents --json` runs on every host on every pass. The
  intended `AGENTS_CADENCE` (once a minute) never applies.

**(c) The GC sweep runs inline in the tick body.**
- `service::gc::maybe_sweep` (tick.rs ~107) is awaited inline in the tick body,
  after the reconcile.
- It does SSH work, so a slow sweep stretches the tick past its period.

### Required behaviour

1. **`list_sessions_with` with `force == false` and stale data:**
   - If at least one pass has completed in this process, return the stored
     rows at once and start a detached background pass. Start it through the
     same `ReconcileGate` so it never overlaps a running pass.
   - If no pass has completed yet (cold start), keep today's inline pass, so
     the first listing is not empty.
   - `force == true` keeps today's inline pass.
   - Whatever `spawn` the pass needs (an `Arc` of the store and deps) must be
     threaded through the public `list_sessions` without changing any Tauri
     command or MCP tool signature.
2. **`last_agents` state lives for the life of the process** for the real deps.
   - Follow the pattern of `crate::service::outcome::pr_probe_cache()`, a
     process-wide static.
   - `ReconcileDeps::fake*` keep their own fresh map, so tests stay isolated.
3. **The tick no longer awaits `gc::maybe_sweep` inline.**
   - It spawns it single-flight, the way `service::usage::spawn_collect` does:
     a second spawn while one runs is a no-op.
   - `playbooks::run` stays inline after the reconcile, because it needs the
     fresh stuck stamps.

### Tests (RED first)

- A stale gate after one completed pass: `list_sessions_with(force=false)`
  returns without awaiting the probe. Use a fake exec that blocks or sleeps
  well past the assertion's timeout. A pass is then observed to have started
  or run through the gate.
- Cold start (no completed pass): `list_sessions_with` still runs the pass
  inline. The existing tests for that path must keep passing.
- `force=true` still runs inline.
- Two consecutive `ReconcileDeps::real` builds share `last_agents`: after
  `agents_due` records host X, a freshly built real deps reports X as not due
  within the cadence.
- Single-flight GC spawn: a second spawn while the first runs does nothing.
  Test at the helper level.

### Out of scope

- Changing the default interval.
- Changing `reconcile_now` semantics.
- The write phase of reconcile (Task 4).

## Task 2: Audit rows for read-only tools; event prune cost

**Files:**
- `crates/fleet-core/src/mcp/tools/mod.rs`
- `crates/fleet-core/src/mcp/tools/support.rs`
- `crates/fleet-core/src/store/timeline.rs`

### Problem

**The audit write runs on every tool call, reads included.**
- `call_tool` calls `persist_audit` (tools/mod.rs ~299) before every tool.
- `find_audit_session` (support.rs ~466-489) falls back to the controller
  session when the arguments name none.
- So `whoami`, `list_hosts`, `list_sessions` and every other read write an
  audit row whenever a controller is set.

**Each audit row is two autocommits under the store mutex.**
- `insert_session_event` (timeline.rs ~95-112) runs an INSERT plus a pruning
  `DELETE … NOT IN (SELECT … LIMIT cap)` as two autocommits.
- The desktop's poll alone adds ~720 audit rows an hour.

### Required behaviour

1. **No audit row for read-only tools.**
   - Tools that are in the read-only set (`READONLY_TOOLS`, or whatever set the
     code already uses for `readonly` tokens) write no audit row.
   - The `audit()` log line (tracing) stays for every call.
   - Refused calls to non-read tools are still audited, as today ("Audit first
     so refused calls are on the timeline too").
   - Peer handling is unchanged.
2. **`insert_session_event` does its INSERT and prune in one transaction.**
   - Use a SAVEPOINT so it works whether or not the caller is already inside a
     transaction (it is called from `atomically` blocks elsewhere).
   - Run the prune only when the session's row count exceeds
     `SESSION_EVENTS_CAP`. A cheap `COUNT(*)`, or pruning every Nth insert, is
     acceptable; pick one and document it in a comment.
   - The cap semantics stay: after any insert, at most `SESSION_EVENTS_CAP`
     rows (or cap + N-1 if you choose every-Nth) remain.

### Tests (RED first)

- With a controller set, a read-only tool call (for example `list_hosts`)
  leaves no new `session_events` row on the controller.
- A mutating tool call is still audited.
- A refused call is still audited.
- `insert_session_event` works inside an outer `atomically` transaction, and
  its row rolls back with it.
- The cap is still enforced after inserting past `SESSION_EVENTS_CAP`.

### Out of scope

- Moving audit writes to a background channel.

## Task 3: Hook ingestion in one transaction; no-op writes skipped

**Files:**
- `crates/fleet-core/src/service/hooks.rs`
- `crates/fleet-core/src/store/sessions.rs`
- `crates/fleet-core/src/store/conversations.rs`
- `crates/fleet-core/src/store/participants.rs`, read only, as the pattern
  reference

### Problem

**A Stop hook costs 8–9 separate commits.**
- The Stop hook (hooks.rs ~873-925) runs each of these as its own commit:
  - `conversation_bump_turns`
  - two transcript-path updates
  - `record_stop_hook_for_row`
  - an event (2 commits before Task 2)
  - the journal
  - possibly a handover
- UserPromptSubmit costs ~5 commits.

**Several updates write even when the value is unchanged.**
- The transcript-path updates (`store/sessions.rs` ~1380,
  `store/conversations.rs` ~283) write unconditionally.
- The `sessions_row_version_bump` trigger (migration 042) turns each of them
  into a real write and a `row_version` bump.
- `refresh_context` runs an unconditional `set_context`
  (conversations.rs ~442).

### Required behaviour

1. **Each `apply_*_hook` body's store writes run in one transaction.** Use the
   existing `Store::atomically`.
   - Rows and events must be identical to today's for the same input.
   - SSH, network or other blocking work must not move inside the
     transaction; restructure the code to compute first, then write.
2. **The transcript-path, `set_context` and similar hook-path updates skip
   no-op writes.**
   - Add an `AND <col> IS NOT ?` guard, as
     `reset_stop_block_streak` in `store/participants.rs` already does.
   - Skip the matching event or `row_version` bump only when nothing changed.

### Tests (RED first)

- Replaying the same Stop hook payload twice does not bump the session's
  `row_version` on the second run for the transcript-path fields.
  Turn counters may legitimately change; assert only on the guarded fields.
- A Stop hook whose last write fails leaves none of its earlier writes
  committed. Use a failure-injection seam that already exists, or a constraint
  violation.
- The existing hook tests keep passing unchanged.

### Out of scope

- Changing what the hooks record.

## Task 4: Reconcile write phase — one transaction per host, no unconditional writes

**Files:**
- `crates/fleet-core/src/service/sessions/reconcile.rs`
- `crates/fleet-core/src/store/reconcile.rs`
- `crates/fleet-core/src/store/hosts_accounts.rs`
- `crates/fleet-core/src/store/sessions.rs`, for `upsert_bg_session` only if
  needed

### Problem

**The per-host write step commits many things outside its transaction.**
`reconcile_write_one_host` holds the guard for the whole per-host write step
(~700-870). Outside the `apply_host_reconcile` transaction it commits:
- `set_host_identity`, unconditionally (reconcile.rs ~748 →
  hosts_accounts.rs ~513)
- `mark_sessions_reconciled`, every pass (~863 → store/reconcile.rs
  ~801-815). This also bumps `row_version` on every live session each pass,
  so it defeats the snapshot cursor.
- `set_pr_signals` in a loop (~790)
- events (~711, ~827)
- `upsert_bg_session` per background agent (~1121)
- `ghost_and_clean_bg_sessions` (~1150)

**The loop gives readers no reliable turn.** The per-host loop (~1458-1471)
drops the lock and immediately re-takes it. `std::sync::Mutex` is not fair, so
readers can wait through the whole write phase.

### Required behaviour

1. **One transaction per host.** For each host, all of the writes above
   commit in one transaction, together with `apply_host_reconcile`'s. Add
   `_in_tx` variants as needed. Events keep being held until commit
   (`atomically`).
2. **`set_host_identity` writes only when a value changed.**
3. **`mark_sessions_reconciled` stops bumping `row_version`** when the only
   change is `last_reconciled_at`.
   - Preferred: fold `last_reconciled_at` into the upsert.
   - Alternative: exclude the column from the `row_version` trigger with a new
     migration.
   - If you exclude it from the trigger, check every consumer of
     `row_version` / snapshot cursors (`fresh_for`, `SessionRow.row_version`,
     `events`) and document why skipping is safe. If it is not safe, keep the
     bump and just fold it into the per-host transaction.
4. **Between hosts, yield so a waiting reader can take the lock.** Dropping
   the guard is enough if it is followed by `tokio::task::yield_now().await`.
5. **Rows, events and ghosting results are unchanged** for the existing
   reconcile tests.

### Tests (RED first)

- A pass whose probe result equals the stored state commits no change to
  `hosts` identity columns.
- A pass whose probe result equals the stored state does not bump any
  session's `row_version`. Only if requirement 3 made that safe; otherwise
  assert the bump happens exactly once per session per pass.
- An error injected after `apply_host_reconcile` rolls back that host's PR
  signals and events too.
- All existing reconcile tests pass.

### Out of scope

- The read path (Task 1).
- Probing.

## Task 5: Usage collection and tracker sync — batch per host, skip no-ops

**Files:**
- `crates/fleet-core/src/service/usage.rs`
- `crates/fleet-core/src/store/usage.rs`
- `crates/fleet-core/src/service/trackers/sync.rs`
- `crates/fleet-core/src/store/tracker_items.rs`

### Problem

**Usage collection commits once per session under one guard.**
- `service/usage.rs` ~635-645 takes one guard and loops over cursors.
- Each `apply_usage` is its own transaction (`store/usage.rs` ~178).
- Its UPDATE runs even when nothing grew (~215).
- This runs every 300 s, per host.

**Tracker sync commits once per item under one guard.**
- `store_items` (`service/trackers/sync.rs` ~379-396) holds one guard and
  autocommits per item.
- An unchanged item still runs `UPDATE work_items SET fetched_at`
  (`tracker_items.rs` ~336).

### Required behaviour

1. **Usage: one transaction per host.**
   - Rows whose token delta is zero and whose offset is unchanged write
     nothing.
   - Add an `apply_usage_in_tx` or equivalent.
2. **Tracker sync: one transaction per batch.**
   - Batch the unchanged items' `fetched_at` into a single
     `UPDATE … WHERE id IN (…)`, chunked if needed.
   - Do not drop `fetched_at`: freshness logic reads it.
3. **Results are unchanged.** Totals, cursors, work items and events come out
   the same.

### Tests (RED first)

- A usage collection with no new transcript bytes writes no row. Assert on
  `total_changes()` or on the row versions/timestamps.
- A usage collection with growth for 2 of 3 sessions updates exactly 2 rows.
- Tracker sync of N unchanged items issues one batched `fetched_at` update and
  leaves the item rows identical except `fetched_at`.

### Out of scope

- Parallelising usage across hosts.

## Task 6: Read-path queries — worktree N+1, index, whoami, lost filter

**Files:**
- `crates/fleet-core/src/service/worktrees.rs`
- `crates/fleet-core/src/store/projects.rs`
- `crates/fleet-core/migrations/056_session_worktree_index.sql`, new
- `crates/fleet-core/src/store/schema.rs`
- `crates/fleet-core/src/service/sessions/targeting.rs`
- `crates/fleet-core/src/store/sessions.rs`

### Problem

- **`list_worktrees` is N+1.** It runs one query per project, then one per
  worktree (`service/worktrees.rs` ~42-60).
- **`alive_sessions_for_worktree` scans the table.** It filters on
  `sessions.worktree_id` (`store/projects.rs` ~449), which has no index.
- **`whoami` loads every session to find one.**
  - `resolve_by_tmux_name` / `whoami` (`targeting.rs` ~103) loads every
    session, lost ones included, to find one `tmux_name`.
  - Each loaded row carries per-row subqueries (`store/rows.rs` ~300-349).
- **`list_all_sessions` filters lost rows late.** It (`store/sessions.rs`
  ~540) filters `lost_at` after loading rather than in SQL, where a caller
  wants live rows only.

### Required behaviour

1. **Migration 056** adds
   `CREATE INDEX IF NOT EXISTS idx_sessions_worktree_live ON sessions(worktree_id) WHERE lost_at IS NULL`.
   - Register it in `MIGRATIONS`.
   - Add a migration test in the `migration_055_*` style.
   - Bump whatever `LATEST_SCHEMA_VERSION` test expects.
2. **`list_worktrees` builds its answer with a constant number of queries**
   (a JOIN or one grouped query), not a query per worktree. Its output stays
   identical.
3. **`whoami` resolves by `WHERE tmux_name = ?`** (plus host if given) with the
   same row shape and the same `E_NOTFOUND` / `E_AMBIGUOUS` semantics,
   including the candidates list.
4. **Any caller that only wants live rows** gets them filtered in SQL. Keep
   `include_lost` behaviour exact.

### Tests (RED first)

- The migration test.
- `list_worktrees` output parity on a fixture with several projects and
  worktrees and live and lost sessions.
- `whoami` returns the same result for the unique, ambiguous and not-found
  cases.
- The query plan uses the index: `EXPLAIN QUERY PLAN` contains
  `idx_sessions_worktree_live`.

### Out of scope

- `fleet_health` roll-up internals.

## Task 7: Readers off the writer — read connection pool and in-memory token cache (hub)

**Files:**
- `crates/fleet-core/src/store/mod.rs`
- `crates/fleet-core/src/mcp/mod.rs` (authorize)
- `crates/fleet-core/src/mcp/auth.rs`
- `crates/fleet-core/src/mcp/tools/mod.rs` (FleetTools)
- the few read tools named below
- `crates/fleet-hub/src/serve.rs`

### Problem

- **`authorize` takes the store lock on every request.**
  - `authorize` (`mcp/mod.rs` ~193-202) locks the store to read the host and
    client token tables on every request, `/hook` included.
  - `HostRouter::agent_alias` / `credential_is_current`
    (`agent/router.rs` ~67-89) lock it per agent frame.
- **Read tools wait behind writers.** They all share the one writer
  connection's mutex.

### Required behaviour

1. **A `ReadPool`.** It is a small fixed set (2–4) of read-only connections on
   the same file:
   - each is a `Mutex<Store>` opened with `Store::open_read_only`;
   - each has the busy timeout that commit `2d7415fd` added;
   - it is built only when the store is file-backed and WAL is active.
   - Hand out a connection round-robin or first-free. It must never block
     forever; fall back to the writer if the pool is absent.
2. **The hub opens the pool** in `serve.rs` and threads it into `FleetTools`
   and the auth state as `Option<…>`. Tests and the desktop pass `None` and
   keep today's behaviour.
3. **These paths read through the pool:**
   - `list_hosts`
   - `whoami`
   - the final read in `list_sessions`
   - `list_worktrees`
   - `list_projects`
   - `fleet_health`
   Each still works with `None`.
4. **The token check in `authorize` hits an in-memory cache.**
   - The cache is rebuilt from the store on start.
   - Every write path that changes host tokens, client tokens or the master
     token invalidates it: mint, rotate, revoke, trust, pair and regenerate.
     Find them all with `grep`.
   - A revoked token must be refused on the very next request. This is
     security-critical.
5. **Read-your-writes is preserved for a caller.** A tool that writes and then
   reads in the same call reads through the writer.

### Tests (RED first)

- A revoked client token is refused on the next request with the cache on.
- A newly paired or minted token is accepted on the next request.
- A read through the pool sees a write committed through the writer
  immediately before it (WAL).
- With the writer mutex held by another thread, `list_hosts` through the pool
  completes without waiting for it. Use a timeout-bounded assertion.
- The pool is absent → the tools behave exactly as before. The existing
  suites cover this.

### Out of scope

- `spawn_blocking` for store work.
- The desktop (`src-tauri`) using the pool.
- Agent-router caching, beyond reading through the pool if trivial.
