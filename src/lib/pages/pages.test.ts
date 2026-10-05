import { describe, it, expect } from 'vitest';
import {
  boundRef,
  evalCondition,
  filterDefaults,
  filterSummary,
  tableText,
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

  it('a guide naming a settled setting does not make search return it twice', () => {
    // SettingsNav renders `{#each hits as h (h.key)}`, so two hits with one
    // key is a Svelte duplicate-key error, not a cosmetic repeat. $allPages is
    // the compiled pages PLUS every live guide, and a guide names settings
    // that already have a home — so one approved guide was enough to throw.
    const home = searchSettings('auto-tidy', bundle.pages, descs, defaults).find(
      (h) => h.key === 'work.auto_tidy',
    )!;
    const guide: (typeof bundle.pages)[number] = {
      spec: '{}',
      id: 'guide.tidy',
      title: 'Tidy up your fleet',
      layout: 'guide',
      sections: [
        {
          title: 'Turn auto-tidy on',
          items: [{ type: 'field', key: 'work.auto_tidy' }],
        },
      ],
    } as (typeof bundle.pages)[number];
    const withGuide = [...bundle.pages, guide];
    const hits = searchSettings('auto-tidy', withGuide, descs, defaults);
    const keys = hits.map((h) => h.key);
    expect(new Set(keys).size).toBe(keys.length);
    // and the one kept is the setting's real home, not the guide
    expect(hits.find((h) => h.key === 'work.auto_tidy')!.page).toBe(home.page);

    // a setting whose own page does not match the query, named only by a
    // guide whose SECTION title does, is still findable through the guide
    // (the haystack is label/key/help/tags plus the section title)
    const viaGuide = searchSettings('turn auto-tidy on', withGuide, descs, defaults);
    expect(viaGuide.map((h) => h.page)).toContain('guide.tidy');
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
    expect(formatCell('time', null)).toBe('never');
    expect(formatCell('time', 1000 - 7200, 1000)).toBe('2 h ago');
  });
});

describe('resources on a paired desktop', () => {
  it('every resource names a command the hub reasons explain: its update, else its create or delete', async () => {
    const { resourceBlock } = await import('../hub');
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
      const reason = resourceBlock(r, remote);
      expect(reason, r.id).toBeTruthy();
      // Standalone: nothing is blocked.
      expect(resourceBlock(r, { ...remote, remote: false, url: null }), r.id).toBeNull();
    }
    const catalog = bundle.resources.find((r) => r.id === 'catalog')!;
    expect(catalog.update).toBeUndefined();
    expect(resourceBlock(catalog, remote)).toContain('fleet-hub catalog add');
  });
});

describe('a data page\'s filter bar', () => {
  const usage = bundle.pages.find((p) => p.id === 'usage')!;
  const byDay = bundle.sources.find((s) => s.id === 'usage.by_day');
  const byHost = bundle.sources.find((s) => s.id === 'usage.by_host');

  it('starts at each default; a host filter at every host', () => {
    expect(filterDefaults(usage)).toEqual({ days: 30, host: null });
    expect(filterDefaults({ ...usage, filters: [{ param: 'days', choices: [7, 30] }] })).toEqual({ days: 7 });
    expect(filterDefaults({ ...usage, filters: undefined })).toEqual({});
  });

  it('binds only the params a source declares, and sends no host for every host', () => {
    expect(boundRef({ id: 'usage.by_day' }, byDay, { days: 7, host: null })).toEqual({
      id: 'usage.by_day',
      params: { days: 7 },
    });
    expect(boundRef({ id: 'usage.by_day' }, byDay, { days: 7, host: 'alpha' })).toEqual({
      id: 'usage.by_day',
      params: { days: 7, host: 'alpha' },
    });
    expect(boundRef({ id: 'usage.by_host' }, byHost, { days: 7, host: 'alpha' })).toEqual({ id: 'usage.by_host' });
    expect(boundRef({ id: 'nope' }, undefined, { days: 7 })).toEqual({ id: 'nope' });
  });

  it('says the filters in words, and a table as lines', () => {
    expect(filterSummary(usage, { days: 7, host: null })).toBe('last 7 d');
    expect(filterSummary(usage, { days: 7, host: 'alpha' })).toBe('last 7 d, host alpha');
    const cols = [
      { id: 'model', label: 'Model', ty: 'text' as const },
      { id: 'sessions', label: 'Sessions', ty: 'int' as const },
      { id: 'cost_micros', label: 'Est. cost', ty: 'usd_micros' as const },
    ];
    expect(tableText('Usage by model', cols, [{ model: 'opus', sessions: 2, cost_micros: 3_000_000 }])).toBe(
      'Usage by model\nopus: 2, $3.00',
    );
  });
});
