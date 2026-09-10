import { writable, derived } from 'svelte/store';
import { invokeCmd, type Result } from './result';

export interface ProjectRow {
  id: number;
  owner: string;
  repo: string;
  base_path: string;
  last_session_at: number | null;
}

export interface WorktreeRow {
  id: number;
  project_id: number;
  name: string;
  path: string;
  branch: string | null;
}

export interface ProjectTreeRow {
  project: ProjectRow;
  worktrees: WorktreeRow[];
}

export const projects = writable<ProjectTreeRow[]>([]);

/** O(1) project-id -> tree-row lookup, derived once per `projects` change. */
export const projectById = derived(projects, ($p) => new Map($p.map((p) => [p.project.id, p])));

export async function loadProjects(): Promise<Result<ProjectTreeRow[]>> {
  const r = await invokeCmd<ProjectTreeRow[]>('list_projects');
  if (r.ok) projects.set(r.value);
  return r;
}

export async function refreshProjects(): Promise<Result<ProjectTreeRow[]>> {
  const r = await invokeCmd<ProjectTreeRow[]>('refresh_projects');
  if (r.ok) projects.set(r.value);
  return r;
}

export async function bootstrapProjects(): Promise<Result<ProjectTreeRow[]>> {
  const r = await invokeCmd<ProjectTreeRow[]>('list_projects');
  if (r.ok) projects.set(r.value);
  return r;
}

export function mergeProject(row: ProjectTreeRow): void {
  projects.update((arr) => {
    const i = arr.findIndex((p) => p.project.id === row.project.id);
    if (i === -1) return [...arr, row];
    const next = arr.slice();
    next[i] = row;
    return next;
  });
}

/**
 * Adapter for the `project:updated` Tauri event, which emits a bare `ProjectRow`
 * (no nested worktrees — those have their own `worktree:updated` event). If the
 * project is already in the store, update only its `project` field and preserve
 * the existing `worktrees` array. If it's new, seed an empty `worktrees: []`.
 */
function mergeProjectRow(arr: ProjectTreeRow[], row: ProjectRow): ProjectTreeRow[] {
  const i = arr.findIndex((p) => p.project.id === row.id);
  if (i === -1) return [...arr, { project: row, worktrees: [] }];
  const next = arr.slice();
  next[i] = { project: row, worktrees: arr[i].worktrees };
  return next;
}

function mergeWorktreeRow(arr: ProjectTreeRow[], row: WorktreeRow): ProjectTreeRow[] {
  const idx = arr.findIndex((p) => p.project.id === row.project_id);
  if (idx === -1) return arr;
  const entry = arr[idx];
  const wts = entry.worktrees ?? [];
  const wIdx = wts.findIndex((w) => w.id === row.id);
  const newWts =
    wIdx === -1 ? [...wts, row] : wts.map((w) => (w.id === row.id ? row : w));
  const next = arr.slice();
  next[idx] = { ...entry, worktrees: newWts };
  return next;
}

function removeWorktreeRow(arr: ProjectTreeRow[], id: number): ProjectTreeRow[] {
  if (!arr.some((entry) => entry.worktrees?.some((w) => w.id === id))) return arr;
  return arr.map((entry) => {
    if (!entry.worktrees?.some((w) => w.id === id)) return entry;
    return { ...entry, worktrees: entry.worktrees.filter((w) => w.id !== id) };
  });
}

export function mergeProjectFromEvent(row: ProjectRow): void {
  projects.update((arr) => mergeProjectRow(arr, row));
}

export function mergeWorktree(row: WorktreeRow): void {
  projects.update((arr) => mergeWorktreeRow(arr, row));
}

export function removeWorktree(id: number): void {
  projects.update((arr) => removeWorktreeRow(arr, id));
}

/** One backend project/worktree event, as delivered by `events.ts`. Both
 *  kinds land in the same `projects` store, so they batch together. */
export type ProjectEvent =
  | { type: 'project_updated'; row: ProjectRow }
  | { type: 'worktree_updated'; row: WorktreeRow }
  | { type: 'worktree_removed'; id: number };

/** Apply a burst of project + worktree events in ONE store update, in order. */
export function applyProjectEvents(events: readonly ProjectEvent[]): void {
  if (events.length === 0) return;
  projects.update((arr) => {
    let next = arr;
    for (const ev of events) {
      if (ev.type === 'project_updated') next = mergeProjectRow(next, ev.row);
      else if (ev.type === 'worktree_updated') next = mergeWorktreeRow(next, ev.row);
      else next = removeWorktreeRow(next, ev.id);
    }
    return next;
  });
}

export interface WorktreeOccupant {
  host_alias: string;
  tmux_name: string;
}

export interface WorktreeOccupancy {
  worktree: WorktreeRow;
  occupants: WorktreeOccupant[];
}

/** List every worktree fleet knows about, each tagged with the alive Claude
 *  sessions currently using it. Pass `projectId` to scope to one project. */
export async function listWorktreeOccupancy(
  projectId: number | null = null,
): Promise<Result<WorktreeOccupancy[]>> {
  return invokeCmd<WorktreeOccupancy[]>('list_worktrees', {
    args: { project_id: projectId },
  });
}

/** Delete a git worktree on its host and drop the fleet row. The backend
 *  refuses (`E_WORKTREE_BUSY`) when an alive session uses it unless `force`. */
export async function deleteWorktree(
  worktreeId: number,
  force = false,
): Promise<Result<void>> {
  const r = await invokeCmd<void>('delete_worktree', {
    args: { worktree_id: worktreeId, force },
  });
  if (r.ok) removeWorktree(worktreeId);
  return r;
}
