// Multi-hub Phase 3b: one human, a profile per fleet.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';

import { PRIMARY, _resetFleetsForTest, _setFleetsForTest, removeFleet, type FleetInfo } from './fleets';
import {
  envChecklist,
  fleetProfiles,
  loadProfile,
  profileFromParts,
  profileLabelOf,
  profileLabels,
  profileLine,
  profileLines,
  profilePerson,
  setProfileLabel,
  isOlderHub,
  _resetProfilesForTest,
  type MyProfile,
} from './fleet_profiles';
import FleetProfiles from './FleetProfiles.svelte';
import FleetSwitcher from './FleetSwitcher.svelte';
import { fleetInfo } from './fleets_fixture';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;

const primary = fleetInfo(PRIMARY, { label: 'rlt', url: 'https://fleet.rlt.sk', local: false, client_name: 'mac', connection: { state: 'connected' } });
const company: FleetInfo = { ...primary, id: 'company', label: 'Papaya POS (hub)', url: 'https://fleet.papayapos.dev', color: '#e8833a', primary: false };

const work: MyProfile = {
  person_id: 4,
  name: 'martin',
  display_name: 'Martin Janči',
  email: 'martin@papayapos.example',
  sso: [{ issuer: 'https://sso.papayapos.example/realms/company' }],
  env: [
    { name: 'GH_TOKEN', secret: true, is_ref: false },
    { name: 'GIT_AUTHOR_EMAIL', secret: false, is_ref: false },
  ],
  git: { email: 'martin@papayapos.example' },
};
const home: MyProfile = { person_id: 1, name: 'martin', is_owner: true };

beforeEach(() => {
  localStorage.clear();
  inv.mockReset();
  _resetFleetsForTest();
  _resetProfilesForTest();
  _setFleetsForTest([primary, company]);
});

describe('a profile in words', () => {
  it('names the person, the mail and nobody', () => {
    expect(profilePerson(work)).toBe('Martin Janči (martin)');
    expect(profilePerson({ person_id: 9 })).toBe('person #9');
    expect(profilePerson({ person_id: null })).toBeNull();
    expect(profileLine('Work', work)).toBe('Work · Martin Janči (martin) · martin@papayapos.example');
    expect(profileLine('Guest', { person_id: null })).toBe('Guest · no person on this hub');
  });

  it('lists the key variables set or not, then the rest, by name only', () => {
    const list = envChecklist({ ...work, env: [...work.env!, { name: 'NPM_TOKEN', secret: true, is_ref: false }] });
    expect(list.find((v) => v.name === 'GH_TOKEN')).toEqual({ name: 'GH_TOKEN', set: true, secret: true });
    expect(list.find((v) => v.name === 'JIRA_API_TOKEN')?.set).toBe(false);
    expect(list.at(-1)?.name).toBe('NPM_TOKEN');
  });

  it("a label is this desktop's, per fleet, and falls back to the fleet's", () => {
    setProfileLabel('company', '  Work  ');
    expect(get(profileLabels)).toEqual({ company: 'Work' });
    expect(profileLabelOf(company, get(profileLabels))).toBe('Work');
    expect(profileLabelOf(primary, get(profileLabels))).toBe('rlt');
    expect(JSON.parse(localStorage.getItem('cf:pref:fleets.profileLabels')!)).toEqual({ company: 'Work' });
    setProfileLabel('company', '');
    expect(get(profileLabels)).toEqual({});
  });
});

describe('reading a profile', () => {
  it("asks that fleet's own hub", async () => {
    inv.mockResolvedValue(work);
    const st = await loadProfile('company');
    expect(inv).toHaveBeenCalledWith('my_profile', { fleet: 'company' });
    expect(st).toEqual({ profile: work, source: 'hub', error: null });
    inv.mockResolvedValue(home);
    await loadProfile(PRIMARY);
    expect(inv).toHaveBeenLastCalledWith('my_profile', {});
    expect(get(fleetProfiles).get(PRIMARY)?.profile).toEqual(home);
  });

  it('pieces it together from my_grants and env_vars_list on an older hub', async () => {
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'my_profile') throw { code: 'E_HUB_PROTOCOL', message: 'unknown tool' };
      if (cmd === 'my_grants') return { person_id: 4, grants: [] };
      if (cmd === 'env_vars_list')
        return [
          { id: 1, scope: 'person', scope_key: '4', name: 'GIT_AUTHOR_NAME', value: 'Martin', secret: false, is_ref: false, updated_at: 1 },
          { id: 2, scope: 'person', scope_key: '4', name: 'GH_TOKEN', secret: true, is_ref: false, updated_at: 1 },
          { id: 3, scope: 'global', scope_key: '', name: 'SHARED', value: 'x', secret: false, is_ref: false, updated_at: 1 },
        ];
      return null;
    });
    const st = await loadProfile('company');
    expect(st.source).toBe('fallback');
    expect(st.profile).toEqual({
      person_id: 4,
      env: [
        { name: 'GIT_AUTHOR_NAME', secret: false, is_ref: false },
        { name: 'GH_TOKEN', secret: true, is_ref: false },
      ],
      git: { name: 'Martin', email: null },
    });
    for (const call of inv.mock.calls.slice(1)) expect(call[1]).toEqual({ fleet: 'company' });
  });

  it("falls back on a paired device's refusal from an older hub, and on nothing else", async () => {
    expect(isOlderHub({ code: 'E_FORBIDDEN', message: 'my_profile is not a client-callable tool (mac refused)' })).toBe(true);
    expect(isOlderHub({ code: 'E_FORBIDDEN', message: 'my_profile is a fleet-admin tool: master token only' })).toBe(false);
    expect(isOlderHub({ code: 'E_HUB_TIMEOUT', message: 'timed out' })).toBe(false);
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'my_profile') throw { code: 'E_FORBIDDEN', message: 'my_profile is not a client-callable tool (mac refused)' };
      if (cmd === 'my_grants') return { person_id: 4, grants: [] };
      if (cmd === 'env_vars_list') return [];
      return null;
    });
    const st = await loadProfile('company');
    expect(st.source).toBe('fallback');
    expect(st.error).toBeNull();
    expect(st.profile?.person_id).toBe(4);
  });

  it('a failed env_vars_list on the fallback is unknown, not unset', async () => {
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'my_profile') throw { code: 'E_HUB_PROTOCOL', message: 'unknown tool' };
      if (cmd === 'my_grants') return { person_id: 4, grants: [] };
      if (cmd === 'env_vars_list') throw { code: 'E_HUB_TIMEOUT', message: 'timed out' };
      return null;
    });
    const st = await loadProfile('company');
    expect(st.source).toBe('fallback');
    expect(st.envError).toBe('timed out');
    expect(st.profile?.env).toBeUndefined();
    render(FleetProfiles);
    await vi.waitFor(() => expect(screen.getByTestId('fleet-profile-env-company').textContent).toContain('Could not read the variables'));
    expect(screen.getByTestId('fleet-profile-env-company').textContent).not.toContain('GH_TOKEN');
  });

  it('a slow stale answer never overwrites a newer one', async () => {
    let failFirst: (e: unknown) => void = () => {};
    inv.mockImplementationOnce(() => new Promise((_, reject) => (failFirst = reject))).mockResolvedValueOnce(work);
    const first = loadProfile('company');
    await loadProfile('company');
    failFirst({ code: 'E_HUB_TIMEOUT', message: 'timed out' });
    await first;
    expect(get(fleetProfiles).get('company')).toEqual({ profile: work, source: 'hub', error: null });
  });

  it("a failed read says so in the profile's line", async () => {
    inv.mockRejectedValue({ code: 'E_HUB_TIMEOUT', message: 'timed out' });
    setProfileLabel('company', 'Work');
    await loadProfile('company');
    expect(get(profileLines).get('company')?.line).toBe('Work · could not read the profile');
  });

  it('removing a fleet forgets its label and profile, so a reused slot inherits neither', async () => {
    inv.mockResolvedValue(work);
    setProfileLabel('company', 'Work');
    setProfileLabel(PRIMARY, 'Private');
    await loadProfile('company');
    let finishLate: (v: unknown) => void = () => {};
    inv.mockImplementationOnce(() => new Promise((resolve) => (finishLate = resolve)));
    const late = loadProfile('company');
    inv.mockImplementation(async (cmd: string) => (cmd === 'fleets_list' ? [primary] : null));
    expect((await removeFleet('company')).ok).toBe(true);
    expect(get(profileLabels)).toEqual({ [PRIMARY]: 'Private' });
    expect(JSON.parse(localStorage.getItem('cf:pref:fleets.profileLabels')!)).toEqual({ [PRIMARY]: 'Private' });
    expect(get(fleetProfiles).has('company')).toBe(false);
    // A read still in flight when the fleet went does not write it back.
    finishLate(work);
    await late;
    expect(get(fleetProfiles).has('company')).toBe(false);
  });

  it('nobody on the fallback is nobody', () => {
    expect(profileFromParts(null, [])).toEqual({ person_id: null });
  });
});

describe('Settings › Fleets › Profiles', () => {
  it('shows each fleet as its own person, and renames a profile locally', async () => {
    inv.mockImplementation(async (cmd: string, args: { fleet?: string }) => {
      if (cmd === 'my_profile') return args?.fleet === 'company' ? work : home;
      return null;
    });
    render(FleetProfiles);
    await vi.waitFor(() => expect(screen.getByTestId('fleet-profile-person-company').textContent).toContain('Martin Janči'));
    expect(screen.getByTestId('fleet-profile-sso-company').textContent).toContain('sso.papayapos.example/realms/company');
    expect(screen.getByTestId('fleet-profile-env-company').textContent).toContain('GH_TOKEN ✓');
    expect(screen.getByTestId('fleet-profile-env-company').textContent).toContain('JIRA_API_TOKEN —');
    expect(screen.getByTestId('fleet-profile-person-primary').textContent).toContain('owns this hub');
    await fireEvent.click(screen.getByTestId('fleet-profile-rename-company'));
    await fireEvent.input(screen.getByTestId('fleet-profile-label-input-company'), { target: { value: 'Work' } });
    await fireEvent.click(screen.getByTestId('fleet-profile-label-save-company'));
    await tick();
    expect(screen.getByTestId('fleet-profile-label-company').textContent).toBe('Work');
    // Nothing about a label is sent anywhere.
    expect(inv.mock.calls.every(([cmd]) => cmd === 'my_profile')).toBe(true);
  });

  it('Rename… moves focus to the input, Escape cancels, and focus comes back', async () => {
    inv.mockResolvedValue(home);
    render(FleetProfiles);
    const rename = screen.getByTestId('fleet-profile-rename-company');
    expect(rename.getAttribute('aria-label')).toContain('Papaya POS (hub)');
    await fireEvent.click(rename);
    const input = screen.getByTestId('fleet-profile-label-input-company');
    expect(document.activeElement).toBe(input);
    await fireEvent.input(input, { target: { value: 'Nope' } });
    await fireEvent.keyDown(input, { key: 'Escape' });
    await tick();
    expect(screen.queryByTestId('fleet-profile-label-input-company')).toBeNull();
    expect(get(profileLabels)).toEqual({});
    expect(document.activeElement).toBe(screen.getByTestId('fleet-profile-rename-company'));
  });
});

describe('the fleet switcher', () => {
  it("carries each fleet's profile on its chip", async () => {
    inv.mockImplementation(async (cmd: string, args: { fleet?: string }) => {
      if (cmd === 'my_profile') return args?.fleet === 'company' ? work : home;
      return null;
    });
    setProfileLabel('company', 'Work');
    setProfileLabel(PRIMARY, 'Private');
    render(FleetSwitcher);
    await vi.waitFor(() => expect(get(profileLines).get('company')?.line).toContain('Martin Janči'));
    await tick();
    const chip = screen.getByTestId('fleet-filter-company');
    expect(chip.getAttribute('title')).toContain('Work · Martin Janči (martin) · martin@papayapos.example');
    expect(screen.getByTestId('fleet-profile-chip-company').textContent).toBe('Work');
    expect(screen.getByTestId('fleet-profile-chip-primary').textContent).toBe('Private');
  });

  it('re-reads only the fleet whose link came back up', async () => {
    inv.mockImplementation(async (cmd: string, args: { fleet?: string }) => {
      if (cmd === 'my_profile') return args?.fleet === 'company' ? work : home;
      return null;
    });
    render(FleetSwitcher);
    await vi.waitFor(() => expect(inv).toHaveBeenCalledTimes(2));
    inv.mockClear();
    _setFleetsForTest([primary, { ...company, connection: { state: 'reconnecting', attempt: 1, retry_in_secs: 2, reason: 'reset' } }]);
    await tick();
    expect(inv).not.toHaveBeenCalled();
    _setFleetsForTest([primary, company]);
    await tick();
    expect(inv.mock.calls).toEqual([['my_profile', { fleet: 'company' }]]);
  });
});
