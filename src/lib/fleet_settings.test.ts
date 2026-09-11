import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import {
  fleetSettings,
  hoursToSecs,
  loadFleetSettings,
  secsToHours,
  setFleetSetting,
  settingBool,
  settingSecs,
  SETTING_DEFAULTS,
  SETTING_KEYS,
} from './fleet_settings';

beforeEach(() => {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
  fleetSettings.set({ ...SETTING_DEFAULTS });
});

describe('fleet settings', () => {
  it('defaults mirror the backend registry (playbooks + gc off, TTLs set)', () => {
    const m = get(fleetSettings);
    expect(settingBool(m, SETTING_KEYS.playbookPressEnter)).toBe(false);
    expect(settingBool(m, SETTING_KEYS.playbookOomRecreate)).toBe(false);
    expect(settingBool(m, SETTING_KEYS.gcEnabled)).toBe(false);
    expect(settingSecs(m, SETTING_KEYS.gcBgIdleSecs)).toBe(86400);
    expect(settingSecs(m, SETTING_KEYS.gcShellIdleSecs)).toBe(604800);
    expect(settingSecs(m, SETTING_KEYS.gcWorkIdleSecs)).toBe(0);
    expect(settingSecs(m, SETTING_KEYS.gcSweepIntervalSecs)).toBe(300);
    expect(settingSecs(m, SETTING_KEYS.reconcileIntervalSecs)).toBe(20);
  });

  it('settingSecs falls back to the default on garbage', () => {
    expect(settingSecs({ 'gc.bg_idle_secs': 'abc' }, SETTING_KEYS.gcBgIdleSecs)).toBe(86400);
    expect(settingSecs({ 'gc.bg_idle_secs': '-5' }, SETTING_KEYS.gcBgIdleSecs)).toBe(86400);
    expect(settingSecs({ 'gc.bg_idle_secs': '42' }, SETTING_KEYS.gcBgIdleSecs)).toBe(42);
  });

  it('loadFleetSettings merges the backend map over the defaults', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce({ 'gc.enabled': 'true' });
    const r = await loadFleetSettings();
    expect(r.ok).toBe(true);
    expect(mockedInvoke).toHaveBeenCalledWith('get_fleet_settings', undefined);
    const m = get(fleetSettings);
    expect(settingBool(m, SETTING_KEYS.gcEnabled)).toBe(true);
    expect(settingSecs(m, SETTING_KEYS.gcBgIdleSecs)).toBe(86400);
  });

  it('setFleetSetting sends key/value and adopts the returned map', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      ...SETTING_DEFAULTS,
      'playbooks.press_enter': 'true',
    });
    const r = await setFleetSetting(SETTING_KEYS.playbookPressEnter, 'true');
    expect(r.ok).toBe(true);
    expect(mockedInvoke).toHaveBeenCalledWith('set_fleet_setting', {
      key: 'playbooks.press_enter',
      value: 'true',
    });
    expect(settingBool(get(fleetSettings), SETTING_KEYS.playbookPressEnter)).toBe(true);
  });

  it('hours <-> seconds round-trip', () => {
    expect(secsToHours(86400)).toBe(24);
    expect(secsToHours(5400)).toBe(1.5);
    expect(hoursToSecs(1.5)).toBe(5400);
    expect(hoursToSecs(-1)).toBe(0);
  });
});
