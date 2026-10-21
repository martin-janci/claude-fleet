// What a device may change on a fleet's Company pages, read from that hub's
// own answers (`org_admin`'s authority and trust rule), and the routing every
// Company command needs to reach an extra fleet.
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import {
  NO_COMPANY,
  OWN_COMPANY,
  companyAccessFrom,
  companyReadonlyReason,
  companyRecordWritable,
  companyResourceFor,
  loadCompanyAccess,
} from './company_access';
import { FLEET_SCOPED, invokeOnFleet } from './fleets';
import { optionSourcesOf, sourceOptionsFrom, type ActionSpec, type ResourceType } from './pages/resources';
import { bundle } from './pages/testing';

const res = (id: string) => bundle.resources.find((r) => r.id === id) as ResourceType;
const device = (trusted: boolean, mode = 'full', mine = true) => ({ name: 'd', mode, trusted, created_at: 1, catalogs: [], this_device: mine });
const inv = () => mockedInvoke as ReturnType<typeof vi.fn>;

beforeEach(() => {
  inv().mockReset();
});

describe('companyAccessFrom', () => {
  it('reads the authority and the trust from the hub’s answers', () => {
    expect(companyAccessFrom(true, [device(true)])).toEqual({ admin: 'fleet', write: true });
    expect(companyAccessFrom(false, [device(true)])).toEqual({ admin: 'org', write: true });
    expect(companyAccessFrom(true, [device(false)])).toEqual({ admin: 'fleet', write: false });
    // Trust needs a full device, and the row must be this one.
    expect(companyAccessFrom(true, [device(true, 'readonly')]).write).toBe(false);
    expect(companyAccessFrom(true, [device(true, 'full', false)]).write).toBe(false);
    expect(companyAccessFrom(true, null)).toEqual(NO_COMPANY);
  });
});

describe('loadCompanyAccess', () => {
  it('asks the picked fleet, and the primary without a fleet', async () => {
    inv().mockImplementation(async (cmd: string) => (cmd === 'list_devices' ? [device(true)] : { can_write: true, proposals: [] }));
    expect(await loadCompanyAccess('company')).toEqual({ admin: 'fleet', write: true });
    expect(inv()).toHaveBeenCalledWith('list_devices', { fleet: 'company' });
    expect(inv()).toHaveBeenCalledWith('setting_proposals', { fleet: 'company' });
    inv().mockClear();
    await loadCompanyAccess('primary');
    expect(inv()).toHaveBeenCalledWith('list_devices', {});
  });

  it('an org admin’s device: setting_proposals refused, list_devices answered', async () => {
    inv().mockImplementation(async (cmd: string) => {
      if (cmd === 'list_devices') return [device(true)];
      throw Object.assign(new Error('no'), { code: 'E_FORBIDDEN', message: 'no' });
    });
    expect(await loadCompanyAccess('company')).toEqual({ admin: 'org', write: true });
  });

  it('a hub that did not answer is unknown, not "none"', async () => {
    inv().mockRejectedValue(Object.assign(new Error('down'), { code: 'E_HUB_TIMEOUT', message: 'down' }));
    const a = await loadCompanyAccess('company');
    expect(a.admin).toBeNull();
    expect(a.error).toBeTruthy();
  });
});

describe('per record and per resource', () => {
  it('an org admin changes their own org and its devices, never a person', () => {
    const orgAdmin = { admin: 'org' as const, write: true };
    expect(companyRecordWritable(orgAdmin, 'org', { id: 1, my_role: 'admin' })).toBe(true);
    expect(companyRecordWritable(orgAdmin, 'org', { id: 2, my_role: 'member' })).toBe(false);
    expect(companyRecordWritable(orgAdmin, 'device', { name: 'x' })).toBe(true);
    expect(companyRecordWritable(orgAdmin, 'person', { id: 3 })).toBe(false);
    expect(companyRecordWritable(OWN_COMPANY, 'person', { id: 3 })).toBe(true);
    expect(companyRecordWritable({ admin: 'fleet', write: false }, 'org', { id: 1 })).toBe(false);
  });

  it('Add and Remove only where the hub would not refuse them whole', () => {
    const orgAdmin = { admin: 'org' as const, write: true };
    expect(companyResourceFor(res('org'), orgAdmin).create).toBeUndefined();
    expect(companyResourceFor(res('org'), orgAdmin).delete).toBeUndefined();
    expect(companyResourceFor(res('person'), orgAdmin).create).toBeUndefined();
    expect(companyResourceFor(res('device'), orgAdmin).create).toBeDefined();
    expect(companyResourceFor(res('device'), { admin: 'org', write: false }).create).toBeUndefined();
    expect(companyResourceFor(res('org'), OWN_COMPANY)).toBe(res('org'));
  });

  it('says why in a person’s words', () => {
    expect(companyReadonlyReason(OWN_COMPANY, 'h', 'n')).toBeNull();
    expect(companyReadonlyReason({ admin: 'fleet', write: false }, 'h', 'lap')).toContain('fleet-hub client trust lap');
    expect(companyReadonlyReason(NO_COMPANY, 'h', null)).toContain('administers no organisation on h');
    expect(companyReadonlyReason({ admin: 'org', write: true }, 'h', null)).toContain('As an org admin on h');
    expect(companyReadonlyReason({ admin: null, write: false, error: 'down' }, 'h', null)).toContain('Could not read');
  });
});

describe('routing the Company pages', () => {
  it('every command the Company resources list, run or preview is fleet-scoped', () => {
    for (const id of ['org', 'device', 'person', 'group']) {
      const r = res(id);
      const acts: ActionSpec[] = [r.create, r.update, r.delete, ...(r.actions ?? [])].filter((a): a is ActionSpec => !!a);
      for (const f of r.fields) {
        if (f.type === 'items') acts.push(...[f.remove, ...f.add, ...(f.each ?? [])].filter((a): a is ActionSpec => !!a));
        if (f.type === 'settings') acts.push(f.set);
      }
      const names = [r.list, ...acts.map((a) => a.command), ...acts.flatMap((a) => (a.preview ? [a.preview] : []))];
      for (const n of names) expect(FLEET_SCOPED as readonly string[], `${id}: ${n}`).toContain(n);
      // …and the lists their option selects read.
      for (const s of optionSourcesOf(r))
        expect(FLEET_SCOPED as readonly string[]).toContain(
          { hosts: 'list_hosts', trackers: 'list_trackers', orgs: 'list_orgs', devices: 'list_devices', catalogs: 'catalog_list_catalogs', people: 'list_people' }[s],
        );
    }
  });

  it('invokeOnFleet sends `fleet` to an extra fleet, nothing to the primary, and refuses a command that is not fleet-scoped', async () => {
    inv().mockResolvedValue([]);
    await invokeOnFleet('list_people', {}, 'company');
    expect(inv()).toHaveBeenLastCalledWith('list_people', { fleet: 'company' });
    await invokeOnFleet('list_people', {}, null);
    expect(inv()).toHaveBeenLastCalledWith('list_people', undefined);
    await invokeOnFleet('update_device', { args: { device: 'd' } }, 'primary');
    expect(inv()).toHaveBeenLastCalledWith('update_device', { args: { device: 'd' } });
    inv().mockClear();
    const r = await invokeOnFleet('org_suggestions', {}, 'company');
    expect(r.ok).toBe(false);
    expect(inv()).not.toHaveBeenCalled();
  });

  it('option rows map the same way from a store or a hub’s list', () => {
    expect(sourceOptionsFrom('hosts', [{ alias: 'a' }])).toEqual([{ value: 'a', label: 'a' }]);
    expect(sourceOptionsFrom('trackers', [{ id: 5, name: 'Jira' }])).toEqual([{ value: '5', label: 'Jira' }]);
    expect(
      sourceOptionsFrom('people', [
        { name: 'ada', display_name: 'Ada L' },
        { name: 'bob', disabled_at: 3 },
      ]),
    ).toEqual([{ value: 'ada', label: 'Ada L (ada)' }]);
    expect(optionSourcesOf(res('device'))).toEqual(expect.arrayContaining(['orgs', 'people', 'catalogs']));
  });
});
