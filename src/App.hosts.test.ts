// App integration of the Hosts view: the ⌘I / Ctrl+Shift+H toggle, ⌘, for
// Settings, leaving the view, focus restore, the terminal surviving a round
// trip, Files/Hosts exclusivity, the entry points that open it, and the
// footer's usage segment.
import { render, screen, fireEvent, waitFor, within } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';
import App from './App.svelte';
import { onboardingDismissed, onboardingWelcomed } from './lib/onboarding';
import { clearToasts } from './lib/toasts';
import { clearSelection, selectSession, selectedSession } from './lib/selection';
import { hostFilter } from './lib/hosts';
import { addProjectRequest, hostsViewOpen, settingsOpen } from './lib/app_views';
import { clearNewSessionRequest, requestNewSession } from './lib/new_session_request';
import { operatorState } from './lib/operator';
import { destination } from './lib/destination';
import type { SessionRow } from './lib/sessions';
import type { AccountUsageSnapshot } from './lib/account_usage_store';
import { clock } from './lib/account_usage';
import {
  ADMIN,
  GMAIL,
  HOUR,
  MIN,
  NOW,
  RESET_5H,
  RESET_WEEK,
  SPARE,
  WORK,
  fleetAccounts,
  fleetHosts,
  fleetUsage,
  outageUsage,
  session,
  snapshot,
} from './lib/hosts_fixture';

const project = {
  project: { id: 7, owner: 'martin-janci', repo: 'claude-fleet', base_path: '/r/cf', last_session_at: 1, adopted: false, system: false },
  worktrees: [{ id: 71, project_id: 7, host_alias: 'local', name: 'main', path: '/r/cf', branch: 'main' }],
};

let rows: SessionRow[] = [];
let original: ((cmd: string, ...rest: unknown[]) => Promise<unknown>) | undefined;
let inv: ReturnType<typeof vi.fn>;

beforeEach(async () => {
  onboardingDismissed.set(true);
  onboardingWelcomed.set(true);
  clearToasts();
  clearSelection();
  hostFilter.set('all');
  settingsOpen.set(false);
  operatorState.set('unknown');
  destination.set('session');
  localStorage.clear();
  rows = [
    session('mefistos', 'dev-mef', { project_id: null }),
    session('claude-fleet-trn', 'dev-trn', { project_id: null }),
  ];
  const { invoke } = await import('@tauri-apps/api/core');
  inv = invoke as ReturnType<typeof vi.fn>;
  original = inv.getMockImplementation() as typeof original;
  inv.mockImplementation(async (cmd: string, ...rest: unknown[]) => {
    switch (cmd) {
      case 'list_hosts':
        return fleetHosts();
      case 'list_accounts':
        return fleetAccounts();
      case 'list_sessions':
        return rows;
      case 'list_projects':
        return [project];
      case 'list_account_usage':
        return Object.values(fleetUsage());
      case 'refresh_account_usage':
        return fleetUsage()[(rest[0] as { args: { account_uuid: string } }).args.account_uuid];
      case 'list_host_tokens':
        return [];
      case 'check_local_prereqs':
        return { tmux: true, claude: true, git: true, ssh: true };
      case 'tunnel_status':
      case 'repo_changes':
        return [];
      case 'repo_tree':
        return { entries: [], truncated: false };
      default:
        return original ? original(cmd, ...rest) : null;
    }
  });
});

afterEach(() => {
  inv.mockImplementation(original!);
  clearSelection();
  // App's New session dialog follows this store; a test that opens it must
  // not leave it open for the next mount.
  clearNewSessionRequest();
  settingsOpen.set(false);
  addProjectRequest.set(null);
  onboardingDismissed.set(true);
});

async function mountApp() {
  const r = render(App);
  // Bootstrap lands the stores.
  await waitFor(() => expect(screen.getAllByTestId('sess-row').length).toBe(2));
  return r;
}

async function openSession(s: SessionRow): Promise<HTMLElement> {
  selectSession(s);
  const grid = await screen.findByTestId('terminal-host');
  grid.focus();
  // The grid hands focus straight to the terminal's hidden IME proxy, which
  // lives inside it (F9) — so assert focus is in the terminal, not on the div.
  expect(grid.contains(document.activeElement)).toBe(true);
  return grid;
}

const hostsView = () => screen.queryByTestId('hosts-view');
/** With no session selected the view opens on the Hosts table (4.6); Enter
 *  opens the selected host in the list-and-detail. */
async function openList() {
  await fireEvent.keyDown(screen.getByTestId('hosts-table'), { key: 'Enter' });
  await tick();
  await tick();
}
async function openHostFromTable(alias: string) {
  const row = screen.getAllByTestId('hosts-table-row').find((r) => r.dataset.alias === alias)!;
  await fireEvent.click(row);
  await tick();
  await openList();
}
const cmdI = (el: Element | Window = document.activeElement ?? document.body) =>
  fireEvent.keyDown(el, { key: 'i', metaKey: true });

describe('App: the Hosts view', () => {
  it('⌘I toggles Hosts while the terminal has focus, and never reaches the PTY', async () => {
    await mountApp();
    const grid = await openSession(rows[0]);
    const writes = () => inv.mock.calls.filter((c) => c[0] === 'pty_write').length;
    const before = writes();
    await cmdI(grid);
    await tick();
    expect(hostsView()).not.toBeNull();
    expect(get(hostsViewOpen)).toBe(true);
    expect(screen.getByTestId('rail-accounts').getAttribute('aria-current')).toBe('page');
    expect(screen.getByTestId('stab-conversation').getAttribute('aria-selected')).toBe('false');
    // ⌘I again, from inside the view.
    await cmdI(screen.getByTestId('hosts-list'));
    await tick();
    expect(hostsView()).toBeNull();
    expect(writes()).toBe(before);
  });

  it('Ctrl+Shift+H toggles Hosts on non-mac, and the rail item names that chord', async () => {
    await mountApp();
    expect(screen.getByTestId('rail-accounts').title).toContain('Ctrl+Shift+H');
    await fireEvent.keyDown(window, { key: 'H', ctrlKey: true, shiftKey: true });
    await tick();
    expect(hostsView()).not.toBeNull();
    await fireEvent.keyDown(window, { key: 'H', ctrlKey: true, shiftKey: true });
    await tick();
    expect(hostsView()).toBeNull();
  });

  it('Esc in the view steps back to the table, then closes it and restores focus to the terminal', async () => {
    await mountApp();
    const grid = await openSession(rows[0]);
    await cmdI(grid);
    await tick();
    const list = screen.getByTestId('hosts-list');
    expect(document.activeElement).toBe(list);
    await fireEvent.keyDown(list, { key: 'Escape' });
    await tick();
    await tick();
    const table = screen.getByTestId('hosts-table');
    expect(hostsView()).not.toBeNull();
    await fireEvent.keyDown(table, { key: 'Escape' });
    await tick();
    await tick();
    expect(hostsView()).toBeNull();
    expect(grid.contains(document.activeElement)).toBe(true);
  });

  it('Esc inside an input in the view does not close it', async () => {
    await mountApp();
    await cmdI(window);
    await tick();
    await openList();
    const filter = screen.getByTestId('hosts-filter');
    filter.focus();
    await fireEvent.input(filter, { target: { value: 'mef' } });
    await fireEvent.keyDown(filter, { key: 'Escape' });
    await tick();
    expect(hostsView()).not.toBeNull();
  });

  it('TerminalView is not unmounted across an open and close', async () => {
    await mountApp();
    const grid = await openSession(rows[0]);
    const opens = () => inv.mock.calls.filter((c) => c[0] === 'pty_open').length;
    const closes = () => inv.mock.calls.filter((c) => c[0] === 'pty_close').length;
    await waitFor(() => expect(opens()).toBeGreaterThan(0));
    const [o, c] = [opens(), closes()];
    await cmdI(grid);
    await tick();
    expect(hostsView()).not.toBeNull();
    // Covered, not removed.
    expect(grid.isConnected).toBe(true);
    await fireEvent.click(screen.getByTestId('rail-sessions'));
    await tick();
    expect(hostsView()).toBeNull();
    expect(screen.getByTestId('terminal-host')).toBe(grid);
    expect(opens()).toBe(o);
    expect(closes()).toBe(c);
  });

  it('selecting a session in the sidebar closes the view and shows that session', async () => {
    await mountApp();
    const grid = await openSession(rows[0]);
    await cmdI(grid);
    await tick();
    const trn = screen.getAllByTestId('sess-row').find((r) => r.textContent?.includes('dev-trn'))!;
    await fireEvent.click(trn);
    await tick();
    expect(hostsView()).toBeNull();
    expect(get(selectedSession)?.tmux_name).toBe('dev-trn');
  });

  it('clicking the already-open session in the sidebar while Hosts shows goes to it instead of deselecting', async () => {
    await mountApp();
    const grid = await openSession(rows[0]);
    await cmdI(grid);
    await tick();
    const mef = screen.getAllByTestId('sess-row').find((r) => r.textContent?.includes('dev-mef'))!;
    await fireEvent.click(mef);
    await tick();
    expect(hostsView()).toBeNull();
    expect(get(selectedSession)?.tmux_name).toBe('dev-mef');
  });

  it('a session row in the Hosts detail closes the view', async () => {
    await mountApp();
    await cmdI(window);
    await tick();
    // No session selected: the view opens on the table; open trn from it.
    await openHostFromTable('claude-fleet-trn');
    const list = screen.getByTestId('hosts-list');
    expect(list.getAttribute('aria-activedescendant')).toContain('claude-fleet-trn');
    await fireEvent.click(within(screen.getByTestId('host-detail')).getByTestId('detail-session'));
    await tick();
    expect(hostsView()).toBeNull();
    expect(get(selectedSession)?.tmux_name).toBe('dev-trn');
  });

  it('preselects the selected session’s host, else the last-viewed host', async () => {
    await mountApp();
    await openSession(rows[1]);
    await cmdI(window);
    await tick();
    expect(screen.getByTestId('host-detail').dataset.alias).toBe('claude-fleet-trn');
    const mef = screen.getAllByTestId('host-row').find((r) => r.dataset.alias === 'mefistos')!;
    await fireEvent.click(mef);
    await tick();
    await cmdI(window);
    await tick();
    clearSelection();
    await tick();
    await cmdI(window);
    await tick();
    expect(screen.getByTestId('host-detail').dataset.alias).toBe('mefistos');
  });

  it('Hosts opens with no session selected; the Sessions rail item leaves the view', async () => {
    await mountApp();
    expect((screen.getByTestId('stab-files') as HTMLButtonElement).disabled).toBe(true);
    await cmdI(window);
    await tick();
    expect(hostsView()).not.toBeNull();
    await fireEvent.click(screen.getByTestId('rail-sessions'));
    await tick();
    expect(hostsView()).toBeNull();
  });

  it('Files and Hosts are mutually exclusive', async () => {
    await mountApp();
    const grid = await openSession(rows[0]);
    const selected = (id: string) => screen.getByTestId(id).getAttribute('aria-selected');
    await fireEvent.click(screen.getByTestId('stab-files'));
    await tick();
    expect(selected('stab-files')).toBe('true');
    await cmdI(grid);
    await tick();
    expect(hostsView()).not.toBeNull();
    expect(selected('stab-files')).toBe('false');
    expect(screen.getByTestId('rail-accounts').getAttribute('aria-current')).toBe('page');
    await fireEvent.click(screen.getByTestId('stab-files'));
    await tick();
    expect(hostsView()).toBeNull();
    expect(selected('stab-files')).toBe('true');
    expect(screen.getByTestId('rail-accounts').getAttribute('aria-current')).toBeNull();
  });

  it('s filters the sidebar to the host and closes the Hosts overlay', async () => {
    await mountApp();
    await openSession(rows[0]);
    await cmdI(window);
    await tick();
    await fireEvent.keyDown(screen.getByTestId('hosts-list'), { key: 's' });
    await tick();
    expect(get(hostFilter)).toBe('mefistos');
    expect(hostsView()).toBeNull();
  });

  it('View sessions from the detail expands a collapsed sidebar, exactly like the `s` key', async () => {
    // The button and the key are one action (`viewHostSessions`): the filter
    // it sets is worthless behind a collapsed rail, and only the key used to
    // expand it.
    localStorage.setItem('cf:pref:layout.sidebar-collapsed', 'true');
    render(App);
    await waitFor(() => expect(screen.getByTestId('sidebar-expand')).toBeTruthy());
    await cmdI(window);
    await tick();
    await openHostFromTable('mefistos');

    await fireEvent.click(within(screen.getByTestId('host-detail')).getByTestId('detail-view-sessions'));
    await tick();
    expect(hostsView()).toBeNull();
    expect(screen.queryByTestId('sidebar-expand')).toBeNull();
    await waitFor(() => expect(screen.getAllByTestId('sess-row').length).toBeGreaterThan(0));
    expect(get(hostFilter)).toBe('mefistos');
  });

  it('an Add project request from the switcher expands a collapsed sidebar and opens the dialog prefilled', async () => {
    // The dialog is mounted by the Sidebar, which is unmounted behind the
    // collapsed rail: without the expand, the request would sit unanswered.
    localStorage.setItem('cf:pref:layout.sidebar-collapsed', 'true');
    render(App);
    await waitFor(() => expect(screen.getByTestId('sidebar-expand')).toBeTruthy());
    addProjectRequest.set({ cloneUrl: 'o/r' });
    await waitFor(() => expect(screen.getByTestId('add-project-dialog')).toBeTruthy());
    expect((screen.getByTestId('clone-url') as HTMLInputElement).value).toBe('o/r');
    expect(screen.queryByTestId('sidebar-expand')).toBeNull();
    expect(get(addProjectRequest)).toBeNull();
  });

  it('n opens the switcher in New session mode, then New session with that host preselected', async () => {
    await mountApp();
    await openSession(rows[0]);
    await cmdI(window);
    await tick();
    await fireEvent.keyDown(screen.getByTestId('hosts-list'), { key: 'n' });
    await tick();
    await tick();
    expect(await screen.findByTestId('mode-chip')).toBeTruthy();
    // The fixture's projects have no recent session: their group starts folded.
    const list = screen.getByTestId('switcher-list');
    await fireEvent.click(within(list).getByRole('option', { name: /^Forks & others · / }));
    const [pick] = within(list).getAllByRole('option', { name: /^claude-fleet/ });
    await fireEvent.click(pick);
    const dialog = await screen.findByRole('dialog', { name: 'New session' });
    await waitFor(() =>
      expect(dialog.querySelector(".host-pick[aria-pressed='true']")?.getAttribute('data-alias')).toBe('mefistos'),
    );
  });

  it('every entry point lands on the one New session dialog App mounts (redesign 1.9)', async () => {
    // The switcher, a project row's + and the end of Add project all publish
    // `newSessionRequest` (Sidebar.test.ts pins the last two); the Sidebar
    // mounts no copy of its own, so one request is one dialog.
    await mountApp();
    requestNewSession({ project, initialHost: 'local' });
    await screen.findByRole('dialog', { name: 'New session' });
    expect(screen.getAllByRole('dialog', { name: 'New session' })).toHaveLength(1);
  });

  it('Ctrl+, opens Settings off the Mac (redesign 1.9)', async () => {
    await mountApp();
    await fireEvent.keyDown(window, { key: ',', ctrlKey: true });
    await tick();
    expect(await screen.findByRole('dialog', { name: 'Settings' })).toBeTruthy();
  });

  it('⌘, opens Settings, which no longer holds the hosts table', async () => {
    await mountApp();
    await fireEvent.keyDown(window, { key: ',', metaKey: true });
    await tick();
    const dialog = await screen.findByRole('dialog', { name: 'Settings' });
    expect(within(dialog).queryByTestId('hosts-table')).toBeNull();
    expect(within(dialog).getByTestId('settings-hosts-summary').textContent).toBe('5 configured · 1 offline');
  });

  it('⌘E is reserved for the agent — it must not fall through to Settings', async () => {
    const routed = inv.getMockImplementation() as (cmd: string, ...rest: unknown[]) => Promise<unknown>;
    inv.mockImplementation(async (cmd: string, ...rest: unknown[]) => {
      if (cmd === 'operator_status') return { ready: true, session: null, blocked: null };
      return routed(cmd, ...rest);
    });
    await mountApp();
    await fireEvent.keyDown(window, { key: 'e', metaKey: true });
    await tick();
    expect(get(settingsOpen)).toBe(false);
    expect(screen.queryByRole('dialog', { name: 'Settings' })).toBeNull();
  });

  it('⌘E toggles Control\'s agent chat, so it never stays pinned over a view', async () => {
    const routed = inv.getMockImplementation() as (cmd: string, ...rest: unknown[]) => Promise<unknown>;
    inv.mockImplementation(async (cmd: string, ...rest: unknown[]) => {
      if (cmd === 'operator_status') return { ready: true, session: null, blocked: null };
      return routed(cmd, ...rest);
    });
    await mountApp();
    expect(screen.queryByTestId('control-agent')).toBeNull();
    await fireEvent.keyDown(window, { key: 'e', metaKey: true });
    await tick();
    expect(get(destination)).toBe('control');
    expect(screen.getByTestId('control-agent')).toBeTruthy();
    expect(get(settingsOpen)).toBe(false);

    // Press again: back where it was.
    await fireEvent.keyDown(window, { key: 'e', metaKey: true });
    await tick();
    expect(get(destination)).not.toBe('control');
    expect(screen.queryByTestId('control-agent')).toBeNull();
  });

  it('Escape leaves the agent chat from the page behind it', async () => {
    const routed = inv.getMockImplementation() as (cmd: string, ...rest: unknown[]) => Promise<unknown>;
    inv.mockImplementation(async (cmd: string, ...rest: unknown[]) => {
      if (cmd === 'operator_status') return { ready: true, session: null, blocked: null };
      return routed(cmd, ...rest);
    });
    await mountApp();
    await fireEvent.keyDown(window, { key: 'e', metaKey: true });
    await tick();
    expect(get(destination)).toBe('control');
    await fireEvent.keyDown(document.body, { key: 'Escape' });
    await tick();
    expect(get(destination)).not.toBe('control');
  });

  it('Settings → Open Hosts closes Settings and opens the view', async () => {
    await mountApp();
    settingsOpen.set(true);
    const dialog = await screen.findByRole('dialog', { name: 'Settings' });
    await fireEvent.click(within(dialog).getByTestId('settings-open-hosts'));
    await waitFor(() => expect(hostsView()).not.toBeNull());
    expect(screen.queryByRole('dialog', { name: 'Settings' })).toBeNull();
  });

  it('a quick-switcher host entry opens Hosts with that host preselected', async () => {
    await mountApp();
    await fireEvent.keyDown(window, { key: 'K', ctrlKey: true, shiftKey: true });
    const input = await screen.findByTestId('switcher-input');
    await fireEvent.input(input, { target: { value: 'claude-fleet-oci' } });
    await tick();
    const hostRow = await screen.findByTestId('switcher-host');
    expect(hostRow.textContent).toContain('host: claude-fleet-oci');
    await fireEvent.click(hostRow);
    await waitFor(() => expect(hostsView()).not.toBeNull());
    expect(screen.getByTestId('host-detail').dataset.alias).toBe('claude-fleet-oci');
    expect(screen.queryByTestId('quick-switcher')).toBeNull();
  });

  it('the onboarding card’s Add a host opens the Hosts view, not Settings', async () => {
    onboardingDismissed.set(false);
    await mountApp();
    const card = await screen.findByTestId('onboarding-card');
    const step = within(card).getAllByRole('button').find((b) => b.textContent?.includes('Add a host'))!;
    await fireEvent.click(step);
    await waitFor(() => expect(hostsView()).not.toBeNull());
    expect(get(settingsOpen)).toBe(false);
    expect(screen.queryByRole('dialog', { name: 'Settings' })).toBeNull();
  });
});

describe('App: the footer usage segment', () => {
  // Pin the wall clock (Date only, so timers and waitFor keep working) to the
  // fixture's NOW; clock text uses the system locale like the app does.
  async function withUsage(snaps: Record<string, AccountUsageSnapshot>, at = NOW) {
    vi.useFakeTimers({ toFake: ['Date'] });
    vi.setSystemTime(at * 1000);
    const routed = inv.getMockImplementation() as (cmd: string, ...rest: unknown[]) => Promise<unknown>;
    inv.mockImplementation(async (cmd: string, ...rest: unknown[]) => {
      if (cmd === 'list_account_usage') return Object.values(snaps);
      if (cmd === 'refresh_account_usage') {
        return snaps[(rest[0] as { args: { account_uuid: string } }).args.account_uuid];
      }
      return routed(cmd, ...rest);
    });
    await mountApp();
    return waitFor(() => {
      const seg = screen.getByTestId('footer-usage');
      expect(seg.tagName).toBe('BUTTON');
      return seg;
    });
  }

  afterEach(() => {
    vi.useRealTimers();
  });

  const fresh = (): Record<string, AccountUsageSnapshot> => ({
    [ADMIN.uuid]: snapshot(ADMIN.uuid, { source_host: 'mefistos', fetched_at: NOW - 3 * MIN }),
    [WORK.uuid]: snapshot(WORK.uuid, { source_host: 'claude-fleet-htz' }),
    [GMAIL.uuid]: snapshot(GMAIL.uuid, { source_host: 'claude-fleet-trn' }),
    // No host is logged in to SPARE: its dead state must not alarm.
    [SPARE.uuid]: snapshot(SPARE.uuid, { usage: null, fetched_at: null, status: 'no_online_host' }),
  });

  const lowAdmin = (): AccountUsageSnapshot =>
    snapshot(ADMIN.uuid, {
      source_host: 'mefistos',
      usage: {
        five_hour: { utilization: 92, resets_at: RESET_5H },
        seven_day: { utilization: 20, resets_at: RESET_WEEK },
        seven_day_opus: null,
        seven_day_sonnet: null,
      },
    });

  it('all accounts fresh and ok — an account with no hosts does not trigger the alarm', async () => {
    const seg = await withUsage(fresh());
    await waitFor(() => expect(seg.textContent).toBe('usage ✓ all accounts · 3m'));
    expect(seg.dataset.state).toBe('ok');
    expect(seg.getAttribute('aria-label')).toContain('all 3 accounts have headroom');
    // It sits in the footer beside the version line.
    expect(seg.closest('footer')?.textContent).toContain('All systems OK');
  });

  it('names the worst account, and clicking it opens Hosts on that account’s host', async () => {
    const seg = await withUsage({ ...fresh(), [ADMIN.uuid]: lowAdmin() });
    await waitFor(() => expect(seg.dataset.state).toBe('attention'));
    expect(seg.textContent).toBe(`usage ▲ admin-janci@users.noreply.github.com 5h 8% left · resets ${clock(RESET_5H)}`);
    expect(seg.classList.contains('tone-alarm')).toBe(true);
    expect(seg.getAttribute('aria-label')).toMatch(/^Account usage: admin-janci@users\.noreply\.github\.com has 8% of its 5-hour window left, resets in 38 min/);
    await fireEvent.click(seg);
    await waitFor(() => expect(hostsView()).not.toBeNull());
    expect(screen.getByTestId('host-detail').dataset.alias).toBe('mefistos');
  });

  it('every account unavailable: ◷ unavailable since the last good check', async () => {
    const seg = await withUsage(outageUsage());
    await waitFor(() => expect(seg.dataset.state).toBe('unavailable'));
    expect(seg.textContent).toBe(`usage ◷ unavailable since ${clock(NOW - 82 * MIN)}`);
    expect(seg.getAttribute('aria-label')).toBe(`Account usage unavailable since ${clock(NOW - 82 * MIN)}. Open Hosts.`);
  });

  it('after 24 hours unavailable it collapses to a muted `usage off`', async () => {
    const seg = await withUsage(outageUsage(), NOW - 82 * MIN + 25 * HOUR);
    await waitFor(() => expect(seg.dataset.state).toBe('off'));
    expect(seg.textContent).toBe('usage off');
    expect(seg.classList.contains('tone-muted')).toBe(true);
  });
});
