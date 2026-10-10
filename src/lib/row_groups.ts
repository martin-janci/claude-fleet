// Redesign step 3.6: the session list grouped by state, host or agent. Project
// and work grouping keep their own trees in Sidebar.svelte (they carry project
// and work actions on the group header); these three are flat: a header with
// a count, then the rows in the order the list already had them.

import {
  ATTENTION_STATES,
  attentionState,
  type AttentionOptions,
  type AttentionState,
} from './attention';
import { get } from 'svelte/store';
import { sessionAgent, type SessionAgent, type SessionRow } from './sessions';
import { startingSessions } from './session_starting';
import { UNASSIGNED, type Scope, type ScopeId } from './orgs';

export type FlatGroupBy = 'state' | 'host' | 'agent' | 'org';

export const FLAT_GROUP_BYS: readonly FlatGroupBy[] = ['state', 'host', 'agent', 'org'];

export function isFlatGroupBy(v: unknown): v is FlatGroupBy {
  return v === 'state' || v === 'host' || v === 'agent' || v === 'org';
}

/** What grouping by organisation needs (Sessions board, "Group by ›
 *  Organisation"): each row's scope (its org, else its project's owner, else
 *  unassigned — `orgs.ts::scopeOfSession`) and the scopes' names. */
export interface OrgGrouping {
  scopeOf: (s: SessionRow) => ScopeId;
  scopes: readonly Scope[];
}

/** The status words of the design manual's content rules (step 7.8 lints
 *  them): one per attention state from step 0.4. The model keeps seven
 *  states, the manual six words: Blocked reads as Needs you, and its reason
 *  line says what it is blocked on (`blockedLine`, transition plan decision
 *  on status words). */
export const STATE_LABELS: Record<AttentionState, string> = {
  action_required: 'Needs you',
  failed: 'Failed',
  blocked: 'Needs you',
  working: 'Working',
  paused: 'Paused',
  done: 'Done',
  idle: 'Idle',
};

const AGENTS: readonly SessionAgent[] = ['claude', 'codex', 'agy', 'shell'];

/** The agent's own name, as the agent tab spells it. */
export const AGENT_LABELS: Record<SessionAgent, string> = {
  claude: 'Claude Code',
  codex: 'Codex',
  agy: 'Agy',
  shell: 'Shell',
};

export interface RowGroup {
  /** Stable across renders: what a collapsed group is remembered by. */
  key: string;
  label: string;
  rows: SessionRow[];
}

/**
 * Split `rows` into groups. States come in urgency order and agents in the
 * manual's order; hosts sort by name. Empty groups are left out, and each
 * group keeps the rows in the order they arrived.
 */
export function groupRows(
  rows: readonly SessionRow[],
  by: FlatGroupBy,
  opts: AttentionOptions,
  starting: ReadonlySet<number> = get(startingSessions),
  org: OrgGrouping = { scopeOf: () => UNASSIGNED, scopes: [] },
): RowGroup[] {
  const buckets = new Map<string, SessionRow[]>();
  const keyOf = (s: SessionRow): string => {
    if (by === 'state') {
      // Step 5.14: a session ⌘N just made lands under Working while its
      // agent comes up, rather than reading Idle before it has run at all.
      const st = attentionState(s, opts);
      // Blocked shares the Needs you group: one header per status word.
      if (st === 'blocked') return 'action_required';
      return st === 'idle' && starting.has(s.id) ? 'working' : st;
    }
    if (by === 'agent') return sessionAgent(s);
    if (by === 'org') return org.scopeOf(s);
    return s.host_alias;
  };
  for (const s of rows) {
    const k = keyOf(s);
    const list = buckets.get(k);
    if (list) list.push(s);
    else buckets.set(k, [s]);
  }
  let order: string[];
  if (by === 'state') order = ATTENTION_STATES.filter((st) => buckets.has(st));
  // An agent a newer hub knows and this build does not still gets a group,
  // last, under its own id: a row is never dropped for its agent.
  else if (by === 'agent')
    order = [
      ...AGENTS.filter((a) => buckets.has(a)),
      ...[...buckets.keys()].filter((k) => !AGENTS.includes(k as SessionAgent)),
    ];
  // Organisations in the selector's order (named orgs, then owners), any
  // scope it does not list yet next, and Unassigned last.
  else if (by === 'org') {
    const listed = org.scopes.map((sc) => sc.id).filter((id) => buckets.has(id));
    const rest = [...buckets.keys()].filter((k) => k !== UNASSIGNED && !listed.includes(k)).sort((a, b) => a.localeCompare(b));
    order = [...listed, ...rest, ...(buckets.has(UNASSIGNED) ? [UNASSIGNED] : [])];
  } else order = [...buckets.keys()].sort((a, b) => a.localeCompare(b));
  const orgLabel = (k: string): string =>
    k === UNASSIGNED ? 'Unassigned' : (org.scopes.find((sc) => sc.id === k)?.label ?? k.replace(/^(org|owner):/, ''));
  return order.map((k) => ({
    key: `${by}:${k}`,
    label:
      by === 'state'
        ? STATE_LABELS[k as AttentionState]
        : by === 'agent'
          ? (AGENT_LABELS[k as SessionAgent] ?? k)
          : by === 'org'
            ? orgLabel(k)
            : k,
    rows: buckets.get(k)!,
  }));
}

/** How many rows the Working group shows before "N more running ›"
 *  (Sessions board: two rows, then the rest behind one line). */
export const RUNNING_CAP = 2;

/** The capped groups: grouped by state, the Working group shows its first
 *  `RUNNING_CAP` rows and counts the rest, unless it was opened (`expanded`
 *  holds its key). A row in `keep` (the selected one) always shows, so the
 *  cap never hides where the person is. Every other group shows whole. */
export function capRows(
  g: RowGroup,
  expanded: ReadonlySet<string>,
  keep: ReadonlySet<number> = new Set(),
  cap: number = RUNNING_CAP,
): { shown: SessionRow[]; hidden: number } {
  if (g.key !== 'state:working' || expanded.has(g.key) || g.rows.length <= cap + 1) {
    return { shown: g.rows, hidden: 0 };
  }
  const shown = g.rows.filter((s, i) => i < cap || keep.has(s.id));
  return { shown, hidden: g.rows.length - shown.length };
}

/** The overflow line's words: "4 more running". */
export function moreRunningText(n: number): string {
  return `${n} more running`;
}
