// What the Get started wizard's last button does (redesign steps 10.5 and
// 10.12): the answers of `wizards/get_started.json` as the calls that build
// a first fleet, in order: the project (unless one in the fleet was picked),
// then the first session in a new worktree, the same `new_session` the New
// session wizard makes. Hosts are only picked, never added: a new machine
// joins from Hosts, which checks it first. Nothing runs before the button.
import { addProject, type AddProjectSource, type ProjectRow } from '../projects';
import type { SessionRow } from '../sessions';
import type { ChatFormOutcome } from './ChatForm.svelte';
import type { FieldProblem, Values } from './forms';
import { NEW_WORKTREE, startNewSession } from './new_session_wizard';
import { WIZARDS, withChoices, type Wizard } from './wizards';

// A form's select takes at most 50 options.
const MAX = 50;

export interface GetStartedChoices {
  projects: readonly Pick<ProjectRow, 'id' | 'owner' | 'repo'>[];
  hosts: readonly { alias: string }[];
}

/** The wizard with the fleet's hosts and projects. With no project yet,
 *  "One in the fleet" and its step leave the form. */
export function getStartedWizard(c: GetStartedChoices): Wizard {
  const hosts: [string, string][] = c.hosts.slice(0, MAX).map((h) => [h.alias, h.alias === 'local' ? 'This machine' : h.alias]);
  const projects: [string, string][] = c.projects.slice(0, MAX).map((p) => [String(p.id), `${p.owner}/${p.repo}`]);
  const w = WIZARDS.get_started;
  return { ...w, spec: withChoices(w.spec, { host: hosts.length ? hosts : [['local', 'This machine']], project: projects }) };
}

/** The project the answers add, or null for one already in the fleet. */
export function getStartedSource(v: Values): AddProjectSource | null {
  const text = (k: string) => String(v[k] ?? '').trim();
  if (v.source === 'folder') return { kind: 'folder', path: text('path') };
  if (v.source === 'existing') return null;
  return { kind: 'clone', url: text('url') };
}

/** Build it: the project, then the session. The dialog goes on to the row. */
export async function buildFirstFleet(
  v: Values,
): Promise<{ ok: true; row: SessionRow } | { ok: false; problems?: FieldProblem[]; error?: string }> {
  const host = String(v.host ?? '').trim();
  if (!host) return { ok: false, problems: [{ field: 'host', problem: 'Pick a host.' }] };
  const source = getStartedSource(v);
  let project = Number(v.project);
  if (source) {
    const r = await addProject(host, source);
    if (!r.ok) return { ok: false, error: r.error.message };
    project = r.value.project.id;
  } else if (!Number.isInteger(project) || project <= 0) {
    return { ok: false, problems: [{ field: 'project', problem: 'Pick a project.' }] };
  }
  const started = await startNewSession({
    project: String(project),
    host,
    agent: v.agent === 'codex' ? 'codex' : 'claude',
    worktree: NEW_WORKTREE,
    label: v.label ?? '',
  });
  if (started.ok) return started;
  // The project is in the fleet now: say so, so a retry picks it.
  const added = source ? 'The project was added; ' : '';
  return { ok: false, error: `${added}${started.error ?? started.problems?.map((p) => p.problem).join(' ') ?? 'the session did not start.'}` };
}

/** As a chat form: the answered line names the session, the Pulse says it starts. */
export async function runGetStarted(v: Values): Promise<ChatFormOutcome> {
  const r = await buildFirstFleet(v);
  if (!r.ok) return r;
  const name = r.row.friendly_name ?? r.row.tmux_name;
  return { ok: true, summary: `${name} on ${r.row.host_alias}`, starting: `Starting ${name} on ${r.row.host_alias}` };
}
