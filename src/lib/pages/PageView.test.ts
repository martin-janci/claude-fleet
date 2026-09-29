import { render, screen, fireEvent, waitFor, within } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import PageView from './PageView.svelte';
import { fleetSettings, SETTING_DEFAULTS } from '../fleet_settings';
import { allDescriptors, bundle, registryRouter } from './testing';
import type { Page } from './pages';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;
const descs = new Map(allDescriptors.map((d) => [d.key, d]));
const defaults = Object.fromEntries(allDescriptors.map((d) => [d.key, d.value]));
const pageOf = (id: string) => bundle.pages.find((p) => p.id === id) as Page;

function show(id: string, values: Record<string, string> = {}, focusKey: string | null = null) {
  const onnavigate = vi.fn();
  render(PageView, {
    props: {
      page: pageOf(id),
      pages: bundle.pages,
      descs,
      values: { ...defaults, ...values },
      sources: bundle.sources,
      focusKey,
      onnavigate,
    },
  });
  return onnavigate;
}

const control = (key: string) =>
  screen.getByTestId(`setting-${key.replace(/[^a-z0-9]+/gi, '-')}`) as HTMLInputElement;

beforeEach(() => {
  inv.mockReset();
  inv.mockImplementation(
    registryRouter({}, (cmd) => {
      if (cmd === 'work_retention_status')
        return { tables: [], last_sweep: null, tick_cap: 2000 };
      return null;
    }).impl,
  );
});
afterEach(() => fleetSettings.set({ ...SETTING_DEFAULTS }));

describe('PageView — every generated page', () => {
  it('renders every field it places, tab by tab, with the registry label and help', async () => {
    // All conditions true: turn on every switch a `when` reads.
    const on = Object.fromEntries(
      allDescriptors.filter((d) => d.kind.type === 'bool').map((d) => [d.key, 'true']),
    );
    // A master_detail page's fields are its resource's (ResourcePage.test.ts).
    for (const page of bundle.pages.filter((p) => p.layout !== 'master_detail')) {
      const { unmount } = render(PageView, {
        props: {
          page,
          pages: bundle.pages,
          descs,
          values: { ...defaults, ...on },
          sources: bundle.sources,
          onnavigate: () => {},
        },
      });
      const tabs = page.tabs ?? [];
      for (let t = 0; t < Math.max(1, tabs.length); t++) {
        if (tabs.length) await fireEvent.click(screen.getByTestId(`page-${page.id}-tab-${t}`));
        const sections = tabs.length ? tabs[t].sections : (page.sections ?? []);
        for (const item of sections.flatMap((s) => s.items)) {
          if (item.type !== 'field') continue;
          const row = screen.getByTestId(`setting-row-${item.key}`);
          const d = descs.get(item.key)!;
          expect(row.textContent, item.key).toContain(d.label);
          expect(row.textContent, item.key).toContain(d.help);
        }
      }
      unmount();
    }
  });
});

describe('PageView — fields', () => {
  it('hides a field whose condition is false', () => {
    show('settings.automation', { 'gc.enabled': 'false' });
    expect(screen.queryByTestId('setting-row-gc.bg_idle_secs')).toBeNull();
    expect(screen.getByTestId('setting-row-gc.external_lost_ttl_secs')).toBeTruthy();
  });

  it('shows seconds in hours and writes seconds', async () => {
    show('settings.automation', { 'gc.enabled': 'true' });
    const input = control('gc.bg_idle_secs');
    expect(input.value).toBe('24');
    input.value = '2';
    await fireEvent.change(input);
    await waitFor(() =>
      expect(inv).toHaveBeenCalledWith('set_fleet_setting', { key: 'gc.bg_idle_secs', value: '7200' }),
    );
  });

  it('never sends what it cannot represent, and shows the backend refusal', async () => {
    show('settings.limits');
    const move = control('move.max_transcript_mb');
    move.value = 'abc';
    await fireEvent.change(move);
    expect((await screen.findByTestId('setting-error-move.max_transcript_mb')).textContent).toContain(
      'enter a number',
    );
    expect(inv).not.toHaveBeenCalledWith('set_fleet_setting', expect.anything());
    move.value = '999999';
    await fireEvent.change(move);
    expect(
      await screen.findByText(/move.max_transcript_mb must be an integer between 1 and 4096/),
    ).toBeTruthy();
  });

  it('asks before a dangerous change, and cancel sends nothing', async () => {
    show('settings.automation');
    await fireEvent.click(control('gc.enabled'));
    const dialog = await screen.findByTestId('confirm-dialog');
    expect(dialog.textContent).toContain('stopped or removed without asking');
    await fireEvent.click(within(dialog).getByText('Cancel'));
    expect(inv).not.toHaveBeenCalledWith('set_fleet_setting', expect.anything());

    await fireEvent.click(control('gc.enabled'));
    await fireEvent.click(await screen.findByTestId('setting-confirm-gc.enabled'));
    await waitFor(() =>
      expect(inv).toHaveBeenCalledWith('set_fleet_setting', { key: 'gc.enabled', value: 'true' }),
    );
  });

  it('writes a multi-choice in the registry order, with human labels', async () => {
    show('settings.work', { 'work.auto_tidy': 'true', 'work.auto_tidy_reasons': 'pr_merged_idle' });
    await fireEvent.click(screen.getByTestId('page-settings.work-tab-1'));
    const row = screen.getByTestId('setting-row-work.auto_tidy_reasons');
    expect(row.textContent).toContain('PR merged, idle');
    await fireEvent.click(screen.getByTestId('setting-work-auto-tidy-reasons-done_idle'));
    await waitFor(() =>
      expect(inv).toHaveBeenCalledWith('set_fleet_setting', {
        key: 'work.auto_tidy_reasons',
        value: 'done_idle,pr_merged_idle',
      }),
    );
  });

  it('marks a changed setting and resets it to the default', async () => {
    show('settings.work', { 'work.recent_days': '30' });
    expect(screen.getByTestId('setting-row-work.recent_days').classList.contains('modified')).toBe(true);
    expect(screen.getByTestId('page-modified-count').textContent).toContain('1 changed');
    await fireEvent.click(screen.getByTestId('setting-reset-work.recent_days'));
    await waitFor(() =>
      expect(inv).toHaveBeenCalledWith('set_fleet_setting', { key: 'work.recent_days', value: '14' }),
    );
  });

  it('shows a key another subsystem owns read-only, with where to change it', () => {
    show('settings.hub', { 'hub.bind': '0.0.0.0' });
    const row = screen.getByTestId('setting-row-hub.bind');
    expect(row.textContent).toContain('0.0.0.0');
    expect(row.textContent).toContain('Change it with fleet-hub serve --bind');
    expect(row.querySelector('input')).toBeNull();
    expect(screen.queryByTestId('setting-reset-hub.bind')).toBeNull();
  });

  it('opens the tab a search hit is on and highlights the setting', async () => {
    show('settings.work', {}, 'work.retention.journal_days');
    await waitFor(() =>
      expect(screen.getByTestId('setting-row-work.retention.journal_days').classList.contains('highlighted')).toBe(
        true,
      ),
    );
    expect(screen.getByTestId('page-settings.work-tab-2').getAttribute('aria-selected')).toBe('true');
  });
});

describe('PageView — a search hit', () => {
  it('opens the tab the setting is on when an earlier tab is hidden by its when', async () => {
    const work = pageOf('settings.work');
    const tabs = work.tabs ?? [];
    expect(tabs.map((t) => t.title)).toEqual(['Detection', 'Tidy-up', 'Retention']);
    const page: Page = { ...work, tabs: [{ ...tabs[0], when: { key: 'work.recent_days', eq: 'never' } }, ...tabs.slice(1)] };
    render(PageView, {
      props: {
        page,
        pages: bundle.pages,
        descs,
        values: defaults,
        sources: bundle.sources,
        focusKey: 'work.tidy_done_days',
        onnavigate: () => {},
      },
    });
    expect(await screen.findByTestId('setting-row-work.tidy_done_days')).toBeTruthy();
    expect(screen.queryByTestId('setting-row-work.retention.journal_days')).toBeNull();
    expect(screen.queryByTestId('page-settings.work-tab-2')).toBeNull();
  });
});

describe('PageView — data and links', () => {
  it('shows stats, a chart with its table, and tables from their sources', async () => {
    show('usage');
    const stat = (await screen.findAllByTestId('data-stat-usage.total'))[0];
    await waitFor(() => expect(stat.textContent).toContain('$12.34'));
    const table = await screen.findByTestId('data-table-usage.by_model');
    await waitFor(() => expect(table.textContent).toContain('claude-opus-5'));
    expect(table.textContent).toContain('$3.00');
    const chart = await screen.findByTestId('data-chart-usage.by_day');
    expect(chart.querySelectorAll('path.bar')).toHaveLength(2);
    await fireEvent.click(screen.getByTestId('data-chart-usage.by_day-table-toggle'));
    expect(screen.getByTestId('data-chart-usage.by_day-table').textContent).toContain('$5.00');
    expect(inv).toHaveBeenCalledWith('fetch_page_source', { id: 'usage.by_day', params: { days: 30 } });
  });

  it('follows a link to another page', async () => {
    const onnavigate = show('settings');
    await fireEvent.click(screen.getByTestId('page-link-settings.automation'));
    expect(onnavigate).toHaveBeenCalledWith('settings.automation');
  });
});

describe('PageView — a page action (declarative pages P5)', () => {
  it('shows work retention from its sources, and Sweep now runs the command and re-reads them', async () => {
    const reads = () =>
      inv.mock.calls.filter((c) => c[0] === 'fetch_page_source' && (c[1] as { id: string }).id === 'work.retention')
        .length;
    const page = bundle.pages.find((p) =>
      JSON.stringify(p).includes('"action":"work.retention_sweep"'),
    ) as Page;
    render(PageView, {
      props: {
        page,
        pages: bundle.pages,
        descs,
        values: defaults,
        sources: bundle.sources,
        actions: bundle.actions,
        focusKey: 'work.retention.journal_days',
        onnavigate: () => {},
      },
    });
    const table = await screen.findByTestId('data-table-work.retention');
    await waitFor(() => expect(within(table).getAllByRole('row')).toHaveLength(4));
    expect(table.textContent).toContain('Work timeline');
    expect(screen.getByTestId('data-record-work.retention_last').textContent).toContain('never');
    const before = reads();
    await fireEvent.click(screen.getByTestId('page-action-work.retention_sweep'));
    await waitFor(() => expect(inv).toHaveBeenCalledWith('work_retention_sweep', undefined));
    await waitFor(() => expect(reads()).toBeGreaterThan(before));
  });

  it('shows no action on a read-only page', async () => {
    const page = bundle.pages.find((p) =>
      JSON.stringify(p).includes('"action":"work.retention_sweep"'),
    ) as Page;
    render(PageView, {
      props: {
        page,
        pages: bundle.pages,
        descs,
        values: defaults,
        sources: bundle.sources,
        actions: bundle.actions,
        readonly: true,
        focusKey: 'work.retention.journal_days',
        onnavigate: () => {},
      },
    });
    await screen.findByTestId('setting-row-work.retention.journal_days');
    expect(screen.queryByTestId('page-action-work.retention_sweep')).toBeNull();
  });
});
