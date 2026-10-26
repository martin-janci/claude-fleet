// Settings › Environment per device and per fleet (person-scope env UI):
// a person's own variables are changeable from any of their paired devices,
// the shared scopes only from a trusted one, and with a second fleet the
// page reads and writes THAT fleet's variables, routed by `fleet`.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import EnvironmentSettings from './EnvironmentSettings.svelte';
import { hubStatus, STANDALONE, type HubStatus } from './hub';
import { _resetFleetsForTest, _setFleetsForTest, type FleetInfo } from './fleets';
import { toasts } from './toasts';
import { allDescriptors, bundle } from './pages/testing';
import type { Page } from './pages/pages';

const page = bundle.pages.find((p) => p.id === 'settings.env') as Page;

const remote: HubStatus = {
  ...STANDALONE,
  remote: true,
  url: 'https://fleet.example.com',
  client_name: 'laptop',
  configured_url: 'https://fleet.example.com',
  configured_client_name: 'laptop',
};

const personRow = { id: 1, scope: 'person', scope_key: '7', name: 'GIT_AUTHOR_NAME', value: 'Ada', secret: false, is_ref: false, updated_at: 1 };
const globalRow = { id: 2, scope: 'global', scope_key: '', name: 'DATABASE_URL', value: 'x', secret: false, is_ref: false, updated_at: 1 };

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

type Answers = Record<string, unknown | ((args: Record<string, unknown> | undefined) => unknown)>;

function route(answers: Answers) {
  const inv = mockedInvoke as ReturnType<typeof vi.fn>;
  inv.mockReset();
  inv.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
    if (cmd in answers) {
      const a = answers[cmd];
      const v = typeof a === 'function' ? (a as (x: typeof args) => unknown)(args) : a;
      if (v instanceof Error) throw v;
      return v;
    }
    if (cmd === 'env_var_set' || cmd === 'env_var_unset') return { ok: true };
    return null;
  });
  return inv;
}

function show(ownsFleet: boolean) {
  render(EnvironmentSettings, {
    props: {
      page,
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

async function scopeChoices(): Promise<string[]> {
  await fireEvent.click(await screen.findByTestId('resource-add'));
  const sel = (await screen.findByTestId('param-env_var.set-scope')) as HTMLSelectElement;
  return Array.from(sel.options).map((o) => o.value);
}

beforeEach(() => {
  hubStatus.set({ ...STANDALONE });
  _resetFleetsForTest();
  toasts.set([]);
});

describe('Settings › Environment: who may change what', () => {
  it('standalone: every scope, no read-only line', async () => {
    route({ env_vars_list: [personRow, globalRow] });
    show(true);
    expect(await scopeChoices()).toEqual(['person', 'global', 'host', 'project']);
    expect(screen.getByTestId('param-env_var.set-scope_key')).toBeInTheDocument();
    expect(screen.queryByTestId('resource-readonly')).toBeNull();
  });

  it('a person’s untrusted device: their own variables only, said in words', async () => {
    hubStatus.set(remote);
    const inv = route({
      env_vars_list: [personRow, globalRow],
      my_grants: { person_id: 7, grants: [] },
      setting_proposals: { can_write: false, proposals: [] },
    });
    show(false);
    const reason = await screen.findByTestId('resource-readonly');
    expect(reason.textContent).toContain('Your own variables');
    expect(reason.textContent).toContain('fleet-hub client trust laptop');
    expect(await scopeChoices()).toEqual(['person']);
    // The person scope's key is the caller's own: not asked.
    expect(screen.queryByTestId('param-env_var.set-scope_key')).toBeNull();
    await fireEvent.input(screen.getByTestId('param-env_var.set-name'), { target: { value: 'GH_TOKEN' } });
    await fireEvent.input(screen.getByTestId('param-env_var.set-value'), { target: { value: 'ghp_x' } });
    await fireEvent.click(screen.getByTestId('run-env_var.set'));
    await waitFor(() => expect(calls(inv, 'env_var_set')).toHaveLength(1));
    const [, sent] = calls(inv, 'env_var_set')[0] as [string, Record<string, unknown>];
    expect(sent).toEqual({ args: { scope: 'person', name: 'GH_TOKEN', value: 'ghp_x', secret: true } });
    // The primary is addressed without a `fleet`.
    expect('fleet' in sent).toBe(false);
  });

  it('removes a person row, but a shared one shows no Remove on an untrusted device', async () => {
    hubStatus.set(remote);
    route({
      env_vars_list: [personRow, globalRow],
      my_grants: { person_id: 7, grants: [] },
      setting_proposals: { can_write: false, proposals: [] },
    });
    show(false);
    await screen.findByTestId('resource-readonly');
    const rows = await screen.findAllByTestId('resource-row');
    await fireEvent.click(rows.find((r) => r.textContent?.includes('GIT_AUTHOR_NAME'))!);
    expect(await screen.findByTestId('record-delete')).toBeInTheDocument();
    await fireEvent.click(rows.find((r) => r.textContent?.includes('DATABASE_URL'))!);
    await waitFor(() => expect(screen.queryByTestId('record-delete')).toBeNull());
  });

  it('a device that speaks for no person and is not trusted: read-only, with why', async () => {
    hubStatus.set(remote);
    route({
      env_vars_list: [],
      my_grants: { person_id: null, grants: [] },
      setting_proposals: { can_write: false, proposals: [] },
    });
    show(false);
    const reason = await screen.findByTestId('resource-readonly');
    expect(reason.textContent).toContain('speaks for no person');
    expect(screen.queryByTestId('resource-add')).toBeNull();
    expect(screen.queryByTestId('env-git')).toBeNull();
  });

  it('an access read that did not get through says so and offers Try again', async () => {
    hubStatus.set(remote);
    let reachable = false;
    const inv = route({
      env_vars_list: [],
      my_grants: () => (reachable ? { person_id: 7, grants: [] } : Object.assign(new Error('down'), { code: 'E_HUB_TIMEOUT', message: 'down' })),
      setting_proposals: () => (reachable ? { can_write: true, proposals: [] } : Object.assign(new Error('down'), { code: 'E_HUB_TIMEOUT', message: 'down' })),
    });
    show(false);
    const reason = await screen.findByTestId('resource-readonly');
    expect(reason.textContent).toContain('Could not read');
    expect(reason.textContent).not.toContain('client trust');
    reachable = true;
    await fireEvent.click(await screen.findByTestId('env-access-retry'));
    await waitFor(() => expect(screen.queryByTestId('resource-readonly')).toBeNull());
    expect(calls(inv, 'my_grants').length).toBe(2);
  });

  it('a trusted person’s device: every scope again', async () => {
    hubStatus.set(remote);
    route({
      env_vars_list: [],
      my_grants: { person_id: 7, grants: [] },
      setting_proposals: { can_write: true, proposals: [] },
    });
    show(false);
    await waitFor(() => expect(calls(mockedInvoke as ReturnType<typeof vi.fn>, 'setting_proposals')).toHaveLength(1));
    expect(await scopeChoices()).toEqual(['person', 'global', 'host', 'project']);
    expect(screen.queryByTestId('resource-readonly')).toBeNull();
  });
});

describe('Settings › Environment with a second fleet', () => {
  it('shows no fleet picker with one fleet', async () => {
    route({ env_vars_list: [] });
    show(true);
    await screen.findByTestId('resource-add');
    expect(screen.queryByTestId('env-fleet')).toBeNull();
  });

  it('routes the list, the access reads and Add to the picked fleet, with its own rule', async () => {
    hubStatus.set(remote);
    _setFleetsForTest([fleet('primary', true), fleet('company', false), fleet('company-2', false, { duplicate_of: 'company' })]);
    const inv = route({
      env_vars_list: (a: Record<string, unknown> | undefined) => (a?.fleet === 'company' ? [{ ...personRow, value: 'Company Ada' }] : [personRow]),
      // Trusted on the primary, a person's plain device on company.
      my_grants: (a: Record<string, unknown> | undefined) => ({ person_id: a?.fleet === 'company' ? 3 : 7, grants: [] }),
      setting_proposals: (a: Record<string, unknown> | undefined) => ({ can_write: a?.fleet !== 'company', proposals: [] }),
    });
    show(false);
    const pick = (await screen.findByTestId('env-fleet')) as HTMLSelectElement;
    // A duplicate is never offered.
    expect(Array.from(pick.options).map((o) => o.value)).toEqual(['primary', 'company']);

    await fireEvent.change(pick, { target: { value: 'company' } });
    await waitFor(() => expect(calls(inv, 'env_vars_list').some((c) => (c[1] as { fleet?: string })?.fleet === 'company')).toBe(true));
    expect(calls(inv, 'my_grants').some((c) => (c[1] as { fleet?: string })?.fleet === 'company')).toBe(true);
    expect(calls(inv, 'setting_proposals').some((c) => (c[1] as { fleet?: string })?.fleet === 'company')).toBe(true);
    const reason = await screen.findByTestId('resource-readonly');
    expect(reason.textContent).toContain('company (https://company.example.com)');
    expect(reason.textContent).toContain('fleet-hub client trust laptop-company');

    expect(await scopeChoices()).toEqual(['person']);
    await fireEvent.input(screen.getByTestId('param-env_var.set-name'), { target: { value: 'GH_TOKEN' } });
    await fireEvent.input(screen.getByTestId('param-env_var.set-value'), { target: { value: 'ghp_c' } });
    await fireEvent.click(screen.getByTestId('run-env_var.set'));
    await waitFor(() => expect(calls(inv, 'env_var_set')).toHaveLength(1));
    expect(calls(inv, 'env_var_set')[0][1]).toEqual({
      args: { scope: 'person', name: 'GH_TOKEN', value: 'ghp_c', secret: true },
      fleet: 'company',
    });
  });
});

describe('Git & tokens', () => {
  it('sets author and committer, and a typed token as a secret, on the picked fleet', async () => {
    hubStatus.set(remote);
    _setFleetsForTest([fleet('primary', true), fleet('company', false)]);
    const inv = route({
      env_vars_list: [],
      my_grants: { person_id: 3, grants: [] },
      setting_proposals: { can_write: false, proposals: [] },
    });
    show(false);
    await fireEvent.change(await screen.findByTestId('env-fleet'), { target: { value: 'company' } });
    await fireEvent.click(await screen.findByTestId('env-git-open'));
    await fireEvent.input(screen.getByTestId('env-git-name'), { target: { value: 'Ada' } });
    await fireEvent.input(screen.getByTestId('env-git-email'), { target: { value: 'ada@example.com' } });
    await fireEvent.input(screen.getByTestId('env-git-gh'), { target: { value: 'ghp_1' } });
    await fireEvent.click(screen.getByTestId('env-git-save'));
    await waitFor(() => expect(calls(inv, 'env_var_set')).toHaveLength(5));
    const sent = calls(inv, 'env_var_set').map((c) => c[1] as { args: Record<string, unknown>; fleet?: string });
    expect(sent.map((s) => s.args.name)).toEqual([
      'GIT_AUTHOR_NAME',
      'GIT_COMMITTER_NAME',
      'GIT_AUTHOR_EMAIL',
      'GIT_COMMITTER_EMAIL',
      'GH_TOKEN',
    ]);
    expect(sent.every((s) => s.fleet === 'company' && s.args.scope === 'person')).toBe(true);
    expect(sent.find((s) => s.args.name === 'GH_TOKEN')!.args.secret).toBe(true);
    expect(sent.find((s) => s.args.name === 'GIT_AUTHOR_NAME')!.args.secret).toBe(false);
  });

  it('empties what was typed when the fleet picker moves, so a token is never Set on the other hub', async () => {
    hubStatus.set(remote);
    _setFleetsForTest([fleet('primary', true), fleet('company', false)]);
    const inv = route({
      env_vars_list: [],
      my_grants: { person_id: 3, grants: [] },
      setting_proposals: { can_write: false, proposals: [] },
    });
    show(false);
    await fireEvent.change(await screen.findByTestId('env-fleet'), { target: { value: 'company' } });
    await fireEvent.click(await screen.findByTestId('env-git-open'));
    await fireEvent.input(screen.getByTestId('env-git-gh'), { target: { value: 'ghp_company' } });
    await fireEvent.change(screen.getByTestId('env-fleet'), { target: { value: 'primary' } });
    await fireEvent.click(await screen.findByTestId('env-git-open'));
    expect((screen.getByTestId('env-git-gh') as HTMLInputElement).value).toBe('');
    expect(calls(inv, 'env_var_set')).toHaveLength(0);
  });
});
