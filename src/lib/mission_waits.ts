// Missions waiting on a person (gap plan G1.6): the class of the one
// attention model that is a mission, not a session. The hub decides it
// (`service::attention::mission_waiting`, on `MissionRow::waiting_on`);
// the Inbox lists them under the sessions that need you, Today beside its
// Needs you, and the rail badge counts them, as it counts failed routines.
import { derived, writable } from 'svelte/store';
import { listMissions, type Mission, type MissionWait } from './missions';
import type { Result } from './result';
import { pollWhileShown } from './window_hidden';
import { ALL_SCOPES, effectiveScope, type ScopeId } from './orgs';

/** How often the list is read while the app shows (as failed routines). */
export const MISSION_WAITS_EVERY_MS = 60_000;

/** The missions that wait on a person, the longest waiting first. */
export function waitingOf(missions: readonly Mission[]): (Mission & { waiting_on: MissionWait })[] {
  return missions
    .filter((m): m is Mission & { waiting_on: MissionWait } => m.state === 'active' && m.waiting_on != null)
    .sort((a, b) => a.waiting_on.since - b.waiting_on.since || a.id - b.id);
}

/** "sign the autonomy grant", "answer its question", "3 to confirm". */
export function waitWords(w: MissionWait): string {
  switch (w.reason) {
    case 'question':
      return 'answer its question';
    case 'sign_grant':
      return 'sign the autonomy grant';
    case 'confirm':
      return `${w.open_cards} to confirm`;
  }
}

/** Every mission the hub says waits on a person, under no scope. */
export const waitingMissions = writable<(Mission & { waiting_on: MissionWait })[]>([]);

/** Whether a mission shows under `scope`, as Today scopes it (`scopeToday`):
 *  by its org; an owner scope or *unassigned* keeps the ones with none. */
export function missionInScope(m: Pick<Mission, 'org_id'>, scope: ScopeId): boolean {
  if (scope === ALL_SCOPES) return true;
  return scope.startsWith('org:') ? m.org_id === Number(scope.slice(4)) : m.org_id == null;
}

/** The waiting missions under the sidebar's scope (review w11-9): what the
 *  Inbox lists and the badge counts, as Today shows them. */
export const scopedWaitingMissions = derived([waitingMissions, effectiveScope], ([$m, $scope]) =>
  $m.filter((m) => missionInScope(m, $scope)),
);
export const waitingMissionCount = derived(scopedWaitingMissions, ($m) => $m.length);

export async function loadWaitingMissions(load: () => Promise<Result<Mission[]>> = listMissions): Promise<void> {
  const r = await load();
  waitingMissions.set(r.ok && Array.isArray(r.value) ? waitingOf(r.value) : []);
}

/** Keep `waitingMissions` fresh while the app shows; stops on the returned
 *  function. A hub that refuses the read (`E_INVALID`) is not asked again. */
export function trackWaitingMissions(
  deps: { load?: () => Promise<Result<Mission[]>>; doc?: Document | null; win?: Window | null; every?: number } = {},
): () => void {
  const load = deps.load ?? listMissions;
  let stopped = false;
  const poller = pollWhileShown(
    async () => {
      const r = await load();
      if (stopped) return false;
      waitingMissions.set(r.ok && Array.isArray(r.value) ? waitingOf(r.value) : []);
      return r.ok || r.error.code !== 'E_INVALID';
    },
    { every: deps.every ?? MISSION_WAITS_EVERY_MS, doc: deps.doc, win: deps.win },
  );
  return () => {
    stopped = true;
    poller.stop();
  };
}
