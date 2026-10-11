// Organisations (work graph M5): the scope selector's entries, the orgs
// Settings shows, and the admin commands.
//
// A scope is a VIEW here — the desktop is the master (or a paired client),
// and both read every org. The boundary for per-host tokens is enforced on
// the hub; nothing in this file decides what anyone may read.
//
// Zero config: with no named org, the scopes are the GitHub owners of the
// live sessions' projects (never `local`, never fleet's own system project).
// The selector appears only when there are two or more scopes, so a user
// with one owner sees no new chrome — and a persisted scope that no longer
// exists (or a single-scope fleet) reads as "all", never as a filter nobody
// can see.
import type { Descriptor } from './pages/pages';
import type { AdminNeed, OrgAccount, OrgShare, PersonSpend, RemovedMember, TeamMember } from './pages/resources';
import { writable, derived, get } from 'svelte/store';
import { invokeCmd, type Result } from './result';
import type { Pairing } from './devices';
import { readPref, writePref } from './prefs';
import { sessions, type SessionRow } from './sessions';
import { projects, type ProjectTreeRow } from './projects';
import { countNeedsYou, type AttentionOptions } from './attention';

export interface OrgRow {
  id: number;
  name: string;
  color?: string | null;
  isolate_sessions?: boolean;
  created_at: number;
  /** Work graph M7: this org's auto-tidy override; absent = inherit
   *  `work.auto_tidy`. */
  auto_tidy?: boolean | null;
  /** Jev evaluation (D31): this org consented to its redacted texts going
   *  to the decision model (docs/decisions.md). Off by default; absent from
   *  an older hub. */
  jev_allowed?: boolean;
  /** Jev evaluation (D48): this org also consented to Claude's reply text
   *  (a pane tail at a turn's end, J2) going to the decision model, on top
   *  of `jev_allowed`. Off by default; absent from an older hub. */
  jev_reply_allowed?: boolean;
  /** D31 (work graph M14): devices bound to this org (`fleet-hub pair
   *  --org`) also see unassigned work and sessions, as a host does. On by
   *  default; absent from an older hub, which has no bound devices. */
  bound_sees_unassigned?: boolean;
}

/** An org's auto-tidy setting as the select shows it. */
export type OrgAutoTidy = 'on' | 'off' | 'inherit';

export function orgAutoTidy(o: Pick<OrgRow, 'auto_tidy'>): OrgAutoTidy {
  return o.auto_tidy == null ? 'inherit' : o.auto_tidy ? 'on' : 'off';
}

export interface OrgRuleRow {
  id: number;
  org_id: number;
  owner?: string | null;
  repo?: string | null;
  path_prefix?: string | null;
  host_alias?: string | null;
}

/** `list_orgs`: an org with its rules, hosts and trackers, and its overview
 *  (org administration phase A; absent from an older hub). */
export interface OrgDetail extends OrgRow {
  rules: OrgRuleRow[];
  hosts: string[];
  trackers: { id: number; name: string }[];
  /** The asset catalogs it owns, by name. */
  catalogs?: string[];
  /** Its live sessions the caller may count, and those waiting on a person. */
  session_count?: number;
  needs_you?: number;
  /** Its bound paired devices — present only for the fleet's administrator. */
  devices?: { name: string; mode: string; trusted: boolean; last_seen_at?: number }[];
  /** Phase C, administrator only: its per-org settings. */
  settings?: OrgSettingRow[];
  /** Phase C, administrator only, when the caller sees every session. */
  spent_today_micros?: number;
  spent_week_micros?: number;
  spent_month_micros?: number;
  budget_daily_usd?: number;
  budget_monthly_usd?: number;
  over_budget?: ('daily' | 'monthly')[];
  /** Redesign 11.1, with the spend: each of the last 14 UTC days, today last. */
  spend_series?: { day: string; cost_micros: number }[];
  /** Redesign 11.8, all or nothing: the spend by person, to an admin who
   *  sees every session. */
  spend_by_person?: PersonSpend[];
  /** Redesign 11.1, for its admins: what they should look at. */
  needs_admin?: AdminNeed[];
  /** Phase D: who is in the company, for its people and the hub's owner. */
  members?: OrgMember[];
  /** Phase D: the caller's own role in it. */
  my_role?: 'admin' | 'member' | 'viewer';
  /** M15 G7.14, with `members`: how many there are. */
  member_count?: number;
  /** Phase D: this company owns the hub. */
  owns_hub?: boolean;
  /** Phase D: its admins see the unclaimed count on its hosts. */
  admins_see_unclaimed?: boolean;
  /** M15 G4.7, for its admins: every live share on its sessions. */
  shares?: OrgShare[];
  /** M15 G4.7: who is working on what. */
  team?: TeamMember[];
  /** M15 G4.7, for its admins: former members and the shares they hold. */
  removed_members?: RemovedMember[];
  /** M15 G4.7: the Claude accounts its hosts use. */
  accounts?: OrgAccount[];
}

/** One member of an org, as its page lists them. */
export interface OrgMember {
  person_id: number;
  name: string;
  display_name?: string;
  role: string;
  /** Redesign 11.2: when they joined (absent from an older hub). */
  added_at?: number;
  /** Redesign 11.2: since when an org share reaches them; absent for a
   *  viewer, who receives none. */
  shares_since?: number;
  /** Redesign 11.2, for whoever administers the org: their devices. */
  devices?: string[];
  /** M15 G7.14, with `devices`: the ones not trusted yet. */
  untrusted_devices?: string[];
  /** Redesign 11.7c: their live sessions in this org (absent when none, or
   *  from an older hub). */
  live_sessions?: number;
  /** Of those, how many you may not open: shown as existing, never named. */
  private_sessions?: number;
}

/** A member's live sessions in words (redesign 11.7c), or '—' for none. A
 *  private one is only counted: you see that it exists, not what it is. */
export function memberSessionsWord(m: Pick<OrgMember, 'live_sessions' | 'private_sessions'>): string {
  const live = m.live_sessions ?? 0;
  const priv = Math.min(m.private_sessions ?? 0, live);
  if (live === 0) return '—';
  if (priv === 0) return `${live} live`;
  if (priv === live) return `${live} live, private`;
  return `${live} live, ${priv} private`;
}

/** How many live shares TO a member stand on the org's sessions. */
export interface MemberGrants {
  watch: number;
  drive: number;
}

/** What happens to those shares when the member leaves: taken back, each
 *  Drive narrowed to Watch, or left as they are. */
export type GrantsOnRemove = 'revoke' | 'narrow' | 'keep';

export function orgMemberGrants(orgId: number, personId: number): Promise<Result<MemberGrants>> {
  return invokeCmd<MemberGrants>('org_member_grants', { args: { org_id: orgId, person_id: personId } });
}

/** Mint a one-time pairing code for a member's new device, fenced to the
 *  org and theirs. A standalone desktop refuses: a code is the hub's. */
export function pairMemberDevice(orgId: number, person: string, device: string): Promise<Result<Pairing>> {
  return invokeCmd<Pairing>('pair_device', { args: { device, mode: 'full', org_id: orgId, person } });
}

/** A member's pairing in progress on an org's page. Kept here, not in the
 *  editor, because the editor is rebuilt on every re-read of the list. */
export const memberPairing = writable<{ org_id: number; person: string; pairing?: Pairing } | null>(null);

/** One per-org setting on an org's page: the setting described with the
 *  fleet's value, and the org's own when it set one. */
export interface OrgSettingRow {
  setting: Descriptor;
  own?: string;
}

/** `org_suggestions`: create org `name` from `owner/*` and/or for a tracker. */
export interface OrgSuggestion {
  name: string;
  owner?: string | null;
  tracker_id?: number | null;
  sessions: number;
  reason: string;
}

/** `'all'`, `'org:<id>'`, `'owner:<name>'` or `'unassigned'`. */
export type ScopeId = string;

export interface Scope {
  id: ScopeId;
  label: string;
  color: string | null;
}

export const ALL_SCOPES: ScopeId = 'all';
export const UNASSIGNED: ScopeId = 'unassigned';

export const orgs = writable<OrgDetail[]>([]);

let loadSeq = 0;

/** Reload the orgs. A stale answer (one a later load overtook) never
 *  overwrites the newer one. */
export async function loadOrgs(): Promise<Result<OrgDetail[]>> {
  const seq = ++loadSeq;
  const r = await invokeCmd<OrgDetail[]>('list_orgs');
  // A hub older than M5 (or a test double) may answer nothing usable.
  if (r.ok && seq === loadSeq) orgs.set(Array.isArray(r.value) ? r.value : []);
  return r;
}

export async function loadOrgSuggestions(): Promise<Result<OrgSuggestion[]>> {
  return invokeCmd<OrgSuggestion[]>('org_suggestions');
}

/** A rule as a chip: `acme/*`, `acme/api`, `path: ~/w/acme`, `host: h`. */
export function ruleChip(r: OrgRuleRow): string {
  const parts: string[] = [];
  if (r.owner) parts.push(r.repo ? `${r.owner}/${r.repo}` : `${r.owner}/*`);
  if (r.path_prefix) parts.push(`path: ${r.path_prefix}`);
  if (r.host_alias) parts.push(`host: ${r.host_alias}`);
  return parts.join(' · ');
}

/** project id → its owner, for projects an owner scope can name. */
export function ownersByProject(rows: readonly ProjectTreeRow[]): Map<number, string> {
  const m = new Map<number, string>();
  for (const p of rows) {
    if (p.project.owner !== 'local' && !p.project.system) m.set(p.project.id, p.project.owner);
  }
  return m;
}

/** The scope a session is in: its org, else its project's owner, else
 *  unassigned. */
export function scopeOfSession(s: SessionRow, owners: ReadonlyMap<number, string>): ScopeId {
  if (s.org_id != null) return `org:${s.org_id}`;
  const owner = s.project_id != null ? owners.get(s.project_id) : undefined;
  return owner ? `owner:${owner}` : UNASSIGNED;
}

/** The selector's scopes: every named org, then the owners of live sessions
 *  that no org covers. Unassigned rows never count towards the "two or
 *  more" that shows the selector (sessions outside any project are not a
 *  company), but the selector offers them once it is shown. */
export function buildScopes(
  rows: readonly SessionRow[],
  named: readonly OrgRow[],
  owners: ReadonlyMap<number, string>,
): Scope[] {
  const out: Scope[] = named.map((o) => ({ id: `org:${o.id}`, label: o.name, color: o.color ?? null }));
  const seen = new Set<string>();
  for (const s of rows) {
    if (s.kind === 'external' || s.org_id != null || s.project_id == null) continue;
    const owner = owners.get(s.project_id);
    if (!owner || seen.has(owner)) continue;
    seen.add(owner);
    out.push({ id: `owner:${owner}`, label: owner, color: null });
  }
  return out;
}

const isString = (v: unknown): v is string => typeof v === 'string';
/** The chosen scope, persisted like `hostFilter`. */
export const scopeFilter = writable<ScopeId>(readPref('scope-filter', ALL_SCOPES, isString));
scopeFilter.subscribe((v) => writePref('scope-filter', v));

export const projectOwners = derived(projects, ($p) => ownersByProject($p));
export const scopes = derived([sessions, orgs, projectOwners], ([$s, $o, $owners]) =>
  buildScopes($s, $o, $owners),
);
/** The selector shows only when there is something to choose between. */
export const scopeSelectorShown = derived(scopes, ($sc) => $sc.length >= 2);

/** The scope actually applied: `all` whenever the selector is hidden or the
 *  persisted choice names a scope that no longer exists. */
export function effectiveScopeOf(chosen: ScopeId, list: readonly Scope[]): ScopeId {
  if (list.length < 2) return ALL_SCOPES;
  if (chosen === ALL_SCOPES || chosen === UNASSIGNED) return chosen;
  return list.some((x) => x.id === chosen) ? chosen : ALL_SCOPES;
}
export const effectiveScope = derived([scopeFilter, scopes], ([$f, $sc]) => effectiveScopeOf($f, $sc));

/** `(session) → scope id` for the builders, current projects applied. */
export const scopeOf = derived(projectOwners, ($owners) => (s: SessionRow) => scopeOfSession(s, $owners));

/** org id → colour, only when two or more orgs exist (a single org gets no
 *  colour bar: there is nothing to tell apart). */
export const orgColorById = derived(orgs, ($o) => {
  const m = new Map<number, string>();
  if ($o.length < 2) return m;
  for (const o of $o) if (o.color) m.set(o.id, o.color);
  return m;
});

/** The colour bar for a session row, or null. */
export function orgColorOf(s: Pick<SessionRow, 'org_id'>, colors: ReadonlyMap<number, string>): string | null {
  return s.org_id != null ? (colors.get(s.org_id) ?? null) : null;
}

/** Needs-you is never hidden by scope (plan decision 6): per OTHER scope,
 *  how many of its sessions are waiting on the person — the same count the
 *  "Needs you" pill uses. Empty when no scope is chosen. */
export function needsYouElsewhere(
  rows: readonly SessionRow[],
  current: ScopeId,
  list: readonly Scope[],
  of: (s: SessionRow) => ScopeId,
  opts: AttentionOptions,
): { scope: ScopeId; label: string; count: number }[] {
  if (current === ALL_SCOPES) return [];
  const by = new Map<ScopeId, SessionRow[]>();
  for (const s of rows) {
    const id = of(s);
    if (id === current) continue;
    if (!by.has(id)) by.set(id, []);
    by.get(id)!.push(s);
  }
  const labelOf = (id: ScopeId) =>
    id === UNASSIGNED ? 'Unassigned' : (list.find((x) => x.id === id)?.label ?? id);
  const out: { scope: ScopeId; label: string; count: number }[] = [];
  for (const [id, group] of by) {
    const count = countNeedsYou(group, opts);
    if (count > 0) out.push({ scope: id, label: labelOf(id), count });
  }
  return out.sort((a, b) => b.count - a.count || a.label.localeCompare(b.label));
}

/** ⌘⇧O / Ctrl+Shift+O: the next scope after the chosen one (wrapping
 *  through "all"). A no-op while the selector is hidden. */
export function cycleScope(): void {
  const list = get(scopes);
  if (list.length < 2) return;
  const ids = [ALL_SCOPES, ...list.map((x) => x.id)];
  const cur = effectiveScopeOf(get(scopeFilter), list);
  const i = ids.indexOf(cur);
  scopeFilter.set(ids[(i + 1) % ids.length]);
}

// --- administration: the hub's master-only `work_admin`; LocalOnly on a
// paired desktop, whose Settings shows the orgs read-only.

async function thenReload<T>(r: Promise<Result<T>>): Promise<Result<T>> {
  const out = await r;
  if (out.ok) await loadOrgs();
  return out;
}

export function addOrg(name: string, color: string | null, isolateSessions: boolean) {
  return thenReload(
    invokeCmd<OrgRow>('add_org', { args: { name, color, isolate_sessions: isolateSessions } }),
  );
}

export function updateOrg(
  orgId: number,
  patch: {
    name?: string;
    color?: string;
    isolate_sessions?: boolean;
    auto_tidy?: OrgAutoTidy;
    jev?: 'on' | 'off';
    jev_reply?: 'on' | 'off';
    bound_sees_unassigned?: boolean;
  },
) {
  return thenReload(invokeCmd<OrgRow>('update_org', { args: { org_id: orgId, ...patch } }));
}

export function removeOrg(orgId: number) {
  return thenReload(invokeCmd<void>('remove_org', { args: { org_id: orgId } }));
}

export function addOrgRule(rule: Omit<OrgRuleRow, 'id'>) {
  return thenReload(invokeCmd<OrgRuleRow>('add_org_rule', { args: rule }));
}

export function removeOrgRule(ruleId: number) {
  return thenReload(invokeCmd<void>('remove_org_rule', { args: { rule_id: ruleId } }));
}

export function assignHostOrg(hostAlias: string, orgId: number | null) {
  return thenReload(
    invokeCmd<void>('assign_host_org', { args: { host_alias: hostAlias, org_id: orgId } }),
  );
}

export function assignTrackerOrg(trackerId: number, orgId: number | null) {
  return thenReload(
    invokeCmd<unknown>('assign_tracker_org', { args: { tracker_id: trackerId, org_id: orgId } }),
  );
}

/** One click on a suggestion: the org, its owner rule, its tracker. */
export async function createFromSuggestion(sg: OrgSuggestion): Promise<Result<OrgRow>> {
  const org = await invokeCmd<OrgRow>('add_org', {
    args: { name: sg.name, color: null, isolate_sessions: false },
  });
  if (!org.ok) return org;
  if (sg.owner) {
    const r = await invokeCmd<OrgRuleRow>('add_org_rule', {
      args: { org_id: org.value.id, owner: sg.owner },
    });
    if (!r.ok) return r;
  }
  if (sg.tracker_id != null) {
    const r = await invokeCmd<unknown>('assign_tracker_org', {
      args: { tracker_id: sg.tracker_id, org_id: org.value.id },
    });
    if (!r.ok) return r;
  }
  await loadOrgs();
  return org;
}
