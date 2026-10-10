import { describe, expect, it } from 'vitest';
import { deliveryOf, globMatch, hasDelivery, startRuleFor, workSpan } from './task_detail';
import { link, task } from './work_view_fixture';

describe('task_detail', () => {
  it('globMatch mirrors the backend: * is any run, case ignored', () => {
    expect(globMatch('PD-*', 'pd-12')).toBe(true);
    expect(globMatch('PD-*', 'PX-12')).toBe(false);
    expect(globMatch('*-12', 'PD-12')).toBe(true);
    expect(globMatch('PD-1*2', 'PD-1002')).toBe(true);
    expect(globMatch('PD-1', 'PD-12')).toBe(false);
    expect(globMatch('PD-**', 'PD-')).toBe(true);
  });

  it('startRuleFor picks the most specific active rule', () => {
    const rules = [
      { pattern: 'PD-*', state: 'active' },
      { pattern: 'PD-1*', state: 'active' },
      { pattern: 'PD-12', state: 'offered' },
    ];
    expect(startRuleFor('PD-12', rules)?.pattern).toBe('PD-1*');
    expect(startRuleFor('PD-2', rules)?.pattern).toBe('PD-*');
    expect(startRuleFor(null, rules)).toBeNull();
    expect(startRuleFor('AB-1', rules)).toBeNull();
  });

  it('workSpan runs from the first link to the last end, or now while one is live', () => {
    const ended = task({
      sessions: [
        link({ link_id: 1, state: 'ended', created_at: 1000, ended_at: 4600 }),
        link({ link_id: 2, state: 'ended', created_at: 2000, ended_at: 8200 }),
      ],
    });
    expect(workSpan(ended, 99_999)).toBe(7200);
    const live = task({ sessions: [link({ link_id: 1, state: 'active', created_at: 1000 })] });
    expect(workSpan(live, 1600)).toBe(600);
    expect(workSpan(task({ sessions: [] }), 10)).toBeNull();
  });

  it('a task with nothing delivered has no block', () => {
    const d = deliveryOf(task({ sessions: [], status_name: null, cost_micros: 0, assignees: [], due_at: null, mine: false }), null, new Map(), 0);
    expect(hasDelivery(d)).toBe(false);
  });
});
