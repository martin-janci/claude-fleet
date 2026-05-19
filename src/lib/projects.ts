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
