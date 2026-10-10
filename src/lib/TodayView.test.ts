// The Today view (work graph M9.1): its sections from the hub's digest, the
// jump to a session, and Copy standup.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('./clipboard', () => ({ copyText: vi.fn(async () => true) }));
vi.mock('./open_external', () => ({ openExternal: vi.fn(async () => true) }));
import { invoke } from '@tauri-apps/api/core';
import { copyText } from './clipboard';
import { openExternal } from './open_external';
import TodayView from './TodayView.svelte';
import { sessions } from './sessions';
import { selectedSession, clearSelection } from './selection';
import { session } from './hosts_fixture';
import type { Today } from './today';
import { tidyRequest, EMPTY_REPORT } from './tidy';
import { expectAccessible } from './a11y_check';

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
    vi.mocked(openExternal).mockClear();
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

  // Minor M3 of the final review: `today.rs::digest` fills
  // `status_category` from the item's EFFECTIVE status, but the desktop
  // header rendered only `status_name` — tracker-only — so the fix was
  // wire- and phone-only here, while docs/work-graph.md promises it on
  // every surface.
  it('the group header shows a local item\'s effective status, not only a tracker name', async () => {
    const withStatus: Today = {
      ...digest,
      groups: [
        {
          bucket: 'in_progress',
          key: 'OPS',
          title: 'Ops cleanup',
          // Work fleet tracks itself: no tracker, so no `status_name` — its
          // live status is all there is.
          status_category: 'in_progress',
          sessions: [{ id: 41, name: 'pay', host_alias: 'mefistos', last_activity_at: 100 }],
        },
        {
          bucket: 'waiting',
          key: 'PAY-7',
          title: 'Refund',
          status_category: 'todo',
          status_name: 'In Review',
          sessions: [{ id: 42, name: 'old', host_alias: 'mefistos', attention: 'waiting', last_activity_at: 100 }],
        },
      ],
    };
    vi.mocked(invoke).mockImplementation(async (cmd: string) => (cmd === 'work_today' ? withStatus : null));
    render(TodayView);
    await flush();
    const shown = screen.getAllByTestId('today-group-status').map((e) => e.textContent?.trim());
    expect(shown).toEqual(['In Review', 'in progress']);
    // Copy standup says the same thing, so the two cannot disagree.
    await fireEvent.click(screen.getByTestId('today-copy'));
    const text = vi.mocked(copyText).mock.calls[0][0] as string;
    expect(text).toContain('OPS Ops cleanup — in progress');
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

  // Review r13 (step 10.6): the first read is a skeleton after 400 ms, never
  // "Pick a session to see details" while it is still loading.
  it('the first read is loading, not empty', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    try {
      vi.mocked(invoke).mockImplementation(() => new Promise(() => {}));
      render(TodayView);
      await flush();
      expect(screen.queryByTestId('details-empty')).toBeNull();
      await vi.advanceTimersByTimeAsync(400);
      expect(screen.getByTestId('skeleton')).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it('a hub without work_today keeps the plain empty state and stops re-asking on row events', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    try {
      for (const code of ['E_FORBIDDEN', 'E_HUB_PROTOCOL', 'E_INVALID']) {
        vi.mocked(invoke).mockClear();
        vi.mocked(invoke).mockImplementation(async () => {
          throw { code, message: `${code}: work_today is not a client-callable tool` };
        });
        const { unmount } = render(TodayView);
        await flush();
        const asked = () => vi.mocked(invoke).mock.calls.filter((c) => c[0] === 'work_today').length;
        expect(asked()).toBe(1);
        expect(screen.queryByTestId('today-error')).toBeNull();
        expect(screen.getByTestId('details-empty').textContent).toBe('Pick a session to see details.');
        // A row event would normally refresh 2 s later; not on this hub.
        sessions.set([session('mefistos', 'pay', { id: 41 })]);
        vi.advanceTimersByTime(2_500);
        await flush();
        expect(asked()).toBe(1);
        expect(screen.queryByTestId('today-error')).toBeNull();
        unmount();
      }
      // Any other refusal still shows, and the row events keep refreshing.
      vi.mocked(invoke).mockClear();
      vi.mocked(invoke).mockImplementation(async () => {
        throw { code: 'E_HUB', message: 'hub unreachable' };
      });
      render(TodayView);
      await flush();
      sessions.set([session('mefistos', 'old', { id: 42 })]);
      vi.advanceTimersByTime(2_500);
      await flush();
      expect(vi.mocked(invoke).mock.calls.filter((c) => c[0] === 'work_today').length).toBe(2);
    } finally {
      vi.useRealTimers();
    }
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

  it('a Stale row still jumps to its session next to Tidy up (M10.4)', async () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'work_today') return digest;
      if (cmd === 'work_tidy')
        return {
          ...EMPTY_REPORT,
          candidates: [{ session_id: 42, host_alias: 'mefistos', tmux_name: 'old', reason: 'done_idle', action: 'safe_kill', since: 0 }],
        };
      return null;
    });
    tidyRequest.set(null);
    const onclose = vi.fn();
    render(TodayView, { onclose });
    await flush();
    await screen.findByTestId('today-tidy');
    await fireEvent.click(screen.getByText(/^old/));
    expect(get(selectedSession)?.id).toBe(42);
    expect(get(tidyRequest)).toBeNull();
    expect(onclose).toHaveBeenCalled();
  });

  it('no Tidy up action when no Stale session is a tidy candidate (M10.4)', async () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'work_today') return digest;
      if (cmd === 'work_tidy') return { ...EMPTY_REPORT, candidates: [] };
      return null;
    });
    render(TodayView);
    await flush();
    expect(screen.getByTestId('today-stale')).toBeTruthy();
    expect(screen.queryByTestId('today-tidy')).toBeNull();
  });
  it('Refresh asks the hub again; Close closes the view, and is offered only over a selected session', async () => {
    const onclose = vi.fn();
    // A fixed clock: a local midnight between the two loads would move `since`.
    const { unmount } = render(TodayView, { onclose, now: () => Date.UTC(2026, 8, 28, 12) });
    await flush();
    const asked = () => vi.mocked(invoke).mock.calls.filter((c) => c[0] === 'work_today').length;
    expect(asked()).toBe(1);
    await fireEvent.click(screen.getByTestId('today-refresh'));
    await flush();
    expect(asked()).toBe(2);
    const since = (c: unknown[]) => (c[1] as { args: { since: number } }).args.since;
    const calls = vi.mocked(invoke).mock.calls.filter((c) => c[0] === 'work_today');
    expect(since(calls[1])).toBe(since(calls[0]));
    expect(screen.getByTestId('today-waiting')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('today-close'));
    expect(onclose).toHaveBeenCalledTimes(1);
    expect(get(selectedSession)).toBeNull();
    unmount();
    // As the empty state of Details there is nothing to close into.
    render(TodayView);
    await flush();
    expect(screen.queryByTestId('today-close')).toBeNull();
  });

  it('Open ticket and PR open the row’s own url and nothing else', async () => {
    const linked: Today = {
      ...digest,
      groups: [
        { ...digest.groups[0], url: 'https://x.atlassian.net/browse/PAY-7' },
        { bucket: 'in_progress', key: 'PAY-9', sessions: [{ id: 42, name: 'old', host_alias: 'mefistos', last_activity_at: 1 }] },
      ],
      shipped: [
        { how: 'pr', key: 'PAY-3', title: 'Receipts', at: 150, url: 'https://x.atlassian.net/browse/PAY-3', pr_url: 'https://github.com/acme/app/pull/1' },
        { how: 'done', key: 'PAY-4', title: 'Totals', at: 160, url: 'https://x.atlassian.net/browse/PAY-4' },
        { how: 'done', title: 'No link', at: 170 },
      ],
    };
    vi.mocked(invoke).mockImplementation(async (cmd: string) => (cmd === 'work_today' ? linked : null));
    render(TodayView);
    await flush();
    // One Open ticket per group with a url (PAY-9 has none), one per shipped
    // row without a PR; a PR outranks the ticket link.
    const tickets = screen.getAllByText('Open ticket');
    expect(tickets).toHaveLength(2);
    await fireEvent.click(tickets[0]);
    expect(openExternal).toHaveBeenLastCalledWith('https://x.atlassian.net/browse/PAY-7');
    await fireEvent.click(tickets[1]);
    expect(openExternal).toHaveBeenLastCalledWith('https://x.atlassian.net/browse/PAY-4');
    // The shipped row's status also reads "PR"; the link is the button.
    const prs = screen.getAllByRole('button', { name: 'PR' });
    expect(prs).toHaveLength(1);
    await fireEvent.click(prs[0]);
    expect(openExternal).toHaveBeenLastCalledWith('https://github.com/acme/app/pull/1');
    expect(openExternal).toHaveBeenCalledTimes(3);
    // A link never selects a session or sends anything.
    expect(get(selectedSession)).toBeNull();
    expect(vi.mocked(invoke).mock.calls.map((c) => c[0])).not.toContain('send_prompt');
  });


  it("Stale's no-work group carries idle, unlinked sessions into Tidy up (M11.3)", async () => {
    const noWork: Today = {
      since: 0,
      now: 200,
      groups: [{ bucket: 'stale', sessions: [{ id: 43, name: 'lonely', host_alias: 'mefistos', stale: 'idle', last_activity_at: 1 }] }],
      shipped: [],
    };
    sessions.set([session('mefistos', 'lonely', { id: 43 })]);
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'work_today') return noWork;
      if (cmd === 'work_tidy')
        return {
          ...EMPTY_REPORT,
          candidates: [{ session_id: 43, host_alias: 'mefistos', tmux_name: 'lonely', reason: 'idle_unlinked', action: 'safe_kill', since: 0 }],
        };
      return null;
    });
    tidyRequest.set(null);
    render(TodayView, { onclose: vi.fn() });
    await flush();
    const btn = await screen.findByTestId('today-tidy');
    expect(btn.textContent).toBe('Tidy up · 1');
    await fireEvent.click(btn);
    expect(get(tidyRequest)?.sessionIds).toEqual([43]);
    expect(vi.mocked(invoke).mock.calls.map((c) => c[0])).not.toContain('tidy_apply');
  });

  it('the digest, with every section, is accessible', async () => {
    const { container } = render(TodayView);
    await flush();
    expect(screen.getByTestId('today-waiting')).toBeTruthy();
    await expectAccessible(container);
  });
});

// Gap plan G3.2 (board Today).
describe('TodayView: tiles, date line, inline limit, provenance (G3.2)', () => {
  beforeEach(() => {
    clearSelection();
    vi.mocked(invoke).mockReset();
  });

  it('draws the four KPI tiles and the date and fleet line', async () => {
    sessions.set([session('mefistos', 'pay', { id: 41 }), session('mefistos', 'old', { id: 42 })]);
    vi.mocked(invoke).mockImplementation(async (cmd: string) => (cmd === 'work_today' ? digest : null));
    render(TodayView, { now: () => new Date(2026, 9, 8, 9, 0).getTime() });
    await flush();
    const tiles = screen.getByTestId('today-kpis').querySelectorAll('li');
    expect(Array.from(tiles).map((t) => t.textContent?.replace(/\s+/g, ' ').trim())).toEqual([
      '1 Needs you',
      '0 In progress',
      '1 Shipped today',
      '1 Stale',
    ]);
    expect(screen.getByTestId('today-dateline').textContent).toMatch(/^Thursday 8 October · \d+ hosts?/);
  });

  it('In progress shows four groups, then "N more ›"; shipped says where it came from', async () => {
    const groups = Array.from({ length: 6 }, (_, i) => ({
      bucket: 'in_progress',
      key: `IP-${i}`,
      sessions: [{ id: 100 + i, name: `w${i}`, host_alias: 'mac', last_activity_at: 100 }],
    }));
    vi.mocked(invoke).mockImplementation(async (cmd: string) =>
      cmd === 'work_today'
        ? { since: 0, now: 200, groups, shipped: [{ how: 'pr', key: 'PR-1', title: 'Sweep', at: 150, pr_url: 'https://x/pull/1', from: 'Morning PR sweep' }] }
        : null,
    );
    sessions.set(groups.map((_, i) => session('mac', `w${i}`, { id: 100 + i })));
    render(TodayView);
    await flush();
    expect(screen.getAllByTestId('today-group')).toHaveLength(4);
    await fireEvent.click(screen.getByTestId('today-more'));
    expect(screen.getAllByTestId('today-group')).toHaveLength(6);
    expect(screen.queryByTestId('today-more')).toBeNull();
    expect(screen.getByTestId('today-shipped-from').textContent).toBe('from Morning PR sweep');
  });

  it('a waiting row paused on a limit offers Switch account and Wait right there, and Open', async () => {
    const { accountUsage } = await import('./account_usage_store');
    const { snapshot } = await import('./hosts_fixture');
    const nowS = Math.floor(Date.now() / 1000);
    accountUsage.set({
      'acc-1': snapshot('acc-1', {
        status: 'ok',
        usage: {
          five_hour: { utilization: 100, resets_at: nowS + 3600 },
          seven_day: { utilization: 10, resets_at: nowS + 86_400 },
          seven_day_opus: null,
          seven_day_sonnet: null,
        },
      }),
    });
    sessions.set([session('mefistos', 'pay', { id: 41, claude_status: 'idle', account_uuid: 'acc-1' }), session('mefistos', 'old', { id: 42 })]);
    vi.mocked(invoke).mockImplementation(async (cmd: string) => (cmd === 'work_today' ? digest : null));
    render(TodayView);
    await flush();
    expect(screen.getByTestId('today-limit').textContent).toMatch(/Switch account|Wait/);
    await fireEvent.click(screen.getAllByTestId('today-session-open')[0]);
    expect(get(selectedSession)?.id).toBe(41);
    accountUsage.set({});
  });
});
