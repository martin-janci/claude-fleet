# Recreate a live tmux session — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the user tear down and rebuild a *live* session's tmux session in one click (for a wedged/frozen or RAM-hungry session), bringing it back in the same worktree with the Claude REPL — by generalizing the existing `recreate_session` to work on running sessions, not just ghosts.

**Architecture:** Backend — `recreate_session` drops its ghost-only precondition, resolves the session's worktree cwd (shared `resolve_session_cwd` helper, generalized from `resolve_review_cwd`) and a kind-appropriate launch command, then `kill-session` (tolerated if absent) → `tmux.new_session(name, cwd, cmd)` → `restore_session`. Frontend — a "Recreate" action (with a confirm modal) on live sessions in `Sidebar` and `SessionDetails`, which forces a PTY re-attach after success because `kill-session` severs the terminal and the unchanged `tmux_name` won't auto-trigger TerminalView's re-open.

**Tech Stack:** Rust (Tauri 2 service layer, `cargo test`), Svelte 5 runes, TypeScript, Vitest.

> **Build/test:** backend from `src-tauri/` (`cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`); frontend from repo root (`pnpm check`, `pnpm exec vitest run ...`). Pre-existing `localStorage`-env frontend test failures are unrelated (verify against `main`).

> **Reference spec:** `docs/specs/2026-05-22-recreate-live-session-design.md`

> **Working directory:** the worktree at `/Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/recreate-session` (branch `recreate-session`).

---

## File structure

Modified only (no new files):

- `src-tauri/src/service/sessions.rs` — rename/generalize `resolve_review_cwd` → `resolve_session_cwd`; add `recreate_pane_command`; rewrite `recreate_session`; update tests.
- `src/lib/Sidebar.svelte` — live Recreate button + confirm modal + force PTY re-attach.
- `src/lib/SessionDetails.svelte` — live Recreate button + confirm modal + force PTY re-attach.

Reused unchanged: the `recreate_session` Tauri command + `recreateSession` IPC wrapper, `tmux::{kill_session,new_session,pane_command,shell_pane_command}`, `store::restore_session`, `selection::selectSession`, `TerminalView` PTY effect.

---

## Phase A — Backend

### Task 1: Generalize `resolve_review_cwd` → `resolve_session_cwd`

**Files:**
- Modify: `src-tauri/src/service/sessions.rs`

The helper currently resolves a *review's* cwd; it's exactly the resolution Recreate needs. Rename it, generalize the doc + error, and repoint `spawn_review`.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `service/sessions.rs` (it already `use`s `crate::store::Store`):

```rust
    #[test]
    fn resolve_session_cwd_prefers_worktree_then_project_then_errors() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        // A session with neither worktree nor project → E_NOREPO.
        s.upsert_session("dev", "local", None, None, 1, 1, "running", None)
            .unwrap();
        let row = s.get_session("dev", "local").unwrap().unwrap();
        let err = resolve_session_cwd(&s, &row).unwrap_err();
        assert_eq!(err.code, "E_NOREPO");
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd src-tauri && cargo test resolve_session_cwd`
Expected: FAIL — `cannot find function resolve_session_cwd` (and the error code is currently `E_INVALID`).

- [ ] **Step 3: Rename + generalize the helper**

Replace the existing `resolve_review_cwd` function with:

```rust
/// Resolve the cwd a session should (re)open in. Order: the session's worktree
/// path (by `worktree_id`) → its project `base_path` → error. Used by both
/// `spawn_review` and `recreate_session`.
fn resolve_session_cwd(s: &Store, row: &crate::store::SessionRow) -> Result<String, IpcError> {
    if let Some(wt_id) = row.worktree_id {
        if let Some(path) = s.worktree_path(wt_id)? {
            return Ok(path);
        }
    }
    if let Some(pid) = row.project_id {
        if let Some(base) = s.project_base_path(pid)? {
            return Ok(base);
        }
    }
    Err(IpcError::new(
        "E_NOREPO",
        "cannot determine a worktree path for this session",
    ))
}
```

In `spawn_review`, change the call `let cwd = resolve_review_cwd(&s, &source)?;` to `let cwd = resolve_session_cwd(&s, &source)?;`.

- [ ] **Step 4: Run to verify it passes**

Run: `cd src-tauri && cargo test resolve_session_cwd`
Expected: PASS.

- [ ] **Step 5: clippy/fmt + commit**

Run: `cd src-tauri && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: clean.

```bash
git add src-tauri/src/service/sessions.rs
git commit -m "refactor(sessions): generalize resolve_review_cwd into resolve_session_cwd"
```

### Task 2: Add `recreate_pane_command(kind)` helper

**Files:**
- Modify: `src-tauri/src/service/sessions.rs`

Mirror `restart_session`'s kind logic as a small pure helper so recreate and tests share it.

- [ ] **Step 1: Write the failing test**

Add to `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn recreate_pane_command_matches_kind() {
        // Shell sessions come back as a bare shell (start command isn't
        // persisted); everything else (work/review) relaunches the REPL.
        assert_eq!(
            recreate_pane_command("shell"),
            crate::tmux::shell_pane_command(None)
        );
        assert_eq!(
            recreate_pane_command("work"),
            crate::tmux::pane_command().to_string()
        );
        assert_eq!(
            recreate_pane_command("review"),
            crate::tmux::pane_command().to_string()
        );
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd src-tauri && cargo test recreate_pane_command`
Expected: FAIL — `cannot find function recreate_pane_command`.

- [ ] **Step 3: Implement the helper**

Add near `recreate_session` in `service/sessions.rs`:

```rust
/// The pane command to relaunch when (re)creating a session of `kind`.
/// `shell` → a bare shell (the original custom start command isn't persisted,
/// matching `restart_session`); anything else → the Claude REPL.
fn recreate_pane_command(kind: &str) -> String {
    if kind == "shell" {
        crate::tmux::shell_pane_command(None)
    } else {
        crate::tmux::pane_command().to_string()
    }
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cd src-tauri && cargo test recreate_pane_command`
Expected: PASS.

- [ ] **Step 5: commit**

```bash
git add src-tauri/src/service/sessions.rs
git commit -m "feat(sessions): add recreate_pane_command kind helper"
```

### Task 3: Rewrite `recreate_session` to rebuild live (and ghost) sessions

**Files:**
- Modify: `src-tauri/src/service/sessions.rs`

Drop the ghost-only precondition; resolve cwd + pane command; kill (tolerated) → rebuild → restore. Keep the host-reachability gate (it runs before any tmux call, so the validation tests stay deterministic).

- [ ] **Step 1: Update the tests first (remove the now-wrong rejection test, add precondition tests)**

In `#[cfg(test)] mod ghost_tests`, **delete** the entire `recreate_session_rejects_non_ghost` test (recreate now *accepts* running sessions, so calling it on a running local session would reach real `tmux` — that test no longer expresses correct behavior). Add these two tests, which exercise the pre-tmux validation paths and stay deterministic:

```rust
    #[test]
    fn recreate_session_errors_when_session_missing() {
        let store = std::sync::Mutex::new(Store::open_in_memory().unwrap());
        let args = RecreateSessionArgs { session_id: 999 };
        let ssh = std::sync::Arc::new(crate::ssh::SshClient::new());
        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt
            .block_on(recreate_session(args, &store, &ssh))
            .unwrap_err();
        assert_eq!(err.code, "E_NOTFOUND");
    }

    #[test]
    fn recreate_session_errors_when_host_offline() {
        let store = std::sync::Mutex::new(Store::open_in_memory().unwrap());
        {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            // upsert_host defaults reachable=false until a probe; good enough
            // to exercise the offline gate before any tmux call.
            s.upsert_session("dev", "local", None, None, 1, 1, "running", None)
                .unwrap();
        }
        let id = store
            .lock()
            .unwrap()
            .get_session("dev", "local")
            .unwrap()
            .unwrap()
            .id;
        let args = RecreateSessionArgs { session_id: id };
        let ssh = std::sync::Arc::new(crate::ssh::SshClient::new());
        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt
            .block_on(recreate_session(args, &store, &ssh))
            .unwrap_err();
        assert_eq!(err.code, "E_HOST_OFFLINE");
    }
```

> Note: confirm `upsert_host("local")` leaves `reachable=false` (the offline gate). If in this codebase `local`/freshly-upserted hosts are reachable by default, instead force it: after `upsert_host`, run `store.lock().unwrap().conn_ref().execute("UPDATE hosts SET reachable=0 WHERE alias='local'", [])` so the test deterministically hits the offline gate. Use whichever the schema requires; the assertion (`E_HOST_OFFLINE`) is the contract.

- [ ] **Step 2: Run to verify the new tests fail (or the old one still compiles wrong)**

Run: `cd src-tauri && cargo test recreate_session_errors`
Expected: FAIL/compile-error until Step 3 (the missing-session test may already pass via the existing early `E_NOTFOUND`; the host-offline test asserts the gate remains).

- [ ] **Step 3: Rewrite `recreate_session`**

Replace the whole `recreate_session` function body with:

```rust
pub async fn recreate_session(
    args: RecreateSessionArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<SessionRow, IpcError> {
    // Snapshot the session, gate on host reachability, and resolve the rebuild
    // cwd + launch command — all under one brief lock, before any tmux call.
    // Works for both `running` sessions (eating RAM / wedged → nuke & rebuild)
    // and `ghost` sessions (lost from tmux → bring back in the right worktree).
    let (sess, cwd, pane_cmd) = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        let sess = s
            .get_session_by_id(args.session_id)?
            .ok_or_else(|| IpcError::new("E_NOTFOUND", "session not found"))?;
        let host = s
            .get_host_row(&sess.host_alias)?
            .ok_or_else(|| IpcError::new("E_NOTFOUND", "host not found"))?;
        if !host.reachable {
            return Err(IpcError::new(
                "E_HOST_OFFLINE",
                format!("host {} is not reachable", host.alias),
            ));
        }
        let cwd = resolve_session_cwd(&s, &sess)?;
        let pane_cmd = recreate_pane_command(&sess.kind);
        (sess, cwd, pane_cmd)
    };

    let tmux = exec_for(&sess.host_alias, ssh);
    // Tear down any live session first (frees the old process tree / wedged
    // session). A ghost has no live session, so tolerate "no such session":
    // we ignore the kill result and rely on new_session below to fail loudly
    // if the old session unexpectedly survived (it would report a duplicate).
    let _ = tmux.kill_session(&sess.tmux_name).await;
    // Rebuild fresh in the worktree with the kind-appropriate command — the
    // same primitive new_session() uses.
    tmux.new_session(&sess.tmux_name, std::path::Path::new(&cwd), &pane_cmd)
        .await?;

    // Mark the row live again and return it.
    let row = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?
        .restore_session(sess.id)?
        .ok_or_else(|| IpcError::new("E_INTERNAL", "session vanished after restore"))?;
    Ok(row)
}
```

- [ ] **Step 4: Run the backend tests**

Run: `cd src-tauri && cargo test commands::sessions::; cargo test --lib service::sessions 2>/dev/null; cargo test recreate resolve_session_cwd`
Expected: PASS (the two new precondition tests pass; `recreate_session_rejects_non_ghost` is gone; `resolve_session_cwd` + `recreate_pane_command` pass). Run the full `cargo test` to confirm nothing else regressed.

- [ ] **Step 5: clippy/fmt + commit**

Run: `cd src-tauri && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: all clean.

```bash
git add src-tauri/src/service/sessions.rs
git commit -m "feat(sessions): recreate_session rebuilds live + ghost sessions in their worktree"
```

---

## Phase B — Frontend

### Task 4: Live Recreate action in `Sidebar.svelte`

**Files:**
- Modify: `src/lib/Sidebar.svelte`

Add a Recreate button to the **non-ghost** row actions (next to `↻ ✎ ×`), a confirm modal mirroring the existing Kill modal, and a handler that recreates then forces a PTY re-attach if the recreated session is the selected one. The ghost path keeps its own `doRecreate`/`↺` (unchanged).

- [ ] **Step 1: Add `tick` import and confirm state**

`recreateSession`, `selectSession`, `$selectedSession` are already imported. Add `tick`:

```ts
  import { tick } from 'svelte';
```

Add state near the existing `pendingKill` declaration:

```ts
  let pendingRecreate = $state<SessionRow | null>(null);
```

- [ ] **Step 2: Add the ask/confirm/cancel handlers**

Add alongside `askKill`/`confirmKill`/`cancelKill`:

```ts
  function askRecreate(sess: SessionRow, e?: Event) {
    e?.stopPropagation();
    pendingRecreate = sess;
    actionError = null;
  }

  function cancelRecreate() {
    pendingRecreate = null;
  }

  async function confirmRecreate() {
    if (!pendingRecreate) return;
    const sess = pendingRecreate;
    pendingRecreate = null;
    const r = await recreateSession(sess.id);
    if (!r.ok) {
      actionError = r.error.message;
      return;
    }
    // kill-session severed the PTY; the tmux_name is unchanged so TerminalView
    // won't auto-reopen. Force a re-attach when this session is selected by
    // dropping and (after the close effect runs) restoring the selection.
    const cur = $selectedSession;
    if (cur && cur.tmux_name === sess.tmux_name) {
      selectSession(null);
      await tick();
      selectSession(r.value);
    }
  }
```

- [ ] **Step 3: Add the button to the non-ghost row actions**

In the `{:else}` (non-ghost) branch's `<div class="row-actions">`, add a Recreate button before the Kill button (after Rename), gated on host reachability like the ghost one:

```svelte
            <button
              class="icon-btn small"
              data-testid="recreate-live"
              onclick={(e) => askRecreate(sess, e)}
              disabled={!hostIsReachable(sess.host_alias)}
              title={hostIsReachable(sess.host_alias)
                ? 'Recreate: kill the tmux session and start it fresh in the same worktree'
                : 'Host is offline'}
              aria-label="Recreate"
            >♻</button>
```

- [ ] **Step 4: Add the confirm modal**

Mirror the existing kill confirm modal. Find the Sidebar's kill-confirm modal block (the `{#if pendingKill}` … `</div>` overlay) and add an analogous block right after it:

```svelte
{#if pendingRecreate}
  <div class="modal-backdrop" onclick={cancelRecreate} role="presentation">
    <div class="confirm" onclick={(e) => e.stopPropagation()} role="presentation">
      <h3>Recreate session?</h3>
      <p>
        This kills the tmux session <code>{pendingRecreate.tmux_name}</code> and the
        running claude state inside it, then starts a fresh session in the same
        worktree. Continue?
      </p>
      <div class="confirm-actions">
        <button onclick={cancelRecreate}>Cancel</button>
        <button class="danger" onclick={confirmRecreate} data-testid="confirm-recreate">Recreate</button>
      </div>
    </div>
  </div>
{/if}
```

> If the Sidebar's kill modal uses different class names than `.modal-backdrop`/`.confirm`/`.confirm-actions`, reuse whatever the kill modal uses verbatim (read it first) so styling is consistent. Don't invent new CSS.

- [ ] **Step 5: Verify**

Run: `pnpm check`
Expected: no new errors in `Sidebar.svelte` (pre-existing `Sidebar.test.ts` `localStorage` failures are unrelated).

Run: `pnpm exec vitest run src/lib/Sidebar.test.ts` — note any failures and compare to `main` (the `localStorage`-env failures pre-exist). New behavior isn't unit-tested here; it's covered in the manual smoke (Task 6).

- [ ] **Step 6: Commit**

```bash
git add src/lib/Sidebar.svelte
git commit -m "feat(sidebar): Recreate action for live sessions with confirm + PTY re-attach"
```

### Task 5: Live Recreate action in `SessionDetails.svelte`

**Files:**
- Modify: `src/lib/SessionDetails.svelte`

Add a Recreate button in the actions section and a confirm modal mirroring the existing `confirmingKill` modal, plus the same forced re-attach (the details panel always shows the selected session, so re-attach always applies on success).

- [ ] **Step 1: Imports + state**

Add `recreateSession` to the existing `./sessions` import (it currently imports `killSession, renameSession, restartSession`):

```ts
  import { killSession, renameSession, restartSession, recreateSession } from './sessions';
```

Add `tick`:

```ts
  import { tick } from 'svelte';
```

Add confirm state near the existing `confirmingKill` declaration:

```ts
  let confirmingRecreate = $state(false);
```

- [ ] **Step 2: Handlers**

Add alongside the kill handlers (`askKill`/`doKill`/`cancelKill`):

```ts
  function askRecreate() {
    confirmingRecreate = true;
    actionError = null;
  }

  function cancelRecreate() {
    confirmingRecreate = false;
  }

  async function doRecreate() {
    confirmingRecreate = false;
    const r = await recreateSession(session.id);
    if (!r.ok) {
      actionError = r.error.message;
      return;
    }
    // kill-session severed the PTY; same tmux_name won't auto-reopen. This
    // panel shows the selected session, so force a re-attach.
    selectSession(null);
    await tick();
    selectSession(r.value);
  }
```

- [ ] **Step 3: Add the button**

In the `<section class="block actions">`, add a Recreate button just before the Kill button:

```svelte
    <button class="ghost" onclick={askRecreate} data-testid="recreate-from-details">
      ♻ Recreate
    </button>
```

- [ ] **Step 4: Add the confirm modal**

After the existing `{#if confirmingKill}` … modal block, add:

```svelte
{#if confirmingRecreate}
  <div class="modal-backdrop" onclick={cancelRecreate} role="presentation">
    <div class="confirm" onclick={(e) => e.stopPropagation()} role="presentation">
      <h3>Recreate session?</h3>
      <p>This kills the tmux session <code>{session.tmux_name}</code> and the running claude state inside it, then starts a fresh session in the same worktree. Continue?</p>
      <div class="confirm-actions">
        <button onclick={cancelRecreate}>Cancel</button>
        <button class="danger" onclick={doRecreate} data-testid="confirm-recreate-details">Recreate</button>
      </div>
    </div>
  </div>
{/if}
```

- [ ] **Step 5: Verify**

Run: `pnpm check`
Expected: no new errors in `SessionDetails.svelte`.

Run: `pnpm exec vitest run src/lib/SessionDetails.test.ts` — compare any failures to `main`.

- [ ] **Step 6: Commit**

```bash
git add src/lib/SessionDetails.svelte
git commit -m "feat(session-details): Recreate action with confirm + PTY re-attach"
```

---

## Phase C — Verify

### Task 6: Full verification + manual smoke

**Files:** none (verification only).

- [ ] **Step 1: Backend suite**

Run: `cd src-tauri && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: all PASS/clean.

- [ ] **Step 2: Frontend type-check + tests**

Run: `pnpm check`
Expected: no new errors from `Sidebar.svelte` / `SessionDetails.svelte`.

Run: `pnpm exec vitest run`
Expected: only the pre-existing `localStorage`-env failures (same set as `main`). Confirm no new failures attributable to this change.

- [ ] **Step 3: Manual smoke (run the app)**

`pnpm tauri dev`. With a live work session selected in the terminal pane:
- Click **Recreate** (Sidebar `♻` or SessionDetails `♻ Recreate`) → confirm the modal.
- The tmux session is killed and rebuilt; the terminal re-attaches to a fresh Claude REPL **in the correct worktree cwd** (verify the prompt/cwd).
- Confirm a `shell` session recreates back to a shell, and the Recreate button is disabled when the host is offline.
- Confirm the ghost `↺` Recreate still works (and now also opens in the worktree with the REPL).

- [ ] **Step 4: Final commit (if any smoke fixes were needed)**

```bash
git add -A
git commit -m "fix(recreate): smoke-test adjustments"
```

(Skip if nothing needed fixing.)

---

## Self-review notes (resolved)

- **Spec coverage:** generalized recreate for running+ghost (Task 3), faithful cwd via `resolve_session_cwd` (Task 1) + kind command via `recreate_pane_command` (Task 2), kill-tolerant teardown + rebuild (Task 3), UI Recreate + confirm in Sidebar (Task 4) and SessionDetails (Task 5), forced PTY re-attach (Tasks 4–5), host-offline gating + `E_NOREPO` error (Tasks 1, 3), tests + manual smoke (all tasks, Task 6). Covered.
- **Testability constraint:** `recreate_session` builds tmux via `exec_for` internally (not injected), matching `restart_session`; so the kill→rebuild success path is covered by manual smoke, while pure helpers (`resolve_session_cwd`, `recreate_pane_command`) and pre-tmux validation (`E_NOTFOUND`, `E_HOST_OFFLINE`) are unit-tested. The obsolete `recreate_session_rejects_non_ghost` test is removed because the precondition it asserted is intentionally gone.
- **Type/name consistency:** `resolve_session_cwd`, `recreate_pane_command`, `recreateSession`, `confirmRecreate`/`doRecreate`, `pendingRecreate`/`confirmingRecreate` are used consistently across tasks. `tmux.new_session(name, &Path, &str)` and `restore_session(id) -> Option<SessionRow>` match the existing signatures.
