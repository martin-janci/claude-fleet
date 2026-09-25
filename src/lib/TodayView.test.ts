// The Today view (work graph M9.1): its sections from the hub's digest, the
// jump to a session, and Copy standup.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('./clipboard', () => ({ copyText: vi.fn(async () => true) }));
import { invoke } from '@tauri-apps/api/core';
import { copyText } from './clipboard';
import TodayView from './TodayView.svelte';
import { sessions } from './sessions';
import { selectedSession, clearSelection } from './selection';
import { session } from './hosts_fixture';
import type { Today } from './today';
import { tidyRequest, EMPTY_REPORT } from './tidy';

const digest: Today = {
  since: 0,
  now: 200,
  groups: [
    {
      bucket: 'waiting',
      key: 'PAY-7',
      title: '<i>Refund</i>',
      sessions: [{ id: 41, name: 'pay', host_alias: 'mefistos', attention: 'waiting', last_activity_at: 100 }],
    },
    { bucket: 'stale', key: 'OLD-1', sessions: [{ id: 42, name: 'old', host_alias: 'mefistos', stale: 'idle', last_activity_at: 1 }] },
  ],
  shipped: [{ how: 'done', key: 'PAY-3', title: 'Receipts', at: 150 }],
};

async function flush() {
  for (let i = 0; i < 5; i++) await tick();
}

describe('TodayView', () => {
  beforeEach(() => {
    clearSelection();
    sessions.set([session('mefistos', 'pay', { id: 41 }), session('mefistos', 'old', { id: 42 })]);
    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockImplementation(async (cmd: string) => (cmd === 'work_today' ? digest : null));
    vi.mocked(copyText).mockClear();
  });

  it('asks the hub for today since local midnight and draws the sections', async () => {
    render(TodayView);
    await flush();
    const call = vi.mocked(invoke).mock.calls.find((c) => c[0] === 'work_today');
    expect(call).toBeTruthy();
    const since = (call![1] as { args: { since: number } }).args.since;
    expect(new Date(since * 1000).getHours()).toBe(0);
    expect(screen.getByTestId('today-waiting')).toBeTruthy();
    expect(screen.getByTestId('today-shipped')).toBeTruthy();
    expect(screen.getByTestId('today-stale')).toBeTruthy();
    expect(screen.queryByTestId('today-in-progress')).toBeNull();
    // A title is text, never markup.
    expect(screen.getByText('PAY-7 <i>Refund</i>')).toBeTruthy();
  });

  it('a session jumps to it and closes the view', async () => {
    const onclose = vi.fn();
    render(TodayView, { onclose });
    await flush();
    await fireEvent.click(screen.getByText('pay (waiting for an answer)'));
    expect(get(selectedSession)?.id).toBe(41);
    expect(onclose).toHaveBeenCalled();
  });

  it('Copy standup copies the plain-text standup and nothing is sent', async () => {
    render(TodayView);
    await flush();
    await fireEvent.click(screen.getByTestId('today-copy'));
    await flush();
    const text = vi.mocked(copyText).mock.calls[0][0];
    expect(text).toContain('Shipped\n- PAY-3 Receipts — done');
    expect(text).toContain('Waiting on me\n- PAY-7 <i>Refund</i> — pay (waiting for an answer)');
    expect(vi.mocked(invoke).mock.calls.map((c) => c[0])).not.toContain('send_prompt');
    expect(screen.getByTestId('today-copy').textContent).toBe('Copied');
  });

  it('shows the hub’s refusal', async () => {
    vi.mocked(invoke).mockImplementation(async () => {
      throw { code: 'E_HUB', message: 'hub unreachable' };
    });
    render(TodayView);
    await flush();
    expect(screen.getByTestId('today-error').textContent).toContain('hub unreachable');
  });

  it('Stale sessions fleet suggests tidying open Tidy up with just those picked (M9)', async () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'work_today') return digest;
      if (cmd === 'work_tidy')
        return {
          ...EMPTY_REPORT,
          candidates: [
            { session_id: 42, host_alias: 'mefistos', tmux_name: 'old', reason: 'done_idle', action: 'safe_kill', since: 0 },
            { session_id: 99, host_alias: 'mefistos', tmux_name: 'x', reason: 'done_idle', action: 'safe_kill', since: 0 },
          ],
        };
      return null;
    });
    tidyRequest.set(null);
    const onclose = vi.fn();
    render(TodayView, { onclose });
    await flush();
    const btn = await screen.findByTestId('today-tidy');
    expect(btn.textContent).toBe('Tidy up · 1');
    await fireEvent.click(btn);
    expect(get(tidyRequest)?.sessionIds).toEqual([42]);
    expect(onclose).toHaveBeenCalled();
    expect(vi.mocked(invoke).mock.calls.map((c) => c[0])).not.toContain('tidy_apply');
  });
});
