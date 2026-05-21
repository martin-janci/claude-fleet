# Performance & Improvements Analysis — 2026-05-21

A two-agent analysis (Rust backend, Svelte frontend) of `claude-fleet` at
commit `aa29280`, focused on performance and quality improvements — distinct
from the bug/security pass in `2026-05-21-hardening-review.md`.

Each finding has an **IMPACT** rating (HIGH / MEDIUM / LOW) for how much perf
or quality it buys. Nothing here is a correctness bug.

## Resolution status (updated 2026-05-21)

**Implemented** (commits on `main`): B1, B2, B3, B4, B6, B7, B8, B9, B10, B11,
B12, B13, B16, B17, B18, B19; F1, F2, F3, F4, F6, F7, F10, F11, F12, F15, F17.
All verified — `cargo test` + `cargo clippy -D warnings` + `vitest` green.

**Deliberately not done** (judgement calls — cost exceeded value):

- **B5** (skip the login-shell spawn at startup) — the only safe heuristic
  risks regressing the Finder-launch PATH recovery; deferred.
- **B14 / B15** (`DELETE`/`INSERT … RETURNING`) — LOW micro-opts; the
  store.rs refactor risk isn't worth a sub-µs gain at current scale.
- **F8** (Map-backed stores) — the real win needs microtask-batched
  notification, which breaks the synchronous store contract the
  optimistic-update path and the test suite rely on; the unbatched Map
  variant is no faster. Deferred.
- **F5** (sidebar virtualization) — fine at the current fleet size; revisit
  if session counts grow large.
- **F9 / F13 / F14** — the analysis itself rated these "acceptable / fine".
- **F16** (extract a `SessionRow` component) — a quality refactor; deferred
  to keep this batch free of behaviour-change risk.
- **F18** (a11y aria-labels) — the badges already carry `title`; minimal
  value for a single-user tool.

## Top wins — cheapest, highest-leverage first

| # | Change | Where | Effort | Payoff |
|---|--------|-------|--------|--------|
| B1 | Add secondary indexes (migration 007) | `migrations/` | trivial | removes every table-scan class |
| B7 | `prepare` → `prepare_cached` in hot helpers | `store.rs` | trivial | no SQL re-parse per reconcile |
| F4 | Delete duplicate store bootstrap | `Sidebar.svelte:59-66` | trivial | −4 startup IPC calls |
| B3 | Hoist `list_projects()` out of the per-session loop | `sessions.rs` | small | kills the worst N+1 |
| B2 | Stop re-reading rows after `apply_host_reconcile` | `sessions.rs` | small | −N redundant scans/reconcile |
| F1 | Adaptive backoff on `pty_drain` poll | `TerminalView.svelte` | small | kills idle 33 Hz IPC churn |
| F2 | Dirty-row tracking in `Screen` | `ansi.ts` | medium | per-frame O(changed) not O(rows×cols) |
| B4 | Cache `remote_home` per host | `ssh.rs`/`sessions.rs` | small | −1 SSH round-trip per `new_session` |

---

## Backend (Rust)

### HIGH

- **B1 — Zero secondary indexes.** The schema (migrations 001–006) has no
  `CREATE INDEX`. Every filter by `host_alias`, `project_id`, `worktree_key`
  is a full table scan; `list_sessions` (fired on every window focus) does N
  scans. Add migration 007 with
  `idx_sessions_host`, `idx_sessions_project_wtkey`, `idx_worktrees_project`.
- **B2 — `reconcile_sessions` re-reads rows it just wrote.**
  `sessions.rs:140-162` calls `list_sessions_for_host` per host *after*
  `apply_host_reconcile`, then `list_hosts()` a third time. Have
  `apply_host_reconcile` return the resulting rows, or do one `SELECT` after
  the loop; reuse the step-1 `hosts` snapshot.
- **B3 — `find_project_id_for_path` runs `list_projects()` per live session.**
  `sessions.rs:283-302` — full project-table read + deserialize + linear scan
  for every session on every host. Hoist `list_projects()` to one call at the
  top of reconcile; pass a `&[ProjectRow]` slice to a pure matcher.
- **B4 — `new_session` does 5–7 serialized SSH round-trips.** `remote_home`
  (`sessions.rs`) is a dedicated `printenv HOME` round-trip. Fold `$HOME` into
  the `ensure_remote_project` script (bash already has it) and/or cache
  `remote_home` per host in a `DashMap` on `SshClient` — it never changes.

### MEDIUM

- **B5 — `import_login_shell_env` spawns a login shell on the startup hot
  path** (`lib.rs:159`), 100–500ms of dead time before the window. Skip the
  shell spawn when `PATH`/`LANG` already look healthy (terminal launch);
  only pay it for Finder-stunted env.
- **B6 — `apply_host_reconcile` updates the project row once per session**
  (`store.rs:1086`). 10 sessions in one project ⇒ 10 `UPDATE`s + 10 emit
  events. Accumulate `max(last_activity)` per `project_id`, write once.
- **B7 — Statements re-`prepare()`d every call.** Use `prepare_cached()` in the
  hot `fetch_*` / `list_sessions_for_host` / `upsert_session` /
  `get_session_account` helpers (not the dynamic `NOT IN` SQL).
- **B8 — `send_prompt` does two serialized SSH calls** (`sessions.rs:864`) for
  the two `send-keys` commands. Join with `;` into one `bash -lc` script.
- **B9 — `probe_local` runs blocking `std::process::Command` in an async fn**
  (`hosts.rs:302`); same in `send_prompt_inner`'s local branch. Use
  `spawn_blocking` or `tokio::process`.
- **B10 — `pty_drain` does a full UTF-8 lossy decode + alloc every 30 ms**
  (`pty.rs:243`) while holding the buffer lock; splits multi-byte sequences at
  read boundaries. Swap the buffer out with `mem::take` and decode off-lock,
  or return raw bytes.
- **B11 — PTY reader buffer is unbounded** (`pty.rs:192`). A non-draining
  frontend (backgrounded tab) lets it grow without limit. Cap it (ring / drop-
  oldest).
- **B12 — `kill`/`rename`/`restart_session` do a full `list_sessions_for_host`
  to find one id.** Use the indexed `Store::get_session(tmux_name, host_alias)`.

### LOW (quality)

- **B13 — `reconcile_one_host` re-inlines ~40 lines** identical to
  `reconcile_write_one_host` — call the helper instead.
- **B14 — `delete_sessions_not_in` builds placeholders/params twice**; use
  `DELETE ... RETURNING id` to collapse SELECT+DELETE.
- **B15 — upserts do INSERT then a separate SELECT for the id** — use
  `INSERT ... ON CONFLICT ... RETURNING id`.
- **B16 — `parse_sessions` allocates a `Vec<&str>` per line** — destructure a
  `split` iterator instead.
- **B17 — `pane_command()` allocates a `String`** for a compile-time constant.
- **B18 — `upsert_host("local")` emits a spurious `host:added` every
  reconcile.** Only emit when the INSERT actually inserts.
- **B19 — `shutdown_all` closes masters serially with blocking `status()`** on
  the quit path — `spawn()` without waiting.

---

## Frontend (Svelte)

### HIGH

- **F1 — `pty_drain` polls at 33 Hz unconditionally**
  (`TerminalView.svelte:127`). Millions of empty IPC round-trips for an idle
  terminal. Adaptive backoff (30 ms → up to ~250 ms on empty drains, reset on
  data); or push a `pty:data` event from Rust and drop polling.
- **F2 — `visibleRows` rebuilds every row + a content-key string every drain**
  (`TerminalView.svelte:288`). 80×40 screen = 40 string-builds + 3200-cell
  scans per frame when usually a few rows changed. Track `dirtyRows` in
  `Screen`; recompute only changed rows, reuse cached `{key, runs}`.
- **F3 — `Screen.lineFeed`/scroll uses `Array.splice` per line** (`ansi.ts`).
  O(n×rows) during fast scroll + per-line allocation. Batch into one `splice`,
  or use a ring-buffer `rowOffset` for O(1) scroll.
- **F4 — Double bootstrap of all four stores.** `App.svelte` *and*
  `Sidebar.svelte` both bootstrap on mount → 8 startup IPC calls instead of 4.
  Delete `Sidebar.svelte:59-66`.
- **F5 — Session tree has no virtualization** (`Sidebar.svelte:417`). Fine at
  current fleet size; revisit if session counts grow. Interim: render row
  action buttons only for the hovered/selected row.

### MEDIUM

- **F6 — `runStyle()` rebuilds an inline style string per run per render**
  (`TerminalView.svelte:306`). Memoize by `${fg}|${bg}|${attrs}`.
- **F7 — Sidebar `filtered` re-derives the whole tree on every keystroke**
  (`Sidebar.svelte:97`). Debounce the search input ~150 ms; hoist `Date.now()`
  out of the per-project loop.
- **F8 — Store merges are O(n) `findIndex` + full `slice()` per event**
  (`sessions.ts:104`, `hosts.ts:109`, `projects.ts`, `accounts.ts`). An event
  burst is O(n²). Back stores with a `Map<id,Row>`, or coalesce bursts into one
  update per microtask.
- **F9 — `relatedCountById`/`sessionsByProject` rebuild fully on any session
  field change** (`Sidebar.svelte:147`). Acceptable once F8 batches bursts.
- **F10 — `SessionDetails` uses linear `.find` on hosts/accounts/projects**
  (`SessionDetails.svelte:21`), recomputed per store change, `accountForRow`
  is O(related×accounts). Export shared `hostByAlias`/`accountByUuid`/
  `projectById` derived maps from the store modules.
- **F11 — `setInterval` drain doesn't wait for the async call** — slow IPC
  piles up concurrent `pty_drain`s. Use a self-rescheduling `setTimeout` loop
  (pairs with F1).

### LOW

- **F12 — Layout `writePref` effects write to localStorage every
  resize-drag frame** (`App.svelte:27-32`). Persist on drag-end.
- **F13 — `formatRelative` recomputes via `Date.now()` in derived** — fine, or
  add a 60 s ticker if staleness matters.
- **F14 — terminal runs keyed by index** (`TerminalView.svelte:364`) — fine
  while rows are recreated wholesale; revisit with F2's dirty-row diffing.
- **F15 — `subscribeToRowEvents` registers 9 listeners serially** — use
  `Promise.all`.
- **F16 — ~65 lines of session-row markup duplicated** between the project
  tree and orphan section (`Sidebar.svelte`) — extract a `SessionRow` snippet/
  component.
- **F17 — `measureCellSize()` re-measures on every `openTerm`** despite a
  "once" comment — guard with `if (cellWidth > 0) return`.
- **F18 — Accessibility:** terminal `role="textbox"` and emoji-only badges —
  add `aria-label`s.

---

## Suggested implementation order

1. **B1 + B7 + B3 + B2** — one `store.rs`/`sessions.rs` commit; turns
   `list_sessions` (most-fired command) from O(H·S) scans into indexed cached
   queries. Highest leverage, lowest risk.
2. **F4 + F15** — startup IPC cleanup, trivial.
3. **F1 + F11** — adaptive self-rescheduling drain loop; kills idle churn.
4. **F2 + F6 + F3** — `Screen` dirty-row tracking + memoized styles +
   ring-buffer scroll; the terminal-render rework (largest, keep `ansi.test.ts`
   green; add tests for the dirty-set).
5. **B4 + B8 + B12** — SSH round-trip reductions.
6. **F8** — Map-backed stores.
7. Remaining MEDIUM/LOW as cleanup.
