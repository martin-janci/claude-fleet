import { beforeEach, describe, expect, it } from 'vitest';
import type { ConvItem, ConvTurn } from './conversation';
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
  startBriefArgs,
  withPlanFirst,
} from './plan_first';

const f = (path: string, status = 'untracked'): ChangedFile => ({ path, status, staged: false, orig_path: null });
const turn = (prompt: string | null, at: string, ...said: string[]): ConvTurn => ({
  prompt,
  at,
  ended_at: at,
  items: said.map((text) => ({ kind: 'text', text })),
});
const text = (words: string): ConvItem => ({ kind: 'text', text: words });
const tool = (summary: string): ConvItem => ({ kind: 'tool', summary, id: null, name: 'Bash', target: null, at: null, ended_at: null, done: true });

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

  it('counts only what the agent said, not what a command printed', () => {
    const t = turn('x', 't1');
    // A search for the words, or a file that holds them, is not the agent saying so.
    expect(planSaid([{ ...t, items: [text('Looking at the last plan.'), tool('grep -r "Plan ready" docs/plans/a.md')] }])).toBeNull();
    // What it said around a tool call is heard as it was said, one line each.
    expect(planSaid([{ ...t, items: [text('Wrote it.'), tool('Write docs/plans/a.md'), text('Plan ready: docs/plans/a.md')] }])).toEqual({
      text: 'Wrote it.\nPlan ready: docs/plans/a.md',
      at: 't1',
    });
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
    // The edited one sorts last by name here, and the new one still wins.
    expect(
      planFile(
        [f('docs/plans/2026-11-12-later.md', 'modified'), f('docs/plans/2026-11-10-b.md')],
        'Plan ready: 2026-11-10-b.md. I also fixed a link in 2026-11-12-later.md',
      ),
    ).toBe('docs/plans/2026-11-10-b.md');
    // A plan that was committed and then changed is announced as an edited file.
    expect(planFile([f('docs/plans/2026-11-10-retry.md', 'modified'), f('src/a.ts', 'modified')], reply)).toBe('docs/plans/2026-11-10-retry.md');
    expect(planFile([f('docs/plans/a.md', 'modified'), f('docs/plans/b.md', 'modified')], 'Plan ready: a.md and b.md')).toBe('docs/plans/b.md');
  });

  it('an answered announcement stays answered on this device; the next one asks again', () => {
    expect(planSettled(7, 'docs/plans/a.md', 't1')).toBe(false);
    settlePlan(7, 'docs/plans/a.md', 't1');
    expect(planSettled(7, 'docs/plans/a.md', 't1')).toBe(true);
    // The same file, said again after another prompt, and another session.
    expect(planSettled(7, 'docs/plans/a.md', 't2')).toBe(false);
    expect(planSettled(8, 'docs/plans/a.md', 't1')).toBe(false);
    // Another plan, announced before any other prompt (a turn no prompt opened).
    expect(planSettled(7, 'docs/plans/b.md', 't1')).toBe(false);
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
    // A start sends the preview only for an older hub: the backend builds
    // the ticket's own brief and puts the paragraph after it.
    const base = { briefOn: true, edited: false, text: 'ABC-7 preview', today: '2026-11-10' };
    expect(startBriefArgs({ ...base, planFirstOn: false })).toEqual({});
    expect(startBriefArgs({ ...base, planFirstOn: false, edited: true })).toEqual({ brief: 'ABC-7 preview' });
    expect(startBriefArgs({ ...base, planFirstOn: true })).toEqual({
      brief: withPlanFirst('ABC-7 preview', '2026-11-10'),
      plan_first: b,
      brief_edited: false,
    });
    expect(startBriefArgs({ ...base, planFirstOn: true, edited: true }).brief_edited).toBe(true);
    expect(startBriefArgs({ ...base, briefOn: false, planFirstOn: true })).toEqual({});
    expect(planApprovedPrompt('docs/plans/a.md')).toContain('docs/plans/a.md is approved');
    expect(planChangesPrefix('docs/plans/a.md')).toContain('say "Plan ready" again');
    expect(planDate(new Date(2026, 0, 5))).toBe('2026-01-05');
    // The day on this machine's clock, at both ends of it (UTC's differs east and west of Greenwich).
    expect(planDate(new Date(2026, 0, 5, 0, 30))).toBe('2026-01-05');
    expect(planDate(new Date(2026, 0, 5, 23, 30))).toBe('2026-01-05');
  });
});
