// The sessions this window is starting (redesign step 5.13): what the Pulse
// sequence follows. `creatingStart` is a ⌘N start whose create command is
// still in flight (its worktree and tmux steps run inside it);
// `startingSessions` holds the rows it made until their agent reports a
// status. Both move only on events (the command returning, a row update);
// nothing here times out.
import { derived, get, type Readable } from 'svelte/store';
import { sameSet, stable } from './stable_store';
import { creatingStart, sessions, startedIds } from './sessions';

export { creatingStart };

/** Rows still starting: their agent has no status yet. A row that gained
 *  one, or went away, drops out on the next row event. */
// `stable`: every row reads this, and a new equal Set on each row event
// re-rendered all of them on every flush (review r16).
export const startingSessions: Readable<ReadonlySet<number>> = stable(
  derived([startedIds, sessions], ([$ids, $rows]) => {
    const out = new Set<number>();
    if ($ids.size === 0) return out as ReadonlySet<number>;
    for (const id of $ids) {
      const row = $rows.find((r) => r.id === id);
      if (row && row.claude_status === null) out.add(id);
    }
    return out as ReadonlySet<number>;
  }),
  sameSet,
);

// Forget the ids that finished, so the set never grows.
startingSessions.subscribe(($s) => {
  const ids = get(startedIds);
  if ([...ids].some((id) => !$s.has(id))) startedIds.set(new Set($s));
});

/** Test hook. */
export function resetStarting(): void {
  startedIds.set(new Set());
  creatingStart.set(null);
}
