import { writable } from 'svelte/store';
import type { ProjectTreeRow } from './projects';

// Which project the user has clicked in the sidebar. Drives the center pane's
// detail view. `null` = nothing selected (show placeholder).
export const selectedProject = writable<ProjectTreeRow | null>(null);

export function selectProject(p: ProjectTreeRow | null): void {
  selectedProject.set(p);
}
