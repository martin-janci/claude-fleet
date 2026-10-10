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

export type FlatGroupBy = 'state' | 'host' | 'agent';

export const FLAT_GROUP_BYS: readonly FlatGroupBy[] = ['state', 'host', 'agent'];

export function isFlatGroupBy(v: unknown): v is FlatGroupBy {
  return v === 'state' || v === 'host' || v === 'agent';
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
  else order = [...buckets.keys()].sort((a, b) => a.localeCompare(b));
  return order.map((k) => ({
    key: `${by}:${k}`,
    label:
      by === 'state'
        ? STATE_LABELS[k as AttentionState]
        : by === 'agent'
          ? (AGENT_LABELS[k as SessionAgent] ?? k)
          : k,
    rows: buckets.get(k)!,
  }));
}
