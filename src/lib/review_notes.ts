// Review notes on a session's diff (docs/review-notes.md): notes on lines of
// the changes, written before anything is pushed. Drafts stay on this device
// (browser storage, per session) until Send, which types them into the
// session as ONE numbered prompt (`review_notes { send }`, Routed). A send
// carries an id, so a double click or a retry types nothing twice. Sent
// notes come back from the session's store with who sent them, and a person
// resolves each one.

import { derived, get, writable, type Readable } from 'svelte/store';
import { invokeCmd, type Result } from './result';
import { sessionIdentity, type SessionRow } from './sessions';

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

/** The session a set of drafts belongs to. Ids are small per-backend
 *  integers that SQLite hands out again once the newest session is reaped,
 *  so the drafts carry who they were written for: the fleet, the host, the
 *  tmux name and the creation time (`sessionIdentity`). */
export type DraftSession = Pick<SessionRow, 'id' | 'host_alias' | 'tmux_name' | 'created_at'> & { fleet?: string };

function ownerOf(s: DraftSession): string {
  return `${s.fleet ?? ''}\u0001${sessionIdentity(s as SessionRow)}`;
}

/** A send whose answer never came: its id, and the drafts (id and body) it
 *  carried. Kept with the drafts, so a remount does not lose it. */
interface PendingSend {
  id: string;
  drafts: { id: string; body: string }[];
}

interface Stored {
  owner: string;
  drafts: NoteDraft[];
  pending?: PendingSend | null;
}

const isDraft = (d: unknown): d is NoteDraft =>
  !!d && typeof (d as NoteDraft).body === 'string' && typeof (d as NoteDraft).path === 'string';

function read(s: DraftSession): Stored {
  const owner = ownerOf(s);
  const empty: Stored = { owner, drafts: [], pending: null };
  try {
    const raw = localStorage.getItem(KEY + s.id);
    const v: unknown = raw ? JSON.parse(raw) : null;
    // Before the owner was kept: a bare array, adopted as this session's.
    if (Array.isArray(v)) return { owner, drafts: v.filter(isDraft), pending: null };
    if (!v || typeof v !== 'object') return empty;
    const st = v as Partial<Stored>;
    // Another session that had this id: its drafts are not this one's.
    if (st.owner !== owner) return empty;
    const pending =
      st.pending && typeof st.pending.id === 'string' && Array.isArray(st.pending.drafts) ? st.pending : null;
    return { owner, drafts: Array.isArray(st.drafts) ? st.drafts.filter(isDraft) : [], pending };
  } catch {
    return empty;
  }
}

function write(id: number, st: Stored): void {
  try {
    if (st.drafts.length || st.pending) localStorage.setItem(KEY + id, JSON.stringify(st));
    else localStorage.removeItem(KEY + id);
  } catch {
    // Storage off (a private window): the drafts live as long as the page.
  }
}

const stores = new Map<string, ReturnType<typeof writable<Stored>>>();

function storeFor(s: DraftSession) {
  const k = `${s.id}\u0002${ownerOf(s)}`;
  let st = stores.get(k);
  if (!st) {
    st = writable(read(s));
    stores.set(k, st);
  }
  return st;
}

/** This device's drafts for a session, kept in browser storage. */
export function noteDrafts(s: DraftSession): Readable<NoteDraft[]> {
  return derived(storeFor(s), (st) => st.drafts);
}

function update(s: DraftSession, f: (st: Stored) => Stored): void {
  storeFor(s).update((cur) => {
    const next = f(cur);
    write(s.id, next);
    return next;
  });
}

function updateDrafts(s: DraftSession, f: (d: NoteDraft[]) => NoteDraft[]): void {
  update(s, (st) => ({ ...st, drafts: f(st.drafts) }));
}

let seq = 0;
function newId(prefix: string): string {
  seq += 1;
  const rnd = Math.random().toString(36).slice(2, 8).replace(/[^a-z0-9]/g, '');
  return `${prefix}-${Date.now().toString(36)}-${seq}${rnd}`;
}

/** Add or replace a draft (same id); an empty body removes it. */
export function saveDraft(s: DraftSession, d: Omit<NoteDraft, 'id'> & { id?: string }): void {
  const body = d.body.trim();
  updateDrafts(s, (all) => {
    if (!body) return all.filter((x) => x.id !== d.id);
    const draft: NoteDraft = { ...d, body, id: d.id ?? newId('nd') };
    const i = all.findIndex((x) => x.id === draft.id);
    return i < 0 ? [...all, draft] : all.map((x, j) => (j === i ? draft : x));
  });
}

export function removeDraft(s: DraftSession, id: string): void {
  updateDrafts(s, (all) => all.filter((x) => x.id !== id));
}

export function clearDrafts(s: DraftSession): void {
  update(s, (st) => ({ ...st, drafts: [], pending: null }));
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

/** A send's answer, and how many drafts it left in place: on a repeat of a
 *  send whose answer never came, the drafts added or edited since. */
export type ReviewNotesSent = SentReviewNotes & { kept: number };

/**
 * Send this device's drafts as one prompt. Each send carries an id; a send
 * whose answer never came keeps it (with the drafts it carried) beside the
 * drafts, and the next send reuses it, so nothing is typed twice. The
 * backend answers a known id with the notes it already has (`repeat`):
 * then only the drafts that send carried, unchanged, are cleared, and any
 * added or edited since stay for another send.
 */
export async function sendReviewNotes(s: DraftSession, drafts: readonly NoteDraft[]): Promise<Result<ReviewNotesSent>> {
  const st = get(storeFor(s));
  const pending = st.pending ?? { id: newSendId(), drafts: drafts.map(({ id, body }) => ({ id, body })) };
  if (!st.pending) update(s, (cur) => ({ ...cur, pending }));
  const notes = ordered(drafts).map(({ path, line, side, quote, body }) => ({
    path,
    line: line ?? undefined,
    side,
    quote: quote ?? undefined,
    body,
  }));
  const r = await invokeCmd<SentReviewNotes>('review_notes', {
    args: { session_id: s.id, action: 'send', client_msg_id: pending.id, notes },
  });
  if (!r.ok) return r;
  const gone = r.value.repeat
    ? new Set(pending.drafts.filter((p) => drafts.some((d) => d.id === p.id && d.body === p.body)).map((p) => p.id))
    : new Set(drafts.map((d) => d.id));
  let kept = 0;
  update(s, (cur) => {
    const left = cur.drafts.filter((d) => !gone.has(d.id));
    kept = left.length;
    return { ...cur, drafts: left, pending: null };
  });
  return { ok: true, value: { ...r.value, kept } };
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
