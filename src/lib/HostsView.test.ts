import { render, screen, fireEvent, waitFor, within } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import HostsView from './HostsView.svelte';
import { hosts, resetTombstonesForTests, type HostRow } from './hosts';
import { accounts, type AccountRow } from './accounts';
import { sessions } from './sessions';
import { accountUsage } from './account_usage_store';
import { hostTokens, hostTokensLoaded } from './host_actions';
import { clearToasts, runToastAction, toasts } from './toasts';
import { selectedSession } from './selection';
import {
  ADMIN,
  GMAIL,
  MIN,
  NOW,
  WORK,
  fleetAccounts,
  fleetHosts,
  fleetSessions,
  fleetTokens,
  fleetUsage,
  host,
  outageUsage,
  snapshot,
} from './hosts_fixture';

const inv = mockedInvoke as unknown as ReturnType<typeof vi.fn>;
const calls = (cmd: string) => inv.mock.calls.filter((c) => c[0] === cmd);

beforeEach(() => {
  resetTombstonesForTests();
  clearToasts();
  hosts.set(fleetHosts());
  accounts.set(fleetAccounts());
  sessions.set(fleetSessions());
  accountUsage.set(fleetUsage());
  hostTokens.set(new Map());
  hostTokensLoaded.set(false);
  inv.mockReset();
  inv.mockImplementation(async (cmd: string, payload?: { args?: Record<string, unknown> }) => {
    const args = payload?.args ?? {};
    const hostBy = (alias: unknown) => get(hosts).find((h) => h.alias === alias) as HostRow;
    switch (cmd) {
      case 'list_host_tokens':
        return fleetTokens();
      case 'discover_hosts':
        return [];
      case 'refresh_account_usage':
        return get(accountUsage)[args.account_uuid as string];
      case 'probe_host':
        return hostBy(args.alias);
      case 'hide_host':
        return { ...hostBy(args.alias), hidden: args.hidden };
      case 'remove_host':
        return hostBy(args.alias);
      case 'rotate_host_token':
        return { host_alias: (payload as { hostAlias: string }).hostAlias, mode: 'full', created_at: 2 };
      case 'set_account_nickname': {
        const acc = get(accounts).find((a) => a.uuid === args.uuid) as AccountRow;
        return { ...acc, nickname: args.nickname };
      }
      default:
        return null;
    }
  });
});

afterEach(() => {
  vi.useRealTimers();
});

function mount(props: Partial<{ preselect: string | null; clock: () => number }> = {}) {
  const handlers = { onClose: vi.fn(), onFilterSidebar: vi.fn(), onNewSession: vi.fn() };
  const r = render(HostsView, {
    props: { clock: () => NOW, locale: 'en-GB', timeZone: 'UTC', ...handlers, ...props },
  });
  return { ...r, ...handlers };
}

const list = () => screen.getByTestId('hosts-list');
const detail = () => screen.getByTestId('host-detail');
const selected = () => list().getAttribute('aria-activedescendant');
const detailAlias = () => detail().dataset.alias;
const rowAliases = () => screen.getAllByTestId('host-row').map((r) => r.dataset.alias);
const key = async (el: Element, k: string) => {
  await fireEvent.keyDown(el, { key: k });
  await tick();
};

describe('HostsView: list', () => {
  it('groups by account in a stable order with local under mj.janci@gmail.com', async () => {
    mount();
    await tick();
    const groups = screen.getAllByTestId('hosts-group');
    expect(groups.map((g) => within(g).getByTestId('group-label').textContent)).toEqual([
      'admin@32bit.sk',
      'm.janci@32bit.sk',
      'mj.janci@gmail.com',
    ]);
    expect(within(groups[2]).getAllByTestId('host-row').map((r) => r.dataset.alias)).toEqual([
      'claude-fleet-trn',
      'local',
    ]);
    expect(rowAliases()).toEqual(['claude-fleet-oci', 'mefistos', 'claude-fleet-htz', 'claude-fleet-trn', 'local']);
    // The account no host is logged in to gets no group.
    expect(list().textContent).not.toContain('spare@32bit.sk');
  });

  it('puts hosts without an account in a last "No Claude account" group, and headroom never reorders', async () => {
    hosts.set([...fleetHosts(), host('aaa-nas')]);
    accountUsage.set({
      ...fleetUsage(),
      [ADMIN.uuid]: snapshot(ADMIN.uuid, {
        usage: {
          five_hour: { utilization: 99, resets_at: NOW + 60 * MIN },
          seven_day: { utilization: 99, resets_at: NOW + 86400 },
          seven_day_opus: null,
          seven_day_sonnet: null,
        },
      }),
    });
    mount();
    await tick();
    const labels = screen.getAllByTestId('hosts-group').map((g) => within(g).getByTestId('group-label').textContent);
    expect(labels).toEqual(['admin@32bit.sk', 'm.janci@32bit.sk', 'mj.janci@gmail.com', 'No Claude account']);
    expect(rowAliases().at(-1)).toBe('aaa-nas');
  });

  it('group header shows the tier, both mini bars with % left and reset, and a freshness mark', async () => {
    mount();
    await tick();
    const header = screen.getAllByTestId('hosts-group-header')[0];
    expect(within(header).getByTestId('group-tier').textContent).toBe('max');
    expect(within(header).getByTestId('group-usage-5h').textContent).toMatch(/91% left\s*· resets 15:10/);
    expect(within(header).getByTestId('group-usage-weekly').textContent).toMatch(/58% left\s*· resets Thu 09:00/);
    expect(within(header).getAllByRole('meter')).toHaveLength(2);
    expect(within(header).getByTestId('group-freshness').textContent).toBe('2m');
    const stale = screen.getAllByTestId('hosts-group-header')[1];
    expect(within(stale).getByTestId('group-freshness').textContent).toBe('◷ 14m');
    expect(within(stale).getByTestId('group-usage-5h').textContent).toContain('~91% left');
  });

  it('an offline host row says offline, and rows show session counts', async () => {
    mount();
    await tick();
    const htz = screen.getAllByTestId('host-row').find((r) => r.dataset.alias === 'claude-fleet-htz')!;
    expect(within(htz).getByTestId('host-offline').textContent).toBe('offline');
    expect(htz.textContent).toContain('○');
    const mef = screen.getAllByTestId('host-row').find((r) => r.dataset.alias === 'mefistos')!;
    expect(within(mef).queryByTestId('host-offline')).toBeNull();
    expect(within(mef).getByTestId('host-counts').textContent).toBe('6 ⚡2 ⏸1');
  });

  it('shows one attention mark with an explaining title, and no × or 🚫 in rows', async () => {
    hosts.set(fleetHosts().map((h) => (h.alias === 'claude-fleet-oci' ? { ...h, claude_version: '2.1.140' } : h)));
    mount();
    await waitFor(() => expect(get(hostTokensLoaded)).toBe(true));
    const marks = screen.getAllByTestId('host-attention');
    expect(marks).toHaveLength(1);
    expect(marks[0].dataset.kind).toBe('claude_old');
    expect(marks[0].getAttribute('title')).toContain('2.1.140 on claude-fleet-oci is older than 2.1.145');
    for (const row of screen.getAllByTestId('host-row')) {
      expect(row.querySelector('button')).toBeNull();
      expect(row.textContent).not.toMatch(/[×🚫]/u);
    }
  });

  it('header counts hosts and online hosts', async () => {
    mount();
    await tick();
    expect(screen.getByTestId('hosts-summary').textContent).toBe('5 · 4 online');
    expect(screen.getByTestId('hosts-view').textContent).toContain('usage every 5 min');
  });
});

describe('HostsView: selection and keyboard', () => {
  it('preselects the prop, else the first host needing attention', async () => {
    const a = mount({ preselect: 'local' });
    await tick();
    expect(detailAlias()).toBe('local');
    a.unmount();
    mount();
    await tick();
    // claude-fleet-htz is offline: the first host needing attention.
    expect(detailAlias()).toBe('claude-fleet-htz');
  });

  it('falls back to the first host when nothing needs attention', async () => {
    hosts.set(fleetHosts().map((h) => ({ ...h, reachable: true })));
    mount();
    await tick();
    expect(detailAlias()).toBe('claude-fleet-oci');
  });

  it('focuses the list with aria-activedescendant on the selected row', async () => {
    mount({ preselect: 'mefistos' });
    await tick();
    expect(document.activeElement).toBe(list());
    const id = selected()!;
    expect(document.getElementById(id)?.dataset.alias).toBe('mefistos');
    expect(document.getElementById(id)?.getAttribute('aria-selected')).toBe('true');
  });

  it('↑↓, j k, Home and End move the selection and the detail follows', async () => {
    mount({ preselect: 'mefistos' });
    await tick();
    await key(list(), 'ArrowDown');
    expect(detailAlias()).toBe('claude-fleet-htz');
    await key(list(), 'j');
    expect(detailAlias()).toBe('claude-fleet-trn');
    await key(list(), 'k');
    await key(list(), 'ArrowUp');
    expect(detailAlias()).toBe('mefistos');
    await key(list(), 'End');
    expect(detailAlias()).toBe('local');
    await key(list(), 'ArrowDown');
    expect(detailAlias()).toBe('local');
    await key(list(), 'Home');
    expect(detailAlias()).toBe('claude-fleet-oci');
  });

  it('Enter and → focus the detail; ← and Esc return to the list; Esc in the list closes', async () => {
    const v = mount({ preselect: 'mefistos' });
    await tick();
    await key(list(), 'Enter');
    expect(document.activeElement).toBe(detail());
    await key(detail(), 'ArrowLeft');
    expect(document.activeElement).toBe(list());
    await key(list(), 'ArrowRight');
    expect(document.activeElement).toBe(detail());
    await key(detail(), 'Escape');
    expect(document.activeElement).toBe(list());
    expect(v.onClose).not.toHaveBeenCalled();
    await key(list(), 'Escape');
    expect(v.onClose).toHaveBeenCalledTimes(1);
  });

  it('↑↓ in the detail move between session rows, and a session row selects the session', async () => {
    mount({ preselect: 'mefistos' });
    await tick();
    await key(list(), 'Enter');
    await key(detail(), 'ArrowDown');
    const rows = screen.getAllByTestId('detail-session');
    expect(rows).toHaveLength(6);
    expect(document.activeElement).toBe(rows[0]);
    await key(rows[0], 'j');
    expect(document.activeElement).toBe(rows[1]);
    await fireEvent.click(rows[1]);
    expect(get(selectedSession)?.tmux_name).toBe('mefistos-s02');
  });

  it('r, u, s and n call their handlers for the selected host, from the list and the detail', async () => {
    const v = mount({ preselect: 'mefistos' });
    await tick();
    const refreshesAtMount = calls('refresh_account_usage').length;
    await key(list(), 'r');
    expect(calls('probe_host').at(-1)?.[1]).toEqual({ args: { alias: 'mefistos' } });
    await key(list(), 'u');
    expect(calls('refresh_account_usage')).toHaveLength(refreshesAtMount + 1);
    expect(calls('refresh_account_usage').at(-1)?.[1]).toEqual({ args: { account_uuid: ADMIN.uuid } });
    await key(list(), 's');
    expect(v.onFilterSidebar).toHaveBeenCalledWith('mefistos');
    await key(list(), 'n');
    expect(v.onNewSession).toHaveBeenCalledWith('mefistos');
    await key(list(), 'Enter');
    await key(detail(), 'n');
    expect(v.onNewSession).toHaveBeenCalledTimes(2);
    await key(detail(), 'r');
    expect(calls('probe_host')).toHaveLength(2);
  });

  it('stray letters do nothing (no type-ahead)', async () => {
    const v = mount({ preselect: 'mefistos' });
    await waitFor(() => expect(get(hostTokensLoaded)).toBe(true));
    const before = inv.mock.calls.length;
    for (const k of ['x', 'q', 'l', 'c', 'm', 'R', 'S', 'd', 'h']) await key(list(), k);
    expect(detailAlias()).toBe('mefistos');
    expect(inv.mock.calls.length).toBe(before);
    expect(v.onClose).not.toHaveBeenCalled();
    expect(v.onFilterSidebar).not.toHaveBeenCalled();
    expect(v.onNewSession).not.toHaveBeenCalled();
  });

  it('/ focuses the filter; letters typed there are ignored as shortcuts', async () => {
    const v = mount({ preselect: 'mefistos' });
    await tick();
    await key(list(), '/');
    const filter = screen.getByTestId('hosts-filter') as HTMLInputElement;
    expect(document.activeElement).toBe(filter);
    const before = inv.mock.calls.length;
    for (const k of ['r', 'u', 's', 'n', 'e', 'j', 'k', '?']) await key(filter, k);
    expect(inv.mock.calls.length).toBe(before);
    expect(v.onFilterSidebar).not.toHaveBeenCalled();
    expect(v.onNewSession).not.toHaveBeenCalled();
    expect(screen.queryByTestId('hosts-legend')).toBeNull();
    await fireEvent.input(filter, { target: { value: 'trn' } });
    await tick();
    expect(rowAliases()).toEqual(['claude-fleet-trn']);
    expect(detailAlias()).toBe('claude-fleet-trn');
    await key(filter, 'Escape');
    expect(filter.value).toBe('');
    await key(filter, 'Escape');
    expect(document.activeElement).toBe(list());
    expect(v.onClose).not.toHaveBeenCalled();
  });

  it('? toggles the legend', async () => {
    mount();
    await tick();
    await key(list(), '?');
    expect(screen.getByTestId('hosts-legend').textContent).toContain('filter the sidebar to this host');
    await key(list(), '?');
    expect(screen.queryByTestId('hosts-legend')).toBeNull();
    await fireEvent.click(screen.getByTestId('hosts-legend-toggle'));
    expect(screen.getByTestId('hosts-legend')).toBeInTheDocument();
  });

  it('u inside the floor shows the countdown instead of refreshing', async () => {
    accountUsage.set({ ...fleetUsage(), [ADMIN.uuid]: snapshot(ADMIN.uuid, { next_try_at: NOW + 130 }) });
    mount({ preselect: 'mefistos' });
    await tick();
    const before = calls('refresh_account_usage').length;
    await key(list(), 'u');
    expect(calls('refresh_account_usage')).toHaveLength(before);
    expect(screen.getByTestId('hosts-refusal').textContent).toContain('refresh available in 2:10');
  });

  it('ticks one clock every 30 seconds and stops on destroy', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    let t = NOW;
    const v = mount({ clock: () => t });
    await tick();
    const mark = () => screen.getAllByTestId('group-freshness')[0].textContent;
    expect(mark()).toBe('2m');
    t = NOW + 10 * MIN;
    vi.advanceTimersByTime(30_000);
    await tick();
    expect(mark()).toBe('◷ 12m');
    expect(vi.getTimerCount()).toBe(1);
    v.unmount();
    expect(vi.getTimerCount()).toBe(0);
  });
});

describe('HostsView: nicknames', () => {
  const header = (i = 0) => screen.getAllByTestId('hosts-group-header')[i];

  it('click edits inline; Enter saves', async () => {
    mount({ preselect: 'mefistos' });
    await tick();
    await fireEvent.click(within(header()).getByTestId('group-label'));
    const input = within(header()).getByTestId('group-label-input') as HTMLInputElement;
    expect(document.activeElement).toBe(input);
    input.value = '  work ';
    await key(input, 'Enter');
    expect(calls('set_account_nickname').at(-1)?.[1]).toEqual({ args: { uuid: ADMIN.uuid, nickname: 'work' } });
    await waitFor(() => expect(screen.queryByTestId('group-label-input')).toBeNull());
    expect(screen.getAllByTestId('group-label').map((l) => l.textContent)).toContain('work');
    expect(document.activeElement).toBe(list());
  });

  it('e edits the selected host’s account; Escape cancels without closing the view', async () => {
    const v = mount({ preselect: 'local' });
    await tick();
    await key(list(), 'e');
    const input = screen.getByTestId('group-label-input') as HTMLInputElement;
    expect(input.closest('[data-testid="hosts-group"]')?.getAttribute('data-key')).toBe(GMAIL.uuid);
    input.value = 'personal';
    await key(input, 'Escape');
    expect(screen.queryByTestId('group-label-input')).toBeNull();
    expect(calls('set_account_nickname')).toHaveLength(0);
    expect(v.onClose).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(list());
  });

  it('an empty value clears the nickname', async () => {
    accounts.set(fleetAccounts().map((a) => (a.uuid === WORK.uuid ? { ...a, nickname: 'work' } : a)));
    mount({ preselect: 'claude-fleet-htz' });
    await tick();
    await key(list(), 'e');
    const input = screen.getByTestId('group-label-input') as HTMLInputElement;
    expect(input.value).toBe('work');
    input.value = '   ';
    await key(input, 'Enter');
    expect(calls('set_account_nickname').at(-1)?.[1]).toEqual({ args: { uuid: WORK.uuid, nickname: null } });
    await waitFor(() =>
      expect(screen.getAllByTestId('group-label').map((l) => l.textContent)).toContain('m.janci@32bit.sk'),
    );
  });

  it('e in the detail edits the detail’s account line', async () => {
    mount({ preselect: 'mefistos' });
    await tick();
    await key(list(), 'Enter');
    await key(detail(), 'e');
    expect(within(detail()).getByTestId('detail-nickname-input')).toBeInTheDocument();
    expect(screen.queryByTestId('group-label-input')).toBeNull();
  });
});

describe('HostsView: usage', () => {
  it('the detail’s usage block names the other host on a shared account', async () => {
    mount({ preselect: 'mefistos' });
    await tick();
    expect(within(detail()).getByTestId('usage-shared').textContent).toBe('· shared with claude-fleet-oci');
    await key(list(), 'End');
    expect(detailAlias()).toBe('local');
    expect(within(detail()).getByTestId('usage-shared').textContent).toBe('· shared with claude-fleet-trn');
  });

  it('shows ONE outage banner when every account is unavailable and suppresses the per-block line', async () => {
    accountUsage.set(outageUsage());
    const writeText = vi.fn(async () => {});
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
    mount({ preselect: 'mefistos' });
    await tick();
    const banners = screen.getAllByTestId('usage-outage-banner');
    expect(banners).toHaveLength(1);
    expect(banners[0].textContent).toContain(
      "Usage unavailable since 13:10. Anthropic's usage endpoint returned an unexpected response (HTTP 404). It's undocumented and may have changed. Sessions are unaffected.",
    );
    expect(screen.getByTestId('outage-retry').textContent).toBe('Retry 14:40');
    const lines = within(detail()).queryAllByTestId('usage-message').map((m) => m.dataset.kind);
    expect(lines).not.toContain('unavailable');
    expect(within(detail()).queryByTestId('usage-copy-details')).toBeNull();

    await fireEvent.click(screen.getByTestId('outage-copy'));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith('status: unavailable\ndetail: HTTP 404: <html>Not Found</html>'));
    const before = calls('refresh_account_usage').length;
    await fireEvent.click(screen.getByTestId('outage-retry'));
    expect(calls('refresh_account_usage')).toHaveLength(before + 3);
  });

  it('no banner while one account works; then the unavailable block keeps its own line', async () => {
    accountUsage.set({ ...outageUsage(), [GMAIL.uuid]: snapshot(GMAIL.uuid) });
    mount({ preselect: 'mefistos' });
    await tick();
    expect(screen.queryByTestId('usage-outage-banner')).toBeNull();
    const lines = within(detail()).queryAllByTestId('usage-message').map((m) => m.dataset.kind);
    expect(lines).toContain('unavailable');
  });
});

describe('HostsView: action safety', () => {
  it('Remove host… confirms with Cancel focused, does nothing on cancel, removes on confirm', async () => {
    mount({ preselect: 'mefistos' });
    await tick();
    await fireEvent.click(screen.getByTestId('detail-remove'));
    await tick();
    const dialog = screen.getByTestId('confirm-dialog');
    expect(document.activeElement).toBe(within(dialog).getByTestId('confirm-cancel'));
    expect(dialog.textContent).toContain('Remove mefistos?');
    expect(dialog.textContent).toContain('Fleet deletes its 6 session rows from its database');
    expect(dialog.textContent).toContain('The tmux sessions on mefistos are not touched and keep running');

    await fireEvent.click(within(dialog).getByTestId('confirm-cancel'));
    await tick();
    expect(screen.queryByTestId('confirm-dialog')).toBeNull();
    expect(calls('remove_host')).toHaveLength(0);
    expect(get(hosts).some((h) => h.alias === 'mefistos')).toBe(true);

    await fireEvent.click(screen.getByTestId('detail-remove'));
    await tick();
    await fireEvent.click(screen.getByTestId('confirm-remove'));
    await waitFor(() => expect(calls('remove_host')).toHaveLength(1));
    expect(calls('remove_host')[0][1]).toEqual({ args: { alias: 'mefistos' } });
    await waitFor(() => expect(get(hosts).some((h) => h.alias === 'mefistos')).toBe(false));
    expect(screen.queryByTestId('confirm-dialog')).toBeNull();
    // The selection moves on to a host that still exists.
    expect(rowAliases()).not.toContain('mefistos');
    expect(get(hosts).some((h) => h.alias === detailAlias())).toBe(true);
  });

  it('Rotate token… confirms with Cancel focused, does nothing on cancel, rotates on confirm', async () => {
    mount({ preselect: 'mefistos' });
    await waitFor(() => expect(screen.getByTestId('detail-rotate')).toBeInTheDocument());
    await fireEvent.click(screen.getByTestId('detail-rotate'));
    await tick();
    const dialog = screen.getByTestId('confirm-dialog');
    expect(document.activeElement).toBe(within(dialog).getByTestId('confirm-cancel'));
    expect(dialog.textContent).toContain('The old token stops working.');

    await fireEvent.click(within(dialog).getByTestId('confirm-cancel'));
    await tick();
    expect(calls('rotate_host_token')).toHaveLength(0);

    await fireEvent.click(screen.getByTestId('detail-rotate'));
    await tick();
    await fireEvent.click(screen.getByTestId('confirm-rotate'));
    await waitFor(() => expect(calls('rotate_host_token')).toHaveLength(1));
    expect(calls('rotate_host_token')[0][1]).toEqual({ hostAlias: 'mefistos' });
  });

  it('neither Remove nor Rotate has a keyboard shortcut', async () => {
    mount({ preselect: 'mefistos' });
    await waitFor(() => expect(screen.getByTestId('detail-rotate')).toBeInTheDocument());
    const keys = [...'abcdefghijklmnopqrstuvwxyz', ...'ABCDEFGHIJKLMNOPQRSTUVWXYZ', 'Delete', 'Backspace', 'Enter', ' '];
    for (const zone of [list, detail]) {
      for (const k of keys) {
        if (k === 'e') continue; // opens the nickname editor, which would take the keys
        await key(zone(), k);
        if (zone === list && k === 'Enter') await key(detail(), 'ArrowLeft');
      }
      if (zone === list) await key(list(), 'Enter');
    }
    expect(screen.queryByTestId('confirm-dialog')).toBeNull();
    expect(calls('remove_host')).toHaveLength(0);
    expect(calls('rotate_host_token')).toHaveLength(0);
    expect(calls('hide_host')).toHaveLength(0);
  });

  it('keys pressed inside a confirm dialog never reach the view', async () => {
    const v = mount({ preselect: 'mefistos' });
    await tick();
    await fireEvent.click(screen.getByTestId('detail-remove'));
    await tick();
    const cancel = screen.getByTestId('confirm-cancel');
    const before = inv.mock.calls.length;
    for (const k of ['Escape', 'r', 'u', 's', 'n', 'j', 'ArrowDown']) await key(cancel, k);
    expect(v.onClose).not.toHaveBeenCalled();
    expect(v.onFilterSidebar).not.toHaveBeenCalled();
    expect(v.onNewSession).not.toHaveBeenCalled();
    expect(inv.mock.calls.length).toBe(before);
    expect(detailAlias()).toBe('mefistos');
  });

  it('Hide host is immediate with an Undo toast, and Undo shows the host again', async () => {
    mount({ preselect: 'mefistos' });
    await tick();
    await fireEvent.click(screen.getByTestId('detail-hide'));
    await waitFor(() => expect(get(hosts).find((h) => h.alias === 'mefistos')?.hidden).toBe(true));
    expect(calls('hide_host')[0][1]).toEqual({ args: { alias: 'mefistos', hidden: true } });
    expect(screen.queryByTestId('confirm-dialog')).toBeNull();
    const toast = get(toasts).find((t) => t.action?.label === 'Undo');
    expect(toast?.message).toBe('mefistos is hidden from the sidebar.');
    expect(screen.getByTestId('detail-hide').textContent).toBe('Show host');

    runToastAction(toast!.id);
    await waitFor(() => expect(get(hosts).find((h) => h.alias === 'mefistos')?.hidden).toBe(false));
    expect(calls('hide_host')[1][1]).toEqual({ args: { alias: 'mefistos', hidden: false } });
    expect(get(toasts)).toHaveLength(0);
  });

  it('the local host offers no hide or remove', async () => {
    mount({ preselect: 'local' });
    await tick();
    expect(screen.queryByTestId('detail-hide')).toBeNull();
    expect(screen.queryByTestId('detail-remove')).toBeNull();
  });
});

describe('HostsView: detail sections', () => {
  it('renders header, usage, sessions, integration and danger in order', async () => {
    mount({ preselect: 'mefistos' });
    await waitFor(() => expect(screen.getByTestId('detail-token-mode')).toBeInTheDocument());
    const d = detail();
    expect(within(d).getByTestId('detail-alias').textContent).toBe('mefistos');
    expect(within(d).getByTestId('detail-ssh').textContent).toBe('mefistos');
    expect(within(d).getByTestId('detail-status').textContent).toBe('● online');
    expect(within(d).getByTestId('detail-ping').textContent?.trim()).toBe('2m ago');
    expect(d.textContent).toContain('2.1.145');
    expect(d.textContent).toContain('3.5a');
    const order = ['detail-alias', 'usage-block', 'detail-session', 'detail-token-mode', 'detail-hooks', 'detail-rotate', 'detail-hide', 'detail-remove'].map(
      (id) => within(d).getAllByTestId(id)[0],
    );
    for (let i = 1; i < order.length; i++) {
      expect(order[i - 1].compareDocumentPosition(order[i]) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    }
    expect(within(d).getByTestId('detail-hooks').textContent).toMatch(/last event 5m ago/);
  });

  it('+ Add host opens the host picker', async () => {
    mount();
    await tick();
    await fireEvent.click(screen.getByTestId('hosts-add'));
    await tick();
    expect(calls('discover_hosts')).toHaveLength(1);
  });

  it('changing the token mode goes through the shared host actions', async () => {
    mount({ preselect: 'mefistos' });
    await waitFor(() => expect(screen.getByTestId('detail-token-mode')).toBeInTheDocument());
    await fireEvent.change(screen.getByTestId('detail-token-mode'), { target: { value: 'readonly' } });
    await waitFor(() => expect(calls('set_host_token_mode')).toHaveLength(1));
    expect(calls('set_host_token_mode')[0][1]).toEqual({ hostAlias: 'mefistos', mode: 'readonly' });
  });
});

// Moved from SettingsDialog.test.ts when the Settings hosts table was removed:
// every fact that table showed (versions, account + tier, status, token mode,
// hook health) and every action it offered (re-probe, hide, remove, token
// mode, rotate) must still be reachable in the Hosts view.
describe('HostsView: what the former Settings hosts table covered', () => {
  it('the detail shows the account email and the group header its seat tier', async () => {
    accounts.set(fleetAccounts().map((a) => (a.uuid === WORK.uuid ? { ...a, seat_tier: 'max' } : a)));
    mount({ preselect: 'claude-fleet-htz' });
    await tick();
    expect(within(detail()).getByTestId('detail-account').textContent).toContain('m.janci@32bit.sk');
    const header = screen.getAllByTestId('hosts-group-header').find((g) => g.textContent?.includes('m.janci@32bit.sk'))!;
    expect(within(header).getByTestId('group-tier').textContent).toBe('max');
  });

  it('a host missing from the token list reads "none", a provisioned one shows its mode', async () => {
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'list_host_tokens') return [{ host_alias: 'mefistos', mode: 'full', created_at: 1 }];
      return null;
    });
    mount({ preselect: 'claude-fleet-oci' });
    await waitFor(() => expect(get(hostTokensLoaded)).toBe(true));
    expect(detail().textContent).toContain('none — provision hosts to mint one');
    expect(within(detail()).getByTestId('detail-hooks').textContent).toBe('not installed');
    expect(within(detail()).getByTestId('detail-hooks').dataset.state).toBe('not_installed');
    await key(list(), 'j'); // claude-fleet-oci → mefistos (same account group)
    expect(detailAlias()).toBe('mefistos');
    expect((within(detail()).getByTestId('detail-token-mode') as HTMLSelectElement).value).toBe('full');
  });

  it('installed hooks with no Stop event yet read "installed · never seen"', async () => {
    sessions.set([]);
    mount({ preselect: 'mefistos' });
    await waitFor(() => expect(within(detail()).getByTestId('detail-hooks').dataset.state).toBe('never_seen'));
    expect(within(detail()).getByTestId('detail-hooks').textContent).toBe('installed · never seen');
  });

  it('the Re-probe button probes the selected host', async () => {
    mount({ preselect: 'claude-fleet-trn' });
    await tick();
    await fireEvent.click(within(detail()).getByTestId('detail-reprobe'));
    await waitFor(() => expect(calls('probe_host')).toHaveLength(1));
    expect(calls('probe_host')[0][1]).toEqual({ args: { alias: 'claude-fleet-trn' } });
  });
});
