// Wizards as forms (redesign step 10.12): each wizard is one fleet.form/1
// spec in `wizards/<id>.json`, the same format an agent's `ask { form }`
// uses, so the one spec renders as a dialog (WizardDialog) or as a card in
// the chat (ChatForm), and fleet-core validates every file here
// (`every_wizard_spec_is_valid`). What the last button does stays with the
// screen that opens the wizard: the spec is data only.
import type { LoaderName } from '../loader-kit.generated';
import type { FormSpec } from './forms';
import linkHub from './wizards/link_hub.json';
import addProject from './wizards/add_project.json';
import addHost from './wizards/add_host.json';
import pairDevice from './wizards/pair_device.json';
import newSession from './wizards/new_session.json';
import getStarted from './wizards/get_started.json';
import linkPeer from './wizards/link_peer.json';

export type WizardId =
  | 'link_hub'
  | 'add_project'
  | 'add_host'
  | 'pair_device'
  | 'new_session'
  | 'get_started'
  | 'link_peer';

export interface Wizard {
  id: WizardId;
  spec: FormSpec;
  /** The last button while it runs, with a Comet before it. */
  sending: string;
  /** The wizard's own loader while the last step runs (the manual's
   *  LoadersInFlows board); shown in the body, never over the screen. */
  loader: LoaderName;
}

export const WIZARDS: Record<WizardId, Wizard> = {
  link_hub: { id: 'link_hub', spec: linkHub as FormSpec, sending: 'Pairing…', loader: 'counter-orbit' },
  // In the chat (an agent's `wizard` block, Control's Add project): 6.11's
  // dialog keeps its GitHub browser (owner picker, "already in fleet"),
  // which a form cannot hold.
  add_project: { id: 'add_project', spec: addProject as FormSpec, sending: 'Adding…', loader: 'comet' },
  // The one-step Add host, for the chat and Get started: 4.9's guided
  // wizard keeps its drafts and per-step checks, which a form cannot hold.
  add_host: { id: 'add_host', spec: addHost as FormSpec, sending: 'Checking…', loader: 'sonar' },
  // Settings › Devices' Pair a device.
  pair_device: { id: 'pair_device', spec: pairDevice as FormSpec, sending: 'Pairing…', loader: 'halo' },
  // ⌘N's start as a form, for the chat and Get started; its answered line
  // carries the Pulse while the agent comes up (5.13).
  new_session: { id: 'new_session', spec: newSession as FormSpec, sending: 'Starting…', loader: 'pulse-sequence' },
  // 10.5's Get started as one form: a host, a project and the first
  // session; the Galaxy while the first fleet is built (10.10).
  get_started: { id: 'get_started', spec: getStarted as FormSpec, sending: 'Building…', loader: 'galaxy' },
  // Settings › Federation's Link a hub (11.12): the
  // Counter-orbit runs while the two hubs trade keys (`link_peer_hub`
  // redeems the code). Nothing waits on a person once it runs: the other
  // side's operator minted the code before, so no step says "waiting for".
  link_peer: { id: 'link_peer', spec: linkPeer as FormSpec, sending: 'Linking…', loader: 'counter-orbit' },
};

/** `spec` with the choices only known when it opens (the fleet's hosts, a
 *  GitHub owner's repositories) in place of the file's example options. A
 *  choice given no options leaves the form, with the option and the step
 *  that lead to it: Add project with no repositories to offer has no From
 *  GitHub. */
export function withChoices(spec: FormSpec, choices: Record<string, [string, string][]>): FormSpec {
  const empty = (name: string) => choices[name]?.length === 0;
  // A step left with no field goes; so does the option that led to it.
  const gone = new Set<string>();
  const steps = spec.steps.flatMap((s) => {
    const fields = s.fields.filter((f) => !empty(f.name));
    if (fields.length > 0) return [{ ...s, fields }];
    if (s.when?.field !== undefined && typeof s.when.eq === 'string') gone.add(`${s.when.field}=${s.when.eq}`);
    return [];
  });
  return {
    ...spec,
    steps: steps.map((s) => ({
      ...s,
      fields: s.fields.map((f) => {
        if (!f.options) return f;
        const options = choices[f.name] ?? f.options.filter(([v]) => !gone.has(`${f.name}=${v}`));
        const kept = f.value === undefined || options.some(([v]) => v === f.value);
        return { ...f, options, value: kept ? f.value : undefined };
      }),
    })),
  };
}
