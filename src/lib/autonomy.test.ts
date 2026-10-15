import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import { autonomyOf, autonomyWrites, setAutonomy } from './autonomy';
import { SETTING_DEFAULTS, type FleetSettings } from './fleet_settings';

// Thread room (2026-10-14, board Autonomy): one dial for three settings.

const inv = invoke as unknown as ReturnType<typeof vi.fn>;
const with_ = (over: Record<string, string>): FleetSettings => ({ ...SETTING_DEFAULTS, ...over });

beforeEach(() => {
  inv.mockReset();
  inv.mockImplementation(async (_cmd: string, a: { key: string; value: string }) => ({ ...SETTING_DEFAULTS, [a.key]: a.value }));
});

describe('autonomy dial', () => {
  it('reads each position from the settings, and a mix it does not name as Custom', () => {
    expect(autonomyOf(with_({ 'orchestrator.max_level': '1', 'control.auto_continue': 'false' }))).toBe('ask');
    expect(autonomyOf(with_({ 'orchestrator.max_level': '2', 'control.auto_continue': 'true', 'control.auto_continue_minutes': '10' }))).toBe('brief');
    expect(autonomyOf(with_({ 'orchestrator.max_level': '3', 'control.auto_continue': 'true', 'control.auto_continue_minutes': '2' }))).toBe('autopilot');
    expect(autonomyOf(with_({ 'orchestrator.max_level': '3', 'control.auto_continue_minutes': '30' }))).toBe('custom');
    expect(autonomyOf(with_({ 'orchestrator.max_level': '2', 'control.react_to_events': 'false' }))).toBe('custom');
  });

  it('a position sets the mission level, auto-continue and its wait, and the event feed together', () => {
    expect(autonomyWrites('autopilot', with_({ 'control.react_to_events': 'false' }))).toEqual([
      ['orchestrator.max_level', '3'],
      ['control.auto_continue_minutes', '2'],
      ['control.react_to_events', 'true'],
    ]);
    expect(autonomyWrites('ask', with_({}))).toEqual([['control.auto_continue', 'false']]);
  });

  it('writes through set_fleet_setting, and stops at the first refusal', async () => {
    expect(await setAutonomy('brief', with_({}))).toBeNull();
    expect(inv.mock.calls.map((c) => [c[0], c[1]])).toEqual([['set_fleet_setting', { key: 'orchestrator.max_level', value: '2' }]]);
    inv.mockReset();
    inv.mockRejectedValue({ code: 'E_FORBIDDEN', message: 'no' });
    expect(await setAutonomy('autopilot', with_({}))).toContain('orchestrator.max_level');
    expect(inv).toHaveBeenCalledTimes(1);
  });
});
