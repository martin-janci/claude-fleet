import { render, screen, fireEvent, waitFor, within } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('../clipboard', () => ({ copyText: vi.fn(() => Promise.resolve(true)) }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { copyText } from '../clipboard';
import { hosts, type HostRow } from '../hosts';
import { accounts } from '../accounts';
import { accountUsage } from '../account_usage_store';
import { WORK, GMAIL, snapshot } from '../hosts_fixture';
import PageView from './PageView.svelte';
import { fleetSettings, SETTING_DEFAULTS } from '../fleet_settings';
import { allDescriptors, bundle, registryRouter } from './testing';
import type { Page } from './pages';
import { expectAccessible } from '../a11y_check';
import { get } from 'svelte/store';
import { toasts, runToastAction } from '../toasts';

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
    await fireEvent.click(screen.getByTestId('page-save'));
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
    await fireEvent.click(screen.getByTestId('page-save'));
    expect(
      await screen.findByText(/move.max_transcript_mb must be an integer between 1 and 4096/),
    ).toBeTruthy();
    // Refused: the typed value stays staged, so nothing typed is lost.
    expect(screen.getByTestId('page-save-count').textContent).toBe('1 change');
    expect(move.value).toBe('999999');
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
    expect(screen.getByTestId('setting-changed-work.recent_days').textContent).toBe('changed from 14 days');
    await fireEvent.click(screen.getByTestId('setting-reset-work.recent_days'));
    expect(control('work.recent_days').value).toBe('14');
    await fireEvent.click(screen.getByTestId('page-save'));
    await waitFor(() =>
      expect(inv).toHaveBeenCalledWith('set_fleet_setting', { key: 'work.recent_days', value: '14' }),
    );
  });

  it('batches typed values: N changes, Discard puts them back, Save writes them all', async () => {
    show('settings.automation', { 'gc.enabled': 'true' });
    expect(screen.queryByTestId('page-save-bar')).toBeNull();
    const idle = control('gc.bg_idle_secs');
    idle.value = '2';
    await fireEvent.change(idle);
    const lost = control('gc.external_lost_ttl_secs');
    const lostBefore = lost.value;
    lost.value = String(Number(lostBefore) + 1);
    await fireEvent.change(lost);
    expect(screen.getByTestId('page-save-count').textContent).toBe('2 changes');
    expect(screen.getByTestId('setting-unsaved-gc.bg_idle_secs')).toBeTruthy();
    expect(inv).not.toHaveBeenCalledWith('set_fleet_setting', expect.anything());

    // Typing the stored value back is no change.
    lost.value = lostBefore;
    await fireEvent.change(lost);
    expect(screen.getByTestId('page-save-count').textContent).toBe('1 change');

    await fireEvent.click(screen.getByTestId('page-discard'));
    expect(screen.queryByTestId('page-save-bar')).toBeNull();
    expect(control('gc.bg_idle_secs').value).toBe('24');
    expect(inv).not.toHaveBeenCalledWith('set_fleet_setting', expect.anything());

    const i2 = control('gc.bg_idle_secs');
    i2.value = '3';
    await fireEvent.change(i2);
    await fireEvent.click(screen.getByTestId('page-save'));
    await waitFor(() => expect(screen.queryByTestId('page-save-bar')).toBeNull());
    expect(inv).toHaveBeenCalledWith('set_fleet_setting', { key: 'gc.bg_idle_secs', value: '10800' });
  });

  it('a switch saves at once, and its toast puts the old value back', async () => {
    toasts.set([]);
    show('settings.work');
    await fireEvent.click(control('work.evidence_snippets'));
    const before = descs.get('work.evidence_snippets')!.value;
    const next = before === 'true' ? 'false' : 'true';
    await waitFor(() =>
      expect(inv).toHaveBeenCalledWith('set_fleet_setting', { key: 'work.evidence_snippets', value: next }),
    );
    expect(screen.queryByTestId('page-save-bar')).toBeNull();
    const toast = get(toasts).at(-1)!;
    expect(toast.action?.label).toBe('Undo');
    runToastAction(toast.id);
    await waitFor(() =>
      expect(inv).toHaveBeenCalledWith('set_fleet_setting', { key: 'work.evidence_snippets', value: before }),
    );
  });

  it('says where each value lives: this device, the hub, or an org that overrides it', async () => {
    const own = new Map(descs);
    own.set('work.summary_model', {
      ...descs.get('work.summary_model')!,
      scope: 'org',
      org_values: [{ org_id: 3, org: '32bit', value: 'opus' }],
    });
    const { unmount } = render(PageView, {
      props: { page: pageOf('settings.work'), pages: bundle.pages, descs: own, values: defaults, sources: bundle.sources, onnavigate: () => {} },
    });
    expect(screen.getByTestId('setting-scope-pill-work.recent_days').textContent).toBe('this device');
    expect(screen.getByTestId('setting-scope-work.recent_days').textContent).toContain('default');
    expect(screen.getByTestId('setting-scope-pill-work.summary_model').textContent).toBe('org 32bit');
    expect(screen.getByTestId('setting-scope-work.summary_model').textContent).toContain('overrides the fleet');
    expect(screen.getByTestId('setting-scope-pill-work.summary_model').getAttribute('title')).toContain('32bit: opus');
    unmount();
    render(PageView, {
      props: { page: pageOf('settings.work'), pages: bundle.pages, descs: own, values: defaults, sources: bundle.sources, remote: true, onnavigate: () => {} },
    });
    expect(screen.getByTestId('setting-scope-pill-work.recent_days').textContent).toBe('hub');
    expect(screen.getByTestId('setting-scope-work.summary_model').textContent).toContain('overrides the hub');
  });

  it('a read-only page stages nothing and offers no Reset', async () => {
    render(PageView, {
      props: {
        page: pageOf('settings.work'),
        pages: bundle.pages,
        descs,
        values: { ...defaults, 'work.recent_days': '30' },
        sources: bundle.sources,
        readonly: true,
        remote: true,
        onnavigate: () => {},
      },
    });
    expect(screen.getByTestId('setting-changed-work.recent_days').textContent).toBe('changed from 14 days');
    expect(screen.queryByTestId('setting-reset-work.recent_days')).toBeNull();
    expect(screen.queryByTestId('page-save-bar')).toBeNull();
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

  it('a filter sets its param on every source that takes it, and re-reads only those (the filter bar)', async () => {
    const host = (alias: string) => ({ alias, hidden: false }) as unknown as HostRow;
    hosts.set([host('beta'), host('alpha')]);
    show('usage');
    const days = (await screen.findByTestId('page-filter-days')) as HTMLSelectElement;
    expect(days.value).toBe('30');
    const hostSel = screen.getByTestId('page-filter-host') as HTMLSelectElement;
    expect(Array.from(hostSel.options).map((o) => o.textContent)).toEqual(['All hosts', 'alpha', 'beta']);
    await waitFor(() => expect(inv).toHaveBeenCalledWith('fetch_page_source', { id: 'usage.total', params: null }));

    await fireEvent.change(days, { target: { value: '7' } });
    await waitFor(() => expect(inv).toHaveBeenCalledWith('fetch_page_source', { id: 'usage.by_day', params: { days: 7 } }));
    // usage.total takes no window: the change does not re-read it.
    expect(inv.mock.calls.filter((c) => (c[1] as { id?: string })?.id === 'usage.by_host')).toHaveLength(1);

    await fireEvent.change(hostSel, { target: { value: 'alpha' } });
    await waitFor(() =>
      expect(inv).toHaveBeenCalledWith('fetch_page_source', { id: 'usage.by_day', params: { days: 7, host: 'alpha' } }),
    );
    expect(inv).toHaveBeenCalledWith('fetch_page_source', { id: 'usage.total', params: { host: 'alpha' } });
    expect(inv).toHaveBeenCalledWith('fetch_page_source', { id: 'usage.by_model', params: { host: 'alpha' } });
    hosts.set([]);
  });

  it('Work graph usage: the counts as a table, copied as text with the window (replaces WorkUsage)', async () => {
    show('usage.work');
    const table = await screen.findByTestId('data-table-work.usage');
    await waitFor(() => expect(table.textContent).toContain('3 made (manual 3)'));
    expect(inv).toHaveBeenCalledWith('fetch_page_source', { id: 'work.usage', params: { days: 30 } });
    await fireEvent.change(screen.getByTestId('page-filter-days'), { target: { value: '90' } });
    await waitFor(() => expect(inv).toHaveBeenCalledWith('fetch_page_source', { id: 'work.usage', params: { days: 90 } }));
    await fireEvent.click(screen.getByTestId('data-table-work.usage-copy'));
    await waitFor(() =>
      expect(copyText).toHaveBeenCalledWith('Work graph usage, last 90 d\nlinks: 3 made (manual 3)\ntrackers: none'),
    );
    expect(screen.getByTestId('data-table-work.usage-copy').textContent).toBe('Copied');
  });

  it('on a paired desktop a data page says where its data is, and reads nothing', async () => {
    render(PageView, {
      props: {
        page: pageOf('usage.work'),
        pages: bundle.pages,
        descs,
        values: defaults,
        sources: bundle.sources,
        remote: true,
        onnavigate: () => {},
      },
    });
    expect(screen.getByTestId('page-data-remote')).toBeInTheDocument();
    expect(screen.queryByTestId('page-filters')).toBeNull();
    expect(screen.queryByTestId('section-Counts')).toBeNull();
    expect(inv.mock.calls.some((c) => c[0] === 'fetch_page_source')).toBe(false);
  });

  it('Claude accounts: a usage block per account from the live store, refreshed through the floor (L8)', async () => {
    accounts.set([WORK, GMAIL]);
    accountUsage.set({ [WORK.uuid]: snapshot(WORK.uuid, { next_try_at: 0 }) });
    show('usage.accounts');
    const blocks = await screen.findAllByTestId('usage-block');
    expect(blocks).toHaveLength(2);
    expect(inv.mock.calls.some((c) => c[0] === 'fetch_page_source')).toBe(false);
    inv.mockImplementation(async (cmd: string) =>
      cmd === 'refresh_account_usage' ? snapshot(WORK.uuid) : null,
    );
    const work = screen
      .getAllByTestId('accounts-usage-account')
      .find((el) => el.getAttribute('data-uuid') === WORK.uuid)!;
    await fireEvent.click(within(work).getByTestId('usage-refresh'));
    await waitFor(() =>
      expect(inv).toHaveBeenCalledWith('refresh_account_usage', { args: { account_uuid: WORK.uuid } }),
    );
    accounts.set([]);
    accountUsage.set({});
  });

  it('Claude accounts: none yet, and nothing on a paired desktop', async () => {
    accounts.set([]);
    const { unmount } = render(PageView, {
      props: { page: pageOf('usage.accounts'), pages: bundle.pages, descs, values: defaults, sources: bundle.sources, onnavigate: () => {} },
    });
    expect(screen.getByTestId('accounts-usage-empty')).toBeInTheDocument();
    unmount();
    accounts.set([WORK]);
    render(PageView, {
      props: { page: pageOf('usage.accounts'), pages: bundle.pages, descs, values: defaults, sources: bundle.sources, remote: true, onnavigate: () => {} },
    });
    expect(screen.getByTestId('page-data-remote')).toBeInTheDocument();
    expect(screen.queryByTestId('usage-block')).toBeNull();
    accounts.set([]);
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

describe('PageView — the notifications matrix (11.9)', () => {
  it('shows a row per state and a column per channel, and a tick saves that channel', async () => {
    show('settings.notifications');
    const grid = screen.getByTestId('settings-matrix');
    for (const h of ['Desktop', 'Phone', 'Sound']) expect(within(grid).getByText(h)).toBeTruthy();
    const desktopDone = screen.getByTestId('matrix-notify.desktop-done') as HTMLInputElement;
    expect(desktopDone.checked).toBe(false);
    expect((screen.getByTestId('matrix-notify.desktop-blocked') as HTMLInputElement).checked).toBe(true);
    await fireEvent.click(desktopDone);
    await waitFor(() =>
      expect(inv).toHaveBeenCalledWith('set_fleet_setting', {
        key: 'notify.desktop',
        value: 'needs_you,failed,blocked,done,routine_failed',
      }),
    );
    // Quiet hours stay ordinary rows.
    expect(control('notify.quiet_hours')).toBeTruthy();
  });

  it('is read-only on a hub client', () => {
    render(PageView, {
      props: {
        page: pageOf('settings.notifications'),
        pages: bundle.pages,
        descs,
        values: defaults,
        sources: bundle.sources,
        readonly: true,
        onnavigate: vi.fn(),
      },
    });
    expect((screen.getByTestId('matrix-notify.phone-failed') as HTMLInputElement).disabled).toBe(true);
  });
});

describe('PageView — Updates lists every part of the fleet (11.9b)', () => {
  const rows = [
    { device: 'Hub', part: 'Hub', version: '0.5.4', update: 'Up to date', reported_at: 100 },
    { device: 'mercury', part: 'Agent', version: '0.5.3', update: 'Update available', offers: '0.5.4', reported_at: 100 },
    { device: 'Device 7', part: 'Phone', version: '0.5.3', update: 'Update available', offers: '0.5.4', reported_at: 100 },
  ];

  function updates(props: { readonly?: boolean; remote?: boolean } = {}) {
    inv.mockImplementation(
      registryRouter({}, (cmd) => (cmd === 'list_update_targets' ? rows : null)).impl,
    );
    render(PageView, {
      props: {
        page: pageOf('settings.updates'),
        pages: bundle.pages,
        descs,
        values: defaults,
        sources: bundle.sources,
        onnavigate: vi.fn(),
        ...props,
      },
    });
  }

  it('reads the hub through list_update_targets, agents and the phone included', async () => {
    updates();
    const table = await screen.findByTestId('data-table-updates.targets');
    await waitFor(() => expect(within(table).getByText('mercury')).toBeTruthy());
    expect(within(table).getByText('Phone')).toBeTruthy();
    expect(within(table).getAllByText('0.5.4').length).toBeGreaterThan(1);
    expect(inv).toHaveBeenCalledWith('list_update_targets', undefined);
    expect(inv).not.toHaveBeenCalledWith('fetch_page_source', expect.anything());
  });

  it('still shows on a paired desktop, where its command routes to the hub', async () => {
    updates({ readonly: true, remote: true });
    const table = await screen.findByTestId('data-table-updates.targets');
    await waitFor(() => expect(within(table).getByText('Device 7')).toBeTruthy());
  });
});

describe('PageView — accessibility', () => {
  const mount = (id: string) =>
    render(PageView, {
      props: {
        page: pageOf(id),
        pages: bundle.pages,
        descs,
        values: defaults,
        sources: bundle.sources,
        onnavigate: vi.fn(),
      },
    });

  it('a settings page and a data page, in the New layout, are accessible', async () => {
    const settings = mount('settings.notifications');
    expect(screen.getByTestId('settings-matrix')).toBeTruthy();
    await expectAccessible(settings.container);
    settings.unmount();

    const data = mount('usage');
    const table = await screen.findByTestId('data-table-usage.by_model');
    await waitFor(() => expect(table.textContent).toContain('claude-opus-5'));
    await screen.findByTestId('data-chart-usage.by_day');
    await expectAccessible(data.container);
  });
});
