// Marks the session on screen as viewed (redesign 2.3, migration 125), so
// its finished turns stop reading as unread (`attention.ts`, `isUnread`).
//
// The stamp is the row's `last_viewed_at`, written by the hub or the local
// backend through `touch_session_viewed`. It is called only when it would
// change something: the open session is unread, or nobody has viewed it
// yet. A turn that ends while the session is open makes it unread for a
// moment, and the row event that says so brings the next call. Nothing is
// stamped while the window is hidden: a turn that ends behind another app
// has not been seen, nor one that ends while another page (Hosts, Accounts,
// the Board...) covers the session.
import { derived, type Readable } from 'svelte/store';
import { destination, type Destination } from './destination';
import { isUnread } from './attention';
import { touchSessionViewed, type SessionRow } from './sessions';

type Touch = (sessionId: number) => unknown;

/** Whether the open session should be stamped viewed now. */
export function wantsViewedStamp(s: SessionRow | null): s is SessionRow {
  if (!s || s.kind === 'external') return false;
  return s.last_viewed_at == null || isUnread(s);
}

/** The destinations that show the selected session. */
const SHOWS_SESSION: ReadonlySet<Destination> = new Set<Destination>(['session', 'files', 'details']);

/** Whether the selected session is what the main window shows. */
export const sessionOnScreen: Readable<boolean> = derived(destination, (d) => SHOWS_SESSION.has(d));

/** Watch `selected` and stamp it viewed as above. Returns the stop. */
export function trackViewedSession(
  selected: Readable<SessionRow | null>,
  touch: Touch = touchSessionViewed,
  doc: Pick<Document, 'visibilityState' | 'addEventListener' | 'removeEventListener'> | null =
    typeof document === 'undefined' ? null : document,
  onScreen: Readable<boolean> = sessionOnScreen,
): () => void {
  let current: SessionRow | null = null;
  let shown = true;
  // One call per (session, finished turn): a refused call (a watcher's
  // client) is not retried on every row event for the same turn.
  let stamped: string | null = null;
  const check = () => {
    const s = current;
    if (!wantsViewedStamp(s)) return;
    if (!shown || doc?.visibilityState === 'hidden') return;
    const key = `${s.id}:${s.last_stop_at ?? 0}`;
    if (key === stamped) return;
    stamped = key;
    void touch(s.id);
  };
  const unShown = onScreen.subscribe((v) => {
    shown = v;
    check();
  });
  const unsubscribe = selected.subscribe((s) => {
    current = s;
    check();
  });
  doc?.addEventListener('visibilitychange', check);
  return () => {
    unShown();
    unsubscribe();
    doc?.removeEventListener('visibilitychange', check);
  };
}
