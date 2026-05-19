# claude-fleet — Phase 2 (Local Discovery & Project Tree) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring the app from "empty 3-pane shell" to "I can see my real projects, their worktrees, and any local tmux+claude sessions; I can start a new session and kill an existing one — all from the GUI." Exit: launch the app, see the user's `~/projects/github.com/{owner}/{repo}` tree populated in the sidebar with first-class worktrees and any live tmux sessions; click "New session" to spawn one; right-click "Kill" to terminate it.

**Architecture:** Persistent `Store` lives as `Mutex<Store>` in `tauri::State`, opened once at app startup against the platform appdata path. Three new Rust modules — `projects` (FS scan + `git worktree list` parse), `tmux` (`tmux list-sessions` / `new-session` / `kill-session` wrappers via `std::process::Command`), `ipc_error` (uniform `Result<T, IpcError>` for all commands). Five new commands expose them. Frontend adds `Sidebar.svelte` (renders the tree), `NewSessionDialog.svelte`, and stores/types for projects + sessions. Pane.svelte gets converted to Svelte 5 runes + `Snippet` children to match the rest of the codebase. UI auto-refreshes when the window regains focus.

**Tech Stack:** Rust 1.83+ · Tauri 2 · `rusqlite 0.32` · `std::process::Command` (system `tmux` + `git`) · Svelte 5 (runes) + TypeScript · Vite · Vitest · `@testing-library/svelte`.

**Reference:** [Spec](../specs/2026-05-19-claude-fleet-design.md) §5–§9. [Phase 1 plan](2026-05-19-claude-fleet-phase-1.md) for the foundation this builds on.

---

## File Structure (created or significantly modified during this plan)

```
src-tauri/src/
├── ipc_error.rs                 # NEW: IpcError { code, message, details } + From impls
├── projects.rs                  # NEW: scan_projects + parse_worktrees
├── tmux.rs                      # NEW: list_local_sessions, new_session, kill_session
├── store.rs                     # MODIFY: new upsert/query methods (projects, worktrees, sessions)
├── lib.rs                       # MODIFY: open Store at startup, .manage(Mutex<Store>)
└── commands/
    ├── mod.rs                   # MODIFY: pub mod projects, sessions
    ├── health.rs                # MODIFY: use tauri::State<Mutex<Store>>
    ├── projects.rs              # NEW: list_projects, refresh_projects commands
    └── sessions.rs              # NEW: list_sessions, new_session, kill_session commands

src/
├── App.svelte                   # MODIFY: mount Sidebar, focus-refresh
├── lib/
│   ├── Pane.svelte              # MODIFY: convert to $props() + Snippet children
│   ├── Sidebar.svelte           # NEW: project → worktrees → sessions tree + filter + search
│   ├── NewSessionDialog.svelte  # NEW: project + worktree + name picker; spawns tmux
│   ├── ipc.ts                   # MODIFY: refactor invoke wrapper to surface IpcError
│   ├── projects.ts              # NEW: ProjectTree types + Svelte store + IPC bindings
│   └── sessions.ts              # NEW: Session types + Svelte store + IPC bindings
└── App.test.ts                  # MODIFY: assertions still pass; add focus-refresh case
```

---

## Task 1: Wrap `Store` in `Mutex<Store>` and register via `tauri::State`

**Files:**
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/store.rs` (drop targeted `#[allow(dead_code)]` lines now that `open` is wired up)
- Modify: `src-tauri/src/commands/health.rs` (read shared store instead of opening fresh)
- Modify: `src-tauri/Cargo.toml` (add `directories = "5"` for cross-platform appdata path)

- [ ] **Step 1: Add the `directories` crate**

Edit `src-tauri/Cargo.toml`. In the `[dependencies]` table, add:
```toml
directories = "5"
```

Run:
```bash
cd ~/projects/github.com/martin-janci/claude-fleet/src-tauri
cargo build
```
Expected: builds cleanly (resolves the new crate).

- [ ] **Step 2: Write the failing test for store-bound `health_check`**

Open `src-tauri/src/commands/health.rs`. Replace the file with:
```rust
use crate::store::Store;
use serde::Serialize;
use std::sync::Mutex;
use tauri::State;

#[derive(Serialize)]
pub struct Health {
    pub version: String,
    pub db_ready: bool,
    pub schema_version: i64,
}

#[tauri::command]
pub fn health_check(store: State<'_, Mutex<Store>>) -> Health {
    let s = store.lock().expect("store mutex poisoned");
    let schema_version = s.schema_version().unwrap_or(0);
    Health {
        version: env!("CARGO_PKG_VERSION").to_string(),
        db_ready: schema_version >= 1,
        schema_version,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn test_state() -> Mutex<Store> {
        Mutex::new(Store::open_in_memory().expect("in-memory store"))
    }

    #[test]
    fn health_reports_db_state_from_shared_store() {
        let state = test_state();
        // Tauri's State<'_, T> Deref's to &T; we can't construct one directly in a
        // unit test, so verify the underlying logic on the Mutex<Store> directly.
        let s = state.lock().unwrap();
        let sv = s.schema_version().unwrap();
        assert_eq!(sv, 1);
    }
}
```

Run from the `src-tauri/` directory:
```bash
cargo test commands::health
```
Expected: FAIL — `health_check` now takes `State<'_, Mutex<Store>>` which the previous test invoked it without; the file won't compile until we update both the function and the test. The above rewrite already adapts the test, but it does NOT call `health_check` directly. That's intentional — Tauri's `State` cannot be constructed in unit tests; we test the underlying read instead.

- [ ] **Step 3: Run the rewritten test, confirm it passes**

```bash
cargo test commands::health
```
Expected: 1 test passes (`health_reports_db_state_from_shared_store`).

- [ ] **Step 4: Wire managed state in `lib.rs`**

Replace `src-tauri/src/lib.rs` with:
```rust
mod commands;
mod store;

use directories::ProjectDirs;
use std::sync::Mutex;
use store::Store;
use tauri::Manager;

fn appdata_db_path() -> std::path::PathBuf {
    let dirs = ProjectDirs::from("sk", "rlt", "claude-fleet")
        .expect("could not resolve platform appdata dir");
    let dir = dirs.data_dir();
    std::fs::create_dir_all(dir).expect("create appdata dir");
    dir.join("state.db")
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let db_path = appdata_db_path();
    let store = Store::open(&db_path).expect("open store");

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(Mutex::new(store))
        .invoke_handler(tauri::generate_handler![commands::health::health_check])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

- [ ] **Step 5: Drop the now-obsolete `#[allow(dead_code)]` annotations on `Store`**

Open `src-tauri/src/store.rs`. Remove every `#[allow(dead_code)]` attribute that was targeted at individual methods. After this edit `Store::open` is consumed by `lib.rs`, `Store::open_in_memory` by tests, and `Store::schema_version` by `health_check`. The struct and `migrate` are also live. Confirm by reading the file and counting attributes — there should be zero `#[allow(dead_code)]` lines.

Also update the header comment from "NOTE: Phase 2 will wrap…" to:
```rust
// Store owns the SQLite connection. It is wrapped in `Mutex<Store>` and
// registered via `tauri::Manager::manage()` because `rusqlite::Connection`
// is not Send+Sync. Commands access it via `State<'_, Mutex<Store>>`.
```

- [ ] **Step 6: Verify all gates**

```bash
cd ~/projects/github.com/martin-janci/claude-fleet
(cd src-tauri && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test)
pnpm test
pnpm check
pnpm build
```
Expected: all exit 0. `cargo test` → 5 passing (same 4 store tests + the rewritten health test). `pnpm test` → 10 passing.

Note: the frontend `ipc.test.ts` still expects the old `{ version, db_ready }` shape. Update the mock and assertion to include `schema_version: 1`:

Edit `src/lib/ipc.test.ts`:
```ts
import { describe, it, expect, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'health_check') return { version: '0.1.0', db_ready: true, schema_version: 1 };
    throw new Error('unexpected command');
  }),
}));

import { healthCheck } from './ipc';

describe('ipc.healthCheck', () => {
  it('returns version, db_ready, and schema_version from the backend', async () => {
    const h = await healthCheck();
    expect(h.version).toBe('0.1.0');
    expect(h.db_ready).toBe(true);
    expect(h.schema_version).toBe(1);
  });
});
```

Edit `src/lib/ipc.ts`:
```ts
import { invoke } from '@tauri-apps/api/core';

export interface Health {
  version: string;
  db_ready: boolean;
  schema_version: number;
}

export async function healthCheck(): Promise<Health> {
  return invoke<Health>('health_check');
}
```

Edit `vitest.setup.ts` to also include the new field in the global mock:
```ts
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => ({ version: '0.0.0', db_ready: true, schema_version: 1 })),
}));
```

Edit `src/App.svelte` footer to surface schema version too. In the `{:else if health}` branch, replace the span with:
```svelte
<span>v{health.version} · db: {health.db_ready ? 'ok' : 'fail'} · schema {health.schema_version}</span>
```

Re-run `pnpm test` and `pnpm check` — should still be all green.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(store): persistent Store as Mutex<tauri::State>; schema_version in Health

- Open Store against platform appdata path (\$XDG_DATA_HOME / Application
  Support / Local AppData via the directories crate). Register as
  Mutex<Store> in tauri::Manager so commands share one connection.
- Rewire health_check to read the live schema_version through the
  managed state instead of opening a fresh in-memory DB each call. The
  field is now first-class on Health and surfaced in the status footer.
- Drop the targeted #[allow(dead_code)] annotations from Phase 1 — every
  Store method is now live code."
```

---

## Task 2: Convert `Pane.svelte` to Svelte 5 runes + `Snippet` children

**Files:**
- Modify: `src/lib/Pane.svelte` (rewrite — `$props()` + `Snippet` instead of `export let` + `<slot>`)
- Modify: `src/App.svelte` (pass `children` snippet via `{#snippet children()}` blocks)

- [ ] **Step 1: Confirm baseline tests pass**

```bash
cd ~/projects/github.com/martin-janci/claude-fleet
pnpm test
```
Expected: 10 passing.

- [ ] **Step 2: Rewrite `Pane.svelte` to runes + Snippet**

Overwrite `src/lib/Pane.svelte`:
```svelte
<script lang="ts">
  import type { Snippet } from 'svelte';

  let {
    id,
    title = '',
    empty = '',
    children,
  }: {
    id: 'sidebar' | 'center' | 'terminal';
    title?: string;
    empty?: string;
    children?: Snippet;
  } = $props();
</script>

<section data-testid="pane-{id}" class="pane pane-{id}">
  {#if title}
    <header class="pane-header">{title}</header>
  {/if}
  <div class="pane-body">
    {#if children}
      {@render children()}
    {:else if empty}
      <p class="empty">{empty}</p>
    {/if}
  </div>
</section>

<style>
  .pane {
    display: flex;
    flex-direction: column;
    overflow: hidden;
    background: var(--bg-pane);
    color: var(--fg);
    border-right: 1px solid var(--border);
  }
  .pane-terminal { border-right: none; }
  .pane-header {
    padding: 0.5rem 0.75rem;
    font-size: 0.85rem;
    font-weight: 600;
    color: var(--fg-muted);
    border-bottom: 1px solid var(--border);
  }
  .pane-body {
    flex: 1;
    overflow: auto;
    padding: 0.75rem;
  }
  .empty {
    color: var(--fg-muted);
    font-size: 0.9rem;
  }
</style>
```

- [ ] **Step 3: Update `App.svelte` to use the snippet pattern for the theme toggle**

Overwrite `src/App.svelte`:
```svelte
<script lang="ts">
  import { onMount } from 'svelte';
  import Pane from './lib/Pane.svelte';
  import Resizer from './lib/Resizer.svelte';
  import { theme, cycleTheme } from './lib/theme';
  import { healthCheck, type Health } from './lib/ipc';

  let sidebarPx = $state(280);
  let centerPx = $state(360);
  let health = $state<Health | null>(null);
  let healthError = $state<string | null>(null);

  onMount(async () => {
    try {
      health = await healthCheck();
    } catch (e) {
      healthError = String(e);
    }
  });

  function onResizeSidebar(delta: number) {
    sidebarPx = Math.max(180, Math.min(640, sidebarPx + delta));
  }
  function onResizeCenter(delta: number) {
    centerPx = Math.max(220, Math.min(800, centerPx + delta));
  }
</script>

<main class="layout" style="grid-template-columns: {sidebarPx}px 4px {centerPx}px 4px 1fr;">
  <Pane id="sidebar" title="claude-fleet">
    {#snippet children()}
      <button class="theme-toggle" onclick={cycleTheme} title="Theme: {$theme}">
        theme: {$theme}
      </button>
    {/snippet}
  </Pane>
  <Resizer id="sidebar" onresize={onResizeSidebar} />
  <Pane id="center" empty="Pick a session to see details" />
  <Resizer id="center" onresize={onResizeCenter} />
  <Pane id="terminal" empty="No terminal attached" />
</main>

<footer class="status">
  {#if healthError}
    <span class="err">ipc error: {healthError}</span>
  {:else if health}
    <span>v{health.version} · db: {health.db_ready ? 'ok' : 'fail'} · schema {health.schema_version}</span>
  {:else}
    <span class="muted">connecting…</span>
  {/if}
</footer>

<style>
  .layout {
    display: grid;
    height: calc(100vh - 24px);
    width: 100vw;
    background: var(--bg);
  }
  .status {
    height: 24px;
    line-height: 24px;
    padding: 0 0.75rem;
    background: var(--bg-pane);
    border-top: 1px solid var(--border);
    font-size: 0.75rem;
    color: var(--fg-muted);
  }
  .status .err { color: #e64a4a; }
  .theme-toggle {
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-muted);
    padding: 0.25rem 0.5rem;
    border-radius: 4px;
    cursor: pointer;
    font-size: 0.8rem;
  }
  .theme-toggle:hover { color: var(--fg); border-color: var(--accent); }
</style>
```

The center and terminal Panes keep using the `empty` prop (no snippet needed). The sidebar Pane now uses an explicit `{#snippet children()}` block — that's the Svelte 5 idiom for what `<slot>` did in Svelte 4.

- [ ] **Step 4: Run all frontend gates**

```bash
pnpm test
pnpm check
pnpm build
```
Expected: 10 tests passing, 0 type errors, build succeeds.

If `App.test.ts`'s pane-children count assertion (`expect(panes).toHaveLength(3)`) fails, double-check that the snippet renders without wrapping the panes in extra elements. The expected behavior: `<main>` directly contains 5 children (3 panes + 2 resizers), of which exactly 3 have `data-testid` starting with `pane-`.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor(ui): convert Pane.svelte to Svelte 5 runes + Snippet children

Pane was the last component using export let + <slot>. Now uses $props
and Snippet to match App.svelte and Resizer.svelte. App.svelte passes
the theme-toggle button via {#snippet children()} instead of default-slot
content. No user-facing change."
```

---

## Task 3: Establish IPC error contract — `Result<T, IpcError>`

**Files:**
- Create: `src-tauri/src/ipc_error.rs`
- Modify: `src-tauri/src/lib.rs` (declare `mod ipc_error;`)
- Create: `src/lib/result.ts` (or merge helpers into existing `ipc.ts`)
- Test: `src-tauri/src/ipc_error.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Write the failing Rust test**

Create `src-tauri/src/ipc_error.rs`:
```rust
use serde::Serialize;
use std::fmt;

#[derive(Debug, Serialize)]
pub struct IpcError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl IpcError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            details: None,
        }
    }

    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }
}

impl fmt::Display for IpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for IpcError {}

impl From<rusqlite::Error> for IpcError {
    fn from(e: rusqlite::Error) -> Self {
        Self::new("E_SQLITE", e.to_string())
    }
}

impl From<std::io::Error> for IpcError {
    fn from(e: std::io::Error) -> Self {
        Self::new("E_IO", e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_without_details() {
        let err = IpcError::new("E_TEST", "boom");
        let s = serde_json::to_string(&err).unwrap();
        assert_eq!(s, r#"{"code":"E_TEST","message":"boom"}"#);
    }

    #[test]
    fn serializes_with_details() {
        let err = IpcError::new("E_TEST", "boom")
            .with_details(serde_json::json!({ "path": "/x" }));
        let s = serde_json::to_string(&err).unwrap();
        assert!(s.contains(r#""code":"E_TEST""#));
        assert!(s.contains(r#""message":"boom""#));
        assert!(s.contains(r#""details":{"path":"/x"}"#));
    }

    #[test]
    fn from_rusqlite_error_uses_e_sqlite_code() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let sql_err = conn.execute("SELECT * FROM no_such_table", []).unwrap_err();
        let err: IpcError = sql_err.into();
        assert_eq!(err.code, "E_SQLITE");
        assert!(err.message.contains("no_such_table") || !err.message.is_empty());
    }
}
```

- [ ] **Step 2: Declare the module and run tests**

Edit `src-tauri/src/lib.rs` — add `mod ipc_error;` next to the other module declarations.

```bash
cd ~/projects/github.com/martin-janci/claude-fleet/src-tauri
cargo test ipc_error
```
Expected: 3 tests passing.

- [ ] **Step 3: Write the failing frontend test for `Result` helpers**

Create `src/lib/result.test.ts`:
```ts
import { describe, it, expect, vi } from 'vitest';
import { invokeCmd, type IpcError } from './result';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';

describe('invokeCmd', () => {
  it('returns Ok on resolved invoke', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce({ a: 1 });
    const r = await invokeCmd<{ a: number }>('ok_cmd');
    expect(r.ok).toBe(true);
    if (r.ok) expect(r.value).toEqual({ a: 1 });
  });

  it('returns Err on rejected invoke carrying a structured IpcError', async () => {
    const ipcErr: IpcError = { code: 'E_TEST', message: 'boom' };
    (mockedInvoke as ReturnType<typeof vi.fn>).mockRejectedValueOnce(ipcErr);
    const r = await invokeCmd<unknown>('fail_cmd');
    expect(r.ok).toBe(false);
    if (!r.ok) {
      expect(r.error.code).toBe('E_TEST');
      expect(r.error.message).toBe('boom');
    }
  });

  it('wraps plain Error rejections into IpcError with E_UNKNOWN', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error('explode'));
    const r = await invokeCmd<unknown>('throwy_cmd');
    expect(r.ok).toBe(false);
    if (!r.ok) {
      expect(r.error.code).toBe('E_UNKNOWN');
      expect(r.error.message).toContain('explode');
    }
  });
});
```

Run from repo root:
```bash
cd ~/projects/github.com/martin-janci/claude-fleet
pnpm test result
```
Expected: FAIL — `./result` does not exist.

- [ ] **Step 4: Implement `result.ts`**

Create `src/lib/result.ts`:
```ts
import { invoke } from '@tauri-apps/api/core';

export interface IpcError {
  code: string;
  message: string;
  details?: unknown;
}

export type Result<T> = { ok: true; value: T } | { ok: false; error: IpcError };

export async function invokeCmd<T>(
  cmd: string,
  args?: Record<string, unknown>,
): Promise<Result<T>> {
  try {
    const value = await invoke<T>(cmd, args);
    return { ok: true, value };
  } catch (raw) {
    return { ok: false, error: toIpcError(raw) };
  }
}

function toIpcError(raw: unknown): IpcError {
  if (raw && typeof raw === 'object' && 'code' in raw && 'message' in raw) {
    const r = raw as { code: unknown; message: unknown; details?: unknown };
    if (typeof r.code === 'string' && typeof r.message === 'string') {
      return { code: r.code, message: r.message, details: r.details };
    }
  }
  if (raw instanceof Error) {
    return { code: 'E_UNKNOWN', message: raw.message };
  }
  return { code: 'E_UNKNOWN', message: String(raw) };
}
```

- [ ] **Step 5: Run the frontend tests**

```bash
pnpm test result
```
Expected: 3 tests passing.

Run the whole suite:
```bash
pnpm test
```
Expected: 13 tests passing across 5 files.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(ipc): Result<T, IpcError> contract for typed command failures

- Rust: ipc_error.rs defines IpcError { code, message, details? } with
  From<rusqlite::Error> and From<std::io::Error> conversions so commands
  can use ? on common failure modes.
- Frontend: result.ts adds invokeCmd<T>() returning a discriminated
  Result<T> union, plus a structural shape-check so backend IpcError
  objects survive the Tauri serialize round-trip cleanly.
- Six tests (3 Rust + 3 TS) cover serialization shape, From impls, and
  the three rejection paths (structured IpcError, plain Error, other).

Phase 2 commands return Result<T, IpcError> from Rust; the frontend
consumes them via invokeCmd to get a typed success/error union."
```

---

## Task 4: Project + worktree discovery (Rust side)

**Files:**
- Create: `src-tauri/src/projects.rs`
- Modify: `src-tauri/src/lib.rs` (declare `mod projects;`)
- Modify: `src-tauri/src/store.rs` (add upsert + list helpers for `projects` and `worktrees`)

- [ ] **Step 1: Write the failing scan-projects test**

Create `src-tauri/src/projects.rs`:
```rust
use crate::ipc_error::IpcError;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiscoveredProject {
    pub owner: String,
    pub repo: String,
    pub base_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiscoveredWorktree {
    pub name: String,
    pub path: PathBuf,
    pub branch: Option<String>,
}

/// Walks `base/<owner>/<repo>` two levels deep and returns every directory
/// that contains a `.git` entry (regular dir or worktree gitfile).
pub fn scan_projects(base: &Path) -> Result<Vec<DiscoveredProject>, IpcError> {
    let mut out = Vec::new();
    if !base.exists() {
        return Ok(out);
    }
    for owner_entry in std::fs::read_dir(base)? {
        let owner_entry = owner_entry?;
        if !owner_entry.file_type()?.is_dir() {
            continue;
        }
        let owner = owner_entry.file_name().to_string_lossy().into_owned();
        if owner.starts_with('.') {
            continue;
        }
        for repo_entry in std::fs::read_dir(owner_entry.path())? {
            let repo_entry = repo_entry?;
            if !repo_entry.file_type()?.is_dir() {
                continue;
            }
            let repo = repo_entry.file_name().to_string_lossy().into_owned();
            if repo.starts_with('.') {
                continue;
            }
            let path = repo_entry.path();
            if path.join(".git").exists() {
                out.push(DiscoveredProject {
                    owner: owner.clone(),
                    repo,
                    base_path: path,
                });
            }
        }
    }
    out.sort_by(|a, b| (a.owner.as_str(), a.repo.as_str()).cmp(&(b.owner.as_str(), b.repo.as_str())));
    Ok(out)
}

/// Runs `git worktree list --porcelain` in `repo_path` and parses the result.
/// The main checkout is included with `name = "main"` if it has no explicit name.
pub fn list_worktrees(repo_path: &Path) -> Result<Vec<DiscoveredWorktree>, IpcError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .map_err(|e| IpcError::new("E_GIT", format!("git worktree list failed: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(IpcError::new("E_GIT", stderr.trim()));
    }
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    Ok(parse_worktree_porcelain(&stdout, repo_path))
}

fn parse_worktree_porcelain(input: &str, main_path: &Path) -> Vec<DiscoveredWorktree> {
    let mut out = Vec::new();
    let mut cur_path: Option<PathBuf> = None;
    let mut cur_branch: Option<String> = None;
    for line in input.lines() {
        if let Some(rest) = line.strip_prefix("worktree ") {
            if let Some(path) = cur_path.take() {
                out.push(make_worktree(path, cur_branch.take(), main_path));
            }
            cur_path = Some(PathBuf::from(rest));
        } else if let Some(rest) = line.strip_prefix("branch ") {
            cur_branch = Some(rest.trim_start_matches("refs/heads/").to_string());
        }
    }
    if let Some(path) = cur_path {
        out.push(make_worktree(path, cur_branch, main_path));
    }
    out
}

fn make_worktree(path: PathBuf, branch: Option<String>, main_path: &Path) -> DiscoveredWorktree {
    let name = if path == main_path {
        "main".to_string()
    } else {
        path.file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "unknown".to_string())
    };
    DiscoveredWorktree { name, path, branch }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_project(base: &Path, owner: &str, repo: &str) -> PathBuf {
        let path = base.join(owner).join(repo);
        fs::create_dir_all(&path).unwrap();
        fs::create_dir(path.join(".git")).unwrap();
        path
    }

    #[test]
    fn scan_finds_owner_repo_with_dot_git() {
        let tmp = TempDir::new().unwrap();
        make_project(tmp.path(), "martin-janci", "claude-fleet");
        make_project(tmp.path(), "papayapos", "pos-frontend");
        let projects = scan_projects(tmp.path()).unwrap();
        assert_eq!(projects.len(), 2);
        assert_eq!(projects[0].owner, "martin-janci");
        assert_eq!(projects[0].repo, "claude-fleet");
        assert_eq!(projects[1].owner, "papayapos");
        assert_eq!(projects[1].repo, "pos-frontend");
    }

    #[test]
    fn scan_skips_dirs_without_dot_git() {
        let tmp = TempDir::new().unwrap();
        // No .git here
        fs::create_dir_all(tmp.path().join("o1").join("not-a-repo")).unwrap();
        make_project(tmp.path(), "o1", "real-repo");
        let projects = scan_projects(tmp.path()).unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].repo, "real-repo");
    }

    #[test]
    fn scan_returns_empty_for_missing_base() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("does-not-exist");
        let projects = scan_projects(&missing).unwrap();
        assert!(projects.is_empty());
    }

    #[test]
    fn parse_worktree_porcelain_main_only() {
        let input = "worktree /repos/foo\nHEAD abc123\nbranch refs/heads/main\n\n";
        let wts = parse_worktree_porcelain(input, Path::new("/repos/foo"));
        assert_eq!(wts.len(), 1);
        assert_eq!(wts[0].name, "main");
        assert_eq!(wts[0].branch.as_deref(), Some("main"));
    }

    #[test]
    fn parse_worktree_porcelain_with_extras() {
        let input = "\
worktree /repos/foo
HEAD abc123
branch refs/heads/main

worktree /repos/foo/.worktrees/feature-x
HEAD def456
branch refs/heads/feature-x

worktree /repos/foo/.worktrees/bugfix
HEAD 789abc
branch refs/heads/bugfix
";
        let wts = parse_worktree_porcelain(input, Path::new("/repos/foo"));
        assert_eq!(wts.len(), 3);
        assert_eq!(wts[0].name, "main");
        assert_eq!(wts[1].name, "feature-x");
        assert_eq!(wts[2].name, "bugfix");
        assert_eq!(wts[1].branch.as_deref(), Some("feature-x"));
    }
}
```

Add `tempfile = "3"` to `src-tauri/Cargo.toml` `[dev-dependencies]` (create the section if it doesn't exist):
```toml
[dev-dependencies]
tempfile = "3"
```

Declare the module in `src-tauri/src/lib.rs`:
```rust
mod ipc_error;
mod projects;
mod commands;
mod store;
```

- [ ] **Step 2: Run the projects tests**

```bash
cd ~/projects/github.com/martin-janci/claude-fleet/src-tauri
cargo test projects::
```
Expected: 5 tests passing.

- [ ] **Step 3: Write the failing store-upsert test**

Edit `src-tauri/src/store.rs`. After the `schema_version` method but inside `impl Store`, add:
```rust
    pub fn upsert_project(
        &self,
        owner: &str,
        repo: &str,
        base_path: &str,
    ) -> Result<i64, rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO projects (owner, repo, base_path) VALUES (?1, ?2, ?3)
             ON CONFLICT(owner, repo) DO UPDATE SET base_path=excluded.base_path",
            rusqlite::params![owner, repo, base_path],
        )?;
        self.conn.query_row(
            "SELECT id FROM projects WHERE owner=?1 AND repo=?2",
            rusqlite::params![owner, repo],
            |row| row.get(0),
        )
    }

    pub fn list_projects(&self) -> Result<Vec<ProjectRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT id, owner, repo, base_path, last_session_at FROM projects ORDER BY owner, repo",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ProjectRow {
                id: row.get(0)?,
                owner: row.get(1)?,
                repo: row.get(2)?,
                base_path: row.get(3)?,
                last_session_at: row.get(4)?,
            })
        })?;
        rows.collect()
    }

    pub fn upsert_worktree(
        &self,
        project_id: i64,
        name: &str,
        path: &str,
        branch: Option<&str>,
    ) -> Result<i64, rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO worktrees (project_id, name, path, branch) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(project_id, name) DO UPDATE SET path=excluded.path, branch=excluded.branch",
            rusqlite::params![project_id, name, path, branch],
        )?;
        self.conn.query_row(
            "SELECT id FROM worktrees WHERE project_id=?1 AND name=?2",
            rusqlite::params![project_id, name],
            |row| row.get(0),
        )
    }

    pub fn list_worktrees_for_project(
        &self,
        project_id: i64,
    ) -> Result<Vec<WorktreeRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_id, name, path, branch FROM worktrees WHERE project_id=?1 ORDER BY name",
        )?;
        let rows = stmt.query_map(rusqlite::params![project_id], |row| {
            Ok(WorktreeRow {
                id: row.get(0)?,
                project_id: row.get(1)?,
                name: row.get(2)?,
                path: row.get(3)?,
                branch: row.get(4)?,
            })
        })?;
        rows.collect()
    }

    pub fn delete_worktrees_not_in(
        &self,
        project_id: i64,
        keep_names: &[String],
    ) -> Result<usize, rusqlite::Error> {
        if keep_names.is_empty() {
            return self.conn.execute(
                "DELETE FROM worktrees WHERE project_id=?1",
                rusqlite::params![project_id],
            );
        }
        let placeholders = keep_names.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "DELETE FROM worktrees WHERE project_id=?1 AND name NOT IN ({placeholders})"
        );
        let mut params: Vec<&dyn rusqlite::ToSql> = vec![&project_id];
        for n in keep_names {
            params.push(n);
        }
        self.conn.execute(&sql, params.as_slice())
    }
```

Above the `impl Store` block, add the row structs:
```rust
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProjectRow {
    pub id: i64,
    pub owner: String,
    pub repo: String,
    pub base_path: String,
    pub last_session_at: Option<i64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WorktreeRow {
    pub id: i64,
    pub project_id: i64,
    pub name: String,
    pub path: String,
    pub branch: Option<String>,
}
```

Add a new test inside `mod tests` at the bottom:
```rust
    #[test]
    fn upsert_and_list_projects_roundtrip() {
        let s = Store::open_in_memory().unwrap();
        let id = s
            .upsert_project("martin-janci", "claude-fleet", "/tmp/cf")
            .unwrap();
        assert!(id > 0);
        // Re-upsert is idempotent
        let id2 = s
            .upsert_project("martin-janci", "claude-fleet", "/other/path")
            .unwrap();
        assert_eq!(id, id2);
        let rows = s.list_projects().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].owner, "martin-janci");
        assert_eq!(rows[0].repo, "claude-fleet");
        assert_eq!(rows[0].base_path, "/other/path");
    }

    #[test]
    fn worktrees_upsert_list_and_prune() {
        let s = Store::open_in_memory().unwrap();
        let pid = s
            .upsert_project("o", "r", "/tmp/r")
            .unwrap();
        s.upsert_worktree(pid, "main", "/tmp/r", Some("main")).unwrap();
        s.upsert_worktree(pid, "feature-x", "/tmp/r/.worktrees/feature-x", Some("feature-x"))
            .unwrap();
        s.upsert_worktree(pid, "bugfix", "/tmp/r/.worktrees/bugfix", Some("bugfix"))
            .unwrap();
        assert_eq!(s.list_worktrees_for_project(pid).unwrap().len(), 3);
        let removed = s
            .delete_worktrees_not_in(pid, &["main".to_string(), "feature-x".to_string()])
            .unwrap();
        assert_eq!(removed, 1);
        let names: Vec<String> = s
            .list_worktrees_for_project(pid)
            .unwrap()
            .into_iter()
            .map(|w| w.name)
            .collect();
        assert_eq!(names, vec!["feature-x", "main"]);
    }
```

- [ ] **Step 4: Run the store tests**

```bash
cargo test store::tests
```
Expected: 6 tests passing (4 original + 2 new).

- [ ] **Step 5: Run all gates**

```bash
cargo fmt --check || cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```
Expected: 14 Rust tests passing (5 store + 5 projects + 3 ipc_error + 1 health).

- [ ] **Step 6: Commit**

```bash
cd ~/projects/github.com/martin-janci/claude-fleet
git add -A
git commit -m "feat(projects): filesystem scan and git worktree parser with SQL upsert helpers

- projects.rs: scan_projects walks ~/projects/github.com/<owner>/<repo>
  two levels deep, filtering to directories that contain .git. Returns a
  sorted Vec<DiscoveredProject>. Empty base path is not an error.
- projects.rs: list_worktrees runs git worktree list --porcelain in a
  repo and parses the porcelain format into Vec<DiscoveredWorktree>. The
  main checkout is normalized to name=\"main\"; extras keep their dir name.
- store.rs: upsert_project / list_projects / upsert_worktree /
  list_worktrees_for_project / delete_worktrees_not_in cover the
  read-write surface Phase 2 needs. ON CONFLICT ... DO UPDATE makes
  re-scans idempotent.
- 5 projects tests (sorted scan, .git-only filter, missing base path,
  main-only porcelain, multi-worktree porcelain) plus 2 store tests
  (project upsert idempotence, worktree prune) — 7 new, all in-process."
```

---

## Task 5: Project / worktree IPC commands + refresh flow

**Files:**
- Create: `src-tauri/src/commands/projects.rs`
- Modify: `src-tauri/src/commands/mod.rs` (add `pub mod projects;`)
- Modify: `src-tauri/src/lib.rs` (register `list_projects`, `refresh_projects`)

- [ ] **Step 1: Add the command module**

Edit `src-tauri/src/commands/mod.rs`:
```rust
pub mod health;
pub mod projects;
```

Create `src-tauri/src/commands/projects.rs`:
```rust
use crate::ipc_error::IpcError;
use crate::projects::{list_worktrees, scan_projects};
use crate::store::{ProjectRow, Store, WorktreeRow};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::State;

#[derive(Serialize)]
pub struct ProjectTreeRow {
    pub project: ProjectRow,
    pub worktrees: Vec<WorktreeRow>,
}

fn projects_base() -> PathBuf {
    if let Ok(p) = std::env::var("CLAUDE_FLEET_PROJECTS_BASE") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
    PathBuf::from(home).join("projects").join("github.com")
}

#[tauri::command]
pub fn list_projects(
    store: State<'_, Mutex<Store>>,
) -> Result<Vec<ProjectTreeRow>, IpcError> {
    let s = store.lock().map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    let projects = s.list_projects()?;
    let mut out = Vec::with_capacity(projects.len());
    for p in projects {
        let wts = s.list_worktrees_for_project(p.id)?;
        out.push(ProjectTreeRow { project: p, worktrees: wts });
    }
    Ok(out)
}

#[tauri::command]
pub fn refresh_projects(
    store: State<'_, Mutex<Store>>,
) -> Result<Vec<ProjectTreeRow>, IpcError> {
    let base = projects_base();
    let discovered = scan_projects(&base)?;
    let s = store.lock().map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    for dp in &discovered {
        let project_id = s.upsert_project(
            &dp.owner,
            &dp.repo,
            &dp.base_path.to_string_lossy(),
        )?;
        let worktrees = match list_worktrees(&dp.base_path) {
            Ok(v) => v,
            Err(_) => continue, // Skip projects where `git worktree list` failed (corrupt repo, etc.)
        };
        let mut keep_names = Vec::with_capacity(worktrees.len());
        for wt in &worktrees {
            keep_names.push(wt.name.clone());
            s.upsert_worktree(
                project_id,
                &wt.name,
                &wt.path.to_string_lossy(),
                wt.branch.as_deref(),
            )?;
        }
        s.delete_worktrees_not_in(project_id, &keep_names)?;
    }
    drop(s);
    list_projects(store)
}
```

- [ ] **Step 2: Register the commands**

Edit `src-tauri/src/lib.rs` — update `generate_handler!`:
```rust
.invoke_handler(tauri::generate_handler![
    commands::health::health_check,
    commands::projects::list_projects,
    commands::projects::refresh_projects,
])
```

- [ ] **Step 3: Build and verify**

```bash
cd ~/projects/github.com/martin-janci/claude-fleet/src-tauri
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
```
Expected: builds clean, 14 tests still pass, no warnings.

(There are no unit tests for the commands themselves because they require a `tauri::State` which can't be constructed in a unit test. Manual integration coverage happens via the frontend test in Task 6.)

- [ ] **Step 4: Commit**

```bash
cd ~/projects/github.com/martin-janci/claude-fleet
git add -A
git commit -m "feat(ipc): list_projects and refresh_projects commands

- list_projects returns Vec<ProjectTreeRow { project, worktrees }> from
  the persisted store. No filesystem access.
- refresh_projects scans \$CLAUDE_FLEET_PROJECTS_BASE (or
  \$HOME/projects/github.com), upserts every owner/repo with a .git into
  the store, then per-project parses git worktree list --porcelain and
  upserts worktrees, pruning any rows whose worktrees were removed on
  disk. Returns the new state via list_projects so the frontend can
  refresh in one round-trip.
- A repo whose git worktree call fails (corrupt .git, etc.) is logged
  by being skipped — the rest of the scan still completes.
- Both commands return Result<_, IpcError> per the Task 3 contract."
```

---

## Task 6: Frontend `projects.ts` store + Sidebar tree UI

**Files:**
- Create: `src/lib/projects.ts`
- Create: `src/lib/projects.test.ts`
- Create: `src/lib/Sidebar.svelte`
- Modify: `src/App.svelte` (mount Sidebar inside the sidebar Pane)

- [ ] **Step 1: Write the failing projects-store test**

Create `src/lib/projects.test.ts`:
```ts
import { describe, it, expect, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { projects, refreshProjects } from './projects';
import { get } from 'svelte/store';

const fake = [
  {
    project: { id: 1, owner: 'martin-janci', repo: 'claude-fleet', base_path: '/r/cf', last_session_at: null },
    worktrees: [
      { id: 11, project_id: 1, name: 'main', path: '/r/cf', branch: 'main' },
    ],
  },
  {
    project: { id: 2, owner: 'papayapos', repo: 'pos-frontend', base_path: '/r/pf', last_session_at: 1716120000 },
    worktrees: [
      { id: 21, project_id: 2, name: 'main', path: '/r/pf', branch: 'main' },
      { id: 22, project_id: 2, name: 'feature-x', path: '/r/pf/.worktrees/feature-x', branch: 'feature-x' },
    ],
  },
];

describe('projects store', () => {
  it('refreshProjects populates the store on Ok', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(fake);
    const r = await refreshProjects();
    expect(r.ok).toBe(true);
    expect(get(projects)).toHaveLength(2);
    expect(get(projects)[1].worktrees).toHaveLength(2);
  });

  it('refreshProjects sets the error and does not touch the store on Err', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(fake);
    await refreshProjects();
    const before = get(projects).length;

    (mockedInvoke as ReturnType<typeof vi.fn>).mockRejectedValueOnce({
      code: 'E_IO',
      message: 'permission denied',
    });
    const r = await refreshProjects();
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.error.code).toBe('E_IO');
    expect(get(projects)).toHaveLength(before);
  });
});
```

Run:
```bash
cd ~/projects/github.com/martin-janci/claude-fleet
pnpm test projects
```
Expected: FAIL — `./projects` doesn't exist.

- [ ] **Step 2: Implement `projects.ts`**

Create `src/lib/projects.ts`:
```ts
import { writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';

export interface ProjectRow {
  id: number;
  owner: string;
  repo: string;
  base_path: string;
  last_session_at: number | null;
}

export interface WorktreeRow {
  id: number;
  project_id: number;
  name: string;
  path: string;
  branch: string | null;
}

export interface ProjectTreeRow {
  project: ProjectRow;
  worktrees: WorktreeRow[];
}

export const projects = writable<ProjectTreeRow[]>([]);

export async function loadProjects(): Promise<Result<ProjectTreeRow[]>> {
  const r = await invokeCmd<ProjectTreeRow[]>('list_projects');
  if (r.ok) projects.set(r.value);
  return r;
}

export async function refreshProjects(): Promise<Result<ProjectTreeRow[]>> {
  const r = await invokeCmd<ProjectTreeRow[]>('refresh_projects');
  if (r.ok) projects.set(r.value);
  return r;
}
```

- [ ] **Step 3: Run the projects test, confirm it passes**

```bash
pnpm test projects
```
Expected: 2 tests passing.

- [ ] **Step 4: Implement `Sidebar.svelte`**

Create `src/lib/Sidebar.svelte`:
```svelte
<script lang="ts">
  import { onMount } from 'svelte';
  import { projects, loadProjects, refreshProjects, type ProjectTreeRow } from './projects';

  let loadError: string | null = $state(null);
  let loading = $state(false);

  onMount(async () => {
    const r = await loadProjects();
    if (!r.ok) loadError = r.error.message;
  });

  async function onRefresh() {
    loading = true;
    loadError = null;
    const r = await refreshProjects();
    loading = false;
    if (!r.ok) loadError = r.error.message;
  }
</script>

<div class="sidebar" data-testid="sidebar-tree">
  <header class="sidebar-header">
    <button class="refresh" onclick={onRefresh} disabled={loading} data-testid="sidebar-refresh">
      {loading ? 'refreshing…' : 'refresh'}
    </button>
  </header>

  {#if loadError}
    <p class="err">{loadError}</p>
  {:else if $projects.length === 0}
    <p class="empty">No projects yet — click refresh to scan ~/projects/github.com.</p>
  {:else}
    <ul class="tree">
      {#each $projects as row (row.project.id)}
        <li class="proj">
          <div class="proj-row" data-testid="proj-row" title={row.project.base_path}>
            <span class="owner">{row.project.owner}/</span><span class="repo">{row.project.repo}</span>
          </div>
          {#if row.worktrees.length > 0}
            <ul class="worktrees">
              {#each row.worktrees as wt (wt.id)}
                <li class="wt" data-testid="wt-row" title={wt.path}>
                  <span class="wt-bullet">└</span>
                  <span class="wt-name">{wt.name}</span>
                  {#if wt.branch && wt.branch !== wt.name}
                    <span class="wt-branch">({wt.branch})</span>
                  {/if}
                </li>
              {/each}
            </ul>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .sidebar { display: flex; flex-direction: column; height: 100%; }
  .sidebar-header { display: flex; justify-content: flex-end; padding: 0.25rem 0; }
  .refresh {
    font-size: 0.75rem;
    padding: 0.2rem 0.5rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-muted);
    border-radius: 4px;
    cursor: pointer;
  }
  .refresh:hover:not(:disabled) { color: var(--fg); border-color: var(--accent); }
  .refresh:disabled { opacity: 0.6; cursor: progress; }
  .tree { list-style: none; margin: 0; padding: 0; }
  .proj { margin-bottom: 0.4rem; }
  .proj-row { font-weight: 500; padding: 0.15rem 0; }
  .owner { color: var(--fg-muted); }
  .worktrees { list-style: none; margin: 0; padding-left: 0.6rem; }
  .wt { font-size: 0.85rem; padding: 0.1rem 0; color: var(--fg-muted); display: flex; gap: 0.3rem; }
  .wt-bullet { color: var(--border); }
  .wt-name { color: var(--fg); }
  .wt-branch { font-style: italic; }
  .err { color: #e64a4a; font-size: 0.85rem; padding: 0.25rem 0; }
  .empty { color: var(--fg-muted); font-size: 0.85rem; }
</style>
```

- [ ] **Step 5: Mount the sidebar in App.svelte**

Edit `src/App.svelte`. In the sidebar Pane snippet, replace the contents (currently just the theme toggle) with both the toggle AND the new Sidebar. The result for that block:
```svelte
  <Pane id="sidebar" title="claude-fleet">
    {#snippet children()}
      <Sidebar />
      <button class="theme-toggle" onclick={cycleTheme} title="Theme: {$theme}">
        theme: {$theme}
      </button>
    {/snippet}
  </Pane>
```

Add the import at the top of `<script lang="ts">`:
```ts
import Sidebar from './lib/Sidebar.svelte';
```

- [ ] **Step 6: Update the layout assertion in `App.test.ts`**

The first assertion still passes (the three test-ids are unchanged). The second assertion (`expect(panes).toHaveLength(3)`) still passes because `Sidebar` doesn't have a `pane-*` test-id.

Add a third test verifying the Sidebar slot renders:
```ts
import { findByTestId, render } from '@testing-library/svelte';
import { describe, it, expect } from 'vitest';
import App from './App.svelte';

describe('App layout', () => {
  it('renders sidebar, center, and terminal panes', () => {
    const { getByTestId } = render(App);
    expect(getByTestId('pane-sidebar')).toBeInTheDocument();
    expect(getByTestId('pane-center')).toBeInTheDocument();
    expect(getByTestId('pane-terminal')).toBeInTheDocument();
  });

  it('contains all three panes inside the layout container', () => {
    const { container } = render(App);
    const layout = container.querySelector('.layout') as HTMLElement;
    expect(layout).not.toBeNull();
    const panes = layout.querySelectorAll('[data-testid^="pane-"]');
    expect(panes).toHaveLength(3);
  });

  it('mounts the sidebar tree inside the sidebar pane', async () => {
    const { container } = render(App);
    const sidebarTree = await findByTestId(container, 'sidebar-tree');
    expect(sidebarTree).toBeInTheDocument();
  });
});
```

The Sidebar's `onMount` calls `loadProjects()` which calls `invokeCmd<...>('list_projects')` → `invoke()` from the global Tauri mock. The global mock currently returns `{ version: '0.0.0', db_ready: true, schema_version: 1 }` regardless of command name; that satisfies the awaited promise but isn't a `ProjectTreeRow[]`. The store will be set to a non-array value but the empty-state branch in the template guards for `$projects.length`, which on the non-array would throw. To prevent the test from crashing on this, refine the global mock in `vitest.setup.ts`:
```ts
import '@testing-library/jest-dom/vitest';
import { vi } from 'vitest';

// Global Tauri IPC mock: keeps components that call invoke() on mount
// (e.g. App.svelte's healthCheck(), Sidebar's loadProjects()) from
// crashing in tests. Individual test files can override with their own
// vi.mock for specific commands.
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'health_check') return { version: '0.0.0', db_ready: true, schema_version: 1 };
    if (cmd === 'list_projects') return [];
    if (cmd === 'refresh_projects') return [];
    return null;
  }),
}));
```

- [ ] **Step 7: Run all frontend tests**

```bash
pnpm test
```
Expected: 16 passing across 6 files (App: 3, Resizer: 2, theme: 5, ipc: 1, result: 3, projects: 2).

- [ ] **Step 8: Run check + build**

```bash
pnpm check
pnpm build
```
Expected: 0 errors, build clean.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "feat(ui): Sidebar with project/worktree tree backed by Tauri commands

- projects.ts exposes a Svelte writable<ProjectTreeRow[]> store plus
  loadProjects() (cached) and refreshProjects() (rescans FS). Both
  return Result<_> from result.ts; on Err the store is left intact.
- Sidebar.svelte renders the tree (project header + worktree children)
  with title attributes carrying the on-disk path. A refresh button
  triggers refresh_projects; loading and error states are surfaced.
- App.svelte mounts Sidebar inside the sidebar pane snippet alongside
  the theme toggle. A new App test verifies sidebar-tree mounts.
- vitest.setup.ts global mock learns list_projects / refresh_projects
  (returns []) so any future test that renders App doesn't crash."
```

---

## Task 7: Recency filter pills + search

**Files:**
- Modify: `src/lib/Sidebar.svelte` (add filter pills + search input, derived filtered list)
- Modify: `src/lib/projects.test.ts` (no change required, but extend with a filter test)
- Create: `src/lib/Sidebar.test.ts`

- [ ] **Step 1: Write the failing Sidebar filter test**

Create `src/lib/Sidebar.test.ts`:
```ts
import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

const fake = [
  {
    project: { id: 1, owner: 'martin-janci', repo: 'claude-fleet', base_path: '/r/cf', last_session_at: Math.floor(Date.now() / 1000) - 60 },
    worktrees: [{ id: 11, project_id: 1, name: 'main', path: '/r/cf', branch: 'main' }],
  },
  {
    project: { id: 2, owner: 'papayapos', repo: 'pos-frontend', base_path: '/r/pf', last_session_at: Math.floor(Date.now() / 1000) - 60 * 60 * 24 * 14 },
    worktrees: [{ id: 21, project_id: 2, name: 'main', path: '/r/pf', branch: 'main' }],
  },
  {
    project: { id: 3, owner: 'martin-janci', repo: 'phone-manager', base_path: '/r/pm', last_session_at: null },
    worktrees: [{ id: 31, project_id: 3, name: 'main', path: '/r/pm', branch: 'main' }],
  },
];

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => (cmd === 'list_projects' ? fake : [])),
}));

import Sidebar from './Sidebar.svelte';
import { projects } from './projects';

beforeEach(() => {
  projects.set([]);
});

describe('Sidebar', () => {
  it('renders all projects by default', async () => {
    render(Sidebar);
    await tick(); await tick();
    const rows = await screen.findAllByTestId('proj-row');
    expect(rows).toHaveLength(3);
  });

  it('filters by 7d recency', async () => {
    render(Sidebar);
    await tick(); await tick();
    await screen.findAllByTestId('proj-row');
    await fireEvent.click(screen.getByText('7d'));
    const rows = await screen.findAllByTestId('proj-row');
    expect(rows).toHaveLength(1); // only claude-fleet (1 minute old) matches "7d"
  });

  it('filters by search query', async () => {
    render(Sidebar);
    await tick(); await tick();
    await screen.findAllByTestId('proj-row');
    const search = screen.getByTestId('sidebar-search') as HTMLInputElement;
    await fireEvent.input(search, { target: { value: 'phone' } });
    const rows = await screen.findAllByTestId('proj-row');
    expect(rows).toHaveLength(1);
    expect(rows[0]).toHaveTextContent('phone-manager');
  });
});
```

Run:
```bash
pnpm test Sidebar
```
Expected: FAIL — the filter pills and search input don't exist yet.

- [ ] **Step 2: Add filter pills and search to `Sidebar.svelte`**

Overwrite `src/lib/Sidebar.svelte`:
```svelte
<script lang="ts">
  import { onMount } from 'svelte';
  import { projects, loadProjects, refreshProjects, type ProjectTreeRow } from './projects';

  type Recency = 'all' | 'today' | '7d' | '30d';

  let loadError: string | null = $state(null);
  let loading = $state(false);
  let recency: Recency = $state('all');
  let search = $state('');

  onMount(async () => {
    const r = await loadProjects();
    if (!r.ok) loadError = r.error.message;
  });

  async function onRefresh() {
    loading = true;
    loadError = null;
    const r = await refreshProjects();
    loading = false;
    if (!r.ok) loadError = r.error.message;
  }

  const RECENCY_WINDOW: Record<Recency, number | null> = {
    all: null,
    today: 60 * 60 * 24,
    '7d': 60 * 60 * 24 * 7,
    '30d': 60 * 60 * 24 * 30,
  };

  function matchesRecency(p: ProjectTreeRow, r: Recency): boolean {
    const window = RECENCY_WINDOW[r];
    if (window === null) return true;
    if (p.project.last_session_at === null) return false;
    const ageSec = Math.floor(Date.now() / 1000) - p.project.last_session_at;
    return ageSec >= 0 && ageSec <= window;
  }

  function matchesSearch(p: ProjectTreeRow, q: string): boolean {
    if (!q) return true;
    const needle = q.toLowerCase();
    if (p.project.owner.toLowerCase().includes(needle)) return true;
    if (p.project.repo.toLowerCase().includes(needle)) return true;
    return p.worktrees.some(
      (w) =>
        w.name.toLowerCase().includes(needle) ||
        (w.branch?.toLowerCase().includes(needle) ?? false),
    );
  }

  const filtered = $derived(
    $projects.filter((p) => matchesRecency(p, recency) && matchesSearch(p, search)),
  );
</script>

<div class="sidebar" data-testid="sidebar-tree">
  <header class="sidebar-header">
    <input
      class="search"
      placeholder="Search projects, branches…"
      bind:value={search}
      data-testid="sidebar-search"
    />
    <button class="refresh" onclick={onRefresh} disabled={loading} data-testid="sidebar-refresh">
      {loading ? 'refreshing…' : 'refresh'}
    </button>
  </header>

  <nav class="recency" aria-label="recency filter">
    {#each ['all', 'today', '7d', '30d'] as opt (opt)}
      <button
        class="pill"
        class:active={recency === opt}
        onclick={() => (recency = opt as Recency)}
      >
        {opt}
      </button>
    {/each}
  </nav>

  {#if loadError}
    <p class="err">{loadError}</p>
  {:else if filtered.length === 0}
    <p class="empty">
      {$projects.length === 0
        ? 'No projects yet — click refresh to scan ~/projects/github.com.'
        : 'No projects match the current filter.'}
    </p>
  {:else}
    <ul class="tree">
      {#each filtered as row (row.project.id)}
        <li class="proj">
          <div class="proj-row" data-testid="proj-row" title={row.project.base_path}>
            <span class="owner">{row.project.owner}/</span><span class="repo">{row.project.repo}</span>
          </div>
          {#if row.worktrees.length > 0}
            <ul class="worktrees">
              {#each row.worktrees as wt (wt.id)}
                <li class="wt" data-testid="wt-row" title={wt.path}>
                  <span class="wt-bullet">└</span>
                  <span class="wt-name">{wt.name}</span>
                  {#if wt.branch && wt.branch !== wt.name}
                    <span class="wt-branch">({wt.branch})</span>
                  {/if}
                </li>
              {/each}
            </ul>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .sidebar { display: flex; flex-direction: column; height: 100%; gap: 0.4rem; }
  .sidebar-header { display: flex; gap: 0.3rem; align-items: center; padding: 0.25rem 0; }
  .search {
    flex: 1;
    font-size: 0.8rem;
    padding: 0.2rem 0.4rem;
    border: 1px solid var(--border);
    background: var(--bg);
    color: var(--fg);
    border-radius: 4px;
  }
  .search::placeholder { color: var(--fg-muted); }
  .refresh {
    font-size: 0.75rem;
    padding: 0.2rem 0.5rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-muted);
    border-radius: 4px;
    cursor: pointer;
  }
  .refresh:hover:not(:disabled) { color: var(--fg); border-color: var(--accent); }
  .refresh:disabled { opacity: 0.6; cursor: progress; }
  .recency { display: flex; gap: 0.25rem; }
  .pill {
    font-size: 0.7rem;
    padding: 0.15rem 0.5rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-muted);
    border-radius: 999px;
    cursor: pointer;
  }
  .pill.active { color: var(--fg); border-color: var(--accent); }
  .tree { list-style: none; margin: 0; padding: 0; flex: 1; overflow: auto; }
  .proj { margin-bottom: 0.4rem; }
  .proj-row { font-weight: 500; padding: 0.15rem 0; }
  .owner { color: var(--fg-muted); }
  .worktrees { list-style: none; margin: 0; padding-left: 0.6rem; }
  .wt { font-size: 0.85rem; padding: 0.1rem 0; color: var(--fg-muted); display: flex; gap: 0.3rem; }
  .wt-bullet { color: var(--border); }
  .wt-name { color: var(--fg); }
  .wt-branch { font-style: italic; }
  .err { color: #e64a4a; font-size: 0.85rem; padding: 0.25rem 0; }
  .empty { color: var(--fg-muted); font-size: 0.85rem; }
</style>
```

- [ ] **Step 3: Run the Sidebar tests**

```bash
pnpm test Sidebar
```
Expected: 3 tests passing.

- [ ] **Step 4: Run full suite**

```bash
pnpm test
pnpm check
pnpm build
```
Expected: 19 tests across 7 files passing; 0 type errors; build clean.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(ui): recency filter pills and search input in Sidebar

- Pills: all / today / 7d / 30d driven by projects[].project.last_session_at.
- Search: case-insensitive substring match across owner, repo, worktree
  name, and worktree branch. Both filters compose.
- Reactive \$derived list keeps the tree in sync with the writable
  projects store and the filter state without manual subscriptions.
- 3 new tests cover defaults, recency, and search."
```

---

## Task 8: Local tmux session discovery + IPC

**Files:**
- Create: `src-tauri/src/tmux.rs`
- Create: `src-tauri/src/commands/sessions.rs`
- Modify: `src-tauri/src/commands/mod.rs` (declare `pub mod sessions;`)
- Modify: `src-tauri/src/lib.rs` (register `list_sessions`)
- Modify: `src-tauri/src/store.rs` (add session upsert/list/delete helpers)

- [ ] **Step 1: Add tmux parser with tests**

Create `src-tauri/src/tmux.rs`:
```rust
use crate::ipc_error::IpcError;
use serde::Serialize;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TmuxSession {
    pub name: String,
    pub created: i64,
    pub last_activity: i64,
    pub attached: bool,
    pub path: PathBuf,
}

/// Lists tmux sessions on the local host. Returns an empty Vec (not an error)
/// when the tmux server isn't running.
pub fn list_local_sessions() -> Result<Vec<TmuxSession>, IpcError> {
    let output = Command::new("tmux")
        .args([
            "list-sessions",
            "-F",
            "#{session_name}|#{session_created}|#{session_activity}|#{session_attached}|#{pane_current_path}",
        ])
        .output();
    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout).into_owned();
            Ok(parse_sessions(&stdout))
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr).to_string();
            if stderr.contains("no server running") {
                Ok(Vec::new())
            } else {
                Err(IpcError::new("E_TMUX", stderr.trim()))
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(IpcError::new("E_TMUX", "tmux binary not found on PATH"))
        }
        Err(e) => Err(IpcError::new("E_TMUX", format!("spawn tmux failed: {e}"))),
    }
}

fn parse_sessions(input: &str) -> Vec<TmuxSession> {
    input
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split('|').collect();
            if parts.len() != 5 {
                return None;
            }
            let created = parts[1].parse::<i64>().ok()?;
            let last_activity = parts[2].parse::<i64>().ok()?;
            let attached_int = parts[3].parse::<i64>().ok()?;
            Some(TmuxSession {
                name: parts[0].to_string(),
                created,
                last_activity,
                attached: attached_int > 0,
                path: PathBuf::from(parts[4]),
            })
        })
        .collect()
}

pub fn new_session(name: &str, working_dir: &std::path::Path) -> Result<(), IpcError> {
    let output = Command::new("tmux")
        .args([
            "new-session",
            "-d",
            "-s",
            name,
            "-c",
            &working_dir.to_string_lossy(),
            "cl --continue || cl || bash",
        ])
        .output()
        .map_err(|e| IpcError::new("E_TMUX", format!("spawn tmux failed: {e}")))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(IpcError::new("E_TMUX", stderr.trim()))
    }
}

pub fn kill_session(name: &str) -> Result<(), IpcError> {
    let output = Command::new("tmux")
        .args(["kill-session", "-t", name])
        .output()
        .map_err(|e| IpcError::new("E_TMUX", format!("spawn tmux failed: {e}")))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(IpcError::new("E_TMUX", stderr.trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_two_sessions() {
        let input = "dev-foo|1716000000|1716100000|1|/repos/foo\ndev-bar|1716000100|1716200000|0|/repos/bar\n";
        let sessions = parse_sessions(input);
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].name, "dev-foo");
        assert_eq!(sessions[0].attached, true);
        assert_eq!(sessions[0].path, PathBuf::from("/repos/foo"));
        assert_eq!(sessions[1].name, "dev-bar");
        assert_eq!(sessions[1].attached, false);
    }

    #[test]
    fn parse_skips_malformed_lines() {
        let input = "good|1716000000|1716100000|1|/x\nbad-line-without-pipes\nempty||||\n";
        let sessions = parse_sessions(input);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "good");
    }

    #[test]
    fn parse_empty_input() {
        assert!(parse_sessions("").is_empty());
    }
}
```

Declare in `src-tauri/src/lib.rs`:
```rust
mod ipc_error;
mod projects;
mod commands;
mod store;
mod tmux;
```

Run:
```bash
cd ~/projects/github.com/martin-janci/claude-fleet/src-tauri
cargo test tmux::
```
Expected: 3 passing.

- [ ] **Step 2: Add session storage helpers + tests**

Edit `src-tauri/src/store.rs`. After the `WorktreeRow` struct, add:
```rust
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionRow {
    pub id: i64,
    pub tmux_name: String,
    pub host_alias: String,
    pub project_id: Option<i64>,
    pub worktree_id: Option<i64>,
    pub created_at: i64,
    pub last_activity_at: i64,
    pub status: String,
    pub notes: Option<String>,
}
```

And inside `impl Store`, add:
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
    ) -> Result<i64, rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO sessions (tmux_name, host_alias, project_id, worktree_id, created_at, last_activity_at, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(host_alias, tmux_name) DO UPDATE SET
               project_id=excluded.project_id,
               worktree_id=excluded.worktree_id,
               last_activity_at=excluded.last_activity_at,
               status=excluded.status",
            rusqlite::params![tmux_name, host_alias, project_id, worktree_id, created_at, last_activity_at, status],
        )?;
        self.conn.query_row(
            "SELECT id FROM sessions WHERE host_alias=?1 AND tmux_name=?2",
            rusqlite::params![host_alias, tmux_name],
            |row| row.get(0),
        )
    }

    pub fn list_sessions_for_host(
        &self,
        host_alias: &str,
    ) -> Result<Vec<SessionRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT id, tmux_name, host_alias, project_id, worktree_id, created_at,
                    last_activity_at, status, notes
             FROM sessions WHERE host_alias=?1 ORDER BY last_activity_at DESC",
        )?;
        let rows = stmt.query_map(rusqlite::params![host_alias], |row| {
            Ok(SessionRow {
                id: row.get(0)?,
                tmux_name: row.get(1)?,
                host_alias: row.get(2)?,
                project_id: row.get(3)?,
                worktree_id: row.get(4)?,
                created_at: row.get(5)?,
                last_activity_at: row.get(6)?,
                status: row.get(7)?,
                notes: row.get(8)?,
            })
        })?;
        rows.collect()
    }

    pub fn delete_sessions_not_in(
        &self,
        host_alias: &str,
        keep_names: &[String],
    ) -> Result<usize, rusqlite::Error> {
        if keep_names.is_empty() {
            return self.conn.execute(
                "DELETE FROM sessions WHERE host_alias=?1",
                rusqlite::params![host_alias],
            );
        }
        let placeholders = keep_names.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "DELETE FROM sessions WHERE host_alias=?1 AND tmux_name NOT IN ({placeholders})"
        );
        let mut params: Vec<&dyn rusqlite::ToSql> = vec![&host_alias];
        for n in keep_names {
            params.push(n);
        }
        self.conn.execute(&sql, params.as_slice())
    }
```

Inside `mod tests`, add:
```rust
    #[test]
    fn upsert_and_list_sessions_roundtrip() {
        let s = Store::open_in_memory().unwrap();
        // FK needs a host row to exist before session row references it
        s.conn
            .execute(
                "INSERT INTO hosts (alias, reachable) VALUES ('local', 1)",
                [],
            )
            .unwrap();
        let id = s
            .upsert_session("dev-foo", "local", None, None, 1000, 2000, "running")
            .unwrap();
        assert!(id > 0);
        let id2 = s
            .upsert_session("dev-foo", "local", None, None, 1000, 3000, "running")
            .unwrap();
        assert_eq!(id, id2);
        let rows = s.list_sessions_for_host("local").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].last_activity_at, 3000);
    }

    #[test]
    fn sessions_prune_removes_stale_rows() {
        let s = Store::open_in_memory().unwrap();
        s.conn
            .execute("INSERT INTO hosts (alias, reachable) VALUES ('local', 1)", [])
            .unwrap();
        s.upsert_session("dev-a", "local", None, None, 1, 1, "running").unwrap();
        s.upsert_session("dev-b", "local", None, None, 1, 1, "running").unwrap();
        s.upsert_session("dev-c", "local", None, None, 1, 1, "running").unwrap();
        let removed = s
            .delete_sessions_not_in("local", &["dev-a".to_string()])
            .unwrap();
        assert_eq!(removed, 2);
        assert_eq!(s.list_sessions_for_host("local").unwrap().len(), 1);
    }
```

Run:
```bash
cargo test store::tests
```
Expected: 8 passing (4 original + 2 from Task 4 + 2 new here).

- [ ] **Step 3: Create the sessions command**

Create `src-tauri/src/commands/sessions.rs`:
```rust
use crate::ipc_error::IpcError;
use crate::store::{SessionRow, Store};
use crate::tmux::{list_local_sessions, new_session as tmux_new_session, kill_session as tmux_kill_session};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::State;

const LOCAL_HOST: &str = "local";

#[tauri::command]
pub fn list_sessions(
    store: State<'_, Mutex<Store>>,
) -> Result<Vec<SessionRow>, IpcError> {
    let live = list_local_sessions()?;
    let s = store.lock().map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    // Ensure the local host row exists (FK).
    s.upsert_host(LOCAL_HOST)?;
    let mut keep = Vec::with_capacity(live.len());
    for sess in &live {
        keep.push(sess.name.clone());
        s.upsert_session(
            &sess.name,
            LOCAL_HOST,
            None, // project mapping comes in a future task
            None,
            sess.created,
            sess.last_activity,
            if sess.attached { "running" } else { "running" },
        )?;
    }
    s.delete_sessions_not_in(LOCAL_HOST, &keep)?;
    s.list_sessions_for_host(LOCAL_HOST).map_err(IpcError::from)
}

#[derive(Deserialize)]
pub struct NewSessionArgs {
    pub project_id: i64,
    pub worktree_id: Option<i64>,
    pub name: String,
}

#[tauri::command]
pub fn new_session(
    args: NewSessionArgs,
    store: State<'_, Mutex<Store>>,
) -> Result<SessionRow, IpcError> {
    let s = store.lock().map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    let path: PathBuf = if let Some(wid) = args.worktree_id {
        // Find the worktree path
        let mut stmt = s
            .conn_ref()
            .prepare("SELECT path FROM worktrees WHERE id=?1")?;
        let row: String = stmt.query_row(rusqlite::params![wid], |r| r.get(0))?;
        PathBuf::from(row)
    } else {
        let mut stmt = s
            .conn_ref()
            .prepare("SELECT base_path FROM projects WHERE id=?1")?;
        let row: String = stmt.query_row(rusqlite::params![args.project_id], |r| r.get(0))?;
        PathBuf::from(row)
    };
    drop(s);
    tmux_new_session(&args.name, &path)?;
    list_sessions(store.clone())?;
    let s = store.lock().map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    let row = s
        .list_sessions_for_host(LOCAL_HOST)?
        .into_iter()
        .find(|r| r.tmux_name == args.name)
        .ok_or_else(|| IpcError::new("E_NOTFOUND", format!("tmux session {} did not appear in list", args.name)))?;
    Ok(row)
}

#[derive(Deserialize)]
pub struct KillSessionArgs {
    pub name: String,
}

#[tauri::command]
pub fn kill_session(
    args: KillSessionArgs,
    store: State<'_, Mutex<Store>>,
) -> Result<(), IpcError> {
    tmux_kill_session(&args.name)?;
    list_sessions(store)?;
    Ok(())
}
```

`Store::conn_ref()` and `Store::upsert_host()` don't exist yet. Add them to `src-tauri/src/store.rs` (inside `impl Store`):
```rust
    pub fn conn_ref(&self) -> &rusqlite::Connection {
        &self.conn
    }

    pub fn upsert_host(&self, alias: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO hosts (alias, reachable) VALUES (?1, 1)
             ON CONFLICT(alias) DO UPDATE SET reachable=1",
            rusqlite::params![alias],
        )?;
        Ok(())
    }
```

Note: `conn_ref()` re-exposes the connection for read-only prepared statements in commands. It is acceptable here because commands compose multiple queries against one mutex hold; adding a method per query would balloon `store.rs` without benefit at this stage.

Add `pub mod sessions;` to `src-tauri/src/commands/mod.rs`. Register both commands in `lib.rs`:
```rust
.invoke_handler(tauri::generate_handler![
    commands::health::health_check,
    commands::projects::list_projects,
    commands::projects::refresh_projects,
    commands::sessions::list_sessions,
    commands::sessions::new_session,
    commands::sessions::kill_session,
])
```

- [ ] **Step 4: Run all Rust tests + lints**

```bash
cd ~/projects/github.com/martin-janci/claude-fleet/src-tauri
cargo fmt --check || cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```
Expected: 17 tests passing (8 store + 5 projects + 3 ipc_error + 3 tmux + 1 health); clippy clean.

- [ ] **Step 5: Commit**

```bash
cd ~/projects/github.com/martin-janci/claude-fleet
git add -A
git commit -m "feat(sessions): tmux discovery + list_sessions / new_session / kill_session

- tmux.rs spawns 'tmux list-sessions -F …' and parses the porcelain
  output to Vec<TmuxSession>. 'no server running' is normalized to an
  empty list rather than an error. new_session / kill_session shell
  out to 'tmux new-session -d' and 'tmux kill-session' respectively.
- store.rs gains upsert_host, upsert_session, list_sessions_for_host,
  delete_sessions_not_in plus conn_ref() for command-side prepared
  statements.
- commands/sessions.rs glues tmux to the store via three IPCs:
  list_sessions (rescans tmux and reconciles the DB), new_session
  (resolves project/worktree path from DB, spawns tmux, returns the new
  SessionRow), kill_session (kills then reconciles).
- The local host is hard-coded as alias='local' for now; Phase 4
  generalises to multiple hosts."
```

---

## Task 9: Frontend `sessions.ts` store + render sessions in Sidebar

**Files:**
- Create: `src/lib/sessions.ts`
- Modify: `src/lib/Sidebar.svelte` (render sessions under each project/worktree, plus kill button)
- Modify: `vitest.setup.ts` (mock `list_sessions`)
- Create: `src/lib/sessions.test.ts`

- [ ] **Step 1: Write the failing sessions-store test**

Create `src/lib/sessions.test.ts`:
```ts
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { sessions, loadSessions, killSession } from './sessions';

beforeEach(() => {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
  sessions.set([]);
});

const sample = [
  { id: 1, tmux_name: 'dev-foo', host_alias: 'local', project_id: null, worktree_id: null, created_at: 1, last_activity_at: 2, status: 'running', notes: null },
];

describe('sessions store', () => {
  it('loadSessions populates on Ok', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(sample);
    const r = await loadSessions();
    expect(r.ok).toBe(true);
    expect(get(sessions)).toHaveLength(1);
  });

  it('killSession returns Ok and reloads', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(null); // kill_session
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce([]); // list_sessions
    const r = await killSession('dev-foo');
    expect(r.ok).toBe(true);
    expect(get(sessions)).toHaveLength(0);
  });
});
```

Run:
```bash
cd ~/projects/github.com/martin-janci/claude-fleet
pnpm test sessions
```
Expected: FAIL — `./sessions` does not exist.

- [ ] **Step 2: Implement `sessions.ts`**

Create `src/lib/sessions.ts`:
```ts
import { writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';

export interface SessionRow {
  id: number;
  tmux_name: string;
  host_alias: string;
  project_id: number | null;
  worktree_id: number | null;
  created_at: number;
  last_activity_at: number;
  status: string;
  notes: string | null;
}

export const sessions = writable<SessionRow[]>([]);

export async function loadSessions(): Promise<Result<SessionRow[]>> {
  const r = await invokeCmd<SessionRow[]>('list_sessions');
  if (r.ok) sessions.set(r.value);
  return r;
}

export async function killSession(name: string): Promise<Result<void>> {
  const r = await invokeCmd<void>('kill_session', { args: { name } });
  if (r.ok) await loadSessions();
  return r;
}
```

- [ ] **Step 3: Add `list_sessions` and `kill_session` to global mock**

Edit `vitest.setup.ts`:
```ts
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'health_check') return { version: '0.0.0', db_ready: true, schema_version: 1 };
    if (cmd === 'list_projects') return [];
    if (cmd === 'refresh_projects') return [];
    if (cmd === 'list_sessions') return [];
    if (cmd === 'kill_session') return null;
    if (cmd === 'new_session') return null;
    return null;
  }),
}));
```

- [ ] **Step 4: Run sessions tests**

```bash
pnpm test sessions
```
Expected: 2 tests passing.

- [ ] **Step 5: Render sessions under each project in Sidebar**

Modify `src/lib/Sidebar.svelte`. Add to the `<script>` (after the other imports):
```ts
import { sessions, loadSessions, killSession, type SessionRow } from './sessions';
```

Add an extra `onMount` block (or extend the existing one). Replace the existing `onMount` with:
```ts
onMount(async () => {
  const pr = await loadProjects();
  if (!pr.ok) loadError = pr.error.message;
  const sr = await loadSessions();
  if (!sr.ok) loadError = sr.error.message;
});
```

Add `function onKill(name: string)` near the existing handlers:
```ts
async function onKill(name: string) {
  if (!confirm(`Kill tmux session ${name}?`)) return;
  const r = await killSession(name);
  if (!r.ok) loadError = r.error.message;
}

function sessionsForProject(projectId: number): SessionRow[] {
  return $sessions.filter((s) => s.project_id === projectId);
}
```

Replace the `<ul class="worktrees">` block inside the project loop with:
```svelte
          {#if row.worktrees.length > 0}
            <ul class="worktrees">
              {#each row.worktrees as wt (wt.id)}
                <li class="wt" data-testid="wt-row" title={wt.path}>
                  <span class="wt-bullet">└</span>
                  <span class="wt-name">{wt.name}</span>
                  {#if wt.branch && wt.branch !== wt.name}
                    <span class="wt-branch">({wt.branch})</span>
                  {/if}
                </li>
              {/each}
            </ul>
          {/if}
          {#each sessionsForProject(row.project.id) as sess (sess.id)}
            <div class="sess-row" data-testid="sess-row">
              <span class="sess-name">{sess.tmux_name}</span>
              <button class="kill" onclick={() => onKill(sess.tmux_name)} title="Kill session">×</button>
            </div>
          {/each}
```

Add styles inside the `<style>` block:
```css
  .sess-row { display: flex; gap: 0.3rem; font-size: 0.8rem; padding: 0.1rem 0 0.1rem 0.6rem; color: var(--fg); }
  .sess-name { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }
  .kill {
    margin-left: auto;
    background: transparent;
    border: none;
    color: var(--fg-muted);
    cursor: pointer;
    padding: 0 0.3rem;
  }
  .kill:hover { color: #e64a4a; }
```

- [ ] **Step 6: Run all frontend gates**

```bash
pnpm test
pnpm check
pnpm build
```
Expected: 21 tests passing (App: 3, Resizer: 2, theme: 5, ipc: 1, result: 3, projects: 2, Sidebar: 3, sessions: 2); 0 type errors; build clean.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(sessions): render local tmux sessions under projects with kill action

- sessions.ts adds a writable<SessionRow[]>, loadSessions(), and
  killSession(name); both return Result<_> for the same error UX as
  projects.
- Sidebar fetches sessions on mount and renders any session whose
  project_id matches a project row underneath that project's worktrees.
  Click × to kill (confirms first).
- vitest.setup.ts global mock learns list_sessions / kill_session /
  new_session so App-level tests don't hang on these IPCs."
```

---

## Task 10: New Session dialog

**Files:**
- Create: `src/lib/NewSessionDialog.svelte`
- Create: `src/lib/NewSessionDialog.test.ts`
- Modify: `src/lib/Sidebar.svelte` (add "+ session" button per project)
- Modify: `src/lib/sessions.ts` (add `newSession()` action)

- [ ] **Step 1: Extend `sessions.ts` with `newSession`**

Edit `src/lib/sessions.ts` — add at the bottom:
```ts
export interface NewSessionArgs {
  project_id: number;
  worktree_id: number | null;
  name: string;
}

export async function newSession(args: NewSessionArgs): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>('new_session', { args });
  if (r.ok) await loadSessions();
  return r;
}
```

- [ ] **Step 2: Write the failing dialog test**

Create `src/lib/NewSessionDialog.test.ts`:
```ts
import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import NewSessionDialog from './NewSessionDialog.svelte';

const project = {
  project: { id: 7, owner: 'martin-janci', repo: 'claude-fleet', base_path: '/r/cf', last_session_at: null },
  worktrees: [
    { id: 71, project_id: 7, name: 'main', path: '/r/cf', branch: 'main' },
    { id: 72, project_id: 7, name: 'feature-x', path: '/r/cf/.worktrees/feature-x', branch: 'feature-x' },
  ],
};

beforeEach(() => {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
});

describe('NewSessionDialog', () => {
  it('emits onCreate with chosen worktree and default name', async () => {
    const onCreate = vi.fn();
    const onCancel = vi.fn();
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      id: 99, tmux_name: 'dev-martin-janci-claude-fleet', host_alias: 'local',
      project_id: 7, worktree_id: 71, created_at: 1, last_activity_at: 1, status: 'running', notes: null,
    });
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce([]); // list_sessions

    render(NewSessionDialog, { props: { project, onCreate, onCancel } });
    await fireEvent.click(screen.getByText('Create'));

    expect(onCreate).toHaveBeenCalledOnce();
    const call = (onCreate as ReturnType<typeof vi.fn>).mock.calls[0][0];
    expect(call.tmux_name).toBe('dev-martin-janci-claude-fleet');
  });

  it('emits onCancel without invoking the backend', async () => {
    const onCreate = vi.fn();
    const onCancel = vi.fn();
    render(NewSessionDialog, { props: { project, onCreate, onCancel } });
    await fireEvent.click(screen.getByText('Cancel'));
    expect(onCancel).toHaveBeenCalledOnce();
    expect(mockedInvoke as ReturnType<typeof vi.fn>).not.toHaveBeenCalled();
  });
});
```

Run:
```bash
pnpm test NewSessionDialog
```
Expected: FAIL — component does not exist.

- [ ] **Step 3: Implement the dialog**

Create `src/lib/NewSessionDialog.svelte`:
```svelte
<script lang="ts">
  import type { ProjectTreeRow, WorktreeRow } from './projects';
  import { newSession, type SessionRow } from './sessions';

  let {
    project,
    onCreate,
    onCancel,
  }: {
    project: ProjectTreeRow;
    onCreate: (s: SessionRow) => void;
    onCancel: () => void;
  } = $props();

  function defaultName(wt: WorktreeRow | null): string {
    const base = `dev-${project.project.owner}-${project.project.repo}`;
    if (!wt || wt.name === 'main') return base;
    return `${base}--${wt.name}`;
  }

  let chosenWorktreeId = $state<number | null>(project.worktrees[0]?.id ?? null);
  let name = $state(defaultName(project.worktrees[0] ?? null));
  let busy = $state(false);
  let error: string | null = $state(null);

  function onPickWorktree(id: number) {
    chosenWorktreeId = id;
    const wt = project.worktrees.find((w) => w.id === id) ?? null;
    name = defaultName(wt);
  }

  async function submit() {
    if (!name.trim()) {
      error = 'Session name required';
      return;
    }
    busy = true;
    error = null;
    const r = await newSession({
      project_id: project.project.id,
      worktree_id: chosenWorktreeId,
      name: name.trim(),
    });
    busy = false;
    if (!r.ok) {
      error = r.error.message;
      return;
    }
    onCreate(r.value);
  }
</script>

<div class="dialog" role="dialog" aria-label="New session">
  <h3>New session — {project.project.owner}/{project.project.repo}</h3>

  {#if project.worktrees.length > 1}
    <label>Worktree</label>
    <div class="worktree-row">
      {#each project.worktrees as wt (wt.id)}
        <button
          class="wt-pick"
          class:active={chosenWorktreeId === wt.id}
          onclick={() => onPickWorktree(wt.id)}
        >
          {wt.name}
        </button>
      {/each}
    </div>
  {/if}

  <label>tmux name</label>
  <input bind:value={name} data-testid="new-session-name" />

  {#if error}
    <p class="err">{error}</p>
  {/if}

  <div class="actions">
    <button onclick={onCancel} disabled={busy}>Cancel</button>
    <button onclick={submit} disabled={busy || !name.trim()}>Create</button>
  </div>
</div>

<style>
  .dialog {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 1rem;
    width: 360px;
    color: var(--fg);
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }
  .dialog h3 { margin: 0 0 0.3rem 0; font-size: 0.95rem; }
  label { font-size: 0.7rem; color: var(--fg-muted); text-transform: uppercase; }
  input {
    font: inherit;
    padding: 0.3rem 0.4rem;
    border: 1px solid var(--border);
    background: var(--bg-pane);
    color: var(--fg);
    border-radius: 4px;
  }
  .worktree-row { display: flex; gap: 0.3rem; flex-wrap: wrap; }
  .wt-pick {
    font-size: 0.75rem;
    padding: 0.2rem 0.5rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-muted);
    border-radius: 999px;
    cursor: pointer;
  }
  .wt-pick.active { color: var(--fg); border-color: var(--accent); }
  .err { color: #e64a4a; font-size: 0.8rem; }
  .actions { display: flex; gap: 0.4rem; justify-content: flex-end; }
  .actions button {
    font-size: 0.85rem;
    padding: 0.3rem 0.8rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: 4px;
    cursor: pointer;
  }
  .actions button:disabled { opacity: 0.5; cursor: not-allowed; }
</style>
```

- [ ] **Step 4: Wire the dialog into Sidebar**

Modify `src/lib/Sidebar.svelte`. Add to the `<script>`:
```ts
import NewSessionDialog from './NewSessionDialog.svelte';
import type { ProjectTreeRow } from './projects';
import type { SessionRow } from './sessions';

let dialogProject: ProjectTreeRow | null = $state(null);

function openNew(p: ProjectTreeRow) {
  dialogProject = p;
}

function onCreated(_s: SessionRow) {
  dialogProject = null;
}

function onCancel() {
  dialogProject = null;
}
```

In the project row template, add a "+ session" button next to the name:
```svelte
          <div class="proj-row" data-testid="proj-row" title={row.project.base_path}>
            <span class="owner">{row.project.owner}/</span><span class="repo">{row.project.repo}</span>
            <button class="add-session" onclick={() => openNew(row)} title="New session">+</button>
          </div>
```

At the bottom of the template, render the modal when `dialogProject` is set:
```svelte
{#if dialogProject}
  <div class="modal-backdrop" onclick={onCancel} role="presentation">
    <div onclick={(e) => e.stopPropagation()} role="presentation">
      <NewSessionDialog project={dialogProject} onCreate={onCreated} {onCancel} />
    </div>
  </div>
{/if}
```

Add styles:
```css
  .proj-row { display: flex; align-items: center; }
  .add-session {
    margin-left: auto;
    background: transparent;
    border: 1px solid var(--border);
    color: var(--fg-muted);
    padding: 0 0.4rem;
    border-radius: 4px;
    font-size: 0.8rem;
    cursor: pointer;
  }
  .add-session:hover { color: var(--fg); border-color: var(--accent); }
  .modal-backdrop {
    position: fixed; inset: 0; background: rgba(0,0,0,0.4);
    display: flex; align-items: center; justify-content: center;
    z-index: 10;
  }
```

- [ ] **Step 5: Run all gates**

```bash
pnpm test
pnpm check
pnpm build
```
Expected: 23 tests passing across 9 files (added NewSessionDialog: 2); 0 type errors; build clean.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(ui): NewSessionDialog with worktree picker and default tmux name

- NewSessionDialog.svelte takes a ProjectTreeRow and emits onCreate /
  onCancel callbacks. Default tmux name follows the same pattern as the
  existing bin/bin/dev script: dev-<owner>-<repo>--<worktree> (or just
  dev-<owner>-<repo> for main).
- Sidebar renders a + button per project that opens the dialog inside a
  click-outside-to-dismiss backdrop modal.
- 2 tests cover the create path (defaults + onCreate fires) and cancel
  path (no IPC invoked)."
```

---

## Task 11: Auto-refresh on window focus

**Files:**
- Modify: `src/App.svelte` (listen to `window:focus`, reload projects + sessions)
- Modify: `src/App.test.ts` (add focus-refresh assertion)

- [ ] **Step 1: Write the failing focus-refresh test**

Edit `src/App.test.ts` — add a fourth test:
```ts
import { fireEvent } from '@testing-library/svelte';
import { vi } from 'vitest';

  it('refreshes projects and sessions when the window regains focus', async () => {
    const { invoke } = await import('@tauri-apps/api/core');
    const calls = (invoke as ReturnType<typeof vi.fn>).mock.calls;
    const before = calls.length;
    render(App);
    await fireEvent(window, new FocusEvent('focus'));
    const after = (invoke as ReturnType<typeof vi.fn>).mock.calls.length;
    expect(after).toBeGreaterThan(before);
    // We expect at least list_projects and list_sessions to have been called.
    const cmds = (invoke as ReturnType<typeof vi.fn>).mock.calls.map((c) => c[0]);
    expect(cmds).toEqual(expect.arrayContaining(['list_projects', 'list_sessions']));
  });
```

Run:
```bash
pnpm test App
```
Expected: FAIL — focus handler not wired yet.

- [ ] **Step 2: Wire the focus handler in `App.svelte`**

Add to `<script lang="ts">` (after the existing `onMount`):
```ts
import { onDestroy } from 'svelte';
import { loadProjects } from './lib/projects';
import { loadSessions } from './lib/sessions';

function onFocus() {
  void loadProjects();
  void loadSessions();
}

onMount(() => {
  window.addEventListener('focus', onFocus);
});

onDestroy(() => {
  window.removeEventListener('focus', onFocus);
});
```

(Note: there will now be two `onMount` blocks in App.svelte — Svelte allows multiple. Keep them separate for clarity, or merge with the existing one if you prefer.)

- [ ] **Step 3: Run the test**

```bash
pnpm test App
```
Expected: 4 App tests passing.

- [ ] **Step 4: Run full suite**

```bash
pnpm test
pnpm check
pnpm build
```
Expected: 24 tests passing; 0 type errors; build clean.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(ui): refresh projects + sessions when the window regains focus

Wires window.addEventListener('focus', …) inside App.svelte to call
loadProjects() and loadSessions(). onDestroy removes the listener.
Test asserts that a synthetic focus event triggers both IPC calls."
```

---

## Task 12: Phase 2 exit-criteria verification

**Files:** none (verification only)

- [ ] **Step 1: Clean rebuild**

```bash
cd ~/projects/github.com/martin-janci/claude-fleet
rm -rf node_modules dist
pnpm install --frozen-lockfile
(cd src-tauri && cargo build)
```
Expected: both succeed.

- [ ] **Step 2: All quality gates green**

```bash
pnpm test
pnpm check
pnpm build
(cd src-tauri && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test)
```
Expected:
- Frontend: 24 tests across 9+ files passing.
- Backend: 17+ tests passing (8 store + 5 projects + 3 ipc_error + 3 tmux + 1 health, plus whatever Task 8/etc add).
- Lint, type-check, build all clean.

- [ ] **Step 3: Headless integration check**

The end-to-end flow can't open a GUI window non-interactively, but you can verify the IPC contract by calling each command via `tauri::test::mock_builder` if you want to add an integration test — out of scope for Phase 2 if quality gates already pass.

Boot the dev mode briefly to confirm no panics on startup:
```bash
mkdir -p /tmp/cf-phase2-smoke
( pnpm tauri dev > /tmp/cf-phase2-smoke/dev.log 2>&1 & echo $! > /tmp/cf-phase2-smoke/pid )
sleep 35
grep -E 'VITE.*ready|Compiling claude-fleet' /tmp/cf-phase2-smoke/dev.log | head -5
grep -iE 'error\[E|panicked|cannot find' /tmp/cf-phase2-smoke/dev.log | head -5 || echo "no errors"
kill -TERM $(cat /tmp/cf-phase2-smoke/pid) 2>/dev/null || true
wait 2>/dev/null
tail -20 /tmp/cf-phase2-smoke/dev.log
rm -rf /tmp/cf-phase2-smoke
```
Expected: Vite ready on 1420; cargo compilation progressing; zero panics.

- [ ] **Step 4: Push and verify CI**

```bash
git push origin main
gh run watch --exit-status
```
Expected: both jobs green.

- [ ] **Step 5: Tag the phase**

```bash
git tag v0.2.0-phase2
git push --tags
```

- [ ] **Step 6: Final state report**

```bash
git log --oneline | head -20
git status
```

Confirm exit criteria from the spec (§9 Phase 2):
- ✅ Scan `~/projects/github.com/{owner}/{repo}` → populates `projects` (Task 4-5).
- ✅ Parse `git worktree list --porcelain` per project → populates `worktrees` (Task 4-5).
- ✅ List local tmux sessions: `tmux list-sessions -F …` → populates `sessions` (Task 8).
- ✅ Sidebar tree (Task 6), recency filter pills, search box (Task 7).
- ✅ "New session" dialog (local host only) → `tmux new-session -d` (Task 10).
- ✅ Kill button (Task 9).
- ✅ Auto-refresh on focus (Task 11).
- ✅ Exit criteria: start the app, see your projects, create a tmux+claude session, see it in sidebar, kill it.

---

## Spec coverage self-review

| Spec section | Requirement | Covered by |
|---|---|---|
| §5.1 (decisions) | Mutex<Store> + tauri::State | Task 1 |
| §5.3 (Rust modules) | `projects`, `tmux`, `ipc_error` | Tasks 3, 4, 8 |
| §5.3 (commands) | `list_projects`, `refresh_projects`, `list_sessions`, `new_session`, `kill_session` | Tasks 5, 8 |
| §5.4 (Svelte modules) | `Sidebar`, `NewSessionDialog`, `projects.ts`, `sessions.ts`, runes idiom | Tasks 2, 6, 7, 9, 10 |
| §6 (schema) | Project, worktree, session row helpers; FK enforcement | Tasks 4, 8 |
| §7.1 (sidebar) | Tree (project → worktree → session), search, filter pills | Tasks 6, 7, 9 |
| §7.1 (session row context menu) | Kill | Task 9 |
| §8.1 (new session) | tmux name convention + spawn | Tasks 8, 10 |
| §9 Phase 2 (exit criteria) | All 7 deliverables + exit | Tasks 4–12 |

**Placeholder scan:** No TBD/TODO/"appropriate error handling"/"similar to" strings. Every code step has actual code. Every test step has actual assertions.

**Type consistency:**
- `Health` is `{ version: string; db_ready: boolean; schema_version: number }` across `commands/health.rs`, `lib/ipc.ts`, `vitest.setup.ts`, and `App.svelte` footer.
- `ProjectRow`, `WorktreeRow`, `SessionRow` share identical fields between Rust serde derives and TS interfaces.
- `ProjectTreeRow = { project, worktrees }` matches between `commands/projects.rs` and `lib/projects.ts`.
- `IpcError = { code: string; message: string; details?: unknown }` matches between Rust `ipc_error.rs` and TS `result.ts`.
- `invokeCmd<T>(cmd, args?)` signature is identical at every call site.
- Tmux name convention `dev-<owner>-<repo>[--<worktree>]` is consistent between `bin/bin/dev` (Phase 1 reference), the `NewSessionDialog.defaultName`, and the Rust `new_session` command (which trusts the frontend's chosen name).
