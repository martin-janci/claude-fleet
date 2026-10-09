// What the Add host wizard's last button does (redesign step 10.12): the
// answers of `wizards/add_host.json` checked the way 4.9's guided wizard
// checks a host, in its order, then added. Nothing runs before the button;
// a host SSH cannot reach is not added, every other check only informs.
import { addHost, type SshHost } from '../hosts';
import { CHECK_KEYS, checkLoader, discardHostSetup, runHostSetupCheck, type SetupCheck } from '../add_host_wizard';
import type { ChatFormOutcome } from './ChatForm.svelte';
import type { Values } from './forms';
import { WIZARDS, withChoices, type Wizard } from './wizards';

/** The `pick` value that means "type the alias". An SSH alias is never `*`
 *  (that is a config wildcard, which discovery leaves out). */
export const TYPED = '*';

/** The wizard with the hosts from ~/.ssh/config to pick from. */
export function addHostWizard(discovered: readonly SshHost[]): Wizard {
  const picks: [string, string][] = discovered
    .filter((h) => h.alias !== TYPED)
    .slice(0, 49)
    .map((h) => [h.alias, h.hostname ? `${h.alias} · ${h.user ? `${h.user}@` : ''}${h.hostname}` : h.alias]);
  const w = WIZARDS.add_host;
  return { ...w, spec: withChoices(w.spec, { pick: [...picks, [TYPED, 'Another alias']] }) };
}

/** The SSH alias and the fleet name the answers name. */
export function addHostTarget(v: Values): { sshAlias: string; alias: string } {
  const picked = typeof v.pick === 'string' ? v.pick : TYPED;
  const sshAlias = (picked === TYPED ? String(v.ssh_alias ?? '') : picked).trim();
  const alias = String(v.alias ?? '').trim() || sshAlias;
  return { sshAlias, alias };
}

/** Check, then add. `onprogress` gets the line beside the wizard's loader
 *  while each check waits. */
export async function runAddHost(v: Values, onprogress?: (text: string) => void): Promise<ChatFormOutcome> {
  const { sshAlias, alias } = addHostTarget(v);
  if (!sshAlias) return { ok: false, problems: [{ field: 'ssh_alias', problem: 'Name the SSH alias to add.' }] };
  const checks: SetupCheck[] = [];
  for (const key of CHECK_KEYS) {
    onprogress?.(checkLoader(key, sshAlias)!.text);
    const r = await runHostSetupCheck(sshAlias, key);
    const c: SetupCheck = r.ok ? r.value : { key, state: 'fail', label: key, detail: r.error.message };
    checks.push(c);
    if (key === 'ssh' && c.state !== 'ok') {
      return { ok: false, error: `Fleet can't reach ${sshAlias} over SSH: ${c.detail || 'no answer'}` };
    }
  }
  onprogress?.(`Adding ${alias}…`);
  const added = await addHost(alias, sshAlias);
  if (!added.ok) return { ok: false, error: added.error.message };
  // The checks keep a draft for the guided wizard to resume; this one is done.
  await discardHostSetup(sshAlias);
  const ok = checks.filter((c) => c.state === 'ok').length;
  return { ok: true, summary: `${alias} added, ${ok} of ${CHECK_KEYS.length} checks ok` };
}
