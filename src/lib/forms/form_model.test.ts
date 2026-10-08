import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { checkAnswers, visibleSteps, stepProblems, type FormSpec } from './form_model';

const doc = JSON.parse(readFileSync('docs/form-examples/answers.json', 'utf8'));
const spec = doc.spec as FormSpec;

describe('form_model, against the shared cases', () => {
  for (const c of doc.cases) {
    it(c.name, () => {
      const r = checkAnswers(spec, c.values);
      if (c.problems) {
        expect(r.ok).toBe(false);
        if (!r.ok) expect(r.problems).toEqual(c.problems);
      } else {
        expect(r.ok).toBe(true);
        if (r.ok) {
          expect(r.answers).toEqual(c.answers);
          expect(r.secrets).toEqual(c.secrets);
        }
      }
    });
  }
});

describe('visibleSteps', () => {
  it('drops a step whose condition does not hold, and follows the answers', () => {
    expect(visibleSteps(spec, { db: false }).map((s) => s.title)).toEqual(['Basics', 'Notes']);
    expect(visibleSteps(spec, { db: true }).map((s) => s.title)).toEqual(['Basics', 'Database', 'Notes']);
    expect(visibleSteps(spec, { tags: ['b'] }).map((s) => s.title)).toEqual(['Basics', 'Features', 'Notes']);
  });
  it('drops a field whose condition does not hold', () => {
    const db = visibleSteps(spec, { db: true, engine: 'sqlite' })[1];
    expect(db.fields.map((f) => f.name)).toEqual(['engine']);
    const pg = visibleSteps(spec, { db: true, engine: 'pg' })[1];
    expect(pg.fields.map((f) => f.name)).toEqual(['engine', 'db_pass']);
  });
});

describe('stepProblems', () => {
  it('reports only the step it is asked about', () => {
    expect(stepProblems(spec, 0, { agree: true })).toEqual([{ field: 'name', problem: 'is required' }]);
    expect(stepProblems(spec, 0, { name: 'x', agree: true })).toEqual([]);
  });
});

describe('a field named like an Object.prototype member', () => {
  it('reads only own values', () => {
    const odd: FormSpec = {
      spec: 'fleet.form/1',
      title: 't',
      steps: [
        { title: 'a', fields: [{ name: 'constructor', type: 'text', label: 'C' }] },
        { title: 'b', when: { field: 'constructor', eq: 'x' }, fields: [{ name: 'z', type: 'text', label: 'Z' }] },
      ],
    };
    expect(visibleSteps(odd, {}).map((s) => s.title)).toEqual(['a']);
    expect(visibleSteps(odd, { constructor: 'x' }).map((s) => s.title)).toEqual(['a', 'b']);
  });
});
