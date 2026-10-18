// The confirm queue in hub mode: a request raised during a gap the hub
// could not replay announces nothing, so `hub:resynced` re-reads the queue.
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
const handlers = vi.hoisted(() => new Map<string, (e: { payload: unknown }) => void>());
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (name: string, cb: (e: { payload: unknown }) => void) => {
    handlers.set(name, cb);
    return () => handlers.delete(name);
  }),
}));
import { invoke } from '@tauri-apps/api/core';
import { confirmQueue, resetConfirmsForTests, startConfirmQueue } from './confirms';

const mock = invoke as ReturnType<typeof vi.fn>;
const req = { nonce: 'n1', tool: 'kill_session', summary: 'kill 4', caller: 'ops', operator: false, asked_at: 1 };

let stop: () => void = () => {};

beforeEach(() => {
  handlers.clear();
  resetConfirmsForTests();
  mock.mockReset();
});

afterEach(() => stop());

describe('confirm queue resync', () => {
  it('a request raised during an unreplayable gap shows after hub:resynced', async () => {
    mock.mockResolvedValueOnce([]).mockResolvedValue([req]);
    stop = startConfirmQueue();
    await vi.waitFor(() => expect(handlers.has('hub:resynced')).toBe(true));
    expect(get(confirmQueue)).toEqual([]);

    handlers.get('hub:resynced')!({ payload: null });

    await vi.waitFor(() => expect(get(confirmQueue)).toHaveLength(1));
    expect(get(confirmQueue)[0].nonce).toBe('n1');
  });

  it('stops re-reading once released', async () => {
    mock.mockResolvedValue([]);
    stop = startConfirmQueue();
    await vi.waitFor(() => expect(handlers.has('hub:resynced')).toBe(true));
    stop();
    expect(handlers.has('hub:resynced')).toBe(false);
  });
});
