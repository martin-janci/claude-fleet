// What a dashboard reads besides the work's counts (owner report
// 2026-11-06, "dashboards from dynamic data"):
//
// - a `fleet` widget counts the fleet's own rows as this desktop already
//   holds them: sessions by state, hosts reachable, accounts' usage;
// - a `feed` widget shows the value written last to its key, by Control,
//   the person, or a routine's reply (`dashboards { feed }`,
//   `<dashboard-feed key="…">`);
// - a summary widget with `sources` sends the same facts with its request,
//   so the model answers from them too (`summaryContext`).
//
// Pure: the stores are read by the caller.
import type { AccountRow } from './accounts';
import { accountLabel } from './accounts';
import type { AccountUsageSnapshot, UsageWindow } from './account_usage_store';
import { freshness, usedPct, type UsageWindowKind } from './account_usage';
import { classify, countNeedsYou, type AttentionFacts } from './attention';
import { hostLoad } from './host_health';
import type { HostRow } from './hosts';
import type { SessionRow } from './sessions';
import type { FleetSource, SummarySource } from './dashboards';

/** A feed as the hub answers it (`DashboardFeedRow`). */
export interface DashboardFeed {
  key: string;
  value: string;
  /** person | agent | routine. */
  by: string;
  /** The routine's name, for a routine's write. */
  by_ref?: string | null;
  updated_at: number;
}

/** What the desktop holds that a `fleet` widget counts. */
export interface FleetData {
  sessions: readonly SessionRow[];
  hosts: readonly HostRow[];
  accounts: readonly AccountRow[];
  usage: Readonly<Record<string, AccountUsageSnapshot>>;
  /** Hosts down and accounts at a limit, as the Needs you badge reads
   *  them (`attention_facts.ts`). */
  facts?: AttentionFacts;
}

export interface FleetRow {
  label: string;
  value: string;
  warn?: boolean;
}

/** A `fleet` widget's answer: its one number and words, and a row each. */
export interface FleetFacts {
  value: string;
  note: string;
  warn: boolean;
  rows: FleetRow[];
}

/** An account's use at which its row is drawn in the warning colour. */
export const USAGE_WARN_PCT = 80;

/** The sessions a widget counts: the live ones, background agents left out. */
function liveSessions(list: readonly SessionRow[]): SessionRow[] {
  return list.filter((s) => s.lost_at == null && s.kind !== 'bg' && !s.fleet);
}

type SessionBucket = 'working' | 'needs_you' | 'idle';

const BUCKET_WORDS: Record<SessionBucket, string> = { working: 'Working', needs_you: 'Needs you', idle: 'Idle' };

const pct = (n: number) => `${Math.round(n)}%`;

/** A window's % used, as every usage surface shows it: `null` once its
 *  number is withheld (never read, too old, or past its reset). */
function shownUsed(kind: UsageWindowKind, w: UsageWindow | null | undefined, snap: AccountUsageSnapshot | undefined, now: number): number | null {
  if (!w || freshness(kind, snap?.fetched_at ?? null, w.resets_at, now, w.utilization) === 'expired') return null;
  return usedPct(w);
}

/** A source's facts from what the desktop holds, at `now` (unix seconds). */
export function fleetFacts(source: FleetSource, d: FleetData, now: number = Math.floor(Date.now() / 1000)): FleetFacts {
  switch (source) {
    case 'sessions': {
      const live = liveSessions(d.sessions);
      // The Needs you badge's own rule (`attention.ts`).
      const opts = { idleSecs: 0, now, facts: d.facts };
      const needs = countNeedsYou(live, opts);
      const working = live.filter((s) => classify(s, opts) === 'working').length;
      const n = { working, needs_you: needs, idle: live.length - working - needs };
      return {
        value: String(n.working),
        note: `working · ${n.needs_you} need you · ${n.idle} idle`,
        warn: false,
        rows: (Object.keys(BUCKET_WORDS) as SessionBucket[]).map((b) => ({
          label: BUCKET_WORDS[b],
          value: String(n[b]),
          ...(b === 'needs_you' && n[b] > 0 ? { warn: true } : {}),
        })),
      };
    }
    case 'hosts': {
      // The status bar's own reading (`host_health.ts`): `local` is always
      // up, and a host in trouble says what.
      const shown = d.hosts.filter((h) => !h.hidden).map((h) => ({ h, l: hostLoad(h, now) }));
      const up = shown.filter(({ l }) => l.reachable).length;
      return {
        value: `${up}/${shown.length}`,
        note: 'hosts reachable',
        warn: shown.some(({ l }) => l.trouble != null),
        rows: shown.map(({ h, l }) => ({
          label: h.alias,
          value: !l.reachable ? 'offline' : (l.trouble ?? (h.load_1m != null ? `load ${h.load_1m.toFixed(1)}` : 'online')),
          ...(l.trouble != null ? { warn: true } : {}),
        })),
      };
    }
    case 'accounts': {
      const rows: (FleetRow & { five: number })[] = d.accounts.map((a) => {
        const snap = d.usage[a.uuid];
        const five = shownUsed('5h', snap?.usage?.five_hour, snap, now);
        const week = shownUsed('weekly', snap?.usage?.seven_day, snap, now);
        const parts = [five != null ? `5h ${pct(five)}` : null, week != null ? `week ${pct(week)}` : null].filter(Boolean);
        const top = Math.max(five ?? 0, week ?? 0);
        return {
          label: accountLabel(a),
          value: parts.length ? parts.join(' · ') : 'no usage read',
          five: five ?? -1,
          ...(top >= USAGE_WARN_PCT ? { warn: true } : {}),
        };
      });
      const busiest = rows.reduce<(typeof rows)[number] | null>((m, r) => (m == null || r.five > m.five ? r : m), null);
      const read = busiest != null && busiest.five >= 0;
      return {
        value: read ? pct(busiest.five) : '—',
        note: read ? `highest 5-hour use · ${busiest.label}` : 'no usage read yet',
        warn: read && busiest.five >= USAGE_WARN_PCT,
        rows: rows.map(({ five: _f, ...r }) => r),
      };
    }
  }
}

/** A feed's value as one number: its first line, when that is short. */
export function feedStat(value: string): string {
  const first = value.split('\n', 1)[0].trim();
  return first.length <= 16 ? first : `${first.slice(0, 15)}…`;
}

/** Who wrote a feed, in words. */
export function feedBy(f: DashboardFeed): string {
  if (f.by === 'routine') return f.by_ref ? `routine ${f.by_ref}` : 'a routine';
  if (f.by === 'agent') return 'Control';
  return 'you';
}

/** The bytes of JSON a summary's feeds may take together: the hub refuses a
 *  context over 24 KiB (`SUMMARY_CONTEXT_MAX_BYTES`), and the other sources,
 *  keys and stamps keep the rest. */
export const SUMMARY_FEEDS_MAX_BYTES = 20 * 1024;

/** The hub's cap on a summary's whole context (`SUMMARY_CONTEXT_MAX_BYTES`
 *  in drafts.rs), less a little slack for the object's own braces. */
export const SUMMARY_CONTEXT_MAX_BYTES = 24 * 1024 - 64;

/** The bytes of JSON each fleet source (hosts, accounts, sessions) may take:
 *  a fleet of many hosts or logins must not crowd the feeds out, or put the
 *  context over the hub's cap on its own (review w04-6). */
export const SUMMARY_SOURCE_MAX_BYTES = 3 * 1024;

const utf8 = new TextEncoder();
const jsonBytes = (v: string) => utf8.encode(JSON.stringify(v)).length;

/** `value`, cut at a character so that it is at most `max` bytes as a JSON
 *  string, its end marked with "…". */
export function cutToJsonBytes(value: string, max: number): string {
  if (jsonBytes(value) <= max) return value;
  const chars = Array.from(value);
  let lo = 0;
  let hi = chars.length;
  while (lo < hi) {
    const mid = Math.ceil((lo + hi) / 2);
    if (jsonBytes(chars.slice(0, mid).join('') + '…') <= max) lo = mid;
    else hi = mid - 1;
  }
  return chars.slice(0, lo).join('') + '…';
}

/** What a summary widget sends besides its request (`context`): the facts
 *  of each source but work, which the hub counts itself. Empty: none.
 *  `feedKeys`: the feeds its dashboard's `feed` widgets name, the only ones
 *  sent (all of a person's feeds would pass the hub's cap on context). */
export function summaryContext(
  sources: readonly SummarySource[] | undefined,
  d: FleetData,
  feeds: Readonly<Record<string, DashboardFeed>>,
  feedKeys: readonly string[] = [],
  now: number = Math.floor(Date.now() / 1000),
): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  const wanted = sources ?? [];
  // The fleet's own sources first, each within its own cap; the feeds then
  // share what is left of the hub's cap on the whole context, so the hub
  // never refuses it for good (reviews w14-2, w04-6).
  for (const s of wanted) {
    if (s === 'work' || s === 'feeds') continue;
    const f = fleetFacts(s, d, now);
    const summary = `${f.value} ${f.note}`;
    out[s] = { summary, rows: capRows(summary, f.rows.map((r) => `${r.label}: ${r.value}`)) };
  }
  if (wanted.includes('feeds')) {
    const named = [...new Set(feedKeys)].flatMap((k) => (feeds[k] ? [feeds[k]] : []));
    const empty = Object.fromEntries(named.map((f) => [f.key, { value: '', by: feedBy(f), at: f.updated_at }]));
    const left = SUMMARY_CONTEXT_MAX_BYTES - objBytes({ ...out, feeds: empty });
    // Each feed gets its share, or a few long values (non-ASCII, escaped
    // newlines) put the whole context over the cap.
    const budget = Math.max(0, Math.min(SUMMARY_FEEDS_MAX_BYTES, left));
    const share = Math.max(2, Math.floor(budget / Math.max(1, named.length)));
    out.feeds = Object.fromEntries(
      named.map((f) => [f.key, { value: cutToJsonBytes(f.value, share), by: feedBy(f), at: f.updated_at }]),
    );
  }
  return out;
}

const objBytes = (v: unknown) => utf8.encode(JSON.stringify(v)).length;

/** `rows`, as many as fit in [`SUMMARY_SOURCE_MAX_BYTES`] next to
 *  `summary`, the rest named by their count. */
function capRows(summary: string, rows: string[]): string[] {
  if (objBytes({ summary, rows }) <= SUMMARY_SOURCE_MAX_BYTES) return rows;
  const kept: string[] = [];
  for (const r of rows) {
    const more = `… and ${rows.length - kept.length - 1} more`;
    if (objBytes({ summary, rows: [...kept, r, more] }) > SUMMARY_SOURCE_MAX_BYTES) break;
    kept.push(r);
  }
  return [...kept, `… and ${rows.length - kept.length} more`];
}
