import { existsSync, readFileSync } from 'node:fs';
import { describe, it, expect } from 'vitest';

// Redesign step 0.2: the parity checklist is the contract every redesign PR
// is checked against, so its rows must point at tests that exist, and the PR
// template must ask for it. Step 13.1 archived it with the Classic layout:
// every row now reads New (or Removed, for the classic layout itself), and
// its proofs must still exist — a PR that deletes one updates the row.
describe('docs/redesign/archive/parity.md', () => {
  const md = readFileSync('docs/redesign/archive/parity.md', 'utf8');
  const rows = md.split('\n').filter((l) => /^\| [PH]\d+ \|/.test(l));

  it('every row names a step, a state and a test that exists', () => {
    expect(rows.length).toBeGreaterThanOrEqual(38);
    const bad: string[] = [];
    for (const r of rows) {
      const cells = r.split('|').map((c) => c.trim());
      const [id, , , , step, now, proof] = cells.slice(1);
      if (!/^\d+\.\d+/.test(step)) bad.push(`${id}: step "${step}"`);
      if (!['New', 'Removed'].includes(now)) bad.push(`${id}: now "${now}"`);
      const files = [...proof.matchAll(/`([^`]+\.test\.ts)`/g)].map((m) => m[1]);
      if (files.length === 0) bad.push(`${id}: no test named`);
      for (const f of files) if (!existsSync(f)) bad.push(`${id}: ${f} does not exist`);
    }
    expect(bad).toEqual([]);
  });

  it('row ids are unique', () => {
    const ids = rows.map((r) => r.split('|')[1].trim());
    expect(ids.filter((id, i) => ids.indexOf(id) !== i)).toEqual([]);
  });

  it('the PR template asks for the parity rows', () => {
    const tpl = readFileSync('.github/pull_request_template.md', 'utf8');
    expect(tpl).toContain('## Redesign parity');
    expect(tpl).toContain('docs/redesign/archive/parity.md');
  });
});
