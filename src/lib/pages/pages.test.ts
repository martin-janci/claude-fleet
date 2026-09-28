import { describe, it, expect } from 'vitest';
import {
  evalCondition,
  formatCell,
  fromDisplay,
  homeOf,
  rangeText,
  searchSettings,
  toDisplay,
  valuesOf,
} from './pages';
import { allDescriptors, bundle, descriptorOf } from './testing';

const descs = new Map(allDescriptors.map((d) => [d.key, d]));
const defaults = Object.fromEntries(allDescriptors.map((d) => [d.key, d.value]));

describe('conditions', () => {
  it('reads each form, and a missing condition shows the item', () => {
    const v = { 'gc.enabled': 'true', 'decide.jev.work_link': 'shadow' };
    expect(evalCondition(undefined, v)).toBe(true);
    expect(evalCondition({ key: 'gc.enabled', truthy: true }, v)).toBe(true);
    expect(evalCondition({ key: 'gc.enabled', truthy: false }, v)).toBe(false);
    expect(evalCondition({ key: 'decide.jev.work_link', eq: 'assist' }, v)).toBe(false);
    expect(evalCondition({ key: 'decide.jev.work_link', in: ['shadow', 'assist'] }, v)).toBe(true);
    expect(
      evalCondition({ all: [{ key: 'gc.enabled', truthy: true }, { not: { key: 'decide.jev.work_link', eq: 'off' } }] }, v),
    ).toBe(true);
    expect(evalCondition({ any: [{ key: 'gc.enabled', truthy: false }] }, v)).toBe(false);
  });
});

describe('units', () => {
  it('shows seconds in the registry unit and sends seconds back', () => {
    const gc = descriptorOf('gc.bg_idle_secs'); // secs shown in hours
    expect(toDisplay(gc, '86400')).toBe('24');
    expect(toDisplay(gc, '5400')).toBe('1.5');
    expect(fromDisplay(gc, '2')).toEqual({ value: '7200' });
    expect(fromDisplay(gc, '0')).toEqual({ value: '0' });
    expect(fromDisplay(gc, '0.0001')).toHaveProperty('error');
    expect(fromDisplay(gc, '')).toHaveProperty('error');
    expect(fromDisplay(gc, '-1')).toHaveProperty('error');
  });

  it('sends an Int as typed and refuses a fraction', () => {
    const days = descriptorOf('work.recent_days');
    expect(toDisplay(days, '14')).toBe('14');
    expect(fromDisplay(days, '30')).toEqual({ value: '30' });
    expect(fromDisplay(days, '1.5')).toHaveProperty('error');
  });

  it('says the range and what 0 means', () => {
    expect(rangeText(descriptorOf('work.retention.journal_days'))).toBe('0–3650 days; 0 = forever');
    expect(rangeText(descriptorOf('gc.bg_idle_secs'))).toBe('hours; 0 = never');
    expect(rangeText(descriptorOf('repair.tick_interval_secs'))).toBe('at least 60 seconds');
    expect(rangeText(descriptorOf('health.context_red_pct'))).toBe('1–100 %');
  });
});

describe('search and the page tree', () => {
  it('finds a setting by label, key or help, and says where it lives', () => {
    const hits = searchSettings('auto-tidy', bundle.pages, descs, defaults);
    expect(hits.map((h) => h.key)).toContain('work.auto_tidy');
    const hit = hits.find((h) => h.key === 'work.auto_tidy')!;
    expect(hit.page).toBe('settings.work');
    expect(hit.tab).toBe(1);
    expect(searchSettings('decide.jev.timeout', bundle.pages, descs, defaults).map((h) => h.key)).toEqual([
      'decide.jev.timeout_ms',
    ]);
    expect(searchSettings('', bundle.pages, descs, defaults)).toEqual([]);
  });

  it('@modified lists only what is off its default; @tag filters by tag', () => {
    const values = { ...defaults, 'gc.enabled': 'true' };
    expect(searchSettings('@modified', bundle.pages, descs, values).map((h) => h.key)).toEqual(['gc.enabled']);
    const exp = searchSettings('@tag:experimental', bundle.pages, descs, values).map((h) => h.key);
    expect(exp).toContain('work.classify_nudge');
    expect(exp).not.toContain('gc.enabled');
  });

  it('every setting has one home, on the page and tab search sends you to', () => {
    for (const d of allDescriptors) {
      expect(homeOf(bundle.pages, d.key), d.key).not.toBeNull();
    }
  });

  it('values come from the live map over what describe returned', () => {
    const v = valuesOf(descs, { 'gc.enabled': 'true' });
    expect(v['gc.enabled']).toBe('true');
    expect(v['work.recent_days']).toBe('14');
  });
});

describe('formatting', () => {
  it('formats by column type', () => {
    expect(formatCell('usd_micros', 12_340_000)).toBe('$12.34');
    expect(formatCell('tokens', 1_500_000)).toBe('1.5M');
    expect(formatCell('tokens', 42_000)).toBe('42.0k');
    expect(formatCell('int', 1234)).toBe('1,234');
    expect(formatCell('day', '2026-09-27')).toBe('2026-09-27');
    expect(formatCell('text', null)).toBe('—');
  });
});

describe('resources on a paired desktop', () => {
  it('every resource update names a command the hub reasons explain', async () => {
    const { hubBlock } = await import('../hub');
    const remote = {
      remote: true,
      url: 'https://fleet.example.com',
      client_name: 'laptop',
      client_mode: null,
      configured_url: 'https://fleet.example.com',
      configured_client_name: 'laptop',
      allow_plaintext: false,
      warning: null,
      restart_required: false,
      unavailable: null,
    };
    for (const r of bundle.resources) {
      if (!r.update) continue;
      const reason = hubBlock(r.update.command as Parameters<typeof hubBlock>[0], remote);
      expect(reason, r.id).toBeTruthy();
    }
  });
});
