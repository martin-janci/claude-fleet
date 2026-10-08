// Redesign step 4.4: a "Paused · limit" row's Switch account and Wait, and
// the bulk move the selection bar offers.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';

import LimitActions from './LimitActions.svelte';
import { moveToHeadroom, resetText, waitingOut } from './account_limits';
import type { Result } from './result';
import { RESET_WEEK, session } from './hosts_fixture';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;
const name = (u: string) => (u === 'acc-spare' ? 'spare@x' : 'own@x');
const over = (suggestion: unknown) => ({
  pause_at_pct: 90,
  chosen: { profile: null, account_uuid: 'acc-own', used_pct: 100 },
  over: true,
  suggestion,
  logins: [],
});
const spare = { profile: 'spare', account_uuid: 'acc-spare', used_pct: 30 };
const restarts = () => invoke.mock.calls.filter((c) => c[0] === 'restart_session');

function answer(headroom: unknown) {
  invoke.mockImplementation(async (cmd: string, a: any) => {
    if (cmd === 'check_account_headroom') return headroom;
    if (cmd === 'restart_session') return session(a.args.host_alias, a.args.name, { claude_profile: a.args.profile });
    return null;
  });
}

beforeEach(() => {
  invoke.mockReset();
  waitingOut.set(new Map());
});

describe('LimitActions', () => {
  it('proposes the login with the most headroom and switches only on the second press', async () => {
    answer(over(spare));
    const sess = session('mac', 'pd-2988');
    render(LimitActions, { props: { sess, resetsAt: RESET_WEEK, accountName: name } });
    await fireEvent.click(screen.getByTestId('limit-switch'));
    await vi.waitFor(() => expect(screen.getByTestId('limit-switch-ask')).toBeTruthy());
    expect(screen.getByTestId('limit-switch-ask').textContent).toContain('spare (spare@x) · 30% used');
    expect(restarts()).toHaveLength(0);
    await fireEvent.click(screen.getByTestId('limit-switch-confirm'));
    await vi.waitFor(() => expect(restarts()).toHaveLength(1));
    expect(restarts()[0][1]).toEqual({ args: { host_alias: 'mac', name: 'pd-2988', profile: 'spare' } });
  });

  it('cancel leaves the session where it is', async () => {
    answer(over(spare));
    render(LimitActions, { props: { sess: session('mac', 'a'), resetsAt: RESET_WEEK, accountName: name } });
    await fireEvent.click(screen.getByTestId('limit-switch'));
    await vi.waitFor(() => expect(screen.getByTestId('limit-switch-cancel')).toBeTruthy());
    await fireEvent.click(screen.getByTestId('limit-switch-cancel'));
    expect(screen.getByTestId('limit-switch')).toBeTruthy();
    expect(restarts()).toHaveLength(0);
  });

  it('with no login under the line there is nothing to confirm', async () => {
    answer(over(null));
    render(LimitActions, { props: { sess: session('mac', 'a'), resetsAt: RESET_WEEK, accountName: name } });
    await fireEvent.click(screen.getByTestId('limit-switch'));
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith('check_account_headroom', expect.anything()));
    expect(screen.queryByTestId('limit-switch-ask')).toBeNull();
  });

  it('Wait folds the buttons into the reset time', async () => {
    const sess = session('mac', 'a');
    render(LimitActions, { props: { sess, resetsAt: RESET_WEEK, accountName: name } });
    const label = `Wait until ${resetText(RESET_WEEK)}`;
    expect(screen.getByTestId('limit-wait').textContent).toBe(label);
    await fireEvent.click(screen.getByTestId('limit-wait'));
    expect(screen.getByTestId('limit-waiting').textContent).toBe(`Waiting until ${resetText(RESET_WEEK)}`);
    expect(get(waitingOut).get(sess.id)).toBe(RESET_WEEK);
    expect(screen.queryByTestId('limit-switch')).toBeNull();
  });

  it('without a reset time there is no Wait to offer', () => {
    render(LimitActions, { props: { sess: session('mac', 'a'), resetsAt: null, accountName: name } });
    expect(screen.getByTestId('limit-switch')).toBeTruthy();
    expect(screen.queryByTestId('limit-wait')).toBeNull();
  });
});

describe('moveToHeadroom', () => {
  it('moves only the rows over the line that have somewhere to go', async () => {
    invoke.mockImplementation(async (cmd: string, a: any) => {
      if (cmd !== 'check_account_headroom') return null;
      if (a.args.host_alias === 'under') return { ...over(null), over: false };
      if (a.args.host_alias === 'full') return over(null);
      return over(spare);
    });
    const restart = vi.fn(
      async (_h: string, n: string): Promise<Result<unknown>> =>
        n === 'bad' ? { ok: false, error: { code: 'E_X', message: 'no' } } : { ok: true, value: null },
    );
    const r = await moveToHeadroom(
      [
        { host_alias: 'mac', tmux_name: 'a' },
        { host_alias: 'mac', tmux_name: 'bad' },
        { host_alias: 'under', tmux_name: 'b' },
        { host_alias: 'full', tmux_name: 'c' },
      ],
      restart,
    );
    expect(r).toEqual({ moved: 1, stayed: 1, nowhere: 1, failed: 1 });
    expect(restart).toHaveBeenCalledWith('mac', 'a', 'spare');
  });
});
