// "Ask for review…" (docs/review-notes.md, HumanLayer plan v2 R11 lean): the
// owner of a session asks a colleague to review its changes. The hub shares
// the session with them (drive, so their notes can go to the agent; watch
// for someone who is only a viewer, who reads and drafts) and puts the ask
// in their Inbox, where they open it and mark it done.
//
// The asks are read like waiting missions (`mission_waits.ts`): a tracker
// started from main.ts keeps them fresh while the window shows, and the
// Inbox's Needs you and the rail badge count them.
import { derived, writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';
import { pollWhileShown } from './window_hidden';

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
  /** The session's name, when the hub sends it (a newer hub does, so the
   *  ask reads right before the session reaches this device's list). */
  session_name?: string | null;
}

/** The open asks to this person, newest first. Empty until the first read,
 *  and on a hub that predates the feature. */
export const reviewRequests = writable<readonly ReviewRequest[]>([]);
/** How many asks the Inbox lists and the rail badge counts. */
export const reviewRequestCount = derived(reviewRequests, ($r) => $r.length);

/** How often the asks are read while the app shows (as waiting missions). */
export const REVIEW_REQUESTS_EVERY_MS = 60_000;

function listAsks(): Promise<Result<ReviewRequest[]>> {
  return invokeCmd<ReviewRequest[]>('review_request', { args: { action: 'list' } });
}

export async function loadReviewRequests(load: () => Promise<Result<ReviewRequest[]>> = listAsks): Promise<void> {
  const r = await load();
  if (r.ok && Array.isArray(r.value)) reviewRequests.set(r.value);
}

/** Keep `reviewRequests` fresh while the app shows; stops on the returned
 *  function. A hub that refuses the read (`E_INVALID`) is not asked again. */
export function trackReviewRequests(
  deps: { load?: () => Promise<Result<ReviewRequest[]>>; doc?: Document | null; win?: Window | null; every?: number } = {},
): () => void {
  const load = deps.load ?? listAsks;
  let stopped = false;
  const poller = pollWhileShown(
    async () => {
      const r = await load();
      if (stopped) return false;
      if (r.ok && Array.isArray(r.value)) reviewRequests.set(r.value);
      return r.ok || r.error.code !== 'E_INVALID';
    },
    { every: deps.every ?? REVIEW_REQUESTS_EVERY_MS, doc: deps.doc, win: deps.win },
  );
  return () => {
    stopped = true;
    poller.stop();
  };
}

/** What the Inbox calls an ask's session: its row on this device, else the
 *  name the hub sent, else its number. */
export function askSessionName(r: ReviewRequest, row?: { friendly_name?: string | null; tmux_name: string } | null): string {
  return row?.friendly_name || row?.tmux_name || r.session_name || `Session ${r.session_id}`;
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
