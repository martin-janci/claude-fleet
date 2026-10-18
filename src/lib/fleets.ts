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
      .map((f) => ({ fleet: f, rows: $rows.get(f.id) ?? emptyRows() })),
);

// ── applying events ─────────────────────────────────────────────────────

export function emptyRows(): FleetRows {
  return { sessions: new Map(), hosts: new Map(), projects: [], error: null };
}

function isObj(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null;
}

/** Apply one `fleet:event` to that fleet's rows, in place. Returns what the
 *  caller has to do beyond that: `relist` after a gap the bridge could not
 *  replay, `connection` when the link moved. Unknown names are ignored —
 *  this fleet's view keeps sessions, hosts and projects only (Phase 1). */
export function applyFleetEvent(
  rows: FleetRows,
  e: Pick<FleetEvent, 'name' | 'payload'>,
): 'relist' | 'connection' | null {
  const p = e.payload;
  switch (e.name) {
    case 'session:created':
    case 'session:updated':
      if (isObj(p) && typeof p.id === 'number') rows.sessions.set(p.id, p as unknown as SessionRow);
      return null;
    case 'session:killed':
      if (isObj(p) && typeof p.id === 'number') rows.sessions.delete(p.id);
      return null;
    case 'host:added':
    case 'host:probed':
      if (isObj(p) && typeof p.alias === 'string') rows.hosts.set(p.alias, p as unknown as HostRow);
      return null;
    case 'host:pinged':
      if (isObj(p) && typeof p.alias === 'string') {
        const old = rows.hosts.get(p.alias);
        if (old) rows.hosts.set(p.alias, { ...old, ...(p as Partial<HostRow>) });
      }
      return null;
    case 'host:removed':
      if (isObj(p) && typeof p.alias === 'string') rows.hosts.delete(p.alias);
      return null;
    case 'project:updated':
      if (isObj(p) && typeof p.id === 'number') {
        const row = p as unknown as ProjectRow;
        const at = rows.projects.findIndex((t) => t.project.id === row.id);
        if (at >= 0) rows.projects[at] = { ...rows.projects[at], project: row };
        else rows.projects.push({ project: row, worktrees: [] });
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

function patchRows(fleet: string, fn: (rows: FleetRows) => void): void {
  rowsStore.update((m) => {
    const next = new Map(m);
    const rows = next.get(fleet) ?? emptyRows();
    // A fresh object so `$fleetRows` subscribers see a change.
    const copy: FleetRows = {
      sessions: new Map(rows.sessions),
      hosts: new Map(rows.hosts),
      projects: [...rows.projects],
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
  let todo: ReturnType<typeof applyFleetEvent> = null;
  patchRows(e.fleet, (rows) => {
    todo = applyFleetEvent(rows, e);
  });
  if (todo === 'connection' && isObj(e.payload)) {
    setConnection(e.fleet, e.payload as unknown as HubConnection);
    if ((e.payload as { state?: string }).state === 'connected') void loadFleetRows(e.fleet);
  } else if (todo === 'relist') {
    void loadFleetRows(e.fleet);
  }
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
  const [s, h, p] = await Promise.all([
    invokeCmd<SessionRow[]>('list_sessions', { fleet }),
    invokeCmd<HostRow[]>('list_hosts', { fleet }),
    invokeCmd<ProjectTreeRow[]>('list_projects', { fleet }),
  ]);
  patchRows(fleet, (rows) => {
    if (s.ok && Array.isArray(s.value)) rows.sessions = new Map(s.value.map((r) => [r.id, r]));
    if (h.ok && Array.isArray(h.value)) rows.hosts = new Map(h.value.map((r) => [r.alias, r]));
    if (p.ok && Array.isArray(p.value)) rows.projects = p.value;
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

/** Call a fleet-scoped command on `fleet`'s hub. */
export function fleetCmd<T>(cmd: FleetScopedCommand, fleet: string, args: Record<string, unknown> = {}): Promise<Result<T>> {
  return invokeCmd<T>(cmd, { ...args, fleet });
}

export function fleetSendPrompt(fleet: string, s: SessionRow, prompt: string): Promise<Result<null>> {
  return fleetCmd('send_prompt', fleet, {
    args: { host_alias: s.host_alias, tmux_name: s.tmux_name, prompt, submit: true },
  });
}

export async function fleetKillSession(fleet: string, s: SessionRow): Promise<Result<number>> {
  const r = await fleetCmd<number>('kill_session', fleet, { args: { host_alias: s.host_alias, name: s.tmux_name } });
  if (r.ok) patchRows(fleet, (rows) => rows.sessions.delete(s.id));
  return r;
}

export async function fleetRenameSession(fleet: string, s: SessionRow, name: string): Promise<Result<SessionRow>> {
  const r = await fleetCmd<SessionRow>('set_session_friendly_name', fleet, {
    args: { host_alias: s.host_alias, tmux_name: s.tmux_name, friendly_name: name },
  });
  if (r.ok) patchRows(fleet, (rows) => rows.sessions.set(r.value.id, r.value));
  return r;
}

export async function fleetPinSession(fleet: string, s: SessionRow, pinned: boolean): Promise<Result<SessionRow>> {
  const r = await fleetCmd<SessionRow>('pin_session', fleet, { args: { session_id: s.id, pinned } });
  if (r.ok) patchRows(fleet, (rows) => rows.sessions.set(r.value.id, r.value));
  return r;
}

export interface FleetNewSession {
  host_alias: string;
  project_id: number;
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
      kind: 'work',
      start_command: null,
      friendly_name: a.friendly_name ?? null,
    },
  });
  if (r.ok) patchRows(fleet, (rows) => rows.sessions.set(r.value.id, r.value));
  return r;
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
  fleetFilter.set('all');
  started = false;
}

/** Test seam: seed the fleet list. */
export function _setFleetsForTest(list: FleetInfo[]): void {
  fleetsStore.set(list);
}
