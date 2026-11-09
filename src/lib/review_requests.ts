// "Ask for review…" (docs/review-notes.md, HumanLayer plan v2 R11 lean): the
// owner of a session asks a colleague to review its changes. The hub shares
// the session with them (drive, so their notes can go to the agent; watch
// for someone who is only a viewer, who reads and drafts) and puts the ask
// in their Inbox, where they open it and mark it done.
import { writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';

/** One ask (`store::ReviewRequestRow`). */
export interface ReviewRequest {
  id: number;
  session_id: number;
  from_person_id: number;
  from_name: string;
  to_person_id: number;
  to_name: string;
  /** The share it came with: drive or watch. */
  level: string;
  note?: string | null;
  created_at: number;
  done_at?: number | null;
}

/** The open asks to this person, newest first. Empty until the first read,
 *  and on a hub that predates the feature. */
export const reviewRequests = writable<readonly ReviewRequest[]>([]);

export async function loadReviewRequests(): Promise<void> {
  const r = await invokeCmd<ReviewRequest[]>('review_request', { args: { action: 'list' } });
  if (r.ok && Array.isArray(r.value)) reviewRequests.set(r.value);
}

/** Ask `person` (by name) to review session `sessionId`, which you own. */
export async function askForReview(sessionId: number, person: string, note: string): Promise<Result<ReviewRequest>> {
  const n = note.trim();
  return invokeCmd<ReviewRequest>('review_request', {
    args: { action: 'ask', session_id: sessionId, person: person.trim(), ...(n ? { note: n } : {}) },
  });
}

/** Mark an ask to you done: it leaves the Inbox. */
export async function finishReviewRequest(id: number): Promise<Result<ReviewRequest>> {
  const r = await invokeCmd<ReviewRequest>('review_request', { args: { action: 'done', id, done: true } });
  if (r.ok) reviewRequests.update((all) => all.filter((x) => x.id !== id));
  return r;
}

/** What the asked colleague can do, in words. */
export function levelWords(level: string): string {
  return level === 'drive' ? 'can send notes to the agent' : 'can read and draft notes';
}
