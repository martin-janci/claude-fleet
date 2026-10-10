import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { checkAnswers, visibleSteps, stepProblems, startingValues, type FormSpec } from './form_model';

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
    expect(visibleSteps(spec, { db: false }).map((s) => s.title)).toEqual(['Basics', 'Notes', 'Review']);
    expect(visibleSteps(spec, { db: true }).map((s) => s.title)).toEqual(['Basics', 'Database', 'Notes', 'Review']);
    expect(visibleSteps(spec, { tags: ['b'] }).map((s) => s.title)).toEqual(['Basics', 'Features', 'Notes', 'Review']);
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

describe('unknown keys', () => {
  it('are reported in sorted order, as the Rust validator does', () => {
    const r = checkAnswers(spec, { zz: 1, aa: 2, mm: 3 });
    expect(r.ok).toBe(false);
    if (!r.ok) {
      const unknown = r.problems.filter((p) => p.problem === 'is not a field of this form');
      expect(unknown.map((p) => p.field)).toEqual(['aa', 'mm', 'zz']);
    }
  });
});

// Review r09: AI never decides. An agent's form does not start with a
// required box ticked, nor with a risky option chosen; the app's own wizards
// keep their defaults.
describe('startingValues', () => {
  const risky: FormSpec = {
    spec: 'fleet.form/1',
    title: 'Ship it',
    steps: [
      {
        title: 'One',
        fields: [
          { name: 'agree', type: 'bool', label: 'I have read the plan', required: true, value: true },
          { name: 'push', type: 'bool', label: 'Push to origin when done', value: true },
          { name: 'tests', type: 'bool', label: 'Run the tests', value: true },
          { name: 'target', type: 'select', label: 'Where', value: 'prod', options: [['prod', 'Deploy to production'], ['stage', 'Staging']] },
          { name: 'safe', type: 'select', label: 'Branch', value: 'stage', options: [['prod', 'Deploy to production'], ['stage', 'Staging']] },
          { name: 'steps', type: 'multiselect', label: 'Steps', value: ['lint', 'approve'], options: [['lint', 'Lint'], ['approve', 'Approve the PR']] },
          { name: 'token', type: 'secret', label: 'Token', value: 'x' },
        ],
      },
    ],
  };

  it("drops an agent's required tick and its risky choices, and keeps the rest", () => {
    expect(startingValues(risky, true)).toEqual({ agree: false, push: false, tests: true, safe: 'stage', steps: ['lint'] });
  });

  it("keeps the app's own defaults, never a secret's", () => {
    expect(startingValues(risky, false)).toEqual({
      agree: true,
      push: true,
      tests: true,
      target: 'prod',
      safe: 'stage',
      steps: ['lint', 'approve'],
    });
  });
});
