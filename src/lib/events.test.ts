import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/event', () => {
  const handlers = new Map<string, (e: { payload: unknown }) => void>();
  return {
    listen: vi.fn(async (name: string, cb: (e: { payload: unknown }) => void) => {
      handlers.set(name, cb);
      return () => handlers.delete(name);
    }),
    emit: vi.fn(async (name: string, payload: unknown) => {
      handlers.get(name)?.({ payload });
    }),
  };
});

import { emit } from '@tauri-apps/api/event';
import { get } from 'svelte/store';
import { subscribeToRowEvents } from './events';
import { sessions, mergeSession, removeSession, applySessionEvents, type SessionRow } from './sessions';
import { hosts, applyHostEvents } from './hosts';

function row(over: Partial<SessionRow> = {}): SessionRow {
  return {
    id: 42,
    tmux_name: 't',
    host_alias: 'h',
    project_id: null,
    worktree_id: null,
    created_at: 0,
    last_activity_at: 0,
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
    safe_kill_state: null,
    safe_kill_nonce: null,
    safe_kill_detail: null,
    safe_kill_requested_at: null,
    ...over,
  };
}

// Fire without awaiting so the handler's enqueue happens synchronously; the
// microtask flush runs before the awaited continuation below.
const fire = (name: string, payload: unknown) => void vi.mocked(emit)(name, payload);
const flush = () => Promise.resolve();

describe('subscribeToRowEvents', () => {
  beforeEach(() => {
    vi.mocked(emit).mockClear();
  });

  it('fires onSessionCreated when session:created is emitted', async () => {
    const seen: number[] = [];
    await subscribeToRowEvents({
      onSessionCreated: (row) => seen.push(row.id),
    });
    await vi.mocked(emit)('session:created', row({ id: 42 }));
    expect(seen).toEqual([42]);
  });

  it('fires onSessionKilled with id payload', async () => {
    const killed: number[] = [];
    await subscribeToRowEvents({
      onSessionKilled: (p) => killed.push(p.id),
    });
    await vi.mocked(emit)('session:killed', { id: 99 });
    expect(killed).toEqual([99]);
  });

  it('returns unsubscribe that detaches all listeners', async () => {
    const seen: number[] = [];
    const unlisten = await subscribeToRowEvents({
      onSessionCreated: (row) => seen.push(row.id),
    });
    unlisten();
    await vi.mocked(emit)('session:created', row({ id: 1 }));
    expect(seen).toEqual([]);
  });

  it('drops events still queued when unsubscribed before the flush', async () => {
    const seen: number[] = [];
    const unlisten = await subscribeToRowEvents({
      onSessionCreated: (row) => seen.push(row.id),
    });
    fire('session:created', row({ id: 1 }));
    unlisten();
    await flush();
    expect(seen).toEqual([]);
  });
});

// End-to-end: emit → handler → store update.
describe('subscribeToRowEvents → store integration', () => {
  it('session:created event updates the sessions store via mergeSession', async () => {
    sessions.set([]);
    await subscribeToRowEvents({
      onSessionCreated: mergeSession,
      onSessionKilled: (p) => removeSession(p.id),
    });
    await vi.mocked(emit)('session:created', row({ id: 7, tmux_name: 'dev-test', host_alias: 'local', created_at: 1, last_activity_at: 1 }));
    expect(get(sessions).map((s) => s.id)).toEqual([7]);
    await vi.mocked(emit)('session:killed', { id: 7 });
    expect(get(sessions)).toEqual([]);
  });
});

// FE-10: the reconcile tick emits one session:updated per session; each one
// used to be its own store flush (60 sessions → 60 sidebar re-derives).
describe('row event batching', () => {
  beforeEach(() => {
    sessions.set([]);
    hosts.set([]);
  });

  it('coalesces 60 synchronous session:updated events into one store notification', async () => {
    // ids 1000+: tombstones from earlier tests in this file (a killed id stays
    // dead for 5 s) must not collide with this batch.
    const seed = Array.from({ length: 60 }, (_, i) =>
      row({ id: 1000 + i, tmux_name: `s${i}`, last_activity_at: 1 }),
    );
    sessions.set(seed);
    const unlisten = await subscribeToRowEvents({ onSessionEvents: applySessionEvents });
    const notify = vi.fn();
    const unsub = sessions.subscribe(notify);
    notify.mockClear(); // drop the initial subscribe call

    for (let i = 0; i < 60; i++) {
      fire('session:updated', row({ id: 1000 + i, tmux_name: `s${i}`, last_activity_at: 2, claude_status: 'working' }));
    }
    // Nothing applied yet — delivery is deferred to the microtask flush.
    expect(notify).not.toHaveBeenCalled();
    await flush();

    expect(notify).toHaveBeenCalledTimes(1);
    const final = get(sessions);
    expect(final).toHaveLength(60);
    expect(final.every((s) => s.claude_status === 'working' && s.last_activity_at === 2)).toBe(true);
    unsub();
    unlisten();
  });

  it('preserves order: a killed after an updated of the same id removes the row', async () => {
    sessions.set([row({ id: 1, last_activity_at: 1 })]);
    const unlisten = await subscribeToRowEvents({ onSessionEvents: applySessionEvents });
    fire('session:updated', row({ id: 1, last_activity_at: 2, claude_status: 'working' }));
    fire('session:created', row({ id: 2, tmux_name: 'other', last_activity_at: 2 }));
    fire('session:killed', { id: 1 });
    await flush();
    expect(get(sessions).map((s) => s.id)).toEqual([2]);
    unlisten();
  });

  it('preserves order: an updated after a killed of the same id stays dead', async () => {
    sessions.set([row({ id: 1, last_activity_at: 1 })]);
    const unlisten = await subscribeToRowEvents({ onSessionEvents: applySessionEvents });
    fire('session:killed', { id: 1 });
    fire('session:updated', row({ id: 1, last_activity_at: 5 }));
    await flush();
    expect(get(sessions)).toEqual([]);
    unlisten();
  });

  it('applies created then updated in one batch with the last write winning', async () => {
    const unlisten = await subscribeToRowEvents({ onSessionEvents: applySessionEvents });
    fire('session:created', row({ id: 3, last_activity_at: 1, claude_status: 'idle' }));
    fire('session:updated', row({ id: 3, last_activity_at: 2, claude_status: 'working' }));
    fire('session:updated', row({ id: 3, last_activity_at: 3, claude_status: 'blocked' }));
    await flush();
    const s = get(sessions);
    expect(s).toHaveLength(1);
    expect(s[0].claude_status).toBe('blocked');
    unlisten();
  });

  it('still invokes per-event handlers in arrival order alongside the batch handler', async () => {
    const order: string[] = [];
    const unlisten = await subscribeToRowEvents({
      onSessionCreated: (r) => order.push(`created:${r.id}`),
      onSessionKilled: (p) => order.push(`killed:${p.id}`),
      onSessionEvents: (evs) => order.push(`batch:${evs.length}`),
    });
    fire('session:created', row({ id: 1 }));
    fire('session:killed', { id: 1 });
    fire('session:created', row({ id: 2 }));
    await flush();
    expect(order).toEqual(['created:1', 'killed:1', 'created:2', 'batch:3']);
    unlisten();
  });

  it('routes host events to the host batch handler in one update', async () => {
    const unlisten = await subscribeToRowEvents({ onHostEvents: applyHostEvents });
    const notify = vi.fn();
    const unsub = hosts.subscribe(notify);
    notify.mockClear();
    const host = (alias: string) => ({
      alias, ssh_alias: alias, reachable: true, claude_version: null, tmux_version: null,
      hidden: false, last_pinged_at: null, account_uuid: null, provisioned: false,
    });
    fire('host:added', host('a'));
    fire('host:added', host('b'));
    fire('host:probed', { ...host('a'), reachable: false });
    fire('host:removed', { alias: 'b' });
    await flush();
    expect(notify).toHaveBeenCalledTimes(1);
    expect(get(hosts)).toEqual([{ ...host('a'), reachable: false }]);
    unsub();
    unlisten();
  });

  it('separate tasks produce separate flushes', async () => {
    const unlisten = await subscribeToRowEvents({ onSessionEvents: applySessionEvents });
    const notify = vi.fn();
    const unsub = sessions.subscribe(notify);
    notify.mockClear();
    fire('session:created', row({ id: 2001 }));
    await flush();
    fire('session:created', row({ id: 2002, tmux_name: 'b' }));
    await flush();
    expect(notify).toHaveBeenCalledTimes(2);
    expect(get(sessions).map((s) => s.id)).toEqual([2001, 2002]);
    unsub();
    unlisten();
  });
});
