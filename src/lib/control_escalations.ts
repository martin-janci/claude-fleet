// Thread room escalations (parity doc 2026-10-14, "New things"): what in the
// fleet needs the person, as amber cards in Control's chat, so the chat says
// it where the person is looking instead of only counting it in the Needs
// you tab. A session asking a question or for a permission, stuck, failed,
// out of its account's limit, without a login, or on a host that is down.
//
// Pure apart from the per-window `later` set. The rows and the triage are
// the Needs you tab's own (`visibleFleet`, `classify`), so a card and the tab
// never disagree. Control's own session is a card in its own chat only when
// its account is at its limit or without a login (control review F7): what
// it cannot say for itself. A signed-out Claude login is one card per host
// and login (F6, `hosts.signed_out_logins`), saying how to sign it in.
import { writable } from 'svelte/store';
import { classify, displayName, type AttentionOptions, type TriageBucket } from './attention';
import type { HostRow, SignedOutLogin } from './hosts';
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
  /** The session to open; null for a card about a login, not a session. */
  session: SessionRow | null;
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

/** Control's own row escalates only for these: the rest it reports itself. */
const OWN: Partial<Record<TriageBucket, string>> = {
  account_limit: "Control's account hit its limit; it moves to a login with headroom when one is free",
  no_credentials: "Control's login is signed out",
};

/** "Aug 9" (UTC), as `login_health::day_label`. */
export function dayLabel(at: number): string {
  const d = new Date(at * 1000);
  const m = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'][d.getUTCMonth()];
  return `${m} ${d.getUTCDate()}`;
}

/** Mirrors `login_health::card_text`: "Claude login expired on htz (since
 *  Aug 9) · ssh htz then claude /login". */
export function loginCardText(host: string, l: SignedOutLogin): string {
  const whose = l.profile ? `${host} (profile ${l.profile})` : host;
  const cmd = l.profile
    ? `ssh ${host} then CLAUDE_CONFIG_DIR=~/.claude-profiles/${l.profile} claude /login`
    : `ssh ${host} then claude /login`;
  return `Claude login expired on ${whose} (since ${dayLabel(l.since)}) · ${cmd}`;
}

/** One card per signed-out login on a visible host, minus those put off. */
export function loginEscalations(hosts: readonly HostRow[], later: ReadonlySet<string>): Escalation[] {
  const out: Escalation[] = [];
  for (const h of hosts) {
    if (h.hidden) continue;
    for (const l of h.signed_out_logins ?? []) {
      const key = `login:${h.alias}:${l.profile ?? ''}:${l.since}`;
      if (later.has(key)) continue;
      out.push({ key, session: null, bucket: 'no_credentials', line: loginCardText(h.alias, l), detail: null });
    }
  }
  return out;
}

/** What needs the person, worst first, leaving out the cards put off and,
 *  for Control itself, everything but its own limit or login. */
export function escalations(
  rows: readonly SessionRow[],
  opts: AttentionOptions,
  operatorId: number | null,
  later: ReadonlySet<string>,
  hosts: readonly HostRow[] = [],
): Escalation[] {
  const out: Escalation[] = loginEscalations(hosts, later);
  // A host that is down is one card, not one per session on it.
  const down = new Map<string, Escalation & { count: number }>();
  for (const s of rows) {
    const bucket = classify(s, opts);
    if (s.id === operatorId) {
      const own = OWN[bucket];
      const key = escalationKey(s, bucket);
      if (own && !later.has(key)) out.push({ key, session: s, bucket, line: own, detail: null });
      continue;
    }
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
    const one = e.session ? displayName(e.session, true) : host;
    out.push({ ...rest, line: `${host} is down: ${count === 1 ? `${one} waits on it` : `${count} sessions wait on it`}` });
  }
  return out.sort((a, b) => ORDER.indexOf(a.bucket) - ORDER.indexOf(b.bucket));
}

/** Cards put off with Later, for this window. */
export const laterEscalations = writable<ReadonlySet<string>>(new Set());

export function putOff(key: string): void {
  laterEscalations.update((s) => new Set([...s, key]));
}
