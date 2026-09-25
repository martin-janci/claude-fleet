import { describe, it, expect } from 'vitest';
import fixture from '../../crates/fleet-core/src/service/work/testdata/recognize_cases.json';
import {
  describeWorkKey,
  extractTicketRefs,
  extractWorkKey,
  keyFromTicketUrl,
  workGroupPrSummary,
  workKeyFor,
  worktreeBranchById,
  type RecognizeCtx,
  type TicketRef,
} from './work_keys';
import type { SessionRow } from './sessions';
import type { ProjectTreeRow } from './projects';

// Only the fields work_keys reads; the rest of a SessionRow is irrelevant here.
function sess(over: Partial<SessionRow>): SessionRow {
  return {
    id: 1,
    tags: [],
    worktree_id: null,
    worktree_key: null,
    pr_url: null,
    ci_status: null,
    ...over,
  } as SessionRow;
}

describe('extractWorkKey', () => {
  it.each([
    ['ABC-123', 'ABC-123'],
    ['feature/ABC-123-refresh-token', 'ABC-123'],
    ['abc-123-fix-login', 'ABC-123'],
    ['user/eng-42-slug', 'ENG-42'],
    ['ABC-123_fix', 'ABC-123'],
    ['ENG2-7', 'ENG2-7'],
    ['work:PAY-9', 'PAY-9'],
  ])('finds the key in %s', (text, key) => {
    expect(extractWorkKey(text)).toBe(key);
  });

  it.each([
    ['main'],
    ['fix-the-login-bug'],
    ['release-2024'],
    ['hotfix-2-login'],
    ['utf-8'],
    ['sha-256'],
    ['python-3-upgrade'],
    ['XABC-12x'],
    ['ABC-12abc'],
    ['dependabot/npm_and_yarn/lodash-4.17.21'],
    ['v2-3'],
    [''],
  ])('finds nothing in %s', (text) => {
    expect(extractWorkKey(text)).toBeNull();
  });

  it('takes an upper-case key even when its prefix is a denied word', () => {
    // Written in capitals on purpose: that is a key, not a word.
    expect(extractWorkKey('FIX-12')).toBe('FIX-12');
  });

  it('returns the first key of several', () => {
    expect(extractWorkKey('ABC-1-and-DEF-2')).toBe('ABC-1');
  });
});

describe('workKeyFor', () => {
  const projects = [
    {
      project: { id: 1 },
      worktrees: [
        { id: 10, branch: 'feature/ABC-123-x' },
        { id: 11, branch: 'plain-branch' },
        { id: 12, branch: null },
      ],
    },
  ] as unknown as ProjectTreeRow[];
  const branches = worktreeBranchById(projects);

  it('indexes only worktrees with a branch', () => {
    expect([...branches.keys()].sort()).toEqual([10, 11]);
  });

  it('prefers a tag over the branch', () => {
    const w = workKeyFor(sess({ tags: ['wip', 'PAY-7'], worktree_id: 10 }), branches);
    expect(w).toEqual({ key: 'PAY-7', source: 'tag', from: 'PAY-7' });
  });

  it('uses the worktree branch', () => {
    const w = workKeyFor(sess({ worktree_id: 10 }), branches);
    expect(w).toEqual({ key: 'ABC-123', source: 'branch', from: 'feature/ABC-123-x' });
    expect(describeWorkKey(w!)).toContain('branch feature/ABC-123-x');
  });

  it('falls back to the worktree directory name', () => {
    const w = workKeyFor(sess({ worktree_id: 99, worktree_key: 'eng-5-search' }), branches);
    expect(w).toEqual({ key: 'ENG-5', source: 'worktree', from: 'eng-5-search' });
  });

  it('prefers an explicit link over tags, branch and worktree', () => {
    const w = workKeyFor(
      sess({
        tags: ['PAY-7'],
        worktree_id: 10,
        work: { link_id: 3, item_id: 4, key: 'OPS-9', title: 'Rotate keys', source: 'manual' },
      }),
      branches,
    );
    expect(w).toEqual({ key: 'OPS-9', source: 'link', from: 'Rotate keys' });
    expect(describeWorkKey(w!)).toBe('OPS-9 — Rotate keys (linked)');
  });

  it('uses a linked item\'s title when it has no key', () => {
    const w = workKeyFor(
      sess({ work: { link_id: 3, item_id: 4, key: null, title: 'Billing migration', source: 'manual' } }),
      branches,
    );
    expect(w).toEqual({ key: 'Billing migration', source: 'link', from: 'Billing migration' });
    expect(describeWorkKey(w!)).toBe('Billing migration — linked to this session');
  });

  it('never recognises a key the user rejected for the row', () => {
    // The branch names ABC-123, the user said "Not this": the next source
    // down is used, and with none the row has no key.
    expect(
      workKeyFor(sess({ worktree_id: 10, work_rejected: ['ABC-123'] }), branches),
    ).toBeNull();
    expect(
      workKeyFor(
        sess({ tags: ['ABC-123'], worktree_id: 99, worktree_key: 'eng-5-x', work_rejected: ['ABC-123'] }),
        branches,
      ),
    ).toEqual({ key: 'ENG-5', source: 'worktree', from: 'eng-5-x' });
  });

  it('is null when nothing names a key', () => {
    expect(workKeyFor(sess({ worktree_id: 11, worktree_key: 'main' }), branches)).toBeNull();
  });
});

describe('workGroupPrSummary', () => {
  it('counts distinct PRs and reports the worst CI', () => {
    const rows = [
      sess({ pr_url: 'https://x/pull/1', ci_status: 'passing' }),
      sess({ pr_url: 'https://x/pull/1', ci_status: 'passing' }),
      sess({ pr_url: 'https://x/pull/2', ci_status: 'failing' }),
      sess({}),
    ];
    expect(workGroupPrSummary(rows)).toEqual({ prCount: 2, ci: 'failing' });
  });

  it('is empty without PRs', () => {
    expect(workGroupPrSummary([sess({})])).toEqual({ prCount: 0, ci: null });
  });
});

describe('keyFromTicketUrl', () => {
  it.each([
    ['https://acme.atlassian.net/browse/ABC-123', 'ABC-123'],
    ['https://acme.atlassian.net/jira/software/projects/ABC/boards/1?selectedIssue=ABC-9', 'ABC-9'],
    ['https://linear.app/acme/issue/ENG-42/fix-search', 'ENG-42'],
    ['  https://acme.atlassian.net/browse/PAY-7  ', 'PAY-7'],
  ])('%s → %s', (url, key) => {
    expect(keyFromTicketUrl(url)).toBe(key);
  });

  it.each([
    ['ABC-123 not a url'],
    ['https://github.com/o/r/pull/12'],
    ['see https://acme.atlassian.net/browse/ABC-1 please'],
    ['https://'],
  ])('ignores %s', (text) => {
    expect(keyFromTicketUrl(text)).toBeNull();
  });
});

// The recogniser's shared fixture (work graph M4.1): the same cases the hub's
// `recognize.rs` runs, so the two can never drift.

describe('extractTicketRefs (shared fixture)', () => {
  const cases = fixture as unknown as {
    name: string;
    text: string;
    ctx: RecognizeCtx;
    expect: TicketRef[];
  }[];

  it('has its cases', () => {
    expect(cases.length).toBeGreaterThanOrEqual(30);
  });

  it.each(cases.map((c) => [c.name, c] as const))('%s', (_name, c) => {
    expect(extractTicketRefs(c.text, c.ctx)).toEqual(c.expect);
  });
});
