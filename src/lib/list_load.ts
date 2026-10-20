import { writable, type Readable } from 'svelte/store';
import type { IpcError, Result } from './result';

/**
 * Review w17-1: where a store-backed list's last full read stands, so an
 * empty list can be told apart from one that is still loading or that could
 * not load. `idle` means nothing asked yet (tests, a page that never loads);
 * a page reads `failed` as "Couldn't load … · Retry" and `loading` as a
 * skeleton, and only `idle` / `loaded` as a real first run.
 */
export type ListLoadState = 'idle' | 'loading' | 'loaded' | 'failed';
export interface ListLoad {
  state: ListLoadState;
  error: IpcError | null;
}

export interface ListLoadTracker extends Readable<ListLoad> {
  /** Wrap one full read: `loading` while it runs, then `loaded` or `failed`. */
  track<T>(read: Promise<Result<T>>): Promise<Result<T>>;
  /** Test hook. */
  set(v: ListLoad): void;
}

export function createListLoad(): ListLoadTracker {
  const store = writable<ListLoad>({ state: 'idle', error: null });
  let seq = 0;
  return {
    subscribe: store.subscribe,
    set: store.set,
    async track(read) {
      const mine = ++seq;
      // A re-read after a failure keeps saying "failed" until it answers, so
      // the Retry button (its own `retrying` state) does not blink away.
      store.update((s) => (s.state === 'failed' ? s : { state: 'loading', error: null }));
      const r = await read;
      if (mine === seq) store.set(r.ok ? { state: 'loaded', error: null } : { state: 'failed', error: r.error });
      return r;
    },
  };
}
