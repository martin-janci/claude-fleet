// Wizards the app itself opens in a conversation (redesign 10.12): Control's
// Add project, Get started in Control's chat. Per window and never saved:
// the card is a view, and what its last button did is in the fleet's own
// rows (the project, the session). An agent opens one with a `wizard`
// block in its reply instead (docs/chat-blocks.md).
import { writable } from 'svelte/store';
import type { ChatWizardId } from './chat_wizard_ids';

export interface OpenChatWizard {
  /** Unique per open, so the same wizard opened twice is two cards. */
  key: string;
  id: ChatWizardId;
  why: string | null;
  /** Who opened it, in the card's "from …" ("Control"). */
  from: string;
  /** Answered or declined: its card is one line now. */
  ended?: boolean;
}

/** Session id → the wizards open in its conversation, oldest first. */
export const chatWizards = writable<ReadonlyMap<number, readonly OpenChatWizard[]>>(new Map());

let seq = 0;

/** Open `id` at the end of `sessionId`'s conversation. One of a kind at a
 *  time: opening a wizard that is already open (and not yet answered) keeps
 *  the open card rather than stacking a second. */
export function openChatWizard(
  sessionId: number,
  id: ChatWizardId,
  opts: { why?: string | null; from?: string } = {},
): string {
  let key = '';
  chatWizards.update((m) => {
    const list = m.get(sessionId) ?? [];
    const open = list.find((w) => w.id === id && !w.ended);
    if (open) {
      key = open.key;
      return m;
    }
    key = `w${++seq}`;
    const next = new Map(m);
    next.set(sessionId, [...list, { key, id, why: opts.why ?? null, from: opts.from ?? 'Fleet' }]);
    return next;
  });
  return key;
}

/** Take a card away (its ✕ after it answered or was declined). */
export function closeChatWizard(sessionId: number, key: string): void {
  chatWizards.update((m) => {
    const list = m.get(sessionId);
    if (!list?.some((w) => w.key === key)) return m;
    const next = new Map(m);
    const kept = list.filter((w) => w.key !== key);
    if (kept.length) next.set(sessionId, kept);
    else next.delete(sessionId);
    return next;
  });
}

/** The card answered or was declined: a second open starts a fresh one. */
export function endChatWizard(sessionId: number, key: string): void {
  chatWizards.update((m) => {
    const list = m.get(sessionId);
    if (!list?.some((w) => w.key === key && !w.ended)) return m;
    const next = new Map(m);
    next.set(
      sessionId,
      list.map((w) => (w.key === key ? { ...w, ended: true } : w)),
    );
    return next;
  });
}
