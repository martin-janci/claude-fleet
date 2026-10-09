import { render, screen, waitFor } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { sessionPulse, thinkingLabel, ticketPulse } from './session_loaders';
import { creatingStart, newSessionAbortable, sessions, type SessionRow } from './sessions';
import { startingSessions, resetStarting } from './session_starting';
import PulseSteps from './PulseSteps.svelte';
import HostStream from './HostStream.svelte';
import { startSteps } from './start_progress';
import type { SessionEvent } from './timeline';
import { expectAccessible } from './a11y_check';
import { motionPref } from './motion';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;

const row = (over: Partial<SessionRow> = {}): SessionRow =>
  ({ id: 9, tmux_name: 'pd-3011', host_alias: 'mercury', kind: 'work', agent: null, claude_status: null, ...over }) as SessionRow;

beforeEach(() => {
  resetStarting();
  sessions.set([]);
  invoke.mockReset();
});

// Step 5.13 (LogoMotion, LoadersInUse, LoadersInFlows boards).
describe('sessionPulse', () => {
  it('is on the worktree while the create runs, then waits on the agent once the row is here', () => {
    const creating = sessionPulse(null);
    expect(creating.steps.map((s) => s.state)).toEqual(['active', 'pending', 'pending']);
    expect(creating.caption).toBe('Setting up worktree · 1 of 3');
    const up = sessionPulse(row());
    expect(up.steps.map((s) => s.state)).toEqual(['done', 'done', 'active']);
    expect(up.caption).toBe('Claude Code starting · 3 of 3');
    const ready = sessionPulse(row({ claude_status: 'idle' }));
    expect(ready.done).toBe(true);
    expect(ready.caption).toBe('Started');
  });

  it('a shell needs nothing more than its pane', () => {
    expect(sessionPulse(row({ kind: 'shell' })).done).toBe(true);
    expect(sessionPulse(null, 'shell').steps[2].label).toBe('Shell');
  });

  it('a ticket start folds its timeline onto the same three steps', () => {
    const ev = (id: number, kind: string, detail: string | null = null): SessionEvent =>
      ({ id, session_id: 9, at: id, kind, detail }) as SessionEvent;
    const spawned = ev(1, 'start_spawned', JSON.stringify({ key: 'PD-1', worktree_id: 3, new_worktree: true, branch: 'b', brief: true }));
    const p1 = ticketPulse(startSteps([spawned])!);
    expect(p1.steps.map((s) => s.state)).toEqual(['done', 'done', 'active']);
    const p2 = ticketPulse(startSteps([spawned, ev(2, 'repl_ready')])!);
    expect(p2.done).toBe(true);
  });
});

describe('thinkingLabel', () => {
  it('says what the agent is reading, and for how long', () => {
    expect(thinkingLabel({ label: 'Read hub/pair.rs', since: '14 s' })).toBe('Thinking · reading hub/pair.rs · 14 s');
    expect(thinkingLabel({ label: 'Search web rust tokio', since: null })).toBe('Thinking · searching the web rust tokio');
    expect(thinkingLabel({ label: 'Explore · map the repo', since: null })).toBe('Thinking · Explore · map the repo');
    expect(thinkingLabel(null)).toBe('Thinking');
  });
});

describe('the start is followed on events, not on time', () => {
  it('a create in flight is the worktree step; the row it returns waits on the agent until its status arrives', async () => {
    let resolve!: (v: SessionRow) => void;
    invoke.mockImplementation((cmd: string) =>
      cmd === 'new_session' ? new Promise<SessionRow>((r) => (resolve = r)) : Promise.resolve(null),
    );
    const pending = newSessionAbortable({ host_alias: 'mercury', project_id: 1, worktree_id: null, name: 'pd-3011' });
    expect(get(creatingStart)).toEqual({ host_alias: 'mercury', name: 'pd-3011', kind: 'work' });
    resolve(row());
    await pending;
    expect(get(creatingStart)).toBeNull();
    expect(get(startingSessions).has(9)).toBe(true);

    vi.useFakeTimers();
    // However long nothing happens, the agent step does not advance.
    vi.advanceTimersByTime(60_000);
    vi.useRealTimers();
    expect(get(startingSessions).has(9)).toBe(true);

    // The row's update with an agent status is what finishes it.
    sessions.update((rs) => rs.map((r) => (r.id === 9 ? { ...r, claude_status: 'idle' } : r)));
    expect(get(startingSessions).has(9)).toBe(false);
  });

  it('the mark lights one satellite per finished step', async () => {
    const { container, rerender } = render(PulseSteps, { props: { pulse: sessionPulse(null), title: 'Starting pd-3011 on mercury' } });
    const states = () => screen.getAllByTestId('pulse-sat').map((s) => s.getAttribute('data-state'));
    expect(states()).toEqual(['active', 'pending', 'pending']);
    expect(screen.getByRole('status').textContent).toBe('Setting up worktree · 1 of 3');
    await rerender({ pulse: sessionPulse(row()), title: 'Starting pd-3011 on mercury' });
    expect(states()).toEqual(['done', 'done', 'active']);
    await expectAccessible(container);
  });

  // Step 0.6, the Loader kit's rule: Reduced fades the active step, Off holds it.
  it('the active step fades under Reduced motion and rests under Off', async () => {
    try {
      motionPref.set('reduced');
      const { unmount } = render(PulseSteps, { props: { pulse: sessionPulse(null), title: 't' } });
      const active = () => screen.getAllByTestId('pulse-sat')[0];
      expect(active().classList.contains('sat--fade')).toBe(true);
      expect(active().classList.contains('sat--still')).toBe(false);
      unmount();
      motionPref.set('off');
      render(PulseSteps, { props: { pulse: sessionPulse(null), title: 't' } });
      expect(active().classList.contains('sat--still')).toBe(true);
      expect(active().classList.contains('sat--fade')).toBe(false);
    } finally {
      motionPref.set('system');
    }
  });
});

describe('HostStream', () => {
  it('rests its dots under Reduced (with a slow fade) and Off (without)', () => {
    try {
      motionPref.set('reduced');
      const { unmount } = render(HostStream, { props: { from: 'mac', to: 'mercury' } });
      let el = screen.getByTestId('host-stream');
      expect([el.classList.contains('hs--still'), el.classList.contains('hs--fade')]).toEqual([true, true]);
      unmount();
      motionPref.set('off');
      render(HostStream, { props: { from: 'mac', to: 'mercury' } });
      el = screen.getByTestId('host-stream');
      expect([el.classList.contains('hs--still'), el.classList.contains('hs--fade')]).toEqual([true, false]);
    } finally {
      motionPref.set('system');
    }
  });

  it('names both hosts', async () => {
    const { container } = render(HostStream, { props: { from: 'mac', to: 'mercury' } });
    await waitFor(() => expect(screen.getByTestId('host-stream').textContent).toContain('mac'));
    expect(screen.getByTestId('host-stream').textContent).toContain('mercury');
    await expectAccessible(container);
  });
});
