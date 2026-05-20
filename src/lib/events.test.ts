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
import { subscribeToRowEvents } from './events';

describe('subscribeToRowEvents', () => {
  beforeEach(() => {
    (emit as ReturnType<typeof vi.fn>).mockClear();
  });

  it('fires onSessionCreated when session:created is emitted', async () => {
    const seen: number[] = [];
    await subscribeToRowEvents({
      onSessionCreated: (row) => seen.push(row.id),
    });
    await (emit as ReturnType<typeof vi.fn>)('session:created', {
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
    });
    expect(seen).toEqual([42]);
  });

  it('fires onSessionKilled with id payload', async () => {
    const killed: number[] = [];
    await subscribeToRowEvents({
      onSessionKilled: (p) => killed.push(p.id),
    });
    await (emit as ReturnType<typeof vi.fn>)('session:killed', { id: 99 });
    expect(killed).toEqual([99]);
  });

  it('returns unsubscribe that detaches all listeners', async () => {
    const seen: number[] = [];
    const unlisten = await subscribeToRowEvents({
      onSessionCreated: (row) => seen.push(row.id),
    });
    unlisten();
    await (emit as ReturnType<typeof vi.fn>)('session:created', {
      id: 1, tmux_name: 't', host_alias: 'h',
      project_id: null, worktree_id: null,
      created_at: 0, last_activity_at: 0,
      status: 'running', notes: null, account_uuid: null,
    });
    expect(seen).toEqual([]);
  });
});

// End-to-end: emit → handler → store update.
describe('subscribeToRowEvents → store integration', () => {
  it('session:created event updates the sessions store via mergeSession', async () => {
    const { sessions, mergeSession, removeSession } = await import('./sessions');
    sessions.set([]);
    await subscribeToRowEvents({
      onSessionCreated: mergeSession,
      onSessionKilled: (p) => removeSession(p.id),
    });
    await (emit as ReturnType<typeof vi.fn>)('session:created', {
      id: 7,
      tmux_name: 'dev-test',
      host_alias: 'local',
      project_id: null,
      worktree_id: null,
      created_at: 1,
      last_activity_at: 1,
      status: 'running',
      notes: null,
      account_uuid: null,
    });
    const { get } = await import('svelte/store');
    expect(get(sessions).map((s) => s.id)).toEqual([7]);
    await (emit as ReturnType<typeof vi.fn>)('session:killed', { id: 7 });
    expect(get(sessions)).toEqual([]);
  });
});
