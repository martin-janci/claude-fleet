import { writable, type Writable } from "svelte/store";

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
 *   return value that raced a newer event — can't clobber a fresher row;
 * - a full re-list (`beginList` / `applyList`) that loses to every frame or
 *   command row merged while it was in flight, and to a newer re-list
 *   (review r07; `loadTrackers` keeps its own copy of the same rule).
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

/** Where the frames stood when a re-list was asked for. */
export interface ListToken {
  readonly gen: number;
  readonly mark: number;
}

/**
 * The re-list rule on its own, for stores that are not row stores (a nested
 * tree, a map, a list re-read on every change): `touch` every key a frame or
 * a command changes, `begin` before asking for the list, then `mergeList`
 * with the token.
 */
export interface ListRace<K> {
  touch(key: K): void;
  begin(): ListToken;
  /** The list for `token` with the rows touched since it started kept as
   *  `cur` has them (added, kept, or still gone); null when a later list
   *  already landed. */
  mergeList<T>(
    cur: readonly T[],
    listed: readonly T[],
    token: ListToken,
    key: (row: T) => K,
    keepCurrent?: (listed: T, current: T) => boolean,
  ): T[] | null;
}

export function createListRace<K>(): ListRace<K> {
  // Every touch bumps `frameSeq` and notes it per key, so a re-list can tell
  // which rows changed while it was in flight.
  let frameSeq = 0;
  const touchedAt = new Map<K, number>();
  let listGen = 0;
  let appliedGen = 0;
  return {
    touch(key) {
      touchedAt.set(key, ++frameSeq);
    },
    begin() {
      return { gen: ++listGen, mark: frameSeq };
    },
    mergeList(cur, listed, token, key, keepCurrent) {
      if (token.gen < appliedGen) return null;
      appliedGen = token.gen;
      const touched = (k: K) => (touchedAt.get(k) ?? 0) > token.mark;
      const byKey = new Map(cur.map((r) => [key(r), r] as const));
      const seen = new Set<K>();
      const out: (typeof cur)[number][] = [];
      for (const row of listed) {
        const k = key(row);
        seen.add(k);
        const current = byKey.get(k);
        if (touched(k)) {
          // A frame got here first: keep its row, or its removal.
          if (current) out.push(current);
          continue;
        }
        out.push(current && keepCurrent?.(row, current) ? current : row);
      }
      for (const r of cur) {
        const k = key(r);
        if (!seen.has(k) && touched(k)) out.push(r);
      }
      return out;
    },
  };
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
  merge(row: T | null | undefined): void;
  remove(key: K): void;
  /** A command result is the authoritative answer to a request the user just
   *  made: clear the key's tombstone, then merge. */
  accept(row: T | null | undefined): void;
  isTombstoned(key: K): boolean;
  /** Call before asking for the full list; hand the token to `applyList`. */
  beginList(): ListToken;
  /** Replace the store with a full list answer. The list owns the order and
   *  every row nothing touched since `beginList`; a row merged or removed
   *  after it keeps that newer state (added, kept or still gone). Returns
   *  false, changing nothing, when a later re-list already landed. */
  applyList(listed: readonly T[], token: ListToken): boolean;
  /** Test hook: forget every tombstone so one test's remove can't shadow the
   *  next test's merge of the same key. Not for production code. */
  resetTombstonesForTests(): void;
}

export function createRowStore<T, K>(
  opts: RowStoreOptions<T, K>,
): RowStore<T, K> {
  const store = writable<T[]>([]);
  /** When the key was removed, and the removed row's identity when we held it. */
  const tombstones = new Map<K, { at: number; ident?: string }>();
  const normalize = opts.normalize ?? ((arr: T[]) => arr);
  // Every merge or remove is a touch, so a re-list can tell which rows
  // changed while it was in flight.
  const race = createListRace<K>();

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
    race.touch(key);
    const i = arr.findIndex((r) => opts.key(r) === key);
    if (i === -1) return normalize([...arr, row]);
    if (opts.isStale?.(row, arr[i])) return arr;
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
    if (
      opts.identity &&
      t?.ident !== undefined &&
      t.ident !== opts.identity(row)
    ) {
      tombstones.delete(key);
    }
    return mergeInto(arr, row);
  }

  function removeFrom(arr: T[], key: K): T[] {
    if (opts.tombstoneMs !== undefined) {
      const held = opts.identity
        ? arr.find((r) => opts.key(r) === key)
        : undefined;
      tombstones.set(key, {
        at: Date.now(),
        ident: held && opts.identity?.(held),
      });
    }
    race.touch(key);
    const next = arr.filter((r) => opts.key(r) !== key);
    return next.length === arr.length ? arr : next;
  }

  return {
    store,
    mergeInto,
    mergeCreatedInto,
    removeFrom,
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
    beginList: race.begin,
    applyList(listed, token) {
      // A mocked or older backend may answer with nothing: keep what we have.
      if (!Array.isArray(listed)) return false;
      let landed = false;
      store.update((cur) => {
        const out = race.mergeList(
          cur,
          listed.filter((r) => !isTombstoned(opts.key(r))),
          token,
          opts.key,
          opts.isStale,
        );
        if (out === null) return cur;
        landed = true;
        return normalize(out);
      });
      return landed;
    },
    resetTombstonesForTests() {
      tombstones.clear();
    },
  };
}
