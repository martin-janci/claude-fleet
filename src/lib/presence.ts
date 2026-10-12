// Presence (redesign 11.7b): who else has the open session on screen.
//
// While a session is open and the window visible, this client reports it to
// the hub (`session_presence`) every `heartbeat_secs`, and once more with
// `leaving` when it is closed. Every report answers the viewers the hub lets
// this person see: the owner sees everyone looking, anyone else the owner
// and themselves. The hub keeps it in memory for two missed heartbeats, so a
// window that crashes drops off on its own.
//
// Standalone there is nobody else to report to, so nothing is sent. A hub
// that predates the tool refuses it; that is not an error to show, the
// header simply names no one.
import { get, writable, type Readable } from 'svelte/store';
import { invokeCmd, type Result } from './result';
import { hubStatus } from './hub';
import type { SessionRow } from './sessions';

export interface Viewer {
  person_id: number;
  /** Display name, else name. */
  name: string;
  /** The paired device they look from; absent for the hub's own token. */
  device?: string;
  /** Unix seconds this device opened it. */
  since: number;
  /** This client's own report. */
  you?: boolean;
}

export interface PresenceView {
  session_id: number;
  viewers: Viewer[];
  heartbeat_secs: number;
}

/** The open session's viewers, or null when none is known. */
export const presence = writable<PresenceView | null>(null);

export function reportPresence(sessionId: number, leaving = false): Promise<Result<PresenceView>> {
  return invokeCmd<PresenceView>('session_presence', {
    args: leaving ? { session_id: sessionId, leaving: true } : { session_id: sessionId },
  });
}

/** Everyone but this device, one entry per person (a person on a phone and a
 *  laptop is one face), earliest first. */
export function othersLooking(view: PresenceView | null): Viewer[] {
  if (!view) return [];
  const seen = new Set<number>();
  const out: Viewer[] = [];
  for (const v of view.viewers) {
    if (v.you || seen.has(v.person_id)) continue;
    seen.add(v.person_id);
    out.push(v);
  }
  return out;
}

/** What the header says about one viewer: their name, and the device when
 *  there is one. */
export function viewerTitle(v: Viewer): string {
  return v.device ? `${v.name} is looking at this session, from ${v.device}` : `${v.name} is looking at this session`;
}

interface Deps {
  report?: typeof reportPresence;
  enabled?: () => boolean;
  doc?: Pick<Document, 'visibilityState' | 'addEventListener' | 'removeEventListener'> | null;
  setTimer?: (fn: () => void, ms: number) => unknown;
  clearTimer?: (t: unknown) => void;
}

/** Report `selected` as above and keep `presence` current. Returns the stop. */
export function trackPresence(selected: Readable<SessionRow | null>, deps: Deps = {}): () => void {
  const report = deps.report ?? reportPresence;
  const enabled = deps.enabled ?? (() => get(hubStatus).remote);
  const doc = deps.doc === undefined ? (typeof document === 'undefined' ? null : document) : deps.doc;
  const setTimer = deps.setTimer ?? ((fn, ms) => setTimeout(fn, ms));
  const clearTimer = deps.clearTimer ?? ((t) => clearTimeout(t as ReturnType<typeof setTimeout>));

  let current: number | null = null;
  let timer: unknown = null;
  // Bumped on every switch, so a late answer for the previous session never
  // lands on the next one.
  let epoch = 0;

  const stopTimer = () => {
    if (timer !== null) clearTimer(timer);
    timer = null;
  };
  const beat = async () => {
    stopTimer();
    const id = current;
    if (id === null || !enabled() || doc?.visibilityState === 'hidden') return;
    const mine = epoch;
    const r = await report(id);
    if (mine !== epoch) return;
    if (!r.ok) {
      // An older hub, or a row this person can no longer read: stop asking.
      presence.set(null);
      return;
    }
    presence.set(r.value);
    timer = setTimer(() => void beat(), Math.max(5, r.value.heartbeat_secs) * 1000);
  };
  const leave = (id: number) => {
    if (enabled()) void report(id, true);
  };

  const unsubscribe = selected.subscribe((s) => {
    const next = s && s.kind !== 'external' ? s.id : null;
    if (next === current) return;
    epoch += 1;
    stopTimer();
    if (current !== null) leave(current);
    current = next;
    presence.set(null);
    void beat();
  });
  // Hidden is not looking: stop reporting and let the hub drop us; come back
  // at once when the window does.
  const onVisibility = () => {
    if (doc?.visibilityState === 'hidden') {
      stopTimer();
    } else {
      void beat();
    }
  };
  doc?.addEventListener('visibilitychange', onVisibility);
  return () => {
    unsubscribe();
    doc?.removeEventListener('visibilitychange', onVisibility);
    epoch += 1;
    stopTimer();
    if (current !== null) leave(current);
    current = null;
    presence.set(null);
  };
}
