// Redesign 8.11 (From AI to rule): the start popover offers "Add rule
// PD-* → acme/pos?" and says when a rule picked the repository;
// Automation › Rules lists offers and rules and edits, turns off and
// deletes them.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import StartPopover from './StartPopover.svelte';
import StartRules from './StartRules.svelte';
import { projects } from './projects';
import { hubStatus, STANDALONE } from './hub';
import { hubConnection } from './hub_connection';
import type { StartPreview } from './start_preview';
import { accountChoices, patternProblem, ruleLaunchLine, ruleLine, startRuleWire, type StartRuleView } from './start_rules';
import { hosts } from './hosts';
import { planLaunchLine } from './start_preview';

async function flush() {
  for (let i = 0; i < 10; i++) await tick();
}

const offer = {
  id: 9,
  pattern: 'PD-*',
  project_id: 4,
  state: 'offered',
  confirmations: 5,
  created_at: 1,
  updated_at: 2,
};

const plan = { key: 'PD-12', title: 'Refund', item_id: 12, project_id: 4, host_alias: 'mac', branch: 'pd-12-refund', name: 'PD-12 Refund' };

const preview: StartPreview = {
  key: 'PD-12',
  title: 'Refund',
  item_id: 12,
  plan,
  missing: null,
  projects: [
    { id: 3, owner: 'acme', repo: 'api' },
    { id: 4, owner: 'acme', repo: 'pos' },
  ],
  hosts: [{ alias: 'mac', reachable: true }],
  conflicts: [],
  rule_offer: offer,
} as StartPreview;

type Call = [string, { args?: Record<string, unknown> } | undefined];
const calls = () => vi.mocked(invoke).mock.calls as unknown as Call[];
const ruleCalls = () => calls().filter(([c]) => c === 'start_rules').map(([, a]) => a?.args);

let listed: StartRuleView[] = [];

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (cmd: string, raw?: unknown) => {
    const a = (raw as { args?: { action?: string; rule_id?: number } } | undefined)?.args;
    if (cmd === 'preview_start_work') return { ...preview, rule_offer: null };
    if (cmd !== 'start_rules') return null;
    if (a?.action === 'list') return listed;
    if (a?.action === 'delete') return { removed: true };
    return { ...offer, state: a?.action === 'dismiss' ? 'dismissed' : 'active', project: 'acme/pos', may_change: true };
  });
  hubStatus.set({ ...STANDALONE });
  hubConnection.set({ state: 'standalone' });
  localStorage.clear();
  listed = [];
});

describe('start rule helpers', () => {
  it('writes a rule as pattern, repository and host', () => {
    expect(ruleLine({ ...offer, project: 'acme/pos' })).toBe('PD-* → acme/pos');
    expect(ruleLine({ ...offer, host_alias: 'mac' }, preview.projects)).toBe('PD-* → acme/pos on mac');
    expect(ruleLine({ ...offer, project_id: 99 })).toBe('PD-* → project 99');
  });
  it('checks a pattern as the backend does', () => {
    expect(patternProblem(' PD-* ')).toBeNull();
    expect(patternProblem('acme/app#*')).toBeNull();
    for (const bad of ['', '*', '**', 'PD *', 'PD-*;rm', 'A'.repeat(65)]) expect(patternProblem(bad)).not.toBeNull();
  });
});

describe('the start popover', () => {
  const props = { base: { item_id: 12, with_brief: true }, preview, heading: 'Start PD-12', onclose: () => {}, onstarted: () => {}, debounceMs: 0 };

  it('offers the rule and adds it', async () => {
    render(StartPopover, { props });
    await flush();
    expect(screen.getByTestId('start-popover-rule-offer').textContent).toContain('PD-* → acme/pos');
    await fireEvent.click(screen.getByTestId('start-popover-rule-add'));
    await flush();
    expect(ruleCalls()).toContainEqual({ action: 'accept', rule_id: 9 });
    expect(screen.queryByTestId('start-popover-rule-offer')).toBeNull();
    expect(screen.getByTestId('start-popover-rule-added').textContent).toContain('PD-* → acme/pos');
  });

  it('dismisses the offer for good', async () => {
    render(StartPopover, { props });
    await flush();
    await fireEvent.click(screen.getByTestId('start-popover-rule-dismiss'));
    await flush();
    expect(ruleCalls()).toContainEqual({ action: 'dismiss', rule_id: 9 });
    expect(screen.queryByTestId('start-popover-rule-offer')).toBeNull();
    expect(screen.queryByTestId('start-popover-rule-added')).toBeNull();
  });

  it('says when a rule picked the repository', async () => {
    render(StartPopover, { props: { ...props, preview: { ...preview, rule_offer: null, plan: { ...plan, rule_id: 9 } } } });
    await flush();
    expect(screen.getByTestId('start-popover-by-rule')).toBeTruthy();
  });

});

describe('Automation › Rules', () => {
  beforeEach(() => {
    projects.set([
      { project: { id: 3, owner: 'acme', repo: 'api', system: false }, worktrees: [] },
      { project: { id: 4, owner: 'acme', repo: 'pos', system: false }, worktrees: [] },
    ] as never);
  });

  it('lists offers first and adds one', async () => {
    listed = [
      { ...offer, project: 'acme/pos', may_change: true },
      { ...offer, id: 2, pattern: 'OM-*', project_id: 3, project: 'acme/api', state: 'active', hits: 4, may_change: true },
    ];
    render(StartRules);
    await flush();
    expect(screen.getByTestId('start-rules-offers').textContent).toContain('PD-* → acme/pos');
    expect(screen.getByTestId('start-rules-active').textContent).toContain('decided 4 starts');
    await fireEvent.click(screen.getByTestId('start-rules-accept'));
    await flush();
    expect(ruleCalls()).toContainEqual({ action: 'accept', rule_id: 9 });
  });

  it('edits and deletes a rule', async () => {
    listed = [{ ...offer, id: 2, pattern: 'OM-*', project_id: 3, project: 'acme/api', state: 'active', may_change: true }];
    render(StartRules);
    await flush();
    await fireEvent.click(screen.getByTestId('start-rules-edit'));
    await flush();
    const input = screen.getByTestId('start-rules-pattern') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'OM-1*' } });
    await fireEvent.change(screen.getByTestId('start-rules-project'), { target: { value: '4' } });
    await fireEvent.click(screen.getByTestId('start-rules-save'));
    await flush();
    expect(ruleCalls()).toContainEqual({ action: 'save', rule_id: 2, rule: { pattern: 'OM-1*', project_id: 4, host_alias: null, fallback_host: null, profile: null, model: null, effort: null, agent: null } });
    await fireEvent.click(screen.getByTestId('start-rules-delete'));
    await flush();
    expect(ruleCalls()).toContainEqual({ action: 'delete', rule_id: 2 });
  });

  it('adds a new rule and refuses a pattern that matches everything', async () => {
    render(StartRules);
    await flush();
    expect(screen.getByTestId('start-rules-empty')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('start-rules-new'));
    await flush();
    await fireEvent.input(screen.getByTestId('start-rules-pattern'), { target: { value: '*' } });
    await fireEvent.change(screen.getByTestId('start-rules-project'), { target: { value: '3' } });
    await flush();
    expect((screen.getByTestId('start-rules-save') as HTMLButtonElement).disabled).toBe(true);
    await fireEvent.input(screen.getByTestId('start-rules-pattern'), { target: { value: 'PD-*' } });
    await flush();
    await fireEvent.click(screen.getByTestId('start-rules-save'));
    await flush();
    expect(ruleCalls()).toContainEqual({ action: 'save', rule: { pattern: 'PD-*', project_id: 3, host_alias: null, fallback_host: null, profile: null, model: null, effort: null, agent: null } });
  });

  it('a rule names the fallback host, account, model, effort and agent (G7.1)', async () => {
    hosts.set([
      { alias: 'mac', claude_profiles: [{ name: 'work', email: 'w@x.com' }] },
      { alias: 'mercury', claude_profiles: [] },
    ] as never);
    render(StartRules);
    await flush();
    await fireEvent.click(screen.getByTestId('start-rules-new'));
    await flush();
    await fireEvent.input(screen.getByTestId('start-rules-pattern'), { target: { value: 'PD-*' } });
    await fireEvent.change(screen.getByTestId('start-rules-project'), { target: { value: '3' } });
    await fireEvent.change(screen.getByTestId('start-rules-host'), { target: { value: 'mac' } });
    await flush();
    // The fallback offers every host but the rule's own.
    const fallback = screen.getByTestId('start-rules-fallback') as HTMLSelectElement;
    expect(Array.from(fallback.options).map((o) => o.value)).toEqual(['', 'mercury']);
    await fireEvent.change(fallback, { target: { value: 'mercury' } });
    const account = screen.getByTestId('start-rules-account') as HTMLSelectElement;
    expect(Array.from(account.options).map((o) => o.textContent)).toEqual(["The host's own login", 'work · w@x.com']);
    await fireEvent.change(account, { target: { value: 'work' } });
    await fireEvent.change(screen.getByTestId('start-rules-model'), { target: { value: 'opus' } });
    await fireEvent.change(screen.getByTestId('start-rules-effort'), { target: { value: 'high' } });
    await flush();
    await fireEvent.click(screen.getByTestId('start-rules-save'));
    await flush();
    expect(ruleCalls()).toContainEqual({
      action: 'save',
      rule: { pattern: 'PD-*', project_id: 3, host_alias: 'mac', fallback_host: 'mercury', profile: 'work', model: 'opus', effort: 'high', agent: null },
    });
    hosts.set([]);
  });

  it('Codex hides the account and the wire drops it', async () => {
    render(StartRules);
    await flush();
    await fireEvent.click(screen.getByTestId('start-rules-new'));
    await flush();
    await fireEvent.change(screen.getByTestId('start-rules-agent'), { target: { value: 'codex' } });
    await flush();
    expect(screen.queryByTestId('start-rules-account')).toBeNull();
    expect(startRuleWire({ pattern: 'PD-*', project_id: 3, agent: 'codex', profile: 'work' }).profile).toBeNull();
  });

  it('lists how an active rule starts', async () => {
    listed = [{ ...offer, id: 2, state: 'active', project: 'acme/pos', may_change: true, host_alias: 'mac', fallback_host: 'mercury', profile: 'work', model: 'opus', effort: 'high' }];
    render(StartRules);
    await flush();
    expect(screen.getByTestId('start-rules-launch').textContent).toBe('mac, else mercury · work · opus · effort high');
  });

  it('shows a rule it may not change without its buttons', async () => {
    listed = [{ ...offer, id: 2, state: 'active', project: 'acme/pos', may_change: false }];
    render(StartRules);
    await flush();
    expect(screen.queryByTestId('start-rules-edit')).toBeNull();
    expect(screen.queryByTestId('start-rules-delete')).toBeNull();
  });
});

describe('rule launch lines (G7.1)', () => {
  it('reads a rule and a plan', () => {
    expect(ruleLaunchLine({ host_alias: null })).toBe('');
    expect(ruleLaunchLine({ fallback_host: 'g', effort: 'max', agent: 'codex' })).toBe('its last host, else g · effort max · Codex');
    expect(planLaunchLine({ profile: 'work', model: 'opus', fell_back_from: 'mac' })).toBe('account work · opus · mac is offline');
  });

  it('offers the accounts of the chosen host, or of every host, and keeps the current one', () => {
    const hs = [
      { alias: 'a', claude_profiles: [{ name: 'work', email: null }] },
      { alias: 'b', claude_profiles: [{ name: 'home', email: 'h@x' }, { name: 'work' }] },
    ];
    expect(accountChoices(hs, 'a').map((c) => c.value)).toEqual(['work']);
    expect(accountChoices(hs, '').map((c) => c.label)).toEqual(['work', 'home · h@x']);
    expect(accountChoices(hs, 'a', 'gone').map((c) => c.value)).toEqual(['work', 'gone']);
  });
});
