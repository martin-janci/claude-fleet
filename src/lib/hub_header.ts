// Settings › Hub & sync's status header (M15 G7.13, SettingsHub board):
// "last sync 4 s ago · 2 more of your devices". Both come from the hub's
// own device list (`list_devices`): the row read through this device says
// when the hub last heard from it, and the person's other paired devices
// are the rows with the same person. Pure.
import type { DeviceSummary } from './devices';

export interface HubHeaderFacts {
  /** "last sync 4 s ago"; null when the hub has no time for this device. */
  lastSync: string | null;
  /** The person's other paired devices; null when this device is not in
   *  the list (an older hub, or a list this device may not read). */
  others: number | null;
}

function ago(secs: number): string {
  if (secs < 60) return `${secs} s ago`;
  if (secs < 3600) return `${Math.floor(secs / 60)} min ago`;
  if (secs < 86_400) return `${Math.floor(secs / 3600)} h ago`;
  return `${Math.floor(secs / 86_400)} d ago`;
}

export function hubHeaderFacts(devices: readonly DeviceSummary[], now: number): HubHeaderFacts {
  const me = devices.find((d) => d.this_device);
  if (!me) return { lastSync: null, others: null };
  const others = devices.filter(
    (d) => !d.this_device && (me.person_id != null ? d.person_id === me.person_id : d.person === me.person),
  ).length;
  const lastSync = me.last_seen_at != null ? `last sync ${ago(Math.max(0, now - me.last_seen_at))}` : null;
  return { lastSync, others };
}

/** "2 more of your devices", or null with none. */
export function othersText(n: number | null): string | null {
  if (!n) return null;
  return `${n} more of your device${n === 1 ? '' : 's'}`;
}
