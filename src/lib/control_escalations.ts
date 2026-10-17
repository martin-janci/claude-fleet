// Thread room escalations (parity doc 2026-10-14, "New things"): what in the
// fleet needs the person, as amber cards in Control's chat, so the chat says
// it where the person is looking instead of only counting it in the Needs
// you tab. A session asking a question or for a permission, stuck, failed,
// out of its account's limit, without a login, or on a host that is down.
//
// Pure apart from the per-window `later` set. The rows and the triage are
// the Needs you tab's own (`visibleFleet`, `classify`), so a card and the tab
// never disagree; Control's own session is never a card in its own chat.
import { writable } from 'svelte/store';
import { classify, displayName, type AttentionOptions, type TriageBucket } from './attention';
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

/** What needs the person, worst first, leaving out Control itself and the cards put off. */
export function escalations(
  rows: readonly SessionRow[],
  opts: AttentionOptions,
  operatorId: number | null,
  later: ReadonlySet<string>,
): Escalation[] {
  const out: Escalation[] = [];
  // A host that is down is one card, not one per session on it.
  const down = new Map<string, Escalation & { count: number }>();
  for (const s of rows) {
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

/** Cards put off with Later, for this window. */
export const laterEscalations = writable<ReadonlySet<string>>(new Set());

export function putOff(key: string): void {
  laterEscalations.update((s) => new Set([...s, key]));
}
