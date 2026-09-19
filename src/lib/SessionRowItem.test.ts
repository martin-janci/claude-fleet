import { render, screen } from '@testing-library/svelte';
import { describe, it, expect, beforeEach } from 'vitest';
import { tick } from 'svelte';
import SessionRowItem from './SessionRowItem.svelte';
import { hosts } from './hosts';

const sampleSession = {
  id: 1,
  tmux_name: 'dev-foo',
  host_alias: 'mefistos',
  project_id: null,
  worktree_id: null,
  created_at: 1,
  last_activity_at: 1,
  status: 'ghost',
  notes: null,
  account_uuid: null,
  kind: 'work',
  reviews_session_id: null,
  worktree_key: null,
  lost_at: 1,
  lost_reason: null as string | null,
  claude_session_id: null,
  claude_status: null,
  effort_level: null,
  pr_url: null,
  current_activity: null,
  friendly_name: null, safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null, safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null, turn_seq: 0, last_stop_at: null, parent_session_id: null, tags: [],
};

const noop = () => {};

function baseProps(sess: typeof sampleSession) {
  return {
    sess,
    selectMode: false,
    isChecked: false,
    isRenaming: false,
    renameValue: '',
    renameInput: undefined,
    renameError: null,
    relatedCount: 0,
    nowSec: Math.floor(Date.now() / 1000),
    onSelectSession: noop,
    onKeySession: noop,
    toggleSelected: noop,
    beginRename: noop,
    beginLabelEdit: noop,
    onRenameKey: noop,
    commitRename: noop,
    askRecreate: noop,
    askKill: noop,
  };
}

beforeEach(() => {
  hosts.set([]);
});

describe('SessionRowItem ghost row lost_reason', () => {
  it('shows "host rebooted" for a lost_reason of host_reboot', async () => {
    render(SessionRowItem, {
      props: baseProps({ ...sampleSession, lost_reason: 'host_reboot' }),
    });
    await tick();
    const label = await screen.findByTestId('lost-reason');
    expect(label.textContent).toContain('host rebooted');
  });

  it('shows "tmux server stopped" for a lost_reason of tmux_server_gone', async () => {
    render(SessionRowItem, {
      props: baseProps({ ...sampleSession, lost_reason: 'tmux_server_gone' }),
    });
    await tick();
    const label = await screen.findByTestId('lost-reason');
    expect(label.textContent).toContain('tmux server stopped');
  });

  it('renders no lost-reason element when lost_reason is null', async () => {
    render(SessionRowItem, {
      props: baseProps({ ...sampleSession, lost_reason: null }),
    });
    await tick();
    expect(screen.queryByTestId('lost-reason')).toBeNull();
  });
});
