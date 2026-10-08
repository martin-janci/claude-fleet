// Wizards as forms (redesign step 10.12): each wizard is one fleet.form/1
// spec in `wizards/<id>.json`, the same format an agent's `ask { form }`
// uses, so the one spec renders as a dialog (WizardDialog) or as a card in
// the chat (ChatForm), and fleet-core validates every file here
// (`every_wizard_spec_is_valid`). What the last button does stays with the
// screen that opens the wizard: the spec is data only.
import type { LoaderName } from '../loader-kit.generated';
import type { FormSpec } from './forms';
import linkHub from './wizards/link_hub.json';

export type WizardId = 'link_hub';

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
};
