// Multi-hub: the Hosts page with another fleet beside this window's own.
// Martin, 2026-11-05: with a second fleet connected, 'All hosts' showed only
// that fleet's hosts — the own table shared the column with the fleets'
// sections and was squeezed to nothing. Own hosts now come first, under
// their own badge, in one scroll with the other fleets.
import { render, screen, within, fireEvent } from '@testing-library/svelte';
import { get } from 'svelte/store';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }));
import HostsView from './HostsView.svelte';
import { hosts, resetTombstonesForTests } from './hosts';
import { accounts } from './accounts';
import { sessions } from './sessions';
import { fleetAccounts, fleetHosts, fleetSessions } from './hosts_fixture';
import { hubStatus, STANDALONE } from './hub';
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
import { fleetInfo } from './fleets_fixture';

const papaya = fleetInfo('papaya', {
  label: 'Papaya POS',
  url: 'https://fleet.papayapos.dev',
  color: '#e8833a',
  fleet_id: 'bbbb',
});
const own: FleetInfo = {
  ...papaya,
  id: PRIMARY,
  primary: true,
  label: 'rlt',
  color: '#4f8cff',
  fleet_id: 'aaaa',
};

const extraHost = (alias: string) =>
  ({
    alias,
    hidden: false,
    reachable: true,
    transport: 'ssh',
    last_pinged_at: null,
  }) as unknown as HostRow;

beforeEach(() => {
  resetTombstonesForTests();
  hubStatus.set({ ...STANDALONE });
  hosts.set(fleetHosts());
  accounts.set(fleetAccounts());
  sessions.set(fleetSessions());
  _resetFleetsForTest();
  fleetFilter.set('all');
  _setFleetsForTest([own, papaya]);
  const rows = emptyRows();
  rows.hosts = new Map(
    Array.from({ length: 30 }, (_, i) => [`pos-${i}`, extraHost(`pos-${i}`)]),
  );
  _setFleetRowsForTest('papaya', rows);
});

describe('Hosts page with two fleets', () => {
  it('lists this window’s own hosts first, under their badge, then the other fleet’s', () => {
    render(HostsView, { props: { onClose: vi.fn(), onFilterSidebar: vi.fn(), onNewSession: vi.fn() } });
    const body = screen.getByTestId('hosts-fleets-body');
    const own = within(body).getByTestId('hosts-own-fleet');
    expect(own.textContent).toContain('rlt');
    expect(
      within(own).getAllByTestId('hosts-table-row').length,
    ).toBeGreaterThan(0);
    expect(own.textContent).toContain('claude-fleet-htz');
    const other = within(body).getByTestId('fleet-hosts-papaya');
    expect(other.textContent).toContain('pos-29');
    // Own first, in the same scroll.
    expect(
      own.compareDocumentPosition(other) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
  });

  it('says the own hosts are filtered out when the fleet filter names the other fleet', () => {
    fleetFilter.set('papaya');
    render(HostsView, { props: { onClose: vi.fn(), onFilterSidebar: vi.fn(), onNewSession: vi.fn() } });
    expect(screen.getByTestId('hosts-primary-filtered')).toBeTruthy();
    expect(screen.queryByTestId('hosts-own-fleet')).toBeNull();
    expect(screen.getByTestId('fleet-hosts-papaya')).toBeTruthy();
  });

  it('names the fleet the filter shows, and Show all fleets brings the own hosts back', async () => {
    fleetFilter.set('papaya');
    render(HostsView, { props: { onClose: vi.fn(), onFilterSidebar: vi.fn(), onNewSession: vi.fn() } });
    expect(screen.getByTestId('hosts-primary-filtered').textContent).toContain('Papaya POS');
    await fireEvent.click(screen.getByTestId('hosts-primary-filtered-show-all'));
    expect(get(fleetFilter)).toBe('all');
    expect(screen.getByTestId('hosts-own-fleet')).toBeTruthy();
    expect(screen.queryByTestId('hosts-primary-filtered')).toBeNull();
  });

  it('keeps the other fleet in view when a host detail was open and the filter hides the own hosts', async () => {
    fleetFilter.set('papaya');
    render(HostsView, { props: { preselect: 'claude-fleet-htz', onClose: vi.fn(), onFilterSidebar: vi.fn(), onNewSession: vi.fn() } });
    expect(screen.getByTestId('hosts-primary-filtered')).toBeTruthy();
    expect(screen.getByTestId('fleet-hosts-papaya')).toBeTruthy();
  });
});
