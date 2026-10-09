// What the Pair a device wizard's last button does (redesign step 10.12):
// `pair_device`, whose answer (the code, the URL and its QR) PairingResult
// shows with its Halo. Nothing is minted before the button.
import { invokeCmd, type Result } from '../../result';
import type { Pairing } from '../../devices';
import type { OrgRow } from '../../orgs';
import type { Values } from '../forms';
import { WIZARDS, withChoices, type Wizard } from '../wizards';

/** The org select's "none". An org's value is its id, so never `none`. */
export const NO_ORG = 'none';

/** The wizard with the orgs a device can be bound to; none drops the field. */
export function pairDeviceWizard(orgs: readonly Pick<OrgRow, 'id' | 'name'>[]): Wizard {
  const picks: [string, string][] = orgs.slice(0, 49).map((o) => [String(o.id), o.name]);
  return withChoices(WIZARDS.pair_device, 'org', picks.length ? [[NO_ORG, 'No org'], ...picks] : []);
}

/** `pair_device`'s arguments from the answers. */
export function pairDeviceArgs(v: Values): Record<string, unknown> {
  const org = typeof v.org === 'string' && v.org !== NO_ORG && /^\d+$/.test(v.org) ? Number(v.org) : null;
  const person = String(v.person ?? '').trim();
  return {
    device: String(v.device ?? '').trim(),
    mode: v.mode === 'readonly' ? 'readonly' : 'full',
    org_id: org,
    person: person || null,
  };
}

export function runPairDevice(v: Values): Promise<Result<Pairing>> {
  return invokeCmd<Pairing>('pair_device', { args: pairDeviceArgs(v) });
}
