import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import {
  announceAccountSwitches,
  autoSwitchState,
  setAutoSwitch,
  switchesAutomatically,
  autoSwitchAccount,
  parseSwitchDetail,
  switchLines,
  EVENT_SWITCHED,
  EVENT_SWITCH_FAILED,
} from './account_switch';
import { sessions } from './sessions';
import { toasts } from './toasts';
import { session } from './hosts_fixture';

const inv = mockedInvoke as unknown as ReturnType<typeof vi.fn>;
const detail = (over: Record<string, unknown> = {}) =>
  JSON.stringify({ from_account: 'acct-out', to_account: 'acct-spare', profile: 'spare', used_pct: 12.4, error: null, ...over });
const name = (uuid: string) => ({ 'acct-out': 'ada@out', 'acct-spare': 'ada@spare' })[uuid] ?? uuid;

beforeEach(() => {
  inv.mockReset();
  toasts.set([]);
});

describe('account_switch', () => {
  it('reads and sets the answer through one command', async () => {
    inv.mockResolvedValue({ enabled: false, fleet_enabled: true });
    await autoSwitchAccount();
    expect(inv).toHaveBeenLastCalledWith('auto_switch_account', { args: {} });
    await autoSwitchAccount({ enabled: false });
    expect(inv).toHaveBeenLastCalledWith('auto_switch_account', { args: { enabled: false } });
    await setAutoSwitch({ continue_after: true });
    expect(inv).toHaveBeenLastCalledWith('auto_switch_account', { args: { continue_after: true } });
    expect(get(autoSwitchState)).toEqual({ enabled: false, fleet_enabled: true });
  });

  it('parses an attempt and refuses what is not one', () => {
    expect(parseSwitchDetail(detail())).toEqual({
      from_account: 'acct-out',
      to_account: 'acct-spare',
      profile: 'spare',
      used_pct: 12.4,
      error: null,
      continued: false,
    });
    expect(parseSwitchDetail(null)).toBeNull();
    expect(parseSwitchDetail('not json')).toBeNull();
    expect(parseSwitchDetail('{"profile":"x"}')).toBeNull();
  });

  it('words a switch and a failure', () => {
    const d = parseSwitchDetail(detail())!;
    expect(switchLines(EVENT_SWITCHED, 'api', d, name)).toEqual({
      message: 'api resumed under spare (ada@spare)',
      sub: 'ada@out ran out · 12% used',
    });
    const f = parseSwitchDetail(detail({ error: 'E_SSH', profile: null }))!;
    expect(switchLines(EVENT_SWITCH_FAILED, 'api', f, name)).toEqual({
      message: 'Could not move api off ada@out',
      sub: 'Switching to ada@spare failed (E_SSH); it stays paused',
    });
  });

  it('toasts each switch in a timeline push and nothing else', () => {
    sessions.set([{ ...session('mefistos', 'api'), id: 7 }]);
    announceAccountSwitches([
      { id: 1, session_id: 7, at: 1, kind: 'status_change', detail: 'idle', claude_session_id: null },
      { id: 2, session_id: 7, at: 2, kind: EVENT_SWITCHED, detail: detail(), claude_session_id: null },
    ]);
    const t = get(toasts);
    expect(t).toHaveLength(1);
    expect(t[0].message).toContain('resumed under spare');
    expect(t[0].action?.label).toBe('Open');
  });

  it('says when the session was told to carry on', () => {
    const d = parseSwitchDetail(detail({ continue: true }))!;
    expect(switchLines(EVENT_SWITCHED, 'api', d, name).sub).toBe('ada@out ran out · 12% used · told to continue');
  });

  it('folds several switches in one push into one toast, failures apart', () => {
    sessions.set([{ ...session('mefistos', 'api'), id: 7 }, { ...session('mefistos', 'web'), id: 8 }]);
    announceAccountSwitches([
      { id: 1, session_id: 7, at: 1, kind: EVENT_SWITCHED, detail: detail(), claude_session_id: null },
      { id: 2, session_id: 8, at: 1, kind: EVENT_SWITCHED, detail: detail(), claude_session_id: null },
      { id: 3, session_id: 9, at: 1, kind: EVENT_SWITCH_FAILED, detail: detail({ error: 'E_SSH' }), claude_session_id: null },
    ]);
    const t = get(toasts);
    expect(t).toHaveLength(2);
    expect(t[0].message).toBe('2 sessions resumed under other logins');
    expect(t[0].sub).toContain('api, web');
    expect(t[1].message).toContain('Could not move session 9');
  });

  it('switches automatically only when the fleet and the person both say so', () => {
    expect(switchesAutomatically(null)).toBe(false);
    expect(switchesAutomatically({ enabled: true, continue_after: false, fleet_enabled: false })).toBe(false);
    expect(switchesAutomatically({ enabled: false, continue_after: false, fleet_enabled: true })).toBe(false);
    expect(switchesAutomatically({ enabled: true, continue_after: false, fleet_enabled: true })).toBe(true);
  });
});
