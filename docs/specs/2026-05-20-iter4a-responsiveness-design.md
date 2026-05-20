# claude-fleet iter 4a — UI responsiveness design

**Status:** Draft
**Author:** Martin via Claude
**Date:** 2026-05-20
**Sibling spec:** `docs/specs/2026-05-20-cross-host-sessions-and-transfer-design.md` (iter 3)
**Follow-up specs (not yet written):** iter 4b — reviews-as-a-feature

## Goal

claude-fleet must stay responsive while doing long SSH / git / tmux I/O across multiple hosts. Today a single unreachable host freezes the UI for the full 5-second SSH `ConnectTimeout` because every mutation runs a full-fleet reconcile while holding the global `Store` mutex. iter 4a removes that whole class of freeze by adopting `tokio`, refactoring the locking pattern, switching to event-driven store updates, and adding cancellation infrastructure.

## Audit findings (load-bearing subset)

Two opus reviewer subagents audited the codebase before this spec. Full reports live in the conversation transcript; the subset that drives the design:

**Backend Critical** — `Mutex<Store>` is held across SSH I/O in every session-mutating command (`list_sessions`, `new_session`, `kill_session`, `rename_session`, `restart_session`). The lock wraps a sequential, sync, full-fleet reconcile. One unreachable host blocks every other Tauri command for 5s. (`src-tauri/src/commands/sessions.rs` lines 147-156, 340-416, 424-437, 446-469, 477-500.)

**Backend Important** — No `tokio` anywhere in `src-tauri/`. All process spawning uses `std::process::Command`. `SshClient::ensure_master` races concurrent first-touches for the same host. `refresh_projects` holds the lock during N local `git worktree list` calls.

**Frontend Critical** — `PromptComposer.send` is a sequential `for-of await` with blanket `disabled={sending}` on every control. `Sidebar` runs `O(M·N·3)` work on every keystroke (`relatedCountFor` × `sessionsForProject` × multiple call sites). Every mutation (`killSession`, `renameSession`, `restartSession`, `newSession`, all host/account mutations) chains `await loadX()` and replaces the entire store wholesale, cascading re-renders. `Sidebar.onMount` loads four collections in series.

**Frontend Important** — No cancellation infrastructure (`invokeCmd` takes no `AbortSignal`). `window.focus` re-fetches projects+sessions on every focus without throttle. `TerminalView` polls `pty_drain` at 33Hz even when idle.

## Architecture

Four foundational decisions; everything else falls out of them.

### 1. Async runtime

Adopt `tokio` across the backend. `SshClient`, `tmux.rs`, and `commands/projects.rs` git ops all move to `tokio::process::Command`. Affected `#[tauri::command]` functions become `async fn`. Each SSH call cooperatively yields instead of burning a blocking-pool thread; concurrent calls multiplex on a small number of executors. Tauri 2's `async_runtime` is already tokio-backed — we ride that.

### 2. Lock pattern: read-snapshot / write-burst

Today the `Mutex<Store>` lock is held across both SQL and I/O. New rule:

1. Acquire lock briefly to clone the rows needed (`Vec<HostRow>`, `Vec<SessionRow>`, etc.).
2. Drop the lock.
3. Do all I/O (SSH, tmux, git, network) off-lock.
4. Re-acquire lock briefly to write results inside `conn.transaction()` — one fsync per batch.

A new `Store::with_snapshot<F, R>(&self, f: F) -> R` helper documents the read-only pattern. A `Store::with_transaction<F, R>` helper wraps the write side. Both exist mainly for clarity; the discipline is what matters.

### 3. Event-driven store updates

Backend emits typed Tauri events on every row change (`session:created`, `session:updated`, `session:killed`, `host:probed`, `account:upserted`, etc.). Frontend stores subscribe via `listen()` and **patch in place by id**. Mutation IPC wrappers no longer chain `await loadX()` — they return the affected row, the frontend patches it optimistically, and the event confirms or corrects.

`loadX()` becomes bootstrap-only: initial paint and explicit recovery.

This single change unblocks roughly half the frontend audit findings: no more wholesale `sessions.set(...)`, no more cascading `$derived` re-renders, no more identity churn in `{#each row (row.id)}`.

### 4. Cancellation via tokens

A new `invokeCmdAbortable(cmd, args, signal)` wrapper in `src/lib/ipc.ts` lets every long-running command take an `AbortSignal`. The wrapper mints a `call_id`, passes it to the backend, and listens for `signal.aborted` to fire a `cancel_command(call_id)` IPC.

Backend keeps a global `CancellationRegistry: DashMap<u64, CancellationToken>`. Each long-running command registers a token, threads it through SSH / git calls, and uses `tokio::select!` to bail out on cancel. SSH child processes get killed via `tokio::process::Child::start_kill`.

UI exposes per-row "Cancel" only where it matters in practice: probe (potentially 5-30s SSH handshake) and `ensure_remote_project`'s 120s `git clone`.

### Two derived patterns

- **Per-host reconcile granularity.** `new/kill/rename/restart_session` only re-probe their target host via a new `reconcile_one_host(alias)`. Full-fleet reconcile is reserved for explicit user-driven "Refresh All".
- **Parallel fan-out.** `tokio::task::JoinSet` on the backend for reconcile across hosts; `Promise.allSettled` on the frontend for `PromptComposer.send`.

### Deferred

Sidebar virtualization (windowing) is **out of scope for iter 4a**. Memoised indices (M5 below) buy more than windowing would for fleets under ~100 sessions, and virtualization conflicts with terminal-attach lifecycle. Re-evaluate when a real user crosses that threshold.

## Backend changes by component

### Dependencies (`src-tauri/Cargo.toml`)

- `tokio = { version = "1", features = ["full"] }`
- `tokio-util = { version = "0.7", features = ["rt"] }`
- `dashmap = "6"` (for `ensure_master` per-host idempotency and the cancellation registry)

Tauri 2's `async_runtime` is already tokio — re-export the runtime handle from `lib.rs` to avoid double-runtime confusion.

### SshClient (`src-tauri/src/ssh.rs`)

```rust
impl SshClient {
    pub async fn run(&self, alias: &str, cmd: &str) -> Result<String, IpcError> { ... }
    pub async fn ensure_master(&self, alias: &str) -> Result<(), IpcError> { ... }
}
```

Internals:

- `tokio::process::Command` for all spawn / `output()` / `status()` calls.
- Replace `Mutex<HashSet<String>>` with `DashMap<String, Arc<OnceCell<()>>>`. First touch wins; concurrent callers `await once_cell.get_or_try_init(|| spawn_master(alias))`.
- Cancellation: `pub async fn run_cancellable(&self, alias, cmd, token: CancellationToken)` selects on the token and `start_kill()`s the child.

### tmux (`src-tauri/src/tmux.rs`)

- `TmuxExec` trait methods all become `async fn`.
- `LocalTmux` uses `tokio::process::Command` directly.
- `RemoteTmux` calls `SshClient::run` (already async).

### Store (`src-tauri/src/store.rs`)

- New helper: `pub fn with_snapshot<F, R>(&self, f: F) -> R where F: FnOnce(&Store) -> R { f(self) }`. Trivial but its presence in callsites communicates "everything inside this closure runs under the lock; release before I/O."
- New helper: `pub fn with_transaction<F, R>(&mut self, f: F) -> rusqlite::Result<R> where F: FnOnce(&Transaction) -> rusqlite::Result<R>`. Wraps `conn.transaction()`.
- New `EventBus` trait the store calls into on mutations:

  ```rust
  pub trait EventBus: Send + Sync {
      fn emit_session_created(&self, row: &SessionRow);
      fn emit_session_updated(&self, row: &SessionRow);
      fn emit_session_killed(&self, id: i64);
      fn emit_host_probed(&self, row: &HostRow);
      fn emit_host_added(&self, row: &HostRow);
      fn emit_host_removed(&self, alias: &str);
      fn emit_account_upserted(&self, row: &AccountRow);
      fn emit_project_updated(&self, row: &ProjectRow);
      fn emit_worktree_updated(&self, row: &WorktreeRow);
  }
  ```

  Default impl in `lib.rs` wraps `tauri::AppHandle::emit`. Tests pass a `NoopEventBus` or a `RecordingEventBus`.

- `upsert_session` and friends call into the `EventBus` after a successful write, inside the same transaction commit window.

### Cancellation registry (`src-tauri/src/cancel.rs`, new)

```rust
pub struct CancellationRegistry {
    tokens: DashMap<u64, CancellationToken>,
    next_id: AtomicU64,
}

impl CancellationRegistry {
    pub fn register(&self) -> (u64, CancellationToken) { ... }
    pub fn bind(&self, external_id: u64, token: CancellationToken) { ... }
    pub fn cancel(&self, id: u64) { ... }
}
```

Registered as `State<'_, Arc<CancellationRegistry>>`.

```rust
#[tauri::command]
pub async fn cancel_command(call_id: u64, reg: State<'_, Arc<CancellationRegistry>>) -> Result<(), IpcError> {
    reg.cancel(call_id);
    Ok(())
}
```

### Reconcile (`src-tauri/src/commands/sessions.rs`)

`reconcile_sessions` becomes an `async fn` that:

1. Briefly locks store, snapshots `(Vec<HostRow>, Vec<ProjectRow>)`, drops lock.
2. Spawns one task per host onto a `JoinSet`, each task running `ssh + tmux list_sessions`.
3. Awaits all tasks.
4. Re-locks store, opens a transaction, applies all upserts/deletes/probes, emits events for each row change, commits.

New `reconcile_one_host(s: &mut Store, ssh: &SshClient, alias: &str)` for targeted mutations.

`new_session`, `kill_session`, `rename_session`, `restart_session`:

- Become `async fn`.
- Call `reconcile_one_host(alias)` instead of full-fleet `reconcile_sessions`.
- Return the affected `SessionRow` so the frontend can patch in place without waiting for the event round-trip.

### list_projects + refresh_projects (`src-tauri/src/commands/projects.rs`)

- `list_projects`: single SQL JOIN (`SELECT p.*, w.* FROM projects p LEFT JOIN worktrees w ON w.project_id = p.id ORDER BY ...`), grouped in Rust. Eliminates the N+1.
- `refresh_projects`: snapshot project list under lock, drop, run `tokio::process::Command::new("git").args(["worktree","list","--porcelain"]).output().await` per project concurrently via `JoinSet`, re-acquire lock for the write burst.

## Frontend changes by component

### IPC bridge (`src/lib/ipc.ts`)

New helper:

```ts
export async function invokeCmdAbortable<T>(
  cmd: string,
  args: Record<string, unknown>,
  signal?: AbortSignal,
): Promise<Result<T>> {
  const call_id = nextCallId();
  const onAbort = () => { invoke('cancel_command', { call_id }).catch(() => {}); };
  signal?.addEventListener('abort', onAbort, { once: true });
  try {
    return await invokeCmd<T>(cmd, { call_id, ...args });
  } finally {
    signal?.removeEventListener('abort', onAbort);
  }
}
```

Existing `invokeCmd` keeps working for short ops; `invokeCmdAbortable` for SSH/git callers.

### Event subscription (`src/lib/events.ts`, new)

```ts
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export async function subscribeToRowEvents(handlers: {
  onSessionCreated?: (row: SessionRow) => void;
  onSessionUpdated?: (row: SessionRow) => void;
  onSessionKilled?: (payload: { id: number }) => void;
  // ... host, account, project, worktree
}): Promise<UnlistenFn> { ... }
```

Single subscription site keeps event names in one place.

### Stores — events-first (`sessions.ts`, `hosts.ts`, `accounts.ts`, `projects.ts`)

Each store gains:

- `bootstrap()`: full IPC list, sets the store. Called once at app start.
- `mergeOne(row)`: patches the array entry by `id` (or `uuid` / `alias` where appropriate), creating if missing.
- `removeOne(key)`: removes by id/uuid/alias.

Existing `loadX()` becomes a thin alias for `bootstrap()` — kept for the manual "Refresh" button only.

Mutation wrappers (e.g. `killSession(id)`) no longer chain `await loadSessions()`. They invoke the backend command, receive the updated row, call `sessions.mergeOne(row)` (or `removeOne(id)`), and return. The event reconciles authoritatively a moment later.

### PromptComposer.send (`src/lib/PromptComposer.svelte`)

```ts
async function send() {
  sending = true;
  errors = {};
  succeeded = {};
  const targets = displayTargets.filter((t) => checked[t.id]);
  await Promise.allSettled(
    targets.map(async (t) => {
      const key = targetKey(t);
      const r = await sendPrompt(t.host_alias, t.tmux_name, prompt);
      if (r.ok) succeeded[key] = true;
      else errors[key] = r.error.message;
    }),
  );
  sending = false;
  if (Object.keys(errors).length === 0) setTimeout(() => onClose(), 600);
}
```

Remove `disabled={sending}` from `Cancel` button, checkboxes, textarea. Keep it on `Send` only.

### Sidebar memoised indices (`src/lib/Sidebar.svelte`)

```ts
const sessionsByProject = $derived.by(() => {
  const m = new Map<number, SessionRow[]>();
  for (const s of $sessions) {
    if (s.project_id == null) continue;
    if (!m.has(s.project_id)) m.set(s.project_id, []);
    m.get(s.project_id)!.push(s);
  }
  return m;
});

const relatedCountById = $derived.by(() => {
  const m = new Map<number, number>();
  const byKey = new Map<string, SessionRow[]>();
  for (const s of $sessions) {
    if (s.project_id == null) continue;
    const k = `${s.project_id}:${s.worktree_id ?? 'null'}`;
    if (!byKey.has(k)) byKey.set(k, []);
    byKey.get(k)!.push(s);
  }
  for (const list of byKey.values()) {
    for (const s of list) m.set(s.id, list.length - 1);
  }
  return m;
});
```

All inline `$sessions.filter(...)` lookups in the template are replaced by O(1) `Map` gets.

### App + Sidebar bootstrap parallelism

`Sidebar.onMount` (or `App.svelte` depending on where the call chain lives) becomes:

```ts
await Promise.all([
  bootstrapProjects(),
  bootstrapSessions(),
  bootstrapHosts(),
  bootstrapAccounts(),
]);
unlisten = await subscribeToRowEvents({ ... });
```

### window.focus throttle (`src/App.svelte`)

```ts
let lastFocusFetch = 0;
window.addEventListener('focus', () => {
  const now = Date.now();
  if (now - lastFocusFetch < 30_000) return;
  lastFocusFetch = now;
  void bootstrapProjects();
  void bootstrapSessions();
});
```

(In spirit obsolete once events keep state fresh, but kept as a recovery net for missed events / sleep wake.)

## Event schema

Backend emits via `app.emit("event:name", payload)`. Frontend listens via `@tauri-apps/api/event`'s `listen`.

| Event              | Payload          | Emitted from                                                | Frontend handler |
| ------------------ | ---------------- | ----------------------------------------------------------- | ---------------- |
| `session:created`  | `SessionRow`     | `Store::upsert_session` (new row case)                      | `mergeOne`       |
| `session:updated`  | `SessionRow`     | `Store::upsert_session` (existing row case)                 | `mergeOne`       |
| `session:killed`   | `{ id: i64 }`    | `Store::delete_session`                                     | `removeOne`      |
| `host:added`       | `HostRow`        | `Store::insert_host`                                        | `mergeOne`       |
| `host:probed`      | `HostRow`        | `Store::update_host_probe` / `set_host_account`             | `mergeOne`       |
| `host:removed`    | `{ alias: str }` | `Store::delete_host`                                        | `removeOne`      |
| `account:upserted` | `AccountRow`     | `Store::upsert_account`                                     | `mergeOne`       |
| `project:updated`  | `ProjectRow`     | `Store::upsert_project` / `touch_project_last_session_at`   | `mergeOne`       |
| `worktree:updated` | `WorktreeRow`    | `Store::upsert_worktree`                                    | `mergeOne`       |

Events are best-effort. If a listener misses one (bootstrap race, dropped event), the next mutation or focus refresh recovers; `bootstrap()` is canonical. Stores never trust events as the sole source of truth.

Event coalescing is **not** in iter 4a. If reconcile emits 50 `session:updated` events in a burst, the frontend handles them serially — still cheaper than wholesale array replace.

## Cancellation contract

```
Frontend                                  Backend
--------                                  -------
ac = new AbortController()                cancel_command(call_id) →
invokeCmdAbortable(cmd, args, ac.signal): registry.cancel(call_id)
  call_id = nextCallId()
  invoke(cmd, { call_id, ...args })       cmd impl (async fn):
                                            let token = reg.bind_or_register(call_id)?;
                                            tokio::select! {
                                              _ = token.cancelled() => Err(E_CANCELLED),
                                              r = inner_work(token.clone()) => r,
                                            }
  on(signal.abort):
    invoke('cancel_command', { call_id }) (fire-and-forget, ignore errors)
```

Cancelled commands return `Err({ code: 'E_CANCELLED', ... })`. SSH children are killed via `child.start_kill()`. `ensure_remote_project` checks the token between operations (`git clone`, `git checkout`, `git fetch`) and partially-cloned dirs are cleaned via a `Drop` guard on a temp dir.

## Error handling matrix

| Scenario                            | Backend                                                  | Frontend                                                  |
| ----------------------------------- | -------------------------------------------------------- | --------------------------------------------------------- |
| One host unreachable in reconcile   | `Err` for that host alone; others complete normally.     | That host marked `unreachable`; other sessions update.    |
| Probe times out                     | `E_TIMEOUT` after 5s ConnectTimeout.                     | Per-row error inline; other rows unaffected.              |
| User cancels long op                | `E_CANCELLED`. Child killed.                             | Treated as expected; no error toast.                      |
| Event listener race (bootstrap)     | n/a                                                      | Store may be stale by one row; next mutation or focus refresh recovers. |
| Backend panic in spawned task       | `JoinSet` returns `JoinError::Panic`; partial reconcile. | Toast: "Some hosts failed to refresh."                    |
| `git clone` partial / killed        | Temp dir removed via `Drop`. Caller sees `E_CANCELLED`.  | UI shows operation cancelled.                             |

## Testing strategy

### Rust (existing 88 stay green)

New tests:

- `ssh::ensure_master` concurrency: two callers, one master spawn (mock counter increments once).
- `reconcile_sessions` parallel fan-out: 3 mock hosts where 1 sleeps 5s — reconcile returns in ~5s, not 15s. Uses a mock `TmuxExec` that sleeps.
- `Store::with_snapshot` semantics: lock release verified via a parallel `try_lock` after closure returns.
- Cancellation: `tokio::select!` returns `E_CANCELLED` when token fires mid-SSH; SSH child seen as killed in mock.
- `EventBus`: `RecordingEventBus` captures every emit; assert correct events for each mutation.

### Vitest (existing 137 stay green)

New tests:

- `subscribeToRowEvents`: simulated `emit` calls update the store via `mergeOne`.
- PromptComposer parallel send: 3 mock targets; assert `sendPrompt` invoked concurrently (track call ordering with timestamps).
- Sidebar indices: render with 1000 mock sessions across 50 projects; assert `relatedCountById.get(id)` is constant time and the keystroke-to-paint budget is sub-frame.
- AbortSignal: `invokeCmdAbortable` cancels mid-flight; backend mock receives `cancel_command(call_id)`.

### Live verify

- Block port 22 of `mefistos` (`sudo pfctl` or `iptables`-equivalent). Open claude-fleet. Confirm UI stays snappy; the blocked host falls into `unreachable` after 5s; everything else (local sessions, other hosts, project switch, terminal attach) stays responsive.
- Open PromptComposer with 5 remote targets (use existing mefistos session + local siblings). Click Send. Confirm all sends fire concurrently (per-target ticks update at roughly the same time). Confirm `Cancel` button stays live; clicking it aborts the in-flight sends.
- Trigger a probe on an unreachable host; click the row's Cancel; confirm `E_CANCELLED` arrives within ~100ms and other rows are unaffected.

## Open risks

1. **`tokio` binary size.** `features = ["full"]` adds ~1-2 MB. Acceptable; trim features later if it matters.
2. **Event ordering across stores.** `session:created` and `account:upserted` may arrive out of order. If a session references an `account_uuid` not yet in the frontend account store, render `—` and let the next event fill in. Don't block render on cross-store consistency.
3. **`tauri::async_runtime` integration.** Need to confirm Tauri 2.x exposes `tokio::spawn` cleanly. If not, use `tauri::async_runtime::spawn` everywhere instead. Verify in M1.
4. **`git clone` cancellation.** `git` doesn't always exit cleanly on SIGKILL; orphaned `.git/objects` chunks possible. Mitigation: clone into a temp dir, atomically rename on success, `rm -rf` on cancel via `Drop`.
5. **Sidebar index rebuild cost.** Building `Map<projectId, SessionRow[]>` is O(N) per `$sessions` change. With event-driven updates that happens per row, not per refresh — but the `$derived` recomputes the whole map. Worth instrumenting once M3 ships; if it's a hot path, switch to in-place patching of the maps tied to the same event handlers.
6. **`EventBus` and `Store` lifetime.** The default `EventBus` impl needs `AppHandle`. `Store` doesn't have one. Solve by holding `Arc<dyn EventBus>` on the store, set during `lib.rs::setup`. Tests inject `NoopEventBus`.

## Out of scope (captured for follow-up specs)

- **Virtualization** (Sidebar windowing). Re-evaluate when a real user has 100+ sessions.
- **TerminalView push-based PTY.** Same `emit` story for byte streams. Spec it after M3 lands.
- **Cross-host related-session worktree mapping** (NULL vs id mismatch from iter 3 caveat). Separate spec.
- **`ensure_master` retry/backoff** for flapping hosts.
- **PTY buffer-lock optimisation** (`pty.rs:237-255`). Tiny win; not freeze-causing.
- **Iter 4b — reviews-as-a-feature.** Brainstorm queued after iter 4a ships.

## Milestone breakdown

Five shippable milestones; **M6 is documented but not shipped in iter 4a**.

### M1 — Tokio + lock-snapshot foundation (~half day)

Files: `src-tauri/Cargo.toml`, `src-tauri/src/ssh.rs`, `src-tauri/src/tmux.rs`, `src-tauri/src/store.rs`.

- Add deps (`tokio`, `tokio-util`, `dashmap`).
- `SshClient::run` + `ensure_master` → async + `tokio::process::Command`.
- `tmux.rs` ops → async on the `TmuxExec` trait.
- `Store::with_snapshot` + `Store::with_transaction` helpers.
- `ensure_master` uses `DashMap<String, Arc<OnceCell<()>>>`.

Behaviour unchanged at this milestone. New tests:

- `ensure_master` concurrency test.

Existing 88 Rust + 137 vitest stay green.

### M2 — Parallel reconcile + per-host granularity (~half day)

Files: `src-tauri/src/commands/sessions.rs`.

- `reconcile_sessions` → async, `JoinSet` fan-out, off-lock probes, batched write-burst.
- New `reconcile_one_host(s, ssh, alias)`.
- `new_session`, `kill_session`, `rename_session`, `restart_session` → async + per-host reconcile.
- Each mutation returns the affected `SessionRow`.

New tests:

- Parallel reconcile fan-out with mock TmuxExec, one slow host.
- Per-host reconcile leaves other hosts unchanged.

### M3 — Delta-event bus + frontend `listen` (~1 day, biggest milestone)

Files: `src-tauri/src/store.rs` (event hooks), `src-tauri/src/lib.rs` (default `EventBus` impl + setup), `src-tauri/src/commands/*.rs` (return-row patterns), `src/lib/events.ts` (new), all four frontend store files, `src/lib/ipc.ts`.

- Define event payloads (Rust enum + TS types).
- `EventBus` trait + `AppHandleEventBus` default impl.
- `Store::upsert_*` / `delete_*` call into `EventBus`.
- Frontend `subscribeToRowEvents` helper.
- Mutation IPC wrappers drop `loadX()` chain; use `mergeOne` / `removeOne`.

New tests:

- `RecordingEventBus` captures correct events per mutation.
- Frontend simulated `emit` updates store by id.

### M4 — Cancellation infrastructure (~half day)

Files: `src-tauri/src/cancel.rs` (new), `src-tauri/src/commands/{hosts,sessions}.rs` (callsites), `src/lib/ipc.ts` (`invokeCmdAbortable`), `src/lib/AddHostPicker.svelte`, `src/lib/NewSessionDialog.svelte`.

- `CancellationRegistry` + `cancel_command` Tauri command.
- `invokeCmdAbortable` wrapper.
- `probe_ssh_alias` + `ensure_remote_project` thread cancellation through SSH/git.
- UI: per-row Cancel button on `AddHostPicker` probe + `NewSessionDialog` clone progress.

New tests:

- AbortSignal aborts mid-call; backend mock receives `cancel_command`.
- Backend `select!` returns `E_CANCELLED`.

### M5 — Frontend perf cleanup (~half day)

Files: `src/lib/Sidebar.svelte`, `src/lib/PromptComposer.svelte`, `src/App.svelte`, `src-tauri/src/commands/projects.rs`.

- Sidebar memoised indices.
- PromptComposer parallel send.
- App `onMount` parallel bootstrap.
- `window.focus` throttle.
- `list_projects` JOIN.
- `refresh_projects` off-lock.

New tests:

- Sidebar with 1000 sessions: keystroke latency under 16ms.
- PromptComposer parallel-send timing assertion.

### M6 — Virtualization (deferred)

Not shipped in iter 4a. Spec entry only.

## Self-review

- **Placeholder scan:** no "TBD" / "TODO" remain. Every milestone names its files. Every code block compiles in spirit (types and method signatures align with existing code).
- **Internal consistency:** the four architectural pillars map cleanly to M1-M5. Event schema in §"Event schema" matches the `EventBus` trait in §"Backend changes". Cancellation contract pseudo-code matches the `CancellationRegistry` design and the frontend wrapper signature.
- **Scope check:** one spec, one feature, one iter. M3 is the biggest milestone — if it bloats during planning, it gets split into M3a (events) + M3b (frontend listen) without changing the spec.
- **Ambiguity check:** lock-pattern helpers are named (`with_snapshot`, `with_transaction`); the event schema is a table, not prose; the cancellation flow includes the abort-side fire-and-forget detail; event coalescing is explicitly declared out of scope.
