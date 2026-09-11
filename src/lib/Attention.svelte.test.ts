import { render, screen } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import Attention from './Attention.svelte';
import { sessions, sessionsLoaded, type SessionRow } from './sessions';
import { toasts, clearToasts } from './toasts';
import { notifyStuckOs, notifyStuckToast } from './notify';

let nextId = 1;
function row(over: Partial<SessionRow> = {}): SessionRow {
  return {
    id: nextId++,
    tmux_name: 'dev-x',
    host_alias: 'local',
    project_id: 1,
    worktree_id: null,
    created_at: 1,
    last_activity_at: 1,
    status: 'running',
    notes: null,
    account_uuid: null,
    kind: 'work',
    reviews_session_id: null,
    worktree_key: 'main',
    lost_at: null,
    claude_session_id: null,
    claude_status: null,
    effort_level: null,
    pr_url: null,
    current_activity: null,
    context_pct: null,
    stuck_kind: null,
    friendly_name: null,
    safe_kill_state: null,
    safe_kill_nonce: null,
    safe_kill_detail: null,
    safe_kill_requested_at: null,
    idle_since: null,
    stuck_since: null,
    last_playbook_at: null,
    last_prompt: null,
    started_at: null,
    last_turn_at: null,
    ci_status: null, turn_seq: 0, last_stop_at: null, parent_session_id: null, tags: [],
    ...over,
  };
}

beforeEach(() => {
  sessions.set([]);
  sessionsLoaded.set(true);
  clearToasts();
  notifyStuckToast.set(true);
  notifyStuckOs.set(false);
});

describe('Attention', () => {
  it('announces a session that transitions into a stuck state, once', async () => {
    const a = row({ tmux_name: 'dev-a', host_alias: 'mefistos' });
    sessions.set([a]);
    render(Attention);
    await tick();
    const region = screen.getByTestId('stuck-announcer');
    expect(region).toHaveAttribute('aria-live', 'assertive');
    expect(region).toHaveTextContent('');

    sessions.set([{ ...a, stuck_kind: 'press_enter' }]);
    await tick();
    expect(region).toHaveTextContent('dev-a on mefistos is stuck: press Enter');
    expect(get(toasts).map((t) => t.message)).toEqual(['dev-a on mefistos is stuck: press Enter']);

    // Same stuck state on the next update: no second announcement/toast.
    sessions.set([{ ...a, stuck_kind: 'press_enter', last_activity_at: 2 }]);
    await tick();
    expect(get(toasts)).toHaveLength(1);
  });

  it('does not replay stuck rows delivered by the bootstrap fill', async () => {
    // App.svelte mounts the sidebar (and this watcher) before the first
    // list_sessions resolves: the store goes [] -> [rows]. That fill is
    // baseline, not a burst of transitions.
    sessionsLoaded.set(false);
    render(Attention);
    await tick();
    const a = row({ tmux_name: 'dev-boot', stuck_kind: 'oom' });
    const b = row({ tmux_name: 'dev-boot2', stuck_kind: 'trust_prompt' });
    sessions.set([a, b]);
    await tick();
    expect(screen.getByTestId('stuck-announcer')).toHaveTextContent('');
    expect(get(toasts)).toHaveLength(0);
    // Bootstrap completes; the same rows re-delivered are still baseline.
    sessionsLoaded.set(true);
    sessions.set([a, b]);
    await tick();
    expect(get(toasts)).toHaveLength(0);
    // A genuinely new transition after load is announced.
    sessions.set([a, b, row({ tmux_name: 'dev-new', stuck_kind: 'reconnect' })]);
    await tick();
    expect(get(toasts).map((t) => t.message)).toEqual(['dev-new on local is stuck: reconnecting']);
  });

  it('does not announce rows that were already stuck at mount', async () => {
    sessions.set([row({ stuck_kind: 'oom' })]);
    render(Attention);
    await tick();
    expect(screen.getByTestId('stuck-announcer')).toHaveTextContent('');
    expect(get(toasts)).toHaveLength(0);
  });

  it('respects the toast toggle but still fills the live region', async () => {
    notifyStuckToast.set(false);
    const a = row({ tmux_name: 'dev-b' });
    sessions.set([a]);
    render(Attention);
    await tick();
    sessions.set([{ ...a, stuck_kind: 'auth_menu' }]);
    await tick();
    expect(screen.getByTestId('stuck-announcer')).toHaveTextContent('dev-b on local is stuck: auth menu');
    expect(get(toasts)).toHaveLength(0);
  });
});
