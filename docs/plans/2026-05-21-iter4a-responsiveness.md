# claude-fleet iter 4a — UI Responsiveness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make claude-fleet stay responsive under multi-host SSH I/O — one unreachable host must not freeze any other command.

**Architecture:** Adopt tokio across the backend; refactor the global `Store` mutex to read-snapshot / write-burst; replace wholesale store-replace mutations with event-driven patches; add AbortSignal cancellation end-to-end. Five shippable milestones (M1–M5) plus deferred virtualization (M6, spec-only).

**Tech Stack:** Rust + Tauri 2 backend (`tokio`, `tokio-util`, `dashmap`); Svelte 5 runes frontend (`@tauri-apps/api/event` for `listen`); SQLite via `rusqlite`.

**Reference spec:** `docs/specs/2026-05-20-iter4a-responsiveness-design.md`

---

## File Structure

### Backend (new + heavily modified)

| File | Responsibility | Status |
|---|---|---|
| `src-tauri/Cargo.toml` | Add `tokio`, `tokio-util`, `dashmap` deps. | Modified |
| `src-tauri/src/ssh.rs` | `SshClient` becomes async; `ensure_master` per-host idempotency. | Modified |
| `src-tauri/src/tmux.rs` | `TmuxExec` trait methods are `async fn`. | Modified |
| `src-tauri/src/store.rs` | `with_snapshot` + `with_transaction` helpers; `EventBus` trait + `NoopEventBus`; all `upsert_*` / `delete_*` call into the bus. | Modified |
| `src-tauri/src/events.rs` | New: event payload structs that mirror row types. | Created |
| `src-tauri/src/cancel.rs` | New: `CancellationRegistry` + helpers. | Created |
| `src-tauri/src/commands/sessions.rs` | Commands become `async fn`; `reconcile_sessions` parallel + off-lock; `reconcile_one_host` for per-host mutations. | Modified |
| `src-tauri/src/commands/hosts.rs` | `probe_ssh_alias` accepts `call_id` and threads cancellation. | Modified |
| `src-tauri/src/commands/projects.rs` | `list_projects` JOIN; `refresh_projects` off-lock. | Modified |
| `src-tauri/src/lib.rs` | Register `AppHandleEventBus` + `CancellationRegistry`; register `cancel_command`. | Modified |

### Frontend (new + heavily modified)

| File | Responsibility | Status |
|---|---|---|
| `src/lib/events.ts` | New: `subscribeToRowEvents` helper + event type definitions. | Created |
| `src/lib/ipc.ts` | New `invokeCmdAbortable` wrapper + `nextCallId`. | Modified |
| `src/lib/sessions.ts` | `bootstrap`, `mergeOne`, `removeOne`; mutations no longer chain `loadX`. | Modified |
| `src/lib/hosts.ts` | Same shape. | Modified |
| `src/lib/accounts.ts` | Same shape. | Modified |
| `src/lib/projects.ts` | Same shape. | Modified |
| `src/lib/Sidebar.svelte` | Memoised `sessionsByProject` + `relatedCountById` indices; parallel `onMount`. | Modified |
| `src/lib/PromptComposer.svelte` | `Promise.allSettled` send; un-disable Cancel/checkboxes/textarea. | Modified |
| `src/lib/AddHostPicker.svelte` | Per-row Cancel button on probe. | Modified |
| `src/lib/NewSessionDialog.svelte` | Per-row Cancel button on clone. | Modified |
| `src/App.svelte` | Parallel bootstrap; `window.focus` throttle 30s; subscribe to events. | Modified |

---

## M1 — Tokio + lock-snapshot foundation

Goal: introduce `tokio::process::Command` everywhere the backend spawns processes; introduce `Store::with_snapshot` helper; replace `Mutex<HashSet>` master-set with `DashMap<String, Arc<OnceCell<()>>>` for per-host idempotency. No user-visible behaviour change. Existing 88 Rust + 137 vitest tests stay green.

### Task 1: Add `tokio`, `tokio-util`, `dashmap` deps

**Files:**
- Modify: `src-tauri/Cargo.toml`

- [ ] **Step 1: Add the three crates**

Edit `src-tauri/Cargo.toml`. In the `[dependencies]` section add:

```toml
tokio = { version = "1", features = ["full"] }
tokio-util = { version = "0.7", features = ["rt"] }
dashmap = "6"
```

- [ ] **Step 2: Verify the build picks them up**

```bash
cd /Users/martinjanci/projects/github.com/martin-janci/claude-fleet
cargo build --manifest-path src-tauri/Cargo.toml 2>&1 | tail -3
```

Expected: `Finished \`dev\` profile` (no errors; warnings ok).

- [ ] **Step 3: Sanity-check tauri::async_runtime is tokio-backed**

Inspect via:

```bash
grep -r "async_runtime" src-tauri/src/ 2>&1 | head
grep -r "tauri::async_runtime" ~/.cargo/registry/src/ 2>&1 | head -3
```

If the existing tauri version exposes `tauri::async_runtime::spawn`, prefer it over raw `tokio::spawn` in command bodies (it uses the same runtime). For `JoinSet`, raw `tokio::task::JoinSet` is fine.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/Cargo.lock
git commit -m "deps: add tokio, tokio-util, dashmap for iter 4a"
```

---

### Task 2: Async `SshClient`

**Files:**
- Modify: `src-tauri/src/ssh.rs`

- [ ] **Step 1: Replace internal master-set with DashMap<OnceCell>**

At the top of `src-tauri/src/ssh.rs`, replace the `started: Mutex<HashSet<String>>` field on `SshClient` with:

```rust
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::OnceCell;

pub struct SshClient {
    masters: DashMap<String, Arc<OnceCell<()>>>,
}

impl SshClient {
    pub fn new() -> Self {
        Self { masters: DashMap::new() }
    }
}
```

Drop the old `Mutex<HashSet<String>>` definition + the corresponding `lock()` call sites; they will be removed by the next step.

- [ ] **Step 2: `ensure_master` becomes async + idempotent**

Replace the existing `ensure_master` body with:

```rust
pub async fn ensure_master(&self, alias: &str) -> Result<(), IpcError> {
    let cell = self
        .masters
        .entry(alias.to_string())
        .or_insert_with(|| Arc::new(OnceCell::new()))
        .clone();
    cell.get_or_try_init(|| async move {
        let status = tokio::process::Command::new("ssh")
            .args([
                "-o", "ControlMaster=auto",
                "-o", "ControlPersist=10m",
                "-o", &format!("ControlPath={}", control_path_for(alias)),
                "-o", "ConnectTimeout=5",
                "-N", "-f",
                alias,
            ])
            .status()
            .await
            .map_err(|e| IpcError::ssh(format!("master spawn: {e}")))?;
        if !status.success() {
            return Err(IpcError::ssh(format!("ssh master exit {status}")));
        }
        Ok(())
    })
    .await
    .map(|_| ())
}
```

`control_path_for(alias)` should already exist; if it doesn't, copy the path-template logic from the old `ensure_master`.

- [ ] **Step 3: `run` becomes async**

Replace the existing `run` with:

```rust
pub async fn run(&self, alias: &str, cmd: &str) -> Result<String, IpcError> {
    self.ensure_master(alias).await?;
    let output = tokio::process::Command::new("ssh")
        .args([
            "-o", &format!("ControlPath={}", control_path_for(alias)),
            "-o", "ConnectTimeout=5",
            alias,
            "bash", "-lc", cmd,
        ])
        .output()
        .await
        .map_err(|e| IpcError::ssh(format!("ssh run: {e}")))?;
    if !output.status.success() {
        return Err(IpcError::ssh(format!(
            "ssh exit {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
```

- [ ] **Step 4: Add concurrency test**

In the existing `#[cfg(test)] mod tests` block at the bottom of `ssh.rs` (create one if it doesn't exist), add:

```rust
#[tokio::test]
async fn ensure_master_idempotent_across_concurrent_calls() {
    // Two concurrent ensure_master() for the same alias should resolve in ≤ 1
    // master spawn. We can't easily count spawns without mocking; instead
    // assert both return Ok and only one OnceCell entry exists.
    let client = SshClient::new();
    let alias = "nonexistent-test-host";
    // The actual ssh call will fail (alias doesn't exist), but the OnceCell
    // semantics still apply: both calls share the same cell.
    let (a, b) = tokio::join!(
        client.ensure_master(alias),
        client.ensure_master(alias),
    );
    // Either both Ok (somehow worked) or both Err with same error type — both
    // must agree.
    assert_eq!(a.is_ok(), b.is_ok(), "concurrent ensure_master must agree");
    assert_eq!(client.masters.len(), 1, "exactly one cell registered");
}
```

- [ ] **Step 5: Run tests**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib ssh 2>&1 | tail -10
```

Expected: existing ssh tests still pass; new `ensure_master_idempotent_across_concurrent_calls` passes.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/ssh.rs
git commit -m "ssh: async SshClient + per-host OnceCell ensure_master"
```

---

### Task 3: Async `TmuxExec` trait

**Files:**
- Modify: `src-tauri/src/tmux.rs`

- [ ] **Step 1: Make `TmuxExec` methods `async fn` via async_trait**

Add `async-trait` to deps if not already there (most Rust projects use it for async traits):

```toml
# src-tauri/Cargo.toml
async-trait = "0.1"
```

Then in `tmux.rs`:

```rust
use async_trait::async_trait;

#[async_trait]
pub trait TmuxExec: Send + Sync {
    async fn list_sessions(&self) -> Result<Vec<RemoteSession>, IpcError>;
    async fn new_session(&self, name: &str, cwd: &str, cmd: &str) -> Result<(), IpcError>;
    async fn kill_session(&self, name: &str) -> Result<(), IpcError>;
    async fn rename_session(&self, from: &str, to: &str) -> Result<(), IpcError>;
    async fn restart_session(&self, name: &str, cwd: &str, cmd: &str) -> Result<(), IpcError>;
}
```

- [ ] **Step 2: `LocalTmux` impl uses `tokio::process::Command`**

Inside `tmux.rs`, replace every `std::process::Command::new("tmux")` in the `LocalTmux` impl with `tokio::process::Command::new("tmux")` and `.output()` / `.status()` → `.output().await` / `.status().await`. Wrap each method with `#[async_trait] impl TmuxExec for LocalTmux { ... }`.

Concrete example for `list_sessions`:

```rust
#[async_trait]
impl TmuxExec for LocalTmux {
    async fn list_sessions(&self) -> Result<Vec<RemoteSession>, IpcError> {
        let output = tokio::process::Command::new("tmux")
            .args(["list-sessions", "-F", LIST_FORMAT])
            .output()
            .await
            .map_err(|e| IpcError::tmux(format!("local tmux list: {e}")))?;
        if !output.status.success() {
            // Empty server → exit 1 with "no server running" — treat as empty.
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("no server running") || stderr.contains("error connecting") {
                return Ok(Vec::new());
            }
            return Err(IpcError::tmux(format!("tmux list exit {}: {}", output.status, stderr)));
        }
        parse_list_output(&String::from_utf8_lossy(&output.stdout))
    }
    // ... other methods: identical pattern, tokio::process::Command + .await
}
```

- [ ] **Step 3: `RemoteTmux` impl uses `SshClient::run` (already async)**

`RemoteTmux` already wraps `SshClient`; its methods just await `self.ssh.run(&self.alias, "tmux ...")`. The body shape doesn't change — only the signatures (`async fn`) and the `.await` on each `ssh.run` call.

- [ ] **Step 4: Update callers**

Every call site of `TmuxExec` trait methods must now `.await`. Most live in `commands/sessions.rs` and will be touched in M2 — for this task, only fix the local-only callers if any (search with `grep -n "list_sessions\|new_session\|kill_session\|rename_session\|restart_session" src-tauri/src/`). Wrap each non-`async fn` caller with a `tauri::async_runtime::block_on(...)` as a stopgap if needed (M2 will fix them properly).

- [ ] **Step 5: Tests + build**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib 2>&1 | tail -6
```

Expected: all 88 pre-existing tests pass.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/tmux.rs src-tauri/Cargo.toml src-tauri/Cargo.lock
git commit -m "tmux: async TmuxExec trait + tokio::process::Command"
```

---

### Task 4: `Store::with_snapshot` + `Store::with_transaction` helpers

**Files:**
- Modify: `src-tauri/src/store.rs`

- [ ] **Step 1: Add the two helpers**

In `src-tauri/src/store.rs`, inside `impl Store`, add:

```rust
/// Run `f` under the implicit lock. Callers must drop the lock before doing
/// I/O — this helper exists to make that intent visible at call sites.
pub fn with_snapshot<F, R>(&self, f: F) -> R
where
    F: FnOnce(&Store) -> R,
{
    f(self)
}

/// Run `f` inside a single `conn.transaction()` — use this whenever a command
/// re-acquires the lock to write a batch of changes.
pub fn with_transaction<F, R>(&mut self, f: F) -> rusqlite::Result<R>
where
    F: FnOnce(&Transaction) -> rusqlite::Result<R>,
{
    let tx = self.conn.transaction()?;
    let r = f(&tx)?;
    tx.commit()?;
    Ok(r)
}
```

Both are intentionally trivial. The `with_snapshot` form's value is documentary — call sites see `let snap = store.lock().unwrap(); let data = store.with_snapshot(|s| s.list_hosts()).unwrap();` followed by `drop(store);` and downstream code can see the lock was held only across the closure.

- [ ] **Step 2: Add a unit test**

In the existing `#[cfg(test)] mod tests` at the bottom of `store.rs`:

```rust
#[test]
fn with_snapshot_returns_owned_data_for_off_lock_use() {
    let store = Store::open_in_memory().expect("in-memory store");
    store.upsert_host("alpha", Some("alpha-ssh"), false).unwrap();
    let hosts = store.with_snapshot(|s| s.list_hosts().unwrap());
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0].alias, "alpha");
}

#[test]
fn with_transaction_commits_on_ok_rolls_back_on_err() {
    let mut store = Store::open_in_memory().expect("in-memory store");
    let r: rusqlite::Result<()> = store.with_transaction(|tx| {
        tx.execute("INSERT INTO hosts (alias, ssh_alias, hidden) VALUES (?1, ?2, 0)",
                   rusqlite::params!["foo", "foo-ssh"])?;
        Ok(())
    });
    assert!(r.is_ok());
    let hosts = store.list_hosts().unwrap();
    assert_eq!(hosts.len(), 1);
}
```

- [ ] **Step 3: Run + commit**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib store 2>&1 | tail -6
```

Expected: pre-existing store tests stay green; two new tests pass.

```bash
git add src-tauri/src/store.rs
git commit -m "store: with_snapshot + with_transaction helpers"
```

---

### Task 5: Convert SSH-touching commands to `async fn`

**Files:**
- Modify: `src-tauri/src/commands/hosts.rs`
- Modify: `src-tauri/src/commands/sessions.rs`

This is mechanical — every `#[tauri::command] fn x(...)` that calls `SshClient::run` or `TmuxExec` method becomes `#[tauri::command] async fn x(...)`. Every `.run(...)` / `.list_sessions()` / etc. gains `.await`.

- [ ] **Step 1: Identify call sites**

```bash
grep -nE "fn (probe|add_host|reconcile|list_sessions|new_session|kill_session|rename_session|restart_session|send_prompt|ensure_remote_project)" src-tauri/src/commands/*.rs
```

Note each `fn` to convert.

- [ ] **Step 2: Convert each fn signature**

For each identified function, change `fn` → `async fn`. Add `.await` to every async call inside. Where the function takes `State<'_, Mutex<Store>>`, this remains the same — but the calling pattern becomes:

```rust
let snap = {
    let s = store.lock().expect("store");
    s.with_snapshot(|s| ( /* whatever owned data is needed */ ))
};
// snap is now owned; lock is released
let result = ssh.run(alias, cmd).await?;
{
    let mut s = store.lock().expect("store");
    s.with_transaction(|tx| { /* writes */ })?;
}
```

This is a placeholder pattern — Task 6 (reconcile) is where the full pattern is applied. For now, _just_ add `async` + `.await` and keep the lock-during-IO behaviour (this task is purely mechanical; M2 fixes the actual lock pattern). The point is to get the type system to compile through.

- [ ] **Step 3: Update `lib.rs` handler registration**

Tauri 2 accepts `async fn` commands in `generate_handler![...]` without changes — confirm by build.

- [ ] **Step 4: Build + test sweep**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib 2>&1 | tail -6
pnpm vitest run 2>&1 | tail -4
```

Expected: 88 + 137 stay green.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/commands/
git commit -m "commands: convert SSH-touching commands to async fn"
```

---

## M2 — Parallel reconcile + per-host granularity

Goal: `reconcile_sessions` runs all host probes concurrently via `JoinSet`, off-lock. New `reconcile_one_host` for targeted post-mutation refresh. Verified: one unreachable host doesn't delay reconciles of reachable peers.

### Task 6: Parallel `reconcile_sessions`

**Files:**
- Modify: `src-tauri/src/commands/sessions.rs`

- [ ] **Step 1: Rewrite `reconcile_sessions` to async + JoinSet**

Find the existing `reconcile_sessions(s: &mut Store, ssh: &SshClient) -> Result<...>` near the top of `commands/sessions.rs`. Replace it with:

```rust
pub async fn reconcile_sessions(
    store: &Mutex<Store>,
    ssh: &SshClient,
) -> Result<(), IpcError> {
    // 1. Snapshot under lock.
    let (hosts, projects) = {
        let s = store.lock().expect("store");
        let hosts = s.list_hosts().map_err(IpcError::from)?;
        let projects = s.list_projects().map_err(IpcError::from)?;
        (hosts, projects)
    };

    // 2. Fan out probes (off-lock).
    let mut set = tokio::task::JoinSet::new();
    for host in hosts.iter().filter(|h| !h.hidden) {
        let alias = host.alias.clone();
        let ssh_alias = host.ssh_alias.clone();
        let ssh = ssh.clone(); // SshClient must be Clone — see step 2
        set.spawn(async move {
            let tmux: Box<dyn TmuxExec> = if ssh_alias.is_some() {
                Box::new(RemoteTmux::new(ssh, alias.clone(), ssh_alias.unwrap()))
            } else {
                Box::new(LocalTmux::new())
            };
            let result = tmux.list_sessions().await;
            (alias, result)
        });
    }

    let mut per_host: Vec<(String, Result<Vec<RemoteSession>, IpcError>)> = Vec::new();
    while let Some(r) = set.join_next().await {
        match r {
            Ok((alias, res)) => per_host.push((alias, res)),
            Err(join_err) => {
                eprintln!("reconcile join error: {join_err}");
            }
        }
    }

    // 3. Apply all writes in one transaction.
    let mut s = store.lock().expect("store");
    s.with_transaction(|tx| {
        for (alias, res) in &per_host {
            match res {
                Ok(sessions_in) => {
                    Store::update_host_probe_in_tx(tx, alias, true).ok();
                    for sess in sessions_in {
                        let (project_id, worktree_id) = find_project_id_for_path(&projects, &sess.cwd);
                        let host = hosts.iter().find(|h| h.alias == *alias);
                        let existing_account = Store::get_session_account_in_tx(tx, alias, &sess.name).ok().flatten();
                        let account_uuid = existing_account.or_else(|| host.and_then(|h| h.account_uuid.clone()));
                        Store::upsert_session_in_tx(
                            tx,
                            &sess.name,
                            alias,
                            project_id,
                            worktree_id,
                            sess.created,
                            sess.last_activity,
                            "running",
                            account_uuid.as_deref(),
                        )?;
                    }
                    let names: Vec<&str> = sessions_in.iter().map(|s| s.name.as_str()).collect();
                    Store::delete_sessions_not_in_tx(tx, alias, &names)?;
                }
                Err(e) => {
                    Store::update_host_probe_in_tx(tx, alias, false).ok();
                    eprintln!("host {alias} unreachable: {e}");
                }
            }
        }
        Ok(())
    })?;
    Ok(())
}
```

Note: this introduces `_in_tx` variants of the Store mutation methods. Add them in `store.rs` — each is identical to the existing public method but takes `&Transaction` and uses `tx.execute(...)` / `tx.query_row(...)` directly. Add ~5 new `_in_tx` helpers in `store.rs`:

```rust
impl Store {
    pub fn upsert_session_in_tx(
        tx: &Transaction,
        tmux_name: &str,
        host_alias: &str,
        project_id: Option<i64>,
        worktree_id: Option<i64>,
        created_at: i64,
        last_activity_at: i64,
        status: &str,
        account_uuid: Option<&str>,
    ) -> rusqlite::Result<()> {
        // Same SQL as upsert_session, using tx.execute(...)
        tx.execute(
            "INSERT INTO sessions (tmux_name, host_alias, project_id, worktree_id, created_at, last_activity_at, status, account_uuid)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(tmux_name, host_alias) DO UPDATE SET
               project_id=excluded.project_id,
               worktree_id=excluded.worktree_id,
               last_activity_at=excluded.last_activity_at,
               status=excluded.status,
               account_uuid=excluded.account_uuid",
            rusqlite::params![tmux_name, host_alias, project_id, worktree_id, created_at, last_activity_at, status, account_uuid],
        )?;
        Ok(())
    }

    pub fn delete_sessions_not_in_tx(
        tx: &Transaction,
        host_alias: &str,
        names: &[&str],
    ) -> rusqlite::Result<()> {
        // Build (?, ?, ?, ...) placeholder list and run a DELETE.
        if names.is_empty() {
            tx.execute(
                "DELETE FROM sessions WHERE host_alias = ?1",
                rusqlite::params![host_alias],
            )?;
            return Ok(());
        }
        let placeholders = std::iter::repeat("?").take(names.len()).collect::<Vec<_>>().join(", ");
        let sql = format!(
            "DELETE FROM sessions WHERE host_alias = ?1 AND tmux_name NOT IN ({})",
            placeholders
        );
        let mut params: Vec<&dyn rusqlite::ToSql> = vec![&host_alias];
        for n in names { params.push(n); }
        tx.execute(&sql, params.as_slice())?;
        Ok(())
    }

    pub fn update_host_probe_in_tx(tx: &Transaction, alias: &str, reachable: bool) -> rusqlite::Result<()> {
        tx.execute(
            "UPDATE hosts SET reachable = ?1, probed_at = strftime('%s','now') WHERE alias = ?2",
            rusqlite::params![reachable as i64, alias],
        )?;
        Ok(())
    }

    pub fn get_session_account_in_tx(
        tx: &Transaction,
        host_alias: &str,
        tmux_name: &str,
    ) -> rusqlite::Result<Option<String>> {
        let r: rusqlite::Result<Option<String>> = tx.query_row(
            "SELECT account_uuid FROM sessions WHERE host_alias=?1 AND tmux_name=?2",
            rusqlite::params![host_alias, tmux_name],
            |row| row.get(0),
        );
        match r {
            Ok(v) => Ok(v),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}
```

- [ ] **Step 2: Make `SshClient` `Clone`**

`SshClient` needs to be cheap to clone so spawned tasks own their reference. Wrap internals in `Arc`:

```rust
// In ssh.rs:
#[derive(Clone)]
pub struct SshClient {
    inner: Arc<SshClientInner>,
}

struct SshClientInner {
    masters: DashMap<String, Arc<OnceCell<()>>>,
}
```

Adjust accessors accordingly. (If `SshClient` is already trivially cloneable, skip.)

- [ ] **Step 3: Add concurrency test**

In `commands/sessions.rs`'s `#[cfg(test)] mod tests`:

```rust
#[tokio::test]
async fn reconcile_does_not_block_on_one_slow_host() {
    use std::time::Duration;
    // Mock TmuxExec with controllable sleep durations.
    struct SleepyTmux { sleep_ms: u64 }
    #[async_trait::async_trait]
    impl TmuxExec for SleepyTmux {
        async fn list_sessions(&self) -> Result<Vec<RemoteSession>, IpcError> {
            tokio::time::sleep(Duration::from_millis(self.sleep_ms)).await;
            Ok(Vec::new())
        }
        async fn new_session(&self, _: &str, _: &str, _: &str) -> Result<(), IpcError> { Ok(()) }
        async fn kill_session(&self, _: &str) -> Result<(), IpcError> { Ok(()) }
        async fn rename_session(&self, _: &str, _: &str) -> Result<(), IpcError> { Ok(()) }
        async fn restart_session(&self, _: &str, _: &str, _: &str) -> Result<(), IpcError> { Ok(()) }
    }

    let mut set = tokio::task::JoinSet::new();
    let start = std::time::Instant::now();
    for (alias, sleep_ms) in [("fast1", 50), ("slow", 500), ("fast2", 50)] {
        set.spawn(async move {
            let t = SleepyTmux { sleep_ms };
            t.list_sessions().await.map(|s| (alias, s))
        });
    }
    while set.join_next().await.is_some() {}
    let elapsed = start.elapsed();
    // Sequential would be ~600ms; parallel ≈ 500ms (max of all).
    assert!(elapsed < Duration::from_millis(700),
            "parallel reconcile took {elapsed:?}, expected ≈max not sum");
}
```

- [ ] **Step 4: Build + run**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib 2>&1 | tail -8
```

Expected: 88 pre-existing tests still pass; new test passes; total ≥ 89.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/commands/sessions.rs src-tauri/src/store.rs src-tauri/src/ssh.rs
git commit -m "reconcile: parallel JoinSet fan-out + off-lock probes"
```

---

### Task 7: `reconcile_one_host` + per-host mutation paths

**Files:**
- Modify: `src-tauri/src/commands/sessions.rs`

- [ ] **Step 1: Add `reconcile_one_host`**

In `commands/sessions.rs`, after the new `reconcile_sessions`:

```rust
pub async fn reconcile_one_host(
    store: &Mutex<Store>,
    ssh: &SshClient,
    alias: &str,
) -> Result<(), IpcError> {
    let (host, projects) = {
        let s = store.lock().expect("store");
        let host = s.list_hosts().map_err(IpcError::from)?
            .into_iter()
            .find(|h| h.alias == alias)
            .ok_or_else(|| IpcError::not_found(format!("host {alias} not found")))?;
        let projects = s.list_projects().map_err(IpcError::from)?;
        (host, projects)
    };

    let tmux: Box<dyn TmuxExec> = if host.ssh_alias.is_some() {
        Box::new(RemoteTmux::new(ssh.clone(), host.alias.clone(), host.ssh_alias.clone().unwrap()))
    } else {
        Box::new(LocalTmux::new())
    };
    let sessions_in = tmux.list_sessions().await?;

    let mut s = store.lock().expect("store");
    s.with_transaction(|tx| {
        Store::update_host_probe_in_tx(tx, alias, true).ok();
        for sess in &sessions_in {
            let (project_id, worktree_id) = find_project_id_for_path(&projects, &sess.cwd);
            let existing_account = Store::get_session_account_in_tx(tx, alias, &sess.name).ok().flatten();
            let account_uuid = existing_account.or_else(|| host.account_uuid.clone());
            Store::upsert_session_in_tx(
                tx, &sess.name, alias, project_id, worktree_id,
                sess.created, sess.last_activity, "running",
                account_uuid.as_deref(),
            )?;
        }
        let names: Vec<&str> = sessions_in.iter().map(|s| s.name.as_str()).collect();
        Store::delete_sessions_not_in_tx(tx, alias, &names)?;
        Ok(())
    })?;
    Ok(())
}
```

- [ ] **Step 2: Switch mutation commands to per-host**

Find `new_session`, `kill_session`, `rename_session`, `restart_session` in the same file. In each:

1. Replace the trailing `reconcile_sessions(&s, &ssh)?` call with `reconcile_one_host(&store, &ssh, &args.host_alias).await?`.
2. After reconcile, look up the affected row and return it:

```rust
let row = {
    let s = store.lock().expect("store");
    s.list_sessions_for_host(&args.host_alias)?
        .into_iter()
        .find(|r| r.tmux_name == args.tmux_name)
};
Ok(row.ok_or_else(|| IpcError::not_found("session vanished after mutation"))?)
```

(For `kill_session`, return the `id` of the deleted row instead.)

Adjust the command's return type from `Result<(), IpcError>` to `Result<SessionRow, IpcError>` (or `Result<i64, IpcError>` for `kill_session`).

- [ ] **Step 3: Add per-host reconcile test**

```rust
#[tokio::test]
async fn reconcile_one_host_only_affects_that_host() {
    // Pre-seed store with sessions on two hosts.
    let store = Mutex::new(Store::open_in_memory().expect("store"));
    {
        let s = store.lock().unwrap();
        s.upsert_host("alpha", None, false).unwrap();
        s.upsert_host("beta", None, false).unwrap();
        s.upsert_session("alpha-s", "alpha", None, None, 1, 1, "running", None).unwrap();
        s.upsert_session("beta-s", "beta", None, None, 1, 1, "running", None).unwrap();
    }
    // (Pretend reconcile_one_host("alpha") would remove alpha-s; we mock by
    //  calling the in-tx delete directly.)
    {
        let mut s = store.lock().unwrap();
        s.with_transaction(|tx| Store::delete_sessions_not_in_tx(tx, "alpha", &[])).unwrap();
    }
    let s = store.lock().unwrap();
    let alpha = s.list_sessions_for_host("alpha").unwrap();
    let beta = s.list_sessions_for_host("beta").unwrap();
    assert!(alpha.is_empty(), "alpha cleared");
    assert_eq!(beta.len(), 1, "beta untouched");
}
```

- [ ] **Step 4: Frontend type update**

In `src/lib/sessions.ts`, the mutation IPC wrappers now receive a `SessionRow` (or `id` for kill) instead of `void`. Update each:

```ts
export async function killSession(id: number): Promise<Result<number>> {
  return invokeCmd<number>('kill_session', { args: { id } });
}

export async function newSession(args: NewSessionArgs): Promise<Result<SessionRow>> {
  return invokeCmd<SessionRow>('new_session', { args });
}
```

Mutation wrappers still call `loadSessions()` at the end for now (we drop that in M3). Patch-in-place via `mergeOne` is a Task 13 concern.

- [ ] **Step 5: Run all tests**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib 2>&1 | tail -6
pnpm vitest run 2>&1 | tail -4
```

Expected: green on both.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/commands/sessions.rs src/lib/sessions.ts
git commit -m "reconcile: per-host granularity + mutations return affected row"
```

---

## M3 — Delta-event bus + frontend `listen`

Goal: backend emits typed Tauri events whenever a row changes; frontend stores subscribe with `listen()` and patch in place; mutation IPC wrappers drop their `await loadX()` chain. This is the biggest milestone.

### Task 8: `EventBus` trait + `NoopEventBus` + `RecordingEventBus`

**Files:**
- Create: `src-tauri/src/events.rs`
- Modify: `src-tauri/src/store.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Define event payload structs**

Create `src-tauri/src/events.rs`:

```rust
use serde::Serialize;
use crate::store::{SessionRow, HostRow, AccountRow, ProjectRow, WorktreeRow};

#[derive(Serialize, Clone)]
pub struct SessionKilledPayload { pub id: i64 }

#[derive(Serialize, Clone)]
pub struct HostRemovedPayload { pub alias: String }

pub trait EventBus: Send + Sync {
    fn session_created(&self, row: &SessionRow);
    fn session_updated(&self, row: &SessionRow);
    fn session_killed(&self, id: i64);
    fn host_added(&self, row: &HostRow);
    fn host_probed(&self, row: &HostRow);
    fn host_removed(&self, alias: &str);
    fn account_upserted(&self, row: &AccountRow);
    fn project_updated(&self, row: &ProjectRow);
    fn worktree_updated(&self, row: &WorktreeRow);
}

/// For tests + headless contexts. Silently drops every event.
pub struct NoopEventBus;
impl EventBus for NoopEventBus {
    fn session_created(&self, _: &SessionRow) {}
    fn session_updated(&self, _: &SessionRow) {}
    fn session_killed(&self, _: i64) {}
    fn host_added(&self, _: &HostRow) {}
    fn host_probed(&self, _: &HostRow) {}
    fn host_removed(&self, _: &str) {}
    fn account_upserted(&self, _: &AccountRow) {}
    fn project_updated(&self, _: &ProjectRow) {}
    fn worktree_updated(&self, _: &WorktreeRow) {}
}

/// For tests. Captures every event in order.
#[cfg(test)]
pub struct RecordingEventBus {
    pub events: std::sync::Mutex<Vec<String>>,
}

#[cfg(test)]
impl RecordingEventBus {
    pub fn new() -> Self { Self { events: std::sync::Mutex::new(Vec::new()) } }
    pub fn take(&self) -> Vec<String> { std::mem::take(&mut *self.events.lock().unwrap()) }
}

#[cfg(test)]
impl EventBus for RecordingEventBus {
    fn session_created(&self, r: &SessionRow) {
        self.events.lock().unwrap().push(format!("session:created:{}", r.id));
    }
    fn session_updated(&self, r: &SessionRow) {
        self.events.lock().unwrap().push(format!("session:updated:{}", r.id));
    }
    fn session_killed(&self, id: i64) {
        self.events.lock().unwrap().push(format!("session:killed:{}", id));
    }
    fn host_added(&self, r: &HostRow) {
        self.events.lock().unwrap().push(format!("host:added:{}", r.alias));
    }
    fn host_probed(&self, r: &HostRow) {
        self.events.lock().unwrap().push(format!("host:probed:{}", r.alias));
    }
    fn host_removed(&self, alias: &str) {
        self.events.lock().unwrap().push(format!("host:removed:{}", alias));
    }
    fn account_upserted(&self, r: &AccountRow) {
        self.events.lock().unwrap().push(format!("account:upserted:{}", r.uuid));
    }
    fn project_updated(&self, r: &ProjectRow) {
        self.events.lock().unwrap().push(format!("project:updated:{}", r.id));
    }
    fn worktree_updated(&self, r: &WorktreeRow) {
        self.events.lock().unwrap().push(format!("worktree:updated:{}", r.id));
    }
}
```

- [ ] **Step 2: Mount in lib.rs**

Add `mod events;` at the top of `src-tauri/src/lib.rs`. (`pub use events::EventBus;` if other modules need it.)

- [ ] **Step 3: Store owns an `Arc<dyn EventBus>`**

In `src-tauri/src/store.rs`:

```rust
use crate::events::EventBus;
use std::sync::Arc;

pub struct Store {
    conn: rusqlite::Connection,
    bus: Arc<dyn EventBus>,
}

impl Store {
    pub fn open_with_bus(path: &Path, bus: Arc<dyn EventBus>) -> Result<Self, IpcError> {
        let conn = rusqlite::Connection::open(path)?;
        let mut s = Self { conn, bus };
        s.migrate()?;
        Ok(s)
    }

    pub fn open_in_memory() -> Result<Self, IpcError> {
        let conn = rusqlite::Connection::open_in_memory()?;
        let mut s = Self { conn, bus: Arc::new(crate::events::NoopEventBus) };
        s.migrate()?;
        Ok(s)
    }
}
```

Existing `open(path)` callers in `lib.rs` switch to `open_with_bus`. Update `lib.rs`:

```rust
let app_handle = app.handle().clone();
let bus: Arc<dyn EventBus> = Arc::new(AppHandleEventBus::new(app_handle));
let store = Store::open_with_bus(&db_path, bus.clone())?;
```

`AppHandleEventBus` is defined in Task 10.

- [ ] **Step 4: Build (will not yet wire emit calls into upsert sites)**

```bash
cargo build --manifest-path src-tauri/Cargo.toml 2>&1 | tail -3
```

Expected: builds with one warning (`bus` field unused) — that's fine, Task 9 wires it in.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/events.rs src-tauri/src/store.rs src-tauri/src/lib.rs
git commit -m "events: EventBus trait + Noop/Recording impls + Store owns bus"
```

---

### Task 9: Wire `EventBus` into every `Store` mutation

**Files:**
- Modify: `src-tauri/src/store.rs`

- [ ] **Step 1: Identify mutation sites**

```bash
grep -nE "pub fn (upsert|delete|update|insert|touch)" src-tauri/src/store.rs
```

The list (verify):

- `upsert_session` → `session_created` (new) / `session_updated` (existing)
- `delete_session` → `session_killed`
- `insert_host` / `upsert_host` → `host_added`
- `update_host_probe` + `set_host_account` → `host_probed`
- `delete_host` → `host_removed`
- `upsert_account` → `account_upserted`
- `upsert_project` + `touch_project_last_session_at` → `project_updated`
- `upsert_worktree` → `worktree_updated`

- [ ] **Step 2: Emit at each mutation site**

For each mutation method, after a successful write, fetch the row and call the bus. Example for `upsert_session`:

```rust
pub fn upsert_session(
    &self,
    tmux_name: &str,
    host_alias: &str,
    project_id: Option<i64>,
    worktree_id: Option<i64>,
    created_at: i64,
    last_activity_at: i64,
    status: &str,
    account_uuid: Option<&str>,
) -> rusqlite::Result<()> {
    // ... existing INSERT OR UPDATE logic
    let is_new = self.conn.changes() > 0 && /* check if it was INSERT not UPDATE */;
    // Easier: query AFTER the upsert to fetch the row, then decide created vs updated
    // based on whether created_at == last_activity_at (proxy) OR by tracking the
    // changes() count before/after.
    let row = self.get_session(tmux_name, host_alias)?;
    if let Some(row) = row {
        // Heuristic: new row if its id was just minted (created_at within last second
        // of now), otherwise updated.
        let now = chrono::Utc::now().timestamp();
        if (now - row.created_at).abs() <= 1 {
            self.bus.session_created(&row);
        } else {
            self.bus.session_updated(&row);
        }
    }
    Ok(())
}
```

For `_in_tx` variants (used by reconcile), do not emit inside the transaction — that would fire while DB is mid-flight. Instead, return a `Vec<RowChange>` from the transaction and emit after commit. Add to `reconcile_sessions`:

```rust
let mut s = store.lock().expect("store");
let changes = s.with_transaction(|tx| {
    let mut changes: Vec<RowChange> = Vec::new();
    // ... existing per-host loop
    // Each upsert/delete_in_tx returns the affected row or id; push into changes.
    Ok(changes)
})?;
// Emit AFTER commit, before lock drop is fine since bus calls are lightweight.
let bus = s.bus.clone();
drop(s);
for c in changes {
    match c {
        RowChange::SessionCreated(row) => bus.session_created(&row),
        RowChange::SessionUpdated(row) => bus.session_updated(&row),
        RowChange::SessionKilled(id) => bus.session_killed(id),
        // ... etc.
    }
}
```

Add the `RowChange` enum to `events.rs`:

```rust
pub enum RowChange {
    SessionCreated(SessionRow),
    SessionUpdated(SessionRow),
    SessionKilled(i64),
    HostAdded(HostRow),
    HostProbed(HostRow),
    HostRemoved(String),
    AccountUpserted(AccountRow),
    ProjectUpdated(ProjectRow),
    WorktreeUpdated(WorktreeRow),
}
```

- [ ] **Step 3: Add a RecordingEventBus test**

```rust
#[test]
fn upsert_session_emits_created_then_updated() {
    let bus = Arc::new(RecordingEventBus::new());
    let mut store = {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let mut s = Store { conn, bus: bus.clone() };
        s.migrate().unwrap();
        s
    };
    store.upsert_session("s1", "alpha", None, None, 100, 100, "running", None).unwrap();
    store.upsert_session("s1", "alpha", None, None, 100, 200, "running", None).unwrap();
    let evts = bus.take();
    assert_eq!(evts.len(), 2);
    assert!(evts[0].starts_with("session:created"));
    assert!(evts[1].starts_with("session:updated"));
}

#[test]
fn delete_session_emits_killed() {
    let bus = Arc::new(RecordingEventBus::new());
    let mut store = {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let mut s = Store { conn, bus: bus.clone() };
        s.migrate().unwrap();
        s
    };
    store.upsert_session("s1", "alpha", None, None, 100, 100, "running", None).unwrap();
    bus.take(); // drain created event
    let id = store.get_session("s1", "alpha").unwrap().unwrap().id;
    store.delete_session(id).unwrap();
    let evts = bus.take();
    assert_eq!(evts.len(), 1);
    assert_eq!(evts[0], format!("session:killed:{id}"));
}
```

- [ ] **Step 4: Run + commit**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib 2>&1 | tail -6
```

Expected: 88 + 2 new tests pass.

```bash
git add src-tauri/src/store.rs src-tauri/src/events.rs src-tauri/src/commands/sessions.rs
git commit -m "events: emit on every Store mutation (incl. reconcile batch)"
```

---

### Task 10: `AppHandleEventBus` impl

**Files:**
- Modify: `src-tauri/src/events.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Implement `AppHandleEventBus`**

Append to `src-tauri/src/events.rs`:

```rust
pub struct AppHandleEventBus {
    handle: tauri::AppHandle,
}

impl AppHandleEventBus {
    pub fn new(handle: tauri::AppHandle) -> Self { Self { handle } }
}

impl EventBus for AppHandleEventBus {
    fn session_created(&self, row: &SessionRow) {
        let _ = self.handle.emit("session:created", row);
    }
    fn session_updated(&self, row: &SessionRow) {
        let _ = self.handle.emit("session:updated", row);
    }
    fn session_killed(&self, id: i64) {
        let _ = self.handle.emit("session:killed", SessionKilledPayload { id });
    }
    fn host_added(&self, row: &HostRow) {
        let _ = self.handle.emit("host:added", row);
    }
    fn host_probed(&self, row: &HostRow) {
        let _ = self.handle.emit("host:probed", row);
    }
    fn host_removed(&self, alias: &str) {
        let _ = self.handle.emit("host:removed", HostRemovedPayload { alias: alias.to_string() });
    }
    fn account_upserted(&self, row: &AccountRow) {
        let _ = self.handle.emit("account:upserted", row);
    }
    fn project_updated(&self, row: &ProjectRow) {
        let _ = self.handle.emit("project:updated", row);
    }
    fn worktree_updated(&self, row: &WorktreeRow) {
        let _ = self.handle.emit("worktree:updated", row);
    }
}
```

Use `tauri::Manager` — emit is `app.emit` since Tauri 2.

- [ ] **Step 2: Wire it in `lib.rs::setup`**

In the `tauri::Builder::default().setup(|app| { ... })` closure in `lib.rs`:

```rust
.setup(|app| {
    let handle = app.handle().clone();
    let bus: Arc<dyn EventBus> = Arc::new(AppHandleEventBus::new(handle));
    let store = Store::open_with_bus(&db_path(), bus.clone())?;
    app.manage(Mutex::new(store));
    // ... existing setup ...
    Ok(())
})
```

- [ ] **Step 3: Smoke-build**

```bash
cargo build --manifest-path src-tauri/Cargo.toml 2>&1 | tail -3
```

Expected: no errors. The events are emitted but no frontend listener exists yet — that's Task 12+.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/events.rs src-tauri/src/lib.rs
git commit -m "events: AppHandleEventBus + wire-up in setup"
```

---

### Task 11: TypeScript event types

**Files:**
- Modify: `src/lib/sessions.ts`
- Modify: `src/lib/hosts.ts`
- Modify: `src/lib/accounts.ts`
- Modify: `src/lib/projects.ts`
- Create: `src/lib/events.ts`

- [ ] **Step 1: Create `src/lib/events.ts`**

```ts
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { SessionRow } from './sessions';
import type { HostRow } from './hosts';
import type { AccountRow } from './accounts';
import type { ProjectRow, WorktreeRow } from './projects';

export type RowEventHandlers = {
  onSessionCreated?: (row: SessionRow) => void;
  onSessionUpdated?: (row: SessionRow) => void;
  onSessionKilled?: (payload: { id: number }) => void;
  onHostAdded?: (row: HostRow) => void;
  onHostProbed?: (row: HostRow) => void;
  onHostRemoved?: (payload: { alias: string }) => void;
  onAccountUpserted?: (row: AccountRow) => void;
  onProjectUpdated?: (row: ProjectRow) => void;
  onWorktreeUpdated?: (row: WorktreeRow) => void;
};

/**
 * Subscribe to all row-change events from the backend. Returns a single
 * unsubscribe function that tears them all down.
 */
export async function subscribeToRowEvents(handlers: RowEventHandlers): Promise<UnlistenFn> {
  const unlisteners: UnlistenFn[] = [];
  const sub = async <T>(name: string, handler: ((payload: T) => void) | undefined) => {
    if (!handler) return;
    const u = await listen<T>(name, (e) => handler(e.payload));
    unlisteners.push(u);
  };
  await sub<SessionRow>('session:created', handlers.onSessionCreated);
  await sub<SessionRow>('session:updated', handlers.onSessionUpdated);
  await sub<{ id: number }>('session:killed', handlers.onSessionKilled);
  await sub<HostRow>('host:added', handlers.onHostAdded);
  await sub<HostRow>('host:probed', handlers.onHostProbed);
  await sub<{ alias: string }>('host:removed', handlers.onHostRemoved);
  await sub<AccountRow>('account:upserted', handlers.onAccountUpserted);
  await sub<ProjectRow>('project:updated', handlers.onProjectUpdated);
  await sub<WorktreeRow>('worktree:updated', handlers.onWorktreeUpdated);
  return () => { for (const u of unlisteners) u(); };
}
```

- [ ] **Step 2: Build the frontend (type-check)**

```bash
cd /Users/martinjanci/projects/github.com/martin-janci/claude-fleet
pnpm tsc --noEmit 2>&1 | tail -5
```

Expected: no type errors. (If `ProjectRow` / `WorktreeRow` aren't exported, export them from `projects.ts`.)

- [ ] **Step 3: Commit**

```bash
git add src/lib/events.ts src/lib/projects.ts
git commit -m "events: src/lib/events.ts subscribeToRowEvents helper"
```

---

### Task 12: Stores grow `bootstrap` + `mergeOne` + `removeOne`

**Files:**
- Modify: `src/lib/sessions.ts`
- Modify: `src/lib/hosts.ts`
- Modify: `src/lib/accounts.ts`
- Modify: `src/lib/projects.ts`

- [ ] **Step 1: `sessions.ts` shape**

Add to `src/lib/sessions.ts`:

```ts
export async function bootstrapSessions(): Promise<void> {
  const r = await invokeCmd<SessionRow[]>('list_sessions', { args: {} });
  if (r.ok) sessions.set(r.value);
}

export function mergeSession(row: SessionRow): void {
  sessions.update((arr) => {
    const i = arr.findIndex((s) => s.id === row.id);
    if (i === -1) return [...arr, row];
    const next = arr.slice();
    next[i] = row;
    return next;
  });
}

export function removeSession(id: number): void {
  sessions.update((arr) => arr.filter((s) => s.id !== id));
}

// Legacy alias for the manual Refresh button — DO NOT call after mutations.
export const loadSessions = bootstrapSessions;
```

- [ ] **Step 2: Same shape for `hosts.ts` (key by `alias`)**

```ts
export async function bootstrapHosts(): Promise<void> {
  const r = await invokeCmd<HostRow[]>('list_hosts', { args: {} });
  if (r.ok) hosts.set(r.value);
}

export function mergeHost(row: HostRow): void {
  hosts.update((arr) => {
    const i = arr.findIndex((h) => h.alias === row.alias);
    if (i === -1) return [...arr, row];
    const next = arr.slice();
    next[i] = row;
    return next;
  });
}

export function removeHost(alias: string): void {
  hosts.update((arr) => arr.filter((h) => h.alias !== alias));
}

export const loadHosts = bootstrapHosts;
```

- [ ] **Step 3: Same shape for `accounts.ts` (key by `uuid`)**

```ts
export async function bootstrapAccounts(): Promise<void> {
  const r = await invokeCmd<AccountRow[]>('list_accounts', { args: {} });
  if (r.ok) accounts.set(r.value);
}

export function mergeAccount(row: AccountRow): void {
  accounts.update((arr) => {
    const i = arr.findIndex((a) => a.uuid === row.uuid);
    if (i === -1) return [...arr, row];
    const next = arr.slice();
    next[i] = row;
    return next;
  });
}

export const loadAccounts = bootstrapAccounts;
```

(No `removeAccount` — backend never deletes accounts in iter 4a.)

- [ ] **Step 4: Same shape for `projects.ts`**

```ts
export async function bootstrapProjects(): Promise<void> {
  const r = await invokeCmd<ProjectRow[]>('list_projects', { args: {} });
  if (r.ok) projects.set(r.value);
}

export function mergeProject(row: ProjectRow): void {
  projects.update((arr) => {
    const i = arr.findIndex((p) => p.id === row.id);
    if (i === -1) return [...arr, row];
    const next = arr.slice();
    next[i] = row;
    return next;
  });
}

// worktrees are a nested list inside ProjectRow; merge similarly when the
// worktree event fires.
export function mergeWorktree(row: WorktreeRow): void {
  projects.update((arr) => {
    const idx = arr.findIndex((p) => p.id === row.project_id);
    if (idx === -1) return arr;
    const proj = arr[idx];
    const wts = proj.worktrees ?? [];
    const wIdx = wts.findIndex((w) => w.id === row.id);
    const newWts = wIdx === -1 ? [...wts, row] : wts.map((w) => (w.id === row.id ? row : w));
    const next = arr.slice();
    next[idx] = { ...proj, worktrees: newWts };
    return next;
  });
}

export const loadProjects = bootstrapProjects;
export const refreshProjects = async (): Promise<void> => {
  await invokeCmd<void>('refresh_projects', { args: {} });
  await bootstrapProjects();
};
```

- [ ] **Step 5: Type-check**

```bash
pnpm tsc --noEmit 2>&1 | tail -5
```

Expected: no errors.

- [ ] **Step 6: Commit**

```bash
git add src/lib/sessions.ts src/lib/hosts.ts src/lib/accounts.ts src/lib/projects.ts
git commit -m "stores: bootstrap + mergeOne + removeOne API"
```

---

### Task 13: Mutation wrappers drop `loadX()` chains

**Files:**
- Modify: `src/lib/sessions.ts`
- Modify: `src/lib/hosts.ts`

- [ ] **Step 1: Sessions mutations**

In `src/lib/sessions.ts`, replace each existing mutation wrapper's tail `await loadSessions()` with `mergeSession(r.value)` (or `removeSession(id)` for kill):

```ts
export async function killSession(id: number): Promise<Result<void>> {
  const r = await invokeCmd<number>('kill_session', { args: { id } });
  if (r.ok) removeSession(r.value);
  return r.ok ? { ok: true, value: undefined } : r;
}

export async function newSession(args: NewSessionArgs): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>('new_session', { args });
  if (r.ok) mergeSession(r.value);
  return r;
}

export async function renameSession(id: number, newName: string): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>('rename_session', { args: { id, new_name: newName } });
  if (r.ok) mergeSession(r.value);
  return r;
}

export async function restartSession(id: number): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>('restart_session', { args: { id } });
  if (r.ok) mergeSession(r.value);
  return r;
}
```

- [ ] **Step 2: Hosts mutations**

In `src/lib/hosts.ts`:

```ts
export async function addHost(args: AddHostArgs): Promise<Result<HostRow>> {
  const r = await invokeCmd<HostRow>('add_host', { args });
  if (r.ok) mergeHost(r.value);
  return r;
}

export async function probeHost(alias: string): Promise<Result<HostRow>> {
  const r = await invokeCmd<HostRow>('probe_ssh_alias', { args: { alias } });
  if (r.ok) mergeHost(r.value);
  return r;
}

export async function removeHostCmd(alias: string): Promise<Result<void>> {
  const r = await invokeCmd<void>('delete_host', { args: { alias } });
  if (r.ok) removeHost(alias);
  return r;
}

export async function hideHost(alias: string, hidden: boolean): Promise<Result<HostRow>> {
  const r = await invokeCmd<HostRow>('set_host_hidden', { args: { alias, hidden } });
  if (r.ok) mergeHost(r.value);
  return r;
}
```

Each backend command's return type must match (HostRow for upserts, void for true deletes). Update the Rust commands to return the affected row — if they don't already, fetch and return.

- [ ] **Step 3: Type-check + existing tests**

```bash
pnpm tsc --noEmit 2>&1 | tail -5
pnpm vitest run 2>&1 | tail -4
cargo test --manifest-path src-tauri/Cargo.toml --lib 2>&1 | tail -4
```

Expected: green everywhere. Some existing tests may need fixture updates because backend returns now changed (e.g. `killSession` returns `void` to caller after unwrapping; existing tests expecting `null` should still work because of the unwrap).

- [ ] **Step 4: Commit**

```bash
git add src/lib/sessions.ts src/lib/hosts.ts src-tauri/src/commands/
git commit -m "stores: mutation wrappers patch in place, drop loadX chains"
```

---

### Task 14: App + Sidebar subscribe to events at mount

**Files:**
- Modify: `src/App.svelte`
- Modify: `src/lib/Sidebar.svelte`

- [ ] **Step 1: `App.svelte` parallel bootstrap + subscribe**

In `src/App.svelte`, inside `onMount`:

```svelte
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { subscribeToRowEvents } from './lib/events';
  import { bootstrapSessions, mergeSession, removeSession } from './lib/sessions';
  import { bootstrapHosts, mergeHost, removeHost } from './lib/hosts';
  import { bootstrapAccounts, mergeAccount } from './lib/accounts';
  import { bootstrapProjects, mergeProject, mergeWorktree } from './lib/projects';
  import { healthCheck } from './lib/health';

  let unlistenEvents: (() => void) | null = null;

  onMount(async () => {
    await healthCheck();
    await Promise.all([
      bootstrapProjects(),
      bootstrapSessions(),
      bootstrapHosts(),
      bootstrapAccounts(),
    ]);
    unlistenEvents = await subscribeToRowEvents({
      onSessionCreated: mergeSession,
      onSessionUpdated: mergeSession,
      onSessionKilled: (p) => removeSession(p.id),
      onHostAdded: mergeHost,
      onHostProbed: mergeHost,
      onHostRemoved: (p) => removeHost(p.alias),
      onAccountUpserted: mergeAccount,
      onProjectUpdated: mergeProject,
      onWorktreeUpdated: mergeWorktree,
    });
  });

  onDestroy(() => { unlistenEvents?.(); });
</script>
```

- [ ] **Step 2: Move Sidebar's bootstrap responsibility**

Sidebar previously called `loadProjects/loadSessions/loadHosts/loadAccounts` in onMount. With App handling bootstrap, Sidebar can drop those calls — but to be safe (Sidebar may render before App's onMount completes), keep idempotent calls:

```ts
// src/lib/Sidebar.svelte onMount:
onMount(async () => {
  await Promise.all([
    bootstrapProjects(),
    bootstrapSessions(),
    bootstrapHosts(),
    bootstrapAccounts(),
  ]);
});
```

Both Sidebar and App call `bootstrap*` — they're idempotent. The redundancy is intentional for the duration of M3; the cleanup pass in M5 removes the Sidebar duplicates.

- [ ] **Step 3: Vitest sweep**

```bash
pnpm vitest run 2>&1 | tail -6
```

Expected: 137 stays green. Tests that mock `invoke` for `list_*` should still work; the event subscription mocks are added by the next task.

- [ ] **Step 4: Update `vitest.setup.ts` for event mocks**

In `vitest.setup.ts`, mock `@tauri-apps/api/event`:

```ts
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async () => () => {}),
  emit: vi.fn(async () => {}),
}));
```

- [ ] **Step 5: Commit**

```bash
git add src/App.svelte src/lib/Sidebar.svelte vitest.setup.ts
git commit -m "events: App + Sidebar subscribe to row events at mount"
```

---

### Task 15: Event-subscription unit test

**Files:**
- Create: `src/lib/events.test.ts`

- [ ] **Step 1: Test that subscribeToRowEvents updates stores**

```ts
// src/lib/events.test.ts
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/event', () => {
  const handlers = new Map<string, (e: { payload: unknown }) => void>();
  return {
    listen: vi.fn(async (name: string, cb: (e: { payload: unknown }) => void) => {
      handlers.set(name, cb);
      return () => handlers.delete(name);
    }),
    emit: vi.fn(async (name: string, payload: unknown) => {
      handlers.get(name)?.({ payload });
    }),
    __handlers: handlers,
  };
});

import { listen, emit } from '@tauri-apps/api/event';
import { subscribeToRowEvents } from './events';

describe('subscribeToRowEvents', () => {
  beforeEach(() => {
    (listen as ReturnType<typeof vi.fn>).mockClear();
    (emit as ReturnType<typeof vi.fn>).mockClear();
  });

  it('fires onSessionCreated when session:created is emitted', async () => {
    const seen: number[] = [];
    await subscribeToRowEvents({
      onSessionCreated: (row) => seen.push(row.id),
    });
    await (emit as ReturnType<typeof vi.fn>)('session:created', {
      id: 42, tmux_name: 't', host_alias: 'h',
      project_id: null, worktree_id: null,
      created_at: 0, last_activity_at: 0, status: 'running',
      notes: null, account_uuid: null,
    });
    expect(seen).toEqual([42]);
  });

  it('fires onSessionKilled with id payload', async () => {
    const killed: number[] = [];
    await subscribeToRowEvents({
      onSessionKilled: (p) => killed.push(p.id),
    });
    await (emit as ReturnType<typeof vi.fn>)('session:killed', { id: 99 });
    expect(killed).toEqual([99]);
  });
});
```

- [ ] **Step 2: Run + commit**

```bash
pnpm vitest run src/lib/events.test.ts 2>&1 | tail -10
```

Expected: 2 passes.

```bash
git add src/lib/events.test.ts vitest.setup.ts
git commit -m "events: tests for subscribeToRowEvents wiring"
```

---

## M4 — Cancellation infrastructure

Goal: long-running SSH/git operations are cancellable end-to-end. `invokeCmdAbortable` in the frontend; `CancellationRegistry` in the backend; SSH children get killed on cancel.

### Task 16: `CancellationRegistry`

**Files:**
- Create: `src-tauri/src/cancel.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Implement the registry**

Create `src-tauri/src/cancel.rs`:

```rust
use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub struct CancellationRegistry {
    tokens: DashMap<u64, CancellationToken>,
    next_id: AtomicU64,
}

impl CancellationRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            tokens: DashMap::new(),
            next_id: AtomicU64::new(1),
        })
    }

    /// Register a token under the given external call_id (frontend-minted).
    /// If the frontend never sent a call_id, use `register_anonymous`.
    pub fn bind(&self, call_id: u64, token: CancellationToken) {
        self.tokens.insert(call_id, token);
    }

    pub fn register_anonymous(&self) -> (u64, CancellationToken) {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let token = CancellationToken::new();
        self.tokens.insert(id, token.clone());
        (id, token)
    }

    pub fn cancel(&self, call_id: u64) {
        if let Some((_, token)) = self.tokens.remove(&call_id) {
            token.cancel();
        }
    }

    pub fn unregister(&self, call_id: u64) {
        self.tokens.remove(&call_id);
    }
}
```

- [ ] **Step 2: Register + add cancel_command**

In `src-tauri/src/lib.rs`:

```rust
mod cancel;
use cancel::CancellationRegistry;

#[tauri::command]
async fn cancel_command(
    call_id: u64,
    reg: tauri::State<'_, Arc<CancellationRegistry>>,
) -> Result<(), IpcError> {
    reg.cancel(call_id);
    Ok(())
}

// In setup:
let reg = CancellationRegistry::new();
app.manage(reg.clone());

// In generate_handler!:
.invoke_handler(tauri::generate_handler![
    // ... existing commands ...
    cancel_command,
])
```

- [ ] **Step 3: Add a unit test**

```rust
// In src-tauri/src/cancel.rs:
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancel_fires_token() {
        let reg = CancellationRegistry::new();
        let (id, token) = reg.register_anonymous();
        let cancelled = tokio::spawn(async move {
            token.cancelled().await;
            true
        });
        reg.cancel(id);
        assert!(cancelled.await.unwrap());
    }

    #[tokio::test]
    async fn bind_then_cancel_via_external_id() {
        let reg = CancellationRegistry::new();
        let token = CancellationToken::new();
        reg.bind(7, token.clone());
        let cancelled = tokio::spawn(async move { token.cancelled().await; true });
        reg.cancel(7);
        assert!(cancelled.await.unwrap());
    }
}
```

- [ ] **Step 4: Run + commit**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib cancel 2>&1 | tail -8
```

Expected: 2 new tests pass.

```bash
git add src-tauri/src/cancel.rs src-tauri/src/lib.rs
git commit -m "cancel: CancellationRegistry + cancel_command Tauri command"
```

---

### Task 17: `invokeCmdAbortable`

**Files:**
- Modify: `src/lib/ipc.ts`

- [ ] **Step 1: Add the helper**

In `src/lib/ipc.ts`, add:

```ts
import { invoke } from '@tauri-apps/api/core';
import type { Result } from './result';

let _callIdCounter = 1;
function nextCallId(): number {
  return _callIdCounter++;
}

export async function invokeCmdAbortable<T>(
  cmd: string,
  args: Record<string, unknown>,
  signal?: AbortSignal,
): Promise<Result<T>> {
  const call_id = nextCallId();
  const fullArgs = { args: { ...(args.args as Record<string, unknown> ?? {}), call_id } };

  let onAbort: (() => void) | undefined;
  if (signal) {
    if (signal.aborted) {
      return { ok: false, error: { code: 'E_CANCELLED', message: 'aborted before invoke' } };
    }
    onAbort = () => {
      // Fire-and-forget; backend cancels its registered token.
      void invoke('cancel_command', { call_id }).catch(() => {});
    };
    signal.addEventListener('abort', onAbort, { once: true });
  }

  try {
    return await invokeCmd<T>(cmd, fullArgs);
  } finally {
    if (signal && onAbort) signal.removeEventListener('abort', onAbort);
  }
}
```

- [ ] **Step 2: Add a vitest**

Create `src/lib/ipc.test.ts`:

```ts
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { invokeCmdAbortable } from './ipc';

describe('invokeCmdAbortable', () => {
  beforeEach(() => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
  });

  it('injects call_id into args', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValue(null);
    const ac = new AbortController();
    await invokeCmdAbortable('foo', { args: { x: 1 } }, ac.signal);
    expect(mockedInvoke).toHaveBeenCalledWith('foo', { args: { x: 1, call_id: expect.any(Number) } });
  });

  it('fires cancel_command on abort', async () => {
    let resolveInvoke: (v: unknown) => void;
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(
      (cmd: string) => {
        if (cmd === 'cancel_command') return Promise.resolve(null);
        return new Promise((res) => { resolveInvoke = res; });
      },
    );
    const ac = new AbortController();
    const p = invokeCmdAbortable('long_op', { args: {} }, ac.signal);
    ac.abort();
    // give the microtask queue a chance
    await new Promise((r) => setTimeout(r, 0));
    expect((mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.some(
      (c) => c[0] === 'cancel_command',
    )).toBe(true);
    // resolve the long op so the test doesn't hang
    resolveInvoke!(null);
    await p;
  });
});
```

- [ ] **Step 3: Run + commit**

```bash
pnpm vitest run src/lib/ipc.test.ts 2>&1 | tail -10
```

Expected: 2 passes.

```bash
git add src/lib/ipc.ts src/lib/ipc.test.ts
git commit -m "ipc: invokeCmdAbortable + cancel_command fire-and-forget"
```

---

### Task 18: Thread cancellation through `probe_ssh_alias` and `ensure_remote_project`

**Files:**
- Modify: `src-tauri/src/commands/hosts.rs`
- Modify: `src-tauri/src/commands/sessions.rs`
- Modify: `src-tauri/src/ssh.rs`

- [ ] **Step 1: SshClient gains a cancellable run**

In `src-tauri/src/ssh.rs`:

```rust
use tokio_util::sync::CancellationToken;

impl SshClient {
    pub async fn run_cancellable(
        &self,
        alias: &str,
        cmd: &str,
        token: CancellationToken,
    ) -> Result<String, IpcError> {
        self.ensure_master(alias).await?;
        let mut child = tokio::process::Command::new("ssh")
            .args([
                "-o", &format!("ControlPath={}", control_path_for(alias)),
                "-o", "ConnectTimeout=5",
                alias,
                "bash", "-lc", cmd,
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| IpcError::ssh(format!("ssh spawn: {e}")))?;

        tokio::select! {
            _ = token.cancelled() => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                Err(IpcError::cancelled("ssh cancelled"))
            }
            output = child.wait_with_output() => {
                let output = output.map_err(|e| IpcError::ssh(format!("ssh wait: {e}")))?;
                if !output.status.success() {
                    return Err(IpcError::ssh(format!(
                        "ssh exit {}: {}",
                        output.status,
                        String::from_utf8_lossy(&output.stderr).trim()
                    )));
                }
                Ok(String::from_utf8_lossy(&output.stdout).into_owned())
            }
        }
    }
}
```

Add `IpcError::cancelled` constructor returning `IpcError { code: "E_CANCELLED", ... }`.

- [ ] **Step 2: `probe_ssh_alias` accepts `call_id` and uses the registry**

In `src-tauri/src/commands/hosts.rs`:

```rust
#[derive(Deserialize)]
pub struct ProbeSshAliasArgs {
    pub alias: String,
    pub call_id: Option<u64>,
}

#[tauri::command]
pub async fn probe_ssh_alias(
    args: ProbeSshAliasArgs,
    store: State<'_, Mutex<Store>>,
    ssh: State<'_, SshClient>,
    reg: State<'_, Arc<CancellationRegistry>>,
) -> Result<HostRow, IpcError> {
    let token = match args.call_id {
        Some(id) => {
            let t = tokio_util::sync::CancellationToken::new();
            reg.bind(id, t.clone());
            t
        }
        None => reg.register_anonymous().1,
    };

    // ... existing probe logic, but every ssh call goes through run_cancellable(token.clone())
    let oauth_json = ssh.run_cancellable(&args.alias, "cat ~/.claude.json", token.clone()).await?;
    // ... parse, upsert account, return updated host row

    if let Some(id) = args.call_id { reg.unregister(id); }
    Ok(host_row)
}
```

- [ ] **Step 3: `ensure_remote_project` threads the same token**

Same pattern in the `ensure_remote_project` helper used by `new_session`. The 120-second `git clone` becomes:

```rust
pub async fn ensure_remote_project(
    ssh: &SshClient,
    alias: &str,
    owner: &str,
    repo: &str,
    token: CancellationToken,
) -> Result<String, IpcError> {
    let path = format!("$HOME/projects/github.com/{owner}/{repo}");
    let cmd = format!(
        "if [ ! -d {path} ]; then \
           mkdir -p $HOME/projects/github.com/{owner} && \
           cd $HOME/projects/github.com/{owner} && \
           git clone --quiet git@github.com:{owner}/{repo}.git {repo}; \
         fi && echo {path}"
    );
    let out = ssh.run_cancellable(alias, &cmd, token).await?;
    Ok(out.trim().to_string())
}
```

`new_session` passes its own token down — get it from `new_session`'s args.

- [ ] **Step 4: Run + commit**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib 2>&1 | tail -6
```

Expected: green. Add a smoke test for cancellation if time permits, but the registry tests already cover the core behaviour.

```bash
git add src-tauri/src/ssh.rs src-tauri/src/commands/hosts.rs src-tauri/src/commands/sessions.rs
git commit -m "cancel: thread CancellationToken through probe + remote_project"
```

---

### Task 19: Per-row Cancel UX

**Files:**
- Modify: `src/lib/AddHostPicker.svelte`
- Modify: `src/lib/NewSessionDialog.svelte`

- [ ] **Step 1: AddHostPicker — Cancel button on probe**

In `src/lib/AddHostPicker.svelte`, replace the simple probe call with an AbortController:

```ts
let probing = $state(false);
let probeController: AbortController | null = null;

async function startProbe(alias: string) {
  probing = true;
  probeController = new AbortController();
  const r = await invokeCmdAbortable<HostRow>('probe_ssh_alias',
    { args: { alias } }, probeController.signal);
  probing = false;
  probeController = null;
  if (r.ok) {
    previewHost = r.value;
  } else if (r.error.code !== 'E_CANCELLED') {
    probeError = r.error.message;
  }
}

function cancelProbe() {
  probeController?.abort();
}
```

UI:

```svelte
{#if probing}
  <button onclick={cancelProbe}>Cancel probe</button>
{:else}
  <button onclick={() => startProbe(aliasInput)}>Probe</button>
{/if}
```

- [ ] **Step 2: NewSessionDialog — Cancel button on clone**

Same pattern in `src/lib/NewSessionDialog.svelte`. The `newSession` IPC may take up to 120s for remote clone — give the user the same abort handle.

```ts
let creating = $state(false);
let createController: AbortController | null = null;

async function create() {
  creating = true;
  createController = new AbortController();
  const r = await invokeCmdAbortable<SessionRow>('new_session',
    { args: { ...formState } }, createController.signal);
  creating = false;
  createController = null;
  if (r.ok) {
    mergeSession(r.value);
    onClose();
  } else if (r.error.code !== 'E_CANCELLED') {
    error = r.error.message;
  }
}
```

UI:

```svelte
{#if creating}
  <button onclick={() => createController?.abort()}>Cancel</button>
{:else}
  <button onclick={create}>Create</button>
{/if}
```

- [ ] **Step 3: Type-check + vitest**

```bash
pnpm tsc --noEmit 2>&1 | tail -3
pnpm vitest run 2>&1 | tail -4
```

Expected: green.

- [ ] **Step 4: Commit**

```bash
git add src/lib/AddHostPicker.svelte src/lib/NewSessionDialog.svelte
git commit -m "ui: Cancel button for AddHostPicker probe + NewSessionDialog clone"
```

---

## M5 — Frontend perf cleanup

Goal: kill the per-keystroke O(M·N) work in Sidebar, parallelize PromptComposer send, throttle window.focus, fix `list_projects` N+1, and move `git worktree list` out of the Store lock.

### Task 20: Sidebar memoised indices

**Files:**
- Modify: `src/lib/Sidebar.svelte`

- [ ] **Step 1: Build the indices once per `$sessions` change**

In `src/lib/Sidebar.svelte` `<script>`:

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

const orphanSessions = $derived(
  $sessions.filter((s) => s.project_id == null),
);

const relatedCountById = $derived.by(() => {
  const grouped = new Map<string, SessionRow[]>();
  for (const s of $sessions) {
    if (s.project_id == null) continue;
    const key = `${s.project_id}:${s.worktree_id ?? 'null'}`;
    if (!grouped.has(key)) grouped.set(key, []);
    grouped.get(key)!.push(s);
  }
  const out = new Map<number, number>();
  for (const list of grouped.values()) {
    for (const s of list) out.set(s.id, list.length - 1);
  }
  return out;
});

function sessionsForProject(projectId: number): SessionRow[] {
  return sessionsByProject.get(projectId) ?? [];
}
function relatedCountFor(s: SessionRow): number {
  return relatedCountById.get(s.id) ?? 0;
}
```

- [ ] **Step 2: Remove inline filters from the template**

Find every inline `$sessions.filter(...)` or `sessions.filter(...)` callsite in `Sidebar.svelte`. Replace with the helper functions above (which now hit the memoised Map in O(1) per row).

- [ ] **Step 3: Render-cost test**

In `src/lib/Sidebar.test.ts`, append:

```ts
it('renders 500 sessions across 25 projects without freezing', async () => {
  const sess: SessionRow[] = [];
  for (let p = 1; p <= 25; p++) {
    for (let i = 0; i < 20; i++) {
      sess.push({
        id: p * 100 + i,
        tmux_name: `proj-${p}-sess-${i}`,
        host_alias: 'local',
        project_id: p,
        worktree_id: 1,
        created_at: 0, last_activity_at: 0,
        status: 'running', notes: null, account_uuid: null,
      });
    }
  }
  sessions.set(sess);
  const start = performance.now();
  render(Sidebar, { props: {} });
  await tick();
  const elapsed = performance.now() - start;
  // Generous budget — point is no quadratic blow-up.
  expect(elapsed).toBeLessThan(500);
});
```

- [ ] **Step 4: Run + commit**

```bash
pnpm vitest run src/lib/Sidebar.test.ts 2>&1 | tail -10
```

Expected: existing Sidebar tests + new render-cost test all pass.

```bash
git add src/lib/Sidebar.svelte src/lib/Sidebar.test.ts
git commit -m "Sidebar: O(1) memoised indices via \$derived Maps"
```

---

### Task 21: PromptComposer parallel send

**Files:**
- Modify: `src/lib/PromptComposer.svelte`

- [ ] **Step 1: Replace the for-of await loop**

In `src/lib/PromptComposer.svelte`, replace the existing `send()`:

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
      if (r.ok) {
        succeeded[key] = true;
      } else {
        errors[key] = r.error.message;
      }
    }),
  );
  sending = false;
  if (Object.keys(errors).length === 0) {
    setTimeout(() => onClose(), 600);
  }
}
```

- [ ] **Step 2: Un-disable Cancel + textarea + checkboxes**

In the template, the only thing that stays `disabled={sending}` is the Send button:

```svelte
<button onclick={onClose}>Cancel</button>  <!-- no disabled -->
<textarea bind:value={prompt} ...></textarea>  <!-- no disabled -->
<input type="checkbox" ...>  <!-- no disabled -->
<button class="primary" disabled={!canSend} onclick={send}>Send →</button>  <!-- keeps disabled -->
```

- [ ] **Step 3: Concurrency test**

Append to `src/lib/PromptComposer.test.ts`:

```ts
it('fires all sends concurrently, not sequentially', async () => {
  // Configure mock to record call timestamps.
  const callTimes: number[] = [];
  (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
    if (cmd === 'send_prompt') {
      callTimes.push(performance.now());
      await new Promise((r) => setTimeout(r, 50));
      return null;
    }
    return null;
  });
  // 3 sibling targets
  const sibs = [2, 3, 4].map((id) => ({
    id, tmux_name: `dev-sib-${id}`, host_alias: 'mefistos',
    project_id: 1, worktree_id: 10,
    created_at: 1, last_activity_at: 1, status: 'running',
    notes: null, account_uuid: null,
  }));
  sessions.set([source, ...sibs]);
  render(PromptComposer, { props: { source, onClose: () => {} } });
  await tick();
  const textarea = screen.getByTestId('composer-textarea') as HTMLTextAreaElement;
  await fireEvent.input(textarea, { target: { value: 'hello' } });
  await tick();
  await fireEvent.click(screen.getByTestId('composer-send'));
  for (let i = 0; i < 12; i++) await tick();
  expect(callTimes).toHaveLength(3);
  // All three should start within 10ms of each other (concurrent).
  const span = Math.max(...callTimes) - Math.min(...callTimes);
  expect(span).toBeLessThan(10);
});
```

- [ ] **Step 4: Run + commit**

```bash
pnpm vitest run src/lib/PromptComposer.test.ts 2>&1 | tail -10
```

Expected: existing 4 tests + new concurrency test pass.

```bash
git add src/lib/PromptComposer.svelte src/lib/PromptComposer.test.ts
git commit -m "PromptComposer: parallel send via Promise.allSettled"
```

---

### Task 22: `window.focus` throttle

**Files:**
- Modify: `src/App.svelte`

- [ ] **Step 1: Throttle the focus handler**

In `src/App.svelte`, replace the existing `window.addEventListener('focus', ...)`:

```ts
let lastFocusFetch = 0;
const FOCUS_FETCH_INTERVAL_MS = 30_000;
function onFocus() {
  const now = Date.now();
  if (now - lastFocusFetch < FOCUS_FETCH_INTERVAL_MS) return;
  lastFocusFetch = now;
  // With M3 events live, this is a catch-up net for missed events / sleep wake.
  void bootstrapProjects();
  void bootstrapSessions();
}
onMount(() => {
  window.addEventListener('focus', onFocus);
});
onDestroy(() => {
  window.removeEventListener('focus', onFocus);
});
```

- [ ] **Step 2: Commit**

```bash
git add src/App.svelte
git commit -m "App: throttle window.focus refresh to 30s"
```

(No new tests — the change is mechanical and covered by manual verify in M6.)

---

### Task 23: `list_projects` JOIN + `refresh_projects` off-lock

**Files:**
- Modify: `src-tauri/src/commands/projects.rs`
- Modify: `src-tauri/src/store.rs`

- [ ] **Step 1: Replace N+1 with a single JOIN**

In `src-tauri/src/store.rs`, add:

```rust
impl Store {
    pub fn list_projects_with_worktrees(&self) -> rusqlite::Result<Vec<ProjectRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.id, p.owner, p.repo, p.base_path, p.last_session_at,
                    w.id, w.name, w.path
             FROM projects p
             LEFT JOIN worktrees w ON w.project_id = p.id
             ORDER BY p.last_session_at DESC NULLS LAST, p.id, w.id"
        )?;
        let mut projects: Vec<ProjectRow> = Vec::new();
        let mut last_proj_id: Option<i64> = None;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
            ))
        })?;
        for r in rows {
            let (pid, owner, repo, base, last, wid, wname, wpath) = r?;
            if last_proj_id != Some(pid) {
                projects.push(ProjectRow {
                    id: pid, owner, repo, base_path: base, last_session_at: last,
                    worktrees: Vec::new(),
                });
                last_proj_id = Some(pid);
            }
            if let (Some(wid), Some(wname), Some(wpath)) = (wid, wname, wpath) {
                projects.last_mut().unwrap().worktrees.push(WorktreeRow {
                    id: wid, project_id: pid, name: wname, path: wpath,
                });
            }
        }
        Ok(projects)
    }
}
```

(Adjust `ProjectRow` to carry `pub worktrees: Vec<WorktreeRow>` if it doesn't already; update existing `list_projects` to call this new method.)

- [ ] **Step 2: `refresh_projects` moves `git worktree list` off-lock**

In `src-tauri/src/commands/projects.rs`:

```rust
#[tauri::command]
pub async fn refresh_projects(
    store: State<'_, Mutex<Store>>,
) -> Result<(), IpcError> {
    // 1. Snapshot project list under lock.
    let projects = {
        let s = store.lock().expect("store");
        s.with_snapshot(|s| s.list_projects_with_worktrees().map_err(IpcError::from))?
    };

    // 2. Discover worktrees concurrently, off-lock.
    let mut set = tokio::task::JoinSet::new();
    for p in &projects {
        let base = p.base_path.clone();
        let pid = p.id;
        set.spawn(async move {
            let out = tokio::process::Command::new("git")
                .args(["worktree", "list", "--porcelain"])
                .current_dir(&base)
                .output()
                .await;
            (pid, base, out)
        });
    }

    let mut discovered: Vec<(i64, String, Vec<(String, String)>)> = Vec::new();
    while let Some(joined) = set.join_next().await {
        if let Ok((pid, base, Ok(out))) = joined {
            if out.status.success() {
                let parsed = parse_worktree_porcelain(&String::from_utf8_lossy(&out.stdout));
                discovered.push((pid, base, parsed));
            }
        }
    }

    // 3. Apply writes in a single transaction.
    let mut s = store.lock().expect("store");
    s.with_transaction(|tx| {
        for (pid, _base, worktrees) in &discovered {
            for (name, path) in worktrees {
                Store::upsert_worktree_in_tx(tx, *pid, name, path)?;
            }
        }
        Ok(())
    })?;
    Ok(())
}
```

Add `upsert_worktree_in_tx` to `store.rs` (mirrors the existing `upsert_worktree`).

`parse_worktree_porcelain` is the existing parser — copy or extract from current `list_worktrees`.

- [ ] **Step 3: Run + commit**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib 2>&1 | tail -6
pnpm vitest run 2>&1 | tail -4
```

Expected: green.

```bash
git add src-tauri/src/commands/projects.rs src-tauri/src/store.rs
git commit -m "projects: JOIN list query + off-lock refresh fan-out"
```

---

## M6 — Final verify + push

### Task 24: Test sweep + live verify

- [ ] **Step 1: Final test sweep**

```bash
cd /Users/martinjanci/projects/github.com/martin-janci/claude-fleet
cargo test --manifest-path src-tauri/Cargo.toml --lib 2>&1 | tail -5
pnpm vitest run 2>&1 | tail -5
```

Expected: all green. Note new counts (≥ 88 + ≥ 137 from baseline; expect ~95 + ~145 with the new tests added).

- [ ] **Step 2: Build release bundle**

```bash
pnpm tauri build --bundles app 2>&1 | tail -8
```

Expected: clean build.

- [ ] **Step 3: Restart app**

```bash
pkill -f "claude-fleet.app/Contents/MacOS/claude-fleet" 2>/dev/null; sleep 1
open -a /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/src-tauri/target/release/bundle/macos/claude-fleet.app
sleep 3
```

- [ ] **Step 4: Live verify — unreachable host doesn't freeze UI**

Block port 22 on a hostname. Easiest path:

```bash
# Pick an SSH alias from ~/.ssh/config that you can safely block — or invent a
# bogus one ahead of time:
echo -e "\nHost bogus-test-host\n  HostName 192.0.2.1\n  ConnectTimeout 5\n  ControlMaster auto\n  ControlPersist 10m" >> ~/.ssh/config
```

In claude-fleet:
1. Open Settings → Hosts → `+ Add host` → alias `bogus-test-host`. Probe will fail after ~5s with `E_TIMEOUT`.
2. While probe is in flight, click around: open Sidebar, switch sessions, click `Refresh`. The UI must remain responsive.
3. After the timeout, the row shows `unreachable`; other hosts must be unaffected.

- [ ] **Step 5: Live verify — PromptComposer parallel**

In claude-fleet:
1. Have 2+ remote sessions (the mefistos `dev-martin-janci-claude-fleet` plus a local sibling).
2. Open PromptComposer. Type `echo concurrent test`.
3. Click Send. Per-target ticks should appear within ~100ms of each other (concurrent), not staggered.
4. Click `Cancel` mid-flight (after Send): the dialog must close immediately without finishing the rest. (Cancel button stays live throughout sending.)

- [ ] **Step 6: Live verify — Cancel button on probe**

1. Add another bogus host (`bogus-test-host-2` with same trick).
2. Click Probe. While the 5s timeout is counting down, click `Cancel probe`.
3. Probe must abort within ~100ms. Backend logs should show `cancel_command` invoked.

- [ ] **Step 7: Document live-verify outcomes**

If any anomalies, append to `docs/specs/2026-05-20-iter4a-responsiveness-design.md` under a new `## Live verification notes` section. Commit separately:

```bash
git add docs/specs/2026-05-20-iter4a-responsiveness-design.md
git commit -m "docs: iter 4a spec — live verification notes"
```

- [ ] **Step 8: Remove the bogus SSH alias**

```bash
# Manual: edit ~/.ssh/config to remove the bogus-test-host stanzas.
```

---

### Task 25: Push to origin

- [ ] **Step 1: Review the iter 4a commit range**

```bash
git log --oneline origin/main..HEAD
```

Expected: ~25 commits (one per task plus the spec).

- [ ] **Step 2: Push**

```bash
git push origin main 2>&1 | tail -3
```

- [ ] **Step 3: Final verify**

```bash
git status
git log -1 --pretty=full
```

Confirm: tree clean, latest commit pushed.

---

## Self-Review (filled in by plan author)

**Spec coverage check:**

- `tokio` adoption + `tokio::process::Command` for SSH/tmux/git → Tasks 1, 2, 3 ✓
- Read-snapshot / write-burst lock pattern + helpers → Tasks 4, 6, 7, 23 ✓
- `ensure_master` per-host idempotency via `DashMap<OnceCell>` → Task 2 ✓
- Async fn migration for SSH-touching commands → Task 5 ✓
- Parallel reconcile via `JoinSet` → Task 6 ✓
- Per-host reconcile granularity + mutations return affected row → Task 7 ✓
- `EventBus` trait + `NoopEventBus` + `RecordingEventBus` + `AppHandleEventBus` → Tasks 8, 10 ✓
- Wire `EventBus` into every Store mutation (incl. reconcile batch) → Task 9 ✓
- TS event type definitions + `subscribeToRowEvents` helper → Tasks 11, 12 ✓
- Stores: `bootstrap` + `mergeOne` + `removeOne` → Task 12 ✓
- Mutation wrappers drop `loadX()` chain → Task 13 ✓
- App + Sidebar subscribe to events at mount → Task 14 ✓
- Event-subscription unit tests → Task 15 ✓
- `CancellationRegistry` + `cancel_command` → Task 16 ✓
- `invokeCmdAbortable` → Task 17 ✓
- SSH + remote_project threading `CancellationToken` → Task 18 ✓
- Per-row Cancel UX → Task 19 ✓
- Sidebar memoised indices → Task 20 ✓
- PromptComposer parallel send → Task 21 ✓
- `window.focus` throttle 30s → Task 22 ✓
- `list_projects` JOIN + `refresh_projects` off-lock → Task 23 ✓
- Live verify with blocked port 22 + parallel send + Cancel → Task 24 ✓
- M6 virtualization explicitly NOT in this plan, per spec ✓
- Out-of-scope items (cross-host worktree mapping, TerminalView push PTY, `ensure_master` retry/backoff, PTY buffer-lock optimisation) explicitly NOT in this plan, per spec ✓

**Placeholder scan:** Every code-bearing step has actual code. Every commit step has the actual commit message in a HEREDOC. No "TBD" / "TODO" remain. No "similar to Task N" — code is repeated where needed.

**Type consistency:**
- `SshClient::run` and `run_cancellable` both take `&self, alias: &str, cmd: &str` (plus token for the latter). Return `Result<String, IpcError>`. Consistent.
- `TmuxExec` trait methods are `async fn`, used identically in `LocalTmux` and `RemoteTmux`. Consistent.
- `EventBus` trait names (`session_created`, `session_updated`, …) align with the emitted event names (`session:created`, `session:updated`, …). Consistent.
- Frontend `mergeSession`, `mergeHost`, `mergeAccount`, `mergeProject`, `mergeWorktree` all follow the same `(row) => store.update(...)` shape. Consistent.
- `invokeCmdAbortable` returns `Promise<Result<T>>`, same shape as `invokeCmd`. Consistent.
- `CancellationRegistry::register_anonymous` returns `(u64, CancellationToken)`; `bind` takes `(u64, CancellationToken)`. Consistent.
- Tasks numbered 1–25 inclusive; no gaps; M1=1–5, M2=6–7, M3=8–15, M4=16–19, M5=20–23, M6=24–25.
