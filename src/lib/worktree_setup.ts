// New worktree setup (docs/work-graph.md#new-worktree-setup): the files a
// project's fresh worktree gets from its base checkout and the command run
// in it before the agent starts. Mirrors `service::work::worktree_setup`;
// `worktree_setup` is Routed, so a paired desktop sets its hub's projects.

import { invokeCmd, type Result } from './result';

export interface WorktreeSetupView {
  project_id: number;
  /** `owner/repo`. */
  project: string;
  copy: string[];
  command?: string | null;
}

/** At most this many globs (`store::SETUP_GLOBS_MAX`). */
export const SETUP_GLOBS_MAX = 32;

/** One glob per line; blank lines and repeats dropped, each trimmed. */
export function parseGlobs(text: string): string[] {
  const out: string[] = [];
  for (const line of text.split('\n')) {
    const g = line.trim();
    if (g && !out.includes(g)) out.push(g);
  }
  return out;
}

/** Why a glob is refused, as the backend's `check_setup_glob` says it; null
 *  when it is fine. Checked here too so the dialog says it before Save. */
export function globError(g: string): string | null {
  if (g.startsWith('/') || g.startsWith('~') || g.includes('\\')) return `“${g}” must be relative to the project folder`;
  if (g.split('/').includes('..')) return `“${g}” must stay inside the project folder`;
  if (g.split('/').includes('.git')) return `“${g}”: git's own files are never copied`;
  return null;
}

export async function getWorktreeSetup(projectId: number): Promise<Result<WorktreeSetupView>> {
  return invokeCmd<WorktreeSetupView>('worktree_setup', { args: { project_id: projectId } });
}

export async function setWorktreeSetup(
  projectId: number,
  copy: string[],
  command: string,
): Promise<Result<WorktreeSetupView>> {
  return invokeCmd<WorktreeSetupView>('worktree_setup', {
    args: { project_id: projectId, copy, command: command.trim() },
  });
}
