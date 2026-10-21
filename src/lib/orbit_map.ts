// The orbit map (automation runs board C): an ambient picture of what runs
// on the fleet's behalf. Planets are hosts, sized by their sessions; a comet
// is a live run circling its host, dimmed and slowed when its session has
// gone quiet, its trail growing with how long it has run; an amber halo is
// a run that waits for a person, a red spark one that failed in the last
// ten minutes. Sessions at work circle their planet as satellites; the
// last hour's finished runs sit on the host's ring as moons, coloured by
// how they ended; a host that ran something lately ripples, and its spoke
// to the hub carries the pulse. The outer ring is a 24-hour clock with now
// at the top: the next 12 hours of routine fires tick clockwise down the
// right half, the last 12 hours of runs dot anticlockwise down the left.
// PURE: `orbitModel` turns the
// runs, hosts, sessions and routines a client already holds into that
// picture; `OrbitMap.svelte` draws it.
import type { RunRow } from './runs';

/** A failure this recent still sparks. */
const SPARK_SECS = 600;
/** A run waiting this long or less still haloes its host. */
const HALO_SECS = 3600;
/** Each half of the outer ring: the next 12 hours of fires on the right,
 *  the last 12 hours of runs on the left. */
export const RING_SECS = 12 * 3600;
/** A run started or ended this recently still ripples its host. */
export const RECENT_SECS = 15 * 60;
/** A run that ended this recently is a moon on its host's ring. */
const MOON_SECS = 3600;
/** At most this many moons, satellites and past dots per host / ring. */
const MOONS_MAX = 6;
const SATELLITES_MAX = 6;
const PAST_MAX = 48;
/** A host name longer than this drops the segments it shares with another. */
const LABEL_MAX = 10;
/** A comet's trail is full after this long. */
const TRAIL_FULL_SECS = 1800;
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

export type OrbitTone = 'working' | 'waiting' | 'failed' | 'done' | 'idle' | 'jev';

/** A finished run on its host's ring. */
export interface OrbitMoon {
  run_id: string;
  owner: string;
  tone: OrbitTone;
  /** Position in percent of the host box. */
  x: number;
  y: number;
  title: string;
}

/** A finished run on the outer ring's left half. */
export interface OrbitPast {
  run_id: string;
  tone: OrbitTone;
  /** Anticlockwise from the top: 0 to -180. */
  deg: number;
  title: string;
}

export interface OrbitHost {
  alias: string;
  /** What the map prints: the alias, or its tail when it shares a long prefix. */
  label: string;
  sessions: number;
  /** Sessions at work here that no live run already draws: satellites. */
  working: number;
  /** 0 to 1: how lately something ran here (1 is just now). */
  heat: number;
  moons: OrbitMoon[];
  /** Planet size, 0.6 to 1.2. */
  scale: number;
  comets: OrbitComet[];
  /** Something is at work here: a live run or a working session. */
  busy: boolean;
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
  run_id: string;
  at: number;
  who: string;
  what: string;
  tone: OrbitTone;
}

export interface OrbitModel {
  hosts: OrbitHost[];
  fires: OrbitFire[];
  past: OrbitPast[];
  feed: OrbitFeed[];
  running: number;
  /** Sessions at work across the fleet. */
  working: number;
  /** 0 to 1: how lately a run with no host ran (it ripples the hub). */
  hubHeat: number;
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

/** "09:05": a moment as the map's clock reads it, in local time. */
export function clockLabel(sec: number): string {
  const d = new Date(sec * 1000);
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
}

/** When a run ended, or started if it has not. */
const endOf = (r: RunRow) => r.ended_at ?? r.started_at;

/** "in 52m", "in 3h 10m". */
function inLabel(secs: number): string {
  const m = Math.max(1, Math.round(secs / 60));
  if (m < 60) return `in ${m}m`;
  const h = Math.floor(m / 60);
  return m % 60 ? `in ${h}h ${m % 60}m` : `in ${h}h`;
}

/** "just now", "12m ago", "3h ago". */
function agoLabel(secs: number): string {
  if (secs < 60) return 'just now';
  const m = Math.round(secs / 60);
  return m < 60 ? `${m}m ago` : `${Math.round(m / 60)}h ago`;
}

/** A summary that repeats its owner ("turn_outcome turn_outcome (assist)…")
 *  says it once. */
function tidy(what: string, who: string): string {
  return what.startsWith(`${who} `) ? what.slice(who.length + 1) : what;
}

function toneOf(r: RunRow): OrbitTone {
  switch (r.outcome) {
    case 'running':
      return 'working';
    case 'needs_person':
      return 'waiting';
    case 'failed':
      return 'failed';
    case 'nothing_to_do':
      return 'idle';
    default:
      return r.source === 'jev' ? 'jev' : 'done';
  }
}

function feedLine(r: RunRow): OrbitFeed {
  const at = endOf(r);
  const who = r.owner;
  const tone = toneOf(r);
  const line = (what: string, when = at) => ({ run_id: r.id, at: when, who, what: tidy(what, who), tone });
  switch (r.outcome) {
    case 'running':
      return line(r.summary ? `running: ${r.summary}` : `started${r.host ? ` on ${r.host}` : ''}`, r.started_at);
    case 'needs_person':
      return line(r.summary ? `asks: ${r.summary}` : 'waits for you');
    case 'failed':
      return line(r.error ? `failed: ${r.error}` : 'failed');
    case 'nothing_to_do':
      return line(r.summary ?? 'had nothing to do');
    default:
      return line(r.summary ?? 'done');
  }
}

/**
 * PURE: what the map prints for each host. A short alias prints whole; a
 * long one that shares its leading segments (split on `-`, `_`, `.`) with
 * another host drops them, so `claude-fleet-ci` and `claude-fleet-trn`
 * read `ci` and `trn` instead of two clipped `claude-fl…`. The full alias
 * stays in the tooltip and the accessible name. Two shortened labels that
 * would print the same (`prod-eu-web1` and `prod-us-web1` both `web1`) put
 * back leading segments until they differ, so no two planets share a name.
 */
export function hostLabels(aliases: string[]): Map<string, string> {
  const seg = (a: string) => a.split(/(?<=[-_.])/);
  const segs = new Map<string, string[]>();
  const drop = new Map<string, number>();
  for (const a of aliases) {
    const mine = seg(a);
    segs.set(a, mine);
    if (a.length <= LABEL_MAX) {
      drop.set(a, 0);
      continue;
    }
    let shared = 0;
    for (const b of aliases) {
      if (b === a) continue;
      const theirs = seg(b);
      let n = 0;
      while (n < mine.length - 1 && n < theirs.length && mine[n] === theirs[n]) n++;
      shared = Math.max(shared, n);
    }
    drop.set(a, shared);
  }
  const label = (a: string) => (segs.get(a) ?? [a]).slice(drop.get(a) ?? 0).join('');
  // Each pass puts one segment back on every shortened label that collides;
  // a label with nothing dropped is its full alias, unique by definition.
  for (;;) {
    const byLabel = new Map<string, string[]>();
    for (const a of drop.keys()) {
      const l = label(a);
      byLabel.set(l, [...(byLabel.get(l) ?? []), a]);
    }
    let changed = false;
    for (const group of byLabel.values()) {
      if (group.length < 2) continue;
      for (const a of group) {
        const d = drop.get(a) ?? 0;
        if (d > 0) {
          drop.set(a, d - 1);
          changed = true;
        }
      }
    }
    if (!changed) break;
  }
  const out = new Map<string, string>();
  for (const a of drop.keys()) out.set(a, label(a));
  return out;
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
  const labels = hostLabels(list);
  const heatOf = (rs: RunRow[]) => {
    const last = Math.max(-Infinity, ...rs.map((r) => (r.outcome === 'running' ? now : endOf(r))));
    const age = now - last;
    return age >= 0 && age < RECENT_SECS ? Math.round((1 - age / RECENT_SECS) * 100) / 100 : 0;
  };
  // Every finished run, newest first: a host's moons and the past ring read it.
  const finished = runs.filter((r) => r.outcome !== 'running').sort((a, b) => endOf(b) - endOf(a));
  const hosts: OrbitHost[] = list.map((alias, i) => {
    const mine = runs.filter((r) => r.host === alias);
    const mineDone = finished.filter((r) => r.host === alias);
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
    const needs = mine.some((r) => r.outcome === 'needs_person' && now - endOf(r) <= HALO_SECS);
    const failed = mine.some((r) => r.outcome === 'failed' && now - endOf(r) <= SPARK_SECS);
    const next = routines
      .filter((r) => r.host_alias === alias && r.enabled && r.trigger === 'cron' && !r.skip_next && (r.next_run_at ?? 0) > now)
      .sort((a, b) => (a.next_run_at ?? 0) - (b.next_run_at ?? 0))[0];
    const drawn = new Set(mine.filter((r) => r.outcome === 'running').flatMap((r) => r.session_ids.slice(0, 1)));
    const working = sessions.filter((s) => s.host_alias === alias && s.claude_status === 'working' && !drawn.has(s.id)).length;
    const lastEnded = mineDone.length > 0 ? endOf(mineDone[0]) : -Infinity;
    const moonRuns = mineDone.filter((r) => now - endOf(r) <= MOON_SECS).slice(0, MOONS_MAX);
    // Moons sit on the ring from the top, the newest first, a sixth apart.
    const moons = moonRuns.map((r, k) => {
      const a = -Math.PI / 2 + (k * Math.PI) / 3 + Math.PI / 6;
      const at = endOf(r);
      const tone = toneOf(r);
      return {
        run_id: r.id,
        owner: r.owner,
        tone,
        x: 50 + 50 * Math.cos(a),
        y: 50 + 50 * Math.sin(a),
        title: `${r.owner} · ${feedLine(r).what} · ${agoLabel(now - at)}`,
      };
    });
    const tail = next
      ? `next ${inLabel(next.next_run_at! - now)}`
      : Number.isFinite(lastEnded)
        ? `last ran ${agoLabel(now - lastEnded)}`
        : '';
    const caption =
      comets.length > 0
        ? comets.map((c) => c.owner + (c.quiet ? ' · going quiet' : '')).join(' · ')
        : working > 0
          ? `${working} working${tail ? ` · ${tail}` : ''}`
          : `quiet${tail ? ` · ${tail}` : ''}`;
    // One host sits top-left; more go round the hub, from the top.
    const angle = list.length === 1 ? -Math.PI * 0.75 : -Math.PI / 2 + (i / list.length) * Math.PI * 2;
    const n = count.get(alias) ?? 0;
    return {
      alias,
      label: labels.get(alias) ?? alias,
      sessions: n,
      working: Math.min(working, SATELLITES_MAX),
      heat: heatOf(mine),
      moons,
      scale: 0.6 + 0.6 * (n / most),
      comets,
      busy: comets.length > 0 || working > 0,
      needs,
      failed,
      caption,
      x: 50 + 30 * Math.cos(angle),
      y: 50 + 30 * Math.sin(angle),
    };
  });

  const fires = routines
    .filter((r) => r.enabled && r.trigger === 'cron' && !r.skip_next && r.next_run_at !== undefined && r.next_run_at > now && r.next_run_at - now <= RING_SECS)
    .map((r) => ({ routine: r.name, at: r.next_run_at!, deg: ((r.next_run_at! - now) / RING_SECS) * 180, label: clockLabel(r.next_run_at!) }))
    .sort((a, b) => a.at - b.at);

  const past = finished
    .filter((r) => now - endOf(r) <= RING_SECS && endOf(r) <= now)
    .slice(0, PAST_MAX)
    .map((r) => {
      const at = endOf(r);
      return { run_id: r.id, tone: toneOf(r), deg: -((now - at) / RING_SECS) * 180, title: `${clockLabel(at)} · ${r.owner} · ${feedLine(r).what}` };
    });

  const feed = runs
    .map(feedLine)
    .sort((a, b) => b.at - a.at)
    .slice(0, 6);

  return {
    hosts,
    fires,
    past,
    feed,
    running: runs.filter((r) => r.outcome === 'running').length,
    working: sessions.filter((s) => s.claude_status === 'working').length,
    hubHeat: heatOf(runs.filter((r) => !r.host || !aliases.has(r.host))),
    jev: runs.filter((r) => r.source === 'jev' && now - r.started_at <= SPARK_SECS).length,
  };
}

/** The stage's accessible name (review w19-12): the counts, and which hosts
 *  have a run waiting for you or one that failed lately, which the map
 *  shows only as an amber halo and a red spark. */
export function orbitStageLabel(m: Pick<OrbitModel, 'hosts' | 'running' | 'working'>): string {
  const base = `${m.running} running and ${m.working} working across ${m.hosts.length} hosts`;
  const waits = m.hosts.filter((h) => h.needs).map((h) => h.alias);
  const failed = m.hosts.filter((h) => h.failed).map((h) => h.alias);
  const parts = [base];
  if (waits.length) parts.push(`a run waits for you on ${waits.join(', ')}`);
  if (failed.length) parts.push(`a run failed lately on ${failed.join(', ')}`);
  return parts.join(' · ');
}
