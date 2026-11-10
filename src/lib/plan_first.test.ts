import { beforeEach, describe, expect, it } from 'vitest';
import type { ChangedFile } from './files';
import {
  isPlanFirst,
  markPlanFirst,
  planApprovedPrompt,
  planDate,
  planFile,
  planFirstBrief,
  planPending,
  settlePlan,
  withPlanFirst,
} from './plan_first';

const f = (path: string, status = 'untracked'): ChangedFile => ({ path, status, staged: false, orig_path: null });

describe('plan first', () => {
  beforeEach(() => localStorage.clear());

  it('finds the plan the session wrote among its changes', () => {
    expect(planFile([f('src/a.ts', 'modified')])).toBeNull();
    expect(planFile([f('docs/plans/2026-11-10-retry.md'), f('src/a.ts', 'modified')])).toBe('docs/plans/2026-11-10-retry.md');
    expect(planFile([f('docs/superpowers/plans/x.md', 'added')])).toBe('docs/superpowers/plans/x.md');
    // A deleted plan, a nested file and another kind of file are not one.
    expect(planFile([f('docs/plans/x.md', 'deleted'), f('docs/plans/img/a.md'), f('docs/plans/x.txt')])).toBeNull();
    // A new plan wins over an edited one; among new ones the last by name.
    expect(
      planFile([f('docs/plans/2026-11-01-old.md', 'modified'), f('docs/plans/2026-11-09-a.md'), f('docs/plans/2026-11-10-b.md')]),
    ).toBe('docs/plans/2026-11-10-b.md');
  });

  it('asks once per plan file, and only for a session started with it', () => {
    expect(planPending(7, 'docs/plans/a.md')).toBe(false);
    markPlanFirst(7);
    expect(isPlanFirst(7)).toBe(true);
    expect(planPending(7, null)).toBe(false);
    expect(planPending(7, 'docs/plans/a.md')).toBe(true);
    settlePlan(7, 'docs/plans/a.md');
    expect(planPending(7, 'docs/plans/a.md')).toBe(false);
    // A second plan in the same session asks again.
    expect(planPending(7, 'docs/plans/b.md')).toBe(true);
    // Settling a session that never had it marks nothing.
    settlePlan(8, 'docs/plans/a.md');
    expect(isPlanFirst(8)).toBe(false);
  });

  it('keeps the newest 200 sessions', () => {
    for (let i = 0; i < 205; i++) markPlanFirst(i);
    expect(isPlanFirst(0)).toBe(false);
    expect(isPlanFirst(204)).toBe(true);
  });

  it('survives storage that holds something else', () => {
    localStorage.setItem('fleet.planFirst.v1', '[1,2]');
    expect(isPlanFirst(1)).toBe(false);
    markPlanFirst(1);
    expect(isPlanFirst(1)).toBe(true);
  });

  it('says where the plan goes and that the work waits', () => {
    const b = planFirstBrief('2026-11-10');
    expect(b).toContain('docs/plans/2026-11-10-<short-slug>.md');
    expect(b).toContain('Stop.');
    expect(withPlanFirst('Fix PROJ-1\n', '2026-11-10')).toBe(`Fix PROJ-1\n\n${b}`);
    expect(withPlanFirst('  ', '2026-11-10')).toBe(b);
    expect(planApprovedPrompt('docs/plans/a.md')).toContain('docs/plans/a.md is approved');
    expect(planDate(new Date(2026, 0, 5))).toBe('2026-01-05');
  });
});
