// Org administration phase C: an org's page shows its spend (when the hub
// sends it) and its own settings — each inherits the fleet's value or takes
// the org's own, written through `set_org_setting`; and an org over budget
// raises one Attention item.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import ResourcePage from './ResourcePage.svelte';
import { hubStatus, STANDALONE } from '../hub';
import { orgs, type OrgDetail } from '../orgs';
import { toasts } from '../toasts';
import { bundle } from './testing';
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
  over_budget: ['daily'],
  settings: [{ setting: budget, own: '10' }, { setting: model }],
};

function route(list: OrgDetail[]) {
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === 'list_orgs') return list;
    if (cmd === 'set_org_setting') return [];
    if (cmd === 'set_org_member') return [];
    if (cmd === 'remove_org_member') return { removed: true, revoked_grants: 0 };
    return null;
  });
}
const argsOf = (cmd: string) =>
  (invoke.mock.calls.filter((c) => c[0] === cmd).at(-1)![1] as { args: Record<string, unknown> }).args;

beforeEach(() => {
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
    expect(screen.getByTestId('item-over_budget').textContent).toContain('daily');
  });

  it('leaves the spend and settings out when the hub does not send them', async () => {
    route([{ ...acme, spent_today_micros: undefined, spent_week_micros: undefined, spent_month_micros: undefined, settings: undefined, over_budget: undefined }]);
    render(ResourcePage, { props: { page, resource } });
    await waitFor(() => expect(screen.getByTestId('record-field-name')).toBeTruthy());
    expect(screen.queryByTestId('record-field-spent_today_micros')).toBeNull();
    expect(screen.queryByTestId('record-field-settings')).toBeNull();
  });

  it('inherits a setting again, and takes one for the org, through set_org_setting', async () => {
    route([acme]);
    render(ResourcePage, { props: { page, resource } });
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
    await waitFor(() => expect(screen.getByTestId('record-field-members').textContent).toContain('jane · admin'));
    expect(screen.getByTestId('record-field-members').textContent).toContain('Bob B · member');
    await fireEvent.input(screen.getByTestId('param-org.set_member-person'), { target: { value: 'cleo' } });
    await fireEvent.change(screen.getByTestId('param-org.set_member-role'), { target: { value: 'viewer' } });
    await fireEvent.click(screen.getByTestId('run-org.set_member'));
    await waitFor(() => expect(argsOf('set_org_member')).toEqual({ org_id: 1, person: 'cleo', role: 'viewer' }));
    await fireEvent.click(screen.getAllByTestId('item-remove-members')[1]);
    expect((await screen.findByTestId('confirm-dialog')).textContent).toContain('taken back');
    await fireEvent.click(screen.getByTestId('record-confirm'));
    await waitFor(() => expect(argsOf('remove_org_member')).toEqual({ org_id: 1, person_id: 3 }));
  });

  it('leaves the members out for someone the hub does not show them to', async () => {
    route([acme]);
    render(ResourcePage, { props: { page, resource } });
    await waitFor(() => expect(screen.getByTestId('record-field-name')).toBeTruthy());
    expect(screen.queryByTestId('record-field-members')).toBeNull();
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
