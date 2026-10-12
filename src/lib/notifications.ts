// The notification centre's list: every toast this window showed, newest
// first, so one that faded (or was dismissed) can still be read. Per window
// and in memory; the hub-side notification matrix is step 11.9.
//
// A toast's button (Save, Undo, Retry) works from here only while the toast
// itself is still up: an action is offered for a short time on purpose, and
// running a stale Undo an hour later could undo something else.

import { derived, writable } from 'svelte/store';
import type { ToastKind } from './toasts';

/** The most kept; the oldest go first. */
export const MAX_NOTIFICATIONS = 100;

export interface Notice {
  id: number;
  /** The toast this came from: its action is offered while it is up. */
  toastId: number;
  /** ms since the epoch of the last time it was pushed. */
  at: number;
  kind: ToastKind;
  code: string | null;
  message: string;
  /** The toast's second line, when it had one. */
  sub?: string;
  /** How many times it was pushed (a deduped toast counts each). */
  count: number;
  read: boolean;
}

export const notices = writable<Notice[]>([]);
export const unreadNotices = derived(notices, (ns) => ns.filter((n) => !n.read).length);

let nextId = 1;

/** Called by `toasts.push` for every push. A push the toast stack deduped
 *  (`fresh` false) bumps the entry of that toast instead of adding one. */
export function recordNotice(
  toastId: number,
  fresh: boolean,
  n: { kind: ToastKind; code: string | null; message: string; sub?: string },
  now = Date.now(),
): void {
  notices.update((list) => {
    if (!fresh) {
      const i = list.findIndex((x) => x.toastId === toastId);
      if (i >= 0) {
        const bumped = { ...list[i], at: now, count: list[i].count + 1, read: false, ...(n.sub !== undefined ? { sub: n.sub } : {}) };
        return [bumped, ...list.slice(0, i), ...list.slice(i + 1)];
      }
    }
    const { sub, ...rest } = n;
    const entry: Notice = { id: nextId++, toastId, at: now, ...rest, ...(sub ? { sub } : {}), count: 1, read: false };
    return [entry, ...list].slice(0, MAX_NOTIFICATIONS);
  });
}

export function markAllNoticesRead(): void {
  notices.update((list) => (list.some((n) => !n.read) ? list.map((n) => (n.read ? n : { ...n, read: true })) : list));
}

export function removeNotice(id: number): void {
  notices.update((list) => list.filter((n) => n.id !== id));
}

export function clearNotices(): void {
  notices.set([]);
}
