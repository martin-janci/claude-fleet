import { render, screen, fireEvent, within } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('./sessions', async () => {
  const actual = await vi.importActual<typeof import('./sessions')>('./sessions');
  return {
    ...actual,
    restoreHostSessions: vi.fn(),
    discoverLostSessions: vi.fn(),
    newSessionAbortable: vi.fn(),
  };
});

import HostDetail from './HostDetail.svelte';
import { sharedWith } from './hosts_view';
import { timeAgo } from './session_status';
import { ADMIN, GMAIL, NOW, fleetHosts, fleetSessions, fleetUsage, host, session } from './hosts_fixture';
import {
  restoreHostSessions,
  discoverLostSessions,
  newSessionAbortable,
  type LostCandidate,
  type SessionRow,
} from './sessions';

const mockedRestore = restoreHostSessions as unknown as ReturnType<typeof vi.fn>;
const mockedDiscover = discoverLostSessions as unknown as ReturnType<typeof vi.fn>;
const mockedNewSession = newSessionAbortable as unknown as ReturnType<typeof vi.fn>;

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
    expect(screen.getByTestId('detail-account').textContent).toContain('admin@32bit.sk');
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
    ...over,
  };
}

describe('HostDetail find lost conversations', () => {
  beforeEach(() => {
    mockedDiscover.mockReset();
    mockedNewSession.mockReset();
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
    expect(list.textContent).toContain('no fleet project for this path');
    expect(screen.getAllByTestId('discover-resume')).toHaveLength(1);
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

    // `now` (the mounted prop) is epoch SECONDS; timeAgo's second param is
    // epoch MILLISECONDS — the expected string is derived from timeAgo
    // itself rather than hardcoded, so this stays correct if its buckets
    // change.
    const expected = timeAgo(threeHoursAgo.transcript_mtime, NOW * 1000);
    expect(screen.getByTestId('discover-list').textContent).toContain(expected);
    // Guard against a vacuous pass: "just now" (what the seconds/ms mixup
    // produces) must not be what we just asserted for a 3h-old transcript.
    expect(expected).not.toBe('just now');
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
