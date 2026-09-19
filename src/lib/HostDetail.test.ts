import { render, screen, fireEvent, within } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('./sessions', async () => {
  const actual = await vi.importActual<typeof import('./sessions')>('./sessions');
  return { ...actual, restoreHostSessions: vi.fn() };
});

import HostDetail from './HostDetail.svelte';
import { sharedWith } from './hosts_view';
import { ADMIN, GMAIL, NOW, fleetHosts, fleetSessions, fleetUsage, host, session } from './hosts_fixture';
import { restoreHostSessions, type SessionRow } from './sessions';

const mockedRestore = restoreHostSessions as unknown as ReturnType<typeof vi.fn>;

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
