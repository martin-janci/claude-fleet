// Redesign step 4.4: bulk Switch account lets the person pick the account.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { invoke } from '@tauri-apps/api/core';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }));

import SidebarFilters from './SidebarFilters.svelte';
import { loadAccounts } from './accounts';

const base = {
  search: '',
  recency: 'all' as never,
  needsYouOnly: false,
  loading: false,
  loadError: null,
  onRefresh: () => {},
  showSettings: false,
  onOpenSettings: () => {},
  needsYouCount: 0,
  selectMode: true,
  toggleSelectMode: () => {},
  selectedCount: 2,
  onBulkSend: () => {},
  onBulkKill: () => {},
  clearSelected: () => {},
};

const account = (uuid: string, email: string) => ({
  uuid,
  email,
  display_name: null,
  organization_name: null,
  organization_uuid: null,
  seat_tier: null,
  last_seen_at: null,
  nickname: null,
  has_extra_usage: false,
});

beforeEach(async () => {
  vi.mocked(invoke).mockImplementation(async (cmd: string) =>
    cmd === 'list_accounts' ? [account('acc-a', 'a@example.com'), account('acc-b', 'b@example.com')] : null,
  );
  await loadAccounts();
});

describe('bulk Switch account (4.4)', () => {
  it('offers the accounts to pick from', async () => {
    const onBulkMoveAccount = vi.fn();
    render(SidebarFilters, { props: { ...base, onBulkMoveAccount } });
    await fireEvent.click(screen.getByTestId('bulk-move-account'));
    expect(onBulkMoveAccount).not.toHaveBeenCalled();
    const pick = screen.getByTestId('bulk-move-account-target') as HTMLSelectElement;
    expect(Array.from(pick.options).map((o) => o.textContent)).toEqual([
      'Most headroom on each host',
      'a@example.com',
      'b@example.com',
    ]);
    await fireEvent.change(pick, { target: { value: 'acc-b' } });
    await fireEvent.click(screen.getByTestId('bulk-move-account-go'));
    expect(onBulkMoveAccount).toHaveBeenCalledWith('acc-b');
  });

  it('the default is the login with the most headroom', async () => {
    const onBulkMoveAccount = vi.fn();
    render(SidebarFilters, { props: { ...base, onBulkMoveAccount } });
    await fireEvent.click(screen.getByTestId('bulk-move-account'));
    await fireEvent.click(screen.getByTestId('bulk-move-account-go'));
    expect(onBulkMoveAccount).toHaveBeenCalledWith(null);
  });
});
