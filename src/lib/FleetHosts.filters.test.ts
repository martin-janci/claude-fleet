// Multi-hub: an extra fleet's hosts show in Control's Hosts view, and the
// sidebar's filters (Machine above all) reach the extra fleets' rows.
import { render, screen } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }));
import ControlHostsView from './ControlHostsView.svelte';
import FleetSessionGroups from './FleetSessionGroups.svelte';
import {
  PRIMARY,
  _resetFleetsForTest,
  _setFleetsForTest,
  _setFleetRowsForTest,
  emptyRows,
  fleetFilter,
  fleetHostFilterId,
  fleetHostFilterOptions,
  fleetHostPasses,
  hostFilterLabel,
  parseFleetHostFilter,
  type FleetInfo,
} from './fleets';
import { effectiveHostOf, extraHostFilterIds, type HostRow } from './hosts';
import type { SessionRow } from './sessions';

const work: FleetInfo = {
  id: 'work',
  label: 'work',
  url: 'https://fleet.example.dev',
  color: '#e8833a',
  primary: false,
  local: false,
  client_name: null,
  connection: { state: 'connected' },
  unavailable: null,
  fleet_id: 'bbbbbbbb-2222',
};
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

describe('the Machine filter value for an extra fleet', () => {
  it('round-trips and passes only that fleet’s host', () => {
    const id = fleetHostFilterId('work', 'mercury');
    expect(parseFleetHostFilter(id)).toEqual({ fleet: 'work', alias: 'mercury' });
    expect(parseFleetHostFilter('mercury')).toBeNull();
    expect(fleetHostPasses('all', 'work', 'venus')).toBe(true);
    expect(fleetHostPasses(id, 'work', 'mercury')).toBe(true);
    expect(fleetHostPasses(id, 'work', 'venus')).toBe(false);
    expect(fleetHostPasses(id, 'other', 'mercury')).toBe(false);
    // A filter on one of this window's own hosts shows none of the fleet's rows.
    expect(fleetHostPasses('mercury', 'work', 'mercury')).toBe(false);
  });

  it('offers the visible fleets’ visible hosts and keeps such a value in effect', () => {
    const ids = get(fleetHostFilterOptions).map((o) => o.id);
    expect(ids).toEqual([fleetHostFilterId('work', 'mercury'), fleetHostFilterId('work', 'venus')]);
    expect(get(hostFilterLabel)(fleetHostFilterId('work', 'venus'))).toBe('work · venus');
    const extra = get(extraHostFilterIds);
    const own = [host('local')];
    expect(effectiveHostOf(fleetHostFilterId('work', 'venus'), own, extra)).toBe(fleetHostFilterId('work', 'venus'));
    expect(effectiveHostOf(fleetHostFilterId('work', 'old'), own, extra)).toBe('all');
    // The fleet filter hiding the fleet takes its machines out of the offer.
    fleetFilter.set(PRIMARY);
    expect(get(fleetHostFilterOptions)).toEqual([]);
  });
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

describe('the sidebar’s extra fleet groups', () => {
  it('show only the rows the filters pass, and say so when none do', () => {
    const id = fleetHostFilterId('work', 'venus');
    render(FleetSessionGroups, {
      props: { match: (f: string, s: SessionRow) => fleetHostPasses(id, f, s.host_alias), filtered: true },
    });
    expect(screen.queryByTestId('fleet-session-work-1')).toBeNull();
    expect(screen.getByTestId('fleet-session-work-2')).toBeTruthy();
  });

  it('a filter nothing passes leaves a note, not a silent empty group', () => {
    render(FleetSessionGroups, { props: { match: () => false, filtered: true } });
    expect(screen.getByTestId('fleet-filtered-work')).toBeTruthy();
  });
});
