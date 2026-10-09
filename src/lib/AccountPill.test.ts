// Redesign step 4.3: the account pill on a session row and a palette row
// (New layout only), its levels, and where a click takes you.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => []) }));

import AccountPill from './AccountPill.svelte';
import SessionRowItem from './SessionRowItem.svelte';
import QuickSwitcher from './QuickSwitcher.svelte';
import AccountsPage from './AccountsPage.svelte';
import { accounts } from './accounts';
import { accountUsage } from './account_usage_store';
import { accountsPageRequest } from './account_pill';
import { destination } from './destination';
import { hosts } from './hosts';
import { sessions, type SessionRow } from './sessions';
import { uiDensity, uiLayout } from './prefs';
import { recentSessions } from './quick_switcher';
import { clearSelection } from './selection';
import { hubStatus, STANDALONE } from './hub';
import { resetAccessForTests } from './access';
import { ADMIN, NOW, WORK, fleetAccounts, fleetHosts, session, snapshot } from './hosts_fixture';

const clock = () => NOW;

/** One account at its weekly limit, one at 85% of its 5-hour window. */
// The rows read the wall clock, so the readings are taken a minute ago and
// reset later than both it and the fixture's NOW.
function atLimitUsage() {
  const real = Math.floor(Date.now() / 1000);
  const w = (five: number, week: number) => ({
    five_hour: { utilization: five, resets_at: Math.max(real, NOW) + 3600 },
    seven_day: { utilization: week, resets_at: Math.max(real, NOW) + 3 * 86400 },
    seven_day_opus: null,
    seven_day_sonnet: null,
  });
  return {
    [ADMIN.uuid]: snapshot(ADMIN.uuid, { usage: w(40, 100), fetched_at: real - 60 }),
    [WORK.uuid]: snapshot(WORK.uuid, { usage: w(85, 30), fetched_at: real - 60 }),
  };
}

beforeEach(() => {
  localStorage.clear();
  hosts.set(fleetHosts());
  accounts.set(fleetAccounts());
  accountUsage.set(atLimitUsage());
  hubStatus.set({ ...STANDALONE });
  resetAccessForTests();
});
afterEach(() => {
  uiLayout.set('classic');
  destination.set('session');
  accountsPageRequest.set(null);
});

describe('AccountPill', () => {
  it('is red at the limit and amber from 80% used, and a click opens its account', async () => {
    render(AccountPill, { uuid: ADMIN.uuid, clock });
    const pill = screen.getByTestId('account-pill');
    expect(pill.getAttribute('data-level')).toBe('limit');
    expect(pill.textContent).toBe(`${ADMIN.email} LIMIT`);
    await fireEvent.click(pill);
    expect(get(destination)).toBe('accounts');
    expect(get(accountsPageRequest)).toBe(ADMIN.uuid);
  });

  it('drops LIMIT once its window resets, without a new reading', async () => {
    vi.useFakeTimers();
    try {
      const reset = NOW + 60;
      accountUsage.set({
        [ADMIN.uuid]: snapshot(ADMIN.uuid, {
          usage: {
            five_hour: { utilization: 100, resets_at: reset },
            seven_day: { utilization: 10, resets_at: NOW + 3 * 86400 },
            seven_day_opus: null,
            seven_day_sonnet: null,
          },
          fetched_at: NOW - 60,
        }),
      });
      let now = NOW;
      render(AccountPill, { uuid: ADMIN.uuid, clock: () => now });
      const pill = screen.getByTestId('account-pill');
      expect(pill.getAttribute('data-level')).toBe('limit');
      now = reset + 30;
      await vi.advanceTimersByTimeAsync(30_000);
      await tick();
      expect(pill.getAttribute('data-level')).not.toBe('limit');
      expect(pill.textContent).not.toContain('LIMIT');
    } finally {
      vi.useRealTimers();
    }
  });

  it('is amber at 85% used', () => {
    render(AccountPill, { uuid: WORK.uuid, clock });
    const pill = screen.getByTestId('account-pill');
    expect(pill.getAttribute('data-level')).toBe('warn');
    expect(pill.textContent).toBe(`${WORK.email} 15%`);
  });
});

function rowProps(sess: SessionRow) {
  const noop = () => {};
  return {
    sess,
    selectMode: false,
    isChecked: false,
    isRenaming: false,
    renameValue: '',
    renameInput: undefined,
    renameError: null,
    relatedCount: 0,
    nowSec: NOW,
    onSelectSession: vi.fn(),
    onKeySession: noop,
    toggleSelected: noop,
    beginRename: noop,
    beginLabelEdit: noop,
    onRenameKey: noop,
    commitRename: noop,
    askRecreate: noop,
    askRestart: noop,
    askKill: noop,
  };
}

describe('the session row', () => {
  const row = session('mefistos', 'dev-a', { id: 7, account_uuid: ADMIN.uuid });

  it('Classic shows no account pill', () => {
    render(SessionRowItem, { props: rowProps(row) });
    expect(screen.queryByTestId('account-pill')).toBeNull();
  });

  it('New shows it, and its click does not select the row', async () => {
    uiLayout.set('new');
    const props = rowProps(row);
    render(SessionRowItem, { props });
    const pill = screen.getByTestId('account-pill');
    expect(pill.getAttribute('data-level')).toBe('limit');
    await fireEvent.click(pill);
    expect(props.onSelectSession).not.toHaveBeenCalled();
    expect(get(destination)).toBe('accounts');
  });

  it('a limit-paused row says why and offers Switch and Wait in Comfortable too', () => {
    uiLayout.set('new');
    uiDensity.set('comfortable');
    render(SessionRowItem, { props: rowProps({ ...row, claude_status: 'idle' }) });
    expect(screen.getByTestId('sess-row').getAttribute('data-bucket')).toBe('account_limit');
    expect(screen.getByTestId('row-blocked-reason').textContent).toContain('Paused · weekly limit on');
    expect(screen.getByTestId('limit-switch')).toBeTruthy();
    expect(screen.getByTestId('limit-wait')).toBeTruthy();
  });

  it('a row with no account has none', () => {
    uiLayout.set('new');
    render(SessionRowItem, { props: rowProps({ ...row, account_uuid: null }) });
    expect(screen.queryByTestId('account-pill')).toBeNull();
  });
});

describe('the palette row', () => {
  beforeEach(() => {
    recentSessions.set([]);
    clearSelection();
    sessions.set([
      session('mefistos', 'dev-a', { id: 1, account_uuid: ADMIN.uuid }),
      session('mefistos', 'dev-b', { id: 2, account_uuid: WORK.uuid }),
    ]);
  });

  async function openSwitcher() {
    await fireEvent.keyDown(window, { key: 'K', ctrlKey: true, shiftKey: true });
    await tick();
  }

  it('Classic shows no account pill', async () => {
    render(QuickSwitcher);
    await openSwitcher();
    expect(screen.queryAllByTestId('switcher-account-pill')).toHaveLength(0);
  });

  it('New shows one per session row; a click opens the account and closes the palette', async () => {
    uiLayout.set('new');
    render(QuickSwitcher);
    await openSwitcher();
    const pills = screen.getAllByTestId('switcher-account-pill');
    expect(pills.map((p) => p.getAttribute('data-level')).sort()).toEqual(['limit', 'warn']);
    const limit = pills.find((p) => p.getAttribute('data-level') === 'limit')!;
    await fireEvent.click(limit);
    await tick();
    expect(get(destination)).toBe('accounts');
    expect(get(accountsPageRequest)).toBe(ADMIN.uuid);
    expect(screen.queryByTestId('quick-switcher')).toBeNull();
  });
});

describe('the Accounts page', () => {
  it('shows the account a pill asked for', async () => {
    sessions.set([]);
    accountsPageRequest.set(WORK.uuid);
    render(AccountsPage, { clock, locale: 'en-GB', timeZone: 'UTC' });
    await waitFor(() =>
      expect(screen.getByTestId('account-detail').textContent).toContain(WORK.email!),
    );
    expect(get(accountsPageRequest)).toBeNull();
  });
});
