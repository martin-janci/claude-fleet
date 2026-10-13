// Debug devices on the desktop (`docs/debug-devices.md`): the row the
// backend answers (`service::debug_devices::DebugDevice`), the commands
// that act on one (`src-tauri/src/commands/debug_devices.rs`,
// `device_mirror.rs`; routed to the hub's `debug_devices` when paired), and
// the pure helpers the Debug devices board, the chat's device cards and the
// session's device strip share.
import { invokeCmd, type Result } from './result';

/** One holder waiting in a device's line. */
export interface QueuedHolder {
  holder: string;
  note?: string;
  queued_at: number;
}

export interface DebugDevice {
  id: number;
  /** The label if a person set one, else the device's own name. */
  title: string;
  host: string;
  platform: 'android' | 'ios' | string;
  kind: 'physical' | 'emulator' | 'simulator' | string;
  key: string;
  serial?: string;
  name: string;
  model?: string;
  os_version?: string;
  /** online | booted (ready) · offline | unauthorized | shutdown | missing | booting */
  state: string;
  ready: boolean;
  label?: string;
  shared: boolean;
  claimed_by?: string;
  claim_note?: string;
  claimed_until?: number;
  queue?: QueuedHolder[];
  waiting?: number;
  /** The asker's own place in `queue` (1: next). */
  queue_place?: number;
  first_seen_at: number;
  last_seen_at: number;
}

export interface RunOutput {
  exit_code: number;
  output: string;
  truncated: boolean;
}

export interface Screenshot {
  caption: string;
  mime: string;
  /** Base64. */
  data: string;
}

export interface MirrorStarted {
  title: string;
  output: string;
  exit_code: number;
  truncated: boolean;
}

/** What the desktop's own claims are held as (`Asker::desktop`). */
export const DESKTOP_HOLDER = 'you (desktop)';

/** The fleet MCP tool a session drives devices with, as a transcript names it. */
export const DEVICE_TOOL = 'mcp__claude-fleet__debug_devices';

// ── Commands ───────────────────────────────────────────────────────────────

export const listDebugDevices = (): Promise<Result<DebugDevice[]>> =>
  invokeCmd<DebugDevice[]>('list_debug_devices');

export const scanDebugDevices = (host?: string) =>
  invokeCmd<{ hosts: unknown[] }>('scan_debug_devices', { args: host ? { host } : {} });

export const claimDebugDevice = (id: number, opts: { note?: string; queue?: boolean } = {}) =>
  invokeCmd<DebugDevice>('claim_debug_device', {
    args: { id, note: opts.note ?? null, queue: opts.queue ?? false },
  });

export const releaseDebugDevice = (id: number) =>
  invokeCmd<DebugDevice>('release_debug_device', { args: { id } });

export const debugDeviceScreenshot = (id: number) =>
  invokeCmd<Screenshot>('debug_device_screenshot', { args: { id } });

export const debugDeviceLogs = (id: number, contains?: string) =>
  invokeCmd<RunOutput>('debug_device_logs', { args: { id, contains: contains ?? null } });

export const bootDebugDevice = (id: number) =>
  invokeCmd<unknown>('boot_debug_device', { args: { id } });

export const shutdownDebugDevice = (id: number) =>
  invokeCmd<DebugDevice>('shutdown_debug_device', { args: { id } });

export const updateDebugDevice = (id: number, change: { label?: string; shared?: boolean }) =>
  invokeCmd<DebugDevice>('update_debug_device', { args: { id, ...change } });

export const forgetDebugDevice = (id: number) =>
  invokeCmd<unknown>('forget_debug_device', { args: { id } });

export const installDebugDevice = (
  id: number,
  path: string,
  opts: { host?: string; claim?: boolean; note?: string } = {},
) =>
  invokeCmd<RunOutput>('install_debug_device', {
    args: { id, path, host: opts.host ?? null, claim: opts.claim ?? true, note: opts.note ?? null },
  });

export const startDeviceMirror = (id: number) =>
  invokeCmd<MirrorStarted>('start_device_mirror', { args: { id } });

export const stopDeviceMirror = (id: number) =>
  invokeCmd<void>('stop_device_mirror', { args: { id } });

// ── Pure helpers ───────────────────────────────────────────────────────────

/** What a session's claims are held as: `host:<alias>#<fleet session id>`. */
export function sessionHolder(session: { id: number; host_alias: string }): string {
  return `host:${session.host_alias}#${session.id}`;
}

/** The devices `holder` holds, and those it waits for (with its place). */
export function devicesOf(
  devices: DebugDevice[],
  holder: string,
): { held: DebugDevice[]; waiting: { device: DebugDevice; place: number }[] } {
  const held = devices.filter((d) => d.claimed_by === holder);
  const waiting = devices.flatMap((d) => {
    const i = (d.queue ?? []).findIndex((w) => w.holder === holder);
    return i < 0 ? [] : [{ device: d, place: i + 1 }];
  });
  return { held, waiting };
}

/**
 * The device a reference names, the way `service::debug_devices::resolve`
 * reads one: `#12` is an id; otherwise a label, name, serial or key
 * (case-insensitive), optionally as `host/that`; a bare number is an id
 * when nothing else matches. `null` when none or several match.
 */
export function resolveDevice(devices: DebugDevice[], reference: string): DebugDevice | null {
  const r = reference.trim();
  const idOf = /^#?(\d+)$/.exec(r);
  const byId = idOf ? devices.find((d) => d.id === Number(idOf[1])) : undefined;
  if (byId && r.startsWith('#')) return byId;
  const slash = r.indexOf('/');
  const hostPart = slash > 0 ? r.slice(0, slash) : null;
  const host = hostPart && devices.some((d) => d.host === hostPart) ? hostPart : null;
  const what = (host ? r.slice(slash + 1) : r).toLowerCase();
  const eq = (a: string | undefined) => a !== undefined && a.toLowerCase() === what;
  const hits = devices.filter(
    (d) =>
      (host === null || d.host === host) &&
      (eq(d.label) || eq(d.name) || eq(d.serial) || eq(d.key)),
  );
  if (byId) return hits.every((h) => h.id === byId.id) ? byId : null;
  return hits.length === 1 ? hits[0] : null;
}

/** A holder in words: `host:mercury#42` → "session #42 on mercury". */
export function holderName(holder: string): string {
  if (holder === DESKTOP_HOLDER) return 'you';
  if (holder.startsWith('host:')) {
    const [host, session] = holder.slice(5).split('#');
    return session ? `session #${session} on ${host}` : `a session on ${host}`;
  }
  if (holder.startsWith('client:')) return holder.slice(7);
  return holder;
}

/** A place in line in words: 1 → "next", 2 → "2nd in line". */
export function placeName(place: number): string {
  if (place <= 1) return 'next';
  const tens = place % 100;
  const suffix =
    tens >= 11 && tens <= 13 ? 'th' : place % 10 === 2 ? 'nd' : place % 10 === 3 ? 'rd' : place % 10 === 1 ? 'st' : 'th';
  return `${place}${suffix} in line`;
}

/** The state in a word or two. */
export function stateLabel(d: DebugDevice): string {
  switch (d.state) {
    case 'online':
    case 'booted':
      return 'Online';
    case 'shutdown':
      return 'Shut down';
    case 'unauthorized':
      return 'Not trusted';
    case 'missing':
      return 'Gone';
    case 'offline':
      return 'Offline';
    case 'booting':
      return 'Starting';
    default:
      return d.state ? d.state[0].toUpperCase() + d.state.slice(1) : 'No state';
  }
}

/** What a not-ready state asks of the person, if anything. */
export function stateHint(d: DebugDevice): string | null {
  switch (d.state) {
    case 'unauthorized':
      return 'Accept the USB debugging prompt on the phone, then rescan.';
    case 'shutdown':
      return 'Start it first.';
    case 'missing':
      return 'Not seen on the last scan: plug it in or start it, then rescan.';
    case 'offline':
      return 'adb sees it offline: reconnect it.';
    default:
      return null;
  }
}

/** `android · emulator · Android 16 · Pixel 9`. */
export function deviceSubtitle(d: DebugDevice): string {
  const os = d.os_version
    ? `${d.platform === 'ios' ? 'iOS' : d.platform === 'android' ? 'Android' : d.platform} ${d.os_version}`
    : null;
  return [d.kind, os, d.model].filter((x): x is string => !!x).join(' · ');
}

/** "12 min left", "under a minute left"; null without a claim. */
export function claimLeft(d: DebugDevice, nowSecs: number): string | null {
  if (d.claimed_until === undefined) return null;
  const s = d.claimed_until - nowSecs;
  if (s <= 0) return 'lapsing';
  if (s < 60) return 'under a minute left';
  const m = Math.round(s / 60);
  if (m < 120) return `${m} min left`;
  return `${Math.round(m / 60)} h left`;
}

/** Whether scrcpy can mirror it (Android only). */
export const canMirror = (d: DebugDevice): boolean => d.platform === 'android' && d.ready;

/** Emulators and simulators start and stop; phones do not. */
export const isVirtual = (d: DebugDevice): boolean => d.kind === 'emulator' || d.kind === 'simulator';

// ── A session's `debug_devices` call, as the chat shows it ─────────────────

/** What one `debug_devices` call's input says. */
export interface DeviceCall {
  action: string;
  device: string | null;
  host: string | null;
  name: string | null;
  args: string[] | null;
  path: string | null;
  note: string | null;
  queue: boolean;
}

/** Read a call's (pretty JSON) input; null when it is not an object. */
export function parseDeviceCall(input: string): DeviceCall | null {
  let v: unknown;
  try {
    v = JSON.parse(input);
  } catch {
    return null;
  }
  if (!v || typeof v !== 'object' || Array.isArray(v)) return null;
  const o = v as Record<string, unknown>;
  const str = (k: string) => (typeof o[k] === 'string' ? (o[k] as string) : null);
  return {
    action: str('action') ?? '',
    device: str('device'),
    host: str('host'),
    name: str('name'),
    args: Array.isArray(o.args) ? o.args.filter((a): a is string => typeof a === 'string') : null,
    path: str('path'),
    note: str('note'),
    queue: o.queue === true,
  };
}

/** A `debug_devices` result, by what it holds. */
export type DeviceResult =
  | { kind: 'device'; device: DebugDevice }
  | { kind: 'list'; devices: DebugDevice[] }
  | { kind: 'output'; output: RunOutput }
  | { kind: 'text'; text: string };

/** Read a call's result text into the shape it holds. */
export function parseDeviceResult(text: string): DeviceResult {
  let v: unknown;
  try {
    v = JSON.parse(text);
  } catch {
    return { kind: 'text', text };
  }
  const isDevice = (x: unknown): x is DebugDevice =>
    !!x && typeof x === 'object' && typeof (x as DebugDevice).id === 'number' && typeof (x as DebugDevice).state === 'string';
  if (isDevice(v)) return { kind: 'device', device: v };
  if (v && typeof v === 'object' && Array.isArray((v as { devices?: unknown }).devices)) {
    return { kind: 'list', devices: (v as { devices: unknown[] }).devices.filter(isDevice) };
  }
  if (v && typeof v === 'object' && typeof (v as RunOutput).output === 'string') {
    const o = v as RunOutput;
    return { kind: 'output', output: { exit_code: o.exit_code ?? 0, output: o.output, truncated: !!o.truncated } };
  }
  return { kind: 'text', text };
}

/** The compact row's words for a call: `screenshot · bench pixel`. */
export function deviceCallLine(call: DeviceCall): string {
  const what = call.device ?? (call.host && call.name ? `${call.host}/${call.name}` : call.host);
  const act = call.action === 'claim' && call.queue ? 'wait in line' : call.action || 'call';
  return what ? `${act} · ${what}` : act;
}
