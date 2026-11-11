// Review notes on a session's diff (docs/review-notes.md): notes on lines of
// the changes, written before anything is pushed. Drafts stay on this device
// (browser storage, per session) until Send, which types them into the
// session as ONE numbered prompt (`review_notes { send }`, Routed). A send
// carries an id, so a double click or a retry types nothing twice. Sent
// notes come back from the session's store with who sent them, and a person
// resolves each one.

import { get, writable, type Readable } from 'svelte/store';
import { errorText } from './error_copy';
import { invokeCmd, type IpcError, type Result } from './result';

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
  /** Absent once the send reached the session. `waiting`: its prompt is
   *  kept for the session's next idle moment; `not_delivered`: that prompt
   *  was cancelled, dropped or failed; `uncertain`: the typing may or may not
   *  have landed. (Older hubs never send it.) */
  delivery?: ReviewNoteDelivery | null;
  /** Why, for `not_delivered` / `uncertain`. */
  delivery_error?: string | null;
  /** The kept prompt carrying the send (Send later's id). */
  queued_id?: number | null;
}

export type ReviewNoteDelivery = 'waiting' | 'not_delivered' | 'uncertain';

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

/** The hub's limits (`store::review_notes`): at most this many notes in
 *  one send, and this many characters in a note's body. */
export const NOTES_MAX = 50;
export const NOTE_MAX = 4000;
/** A note's quote (the line it is on) is cut to this many characters, so a
 *  long line (minified code, a data URI) never makes a send too long. */
export const QUOTE_MAX = 300;

/** A line as a note quotes it: trailing space off, cut to `QUOTE_MAX`
 *  characters. A sent note sits on a row only where the row's quote equals
 *  the note's. */
export function quoteOf(text: string | null | undefined): string {
  const t = (text ?? '').trimEnd();
  return t.length <= QUOTE_MAX ? t : Array.from(t).slice(0, QUOTE_MAX).join('');
}

// Drafts and the pending send are keyed by the session's identity
// (`sessionIdentity`: host, tmux name, created_at), not its id: a killed
// newest session's id goes to the next one, and an id means another
// session on another hub.
const KEY = 'review-note-drafts:';
const SEND_KEY = 'review-note-send:';

function storageKey(prefix: string, key: string): string {
  return prefix + encodeURIComponent(key);
}

function read(key: string): NoteDraft[] {
  try {
    const raw = localStorage.getItem(storageKey(KEY, key));
    const v: unknown = raw ? JSON.parse(raw) : [];
    return Array.isArray(v) ? (v as NoteDraft[]).filter((d) => d && typeof d.body === 'string' && typeof d.path === 'string') : [];
  } catch {
    return [];
  }
}

function write(key: string, drafts: NoteDraft[]): void {
  try {
    if (drafts.length) localStorage.setItem(storageKey(KEY, key), JSON.stringify(drafts));
    else localStorage.removeItem(storageKey(KEY, key));
  } catch {
    // Storage off (a private window): the drafts live as long as the page.
  }
}

const stores = new Map<string, ReturnType<typeof writable<NoteDraft[]>>>();

function storeFor(key: string) {
  let s = stores.get(key);
  if (!s) {
    s = writable(read(key));
    stores.set(key, s);
  }
  return s;
}

/** This device's drafts for a session (by `sessionIdentity`), kept in
 *  browser storage. */
export function noteDrafts(key: string): Readable<NoteDraft[]> {
  return { subscribe: storeFor(key).subscribe };
}

function update(key: string, f: (d: NoteDraft[]) => NoteDraft[]): void {
  storeFor(key).update((d) => {
    const next = f(d);
    write(key, next);
    return next;
  });
}

let seq = 0;
function newId(prefix: string): string {
  seq += 1;
  const rnd = Math.random().toString(36).slice(2, 8);
  return `${prefix}-${Date.now().toString(36)}-${seq}${rnd}`;
}

/** Add or replace a draft (same id); an empty body removes it. */
export function saveDraft(key: string, d: Omit<NoteDraft, 'id'> & { id?: string }): void {
  const body = d.body.trim();
  update(key, (all) => {
    if (!body) return all.filter((x) => x.id !== d.id);
    const draft: NoteDraft = { ...d, body, id: d.id ?? newId('nd') };
    const i = all.findIndex((x) => x.id === draft.id);
    return i < 0 ? [...all, draft] : all.map((x, j) => (j === i ? draft : x));
  });
}

export function removeDraft(key: string, id: string): void {
  update(key, (all) => all.filter((x) => x.id !== id));
}

/**
 * A send whose answer has not come in, per session beside its drafts: its
 * id, a hash of the notes it carried, and whether a call is running now.
 * Every view of the session sees it (a second diff cannot send the same
 * notes again while the first call runs), and the id survives a remount or
 * a reload, so a retry of the SAME notes reuses it and the hub types
 * nothing twice. Different notes get a new id.
 */
export interface PendingSend {
  id: string;
  hash: string;
  inFlight: boolean;
}

const sendStores = new Map<string, ReturnType<typeof writable<PendingSend | null>>>();

function readSend(key: string): PendingSend | null {
  try {
    const raw = localStorage.getItem(storageKey(SEND_KEY, key));
    const v = raw ? (JSON.parse(raw) as Partial<PendingSend>) : null;
    // A call does not outlive the page: a stored send is never in flight.
    return v && typeof v.id === 'string' && typeof v.hash === 'string' ? { id: v.id, hash: v.hash, inFlight: false } : null;
  } catch {
    return null;
  }
}

function sendStoreFor(key: string) {
  let s = sendStores.get(key);
  if (!s) {
    s = writable(readSend(key));
    sendStores.set(key, s);
  }
  return s;
}

function setSend(key: string, v: PendingSend | null): void {
  sendStoreFor(key).set(v);
  try {
    if (v) localStorage.setItem(storageKey(SEND_KEY, key), JSON.stringify({ id: v.id, hash: v.hash }));
    else localStorage.removeItem(storageKey(SEND_KEY, key));
  } catch {
    // Storage off: the id lives as long as the page.
  }
}

/** The session's send whose answer has not come in, if any. */
export function pendingSend(key: string): Readable<PendingSend | null> {
  return { subscribe: sendStoreFor(key).subscribe };
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

function wireNotes(drafts: readonly NoteDraft[]) {
  return ordered(drafts).map(({ path, line, side, quote, body }) => ({
    path,
    line: line ?? undefined,
    side,
    quote: quote ?? undefined,
    body,
  }));
}

/** A short hash (FNV-1a) of the notes a send carries, in order. */
export function notesHash(drafts: readonly NoteDraft[]): string {
  const s = JSON.stringify(wireNotes(drafts));
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h.toString(16).padStart(8, '0');
}

const sameNote = (d: NoteDraft, n: Pick<ReviewNote, 'path' | 'side' | 'line' | 'body'>) =>
  d.path === n.path && d.side === n.side && (d.line ?? null) === (n.line ?? null) && d.body.trim() === n.body.trim();

/**
 * Send the session's drafts (`key`: its `sessionIdentity`) as one prompt.
 * The send's id is kept with the drafts until the answer comes: a retry of
 * the same notes, from this view or another, after a remount or a reload,
 * reuses it and nothing is typed twice; changed notes get a new id. While
 * a call runs, another send of the session answers `E_IN_FLIGHT` here.
 * Only the drafts that were in the send are dropped, once it is in (on a
 * `repeat`, the ones the hub's stored batch holds).
 */
export async function sendReviewNotes(sessionId: number, key: string): Promise<Result<SentReviewNotes>> {
  const was = get(sendStoreFor(key));
  if (was?.inFlight) {
    return { ok: false, error: { code: 'E_IN_FLIGHT', message: 'These notes are still being sent.' } };
  }
  const drafts = get(storeFor(key));
  if (drafts.length === 0) return { ok: false, error: { code: 'E_INVALID', message: 'No notes to send.' } };
  if (drafts.length > NOTES_MAX) {
    return {
      ok: false,
      error: { code: 'E_INVALID', message: `At most ${NOTES_MAX} notes go in one send; you have ${drafts.length}.` },
    };
  }
  const hash = notesHash(drafts);
  const id = was?.hash === hash ? was.id : newId('rn');
  setSend(key, { id, hash, inFlight: true });
  const r = await invokeCmd<SentReviewNotes>('review_notes', {
    args: { session_id: sessionId, action: 'send', client_msg_id: id, notes: wireNotes(drafts) },
  });
  if (r.ok) {
    setSend(key, null);
    const inSend = r.value.repeat
      ? drafts.filter((d) => (r.value.notes ?? []).some((n) => sameNote(d, n)))
      : drafts;
    const sent = new Set(inSend.map((d) => d.id));
    update(key, (all) => all.filter((d) => !sent.has(d.id)));
  } else if (r.error.code === 'E_CONFLICT') {
    // The hub holds other notes under this id: the next send is a new one.
    setSend(key, null);
  } else {
    // No answer, or a refusal: keep the id for a retry of these notes.
    setSend(key, { id, hash, inFlight: false });
  }
  return r;
}

/** A failed send in words. The drafts are kept whatever the failure. */
export function sendFailureText(e: IpcError): string {
  switch (e.code) {
    case 'E_IN_FLIGHT':
      return 'These notes are still being sent. Your drafts are kept; try again in a moment.';
    case 'E_CONFLICT':
      return 'An earlier try of this send carried other notes. Nothing was sent now and your drafts are kept: send again.';
    default:
      return errorText(e);
  }
}

export async function resolveReviewNote(sessionId: number, noteId: number, resolved = true): Promise<Result<ReviewNote>> {
  return invokeCmd<ReviewNote>('review_notes', {
    args: { session_id: sessionId, action: 'resolve', note_id: noteId, resolved },
  });
}

/** For tests: forget the in-memory stores. */
export function resetNoteDraftsForTest(): void {
  stores.clear();
  sendStores.clear();
}
