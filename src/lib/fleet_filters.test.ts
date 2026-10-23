// Multi-hub Phase 3a: the Machine and Organisation filters across fleets.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import {
  PRIMARY,
  emptyRows,
  _resetFleetsForTest,
  _setFleetRowsForTest,
  _setFleetsForTest,
  fleetFilter,
  type FleetInfo,
  type FleetRows,
} from './fleets';
import {
  anchor,
  anchorFacetWord,
  anchorText,
  fleetHostFilter,
  fleetListView,
  fleetScopeFilter,
  fleetScopesOf,
  machineOrgAnchor,
  pickFleetHost,
  pickFleetScope,
  pickPrimaryHost,
  pickPrimaryScope,
  primaryHiddenByAnchor,
  clearMachineOrg,
  cyclePrimaryScope,
  forgetFleetFilters,
  fleetGroupsFolded,
  primaryFleetOpen,
  showPrimaryList,
  _resetFleetFiltersForTest,
  type AnchorInput,
  type FleetListCtx,
} from './fleet_filters';
import { hostFilter } from './hosts';
import { scopeFilter, type OrgDetail } from './orgs';
import type { SessionRow } from './sessions';
import type { HostRow } from './hosts';
import { fleetInfo } from './fleets_fixture';

function fleet(id: string, extra: Partial<FleetInfo> = {}): FleetInfo {
  return fleetInfo(id, {
    label: id === PRIMARY ? 'rlt' : 'Papaya POS (hub)',
    url: `https://${id}.example.com`,
    color: id === PRIMARY ? '#5b8def' : '#e8833a',
    local: false,
    client_name: 'laptop',
    connection: { state: 'connected' },
    ...extra,
  });
}

const NOW = 1_000_000;

function sess(id: number, extra: Partial<SessionRow> = {}): SessionRow {
  return {
    id,
    tmux_name: `s-${id}`,
    host_alias: 'eva-air',
    project_id: 1,
    status: 'running',
    kind: 'work',
    last_activity_at: NOW - 60,
    claude_status: 'idle',
    org_id: null,
    ...extra,
  } as SessionRow;
}

function host(alias: string): HostRow {
  return { alias, reachable: true, hidden: false } as HostRow;
}

const org = (id: number, name: string) => ({ id, name, created_at: 1, rules: [], hosts: [], trackers: [] }) as OrgDetail;

/** The company fleet: org 3 PapayaPOS; a bg agent idle on eva-air and a
 *  work session on papayapos-dev. */
function companyRows(): FleetRows {
  return {
    ...emptyRows(),
    sessions: new Map([
      [1, sess(1, { kind: 'bg', org_id: 3 })],
      [2, sess(2, { host_alias: 'papayapos-dev', org_id: 3 })],
      [3, sess(3, { host_alias: 'papayapos-dev', status: 'lost' })],
    ]),
    hosts: new Map([
      ['eva-air', host('eva-air')],
      ['papayapos-dev', host('papayapos-dev')],
    ]),
    projects: [
      {
        project: { id: 1, owner: 'papayapos', repo: 'pos', base_path: '/p', last_session_at: null, adopted: false, system: false },
        worktrees: [],
      },
    ] as FleetRows['projects'],
    orgs: [org(3, 'PapayaPOS')],
  };
}

const ctx = (extra: Partial<FleetListCtx> = {}): FleetListCtx => ({
  anchor: null,
  search: '',
  recency: 'all',
  nowSec: NOW,
  showBg: true,
  agent: 'any',
  needsYouOnly: false,
  needs: new Set(),
  ...extra,
});

const known = [fleet(PRIMARY), fleet('company')];
const input = (extra: Partial<AnchorInput> = {}): AnchorInput => ({
  filter: 'all',
  known,
  rows: new Map([['company', companyRows()]]),
  primaryHost: 'all',
  primaryScope: 'all',
  fleetHost: null,
  fleetScope: null,
  ...extra,
});

beforeEach(() => {
  _resetFleetsForTest();
  _resetFleetFiltersForTest();
  hostFilter.set('all');
  scopeFilter.set('all');
});

describe('an extra fleet in the Sessions list', () => {
  it('lists every session but a lost one, background agents included', () => {
    const v = fleetListView('company', companyRows(), ctx());
    expect(v.shown.map((s) => s.id).sort()).toEqual([1, 2]);
    expect(v.hiddenByAnchor).toBe(0);
  });

  it('passes the same shared filters as the primary list', () => {
    const rows = companyRows();
    expect(fleetListView('company', rows, ctx({ showBg: false })).shown.map((s) => s.id)).toEqual([2]);
    expect(fleetListView('company', rows, ctx({ search: 's-1' })).shown.map((s) => s.id)).toEqual([1]);
    expect(fleetListView('company', rows, ctx({ search: 'papayapos pos' })).shown).toHaveLength(2);
    expect(fleetListView('company', rows, ctx({ needsYouOnly: true, needs: new Set([2]) })).shown.map((s) => s.id)).toEqual([2]);
    const old = { ...rows, sessions: new Map([[9, sess(9, { last_activity_at: NOW - 3 * 86400 })]]) };
    expect(fleetListView('company', old, ctx({ recency: '1d' })).shown).toHaveLength(0);
  });
});

describe('the anchor: one fleet owns Machine and Org', () => {
  it('nothing chosen is no anchor', () => {
    expect(machineOrgAnchor(input())).toBeNull();
  });

  it("a primary org anchors the primary and hides the extra fleet's rows, counted", () => {
    const a = machineOrgAnchor(input({ primaryScope: 'org:7' }))!;
    expect(a).toEqual({ fleet: PRIMARY, host: null, scope: 'org:7' });
    expect(primaryHiddenByAnchor(a)).toBe(false);
    const v = fleetListView('company', companyRows(), ctx({ anchor: a }));
    expect(v.shown).toHaveLength(0);
    expect(v.hiddenByAnchor).toBe(2);
    expect(anchorFacetWord(a)).toBe('org filter');
    expect(anchorText(a, known, () => 'Papaya POS')).toBe('Org: Papaya POS on rlt');
  });

  it("an extra fleet's machine anchors that fleet and narrows its rows", () => {
    const a = machineOrgAnchor(input({ fleetHost: { fleet: 'company', alias: 'eva-air' } }))!;
    expect(a).toEqual({ fleet: 'company', host: 'eva-air', scope: null });
    expect(primaryHiddenByAnchor(a)).toBe(true);
    expect(fleetListView('company', companyRows(), ctx({ anchor: a })).shown.map((s) => s.id)).toEqual([1]);
  });

  it("an extra fleet's org narrows by that hub's org ids, never the primary's", () => {
    const a = machineOrgAnchor(input({ fleetScope: { fleet: 'company', scope: 'org:3' } }))!;
    expect(fleetListView('company', companyRows(), ctx({ anchor: a })).shown.map((s) => s.id).sort()).toEqual([1, 2]);
    expect(anchorText(a, known, () => 'PapayaPOS')).toBe('Org: PapayaPOS on Papaya POS (hub)');
  });

  it('the switcher wins: an anchor in a fleet it hides is ignored', () => {
    expect(machineOrgAnchor(input({ filter: 'company', primaryHost: 'mac' }))).toBeNull();
    expect(machineOrgAnchor(input({ filter: PRIMARY, fleetHost: { fleet: 'company', alias: 'eva-air' } }))).toBeNull();
  });

  it('a choice naming a host or fleet that is gone reads as none', () => {
    expect(machineOrgAnchor(input({ fleetHost: { fleet: 'company', alias: 'gone' } }))).toBeNull();
    expect(machineOrgAnchor(input({ fleetHost: { fleet: 'lab', alias: 'eva-air' } }))).toBeNull();
    expect(machineOrgAnchor(input({ fleetScope: { fleet: 'company', scope: 'org:99' } }))).toBeNull();
  });

  it("an extra fleet's scopes are its own orgs and uncovered owners", () => {
    const rows = companyRows();
    rows.sessions.set(4, sess(4, { org_id: null }));
    expect(fleetScopesOf(rows)).toEqual([
      { id: 'org:3', label: 'PapayaPOS', color: null },
      { id: 'owner:papayapos', label: 'papayapos', color: null },
    ]);
  });
});

describe('choosing keeps one anchor', () => {
  it("an extra fleet's machine clears the primary's machine and org", () => {
    hostFilter.set('mac');
    scopeFilter.set('org:7');
    pickFleetHost('company', 'eva-air');
    expect(get(hostFilter)).toBe('all');
    expect(get(scopeFilter)).toBe('all');
    expect(get(fleetHostFilter)).toEqual({ fleet: 'company', alias: 'eva-air' });
    pickFleetHost('company', 'eva-air');
    expect(get(fleetHostFilter)).toBeNull();
  });

  it("a fleet's machine and org combine; another fleet's choice clears both", () => {
    pickFleetHost('company', 'eva-air');
    pickFleetScope('company', 'org:3');
    expect(get(fleetHostFilter)).toEqual({ fleet: 'company', alias: 'eva-air' });
    expect(get(fleetScopeFilter)).toEqual({ fleet: 'company', scope: 'org:3' });
    pickPrimaryScope('org:7');
    expect(get(fleetHostFilter)).toBeNull();
    expect(get(fleetScopeFilter)).toBeNull();
    expect(get(scopeFilter)).toBe('org:7');
  });

  it('Any clears that facet in every fleet; Show every fleet clears both', () => {
    pickFleetHost('company', 'eva-air');
    pickPrimaryHost('all');
    expect(get(fleetHostFilter)).toBeNull();
    pickPrimaryScope('org:7');
    pickPrimaryHost('mac');
    clearMachineOrg();
    expect([get(hostFilter), get(scopeFilter), get(fleetHostFilter), get(fleetScopeFilter)]).toEqual(['all', 'all', null, null]);
  });

  it("a primary jump (reveal, ⌘⇧O) drops an extra fleet's choice and unfolds the primary", () => {
    pickFleetHost('company', 'eva-air');
    pickFleetScope('company', 'org:3');
    primaryFleetOpen.set(false);
    showPrimaryList();
    expect([get(fleetHostFilter), get(fleetScopeFilter), get(primaryFleetOpen)]).toEqual([null, null, true]);
    // ⌘⇧O is a no-op with fewer than two primary scopes, and must not drop
    // anything then; with two it picks through pickPrimaryScope.
    pickFleetHost('company', 'eva-air');
    cyclePrimaryScope();
    expect(get(fleetHostFilter)).toEqual({ fleet: 'company', alias: 'eva-air' });
  });

  it("forgetting a fleet drops only that fleet's choices and fold", () => {
    pickFleetHost('company', 'eva-air');
    pickFleetScope('company', 'org:3');
    fleetGroupsFolded.set(['company', 'lab']);
    forgetFleetFilters('lab');
    expect(get(fleetHostFilter)).toEqual({ fleet: 'company', alias: 'eva-air' });
    expect(get(fleetGroupsFolded)).toEqual(['company']);
    forgetFleetFilters('company');
    expect([get(fleetHostFilter), get(fleetScopeFilter), get(fleetGroupsFolded)]).toEqual([null, null, []]);
  });

  it('the anchor store follows the stores', () => {
    _setFleetsForTest(known);
    _setFleetRowsForTest('company', companyRows());
    pickFleetHost('company', 'eva-air');
    expect(get(anchor)).toEqual({ fleet: 'company', host: 'eva-air', scope: null });
    fleetFilter.set(PRIMARY);
    expect(get(anchor)).toBeNull();
  });
});
