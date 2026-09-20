import { render, screen, fireEvent, within } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import HostsList from './HostsList.svelte';
import { groupHostsByAccount, sessionCounts, type HostRowInfo } from './hosts_view';
import { NOW, fleetAccounts, fleetHosts, fleetSessions, fleetUsage, host } from './hosts_fixture';
import { hubStatus, STANDALONE, type HubStatus } from './hub';
import { hubConnection } from './hub_connection';

beforeEach(() => {
  hubStatus.set({ ...STANDALONE });
  hubConnection.set({ state: 'standalone' });
});

function mount(over: Record<string, unknown> = {}) {
  const hosts = (over.hosts as ReturnType<typeof fleetHosts>) ?? fleetHosts();
  const rows = fleetSessions();
  const rowInfo = new Map<string, HostRowInfo>(
    hosts.map((h) => [h.alias, { counts: sessionCounts(h.alias, rows), attention: null }]),
  );
  const onselect = vi.fn();
  render(HostsList, {
    props: {
      groups: groupHostsByAccount(hosts, fleetAccounts()),
      rowInfo,
      snapshots: fleetUsage(),
      selectedAlias: 'mefistos',
      listId: 'test-list',
      now: NOW,
      locale: 'en-GB',
      timeZone: 'UTC',
      editingUuid: null,
      onselect,
      oneditstart: vi.fn(),
      oneditdone: vi.fn(),
      ...over,
    },
  });
  return { onselect };
}

describe('HostsList', () => {
  it('renders groups in stable order with a No Claude account group last', () => {
    mount({ hosts: [...fleetHosts(), host('nas')] });
    const labels = screen.getAllByTestId('group-label').map((l) => l.textContent);
    expect(labels).toEqual(['admin-janci@users.noreply.github.com', 'm-janci@users.noreply.github.com', 'mj-janci@users.noreply.github.com', 'No Claude account']);
    // The no-account group has no usage bars.
    const last = screen.getAllByTestId('hosts-group').at(-1)!;
    expect(within(last).queryAllByRole('meter')).toHaveLength(0);
  });

  it('is one listbox with aria-activedescendant and option rows', () => {
    mount();
    const list = screen.getByRole('listbox', { name: 'Hosts' });
    expect(list.getAttribute('aria-activedescendant')).toBe('test-list-opt-mefistos');
    expect(screen.getAllByRole('option')).toHaveLength(5);
    expect(screen.getByRole('option', { selected: true }).dataset.alias).toBe('mefistos');
  });

  it('clicking a row selects it; rows carry no buttons', async () => {
    const { onselect } = mount();
    const trn = screen.getAllByTestId('host-row').find((r) => r.dataset.alias === 'claude-fleet-trn')!;
    await fireEvent.click(trn);
    expect(onselect).toHaveBeenCalledWith('claude-fleet-trn');
    expect(within(trn).getByTestId('host-counts').textContent).toBe('14 ⚡2 ⏸1');
    for (const row of screen.getAllByTestId('host-row')) expect(row.querySelector('button')).toBeNull();
  });

  it('an offline host says offline in words', () => {
    mount();
    const offline = screen.getAllByTestId('host-offline');
    expect(offline).toHaveLength(1);
    expect(offline[0].closest('[data-testid="host-row"]')?.getAttribute('data-alias')).toBe('claude-fleet-htz');
  });

  it('marks an agent-transport host and leaves ssh hosts unmarked', () => {
    mount({ hosts: [...fleetHosts(), host('agent-box', { transport: 'agent' })] });
    const marks = screen.getAllByTestId('host-transport-agent');
    expect(marks).toHaveLength(1);
    expect(marks[0].closest('[data-testid="host-row"]')?.getAttribute('data-alias')).toBe('agent-box');
    // The default (ssh) hosts stay quiet — no marker on any of them.
    for (const row of screen.getAllByTestId('host-row')) {
      if (row.dataset.alias !== 'agent-box') {
        expect(within(row).queryByTestId('host-transport-agent')).toBeNull();
      }
    }
  });
});

// #195: the same honest-empty-state fix as Sidebar.svelte — an empty list
// while the hub's wire contract is skewed must not read as "no hosts".
describe('HostsList: a hub contract skew', () => {
  const remote: HubStatus = {
    remote: true,
    url: 'https://fleet.example.com',
    client_name: 'laptop',
    client_mode: null,
    configured_url: 'https://fleet.example.com',
    configured_client_name: 'laptop',
    allow_plaintext: false,
    warning: null,
    restart_required: false,
    unavailable: null,
  };

  it('shows the connection banner’s sentence instead of "No hosts yet"', () => {
    hubStatus.set(remote);
    hubConnection.set({ state: 'hub_too_old', hub_contract: 1, min_contract: 3 });
    mount({ hosts: [] });
    const empty = screen.getByTestId('hosts-empty');
    expect(empty.textContent).not.toContain('No hosts yet');
    expect(empty.textContent).toContain('fleet.example.com');
    expect(empty.textContent?.toLowerCase()).toContain('update the hub');
  });

  it('a search filter still wins over the skew sentence', () => {
    hubStatus.set(remote);
    hubConnection.set({ state: 'hub_too_old', hub_contract: 1, min_contract: 3 });
    mount({ hosts: [], filter: 'nope' });
    const empty = screen.getByTestId('hosts-empty');
    expect(empty.textContent).toContain('No host matches');
  });

  it('a connected hub with no skew renders the ordinary empty state', () => {
    hubStatus.set(remote);
    hubConnection.set({ state: 'connected' });
    mount({ hosts: [] });
    expect(screen.getByTestId('hosts-empty').textContent).toContain('No hosts yet');
  });

  it('standalone mode is untouched', () => {
    mount({ hosts: [] });
    expect(screen.getByTestId('hosts-empty').textContent).toContain('No hosts yet');
  });
});
