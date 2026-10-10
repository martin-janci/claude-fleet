// The counts after the Work tabs (gap plan G3.3, the Work board's "Tasks 5",
// "Pull requests 3"): the open tasks the filters show, the missions not yet
// finished, and the open pull requests. Pure, but for `loadTabCounts`, which
// reads the two lists the tree does not.
import { isFinal, listMissions, type Mission } from './missions';
import { listPullRequests, type PrList } from './prs';
import type { Result } from './result';
import { stageOf } from './work_row';
import type { WorkTreePage } from './work_view';

export interface TabCounts {
  tasks: number | null;
  missions: number | null;
  prs: number | null;
}

/** The Tasks tab's count. The List layout reads archived tasks too, so it
 *  counts the rows not done (To do and Doing); the tree's page is already
 *  the open view, so its total is the count. `null` before a read. */
export function taskTabCount(page: Pick<WorkTreePage, 'tasks' | 'total'> | null, listMode: boolean): number | null {
  if (!page) return null;
  if (!listMode) return typeof page.total === 'number' ? page.total : null;
  return (page.tasks ?? []).filter((t) => stageOf(t) !== 'done').length;
}

/** Missions still moving: drafts, active and paused ones. */
export function missionTabCount(missions: readonly Pick<Mission, 'state'>[]): number {
  return missions.filter((m) => !isFinal(m.state)).length;
}

/** The two counts the tree does not read. A list the hub refuses leaves its
 *  count `null` (no number shown), never 0. */
export async function loadTabCounts(
  deps: {
    missions?: () => Promise<Result<Mission[]>>;
    prs?: () => Promise<Result<PrList>>;
  } = {},
): Promise<Pick<TabCounts, 'missions' | 'prs'>> {
  const [m, p] = await Promise.all([
    (deps.missions ?? listMissions)(),
    (deps.prs ?? (() => listPullRequests({ state: 'open', limit: 1 })))(),
  ]);
  return {
    missions: m.ok && Array.isArray(m.value) ? missionTabCount(m.value) : null,
    prs: p.ok && p.value ? (typeof p.value.total === 'number' ? p.value.total : (p.value.items ?? []).length) : null,
  };
}
