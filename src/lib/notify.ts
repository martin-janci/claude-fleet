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
