// The orbit map (automation runs board C): an ambient picture of what runs
// on the fleet's behalf. Planets are hosts, sized by their sessions; a comet
// is a live run circling its host, dimmed and slowed when its session has
// gone quiet, its trail growing with how long it has run; an amber halo is
// a run that waits for a person, a red spark one that failed in the last
// ten minutes; ticks on the outer ring are the next routine fires, the
// clock running clockwise from now at the top. PURE: `orbitModel` turns the
// runs, hosts, sessions and routines a client already holds into that
// picture; `OrbitMap.svelte` draws it.
import type { RunRow } from './runs';

/** A failure this recent still sparks. */
export const SPARK_SECS = 600;
/** A run waiting this long or less still haloes its host. */
export const HALO_SECS = 3600;
/** The outer ring's span: the next 12 hours of fires. */
export const RING_SECS = 12 * 3600;
/** A comet's trail is full after this long. */
export const TRAIL_FULL_SECS = 1800;
/** One lap, in seconds, of a busy comet and of one gone quiet. */
export const LAP_BUSY = 6;
export const LAP_QUIET = 16;

export interface OrbitComet {
  run_id: string;
  owner: string;
  /** Seconds a lap takes. */
  lap: number;
  /** 0.1 to 0.8 of the circle. */
  trail: number;
  quiet: boolean;
}

export interface OrbitHost {
  alias: string;
  sessions: number;
  /** Planet size, 0.6 to 1.2. */
  scale: number;
  comets: OrbitComet[];
  needs: boolean;
  failed: boolean;
  caption: string;
  /** Position in percent of the stage. */
  x: number;
  y: number;
}

export interface OrbitFire {
  routine: string;
  at: number;
  /** Clockwise from the top. */
  deg: number;
  label: string;
}

export interface OrbitFeed {
  at: number;
  who: string;
  what: string;
  tone: 'working' | 'waiting' | 'failed' | 'done' | 'idle' | 'jev';
}

export interface OrbitModel {
  hosts: OrbitHost[];
  fires: OrbitFire[];
  feed: OrbitFeed[];
  running: number;
  /** Jev decisions in the last ten minutes: diamonds on the hub ring. */
  jev: number;
}

interface SessionLike {
  id: number;
  host_alias: string;
  claude_status: string | null;
}
interface RoutineLike {
  name: string;
  enabled: boolean;
  trigger: string;
  host_alias: string;
  next_run_at?: number;
  skip_next?: boolean;
}

const hm = (sec: number) => {
  const d = new Date(sec * 1000);
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
};

function feedLine(r: RunRow): OrbitFeed {
  const at = r.ended_at ?? r.started_at;
  const who = r.owner;
  switch (r.outcome) {
    case 'running':
      return { at: r.started_at, who, what: r.summary ? `running: ${r.summary}` : `started${r.host ? ` on ${r.host}` : ''}`, tone: 'working' };
    case 'needs_person':
      return { at, who, what: r.summary ? `asks: ${r.summary}` : 'waits for you', tone: 'waiting' };
    case 'failed':
      return { at, who, what: r.error ? `failed: ${r.error}` : 'failed', tone: 'failed' };
    case 'nothing_to_do':
      return { at, who, what: r.summary ?? 'had nothing to do', tone: 'idle' };
    default:
      return { at, who, what: r.summary ?? 'done', tone: r.source === 'jev' ? 'jev' : 'done' };
  }
}

/**
 * PURE: the orbit picture at `now`. Hosts are every host alias known (from
 * `hostAliases` and the runs), laid on a circle in name order; only the
 * runs a client holds count, so a narrower read draws less.
 */
export function orbitModel(args: {
  runs: RunRow[];
  hostAliases: string[];
  sessions: SessionLike[];
  routines: RoutineLike[];
  now: number;
}): OrbitModel {
  const { runs, sessions, routines, now } = args;
  const aliases = new Set(args.hostAliases);
  for (const r of runs) if (r.host && r.outcome === 'running') aliases.add(r.host);
  const sessionById = new Map(sessions.map((s) => [s.id, s]));
  const count = new Map<string, number>();
  for (const s of sessions) count.set(s.host_alias, (count.get(s.host_alias) ?? 0) + 1);
  const most = Math.max(1, ...count.values());

  const list = [...aliases].sort((a, b) => a.localeCompare(b));
  const hosts: OrbitHost[] = list.map((alias, i) => {
    const mine = runs.filter((r) => r.host === alias);
    const comets = mine
      .filter((r) => r.outcome === 'running')
      .map((r) => {
        const sid = r.session_ids[0];
        const status = sid !== undefined ? sessionById.get(sid)?.claude_status : undefined;
        const quiet = status !== undefined && status !== null && status !== 'working';
        const age = Math.max(0, now - r.started_at);
        return {
          run_id: r.id,
          owner: r.owner,
          lap: quiet ? LAP_QUIET : LAP_BUSY,
          trail: 0.1 + 0.7 * Math.min(age / TRAIL_FULL_SECS, 1),
          quiet,
        };
      });
    const needs = mine.some((r) => r.outcome === 'needs_person' && now - (r.ended_at ?? r.started_at) <= HALO_SECS);
    const failed = mine.some((r) => r.outcome === 'failed' && now - (r.ended_at ?? r.started_at) <= SPARK_SECS);
    const next = routines
      .filter((r) => r.host_alias === alias && r.enabled && r.trigger === 'cron' && !r.skip_next && (r.next_run_at ?? 0) > now)
      .sort((a, b) => (a.next_run_at ?? 0) - (b.next_run_at ?? 0))[0];
    const caption =
      comets.length > 0
        ? comets.map((c) => c.owner + (c.quiet ? ' · going quiet' : '')).join(' · ')
        : next
          ? `quiet · next fire ${hm(next.next_run_at!)}`
          : 'quiet';
    // One host sits top-left; more go round the hub, from the top.
    const angle = list.length === 1 ? -Math.PI * 0.75 : -Math.PI / 2 + (i / list.length) * Math.PI * 2;
    const n = count.get(alias) ?? 0;
    return {
      alias,
      sessions: n,
      scale: 0.6 + 0.6 * (n / most),
      comets,
      needs,
      failed,
      caption,
      x: 50 + 30 * Math.cos(angle),
      y: 50 + 30 * Math.sin(angle),
    };
  });

  const fires = routines
    .filter((r) => r.enabled && r.trigger === 'cron' && !r.skip_next && r.next_run_at !== undefined && r.next_run_at > now && r.next_run_at - now <= RING_SECS)
    .map((r) => ({ routine: r.name, at: r.next_run_at!, deg: ((r.next_run_at! - now) / RING_SECS) * 360, label: hm(r.next_run_at!) }))
    .sort((a, b) => a.at - b.at);

  const feed = runs
    .map(feedLine)
    .sort((a, b) => b.at - a.at)
    .slice(0, 6);

  return {
    hosts,
    fires,
    feed,
    running: runs.filter((r) => r.outcome === 'running').length,
    jev: runs.filter((r) => r.source === 'jev' && now - r.started_at <= SPARK_SECS).length,
  };
}
