// Redesign step 4.4: a "Paused · limit" row's Switch account and Wait, and
// the bulk move the selection bar offers.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';

import LimitActions from './LimitActions.svelte';
import {
  freestLogin,
  headroomForAccount,
  moveToAccount,
  moveToHeadroom,
  nextReset,
  resetText,
  switchCheck,
  usedText,
  waitingOut,
} from './account_limits';
import type { Result } from './result';
import { RESET_WEEK, session } from './hosts_fixture';
import { autoSwitchState } from './account_switch';

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
  autoSwitchState.set(null);
});

describe('LimitActions with automatic switching on', () => {
  const on = { enabled: true, continue_after: false, fleet_enabled: true };

  it('says where the fleet will move it and keeps Switch now', async () => {
    answer(over(spare));
    autoSwitchState.set(on);
    render(LimitActions, { props: { sess: session('mac', 'a'), resetsAt: RESET_WEEK, accountName: name } });
    await vi.waitFor(() => expect(screen.getByTestId('limit-auto').textContent).toContain('spare (spare@x)'));
    expect(screen.getByTestId('limit-switch').textContent).toBe('Switch now');
  });

  it('says when no login has room, and offers only Wait', async () => {
    answer(over(null));
    autoSwitchState.set(on);
    render(LimitActions, { props: { sess: session('mac', 'a'), resetsAt: RESET_WEEK, accountName: name } });
    await vi.waitFor(() => expect(screen.getByTestId('limit-auto-none').textContent).toContain('mac'));
    expect(screen.queryByTestId('limit-switch')).toBeNull();
    expect(screen.getByTestId('limit-wait')).toBeTruthy();
  });

  it('off for the person, the row is as before', () => {
    autoSwitchState.set({ ...on, enabled: false });
    render(LimitActions, { props: { sess: session('mac', 'a'), resetsAt: RESET_WEEK, accountName: name } });
    expect(screen.queryByTestId('limit-auto')).toBeNull();
    expect(screen.getByTestId('limit-switch').textContent).toBe('Switch account');
  });
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

  it('a wait for a reset that has passed does not hide a later limit', () => {
    const sess = session('mac', 'a');
    const real = Math.floor(Date.now() / 1000);
    // Waited out an earlier limit; the row paused again on a later one.
    waitingOut.set(new Map([[sess.id, real - 3600]]));
    render(LimitActions, { props: { sess, resetsAt: real + 3600, accountName: name } });
    expect(screen.queryByTestId('limit-waiting')).toBeNull();
    expect(screen.getByTestId('limit-switch')).toBeTruthy();
    expect(screen.getByTestId('limit-wait').textContent).toBe(`Wait until ${resetText(real + 3600)}`);
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

// Review r05: a row bills its `account_uuid`, which can differ from the
// account its profile's login holds now; the move reads the row's account.
describe('the account a row bills', () => {
  const logins = [
    { profile: null, account_uuid: 'acc-own', used_pct: 40 },
    { profile: 'work', account_uuid: 'acc-work', used_pct: 95 },
    { profile: 'spare', account_uuid: 'acc-spare', used_pct: 30 },
  ];
  const byProfile = { pause_at_pct: 90, chosen: logins[0], over: false, suggestion: null, logins };

  it('reads over and the suggestion for the row\'s account, not its profile', () => {
    const h = headroomForAccount(byProfile, 'acc-work');
    expect(h.chosen?.account_uuid).toBe('acc-work');
    expect(h.over).toBe(true);
    expect(h.suggestion?.profile).toBe('spare');
    expect(headroomForAccount(byProfile, null)).toBe(byProfile);
    expect(headroomForAccount(byProfile, 'acc-unknown')).toBe(byProfile);
  });

  it('moves a row whose account is over even when its profile\'s login is not', async () => {
    invoke.mockImplementation(async (cmd: string) => (cmd === 'check_account_headroom' ? byProfile : null));
    const restart = vi.fn(async (): Promise<Result<unknown>> => ({ ok: true, value: null }));
    const r = await moveToHeadroom([{ host_alias: 'mac', tmux_name: 'a', account_uuid: 'acc-work' }], restart);
    expect(r).toEqual({ moved: 1, stayed: 0, nowhere: 0, failed: 0 });
    expect(restart).toHaveBeenCalledWith('mac', 'a', 'spare');
  });
});

// Step 4.4: the person picks the account a bulk move goes to.
describe('moveToAccount', () => {
  const logins = [
    { profile: null, account_uuid: 'acc-own', used_pct: 40 },
    { profile: 'work', account_uuid: 'acc-work', used_pct: 95 },
    { profile: 'spare', account_uuid: 'acc-spare', used_pct: 30 },
  ];
  const mac = { pause_at_pct: 90, chosen: logins[0], over: false, suggestion: null, logins };
  const lin = { pause_at_pct: 90, chosen: logins[0], over: false, suggestion: null, logins: [logins[0]] };

  it('resumes each session under its host’s login on the picked account', async () => {
    invoke.mockImplementation(async (cmd: string, a: any) =>
      cmd === 'check_account_headroom' ? (a.args.host_alias === 'lin' ? lin : mac) : null,
    );
    const restart = vi.fn(async (): Promise<Result<unknown>> => ({ ok: true, value: null }));
    const r = await moveToAccount(
      [
        // Under the line, but the person picked another account: it moves.
        { host_alias: 'mac', tmux_name: 'a', account_uuid: 'acc-own' },
        // Already there.
        { host_alias: 'mac', tmux_name: 'b', account_uuid: 'acc-spare', claude_profile: 'spare' },
        // No login on that account on this host.
        { host_alias: 'lin', tmux_name: 'c', account_uuid: 'acc-own' },
      ],
      'acc-spare',
      restart,
    );
    expect(r).toEqual({ moved: 1, stayed: 1, nowhere: 1, failed: 0 });
    expect(restart).toHaveBeenCalledTimes(1);
    expect(restart).toHaveBeenCalledWith('mac', 'a', 'spare');
  });

  it('with no account picked, moves to headroom as before', async () => {
    invoke.mockImplementation(async (cmd: string) => (cmd === 'check_account_headroom' ? mac : null));
    const restart = vi.fn(async (): Promise<Result<unknown>> => ({ ok: true, value: null }));
    const r = await moveToAccount([{ host_alias: 'mac', tmux_name: 'a', account_uuid: 'acc-work' }], null, restart);
    expect(r.moved).toBe(1);
    expect(restart).toHaveBeenCalledWith('mac', 'a', 'spare');
  });
});

// Review w02-4: a signed-out login is never proposed, whatever its reading.
describe('signed-out logins', () => {
  const logins = [
    { profile: null, account_uuid: 'acc-own', used_pct: 95 },
    { profile: 'gone', account_uuid: 'acc-gone', used_pct: 5, signed_out: true },
    { profile: 'spare', account_uuid: 'acc-spare', used_pct: 30 },
  ];
  const h = { pause_at_pct: 90, chosen: logins[0], over: true, suggestion: null, logins };

  it('are left out of the suggestion and the New session default, and read "signed out"', () => {
    expect(headroomForAccount(h, 'acc-own').suggestion?.profile).toBe('spare');
    expect(freestLogin(logins)?.profile).toBe('spare');
    expect(usedText(logins[1])).toBe('signed out');
  });
});

// Review w03-3: a signed-out row offers Switch account (no Wait, no
// automatic move) to any signed-in login with room.
describe('LimitActions on a signed-out row', () => {
  it('proposes another signed-in login even though its own account is not over', async () => {
    const logins = [
      { profile: 'gone', account_uuid: 'acc-own', used_pct: 10, signed_out: true },
      { profile: null, account_uuid: 'acc-host', used_pct: 40 },
    ];
    answer({ pause_at_pct: 90, chosen: logins[0], over: false, suggestion: null, logins });
    autoSwitchState.set({ enabled: true, continue_after: false, fleet_enabled: true });
    const sess = session('mac', 'a', { claude_profile: 'gone', account_uuid: 'acc-own' });
    render(LimitActions, { props: { sess, resetsAt: null, signedOut: true, accountName: name } });
    expect(screen.queryByTestId('limit-auto')).toBeNull();
    expect(screen.queryByTestId('limit-wait')).toBeNull();
    await fireEvent.click(screen.getByTestId('limit-switch'));
    await vi.waitFor(() => expect(screen.getByTestId('limit-switch-ask').textContent).toContain('40% used'));
  });
});

// Review w03-9: a failed check is an error, never "no room".
describe('switchCheck', () => {
  it('keeps an error an error', async () => {
    invoke.mockImplementation(async () => {
      throw { code: 'E_UNREACHABLE', message: 'mac is unreachable' };
    });
    const r = await switchCheck({ host_alias: 'mac' });
    expect(r.ok).toBe(false);
  });

  it('names the soonest reset among the logins for the no-room line', () => {
    const logins = [
      { profile: null, account_uuid: 'a', used_pct: 95 },
      { profile: 'w', account_uuid: 'b', used_pct: 99 },
    ];
    const usage = {
      a: { usage: { five_hour: { utilization: 95, resets_at: 5000 }, seven_day: { utilization: 20, resets_at: 9000 } } },
      b: { usage: { five_hour: { utilization: 99, resets_at: 3000 }, seven_day: { utilization: 91, resets_at: 8000 } } },
    };
    const next = nextReset({ pause_at_pct: 90, chosen: logins[0], over: true, suggestion: null, logins }, usage, 1000);
    expect(next).toEqual({ login: logins[0], at: 5000 });
  });
});

// Review w03-12: the reset reads as the Accounts card reads it.
describe('resetText', () => {
  const now = Date.UTC(2026, 9, 19, 12) / 1000; // Mon 12:00 UTC
  const o = { now, locale: 'en-GB', timeZone: 'UTC' };
  it('a clock within five hours, a weekday within the week, a date beyond', () => {
    expect(resetText(now + 2 * 3600, o)).toBe('14:00');
    expect(resetText(now + 2 * 86400, o)).toBe('Wed 12:00');
    expect(resetText(now + 6 * 86400 + 23 * 3600, o)).toBe('Oct 26 11:00');
  });
});
