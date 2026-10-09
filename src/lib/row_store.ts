import { writable, type Writable } from 'svelte/store';

/**
 * The one merge/remove core behind the row stores (sessions, hosts,
 * accounts, tasks). Each store keeps its own public API; this only owns the
 * three behaviours they had all hand-copied:
 *
 * - upsert by key, returning the input array untouched when nothing changed
 *   (so callers can skip a store flush);
 * - tombstones: a row removed in the last `tombstoneMs` ms is not
 *   re-inserted by an event still in flight for it (a `session:updated`
 *   after a kill, a `host:probed` after a remove). Entries expire so a
 *   genuinely new key is never blocked;
 * - an optional monotonic guard (`isStale`) so a stale payload — a command
 *   return value that raced a newer event — can't clobber a fresher row.
 */
export interface RowStoreOptions<T, K> {
  key: (row: T) => K;
  /** Enable tombstones with this expiry. Omit for stores whose rows are never removed. */
  tombstoneMs?: number;
  /** Return true when `incoming` must not replace `current`. */
  isStale?: (incoming: T, current: T) => boolean;
  /** Applied to the array after every change (e.g. to keep a sort order). */
  normalize?: (arr: T[]) => T[];
  /** What tells two rows that share a key apart (a reused SQLite id). With it,
   *  `mergeCreatedInto` lets a row the backend says is NEW past the tombstone
   *  of a removed row whose identity differs. */
  identity?: (row: T) => string;
}

export interface ReconcileOptions {
  /** Keep the row we hold over the listed one when an event changed it after
   *  the list call began (for stores without a monotonic `isStale` guard). */
  preferTouched?: boolean;
}

export interface RowStore<T, K> {
  store: Writable<T[]>;
  /** Pure upsert step, shared by the single-row and batched paths. */
  mergeInto(arr: T[], row: T): T[];
  /** Pure upsert step for a row the backend reports as newly created: clears
   *  the key's tombstone first when the removed row's identity differs (the
   *  key was reused), then merges. A late `created` for the removed row itself
   *  is still dropped. */
  mergeCreatedInto(arr: T[], row: T): T[];
  /** Pure remove step; tombstones the key. */
  removeFrom(arr: T[], key: K): T[];
  /** Note that a list call starts now. Pass the token to `reconcileList` (or
   *  `endList` when the call failed) once it answers. */
  beginList(): number;
  endList(token: number): void;
  /** Rebuild from a list answer that may have been read before events applied
   *  while it was in flight: the list owns order; a key removed since the call
   *  began stays gone; a row we hold that the list lacks but an event merged
   *  since the call began is kept (in front). Ends the list. */
  reconcileList(cur: T[], listed: readonly T[], token: number, ropts?: ReconcileOptions): T[];
  merge(row: T | null | undefined): void;
  remove(key: K): void;
  /** A command result is the authoritative answer to a request the user just
   *  made: clear the key's tombstone, then merge. */
  accept(row: T | null | undefined): void;
  isTombstoned(key: K): boolean;
  /** Test hook: forget every tombstone so one test's remove can't shadow the
   *  next test's merge of the same key. Not for production code. */
  resetTombstonesForTests(): void;
}

export function createRowStore<T, K>(opts: RowStoreOptions<T, K>): RowStore<T, K> {
  const store = writable<T[]>([]);
  /** When the key was removed, and the removed row's identity when we held it. */
  const tombstones = new Map<K, { at: number; ident?: string }>();
  const normalize = opts.normalize ?? ((arr: T[]) => arr);

  // List-vs-event bookkeeping. `epoch` moves on every `beginList`; while any
  // list is in flight each event-applied change records the epoch it landed
  // in, so a list answer can tell "changed after I started" from "before".
  let epoch = 0;
  const inflight = new Set<number>();
  const touched = new Map<K, { epoch: number; removed: boolean }>();
  function touch(key: K, removed: boolean): void {
    if (inflight.size > 0) touched.set(key, { epoch, removed });
  }
  function touchedSince(key: K, token: number): { removed: boolean } | null {
    const t = touched.get(key);
    return t !== undefined && t.epoch >= token ? t : null;
  }

  function isTombstoned(key: K): boolean {
    if (opts.tombstoneMs === undefined) return false;
    const t = tombstones.get(key);
    if (t === undefined) return false;
    if (Date.now() - t.at > opts.tombstoneMs) {
      tombstones.delete(key);
      return false;
    }
    return true;
  }

  function mergeInto(arr: T[], row: T): T[] {
    if (!row) return arr;
    const key = opts.key(row);
    if (isTombstoned(key)) return arr;
    const i = arr.findIndex((r) => opts.key(r) === key);
    if (i === -1) {
      touch(key, false);
      return normalize([...arr, row]);
    }
    if (opts.isStale?.(row, arr[i])) return arr;
    touch(key, false);
    const next = arr.slice();
    next[i] = row;
    return normalize(next);
  }

  function mergeCreatedInto(arr: T[], row: T): T[] {
    if (!row) return arr;
    const key = opts.key(row);
    const t = tombstones.get(key);
    // Unknown identity (the removed row was never held) stays blocked: we
    // cannot tell a reused key from a late event for the removed row.
    if (opts.identity && t?.ident !== undefined && t.ident !== opts.identity(row)) {
      tombstones.delete(key);
    }
    return mergeInto(arr, row);
  }

  function removeFrom(arr: T[], key: K): T[] {
    if (opts.tombstoneMs !== undefined) {
      const held = opts.identity ? arr.find((r) => opts.key(r) === key) : undefined;
      tombstones.set(key, { at: Date.now(), ident: held && opts.identity?.(held) });
    }
    touch(key, true);
    const next = arr.filter((r) => opts.key(r) !== key);
    return next.length === arr.length ? arr : next;
  }

  function endList(token: number): void {
    inflight.delete(token);
    if (inflight.size === 0) touched.clear();
  }

  function reconcileList(cur: T[], listed: readonly T[], token: number, ropts: ReconcileOptions = {}): T[] {
    const byKey = new Map(cur.map((r) => [opts.key(r), r] as const));
    const listedKeys = new Set<K>();
    const next: T[] = [];
    for (const l of listed) {
      const key = opts.key(l);
      listedKeys.add(key);
      if (isTombstoned(key)) continue;
      const since = touchedSince(key, token);
      if (since?.removed) continue;
      const current = byKey.get(key);
      const keepCurrent =
        current !== undefined && (opts.isStale?.(l, current) || (ropts.preferTouched && since !== null));
      next.push(keepCurrent ? current : l);
    }
    const fresh = cur.filter((r) => {
      const key = opts.key(r);
      return !listedKeys.has(key) && touchedSince(key, token) !== null;
    });
    endList(token);
    return normalize(fresh.length > 0 ? [...fresh, ...next] : next);
  }

  return {
    store,
    mergeInto,
    mergeCreatedInto,
    removeFrom,
    beginList() {
      epoch += 1;
      inflight.add(epoch);
      return epoch;
    },
    endList,
    reconcileList,
    merge(row) {
      if (!row) return;
      if (isTombstoned(opts.key(row))) return;
      store.update((arr) => mergeInto(arr, row));
    },
    remove(key) {
      store.update((arr) => removeFrom(arr, key));
    },
    accept(row) {
      if (!row) return;
      tombstones.delete(opts.key(row));
      store.update((arr) => mergeInto(arr, row));
    },
    isTombstoned,
    resetTombstonesForTests() {
      tombstones.clear();
    },
  };
}
