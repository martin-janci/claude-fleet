// What the Add project wizard's last button does (redesign step 10.12): the
// answers of `wizards/add_project.json` as `add_project` calls, one per
// repository, in turn. The first failure stops the rest and says what was
// already added, as 6.11's dialog does.
import { addProject, type AddProjectSource } from '../projects';
import type { ChatFormOutcome } from './ChatForm.svelte';
import type { Values } from './forms';

/** The host and the sources the answers ask for. */
export function addProjectSources(values: Values): { host: string; sources: AddProjectSource[] } {
  const host = String(values.host ?? '');
  const text = (k: string) => String(values[k] ?? '').trim();
  switch (values.source) {
    case 'github':
      return { host, sources: (Array.isArray(values.repos) ? values.repos : []).map((r) => ({ kind: 'clone', url: String(r) })) };
    case 'clone':
      return { host, sources: [{ kind: 'clone', url: text('url') }] };
    case 'folder':
      return { host, sources: [{ kind: 'folder', path: text('path') }] };
    case 'new':
      // Local only: creating it on GitHub needs the dialog's confirmation.
      return { host, sources: [{ kind: 'new', owner: text('owner'), repo: text('repo'), create_remote: false }] };
    default:
      return { host, sources: [] };
  }
}

export async function runAddProject(values: Values): Promise<ChatFormOutcome> {
  const { host, sources } = addProjectSources(values);
  const added: string[] = [];
  for (const s of sources) {
    const r = await addProject(host, s);
    if (!r.ok) {
      const before = added.length ? `Added ${added.join(', ')}. ` : '';
      return { ok: false, error: `${before}${r.error.message}` };
    }
    added.push(`${r.value.project.owner}/${r.value.project.repo}`);
  }
  return { ok: true, summary: `${added.join(', ')} on ${host}` };
}
