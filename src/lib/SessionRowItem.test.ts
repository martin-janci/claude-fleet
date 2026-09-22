import { render, screen } from '@testing-library/svelte';
import { describe, it, expect, beforeEach } from 'vitest';
import { tick } from 'svelte';
import SessionRowItem from './SessionRowItem.svelte';
import { hosts } from './hosts';
import { session } from './hosts_fixture';
import type { SessionRow } from './sessions';

// Built from the shared fixture rather than a hand-written literal: this
// file's own copy fell behind `SessionRow` twice (`model`, the context block,
// `pending_input`), and a stale literal only fails at type-check time.
const sampleSession: SessionRow = session('mefistos', 'dev-foo', {
  id: 1,
  status: 'ghost',
  claude_status: null,
  lost_at: 1,
  lost_reason: null,
  turn_seq: 0,
  last_stop_at: null,
});

const noop = () => {};

function baseProps(sess: SessionRow) {
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
