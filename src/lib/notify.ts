// Operator notifications for stuck transitions (PROD-3).
//
// No Tauri notification plugin is in the manifest, so OS-level alerts go
// through the Web Notification API, which WKWebView / WebKitGTK expose behind
// the usual permission prompt. The prompt can only be triggered from a user
// gesture, so Settings owns the "enable" button; this module only reads the
// prefs and fires. Every call is guarded — a webview without `Notification`
// simply gets the in-app toast + aria-live announcement.
import { writable } from 'svelte/store';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { readPref, writePref } from './prefs';
import { attentionState, classify, type AttentionOptions, type AttentionState } from './attention';
import type { SessionRow } from './sessions';

const isBool = (v: unknown): v is boolean => typeof v === 'boolean';
const isNumber = (v: unknown): v is number => typeof v === 'number' && Number.isFinite(v);

/** In-app toast + aria-live announcement on a stuck transition. */
export const notifyStuckToast = writable<boolean>(readPref('notify.stuck-toast', true, isBool));
notifyStuckToast.subscribe((v) => writePref('notify.stuck-toast', v));

/** OS notification (Web Notification API) on a stuck transition. Off by
 *  default: it needs a permission grant from Settings first. */
export const notifyStuckOs = writable<boolean>(readPref('notify.stuck-os', false, isBool));
notifyStuckOs.subscribe((v) => writePref('notify.stuck-os', v));

/** Idle threshold (minutes) for the "needs attention" filter's idle rule. */
export const attentionIdleMinutes = writable<number>(
  readPref('attention.idle-minutes', 30, isNumber),
);
attentionIdleMinutes.subscribe((v) => writePref('attention.idle-minutes', v));

export type NotificationPermissionState = 'granted' | 'denied' | 'default' | 'unsupported';

type NotificationCtor = {
  new (title: string, opts?: { body?: string; tag?: string }): unknown;
  permission: NotificationPermission;
  requestPermission: () => Promise<NotificationPermission>;
};

function notificationApi(): NotificationCtor | null {
  const g = globalThis as { Notification?: NotificationCtor };
  return typeof g.Notification === 'function' ? g.Notification : null;
}

export function notificationPermission(): NotificationPermissionState {
  const api = notificationApi();
  if (!api) return 'unsupported';
  return api.permission;
}

/** Ask the OS for permission. Must be called from a user gesture (a button
 *  in Settings). Resolves to the resulting state. */
export async function requestNotificationPermission(): Promise<NotificationPermissionState> {
  const api = notificationApi();
  if (!api) return 'unsupported';
  try {
    return await api.requestPermission();
  } catch {
    return api.permission;
  }
}

/** Bring the app's window forward: the webview's own focus, and the
 *  native window's in Tauri (a webview cannot raise its window itself). */
export function focusAppWindow(): void {
  try {
    globalThis.focus?.();
  } catch {
    /* no window */
  }
  try {
    void getCurrentWindow()
      .setFocus()
      .catch(() => {});
  } catch {
    /* outside Tauri */
  }
}

type ShownNotification = {
  onclick?: ((e: Event) => void) | null;
  onclose?: (() => void) | null;
  close?: () => void;
};

/** The notifications still up, by tag (review w11-10): what
 *  `closeOsNotification` takes down once its row no longer waits. */
const shown = new Map<string, ShownNotification>();

/** Take down the notification posted under `tag`, if it is still up: the
 *  session was answered elsewhere, so a later click would open a row that
 *  is working. */
export function closeOsNotification(tag: string): void {
  const n = shown.get(tag);
  if (!n) return;
  shown.delete(tag);
  try {
    n.close?.();
  } catch {
    /* already gone */
  }
}

/** Fire an OS notification if permitted. Returns true when one was shown.
 *  `onclick` runs when the person clicks it, after the window comes forward
 *  (review w11-4: a click used to do nothing visible). */
export function showOsNotification(title: string, body: string, tag?: string, onclick?: () => void): boolean {
  const api = notificationApi();
  if (!api || api.permission !== 'granted') return false;
  try {
    const n = new api(title, { body, tag }) as ShownNotification | undefined;
    if (n && typeof n === 'object') {
      n.onclick = (e: Event) => {
        e?.preventDefault?.();
        focusAppWindow();
        if (tag) closeOsNotification(tag);
        onclick?.();
      };
      if (tag) {
        shown.set(tag, n);
        n.onclose = () => {
          if (shown.get(tag) === n) shown.delete(tag);
        };
      }
    }
    return true;
  } catch {
    return false;
  }
}

// ── The hub's notifications matrix (Orbit Fleet 11.9) ──
//
// `notify.desktop` / `notify.phone` / `notify.sound` each list the session
// states that reach that channel; `notify.quiet_hours` ("22:00-07:30", may
// run past midnight, "" for none) silences every channel except for the
// states in `notify.quiet_except`. The hub stores them, so the phone reads
// the same answer through `get_settings`.

export type NotifyChannel = 'desktop' | 'phone' | 'sound';
export type NotifyState = 'needs_you' | 'failed' | 'blocked' | 'done' | 'routine_failed';

const list = (v: string | undefined) => new Set((v ?? '').split(',').map((s) => s.trim()).filter(Boolean));

/** `"22:00-07:30"` → minutes of the day; null for none or a malformed one. */
export function parseQuietHours(v: string | undefined): [number, number] | null {
  const m = /^\s*(\d{1,2}):(\d{2})\s*-\s*(\d{1,2}):(\d{2})\s*$/.exec(v ?? '');
  if (!m) return null;
  const [h1, m1, h2, m2] = m.slice(1).map(Number);
  if (h1 > 23 || h2 > 23 || m1 > 59 || m2 > 59) return null;
  const from = h1 * 60 + m1;
  const until = h2 * 60 + m2;
  return from === until ? null : [from, until];
}

/** Whether `minute` (of the day) falls inside the range, which may wrap
 *  past midnight. */
export function inQuietHours(range: [number, number] | null, minute: number): boolean {
  if (!range) return false;
  const [from, until] = range;
  return from < until ? minute >= from && minute < until : minute >= from || minute < until;
}

/** Whether a notification about `state` may go out on `channel` now, by the
 *  hub's settings (`fleetSettings`) and this device's clock. */
export function notificationAllowed(
  channel: NotifyChannel,
  state: NotifyState,
  settings: Record<string, string>,
  at: Date = new Date(),
): boolean {
  if (!list(settings[`notify.${channel}`]).has(state)) return false;
  const quiet = inQuietHours(parseQuietHours(settings['notify.quiet_hours']), at.getHours() * 60 + at.getMinutes());
  return !quiet || list(settings['notify.quiet_except']).has(state);
}

/** The matrix's row for an attention state (`attention.ts`); null for the
 *  states no channel announces (working, idle, paused). The phone files its
 *  attention reasons the same way (fleet-mobile `notifyStateOf`). */
export function notifyStateOf(state: AttentionState): NotifyState | null {
  switch (state) {
    case 'action_required':
      return 'needs_you';
    case 'failed':
    case 'blocked':
    case 'done':
      return state;
    default:
      return null;
  }
}

/** Each row's attention state, for the next diff. `external` rows never
 *  announce, so they are left out. */
export function notifySnapshot(rows: readonly SessionRow[], opts: AttentionOptions): Map<number, AttentionState> {
  const m = new Map<number, AttentionState>();
  for (const s of rows) if (s.kind !== 'external') m.set(s.id, attentionState(s, opts));
  return m;
}

/** Rows that moved into a state the matrix lists since `prev`, with that
 *  state. A row new to the snapshot counts (a session that starts out
 *  needing you is news); a row that stays put does not. */
export function newlyNotifiable(
  prev: ReadonlyMap<number, AttentionState>,
  rows: readonly SessionRow[],
  opts: AttentionOptions,
  prevOwn?: ReadonlyMap<number, AttentionState>,
): { row: SessionRow; state: NotifyState }[] {
  const out: { row: SessionRow; state: NotifyState }[] = [];
  for (const s of rows) {
    // A stuck row is the stuck watcher's (Attention.svelte), which files it
    // under Blocked and announces it already.
    if (s.kind === 'external' || s.stuck_kind) continue;
    const now = attentionState(s, opts);
    if (prev.get(s.id) === now) continue;
    const state = notifyStateOf(now);
    if (!state) continue;
    // A routine's run finishing is the routine's news, read in its Runs:
    // one a day per routine would otherwise ring like a person's session.
    // Its failures and questions still announce.
    if (state === 'done' && s.origin === 'routine') continue;
    // Review w11-3: a host coming back (or a limit resetting) uncovers the
    // Failed or Done the row already was. That is news only when the row's
    // own state moved, apart from the fleet's facts.
    if ((state === 'failed' || state === 'done') && prevOwn && opts.facts) {
      if (prevOwn.get(s.id) === attentionState(s, { ...opts, facts: undefined })) continue;
    }
    out.push({ row: s, state });
  }
  return out;
}

/** `notifySnapshot` without the fleet's facts: each row's own state, which a
 *  host going down or an account limit does not move (review w11-3). */
export function notifyOwnSnapshot(rows: readonly SessionRow[], opts: AttentionOptions): Map<number, AttentionState> {
  return notifySnapshot(rows, { ...opts, facts: undefined });
}

/** What blocks a row that is not the row's own doing: a host or a login. */
export interface BlockCause {
  /** Stable per cause; the notification's tag is `blocked-${key}`. */
  key: string;
  kind: 'host_down' | 'account_limit' | 'no_credentials';
  /** The host alias, or the account's uuid ('' when the row names none). */
  subject: string;
}

/** The fact that blocks `row`, if it is blocked by one (review w11-2): a
 *  down host, an account at its limit, a signed-out login. One cause holds
 *  every session on that host or login, so it is announced once. */
export function blockCause(row: SessionRow, opts: AttentionOptions): BlockCause | null {
  const bucket = classify(row, opts);
  if (bucket === 'host_down') return { key: `host-${row.host_alias}`, kind: bucket, subject: row.host_alias };
  if (bucket === 'account_limit') {
    const subject = row.account_uuid ?? '';
    return { key: `limit-${subject}`, kind: bucket, subject };
  }
  if (bucket === 'no_credentials') {
    const subject = row.account_uuid ?? '';
    return { key: `login-${subject || row.host_alias}`, kind: bucket, subject };
  }
  return null;
}

/** Every cause blocking some row of `rows` now, by key, with its rows. */
export function blockCauses(
  rows: readonly SessionRow[],
  opts: AttentionOptions,
): Map<string, { cause: BlockCause; rows: SessionRow[] }> {
  const out = new Map<string, { cause: BlockCause; rows: SessionRow[] }>();
  if (!opts.facts) return out;
  for (const s of rows) {
    if (s.kind === 'external' || s.stuck_kind) continue;
    const cause = blockCause(s, opts);
    if (!cause) continue;
    const g = out.get(cause.key) ?? { cause, rows: [] };
    g.rows.push(s);
    out.set(cause.key, g);
  }
  return out;
}

/** Split what `newlyNotifiable` found into the rows to announce one by one
 *  and the causes to announce once each (review w11-2). A cause is listed
 *  with EVERY row it holds now, not only the new ones, so the line can say
 *  how many sessions wait on it. */
export function coalesceByCause(
  fresh: readonly { row: SessionRow; state: NotifyState }[],
  rows: readonly SessionRow[],
  opts: AttentionOptions,
): { single: { row: SessionRow; state: NotifyState }[]; causes: { cause: BlockCause; rows: SessionRow[] }[] } {
  const single: { row: SessionRow; state: NotifyState }[] = [];
  const keys = new Set<string>();
  for (const f of fresh) {
    const cause = f.state === 'blocked' ? blockCause(f.row, opts) : null;
    if (cause) keys.add(cause.key);
    else single.push(f);
  }
  if (keys.size === 0) return { single, causes: [] };
  const all = blockCauses(rows, opts);
  return { single, causes: [...keys].flatMap((k) => (all.has(k) ? [all.get(k)!] : [])) };
}

/** The title of a cause's one notification: "trn is down · 12 sessions". */
export function causeTitle(cause: BlockCause, count: number, accountName: (uuid: string) => string): string {
  const n = `${count} session${count === 1 ? '' : 's'}`;
  switch (cause.kind) {
    case 'host_down':
      return `${cause.subject} is down · ${n}`;
    case 'account_limit':
      return `${cause.subject ? accountName(cause.subject) : 'An account'} is at its limit · ${n}`;
    case 'no_credentials':
      return `${cause.subject ? accountName(cause.subject) : 'A login'} is signed out · ${n}`;
  }
}

/** A short two-note chime through Web Audio, for the matrix's Sound column.
 *  Guarded like the OS notification: a webview without `AudioContext` plays
 *  nothing. Returns true when it played. */
export function playChime(): boolean {
  const g = globalThis as { AudioContext?: new () => AudioContext };
  if (typeof g.AudioContext !== 'function') return false;
  try {
    const ctx = new g.AudioContext();
    const t = ctx.currentTime;
    for (const [i, hz] of [880, 1320].entries()) {
      const osc = ctx.createOscillator();
      const gain = ctx.createGain();
      osc.frequency.value = hz;
      gain.gain.setValueAtTime(0.0001, t + i * 0.12);
      gain.gain.exponentialRampToValueAtTime(0.15, t + i * 0.12 + 0.02);
      gain.gain.exponentialRampToValueAtTime(0.0001, t + i * 0.12 + 0.2);
      osc.connect(gain).connect(ctx.destination);
      osc.start(t + i * 0.12);
      osc.stop(t + i * 0.12 + 0.22);
    }
    setTimeout(() => void ctx.close(), 600);
    return true;
  } catch {
    return false;
  }
}
