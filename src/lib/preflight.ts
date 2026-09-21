// A read-only preview of what a Transfer would do, kept per (session, target
// host) so the Transfer sheet can show it before the user commits. Debounced
// per session — flicking through target hosts fires one call, for the host
// the user stopped on — and safe against the same request firing twice with
// the answers arriving out of order (see the sequence check in `settle`).
import { writable, type Readable } from 'svelte/store';
import { previewMove, type MovePreview } from './moveSession';
import type { IpcError, Result } from './result';

export const PREFLIGHT_DEBOUNCE_MS = 250;
/** Older than this, a result shows its age rather than presenting as current. */
export const PREFLIGHT_STALE_MS = 30_000;

export interface PreflightEntry {
  sessionId: number;
  toHost: string;
  status: 'loading' | 'ready' | 'refused';
  preview: MovePreview | null;
  /** The refusal the real move would return. */
  error: IpcError | null;
  /** When the answer arrived; null while loading. */
  at: number | null;
}

function keyOf(sessionId: number, toHost: string): string {
  return `${sessionId}\u0000${toHost}`;
}

const store = writable<Map<string, PreflightEntry>>(new Map());

export const preflights: Readable<Map<string, PreflightEntry>> = { subscribe: store.subscribe };

/** Pending debounce timer per SESSION (not per key): a burst of target
 *  changes for one session must collapse to a single call, for whichever
 *  host the user stopped on. */
const timers = new Map<number, ReturnType<typeof setTimeout>>();

/** Highest request sequence number issued per (session, host) key. An answer
 *  whose sequence is not the latest for its key is a straggler from a request
 *  the same key already superseded (e.g. the sheet reopened and re-requested
 *  the same host) — dropped in `settle` so it can never overwrite a newer
 *  answer, however late it arrives. */
const sequences = new Map<string, number>();

function put(entry: PreflightEntry): void {
  store.update((m) => new Map(m).set(keyOf(entry.sessionId, entry.toHost), entry));
}

/** Test-only: seed an entry directly, keyed the same way `settle` would —
 *  so a consumer's tests (the Transfer sheet's setup view) can arm a specific
 *  loading/ready/refused state without reimplementing this module's private
 *  key format. */
export function putPreflightForTest(entry: PreflightEntry): void {
  put(entry);
}

export function preflightFor(
  map: Map<string, PreflightEntry>,
  sessionId: number,
  toHost: string,
): PreflightEntry | undefined {
  return map.get(keyOf(sessionId, toHost));
}

/** Milliseconds since the answer, or null while loading. */
export function preflightAge(entry: PreflightEntry, now: number): number | null {
  return entry.at === null ? null : now - entry.at;
}

function settle(sessionId: number, toHost: string, seq: number, r: Result<MovePreview>): void {
  const key = keyOf(sessionId, toHost);
  // The straggler guard: this key may have been requested again (or several
  // times) since `seq` went out, and an earlier request's answer can still
  // arrive after a later one already settled the key. Only the latest
  // sequence for this key may write — anything older is dropped outright,
  // even though it is a perfectly good answer to a question that is no
  // longer the current one.
  if (sequences.get(key) !== seq) return;
  if (r.ok) {
    put({ sessionId, toHost, status: 'ready', preview: r.value, error: null, at: Date.now() });
  } else {
    put({ sessionId, toHost, status: 'refused', preview: null, error: r.error, at: Date.now() });
  }
}

/** Ask for a preview, debounced per session: a burst of requests for one
 *  session fires only the last. */
export function requestPreflight(sessionId: number, toHost: string): void {
  const existing = timers.get(sessionId);
  if (existing !== undefined) clearTimeout(existing);
  const timer = setTimeout(() => {
    timers.delete(sessionId);
    const key = keyOf(sessionId, toHost);
    const seq = (sequences.get(key) ?? 0) + 1;
    sequences.set(key, seq);
    put({ sessionId, toHost, status: 'loading', preview: null, error: null, at: null });
    void previewMove(sessionId, toHost).then((r) => settle(sessionId, toHost, seq, r));
  }, PREFLIGHT_DEBOUNCE_MS);
  timers.set(sessionId, timer);
}

export function resetPreflightsForTest(): void {
  for (const t of timers.values()) clearTimeout(t);
  timers.clear();
  sequences.clear();
  store.set(new Map());
}
