// Org administration phase B: Settings → Devices and Settings → People are
// master_detail pages over the `device` and `person` resources, and the org
// page binds and unbinds devices. Every change runs a command that routes to
// the hub's `org_admin`.
import { render, screen, fireEvent, waitFor, within } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import ResourcePage from './ResourcePage.svelte';
import { hubStatus, STANDALONE } from '../hub';
import { orgs, type OrgDetail } from '../orgs';
import { devices, people, qrRects, type DeviceSummary } from '../devices';
import { catalogStatuses } from '../assets_workspace';
import { toasts } from '../toasts';
import { bundle, openRecordTab } from './testing';
import type { Page } from './pages';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;
const pageOf = (id: string) => bundle.pages.find((p) => p.id === id) as Page;
const resourceOf = (id: string) => bundle.resources.find((r) => r.id === id)!;

const phone: DeviceSummary = {
  name: 'ada-phone',
  mode: 'readonly',
  trusted: false,
  person: 'ada',
  person_id: 2,
  created_at: 1,
  catalogs: ['personal'],
};
const laptop: DeviceSummary = { name: 'laptop', mode: 'full', trusted: true, created_at: 1, catalogs: [], this_device: true };
const acme = { id: 1, name: 'Acme', rules: [], hosts: [], trackers: [], devices: [] } as unknown as OrgDetail;

const PAIRING = {
  url: 'https://hub.example.com/pair#abc123',
  code: 'abc123',
  expires_in_s: 600,
  name: 'new-phone',
  mode: 'full',
  trusted: false,
  person: 'owner',
  qr: ['1110', '1000', '0001', '0111'],
};

function route(extra: Record<string, unknown> = {}) {
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd in extra) return extra[cmd];
    if (cmd === 'list_devices') return [laptop, phone];
    if (cmd === 'list_orgs') return [acme];
    if (cmd === 'list_people')
      return [
        { id: 1, name: 'owner', owner: true, created_at: 1, devices: ['laptop'] },
        { id: 2, name: 'ada', owner: false, created_at: 2, devices: ['ada-phone'] },
      ];
    if (cmd === 'catalog_list_catalogs') return [{ name: 'personal' }, { name: 'acme' }];
    if (cmd === 'pair_device') return PAIRING;
    return null;
  });
}

const argsOf = (cmd: string) =>
  (invoke.mock.calls.filter((c) => c[0] === cmd).at(-1)![1] as { args: Record<string, unknown> }).args;

beforeEach(() => {
  hubStatus.set({ ...STANDALONE });
  toasts.set([]);
  orgs.set([acme]);
  devices.set([laptop, phone]);
  people.set([
    { id: 1, name: 'owner', owner: true, created_at: 1, devices: ['laptop'] },
    { id: 2, name: 'ada', owner: false, created_at: 2, devices: ['ada-phone'] },
  ]);
  catalogStatuses.set([{ name: 'personal' }, { name: 'acme' }] as never);
  route();
});

describe('Settings → Devices', () => {
  const show = () => render(ResourcePage, { props: { page: pageOf('settings.devices'), resource: resourceOf('device') } });

  it('lists the devices with their badges', async () => {
    show();
    await waitFor(() => expect(screen.getAllByTestId('resource-row')).toHaveLength(2));
    const [me, other] = screen.getAllByTestId('resource-row');
    expect(within(me).getAllByTestId('resource-badge').map((b) => b.textContent)).toEqual(['Full', 'this device', 'trusted']);
    expect(within(other).getAllByTestId('resource-badge').map((b) => b.textContent)).toEqual(['Watch only']);
  });

  it('M15 G4.7: one People & devices table, filtered by org and person, grouped by person; a row opens it', async () => {
    const tablet: DeviceSummary = { name: 'ada-tablet', mode: 'answer', trusted: true, person: 'ada', org: 'Acme', created_at: 1, catalogs: [] };
    route({ list_devices: [laptop, phone, tablet] });
    devices.set([laptop, phone, tablet]);
    show();
    const table = await screen.findByTestId('resource-table');
    await waitFor(() => expect(within(table).getAllByTestId('table-row')).toHaveLength(3));
    expect(Array.from(table.querySelectorAll('th')).map((t) => t.textContent)).toEqual([
      'Belongs to',
      'Name',
      'Org',
      'Mode',
      'Trusted',
      'Last seen',
    ]);
    const cells = (r: HTMLElement) => Array.from(r.querySelectorAll('td')).map((c) => c.textContent?.trim());
    expect(cells(within(table).getAllByTestId('table-row')[2])).toEqual(['ada', 'ada-tablet', 'Acme', 'Answer only', 'Yes', 'never']);

    await fireEvent.change(within(table).getByTestId('table-filter-org'), { target: { value: 'Acme' } });
    expect(within(table).getAllByTestId('table-row')).toHaveLength(1);
    await fireEvent.change(within(table).getByTestId('table-filter-org'), { target: { value: '' } });
    await fireEvent.change(within(table).getByTestId('table-filter-person'), { target: { value: 'ada' } });
    expect(within(table).getAllByTestId('table-row')).toHaveLength(2);
    await fireEvent.change(within(table).getByTestId('table-filter-person'), { target: { value: '' } });

    await fireEvent.click(within(table).getByTestId('table-group'));
    expect(within(table).getAllByTestId('table-group-head').map((h) => h.textContent)).toEqual(['ada · 2', '— · 1']);

    await fireEvent.click(within(table).getByTestId('table-group'));
    await fireEvent.click(within(table).getAllByTestId('table-row')[1]);
    expect(await screen.findByTestId('record-device-ada-phone')).toBeTruthy();
  });

  it('M15 G7.14: picks its org and its person in the edit form, and grants and takes back a catalog', async () => {
    show();
    await fireEvent.click((await screen.findAllByTestId('resource-row'))[1]);
    expect(screen.queryByTestId('record-action-device.bind')).toBeNull();
    const org = screen.getByTestId('edit-org') as HTMLSelectElement;
    expect(Array.from(org.options).map((o) => [o.value, o.textContent])).toEqual([
      ['', 'No org — every org'],
      ['Acme', 'Acme'],
    ]);
    const person = screen.getByTestId('edit-person') as HTMLSelectElement;
    expect(person.value).toBe('ada');
    expect(Array.from(person.options).map((o) => o.value)).toEqual(['owner', 'ada']);
    await fireEvent.change(org, { target: { value: 'Acme' } });
    await fireEvent.change(person, { target: { value: 'owner' } });
    await fireEvent.click(screen.getByTestId('record-apply'));
    await fireEvent.click(screen.getByTestId('record-confirm'));
    await waitFor(() => expect(argsOf('update_device')).toEqual({ device: 'ada-phone', org: 'Acme', person: 'owner' }));

    // The catalog it holds is left out of the grant select.
    const grant = screen.getByTestId('param-device.grant_catalog-catalog') as HTMLSelectElement;
    expect(Array.from(grant.options).map((o) => o.value)).toEqual(['', 'acme']);
    await fireEvent.change(grant, { target: { value: 'acme' } });
    await fireEvent.click(screen.getByTestId('run-device.grant_catalog'));
    await waitFor(() => expect(argsOf('grant_device_catalog')).toEqual({ device: 'ada-phone', catalog: 'acme', on: true }));
    await fireEvent.click(screen.getByTestId('item-remove-catalogs'));
    await waitFor(() => expect(argsOf('grant_device_catalog')).toEqual({ device: 'ada-phone', catalog: 'personal', on: false }));
  });

  it('trusts a device after asking, and revokes one after asking', async () => {
    show();
    await fireEvent.click((await screen.findAllByTestId('resource-row'))[1]);
    await fireEvent.click(screen.getByTestId('edit-trusted'));
    await fireEvent.click(screen.getByTestId('record-apply'));
    await fireEvent.click(screen.getByTestId('record-confirm'));
    await waitFor(() => expect(argsOf('update_device')).toEqual({ device: 'ada-phone', trusted: true }));
    await fireEvent.click(screen.getByTestId('record-delete'));
    await fireEvent.click(screen.getByTestId('record-confirm'));
    await waitFor(() => expect(argsOf('revoke_device')).toEqual({ device: 'ada-phone' }));
  });
});

describe('Settings → Devices: what each one runs (M15 G7.14)', () => {
  it('says the app under the name in the table, and its kind and app in the record', async () => {
    const pixel: DeviceSummary = { ...phone, kind: 'phone', app: 'phone · fleet-mobile 0.5.4' };
    route({ list_devices: [laptop, pixel] });
    render(ResourcePage, { props: { page: pageOf('settings.devices'), resource: resourceOf('device') } });
    const table = await screen.findByTestId('resource-table');
    await waitFor(() => expect(within(table).getAllByTestId('table-subtitle').map((s) => s.textContent)).toEqual(['phone · fleet-mobile 0.5.4']));
    await fireEvent.click(within(table).getAllByTestId('table-row')[1]);
    expect(screen.getByTestId('value-kind').textContent).toBe('Phone');
    expect(screen.getByTestId('value-app').textContent).toBe('phone · fleet-mobile 0.5.4');
  });
});

describe('Settings → Devices: rename and mode (11.3)', () => {
  const show = () => render(ResourcePage, { props: { page: pageOf('settings.devices'), resource: resourceOf('device') } });

  it('renames a device and changes its mode in one Apply, sending only what changed', async () => {
    show();
    await fireEvent.click((await screen.findAllByTestId('resource-row'))[1]);
    await fireEvent.input(screen.getByTestId('edit-name'), { target: { value: "Ada's Pixel" } });
    await fireEvent.click(screen.getByTestId('edit-mode-full'));
    await fireEvent.click(screen.getByTestId('record-apply'));
    // Addressed by the name it has now; the new one rides `name`.
    await waitFor(() =>
      expect(argsOf('update_device')).toEqual({ device: 'ada-phone', name: "Ada's Pixel", mode: 'full' }),
    );
  });

  it('offers both modes, with the current one checked', async () => {
    show();
    await fireEvent.click((await screen.findAllByTestId('resource-row'))[1]);
    expect((screen.getByTestId('edit-mode-readonly') as HTMLInputElement).checked).toBe(true);
    expect((screen.getByTestId('edit-mode-full') as HTMLInputElement).checked).toBe(false);
  });
});

describe('Settings → Devices: Pair a device is a wizard (10.12)', () => {
  const show = () => render(ResourcePage, { props: { page: pageOf('settings.devices'), resource: resourceOf('device') } });

  it('opens the pair_device wizard, offers the orgs, and shows the code it mints', async () => {
    show();
    await fireEvent.click(screen.getByTestId('resource-add'));
    expect(screen.getByTestId('wizard-pair_device')).toBeInTheDocument();
    // Not the inline create form.
    expect(screen.queryByTestId('resource-create')).toBeNull();
    expect(screen.getByTestId('form-field-org-1').textContent).toContain('Acme');
    await fireEvent.input(screen.getByTestId('form-field-device'), { target: { value: 'new-phone' } });
    await fireEvent.click(screen.getByTestId('form-field-org-1'));
    await fireEvent.click(screen.getByTestId('form-submit'));
    await waitFor(() => expect(argsOf('pair_device')).toEqual({ device: 'new-phone', mode: 'full', org_id: 1, person: null }));
    await waitFor(() => expect(screen.queryByTestId('wizard-pair_device')).toBeNull());
    expect(screen.getByTestId('pairing-code').textContent).toBe('abc123');
    expect(screen.getByTestId('pairing-url').textContent).toBe(PAIRING.url);
    // 11.12: a Halo round the code while it waits for the device.
    expect(screen.getByTestId('pairing-halo').querySelector('[data-loader="halo"]')).toBeTruthy();
    expect(screen.getByTestId('pairing-left').textContent).toBe('10:00');
    expect(screen.getByTestId('pairing-qr').querySelectorAll('rect').length).toBe(1 + qrRects(PAIRING.qr).length);
    await fireEvent.click(screen.getByTestId('pairing-close'));
    expect(screen.queryByTestId('pairing-result')).toBeNull();
  });

  it('keeps the wizard open with the refusal when pairing fails', async () => {
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'pair_device') throw { code: 'E_FORBIDDEN', message: 'Only the owner pairs devices.' };
      if (cmd === 'list_devices') return [laptop, phone];
      return null;
    });
    show();
    await fireEvent.click(screen.getByTestId('resource-add'));
    await fireEvent.input(screen.getByTestId('form-field-device'), { target: { value: 'x' } });
    await fireEvent.click(screen.getByTestId('form-submit'));
    expect((await screen.findByTestId('wizard-error')).textContent?.trim()).toBe('Only the owner pairs devices.');
    expect(screen.getByTestId('wizard-pair_device')).toBeInTheDocument();
    expect(screen.queryByTestId('pairing-result')).toBeNull();
  });
});

describe('Settings → People', () => {
  const show = () => render(ResourcePage, { props: { page: pageOf('settings.people'), resource: resourceOf('person') } });

  it('M15 G7.14: + Person adds someone before any device is paired to them', async () => {
    show();
    await fireEvent.click(await screen.findByTestId('resource-add'));
    await fireEvent.input(screen.getByTestId('param-person.add-name'), { target: { value: 'jana' } });
    await fireEvent.input(screen.getByTestId('param-person.add-display_name'), { target: { value: 'Jana N.' } });
    await fireEvent.click(screen.getByTestId('run-person.add'));
    await waitFor(() => expect(argsOf('add_person')).toEqual({ name: 'jana', display_name: 'Jana N.' }));
  });

  it('renames a person and disables one after asking', async () => {
    show();
    await waitFor(() => expect(screen.getAllByTestId('resource-row')).toHaveLength(2));
    expect(within(screen.getAllByTestId('resource-row')[0]).getByTestId('resource-badge').textContent).toBe('owner');
    await fireEvent.click(screen.getAllByTestId('resource-row')[1]);
    expect(screen.getByTestId('item-devices').textContent).toContain('ada-phone');
    await fireEvent.input(screen.getByTestId('edit-display_name'), { target: { value: 'Ada L.' } });
    await fireEvent.click(screen.getByTestId('record-apply'));
    await waitFor(() => expect(argsOf('rename_person')).toEqual({ person_id: 2, display_name: 'Ada L.' }));
    await fireEvent.click(screen.getByTestId('record-delete'));
    await fireEvent.click(screen.getByTestId('record-confirm'));
    await waitFor(() => expect(argsOf('disable_person')).toEqual({ person_id: 2 }));
  });
});

describe('the org page binds and unbinds devices', () => {
  it('binds a device picked from the devices, and unbinds one', async () => {
    route({ list_orgs: [{ ...acme, devices: [{ name: 'ada-phone', mode: 'readonly', trusted: false }] }] });
    render(ResourcePage, { props: { page: pageOf('settings.orgs'), resource: resourceOf('org') } });
    await openRecordTab('Devices');
    const select = (await screen.findByTestId('param-org.bind_device-device')) as HTMLSelectElement;
    // The device already bound is left out.
    expect(Array.from(select.options).map((o) => o.value)).toEqual(['', 'laptop']);
    await fireEvent.change(select, { target: { value: 'laptop' } });
    await fireEvent.click(screen.getByTestId('run-org.bind_device'));
    await waitFor(() => expect(argsOf('bind_device_org')).toEqual({ device: 'laptop', org_id: 1 }));
    await fireEvent.click(screen.getByTestId('item-remove-devices'));
    await waitFor(() => expect(argsOf('bind_device_org')).toEqual({ device: 'ada-phone' }));
  });
});

describe('qrRects', () => {
  it('draws one rect per dark run, inside a 4-module quiet zone', () => {
    expect(qrRects(['1101', '0010'])).toEqual([
      { x: 4, y: 4, w: 2 },
      { x: 7, y: 4, w: 1 },
      { x: 6, y: 5, w: 1 },
    ]);
  });
});
