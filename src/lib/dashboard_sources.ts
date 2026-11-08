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
import type { AccountUsageSnapshot } from './account_usage_store';
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

/** The longest feed value the hub keeps (`FEED_VALUE_MAX_CHARS`). */
export const FEED_VALUE_MAX = 4000;

/** What the desktop holds that a `fleet` widget counts. */
export interface FleetData {
  sessions: readonly SessionRow[];
  hosts: readonly HostRow[];
  accounts: readonly AccountRow[];
  usage: Readonly<Record<string, AccountUsageSnapshot>>;
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

function bucketOf(s: SessionRow): SessionBucket {
  if (s.claude_status === 'working') return 'working';
  if (s.claude_status === 'blocked' || s.status === 'blocked') return 'needs_you';
  return 'idle';
}

const BUCKET_WORDS: Record<SessionBucket, string> = { working: 'Working', needs_you: 'Needs you', idle: 'Idle' };

const pct = (n: number) => `${Math.round(n)}%`;

/** A source's facts from what the desktop holds. */
export function fleetFacts(source: FleetSource, d: FleetData): FleetFacts {
  switch (source) {
    case 'sessions': {
      const live = liveSessions(d.sessions);
      const n = { working: 0, needs_you: 0, idle: 0 };
      for (const s of live) n[bucketOf(s)] += 1;
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
      const shown = d.hosts.filter((h) => !h.hidden);
      const up = shown.filter((h) => h.reachable).length;
      return {
        value: `${up}/${shown.length}`,
        note: 'hosts reachable',
        warn: up < shown.length,
        rows: shown.map((h) => ({
          label: h.alias,
          value: h.reachable ? (h.load_1m != null ? `load ${h.load_1m.toFixed(1)}` : 'online') : 'offline',
          ...(h.reachable ? {} : { warn: true }),
        })),
      };
    }
    case 'accounts': {
      const rows: (FleetRow & { five: number })[] = d.accounts.map((a) => {
        const u = d.usage[a.uuid]?.usage;
        const five = u?.five_hour?.utilization;
        const week = u?.seven_day?.utilization;
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

/** What a summary widget sends besides its request (`context`): the facts
 *  of each source but work, which the hub counts itself. Empty: none. */
export function summaryContext(
  sources: readonly SummarySource[] | undefined,
  d: FleetData,
  feeds: Readonly<Record<string, DashboardFeed>>,
): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const s of sources ?? []) {
    if (s === 'work') continue;
    if (s === 'feeds') {
      out.feeds = Object.fromEntries(Object.values(feeds).map((f) => [f.key, { value: f.value, by: feedBy(f), at: f.updated_at }]));
      continue;
    }
    const f = fleetFacts(s, d);
    out[s] = { summary: `${f.value} ${f.note}`, rows: f.rows.map((r) => `${r.label}: ${r.value}`) };
  }
  return out;
}
