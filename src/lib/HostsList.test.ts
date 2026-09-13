import { render, screen, fireEvent, within } from '@testing-library/svelte';
import { describe, it, expect, vi } from 'vitest';
import HostsList from './HostsList.svelte';
import { groupHostsByAccount, sessionCounts, type HostRowInfo } from './hosts_view';
import { NOW, fleetAccounts, fleetHosts, fleetSessions, fleetUsage, host } from './hosts_fixture';

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
    expect(labels).toEqual(['admin@32bit.sk', 'm.janci@32bit.sk', 'mj.janci@gmail.com', 'No Claude account']);
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
});
