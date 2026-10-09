// The Sessions list's owner tabs and agent facet (Sessions board): "All 25 ·
// Mine · Shared" above the list, and "Agent: Codex" among the filters. Both
// are view filters for this window only, like Needs you: a list narrowed to
// one agent should not come back narrowed after a restart unnoticed.
import { writable } from 'svelte/store';
import { sessionAgent, type SessionAgent, type SessionRow } from './sessions';

/** All rows, the ones this person runs, or the ones shared with them. */
export type OwnerTab = 'all' | 'mine' | 'shared';

export const OWNER_TABS: readonly { id: OwnerTab; label: string }[] = [
  { id: 'all', label: 'All' },
  { id: 'mine', label: 'Mine' },
  { id: 'shared', label: 'Shared' },
];

export const ownerTab = writable<OwnerTab>('all');

/** `all`, or the one agent the list shows. */
export type AgentFilter = 'all' | SessionAgent;

export const agentFilter = writable<AgentFilter>('all');

/** The row passes the agent facet. */
export function agentMatches(s: Pick<SessionRow, 'agent' | 'kind'>, f: AgentFilter): boolean {
  return f === 'all' || sessionAgent(s) === f;
}

/** The tab counts: every row the list could show, and how many of them are
 *  shared with this person. */
export function ownerCounts(rows: readonly SessionRow[], shared: (s: SessionRow) => boolean): Record<OwnerTab, number> {
  let n = 0;
  for (const s of rows) if (shared(s)) n++;
  return { all: rows.length, mine: rows.length - n, shared: n };
}
