// Find-in-conversation and the turn index for the Conversations tab: pure
// helpers over the panel's thread rows (`buildThread`).
import type { ConvItem, ThreadRow } from './conversation';

/** The key the panel's `{#each thread}` uses for a row. */
export function rowKey(row: ThreadRow): string {
  return row.kind === 'turn' ? `t${row.index}` : `e${row.event.id}`;
}

export interface Match {
  rowKey: string;
}

function itemTexts(it: ConvItem): (string | null)[] {
  switch (it.kind) {
    case 'text':
      return [it.text];
    case 'tool':
      return [it.summary, it.target];
    case 'command':
      return [it.name, it.args, it.output];
    case 'subagent':
      return [it.description, it.result];
    case 'compact':
      return [it.summary];
    case 'notification':
      return [it.summary, it.event];
    case 'interrupt':
      return [];
  }
}

function rowTexts(row: ThreadRow): (string | null)[] {
  if (row.kind === 'event') return [row.event.label, row.event.detail];
  return [row.turn.prompt, ...row.turn.items.flatMap(itemTexts)];
}

/** Row keys (`t<index>` / `e<id>`) whose searchable text contains `query`
 *  (case-insensitive, trimmed; empty → []). Searchable text: prompt, text
 *  items, tool summaries/targets, command name/args/output, subagent
 *  description/result, compaction summary, event label/detail. In document
 *  order. */
export function findMatches(rows: ThreadRow[], query: string): Match[] {
  const q = query.trim().toLowerCase();
  if (q === '') return [];
  return rows
    .filter((row) => rowTexts(row).some((t) => t !== null && t.toLowerCase().includes(q)))
    .map((row) => ({ rowKey: rowKey(row) }));
}

export interface TurnIndexEntry {
  rowKey: string;
  label: string;
  at: string | null;
}

/** Longest turn-index label, ellipsis included. */
const INDEX_LABEL_MAX = 80;

/** One entry per turn that has a prompt or a command: the prompt's first
 *  line (≤ 80 chars, "…" when cut), or the command text ("/model opus"). */
export function turnIndex(rows: ThreadRow[]): TurnIndexEntry[] {
  const out: TurnIndexEntry[] = [];
  for (const row of rows) {
    if (row.kind !== 'turn') continue;
    const { turn } = row;
    let label: string | null = null;
    if (turn.prompt !== null && turn.prompt.trim() !== '') {
      const first = turn.prompt.trim().split('\n')[0].trim();
      label = first.length > INDEX_LABEL_MAX ? `${first.slice(0, INDEX_LABEL_MAX - 1)}…` : first;
    } else {
      const cmd = turn.items.find((i) => i.kind === 'command');
      if (cmd?.kind === 'command') label = cmd.args ? `${cmd.name} ${cmd.args}` : cmd.name;
    }
    if (label !== null) out.push({ rowKey: rowKey(row), label, at: turn.at });
  }
  return out;
}
