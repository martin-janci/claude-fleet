// Operator notifications for stuck transitions (PROD-3).
//
// No Tauri notification plugin is in the manifest, so OS-level alerts go
// through the Web Notification API, which WKWebView / WebKitGTK expose behind
// the usual permission prompt. The prompt can only be triggered from a user
// gesture, so Settings owns the "enable" button; this module only reads the
// prefs and fires. Every call is guarded — a webview without `Notification`
// simply gets the in-app toast + aria-live announcement.
import { writable } from 'svelte/store';
import { readPref, writePref } from './prefs';

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

/** Fire an OS notification if permitted. Returns true when one was shown. */
export function showOsNotification(title: string, body: string, tag?: string): boolean {
  const api = notificationApi();
  if (!api || api.permission !== 'granted') return false;
  try {
    new api(title, { body, tag });
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
