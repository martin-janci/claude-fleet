# Session Tab Merge Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the separate `Terminal` and `Conversation` tabs with one `Session` tab that holds both, switched by a segmented control at the right end of the same strip.

**Architecture:** The four mutually-exclusive mode flags in `App.svelte` stay as they are; only `conversationMode` changes from `$state` to `$derived`. Which sub-view shows is decided by one pure function, `resolveSessionView(pref, noPane, hasClaudeId)`, fed by a new localStorage-backed `sessionView` pref. The Conversation-over-Terminal overlay stacking is untouched, so the PTY still survives every round trip.

**Tech Stack:** Svelte 5 runes, TypeScript, Vitest + @testing-library/svelte, jsdom.

**Spec:** `docs/superpowers/specs/2026-09-20-session-tab-merge-design.md`

## Global Constraints

- Frontend only. No Rust, no migrations, no MCP tool changes — so
  `docs/control-api-reference.md` does not need regenerating.
- **Run Vitest with `--pool=threads`.** The default `forks` pool times out
  on this machine with `[vitest-pool]: Failed to start forks worker`. Every
  test command in this plan already carries the flag.
- Package manager is `corepack pnpm@10` (local `pnpm` 9 is broken on this
  host). Dependencies are already installed in this worktree.
- Baseline on this branch is green: `src/App.test.ts` and
  `src/App.hosts.test.ts` = 36 passing, `src/lib/app_views.test.ts` = 4
  passing. Any failure you see is yours.
- The pref default is `'conversation'` on a clean `localStorage`.
- Keep the existing comment style: explain *why*, not *what*. The codebase
  comments decisions and tradeoffs, never restates the code.
- Conventional Commits — release-please reads them. Never bump a version by
  hand.

---

### Task 1: The sub-view decision and its pref

**Files:**
- Create: `src/lib/session_view.ts`
- Create: `src/lib/session_view.test.ts`
- Modify: `src/lib/prefs.ts` (append a new section at the end)

**Interfaces:**
- Consumes: `readPref` / `writePref` from `src/lib/prefs.ts`.
- Produces:
  - `type SessionView = 'conversation' | 'terminal'`
  - `resolveSessionView(pref: SessionView, noPane: boolean, hasClaudeId: boolean): SessionView`
  - `otherSessionView(v: SessionView): SessionView`
  - `sessionView: Writable<SessionView>` exported from `src/lib/prefs.ts`

- [ ] **Step 1: Write the failing test**

Create `src/lib/session_view.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { resolveSessionView, otherSessionView } from './session_view';

describe('resolveSessionView', () => {
  it('honours the stored preference when the row can show either view', () => {
    expect(resolveSessionView('conversation', false, true)).toBe('conversation');
    expect(resolveSessionView('terminal', false, true)).toBe('terminal');
  });

  it('forces Conversation on a row with no tmux pane — there is no PTY to show', () => {
    expect(resolveSessionView('terminal', true, true)).toBe('conversation');
    expect(resolveSessionView('conversation', true, true)).toBe('conversation');
  });

  it('forces Terminal on a row with no claude_session_id — there is no transcript yet', () => {
    expect(resolveSessionView('conversation', false, false)).toBe('terminal');
    expect(resolveSessionView('terminal', false, false)).toBe('terminal');
  });

  it('prefers Conversation when a row is both pane-less and id-less', () => {
    expect(resolveSessionView('terminal', true, false)).toBe('conversation');
    expect(resolveSessionView('conversation', true, false)).toBe('conversation');
  });
});

describe('otherSessionView', () => {
  it('flips between the two views', () => {
    expect(otherSessionView('conversation')).toBe('terminal');
    expect(otherSessionView('terminal')).toBe('conversation');
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
corepack pnpm@10 exec vitest run --pool=threads src/lib/session_view.test.ts
```

Expected: FAIL — `Failed to load url ./session_view` (the module does not exist yet).

- [ ] **Step 3: Write the implementation**

Create `src/lib/session_view.ts`:

```ts
/**
 * Which of the Session tab's two sub-views to show.
 *
 * Conversation and Terminal are two views of one running session, so the
 * choice between them is a remembered preference rather than a side effect
 * of which mode flag happens to be false. Some rows can only offer one of
 * the two; those override the preference for that row *without* rewriting
 * it, so stepping off such a row puts you back where you were.
 */
export type SessionView = 'conversation' | 'terminal';

export function resolveSessionView(
  pref: SessionView,
  noPane: boolean,
  hasClaudeId: boolean,
): SessionView {
  // No tmux pane (a background agent, an external Claude session) means no
  // PTY. This wins over the missing-transcript case below: a row that is
  // both has nothing else to offer, and ConversationPanel has its own empty
  // state for exactly that.
  if (noPane) return 'conversation';
  // The session has not reported a Claude session id, so there is no
  // transcript to render.
  if (!hasClaudeId) return 'terminal';
  return pref;
}

export function otherSessionView(v: SessionView): SessionView {
  return v === 'conversation' ? 'terminal' : 'conversation';
}
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
corepack pnpm@10 exec vitest run --pool=threads src/lib/session_view.test.ts
```

Expected: PASS — `Test Files 1 passed (1)`, `Tests 5 passed (5)`.

- [ ] **Step 5: Add the pref**

Append to the end of `src/lib/prefs.ts`:

```ts
// ─── Right-panel prefs ───────────────────────────────────────────────────

const isSessionView = (v: unknown): v is SessionView =>
  v === 'conversation' || v === 'terminal';

/**
 * Which sub-view the Session tab shows. One choice for the whole app, kept
 * across restarts — picking a session should not decide for you which of
 * its two views you get. Rows that can only offer one view override this
 * without writing to it; see `resolveSessionView`.
 */
export const sessionView = writable<SessionView>(
  readPref<SessionView>('ui.sessionView', 'conversation', isSessionView),
);
sessionView.subscribe((v) => writePref('ui.sessionView', v));
```

And add the type import to the top of the file, below the existing
`import { detectMac } from './terminal_keys';`:

```ts
import type { SessionView } from './session_view';
```

- [ ] **Step 6: Type-check**

```bash
corepack pnpm@10 run check
```

Expected: no new errors mentioning `prefs.ts` or `session_view.ts`.

- [ ] **Step 7: Commit**

```bash
git add src/lib/session_view.ts src/lib/session_view.test.ts src/lib/prefs.ts
git commit -m "feat(ui): sessionView pref and the rule that resolves it"
```

---

### Task 2: The ⌘J / Ctrl+Shift+J chord

**Files:**
- Modify: `src/lib/app_views.ts:60` (the `AppChord` type), `src/lib/app_views.ts:67-79` (`appChord`), and the end of the file
- Test: `src/lib/app_views.test.ts`

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces:
  - `type AppChord = 'hosts' | 'settings' | 'session-view'`
  - `sessionViewChordLabel(isMac: boolean): string`

- [ ] **Step 1: Write the failing test**

In `src/lib/app_views.test.ts`, change the import line to:

```ts
import { appChord, hostsChordLabel, sessionViewChordLabel } from './app_views';
```

and add these two tests inside the existing `describe('appChord', ...)` block,
after the `'labels the Hosts chord per platform'` test:

```ts
  it('⌘J / Ctrl+Shift+J flips the Session view; plain Ctrl+J stays with the terminal', () => {
    expect(appChord(ev('j', { metaKey: true }), true)).toBe('session-view');
    expect(appChord(ev('J', { metaKey: true }), true)).toBe('session-view');
    expect(appChord(ev('J', { ctrlKey: true, shiftKey: true }), false)).toBe('session-view');
    // Plain Ctrl+J is line-feed in a terminal — it must reach the PTY.
    expect(appChord(ev('j', { ctrlKey: true }), false)).toBeNull();
    expect(appChord(ev('j', { ctrlKey: true }), true)).toBeNull();
    expect(appChord(ev('j'), true)).toBeNull();
    // Modifier variants are not the chord, same rule as ⌘I.
    expect(appChord(ev('j', { metaKey: true, altKey: true }), true)).toBeNull();
    expect(appChord(ev('j', { metaKey: true, shiftKey: true }), true)).toBeNull();
  });

  it('labels the Session-view chord per platform', () => {
    expect(sessionViewChordLabel(true)).toBe('⌘J');
    expect(sessionViewChordLabel(false)).toBe('Ctrl+Shift+J');
  });
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
corepack pnpm@10 exec vitest run --pool=threads src/lib/app_views.test.ts
```

Expected: FAIL — `sessionViewChordLabel is not a function`, and the chord
assertions get `null` instead of `'session-view'`.

- [ ] **Step 3: Write the implementation**

In `src/lib/app_views.ts`, widen the type:

```ts
export type AppChord = 'hosts' | 'settings' | 'session-view';
```

Replace the doc comment and body of `appChord` with:

```ts
/**
 * The app-level chords, platform-correct like the quick switcher's:
 * ⌘I toggles Hosts, ⌘J flips the Session view and ⌘, opens Settings (Cmd
 * never reaches the PTY); on non-mac Ctrl+Shift+H and Ctrl+Shift+J do the
 * same two views (Ctrl+Shift+I is the devtools chord). Plain Ctrl chords
 * stay with the terminal — Ctrl+J is line-feed there.
 */
export function appChord(
  e: { key: string; metaKey: boolean; ctrlKey: boolean; altKey: boolean; shiftKey: boolean },
  isMac: boolean,
): AppChord | null {
  if (e.altKey) return null;
  const k = e.key.toLowerCase();
  if (e.metaKey && !e.ctrlKey && !e.shiftKey) {
    if (k === 'i') return 'hosts';
    if (k === 'j') return 'session-view';
    if (k === ',') return 'settings';
    return null;
  }
  if (!isMac && e.ctrlKey && e.shiftKey && !e.metaKey) {
    if (k === 'h') return 'hosts';
    if (k === 'j') return 'session-view';
  }
  return null;
}
```

Append after `hostsChordLabel`:

```ts
/** Label for the Session-view chord, for the segment's tooltip. */
export function sessionViewChordLabel(isMac: boolean): string {
  return isMac ? '⌘J' : 'Ctrl+Shift+J';
}
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
corepack pnpm@10 exec vitest run --pool=threads src/lib/app_views.test.ts
```

Expected: PASS — `Tests 6 passed (6)`.

- [ ] **Step 5: Commit**

```bash
git add src/lib/app_views.ts src/lib/app_views.test.ts
git commit -m "feat(ui): add the session-view chord to appChord"
```

---

### Task 3: The Session tab and its segment

This is the integration task. The markup change and the state change cannot
land separately — the strip's tests fail with either one alone.

**Files:**
- Modify: `src/App.svelte` — imports (~line 32 and 42), the `conversationMode`
  block (lines 285-308), `openHosts` (line 349), the `show*` helpers (lines
  362-388), `onChordKeydown` (lines 434-443), the strip markup (lines 590-646),
  the `.view-tabs` CSS (lines 871-917)
- Test: `src/App.test.ts`, `src/App.hosts.test.ts`

**Interfaces:**
- Consumes: `sessionView` from `src/lib/prefs.ts`; `resolveSessionView`,
  `otherSessionView`, `type SessionView` from `src/lib/session_view.ts`;
  `sessionViewChordLabel` and the `'session-view'` `AppChord` from
  `src/lib/app_views.ts`.
- Produces: the test ids `tab-session`, `subtab-conversation`,
  `subtab-terminal`. `tab-terminal` and `tab-conversation` cease to exist.

- [ ] **Step 1: Write the failing tests**

In `src/App.test.ts`, replace the test
`'marks only the Assets tab active (not Terminal) when Assets is open'`
(lines 117-124) with:

```ts
  it('marks only the Assets tab active (not Session) when Assets is open', async () => {
    const { getByTestId } = render(App);
    await fireEvent.click(getByTestId('tab-assets'));
    expect(getByTestId('tab-session').classList.contains('active')).toBe(false);
    expect(getByTestId('tab-session').getAttribute('aria-selected')).toBe('false');
    expect(getByTestId('tab-assets').classList.contains('active')).toBe(true);
    expect(getByTestId('tab-assets').getAttribute('aria-selected')).toBe('true');
  });
```

In the `describe('App: the Conversation tab', ...)` block, replace the two
tests at lines 191-236 with:

```ts
  const subtab = (id: string) => screen.getByTestId(id) as HTMLButtonElement;
  const checked = (id: string) => subtab(id).getAttribute('aria-checked');

  it('the Conversation sub-view is disabled without a claude_session_id and enabled with one', async () => {
    await mountAndSelect(noId);
    expect(subtab('subtab-conversation').disabled).toBe(true);
    expect(subtab('subtab-conversation').title).toBe('No Claude session id yet');
    // With no transcript the row falls back to the terminal, without
    // touching the stored preference.
    expect(checked('subtab-terminal')).toBe('true');
    await select(work);
    expect(subtab('subtab-conversation').disabled).toBe(false);
  });

  it('defaults to Conversation, and the segment flips between the two views', async () => {
    await mountAndSelect(work);
    const grid = await screen.findByTestId('terminal-host');
    // Default pref is 'conversation'.
    expect(screen.getByTestId('conversation-panel')).toBeInTheDocument();
    expect(checked('subtab-conversation')).toBe('true');
    expect(selected('tab-session')).toBe('true');
    // The PTY stays mounted underneath.
    expect(grid.isConnected).toBe(true);
    // The center (Details) pane stays visible, unlike Files/Hosts.
    expect(screen.getByTestId('pane-center')).toBeInTheDocument();

    await fireEvent.click(subtab('subtab-terminal'));
    await tick();
    expect(screen.queryByTestId('conversation-panel')).toBeNull();
    expect(checked('subtab-terminal')).toBe('true');
    expect(selected('tab-session')).toBe('true');

    await fireEvent.click(subtab('subtab-conversation'));
    await tick();
    expect(screen.getByTestId('conversation-panel')).toBeInTheDocument();
  });

  it('Files and Hosts take the panel from the Session tab and give it back', async () => {
    await mountAndSelect(work);
    expect(screen.getByTestId('conversation-panel')).toBeInTheDocument();

    await fireEvent.click(tab('tab-files'));
    await tick();
    expect(screen.queryByTestId('conversation-panel')).toBeNull();
    expect(selected('tab-files')).toBe('true');
    expect(selected('tab-session')).toBe('false');
    // The segment belongs to the Session tab; it is gone while another view
    // owns the panel. (A session *is* selected here, so this is not passing
    // for the trivial reason.)
    expect(screen.queryByTestId('subtab-conversation')).toBeNull();
    expect(screen.queryByTestId('subtab-terminal')).toBeNull();

    await fireEvent.click(tab('tab-session'));
    await tick();
    // Back to the remembered sub-view, not to the terminal.
    expect(screen.getByTestId('conversation-panel')).toBeInTheDocument();

    await fireEvent.click(tab('tab-hosts'));
    await tick();
    expect(screen.queryByTestId('conversation-panel')).toBeNull();
    expect(selected('tab-hosts')).toBe('true');
    expect(selected('tab-session')).toBe('false');
  });

  it('a pane-less row shows Conversation without overwriting the stored preference', async () => {
    const { sessionView } = await import('./lib/prefs');
    const { get } = await import('svelte/store');
    await mountAndSelect(work);
    await fireEvent.click(subtab('subtab-terminal'));
    await tick();
    expect(get(sessionView)).toBe('terminal');

    await select(bg);
    await tick();
    // No PTY on this row, so Conversation regardless of the preference.
    expect(screen.getByTestId('conversation-panel')).toBeInTheDocument();
    expect(subtab('subtab-terminal').disabled).toBe(true);
    expect(subtab('subtab-terminal').title).toBe(NO_PANE_TITLE_TEXT);
    expect(get(sessionView)).toBe('terminal');

    await select(work);
    await tick();
    // Back on a tmux row: the preference survived the detour.
    expect(screen.queryByTestId('conversation-panel')).toBeNull();
    expect(checked('subtab-terminal')).toBe('true');
  });
```

Beside the existing `tab` / `selected` helpers in that `describe` block, add
the constant the last test needs:

```ts
  const NO_PANE_TITLE_TEXT = 'Runs outside tmux — no terminal';
```

The `bg` fixture (line 143, `kind: 'bg'` — what `hasNoPane()` keys on, see
`src/lib/sessions.ts:548`) already exists; no new fixture is needed. This
file imports `get` from `svelte/store` dynamically inside helpers rather than
at the top, so the two new tests that need it do the same:

```ts
    const { get } = await import('svelte/store');
```

**Reset the pref between tests.** `sessionView` is a module-level store read
from `localStorage` once at import time, so a test that switches to Terminal
would leak into every test after it. Add to that `describe`'s existing
`beforeEach`, beside `localStorage.removeItem('cf:pref:session.last');`:

```ts
    localStorage.removeItem('cf:pref:ui.sessionView');
    const { sessionView } = await import('./lib/prefs');
    sessionView.set('conversation');
```

In `src/App.hosts.test.ts`, replace every `tab-terminal` occurrence (lines
128, 185, 259) with `tab-session`. The three sites are:
- line 128: `expect(screen.getByTestId('tab-session').getAttribute('aria-selected')).toBe(...)`
- line 185: `await fireEvent.click(screen.getByTestId('tab-session'));`
- line 259: `await fireEvent.click(screen.getByTestId('tab-session'));`

- [ ] **Step 2: Run the tests to verify they fail**

```bash
corepack pnpm@10 exec vitest run --pool=threads src/App.test.ts src/App.hosts.test.ts
```

Expected: FAIL — `Unable to find an element by: [data-testid="tab-session"]`.

- [ ] **Step 3: Wire the imports**

In `src/App.svelte`, add `sessionViewChordLabel` to the existing
`from './lib/app_views'` import block (it already imports `appChord`), then
change line 42 and add one import below it:

```ts
import { readPref, writePref, sessionView } from './lib/prefs';
import { resolveSessionView, otherSessionView, type SessionView } from './lib/session_view';
```

- [ ] **Step 4: Replace the conversationMode state with derived values**

Delete lines 285-308 — the whole comment block, `let conversationMode = $state(false);`,
`let prevNoPane = false;` and the `$effect` that follows — and put in their place:

```ts
  // Conversation and Terminal are two views of one session under a single
  // Session tab, so neither is "no mode set": which one shows is the stored
  // preference, narrowed by what this row can actually offer. Conversation
  // reuses the Files overlay for a tmux row, so the PTY stays mounted
  // underneath. Unlike Files/Hosts the Session tab keeps the center
  // (Details) pane — both its views are views *of* the session.
  const sessionTabActive = $derived(!filesMode && !assetsMode && !hostsMode);
  const effectiveView = $derived(resolveSessionView($sessionView, selNoPane, selHasClaudeId));
  const conversationMode = $derived(sessionTabActive && effectiveView === 'conversation');
```

- [ ] **Step 5: Drop the stale conversationMode assignment in openHosts**

In `openHosts`, delete these two lines (currently 347-349):

```ts
    // A no-pane row has nothing but the Conversation under the Hosts overlay,
    // so it stays the view to return to; a tmux row returns to the terminal.
    if (!selNoPane) conversationMode = false;
```

`conversationMode` is derived now — `hostsMode = true` already turns it off
through `sessionTabActive`, and closing Hosts restores the remembered view.

- [ ] **Step 6: Rewrite the show* helpers**

Replace `showTerminal`, `showFiles`, `showConversation` and `showAssets`
(lines 362-388, keeping `showFiles` and `showAssets` behaviour intact) with:

```ts
  function showSession() {
    filesMode = false;
    assetsMode = false;
    closeHosts();
  }
  function showFiles() {
    if (!$selectedSession) return;
    closeHosts(false);
    assetsMode = false;
    filesMode = true;
  }
  function showAssets() {
    closeHosts(false);
    filesMode = false;
    assetsMode = true;
  }
  /** Pick a sub-view. A row that cannot show it is left alone. */
  function setSessionView(v: SessionView) {
    if (resolveSessionView(v, selNoPane, selHasClaudeId) !== v) return;
    sessionView.set(v);
    showSession();
  }
  /** ⌘J: flip to the other sub-view, closing whatever covers the panel. */
  function flipSessionView() {
    setSessionView(otherSessionView(effectiveView));
  }
```

Then fix the two remaining call sites of the old `showTerminal`:
- the `ConversationPanel` prop in the strip body:
  `onOpenTerminal={() => setSessionView('terminal')}`
- anywhere else `showTerminal` is referenced — search with
  `grep -n showTerminal src/App.svelte` and replace each with
  `() => setSessionView('terminal')`.

Also delete `showConversation` entirely; `setSessionView('conversation')`
replaces it.

- [ ] **Step 7: Handle the chord**

In `onChordKeydown` (lines 434-443), replace the dispatch tail:

```ts
    if (chord === 'hosts') toggleHosts();
    else if (chord === 'session-view') flipSessionView();
    else settingsOpen.set(true);
```

- [ ] **Step 8: Replace the strip markup**

Replace the `Terminal` button (lines 591-600) with the `Session` tab, delete
the `Conversation` button (lines 615-624) entirely, and put the segment plus
`Hosts` inside a trailing container. The strip becomes:

```svelte
    <div class="view-tabs" role="tablist">
      <button
        class="view-tab"
        class:active={sessionTabActive}
        role="tab"
        aria-selected={sessionTabActive}
        disabled={!$selectedSession}
        title={!$selectedSession ? 'Select a session first' : 'The running session — its conversation and its terminal'}
        onclick={showSession}
        data-testid="tab-session">Session</button
      >
      <button
        class="view-tab"
        class:active={filesMode}
        role="tab"
        aria-selected={filesMode}
        disabled={!$selectedSession || selNoPane}
        title={!$selectedSession
          ? 'Select a session first'
          : selNoPane
            ? NO_PANE_TITLE
            : 'Browse the session worktree'}
        onclick={showFiles}
        data-testid="tab-files">Files</button
      >
      <!-- Fleet-scoped like Hosts: never disabled, no selected session needed. -->
      <button
        class="view-tab"
        class:active={assetsMode && !hostsMode}
        role="tab"
        aria-selected={assetsMode && !hostsMode}
        title="The asset catalog and its per-host drift state"
        onclick={showAssets}
        data-testid="tab-assets">Assets</button
      >
      <!-- Always present so Hosts keeps its place; the segment inside it
           appears only while the Session tab owns the panel. Not a nested
           tablist — two tablists in one strip would have a screen reader
           announce two independent tab positions for one place. -->
      <div class="tab-tail">
        {#if sessionTabActive && $selectedSession}
          <div class="subtabs" role="radiogroup" aria-label="Session view">
            <button
              class="subtab"
              class:active={effectiveView === 'conversation'}
              role="radio"
              aria-checked={effectiveView === 'conversation'}
              aria-keyshortcuts={isMac ? 'Meta+J' : 'Control+Shift+J'}
              disabled={!selHasClaudeId && !selNoPane}
              title={!selHasClaudeId && !selNoPane
                ? 'No Claude session id yet'
                : `Claude conversation from the transcript (${sessionViewChord})`}
              onclick={() => setSessionView('conversation')}
              data-testid="subtab-conversation">Conversation</button
            >
            <button
              class="subtab"
              class:active={effectiveView === 'terminal'}
              role="radio"
              aria-checked={effectiveView === 'terminal'}
              aria-keyshortcuts={isMac ? 'Meta+J' : 'Control+Shift+J'}
              disabled={selNoPane}
              title={selNoPane ? NO_PANE_TITLE : `The tmux pane (${sessionViewChord})`}
              onclick={() => setSessionView('terminal')}
              data-testid="subtab-terminal">Terminal</button
            >
          </div>
        {/if}
      </div>
      <!-- Fleet-scoped, so set apart on the right and never disabled. -->
      <button
        class="view-tab hosts-tab"
        class:active={hostsMode}
        role="tab"
        aria-selected={hostsMode}
        aria-keyshortcuts={isMac ? 'Meta+I' : 'Control+Shift+H'}
        title="Every host, grouped by Claude account ({hostsChord})"
        onclick={toggleHosts}
        data-testid="tab-hosts">Hosts <kbd>{hostsChord}</kbd></button
      >
    </div>
```

Add the chord label beside the existing `const hostsChord = hostsChordLabel(isMac);`:

```ts
  const sessionViewChord = sessionViewChordLabel(isMac);
```

- [ ] **Step 9: Style the segment**

In the `<style>` block, change `.hosts-tab`'s `margin-left: auto` to
`margin-left: 0.75rem` and add, just above `.hosts-tab`:

```css
  /* Claims the free space so Hosts stays pinned right whether or not the
     segment is showing. */
  .tab-tail {
    margin-left: auto;
    display: flex;
    align-items: center;
  }
  /* A pill, deliberately unlike the tabs above it: this is a switch within
     the active tab, not a sibling of it. */
  .subtabs {
    display: flex;
    gap: 1px;
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 1px;
    margin-bottom: 0.2rem;
  }
  .subtab {
    background: transparent;
    border: none;
    border-radius: 999px;
    color: var(--fg-muted);
    cursor: pointer;
    font-size: 0.7rem;
    padding: 0.1rem 0.6rem;
  }
  .subtab:hover:not(:disabled) { color: var(--fg); }
  .subtab.active {
    background: var(--bg);
    color: var(--fg);
  }
  .subtab:disabled { opacity: 0.4; cursor: not-allowed; }
```

- [ ] **Step 10: Run the tests**

```bash
corepack pnpm@10 exec vitest run --pool=threads src/App.test.ts src/App.hosts.test.ts
```

Expected: PASS, both files green. If a test still references `tab-terminal`
or `tab-conversation`, grep for stragglers:

```bash
grep -rn "tab-terminal\|tab-conversation" src/
```

Expected: no matches.

- [ ] **Step 11: Type-check**

```bash
corepack pnpm@10 run check
```

Expected: no errors in `App.svelte`.

- [ ] **Step 12: Commit**

```bash
git add src/App.svelte src/App.test.ts src/App.hosts.test.ts
git commit -m "feat(ui): merge Terminal and Conversation into one Session tab"
```

---

### Task 4: Full-suite verification

**Files:** none changed unless a straggler test breaks.

**Interfaces:**
- Consumes: everything from Tasks 1-3.
- Produces: a green suite.

- [ ] **Step 1: Run the whole frontend suite**

```bash
corepack pnpm@10 exec vitest run --pool=threads
```

Expected: all files pass. Any failure naming `tab-terminal`,
`tab-conversation` or `conversationMode` is a straggler from Task 3 — fix
the test to use `tab-session` / `subtab-*` and re-run.

- [ ] **Step 2: Type-check the whole project**

```bash
corepack pnpm@10 run check
```

Expected: no errors.

- [ ] **Step 3: Commit any straggler fixes**

```bash
git add -A
git commit -m "test: update remaining view-tab assertions for the Session tab"
```

Skip this step if Step 1 was green on the first run.

---

## Manual smoke test (optional)

A Tauri build needs the system libraries (dbus, gtk/atk, pkg-config) and
fails on a headless box — that is an environment gap, not a code error. Where
the app does run:

1. Clear `localStorage` key `cf:pref:ui.sessionView`, launch, select a tmux
   session → the Conversation shows, segment reads `Conversation`.
2. Press `⌘J` / `Ctrl+Shift+J` → the Terminal shows, PTY alive and scrolled
   where you left it.
3. Click `Files`, then `Session` → back on the Terminal, not the
   Conversation.
4. Select a background agent → the Conversation shows, `Terminal` in the
   segment is disabled. Select the tmux row again → the Terminal is back.
5. Restart the app → it opens on the Terminal.
