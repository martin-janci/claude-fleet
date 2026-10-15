// Host health in the status bar (Thread room, 2026-10-14; board Main's
// footer and A4Events): CPU, memory and disk of the host the selected
// session runs on (in Control with nothing selected, the operator's host),
// and a flag when another host is in trouble; a click lists every host.
//
// Pure. The numbers are the host row's health sample (`host:pinged`,
// migration 193 adds cpu_pct, mem_pressure and the two judgments mem_low
// and cpu_saturated); a sample older than `HEALTH_SAMPLE_FRESH_SECS` says
// nothing about now, so its numbers show but no trouble is judged on them.
import type { HostRow } from './hosts';
import { diskMeter, healthSampleFresh, memUsedPct } from './hosts_view';

export type LoadLevel = 'ok' | 'warn' | 'crit';

export interface HostLoad {
  alias: string;
  reachable: boolean;
  /** The sample is recent enough to judge. */
  fresh: boolean;
  cpu: number | null;
  mem: number | null;
  /** Memory pressure 0–100, when the host reports one. */
  pressure: number | null;
  disk: number | null;
  /** When it was sampled, unix seconds. */
  at: number | null;
  /** What is wrong, in a few words ("mem 94%", "offline"); null when fine. */
  trouble: string | null;
  level: LoadLevel;
  cpuLevel: LoadLevel;
  memLevel: LoadLevel;
  diskLevel: LoadLevel;
}

export interface LoadLimits {
  /** `health.disk_low_pct`: used percent of `$HOME` that counts as low. */
  diskLowPct: number;
  /** `health.mem_low_pct`: AVAILABLE percent at or under which memory is low. */
  memLowPct: number;
  /** `health.cpu_busy_pct`: busy percent that counts as saturated. */
  cpuBusyPct: number;
}

export const DEFAULT_LIMITS: LoadLimits = { diskLowPct: 90, memLowPct: 10, cpuBusyPct: 90 };

const pct = (v: number | null | undefined): number | null => (v == null || !Number.isFinite(v) ? null : Math.round(Math.min(100, Math.max(0, v))));

function level(v: number | null, warn: number, crit: number): LoadLevel {
  if (v == null) return 'ok';
  return v >= crit ? 'crit' : v >= warn ? 'warn' : 'ok';
}

/** One host's numbers and whether they are trouble. */
export function hostLoad(h: HostRow, now: number, limits: LoadLimits = DEFAULT_LIMITS): HostLoad {
  const fresh = healthSampleFresh(h, now);
  const cpu = pct(h.cpu_pct);
  const mem = memUsedPct(h);
  const disk = diskMeter(h)?.pct ?? null;
  const memHigh = 100 - limits.memLowPct;
  const cpuLevel = level(cpu, Math.min(75, limits.cpuBusyPct), limits.cpuBusyPct);
  const memLevel = fresh && h.mem_low ? 'crit' : level(mem, Math.min(85, memHigh), memHigh);
  const diskLevel = level(disk, limits.diskLowPct, Math.max(limits.diskLowPct, 95));
  let trouble: string | null = null;
  if (!h.reachable && h.alias !== 'local') trouble = 'offline';
  else if (fresh && (h.mem_low || (mem != null && mem >= memHigh))) trouble = `mem ${mem ?? '—'}%`;
  else if (fresh && h.cpu_saturated) trouble = `cpu ${cpu ?? '—'}%`;
  else if (fresh && disk != null && disk >= limits.diskLowPct) trouble = `disk ${disk}%`;
  const worst: LoadLevel = trouble
    ? 'crit'
    : [cpuLevel, memLevel, diskLevel].includes('warn') && fresh
      ? 'warn'
      : 'ok';
  return {
    alias: h.alias,
    reachable: h.reachable || h.alias === 'local',
    fresh,
    cpu,
    mem,
    pressure: pct(h.mem_pressure),
    disk,
    at: h.health_at ?? null,
    trouble,
    level: worst,
    cpuLevel: fresh ? cpuLevel : 'ok',
    memLevel: fresh ? memLevel : 'ok',
    diskLevel: fresh ? diskLevel : 'ok',
  };
}

/** The chip's words for a host: "CPU 18% · Mem 55% · Disk 40%", the parts
 *  it answered; "offline" or "not sampled yet" when it answered none. */
export function loadParts(l: HostLoad): string[] {
  if (!l.reachable) return ['offline'];
  const parts: string[] = [];
  if (l.cpu != null) parts.push(`CPU ${l.cpu}%`);
  if (l.mem != null) parts.push(`Mem ${l.mem}%`);
  if (l.disk != null) parts.push(`Disk ${l.disk}%`);
  return parts.length ? parts : ['not sampled yet'];
}

/** Every visible host's load: the focused one first, then those in
 *  trouble, then the rest by name. */
export function fleetLoads(rows: readonly HostRow[], focus: string | null, now: number, limits: LoadLimits = DEFAULT_LIMITS): HostLoad[] {
  const loads = rows.filter((h) => !h.hidden || h.alias === focus).map((h) => hostLoad(h, now, limits));
  const rank = (l: HostLoad) => (l.alias === focus ? 0 : l.trouble ? 1 : 2);
  return loads.sort((a, b) => rank(a) - rank(b) || a.alias.localeCompare(b.alias));
}

/** The other host whose trouble the chip names, worst first (offline,
 *  then memory, CPU, disk); null when every other host is fine. */
export function otherTrouble(loads: readonly HostLoad[], focus: string | null): HostLoad | null {
  const order = (t: string) => (t === 'offline' ? 0 : t.startsWith('mem') ? 1 : t.startsWith('cpu') ? 2 : 3);
  const others = loads.filter((l) => l.alias !== focus && l.trouble);
  others.sort((a, b) => order(a.trouble!) - order(b.trouble!));
  return others[0] ?? null;
}

/**
 * Which host the chip follows: the selected session's, or the operator's
 * in Control when the right panel shows no session (Control's chat is the
 * operator's session). Outside Control with nothing selected, the
 * operator's host too.
 */
export function chipHost(input: {
  inControl: boolean;
  sessionShown: boolean;
  selectedHost: string | null;
  operatorHost: string;
}): string {
  if (input.inControl) return input.sessionShown && input.selectedHost ? input.selectedHost : input.operatorHost;
  return input.selectedHost ?? input.operatorHost;
}

/** "checked 20s ago" for the popover. */
export function checkedAgo(at: number | null, now: number): string {
  if (at == null) return 'not sampled yet';
  const s = Math.max(0, now - at);
  if (s < 60) return `checked ${s}s ago`;
  if (s < 3600) return `checked ${Math.floor(s / 60)}m ago`;
  return `checked ${Math.floor(s / 3600)}h ago`;
}
