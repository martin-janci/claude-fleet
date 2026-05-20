import { writable } from 'svelte/store';
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

export async function bootstrapProjects(): Promise<void> {
  const r = await invokeCmd<ProjectTreeRow[]>('list_projects');
  if (r.ok) projects.set(r.value);
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

export function mergeWorktree(row: WorktreeRow): void {
  projects.update((arr) => {
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
  });
}
