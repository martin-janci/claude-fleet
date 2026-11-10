import { beforeEach, describe, expect, it } from 'vitest';
import type { ConvTurn } from './conversation';
import type { ChangedFile } from './files';
import {
  planApprovedPrompt,
  planChangesPrefix,
  planDate,
  planFile,
  planFirstBrief,
  planSaid,
  planSettled,
  settlePlan,
  withPlanFirst,
} from './plan_first';

const f = (path: string, status = 'untracked'): ChangedFile => ({ path, status, staged: false, orig_path: null });
const turn = (prompt: string | null, at: string, ...said: string[]): ConvTurn => ({
  prompt,
  at,
  ended_at: at,
  items: said.map((text) => ({ kind: 'text', text })),
});

describe('plan first', () => {
  beforeEach(() => localStorage.clear());

  it('hears "Plan ready" in what the agent said since the last prompt', () => {
    expect(planSaid([])).toBeNull();
    expect(planSaid([turn('Add retries', 't1', 'Reading the code.')])).toBeNull();
    const said = planSaid([turn('Add retries', 't1', 'I read the loop.', '**Plan ready**: `docs/plans/2026-11-10-retry.md`')]);
    expect(said).toEqual({ text: 'I read the loop.\n**Plan ready**: `docs/plans/2026-11-10-retry.md`', at: 't1' });
    expect(planSaid([turn('x', 't1', 'The plan is ready in docs/plans/a.md.')])?.at).toBe('t1');
    // A turn no prompt opened (a notification) belongs to the prompt before it.
    expect(planSaid([turn('x', 't1', 'Plan ready: docs/plans/a.md'), turn(null, 't2', 'The build finished.')])?.at).toBe('t1');
    // The next prompt, whoever sent it, ends the announcement.
    expect(planSaid([turn('x', 't1', 'Plan ready: docs/plans/a.md'), turn('Looks good, go ahead', 't2', 'Starting step 1.')])).toBeNull();
    expect(planSaid([turn('x', 't1', 'Plan ready: docs/plans/a.md'), turn('go', 't2')])).toBeNull();
  });

  it('finds the plan the reply names among the changes', () => {
    const reply = 'Plan ready: docs/plans/2026-11-10-retry.md';
    expect(planFile([f('src/a.ts', 'modified')], reply)).toBeNull();
    expect(planFile([f('docs/plans/2026-11-10-retry.md'), f('src/a.ts', 'modified')], reply)).toBe('docs/plans/2026-11-10-retry.md');
    expect(planFile([f('web/docs/superpowers/plans/x.md', 'added')], 'Plan ready, see x.md')).toBe('web/docs/superpowers/plans/x.md');
    // A plan the reply does not name is somebody else's, or an older one.
    expect(planFile([f('docs/plans/2026-11-01-other.md')], reply)).toBeNull();
    // A deleted plan, a nested file and another kind of file are not one.
    expect(planFile([f('docs/plans/x.md', 'deleted'), f('docs/plans/img/x.md'), f('docs/plans/x.txt')], 'Plan ready: x.md x.txt')).toBeNull();
    // Several named: a new plan wins over an edited one; among new ones the last by name.
    expect(
      planFile(
        [f('docs/plans/2026-11-01-old.md', 'modified'), f('docs/plans/2026-11-09-a.md'), f('docs/plans/2026-11-10-b.md')],
        'Plan ready. I split it: 2026-11-01-old.md, 2026-11-09-a.md and 2026-11-10-b.md',
      ),
    ).toBe('docs/plans/2026-11-10-b.md');
  });

  it('an answered announcement stays answered on this device; the next one asks again', () => {
    expect(planSettled(7, 'docs/plans/a.md', 't1')).toBe(false);
    settlePlan(7, 'docs/plans/a.md', 't1');
    expect(planSettled(7, 'docs/plans/a.md', 't1')).toBe(true);
    // The same file, said again after another prompt, and another session.
    expect(planSettled(7, 'docs/plans/a.md', 't2')).toBe(false);
    expect(planSettled(8, 'docs/plans/a.md', 't1')).toBe(false);
  });

  it('keeps the newest 200 sessions, by when they were answered', () => {
    for (let i = 0; i < 205; i++) settlePlan(1000 - i, 'docs/plans/a.md', 't1');
    expect(planSettled(1000, 'docs/plans/a.md', 't1')).toBe(false);
    expect(planSettled(796, 'docs/plans/a.md', 't1')).toBe(true);
  });

  it('survives storage that holds something else', () => {
    localStorage.setItem('fleet.planSettled.v1', '{"7":"x"}');
    expect(planSettled(7, 'x', '')).toBe(false);
    settlePlan(7, 'docs/plans/a.md', 't1');
    expect(planSettled(7, 'docs/plans/a.md', 't1')).toBe(true);
    localStorage.setItem('fleet.planSettled.v1', 'not json');
    expect(planSettled(7, 'docs/plans/a.md', 't1')).toBe(false);
  });

  it('says where the plan goes, what to say, and that the work waits', () => {
    const b = planFirstBrief('2026-11-10');
    expect(b).toContain('docs/plans/2026-11-10-<short-slug>.md');
    expect(b).toContain('do not commit it');
    expect(b).toContain('Say "Plan ready"');
    expect(b).toContain('Stop.');
    expect(withPlanFirst('Fix PROJ-1\n', '2026-11-10')).toBe(`Fix PROJ-1\n\n${b}`);
    expect(withPlanFirst('  ', '2026-11-10')).toBe(b);
    expect(planApprovedPrompt('docs/plans/a.md')).toContain('docs/plans/a.md is approved');
    expect(planChangesPrefix('docs/plans/a.md')).toContain('say "Plan ready" again');
    expect(planDate(new Date(2026, 0, 5))).toBe('2026-01-05');
  });
});
