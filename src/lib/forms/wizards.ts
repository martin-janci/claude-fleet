// Wizards as forms (redesign step 10.12): each wizard is one fleet.form/1
// spec in `wizards/<id>.json`, the same format an agent's `ask { form }`
// uses, so the one spec renders as a dialog (WizardDialog) or as a card in
// the chat (ChatForm), and fleet-core validates every file here
// (`every_wizard_spec_is_valid`). What the last button does stays with the
// screen that opens the wizard: the spec is data only.
import type { LoaderName } from '../loader-kit.generated';
import type { FormSpec } from './forms';
import addHost from './wizards/add_host.json';
import linkHub from './wizards/link_hub.json';
import pairDevice from './wizards/pair_device.json';

export type WizardId = 'add_host' | 'link_hub' | 'pair_device';

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
  add_host: { id: 'add_host', spec: addHost as FormSpec, sending: 'Checking…', loader: 'sonar' },
  link_hub: { id: 'link_hub', spec: linkHub as FormSpec, sending: 'Pairing…', loader: 'counter-orbit' },
  pair_device: { id: 'pair_device', spec: pairDevice as FormSpec, sending: 'Pairing…', loader: 'halo' },
};

/**
 * The wizard with one select's options replaced by what is only known when
 * it opens (the hosts in ~/.ssh/config, the hub's orgs). The spec on disk
 * carries a placeholder so it validates on its own. No options at all drops
 * the field, as nothing could be chosen; a default that is no longer offered
 * is dropped with it. The registry row is never changed.
 */
export function withChoices(wizard: Wizard, field: string, options: [string, string][]): Wizard {
  const steps = wizard.spec.steps.map((step) => ({
    ...step,
    fields: step.fields.flatMap((f) => {
      if (f.name !== field) return [f];
      if (options.length === 0) return [];
      const keep = f.value !== undefined && options.some(([v]) => v === f.value);
      return [{ ...f, options, value: keep ? f.value : undefined }];
    }),
  }));
  return { ...wizard, spec: { ...wizard.spec, steps } };
}
