// Frontend layout modes for `master_detail` pages (boards OrgOverview,
// OrgMembers, OrgSpend, OrgDevices, DebugDevices, Federation). The page spec
// stays as it is: which mode a resource takes, the columns of a table and
// the tabs of a record are derived here from the resource's id, its declared
// fields and actions, and the spec's section titles. Nothing new is said to
// the backend, so every action is still one the resource declares.
import type { Section } from './pages';
import {
  ago,
  applies,
  choiceLabel,
  fieldValue,
  rawOf,
  type ActionSpec,
  type FieldSpec,
  type ResourceRecord,
  type ResourceType,
} from './resources';

/** How a resource's records are laid out: the list beside the editor
 *  (the default), a table above it, or the Federation topology. */
export type ResourceLayout = 'list' | 'table' | 'federation';

/** A table's shape for one resource: the field shown under the title, the
 *  fields that get a column, and the record actions offered on the row. */
interface TableShape {
  /** Fields joined with " · " under the title. */
  sub: string[];
  columns: string[];
  /** Record actions shown on the row, in this order, when they apply. */
  inline: string[];
}

const TABLES: Record<string, TableShape> = {
  device: { sub: [], columns: ['person', 'org', 'last_seen_at'], inline: [] },
  debug_device: {
    sub: ['os_version', 'model'],
    columns: ['host', 'claimed_by'],
    inline: [
      'debug_device.install',
      'debug_device.logs',
      'debug_device.screenshot',
      'debug_device.release',
      'debug_device.boot',
      'debug_device.shutdown',
    ],
  },
  tracker: { sub: ['site_url'], columns: ['last_sync_at'], inline: ['tracker.test'] },
};

export function layoutOf(r: ResourceType): ResourceLayout {
  if (r.id === 'peer_link') return 'federation';
  return r.id in TABLES ? 'table' : 'list';
}

export function columnsOf(r: ResourceType): FieldSpec[] {
  const shape = TABLES[r.id];
  if (!shape) return [];
  return shape.columns.flatMap((id) => r.fields.find((f) => f.id === id) ?? []);
}

/** A scalar field in a cell: a time as how long ago, a choice by its
 *  label, nothing as "—". */
export function cellText(f: FieldSpec, record: ResourceRecord, now: number): string {
  const raw = rawOf(f, record);
  if (f.type === 'time') return raw === null || raw === undefined ? '—' : ago(raw, now);
  if (f.type === 'bool') return fieldValue(f, record) ? 'yes' : 'no';
  if (raw === null || raw === undefined || raw === '') return '—';
  if (f.type === 'choice') return choiceLabel(f, String(raw));
  return String(raw);
}

/** The line under a row's title, or `''`. */
export function subOf(r: ResourceType, record: ResourceRecord): string {
  const shape = TABLES[r.id];
  if (!shape) return '';
  return shape.sub
    .map((id) => record[id])
    .filter((v) => v !== null && v !== undefined && v !== '')
    .map(String)
    .join(' · ');
}

const RUNNING = new Set(['online', 'booted', 'booting']);

/** Whether a row action makes sense for this record now: a claim is
 *  released only while one stands, a device started only while it is down
 *  and stopped only while it runs. Anything else: when it applies. */
function fits(r: ResourceType, a: ActionSpec, record: ResourceRecord): boolean {
  if (!applies(r, a, record)) return false;
  const state = String(record.state ?? '');
  if (a.id === 'debug_device.release') return !!record.claimed_by;
  if (a.id === 'debug_device.boot') return !RUNNING.has(state);
  if (a.id === 'debug_device.shutdown') return RUNNING.has(state);
  return true;
}

/** The record actions a row offers: the shape's, in its order, when they fit. */
export function rowActions(r: ResourceType, record: ResourceRecord): ActionSpec[] {
  const shape = TABLES[r.id];
  if (!shape) return [];
  const all = r.actions ?? [];
  return shape.inline.flatMap((id) => all.find((a) => a.id === id) ?? []).filter((a) => fits(r, a, record));
}

// --- an org's tabs (boards OrgOverview, OrgMembers, OrgSpend) --------------

export interface RecordTab {
  id: string;
  label: string;
  count?: number;
  sections: Section[];
}

/** Which tab a spec section lands on, by its title; anything not named
 *  here is a setting of the org. */
function tabOfSection(title: string): string {
  const t = title.toLowerCase();
  if (t === 'overview' || t === 'what belongs to it' || t === 'needs an admin') return 'overview';
  if (t === 'members') return 'members';
  if (t === 'devices') return 'devices';
  if (t === 'spend') return 'spend';
  if (t.includes('shar')) return 'sharing';
  return 'settings';
}

const TAB_ORDER: [string, string][] = [
  ['overview', 'Overview'],
  ['members', 'Members'],
  ['devices', 'Devices'],
  ['settings', 'Settings'],
  ['spend', 'Spend'],
  ['sharing', 'Sharing'],
];

/** Field types the record may leave out (an older hub, or a caller it is
 *  not shown to): a section of only those shows nothing then. */
const OPTIONAL = new Set(['items', 'money', 'money_series', 'settings', 'sync']);

/** A section shows something for this record: a field it carries. */
function hasContent(s: Section, r: ResourceType, record: ResourceRecord): boolean {
  return s.items.some((i) => {
    if (i.type !== 'field') return false;
    const f = r.fields.find((x) => x.id === i.key);
    return !!f && !(OPTIONAL.has(f.type) && record[f.id] == null);
  });
}

/** An org's sections grouped into tabs: Overview, Members, Devices,
 *  Settings, Spend, and Sharing only when a section is about it. A tab with
 *  nothing this record carries is left out. Only for the org resource;
 *  any other gets none. */
export function recordTabs(r: ResourceType, sections: Section[], record: ResourceRecord): RecordTab[] {
  if (r.id !== 'org') return [];
  const count = (id: string) => (Array.isArray(record[id]) ? (record[id] as unknown[]).length : undefined);
  return TAB_ORDER.map(([id, label]) => ({
    id,
    label,
    count: id === 'members' || id === 'devices' ? count(id) : undefined,
    sections: sections.filter((s) => tabOfSection(s.title) === id),
  })).filter((t) => t.sections.some((s) => hasContent(s, r, record)));
}

/** An org header's line: "Organisation · owns the hub · you are an admin". */
export function orgEyebrow(record: ResourceRecord): string {
  const parts = ['Organisation'];
  if (record.owns_hub === true) parts.push('owns the hub');
  const role = record.my_role;
  if (role === 'admin') parts.push('you are an admin');
  else if (role === 'member') parts.push('you are a member');
  else if (role === 'viewer') parts.push('you are a viewer');
  return parts.join(' · ');
}

// --- Federation (board Federation) ------------------------------------------

/** A peer link is up while its last exchange went through. */
export function peerUp(record: ResourceRecord): boolean {
  return record.state === 'connected';
}

/** A peer row's line: "peer · 42 ms · 7 messages today" while up; while
 *  down, its state, since when, and the last error. */
export function peerLine(r: ResourceType, record: ResourceRecord, now: number): string {
  const state = r.fields.find((f) => f.id === 'state');
  if (!peerUp(record)) {
    const word = state ? choiceLabel(state, String(record.state ?? '')) : String(record.state ?? 'down');
    const since = typeof record.last_exchange_at === 'number' ? `last exchange ${ago(record.last_exchange_at, now)}` : '';
    const err = typeof record.last_error === 'string' && record.last_error ? record.last_error : '';
    return ['down', word.toLowerCase(), err, since].filter(Boolean).join(' · ');
  }
  const n = typeof record.messages_today === 'number' ? record.messages_today : 0;
  return ['peer', typeof record.latency === 'string' ? record.latency : '', `${n} ${n === 1 ? 'message' : 'messages'} today`]
    .filter(Boolean)
    .join(' · ');
}

export interface TopologyNode {
  id: string;
  label: string;
  x: number;
  y: number;
  up: boolean;
}

/** Where each peer sits around this hub: evenly on a circle, the first
 *  at the upper left as on the board. Coordinates are in a 320 × 200 box
 *  centred on (160, 100). */
export function topology(peers: { id: string; label: string; up: boolean }[]): TopologyNode[] {
  const n = peers.length;
  return peers.map((p, i) => {
    const a = -Math.PI * 0.8 + (2 * Math.PI * i) / Math.max(n, 1);
    return { ...p, x: Math.round(160 + 120 * Math.cos(a)), y: Math.round(100 + 72 * Math.sin(a)) };
  });
}

// --- trusting a device from a row or a need ---------------------------------

/** "Trust device": the device resource's own update with `trusted` on, and
 *  the sentence its field asks first. Null when the bundle has no device
 *  resource that can do it. */
export function trustOf(
  resources: ResourceType[],
  device: string,
): { action: ActionSpec; args: Record<string, unknown>; confirm?: string } | null {
  const r = resources.find((x) => x.id === 'device');
  const f = r?.fields.find((x) => x.id === 'trusted');
  if (!r?.update || !f?.edit || f.type !== 'bool') return null;
  const args: Record<string, unknown> = {};
  for (const [arg, bind] of r.update.bind) if (bind.from === 'record' && bind.name === r.id_field) args[arg] = device;
  args[f.edit] = f.on_off ? 'on' : true;
  return { action: r.update, args, confirm: f.confirm };
}
