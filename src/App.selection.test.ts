// Redesign step 3.4: the selection drives every pane. A task picked in the
// Work view shows its detail beside one of its own sessions or none, never
// beside the session another task had open.
import { render, waitFor } from '@testing-library/svelte';
import { describe, it, beforeAll, expect, beforeEach, afterEach, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { get } from 'svelte/store';
import App from './App.svelte';
import { onboardingDismissed } from './lib/onboarding';
import { clearToasts } from './lib/toasts';
import { sessionView } from './lib/prefs';
import { sessions } from './lib/sessions';
import { session } from './lib/hosts_fixture';
import { clearSelection, closeTask, selectedSession, selectSessionExplicitly } from './lib/selection';
import { openTask, selectedTaskId, sidebarView } from './lib/work_view';
import { preloadLazyViews } from './lib/lazy_views';
import { POP_BACK_IN_EVENT } from './lib/terminal_popout';

// The off-screen views load lazily in the app; here they are in place
// before the first render, so a test sees them on the frame they open.
beforeAll(() => preloadLazyViews());

const s225 = session('mercury', 'task-225', { id: 225, claude_session_id: 'c-225' });
const s223 = session('mercury', 'task-223', { id: 223, claude_session_id: 'c-223' });

const inv = vi.mocked(invoke);
let original: ((cmd: string, ...rest: unknown[]) => Promise<unknown>) | undefined;

beforeEach(() => {
  original = inv.getMockImplementation() as typeof original;
  // The bootstrap's own read answers the same rows, or it would empty the store.
  inv.mockImplementation(async (cmd: string, ...rest: unknown[]) => {
    if (cmd === 'list_sessions') return [s225, s223];
    if (cmd === 'session_conversation') return { turns: [], truncated: false, context: null, events: [] };
    return original ? original(cmd, ...rest) : null;
  });
  onboardingDismissed.set(true);
  clearToasts();
  sessions.set([s225, s223]);
  sidebarView.set('work');
  sessionView.set('conversation');
});
afterEach(() => {
  if (original) inv.mockImplementation(original);
  clearSelection();
  closeTask();
  selectedTaskId.set(null);
  sessions.set([]);
});

const conversation = (c: HTMLElement) => c.querySelector('.right-body [data-testid="conversation-panel"]');

describe('App: one selection drives every pane', () => {
  it('picking TASK-223 never shows TASK-225\'s conversation', async () => {
    const { container, findByTestId, getByTestId } = render(App);
    selectSessionExplicitly(s225, { task: 'item:225' });
    await waitFor(() => expect(conversation(container)).not.toBeNull());

    // TASK-223 has no live session: the pane empties, Details shows the task.
    inv.mockClear();
    openTask('item:223', []);
    await waitFor(() => expect(conversation(container)).toBeNull());
    expect(get(selectedSession)).toBeNull();
    expect(await findByTestId('work-task-detail')).toBeTruthy();
    await waitFor(() => expect(getByTestId('work-task-close').textContent).toContain('Back to task-225'));

    // Now it has one: that one shows, never TASK-225's.
    openTask('item:223', [{ session_id: 223, state: 'active' }]);
    await waitFor(() => expect(conversation(container)).not.toBeNull());
    expect(get(selectedSession)?.id).toBe(223);
    // Nothing read TASK-225's conversation after the pick.
    const reads = inv.mock.calls.filter(([cmd]) => cmd === 'session_conversation');
    expect(reads.length).toBeGreaterThan(0);
    expect(reads.every(([, a]) => (a as { args: { session_id: number } }).args.session_id === 223)).toBe(true);
  });
});

describe('App: a pop-out pops back in (M15 G4.4)', () => {
  it("shows the pop-out's session on its terminal, whatever was open before", async () => {
    // Only this mount's listener: an earlier test's App registered one too.
    vi.mocked(listen).mockClear();
    const { getByTestId } = render(App);
    selectSessionExplicitly(s225);
    // The start-up loads are in: the hub has said this client owns the rows.
    await waitFor(() => expect((getByTestId('stab-agent') as HTMLButtonElement).disabled).toBe(false));
    let handler: ((e: { payload: unknown }) => void) | undefined;
    await waitFor(() => {
      const call = vi.mocked(listen).mock.calls.find(([name]) => name === POP_BACK_IN_EVENT);
      expect(call).toBeTruthy();
      handler = call![1] as typeof handler;
    });
    expect(get(sessionView)).toBe('conversation');
    handler!({ payload: { sessionId: 223, shell: null } });
    await waitFor(() => expect(get(selectedSession)?.id).toBe(223));
    await waitFor(() => expect(get(sessionView)).toBe('terminal'));
    expect(getByTestId('stab-agent').getAttribute('aria-selected')).toBe('true');
    // A session that is gone changes nothing.
    handler!({ payload: { sessionId: 999, shell: null } });
    expect(get(selectedSession)?.id).toBe(223);
  });
});
