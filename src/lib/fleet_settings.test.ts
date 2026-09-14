import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import {
  fleetSettings,
  basePathError,
  hoursToSecs,
  loadFleetSettings,
  projectDir,
  projectPathPreview,
  projectsDefaultRoot,
  secsToHours,
  settingLayout,
  settingPathMap,
  setFleetSetting,
  settingBool,
  settingSecs,
  settingInt,
  parseHoursInput,
  parseIntInput,
  parsePricesJsonInput,
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
    expect(settingBool(m, SETTING_KEYS.repairAutoOnTick)).toBe(false);
    expect(settingSecs(m, SETTING_KEYS.repairTickIntervalSecs)).toBe(600);
    expect(settingSecs(m, SETTING_KEYS.tasksMaxAgeSecs)).toBe(86400);
    expect(settingSecs(m, SETTING_KEYS.moveMaxTranscriptMb)).toBe(200);
    expect(settingBool(m, SETTING_KEYS.usageEnabled)).toBe(true);
    expect(settingSecs(m, SETTING_KEYS.usageIntervalSecs)).toBe(300);
    expect(m[SETTING_KEYS.usagePricesJson]).toBe('{}');
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

  it('settingInt reads a count and falls back to the default on garbage', () => {
    expect(settingInt({}, SETTING_KEYS.moveMaxTranscriptMb)).toBe(200);
    expect(settingInt({ 'move.max_transcript_mb': '64' }, SETTING_KEYS.moveMaxTranscriptMb)).toBe(64);
    expect(settingInt({ 'move.max_transcript_mb': 'x' }, SETTING_KEYS.moveMaxTranscriptMb)).toBe(200);
  });

  it('parseHoursInput refuses what would silently mean "never"', () => {
    expect(parseHoursInput('2')).toEqual({ secs: 7200 });
    expect(parseHoursInput(' 0 ')).toEqual({ secs: 0 });
    expect(parseHoursInput('0.5')).toEqual({ secs: 1800 });
    // Above the backend cap is passed through: the backend reports the range.
    expect(parseHoursInput('100000')).toEqual({ secs: 360000000 });
    for (const [raw, message] of [
      ['', /enter a number/],
      ['abc', /not a number/],
      ['-1', /0 or more/],
      ['0.00001', /rounds to 0 seconds/],
    ] as const) {
      const r = parseHoursInput(raw);
      expect('error' in r && r.error).toMatch(message);
    }
  });

  it('parseIntInput refuses empty and non-integer input, passes integers through', () => {
    expect(parseIntInput('64')).toEqual({ value: '64' });
    expect(parseIntInput(' 007 ')).toEqual({ value: '7' });
    expect(parseIntInput('5000')).toEqual({ value: '5000' });
    expect(parseIntInput('-1')).toEqual({ value: '-1' });
    expect(parseIntInput('')).toEqual({ error: 'enter a whole number' });
    expect(parseIntInput('1.5')).toEqual({ error: '"1.5" is not a whole number' });
  });

  it('parsePricesJsonInput maps empty to {} and refuses what is not a JSON object', () => {
    expect(parsePricesJsonInput('  ')).toEqual({ value: '{}' });
    expect(parsePricesJsonInput(' {"opus":{"input":1}} ')).toEqual({ value: '{"opus":{"input":1}}' });
    expect(parsePricesJsonInput('{oops')).toEqual({ error: 'not valid JSON' });
    for (const raw of ['[]', 'null', '42', '"x"']) {
      expect('error' in parsePricesJsonInput(raw)).toBe(true);
    }
  });

  it('hours <-> seconds round-trip', () => {
    expect(secsToHours(86400)).toBe(24);
    expect(secsToHours(5400)).toBe(1.5);
    expect(hoursToSecs(1.5)).toBe(5400);
    expect(hoursToSecs(-1)).toBe(0);
  });
});

describe('projects settings helpers (W5 G3)', () => {
  it('defaults: no per-host overrides, github layout', () => {
    const m = get(fleetSettings);
    expect(settingPathMap(m, SETTING_KEYS.projectsBasePath)).toEqual({});
    expect(settingLayout(m)).toBe('github');
  });

  it('settingPathMap tolerates garbage and drops non-string values', () => {
    expect(settingPathMap({ k: '{oops' }, 'k')).toEqual({});
    expect(settingPathMap({ k: '[1]' }, 'k')).toEqual({});
    expect(settingPathMap({ k: '{"a":"/x","b":2}' }, 'k')).toEqual({ a: '/x' });
  });

  it('basePathError mirrors the backend validation', () => {
    for (const ok of ['', '/srv/repos', '~', '~/code', '/a b/c-d!']) expect(basePathError(ok)).toBeNull();
    expect(basePathError('code')).toMatch(/absolute/);
    expect(basePathError('~user/x')).toMatch(/absolute/);
    expect(basePathError('/a/../b')).toMatch(/\.\./);
    expect(basePathError('/a\nb')).toMatch(/control/);
    // C1 controls (U+0080..U+009F) and DEL, like Rust's char::is_control
    expect(basePathError('/a' + String.fromCharCode(0x85) + 'b')).toMatch(/control/);
    expect(basePathError('/a' + String.fromCharCode(0x7f) + 'b')).toMatch(/control/);
    expect(basePathError('/a' + String.fromCharCode(0xa0) + 'b')).toBeNull();
    // the backend's 1024-char cap
    expect(basePathError('/' + 'a'.repeat(1023))).toBeNull();
    expect(basePathError('/' + 'a'.repeat(1024))).toMatch(/too long/);
  });

  it('projectDir mirrors Layout::project_dir', () => {
    expect(projectDir('~/code/', 'flat', 'o', 'r')).toBe('~/code/r');
    expect(projectDir('~/projects/github.com', 'github', 'o', 'r')).toBe('~/projects/github.com/o/r');
  });

  it('previews per layout and exposes the layout defaults', () => {
    expect(projectPathPreview('~/code/', 'flat')).toBe('~/code/<repo>');
    expect(projectPathPreview('/p', 'github')).toBe('/p/<owner>/<repo>');
    expect(projectsDefaultRoot('github')).toBe('~/projects/github.com');
    expect(projectsDefaultRoot('flat')).toBe('~/projects');
  });
});
