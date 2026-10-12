// The New session picker's per-project choices (project picker spec v2):
// pinned, visibility (hide | keep) and the picker group. Keyed by
// `owner/repo`, never the project id — the backend re-creates project rows
// (review C22). Loaded at startup and whenever the switcher opens; writes
// patch this store optimistically.
import { get, writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';
import { push, pushError } from './toasts';

export type Vis = 'hide' | 'keep';

export interface ProjectPick {
  owner: string;
  repo: string;
  pinned: boolean;
  vis: Vis | null;
  grp: string | null;
}

export const pickKey = (owner: string, repo: string): string => `${owner}/${repo}`;

export const projectPicks = writable<ReadonlyMap<string, ProjectPick>>(new Map());

const previous = new Map<string, ProjectPick>();

/** The state before the last `setProjectPick` on this project (Undo). */
export function previousPick(owner: string, repo: string): ProjectPick | undefined {
  return previous.get(pickKey(owner, repo));
}

export async function loadProjectPicks(): Promise<Result<ProjectPick[]>> {
  const r = await invokeCmd<ProjectPick[]>('project_picks');
  // A hub older than this feature has no such tool: the picker then runs on
  // the rules alone, nothing pinned.
  if (r.ok && Array.isArray(r.value)) {
    projectPicks.set(new Map(r.value.map((p) => [pickKey(p.owner, p.repo), p])));
  }
  return r;
}

const EMPTY = (owner: string, repo: string): ProjectPick => ({
  owner,
  repo,
  pinned: false,
  vis: null,
  grp: null,
});

// An older hub has no `set_project_pick` tool (E_HUB_PROTOCOL) and a token
// it refuses gets E_FORBIDDEN: neither will change on a retry, so say it
// once per session rather than on every pin.
const UNSUPPORTED = new Set(['E_HUB_PROTOCOL', 'E_FORBIDDEN']);
let unsupportedSaid = false;

export function resetPickNoticeForTests(): void {
  unsupportedSaid = false;
}

/** Change some of one project's choices. The command is a full replace, so
 *  the fields not in `patch` are sent as they are now. Optimistic: the store
 *  changes at once and rolls back if the write fails — with a toast, unless
 *  `quiet` (a write the person did not ask for). */
export async function setProjectPick(
  owner: string,
  repo: string,
  patch: { pinned?: boolean; vis?: Vis | null; grp?: string | null },
  opts: { quiet?: boolean } = {},
): Promise<Result<ProjectPick>> {
  const key = pickKey(owner, repo);
  const before = get(projectPicks).get(key) ?? EMPTY(owner, repo);
  const next: ProjectPick = {
    owner,
    repo,
    pinned: patch.pinned ?? before.pinned,
    vis: patch.vis !== undefined ? patch.vis : before.vis,
    grp: patch.grp !== undefined ? patch.grp : before.grp,
  };
  previous.set(key, before);
  projectPicks.update((m) => new Map(m).set(key, next));
  const r = await invokeCmd<ProjectPick>('set_project_pick', {
    args: { owner, repo, pinned: next.pinned, vis: next.vis, grp: next.grp },
  });
  if (r.ok && r.value) {
    const saved = r.value;
    projectPicks.update((m) => new Map(m).set(key, saved));
  } else if (!r.ok) {
    projectPicks.update((m) => new Map(m).set(key, before));
    if (opts.quiet) return r;
    if (UNSUPPORTED.has(r.error.code)) {
      if (!unsupportedSaid) push({ kind: 'info', message: "This hub doesn't support project pins yet" });
      unsupportedSaid = true;
    } else {
      pushError(r.error, 'Could not save the project choice');
    }
  }
  return r;
}
