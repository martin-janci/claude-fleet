// Find-in-conversation and the turn index for the Conversations tab: pure
// helpers over the panel's thread rows (`buildThread`).
import { get } from 'svelte/store';
import { sessions, sessionsLoaded } from './sessions';
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
    case 'bash':
      return [it.command, it.stdout, it.stderr];
    case 'harness':
      return [it.tag, it.body];
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

// ---- Scroll memory -------------------------------------------------------
//
// Where the panel was scrolled to in each session's conversation view, kept
// for the app's lifetime (one Map, not persisted) so switching to another
// session and back restores the read position instead of snapping to the
// bottom every time.

export interface ScrollSnapshot {
  /** Content anchor: the `at` of the turn the remembered row belongs to.
   *  The row KEY cannot be the anchor — `t<i>` is a position inside the
   *  loaded window, so the same key names a different turn as soon as the
   *  tail moves or "Load older" grows the window. A timestamp names the
   *  turn itself, whatever window it lands in next time. Null when no turn
   *  at-or-before the row carries one; the restore then stays pinned. */
  turnAt: string | null;
  /** The key the row had when the snapshot was taken. Window-relative for a
   *  turn (see `turnAt`), but stable for an inline event (`e<id>` is the
   *  backend's own event id), which is why an event row is restored by it. */
  rowKey: string;
  atBottom: boolean;
}

export const scrollMemory = new Map<number, ScrollSnapshot>();

// A session that leaves the store (killed, removed, or gone from the fleet)
// can never be returned to, so its remembered position is dead weight — and
// worse, a NEW session could one day reuse the id and inherit it. Prune on
// every store write, but only once the first list has landed: before that an
// empty store means "not loaded yet", not "no sessions".
sessions.subscribe((rows) => {
  if (scrollMemory.size === 0 || !get(sessionsLoaded)) return;
  const live = new Set(rows.map((r) => r.id));
  for (const id of [...scrollMemory.keys()]) if (!live.has(id)) scrollMemory.delete(id);
});

/** Records where session `sessionId`'s view was left. A snapshot at the
 *  bottom is dropped rather than stored: recalling "no entry" already means
 *  "go to the bottom", which is the panel's default view. */
export function rememberScroll(sessionId: number, snapshot: ScrollSnapshot): void {
  if (snapshot.atBottom) {
    scrollMemory.delete(sessionId);
    return;
  }
  scrollMemory.set(sessionId, snapshot);
}

/** The remembered snapshot for `sessionId`, or null when there is none
 *  (never scrolled away from the bottom, or never visited). */
export function recallScroll(sessionId: number): ScrollSnapshot | null {
  return scrollMemory.get(sessionId) ?? null;
}

/** Forget where `sessionId` was left: the conversation on screen is being
 *  replaced (a /clear or /resume the session followed, the header's
 *  conversation switcher), so a position inside the old transcript would
 *  restore into unrelated content. */
export function forgetScroll(sessionId: number): void {
  scrollMemory.delete(sessionId);
}

/** The content anchor for a read position: the `at` of the index entry
 *  nearest at-or-before `turnKey` (`nearestTurn`'s rule). Null when the
 *  index is empty, the key is null, or that turn carries no timestamp. */
export function anchorAt(index: TurnIndexEntry[], turnKey: string | null): string | null {
  if (turnKey === null || index.length === 0) return null;
  return index[nearestTurn(index, turnKey)]?.at ?? null;
}

/** Where a returning session should scroll, resolved against the FRESHLY
 *  built index: the current key of the turn the snapshot was anchored to.
 *  An inline event keeps its own key (`e<id>` is window-independent).
 *  Null when nothing matches — the caller then leaves the view pinned to
 *  the bottom rather than scrolling somewhere arbitrary. */
export function resolveScroll(index: TurnIndexEntry[], snap: ScrollSnapshot): string | null {
  if (/^e\d+$/.test(snap.rowKey)) return snap.rowKey;
  if (snap.turnAt === null) return null;
  return index.find((e) => e.at === snap.turnAt)?.rowKey ?? null;
}

// ---- Turn stepper ---------------------------------------------------------

/** Position in `index` of the turn at or immediately before `topVisibleKey`
 *  (an exact match "contains" it; otherwise the closest earlier turn).
 *  0 when `topVisibleKey` is null, matches nothing, or isn't a turn row key
 *  (e.g. it names an inline event) — the safe "unknown" fallback is the
 *  first turn. */
export function nearestTurn(index: TurnIndexEntry[], topVisibleKey: string | null): number {
  if (topVisibleKey === null || index.length === 0) return 0;
  const m = /^t(\d+)$/.exec(topVisibleKey);
  if (!m) return 0;
  const target = Number(m[1]);
  let best = 0;
  for (let i = 0; i < index.length; i++) {
    const em = /^t(\d+)$/.exec(index[i].rowKey);
    const n = em ? Number(em[1]) : null;
    if (n !== null && n <= target) best = i;
    else break;
  }
  return best;
}

/** The turn-row key a read position resolves to. `key` itself when it
 *  already names a turn row; an inline event has no turn of its own, so the
 *  nearest `t…` in `dir` (1 = the turn BELOW it, -1 = the turn above) in
 *  document order, falling back to the other direction when that side has
 *  none. Null when `key` is null/unknown or `keys` holds no turn row.
 *
 *  Stepping forward matters: resolving an event row through `nearestTurn`
 *  alone yields 0 — the first turn — so `]` from an event jumped the reader
 *  to the top of the conversation instead of to the next turn.
 */
export function turnKeyNear(keys: string[], key: string | null, dir: 1 | -1): string | null {
  if (key === null) return null;
  const i = keys.indexOf(key);
  if (i < 0) return null;
  const isTurn = (k: string) => /^t\d+$/.test(k);
  if (isTurn(key)) return key;
  const below = keys.slice(i + 1).find(isTurn) ?? null;
  const above = keys.slice(0, i).reverse().find(isTurn) ?? null;
  return dir === 1 ? (below ?? above) : (above ?? below);
}

/** The turn one step (`delta`) away from `current` (a position in `index`),
 *  or null past either end. */
export function adjacentTurn(index: TurnIndexEntry[], current: number, delta: 1 | -1): TurnIndexEntry | null {
  const next = current + delta;
  return next >= 0 && next < index.length ? index[next] : null;
}
