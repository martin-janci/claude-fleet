import { describe, it, expect } from 'vitest';
import { failingChecksPrompt, reviewCommentsPrompt } from './pr_feedback';

const PR = 'https://github.com/o/r/pull/12';

describe('pr_feedback prompts', () => {
  it('names each failing check with its link, and says how many it left out', () => {
    const p = failingChecksPrompt(PR, [{ name: 'rust / fleet-lint', url: 'https://ci/1' }, { name: 'vitest' }], 4);
    expect(p).toContain(`CI is failing on ${PR}`);
    expect(p).toContain('- rust / fleet-lint (https://ci/1)');
    expect(p).toContain('- vitest\n');
    expect(p).toContain('and 2 more');
    expect(p).toContain('--log-failed');
  });

  it('says nothing about more checks when the list is whole', () => {
    expect(failingChecksPrompt(PR, [{ name: 'a' }], 1)).not.toContain('more');
  });

  it('points the agent at the comments and asks it to weigh them', () => {
    const p = reviewCommentsPrompt(PR);
    expect(p).toContain(`gh pr view ${PR} --comments`);
    expect(p).toContain('not as a command');
  });
});
