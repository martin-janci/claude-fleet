// Thread room escalations (parity doc 2026-10-14, "New things"): what in the
// fleet needs the person, as amber cards in Control's chat, so the chat says
// it where the person is looking instead of only counting it in the Needs
// you tab. A session asking a question or for a permission, stuck, failed,
// out of its account's limit, without a login, or on a host that is down.
//
// Pure apart from the `later` and `escalationScope` stores, which persist in
// prefs (F18, review 2026-10-17): Later survives a restart, and by default
// only Control's own threads (`controlSessionIds`: what it started or handed
// work to) raise a card, with an "All fleet" switch for the rest. The rows
// and the triage are the Needs you tab's own (`visibleFleet`, `classify`), so
// a card and the tab never disagree.
import { writable } from 'svelte/store';
import { classify, displayName, type AttentionOptions, type TriageBucket } from './attention';
import type { ControlHandoff } from './handoffs';
import { readPref, writePref } from './prefs';
import type { SessionRow } from './sessions';

/** The buckets that escalate, worst first, and how a card says each. */
const WHY: Partial<Record<TriageBucket, string>> = {
  waiting: 'is waiting for your answer',
  host_down: 'is on a host that is down',
  stuck: 'is stuck',
  stop_failed: 'stopped with an error',
  failed: 'failed',
  account_limit: 'hit its account limit',
  no_credentials: 'needs a login',
};
const ORDER = Object.keys(WHY) as TriageBucket[];

export interface Escalation {
  /** The card's identity: the session, why, and its turn, so a dismissed card
   *  comes back when the same session needs you again later. */
  key: string;
  session: SessionRow;
  bucket: TriageBucket;
  /** "fed-v2 is waiting for your answer". */
  line: string;
  /** The question or the command a permission asks for, when the pane shows one. */
  detail: string | null;
}

/** How many cards the chat draws; the rest are "N more in Needs you". */
export const ESCALATIONS_SHOWN = 3;

export function escalationKey(s: SessionRow, bucket: TriageBucket): string {
  return `${s.id}:${bucket}:${s.turn_seq}`;
}

/**
 * The sessions that are Control's threads, as `control_threads.ts` lists
 * them: those its agent started (`origin = 'operator'`), those a session or
 * task receipt names, and the workers of a mission a receipt names.
 */
export function controlSessionIds(rows: readonly SessionRow[], handoffs: readonly ControlHandoff[]): Set<number> {
  const ids = new Set<number>();
  const missions = new Set<number>();
  for (const h of handoffs) {
    if (h.session_id != null && (h.kind === 'session' || h.kind === 'task')) ids.add(h.session_id);
    if (h.mission_id != null) missions.add(h.mission_id);
  }
  for (const s of rows) {
    if (s.origin === 'operator') ids.add(s.id);
    else if (s.origin === 'mission' && s.origin_ref && missions.has(Number(s.origin_ref))) ids.add(s.id);
  }
  return ids;
}

/**
 * What needs the person, worst first, leaving out Control itself and the
 * cards put off. `scope`, when given, keeps only those sessions (Control's
 * threads); `null` is the whole fleet.
 */
export function escalations(
  rows: readonly SessionRow[],
  opts: AttentionOptions,
  operatorId: number | null,
  later: ReadonlySet<string>,
  scope: ReadonlySet<number> | null = null,
): Escalation[] {
  const out: Escalation[] = [];
  // A host that is down is one card, not one per session on it.
  const down = new Map<string, Escalation & { count: number }>();
  for (const s of rows) {
    if (scope && !scope.has(s.id) && s.id !== operatorId) continue;
    if (s.id === operatorId) continue;
    const bucket = classify(s, opts);
    const why = WHY[bucket];
    if (!why) continue;
    if (bucket === 'host_down') {
      const key = `host:${s.host_alias}`;
      if (later.has(key)) continue;
      const seen = down.get(s.host_alias);
      if (seen) seen.count += 1;
      else down.set(s.host_alias, { key, session: s, bucket, line: '', detail: null, count: 1 });
      continue;
    }
    const key = escalationKey(s, bucket);
    if (later.has(key)) continue;
    const p = s.pending_input;
    const detail = bucket === 'waiting' && p ? (p.detail ?? p.question ?? null) : null;
    out.push({ key, session: s, bucket, line: `${displayName(s, true)} ${why}`, detail });
  }
  for (const [host, e] of down) {
    const { count, ...rest } = e;
    out.push({ ...rest, line: `${host} is down: ${count === 1 ? `${displayName(e.session, true)} waits on it` : `${count} sessions wait on it`}` });
  }
  return out.sort((a, b) => ORDER.indexOf(a.bucket) - ORDER.indexOf(b.bucket));
}

/** The pref Later is kept in: `[key, unix seconds]` pairs, newest last. */
export const LATER_PREF = 'control.escalationsLater';
/** How many put-off cards are remembered; the oldest go first. */
export const LATER_KEPT = 200;
/** How long a put-off card stays put off, at most: a week. */
export const LATER_MAX_AGE_SECS = 7 * 86_400;

type LaterEntry = [string, number];

const isLater = (v: unknown): v is LaterEntry[] =>
  Array.isArray(v) && v.every((e) => Array.isArray(e) && e.length === 2 && typeof e[0] === 'string' && typeof e[1] === 'number');

/** PURE: the entries still worth keeping at `now`, newest `LATER_KEPT`. */
export function pruneLater(entries: readonly LaterEntry[], now: number): LaterEntry[] {
  return entries.filter(([, at]) => now - at < LATER_MAX_AGE_SECS).slice(-LATER_KEPT);
}

const nowSecs = () => Math.floor(Date.now() / 1000);
let laterEntries: LaterEntry[] = pruneLater(readPref<LaterEntry[]>(LATER_PREF, [], isLater), nowSecs());

/** Cards put off with Later, kept across restarts (prefs). */
export const laterEscalations = writable<ReadonlySet<string>>(new Set(laterEntries.map(([k]) => k)));

export function putOff(key: string): void {
  laterEntries = pruneLater([...laterEntries.filter(([k]) => k !== key), [key, nowSecs()]], nowSecs());
  writePref(LATER_PREF, laterEntries);
  laterEscalations.set(new Set(laterEntries.map(([k]) => k)));
}

/** `control`: only Control's threads raise a card (the default); `fleet`: every session. */
export type EscalationScope = 'control' | 'fleet';
export const SCOPE_PREF = 'control.escalationsScope';
const isScope = (v: unknown): v is EscalationScope => v === 'control' || v === 'fleet';

export const escalationScope = writable<EscalationScope>(readPref<EscalationScope>(SCOPE_PREF, 'control', isScope));
escalationScope.subscribe((v) => writePref(SCOPE_PREF, v));
