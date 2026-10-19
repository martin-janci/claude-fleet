// Multi-hub Phase 3a: an extra fleet's sessions in the Sessions list, under
// the same filters as this window's own, and never hidden without a word.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }));

import FleetSessionGroups from './FleetSessionGroups.svelte';
import SidebarFilters from './SidebarFilters.svelte';
import PrimaryFleetHeader from './PrimaryFleetHeader.svelte';
import {
  PRIMARY,
  emptyRows,
  _resetFleetsForTest,
  _setFleetRowsForTest,
  _setFleetsForTest,
  type FleetInfo,
  type FleetRows,
} from './fleets';
import { _resetFleetFiltersForTest, fleetHostFilter, pickFleetHost } from './fleet_filters';
import { hostFilter, hosts, type HostRow } from './hosts';
import { scopeFilter, orgs, type OrgDetail } from './orgs';
import { showBgAgents, type SessionRow } from './sessions';

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

const now = Math.floor(Date.now() / 1000);
const sess = (id: number, extra: Partial<SessionRow> = {}) =>
  ({ id, tmux_name: `s-${id}`, host_alias: 'eva-air', project_id: null, status: 'running', kind: 'work', last_activity_at: now, claude_status: 'idle', ...extra }) as SessionRow;
const host = (alias: string) => ({ alias, reachable: true, hidden: false }) as HostRow;

function rows(): FleetRows {
  return {
    ...emptyRows(),
    sessions: new Map([[7, sess(7, { kind: 'bg', tmux_name: 'bg-idle' })]]),
    hosts: new Map([
      ['eva-air', host('eva-air')],
      ['papayapos-dev', host('papayapos-dev')],
    ]),
    orgs: [{ id: 3, name: 'PapayaPOS', created_at: 1, rules: [], hosts: [], trackers: [] } as OrgDetail],
  };
}

beforeEach(() => {
  localStorage.clear();
  _resetFleetsForTest();
  _resetFleetFiltersForTest();
  hostFilter.set('all');
  scopeFilter.set('all');
  showBgAgents.set(true);
  _setFleetsForTest([primary, company]);
  _setFleetRowsForTest('company', rows());
});

describe('FleetSessionGroups', () => {
  it("lists an extra fleet's idle background agent under its badge", async () => {
    render(FleetSessionGroups);
    await tick();
    expect(screen.getByTestId('fleet-badge-company').textContent).toBe('Papaya POS (hub)');
    expect(screen.getByTestId('fleet-session-company-7').textContent).toContain('bg-idle');
    expect(screen.getByTestId('fleet-bg-company-7')).toBeTruthy();
  });

  it('honours the search and the background-agents switch', async () => {
    render(FleetSessionGroups, { props: { search: 'nothing-like-it' } });
    await tick();
    expect(screen.queryByTestId('fleet-session-company-7')).toBeNull();
    expect(screen.getByTestId('fleet-empty-company').textContent).toContain('matches the filters');
  });

  it("says how many sessions a primary org filter hides, and shows them again", async () => {
    // A primary org needs two primary scopes to stand.
    orgs.set([
      { id: 1, name: '32bit', created_at: 1, rules: [], hosts: [], trackers: [] } as OrgDetail,
      { id: 2, name: 'Papaya POS', created_at: 1, rules: [], hosts: [], trackers: [] } as OrgDetail,
    ]);
    scopeFilter.set('org:2');
    render(FleetSessionGroups);
    await tick();
    expect(screen.queryByTestId('fleet-session-company-7')).toBeNull();
    const note = screen.getByTestId('fleet-hidden-company');
    expect(note.textContent).toContain('1 session on Papaya POS (hub) hidden');
    expect(note.textContent).toContain('Org: Papaya POS on rlt');
    await fireEvent.click(screen.getByTestId('fleet-hidden-clear-company'));
    await tick();
    expect(get(scopeFilter)).toBe('all');
    expect(screen.getByTestId('fleet-session-company-7')).toBeTruthy();
    orgs.set([]);
  });

  it("narrows to an extra fleet's machine", async () => {
    pickFleetHost('company', 'papayapos-dev');
    render(FleetSessionGroups);
    await tick();
    expect(screen.queryByTestId('fleet-session-company-7')).toBeNull();
    expect(screen.queryByTestId('fleet-hidden-company')).toBeNull();
  });
});

describe('PrimaryFleetHeader', () => {
  it("names this window's fleet, and says when another fleet's machine hides it", async () => {
    render(PrimaryFleetHeader, { props: { count: 4, hiddenCount: 4 } });
    await tick();
    expect(screen.getByTestId('primary-fleet-badge').textContent).toBe('rlt');
    expect(screen.getByTestId('primary-fleet-count').textContent).toBe('4');
    pickFleetHost('company', 'eva-air');
    await tick();
    const note = screen.getByTestId('primary-fleet-hidden');
    expect(note.textContent).toContain('4 sessions on rlt hidden');
    expect(note.textContent).toContain('Machine: eva-air on Papaya POS (hub)');
    await fireEvent.click(screen.getByTestId('primary-fleet-hidden-clear'));
    expect(get(fleetHostFilter)).toBeNull();
  });
});

describe('SidebarFilters with an extra fleet', () => {
  const props = {
    search: '',
    recency: 'all' as const,
    needsYouOnly: false,
    loading: false,
    loadError: null,
    onRefresh: () => {},
    needsYouCount: 0,
    selectMode: false,
    toggleSelectMode: () => {},
    selectedCount: 0,
    onBulkSend: () => {},
    onBulkKill: () => {},
    clearSelected: () => {},
  };

  it("offers each fleet's machines and orgs under its own label, and strips the choice with its fleet", async () => {
    hosts.set([host('mac')]);
    render(SidebarFilters, { props });
    await fireEvent.click(screen.getByTestId('filters-open'));
    await tick();
    expect(screen.getByRole('group', { name: 'Machine · rlt' })).toBeTruthy();
    expect(screen.getByRole('group', { name: 'Machine · Papaya POS (hub)' })).toBeTruthy();
    expect(screen.getByRole('group', { name: 'Organisation · Papaya POS (hub)' })).toBeTruthy();
    expect(screen.getByTestId('filter-host-mac')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('filter-fleet-host-company-eva-air'));
    await tick();
    expect(get(fleetHostFilter)).toEqual({ fleet: 'company', alias: 'eva-air' });
    expect(screen.getByTestId('filter-fleet-host-company-eva-air').getAttribute('aria-pressed')).toBe('true');
    expect(screen.getByTestId('filter-host-all').getAttribute('aria-pressed')).toBe('false');
    expect(document.body.textContent).toContain('Host: eva-air · Papaya POS (hub)');
    await fireEvent.click(screen.getByTestId('filter-host-mac'));
    await tick();
    expect(get(fleetHostFilter)).toBeNull();
    expect(get(hostFilter)).toBe('mac');
    expect(document.body.textContent).toContain('Host: mac · rlt');
  });
});
