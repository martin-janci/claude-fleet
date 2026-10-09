// What the Add host wizard's last button does (redesign step 10.12): the
// same checks the guided wizard (4.9) runs, in its order, each reported as
// it answers, then add the host. Nothing runs before the button; a host SSH
// cannot reach is not added, every other check only informs.
import { addHost, type HostRow, type SshHost } from '../../hosts';
import { CHECK_KEYS, checkLoader, discardHostSetup, runHostSetupCheck, type SetupCheck } from '../../add_host_wizard';
import type { Result } from '../../result';
import type { Values } from '../forms';
import { WIZARDS, withChoices, type Wizard } from '../wizards';

/** The `pick` value that means "type the alias". An SSH alias is never `*`
 *  (that is a config wildcard, which discovery leaves out). */
export const TYPED = '*';

/** The wizard with the hosts from ~/.ssh/config to pick from. */
export function addHostWizard(discovered: readonly SshHost[]): Wizard {
  const picks: [string, string][] = discovered
    .filter((h) => h.alias !== TYPED)
    .slice(0, 49)
    .map((h) => [h.alias, h.hostname ? `${h.alias} · ${h.user ? `${h.user}@` : ''}${h.hostname}` : h.alias]);
  return withChoices(WIZARDS.add_host, 'pick', [...picks, [TYPED, 'Another alias']]);
}

/** The SSH alias and the fleet name the answers name. */
export function addHostTarget(v: Values): { sshAlias: string; alias: string } {
  const picked = typeof v.pick === 'string' ? v.pick : TYPED;
  const sshAlias = (picked === TYPED ? String(v.ssh_alias ?? '') : picked).trim();
  const alias = String(v.alias ?? '').trim() || sshAlias;
  return { sshAlias, alias };
}

export interface AddHostOutcome {
  host: HostRow;
  checks: SetupCheck[];
}

/** Check, then add. `onprogress` gets the line beside the wizard's loader
 *  while each check waits. */
export async function runAddHost(v: Values, onprogress?: (text: string) => void): Promise<Result<AddHostOutcome>> {
  const { sshAlias, alias } = addHostTarget(v);
  if (!sshAlias) return { ok: false, error: { code: 'E_VALIDATE', message: 'Name the SSH alias to add.' } };
  const checks: SetupCheck[] = [];
  for (const key of CHECK_KEYS) {
    onprogress?.(checkLoader(key, sshAlias)!.text);
    const r = await runHostSetupCheck(sshAlias, key);
    const c: SetupCheck = r.ok ? r.value : { key, state: 'fail', label: key, detail: r.error.message };
    checks.push(c);
    if (key === 'ssh' && c.state !== 'ok') {
      return { ok: false, error: { code: 'E_SSH', message: `Fleet can't reach ${sshAlias} over SSH: ${c.detail || 'no answer'}` } };
    }
  }
  onprogress?.(`Adding ${alias}…`);
  const added = await addHost(alias, sshAlias);
  if (!added.ok) return added;
  // The checks keep a draft for the guided wizard to resume; this one is done.
  await discardHostSetup(sshAlias);
  return { ok: true, value: { host: added.value, checks } };
}
