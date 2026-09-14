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
    vi.mocked(emit).mockClear();
  });

  it('fires onSessionCreated when session:created is emitted', async () => {
    const seen: number[] = [];
    await subscribeToRowEvents({
      onSessionCreated: (row) => seen.push(row.id),
    });
    await vi.mocked(emit)('session:created', {
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
    });
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
    await vi.mocked(emit)('session:created', {
      id: 1, tmux_name: 't', host_alias: 'h',
      project_id: null, worktree_id: null,
      created_at: 0, last_activity_at: 0,
      status: 'running', notes: null, account_uuid: null,
      kind: 'work', reviews_session_id: null, worktree_key: null,
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
    await vi.mocked(emit)('session:created', {
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
      kind: 'work',
      reviews_session_id: null,
      worktree_key: null,
    });
    const { get } = await import('svelte/store');
    expect(get(sessions).map((s) => s.id)).toEqual([7]);
    await vi.mocked(emit)('session:killed', { id: 7 });
    expect(get(sessions)).toEqual([]);
  });

  it('fires asset inventory and catalog handlers', async () => {
    const seen: string[] = [];
    await subscribeToRowEvents({
      onAssetInventoryUpdated: (row) => seen.push(`upd:${row.host_alias}:${row.name}`),
      onAssetInventoryCleared: (p) => seen.push(`clr:${p.host_alias}:${p.harness}`),
      onCatalogLoaded: (s) => seen.push(`cat:${s.head}`),
    });
    await vi.mocked(emit)('asset_inventory:cleared', { host_alias: 'local', harness: 'claude' });
    await vi.mocked(emit)('asset_inventory:updated', { host_alias: 'local', harness: 'claude', kind: 'skill', name: 's', state: 'in_sync', catalog_hash: null, host_hash: null, scanned_at: 1 });
    await vi.mocked(emit)('catalog:loaded', { head: 'h', loaded_at: 1, asset_count: 0, problem_count: 0 });
    expect(seen).toEqual(['clr:local:claude', 'upd:local:s', 'cat:h']);
  });
});
