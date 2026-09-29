// The rule editor (work graph M14): a rule is saved only after a preview of
// exactly the draft being saved (any edit that changes the wire draft needs
// a new preview), it is sent as `ruleWire(draft)` with the version it was
// read at, and a rule changed or deleted elsewhere is said so and needs a
// fresh preview at the current version.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach, vi, type Mock } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import WorkRuleEditor from './WorkRuleEditor.svelte';
import { ruleWire, workTreeMeta, type WorkRule, type WorkRuleDraft } from './work_view';
import { hubStatus, STANDALONE } from './hub';
import { hubConnection } from './hub_connection';

const fresh: WorkRuleDraft = {
  name: 'Payments',
  enabled: true,
  group: 'Payments',
  expected_version: 0,
  conditions: { tracker_id: null, container: null, key_prefix: 'ABC', title_contains: null, repo: null },
};

const existing: WorkRuleDraft = {
  id: 9,
  name: 'Payments',
  enabled: true,
  group: 'Payments',
  expected_version: 3,
  conditions: { tracker_id: 1, container: 'PAY', key_prefix: null, title_contains: null, repo: null },
};

const saved = (over: Partial<WorkRule> = {}): WorkRule => ({
  id: 9,
  name: 'Payments',
  enabled: true,
  version: 4,
  conditions: { tracker_id: 1, container: 'PAY' },
  group: 'Payments',
  ...over,
});

type Handler = (args: Record<string, unknown>) => unknown;
let handlers: Record<string, Handler>;

function calls(cmd: string) {
  return vi
    .mocked(invoke)
    .mock.calls.filter((c) => c[0] === cmd)
    .map((c) => (c[1] as { args: Record<string, unknown> }).args);
}

async function flush() {
  for (let i = 0; i < 10; i++) await tick();
}

const btn = (id: string) => screen.getByTestId(id) as HTMLButtonElement;

async function type(id: string, value: string) {
  await fireEvent.input(screen.getByTestId(id), { target: { value } });
  await flush();
}

async function preview() {
  await fireEvent.click(btn('rule-preview-btn'));
  await flush();
}

describe('WorkRuleEditor', () => {
  let onclose: Mock<() => void>;
  let onsaved: Mock<(r: WorkRule) => void>;

  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    workTreeMeta.set({ orgs: [], trackers: [{ id: 1, name: 'Jira (acme)', provider: 'jira', state: 'ok' }], groups: [] });
    hubStatus.set({ ...STANDALONE });
    hubConnection.set({ state: 'standalone' });
    onclose = vi.fn();
    onsaved = vi.fn();
    handlers = {
      work_rule_preview: () => ({
        affected: [{ task_id: 'item:14', key: 'ABC-14', title: 'x', from: { id: 'none', label: '', source: 'none' }, to: { id: 'label:Payments', label: 'Payments', source: 'rule' } }],
        total: 1,
        kept_manual: 0,
      }),
      save_work_rule: () => saved(),
      work_rules: () => [],
    };
    vi.mocked(invoke).mockImplementation(async (cmd: string, raw?: unknown) => {
      const h = handlers[cmd];
      return h ? h((raw as { args: Record<string, unknown> } | undefined)?.args ?? {}) : null;
    });
  });

  afterEach(() => {
    workTreeMeta.set({ orgs: [], trackers: [], groups: [] });
  });

  const mount = (initial: WorkRuleDraft) => render(WorkRuleEditor, { initial, onclose, onsaved });

  it('Save waits for a preview of the draft as the wire takes it', async () => {
    mount(fresh);
    expect(btn('rule-save').disabled).toBe(true);
    expect(btn('rule-preview-btn').disabled).toBe(false);
    await preview();
    // The preview carries the draft without its version.
    const { expected_version: _v, ...draftOnly } = fresh;
    expect(calls('work_rule_preview')).toEqual([{ rule: ruleWire(draftOnly) }]);
    expect(screen.getByTestId('rule-preview')).toBeTruthy();
    expect(btn('rule-save').disabled).toBe(false);
  });

  it('an edit that changes the draft needs a new preview; whitespace does not', async () => {
    mount(fresh);
    await preview();
    expect(btn('rule-save').disabled).toBe(false);
    // Trimmed on the wire: still the previewed rule.
    await type('rule-key-prefix', 'ABC  ');
    expect(btn('rule-save').disabled).toBe(false);
    await type('rule-key-prefix', 'ABD');
    expect(btn('rule-save').disabled).toBe(true);
    expect(screen.getByTestId('rule-preview-stale')).toBeTruthy();
    await fireEvent.click(btn('rule-save'));
    await flush();
    expect(calls('save_work_rule')).toEqual([]);
    await preview();
    expect(btn('rule-save').disabled).toBe(false);
    for (const [id, value] of [
      ['rule-name', 'Pay'],
      ['rule-group', 'Billing'],
      ['rule-container', 'PAY'],
      ['rule-title-contains', 'refund'],
      ['rule-repo', 'acme/api'],
    ] as const) {
      await type(id, value);
      expect(btn('rule-save').disabled, id).toBe(true);
      await preview();
      expect(btn('rule-save').disabled, id).toBe(false);
    }
  });

  it('saves ruleWire(draft) with the expected version, then closes', async () => {
    mount(existing);
    await type('rule-group', '  Billing ');
    await preview();
    await fireEvent.click(btn('rule-save'));
    await flush();
    expect(calls('save_work_rule')).toEqual([{ rule: ruleWire({ ...existing, group: 'Billing', expected_version: 3 }) }]);
    expect(calls('save_work_rule')[0].rule).toMatchObject({ id: 9, group: 'Billing', expected_version: 3 });
    expect(onsaved).toHaveBeenCalledWith(saved());
    expect(onclose).toHaveBeenCalledTimes(1);
  });

  it('a new rule is saved expecting version 0', async () => {
    mount(fresh);
    await preview();
    await fireEvent.click(btn('rule-save'));
    await flush();
    expect(calls('save_work_rule')[0].rule).toEqual(ruleWire(fresh));
    expect((calls('save_work_rule')[0].rule as WorkRuleDraft).expected_version).toBe(0);
  });

  it('a rule changed elsewhere: says so, clears the preview, and saves at the new version', async () => {
    let first = true;
    handlers.save_work_rule = () => {
      if (first) {
        first = false;
        throw { code: 'E_CONFLICT', message: 'rule changed', details: { rule_id: 9, version: 5 } };
      }
      return saved({ version: 6 });
    };
    handlers.work_rules = () => [saved({ name: 'Pay (ops)', group: 'Ops', version: 5, enabled: false }), saved({ id: 10, version: 1 })];
    mount(existing);
    await preview();
    await fireEvent.click(btn('rule-save'));
    await flush();
    expect(calls('work_rules')).toHaveLength(1);
    const err = screen.getByTestId('rule-error').textContent ?? '';
    expect(err).toContain('This rule changed elsewhere');
    expect(err).toContain('“Pay (ops)”, off, placing in “Ops” (version 5)');
    expect(btn('rule-save').disabled).toBe(true);
    expect(screen.queryByTestId('rule-preview')).toBeNull();
    expect(onclose).not.toHaveBeenCalled();
    await preview();
    await fireEvent.click(btn('rule-save'));
    await flush();
    expect(calls('save_work_rule').map((a) => (a.rule as WorkRuleDraft).expected_version)).toEqual([3, 5]);
    expect(onclose).toHaveBeenCalledTimes(1);
  });

  it('a rule deleted elsewhere says so', async () => {
    handlers.save_work_rule = () => {
      throw { code: 'E_CONFLICT', message: 'rule gone', details: { rule_id: 9 } };
    };
    handlers.work_rules = () => [saved({ id: 10 })];
    mount(existing);
    await preview();
    await fireEvent.click(btn('rule-save'));
    await flush();
    expect(screen.getByTestId('rule-error').textContent).toContain('This rule was deleted elsewhere.');
    expect(btn('rule-save').disabled).toBe(true);
  });

  it('Preview needs a name, a group and at least one condition', async () => {
    mount({ ...fresh, conditions: {} });
    expect(btn('rule-preview-btn').disabled).toBe(true);
    expect(screen.getByTestId('work-rule-editor').textContent).toContain('Give the rule at least one condition.');
    // Whitespace is no condition.
    await type('rule-repo', '   ');
    expect(btn('rule-preview-btn').disabled).toBe(true);
    await type('rule-repo', 'acme/api');
    expect(btn('rule-preview-btn').disabled).toBe(false);
    await type('rule-name', '  ');
    expect(btn('rule-preview-btn').disabled).toBe(true);
    await type('rule-name', 'Api');
    expect(btn('rule-preview-btn').disabled).toBe(false);
    await type('rule-group', '');
    expect(btn('rule-preview-btn').disabled).toBe(true);
    await fireEvent.click(btn('rule-preview-btn'));
    await flush();
    expect(calls('work_rule_preview')).toEqual([]);
  });

  it('a tracker alone is a condition', async () => {
    mount({ ...fresh, conditions: {} });
    await fireEvent.change(screen.getByTestId('rule-tracker'), { target: { value: '1' } });
    await flush();
    expect(btn('rule-preview-btn').disabled).toBe(false);
    await preview();
    expect((calls('work_rule_preview')[0].rule as WorkRuleDraft).conditions.tracker_id).toBe(1);
  });
});
