import { render, screen, fireEvent, within } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';
import { clearToasts, toasts } from './toasts';

vi.mock('./sessions', async () => {
  const actual = await vi.importActual<typeof import('./sessions')>('./sessions');
  return {
    ...actual,
    restoreHostSessions: vi.fn(),
    discoverLostSessions: vi.fn(),
    newSessionAbortable: vi.fn(),
    adoptSession: vi.fn(),
  };
});

vi.mock('./lost_found', async () => {
  const actual = await vi.importActual<typeof import('./lost_found')>('./lost_found');
  return { ...actual, lostTarget: vi.fn(), placeTranscript: vi.fn() };
});

vi.mock('./hosts', async () => {
  const actual = await vi.importActual<typeof import('./hosts')>('./hosts');
  return { ...actual, setHostHarnesses: vi.fn() };
});

import HostDetail from './HostDetail.svelte';
import { sharedWith } from './hosts_view';
import { shortAge } from './session_status';
import { hubStatus, STANDALONE } from './hub';
import { UNKNOWN_SESSION_REASON } from './share';
import { hubConnection } from './hub_connection';
import { ADMIN, GMAIL, NOW, fleetHosts, fleetSessions, fleetUsage, host, session } from './hosts_fixture';
import { viewHostSessions } from './host_actions';
import { hostFilter, setHostHarnesses } from './hosts';
import { onHostsCloseRequested } from './app_views';
import { lostTarget, placeTranscript } from './lost_found';
import { projects } from './projects';
import {
  adoptSession,
  restoreHostSessions,
  discoverLostSessions,
  newSessionAbortable,
  type LostCandidate,
  type SessionRow,
} from './sessions';

const mockedRestore = restoreHostSessions as unknown as ReturnType<typeof vi.fn>;
const mockedDiscover = discoverLostSessions as unknown as ReturnType<typeof vi.fn>;
const mockedNewSession = newSessionAbortable as unknown as ReturnType<typeof vi.fn>;
const mockedSetHarnesses = setHostHarnesses as unknown as ReturnType<typeof vi.fn>;
const mockedAdopt = adoptSession as unknown as ReturnType<typeof vi.fn>;
const mockedTarget = lostTarget as unknown as ReturnType<typeof vi.fn>;
const mockedPlace = placeTranscript as unknown as ReturnType<typeof vi.fn>;

function mount(alias: string, over: Record<string, unknown> = {}) {
  const hosts = fleetHosts();
  const h = hosts.find((x) => x.alias === alias) ?? host(alias);
  const acct = h.account_uuid === ADMIN.uuid ? ADMIN : h.account_uuid === GMAIL.uuid ? GMAIL : null;
  const props = {
    host: h,
    account: acct,
    snapshot: h.account_uuid ? fleetUsage()[h.account_uuid] : null,
    sharedWith: sharedWith(h, hosts),
    hostSessions: fleetSessions().filter((s) => s.host_alias === alias),
    token: { host_alias: alias, mode: 'full', created_at: 1 },
    tokensLoaded: true,
    hook: { state: 'seen' as const, lastAt: NOW - 300 },
    attention: null,
    now: NOW,
    locale: 'en-GB',
    timeZone: 'UTC',
    editingNickname: false,
    oneditstart: vi.fn(),
    oneditdone: vi.fn(),
    onreprobe: vi.fn(),
    onrefreshusage: vi.fn(),
    ...over,
  };
  render(HostDetail, { props });
  return props;
}

describe('HostDetail', () => {
  it('shares the account with the other host in the usage block', () => {
    mount('claude-fleet-oci');
    expect(screen.getByTestId('usage-shared').textContent).toBe('· shared with mefistos');
    expect(screen.getByTestId('detail-account').textContent).toContain('admin-janci@users.noreply.github.com');
  });

  it('lists the host’s login profiles with their logins', () => {
    const h = { ...fleetHosts().find((x) => x.alias === 'claude-fleet-oci')! };
    h.claude_profiles = [
      { name: 'work', account_uuid: 'acc-w', email: 'work@example.com' },
      { name: 'fresh', account_uuid: null, email: null },
    ];
    mount('claude-fleet-oci', { host: h });
    const text = screen.getByTestId('detail-profiles').textContent ?? '';
    expect(text).toContain('work');
    expect(text).toContain('work@example.com');
    expect(text).toContain('fresh not logged in');
  });

  it('shows no profiles line for a host without any', () => {
    mount('claude-fleet-oci');
    expect(screen.queryByTestId('detail-profiles')).toBeNull();
  });

  it('a host with no account says so and has no refresh', () => {
    mount('nas', { hostSessions: [] });
    expect(screen.getByTestId('usage-block').textContent).toContain('Not logged in to Claude on this host');
    expect(screen.queryByTestId('usage-refresh')).toBeNull();
    expect(screen.queryByTestId('detail-account')).toBeNull();
  });

  it('the remove confirm counts this host’s session rows, with Cancel focused', async () => {
    mount('claude-fleet-trn');
    await fireEvent.click(screen.getByTestId('detail-remove'));
    await tick();
    const dialog = screen.getByTestId('confirm-dialog');
    expect(dialog.textContent).toContain('Fleet deletes its 14 session rows');
    expect(document.activeElement).toBe(within(dialog).getByTestId('confirm-cancel'));
  });

  it('the re-probe button and the usage refresh call their props', async () => {
    const p = mount('mefistos');
    await fireEvent.click(screen.getByTestId('detail-reprobe'));
    expect(p.onreprobe).toHaveBeenCalled();
    await fireEvent.click(screen.getByTestId('usage-refresh'));
    expect(p.onrefreshusage).toHaveBeenCalled();
  });

  it('no token: no mode control and no Rotate', () => {
    mount('mefistos', { token: null });
    expect(screen.queryByTestId('detail-token-mode')).toBeNull();
    expect(screen.queryByTestId('detail-rotate')).toBeNull();
    expect(screen.getByText('none — provision hosts to mint one')).toBeInTheDocument();
  });

  it('marks an agent-transport host in the facts list; an ssh host stays quiet', () => {
    mount('mefistos', { host: { ...host('mefistos'), transport: 'agent' } });
    expect(screen.getByTestId('detail-transport').textContent).toBe('agent');
  });

  it('an ssh host (the default) shows no transport fact', () => {
    mount('mefistos');
    expect(screen.queryByTestId('detail-transport')).toBeNull();
  });

  it('shows a Health block with the disk meter, versions and their age', () => {
    mount('claude-fleet-trn', {
      host: host('claude-fleet-trn', {
        transport: 'agent',
        agent_version: '0.2.26',
        disk_home_free_kb: 3_600_000,
        disk_home_total_kb: 150_000_000,
        load_1m: 5.25,
        uptime_secs: 144 * 86400,
        claude_version: '2.1.282',
        claude_version_at: NOW - 2 * 3600,
        health_at: NOW - 60,
      }),
    });
    const block = screen.getByTestId('detail-health');
    expect(block.textContent).toContain('disk 98% · 3.4 GB free');
    expect(block.textContent).toContain('load 5.3');
    expect(block.textContent).toContain('up 144d');
    expect(block.textContent).toContain('agent 0.2.26');
    expect(screen.getByTestId('detail-health-meter').getAttribute('data-level')).toBe('crit');
    expect(screen.getByTestId('detail-claude-age').textContent).toBe('checked 2h ago');
  });

  it('the "View sessions" button jumps the sidebar filter and asks to close the Hosts overlay', async () => {
    hostFilter.set('all');
    const onCloseRequested = vi.fn();
    const unsub = onHostsCloseRequested(onCloseRequested);
    mount('mefistos');
    await fireEvent.click(screen.getByTestId('detail-view-sessions'));
    expect(get(hostFilter)).toBe('mefistos');
    expect(onCloseRequested).toHaveBeenCalledTimes(1);
    unsub();
  });
});

describe('viewHostSessions', () => {
  it('sets hostFilter to the alias and fires the Hosts-overlay-close signal', () => {
    hostFilter.set('all');
    const onCloseRequested = vi.fn();
    const unsub = onHostsCloseRequested(onCloseRequested);
    viewHostSessions('mefistos');
    expect(get(hostFilter)).toBe('mefistos');
    expect(onCloseRequested).toHaveBeenCalledTimes(1);
    unsub();
  });
});

function lost(alias: string, name: string, over: Partial<SessionRow> = {}): SessionRow {
  return session(alias, name, { lost_at: NOW - 600, claude_session_id: `cs-${name}`, kind: 'work', ...over });
}

describe('HostDetail restore lost sessions', () => {
  beforeEach(() => {
    mockedRestore.mockReset();
  });

  it('hides the button with no restorable rows, shows a count with some', () => {
    mount('mefistos', { hostSessions: [] });
    expect(screen.queryByTestId('restore-lost')).toBeNull();

    const rows = [lost('mefistos', 'mefistos-a'), lost('mefistos', 'mefistos-b')];
    mount('mefistos', { hostSessions: rows });
    expect(screen.getByTestId('restore-lost').textContent).toBe('Restore 2 lost sessions…');
  });

  it('a bg/external lost session, or one without a claude_session_id, is not restorable', () => {
    const allExcluded = [
      lost('mefistos', 'a', { kind: 'bg' }),
      lost('mefistos', 'b', { kind: 'external' }),
      lost('mefistos', 'c', { claude_session_id: null }),
    ];
    mount('mefistos', { hostSessions: allExcluded });
    expect(screen.queryByTestId('restore-lost')).toBeNull();

    // Mixed with one genuinely restorable row: the count must exclude the
    // three above, proving the filter actually screens them out rather than
    // this test passing merely because the button vanishes for other reasons.
    const mixed = [...allExcluded, lost('mefistos', 'd')];
    mount('mefistos', { hostSessions: mixed });
    expect(screen.getByTestId('restore-lost').textContent).toBe('Restore 1 lost session…');
  });

  it('clicking runs a dry run and the dialog lists the plan entries', async () => {
    const rows = [lost('mefistos', 'a', { friendly_name: 'Alpha' }), lost('mefistos', 'b')];
    mockedRestore.mockResolvedValueOnce({
      ok: true,
      value: {
        host_alias: 'mefistos',
        dry_run: true,
        plan: [
          {
            session_id: rows[0].id,
            tmux_name: rows[0].tmux_name,
            cwd: '/work/a',
            claude_session_id: 'cs-a',
            friendly_name: 'Alpha',
            action: 'restore',
            reason: null,
          },
          {
            session_id: rows[1].id,
            tmux_name: rows[1].tmux_name,
            cwd: '/work/b',
            claude_session_id: 'cs-b',
            friendly_name: null,
            action: 'restore',
            reason: null,
          },
        ],
        results: [],
      },
    });
    mount('mefistos', { hostSessions: rows });
    await fireEvent.click(screen.getByTestId('restore-lost'));
    await tick();
    expect(mockedRestore).toHaveBeenCalledWith('mefistos', { dryRun: true });
    const dialog = screen.getByTestId('confirm-dialog');
    expect(dialog.textContent).toContain('Restore lost sessions on mefistos?');
    expect(dialog.textContent).toContain('Alpha');
    expect(dialog.textContent).toContain(rows[1].tmux_name);
    expect(dialog.textContent).toContain(
      'Each session resumes its Claude conversation. Any first-run prompt waits for you.',
    );
  });

  it('a skipped plan entry shows its reason', async () => {
    const rows = [lost('mefistos', 'a')];
    mockedRestore.mockResolvedValueOnce({
      ok: true,
      value: {
        host_alias: 'mefistos',
        dry_run: true,
        plan: [
          {
            session_id: rows[0].id,
            tmux_name: rows[0].tmux_name,
            cwd: '/work/a',
            claude_session_id: 'cs-a',
            friendly_name: null,
            action: 'skip',
            reason: 'worktree missing',
          },
        ],
        results: [],
      },
    });
    mount('mefistos', { hostSessions: rows });
    await fireEvent.click(screen.getByTestId('restore-lost'));
    await tick();
    expect(screen.getByTestId('confirm-dialog').textContent).toContain('worktree missing');
  });

  it('confirming restores by id and reports ok/failure counts', async () => {
    const rows = [lost('mefistos', 'a'), lost('mefistos', 'b')];
    mockedRestore.mockResolvedValueOnce({
      ok: true,
      value: {
        host_alias: 'mefistos',
        dry_run: true,
        plan: [
          {
            session_id: rows[0].id,
            tmux_name: rows[0].tmux_name,
            cwd: '/work/a',
            claude_session_id: 'cs-a',
            friendly_name: null,
            action: 'restore',
            reason: null,
          },
          {
            session_id: rows[1].id,
            tmux_name: rows[1].tmux_name,
            cwd: '/work/b',
            claude_session_id: 'cs-b',
            friendly_name: null,
            action: 'restore',
            reason: null,
          },
        ],
        results: [],
      },
    });
    mockedRestore.mockResolvedValueOnce({
      ok: true,
      value: {
        host_alias: 'mefistos',
        dry_run: false,
        plan: [],
        results: [
          { session_id: rows[0].id, tmux_name: rows[0].tmux_name, ok: true, error: null },
          { session_id: rows[1].id, tmux_name: rows[1].tmux_name, ok: false, error: 'worktree gone' },
        ],
      },
    });
    mount('mefistos', { hostSessions: rows });
    await fireEvent.click(screen.getByTestId('restore-lost'));
    await tick();
    await fireEvent.click(screen.getByTestId('confirm-restore'));
    await tick();
    expect(mockedRestore).toHaveBeenLastCalledWith('mefistos', { sessionIds: [rows[0].id, rows[1].id] });
    expect(screen.queryByTestId('confirm-dialog')).toBeNull();
    const summary = screen.getByTestId('restore-summary');
    expect(summary.textContent).toContain('Restored 1 of 2');
    expect(summary.textContent).toContain(`${rows[1].tmux_name}: worktree gone`);
  });

  it('a plan with nothing to restore disables Restore and says so', async () => {
    const rows = [lost('mefistos', 'a')];
    mockedRestore.mockResolvedValueOnce({
      ok: true,
      value: {
        host_alias: 'mefistos',
        dry_run: true,
        plan: [
          {
            session_id: rows[0].id,
            tmux_name: rows[0].tmux_name,
            cwd: null,
            claude_session_id: 'cs-a',
            friendly_name: null,
            action: 'skip',
            reason: 'fleet controller: recreate it explicitly with force',
          },
        ],
        results: [],
      },
    });
    mount('mefistos', { hostSessions: rows });
    await fireEvent.click(screen.getByTestId('restore-lost'));
    await tick();
    expect(screen.getByTestId('confirm-restore')).toBeDisabled();
    expect(screen.getByTestId('restore-nothing')).toBeInTheDocument();
    // Cancel still works.
    expect(screen.getByTestId('confirm-cancel')).not.toBeDisabled();
  });

  it('a plan with a restore entry keeps Restore enabled', async () => {
    const rows = [lost('mefistos', 'a')];
    mockedRestore.mockResolvedValueOnce({
      ok: true,
      value: {
        host_alias: 'mefistos',
        dry_run: true,
        plan: [
          {
            session_id: rows[0].id,
            tmux_name: rows[0].tmux_name,
            cwd: null,
            claude_session_id: 'cs-a',
            friendly_name: null,
            action: 'restore',
            reason: null,
          },
        ],
        results: [],
      },
    });
    mount('mefistos', { hostSessions: rows });
    await fireEvent.click(screen.getByTestId('restore-lost'));
    await tick();
    expect(screen.getByTestId('confirm-restore')).not.toBeDisabled();
    expect(screen.queryByTestId('restore-nothing')).toBeNull();
  });

  it('a failing restore call closes the dialog and shows the error inline', async () => {
    const rows = [lost('mefistos', 'a')];
    mockedRestore.mockResolvedValueOnce({
      ok: true,
      value: {
        host_alias: 'mefistos',
        dry_run: true,
        plan: [
          {
            session_id: rows[0].id,
            tmux_name: rows[0].tmux_name,
            cwd: null,
            claude_session_id: 'cs-a',
            friendly_name: null,
            action: 'restore',
            reason: null,
          },
        ],
        results: [],
      },
    });
    mockedRestore.mockResolvedValueOnce({
      ok: false,
      error: { code: 'E_INVALID_STATE', message: 'a restore of mefistos is already in progress' },
    });
    mount('mefistos', { hostSessions: rows });
    await fireEvent.click(screen.getByTestId('restore-lost'));
    await tick();
    await fireEvent.click(screen.getByTestId('confirm-restore'));
    await tick();
    expect(mockedRestore).toHaveBeenLastCalledWith('mefistos', { sessionIds: [rows[0].id] });
    expect(screen.queryByTestId('confirm-dialog')).toBeNull();
    expect(screen.getByTestId('restore-error').textContent).toContain('already in progress');
    expect(screen.queryByTestId('restore-summary')).toBeNull();
    // The button is usable again for a retry.
    expect(screen.getByTestId('restore-lost')).not.toBeDisabled();
  });

  it('shows an inline error when the dry run fails, without opening the dialog', async () => {
    const rows = [lost('mefistos', 'a')];
    mockedRestore.mockResolvedValueOnce({
      ok: false,
      error: { code: 'E_UNREACHABLE', message: 'host unreachable' },
    });
    mount('mefistos', { hostSessions: rows });
    await fireEvent.click(screen.getByTestId('restore-lost'));
    await tick();
    expect(screen.getByTestId('restore-error').textContent).toContain('host unreachable');
    expect(screen.queryByTestId('confirm-dialog')).toBeNull();
  });
});

function candidate(over: Partial<LostCandidate> = {}): LostCandidate {
  return {
    cwd: '/work/a',
    git_branch: 'main',
    claude_session_id: 'cs-a',
    transcript_mtime: NOW - 300,
    derived_tmux_name: 'proj-a',
    project_id: 42,
    worktree_id: 7,
    existing_session_id: null,
    rank_hint: 'before_boot',
    resumable: true,
    ...over,
  };
}

describe('HostDetail find lost conversations', () => {
  beforeEach(() => {
    mockedDiscover.mockReset();
    mockedNewSession.mockReset();
  });
  afterEach(() => {
    hubStatus.set({ ...STANDALONE });
    hubConnection.set({ state: 'standalone' });
  });

  // The positive control for the gate below: a standalone desktop IS the
  // fleet, every row and every transcript on its hosts is this person's, and
  // nothing about multi-user M1 changes what a single-user install does.
  it('a standalone desktop offers Resume', async () => {
    mockedDiscover.mockResolvedValueOnce({ ok: true, value: [candidate()] });
    mount('mefistos');
    await fireEvent.click(screen.getByTestId('discover-lost'));
    await tick();

    expect(screen.queryByTestId('discover-hub-note')).toBeNull();
    expect(screen.getByTestId('discover-resume')).toBeTruthy();
  });

  /**
   * Multi-user M1 (F2c). `new_session` routes, so the hub half is satisfied by
   * a live link — and that is exactly why the access half had to be added
   * here: nothing else was asking WHOSE conversation the transcript is.
   *
   * `discover_lost_sessions` reads the host's `~/.claude/projects`, which on a
   * shared machine holds every person's conversations, and a candidate the
   * Resume button is offered for is by construction one fleet has NO row for
   * (`existing_session_id` is null; a candidate with a row shows "already in
   * fleet" instead). So there is nothing to resolve an owner from, and the
   * gate fails closed rather than offering to adopt the transcript.
   */
  it('a connected paired desktop offers no Resume: the transcript has no owner it can check', async () => {
    hubStatus.set({ ...STANDALONE, remote: true, url: 'https://hub.example' });
    hubConnection.set({ state: 'connected' });
    mockedDiscover.mockResolvedValueOnce({ ok: true, value: [candidate()] });
    mount('mefistos');
    await fireEvent.click(screen.getByTestId('discover-lost'));
    await tick();

    // The hub half says nothing: the link is up.
    expect(screen.queryByTestId('discover-hub-note')).toBeNull();
    expect(screen.queryByTestId('discover-resume')).toBeNull();
    expect(screen.getByTestId('discover-resume-blocked').getAttribute('title')).toBe(
      UNKNOWN_SESSION_REASON,
    );
    expect(mockedNewSession).not.toHaveBeenCalled();
  });

  it('an offline paired desktop offers no Resume and says why', async () => {
    hubStatus.set({ ...STANDALONE, remote: true, url: 'https://hub.example' });
    hubConnection.set({ state: 'offline', attempt: 1, retry_in_secs: 5, reason: 'refused' });
    mockedDiscover.mockResolvedValueOnce({ ok: true, value: [candidate()] });
    mount('mefistos');
    await fireEvent.click(screen.getByTestId('discover-lost'));
    await tick();

    expect(screen.queryByTestId('discover-resume')).toBeNull();
    expect(screen.getByTestId('discover-hub-note').textContent).toContain('https://hub.example');
    expect(screen.getByTestId('discover-list').textContent).toContain('resume unavailable');
    expect(mockedNewSession).not.toHaveBeenCalled();
  });

  it('is hidden when the host is unreachable', () => {
    mount('claude-fleet-htz');
    expect(screen.queryByTestId('discover-lost')).toBeNull();
  });

  it('renders the three candidate shapes: resumable, already in fleet, no project', async () => {
    const resumable = candidate();
    const existing = candidate({
      cwd: '/work/b',
      git_branch: null,
      claude_session_id: 'cs-b',
      transcript_mtime: NOW - 7200,
      existing_session_id: 99,
      rank_hint: 'after_boot',
    });
    const noProject = candidate({
      cwd: '/work/c',
      git_branch: null,
      claude_session_id: 'cs-c',
      transcript_mtime: NOW - 90000,
      derived_tmux_name: null,
      project_id: null,
      rank_hint: 'stale',
      resumable: false,
    });
    mockedDiscover.mockResolvedValueOnce({ ok: true, value: [resumable, existing, noProject] });
    mount('mefistos');
    await fireEvent.click(screen.getByTestId('discover-lost'));
    await tick();

    expect(mockedDiscover).toHaveBeenCalledWith('mefistos');
    const list = screen.getByTestId('discover-list');
    expect(list.textContent).toContain('/work/a');
    expect(list.textContent).toContain('main');
    expect(list.textContent).toContain('before reboot');
    expect(list.textContent).toContain('proj-a');
    expect(list.textContent).toContain('already in fleet');
    // The one with no project is restored into one (4.12).
    expect(screen.getAllByTestId('discover-restore-into')).toHaveLength(1);
    expect(screen.getAllByTestId('discover-resume')).toHaveLength(1);
  });

  it('a candidate in a project but not at a resumable path has no Resume, only Restore into', async () => {
    const subdir = candidate({
      cwd: '/work/a/src',
      claude_session_id: 'cs-sub',
      derived_tmux_name: null,
      worktree_id: null,
      resumable: false,
    });
    mockedDiscover.mockResolvedValueOnce({ ok: true, value: [subdir] });
    mount('mefistos');
    await fireEvent.click(screen.getByTestId('discover-lost'));
    await tick();

    expect(screen.getByTestId('discover-restore-into')).toBeTruthy();
    expect(screen.queryByTestId('discover-resume')).toBeNull();
  });

  it('never offers Resume for a non-resumable candidate even if a name is present', async () => {
    // Defence in depth: `resumable` is the gate, not the presence of a name.
    mockedDiscover.mockResolvedValueOnce({
      ok: true,
      value: [candidate({ claude_session_id: 'cs-x', resumable: false })],
    });
    mount('mefistos');
    await fireEvent.click(screen.getByTestId('discover-lost'));
    await tick();

    expect(screen.queryByTestId('discover-resume')).toBeNull();
    expect(screen.getByTestId('discover-restore-into')).toBeTruthy();
  });

  it('Resume calls newSessionAbortable with the exact args, including resume_claude_session_id', async () => {
    const resumable = candidate();
    mockedDiscover.mockResolvedValueOnce({ ok: true, value: [resumable] });
    mockedNewSession.mockResolvedValueOnce({ ok: true, value: session('mefistos', 'proj-a') });
    mount('mefistos');
    await fireEvent.click(screen.getByTestId('discover-lost'));
    await tick();
    await fireEvent.click(screen.getByTestId('discover-resume'));
    await tick();

    expect(mockedNewSession).toHaveBeenCalledWith({
      host_alias: 'mefistos',
      project_id: 42,
      worktree_id: 7,
      name: 'proj-a',
      resume_claude_session_id: 'cs-a',
    });
    expect(screen.getByTestId('discover-list').textContent).toContain('resumed');
    expect(screen.queryByTestId('discover-resume')).toBeNull();
  });

  it('shows a resume error inline and leaves the button in place', async () => {
    const resumable = candidate();
    mockedDiscover.mockResolvedValueOnce({ ok: true, value: [resumable] });
    mockedNewSession.mockResolvedValueOnce({
      ok: false,
      error: { code: 'E_EXISTS', message: 'already resumed on this host' },
    });
    mount('mefistos');
    await fireEvent.click(screen.getByTestId('discover-lost'));
    await tick();
    await fireEvent.click(screen.getByTestId('discover-resume'));
    await tick();

    expect(screen.getByTestId('discover-item-error').textContent).toContain('already resumed on this host');
    expect(screen.getByTestId('discover-resume')).toBeInTheDocument();
  });

  it('renders the transcript age from the seconds-based now prop, not a raw ms mismatch', async () => {
    const threeHoursAgo = candidate({ claude_session_id: 'cs-age', transcript_mtime: NOW - 3 * 3600 });
    mockedDiscover.mockResolvedValueOnce({ ok: true, value: [threeHoursAgo] });
    mount('mefistos');
    await fireEvent.click(screen.getByTestId('discover-lost'));
    await tick();

    // `now` (the mounted prop) is epoch SECONDS, as shortAge's second
    // param is — the expected string is derived from shortAge itself rather
    // than hardcoded, so this stays correct if its buckets change.
    const expected = shortAge(threeHoursAgo.transcript_mtime, NOW);
    expect(screen.getByTestId('discover-list').textContent).toContain(expected);
    // Guard against a vacuous pass: "now" (what a seconds/ms mixup
    // produces) must not be what we just asserted for a 3h-old transcript.
    expect(expected).toBe('3h');
  });

  it('an empty result says so', async () => {
    mockedDiscover.mockResolvedValueOnce({ ok: true, value: [] });
    mount('mefistos');
    await fireEvent.click(screen.getByTestId('discover-lost'));
    await tick();

    expect(screen.getByTestId('discover-list').textContent).toContain(
      'No Claude conversations found on mefistos',
    );
  });

  it('shows an inline error when the scan itself fails', async () => {
    mockedDiscover.mockResolvedValueOnce({ ok: false, error: { code: 'E_UNREACHABLE', message: 'host offline' } });
    mount('mefistos');
    await fireEvent.click(screen.getByTestId('discover-lost'));
    await tick();

    expect(screen.getByTestId('discover-error').textContent).toContain('host offline');
    expect(screen.queryByTestId('discover-list')).toBeNull();
  });
});

describe('HostDetail Codex assets (F3a)', () => {
  beforeEach(() => mockedSetHarnesses.mockReset());
  afterEach(() => {
    hubStatus.set({ ...STANDALONE });
    hubConnection.set({ state: 'standalone' });
  });

  it('a host with no choice reads auto, and picking on sends the explicit list', async () => {
    mockedSetHarnesses.mockResolvedValueOnce({ ok: true, value: host('mefistos', { harnesses: ['claude', 'codex'] }) });
    mount('mefistos');
    const sel = screen.getByTestId('detail-codex') as HTMLSelectElement;
    expect(sel.value).toBe('auto');
    await fireEvent.change(sel, { target: { value: 'on' } });
    expect(mockedSetHarnesses).toHaveBeenCalledWith('mefistos', ['claude', 'codex']);
  });

  it('an explicit list without codex reads off, and auto sends null', async () => {
    mockedSetHarnesses.mockResolvedValueOnce({ ok: true, value: host('mefistos') });
    mount('mefistos', { host: host('mefistos', { harnesses: ['claude'] }) });
    const sel = screen.getByTestId('detail-codex') as HTMLSelectElement;
    expect(sel.value).toBe('off');
    await fireEvent.change(sel, { target: { value: 'auto' } });
    expect(mockedSetHarnesses).toHaveBeenCalledWith('mefistos', null);
  });

  it('an offline paired desktop cannot change it and says why', () => {
    hubStatus.set({ ...STANDALONE, remote: true, url: 'https://hub.example' });
    hubConnection.set({ state: 'offline', attempt: 1, retry_in_secs: 5, reason: 'refused' });
    mount('mefistos');
    const sel = screen.getByTestId('detail-codex') as HTMLSelectElement;
    expect(sel.disabled).toBe(true);
    expect(sel.title).toContain('https://hub.example');
  });
});

describe('an offline host (states kit, step 10.6)', () => {
  it('says so in its own pane, with Try again running the probe', async () => {
    const onreprobe = vi.fn();
    // A failed probe stamps `last_pinged_at` too; "last answered" is the
    // last time the host really answered (review r13).
    mount('mercury', {
      host: host('mercury', { reachable: false, last_pinged_at: NOW - 20, health_at: NOW - 360 }),
      onreprobe,
      now: NOW,
    });
    const off = screen.getByTestId('host-offline-state');
    expect(off.textContent).toContain('last answered 6 m ago');
    await fireEvent.click(screen.getByTestId('host-offline-try'));
    expect(onreprobe).toHaveBeenCalledOnce();
  });

  // Step 3.14: its sessions wait as Paused, and Show sessions goes to them.
  it('names its sessions as Paused, and Show sessions focuses the list', async () => {
    mount('mefistos', { host: host('mefistos', { reachable: false, health_at: NOW - 360 }) });
    const rows = screen.getAllByTestId('detail-session');
    expect(rows.length).toBeGreaterThan(0);
    const paused = screen.getByTestId('host-offline-paused').textContent ?? '';
    expect(paused).toContain('Paused');
    expect(paused).toContain(rows[0].querySelector('.s-name')?.textContent ?? '?');
    // Nothing can wake a host yet, so no Wake host.
    expect(screen.queryByTestId('host-offline-wake')).toBeNull();
    await fireEvent.click(screen.getByTestId('host-offline-show'));
    expect(document.activeElement).toBe(rows[0]);
  });

  it('says why the last probe failed and when the host last answered', () => {
    mount('mercury', {
      host: host('mercury', {
        reachable: false,
        last_pinged_at: NOW - 5,
        last_reachable_at: NOW - 600,
        last_probe_error: 'SSH timed out after 10 s',
        last_probe_error_code: 'E_SSH_TIMEOUT',
      }),
      now: NOW,
    });
    const off = screen.getByTestId('host-offline-state');
    expect(off.textContent).toContain('last answered 10 m ago');
    expect(off.textContent).toContain('SSH timed out after 10 s');
    expect(screen.getByTestId('host-offline-code').textContent).toBe('E_SSH_TIMEOUT');
  });

  it('a reachable host shows no offline state', () => {
    mount('mercury', { host: host('mercury', { reachable: true }) });
    expect(screen.queryByTestId('host-offline-state')).toBeNull();
  });
});

describe('HostDetail Lost and found with proposals (4.12)', () => {
  const papaya = {
    id: 3,
    owner: 'acme',
    repo: 'papaya-pos',
    base_path: '/p/acme/papaya-pos',
    last_session_at: 5,
    adopted: false,
    system: false,
  };
  const settle = async () => {
    for (let i = 0; i < 6; i++) await tick();
  };

  beforeEach(() => {
    mockedAdopt.mockReset();
    mockedTarget.mockReset();
    mockedPlace.mockReset();
    mockedDiscover.mockReset();
    mockedNewSession.mockReset();
    projects.set([{ project: papaya, worktrees: [] }]);
  });

  function withScratch() {
    const scratch = session('mefistos', 'fleet-trn-scratch', { started_at: null, created_at: NOW - 7200 });
    const ours = session('mefistos', 'dev-acme-papaya-pos', { started_at: NOW - 60 });
    return { scratch, hostSessions: [scratch, ours] };
  }

  it('lists a pane fleet did not start, and Adopt asks to confirm the prefilled project', async () => {
    const { scratch, hostSessions } = withScratch();
    mockedTarget.mockResolvedValueOnce({
      ok: true,
      value: { project_id: 3, source: 'jev', reason: 'directory and the name fleet-trn-scratch', confidence_pct: 81 },
    });
    mockedAdopt.mockResolvedValueOnce({ ok: true, value: { ...scratch, started_at: NOW } });
    mount('mefistos', { hostSessions });
    const list = screen.getByTestId('outside-panes');
    expect(list.textContent).toContain('fleet-trn-scratch');
    expect(list.textContent).toContain('outside fleet');
    expect(list.textContent).not.toContain('dev-acme-papaya-pos');

    await fireEvent.click(screen.getByTestId('outside-adopt'));
    await settle();
    expect(mockedTarget).toHaveBeenCalledWith({ session_id: scratch.id });
    expect((screen.getByTestId('lost-target-project') as HTMLSelectElement).value).toBe('3');
    expect(screen.getByTestId('lost-target-proposed').textContent).toContain('Proposed by Jev');

    await fireEvent.click(screen.getByTestId('lost-target-submit'));
    await tick();
    expect(mockedAdopt).not.toHaveBeenCalled();
    await fireEvent.click(screen.getByTestId('lost-target-confirm'));
    await settle();
    expect(mockedAdopt).toHaveBeenCalledWith(scratch.id, 3);
  });

  it('Restore into copies the conversation, then resumes it in the chosen project', async () => {
    mockedDiscover.mockResolvedValueOnce({
      ok: true,
      value: [candidate({ cwd: '/home/ada/tmp', project_id: null, resumable: false, derived_tmux_name: null })],
    });
    mockedTarget.mockResolvedValueOnce({ ok: true, value: { unsure: true } });
    mockedPlace.mockResolvedValueOnce({
      ok: true,
      value: { project_id: 3, tmux_name: 'dev-acme-papaya-pos', copied: true },
    });
    mockedNewSession.mockResolvedValueOnce({ ok: true, value: session('mefistos', 'dev-acme-papaya-pos') });
    mount('mefistos');
    await fireEvent.click(screen.getByTestId('discover-lost'));
    await tick();
    await fireEvent.click(screen.getByTestId('discover-restore-into'));
    await settle();
    expect(screen.getByTestId('lost-target-unsure').textContent).toContain('Jev was unsure');
    const sel = screen.getByTestId('lost-target-project') as HTMLSelectElement;
    expect(sel.value).toBe('');
    sel.value = '3';
    await fireEvent.change(sel);
    await fireEvent.click(screen.getByTestId('lost-target-submit'));
    await tick();
    await fireEvent.click(screen.getByTestId('lost-target-confirm'));
    await settle();
    expect(mockedPlace).toHaveBeenCalledWith({ host_alias: 'mefistos', claude_session_id: 'cs-a', project_id: 3 });
    expect(mockedNewSession).toHaveBeenCalledWith({
      host_alias: 'mefistos',
      project_id: 3,
      worktree_id: null,
      name: 'dev-acme-papaya-pos',
      resume_claude_session_id: 'cs-a',
    });
    expect(screen.getByTestId('discover-list').textContent).toContain('resumed');
  });


  // Gap plan G2.7, the FormsSession board's "Adopt a lost session · Ignore":
  // a found conversation is left out of later searches on this device, with
  // Undo, and "Show them" brings the ignored ones back into view.
  it('Ignore leaves a found conversation out from now on, with Undo', async () => {
    localStorage.clear();
    clearToasts();
    const orphan = candidate({ cwd: '/work/c', claude_session_id: 'cs-c', project_id: null, derived_tmux_name: null, resumable: false });
    mockedDiscover.mockResolvedValue({ ok: true, value: [orphan] });
    mockedTarget.mockResolvedValue({ ok: true, value: { unsure: true } });
    mount('mefistos');
    await fireEvent.click(screen.getByTestId('discover-lost'));
    await settle();
    await fireEvent.click(screen.getByTestId('discover-restore-into'));
    await settle();
    await fireEvent.click(screen.getByTestId('lost-target-ignore'));
    await settle();
    expect(screen.queryByTestId('discover-restore-into')).toBeNull();
    expect(screen.getByTestId('discover-ignored-count').textContent).toContain('1 ignored on this device');
    // A second search still leaves it out.
    await fireEvent.click(screen.getByTestId('discover-lost'));
    await settle();
    expect(screen.queryByTestId('discover-restore-into')).toBeNull();
    // Show them, then Undo from the toast.
    await fireEvent.click(screen.getByTestId('discover-show-ignored'));
    expect(screen.getByTestId('discover-ignored')).toBeTruthy();
    get(toasts).at(-1)!.action!.run();
    await settle();
    expect(screen.queryByTestId('discover-ignored')).toBeNull();
    expect(screen.queryByTestId('discover-ignored-count')).toBeNull();
    mockedDiscover.mockReset();
    localStorage.clear();
  });
});
