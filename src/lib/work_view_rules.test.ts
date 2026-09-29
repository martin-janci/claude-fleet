// `ruleWire` (work graph M14): a rule draft as the wire takes it. The rule
// editor signs its preview with it, and `save_work_rule` /
// `work_rule_preview` send it, so what it trims, drops and keeps decides
// whether a preview still answers for the draft being saved.
import { describe, it, expect } from 'vitest';
import { ruleWire, type WorkRuleDraft } from './work_view';

const draft = (over: Partial<WorkRuleDraft> = {}): WorkRuleDraft => ({
  name: 'Payments',
  enabled: true,
  group: 'Payments',
  conditions: {},
  ...over,
});

describe('ruleWire', () => {
  it('trims the name, the group and every string condition', () => {
    const w = ruleWire(
      draft({
        name: '  Pay rule ',
        group: '\tPayments  ',
        conditions: { container: ' PAY ', key_prefix: ' ABC', title_contains: 'refund  ', repo: '  acme/api  ' },
      }),
    );
    expect(w.name).toBe('Pay rule');
    expect(w.group).toBe('Payments');
    expect(w.conditions).toEqual({
      tracker_id: null,
      container: 'PAY',
      key_prefix: 'ABC',
      title_contains: 'refund',
      repo: 'acme/api',
    });
  });

  it('turns a blank or whitespace-only condition into null', () => {
    const w = ruleWire(draft({ conditions: { container: '', key_prefix: '   ', title_contains: '\t\n', repo: null } }));
    expect(w.conditions).toEqual({ tracker_id: null, container: null, key_prefix: null, title_contains: null, repo: null });
  });

  it('passes tracker_id through and reads an absent one as null', () => {
    expect(ruleWire(draft({ conditions: { tracker_id: 3 } })).conditions.tracker_id).toBe(3);
    expect(ruleWire(draft({ conditions: { tracker_id: undefined } })).conditions.tracker_id).toBeNull();
    expect(ruleWire(draft({ conditions: {} })).conditions.tracker_id).toBeNull();
  });

  it('drops an absent id and expected_version, keeps present ones (0 included)', () => {
    const bare = ruleWire(draft());
    expect('id' in bare).toBe(false);
    expect('expected_version' in bare).toBe(false);
    expect(Object.keys(bare).sort()).toEqual(['conditions', 'enabled', 'group', 'name']);
    const edit = ruleWire(draft({ id: 9, expected_version: 4 }));
    expect(edit.id).toBe(9);
    expect(edit.expected_version).toBe(4);
    expect(ruleWire(draft({ expected_version: 0 })).expected_version).toBe(0);
  });

  it('keeps enabled as given', () => {
    expect(ruleWire(draft({ enabled: false })).enabled).toBe(false);
  });

  it('is idempotent', () => {
    const cases: WorkRuleDraft[] = [
      draft(),
      draft({ id: 2, expected_version: 5, name: ' x ', group: ' y ', conditions: { tracker_id: 1, container: ' c ', repo: '  ' } }),
      draft({ conditions: { key_prefix: 'ABC', title_contains: '' } }),
    ];
    for (const d of cases) {
      const once = ruleWire(d);
      expect(ruleWire(once)).toEqual(once);
    }
  });
});
