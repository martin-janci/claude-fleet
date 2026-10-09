// J8 (redesign step 5.11): the agent tab warns when no pane rule could read
// the screen at the end of the session's last turn, and stops warning once a
// later turn ends, the session works again, or the timeline says nothing.
import { render, screen } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import PaneUnreadableNotice from './PaneUnreadableNotice.svelte';
import { dispatchTimelineEvents } from './live_events';
import { paneUnreadable, type SessionEvent } from './timeline';
import { expectAccessible } from './a11y_check';

const ev = (id: number, kind: string, session_id = 7): SessionEvent => ({
  id,
  session_id,
  at: 1_790_000_000 + id,
  kind,
  detail: null,
  claude_session_id: 'c1',
});

let history: SessionEvent[];

async function flush() {
  for (let i = 0; i < 6; i++) await tick();
}

describe('paneUnreadable', () => {
  it('is the newest turn edge when it is a pane_unreadable entry', () => {
    expect(paneUnreadable([])).toBeNull();
    expect(paneUnreadable([ev(1, 'turn_done'), ev(2, 'pane_unreadable')])?.id).toBe(2);
    // Newest first, as session_history answers: same answer.
    expect(paneUnreadable([ev(2, 'pane_unreadable'), ev(1, 'turn_done')])?.id).toBe(2);
    // A later turn's end, or a new conversation, takes it back.
    expect(paneUnreadable([ev(2, 'pane_unreadable'), ev(3, 'turn_done')])).toBeNull();
    expect(paneUnreadable([ev(2, 'pane_unreadable'), ev(5, 'conversation_started')])).toBeNull();
    // Other entries say nothing either way.
    expect(paneUnreadable([ev(2, 'pane_unreadable'), ev(9, 'mcp_call')])?.id).toBe(2);
  });
});

describe('PaneUnreadableNotice', () => {
  beforeEach(() => {
    history = [];
    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockImplementation(async (cmd: string) => (cmd === 'session_history' ? history : null));
  });

  it('warns in the agent tab after a turn the rules could not read', async () => {
    history = [ev(4, 'pane_unreadable'), ev(3, 'turn_done')];
    render(PaneUnreadableNotice, { session: { id: 7, claude_status: 'idle' } });
    await flush();
    const b = screen.getByTestId('pane-unreadable');
    expect(b.textContent).toContain('Fleet could not read this screen');
    expect(b.getAttribute('role')).toBe('status');
    await expectAccessible(b);
  });

  it('says nothing when the last turn was read, or while the session works', async () => {
    history = [ev(5, 'turn_done'), ev(4, 'pane_unreadable')];
    render(PaneUnreadableNotice, { session: { id: 7, claude_status: 'idle' } });
    await flush();
    expect(screen.queryByTestId('pane-unreadable')).toBeNull();
  });

  it('hides while the session is working again', async () => {
    history = [ev(4, 'pane_unreadable')];
    render(PaneUnreadableNotice, { session: { id: 7, claude_status: 'working' } });
    await flush();
    expect(screen.queryByTestId('pane-unreadable')).toBeNull();
  });

  it('follows the live timeline: a pushed entry shows it, the next turn takes it back', async () => {
    history = [ev(3, 'turn_done')];
    render(PaneUnreadableNotice, { session: { id: 7, claude_status: 'idle' } });
    await flush();
    expect(screen.queryByTestId('pane-unreadable')).toBeNull();
    dispatchTimelineEvents([ev(6, 'pane_unreadable', 8)]);
    await flush();
    expect(screen.queryByTestId('pane-unreadable')).toBeNull();
    dispatchTimelineEvents([ev(6, 'pane_unreadable')]);
    await flush();
    expect(screen.getByTestId('pane-unreadable')).toBeTruthy();
    dispatchTimelineEvents([ev(7, 'turn_done')]);
    await flush();
    expect(screen.queryByTestId('pane-unreadable')).toBeNull();
  });
});
