// Extra fleets: further hubs this window shows beside the one it resolved to
// at launch (multi-hub design, docs/superpowers/specs/2026-10-17-desktop-multi-hub-design.md).
//
// The primary fleet's rows stay in the stores they always lived in
// (sessions.ts, hosts.ts, projects.ts). An extra fleet's rows live HERE, one
// map per fleet, because ids collide across hubs: both hubs have a session 1
// and a host `mac`. Its events arrive on their own channel, `fleet:event`
// (`{fleet, name, payload}`, `backend/fleets.rs` `TaggedSink`), so no row
// from another hub can ever be merged into a primary store by mistake.
//
// Every action on an extra fleet's row passes `fleet`, and the backend routes
// it to that fleet's hub with the same argument mapping the primary uses
// (`Fleets::select`, design D6). Only the commands in the backend's
// `FLEET_SCOPED` list accept it; `FLEET_SCOPED` below mirrors that list and
// `fleets.test.ts` holds the two together.
import { derived, get, writable, type Readable } from 'svelte/store';
import { listen } from '@tauri-apps/api/event';
import { invokeCmd, type IpcError, type Result } from './result';
import type { HubConnection } from './hub_connection';
import type { SessionRow } from './sessions';
import type { HostRow } from './hosts';
import type { ProjectRow, ProjectTreeRow } from './projects';
import { readPref, writePref } from './prefs';
import { attentionState, byTriage, countsTowardBadge } from './attention';
import { attentionFactsFrom } from './attention_facts';
import { attentionIdleMinutes } from './notify';
import type { PrFilter, PrList } from './prs';

/** The slot id of the launch-time fleet (`backend::fleets::PRIMARY`). */
export const PRIMARY = 'primary';

/** The commands that take a `fleet` argument (`backend::fleets::FLEET_SCOPED`). */
export const FLEET_SCOPED = [
  'list_sessions',
  'new_session',
  'kill_session',
  'send_prompt',
  'set_session_friendly_name',
  'pin_session',
  'list_hosts',
  'list_projects',
  // Phase 2: an extra fleet's session opens in its own view.
  'session_conversation',
  'session_tool_detail',
  'capture_session',
  'touch_session_viewed',
  // Phase 2: the PRs page per fleet.
  'list_pull_requests',
  // Settings › Environment per fleet: a person's own variables on each hub,
  // and the two reads that say what this device may change there.
  'env_vars',
  'env_vars_list',
  'env_var_set',
  'env_var_unset',
  'my_grants',
  'setting_proposals',
] as const;
export type FleetScopedCommand = (typeof FLEET_SCOPED)[number];

/** One fleet, as `fleets_list` answers it. Carries no token. */
export interface FleetInfo {
  id: string;
  label: string;
  url: string | null;
  color: string;
  primary: boolean;
  /** A standalone primary: this app's own fleet. */
  local: boolean;
  client_name: string | null;
  /** The live link; null for a fleet that is not connected at all. */
  connection: HubConnection | null;
  /** Why a configured fleet is not in use. */
  unavailable: string | null;
  /** The fleet id the hub's `ready` frame named (Phase 2); null until it
   *  has, or from a hub older than the field. */
  fleet_id?: string | null;
  /** The fleet this one is the same hub as (Phase 2, by `fleet_id`): its
   *  rows are not shown and its calls are refused. */
  duplicate_of?: string | null;
}

/** One extra fleet's rows, keyed within that fleet only. */
export interface FleetRows {
  sessions: Map<number, SessionRow>;
  hosts: Map<string, HostRow>;
  projects: ProjectTreeRow[];
  /** A list call failed; shown on the fleet's header. */
  error: string | null;
}

/** The envelope of `fleet:event`. */
export interface FleetEvent {
  fleet: string;
  name: string;
  payload: unknown;
}

const fleetsStore = writable<FleetInfo[]>([]);
/** Every fleet this window shows, the primary first. */
export const fleets: Readable<FleetInfo[]> = { subscribe: fleetsStore.subscribe };

const rowsStore = writable<Map<string, FleetRows>>(new Map());
/** Each extra fleet's rows, by slot id. */
export const fleetRows: Readable<Map<string, FleetRows>> = { subscribe: rowsStore.subscribe };

/** The extra fleets only. */
export const extraFleets = derived(fleetsStore, ($f) => $f.filter((f) => !f.primary));

/** Whether this window shows more than one fleet — the switcher, badges and
 *  extra groups appear only then. */
export const multiFleet = derived(extraFleets, ($x) => $x.length > 0);

/** A key unique across fleets, for any flat collection that has to hold rows
 *  from more than one hub (`company\u0000 12`). */
export function fleetKey(fleet: string, id: number | string): string {
  return `${fleet}\u0000${id}`;
}

// ── the filter ──────────────────────────────────────────────────────────

const FILTER_PREF = 'ui.fleetFilter';
const isFilter = (v: unknown): v is string => typeof v === 'string' && v.length > 0 && v.length <= 32;

/** `all`, or one fleet's slot id. A per-viewer convenience, kept in prefs. */
export const fleetFilter = writable<string>(readPref<string>(FILTER_PREF, 'all', isFilter));
fleetFilter.subscribe((v) => writePref(FILTER_PREF, v));

/** Whether the filter shows fleet `id`. A filter naming a fleet that is gone
 *  shows everything rather than nothing. */
export function filterShows(filter: string, id: string, known: FleetInfo[]): boolean {
  if (filter === 'all' || !known.some((f) => f.id === filter)) return true;
  return filter === id;
}

/** Whether the primary's own lists are shown under the current filter. */
export const showPrimary = derived([fleetFilter, fleetsStore], ([$filter, $fleets]) =>
  filterShows($filter, PRIMARY, $fleets),
);

/** The extra fleets the filter shows, with their rows. */
export const visibleExtraFleets = derived(
  [fleetFilter, fleetsStore, rowsStore],
  ([$filter, $fleets, $rows]) =>
    $fleets
      .filter((f) => !f.primary && filterShows($filter, f.id, $fleets))
      // A duplicate's rows are the other fleet's twice over: never shown.
      .map((f) => ({ fleet: f, rows: f.duplicate_of ? emptyRows() : ($rows.get(f.id) ?? emptyRows()) })),
);

// ── applying events ─────────────────────────────────────────────────────

export function emptyRows(): FleetRows {
  return { sessions: new Map(), hosts: new Map(), projects: [], error: null };
}

function isObj(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null;
}

// ── the guards the primary's session store has (`sessions.ts`) ──────────
//
// An extra fleet's rows race the same way the primary's do: a command's
// returned snapshot can land after a newer `session:updated`, a
// `session:updated` already queued can land after the kill ("ghost
// session"), and a list answer can land after events that are newer than
// it. So, per fleet: a known older `row_version` of the same session is
// dropped, a killed id keeps a short tombstone, and a list merges with what
// changed while it was out instead of replacing it.

/** How long a killed session's id refuses a late `session:updated`. */
export const FLEET_TOMBSTONE_MS = 5000;

/** The same rule as `sessions.ts::sessionIsStale`: only a KNOWN older
 *  version of the SAME session (ids are reused after a kill) is stale. */
function fleetSessionIdentity(s: SessionRow): string {
  return `${s.host_alias}\u0000${s.tmux_name}\u0000${s.created_at}`;
}
function fleetSessionIsStale(incoming: SessionRow, current: SessionRow): boolean {
  return (
    fleetSessionIdentity(incoming) === fleetSessionIdentity(current) &&
    incoming.row_version !== undefined &&
    current.row_version !== undefined &&
    incoming.row_version < current.row_version
  );
}

/** Per fleet, per killed id: until when, and which session it was (null when
 *  the kill named only the id of a row this window never held). */
const tombstones = new Map<string, Map<number, { until: number; identity: string | null }>>();

/** What changed while one list call was out, so its answer does not undo it. */
interface Touched {
  sessions: Set<number>;
  hosts: Set<string>;
  projects: Set<number>;
}
const listsOut = new Map<string, Set<Touched>>();

function touch(fleet: string | undefined, part: keyof Touched, id: number | string): void {
  if (fleet === undefined) return;
  for (const t of listsOut.get(fleet) ?? []) (t[part] as Set<number | string>).add(id);
}

function liveTombstone(fleet: string | undefined, id: number): { identity: string | null } | null {
  if (fleet === undefined) return null;
  const t = tombstones.get(fleet)?.get(id);
  if (!t) return null;
  if (t.until > Date.now()) return t;
  tombstones.get(fleet)?.delete(id);
  return null;
}

/** Whether `row` may be written over what `rows` holds for its id. `how`:
 *  `updated` (an event about a row we may have killed), `created` or
 *  `command` (a row that is new, or the answer to the person's own request:
 *  passes a tombstone unless it is the very session that was killed). */
function admitSession(
  fleet: string | undefined,
  rows: FleetRows,
  row: SessionRow,
  how: 'updated' | 'created' | 'command',
): boolean {
  const tomb = liveTombstone(fleet, row.id);
  if (tomb) {
    if (how === 'updated') return false;
    if (tomb.identity === null || tomb.identity === fleetSessionIdentity(row)) return false;
    tombstones.get(fleet!)?.delete(row.id);
  }
  const current = rows.sessions.get(row.id);
  return !(current && fleetSessionIsStale(row, current));
}

function putSession(fleet: string | undefined, rows: FleetRows, row: SessionRow, how: 'updated' | 'created' | 'command'): void {
  if (!admitSession(fleet, rows, row, how)) return;
  rows.sessions.set(row.id, row);
  touch(fleet, 'sessions', row.id);
}

function dropSession(fleet: string | undefined, rows: FleetRows, id: number): void {
  const current = rows.sessions.get(id);
  rows.sessions.delete(id);
  touch(fleet, 'sessions', id);
  if (fleet === undefined) return;
  let m = tombstones.get(fleet);
  if (!m) tombstones.set(fleet, (m = new Map()));
  m.set(id, { until: Date.now() + FLEET_TOMBSTONE_MS, identity: current ? fleetSessionIdentity(current) : null });
}

/** The event names that change an extra fleet's rows, and which of its
 *  collections each one writes. */
const ROW_EVENTS = new Map<string, keyof Touched>([
  ['session:created', 'sessions'],
  ['session:updated', 'sessions'],
  ['session:killed', 'sessions'],
  ['host:added', 'hosts'],
  ['host:probed', 'hosts'],
  ['host:pinged', 'hosts'],
  ['host:removed', 'hosts'],
  ['project:updated', 'projects'],
]);

/** Apply one `fleet:event` to that fleet's rows, in place. Returns what the
 *  caller has to do beyond that: `relist` after a gap the bridge could not
 *  replay, `connection` when the link moved. Unknown names are ignored —
 *  this fleet's view keeps sessions, hosts and projects only (Phase 1).
 *  With `fleet`, that fleet's stale-row and tombstone guards apply. */
export function applyFleetEvent(
  rows: FleetRows,
  e: Pick<FleetEvent, 'name' | 'payload'>,
  fleet?: string,
): 'relist' | 'connection' | null {
  const p = e.payload;
  switch (e.name) {
    case 'session:created':
    case 'session:updated':
      if (isObj(p) && typeof p.id === 'number')
        putSession(fleet, rows, p as unknown as SessionRow, e.name === 'session:created' ? 'created' : 'updated');
      return null;
    case 'session:killed':
      if (isObj(p) && typeof p.id === 'number') dropSession(fleet, rows, p.id);
      return null;
    case 'host:added':
    case 'host:probed':
      if (isObj(p) && typeof p.alias === 'string') {
        rows.hosts.set(p.alias, p as unknown as HostRow);
        touch(fleet, 'hosts', p.alias);
      }
      return null;
    case 'host:pinged':
      if (isObj(p) && typeof p.alias === 'string') {
        const old = rows.hosts.get(p.alias);
        if (old) {
          rows.hosts.set(p.alias, { ...old, ...(p as Partial<HostRow>) });
          touch(fleet, 'hosts', p.alias);
        }
      }
      return null;
    case 'host:removed':
      if (isObj(p) && typeof p.alias === 'string') {
        rows.hosts.delete(p.alias);
        touch(fleet, 'hosts', p.alias);
      }
      return null;
    case 'project:updated':
      if (isObj(p) && typeof p.id === 'number') {
        const row = p as unknown as ProjectRow;
        const at = rows.projects.findIndex((t) => t.project.id === row.id);
        if (at >= 0) rows.projects[at] = { ...rows.projects[at], project: row };
        else rows.projects.push({ project: row, worktrees: [] });
        touch(fleet, 'projects', row.id);
      }
      return null;
    case 'hub:resynced':
      return 'relist';
    case 'hub:connection':
      return 'connection';
    default:
      return null;
  }
}

/** Change one fleet's rows. A fresh `FleetRows` so `$fleetRows` subscribers
 *  see a change; only the collections named in `parts` are copied (and only
 *  those may be mutated by `fn`), so a resync's burst of session events does
 *  not copy the hosts and projects once per event. */
function patchRows(fleet: string, fn: (rows: FleetRows) => void, parts: readonly (keyof Touched)[] = ['sessions', 'hosts', 'projects']): void {
  rowsStore.update((m) => {
    const next = new Map(m);
    const rows = next.get(fleet) ?? emptyRows();
    const copy: FleetRows = {
      sessions: parts.includes('sessions') ? new Map(rows.sessions) : rows.sessions,
      hosts: parts.includes('hosts') ? new Map(rows.hosts) : rows.hosts,
      projects: parts.includes('projects') ? [...rows.projects] : rows.projects,
      error: rows.error,
    };
    fn(copy);
    next.set(fleet, copy);
    return next;
  });
}

function setConnection(fleet: string, c: HubConnection): void {
  fleetsStore.update((list) => list.map((f) => (f.id === fleet ? { ...f, connection: c } : f)));
}

/** Handle one `fleet:event` (exported for the tests). */
export function onFleetEvent(e: FleetEvent): void {
  if (!e || typeof e.fleet !== 'string' || e.fleet === PRIMARY) return;
  const fleet = e.fleet;
  const part = ROW_EVENTS.get(e.name);
  if (part) {
    patchRows(fleet, (rows) => void applyFleetEvent(rows, e, fleet), [part]);
  } else if (e.name === 'hub:connection' && isObj(e.payload)) {
    setConnection(fleet, e.payload as unknown as HubConnection);
    // The ready frame that connected it also named its fleet id, which
    // only the list carries (and with it whether it is a duplicate). The
    // rows are not re-listed here: a connection the hub could not resume
    // resyncs and ends with `hub:resynced`, which re-lists; one it resumed
    // replays what was missed.
    if ((e.payload as { state?: string }).state === 'connected') void loadFleets();
  } else if (e.name === 'hub:resynced') {
    void loadFleetRows(fleet);
  }
  // Anything else (tasks, accounts, `work:changed`) is not kept here.
}

/** The primary's own link moved. When it comes up its `ready` frame names
 *  the primary's fleet id, which decides whether an extra fleet is the same
 *  hub twice (`duplicate_of`): an extra fleet that connected first was
 *  listed before that was known, so the list is read again (exported for
 *  the tests). */
export function onPrimaryConnection(c: unknown): void {
  if (isObj(c) && c.state === 'connected') void loadFleets();
}

// ── loading ─────────────────────────────────────────────────────────────

/** Re-read the list of fleets. */
export async function loadFleets(): Promise<Result<FleetInfo[]>> {
  const r = await invokeCmd<FleetInfo[]>('fleets_list');
  // A shape this build does not expect (an older backend, a test double)
  // keeps what we had rather than breaking every derived store.
  if (r.ok && !Array.isArray(r.value)) return { ok: false, error: { code: 'E_PARSE', message: 'fleets_list answered no list' } };
  if (r.ok) {
    fleetsStore.set(r.value);
    // Drop the rows of a fleet that is gone.
    const ids = new Set(r.value.map((f) => f.id));
    rowsStore.update((m) => new Map([...m].filter(([id]) => ids.has(id))));
  }
  return r;
}

/** List one extra fleet's sessions, hosts and projects from its hub. */
export async function loadFleetRows(fleet: string): Promise<void> {
  if (fleet === PRIMARY) return;
  // What changes while the lists are out wins over their answer: an event
  // or a command result applied meanwhile is newer than the list.
  const touched: Touched = { sessions: new Set(), hosts: new Set(), projects: new Set() };
  let out = listsOut.get(fleet);
  if (!out) listsOut.set(fleet, (out = new Set()));
  out.add(touched);
  let s: Result<SessionRow[]>, h: Result<HostRow[]>, p: Result<ProjectTreeRow[]>;
  try {
    [s, h, p] = await Promise.all([
      invokeCmd<SessionRow[]>('list_sessions', { fleet }),
      invokeCmd<HostRow[]>('list_hosts', { fleet }),
      invokeCmd<ProjectTreeRow[]>('list_projects', { fleet }),
    ]);
  } finally {
    out.delete(touched);
    if (out.size === 0) listsOut.delete(fleet);
  }
  patchRows(fleet, (rows) => {
    if (s.ok && Array.isArray(s.value)) {
      const next = new Map<number, SessionRow>();
      for (const r of s.value) {
        if (touched.sessions.has(r.id)) continue;
        if (!admitSession(fleet, rows, r, 'command')) {
          // Killed (tombstoned), or older than the row held: keep what is held.
          const held = rows.sessions.get(r.id);
          if (held) next.set(r.id, held);
          continue;
        }
        next.set(r.id, r);
      }
      for (const id of touched.sessions) {
        const held = rows.sessions.get(id);
        if (held) next.set(id, held);
      }
      rows.sessions = next;
    }
    if (h.ok && Array.isArray(h.value)) {
      const next = new Map(h.value.filter((r) => !touched.hosts.has(r.alias)).map((r) => [r.alias, r]));
      for (const alias of touched.hosts) {
        const held = rows.hosts.get(alias);
        if (held) next.set(alias, held);
      }
      rows.hosts = next;
    }
    if (p.ok && Array.isArray(p.value)) {
      const next = p.value.filter((t) => !touched.projects.has(t.project.id));
      for (const id of touched.projects) {
        const held = rows.projects.find((t) => t.project.id === id);
        if (held) next.push(held);
      }
      rows.projects = next;
    }
    const failed = [s, h, p].find((r) => !r.ok) as { ok: false; error: IpcError } | undefined;
    rows.error = failed ? failed.error.message : null;
  });
}

let started = false;

/** Start following every extra fleet: listen first, then list, so an event
 *  landing between the two is applied on top of the answer. */
export async function startFleets(): Promise<void> {
  if (started) return;
  started = true;
  await listen<FleetEvent>('fleet:event', (e) => onFleetEvent(e.payload));
  await listen<unknown>('hub:connection', (e) => onPrimaryConnection(e.payload));
  const r = await loadFleets();
  if (!r.ok) return;
  for (const f of r.value) if (!f.primary && !f.unavailable) void loadFleetRows(f.id);
}

// ── managing ────────────────────────────────────────────────────────────

export interface FleetAdd {
  url: string;
  code: string;
  label?: string;
  color?: string;
  allow_plaintext?: boolean;
}

/** Pair with another hub and start showing it (Settings → Hub & sync). */
export async function addFleet(args: FleetAdd): Promise<Result<FleetInfo>> {
  const r = await invokeCmd<FleetInfo>('fleet_add', { args });
  if (r.ok) {
    await loadFleets();
    void loadFleetRows(r.value.id);
  }
  return r;
}

/** Stop showing an extra fleet and forget its token here. Revokes nothing. */
export async function removeFleet(id: string): Promise<Result<null>> {
  const r = await invokeCmd<null>('fleet_remove', { args: { id } });
  if (r.ok) {
    if (get(fleetFilter) === id) fleetFilter.set('all');
    await loadFleets();
  }
  return r;
}

/** An extra fleet's Retry. */
export async function retryFleet(id: string): Promise<Result<null>> {
  const r = await invokeCmd<null>('fleet_retry_now', { args: { id } });
  if (r.ok) await loadFleets();
  return r;
}

// ── acting on an extra fleet's rows ─────────────────────────────────────

/** Apply a row a command returned, under the same guards as an event: it
 *  never overwrites a newer version of that session, and never brings back
 *  the session a kill just removed (`sessions.ts::acceptCommandRow`). */
function acceptFleetRow(fleet: string, row: unknown): void {
  if (!isObj(row) || typeof row.id !== 'number') return;
  patchRows(fleet, (rows) => putSession(fleet, rows, row as unknown as SessionRow, 'command'), ['sessions']);
}

/** Call a fleet-scoped command on `fleet`'s hub. */
export function fleetCmd<T>(cmd: FleetScopedCommand, fleet: string, args: Record<string, unknown> = {}): Promise<Result<T>> {
  return invokeCmd<T>(cmd, { ...args, fleet });
}

export function fleetSendPrompt(fleet: string, s: SessionRow, prompt: string): Promise<Result<null>> {
  return fleetCmd('send_prompt', fleet, {
    args: { host_alias: s.host_alias, tmux_name: s.tmux_name, prompt, submit: true },
  });
}

/** Press one key in an extra fleet's session instead of typing: answer or
 *  dismiss a dialog, interrupt a turn. */
export function fleetSendKey(fleet: string, s: SessionRow, key: 'Enter' | 'Escape' | 'C-c'): Promise<Result<null>> {
  return fleetCmd('send_prompt', fleet, {
    args: { host_alias: s.host_alias, tmux_name: s.tmux_name, prompt: '', submit: false, keys: key },
  });
}

export async function fleetKillSession(fleet: string, s: SessionRow): Promise<Result<number>> {
  const r = await fleetCmd<number>('kill_session', fleet, { args: { host_alias: s.host_alias, name: s.tmux_name } });
  if (r.ok) patchRows(fleet, (rows) => dropSession(fleet, rows, s.id), ['sessions']);
  return r;
}

export async function fleetRenameSession(fleet: string, s: SessionRow, name: string): Promise<Result<SessionRow>> {
  const r = await fleetCmd<SessionRow>('set_session_friendly_name', fleet, {
    args: { host_alias: s.host_alias, tmux_name: s.tmux_name, friendly_name: name },
  });
  if (r.ok) acceptFleetRow(fleet, r.value);
  return r;
}

export async function fleetPinSession(fleet: string, s: SessionRow, pinned: boolean): Promise<Result<SessionRow>> {
  const r = await fleetCmd<SessionRow>('pin_session', fleet, { args: { session_id: s.id, pinned } });
  if (r.ok) acceptFleetRow(fleet, r.value);
  return r;
}

export interface FleetNewSession {
  host_alias: string;
  project_id: number;
  /** Phase 2: `work` (the default) or `shell`. */
  kind?: 'work' | 'shell';
  /** The tmux name. */
  name: string;
  friendly_name?: string | null;
  /** A branch for a fresh worktree; null works in the project's checkout. */
  new_worktree?: string | null;
}

export async function fleetNewSession(fleet: string, a: FleetNewSession): Promise<Result<SessionRow>> {
  const r = await fleetCmd<SessionRow>('new_session', fleet, {
    args: {
      host_alias: a.host_alias,
      project_id: a.project_id,
      worktree_id: null,
      name: a.name,
      new_worktree: a.new_worktree ?? null,
      base_branch: null,
      kind: a.kind ?? 'work',
      start_command: null,
      friendly_name: a.friendly_name ?? null,
    },
  });
  if (r.ok) acceptFleetRow(fleet, r.value);
  return r;
}

// ── an extra fleet's session, opened (Phase 2) ──────────────────────────

/** The extra fleet's session on screen, by its fleet and its id there. Null
 *  while the primary's own selection (`selectedSession`) is what is shown.
 *  Kept apart from that selection on purpose: every primary module reads it
 *  as an id in the primary's space, where this id means another session. */
export const fleetSelection = writable<{ fleet: string; id: number } | null>(null);

/** The opened session's fleet and live row, or null when none is open or it
 *  has gone (killed, its fleet removed). */
export const selectedFleetSession = derived([fleetSelection, fleetsStore, rowsStore], ([$sel, $fleets, $rows]) => {
  if (!$sel) return null;
  const fleet = $fleets.find((f) => f.id === $sel.fleet && !f.primary);
  const row = $rows.get($sel.fleet)?.sessions.get($sel.id);
  return fleet && row ? { fleet, row } : null;
});

/** Open an extra fleet's session in the main view. */
export function openFleetSession(fleet: string, id: number): void {
  if (fleet === PRIMARY) return;
  fleetSelection.set({ fleet, id });
}

/** Go back to the primary's own selection. */
export function closeFleetSession(): void {
  fleetSelection.set(null);
}

/** Its finished turns read as seen, on its own hub. */
export async function fleetTouchViewed(fleet: string, s: SessionRow): Promise<void> {
  const r = await fleetCmd<SessionRow>('touch_session_viewed', fleet, { args: { session_id: s.id } });
  if (r.ok) acceptFleetRow(fleet, r.value);
}

/** A read-only snapshot of its pane, from its own hub. */
export function fleetCaptureSession(fleet: string, s: SessionRow, scrollback = 200): Promise<Result<string>> {
  return fleetCmd<string>('capture_session', fleet, { args: { session_id: s.id, scrollback_lines: scrollback } });
}

/** The pty id an extra fleet's terminal attaches under: never one the
 *  primary's panes use (`agent`, `shell-N`, a pop-out's label). */
export function fleetPtyId(fleet: string, s: Pick<SessionRow, 'id'>): string {
  return `fleet-${fleet}-${s.id}`;
}

// ── New session: which fleet (Phase 2) ──────────────────────────────────

/** An extra fleet that can start a session in a repository: its project row
 *  for it, by the id that fleet's hub knows it under. */
export interface FleetChoice {
  fleet: FleetInfo;
  projectId: number;
}

const sameText = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();
/** Lower-case letters and digits only: `PapayaPOS` and `papayapos` agree. */
const squash = (s: string) => s.toLowerCase().replace(/[^a-z0-9]/g, '');

/** The extra fleets that have `owner/repo` among their projects and can be
 *  used now (connected or connecting, not a duplicate, not unavailable). */
export function fleetChoicesFor(
  owner: string,
  repo: string,
  known: readonly FleetInfo[],
  rows: ReadonlyMap<string, FleetRows>,
): FleetChoice[] {
  const out: FleetChoice[] = [];
  for (const f of known) {
    if (f.primary || f.unavailable || f.duplicate_of) continue;
    const p = rows.get(f.id)?.projects.find((t) => !t.project.system && sameText(t.project.owner, owner) && sameText(t.project.repo, repo));
    if (p) out.push({ fleet: f, projectId: p.project.id });
  }
  return out;
}

/** Whether a fleet reads as the one that owns `owner`'s work: its label, or
 *  a label of its hub's host name, is the owner's name (`PapayaPOS` →
 *  the fleet at `fleet.papayapos.dev`, or one labelled "papayapos"). */
export function fleetOwnsOwner(f: Pick<FleetInfo, 'label' | 'url'>, owner: string): boolean {
  const o = squash(owner);
  if (!o) return false;
  if (squash(f.label) === o) return true;
  let host = '';
  try {
    host = f.url ? new URL(f.url).hostname : '';
  } catch {
    host = '';
  }
  return host.split('.').some((part) => squash(part) === o);
}

const FLEET_BY_REPO_PREF = 'newSession.fleetByRepo';
const isRepoMap = (v: unknown): v is Record<string, string> =>
  isObj(v) && Object.values(v).every((x) => typeof x === 'string');

/** The fleet a person last started `owner/repo` on, if any. */
export function rememberedFleetFor(owner: string, repo: string): string | null {
  return readPref<Record<string, string>>(FLEET_BY_REPO_PREF, {}, isRepoMap)[`${owner}/${repo}`.toLowerCase()] ?? null;
}

/** Remember where `owner/repo` was last started (the primary forgets it). */
export function rememberFleetFor(owner: string, repo: string, fleet: string): void {
  const map = { ...readPref<Record<string, string>>(FLEET_BY_REPO_PREF, {}, isRepoMap) };
  const key = `${owner}/${repo}`.toLowerCase();
  if (fleet === PRIMARY) delete map[key];
  else map[key] = fleet;
  writePref(FLEET_BY_REPO_PREF, map);
}

/** Where New session starts `owner/repo` unless the person picks: the fleet
 *  they last started it on, else the extra fleet that owns the owner's work
 *  (design: "PapayaPOS → company"), else the primary. Only ever one of
 *  `choices` or the primary. */
export function defaultFleetFor(owner: string, choices: readonly FleetChoice[], remembered: string | null): string {
  if (remembered && (remembered === PRIMARY || choices.some((c) => c.fleet.id === remembered))) return remembered;
  return choices.find((c) => fleetOwnsOwner(c.fleet, owner))?.fleet.id ?? PRIMARY;
}

// ── attention per fleet (Phase 2) ───────────────────────────────────────

/** An extra fleet's sessions that need you, worst first, by the same table
 *  the primary's Inbox asks (`inbox.ts::inboxRows`, spelled out here so the
 *  Inbox's badge can count these without an import cycle), with that
 *  fleet's own host facts. Usage facts are the primary's alone, so none. */
export function fleetNeedsYou(rows: FleetRows, idleSecs: number, now: number): SessionRow[] {
  const opts = { idleSecs, now, facts: attentionFactsFrom([...rows.hosts.values()], {}, now) };
  return byTriage(
    [...rows.sessions.values()].filter((s) => countsTowardBadge(attentionState(s, opts))),
    opts,
  );
}

/** Per extra fleet the filter shows: its sessions that need you. Duplicates
 *  and fleets with nothing waiting are left out. */
export const fleetInbox = derived(
  [visibleExtraFleets, attentionIdleMinutes],
  ([$visible, $idle]) => {
    const now = Math.floor(Date.now() / 1000);
    return $visible
      .map(({ fleet, rows }) => ({ fleet, rows: fleetNeedsYou(rows, $idle * 60, now) }))
      .filter((g) => g.rows.length > 0);
  },
);

/** Per extra fleet, filter or not: how many of its sessions need you — the
 *  fleet switcher's chip count. */
export const fleetNeedsCount = derived(
  [fleetsStore, rowsStore, attentionIdleMinutes],
  ([$fleets, $rows, $idle]) => {
    const now = Math.floor(Date.now() / 1000);
    const out = new Map<string, number>();
    for (const f of $fleets) {
      if (f.primary || f.duplicate_of) continue;
      const rows = $rows.get(f.id);
      if (rows) out.set(f.id, fleetNeedsYou(rows, $idle * 60, now).length);
    }
    return out;
  },
);

/** How many sessions need you across the extra fleets the filter shows. */
export const fleetInboxCount = derived(fleetInbox, ($g) => $g.reduce((n, g) => n + g.rows.length, 0));

// ── the PRs page per fleet (Phase 2) ────────────────────────────────────

/** An extra fleet's pull requests, from its own hub. */
export function fleetListPullRequests(
  fleet: string,
  q: { state?: PrFilter; limit?: number } = {},
): Promise<Result<PrList>> {
  const args: { state?: PrFilter; limit?: number } = {};
  if (q.state) args.state = q.state;
  if (q.limit) args.limit = q.limit;
  return fleetCmd<PrList>('list_pull_requests', fleet, { args });
}

// ── federation addresses (Phase 2) ──────────────────────────────────────

/** A federation address (`<fleet-uuid>/session/<host>/<name>`) as this
 *  window names it: `company › mac/dev-x` when the uuid is one of the fleets
 *  it shows, with that fleet. Anything else comes back as it was, fleet null. */
export function fleetAddress(addr: string, known: readonly FleetInfo[]): { text: string; fleet: FleetInfo | null; host: string | null; name: string | null } {
  const m = /^([0-9a-fA-F-]{8,64})\/session\/([^/]+)\/(.+)$/.exec(addr.trim());
  if (!m) return { text: addr, fleet: null, host: null, name: null };
  const fleet = known.find((f) => f.fleet_id && f.fleet_id.toLowerCase() === m[1].toLowerCase()) ?? null;
  if (!fleet) return { text: addr, fleet: null, host: m[2], name: m[3] };
  return { text: `${fleet.label} › ${m[2]}/${m[3]}`, fleet, host: m[2], name: m[3] };
}

/** The extra fleet's session an address names, when that fleet is shown and
 *  has it: what a click on a federation address opens. */
export function fleetSessionAt(
  addr: string,
  known: readonly FleetInfo[],
  rows: ReadonlyMap<string, FleetRows>,
): { fleet: string; id: number } | null {
  const a = fleetAddress(addr, known);
  if (!a.fleet || a.fleet.primary) return null;
  const hit = [...(rows.get(a.fleet.id)?.sessions.values() ?? [])].find(
    (s) => s.host_alias === a.host && s.tmux_name === a.name,
  );
  return hit ? { fleet: a.fleet.id, id: hit.id } : null;
}

/** POSIX single-quoting for a command a person pastes into a shell. */
function shq(s: string): string {
  return /^[A-Za-z0-9_.@%+=:,/-]+$/.test(s) ? s : `'${s.replace(/'/g, `'\\''`)}'`;
}

/** The command that attaches to an extra fleet's session from this machine.
 *  Works where this machine's ssh knows the host alias (design: Risks). */
export function fleetAttachCommand(s: Pick<SessionRow, 'host_alias' | 'tmux_name'>): string {
  return `ssh -t ${shq(s.host_alias)} tmux attach -t ${shq(`=${s.tmux_name}`)}`;
}

/** Why this machine must not attach to an extra fleet's session, or null
 *  when it may try. `local` is the name a hub gives its OWN machine: from
 *  here it would mean this desktop, so `pty_open` would attach to this
 *  machine's tmux (a different session that may share the name, typed into
 *  blind) and the copied `ssh -t local …` reaches nothing. */
export function fleetAttachRefusal(s: Pick<SessionRow, 'host_alias'>): string | null {
  if (s.host_alias !== 'local') return null;
  return "This session runs on that hub's own machine, which it calls \"local\". From here \"local\" would mean this machine, so it cannot be attached from here: use the Conversation and Snapshot tabs, or attach on that machine.";
}

/** The hosts an extra fleet's New… form offers: the visible ones. */
export function fleetHostChoices(rows: Pick<FleetRows, 'hosts'>): HostRow[] {
  return [...rows.hosts.values()].filter((h) => !h.hidden);
}

/** The host that form starts on: the first offered one that is reachable,
 *  else the first offered one — never a hidden host the picker cannot show
 *  (NewSessionDialog's `fleetHost` rule). */
export function defaultFleetHost(rows: Pick<FleetRows, 'hosts'>): string {
  const list = fleetHostChoices(rows);
  return list.find((h) => h.reachable)?.alias ?? list[0]?.alias ?? '';
}

/** Per extra fleet the filter shows, the ids of its sessions that need you
 *  (`fleetInbox`, as a lookup for the sidebar's rows). */
export const fleetNeedsIds = derived(fleetInbox, ($g) => new Map($g.map((g) => [g.fleet.id, new Set(g.rows.map((s) => s.id))])));

/** A fleet's connection, as one short word for its header and chip. */
export function connectionWord(f: Pick<FleetInfo, 'connection' | 'unavailable' | 'local'>): {
  word: string;
  tone: 'ok' | 'warn' | 'bad' | 'idle';
} {
  if (f.unavailable) return { word: 'unavailable', tone: 'bad' };
  if (f.local) return { word: 'this machine', tone: 'ok' };
  switch (f.connection?.state) {
    case 'connected':
      return { word: 'live', tone: 'ok' };
    case 'connecting':
      return { word: 'connecting', tone: 'idle' };
    case 'reconnecting':
      return { word: 'reconnecting', tone: 'warn' };
    case 'offline':
      return { word: 'offline', tone: 'bad' };
    case 'hub_too_old':
      return { word: 'hub too old', tone: 'bad' };
    case 'hub_too_new':
      return { word: 'update this app', tone: 'bad' };
    default:
      return { word: 'not connected', tone: 'idle' };
  }
}

/** Test seam: reset module state. */
export function _resetFleetsForTest(): void {
  fleetsStore.set([]);
  rowsStore.set(new Map());
  tombstones.clear();
  listsOut.clear();
  fleetFilter.set('all');
  fleetSelection.set(null);
  started = false;
}

/** Test seam: seed one extra fleet's rows. */
export function _setFleetRowsForTest(fleet: string, rows: FleetRows): void {
  rowsStore.update((m) => new Map(m).set(fleet, rows));
}

/** Test seam: seed the fleet list. */
export function _setFleetsForTest(list: FleetInfo[]): void {
  fleetsStore.set(list);
}
