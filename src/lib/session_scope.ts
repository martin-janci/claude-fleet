// The Sessions list's scope tabs and agent facet (Sessions board, "Fleet all
// sessions"): All / Mine / Shared with me above the list, "Agent: any" beside
// Filters, and the line a shared row carries ("shared by Petra · can watch").
//
// Whose a row is comes from `access.ts` (owner, grant level, or unknown): this
// file only says which tab a level falls under and how a share reads. An
// unknown access (`null`: an unclaimed row, or a hub that has not answered)
// is neither mine nor shared, so it shows under All only.
import { writable } from 'svelte/store';
import type { GrantLevel, SessionAccess } from './access';
import type { OrgDetail } from './orgs';
import { readPref, writePref } from './prefs';
import { AGENT_LABELS } from './row_groups';
import type { SessionAgent, SessionRow } from './sessions';

export type ScopeTab = 'all' | 'mine' | 'shared';
export const SCOPE_TABS: readonly ScopeTab[] = ['all', 'mine', 'shared'];
export const SCOPE_TAB_LABELS: Record<ScopeTab, string> = {
  all: 'All',
  mine: 'Mine',
  shared: 'Shared with me',
};

const isScopeTab = (v: unknown): v is ScopeTab => v === 'all' || v === 'mine' || v === 'shared';
/** The chosen tab, persisted like the other list choices. */
export const scopeTab = writable<ScopeTab>(readPref('sessions.scope-tab', 'all', isScopeTab));
scopeTab.subscribe((v) => writePref('sessions.scope-tab', v));

/** Someone else's session this person was given: any grant level. */
export function isSharedAccess(a: SessionAccess): a is GrantLevel {
  return a === 'watch' || a === 'answer' || a === 'drive';
}

/** A row with access `a` shows under `tab`. */
export function inScopeTab(tab: ScopeTab, a: SessionAccess): boolean {
  if (tab === 'mine') return a === 'own';
  if (tab === 'shared') return isSharedAccess(a);
  return true;
}

/** The tabs' counts over the rows the list would show under All. */
export function scopeTabCounts(
  rows: readonly SessionRow[],
  accessOf: (s: SessionRow) => SessionAccess,
): Record<ScopeTab, number> {
  const out: Record<ScopeTab, number> = { all: rows.length, mine: 0, shared: 0 };
  for (const s of rows) {
    const a = accessOf(s);
    if (a === 'own') out.mine++;
    else if (isSharedAccess(a)) out.shared++;
  }
  return out;
}

/** What a share lets this person do, in the board's words. */
export const SHARE_LEVEL_WORDS: Record<GrantLevel, string> = {
  watch: 'can watch',
  answer: 'can answer',
  drive: 'can steer',
};

/** A person's name from the orgs' member lists, or null when no org this
 *  client can read lists them. */
export function personName(personId: number | null | undefined, orgs: readonly OrgDetail[]): string | null {
  if (personId == null) return null;
  for (const o of orgs) {
    const m = o.members?.find((x) => x.person_id === personId);
    if (m) return m.display_name || m.name;
  }
  return null;
}

/** A shared row's line: "Shared by Petra · can watch", or "Shared with you ·
 *  can watch" when the sharer's name is not known here. */
export function sharedByLine(
  row: Pick<SessionRow, 'owner_person_id'>,
  level: GrantLevel,
  orgs: readonly OrgDetail[],
): string {
  const who = personName(row.owner_person_id, orgs);
  return `${who ? `Shared by ${who}` : 'Shared with you'} · ${SHARE_LEVEL_WORDS[level]}`;
}

// ── Agent facet ──

export type AgentFilter = 'any' | SessionAgent;
export const AGENT_FILTERS: readonly AgentFilter[] = ['any', 'claude', 'codex', 'agy', 'shell'];
const isAgentFilter = (v: unknown): v is AgentFilter =>
  typeof v === 'string' && (AGENT_FILTERS as readonly string[]).includes(v);
/** "Agent: any" — persisted like the host filter. */
export const agentFilter = writable<AgentFilter>(readPref('sessions.agent-filter', 'any', isAgentFilter));
agentFilter.subscribe((v) => writePref('sessions.agent-filter', v));

export function agentFilterLabel(f: AgentFilter): string {
  return f === 'any' ? 'Any' : AGENT_LABELS[f];
}
