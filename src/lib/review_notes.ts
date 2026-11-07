// Review notes on a session's diff (docs/review-notes.md): notes on lines of
// the changes, written before anything is pushed. Drafts stay on this device
// (browser storage, per session) until Send, which types them into the
// session as ONE numbered prompt (`review_notes { send }`, Routed). A send
// carries an id, so a double click or a retry types nothing twice. Sent
// notes come back from the session's store with who sent them, and a person
// resolves each one.

import { writable, type Readable } from 'svelte/store';
import { invokeCmd, type Result } from './result';

export type NoteSide = 'new' | 'old';

/** A note as it goes out (`store::NewReviewNote`). */
export interface NoteDraft {
  /** This device's id for the draft. */
  id: string;
  path: string;
  /** Absent: the whole file. */
  line?: number | null;
  side: NoteSide;
  quote?: string | null;
  body: string;
}

/** A sent note (`store::ReviewNoteRow`). */
export interface ReviewNote {
  id: number;
  session_id: number;
  batch: string;
  seq: number;
  path: string;
  line?: number | null;
  side: NoteSide;
  quote?: string | null;
  body: string;
  /** Who sent it ("Ada · desktop"). */
  author: string;
  author_person_id?: number | null;
  created_at: number;
  resolved_at?: number | null;
  resolved_by?: string | null;
}

export interface SentReviewNotes {
  session_id: number;
  notes: ReviewNote[];
  delivered: boolean;
  queued_id?: number | null;
  repeat?: boolean;
}

/** Where a note sits in a diff: one key per side and line. */
export function lineKey(path: string, side: NoteSide, line: number | null | undefined): string {
  return `${path}\u0000${side}\u0000${line ?? ''}`;
}

const KEY = 'review-note-drafts:';

function read(sessionId: number): NoteDraft[] {
  try {
    const raw = localStorage.getItem(KEY + sessionId);
    const v: unknown = raw ? JSON.parse(raw) : [];
    return Array.isArray(v) ? (v as NoteDraft[]).filter((d) => d && typeof d.body === 'string' && typeof d.path === 'string') : [];
  } catch {
    return [];
  }
}

function write(sessionId: number, drafts: NoteDraft[]): void {
  try {
    if (drafts.length) localStorage.setItem(KEY + sessionId, JSON.stringify(drafts));
    else localStorage.removeItem(KEY + sessionId);
  } catch {
    // Storage off (a private window): the drafts live as long as the page.
  }
}

const stores = new Map<number, ReturnType<typeof writable<NoteDraft[]>>>();

function storeFor(sessionId: number) {
  let s = stores.get(sessionId);
  if (!s) {
    s = writable(read(sessionId));
    stores.set(sessionId, s);
  }
  return s;
}

/** This device's drafts for a session, kept in browser storage. */
export function noteDrafts(sessionId: number): Readable<NoteDraft[]> {
  return { subscribe: storeFor(sessionId).subscribe };
}

function update(sessionId: number, f: (d: NoteDraft[]) => NoteDraft[]): void {
  storeFor(sessionId).update((d) => {
    const next = f(d);
    write(sessionId, next);
    return next;
  });
}

let seq = 0;
function newId(prefix: string): string {
  seq += 1;
  const rnd = Math.random().toString(36).slice(2, 8).replace(/[^a-z0-9]/g, '');
  return `${prefix}-${Date.now().toString(36)}-${seq}${rnd}`;
}

/** Add or replace a draft (same id); an empty body removes it. */
export function saveDraft(sessionId: number, d: Omit<NoteDraft, 'id'> & { id?: string }): void {
  const body = d.body.trim();
  update(sessionId, (all) => {
    if (!body) return all.filter((x) => x.id !== d.id);
    const draft: NoteDraft = { ...d, body, id: d.id ?? newId('nd') };
    const i = all.findIndex((x) => x.id === draft.id);
    return i < 0 ? [...all, draft] : all.map((x, j) => (j === i ? draft : x));
  });
}

export function removeDraft(sessionId: number, id: string): void {
  update(sessionId, (all) => all.filter((x) => x.id !== id));
}

export function clearDrafts(sessionId: number): void {
  update(sessionId, () => []);
}

/** Drafts in the order the prompt numbers them: by file, then line. */
export function ordered(drafts: readonly NoteDraft[]): NoteDraft[] {
  return [...drafts].sort(
    (a, b) => a.path.localeCompare(b.path) || (a.line ?? 0) - (b.line ?? 0) || (a.side === b.side ? 0 : a.side === 'old' ? -1 : 1),
  );
}

export async function listReviewNotes(sessionId: number): Promise<Result<ReviewNote[]>> {
  return invokeCmd<ReviewNote[]>('review_notes', { args: { session_id: sessionId } });
}

/**
 * Send this device's drafts as one prompt. `clientMsgId` is the send's id:
 * pass the same one to retry a send whose answer never came, and nothing is
 * typed twice. The drafts are cleared only once the send is in.
 */
export async function sendReviewNotes(
  sessionId: number,
  drafts: readonly NoteDraft[],
  clientMsgId: string,
): Promise<Result<SentReviewNotes>> {
  const notes = ordered(drafts).map(({ path, line, side, quote, body }) => ({
    path,
    line: line ?? undefined,
    side,
    quote: quote ?? undefined,
    body,
  }));
  const r = await invokeCmd<SentReviewNotes>('review_notes', {
    args: { session_id: sessionId, action: 'send', client_msg_id: clientMsgId, notes },
  });
  if (r.ok) {
    const sent = new Set(drafts.map((d) => d.id));
    update(sessionId, (all) => all.filter((d) => !sent.has(d.id)));
  }
  return r;
}

export function newSendId(): string {
  return newId('rn');
}

export async function resolveReviewNote(sessionId: number, noteId: number, resolved = true): Promise<Result<ReviewNote>> {
  return invokeCmd<ReviewNote>('review_notes', {
    args: { session_id: sessionId, action: 'resolve', note_id: noteId, resolved },
  });
}

/** For tests: forget the in-memory stores. */
export function resetNoteDraftsForTest(): void {
  stores.clear();
}
