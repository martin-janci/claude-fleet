// "Show its count on the rail" (gap plan G2.2, board FormsWork "Save filters
// as a view"): one saved Work view whose task count the rail's Work item
// carries. Which view is a per-device choice (a pref, like the active view):
// the views themselves live on the hub, the count is read from the hub's own
// tree for that view's filters, so the number is what opening the view
// would list. The badge is quiet — a count to glance at, not a Needs you ask
// (that stays the Inbox's alone).
import { get, writable } from 'svelte/store';
import { readPref, writePref } from './prefs';
import { workTree, workViews } from './work_view';
import { onWorkChangedDebounced } from './work';

const isViewId = (v: unknown): v is number | null => v === null || (typeof v === 'number' && Number.isInteger(v));

/** The saved view whose count the rail shows (null: none). */
export const railWorkViewId = writable<number | null>(readPref('work.rail_view_id', null, isViewId));
railWorkViewId.subscribe((v) => writePref('work.rail_view_id', v));

/** That view's name and task count, once read (null: none, or not yet). */
export const railWorkView = writable<{ id: number; name: string; count: number } | null>(null);

let seq = 0;

/** Re-read the chosen view and its count. A read that fails keeps the last
 *  count; a view deleted elsewhere is dropped from the rail. */
export async function refreshRailWorkView(): Promise<void> {
  const mine = ++seq;
  const id = get(railWorkViewId);
  if (id == null) {
    railWorkView.set(null);
    return;
  }
  const vs = await workViews();
  if (mine !== seq || !vs.ok || !Array.isArray(vs.value)) return;
  const v = vs.value.find((x) => x.id === id);
  if (!v) {
    railWorkViewId.set(null);
    railWorkView.set(null);
    return;
  }
  const t = await workTree({ filters: v.filters, limit: 1, per_task: 0 });
  if (mine !== seq || !t.ok) return;
  railWorkView.set({ id: v.id, name: v.name, count: t.value?.total ?? 0 });
}

/** Keep the count current while the rail is on screen: on a new choice, and
 *  after work changes (debounced, at most every `maxWaitMs`). */
export function startRailWorkView(debounceMs = 1500, maxWaitMs = 10_000): () => void {
  const offId = railWorkViewId.subscribe(() => void refreshRailWorkView());
  const offChanged = onWorkChangedDebounced(
    () => void refreshRailWorkView(),
    () => debounceMs,
    () => maxWaitMs,
  );
  return () => {
    offId();
    offChanged();
  };
}
