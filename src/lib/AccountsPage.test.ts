import { render, screen, fireEvent, waitFor, within } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import AccountsPage from './AccountsPage.svelte';
import { get } from 'svelte/store';
import { hostsViewRequest } from './app_views';
import { accountsPageRequest } from './account_pill';
import { hosts } from './hosts';
import { accounts } from './accounts';
import { sessions } from './sessions';
import { accountUsage } from './account_usage_store';
import {
  ADMIN,
  HOUR,
  NOW,
  SPARE,
  fleetAccounts,
  fleetHosts,
  fleetUsage,
  RESET_WEEK,
  session,
  snapshot,
} from './hosts_fixture';
import type { UsageSnapshotRow } from './accounts_page';

const inv = mockedInvoke as unknown as ReturnType<typeof vi.fn>;

function hist(at: number, five: number, week: number): UsageSnapshotRow {
  return {
    account_uuid: ADMIN.uuid,
    fetched_at: at,
    usage: {
      five_hour: { utilization: five, resets_at: null },
      seven_day: { utilization: week, resets_at: null },
      seven_day_opus: null,
      seven_day_sonnet: null,
    },
    subscription: 'max',
    source_host: 'mefistos',
  };
}

beforeEach(() => {
  hosts.set(fleetHosts());
  accounts.set(fleetAccounts());
  sessions.set([
    session('mefistos', 'admin-1', { account_uuid: ADMIN.uuid }),
    session('claude-fleet-oci', 'admin-2', { account_uuid: ADMIN.uuid }),
  ]);
  accountUsage.set(fleetUsage());
  inv.mockReset();
  inv.mockImplementation(async (cmd: string, payload?: { args?: { account_uuid?: string } }) => {
    if (cmd === 'account_usage_history') {
      return payload?.args?.account_uuid === ADMIN.uuid
        ? [hist(NOW - 3 * HOUR, 20, 30), hist(NOW - HOUR, 70, 40), hist(NOW, 91, 42)]
        : [];
    }
    if (cmd === 'refresh_account_usage') return fleetUsage()[payload!.args!.account_uuid!];
    return null;
  });
});

const props = { clock: () => NOW, locale: 'en-GB', timeZone: 'UTC' };

describe('AccountsPage', () => {
  it('renders one card per known account from the usage snapshots', () => {
    render(AccountsPage, props);
    const cards = screen.getAllByTestId('account-card');
    expect(cards).toHaveLength(4);
    expect(screen.getByTestId('accounts-count').textContent).toContain('4 accounts');
    // fleetUsage: 9% used of 5 h, 42% of the week.
    expect(cards[0].textContent).toContain('91% left');
    expect(cards[0].textContent).toContain('58% left');
    expect(cards[0].textContent).toContain('Max');
  });

  it('shows the picked account’s windows, reset times, history, logins and sessions', async () => {
    render(AccountsPage, props);
    const admin = screen
      .getAllByTestId('account-card')
      .find((c) => c.getAttribute('data-account') === ADMIN.uuid)!;
    await fireEvent.click(admin);
    const detail = screen.getByTestId('account-detail');
    expect(detail.textContent).toContain(ADMIN.email!);
    expect(within(detail).getByTestId('account-reset-5h').textContent).toBe('resets in 38 min (15:10)');
    expect(within(detail).getByTestId('account-reset-weekly').textContent).toContain('Thu 09:00');
    await waitFor(() => expect(within(detail).getByTestId('account-history-5h')).toBeTruthy());
    expect(detail.textContent).toContain('peaked at 91% used');
    expect(inv.mock.calls.find((c) => c[0] === 'account_usage_history')).toBeTruthy();
    const logins = within(detail).getByTestId('account-logins');
    expect(logins.textContent).toContain('mefistos');
    expect(logins.textContent).toContain('claude-fleet-oci');
    const s = within(detail).getByTestId('account-sessions');
    expect(s.textContent).toContain('admin-1');
    expect(s.textContent).toContain('admin-2');
  });

  it('review r08: a login host and All hosts open the Hosts view', async () => {
    render(AccountsPage, props);
    const admin = screen.getAllByTestId('account-card').find((c) => c.getAttribute('data-account') === ADMIN.uuid)!;
    await fireEvent.click(admin);
    const host = within(screen.getByTestId('account-logins'))
      .getAllByTestId('account-login-host')
      .find((b) => b.textContent === 'mefistos')!;
    await fireEvent.click(host);
    expect(get(hostsViewRequest)).toEqual({ host: 'mefistos' });
    await fireEvent.click(screen.getByTestId('accounts-all-hosts'));
    expect(get(hostsViewRequest)).toEqual({ host: null });
    hostsViewRequest.set(null);
  });

  it('says so when an account has no reading and no history', async () => {
    render(AccountsPage, props);
    const spare = screen
      .getAllByTestId('account-card')
      .find((c) => c.getAttribute('data-account') === SPARE.uuid)!;
    await fireEvent.click(spare);
    const detail = screen.getByTestId('account-detail');
    expect(within(detail).getByTestId('account-window-5h').textContent).toContain('no reading yet');
    await waitFor(() => expect(within(detail).getByTestId('account-history-empty-5h')).toBeTruthy());
    expect(detail.textContent).toContain('No host is logged in to this account right now.');
  });

  it('withholds a window past its reset: ? left, no LIMIT badge', async () => {
    accountUsage.set({
      ...fleetUsage(),
      [ADMIN.uuid]: snapshot(ADMIN.uuid, {
        usage: {
          five_hour: { utilization: 100, resets_at: NOW - 60 },
          seven_day: { utilization: 42, resets_at: RESET_WEEK },
          seven_day_opus: null,
          seven_day_sonnet: null,
        },
      }),
    });
    render(AccountsPage, props);
    const admin = screen
      .getAllByTestId('account-card')
      .find((c) => c.getAttribute('data-account') === ADMIN.uuid)!;
    expect(admin.textContent).toContain('? left');
    expect(admin.textContent).not.toContain('0% left');
    await fireEvent.click(admin);
    const five = within(screen.getByTestId('account-detail')).getByTestId('account-window-5h');
    expect(five.textContent).toContain('? left');
    expect(five.textContent).not.toContain('LIMIT');
    expect(five.getAttribute('data-level')).toBe('none');
    expect(within(screen.getByTestId('account-detail')).getByTestId('account-window-weekly').textContent).toContain(
      '58% left',
    );
  });

  it('Refresh asks for the picked account only', async () => {
    render(AccountsPage, props);
    await fireEvent.click(screen.getByTestId('account-refresh'));
    const refreshes = inv.mock.calls.filter((c) => c[0] === 'refresh_account_usage');
    expect(refreshes).toHaveLength(1);
  });

  it('review r05: a pill for an account not in the list shows no other account', async () => {
    render(AccountsPage, props);
    accountsPageRequest.set('no-such-account-uuid');
    await waitFor(() => expect(screen.getByTestId('account-missing')).toBeTruthy());
    expect(screen.queryByTestId('account-detail')).toBeNull();
    for (const c of screen.getAllByTestId('account-card')) {
      expect(c.getAttribute('aria-selected')).toBe('false');
    }
    // Picking one by hand afterwards shows it.
    await fireEvent.click(screen.getAllByTestId('account-card')[0]);
    expect(screen.getByTestId('account-detail')).toBeTruthy();
    expect(screen.queryByTestId('account-missing')).toBeNull();
  });

  it('has an empty state with no accounts', () => {
    accounts.set([]);
    render(AccountsPage, props);
    expect(screen.getByTestId('accounts-empty')).toBeTruthy();
  });
});
