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
};

beforeEach(() => {
  hosts.set([]);
  accounts.set([]);
  projects.set([]);
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
});
