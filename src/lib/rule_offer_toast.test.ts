// Toasts board (G4.8): the rule-suggestion toast. A start whose key fleet
// now offers a start rule for says so in the corner, two buttons: Add rule
// accepts the offer; Not now only closes the toast.
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import { clearToasts, runToastAction, toasts } from './toasts';
import { offerFor, offerRuleAfterStart, ruleMatches, _resetRuleOfferToastsForTests } from './rule_offer_toast';
import { startWork } from './trackers';
import type { StartRuleView } from './start_rules';
import { session } from './hosts_fixture';

const OFFER: StartRuleView = {
  id: 7,
  pattern: 'PD-*',
  project_id: 3,
  project: 'acme/papaya-pos',
  state: 'offered',
  confirmations: 5,
  created_at: 0,
  updated_at: 0,
  may_change: true,
};

let rules: StartRuleView[] = [];

beforeEach(() => {
  clearToasts();
  _resetRuleOfferToastsForTests();
  rules = [OFFER];
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (cmd: string, a?: unknown) => {
    const args = (a as { args?: { action?: string } } | undefined)?.args;
    if (cmd === 'start_rules' && args?.action === 'list') return rules;
    if (cmd === 'start_rules' && args?.action === 'accept') return { ...OFFER, state: 'active' };
    if (cmd === 'start_work') return session('mac', 'pd-2988', { work: { link_id: 1, item_id: 9, key: 'PD-2988', title: 't', source: 'started' } });
    return null;
  });
});

const flush = () => new Promise((r) => setTimeout(r, 0));

describe('rule-suggestion toast', () => {
  it('a start whose key fleet now offers a rule for toasts it, and Add rule accepts the offer', async () => {
    const r = await startWork({ reference: 'PD-2988' });
    expect(r.ok).toBe(true);
    await flush();
    const [t] = get(toasts);
    expect(t.message).toBe('You picked acme/papaya-pos for PD-* 5 times');
    expect(t.sub).toBe('Add rule PD-* → acme/papaya-pos?');
    expect(t.action?.label).toBe('Add rule');
    expect(t.secondary?.label).toBe('Not now');

    runToastAction(t.id);
    await flush();
    expect(invoke).toHaveBeenCalledWith('start_rules', { args: { action: 'accept', rule_id: 7 } });
    expect(get(toasts).map((x) => x.message)).toEqual(['Rule added: PD-* → acme/papaya-pos']);
  });

  it('Not now changes nothing, and the same offer does not toast twice in a window', async () => {
    const id = await offerRuleAfterStart('PD-1');
    runToastAction(id!, 'secondary');
    await flush();
    expect(vi.mocked(invoke).mock.calls.some((c) => (c[1] as { args?: { action?: string } })?.args?.action !== 'list')).toBe(false);
    expect(await offerRuleAfterStart('PD-2')).toBeNull();
    expect(get(toasts)).toEqual([]);
  });

  it('says nothing for a key no offer matches, an offer this person may not answer, or a failed read', async () => {
    expect(await offerRuleAfterStart('ABC-1')).toBeNull();
    rules = [{ ...OFFER, may_change: false }];
    expect(await offerRuleAfterStart('PD-1')).toBeNull();
    vi.mocked(invoke).mockRejectedValue({ code: 'E_IO', message: 'down' });
    expect(await offerRuleAfterStart('PD-1')).toBeNull();
    expect(await offerRuleAfterStart(null)).toBeNull();
    expect(get(toasts)).toEqual([]);
  });

  it('matches like the backend glob: * is any run, case ignored', () => {
    expect(ruleMatches('PD-*', 'pd-12')).toBe(true);
    expect(ruleMatches('PD-1*', 'PD-2')).toBe(false);
    expect(ruleMatches('a.b*', 'axb1')).toBe(false);
    expect(offerFor([{ ...OFFER, state: 'active' }], 'PD-1', new Set())).toBeNull();
    expect(offerFor([OFFER], 'PD-1', new Set([7]))).toBeNull();
  });
});
