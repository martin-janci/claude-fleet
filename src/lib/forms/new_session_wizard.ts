// What the New session wizard's last button does (redesign step 10.12): the
// answers of `wizards/new_session.json` as one `new_session` call, the same
// one ⌘N's dialog makes, so the start shows its Pulse (5.13) wherever the
// wizard ran. The dialog keeps what a form cannot hold: the duplicate check,
// the account headroom ask, a ticket start and Cancel creation.
import { finalizeBranchSlug } from '../branch-slug';
import type { ProjectRow, WorktreeRow } from '../projects';
import { newSessionAbortable, type NewSessionArgs } from '../sessions';
import type { ChatFormOutcome } from './ChatForm.svelte';
import type { FieldProblem, Values } from './forms';
import { WIZARDS, withChoices, type Wizard } from './wizards';

/** The `worktree` value for a new worktree; an existing one is its id. */
export const NEW_WORKTREE = 'new';
/** The model and effort value that leaves the host's default. */
export const HOST_DEFAULT = 'default';

const PROFILE = /^[A-Za-z0-9][A-Za-z0-9_-]{0,31}$/;
// A form's select takes at most 50 options.
const MAX = 50;

export interface NewSessionChoices {
  projects: readonly Pick<ProjectRow, 'id' | 'owner' | 'repo'>[];
  hosts: readonly { alias: string }[];
  /** One project's worktrees on one host, when the wizard opens for them
   *  (a project's "+", a worktree's row); otherwise a new worktree only. */
  worktrees?: readonly Pick<WorktreeRow, 'id' | 'name' | 'branch'>[];
}

/** The wizard with the fleet's projects, hosts and, when given, worktrees. */
export function newSessionWizard(c: NewSessionChoices): Wizard {
  const projects: [string, string][] = c.projects.slice(0, MAX).map((p) => [String(p.id), `${p.owner}/${p.repo}`]);
  const hosts: [string, string][] = c.hosts.slice(0, MAX).map((h) => [h.alias, h.alias === 'local' ? 'This machine' : h.alias]);
  const worktrees: [string, string][] = [
    [NEW_WORKTREE, 'New worktree'],
    ...(c.worktrees ?? []).slice(0, MAX - 1).map((w): [string, string] => [String(w.id), w.branch ? `${w.name} · ${w.branch}` : w.name]),
  ];
  const w = WIZARDS.new_session;
  return { ...w, spec: withChoices(w.spec, { project: projects, host: hosts, worktree: worktrees }) };
}

/** The `new_session` arguments the answers ask for, or the fields to fix. */
export function newSessionArgs(v: Values): { args: NewSessionArgs } | { problems: FieldProblem[] } {
  const text = (k: string) => String(v[k] ?? '').trim();
  const agent = v.agent === 'codex' || v.agent === 'shell' ? v.agent : 'claude';
  const project = Number(v.project);
  const problems: FieldProblem[] = [];
  if (!Number.isInteger(project) || project <= 0) problems.push({ field: 'project', problem: 'Pick a project.' });
  if (!text('host')) problems.push({ field: 'host', problem: 'Pick a host.' });
  const profile = agent === 'claude' ? text('profile') : '';
  if (profile && !PROFILE.test(profile)) {
    problems.push({ field: 'profile', problem: 'Letters, digits, - and _, up to 32, starting with a letter or digit.' });
  }
  if (problems.length > 0) return { problems };
  const fresh = (v.worktree ?? NEW_WORKTREE) === NEW_WORKTREE;
  const pick = (k: string) => (agent === 'claude' && v[k] && v[k] !== HOST_DEFAULT ? String(v[k]) : null);
  return {
    args: {
      host_alias: text('host'),
      project_id: project,
      worktree_id: fresh ? null : Number(v.worktree),
      // Empty: the backend mints the tmux name (`fill_session_name`).
      name: '',
      new_worktree: fresh ? finalizeBranchSlug(text('branch')) || null : null,
      base_branch: fresh ? text('base_branch') || null : null,
      kind: agent === 'shell' ? 'shell' : 'work',
      agent,
      start_command: agent === 'shell' ? text('start_command') || null : null,
      friendly_name: text('label') || null,
      model: pick('model'),
      effort: pick('effort'),
      profile: profile || null,
    },
  };
}

/** Start it. The card's answered line names the session; the Pulse beside
 *  it says it is starting, and the session's own view follows the agent. */
export async function runNewSession(v: Values): Promise<ChatFormOutcome> {
  const a = newSessionArgs(v);
  if ('problems' in a) return { ok: false, problems: a.problems };
  const r = await newSessionAbortable(a.args);
  if (!r.ok) return { ok: false, error: r.error.message };
  const name = r.value.friendly_name ?? r.value.tmux_name;
  return { ok: true, summary: `${name} on ${r.value.host_alias}`, starting: `Starting ${name} on ${r.value.host_alias}` };
}
