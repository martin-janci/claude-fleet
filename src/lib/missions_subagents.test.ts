// Mission improvements M7: the subagents a task's run used, on its row.
import { describe, it, expect } from 'vitest';
import { attemptLine, subagentsText } from './missions';

describe('a task\'s subagents', () => {
  it('counts them, the most used type first', () => {
    expect(subagentsText({ 'general-purpose': 1, Explore: 2 })).toBe('3 subagents (Explore 2, general-purpose 1)');
    expect(subagentsText({ Explore: 1 })).toBe('1 subagent (Explore 1)');
    expect(subagentsText({})).toBeNull();
    expect(subagentsText(undefined)).toBeNull();
  });

  it('ride the attempt line, after what git showed', () => {
    expect(
      attemptLine({
        task_id: 1,
        role: 'implement',
        attempt: 1,
        state: 'done',
        evidence: { commits_total: 1, files_total: 2 } as never,
        subagents: { Explore: 2 },
      }),
    ).toBe('implement #1 · done · 1 commit, 2 files · 2 subagents (Explore 2)');
  });
});
