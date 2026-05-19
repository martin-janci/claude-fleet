import { writable } from 'svelte/store';
import type { ProjectTreeRow } from './projects';
import type { SessionRow } from './sessions';

// Two mutually-exclusive selection slots drive the center pane:
//   - selectedProject: the user clicked a project row in the sidebar.
//   - selectedSession: the user clicked a session row in the sidebar.
// Setting one clears the other so the center pane always has a single
// unambiguous focus.

export const selectedProject = writable<ProjectTreeRow | null>(null);
export const selectedSession = writable<SessionRow | null>(null);

export function selectProject(p: ProjectTreeRow | null): void {
  selectedProject.set(p);
  if (p !== null) selectedSession.set(null);
}

export function selectSession(s: SessionRow | null): void {
  selectedSession.set(s);
  if (s !== null) selectedProject.set(null);
}

export function clearSelection(): void {
  selectedProject.set(null);
  selectedSession.set(null);
}
