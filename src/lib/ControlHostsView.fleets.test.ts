// Multi-hub: an extra fleet's hosts show in Control's Hosts view.
import { render, screen } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }));
import ControlHostsView from './ControlHostsView.svelte';
import {
  PRIMARY,
  _resetFleetsForTest,
  _setFleetsForTest,
  _setFleetRowsForTest,
  emptyRows,
  fleetFilter,
  type FleetInfo,
} from './fleets';
import type { HostRow } from './hosts';
import type { SessionRow } from './sessions';
import { fleetInfo } from './fleets_fixture';

const work = fleetInfo('work', { url: 'https://fleet.example.dev', color: '#e8833a', fleet_id: 'bbbbbbbb-2222' });
const home: FleetInfo = { ...work, id: PRIMARY, primary: true, label: 'home', fleet_id: 'aaaaaaaa-1111' };

const host = (alias: string, extra: Partial<HostRow> = {}) =>
  ({ alias, hidden: false, reachable: true, transport: 'ssh', last_pinged_at: null, ...extra }) as unknown as HostRow;
const sess = (id: number, host_alias: string, name: string) =>
  ({ id, tmux_name: name, host_alias, status: 'running', last_activity_at: 1, claude_status: 'idle', kind: 'claude' }) as unknown as SessionRow;

function seed() {
  _setFleetsForTest([home, work]);
  const rows = emptyRows();
  rows.hosts = new Map([
    ['mercury', host('mercury')],
    ['venus', host('venus', { reachable: false })],
    ['old', host('old', { hidden: true })],
  ]);
  rows.sessions = new Map([
    [1, sess(1, 'mercury', 'api')],
    [2, sess(2, 'venus', 'web')],
  ]);
  _setFleetRowsForTest('work', rows);
}

beforeEach(() => {
  _resetFleetsForTest();
  seed();
});

describe("Control's Hosts view", () => {
  it('lists an extra fleet’s hosts under its badge', () => {
    render(ControlHostsView);
    const group = screen.getByTestId('fleet-host-health-work');
    expect(group.textContent).toContain('work');
    expect(group.textContent).toContain('mercury');
    expect(group.textContent).toContain('venus');
    expect(group.textContent).not.toContain('old');
  });

  it('drops this window’s own hosts when the fleet filter names the extra fleet', () => {
    fleetFilter.set('work');
    render(ControlHostsView);
    expect(screen.getByTestId('control-hosts-primary-filtered')).toBeTruthy();
    expect(screen.getByTestId('fleet-host-health-work')).toBeTruthy();
  });
});
