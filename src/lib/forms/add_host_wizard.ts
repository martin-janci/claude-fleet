// What the Add host wizard's last button does (redesign step 10.12): the
// answers of `wizards/add_host.json` checked the way 4.9's guided wizard
// checks a host, in its order, then added. Nothing runs before the button;
// a host SSH cannot reach is not added, every other check only informs.
import { get } from 'svelte/store';
import { addHost, type SshHost } from '../hosts';
import { hubStatus } from '../hub';
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

/** The `via` answer for a host the hub cannot reach (review w14-2). */
export const VIA_AGENT = 'agent';

/** The Add host wizard on a desktop paired with a hub (HubAddHostDialog):
 *  it says what really runs (the hub's own probe, not this machine's
 *  checks, review w14-10) and asks how the hub reaches the host, so one it
 *  cannot reach is added for fleet-agent (review w14-2). */
export function hubAddHostWizard(discovered: readonly SshHost[]): Wizard {
  const w = addHostWizard(discovered);
  const [first, ...rest] = w.spec.steps;
  return {
    ...w,
    sending: 'Adding…',
    spec: {
      ...w.spec,
      intro:
        'The hub connects over its own SSH and adds the host when it answers. A host the hub cannot reach runs fleet-agent, which dials the hub instead.',
      submit: 'Add',
      steps: [
        {
          ...first,
          fields: [
            ...first.fields,
            {
              name: 'via',
              type: 'select',
              label: 'How the hub reaches it',
              required: true,
              value: 'ssh',
              options: [
                ['ssh', 'Over SSH, from the hub'],
                [VIA_AGENT, "The hub can't reach it: use fleet-agent"],
              ],
            },
          ],
        },
        ...rest,
      ],
    },
  };
}

/** What a person runs to put fleet-agent on a host added for it: the
 *  token on the hub (no client is ever sent one), then the install on the
 *  host with the token pasted into stdin (docs/hub.md, *Set it up*). */
export function agentSetupCommands(alias: string, hubUrl: string | null): { token: string; install: string } {
  return {
    token: `fleet-hub agent-token ${alias}`,
    install: `fleet-agent install --user --hub ${hubUrl ?? '<hub URL>'} --token-file -`,
  };
}

/** The SSH alias and the fleet name the answers name. */
export function addHostTarget(v: Values): { sshAlias: string; alias: string } {
  const picked = typeof v.pick === 'string' ? v.pick : TYPED;
  const sshAlias = (picked === TYPED ? String(v.ssh_alias ?? '') : picked).trim();
  const alias = String(v.alias ?? '').trim() || sshAlias;
  return { sshAlias, alias };
}

/** Check, then add. `onprogress` gets the line beside the wizard's loader
 *  while each check waits.
 *
 *  A desktop paired with a hub adds through the hub's `add_host`, which
 *  probes the alias over the HUB's SSH and keeps the host only if it
 *  answers: this machine's checks would test the wrong route (and refuse,
 *  `E_LOCAL_ONLY`), so they are left to that probe, as on the phone. */
export async function runAddHost(v: Values, onprogress?: (text: string) => void): Promise<ChatFormOutcome> {
  const { sshAlias, alias } = addHostTarget(v);
  if (!sshAlias) return { ok: false, problems: [{ field: 'ssh_alias', problem: 'Name the SSH alias to add.' }] };
  if (get(hubStatus).remote) {
    if (v.via === VIA_AGENT) {
      // No probe: the hub cannot reach it, and its agent dials in later.
      onprogress?.(`Adding ${alias} for fleet-agent…`);
      const added = await addHost(alias, sshAlias, 'agent');
      if (!added.ok) return { ok: false, error: added.error.message };
      return { ok: true, summary: `${alias} added on the hub; it shows offline until its fleet-agent connects` };
    }
    onprogress?.(`The hub is checking ${sshAlias}…`);
    const added = await addHost(alias, sshAlias);
    if (!added.ok) return { ok: false, error: added.error.message };
    return { ok: true, summary: `${alias} added on the hub` };
  }
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
