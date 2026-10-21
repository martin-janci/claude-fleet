// The app's wizards as chat forms (redesign 10.12): for each id an agent's
// `wizard` block or Control may open, what the card reads while it builds
// (the choices only known when it opens), the spec with those choices, and
// what its last button runs. The run is the same one the wizard's own
// screen makes; nothing here acts before the button.
import { get } from 'svelte/store';
import { hosts, discoverHosts } from '../hosts';
import { projects } from '../projects';
import { orgs, loadOrgs } from '../orgs';
import type { ChatFormOutcome } from './ChatForm.svelte';
import type { Values } from './forms';
import type { ChatWizardId } from './chat_wizard_ids';
import { WIZARDS, withChoices, type Wizard } from './wizards';
import { addHostWizard, runAddHost } from './add_host_wizard';
import { runAddProject } from './add_project_wizard';
import { newSessionWizard, runNewSession } from './new_session_wizard';
import { pairDeviceWizard, runPairDevice } from './pair_device_wizard';
import { getStartedWizard, runGetStarted } from './get_started_wizard';
import { fleetForm, fleetWizard, grantFleet, loadAgents } from '../agents';
import { errorText } from '../error_copy';

export interface ChatWizard {
  wizard: Wizard;
  run: (values: Values, onprogress?: (text: string) => void) => Promise<ChatFormOutcome>;
}

/** What the card says it reads while the choices load ("Building a form ·
 *  reading your hosts"). */
export const CHAT_WIZARD_READS: Record<ChatWizardId, string> = {
  add_host: 'your SSH config',
  add_project: 'your hosts',
  fleet_agent: 'the catalog and your hosts',
  get_started: 'your hosts and projects',
  new_session: 'your hosts and projects',
  pair_device: 'your orgs',
};

function visibleHosts(): { alias: string }[] {
  return get(hosts).filter((h) => !h.hidden);
}

function fleetProjects() {
  return get(projects).map((p) => p.project);
}

/** The Add project spec for the chat: the fleet's hosts; no GitHub list (a
 *  form cannot browse an owner), so "From GitHub" leaves the form. */
export function addProjectChatWizard(hostList: readonly { alias: string }[]): Wizard {
  const w = WIZARDS.add_project;
  const host: [string, string][] = hostList.slice(0, 50).map((h) => [h.alias, h.alias === 'local' ? 'This machine' : h.alias]);
  return { ...w, spec: withChoices(w.spec, { host: host.length ? host : [['local', 'This machine']], repos: [] }) };
}

/** A fleet agent's grant form, and its last button: the grant (each MCP
 *  store is asked for its tools on the host first; a failure is the card's
 *  error). Only a person's own device sends it. */
async function fleetAgentChatWizard(definition: string | null | undefined): Promise<ChatWizard> {
  if (!definition) throw new Error('the block names no fleet agent');
  const r = await fleetForm(definition);
  if (!r.ok) throw new Error(errorText(r.error));
  return {
    wizard: fleetWizard(r.value),
    run: async (values, onprogress) => {
      onprogress?.('Checking its stores on the host…');
      const g = await grantFleet(definition, values);
      if (!g.ok) return { ok: false, error: errorText(g.error) };
      void loadAgents();
      return { ok: true, summary: `${definition} instance ${String(values.f_instance ?? '')} started` };
    },
  };
}

/** The wizard `id` with its choices, ready to show; `definition` names a
 *  `fleet_agent`'s catalog agent. */
export async function chatWizard(id: ChatWizardId, definition?: string | null): Promise<ChatWizard> {
  switch (id) {
    case 'fleet_agent':
      return fleetAgentChatWizard(definition);
    case 'add_host': {
      const r = await discoverHosts();
      return { wizard: addHostWizard(r.ok ? r.value : []), run: runAddHost };
    }
    case 'add_project':
      return { wizard: addProjectChatWizard(visibleHosts()), run: runAddProject };
    case 'new_session':
      return { wizard: newSessionWizard({ projects: fleetProjects(), hosts: visibleHosts() }), run: runNewSession };
    case 'get_started':
      return { wizard: getStartedWizard({ projects: fleetProjects(), hosts: visibleHosts() }), run: runGetStarted };
    case 'pair_device': {
      if (get(orgs).length === 0) await loadOrgs();
      return { wizard: pairDeviceWizard(get(orgs)), run: runPairDevice };
    }
  }
}
