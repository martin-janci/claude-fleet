# Background-session filter + summary panel — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a sidebar toggle to show/hide background (`kind === 'bg'`) sessions, and replace the broken console for a selected bg session with a status + logs summary panel.

**Architecture:** Frontend-only. A new persisted `showBgAgents` store gates bg rows in the sidebar's two derived session lists. A new `BgSessionPanel.svelte` renders status/activity/PR (already on the row, kept fresh by row events) plus the transcript from the existing `peekSession` (`claude logs <id>`) command, auto-polled every 10s. `App.svelte` renders the panel instead of `TerminalView` for bg sessions, so `pty_open` is never called and the "no tmux" error disappears.

**Tech Stack:** Svelte 5 runes, TypeScript, Vitest + @testing-library/svelte. Tauri command `peek_session` (already exists).

**Spec:** `docs/superpowers/specs/2026-05-23-bg-session-filter-and-summary-design.md`

**Conventions to follow:**
- Persisted prefs use `readPref(key, fallback, guard)` / `writePref(key, value)` from `src/lib/prefs.ts`, mirroring `hostFilter` in `src/lib/hosts.ts:34-35`.
- Backend calls go through `invokeCmd` and return `Result<T>` (`{ ok: true, value } | { ok: false, error }`) from `src/lib/result.ts`.
- Run frontend tests with `pnpm test`. Type-check with `pnpm check`.
- Pre-existing test caveat (per CLAUDE.md): some tests fail with `localStorage is undefined` on `main`. Verify a failure reproduces on `main` before blaming your change.

---

## File Structure

- `src/lib/sessions.ts` — **modify**: add `showBgAgents` persisted store.
- `src/lib/Sidebar.svelte` — **modify**: import store, gate bg rows in `filteredSessionsByProject` (`:169-178`) and `orphanSessions` (`:205-211`), add toggle button to the filter header (near `:711`), add a `bg` badge (near `:589-594`).
- `src/lib/BgSessionPanel.svelte` — **create**: status + activity + PR + auto-polled logs panel.
- `src/App.svelte` — **modify**: render `BgSessionPanel` instead of `TerminalView` when `$selectedSession?.kind === 'bg'` (`:270-282`).
- Tests: `src/lib/sessions.test.ts`, `src/lib/Sidebar.test.ts`, `src/lib/BgSessionPanel.test.ts` (new).

---

## Task 1: `showBgAgents` persisted store

**Files:**
- Modify: `src/lib/sessions.ts:1-2,25` (imports + store definition near the `sessions` store)
- Test: `src/lib/sessions.test.ts`

- [ ] **Step 1: Write the failing test**

Add to `src/lib/sessions.test.ts` (it already imports from `./sessions`; add `showBgAgents` to that import and `get` from `svelte/store` if not present):

```ts
import { get } from 'svelte/store';
import { showBgAgents } from './sessions';

describe('showBgAgents', () => {
  it('defaults to true and persists changes to localStorage', () => {
    localStorage.clear();
    expect(get(showBgAgents)).toBe(true);
    showBgAgents.set(false);
    expect(localStorage.getItem('show-bg-agents')).toBe('false');
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm test -- src/lib/sessions.test.ts -t showBgAgents`
Expected: FAIL — `showBgAgents` is not exported.

- [ ] **Step 3: Implement the store**

In `src/lib/sessions.ts`, update the top import line:

```ts
import { writable } from 'svelte/store';
import { invokeCmd, invokeCmdAbortable, type Result } from './result';
import { readPref, writePref } from './prefs';
```

Then directly after `export const sessions = writable<SessionRow[]>([]);` (line 25) add:

```ts
// Sidebar filter — when false, background (`kind === 'bg'`) sessions are
// hidden from the tree. Defaults to true (shown). Persisted across restarts.
const isBool = (v: unknown): v is boolean => typeof v === 'boolean';
export const showBgAgents = writable<boolean>(readPref('show-bg-agents', true, isBool));
showBgAgents.subscribe((v) => writePref('show-bg-agents', v));
```

- [ ] **Step 4: Run test to verify it passes**

Run: `pnpm test -- src/lib/sessions.test.ts -t showBgAgents`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/lib/sessions.ts src/lib/sessions.test.ts
git commit -m "feat(sessions): add persisted showBgAgents store"
```

---

## Task 2: Sidebar filtering, toggle, and bg badge

**Files:**
- Modify: `src/lib/Sidebar.svelte` (import ~`:24`, derived lists `:169-178` and `:205-211`, header nav ~`:711`, badge ~`:589-594`)
- Test: `src/lib/Sidebar.test.ts`

- [ ] **Step 1: Write the failing test**

Add to `src/lib/Sidebar.test.ts`. It already imports `sessions` and has `sessionFor(projectId, name)`; add `showBgAgents` to the `./sessions` import. Reset it in `beforeEach` (add `showBgAgents.set(true);` next to `hostFilter.set('all');`). Then add:

```ts
import { showBgAgents } from './sessions';

describe('background-session filter', () => {
  it('hides bg sessions when showBgAgents is false, shows them when true', async () => {
    const normal = sessionFor(1, 'dev-1');           // kind: 'work'
    const bg = { ...sessionFor(1, 'bg:abc'), kind: 'bg' };
    mockBackend(fakeProjects, [normal, bg]);
    showBgAgents.set(true);
    render(Sidebar);
    await tick(); await tick();
    expect(screen.queryByText('bg:abc')).not.toBeNull();

    showBgAgents.set(false);
    await tick(); await tick();
    expect(screen.queryByText('bg:abc')).toBeNull();
    // The normal session is unaffected.
    expect(screen.queryByText('dev-1')).not.toBeNull();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm test -- src/lib/Sidebar.test.ts -t "background-session filter"`
Expected: FAIL — `bg:abc` is still visible after `showBgAgents.set(false)` (no filtering yet).

- [ ] **Step 3: Import the store**

In `src/lib/Sidebar.svelte`, the existing import at line 24 reads:

```ts
import { hosts, hostFilter, hostByAlias } from './hosts';
```

Find the import of `sessions` / `SessionRow` (from `./sessions`) and add `showBgAgents` to it. For example if it reads `import { sessions, type SessionRow } from './sessions';` change it to:

```ts
import { sessions, showBgAgents, type SessionRow } from './sessions';
```

- [ ] **Step 4: Gate bg rows in the derived lists**

In `filteredSessionsByProject` (`:169-178`), add the bg check immediately after the existing `hostFilter` continue:

```ts
  const filteredSessionsByProject = $derived.by(() => {
    const m = new Map<number, SessionRow[]>();
    for (const s of $sessions) {
      if (s.project_id == null) continue;
      if ($hostFilter !== 'all' && s.host_alias !== $hostFilter) continue;
      if (!$showBgAgents && s.kind === 'bg') continue;
      if (!m.has(s.project_id)) m.set(s.project_id, []);
      m.get(s.project_id)!.push(s);
    }
    return m;
  });
```

In `orphanSessions` (`:205-211`), add the same condition to the filter predicate:

```ts
  const orphanSessions = $derived(
    $sessions.filter(
      (s) =>
        s.project_id === null &&
        ($hostFilter === 'all' || s.host_alias === $hostFilter) &&
        ($showBgAgents || s.kind !== 'bg'),
    ),
  );
```

- [ ] **Step 5: Run test to verify it passes**

Run: `pnpm test -- src/lib/Sidebar.test.ts -t "background-session filter"`
Expected: PASS.

- [ ] **Step 6: Add the toggle button to the filter header**

In `src/lib/Sidebar.svelte`, the recency nav ends at line 721. Immediately after that `</nav>` (and before the `{#if loadError}` block at `:723`), add a toggle nav:

```svelte
    <nav class="bg-toggle" aria-label="background agents filter">
      <button
        class="pill"
        class:active={$showBgAgents}
        data-testid="bg-toggle"
        title={$showBgAgents ? 'Hide background agents' : 'Show background agents'}
        onclick={() => showBgAgents.update((v) => !v)}
      >
        🤖 bg {$showBgAgents ? 'on' : 'off'}
      </button>
    </nav>
```

(The `.pill` class is already styled and used by the recency nav, so no new CSS is required. `.bg-toggle` inherits nothing it needs.)

- [ ] **Step 7: Add the bg badge to the session row**

In the session row snippet, the `review`/`shell` badges are at `:589-594`. Add a bg badge alongside them — insert after the `shell` badge block (after line 593):

```svelte
          {#if sess.kind === 'bg'}
            <span class="bg-badge" role="img" title="background agent" aria-label="background agent">🤖</span>
          {/if}
```

- [ ] **Step 8: Run the full Sidebar test file + type-check**

Run: `pnpm test -- src/lib/Sidebar.test.ts`
Expected: PASS (the new test plus existing ones; ignore any pre-existing `localStorage is undefined` failures that also fail on `main`).

Run: `pnpm check`
Expected: no new type errors.

- [ ] **Step 9: Commit**

```bash
git add src/lib/Sidebar.svelte src/lib/Sidebar.test.ts
git commit -m "feat(sidebar): bg-agent filter toggle and badge"
```

---

## Task 3: `BgSessionPanel.svelte` with auto-polled logs

**Files:**
- Create: `src/lib/BgSessionPanel.svelte`
- Test: `src/lib/BgSessionPanel.test.ts` (new)

The component takes the selected `SessionRow` as a prop, shows `claude_status`, `current_activity`, and `pr_url`, and fetches the transcript via `peekSession(host_alias, claude_session_id)`. It fetches once on mount and polls every 10s; the interval is cleared on destroy. If `claude_session_id` is null it renders a placeholder and never fetches. On fetch error it keeps the last good transcript and shows an inline error.

- [ ] **Step 1: Write the failing test**

Create `src/lib/BgSessionPanel.test.ts`:

```ts
import { render, screen } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import BgSessionPanel from './BgSessionPanel.svelte';
import type { SessionRow } from './sessions';

function bgSession(over: Partial<SessionRow> = {}): SessionRow {
  return {
    id: 1, tmux_name: 'bg:abc', host_alias: 'local', project_id: 1, worktree_id: null,
    created_at: 1, last_activity_at: 1, status: 'running', notes: null, account_uuid: null,
    kind: 'bg', reviews_session_id: null, worktree_key: null, lost_at: null,
    claude_session_id: 'sess-abc', claude_status: 'working', effort_level: null,
    pr_url: null, current_activity: 'editing files', ...over,
  };
}

beforeEach(() => {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
});

describe('BgSessionPanel', () => {
  it('renders status and activity and the fetched transcript', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValue('hello from claude logs');
    render(BgSessionPanel, { session: bgSession() });
    await tick(); await tick(); await Promise.resolve(); await tick();
    expect(screen.getByText('working')).toBeTruthy();
    expect(screen.getByText('editing files')).toBeTruthy();
    expect(await screen.findByText(/hello from claude logs/)).toBeTruthy();
    expect(mockedInvoke).toHaveBeenCalledWith('peek_session', { args: { host_alias: 'local', claude_session_id: 'sess-abc' } });
  });

  it('renders a placeholder and does not fetch when claude_session_id is null', async () => {
    render(BgSessionPanel, { session: bgSession({ claude_session_id: null }) });
    await tick(); await tick();
    expect(screen.getByText(/no logs available yet/i)).toBeTruthy();
    expect(mockedInvoke).not.toHaveBeenCalled();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm test -- src/lib/BgSessionPanel.test.ts`
Expected: FAIL — module `./BgSessionPanel.svelte` does not exist.

- [ ] **Step 3: Implement the component**

Create `src/lib/BgSessionPanel.svelte`:

```svelte
<script lang="ts">
  import { onDestroy } from 'svelte';
  import { peekSession, type SessionRow } from './sessions';

  let { session }: { session: SessionRow } = $props();

  const POLL_MS = 10_000;

  let logs = $state<string>('');
  let logError = $state<string | null>(null);
  let loading = $state(false);
  let timer: ReturnType<typeof setInterval> | undefined;

  async function fetchLogs() {
    const id = session.claude_session_id;
    if (!id) return;
    loading = true;
    const r = await peekSession(session.host_alias, id);
    loading = false;
    if (r.ok) {
      logs = r.value;
      logError = null;
    } else {
      // Keep the last good transcript; surface the error inline.
      // `r.error` is an IpcError (see src/lib/result.ts) with a `.message: string`.
      logError = r.error.message;
    }
  }

  // Re-arm whenever the selected bg session changes. Clears any prior interval,
  // resets state, fetches immediately, then polls. No fetch when there is no
  // claude_session_id (synthetic row with nothing to read).
  $effect(() => {
    const id = session.claude_session_id;
    if (timer) clearInterval(timer);
    logs = '';
    logError = null;
    if (!id) return;
    void fetchLogs();
    timer = setInterval(() => void fetchLogs(), POLL_MS);
    return () => { if (timer) clearInterval(timer); };
  });

  onDestroy(() => { if (timer) clearInterval(timer); });
</script>

<div class="bg-panel" data-testid="bg-panel">
  <header class="bg-head">
    <span class="bg-name">🤖 {session.tmux_name}</span>
    {#if session.claude_status}
      <span class="bg-status" data-testid="bg-status">{session.claude_status}</span>
    {/if}
    <button
      class="refresh"
      data-testid="bg-refresh"
      disabled={!session.claude_session_id || loading}
      onclick={() => void fetchLogs()}
    >↻ Refresh</button>
  </header>

  {#if session.current_activity}
    <p class="bg-activity">{session.current_activity}</p>
  {/if}
  {#if session.pr_url}
    <p class="bg-pr"><a href={session.pr_url} target="_blank" rel="noreferrer">{session.pr_url}</a></p>
  {/if}

  {#if !session.claude_session_id}
    <p class="bg-empty">Background agent — no logs available yet.</p>
  {:else}
    {#if logError}
      <p class="err" data-testid="bg-log-error">log error: {logError}</p>
    {/if}
    <pre class="bg-logs" data-testid="bg-logs">{logs}</pre>
  {/if}
</div>

<style>
  .bg-panel { display: flex; flex-direction: column; height: 100%; padding: 0.5rem; gap: 0.4rem; overflow: hidden; }
  .bg-head { display: flex; align-items: center; gap: 0.5rem; }
  .bg-name { font-weight: 600; }
  .bg-status { font-size: 0.85em; opacity: 0.85; }
  .refresh { margin-left: auto; }
  .bg-activity { margin: 0; font-size: 0.9em; opacity: 0.9; }
  .bg-pr { margin: 0; font-size: 0.85em; }
  .bg-empty { opacity: 0.7; font-style: italic; }
  .bg-logs { flex: 1; overflow: auto; white-space: pre-wrap; font-family: var(--mono, monospace); font-size: 0.85em; margin: 0; }
  .err { color: var(--err, #c00); font-size: 0.85em; margin: 0; }
</style>
```

- [ ] **Step 4: Run test to verify it passes**

Run: `pnpm test -- src/lib/BgSessionPanel.test.ts`
Expected: PASS (both cases).

- [ ] **Step 5: Type-check**

Run: `pnpm check`
Expected: no new type errors.

- [ ] **Step 6: Commit**

```bash
git add src/lib/BgSessionPanel.svelte src/lib/BgSessionPanel.test.ts
git commit -m "feat: BgSessionPanel with auto-polled claude logs"
```

---

## Task 4: Wire the panel into App.svelte

**Files:**
- Modify: `src/App.svelte` (import near `:8`, right-body render `:270-282`)

- [ ] **Step 1: Import the component**

In `src/App.svelte`, after the `TerminalView` import (line 8):

```ts
  import BgSessionPanel from './lib/BgSessionPanel.svelte';
```

- [ ] **Step 2: Render conditionally in the right body**

Replace the right-body block (`:270-282`) so bg sessions get the panel and `TerminalView` is not mounted for them (this is what removes the `pty_open` "no tmux" error):

```svelte
    <div class="right-body">
      {#if $selectedSession?.kind === 'bg'}
        <div class="view-slot">
          <BgSessionPanel session={$selectedSession} />
        </div>
      {:else}
        <!-- TerminalView stays mounted underneath so the PTY and its ANSI
             buffer survive a Files-mode round trip — flipping back is instant
             and never re-fits or reconnects the terminal. -->
        <div class="view-slot">
          <TerminalView />
        </div>
        {#if filesMode && $selectedSession}
          <div class="view-slot overlay">
            <FilesPanel session={$selectedSession} />
          </div>
        {/if}
      {/if}
    </div>
```

- [ ] **Step 3: Type-check and run the full frontend suite**

Run: `pnpm check`
Expected: no new type errors.

Run: `pnpm test`
Expected: PASS, except any pre-existing `localStorage is undefined` failures (`session_ui.test.ts`, `App.test.ts`, …) that also fail on `main` — verify with `git stash && pnpm test` on `main` if unsure.

- [ ] **Step 4: Manual smoke check (optional but recommended)**

Run the app (`pnpm tauri dev` if the environment has the Tauri system libs). Select a background agent: the right pane shows status/activity/logs instead of a PTY error. Toggle "🤖 bg off" in the sidebar: bg rows disappear from the tree; toggle on: they return.

- [ ] **Step 5: Commit**

```bash
git add src/App.svelte
git commit -m "feat(app): show BgSessionPanel for bg sessions instead of terminal"
```

---

## Self-review notes (for the implementer)

- **Spec coverage:** Task 1 = `showBgAgents` store; Task 2 = sidebar toggle + filter exclusion (both derived lists) + bg badge; Task 3 = summary panel (status/activity/PR/logs), placeholder for missing id, 10s poll, manual refresh, keep-last-good-on-error; Task 4 = right-pane swap that eliminates the `pty_open` error path. All spec sections are covered.
- **Type consistency:** `showBgAgents` (boolean store), `peekSession(hostAlias, claudeSessionId): Promise<Result<string>>`, `SessionRow.kind === 'bg'`, and `Result.error` as `IpcError` with `.message: string` (confirmed in `src/lib/result.ts`) are used consistently across tasks.
