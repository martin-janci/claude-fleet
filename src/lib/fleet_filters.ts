// The sidebar's Machine and Organisation filters across fleets (multi-hub
// design, Phase 3a: docs/superpowers/specs/2026-10-17-desktop-multi-hub-design.md).
//
// A host alias and an org id mean something only inside one hub: both hubs
// can have a host `mac` and an org 3. So a Machine or Organisation choice
// always belongs to exactly ONE fleet — its "anchor":
//
//   - the primary's choice stays where it always lived (`hostFilter`,
//     `scopeFilter`), so a one-hub window behaves exactly as before;
//   - an extra fleet's choice lives here (`fleetHostFilter`,
//     `fleetScopeFilter`), named by the fleet's slot id;
//   - choosing a value in one fleet clears the Machine and Organisation
//     values of every other fleet (`pick*`), so the two never contradict;
//   - while a fleet anchors the filters, every OTHER fleet's rows are hidden
//     and each one says so in place, with a count and a way back
//     (`fleetListView().hiddenByAnchor`, `primaryHiddenByAnchor`) — never a
//     silent empty list;
//   - the fleet switcher wins: an anchor in a fleet the switcher hides is
//     ignored, so it can never hide the fleet the person asked to see.
//
// Search, Last active, Needs you, Agent and Background agents are not
// per-hub: they apply to every fleet's rows alike.
import { derived, writable, get } from 'svelte/store';
import {
  PRIMARY,
  filterShows,
  fleetFilter,
  fleetRows,
  fleets,
  onFleetForgotten,
  type FleetInfo,
  type FleetRows,
} from './fleets';
import { hostFilter, effectiveHostFilter } from './hosts';
import {
  ALL_SCOPES,
  UNASSIGNED,
  buildScopes,
  cycleScope,
  effectiveScope,
  ownersByProject,
  scopeFilter,
  scopeOfSession,
  type Scope,
  type ScopeId,
} from './orgs';
import { readPref, writePref } from './prefs';
import { withinRecency, type Recency } from './session_status';
import { sessionMatchesSearch } from './search';
import { sessionAgent, type SessionRow } from './sessions';
import { sessionVisible } from './sidebar_index';
import type { AgentFilter } from './session_scope';

/** An extra fleet's chosen machine. */
export interface FleetHostPick {
  fleet: string;
  alias: string;
}
/** An extra fleet's chosen org (`org:<id>`, `owner:<name>`, `unassigned`). */
export interface FleetScopePick {
  fleet: string;
  scope: ScopeId;
}

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null;
const isHostPick = (v: unknown): v is FleetHostPick | null =>
  v === null || (isObj(v) && typeof v.fleet === 'string' && typeof v.alias === 'string');
const isScopePick = (v: unknown): v is FleetScopePick | null =>
  v === null || (isObj(v) && typeof v.fleet === 'string' && typeof v.scope === 'string');

/** An extra fleet's machine, or null. Persisted like `hostFilter`. */
export const fleetHostFilter = writable<FleetHostPick | null>(readPref('fleet-host-filter', null, isHostPick));
fleetHostFilter.subscribe((v) => writePref('fleet-host-filter', v));

/** An extra fleet's org, or null. Persisted like `scopeFilter`. */
export const fleetScopeFilter = writable<FleetScopePick | null>(readPref('fleet-scope-filter', null, isScopePick));
fleetScopeFilter.subscribe((v) => writePref('fleet-scope-filter', v));

const isBool = (v: unknown): v is boolean => typeof v === 'boolean';
/** This window's own list, unfolded under its fleet header (shown only
 *  beside another fleet). Per viewer. */
export const primaryFleetOpen = writable<boolean>(readPref('primary-fleet-open', true, isBool));
primaryFleetOpen.subscribe((v) => writePref('primary-fleet-open', v));

const isFolds = (v: unknown): v is string[] => Array.isArray(v) && v.every((x) => typeof x === 'string');
/** The extra fleets folded under their header (`FleetSessionGroups`), by
 *  slot id. Per viewer. */
export const fleetGroupsFolded = writable<string[]>(readPref('fleet-groups-folded', [], isFolds));
fleetGroupsFolded.subscribe((v) => writePref('fleet-groups-folded', v));

/** Fold or unfold one extra fleet's group. */
export function toggleFleetGroupFold(id: string): void {
  fleetGroupsFolded.update((f) => (f.includes(id) ? f.filter((x) => x !== id) : [...f, id]));
}

/** A removed fleet's picks and fold go with it. Slot ids are reused by
 *  label, so another hub added as "Work" must not inherit the old one's
 *  machine or org — an alias like `mac` or an org id like `org:3` can mean
 *  something there too, and would hide every other fleet behind it. */
export function forgetFleetFilters(id: string): void {
  if (get(fleetHostFilter)?.fleet === id) fleetHostFilter.set(null);
  if (get(fleetScopeFilter)?.fleet === id) fleetScopeFilter.set(null);
  if (get(fleetGroupsFolded).includes(id)) fleetGroupsFolded.update((f) => f.filter((x) => x !== id));
}
onFleetForgotten(forgetFleetFilters);

// ── one fleet's scopes ──────────────────────────────────────────────────

/** An extra fleet's Organisation chips: its named orgs, then the owners of
 *  its live sessions that no org covers — the primary's rule
 *  (`orgs.ts::buildScopes`) over that fleet's own rows. */
export function fleetScopesOf(rows: Pick<FleetRows, 'sessions' | 'projects' | 'orgs'>): Scope[] {
  return buildScopes([...rows.sessions.values()], rows.orgs ?? [], ownersByProject(rows.projects));
}

/** `(session) → scope id` inside one extra fleet. */
export function fleetScopeOfFn(rows: Pick<FleetRows, 'projects'>): (s: SessionRow) => ScopeId {
  const owners = ownersByProject(rows.projects);
  return (s) => scopeOfSession(s, owners);
}

/** A scope's label in one fleet: `Unassigned`, the org's name, or its id. */
export function scopeLabelIn(list: readonly Scope[], id: ScopeId): string {
  if (id === UNASSIGNED) return 'Unassigned';
  return list.find((x) => x.id === id)?.label ?? id;
}

// ── the anchor ──────────────────────────────────────────────────────────

/** The fleet whose Machine / Organisation choice is in force, and what it
 *  chose (null = that facet is not chosen). Null when nothing is chosen. */
export interface Anchor {
  fleet: string;
  host: string | null;
  scope: ScopeId | null;
}

export interface AnchorInput {
  /** The fleet switcher (`fleetFilter`). */
  filter: string;
  known: readonly FleetInfo[];
  rows: ReadonlyMap<string, FleetRows>;
  /** The primary's effective choices (`effectiveHostFilter`, `effectiveScope`). */
  primaryHost: string;
  primaryScope: ScopeId;
  fleetHost: FleetHostPick | null;
  fleetScope: FleetScopePick | null;
}

/** Whether an extra fleet can anchor: known, not the primary, not a
 *  duplicate, and shown by the switcher. */
function anchorable(fleet: string, i: Pick<AnchorInput, 'filter' | 'known'>): boolean {
  const f = i.known.find((x) => x.id === fleet);
  return !!f && !f.primary && !f.duplicate_of && filterShows(i.filter, fleet, i.known as FleetInfo[]);
}

/** The pick still names something that fleet has. Before its rows load (no
 *  hosts, no sessions) a remembered choice stands, as the primary's does. */
function hostStands(p: FleetHostPick, rows: ReadonlyMap<string, FleetRows>): boolean {
  const r = rows.get(p.fleet);
  return !r || r.hosts.size === 0 || r.hosts.has(p.alias);
}
function scopeStands(p: FleetScopePick, rows: ReadonlyMap<string, FleetRows>): boolean {
  if (p.scope === UNASSIGNED) return true;
  const r = rows.get(p.fleet);
  if (!r) return true;
  const list = fleetScopesOf(r);
  return list.length === 0 || list.some((x) => x.id === p.scope);
}

/** The anchor in force (see the module docs), or null. */
export function machineOrgAnchor(i: AnchorInput): Anchor | null {
  const host = i.fleetHost && anchorable(i.fleetHost.fleet, i) && hostStands(i.fleetHost, i.rows) ? i.fleetHost : null;
  const scope =
    i.fleetScope && anchorable(i.fleetScope.fleet, i) && scopeStands(i.fleetScope, i.rows) ? i.fleetScope : null;
  // The pickers keep both in one fleet; should they ever disagree, the
  // machine (the narrower) wins and the other fleet's org is ignored.
  const extra = host?.fleet ?? scope?.fleet ?? null;
  if (extra) {
    return {
      fleet: extra,
      host: host?.fleet === extra ? host.alias : null,
      scope: scope?.fleet === extra ? scope.scope : null,
    };
  }
  if (!filterShows(i.filter, PRIMARY, i.known as FleetInfo[])) return null;
  if (i.primaryHost === 'all' && i.primaryScope === ALL_SCOPES) return null;
  return {
    fleet: PRIMARY,
    host: i.primaryHost === 'all' ? null : i.primaryHost,
    scope: i.primaryScope === ALL_SCOPES ? null : i.primaryScope,
  };
}

/** The anchor in force now. */
export const anchor = derived(
  [fleetFilter, fleets, fleetRows, effectiveHostFilter, effectiveScope, fleetHostFilter, fleetScopeFilter],
  ([$filter, $known, $rows, $host, $scope, $fh, $fs]) =>
    machineOrgAnchor({
      filter: $filter,
      known: $known,
      rows: $rows,
      primaryHost: $host,
      primaryScope: $scope,
      fleetHost: $fh,
      fleetScope: $fs,
    }),
);

/** Which facets of an anchor are on, in words: `machine`, `org`, or both. */
export function anchorFacetWord(a: Anchor): string {
  if (a.host !== null && a.scope !== null) return 'machine and org filters';
  return a.host !== null ? 'machine filter' : 'org filter';
}

/** The primary's own list is hidden by an extra fleet's anchor. */
export function primaryHiddenByAnchor(a: Anchor | null): boolean {
  return a !== null && a.fleet !== PRIMARY;
}

/** "Machine: eva-air · Org: PapayaPOS on Papaya POS (hub)" — what the
 *  hidden-rows notice names, so the person can tell which fleet's filter it
 *  is and why another fleet's rows are gone. */
export function anchorText(a: Anchor, known: readonly FleetInfo[], scopeLabel: (id: ScopeId) => string): string {
  const parts: string[] = [];
  if (a.host !== null) parts.push(`Machine: ${a.host}`);
  if (a.scope !== null) parts.push(`Org: ${scopeLabel(a.scope)}`);
  const label = known.find((f) => f.id === a.fleet)?.label ?? a.fleet;
  return `${parts.join(' · ')} on ${label}`;
}

// ── one extra fleet's list ──────────────────────────────────────────────

/** The filters every fleet's rows share, plus the anchor. */
export interface FleetListCtx {
  anchor: Anchor | null;
  search: string;
  recency: Recency;
  nowSec: number;
  showBg: boolean;
  agent: AgentFilter;
  needsYouOnly: boolean;
  /** That fleet's sessions that need you (`fleetNeedsIds`). */
  needs: ReadonlySet<number>;
}

/** An extra fleet's rows as the Sessions list shows them: `shown` passes
 *  every filter; `hiddenByAnchor` counts the rows that pass the shared
 *  filters but are hidden because ANOTHER fleet anchors Machine / Org. */
export function fleetListView(
  fleet: string,
  rows: Pick<FleetRows, 'sessions' | 'projects' | 'orgs'>,
  ctx: FleetListCtx,
): { shown: SessionRow[]; hiddenByAnchor: number } {
  const names = new Map(rows.projects.map((t) => [t.project.id, `${t.project.owner}/${t.project.repo}`]));
  const q = ctx.search.trim();
  const shared = [...rows.sessions.values()].filter(
    (s) =>
      s.status !== 'lost' &&
      (ctx.showBg || s.kind !== 'bg') &&
      withinRecency(s.last_activity_at, ctx.recency, ctx.nowSec) &&
      (ctx.agent === 'any' || sessionAgent(s) === ctx.agent) &&
      (!ctx.needsYouOnly || ctx.needs.has(s.id)) &&
      (!q || sessionMatchesSearch(s, q, s.project_id == null ? null : (names.get(s.project_id) ?? null))),
  );
  const a = ctx.anchor;
  if (a && a.fleet !== fleet) return { shown: [], hiddenByAnchor: shared.length };
  if (!a) return { shown: shared, hiddenByAnchor: 0 };
  const scope = a.scope === null ? null : { id: a.scope, of: fleetScopeOfFn(rows) };
  return {
    shown: shared.filter((s) => sessionVisible(s, a.host ?? 'all', true, null, scope)),
    hiddenByAnchor: 0,
  };
}

// ── choosing ────────────────────────────────────────────────────────────

/** Machine › Any: no machine in any fleet. Org choices stay. */
export function clearMachines(): void {
  hostFilter.set('all');
  fleetHostFilter.set(null);
}

/** Organisation › Any: no org in any fleet. Machine choices stay. */
export function clearScopes(): void {
  scopeFilter.set(ALL_SCOPES);
  fleetScopeFilter.set(null);
}

/** Every Machine and Organisation choice, in every fleet. */
export function clearMachineOrg(): void {
  clearMachines();
  clearScopes();
}

/** A primary machine: every extra fleet's choices go. */
export function pickPrimaryHost(alias: string): void {
  if (alias === 'all') return clearMachines();
  hostFilter.set(alias);
  fleetHostFilter.set(null);
  fleetScopeFilter.set(null);
}

/** A primary org: every extra fleet's choices go. */
export function pickPrimaryScope(id: ScopeId): void {
  if (id === ALL_SCOPES) return clearScopes();
  scopeFilter.set(id);
  fleetHostFilter.set(null);
  fleetScopeFilter.set(null);
}

/** Show a primary session the person asked for (a reveal, a focus): an
 *  extra fleet's anchor would hide the primary list it is in, so it goes,
 *  and a folded primary list unfolds. The primary's own choices stay — the
 *  caller widens those as it needs. */
export function showPrimaryList(): void {
  fleetHostFilter.set(null);
  fleetScopeFilter.set(null);
  primaryFleetOpen.set(true);
}

/** ⌘⇧O / Ctrl+Shift+O on the primary's orgs (`orgs.ts::cycleScope`). */
export function cyclePrimaryScope(): void {
  cycleScope(pickPrimaryScope);
}

/** An extra fleet's machine; picking the chosen one again clears it. The
 *  primary's choices, and any other fleet's, go. */
export function pickFleetHost(fleet: string, alias: string): void {
  const cur = get(fleetHostFilter);
  hostFilter.set('all');
  scopeFilter.set(ALL_SCOPES);
  if (get(fleetScopeFilter)?.fleet !== fleet) fleetScopeFilter.set(null);
  fleetHostFilter.set(cur?.fleet === fleet && cur.alias === alias ? null : { fleet, alias });
}

/** An extra fleet's org; picking the chosen one again clears it. */
export function pickFleetScope(fleet: string, scope: ScopeId): void {
  const cur = get(fleetScopeFilter);
  hostFilter.set('all');
  scopeFilter.set(ALL_SCOPES);
  if (get(fleetHostFilter)?.fleet !== fleet) fleetHostFilter.set(null);
  fleetScopeFilter.set(cur?.fleet === fleet && cur.scope === scope ? null : { fleet, scope });
}

/** Test seam. */
export function _resetFleetFiltersForTest(): void {
  primaryFleetOpen.set(true);
  fleetHostFilter.set(null);
  fleetScopeFilter.set(null);
  fleetGroupsFolded.set([]);
}
