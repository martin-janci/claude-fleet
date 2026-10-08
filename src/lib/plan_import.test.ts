// "Import plan": a markdown plan read into step rows, the transition
// plan's shape included (milestone tables plus a Lanes table).
import { describe, it, expect } from 'vitest';
import { importLine, parsePlan, shortTitle } from './plan_import';

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
