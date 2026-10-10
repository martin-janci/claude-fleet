// "Import plan": a markdown plan read into step rows, the transition
// plan's shape included (milestone tables plus a Lanes table).
import { describe, it, expect } from 'vitest';
import { importLine, initialRepoPicks, matchRepo, missingRepoLines, parsePlan, shortTitle, wireRows } from './plan_import';

const PLAN = `
# Plan

### M3 · The new shell

| # | Step | Layer | Needs | Verified by |
| --- | --- | --- | --- | --- |
| 3.1 | **Layout switch**; a pref that picks the shell | UI | 0.3 | Vitest |
| 3.2 | Rail with \`of-rail\` (see the board) | UI | 3.1, 0.8 | Vitest |
| 3.3 | Status bar \\| footer | UI | — | Vitest |

### M0

| # | Step | Layer | Needs | Verified by |
| --- | --- | --- | --- | --- |
| 0.3 | Shortcut registry | UI | — | test |
| 0.3 | Again | UI | — | test |

### Lanes

| Lane | Owns | Steps in order |
| --- | --- | --- |
| B · Shell and cutover | App.svelte | 0.3, 3.1, 3.2 |
| C · Design system | tokens | 0.8 |

### Waves

| Wave | Steps that can start | Count |
| --- | --- | --- |
| W0 | 0.3 | 1 |
`;

describe('parsePlan', () => {
  const p = parsePlan(PLAN);

  it('reads every step table in order, skipping a repeated step', () => {
    expect(p.rows.map((r) => r.step)).toEqual(['3.1', '3.2', '3.3', '0.3']);
    expect(p.notes).toEqual(['Step 0.3 is listed twice; the first one is kept.']);
  });

  it('takes the needs as step ids and drops dashes', () => {
    expect(p.rows.find((r) => r.step === '3.2')!.needs).toEqual(['3.1', '0.8']);
    expect(p.rows.find((r) => r.step === '3.3')!.needs).toBeUndefined();
  });

  it('gives each step its lane from the Lanes table', () => {
    expect(Object.fromEntries(p.rows.map((r) => [r.step, r.lane ?? null]))).toEqual({
      '3.1': 'Lane B',
      '3.2': 'Lane B',
      '3.3': null,
      '0.3': 'Lane B',
    });
  });

  it('keeps one plain line of the step as its title', () => {
    expect(p.rows[0].title).toBe('Layout switch');
    expect(p.rows[1].title).toBe('Rail with of-rail');
    expect(p.rows[2].title).toBe('Status bar | footer');
  });

  it('reads lane and status columns in a table of its own', () => {
    const own = parsePlan(`| Id | Title | Lane | Status | Depends on |
|---|---|---|---|---|
| 1.1 | Schema | Ana | done | |
| 1.2 | API | Bo | in progress | 1.1 |
| x | not a step | Bo | | |`);
    expect(own.rows).toEqual([
      { step: '1.1', title: 'Schema', lane: 'Ana', status: 'done' },
      { step: '1.2', title: 'API', lane: 'Bo', status: 'in_progress', needs: ['1.1'] },
    ]);
  });

  it('finds nothing in text without a step table', () => {
    expect(parsePlan('just words\n| a | b |\n').rows).toEqual([]);
  });
});

describe('shortTitle', () => {
  it('cuts a long step to its first clause and at most 120 characters', () => {
    expect(shortTitle('Board columns from tracker status names; a drag writes back')).toBe('Board columns from tracker status names');
    expect(shortTitle('x'.repeat(200)).length).toBe(120);
  });
});

describe('importLine', () => {
  it('says what changed', () => {
    expect(importLine({ created: 3, updated: 1, unchanged: 0, deps_added: 2, deps_removed: 1 })).toBe(
      'Imported: 3 added, 1 updated, 2 links added, 1 removed.',
    );
    expect(importLine({ created: 0, updated: 0, unchanged: 0, deps_added: 0, deps_removed: 0 })).toBe('Nothing to import.');
  });
});

describe('a repo per row (G2.5)', () => {
  const projects = [
    { id: 3, owner: 'acme', repo: 'api' },
    { id: 5, owner: 'acme', repo: 'web' },
    { id: 6, owner: 'other', repo: 'web' },
  ];
  const plan = parsePlan(`| # | Step | Repo |
|---|---|---|
| 1.1 | Schema | acme/api |
| 1.2 | UI | web |
| 1.3 | Docs | — |
| 1.4 | API | API |`);

  it('reads the Repo column; a table without one needs none', () => {
    expect(plan.hasRepos).toBe(true);
    expect(plan.rows.map((r) => r.repo)).toEqual(['acme/api', 'web', undefined, 'API']);
    expect(parsePlan('| # | Step |\n|---|---|\n| 1.1 | A |').hasRepos).toBe(false);
  });

  it('matches owner/repo, or a repository name only one project has', () => {
    expect(matchRepo('acme/api', projects)).toBe(3);
    expect(matchRepo('https://github.com/acme/web.git', projects)).toBe(5);
    expect(matchRepo('API', projects)).toBe(3);
    expect(matchRepo('web', projects)).toBeNull();
    expect(matchRepo('', projects)).toBeNull();
  });

  it('names each row with no repo, numbered from 1, and sends the picks without the cell', () => {
    const picks = initialRepoPicks(plan.rows, projects);
    expect(missingRepoLines(plan, picks)).toEqual(['Row 2 has no repo: pick one', 'Row 3 has no repo: pick one']);
    const done = { ...picks, '1.2': 6, '1.3': 5 };
    expect(missingRepoLines(plan, done)).toEqual([]);
    expect(wireRows(plan.rows, done)).toEqual([
      { step: '1.1', title: 'Schema', project_id: 3 },
      { step: '1.2', title: 'UI', project_id: 6 },
      { step: '1.3', title: 'Docs', project_id: 5 },
      { step: '1.4', title: 'API', project_id: 3 },
    ]);
  });
});
