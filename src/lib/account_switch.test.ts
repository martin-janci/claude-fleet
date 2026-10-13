import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import {
  announceAccountSwitches,
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
    await autoSwitchAccount(false);
    expect(inv).toHaveBeenLastCalledWith('auto_switch_account', { args: { enabled: false } });
  });

  it('parses an attempt and refuses what is not one', () => {
    expect(parseSwitchDetail(detail())).toEqual({
      from_account: 'acct-out',
      to_account: 'acct-spare',
      profile: 'spare',
      used_pct: 12.4,
      error: null,
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
});
