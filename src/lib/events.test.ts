import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

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
import { subscribeToRowEvents, ROW_EVENT_FLUSH_MS } from './events';
import {
  sessions,
  mergeSession,
  removeSession,
  applySessionEvents,
  resetTombstonesForTests,
  type SessionRow,
} from './sessions';
import { hosts, applyHostEvents, resetTombstonesForTests as resetHostTombstones } from './hosts';

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

// Deliver one event the way Tauri does in production: the listener runs in
// its own task. The mock's `emit` calls the handler synchronously, so `fire`
// is one task; the tests below interleave `vi.advanceTimersByTimeAsync` to
// put real (fake) time between deliveries.
const fire = (name: string, payload: unknown) => void vi.mocked(emit)(name, payload);
// Let the batch timer expire.
const flush = () => vi.advanceTimersByTimeAsync(ROW_EVENT_FLUSH_MS + 1);

beforeEach(() => {
  vi.useFakeTimers();
  vi.mocked(emit).mockClear();
  resetTombstonesForTests();
  resetHostTombstones();
  sessions.set([]);
  hosts.set([]);
});
afterEach(() => {
  vi.useRealTimers();
});

describe('subscribeToRowEvents', () => {
  it('fires onSessionCreated when session:created is emitted', async () => {
    const seen: number[] = [];
    await subscribeToRowEvents({
      onSessionCreated: (row) => seen.push(row.id),
    });
    fire('session:created', row({ id: 42 }));
    await flush();
    expect(seen).toEqual([42]);
  });

  it('fires onSessionKilled with id payload', async () => {
    const killed: number[] = [];
    await subscribeToRowEvents({
      onSessionKilled: (p) => killed.push(p.id),
    });
    fire('session:killed', { id: 99 });
    await flush();
    expect(killed).toEqual([99]);
  });

  it('returns unsubscribe that detaches all listeners', async () => {
    const seen: number[] = [];
    const unlisten = await subscribeToRowEvents({
      onSessionCreated: (row) => seen.push(row.id),
    });
    unlisten();
    fire('session:created', row({ id: 1 }));
    await flush();
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
    await subscribeToRowEvents({
      onSessionCreated: mergeSession,
      onSessionKilled: (p) => removeSession(p.id),
    });
    fire('session:created', row({ id: 7, tmux_name: 'dev-test', host_alias: 'local', created_at: 1, last_activity_at: 1 }));
    await flush();
    expect(get(sessions).map((s) => s.id)).toEqual([7]);
    fire('session:killed', { id: 7 });
    await flush();
    expect(get(sessions)).toEqual([]);
  });
});

// FE-10: the reconcile tick emits one session:updated per session; each one
// used to be its own store flush (60 sessions → 60 sidebar re-derives).
describe('row event batching', () => {
  const seed60 = () =>
    Array.from({ length: 60 }, (_, i) => row({ id: i + 1, tmux_name: `s${i + 1}`, last_activity_at: 1 }));
  const update = (i: number) =>
    row({ id: i + 1, tmux_name: `s${i + 1}`, last_activity_at: 2, claude_status: 'working' });

  it('coalesces 60 synchronous session:updated events into one store notification', async () => {
    sessions.set(seed60());
    const unlisten = await subscribeToRowEvents({ onSessionEvents: applySessionEvents });
    const notify = vi.fn();
    const unsub = sessions.subscribe(notify);
    notify.mockClear(); // drop the initial subscribe call

    for (let i = 0; i < 60; i++) fire('session:updated', update(i));
    // Nothing applied yet — delivery waits for the batch timer.
    expect(notify).not.toHaveBeenCalled();
    await flush();

    expect(notify).toHaveBeenCalledTimes(1);
    const final = get(sessions);
    expect(final).toHaveLength(60);
    expect(final.every((s) => s.claude_status === 'working' && s.last_activity_at === 2)).toBe(true);
    unsub();
    unlisten();
  });

  it('coalesces events delivered in SEPARATE tasks (as Tauri does) into one notification', async () => {
    // Tauri hands each emitted event to the webview as its own eval(), i.e.
    // its own macrotask, so the microtask queue drains between them. The
    // batch window must span real time, not just the current task.
    sessions.set(seed60());
    const unlisten = await subscribeToRowEvents({ onSessionEvents: applySessionEvents });
    const notify = vi.fn();
    const unsub = sessions.subscribe(notify);
    notify.mockClear();

    for (let i = 0; i < 60; i++) {
      fire('session:updated', update(i));
      // A few hundred µs between deliveries — well inside the window overall
      // (60 × 0.2 ms = 12 ms < ROW_EVENT_FLUSH_MS); each advance yields the
      // task so every listener call really is a separate macrotask.
      await vi.advanceTimersByTimeAsync(0.2);
    }
    expect(notify).not.toHaveBeenCalled();
    await flush();

    expect(notify).toHaveBeenCalledTimes(1);
    const final = get(sessions);
    expect(final).toHaveLength(60);
    expect(final.every((s) => s.claude_status === 'working')).toBe(true);
    unsub();
    unlisten();
  });

  it('a burst longer than the window is split, but into far fewer flushes than events', async () => {
    sessions.set(seed60());
    const unlisten = await subscribeToRowEvents({ onSessionEvents: applySessionEvents });
    const notify = vi.fn();
    const unsub = sessions.subscribe(notify);
    notify.mockClear();
    // 60 events 1 ms apart = 60 ms ≈ 4 windows.
    for (let i = 0; i < 60; i++) {
      fire('session:updated', update(i));
      await vi.advanceTimersByTimeAsync(1);
    }
    await flush();
    expect(notify.mock.calls.length).toBeGreaterThanOrEqual(1);
    expect(notify.mock.calls.length).toBeLessThanOrEqual(6);
    expect(get(sessions).every((s) => s.claude_status === 'working')).toBe(true);
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

  it('events after a flush start a new batch', async () => {
    const unlisten = await subscribeToRowEvents({ onSessionEvents: applySessionEvents });
    const notify = vi.fn();
    const unsub = sessions.subscribe(notify);
    notify.mockClear();
    fire('session:created', row({ id: 1 }));
    await flush();
    fire('session:created', row({ id: 2, tmux_name: 'b' }));
    await flush();
    expect(notify).toHaveBeenCalledTimes(2);
    expect(get(sessions).map((s) => s.id)).toEqual([1, 2]);
    unsub();
    unlisten();
  });
});
