// Settings › Company per device and per fleet: Organisations, Devices,
// People and Groups read and change the hub the Fleet picker names, and what
// this device may change there is that hub's `org_admin` rule — a fleet
// admin's unbound device or an org admin's, trusted and full to change
// anything.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import CompanySettings from './CompanySettings.svelte';
import { hubStatus, STANDALONE, type HubStatus } from './hub';
import { _resetFleetsForTest, _setFleetsForTest, type FleetInfo } from './fleets';
import { hosts, type HostRow } from './hosts';
import { orgs } from './orgs';
import { toasts } from './toasts';
import { allDescriptors, bundle } from './pages/testing';
import type { Page } from './pages/pages';

const pageOf = (id: string) => bundle.pages.find((p) => p.id === id) as Page;

const remote: HubStatus = {
  ...STANDALONE,
  remote: true,
  url: 'https://fleet.example.com',
  client_name: 'laptop',
  configured_url: 'https://fleet.example.com',
  configured_client_name: 'laptop',
};

const fleet = (id: string, primary: boolean, extra: Partial<FleetInfo> = {}): FleetInfo => ({
  id,
  label: primary ? 'home' : id,
  url: primary ? 'https://fleet.example.com' : `https://${id}.example.com`,
  color: '#5b8def',
  primary,
  local: false,
  client_name: primary ? 'laptop' : `laptop-${id}`,
  connection: { state: 'connected' } as FleetInfo['connection'],
  unavailable: null,
  ...extra,
});

const homeOrg = { id: 1, name: 'Home Co', created_at: 1, rules: [], hosts: [], trackers: [] };
const papaya = { id: 1, name: 'PapayaPOS', created_at: 1, rules: [], hosts: [], trackers: [], my_role: 'admin' };
const other = { id: 2, name: 'Other Co', created_at: 1, rules: [], hosts: [], trackers: [] };
const forbidden = () => Object.assign(new Error('no'), { code: 'E_FORBIDDEN', message: 'no' });
const me = (trusted: boolean) => ({ name: 'laptop-company', mode: 'full', trusted, created_at: 1, catalogs: [], this_device: true });

type Args = Record<string, unknown> | undefined;
type Answers = Record<string, unknown | ((args: Args) => unknown)>;

function route(answers: Answers) {
  const inv = mockedInvoke as ReturnType<typeof vi.fn>;
  inv.mockReset();
  inv.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
    if (cmd in answers) {
      const a = answers[cmd];
      const v = typeof a === 'function' ? (a as (x: Args) => unknown)(args) : a;
      if (v instanceof Error) throw v;
      return v;
    }
    return null;
  });
  return inv;
}

/** The company hub's answers: an org admin of PapayaPOS there (no fleet
 *  admin, so `setting_proposals` refuses), trusted or not; the primary's
 *  answers otherwise. */
function twoHubs(trustedOnCompany: boolean): Answers {
  const on = (a: Args) => a?.fleet === 'company';
  return {
    list_orgs: (a: Args) => (on(a) ? [papaya, other] : [homeOrg]),
    setting_proposals: (a: Args) => (on(a) ? forbidden() : { can_write: true, proposals: [] }),
    list_devices: (a: Args) => (on(a) ? [me(trustedOnCompany)] : [{ ...me(true), name: 'laptop' }]),
    list_hosts: (a: Args) => (on(a) ? [{ alias: 'papaya-1' }, { alias: 'papaya-2' }] : []),
    list_trackers: [],
    update_org: (a: Args) => ({ ...papaya, ...((a?.args as object) ?? {}) }),
  };
}

function show(pageId: string, ownsFleet: boolean) {
  render(CompanySettings, {
    props: {
      page: pageOf(pageId),
      pages: bundle.pages,
      descs: new Map(allDescriptors.map((d) => [d.key, d])),
      values: {},
      sources: bundle.sources,
      resources: bundle.resources,
      actions: bundle.actions,
      ownsFleet,
      onnavigate: () => {},
    },
  });
}

const calls = (inv: ReturnType<typeof vi.fn>, cmd: string) => inv.mock.calls.filter((c) => c[0] === cmd);
const onCompany = (inv: ReturnType<typeof vi.fn>, cmd: string) =>
  calls(inv, cmd).filter((c) => (c[1] as { fleet?: string } | undefined)?.fleet === 'company');

async function pickCompany() {
  const pick = (await screen.findByTestId('company-fleet')) as HTMLSelectElement;
  await fireEvent.change(pick, { target: { value: 'company' } });
}

beforeEach(() => {
  hubStatus.set({ ...STANDALONE });
  _resetFleetsForTest();
  orgs.set([]);
  hosts.set([]);
  toasts.set([]);
});

describe('Settings › Company with one fleet', () => {
  it('standalone: no picker, no read-only line, every org editable and addable', async () => {
    route({ list_orgs: [homeOrg] });
    show('settings.orgs', true);
    expect(await screen.findByTestId('edit-name')).toBeInTheDocument();
    expect(screen.getByTestId('resource-add')).toBeInTheDocument();
    expect(screen.queryByTestId('company-fleet')).toBeNull();
    expect(screen.queryByTestId('resource-readonly')).toBeNull();
  });

  it('a paired desktop the hub does not trust: read, with the operator’s one command', async () => {
    hubStatus.set(remote);
    route({
      list_orgs: [homeOrg],
      setting_proposals: { can_write: false, proposals: [] },
      list_devices: [{ ...me(false), name: 'laptop' }],
    });
    show('settings.orgs', false);
    const reason = await screen.findByTestId('resource-readonly');
    expect(reason.textContent).toContain('fleet-hub client trust laptop');
    expect(screen.queryByTestId('edit-name')).toBeNull();
    expect(screen.queryByTestId('resource-add')).toBeNull();
  });

  it('a paired fleet admin’s trusted device changes the company as a standalone one does', async () => {
    hubStatus.set(remote);
    const inv = route({
      list_orgs: [homeOrg],
      setting_proposals: { can_write: true, proposals: [] },
      list_devices: [{ ...me(true), name: 'laptop' }],
    });
    show('settings.orgs', false);
    await waitFor(() => expect(calls(inv, 'list_devices').length).toBeGreaterThan(0));
    expect(await screen.findByTestId('edit-name')).toBeInTheDocument();
    expect(screen.getByTestId('resource-add')).toBeInTheDocument();
    expect(screen.queryByTestId('resource-readonly')).toBeNull();
  });
});

describe('Settings › Company with a second fleet', () => {
  it('offers every fleet but a duplicate, the primary first', async () => {
    hubStatus.set(remote);
    _setFleetsForTest([fleet('primary', true), fleet('company', false), fleet('company-2', false, { duplicate_of: 'company' })]);
    route(twoHubs(true));
    show('settings.orgs', false);
    const pick = (await screen.findByTestId('company-fleet')) as HTMLSelectElement;
    expect(Array.from(pick.options).map((o) => o.value)).toEqual(['primary', 'company']);
  });

  it('an org admin there: reads that hub, changes its own org there and nothing on the primary', async () => {
    hubStatus.set(remote);
    _setFleetsForTest([fleet('primary', true), fleet('company', false)]);
    const inv = route(twoHubs(true));
    show('settings.orgs', false);
    await pickCompany();
    await waitFor(() => expect(onCompany(inv, 'list_orgs').length).toBeGreaterThan(0));
    // The access is that hub's answer, asked of that hub.
    expect(onCompany(inv, 'setting_proposals').length).toBeGreaterThan(0);
    expect(onCompany(inv, 'list_devices').length).toBeGreaterThan(0);
    const reason = await screen.findByTestId('resource-readonly');
    expect(reason.textContent).toContain('As an org admin on company (https://company.example.com)');
    // Adding an org is a fleet admin's: no Add.
    expect(screen.queryByTestId('resource-add')).toBeNull();
    const rows = await screen.findAllByTestId('resource-row');
    expect(rows.map((r) => r.textContent?.trim())).toEqual([expect.stringContaining('PapayaPOS'), expect.stringContaining('Other Co')]);

    // Their own org: editable, and Apply goes to that hub.
    const name = (await screen.findByTestId('edit-name')) as HTMLInputElement;
    await fireEvent.input(name, { target: { value: 'PapayaPOS s.r.o.' } });
    await fireEvent.click(screen.getByTestId('record-apply'));
    await waitFor(() => expect(calls(inv, 'update_org')).toHaveLength(1));
    expect(calls(inv, 'update_org')[0][1]).toEqual({ args: { org_id: 1, name: 'PapayaPOS s.r.o.' }, fleet: 'company' });

    // Another org on that hub: shown, not changeable.
    await fireEvent.click((await screen.findAllByTestId('resource-row'))[1]);
    await waitFor(() => expect(screen.queryByTestId('edit-name')).toBeNull());
  });

  it('the option selects offer that hub’s hosts, never the primary’s', async () => {
    hubStatus.set(remote);
    _setFleetsForTest([fleet('primary', true), fleet('company', false)]);
    hosts.set([{ alias: 'home-mac' } as HostRow]);
    const inv = route({ ...twoHubs(true), setting_proposals: { can_write: true, proposals: [] } });
    show('settings.orgs', false);
    await pickCompany();
    await waitFor(() => expect(onCompany(inv, 'list_hosts').length).toBeGreaterThan(0));
    await waitFor(async () => {
      const sel = (await screen.findByTestId('param-org.assign_host-host')) as HTMLSelectElement;
      expect(Array.from(sel.options, (o) => o.value)).toEqual(['', 'papaya-1', 'papaya-2']);
    });
  });

  it('an untrusted device there: read-only, naming that hub’s trust command', async () => {
    hubStatus.set(remote);
    _setFleetsForTest([fleet('primary', true), fleet('company', false)]);
    route(twoHubs(false));
    show('settings.orgs', false);
    await pickCompany();
    await waitFor(async () =>
      expect((await screen.findByTestId('resource-readonly')).textContent).toContain('fleet-hub client trust laptop-company'),
    );
    expect(screen.queryByTestId('edit-name')).toBeNull();
  });

  it('People on that hub: listed from it, names and disabling stay a fleet admin’s', async () => {
    hubStatus.set(remote);
    _setFleetsForTest([fleet('primary', true), fleet('company', false)]);
    const ada = { id: 4, name: 'ada', owner: false, created_at: 1, devices: [], groups: [], roles: [], roles_display: [] };
    const inv = route({ ...twoHubs(true), list_people: (a: Args) => (a?.fleet === 'company' ? [ada] : []) });
    show('settings.people', false);
    await pickCompany();
    await waitFor(() => expect(onCompany(inv, 'list_people').length).toBeGreaterThan(0));
    expect((await screen.findAllByTestId('resource-row'))[0].textContent).toContain('ada');
    expect(screen.queryByTestId('resource-add')).toBeNull();
    expect(screen.queryByTestId('edit-name')).toBeNull();
  });

  it('a device that administers nothing there says so', async () => {
    hubStatus.set(remote);
    _setFleetsForTest([fleet('primary', true), fleet('company', false)]);
    route({ ...twoHubs(true), list_devices: (a: Args) => (a?.fleet === 'company' ? forbidden() : []) });
    show('settings.orgs', false);
    await pickCompany();
    await waitFor(async () =>
      expect((await screen.findByTestId('resource-readonly')).textContent).toContain('administers no organisation on company'),
    );
  });
});
