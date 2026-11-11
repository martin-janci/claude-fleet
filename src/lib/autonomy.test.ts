import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import { autonomyOf, autonomyWrites, setAutonomy } from './autonomy';
import { fleetSettings, SETTING_DEFAULTS, type FleetSettings } from './fleet_settings';

// Thread room (2026-10-14, board Autonomy): one dial for three settings.

const inv = invoke as unknown as ReturnType<typeof vi.fn>;
const with_ = (over: Record<string, string>): FleetSettings => ({ ...SETTING_DEFAULTS, ...over });

/** What the hub holds; `get_fleet_settings` answers it and each write
 *  changes it, so a test can make it differ from the window's copy. */
let hub: FleetSettings = with_({});

beforeEach(() => {
  hub = with_({});
  inv.mockReset();
  inv.mockImplementation(async (cmd: string, a: { key: string; value: string }) => {
    if (cmd === 'get_fleet_settings') return { ...hub };
    if (cmd === 'set_fleet_setting') {
      hub = { ...hub, [a.key]: a.value };
      return { ...hub };
    }
    return null;
  });
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

  it('reads the hub first, writes through set_fleet_setting, and stops at the first refusal', async () => {
    expect(await setAutonomy('brief')).toBeNull();
    expect(inv.mock.calls.map((c) => [c[0], c[1]])).toEqual([
      ['get_fleet_settings', undefined],
      ['set_fleet_setting', { key: 'orchestrator.max_level', value: '2' }],
    ]);
    inv.mockReset();
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'get_fleet_settings') return { ...hub };
      throw { code: 'E_FORBIDDEN', message: 'no' };
    });
    expect(await setAutonomy('autopilot')).toContain('orchestrator.max_level');
    expect(inv.mock.calls.filter((c) => c[0] === 'set_fleet_setting')).toHaveLength(1);
  });

  it('works the writes out from what the hub holds, not from the window copy (probe3 c-1)', async () => {
    // The window still holds Ask first; the phone moved the hub to Autopilot.
    fleetSettings.set(with_({ 'orchestrator.max_level': '1', 'control.auto_continue': 'false' }));
    hub = with_({ 'orchestrator.max_level': '3', 'control.auto_continue': 'true', 'control.auto_continue_minutes': '2' });
    expect(await setAutonomy('ask')).toBeNull();
    const writes = inv.mock.calls.filter((c) => c[0] === 'set_fleet_setting').map((c) => [c[1].key, c[1].value]);
    expect(writes).toEqual([
      ['orchestrator.max_level', '1'],
      ['control.auto_continue', 'false'],
    ]);
    expect(autonomyOf(hub)).toBe('ask');
  });

  it('a pick that does not land is reported, and a failed read writes nothing', async () => {
    // Another device puts the level back between this window's writes.
    inv.mockImplementation(async (cmd: string, a: { key: string; value: string }) => {
      if (cmd === 'get_fleet_settings') return { ...hub };
      hub = { ...hub, [a.key]: a.value, 'orchestrator.max_level': '3' };
      return { ...hub };
    });
    expect(await setAutonomy('brief')).toContain('Pick again');

    inv.mockReset();
    inv.mockRejectedValue({ code: 'E_HUB_UNREACHABLE', message: 'down' });
    expect(await setAutonomy('ask')).toContain("Couldn't read");
    expect(inv.mock.calls.some((c) => c[0] === 'set_fleet_setting')).toBe(false);
  });
});
