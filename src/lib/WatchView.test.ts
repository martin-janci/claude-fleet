import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import WatchView from './WatchView.svelte';
import type { SessionRow } from './sessions';
import { hubStatus, STANDALONE, type HubStatus } from './hub';

function makeSession(over: Partial<SessionRow> = {}): SessionRow {
  return {
    id: 42,
    tmux_name: 'dev-martin-janci-claude-fleet',
    host_alias: 'trn',
    project_id: 1,
    worktree_id: null,
    created_at: 1,
    last_activity_at: 1,
    status: 'running',
    notes: null,
    account_uuid: null,
    kind: 'work',
    reviews_session_id: null,
    worktree_key: 'main',
    lost_at: null,
    claude_session_id: null,
    claude_status: null,
    effort_level: null,
    pr_url: null,
    current_activity: null,
    friendly_name: null,
    safe_kill_state: null,
    safe_kill_nonce: null,
    safe_kill_detail: null,
    safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null,
    stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null,
    last_turn_at: null, ci_status: null, turn_seq: 0, last_stop_at: null,
    parent_session_id: null, tags: [],
    model: null, context_tokens: null, context_window: null, context_source: null,
    context_at: null, context_stale: false, tmux_pane_id: null, pending_input: null,
    owner_person_id: 4,
    visibility: 'private',
    ...over,
  };
}

const remote: HubStatus = {
  ...STANDALONE,
  remote: true,
  url: 'https://fleet.example.com',
  client_name: 'laptop',
  configured_url: 'https://fleet.example.com',
  configured_client_name: 'laptop',
};

const inv = () => mockedInvoke as ReturnType<typeof vi.fn>;
const names = () => inv().mock.calls.map((c) => c[0] as string);

beforeEach(() => {
  inv().mockReset();
  inv().mockImplementation(async (cmd: string) => {
    if (cmd === 'capture_session') return 'claude> working on the ticket\n';
    return null;
  });
  hubStatus.set(remote);
});

afterEach(() => {
  hubStatus.set({ ...STANDALONE });
});

describe('the watcher’s pane view', () => {
  it('captures the pane and renders it read-only', async () => {
    render(WatchView, { props: { session: makeSession(), access: 'watch' } });
    const pane = await screen.findByTestId('watch-pane');
    expect(pane.textContent).toContain('working on the ticket');
    const call = inv().mock.calls.find((c) => c[0] === 'capture_session');
    expect((call?.[1] as { args: { session_id: number } }).args.session_id).toBe(42);
  });

  // The rule this component exists to make keepable: a grant hands over no
  // PTY, because a PTY is this machine's own `ssh … tmux attach` and no revoke
  // on the hub can reach it.
  it('never touches the PTY — not open, not write, not resize, not even close', async () => {
    render(WatchView, { props: { session: makeSession(), access: 'drive' } });
    await screen.findByTestId('watch-pane');
    expect(names().filter((n) => n.startsWith('pty_'))).toEqual([]);
  });

  // The other unrevocable channel: `upload_to_session` scps onto the owner's
  // host over this machine's SSH. There is no drop target here to reach it.
  it('never uploads anything to the session', async () => {
    render(WatchView, { props: { session: makeSession(), access: 'drive' } });
    await screen.findByTestId('watch-pane');
    expect(names()).not.toContain('upload_to_session');
    expect(screen.queryByTestId('terminal-drop-overlay')).toBeNull();
  });

  it('says why there is no terminal, as a rule rather than a failure', async () => {
    render(WatchView, { props: { session: makeSession(), access: 'watch' } });
    const why = await screen.findByTestId('watch-reason');
    expect(why.textContent).toContain('revoke');
    expect(screen.getByTestId('watch-level').textContent).toBe('watch');
  });

  it('names the level it was given', async () => {
    render(WatchView, { props: { session: makeSession(), access: 'drive' } });
    expect((await screen.findByTestId('watch-level')).textContent).toBe('drive');
  });

  // `null` is "we cannot tell": an unreachable hub, or one that has not said
  // who this device is. Polling then would be an error loop against a hub that
  // is already saying something is wrong.
  it('does not capture anything when the access answer is unknown', async () => {
    render(WatchView, { props: { session: makeSession(), access: null } });
    await screen.findByTestId('watch-unavailable');
    expect(names()).not.toContain('capture_session');
    expect(screen.getByTestId('watch-reason').textContent).toContain('who this device is');
    expect((screen.getByTestId('watch-refresh') as HTMLButtonElement).disabled).toBe(true);
    // And the header must not contradict that paragraph. A badge reading
    // "WATCH" here says "this session is shared with you" — in the one state
    // where nothing has told us anything (an unreachable hub, or one that has
    // not said who this device is), which is exactly what the plan says these
    // two states must never read as.
    expect(screen.queryByTestId('watch-level')).toBeNull();
  });

  // The other `null` state: a configured hub this launch could not use.
  it('shows no level badge when the hub is unavailable', async () => {
    hubStatus.set({
      ...STANDALONE,
      unavailable: 'no stored token for https://fleet.example.com',
      configured_url: 'https://fleet.example.com',
    });
    render(WatchView, { props: { session: makeSession(), access: null } });
    await screen.findByTestId('watch-unavailable');
    expect(screen.queryByTestId('watch-level')).toBeNull();
    expect(screen.getByTestId('watch-reason').textContent).toContain('no stored token');
  });

  // The scrollback is only worth asking for if the reply may carry it: the
  // backend's own `max_lines` default is 200, about one screen, so without an
  // explicit cap the history asked for above is trimmed straight back off.
  it('asks for a line cap big enough to keep the scrollback it requested', async () => {
    render(WatchView, { props: { session: makeSession(), access: 'watch' } });
    await screen.findByTestId('watch-pane');
    const call = inv().mock.calls.find((c) => c[0] === 'capture_session');
    const args = (call?.[1] as { args: { scrollback_lines: number; max_lines?: number } }).args;
    expect(args.scrollback_lines).toBe(200);
    expect(args.max_lines).toBeDefined();
    expect(args.max_lines!).toBeGreaterThan(args.scrollback_lines);
  });

  it('does not capture while an overlay covers the pane', async () => {
    render(WatchView, { props: { session: makeSession(), access: 'watch', visible: false } });
    await Promise.resolve();
    expect(names()).not.toContain('capture_session');
  });

  it('shows a failed capture in place rather than toasting a poll', async () => {
    inv().mockImplementation(async (cmd: string) => {
      if (cmd === 'capture_session') throw { code: 'E_HOST_OFFLINE', message: 'trn is unreachable' };
      return null;
    });
    render(WatchView, { props: { session: makeSession(), access: 'watch' } });
    const err = await screen.findByTestId('watch-error');
    expect(err.textContent).toContain('E_HOST_OFFLINE');
    expect(screen.queryByTestId('watch-pane')).toBeNull();
  });

  it('re-captures on demand', async () => {
    render(WatchView, { props: { session: makeSession(), access: 'watch' } });
    await screen.findByTestId('watch-pane');
    const before = names().filter((n) => n === 'capture_session').length;
    await fireEvent.click(screen.getByTestId('watch-refresh'));
    await waitFor(() =>
      expect(names().filter((n) => n === 'capture_session').length).toBe(before + 1),
    );
  });

  it('does not stack captures behind one that outlasts the poll interval', async () => {
    vi.useFakeTimers();
    try {
      let release: (v: string) => void = () => {};
      inv().mockImplementation((cmd: string) => {
        if (cmd === 'capture_session') return new Promise<string>((r) => (release = r));
        return Promise.resolve(null);
      });
      render(WatchView, { props: { session: makeSession(), access: 'watch' } });
      await vi.advanceTimersByTimeAsync(10_000);
      expect(names().filter((n) => n === 'capture_session').length).toBe(1);
      release('slow host, finally\n');
      await vi.advanceTimersByTimeAsync(3_000);
      expect(names().filter((n) => n === 'capture_session').length).toBe(2);
    } finally {
      vi.useRealTimers();
    }
  });
});
