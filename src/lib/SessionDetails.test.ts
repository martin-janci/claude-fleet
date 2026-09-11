import { render, screen } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import SessionDetails from './SessionDetails.svelte';
import { hosts } from './hosts';
import { accounts } from './accounts';
import { projects } from './projects';
import { sessions } from './sessions';

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
  friendly_name: null, safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null, safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null,
};

beforeEach(() => {
  hosts.set([]);
  accounts.set([]);
  projects.set([]);
  sessions.set([]);
});

describe('SessionDetails', () => {
  it('shows host alias from session', async () => {
    hosts.set([
      { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false },
    ]);
    render(SessionDetails, { props: { session: sampleSession } });
    await tick();
    expect((await screen.findByTestId('session-host')).textContent).toBe('mefistos');
  });

  it('shows account when host has one linked', async () => {
    hosts.set([
      { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: 'u1', provisioned: false },
    ]);
    accounts.set([
      { uuid: 'u1', email: 'm.janci@32bit.sk', display_name: 'M', organization_name: null, organization_uuid: null, seat_tier: 'max', last_seen_at: 1 },
    ]);
    render(SessionDetails, { props: { session: sampleSession } });
    await tick();
    const cell = await screen.findByTestId('session-account');
    expect(cell.textContent).toContain('m.janci@32bit.sk');
    expect(cell.textContent).toContain('max');
  });

  it('shows — when host has no account', async () => {
    hosts.set([
      { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false },
    ]);
    accounts.set([]);
    render(SessionDetails, { props: { session: sampleSession } });
    await tick();
    expect((await screen.findByTestId('session-account')).textContent?.trim()).toBe('—');
  });

  it('shows Related sessions panel when siblings exist', async () => {
    const source = { ...sampleSession, id: 1, project_id: 1, worktree_id: 10, worktree_key: 'main' };
    const sibling = { ...sampleSession, id: 2, tmux_name: 'dev-sib', host_alias: 'mefistos', project_id: 1, worktree_id: 10, worktree_key: 'main' };
    hosts.set([
      { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false },
    ]);
    accounts.set([]);
    sessions.set([source, sibling]);
    render(SessionDetails, { props: { session: source } });
    await tick();
    const rows = await screen.findAllByTestId('related-row');
    expect(rows).toHaveLength(1);
    expect(rows[0].textContent).toContain('dev-sib');
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
    (invoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      session_id: 7, host_alias: 'mefistos', tmux_name: 'dev-foo', cwd: '/r/.worktrees/x',
      healthy: false,
      actions: ['git worktree remove --force -- /r/.worktrees/x', 'tmux respawn-pane -k -c /r/.worktrees/x'],
      warnings: [], needs_explicit_repair: false, deferred: [], branch_source: 'branch_local',
      tmux: 'respawned', tmux_alive: true, tmux_dead: false,
      tmux_cwd_stale: false, worktree_row_updated: false, sibling_session_ids: [],
    });
    render(SessionDetails, { props: { session: { ...sampleSession, id: 7, project_id: 1 } } });
    await tick();
    const btn = await screen.findByTestId('repair-from-details');
    btn.click();
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
    expect(chip).toHaveTextContent('stuck: trust prompt');
    expect(chip).toHaveTextContent('2m');
    // Stuck outranks claude_status.
    expect(screen.queryByTestId('details-claude-status')).toBeNull();
  });

  it('shows the claude status chip and the context percentage when not stuck', async () => {
    render(SessionDetails, {
      props: { session: { ...sampleSession, claude_status: 'blocked', context_pct: 91 } },
    });
    await tick();
    expect(screen.getByTestId('details-claude-status')).toHaveTextContent('blocked');
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
    expect(pr).toHaveTextContent('martin-janci/claude-fleet/pull/42');
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
