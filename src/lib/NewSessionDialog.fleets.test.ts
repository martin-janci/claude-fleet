// Multi-hub Phase 2: New session can start on an extra fleet, by that
// fleet's own project id (cloning the repository there first when it has
// none) and hosts, and opens the
// new session in its own view rather than selecting it as one of this
// window's.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import NewSessionDialog from './NewSessionDialog.svelte';
import { hosts } from './hosts';
import { hubStatus, STANDALONE } from './hub';
import { hubConnection } from './hub_connection';
import {
  PRIMARY,
  emptyRows,
  fleetSelection,
  _resetFleetsForTest,
  _setFleetRowsForTest,
  _setFleetsForTest,
  type FleetInfo,
} from './fleets';
import { fleetInfo } from './fleets_fixture';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;

const project = {
  project: { id: 1, owner: 'PapayaPOS', repo: 'pos', base_path: '/r/pos', last_session_at: null, adopted: false, system: false },
  worktrees: [],
};

function fleet(id: string, extra: Partial<FleetInfo> = {}): FleetInfo {
  return fleetInfo(id, { url: id === PRIMARY ? null : `https://fleet.${id}.dev`, color: '#e8833a', ...extra });
}

beforeEach(() => {
  inv.mockReset();
  inv.mockResolvedValue(null);
  localStorage.clear();
  hubStatus.set({ ...STANDALONE });
  hubConnection.set({ state: 'standalone' });
  hosts.set([
    { alias: 'local', ssh_alias: null, reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false, transport: 'ssh' },
  ]);
  _resetFleetsForTest();
});

function seedCompany() {
  _setFleetsForTest([fleet(PRIMARY), fleet('papayapos')]);
  _setFleetRowsForTest('papayapos', {
    ...emptyRows(),
    hosts: new Map([
      ['mac', { alias: 'mac', ssh_alias: 'mac', reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false, transport: 'ssh' }],
    ]),
    projects: [{ project: { id: 42, owner: 'papayapos', repo: 'pos', base_path: '', last_session_at: null, adopted: false, system: false }, worktrees: [] }],
  });
}

describe('New session on an extra fleet', () => {
  it('shows no fleet row while there is no extra fleet', async () => {
    _setFleetsForTest([fleet(PRIMARY)]);
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    expect(screen.queryByTestId('new-session-fleet')).toBeNull();
  });

  it("offers a fleet's machines without the repository, and clones it there before the start", async () => {
    _setFleetsForTest([fleet(PRIMARY), fleet('papayapos')]);
    _setFleetRowsForTest('papayapos', {
      ...emptyRows(),
      hosts: new Map([
        ['mac', { alias: 'mac', ssh_alias: 'mac', reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false, transport: 'ssh' }],
      ]),
    });
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'add_project')
        return { project: { id: 51, owner: 'PapayaPOS', repo: 'pos', base_path: '', last_session_at: null, adopted: false, system: false }, worktrees: [] };
      if (cmd === 'new_session') return { id: 8, tmux_name: 'pos-1', host_alias: 'mac', status: 'running', last_activity_at: 1 };
      return null;
    });
    const onCancel = vi.fn();
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel } });
    await tick();
    expect(screen.getByTestId('new-session-fleet-papayapos').getAttribute('aria-checked')).toBe('true');
    expect((screen.getByTestId('new-session-fleet-host') as HTMLSelectElement).value).toBe('mac');
    expect(screen.getByTestId('new-session-fleet-clone').textContent).toContain('clones it onto mac');
    await fireEvent.click(screen.getByTestId('create-btn'));
    await vi.waitFor(() => expect(onCancel).toHaveBeenCalled());
    const add = inv.mock.calls.find(([cmd]) => cmd === 'add_project')!;
    expect(add[1]).toEqual({
      fleet: 'papayapos',
      args: { host_alias: 'mac', source: { kind: 'clone', url: 'https://github.com/PapayaPOS/pos' } },
    });
    const call = inv.mock.calls.find(([cmd]) => cmd === 'new_session')!;
    expect(call[1]).toMatchObject({ fleet: 'papayapos', args: { host_alias: 'mac', project_id: 51 } });
    expect(get(fleetSelection)).toEqual({ fleet: 'papayapos', id: 8 });
  });

  it("defaults to the fleet that owns the repository's owner and starts there", async () => {
    seedCompany();
    inv.mockImplementation(async (cmd: string) =>
      cmd === 'new_session' ? { id: 7, tmux_name: 'pos-1', host_alias: 'mac', status: 'running', last_activity_at: 1 } : null,
    );
    const onCreate = vi.fn();
    const onCancel = vi.fn();
    render(NewSessionDialog, { props: { project, onCreate, onCancel } });
    await tick();
    expect(screen.getByTestId('new-session-fleet-papayapos').getAttribute('aria-checked')).toBe('true');
    expect((screen.getByTestId('new-session-fleet-host') as HTMLSelectElement).value).toBe('mac');
    await fireEvent.click(screen.getByTestId('create-btn'));
    await vi.waitFor(() => expect(onCancel).toHaveBeenCalled());
    const call = inv.mock.calls.find(([cmd]) => cmd === 'new_session')!;
    expect(call[1]).toMatchObject({ fleet: 'papayapos', args: { host_alias: 'mac', project_id: 42 } });
    expect(onCreate).not.toHaveBeenCalled();
    expect(get(fleetSelection)).toEqual({ fleet: 'papayapos', id: 7 });
  });

  it('picking this window again starts here as before', async () => {
    seedCompany();
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await fireEvent.click(screen.getByTestId(`new-session-fleet-${PRIMARY}`));
    await tick();
    expect(screen.queryByTestId('new-session-fleet-host')).toBeNull();
    expect(screen.getByTestId('create-btn').textContent).toBe('Create');
  });
});

// Multi-hub Phase 3b: the dialog says which profile the session runs as.
describe('New session names its profile', () => {
  it('follows the fleet picked, read from that fleet', async () => {
    seedCompany();
    const { setProfileLabel, _resetProfilesForTest } = await import('./fleet_profiles');
    _resetProfilesForTest();
    setProfileLabel('papayapos', 'Work');
    inv.mockImplementation(async (cmd: string, args: { fleet?: string }) => {
      if (cmd === 'my_profile')
        return args?.fleet === 'papayapos'
          ? { person_id: 4, name: 'martin', email: 'martin@papayapos.example' }
          : { person_id: 1, name: 'martin', email: 'me@home.example' };
      return null;
    });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await vi.waitFor(() =>
      expect(screen.getByTestId('new-session-profile').textContent).toBe('Runs as Work · martin · martin@papayapos.example'),
    );
    await fireEvent.click(screen.getByTestId(`new-session-fleet-${PRIMARY}`));
    await vi.waitFor(() => expect(screen.getByTestId('new-session-profile').textContent).toContain('me@home.example'));
    expect(inv.mock.calls.filter(([cmd]) => cmd === 'my_profile').map(([, a]) => a)).toEqual([{ fleet: 'papayapos' }, {}]);
  });
});

describe('New session on an extra fleet: launch choices (review w01-2)', () => {
  const started = { id: 7, tmux_name: 'pos-1', host_alias: 'mac', status: 'running', last_activity_at: 1 };

  it("sends the model and effort to that hub, and offers no login of this window's", async () => {
    seedCompany();
    inv.mockImplementation(async (cmd: string) => (cmd === 'new_session' ? started : null));
    const onCancel = vi.fn();
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel } });
    await tick();
    expect(screen.queryByTestId('launch-account')).toBeNull();
    expect(screen.queryByTestId('launch-profile')).toBeNull();
    expect(screen.getByTestId('fleet-login-note').textContent).toContain('papayapos');
    await fireEvent.change(screen.getByTestId('launch-model'), { target: { value: 'opus' } });
    await fireEvent.change(screen.getByTestId('launch-effort'), { target: { value: 'high' } });
    await fireEvent.click(screen.getByTestId('create-btn'));
    await vi.waitFor(() => expect(onCancel).toHaveBeenCalled());
    const call = inv.mock.calls.find(([cmd]) => cmd === 'new_session')!;
    expect(call[1]).toMatchObject({ fleet: 'papayapos', args: { model: 'opus', effort: 'high', agent: null } });
  });

  it("offers the agents the fleet's host has, and starts that agent there", async () => {
    seedCompany();
    _setFleetRowsForTest('papayapos', {
      ...emptyRows(),
      hosts: new Map([
        ['mac', { alias: 'mac', ssh_alias: 'mac', reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false, transport: 'ssh', agents_on_path: ['codex'] }],
      ]),
      projects: [{ project: { id: 42, owner: 'papayapos', repo: 'pos', base_path: '', last_session_at: null, adopted: false, system: false }, worktrees: [] }],
    });
    inv.mockImplementation(async (cmd: string) => (cmd === 'new_session' ? started : null));
    const onCancel = vi.fn();
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel } });
    await tick();
    const codex = screen.getByTestId('agent-codex') as HTMLButtonElement;
    expect(codex.disabled).toBe(false);
    await fireEvent.click(codex);
    await tick();
    await fireEvent.click(screen.getByTestId('create-btn'));
    await vi.waitFor(() => expect(onCancel).toHaveBeenCalled());
    const call = inv.mock.calls.find(([cmd]) => cmd === 'new_session')!;
    expect(call[1]).toMatchObject({ fleet: 'papayapos', args: { agent: 'codex' } });
  });
});
