// Multi-hub Phase 3b: one human, a profile per fleet.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';

import { PRIMARY, _resetFleetsForTest, _setFleetsForTest, type FleetInfo } from './fleets';
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
  _resetProfilesForTest,
  type MyProfile,
} from './fleet_profiles';
import FleetProfiles from './FleetProfiles.svelte';
import FleetSwitcher from './FleetSwitcher.svelte';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;

const primary: FleetInfo = {
  id: PRIMARY,
  label: 'rlt',
  url: 'https://fleet.rlt.sk',
  color: '#5b8def',
  primary: true,
  local: false,
  client_name: 'mac',
  connection: { state: 'connected' },
  unavailable: null,
};
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
});
