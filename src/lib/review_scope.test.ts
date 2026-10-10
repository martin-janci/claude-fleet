import { describe, it, expect } from 'vitest';
import { reviewPreamble, reviewPrompt, reviewerOf, reviewerSkills, REVIEW_SCOPES } from './review_scope';
import type { AssetInventoryRow } from './assets';

const row = (over: Partial<AssetInventoryRow>): AssetInventoryRow => ({
  host_alias: 'mac', harness: 'claude', kind: 'skill', name: 's', state: 'in_sync',
  catalog_hash: null, host_hash: null, scanned_at: 0, managed: true, ...over,
});

describe('review scope and reviewer', () => {
  it('puts the skill, the scope and the read-only rule ahead of the text', () => {
    const p = reviewPrompt('pr-review', 'uncommitted', '  Look hard.  ');
    expect(p.split('\n')).toEqual([
      'Use the pr-review skill for this review.',
      REVIEW_SCOPES.find((s) => s.value === 'uncommitted')!.line,
      'Read and comment only: never commit, push or edit files.',
      '',
      'Look hard.',
    ]);
  });

  it('names no skill when none is picked', () => {
    expect(reviewPreamble(null, 'branch')).not.toMatch(/skill/);
    expect(reviewPreamble(null, 'last_commit')).toMatch(/git show HEAD/);
  });

  it("offers only the session host's Claude skills, review skills first", () => {
    const rows = [
      row({ name: 'worktree' }),
      row({ name: 'pr-review' }),
      row({ name: 'code-review', state: 'drifted' }),
      row({ name: 'gone-review', state: 'missing' }),
      row({ name: 'codex-review', harness: 'codex' }),
      row({ name: 'other-host-review', host_alias: 'nas' }),
      row({ name: 'an-agent', kind: 'agent' }),
      row({ name: 'pr-review' }),
    ];
    expect(reviewerSkills(rows, 'mac')).toEqual(['code-review', 'pr-review', 'worktree']);
  });
});

describe('reviewerOf (gap plan G4.3)', () => {
  it('names the skill from the seeded prompt and the agent', () => {
    expect(reviewerOf({ kind: 'review', agent: 'claude', last_prompt: 'Use the pr-review skill for this review.\nRead and comment only.' })).toBe(
      'pr-review skill · Claude Code',
    );
  });
  it('reads back what the Review dialog seeds', () => {
    expect(reviewerOf({ kind: 'review', agent: 'claude', last_prompt: reviewPrompt('code-review', 'branch', 'x') })).toBe(
      'code-review skill · Claude Code',
    );
  });
  it('is the agent alone when no skill is recorded', () => {
    expect(reviewerOf({ kind: 'review', agent: 'codex', last_prompt: 'please look' })).toBe('Codex');
    expect(reviewerOf({ kind: 'review', last_prompt: null })).toBe('Claude Code');
  });
});
