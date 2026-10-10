// Org administration phase C: an org's page shows its spend (when the hub
// sends it) and its own settings — each inherits the fleet's value or takes
// the org's own, written through `set_org_setting`; and an org over budget
// raises one Attention item. Redesign 11.1: the overview is tiles, a 14-day
// spend chart and a "Needs an admin" list.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import ResourcePage from './ResourcePage.svelte';
import { hubStatus, STANDALONE } from '../hub';
import { memberPairing, orgs, type OrgDetail } from '../orgs';
import { toasts } from '../toasts';
import { bundle, openRecordTab as openTab } from './testing';
import type { Descriptor, Page } from './pages';
import { orgBudgetItems } from '../org_budget';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;
const page = bundle.pages.find((p) => p.id === 'settings.orgs') as Page;
const resource = bundle.resources.find((r) => r.id === 'org')!;

const budget: Descriptor = {
  key: 'budget.org_daily_usd',
  label: 'Org daily budget',
  help: 'Estimated spend an org may reach in a day.',
  kind: { type: 'int', min: 0, max: 1_000_000 },
  default: '0',
  value: '20',
  modified: true,
  unit: 'usd',
  zero: 'none',
  tags: [],
  danger: { level: 'none' },
  restart: 'none',
  ai: 'suggest',
  per_org: true,
} as unknown as Descriptor;
const model: Descriptor = {
  ...budget,
  key: 'work.summary_model',
  label: 'Summary model',
  kind: { type: 'choice', options: ['haiku', 'sonnet', 'opus'] },
  default: 'haiku',
  value: 'haiku',
  unit: 'none',
  zero: undefined,
} as unknown as Descriptor;

const acme: OrgDetail = {
  id: 1,
  name: 'Acme',
  isolate_sessions: false,
  created_at: 1,
  rules: [],
  hosts: [],
  trackers: [],
  spent_today_micros: 12_340_000,
  spent_week_micros: 50_000_000,
  spent_month_micros: 90_500_000,
  budget_daily_usd: 10,
  budget_monthly_usd: 0,
  over_budget: ['daily'],
  spend_series: Array.from({ length: 14 }, (_, i) => ({ day: `2026-09-${String(25 + i).padStart(2, '0')}`, cost_micros: i * 1_000_000 })),
  needs_admin: [
    { kind: 'budget', period: 'daily', spent_micros: 12_340_000, budget_micros: 10_000_000 },
    { kind: 'untrusted_device', device: "Peter's iPhone", paired_at: Math.floor(Date.now() / 1000) - 7200 },
    { kind: 'unclaimed_sessions', host: 'mercury', count: 4 },
  ],
  settings: [{ setting: budget, own: '10' }, { setting: model }],
};

function route(list: OrgDetail[]) {
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === 'list_orgs') return list;
    if (cmd === 'set_org_setting') return [];
    if (cmd === 'set_org_member') return [];
    if (cmd === 'remove_org_member') return { removed: true, revoked_grants: 0 };
    if (cmd === 'org_member_grants') return grants;
    if (cmd === 'pair_device') return pairing;
    return null;
  });
}
let grants: { watch: number; drive: number } | null = null;
const pairing = { url: 'https://hub/pair#M4D', code: 'M4D-82P-QX7', expires_in_s: 600, name: 'cleo-device', mode: 'full', trusted: false, person: 'cleo', qr: [] };
const argsOf = (cmd: string) =>
  (invoke.mock.calls.filter((c) => c[0] === cmd).at(-1)![1] as { args: Record<string, unknown> }).args;

beforeEach(() => {
  grants = null;
  memberPairing.set(null);
  hubStatus.set({ ...STANDALONE });
  toasts.set([]);
  orgs.set([]);
});

describe('an org’s spend and its own settings', () => {
  it('shows the spend in dollars and the budgets it reached', async () => {
    route([acme]);
    render(ResourcePage, { props: { page, resource } });
    await waitFor(() => expect(screen.getByTestId('value-spent_today_micros').textContent).toBe('$12.34'));
    expect(screen.getByTestId('value-spent_month_micros').textContent).toBe('$90.50');
    expect(screen.getAllByTestId('item-needs_admin')[0].textContent).toContain('Daily budget reached');
  });

  it('shows the overview as tiles with each budget under its amount', async () => {
    route([acme]);
    render(ResourcePage, { props: { page, resource } });
    await waitFor(() => expect(screen.getByTestId('tile-spent_today_micros')).toBeTruthy());
    expect(screen.getByTestId('tile-sub-spent_today_micros').textContent).toBe('123% of $10');
    // No monthly budget: no line under the month.
    expect(screen.queryByTestId('tile-sub-spent_month_micros')).toBeNull();
    expect(screen.getByTestId('tile-session_count')).toBeTruthy();
  });

  it('charts the last 14 days of spend, today last', async () => {
    route([acme]);
    render(ResourcePage, { props: { page, resource } });
    await openTab('Spend');
    await fireEvent.click(await screen.findByTestId('chart-spend_series-table-toggle'));
    const rows = screen.getByTestId('chart-spend_series-table').querySelectorAll('tbody tr');
    expect(rows).toHaveLength(14);
    expect(rows[13].textContent).toContain('$13.00');
  });

  it('lists what needs an admin: a budget, an untrusted device, unclaimed sessions', async () => {
    route([acme]);
    render(ResourcePage, { props: { page, resource } });
    const items = await screen.findAllByTestId('item-needs_admin');
    expect(items.map((i) => i.textContent)).toEqual([
      expect.stringContaining('Daily budget reached'),
      expect.stringContaining("Peter's iPhone is not trusted yet"),
      expect.stringContaining('4 unclaimed sessions on mercury'),
    ]);
    expect(items[1].textContent).toContain('paired 2 h ago');
  });

  it('splits the spend by person, nobody’s last (11.8)', async () => {
    route([
      {
        ...acme,
        spend_by_person: [
          { person_id: 2, name: 'Martin', today_micros: 18_400_000, week_micros: 121_000_000, month_micros: 402_000_000 },
          { today_micros: 3_900_000, week_micros: 19_000_000, month_micros: 50_000_000 },
        ],
      },
    ]);
    render(ResourcePage, { props: { page, resource } });
    await openTab('Spend');
    const rows = await screen.findAllByTestId('item-spend_by_person');
    expect(rows.map((r) => r.textContent)).toEqual([
      expect.stringMatching(/Martin.*\$18\.40.*\$121\.00.*\$402\.00/),
      expect.stringMatching(/Routines, missions and unclaimed.*\$3\.90/),
    ]);
  });

  it('never shows part of the spend by person: no table without the key', async () => {
    route([acme]);
    render(ResourcePage, { props: { page, resource } });
    await waitFor(() => expect(screen.getByTestId('tile-spent_today_micros')).toBeTruthy());
    await openTab('Spend');
    expect(screen.getByTestId('record-field-spend_series')).toBeTruthy();
    expect(screen.queryByTestId('record-field-spend_by_person')).toBeNull();
  });

  it('leaves the spend and settings out when the hub does not send them', async () => {
    route([{ ...acme, spent_today_micros: undefined, spent_week_micros: undefined, spent_month_micros: undefined, settings: undefined, over_budget: undefined, spend_series: undefined, needs_admin: undefined }]);
    render(ResourcePage, { props: { page, resource } });
    await waitFor(() => expect(screen.getByTestId('record-field-name')).toBeTruthy());
    expect(screen.queryByTestId('tile-spent_today_micros')).toBeNull();
    expect(screen.queryByTestId('record-field-needs_admin')).toBeNull();
    await openTab('Spend');
    expect(screen.queryByTestId('record-field-spend_series')).toBeNull();
    await openTab('Settings');
    expect(screen.queryByTestId('record-field-settings')).toBeNull();
  });

  it('inherits a setting again, and takes one for the org, through set_org_setting', async () => {
    route([acme]);
    render(ResourcePage, { props: { page, resource } });
    await openTab('Settings');
    await fireEvent.click(await screen.findByTestId('org-setting-inherit-budget.org_daily_usd'));
    await waitFor(() => expect(argsOf('set_org_setting')).toEqual({ org_id: 1, key: 'budget.org_daily_usd', value: null }));
    expect(screen.getByTestId('org-setting-fleet-work.summary_model').textContent).toContain("(the fleet's)");
    await fireEvent.click(screen.getByTestId('org-setting-own-work.summary_model'));
    await waitFor(() => expect(argsOf('set_org_setting')).toEqual({ org_id: 1, key: 'work.summary_model', value: 'haiku' }));
  });
});

describe('an org’s members (phase D)', () => {
  it('lists who is in it with their role, adds one and removes one', async () => {
    route([
      {
        ...acme,
        members: [
          { person_id: 2, name: 'jane', role: 'admin' },
          { person_id: 3, name: 'bob', display_name: 'Bob B', role: 'member' },
        ],
      },
    ]);
    render(ResourcePage, { props: { page, resource } });
    await openTab('Members');
    await waitFor(() => expect(screen.getAllByTestId('item-members')[0].textContent).toContain('jane'));
    expect(screen.getAllByTestId('item-members')[0].textContent).toContain('admin');
    expect(screen.getAllByTestId('item-members')[1].textContent).toContain('Bob B');
    await fireEvent.input(screen.getByTestId('param-org.set_member-person'), { target: { value: 'cleo' } });
    await fireEvent.change(screen.getByTestId('param-org.set_member-role'), { target: { value: 'viewer' } });
    await fireEvent.click(screen.getByTestId('run-org.set_member'));
    await waitFor(() => expect(argsOf('set_org_member')).toEqual({ org_id: 1, person: 'cleo', role: 'viewer' }));
    await fireEvent.click(screen.getAllByTestId('item-remove-members')[1]);
    expect((await screen.findByTestId('confirm-dialog')).textContent).toContain('taken back');
    await fireEvent.click(screen.getByTestId('record-confirm'));
    await waitFor(() => expect(argsOf('remove_org_member')).toEqual({ org_id: 1, person_id: 3 }));
  });

  // Redesign 11.2, board OrgMembers.
  const joined = Math.floor(Date.parse('2026-09-12T10:00:00Z') / 1000);
  const team = {
    ...acme,
    members: [
      { person_id: 2, name: 'jane', role: 'admin', added_at: joined, shares_since: joined, devices: ['jane-mac', 'jane-phone'] },
      { person_id: 3, name: 'bob', role: 'member', added_at: joined, shares_since: joined, devices: [] },
      { person_id: 4, name: 'audit', role: 'viewer', added_at: joined, devices: ['browser'] },
    ],
  };

  it('shows since when a share reaches each member, and their devices', async () => {
    route([team]);
    render(ResourcePage, { props: { page, resource } });
    await openTab('Members');
    const since = await screen.findAllByTestId('member-shares-since');
    expect(since.map((s) => s.textContent)).toEqual(['2026-09-12', '2026-09-12', 'never (viewer)']);
    expect(screen.getAllByTestId('member-devices').map((d) => d.textContent)).toEqual(['jane-mac, jane-phone', 'none', 'browser']);
  });

  it('removing a member with 6 shares offers all three choices', async () => {
    grants = { watch: 4, drive: 2 };
    route([team]);
    render(ResourcePage, { props: { page, resource } });
    await openTab('Members');
    await fireEvent.click((await screen.findAllByTestId('item-remove-members'))[1]);
    expect((await screen.findByTestId('member-grants')).textContent).toContain('6 sessions of Acme are shared with them');
    expect(argsOf('org_member_grants')).toEqual({ org_id: 1, person_id: 3 });
    expect(screen.getByTestId('member-grants-revoke')).toBeTruthy();
    expect(screen.getByTestId('member-grants-narrow')).toBeTruthy();
    expect(screen.getByTestId('member-grants-keep')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('member-grants-narrow'));
    await fireEvent.click(screen.getByTestId('record-confirm'));
    await waitFor(() => expect(argsOf('remove_org_member')).toEqual({ org_id: 1, person_id: 3, grants: 'narrow' }));
  });

  it('takes the shares back by default', async () => {
    grants = { watch: 1, drive: 0 };
    route([team]);
    render(ResourcePage, { props: { page, resource } });
    await openTab('Members');
    await fireEvent.click((await screen.findAllByTestId('item-remove-members'))[1]);
    expect((await screen.findByTestId('member-grants')).textContent).toContain('1 session of Acme is shared with them');
    await fireEvent.click(screen.getByTestId('record-confirm'));
    await waitFor(() => expect(argsOf('remove_org_member')).toEqual({ org_id: 1, person_id: 3, grants: 'revoke' }));
  });

  it('adding a member offers a pairing code for their device', async () => {
    route([team]);
    render(ResourcePage, { props: { page, resource } });
    await openTab('Members');
    await fireEvent.input(await screen.findByTestId('param-org.set_member-person'), { target: { value: 'cleo' } });
    await fireEvent.change(screen.getByTestId('param-org.set_member-role'), { target: { value: 'member' } });
    await fireEvent.click(screen.getByTestId('run-org.set_member'));
    expect(((await screen.findByTestId('member-pair-device')) as HTMLInputElement).value).toBe('cleo-device');
    await fireEvent.click(screen.getByTestId('member-pair-mint'));
    await waitFor(() => expect(screen.getByTestId('pairing-code').textContent).toBe('M4D-82P-QX7'));
    expect(argsOf('pair_device')).toEqual({ device: 'cleo-device', mode: 'full', org_id: 1, person: 'cleo' });
    await fireEvent.click(screen.getByTestId('pairing-close'));
    expect(screen.queryByTestId('member-pairing')).toBeNull();
  });

  it('says why when no code can be minted (a standalone desktop)', async () => {
    route([team]);
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'list_orgs') return [team];
      if (cmd === 'pair_device') throw { code: 'E_INVALID_STATE', message: 'pairing codes are minted by a hub' };
      return null;
    });
    render(ResourcePage, { props: { page, resource } });
    await openTab('Members');
    await fireEvent.click(await screen.findByTestId('member-pair-3'));
    await fireEvent.click(screen.getByTestId('member-pair-mint'));
    expect((await screen.findByTestId('member-pair-error')).textContent).toContain('minted by a hub');
  });

  it('leaves the members out for someone the hub does not show them to', async () => {
    route([acme]);
    render(ResourcePage, { props: { page, resource } });
    await waitFor(() => expect(screen.getByTestId('record-field-name')).toBeTruthy());
    await openTab('Members');
    expect(screen.queryByTestId('record-field-members')).toBeNull();
  });
});

describe('the org overview boards (OrgOverview, OrgSpend)', () => {
  it('lays the org out in tabs, a list tab with its count', async () => {
    route([{ ...acme, members: [{ person_id: 2, name: 'jane', role: 'admin' }], devices: [] }]);
    render(ResourcePage, { props: { page, resource } });
    await openTab('Overview');
    const tabs = Array.from(document.querySelectorAll('[data-testid="record-tabs"] [role="tab"]')).map((t) => t.getAttribute('data-tab'));
    expect(tabs).toEqual(['Overview', 'Members', 'Devices', 'Spend', 'Settings']);
    expect(document.querySelector('[data-tab="Members"]')!.textContent).toContain('1');
    // The overview holds the tiles and the needs, not the members.
    expect(screen.getByTestId('tile-spent_today_micros')).toBeTruthy();
    expect(screen.queryByTestId('record-field-members')).toBeNull();
  });

  it('draws a budget tile’s Meter: crit once reached, warn from 80%', async () => {
    route([{ ...acme, budget_monthly_usd: 100 }]);
    render(ResourcePage, { props: { page, resource } });
    const today = await screen.findByTestId('tile-meter-spent_today_micros');
    expect(today.getAttribute('aria-valuenow')).toBe('100');
    expect(today.classList.contains('crit')).toBe(true);
    expect(today.getAttribute('aria-label')).toBe('Spent today: 123% of $10');
    const month = screen.getByTestId('tile-meter-spent_month_micros');
    expect(month.getAttribute('aria-valuenow')).toBe('91');
    expect(month.classList.contains('warn')).toBe(true);
  });

  it('opens the tab a need is acted on from', async () => {
    route([acme]);
    render(ResourcePage, { props: { page, resource } });
    const go = await screen.findAllByTestId('need-go-needs_admin');
    // The budget opens Spend, the untrusted device Devices; unclaimed sessions have none.
    expect(go.map((b) => b.textContent)).toEqual(['Spend', 'Devices']);
    await fireEvent.click(go[0]);
    expect(await screen.findByTestId('chart-spend_series')).toBeTruthy();
  });

  it('draws the daily budget across the 14-day chart and names the days that went over', async () => {
    route([
      {
        ...acme,
        spend_series: [
          { day: '2026-10-01', cost_micros: 5_000_000 },
          { day: '2026-10-02', cost_micros: 71_000_000 },
          { day: '2026-10-03', cost_micros: 10_000_000 },
        ],
      },
    ]);
    render(ResourcePage, { props: { page, resource } });
    await openTab('Spend');
    expect((await screen.findByTestId('chart-spend_series-limit')).textContent).toContain('daily budget $10');
    expect(screen.getByTestId('chart-spend_series-limit-line')).toBeTruthy();
    // Only the day above the budget, not the one that met it.
    expect(screen.getByTestId('chart-over-spend_series').textContent).toBe(
      '2 Oct went over: $71.00. Fleet warns; it never stops a session.',
    );
  });

  it('says the by-person rule beside the spend', async () => {
    route([acme]);
    render(ResourcePage, { props: { page, resource } });
    await openTab('Spend');
    expect(screen.getByText(/hidden whole, never in part/)).toBeTruthy();
  });

  it('names the org in its Jev consent (7.7)', async () => {
    route([acme]);
    render(ResourcePage, { props: { page, resource } });
    await openTab('Settings');
    expect(screen.getByTestId('record-field-jev_allowed').textContent).toContain("Allow Jev (decision model) for Acme's work");
  });
});

describe('orgBudgetItems', () => {
  it('raises one item per org and period, in dollars', () => {
    const items = orgBudgetItems([{ org_id: 1, org: 'Acme', period: 'daily', spent_micros: 31_000_000, budget_micros: 30_000_000 }]);
    expect(items).toEqual([
      {
        key: '1:daily',
        label: 'Acme over its daily budget',
        detail: '$31.00 spent today of a $30.00 budget · fleet only warns · Open Settings → Organisations',
        page: 'settings.orgs',
      },
    ]);
    expect(orgBudgetItems(undefined)).toEqual([]);
  });
});
