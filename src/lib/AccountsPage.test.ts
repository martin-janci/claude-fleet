import { waitingOut } from './account_limits';
import { render, screen, fireEvent, waitFor, within } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import AccountsPage from './AccountsPage.svelte';
import { get } from 'svelte/store';
import { hostsViewRequest } from './app_views';
import { accountsPageRequest, accountsPausedRequest, openPausedSessions } from './account_pill';
import { hosts } from './hosts';
import { accounts } from './accounts';
import { sessions } from './sessions';
import { accountUsage } from './account_usage_store';
import { hubStatus } from './hub';
import {
  ADMIN,
  WORK,
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

describe('AccountsPage: spend, routines and paused sessions (4.1 / 4.2)', () => {
  const FAR = Math.floor(Date.now() / 1000) + 3 * HOUR;
  const atLimit = () => ({
    ...fleetUsage(),
    [ADMIN.uuid]: snapshot(ADMIN.uuid, {
      source_host: 'mefistos',
      usage: {
        five_hour: { utilization: 100, resets_at: FAR },
        seven_day: { utilization: 60, resets_at: FAR + 86400 },
        seven_day_opus: null,
        seven_day_sonnet: null,
      },
    }),
  });

  beforeEach(() => {
    const base = inv.getMockImplementation() as (cmd: string, payload?: unknown) => Promise<unknown>;
    inv.mockImplementation(async (cmd: string, payload?: { args?: Record<string, unknown> }) => {
      if (cmd === 'account_spend') {
        return [
          { account_uuid: ADMIN.uuid, cost_micros: 18_400_000, models: [], by_day: [] },
          { account_uuid: WORK.uuid, cost_micros: 1_250_000, models: [], by_day: [] },
        ];
      }
      if (cmd === 'routines') {
        return [
          { id: 1, name: 'nightly', enabled: true, host_alias: 'mefistos', project_id: 1, prompt: 'x', trigger: 'cron', utc_offset_min: 0, overlap: 'skip', skip_next: false, created_at: 1, updated_at: 1 },
          { id: 2, name: 'weekly', enabled: true, host_alias: 'claude-fleet-oci', project_id: 1, prompt: 'x', trigger: 'cron', utc_offset_min: 0, overlap: 'skip', skip_next: false, created_at: 1, updated_at: 1 },
          { id: 3, name: 'off', enabled: false, host_alias: 'mefistos', project_id: 1, prompt: 'x', trigger: 'cron', utc_offset_min: 0, overlap: 'skip', skip_next: false, created_at: 1, updated_at: 1 },
        ];
      }
      if (cmd === 'check_account_headroom') {
        const admin = { profile: null, account_uuid: ADMIN.uuid, used_pct: 100 };
        const work = { profile: 'work', account_uuid: WORK.uuid, used_pct: 12 };
        return { pause_at_pct: 90, chosen: admin, over: true, suggestion: work, logins: [admin, work] };
      }
      if (cmd === 'restart_session') return session(String(payload?.args?.host_alias), String(payload?.args?.name), { account_uuid: WORK.uuid });
      return base(cmd, payload);
    });
  });

  it('each card says its sessions, switched-on routines and spend today', async () => {
    render(AccountsPage, props);
    await waitFor(() => expect(screen.getAllByTestId('account-counts')[0].textContent).toContain('$18.40 today'));
    expect(screen.getAllByTestId('account-counts')[0].textContent?.trim()).toBe('2 sessions · 2 routines · $18.40 today');
    expect(screen.getAllByTestId('account-counts')[1].textContent).toContain('$1.25 today');
    expect(screen.getByTestId('account-spend').textContent).toBe('$18.40 today');
    expect(inv.mock.calls.find((c) => c[0] === 'account_spend')?.[1]).toEqual({ args: { since: NOW } });
  });

  it('a paired desktop does not ask for spend (the roll-up is local only)', async () => {
    hubStatus.update((h) => ({ ...h, remote: true }));
    try {
      render(AccountsPage, props);
      await waitFor(() => expect(inv.mock.calls.some((c) => c[0] === 'routines')).toBe(true));
      expect(inv.mock.calls.some((c) => c[0] === 'account_spend')).toBe(false);
      expect(screen.getAllByTestId('account-counts')[0].textContent).not.toContain('today');
    } finally {
      hubStatus.update((h) => ({ ...h, remote: false }));
    }
  });

  it('an account at its limit offers Show and Switch to the account with headroom; nothing moves before the click', async () => {
    accountUsage.set(atLimit());
    render(AccountsPage, props);
    const paused = await screen.findByTestId('account-paused');
    expect(paused.textContent).toContain('2 paused sessions → Show');
    const sw = await screen.findByTestId('account-paused-switch');
    expect(sw.textContent).toBe(`Switch to ${WORK.email}`);
    expect(inv.mock.calls.some((c) => c[0] === 'restart_session')).toBe(false);

    await fireEvent.click(screen.getByTestId('account-paused-show'));
    expect(screen.getByTestId('account-detail').textContent).toContain('Paused by its limit');
    expect(within(screen.getByTestId('account-sessions')).getAllByRole('listitem')).toHaveLength(2);
    // M15 G7.12: each paused row answers on the spot, Switch account or Wait.
    expect(within(screen.getByTestId('account-sessions')).getAllByText(/Paused · limit/)).toHaveLength(2);

    await fireEvent.click(sw);
    await waitFor(() => expect(inv.mock.calls.filter((c) => c[0] === 'restart_session')).toHaveLength(2));
    expect(inv.mock.calls.find((c) => c[0] === 'restart_session')?.[1]).toEqual({
      args: { host_alias: 'mefistos', name: 'admin-1', profile: 'work' },
    });
  });

  it('the limit-hit toast\'s Show paused sessions opens the account narrowed to its paused sessions (G4.8)', async () => {
    accountUsage.set(atLimit());
    openPausedSessions(ADMIN.uuid);
    render(AccountsPage, props);
    await waitFor(() => expect(screen.getByTestId('account-detail').textContent).toContain('Paused by its limit'));
    expect(within(screen.getByTestId('account-sessions')).getAllByRole('listitem')).toHaveLength(2);
    expect(get(accountsPausedRequest)).toBe(false);
    await fireEvent.click(screen.getByTestId('account-paused-all'));
    expect(screen.getByTestId('account-detail').textContent).toContain('Sessions on it');
  });

  it('Wait until puts every paused session of the account on wait (M15 G7.12)', async () => {
    accountUsage.set(atLimit());
    render(AccountsPage, props);
    const wait = await screen.findByTestId('account-paused-wait');
    expect(wait.textContent).toContain('Wait until');
    await fireEvent.click(wait);
    expect(get(waitingOut).size).toBe(2);
    expect(inv.mock.calls.some((c) => c[0] === 'restart_session')).toBe(false);
  });

  it('the header reads when usage was refreshed, and Refresh reads every account again (M15 G7.12)', async () => {
    render(AccountsPage, props);
    const btn = screen.getByTestId('accounts-refresh-all');
    expect(screen.getByTestId('accounts-count').textContent).toMatch(/usage refreshed/);
    await fireEvent.click(btn);
    await waitFor(() => expect(inv.mock.calls.filter((c) => c[0] === 'refresh_account_usage').length).toBeGreaterThan(1));
  });

  it('an account under its limit shows no paused line', () => {
    render(AccountsPage, props);
    expect(screen.queryByTestId('account-paused')).toBeNull();
  });
});
