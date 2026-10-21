/**
 * Per-session fan-out of the backend's timeline pushes (`session:event`) and
 * conversation-list changes (`session:conversations`). App.svelte feeds it
 * from its single `subscribeToRowEvents`; components register for one
 * session and get only that session's events.
 */
import type { SessionEvent } from './timeline';

type Subs<T> = Map<number, Set<(v: T) => void>>;
const timelineSubs: Subs<SessionEvent> = new Map();
const conversationSubs: Subs<void> = new Map();
const anySubs = new Set<(e: SessionEvent) => void>();

function add<T>(subs: Subs<T>, id: number, fn: (v: T) => void): () => void {
  let set = subs.get(id);
  if (!set) {
    set = new Set();
    subs.set(id, set);
  }
  set.add(fn);
  return () => {
    set!.delete(fn);
    if (set!.size === 0) subs.delete(id);
  };
}

export function onTimelineEvent(sessionId: number, fn: (e: SessionEvent) => void): () => void {
  return add(timelineSubs, sessionId, fn);
}

export function onConversationsChanged(sessionId: number, fn: () => void): () => void {
  return add(conversationSubs, sessionId, () => fn());
}

/** Every session's timeline pushes, for a module that follows an event
 *  kind fleet-wide rather than one session (a finished move's composer
 *  carry, `moveSession.ts`). */
export function onAnyTimelineEvent(fn: (e: SessionEvent) => void): () => void {
  anySubs.add(fn);
  return () => {
    anySubs.delete(fn);
  };
}

export function dispatchTimelineEvents(events: SessionEvent[]): void {
  for (const e of events) {
    for (const fn of timelineSubs.get(e.session_id) ?? []) fn(e);
    for (const fn of anySubs) fn(e);
  }
}

export function dispatchConversationsChanged(sessionIds: number[]): void {
  for (const id of new Set(sessionIds)) {
    for (const fn of conversationSubs.get(id) ?? []) fn();
  }
}
