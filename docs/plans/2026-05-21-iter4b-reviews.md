# claude-fleet iter 4b — Reviews-as-a-feature Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an on-demand "Review" action that spawns an interactive `claude` tmux session in the source session's worktree, seeded with an editable multipass review prompt, surfaced and linked in the UI.

**Architecture:** Reuse the existing spawn (`new_session`/`exec_for`), reconcile (`reconcile_one_host`), and send-prompt (`send_prompt`/`build_send_commands`) machinery. A new `spawn_review` command creates the session and tags it `kind='review'` + `reviews_session_id`. The review's output lives in the embedded terminal — no verdict scraping in v1.

**Tech Stack:** Rust + Tauri 2 (`tokio`, `rusqlite`), Svelte 5 runes.

**Reference spec:** `docs/specs/2026-05-21-iter4b-reviews-design.md`

**Planning refinement vs spec:** `kind`/`reviews_session_id` are write-once by `spawn_review`; reconcile never writes them. Because `upsert_session`'s `ON CONFLICT DO UPDATE SET` clause omits them, they're preserved automatically across re-probe. So `upsert_session`'s signature is **unchanged**; a dedicated `set_session_kind()` writer sets the review metadata. No explicit reconcile-preservation code needed (simpler than the spec's "mirror account_uuid" framing).

---

## File Structure

| File | Responsibility | Status |
|---|---|---|
| `src-tauri/migrations/005_session_reviews.sql` | Add `kind` + `reviews_session_id` columns, bump schema_version. | Created |
| `src-tauri/src/store.rs` | `SessionRow` gains 2 fields; all SELECT projections updated; new `set_session_kind`; migration wired. | Modified |
| `src-tauri/src/commands/health.rs` | schema_version assertion → 5. | Modified |
| `src-tauri/src/commands/sessions.rs` | New `spawn_review` command + worktree-path resolution. | Modified |
| `src-tauri/src/lib.rs` | Register `spawn_review` in `generate_handler!`. | Modified |
| `src/lib/sessions.ts` | `SessionRow` gains 2 fields; `spawnReview` wrapper; `DEFAULT_REVIEW_PROMPT`. | Modified |
| `src/lib/ReviewDialog.svelte` | Modal: source + editable template + Start review. | Created |
| `src/lib/SessionDetails.svelte` | "Review" button + "Reviewing: X" row + Reviews list. | Modified |
| `src/lib/Sidebar.svelte` | 🔍 badge for `kind === 'review'` rows. | Modified |
| `vitest.setup.ts` | mock `spawn_review`. | Modified |

---

## M1 — Data model

### Task 1: Migration 005 + SessionRow read-path + set_session_kind

**Files:**
- Create: `src-tauri/migrations/005_session_reviews.sql`
- Modify: `src-tauri/src/store.rs`
- Modify: `src-tauri/src/commands/health.rs`

- [ ] **Step 1: Write the migration**

Create `src-tauri/migrations/005_session_reviews.sql`:

```sql
ALTER TABLE sessions ADD COLUMN kind TEXT NOT NULL DEFAULT 'work';
ALTER TABLE sessions ADD COLUMN reviews_session_id INTEGER REFERENCES sessions(id);
INSERT OR IGNORE INTO schema_version (version) VALUES (5);
```

- [ ] **Step 2: Wire migration into `migrate()`**

In `src-tauri/src/store.rs`, after the `if v < 4 { ... }` block:

```rust
if v < 5 {
    self.conn
        .execute_batch(include_str!("../migrations/005_session_reviews.sql"))?;
}
```

- [ ] **Step 3: Add fields to `SessionRow`**

In `src-tauri/src/store.rs`, the `SessionRow` struct:

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
    pub account_uuid: Option<String>,
    pub kind: String,
    pub reviews_session_id: Option<i64>,
}
```

- [ ] **Step 4: Update every SELECT projection that builds `SessionRow`**

Find them:

```bash
cd /Users/martinjanci/projects/github.com/martin-janci/claude-fleet
grep -n "SessionRow {" src-tauri/src/store.rs
```

Each projection (`get_session`, `list_sessions_for_host`, `list_related_sessions`, and any others) must add the two columns to BOTH the `SELECT` column list and the row builder. Pattern — change the SQL from:

```sql
SELECT id, tmux_name, host_alias, project_id, worktree_id, created_at,
       last_activity_at, status, notes, account_uuid
FROM sessions ...
```

to:

```sql
SELECT id, tmux_name, host_alias, project_id, worktree_id, created_at,
       last_activity_at, status, notes, account_uuid, kind, reviews_session_id
FROM sessions ...
```

and the builder from:

```rust
account_uuid: row.get(9)?,
```

to:

```rust
account_uuid: row.get(9)?,
kind: row.get(10)?,
reviews_session_id: row.get(11)?,
```

Adjust the column indices to match each query's actual column order. Do this for every `SessionRow { ... }` construction site.

- [ ] **Step 5: Add `set_session_kind`**

In `src-tauri/src/store.rs`, inside `impl Store`:

```rust
/// Mark a session as a review of `reviews_session_id` (or back to plain
/// 'work' with `None`). Write-once at spawn_review time; reconcile never
/// touches these columns, so they survive re-probe via the ON CONFLICT
/// clause that omits them.
pub fn set_session_kind(
    &self,
    id: i64,
    kind: &str,
    reviews_session_id: Option<i64>,
) -> Result<(), rusqlite::Error> {
    self.conn.execute(
        "UPDATE sessions SET kind = ?1, reviews_session_id = ?2 WHERE id = ?3",
        rusqlite::params![kind, reviews_session_id, id],
    )?;
    // Emit so the frontend patches the row in place (kind/reviews_session_id
    // ride inside the SessionRow payload).
    if let Some(row) = self.get_session_by_id(id)? {
        self.bus.session_updated(&row);
    }
    Ok(())
}

fn get_session_by_id(&self, id: i64) -> Result<Option<SessionRow>, rusqlite::Error> {
    let mut stmt = self.conn.prepare(
        "SELECT id, tmux_name, host_alias, project_id, worktree_id, created_at,
                last_activity_at, status, notes, account_uuid, kind, reviews_session_id
         FROM sessions WHERE id = ?1",
    )?;
    let mut rows = stmt.query_map(rusqlite::params![id], |row| {
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
            account_uuid: row.get(9)?,
            kind: row.get(10)?,
            reviews_session_id: row.get(11)?,
        })
    })?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}
```

- [ ] **Step 6: Bump health assertion**

In `src-tauri/src/commands/health.rs`, the test asserting schema_version:

```rust
assert_eq!(h.schema_version, 5);
```

- [ ] **Step 7: Tests**

In `src-tauri/src/store.rs` `#[cfg(test)] mod tests`:

```rust
#[test]
fn migration_005_adds_kind_and_reviews_columns_with_defaults() {
    let store = Store::open_in_memory().expect("store");
    store.upsert_host("alpha").unwrap();
    store.upsert_session("s1", "alpha", None, None, 1, 1, "running", None).unwrap();
    let rows = store.list_sessions_for_host("alpha").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, "work");
    assert_eq!(rows[0].reviews_session_id, None);
}

#[test]
fn set_session_kind_marks_review_and_survives_reupsert() {
    let store = Store::open_in_memory().expect("store");
    store.upsert_host("alpha").unwrap();
    let src = store.upsert_session("src", "alpha", None, None, 1, 1, "running", None).unwrap();
    let rev = store.upsert_session("src--review-1", "alpha", None, None, 1, 1, "running", None).unwrap();
    store.set_session_kind(rev, "review", Some(src)).unwrap();
    // Re-upsert the review row (simulating reconcile re-probe).
    store.upsert_session("src--review-1", "alpha", None, None, 1, 2, "running", None).unwrap();
    let row = store.list_sessions_for_host("alpha").unwrap()
        .into_iter().find(|r| r.tmux_name == "src--review-1").unwrap();
    assert_eq!(row.kind, "review", "kind must survive re-upsert");
    assert_eq!(row.reviews_session_id, Some(src));
}
```

Note: `upsert_session` returns the row `id` (it does as of iter 4a Task 9). If it returns `()` in the current code, adjust the test to fetch the id via `get_session`.

- [ ] **Step 8: Run + commit**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib 2>&1 | tail -6
```

Expected: 104 + 2 new = 106 passing, including the bumped health assertion.

```bash
git add src-tauri/migrations/005_session_reviews.sql src-tauri/src/store.rs src-tauri/src/commands/health.rs
git commit -m "store: migration 005 (sessions.kind + reviews_session_id) + set_session_kind"
```

---

### Task 2: Frontend SessionRow type + fixtures

**Files:**
- Modify: `src/lib/sessions.ts`
- Modify: test files with inline SessionRow fixtures

- [ ] **Step 1: Extend the TS type**

In `src/lib/sessions.ts`, the `SessionRow` interface:

```ts
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
  account_uuid: string | null;
  kind: string;
  reviews_session_id: number | null;
}
```

- [ ] **Step 2: Patch inline fixtures**

Find every literal `SessionRow` object in tests:

```bash
grep -rn "account_uuid: null" src/ | grep -v node_modules
```

Each fixture that constructs a `SessionRow` needs `kind: 'work'` and `reviews_session_id: null` added. Known sites (verify): `Sidebar.test.ts` (`sessionFor` helper), `SessionDetails.test.ts`, `PromptComposer.test.ts`, `sessions.test.ts`, `events.test.ts`. Add the two fields to each.

For `Sidebar.test.ts`'s `sessionFor` helper, add to the returned object:

```ts
kind: 'work',
reviews_session_id: null,
```

- [ ] **Step 3: Run + commit**

```bash
pnpm tsc --noEmit 2>&1 | tail -5
pnpm vitest run 2>&1 | tail -4
```

Expected: tsc clean (pre-existing carry forward), vitest 150 passing.

```bash
git add src/lib/sessions.ts src/lib/*.test.ts
git commit -m "sessions.ts: SessionRow gains kind + reviews_session_id"
```

---

## M2 — `spawn_review` backend

### Task 3: `spawn_review` command

**Files:**
- Modify: `src-tauri/src/commands/sessions.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Add worktree-path resolution helper**

In `src-tauri/src/commands/sessions.rs`:

```rust
/// Resolve the working directory a review session should open in, given the
/// source session row. Order: the source's worktree path (by worktree_id) →
/// the source session's recorded cwd if we have one → the project base_path →
/// error. For remote hosts the worktree table holds local paths, so a recorded
/// per-session cwd (if present) is preferred; v1 falls back to project base.
fn resolve_review_cwd(s: &Store, source: &SessionRow) -> Result<String, IpcError> {
    if let Some(wt_id) = source.worktree_id {
        if let Some(path) = s.worktree_path(wt_id)? {
            return Ok(path);
        }
    }
    if let Some(pid) = source.project_id {
        if let Some(base) = s.project_base_path(pid)? {
            return Ok(base);
        }
    }
    Err(IpcError::new(
        "E_INVALID",
        "cannot determine a worktree path to review for this session",
    ))
}
```

Add the two small Store getters in `store.rs` if they don't exist:

```rust
pub fn worktree_path(&self, id: i64) -> Result<Option<String>, rusqlite::Error> {
    self.conn.query_row(
        "SELECT path FROM worktrees WHERE id = ?1",
        rusqlite::params![id],
        |row| row.get(0),
    ).optional()
}

pub fn project_base_path(&self, id: i64) -> Result<Option<String>, rusqlite::Error> {
    self.conn.query_row(
        "SELECT base_path FROM projects WHERE id = ?1",
        rusqlite::params![id],
        |row| row.get(0),
    ).optional()
}
```

(`OptionalExtension` is already imported in store.rs as of iter 4a.)

- [ ] **Step 2: Add the `spawn_review` command**

```rust
#[derive(Deserialize)]
pub struct SpawnReviewArgs {
    pub source_session_id: i64,
    pub prompt: String,
    pub call_id: Option<u64>,
}

#[tauri::command]
pub async fn spawn_review(
    args: SpawnReviewArgs,
    store: State<'_, Mutex<Store>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SessionRow, IpcError> {
    // 1. Snapshot the source row + resolve cwd under a brief lock.
    let (source, cwd) = {
        let s = store.lock().map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        let source = s
            .list_sessions_for_host_any(args.source_session_id)?
            .ok_or_else(|| IpcError::new("E_NOTFOUND", "source session not found"))?;
        let cwd = resolve_review_cwd(&s, &source)?;
        (source, cwd)
    };

    // 2. Derive a unique-ish review name and spawn the tmux session (off-lock).
    let short = format!("{:x}", now_unix() & 0xfffff);
    let review_name = format!("{}--review-{}", source.tmux_name, short);
    let tmux = exec_for(&source.host_alias, &ssh);
    tmux.new_session(&review_name, std::path::Path::new(&cwd), "cl").await?;

    // 3. Register the new session via per-host reconcile.
    reconcile_one_host(&store, &ssh, &source.host_alias).await?;

    // 4. Tag it as a review + look up its id.
    let review_id = {
        let s = store.lock().map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        let row = s.list_sessions_for_host(&source.host_alias)?
            .into_iter()
            .find(|r| r.tmux_name == review_name)
            .ok_or_else(|| IpcError::new("E_INTERNAL", "review session vanished after spawn"))?;
        s.set_session_kind(row.id, "review", Some(source.id))?;
        row.id
    };

    // 5. Seed the review prompt. cl needs a beat to boot its TUI before
    //    send-keys lands in the prompt box — poll/sleep briefly.
    wait_for_repl_ready(&tmux, &review_name).await;
    for cmd in build_send_commands(&review_name, &args.prompt) {
        tmux_send_raw(&source.host_alias, &ssh, &cmd).await?;
    }

    // 6. Return the tagged review row.
    let s = store.lock().map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.get_session_public_by_id(review_id)?
        .ok_or_else(|| IpcError::new("E_INTERNAL", "review row missing after tag"))
}
```

Notes for the implementer:
- `list_sessions_for_host_any(id)` — a by-id lookup that searches across hosts. If no such helper exists, add a `get_session_public_by_id(id) -> Result<Option<SessionRow>>` (a pub wrapper over the private `get_session_by_id` from Task 1) and use it for both the source lookup (step 1) and the return (step 6).
- `build_send_commands(name, prompt)` already exists (iter 3) — returns the 2-element literal-`send-keys` + `Enter` Vec.
- `tmux_send_raw(host, ssh, cmd)` — the existing `send_prompt` already runs these via local `bash -c` or remote SSH. Reuse `send_prompt`'s internal dispatch: factor the "run one tmux command string on a host" logic into a helper both call, OR call the existing `send_prompt(SendPromptArgs { host_alias, tmux_name, prompt })` directly instead of looping yourself. **Preferred: call the existing `send_prompt` command logic** — extract its body into `async fn send_prompt_inner(store, ssh, host_alias, tmux_name, prompt)` and call it from both `send_prompt` (the command) and `spawn_review`. This avoids duplicating the literal-mode/quoting logic.
- `wait_for_repl_ready` — see Step 3.

- [ ] **Step 3: REPL-readiness wait**

`cl` (claude) needs ~1-2s to start its TUI. Add a helper that polls the pane for a readiness signal, falling back to a fixed delay:

```rust
async fn wait_for_repl_ready(tmux: &Box<dyn TmuxExec>, name: &str) {
    // Best-effort: poll capture-pane for the claude prompt box border or the
    // ">" prompt for up to ~3s; if we can't detect it, fall back to the time
    // we already waited. A missed prompt just means the user presses Enter
    // themselves — non-fatal.
    for _ in 0..15 {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        if let Ok(pane) = tmux.capture_pane(name).await {
            if pane.contains('>') || pane.contains("│") {
                return;
            }
        }
    }
}
```

This needs a `capture_pane(name)` method on `TmuxExec`. If it doesn't exist, add it to the trait + both impls:

```rust
async fn capture_pane(&self, name: &str) -> Result<String, IpcError>;
```

`LocalTmux`: `tmux capture-pane -t <name> -p`. `RemoteTmux`: same via `ssh.run`. If adding to the trait is too broad for this task, inline a simpler fixed `tokio::time::sleep(Duration::from_millis(1500))` and note it — the readiness poll is a nice-to-have, the fixed delay is acceptable for v1.

- [ ] **Step 4: Register in `lib.rs`**

Add `commands::sessions::spawn_review` to `tauri::generate_handler![...]`.

- [ ] **Step 5: Test**

In `commands/sessions.rs` tests, verify `spawn_review` tags the row. Use a mock `TmuxExec` (the codebase has the `SleepyTmux`/mock pattern from iter 4a). Since wiring a full mock through `State<Mutex<Store>>` + `State<SshClient>` is heavy, the focused test asserts the Store-level invariant instead:

```rust
#[test]
fn set_session_kind_links_review_to_source() {
    let store = Store::open_in_memory().expect("store");
    store.upsert_host("alpha").unwrap();
    let src = store.upsert_session("src", "alpha", None, None, 1, 1, "running", None).unwrap();
    let rev = store.upsert_session("src--review-abc", "alpha", None, None, 1, 1, "running", None).unwrap();
    store.set_session_kind(rev, "review", Some(src)).unwrap();
    let rows = store.list_sessions_for_host("alpha").unwrap();
    let review = rows.iter().find(|r| r.tmux_name == "src--review-abc").unwrap();
    assert_eq!(review.kind, "review");
    assert_eq!(review.reviews_session_id, Some(src));
}
```

(The end-to-end spawn is exercised in live verify, Task 7 — mocking tmux+ssh+reconcile in a unit test is disproportionate.)

- [ ] **Step 6: Run + commit**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib 2>&1 | tail -6
pnpm vitest run 2>&1 | tail -4
```

Expected: cargo 106 + 1 = 107; vitest 150.

```bash
git add src-tauri/src/commands/sessions.rs src-tauri/src/store.rs src-tauri/src/lib.rs
git commit -m "sessions: spawn_review command (spawn in source worktree + tag + seed)"
```

---

## M3 — ReviewDialog + spawnReview wrapper

### Task 4: `spawnReview` wrapper + `DEFAULT_REVIEW_PROMPT` + ReviewDialog

**Files:**
- Modify: `src/lib/sessions.ts`
- Create: `src/lib/ReviewDialog.svelte`
- Create: `src/lib/ReviewDialog.test.ts`
- Modify: `vitest.setup.ts`

- [ ] **Step 1: Add the wrapper + default prompt to `sessions.ts`**

```ts
export const DEFAULT_REVIEW_PROMPT = `Review the work in this worktree. Run \`git diff\` and \`git log\` against the base branch to see what changed.

Pass 1 — correctness: does the code do what it should? Any bugs?
Pass 2 — code quality: clarity, structure, test coverage.
Pass 3 — risk: anything dangerous, security-sensitive, or destructive?

Cite file:line for every point. End with an overall verdict: approve / approve-with-fixes / needs-rework.`;

export async function spawnReview(
  sourceSessionId: number,
  prompt: string,
  signal?: AbortSignal,
): Promise<Result<SessionRow>> {
  const r = await invokeCmdAbortable<SessionRow>(
    'spawn_review',
    { args: { source_session_id: sourceSessionId, prompt } },
    signal,
  );
  if (r.ok) mergeSession(r.value);
  return r;
}
```

(`invokeCmdAbortable` is from iter 4a; it injects `call_id` into `args`.)

- [ ] **Step 2: Mock `spawn_review` in `vitest.setup.ts`**

Append before the final `return null`:

```ts
    if (cmd === 'spawn_review') return null;
```

- [ ] **Step 3: Create `src/lib/ReviewDialog.svelte`**

```svelte
<script lang="ts">
  import { spawnReview, DEFAULT_REVIEW_PROMPT, type SessionRow } from './sessions';
  import { selectSession } from './selection';

  let { source, onClose }: { source: SessionRow; onClose: () => void } = $props();

  let prompt = $state(DEFAULT_REVIEW_PROMPT);
  let spawning = $state(false);
  let error = $state<string | null>(null);
  let controller: AbortController | null = null;

  const canStart = $derived(prompt.trim().length > 0 && !spawning);

  async function start() {
    spawning = true;
    error = null;
    controller = new AbortController();
    try {
      const r = await spawnReview(source.id, prompt, controller.signal);
      if (r.ok) {
        selectSession(r.value);
        onClose();
      } else if (r.error.code !== 'E_CANCELLED') {
        error = r.error.message;
      }
    } finally {
      spawning = false;
      controller = null;
    }
  }
</script>

<div class="modal-backdrop" onclick={onClose} role="presentation">
  <div class="dialog" onclick={(e) => e.stopPropagation()} role="dialog" aria-label="Start review">
    <h3>Review session</h3>
    <p class="src">
      <span class="host-badge">[{source.host_alias}]</span>
      <span class="sess-name">{source.tmux_name}</span>
    </p>
    <p class="muted">Spawns a claude review session in this session's worktree, seeded with the prompt below. Reviews the worktree's current state.</p>

    <section class="prompt-section">
      <h4>Review prompt</h4>
      <textarea bind:value={prompt} rows="10" data-testid="review-textarea"></textarea>
    </section>

    {#if error}
      <p class="err" data-testid="review-error">{error}</p>
    {/if}

    <div class="actions">
      <button onclick={onClose}>Cancel</button>
      <button
        class="primary"
        disabled={!canStart}
        onclick={start}
        data-testid="review-start"
      >{spawning ? 'Starting…' : 'Start review'}</button>
    </div>
  </div>
</div>

<style>
  .modal-backdrop { position: fixed; inset: 0; background: rgba(0,0,0,0.4); display: flex; align-items: center; justify-content: center; z-index: 20; }
  .dialog { background: var(--bg); border: 1px solid var(--border); border-radius: 6px; padding: 1rem; width: 560px; max-height: 80vh; overflow: auto; color: var(--fg); display: flex; flex-direction: column; gap: 0.7rem; }
  .dialog h3 { margin: 0; font-size: 1rem; }
  .dialog h4 { margin: 0 0 0.3rem 0; font-size: 0.7rem; color: var(--fg-muted); text-transform: uppercase; letter-spacing: 0.05em; }
  .src { margin: 0; display: flex; gap: 0.4rem; align-items: center; }
  .host-badge { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 0.7rem; color: var(--fg-muted); border: 1px solid var(--border); padding: 0.05rem 0.3rem; border-radius: 3px; }
  .sess-name { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 0.8rem; }
  .muted { color: var(--fg-muted); font-size: 0.8rem; margin: 0; }
  .prompt-section textarea { width: 100%; font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 0.82rem; padding: 0.5rem; border: 1px solid var(--border); background: var(--bg-pane); color: var(--fg); border-radius: 4px; resize: vertical; min-height: 8rem; }
  .err { color: #e64a4a; font-size: 0.8rem; margin: 0; }
  .actions { display: flex; gap: 0.4rem; justify-content: flex-end; }
  .actions button { font-size: 0.85rem; padding: 0.3rem 0.8rem; border: 1px solid var(--border); background: transparent; color: var(--fg); border-radius: 4px; cursor: pointer; }
  .actions button:disabled { opacity: 0.5; cursor: not-allowed; }
  .actions button.primary { border-color: var(--accent); }
</style>
```

- [ ] **Step 4: Create `src/lib/ReviewDialog.test.ts`**

```ts
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import ReviewDialog from './ReviewDialog.svelte';
import { DEFAULT_REVIEW_PROMPT, type SessionRow } from './sessions';

const source: SessionRow = {
  id: 1, tmux_name: 'dev-source', host_alias: 'local',
  project_id: 1, worktree_id: 10, created_at: 1, last_activity_at: 1,
  status: 'running', notes: null, account_uuid: null, kind: 'work', reviews_session_id: null,
};

beforeEach(() => { (mockedInvoke as ReturnType<typeof vi.fn>).mockReset(); });

describe('ReviewDialog', () => {
  it('prefills the default multipass prompt', async () => {
    render(ReviewDialog, { props: { source, onClose: () => {} } });
    await tick();
    const ta = screen.getByTestId('review-textarea') as HTMLTextAreaElement;
    expect(ta.value).toBe(DEFAULT_REVIEW_PROMPT);
  });

  it('Start review calls spawn_review with source id + prompt', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'spawn_review') return { ...source, id: 2, tmux_name: 'dev-source--review-abc', kind: 'review', reviews_session_id: 1 };
      return null;
    });
    render(ReviewDialog, { props: { source, onClose: () => {} } });
    await tick();
    await fireEvent.click(screen.getByTestId('review-start'));
    for (let i = 0; i < 6; i++) await tick();
    const call = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.find((c) => c[0] === 'spawn_review');
    expect(call).toBeDefined();
    const payload = call![1] as { args: { source_session_id: number; prompt: string } };
    expect(payload.args.source_session_id).toBe(1);
    expect(payload.args.prompt).toContain('Pass 1');
  });

  it('Start is disabled when prompt is emptied', async () => {
    render(ReviewDialog, { props: { source, onClose: () => {} } });
    await tick();
    const ta = screen.getByTestId('review-textarea') as HTMLTextAreaElement;
    await fireEvent.input(ta, { target: { value: '   ' } });
    await tick();
    expect((screen.getByTestId('review-start') as HTMLButtonElement).disabled).toBe(true);
  });
});
```

- [ ] **Step 5: Run + commit**

```bash
pnpm vitest run src/lib/ReviewDialog.test.ts 2>&1 | tail -10
pnpm vitest run 2>&1 | tail -4
```

Expected: 3 ReviewDialog tests pass; full sweep 150 + 3 = 153.

```bash
git add src/lib/sessions.ts src/lib/ReviewDialog.svelte src/lib/ReviewDialog.test.ts vitest.setup.ts
git commit -m "ReviewDialog: editable multipass template + spawnReview wrapper"
```

---

## M4 — SessionDetails + Sidebar surfacing

### Task 5: SessionDetails Review button + linking

**Files:**
- Modify: `src/lib/SessionDetails.svelte`

- [ ] **Step 1: Add the Review button + dialog state**

In `src/lib/SessionDetails.svelte` `<script>`:

```ts
  import ReviewDialog from './ReviewDialog.svelte';
  let reviewOpen = $state(false);
```

In the action row (next to the existing "→ Send prompt" button):

```svelte
  <button class="ghost" onclick={() => (reviewOpen = true)} data-testid="open-review">
    🔍 Review
  </button>
```

And the dialog mount:

```svelte
  {#if reviewOpen}
    <ReviewDialog source={session} onClose={() => (reviewOpen = false)} />
  {/if}
```

- [ ] **Step 2: "Reviewing: X" row for review sessions**

```ts
  import { sessions } from './sessions';
  const reviewedSource = $derived.by(() => {
    if (session.kind !== 'review' || session.reviews_session_id == null) return null;
    return $sessions.find((s) => s.id === session.reviews_session_id) ?? null;
  });
```

In the meta block:

```svelte
  {#if reviewedSource}
    <div class="meta-row">
      <span class="label">Reviewing</span>
      <button class="link" onclick={() => selectSession(reviewedSource)} data-testid="reviewing-link">
        {reviewedSource.tmux_name}
      </button>
    </div>
  {/if}
```

- [ ] **Step 3: "Reviews" list for source sessions**

```ts
  const reviewsOfThis = $derived(
    $sessions.filter((s) => s.kind === 'review' && s.reviews_session_id === session.id),
  );
```

Render a section (mirror the existing Related-sessions panel pattern) when `reviewsOfThis.length > 0`, each row a click-to-switch button to that review session.

- [ ] **Step 4: Run + commit**

```bash
pnpm vitest run 2>&1 | tail -4
```

Expected: 153 passing (existing SessionDetails tests still green; add an assertion if the test file is easy to extend, otherwise rely on live verify).

```bash
git add src/lib/SessionDetails.svelte
git commit -m "SessionDetails: Review button + Reviewing/Reviews linking"
```

---

### Task 6: Sidebar 🔍 review badge

**Files:**
- Modify: `src/lib/Sidebar.svelte`

- [ ] **Step 1: Render the badge**

In `src/lib/Sidebar.svelte`, in the session-row markup (both project-grouped and orphan loops), next to the existing 🔗N related badge:

```svelte
  {#if sess.kind === 'review'}
    <span class="review-badge" title="review session">🔍</span>
  {/if}
```

- [ ] **Step 2: Scoped style**

```svelte
  .review-badge { font-size: 0.7rem; margin-left: 0.2rem; }
```

- [ ] **Step 3: Test**

In `src/lib/Sidebar.test.ts`:

```ts
it('shows a 🔍 badge for review sessions', async () => {
  const rev = sessionFor(1, 'dev-foo--review-1');
  rev.kind = 'review';
  rev.reviews_session_id = 999;
  mockBackend(fakeProjects, [sessionFor(1, 'dev-foo'), rev]);
  render(Sidebar);
  await tick(); await tick();
  expect(screen.getByText('🔍')).toBeInTheDocument();
});
```

- [ ] **Step 4: Run + commit**

```bash
pnpm vitest run src/lib/Sidebar.test.ts 2>&1 | tail -8
pnpm vitest run 2>&1 | tail -4
```

Expected: 153 + 1 = 154 passing.

```bash
git add src/lib/Sidebar.svelte src/lib/Sidebar.test.ts
git commit -m "Sidebar: 🔍 badge for review sessions"
```

---

## M5 — Live verify + push

### Task 7: Live verify + push

- [ ] **Step 1: Final test sweep**

```bash
cd /Users/martinjanci/projects/github.com/martin-janci/claude-fleet
cargo test --manifest-path src-tauri/Cargo.toml --lib 2>&1 | tail -5
pnpm vitest run 2>&1 | tail -5
```

Expected: cargo ~107, vitest ~154, all green.

- [ ] **Step 2: Build + restart**

```bash
pnpm tauri build --bundles app 2>&1 | tail -6
pkill -f "claude-fleet.app/Contents/MacOS/claude-fleet" 2>/dev/null; sleep 1
open -a /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/src-tauri/target/release/bundle/macos/claude-fleet.app
```

- [ ] **Step 3: Live verify (local)**

1. Select a local session with real work in its worktree. Click 🔍 Review.
2. Dialog opens pre-filled with the multipass template. Edit if desired. Start review.
3. A new `<name>--review-<id>` session spawns in the SAME worktree, shows a 🔍 badge in the Sidebar, and is auto-selected.
4. Attach: the seeded prompt should have fired — the reviewer claude starts running `git diff`.
5. SessionDetails of the review shows "Reviewing: <source>"; SessionDetails of the source shows the review under "Reviews".

- [ ] **Step 4: Live verify (cross-host, if mefistos reachable)**

Review a mefistos session → review spawns on mefistos in the same worktree, prompt fires over SSH.

- [ ] **Step 5: Edge cases**

- Kill the review session via the existing kill flow — works, badge disappears.
- Multi-line prompt with quotes survives the send (same literal-mode path as iter 3's send_prompt).
- Seeding timing: if the prompt didn't land (claude wasn't ready), confirm the fallback is acceptable (user presses Enter / re-sends). If it frequently misses, increase the readiness wait in `wait_for_repl_ready`.

- [ ] **Step 6: Push**

```bash
git push origin main 2>&1 | tail -3
```

- [ ] **Step 7: Document anomalies (if any)**

If seeding timing or cross-host paths surfaced issues, append a "Live verification notes" section to `docs/specs/2026-05-21-iter4b-reviews-design.md` and commit.

---

## Self-Review (filled in by plan author)

**Spec coverage check:**
- Migration 005 (`kind` + `reviews_session_id`) + schema bump → Task 1 ✓
- SessionRow read-path (Rust + TS) → Tasks 1, 2 ✓
- Reconcile preservation → achieved via ON CONFLICT omission (Task 1 design note) + write-once `set_session_kind` ✓
- `spawn_review` (spawn in source worktree + tag + seed) → Task 3 ✓
- Worktree-path resolution order (worktree_id → project base → error) → Task 3 `resolve_review_cwd` ✓ (per-session cwd fallback omitted — claude-fleet doesn't currently store per-session cwd separately from worktree_id; documented as project-base fallback)
- Seeding-timing risk (REPL readiness) → Task 3 `wait_for_repl_ready` ✓
- `spawnReview` wrapper + `DEFAULT_REVIEW_PROMPT` + ReviewDialog → Task 4 ✓
- SessionDetails Review button + Reviewing/Reviews linking → Task 5 ✓
- Sidebar 🔍 badge → Task 6 ✓
- Live verify + push → Task 7 ✓
- Non-goals (auto-trigger, verdict scraping, dashboard, profiles, isolated worktree) → not present in any task ✓

**Placeholder scan:** every code step has concrete code; commit messages are literal; the default prompt and migration SQL are concrete. The one acceptable judgment call is the `wait_for_repl_ready` fallback (fixed sleep vs pane poll), explicitly offered as a choice with a documented default.

**Type consistency:**
- `SessionRow.kind: String` (Rust) ↔ `kind: string` (TS); `reviews_session_id: Option<i64>` ↔ `reviews_session_id: number | null` — consistent.
- `set_session_kind(id, kind, reviews_session_id)` signature consistent between Task 1 (def) and Task 3 (call).
- `spawn_review` args `{ source_session_id, prompt, call_id }` (Rust `SpawnReviewArgs`) ↔ `spawnReview(sourceSessionId, prompt, signal)` wrapping `{ args: { source_session_id, prompt } }` — consistent (call_id injected by `invokeCmdAbortable`).
- `DEFAULT_REVIEW_PROMPT` defined once (Task 4), referenced in ReviewDialog + its test.
- Tasks numbered 1–7; M1=1–2, M2=3, M3=4, M4=5–6, M5=7.
