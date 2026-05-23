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
  friendly_name: null,
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
      { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: null },
    ]);
    render(SessionDetails, { props: { session: sampleSession } });
    await tick();
    expect((await screen.findByTestId('session-host')).textContent).toBe('mefistos');
  });

  it('shows account when host has one linked', async () => {
    hosts.set([
      { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: 'u1' },
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
      { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: null },
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
      { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: null },
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
