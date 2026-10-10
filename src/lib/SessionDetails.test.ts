import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

// Only startMove/resolveMoveRun are replaced with spies: the panel must
// trigger recovery through them (never resolve a move itself), and the
// details-move-back / details-finish-move / details-undo-move tests assert
// on how they were called. transferSheetFor and adoptPartial keep their real
// behaviour — the sheet-opening tests only look at the store's value.
vi.mock('./moves', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./moves')>();
  return {
    ...actual,
    startMove: vi.fn(),
    resolveMoveRun: vi.fn(),
  };
});

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import SessionDetails from './SessionDetails.svelte';
import { hosts } from './hosts';
import { accounts } from './accounts';
import { projects } from './projects';
import { sessions, type SessionRow } from './sessions';
import { get } from 'svelte/store';
import { moves, resetMovesForTest, startMove, resolveMoveRun, transferSheetFor } from './moves';
import { hubStatus, STANDALONE, type HubStatus } from './hub';
import { hubConnection } from './hub_connection';
import { applyGrantChanges, resetAccessForTests, setMyGrants } from './access';
import { shareSheetFor } from './share';
import { expectAccessible } from './a11y_check';

const sampleSession = {
  id: 1,
  tmux_name: 'dev-foo',
  host_alias: 'mefistos',
  project_id: null,
  worktree_id: null,
  created_at: 1,
  last_activity_at: 1,
  status: 'running',
  notes: null,
  account_uuid: null,
  kind: 'work',
  reviews_session_id: null,
  worktree_key: null,
  lost_at: null,
  claude_session_id: null,
  claude_status: null,
  effort_level: null,
  pr_url: null,
  current_activity: null,
  friendly_name: null, safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null, safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null, turn_seq: 0, last_stop_at: null, parent_session_id: null, tags: [], model: null, context_tokens: null, context_window: null, context_source: null, context_at: null, context_stale: false, tmux_pane_id: null, pending_input: null,
};

beforeEach(() => {
  hosts.set([]);
  accounts.set([]);
  projects.set([]);
  sessions.set([]);
  // Standalone, which is what every test here assumes: the desktop owns its
  // fleet, so the multi-user derivation answers `own` for every row and the
  // panel looks exactly as it did before M1.
  hubStatus.set({ ...STANDALONE });
  hubConnection.set({ state: 'standalone' });
  resetAccessForTests();
  shareSheetFor.set(null);
});

describe('SessionDetails', () => {
  it('shows host alias from session', async () => {
    hosts.set([
      { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false, transport: 'ssh' },
    ]);
    render(SessionDetails, { props: { session: sampleSession } });
    await tick();
    // The host, then whether it is online (UX audit 2026-10-09, D1).
    expect((await screen.findByTestId('session-host')).textContent).toMatch(/^mefistos\b/);
  });

  it('shows account when host has one linked', async () => {
    hosts.set([
      { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: 'u1', provisioned: false, transport: 'ssh' },
    ]);
    accounts.set([
      { uuid: 'u1', email: 'm-janci@users.noreply.github.com', display_name: 'M', organization_name: null, organization_uuid: null, seat_tier: 'max', last_seen_at: 1, nickname: null, has_extra_usage: false },
    ]);
    render(SessionDetails, { props: { session: sampleSession } });
    await tick();
    const cell = await screen.findByTestId('session-account');
    expect(cell.textContent).toContain('m-janci@users.noreply.github.com');
    expect(cell.textContent).toContain('max');
  });

  it('shows — when host has no account', async () => {
    hosts.set([
      { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false, transport: 'ssh' },
    ]);
    accounts.set([]);
    render(SessionDetails, { props: { session: sampleSession } });
    await tick();
    expect((await screen.findByTestId('session-account')).textContent?.trim()).toBe('—');
  });

  it('a profile session shows its own login’s account, not the host’s', async () => {
    hosts.set([
      { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: 'u1', provisioned: false, transport: 'ssh' },
    ]);
    accounts.set([
      { uuid: 'u1', email: 'host@x.com', display_name: 'H', organization_name: null, organization_uuid: null, seat_tier: 'max', last_seen_at: 1, nickname: null, has_extra_usage: false },
      { uuid: 'u2', email: 'work@x.com', display_name: 'W', organization_name: null, organization_uuid: null, seat_tier: 'pro', last_seen_at: 1, nickname: null, has_extra_usage: false },
    ]);
    render(SessionDetails, { props: { session: { ...sampleSession, claude_profile: 'work', account_uuid: 'u2' } } });
    await tick();
    const cell = await screen.findByTestId('session-account');
    expect(cell.textContent).toContain('work@x.com');
    expect(cell.textContent).not.toContain('host@x.com');
  });

  it('a host-login session shows the account it runs on, not the host’s current one', async () => {
    hosts.set([
      { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: 'u1', provisioned: false, transport: 'ssh' },
    ]);
    accounts.set([
      { uuid: 'u1', email: 'host-b@x.com', display_name: 'B', organization_name: null, organization_uuid: null, seat_tier: 'max', last_seen_at: 1, nickname: null, has_extra_usage: false },
      { uuid: 'u2', email: 'sess-a@x.com', display_name: 'A', organization_name: null, organization_uuid: null, seat_tier: 'pro', last_seen_at: 1, nickname: null, has_extra_usage: false },
    ]);
    render(SessionDetails, { props: { session: { ...sampleSession, account_uuid: 'u2' } } });
    await tick();
    const cell = await screen.findByTestId('session-account');
    expect(cell.textContent).toContain('sess-a@x.com');
    expect(cell.textContent).not.toContain('host-b@x.com');
  });

  it('switching the login asks first, then restarts under the picked profile', async () => {
    const { invoke } = await import('@tauri-apps/api/core');
    const inv = invoke as unknown as ReturnType<typeof vi.fn>;
    inv.mockReset();
    inv.mockResolvedValue({ ...sampleSession, claude_profile: 'work' });
    hosts.set([
      {
        alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false, transport: 'ssh',
        claude_profiles: [{ name: 'work', account_uuid: 'u2', email: 'work@x.com' }],
      },
    ]);
    render(SessionDetails, { props: { session: sampleSession } });
    await tick();
    const pick = (await screen.findByTestId('session-login-pick')) as HTMLSelectElement;
    expect(Array.from(pick.options).map((o) => o.textContent)).toEqual(['Host login · current', 'work (work@x.com)']);
    expect(screen.queryByTestId('session-login-switch')).toBeNull();
    await fireEvent.change(pick, { target: { value: 'work' } });
    await tick();
    (await screen.findByTestId('session-login-switch')).click();
    await tick();
    expect(inv.mock.calls.some((c) => c[0] === 'restart_session')).toBe(false);
    (await screen.findByTestId('confirm-login-switch')).click();
    await tick();
    await tick();
    expect(inv.mock.calls).toContainEqual([
      'restart_session',
      { args: { host_alias: 'mefistos', name: 'dev-foo', profile: 'work' } },
    ]);
  });

  it('a login picked on one session does not carry over to the next one selected (review r07)', async () => {
    hosts.set([
      {
        alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false, transport: 'ssh',
        claude_profiles: [{ name: 'work', account_uuid: 'u2', email: 'work@x.com' }],
      },
    ]);
    const { rerender } = render(SessionDetails, { props: { session: sampleSession } });
    await tick();
    const pick = (await screen.findByTestId('session-login-pick')) as HTMLSelectElement;
    await fireEvent.change(pick, { target: { value: 'work' } });
    await tick();
    expect(screen.getByTestId('session-login-switch')).toBeTruthy();
    await rerender({ session: { ...sampleSession, id: 2, tmux_name: 'dev-bar' } });
    await tick();
    expect(screen.queryByTestId('session-login-switch')).toBeNull();
    expect((screen.getByTestId('session-login-pick') as HTMLSelectElement).value).toBe('');
  });

  it('a shell session has no login to switch', async () => {
    render(SessionDetails, { props: { session: { ...sampleSession, kind: 'shell' } } });
    await tick();
    expect(screen.queryByTestId('session-login')).toBeNull();
  });

  it('shows Related sessions panel when siblings exist', async () => {
    const source = { ...sampleSession, id: 1, project_id: 1, worktree_id: 10, worktree_key: 'main' };
    const sibling = { ...sampleSession, id: 2, tmux_name: 'dev-sib', host_alias: 'mefistos', project_id: 1, worktree_id: 10, worktree_key: 'main' };
    hosts.set([
      { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false, transport: 'ssh' },
    ]);
    accounts.set([]);
    sessions.set([source, sibling]);
    render(SessionDetails, { props: { session: source } });
    await tick();
    const rows = await screen.findAllByTestId('related-row');
    expect(rows).toHaveLength(1);
    expect(rows[0].textContent).toContain('dev-sib');
  });

  describe('N1: Jev proposes a session on the same work (redesign 6.9)', () => {
    const me = { ...sampleSession, id: 1, project_id: 1, worktree_id: 10, worktree_key: 'main', proposals: [{ feature: 'related_session', value: 's3', source: 'jev' as const, confidence_pct: 81 }] };
    const twin = { ...sampleSession, id: 3, tmux_name: 'dev-twin', host_alias: 'mefistos', project_id: 2, worktree_id: 20, worktree_key: 'fix' };

    it('lists it under Related sessions', async () => {
      sessions.set([me, twin]);
      render(SessionDetails, { props: { session: me } });
      await tick();
      expect(screen.getByTestId('related-sessions').textContent).toMatch(/Related sessions\s*1/);
      expect(screen.getByTestId('related-proposed-row').textContent).toContain('dev-twin');
      expect(screen.getByTestId('related-proposed-row').textContent).toContain('same work?');
      expect(screen.getByTestId('related-proposed-by').textContent).toContain('81%');
    });

    it('shows nothing when the session is not in the list', async () => {
      sessions.set([me]);
      render(SessionDetails, { props: { session: me } });
      await tick();
      expect(screen.queryByTestId('related-proposed')).toBeNull();
    });
  });

  it('hides Related panel when session has no siblings', async () => {
    const lone = { ...sampleSession, id: 1, project_id: 1, worktree_id: 10, worktree_key: 'main' };
    sessions.set([lone]);
    render(SessionDetails, { props: { session: lone } });
    await tick();
    expect(screen.queryByTestId('related-sessions')).toBeNull();
  });

  it('hides Related panel for orphan sessions (project_id=null)', async () => {
    const orphan = { ...sampleSession, id: 1, project_id: null, worktree_id: null, worktree_key: null };
    const otherOrphan = { ...sampleSession, id: 2, tmux_name: 'dev-other', project_id: null, worktree_id: null, worktree_key: null };
    sessions.set([orphan, otherOrphan]);
    render(SessionDetails, { props: { session: orphan } });
    await tick();
    expect(screen.queryByTestId('related-sessions')).toBeNull();
  });

  it('hides Related panel for sessions with different worktree_key', async () => {
    const source = { ...sampleSession, id: 1, project_id: 1, worktree_id: 10, worktree_key: 'main' };
    const diffKey = { ...sampleSession, id: 2, tmux_name: 'dev-feat', project_id: 1, worktree_id: 11, worktree_key: 'feature-x' };
    sessions.set([source, diffKey]);
    render(SessionDetails, { props: { session: source } });
    await tick();
    expect(screen.queryByTestId('related-sessions')).toBeNull();
  });

  it('shows Repair workspace only for project-backed, non-bg sessions and reports the outcome', async () => {
    const { invoke } = await import('@tauri-apps/api/core');
    const { toasts, clearToasts } = await import('./toasts');
    const { get } = await import('svelte/store');
    clearToasts();
    // Orphan (no project): nothing to repair, no button.
    render(SessionDetails, { props: { session: { ...sampleSession, project_id: null } } });
    await tick();
    expect(screen.queryByTestId('repair-from-details')).toBeNull();
    // Project-backed work session: button present; click → repair_session(id) → toast.
    render(SessionDetails, { props: { session: { ...sampleSession, id: 7, project_id: 1 } } });
    await tick();
    const btn = await screen.findByTestId('repair-from-details');
    // Queue the repair response only now: the Timeline's mount-time
    // session_history call would otherwise consume a once-value set earlier.
    (invoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      session_id: 7, host_alias: 'mefistos', tmux_name: 'dev-foo', cwd: '/r/.worktrees/x',
      healthy: false,
      actions: ['git worktree remove --force -- /r/.worktrees/x', 'tmux respawn-pane -k -c /r/.worktrees/x'],
      warnings: [], needs_explicit_repair: false, deferred: [], branch_source: 'branch_local',
      tmux: 'respawned', tmux_alive: true, tmux_dead: false,
      tmux_cwd_stale: false, worktree_row_updated: false, sibling_session_ids: [],
    });
    btn.click();
    await tick();
    // Repair can respawn a live pane, so it asks first.
    (await screen.findByTestId('confirm-repair-details')).click();
    await tick();
    await tick();
    expect((invoke as ReturnType<typeof vi.fn>).mock.calls).toContainEqual([
      'repair_session',
      { args: { session_id: 7, explicit: true } },
    ]);
    const shown = get(toasts);
    const done = shown.find((t) => t.kind === 'success' && t.message.includes('git worktree remove --force'));
    expect(done).toBeTruthy();
    // branch_source is always shown for a repair that re-added the worktree.
    expect(done?.message).toContain('branch_local');
  });

  // Both of these stop the claude process that is running right now. Kill and
  // Recreate beside them have always confirmed; these two wear a `↻` and a
  // plaster, which read like Refresh.
  it('Restart asks before stopping the running claude, and Cancel sends nothing', async () => {
    const { invoke } = await import('@tauri-apps/api/core');
    const inv = invoke as ReturnType<typeof vi.fn>;
    render(SessionDetails, { props: { session: { ...sampleSession, id: 11, project_id: 1 } } });
    await tick();
    (await screen.findByTestId('restart-from-details')).click();
    await tick();
    const dialog = await screen.findByTestId('confirm-dialog');
    expect(dialog.textContent).toContain('dev-foo');
    expect(screen.getByTestId('confirm-restart-details')).toBeTruthy();
    // Destructive: the safe button holds initial focus.
    expect(document.activeElement).toBe(screen.getByTestId('confirm-cancel'));

    inv.mockClear();
    screen.getByTestId('confirm-cancel').click();
    await tick();
    expect(screen.queryByTestId('confirm-restart-details')).toBeNull();
    expect(inv.mock.calls.some((c) => c[0] === 'restart_session')).toBe(false);

    // Confirming does send it.
    (await screen.findByTestId('restart-from-details')).click();
    await tick();
    (await screen.findByTestId('confirm-restart-details')).click();
    await tick();
    await tick();
    expect(inv.mock.calls).toContainEqual([
      'restart_session',
      { args: { host_alias: 'mefistos', name: 'dev-foo' } },
    ]);
  });

  it('Repair asks before respawning the pane, and Cancel sends nothing', async () => {
    const { invoke } = await import('@tauri-apps/api/core');
    const inv = invoke as ReturnType<typeof vi.fn>;
    render(SessionDetails, { props: { session: { ...sampleSession, id: 12, project_id: 1 } } });
    await tick();
    (await screen.findByTestId('repair-from-details')).click();
    await tick();
    expect(screen.getByTestId('confirm-dialog').textContent).toContain('respawn the pane');
    expect(document.activeElement).toBe(screen.getByTestId('confirm-cancel'));

    inv.mockClear();
    screen.getByTestId('confirm-cancel').click();
    await tick();
    expect(screen.queryByTestId('confirm-repair-details')).toBeNull();
    expect(inv.mock.calls.some((c) => c[0] === 'repair_session')).toBe(false);
  });

  it('offers Move to host… for resumable worktree sessions and opens the Transfer sheet', async () => {
    const { transferSheetFor, resetMovesForTest } = await import('./moves');
    const { get } = await import('svelte/store');
    resetMovesForTest();
    // No Claude session id → nothing to resume, no button.
    render(SessionDetails, { props: { session: { ...sampleSession, project_id: 1, worktree_id: 10 } } });
    await tick();
    expect(screen.queryByTestId('move-from-details')).toBeNull();

    const movable = {
      ...sampleSession, id: 5, project_id: 1, worktree_id: 10,
      claude_session_id: '550e8400-e29b-41d4-a716-446655440000',
    };
    render(SessionDetails, { props: { session: movable } });
    await tick();
    (await screen.findByTestId('move-from-details')).click();
    await tick();
    expect(get(transferSheetFor)).toBe(5);
    // The panel no longer owns a dialog: the app's one TransferSheet does.
    expect(screen.queryByTestId('move-dialog')).toBeNull();
    expect(screen.queryByTestId('confirm-move')).toBeNull();
  });

  it('hides Move to host… for shell sessions', async () => {
    render(SessionDetails, {
      props: {
        session: {
          ...sampleSession, kind: 'shell', project_id: 1, worktree_id: 10,
          claude_session_id: '550e8400-e29b-41d4-a716-446655440000',
        },
      },
    });
    await tick();
    expect(screen.queryByTestId('move-from-details')).toBeNull();
  });

  it('shows the Review button', async () => {
    sessions.set([sampleSession]);
    render(SessionDetails, { props: { session: sampleSession } });
    await tick();
    expect(screen.getByTestId('open-review')).toBeTruthy();
  });

  it('shows "Reviewing: <source>" for a review session', async () => {
    const source = { ...sampleSession, id: 1, tmux_name: 'dev-source', kind: 'work', reviews_session_id: null };
    const review = { ...sampleSession, id: 2, tmux_name: 'review-foo', kind: 'review', reviews_session_id: 1 };
    sessions.set([source, review]);
    render(SessionDetails, { props: { session: review } });
    await tick();
    const link = await screen.findByTestId('reviewing-link');
    expect(link.textContent?.trim()).toBe('dev-source');
  });

  it('lists reviews pointing at a source session', async () => {
    const source = { ...sampleSession, id: 1, tmux_name: 'dev-source', kind: 'work', reviews_session_id: null };
    const review = { ...sampleSession, id: 2, tmux_name: 'review-foo', kind: 'review', reviews_session_id: 1 };
    sessions.set([source, review]);
    render(SessionDetails, { props: { session: source } });
    await tick();
    const panel = await screen.findByTestId('reviews-panel');
    expect(panel).toBeTruthy();
    expect(panel.textContent).toContain('review-foo');
  });
});

describe('SessionDetails outcome + triage fields (W2 Track D)', () => {
  it('shows the stuck chip with its kind and how long it has been stuck', async () => {
    const now = Math.floor(Date.now() / 1000);
    render(SessionDetails, {
      props: { session: { ...sampleSession, claude_status: 'working', stuck_kind: 'trust_prompt', stuck_since: now - 120 } },
    });
    await tick();
    const chip = screen.getByTestId('details-stuck');
    expect(chip).toHaveTextContent('Failed · trust prompt');
    expect(chip).toHaveTextContent('2m');
    // Stuck outranks claude_status.
    expect(screen.queryByTestId('details-claude-status')).toBeNull();
  });

  it('shows token usage and the estimated cost once usage was counted', async () => {
    render(SessionDetails, {
      props: {
        session: {
          ...sampleSession,
          usage_input_tokens: 1_234,
          usage_output_tokens: 56_000,
          usage_cache_write_tokens: 2_000_000,
          usage_cache_read_tokens: 45_000_000,
          usage_cost_micros: 12_340_000,
          usage_model: 'claude-opus-5',
          usage_updated_at: 1,
        },
      },
    });
    await tick();
    const usage = screen.getByTestId('details-usage');
    expect(screen.getByTestId('details-cost')).toHaveTextContent('$12.34 estimated');
    expect(usage).toHaveTextContent('1.2k in');
    expect(usage).toHaveTextContent('56k out');
    expect(usage).toHaveTextContent('2.00M cache write');
    expect(usage).toHaveTextContent('45.0M cache read');
    expect(usage).toHaveTextContent('claude-opus-5');
    expect(usage.getAttribute('title')).toContain('Estimated');
  });

  it('says "unpriced (model)" when tokens were counted but the model has no price', async () => {
    render(SessionDetails, {
      props: {
        session: {
          ...sampleSession,
          usage_input_tokens: 500,
          usage_output_tokens: 10,
          usage_cache_write_tokens: 0,
          usage_cache_read_tokens: 0,
          usage_cost_micros: 0,
          usage_model: 'local-llm-7b',
          usage_updated_at: 1,
        },
      },
    });
    await tick();
    const cost = screen.getByTestId('details-cost');
    expect(cost).toHaveTextContent('unpriced (local-llm-7b)');
    expect(cost).not.toHaveTextContent('$0.00');
  });

  it('hides the usage row when nothing was counted', async () => {
    render(SessionDetails, { props: { session: sampleSession } });
    await tick();
    expect(screen.queryByTestId('details-usage')).toBeNull();
  });

  it('shows the claude status chip and the context percentage when not stuck', async () => {
    render(SessionDetails, {
      props: { session: { ...sampleSession, claude_status: 'blocked', context_pct: 91 } },
    });
    await tick();
    expect(screen.getByTestId('details-claude-status')).toHaveTextContent('Needs you');
    const ctx = screen.getByTestId('details-context');
    expect(ctx).toHaveTextContent('91%');
    expect(ctx).toHaveAttribute('data-level', 'crit');
  });

  it('shows elapsed since started_at, the last prompt, the PR link and CI badge', async () => {
    const now = Math.floor(Date.now() / 1000);
    render(SessionDetails, {
      props: {
        session: {
          ...sampleSession,
          started_at: now - 2 * 86400 - 3600,
          last_turn_at: now - 30,
          last_prompt: 'Ship the GC sweeper',
          pr_url: 'https://github.com/martin-janci/claude-fleet/pull/42',
          ci_status: 'passing',
        },
      },
    });
    await tick();
    expect(screen.getByTestId('details-elapsed')).toHaveTextContent('2d 1h');
    expect(screen.getByTestId('details-last-turn')).toHaveTextContent('just now');
    expect(screen.getByTestId('details-last-prompt')).toHaveTextContent('Ship the GC sweeper');
    const pr = screen.getByTestId('details-pr');
    expect(pr.querySelector('a')).toHaveAttribute('href', 'https://github.com/martin-janci/claude-fleet/pull/42');
    expect(pr).toHaveTextContent('#42');
    expect(screen.getByTestId('details-ci')).toHaveTextContent('CI');
  });

  it('falls back to created_at for elapsed and hides the optional rows', async () => {
    const now = Math.floor(Date.now() / 1000);
    render(SessionDetails, { props: { session: { ...sampleSession, created_at: now - 90 } } });
    await tick();
    expect(screen.getByTestId('details-elapsed')).toHaveTextContent('1m');
    expect(screen.queryByTestId('details-last-prompt')).toBeNull();
    expect(screen.queryByTestId('details-pr')).toBeNull();
    expect(screen.queryByTestId('details-stuck')).toBeNull();
  });
});

describe('SessionDetails label editing and timeline', () => {
  const inv = () => mockedInvoke as ReturnType<typeof vi.fn>;
  const events = [
    { id: 3, session_id: 1, at: 1_700_000_300, kind: 'stuck', detail: 'auth_menu' },
    { id: 2, session_id: 1, at: 1_700_000_200, kind: 'prompt_sent', detail: 'fix the login bug' },
    { id: 1, session_id: 1, at: 1_700_000_100, kind: 'status_change', detail: 'working' },
  ];

  beforeEach(() => {
    inv().mockReset();
    inv().mockImplementation(async (cmd: string, args?: { args?: { friendly_name?: string } }) => {
      if (cmd === 'session_history') return events;
      if (cmd === 'set_session_friendly_name') {
        return { ...sampleSession, friendly_name: args?.args?.friendly_name || null };
      }
      return undefined;
    });
  });

  it('Rename and label… opens the sheet on the name the session has (G2.7)', async () => {
    render(SessionDetails, { props: { session: { ...sampleSession, friendly_name: 'Fix login' } } });
    await tick();
    await fireEvent.click(screen.getByTestId('label-from-details'));
    const input = (await screen.findByTestId('rename-label-name')) as HTMLInputElement;
    expect(input.value).toBe('Fix login');
    expect(screen.getByTestId('rename-label-sheet')).toBeTruthy();
  });

  it('Save writes the name through set_session_friendly_name', async () => {
    render(SessionDetails, { props: { session: sampleSession } });
    await tick();
    await fireEvent.click(await screen.findByTestId('label-from-details'));
    const input = await screen.findByTestId('rename-label-name');
    await fireEvent.input(input, { target: { value: 'New label' } });
    await fireEvent.click(screen.getByTestId('rename-label-save'));
    await tick();
    await Promise.resolve();
    await tick();
    const calls = inv().mock.calls;
    const call = calls.filter((c) => c[0] === 'set_session_friendly_name');
    expect(call).toHaveLength(1);
    expect(call[0][1]).toEqual({
      args: { host_alias: 'mefistos', tmux_name: 'dev-foo', friendly_name: 'New label' },
    });
    expect(calls.some((c) => c[0] === 'rename_session')).toBe(false);
    expect(calls.some((c) => c[0] === 'set_session_tags')).toBe(false);
  });

  it('shows the label (the session tags) among the facts', async () => {
    render(SessionDetails, { props: { session: { ...sampleSession, tags: ['release', 'wip'] } } });
    await tick();
    expect(screen.getByTestId('details-tags').textContent).toBe('releasewip');
  });

  it('Rename tmux session opens the tmux-name editor; Escape cancels', async () => {
    render(SessionDetails, { props: { session: sampleSession } });
    await tick();
    await fireEvent.click(await screen.findByTestId('rename-from-details'));
    const input = (await screen.findByTestId('details-rename')) as HTMLInputElement;
    expect(input.value).toBe('dev-foo');
    expect(document.activeElement).toBe(input);
    await fireEvent.keyDown(input, { key: 'Escape' });
    expect(screen.queryByTestId('details-rename')).toBeNull();
    expect(inv().mock.calls.some((c) => c[0] === 'rename_session')).toBe(false);
  });

  it('renders the session_history timeline with filter chips', async () => {
    render(SessionDetails, { props: { session: sampleSession } });
    const rows = await screen.findAllByTestId('timeline-event');
    expect(rows.map((r) => r.getAttribute('data-kind'))).toEqual(['stuck', 'prompt_sent', 'status_change']);
    expect(inv().mock.calls.find((c) => c[0] === 'session_history')![1]).toEqual({
      args: { session_id: 1, limit: null },
    });
    const errors = screen.getByTestId('timeline-chip-errors');
    await fireEvent.click(errors);
    expect(errors.getAttribute('aria-pressed')).toBe('true');
    expect(screen.getAllByTestId('timeline-event').map((r) => r.getAttribute('data-kind'))).toEqual(['stuck']);
    // Chips are additive: swap errors for ops, which matches nothing here.
    await fireEvent.click(errors);
    await fireEvent.click(screen.getByTestId('timeline-chip-ops'));
    expect(screen.getByTestId('timeline-empty').textContent).toContain('No events match');
  });

  it('shows an empty state when nothing was recorded', async () => {
    inv().mockImplementation(async (cmd: string) => (cmd === 'session_history' ? [] : undefined));
    render(SessionDetails, { props: { session: sampleSession } });
    expect((await screen.findByTestId('timeline-empty')).textContent).toContain('No events recorded');
  });
});

describe('SessionDetails Remove from list (inactive bg agents)', () => {
  const inv = () => mockedInvoke as ReturnType<typeof vi.fn>;

  beforeEach(() => {
    inv().mockReset();
    inv().mockImplementation(async () => undefined);
  });

  it('shows Remove from list for a stopped bg row and calls dismiss_agent_session', async () => {
    const row = { ...sampleSession, id: 9, kind: 'bg', tmux_name: 'bg:c9', claude_status: 'stopped' as const };
    render(SessionDetails, { props: { session: row } });
    await tick();
    const btn = await screen.findByTestId('remove-from-list-details');
    expect(btn.textContent).toContain('Remove from list');
    inv().mockResolvedValueOnce(null);
    await fireEvent.click(btn);
    await tick();
    expect(inv().mock.calls).toContainEqual(['dismiss_agent_session', { args: { session_id: 9 } }]);
  });

  it('surfaces a failed removal as an error toast', async () => {
    const { toasts, clearToasts } = await import('./toasts');
    const { get } = await import('svelte/store');
    clearToasts();
    inv().mockImplementation(async (cmd: string) => {
      if (cmd === 'dismiss_agent_session') throw { code: 'E_INVALID_STATE', message: 'still working' };
      return undefined;
    });
    const row = { ...sampleSession, id: 9, kind: 'bg', tmux_name: 'bg:c9', claude_status: 'stopped' as const };
    render(SessionDetails, { props: { session: row } });
    await fireEvent.click(await screen.findByTestId('remove-from-list-details'));
    await tick();
    await tick();
    const err = get(toasts).find((t) => t.kind === 'error');
    expect(err?.message).toContain('still working');
  });

  it('hides it for a live bg row, an external row and a tmux row', async () => {
    for (const row of [
      { ...sampleSession, id: 10, kind: 'bg', tmux_name: 'bg:c10', claude_status: 'working' as const },
      { ...sampleSession, id: 11, kind: 'external', tmux_name: 'bg:c11', claude_status: 'stopped' as const },
      { ...sampleSession, id: 12, claude_status: 'stopped' as const },
    ]) {
      const { unmount } = render(SessionDetails, { props: { session: row } });
      await tick();
      expect(screen.queryByTestId('remove-from-list-details')).toBeNull();
      unmount();
    }
  });
});

describe('SessionDetails actions for pane-less rows (external read-only, inactive agents)', () => {
  const ACTION_IDS = [
    'rename-from-details',
    'restart-from-details',
    'repair-from-details',
    'send-prompt-from-details',
    'open-review',
    'recreate-from-details',
    'move-from-details',
    'remove-from-list-details',
    'safe-kill-from-details',
    'kill-from-details',
  ];

  it('an external row shows no action except Rename, and no tmux attach command', async () => {
    const ext = {
      ...sampleSession,
      id: 21,
      kind: 'external',
      tmux_name: 'bg:ext-1',
      project_id: 1,
      worktree_id: 10,
      claude_session_id: 'ext-1',
      claude_status: 'working' as const,
    };
    render(SessionDetails, { props: { session: ext } });
    await tick();
    expect(screen.getByTestId('label-from-details')).toBeTruthy();
    for (const id of ACTION_IDS) {
      expect(screen.queryByTestId(id), id).toBeNull();
    }
    expect(screen.queryByTestId('attach-command')).toBeNull();
    expect(screen.queryByTestId('copy-attach')).toBeNull();
  });

  it('a bg row does not offer a tmux attach command', async () => {
    const bg = { ...sampleSession, id: 22, kind: 'bg', tmux_name: 'bg:c22', claude_status: 'working' as const };
    render(SessionDetails, { props: { session: bg } });
    await tick();
    expect(screen.queryByTestId('attach-command')).toBeNull();
    // A live agent keeps its stop path.
    expect(screen.getByTestId('kill-from-details')).toBeTruthy();
  });

  it('an inactive bg agent offers Remove from list as its only removal action', async () => {
    const bg = { ...sampleSession, id: 23, kind: 'bg', tmux_name: 'bg:c23', claude_status: 'stopped' as const };
    render(SessionDetails, { props: { session: bg } });
    await tick();
    expect(screen.getByTestId('remove-from-list-details')).toBeTruthy();
    expect(screen.queryByTestId('kill-from-details')).toBeNull();
    expect(screen.queryByTestId('safe-kill-from-details')).toBeNull();
  });

  it('a tmux row still shows its attach command and Kill', async () => {
    render(SessionDetails, { props: { session: sampleSession } });
    await tick();
    expect(screen.getByTestId('attach-command').textContent).toBe('tmux attach -t dev-foo');
    expect(screen.getByTestId('kill-from-details')).toBeTruthy();
  });
});

describe('SessionDetails recovery actions from the timeline', () => {
  const inv = () => mockedInvoke as ReturnType<typeof vi.fn>;
  function row(over: Partial<SessionRow> = {}): SessionRow {
    return { ...sampleSession, ...over } as SessionRow;
  }

  beforeEach(() => {
    inv().mockReset();
    (startMove as ReturnType<typeof vi.fn>).mockReset();
    (resolveMoveRun as ReturnType<typeof vi.fn>).mockReset();
    transferSheetFor.set(null);
    resetMovesForTest();
  });

  it('offers Move back when the session was moved here', async () => {
    const events = [
      {
        id: 1,
        session_id: 8,
        at: 1700000000,
        kind: 'session_moved',
        detail: JSON.stringify({ from_host: 'alpha', to_host: 'beta', claude_session_id: 'c1' }),
        claude_session_id: null,
      },
    ];
    inv().mockImplementation(async (cmd: string) => (cmd === 'session_history' ? events : undefined));
    const { getByTestId } = render(SessionDetails, { props: { session: row({ id: 8, host_alias: 'beta' }) } });
    await waitFor(() => expect(getByTestId('details-move-back')).toBeTruthy());
    expect(getByTestId('details-move-back').textContent).toContain('alpha');
    await fireEvent.click(getByTestId('details-move-back'));
    expect(startMove).toHaveBeenCalledWith(expect.objectContaining({ id: 8 }), 'alpha', {
      keepSource: false,
    });
  });

  it('offers Finish and Undo for an unresolved partial', async () => {
    const events = [
      {
        id: 1,
        session_id: 8,
        at: 1700000000,
        kind: 'session_move_partial',
        detail: JSON.stringify({
          step: 'killing the source s on alpha',
          from_host: 'alpha',
          to_host: 'beta',
          from_session_id: 7,
          to_session_id: 8,
        }),
        claude_session_id: null,
      },
    ];
    inv().mockImplementation(async (cmd: string) => (cmd === 'session_history' ? events : undefined));
    const { getByTestId } = render(SessionDetails, { props: { session: row({ id: 8, host_alias: 'beta' }) } });
    await waitFor(() => expect(getByTestId('details-finish-move')).toBeTruthy());
    expect(getByTestId('details-undo-move')).toBeTruthy();
  });

  // Whole-branch review, finding 2: `session_moved` is recorded on BOTH rows
  // and a `keep_source` move leaves the origin running this very conversation
  // in the same worktree — so "Move back" there would aim a transfer at that
  // live session's own worktree. One `$sessions` check covers both shapes:
  // the target's panel after a kept-source move, and the source's own panel
  // (whose copy of the event names the host it is already on).
  it('does not offer Move back while the origin still runs this conversation', async () => {
    const moved = [
      {
        id: 1,
        session_id: 8,
        at: 1700000000,
        kind: 'session_moved',
        detail: JSON.stringify({ from_host: 'alpha', to_host: 'beta', claude_session_id: 'c1' }),
        claude_session_id: null,
      },
    ];
    inv().mockImplementation(async (cmd: string) => (cmd === 'session_history' ? moved : undefined));
    const targetRow = row({ id: 8, host_alias: 'beta', claude_session_id: 'c1' });
    sessions.set([targetRow]);
    const { getByTestId, queryByTestId } = render(SessionDetails, { props: { session: targetRow } });
    // The source is gone (an ordinary move killed it): the trip back is on.
    await waitFor(() => expect(getByTestId('details-move-back')).toBeTruthy());
    // …and it goes away the moment a live session on that host is holding
    // the same conversation.
    sessions.set([
      targetRow,
      row({ id: 7, host_alias: 'alpha', claude_session_id: 'c1', status: 'running' }),
    ]);
    await waitFor(() => expect(queryByTestId('details-move-back')).toBeNull());
  });

  it("does not offer the source's own panel a move back to where it already is", async () => {
    const moved = [
      {
        id: 1,
        session_id: 7,
        at: 1700000000,
        kind: 'session_moved',
        detail: JSON.stringify({ from_host: 'alpha', to_host: 'beta', claude_session_id: 'c1' }),
        claude_session_id: null,
      },
    ];
    inv().mockImplementation(async (cmd: string) => (cmd === 'session_history' ? moved : undefined));
    const sourceRow = row({ id: 7, host_alias: 'alpha', claude_session_id: 'c1', status: 'running' });
    sessions.set([sourceRow, row({ id: 8, host_alias: 'beta', claude_session_id: 'c1' })]);
    const { queryByTestId, findByTestId } = render(SessionDetails, { props: { session: sourceRow } });
    // Wait for the timeline to have been read before asserting on an absence.
    await findByTestId('session-host');
    await waitFor(() => expect(inv()).toHaveBeenCalled());
    await tick();
    expect(queryByTestId('details-move-back')).toBeNull();
  });

  // The id-matching between `adoptPartial`'s key and the id the sheet opens
  // on, driven rather than hand-traced — and the panel must never resolve a
  // move itself: the sheet owns the confirmations.
  it('Finish and Undo hand the partial to the sheet, and resolve nothing themselves', async () => {
    const events = [
      {
        id: 1,
        session_id: 8,
        at: 1700000000,
        kind: 'session_move_partial',
        detail: JSON.stringify({
          step: 'killing the source s on alpha',
          from_host: 'alpha',
          to_host: 'beta',
          from_session_id: 7,
          to_session_id: 8,
          kept_source: true,
        }),
        claude_session_id: null,
      },
    ];
    inv().mockImplementation(async (cmd: string) => (cmd === 'session_history' ? events : undefined));
    const targetRow = row({ id: 8, tmux_name: 'sess8', host_alias: 'beta' });
    sessions.set([targetRow]);
    const { getByTestId } = render(SessionDetails, { props: { session: targetRow } });
    await waitFor(() => expect(getByTestId('details-finish-move')).toBeTruthy());

    await fireEvent.click(getByTestId('details-finish-move'));
    // Keyed by the SOURCE id — which is the id the sheet is opened on.
    expect(get(transferSheetFor)).toBe(7);
    const run = get(moves).get(7)!;
    expect(run.status).toBe('partial');
    expect(run.toHost).toBe('beta');
    expect(run.keepSource).toBe(true);
    expect(resolveMoveRun).not.toHaveBeenCalled();

    // Undo goes to the same place: neither button kills anything from here.
    transferSheetFor.set(null);
    await fireEvent.click(getByTestId('details-undo-move'));
    expect(get(transferSheetFor)).toBe(7);
    expect(resolveMoveRun).not.toHaveBeenCalled();
  });

  it('offers neither when the timeline has no move in it', async () => {
    inv().mockImplementation(async (cmd: string) => (cmd === 'session_history' ? [] : undefined));
    const { queryByTestId } = render(SessionDetails, { props: { session: row({ id: 8 }) } });
    await waitFor(() => expect(queryByTestId('details-move-back')).toBeNull());
    expect(queryByTestId('details-finish-move')).toBeNull();
    expect(queryByTestId('details-resume-wait')).toBeNull();
  });

  // Task 8: after a restart there is no in-memory run for a wait that was
  // still pending — the durable record is the source's own timeline, exactly
  // like `unresolvedPartial`/`adoptPartial` above.
  it('offers to resume an unresolved wait, and hands it to the sheet', async () => {
    const events = [
      {
        id: 1,
        session_id: 8,
        at: 1700000000,
        kind: 'session_move_waiting',
        detail: JSON.stringify({ to_host: 'beta', deadline_unix: 1700003600 }),
        claude_session_id: null,
      },
    ];
    inv().mockImplementation(async (cmd: string) => (cmd === 'session_history' ? events : undefined));
    const waitingRow = row({ id: 8, host_alias: 'alpha', tmux_name: 'sess8' });
    sessions.set([waitingRow]);
    const { getByTestId } = render(SessionDetails, { props: { session: waitingRow } });
    await waitFor(() => expect(getByTestId('details-resume-wait')).toBeTruthy());
    expect(getByTestId('details-resume-wait').textContent).toContain('beta');
    await fireEvent.click(getByTestId('details-resume-wait'));
    expect(get(transferSheetFor)).toBe(8);
    const run = get(moves).get(8)!;
    expect(run.status).toBe('waiting');
    expect(run.toHost).toBe('beta');
  });

  // A `session_move_wait_ended` closes the record — nothing left to resume.
  it('does not offer to resume a wait that already ended', async () => {
    const events = [
      {
        id: 2,
        session_id: 8,
        at: 1700000100,
        kind: 'session_move_wait_ended',
        detail: JSON.stringify({ reason: 'cancelled' }),
        claude_session_id: null,
      },
      {
        id: 1,
        session_id: 8,
        at: 1700000000,
        kind: 'session_move_waiting',
        detail: JSON.stringify({ to_host: 'beta', deadline_unix: 1700003600 }),
        claude_session_id: null,
      },
    ];
    inv().mockImplementation(async (cmd: string) => (cmd === 'session_history' ? events : undefined));
    const { queryByTestId, findByTestId } = render(SessionDetails, {
      props: { session: row({ id: 8, host_alias: 'alpha' }) },
    });
    await findByTestId('session-host');
    await waitFor(() => expect(inv()).toHaveBeenCalled());
    await tick();
    expect(queryByTestId('details-resume-wait')).toBeNull();
  });
});


// Multi-user M1: "sharing never confers a terminal" (spec §4.3 invariant 4)
// reaches further than `pty_open`. This section hands over the incantation and
// the tmux name — which §4.3 counts as content — for the person to paste into
// their own shell, where no hub is in the path and nothing can refuse it. It
// invokes nothing, which is exactly why it is easy to miss.
describe('SessionDetails attach command against a grant', () => {
  const ME = 7;
  const OTHER = 9;
  const remote: HubStatus = {
    ...STANDALONE,
    remote: true,
    url: 'https://fleet.example.com',
    client_name: 'laptop',
    configured_url: 'https://fleet.example.com',
    configured_client_name: 'laptop',
  };
  const theirs = { ...sampleSession, id: 31, owner_person_id: OTHER, visibility: 'private' as const };

  it('an owned session still offers it, on a paired desktop as on a standalone one', async () => {
    hubStatus.set(remote);
    setMyGrants(ME, []);
    const mine = { ...sampleSession, id: 32, owner_person_id: ME, visibility: 'private' as const };
    render(SessionDetails, { props: { session: mine } });
    await tick();
    expect(screen.getByTestId('attach-command').textContent).toBe('tmux attach -t dev-foo');
    expect(screen.getByTestId('copy-attach')).toBeTruthy();
  });

  it.each([['watch'], ['drive']] as const)(
    'a %s-granted session offers neither the command nor the copy button',
    async (level) => {
      hubStatus.set(remote);
      setMyGrants(ME, [{ session_id: theirs.id, level }]);
      render(SessionDetails, { props: { session: theirs } });
      await tick();
      expect(screen.queryByTestId('attach-command')).toBeNull();
      expect(screen.queryByTestId('copy-attach')).toBeNull();
    },
  );

  it('takes it away on a revoke with no row event, and never says the row has no pane', async () => {
    hubStatus.set(remote);
    setMyGrants(ME, [{ session_id: theirs.id, level: 'drive' }]);
    render(SessionDetails, { props: { session: theirs } });
    await tick();
    expect(screen.queryByTestId('attach-command')).toBeNull();
    // Still absent after the revoke — and the panel's other sections are
    // untouched, so the gate is the section's own and not a pane-less row's.
    applyGrantChanges([{ session_id: theirs.id, person_id: ME, level: null }]);
    await tick();
    expect(screen.queryByTestId('attach-command')).toBeNull();
    expect(screen.getByTestId('session-host')).toBeTruthy();
  });
});

describe('SessionDetails Share… and the per-session action gate (multi-user M1)', () => {
  const ME = 7;
  const remote: HubStatus = {
    ...STANDALONE,
    remote: true,
    url: 'https://fleet.example.com',
    client_name: 'laptop',
    configured_url: 'https://fleet.example.com',
    configured_client_name: 'laptop',
  };
  const mine = { ...sampleSession, id: 41, owner_person_id: ME, visibility: 'private' as const };
  const theirs = { ...sampleSession, id: 42, owner_person_id: 9, visibility: 'private' as const };
  const dis = (testid: string) => (screen.getByTestId(testid) as HTMLButtonElement).disabled;

  it('opens the app’s one Share sheet, the way Move to host… opens the Transfer sheet', async () => {
    shareSheetFor.set(null);
    render(SessionDetails, { props: { session: mine } });
    await tick();
    const btn = screen.getByTestId('share-from-details') as HTMLButtonElement;
    expect(btn.disabled).toBe(false);
    await fireEvent.click(btn);
    expect(get(shareSheetFor)).toBe(41);
  });

  it('a grantee is offered Share… only as a disabled control with the reason on it', async () => {
    // "A grantee cannot grant on" (spec §4.3 invariant 2), in front of the
    // control rather than at the click.
    hubStatus.set(remote);
    setMyGrants(ME, [{ session_id: theirs.id, level: 'drive' }]);
    render(SessionDetails, { props: { session: theirs } });
    await tick();
    expect(dis('share-from-details')).toBe(true);
    expect(screen.getByTestId('share-from-details').title).toMatch(/only the session’s owner/i);
  });

  it('the owner’s Share… is disabled while the paired hub is unreachable', async () => {
    // Multi-user M1, F3: `session_share` routes to the hub, so the button
    // asks the live link as well as the access half. The row IS this
    // person's, so the sentence must be the offline one — opening a sheet
    // whose every button then fails is the failure this prevents.
    hubStatus.set(remote);
    hubConnection.set({ state: 'offline', attempt: 2, retry_in_secs: 5, reason: 'refused' });
    setMyGrants(ME, []);
    render(SessionDetails, { props: { session: mine } });
    await tick();
    expect(dis('share-from-details')).toBe(true);
    expect(screen.getByTestId('share-from-details').title).toMatch(/unreachable right now/i);
  });

  it('a driver keeps Send prompt and loses the own-tier actions', async () => {
    hubStatus.set(remote);
    setMyGrants(ME, [{ session_id: theirs.id, level: 'drive' }]);
    render(SessionDetails, { props: { session: theirs } });
    await tick();
    expect(dis('send-prompt-from-details')).toBe(false);
    for (const t of [
      'kill-from-details',
      'restart-from-details',
      'recreate-from-details',
      'rename-from-details',
      'open-review',
      'safe-kill-from-details',
    ]) {
      expect(dis(t), t).toBe(true);
    }
    // Spec §4.3: reading the label is content, but WRITING it is not an
    // owner-only act — it is the sidebar caption, which fleet rewrites itself
    // without asking anybody — so a driver may still change it.
    expect(dis('label-from-details')).toBe(false);
  });

  it('a watcher loses Send prompt too, and a narrow takes it away with no row event', async () => {
    hubStatus.set(remote);
    setMyGrants(ME, [{ session_id: theirs.id, level: 'drive' }]);
    render(SessionDetails, { props: { session: theirs } });
    await tick();
    expect(dis('send-prompt-from-details')).toBe(false);
    // A narrow moves no column on the row: the only thing that changes is the
    // grant map, which is exactly why this re-disables without a re-list.
    applyGrantChanges([{ session_id: theirs.id, person_id: ME, level: 'watch' }]);
    await tick();
    expect(dis('send-prompt-from-details')).toBe(true);
    expect(screen.getByTestId('send-prompt-from-details').title).toMatch(/needs drive/i);
    expect(dis('label-from-details')).toBe(true);
  });

  // Spec §4.3 puts the whole move LIFECYCLE in the `own` tier, and "Move to
  // host…" was gated on that from F2 — but the four controls that move a
  // session BACK, finish a half-done move, undo one, or reopen a pending wait
  // had no gate at all, so a grantee could finish or undo the owner's move,
  // which kills a session. Same `moveBlocked`, same four controls.
  const MOVED = [
    {
      id: 1,
      session_id: 42,
      at: 1700000000,
      kind: 'session_moved',
      detail: JSON.stringify({ from_host: 'alpha', to_host: 'beta', claude_session_id: 'c1' }),
      claude_session_id: null,
    },
  ];
  const PARTIAL = [
    {
      id: 1,
      session_id: 42,
      at: 1700000000,
      kind: 'session_move_partial',
      detail: JSON.stringify({
        step: 'killing the source s on alpha',
        from_host: 'alpha',
        to_host: 'beta',
        from_session_id: 41,
        to_session_id: 42,
      }),
      claude_session_id: null,
    },
  ];
  const WAITING = [
    {
      id: 1,
      session_id: 42,
      at: 1700000000,
      kind: 'session_move_waiting',
      detail: JSON.stringify({ to_host: 'beta', deadline: 1700000600 }),
      claude_session_id: null,
    },
  ];

  function withHistory(events: unknown[]) {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) =>
      cmd === 'session_history' ? events : undefined,
    );
  }

  it('a grantee cannot move back, finish, undo or resume a wait', async () => {
    hubStatus.set(remote);
    setMyGrants(ME, [{ session_id: theirs.id, level: 'drive' }]);
    (startMove as ReturnType<typeof vi.fn>).mockReset();
    for (const [events, testid] of [
      [MOVED, 'details-move-back'],
      [PARTIAL, 'details-finish-move'],
      [PARTIAL, 'details-undo-move'],
      [WAITING, 'details-resume-wait'],
    ] as const) {
      withHistory(events as unknown[]);
      transferSheetFor.set(null);
      const { unmount } = render(SessionDetails, { props: { session: theirs } });
      await waitFor(() => expect(screen.getByTestId(testid)).toBeTruthy());
      expect(dis(testid), testid).toBe(true);
      expect(screen.getByTestId(testid).title, testid).toMatch(/only the session’s owner/i);
      // And a click that lands anyway neither starts a move nor opens the sheet.
      await fireEvent.click(screen.getByTestId(testid));
      await tick();
      expect(startMove, testid).not.toHaveBeenCalled();
      expect(get(transferSheetFor), testid).toBeNull();
      unmount();
    }
  });

  it('the owner keeps all four, on the same rows', async () => {
    hubStatus.set(remote);
    setMyGrants(ME, []);
    for (const [events, testid] of [
      [MOVED, 'details-move-back'],
      [PARTIAL, 'details-finish-move'],
      [PARTIAL, 'details-undo-move'],
      [WAITING, 'details-resume-wait'],
    ] as const) {
      withHistory(events as unknown[]);
      const { unmount } = render(SessionDetails, {
        props: { session: { ...theirs, owner_person_id: ME } },
      });
      await waitFor(() => expect(screen.getByTestId(testid)).toBeTruthy());
      expect(dis(testid), testid).toBe(false);
      unmount();
    }
  });

  it('leaves a standalone desktop’s panel exactly as it was', async () => {
    // The single-user shape: no `my_grants` answer at all, and every control
    // still enabled — `access.ts`'s rule 1 is what makes that true.
    render(SessionDetails, { props: { session: theirs } });
    await tick();
    for (const t of [
      'kill-from-details',
      'restart-from-details',
      'recreate-from-details',
      'send-prompt-from-details',
      'open-review',
      'share-from-details',
    ]) {
      expect(dis(t), t).toBe(false);
    }
  });
});

describe('SessionDetails action hierarchy (redesign 1.5)', () => {
  it('one primary, actions grouped Steer / Place / Share, and Kill last (SessionDetails board)', async () => {
    render(SessionDetails, { props: { session: sampleSession } });
    await tick();
    // Exactly one primary in the view, and it is Send prompt.
    const primaries = document.querySelectorAll('[data-testid="session-details"] .btn--primary');
    expect(primaries).toHaveLength(1);
    expect(primaries[0]).toBe(screen.getByTestId('send-prompt-from-details'));
    const ids = (group: string) =>
      Array.from(screen.getByTestId(group).querySelectorAll('button')).map((el) => el.getAttribute('data-testid'));
    expect(ids('actions-steer')).toEqual([
      'send-prompt-from-details',
      'open-review',
      'switch-account-from-details',
      'change-model-from-details',
      'restart-from-details',
    ]);
    expect(ids('actions-place')).toEqual(
      expect.arrayContaining(['recreate-from-details', 'label-from-details', 'rename-from-details']),
    );
    expect(ids('actions-share')[0]).toBe('share-from-details');
    // The destructive ones sit apart, Kill last.
    const all = Array.from(screen.getByTestId('details-actions').querySelectorAll('button')).map((el) =>
      el.getAttribute('data-testid'),
    );
    expect(all.at(-1)).toBe('kill-from-details');
    expect(all.at(-2)).toBe('safe-kill-from-details');
    expect(screen.getByTestId('kill-from-details').classList.contains('kill')).toBe(true);
    expect(screen.queryByTestId('details-more')).toBeNull();
  });

  it('the inspector keeps the everyday actions and points at Details for the rest (Main board)', async () => {
    render(SessionDetails, { props: { session: sampleSession, variant: 'inspector' } });
    await tick();
    expect(screen.getByTestId('session-details').dataset.variant).toBe('inspector');
    expect(screen.getByTestId('inspector-facts')).toBeTruthy();
    for (const id of ['send-prompt-from-details', 'open-review', 'restart-from-details', 'share-from-details', 'kill-from-details']) {
      expect(screen.getByTestId(id), id).toBeTruthy();
    }
    expect(screen.queryByTestId('recreate-from-details')).toBeNull();
    expect(screen.getByTestId('inspector-open-details')).toBeTruthy();
  });

  it('Kill still asks before it kills', async () => {
    render(SessionDetails, { props: { session: sampleSession } });
    await tick();
    await fireEvent.click(screen.getByTestId('kill-from-details'));
    await tick();
    expect(screen.getByTestId('confirm-kill-details')).toBeTruthy();
    expect(vi.mocked(mockedInvoke).mock.calls.some((c) => c[0] === 'kill_session')).toBe(false);
  });
});

describe('SessionDetails accessibility (7.2)', () => {
  it('passes the axe and audit checks', async () => {
    hosts.set([
      { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false, transport: 'ssh' },
    ]);
    const { container } = render(SessionDetails, { props: { session: sampleSession } });
    await screen.findByTestId('session-host');
    await expectAccessible(container);
  });
});
