// Projects on the hub (Settings › Hub & sync, gap plan G4.6): on a paired
// desktop, which of the hub's projects this desktop lists and offers to start
// sessions in. A choice of this device only; the hub and every other device
// are unchanged, and a session already running in a project left out stays
// listed with its host.
import { derived, writable, type Readable } from 'svelte/store';
import { readPref, writePref } from './prefs';
import { hubStatus } from './hub';

/** Picked project ids by hub URL; a hub with no entry shows every project. */
export type HubProjectPicks = Record<string, number[]>;

const isPicks = (v: unknown): v is HubProjectPicks =>
  typeof v === 'object' &&
  v !== null &&
  !Array.isArray(v) &&
  Object.values(v).every((ids) => Array.isArray(ids) && ids.every((n) => Number.isInteger(n)));

export const hubProjectPicks = writable<HubProjectPicks>(readPref<HubProjectPicks>('ui.hubProjects', {}, isPicks));
hubProjectPicks.subscribe((v) => writePref('ui.hubProjects', v));

/** The ids this desktop lists from its hub, or null for all of them (not
 *  paired, or nothing picked). */
export const hubProjectFilter: Readable<ReadonlySet<number> | null> = derived(
  [hubProjectPicks, hubStatus],
  ([$picks, $hub]) => {
    if (!$hub.remote || !$hub.url) return null;
    const ids = $picks[$hub.url];
    return ids ? new Set(ids) : null;
  },
);

/** `rows` under `filter`. A pick that names none of the listed projects (a
 *  hub whose projects were all removed or renumbered) shows them all rather
 *  than an empty fleet. */
export function filterHubProjects<T extends { project: { id: number } }>(
  rows: readonly T[],
  filter: ReadonlySet<number> | null,
): T[] {
  if (!filter) return rows as T[];
  const kept = rows.filter((r) => filter.has(r.project.id));
  return kept.length > 0 || rows.length === 0 ? kept : (rows as T[]);
}

/** Save the pick for `url`; `null` (or every project) goes back to all. */
export function pickHubProjects(url: string, ids: number[] | null, total: number): void {
  hubProjectPicks.update((cur) => {
    const next = { ...cur };
    if (ids === null || ids.length >= total) delete next[url];
    else next[url] = [...new Set(ids)].sort((a, b) => a - b);
    return next;
  });
}

/** "6 of 9", or "All 9". */
export function hubProjectsLabel(shown: number, total: number): string {
  return shown >= total ? `All ${total}` : `${shown} of ${total}`;
}
