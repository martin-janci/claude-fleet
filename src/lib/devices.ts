// The company's paired devices and people (org administration phase B):
// what Settings → Devices and Settings → People list, and the option list
// the org page's "Bind a device" picks from. Every command routes to the
// hub's `org_admin` on a paired desktop.
import { writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';

/** `list_devices`: a person's paired device (never a peer link or an
 *  updater token). */
export interface DeviceSummary {
  name: string;
  /** `answer`: answers questions and permission prompts, never types a
   *  prompt (M15 G2.10). */
  mode: 'full' | 'answer' | 'readonly' | string;
  trusted: boolean;
  org_id?: number;
  org?: string;
  person_id?: number;
  person?: string;
  created_at: number;
  last_seen_at?: number;
  catalogs: string[];
  /** The device this list was read through. */
  this_device?: boolean;
  /** M15 G7.14: `desktop` or `phone`, from its app's last header. */
  kind?: string;
  /** M15 G7.14: "phone · fleet-mobile 0.5.4". */
  app?: string;
}

/** `pair_device`: the one-time code, the URL a phone opens and the URL's QR
 *  as rows of `1` (dark) / `0`. */
export interface Pairing {
  url: string;
  code: string;
  expires_in_s: number;
  name: string;
  mode: string;
  trusted: boolean;
  org_id?: number | null;
  person?: string | null;
  qr: string[];
}

export const devices = writable<DeviceSummary[]>([]);

/** `list_people`: a person this hub knows (Settings → People). */
export interface PersonSummary {
  id: number;
  name: string;
  display_name?: string;
  owner: boolean;
  created_at: number;
  disabled_at?: number;
  devices: string[];
}

export const people = writable<PersonSummary[]>([]);

/** Re-read the people (M15 G2.10: the person pickers offer them). A failure
 *  leaves the list as it was, as for the devices. */
export async function loadPeople(): Promise<Result<PersonSummary[]>> {
  const r = await invokeCmd<PersonSummary[]>('list_people');
  if (r.ok) people.set(Array.isArray(r.value) ? r.value : []);
  return r;
}

/** Trust a paired device (M15 G7.14: from an org's member row). Its
 *  prompts then reach agents unmarked; the hub refuses a caller that may
 *  not. */
export function trustDevice(device: string): Promise<Result<DeviceSummary>> {
  return invokeCmd<DeviceSummary>('update_device', { args: { device, trusted: true } });
}

/** Re-read the devices. A failure (a readonly device the hub refuses, an
 *  older hub) leaves the list as it was: it only feeds option selects. */
export async function loadDevices(): Promise<Result<DeviceSummary[]>> {
  const r = await invokeCmd<DeviceSummary[]>('list_devices');
  if (r.ok) devices.set(Array.isArray(r.value) ? r.value : []);
  return r;
}

/** The QR's rows as SVG rects (one per dark run), for a viewBox of the
 *  QR's width plus a 4-module quiet zone on every side. */
export function qrRects(rows: string[]): { x: number; y: number; w: number }[] {
  const out: { x: number; y: number; w: number }[] = [];
  rows.forEach((row, y) => {
    let x = 0;
    while (x < row.length) {
      if (row[x] !== '1') {
        x += 1;
        continue;
      }
      const start = x;
      while (x < row.length && row[x] === '1') x += 1;
      out.push({ x: start + 4, y: y + 4, w: x - start });
    }
  });
  return out;
}
