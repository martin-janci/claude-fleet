// The Debug devices board's pure parts (Settings › Organisations › Debug
// devices, `DebugDevicesBoard.svelte`): which devices a filter keeps, how
// they group by host, the header's counts, and which actions a card offers.
// The row, the commands and the shared word helpers live in
// `debug_devices.ts`; this module only arranges them for the board.
import {
  DESKTOP_HOLDER,
  canMirror,
  claimLeft,
  holderName,
  isVirtual,
  type DebugDevice,
} from './debug_devices';

/** The state filter's chips. "Ready to use" is ready and free: what a person
 *  scanning the board for a phone to pick up wants. */
export type StateFilter = 'all' | 'ready' | 'in-use' | 'not-ready';
export type PlatformFilter = 'any' | 'android' | 'ios';

export const STATE_FILTERS: readonly { id: StateFilter; label: string }[] = [
  { id: 'all', label: 'All' },
  { id: 'ready', label: 'Ready to use' },
  { id: 'in-use', label: 'In use' },
  { id: 'not-ready', label: 'Not ready' },
];

export const PLATFORM_FILTERS: readonly { id: Exclude<PlatformFilter, 'any'>; label: string }[] = [
  { id: 'android', label: 'Android' },
  { id: 'ios', label: 'iOS' },
];

/** Whether `d` passes the state chip. */
export function matchesState(d: DebugDevice, f: StateFilter): boolean {
  switch (f) {
    case 'ready':
      return d.ready && !d.claimed_by;
    case 'in-use':
      return !!d.claimed_by;
    case 'not-ready':
      return !d.ready;
    default:
      return true;
  }
}

/** Whether `d` matches the search box: its title, host, model, name, label
 *  or serial contains every word typed (case-insensitive). */
export function matchesQuery(d: DebugDevice, query: string): boolean {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return true;
  const hay = [d.title, d.host, d.model, d.name, d.label, d.serial]
    .filter((x): x is string => !!x)
    .join(' ')
    .toLowerCase();
  return words.every((w) => hay.includes(w));
}

export function filterDevices(
  devices: DebugDevice[],
  f: { state: StateFilter; platform: PlatformFilter; query: string },
): DebugDevice[] {
  return devices.filter(
    (d) =>
      matchesState(d, f.state) &&
      (f.platform === 'any' || d.platform === f.platform) &&
      matchesQuery(d, f.query),
  );
}

export interface HostGroup {
  host: string;
  devices: DebugDevice[];
  /** The newest `last_seen_at` of its devices: when a scan last found one. */
  lastSeen: number;
}

/** Devices by host, hosts by name and each host's devices by title (ready
 *  ones first), so a card keeps its place across refreshes. */
export function groupByHost(devices: DebugDevice[]): HostGroup[] {
  const by = new Map<string, DebugDevice[]>();
  for (const d of devices) by.set(d.host, [...(by.get(d.host) ?? []), d]);
  return [...by.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([host, ds]) => ({
      host,
      devices: [...ds].sort(
        (a, b) => Number(b.ready) - Number(a.ready) || a.title.localeCompare(b.title) || a.id - b.id,
      ),
      lastSeen: Math.max(...ds.map((d) => d.last_seen_at)),
    }));
}

/** How many wait in `d`'s line. */
export const waitingOf = (d: DebugDevice): number => d.waiting ?? d.queue?.length ?? 0;

/** The header's sentence: "5 devices · 2 in use · 1 waiting". */
export function countLine(devices: DebugDevice[]): string {
  const n = devices.length;
  const inUse = devices.filter((d) => !!d.claimed_by).length;
  const waiting = devices.reduce((s, d) => s + waitingOf(d), 0);
  const parts = [`${n} ${n === 1 ? 'device' : 'devices'}`];
  if (inUse) parts.push(`${inUse} in use`);
  if (waiting) parts.push(`${waiting} waiting`);
  return parts.join(' · ');
}

/** "In use by session #42 on mercury · login flow · 12 min left", or "Free". */
export function claimLine(d: DebugDevice, nowSecs: number): string {
  if (!d.claimed_by) return 'Free';
  return [`In use by ${holderName(d.claimed_by)}`, d.claim_note, claimLeft(d, nowSecs)]
    .filter((x): x is string => !!x)
    .join(' · ');
}

/** The note a mirror claims its device with (`device_mirror.rs`). */
export const MIRROR_NOTE = 'scrcpy mirror';

export type CardAction =
  | 'claim'
  | 'queue'
  | 'leave'
  | 'release'
  | 'screenshot'
  | 'mirror'
  | 'stop-mirror'
  | 'logs'
  | 'boot'
  | 'shutdown';

/**
 * The actions a card offers, in the order it draws them. `mirroring` is
 * whether this board started a mirror on it; a desktop claim with the
 * mirror's note says so too, after a reload.
 *
 * - Claim a free, ready device; Wait in line for one someone else holds.
 * - Leave line from inside it. Release a held device otherwise: from inside
 *   the line `release` only leaves it (`docs/debug-devices.md`, Queue).
 * - Screenshot, logs and mirror need it ready; mirror is Android only.
 * - Start a stopped emulator or simulator, Stop a running one.
 */
export function cardActions(d: DebugDevice, mirroring = false): CardAction[] {
  const out: CardAction[] = [];
  const held = !!d.claimed_by;
  const mine = d.claimed_by === DESKTOP_HOLDER;
  const inLine = d.queue_place !== undefined && d.queue_place !== null;
  if (!held && d.ready) out.push('claim');
  if (held && !mine && !inLine) out.push('queue');
  if (inLine) out.push('leave');
  else if (held) out.push('release');
  const mirrored = mirroring || (mine && d.claim_note === MIRROR_NOTE);
  if (mirrored) out.push('stop-mirror');
  else if (canMirror(d)) out.push('mirror');
  if (d.ready) out.push('logs');
  if (isVirtual(d) && !d.ready && d.state !== 'booting') out.push('boot');
  if (isVirtual(d) && d.ready) out.push('shutdown');
  return out;
}

/** A screenshot as an `<img src>`. */
export const screenshotSrc = (s: { mime: string; data: string }): string => `data:${s.mime};base64,${s.data}`;

/** The chip tone a device's state reads in: ready green, starting busy,
 *  stopped quiet, anything else (offline, not trusted, gone) red. */
export function stateTone(d: DebugDevice): 'done' | 'working' | 'idle' | 'failed' {
  if (d.ready) return 'done';
  if (d.state === 'booting') return 'working';
  if (d.state === 'shutdown') return 'idle';
  return 'failed';
}
