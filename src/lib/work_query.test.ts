import { describe, expect, it } from 'vitest';
import { parseWorkQuery, suggestWorkQuery, type WorkQueryVocab } from './work_query';

const vocab: WorkQueryVocab = {
  facets: {
    iterations: [
      { name: 'Sprint 42', active: true, count: 3 },
      { name: 'Sprint 41', count: 1 },
    ],
    epics: [{ task_id: 'item:10', key: 'TK-10', title: 'Prihlásenie cez SSO', count: 2 }],
    item_types: ['Bug', 'Story'],
  },
  people: ['Ana Nováková'],
};

describe('parseWorkQuery', () => {
  it('turns finished tokens into filters and leaves the words', () => {
    const p = parseWorkQuery('sprint:current type:bug login fails ', vocab);
    expect(p.patch).toEqual({ iteration: 'current', item_type: 'Bug' });
    expect(p.rest).toBe('login fails ');
    expect(p.query).toBe('login fails');
  });

  it('keeps a token still being typed, out of the query', () => {
    const p = parseWorkQuery('login sprint:cur', vocab);
    expect(p.patch).toEqual({});
    expect(p.rest).toBe('login sprint:cur');
    expect(p.query).toBe('login');
  });

  it('takes the last token too on Enter, quoted names included', () => {
    const p = parseWorkQuery('sprint:"sprint 41"', vocab, true);
    expect(p.patch).toEqual({ iteration: 'Sprint 41' });
    expect(p.query).toBe('');
  });

  it('reads epics, people, statuses and flags', () => {
    const p = parseWorkQuery('epic:tk-10 assignee:"ana novakova" status:doing status:review is:review sort:key x', vocab);
    expect(p.patch).toEqual({
      epic: 'TK-10',
      mine: undefined,
      assignee: 'Ana Nováková',
      stages: ['in_progress', 'in_review'],
      review: true,
      sort: 'key',
    });
    expect(p.query).toBe('x');
  });

  it('leaves an unknown value in the box, and a word with a colon alone', () => {
    const p = parseWorkQuery('type:feature fix: login ', vocab);
    expect(p.patch).toEqual({});
    expect(p.rest).toBe('type:feature fix: login ');
    expect(p.query).toBe('fix: login');
  });
});

describe('suggestWorkQuery', () => {
  it('completes a field name', () => {
    expect(suggestWorkQuery('login sp', vocab).map((s) => s.input)).toEqual(['login sprint:']);
  });

  it('completes a value, quoting names with spaces', () => {
    const s = suggestWorkQuery('sprint:', vocab).map((x) => x.input);
    expect(s).toEqual(['sprint:current ', 'sprint:"Sprint 42" ', 'sprint:"Sprint 41" ', 'sprint:none ']);
    expect(suggestWorkQuery('epic:tk', vocab)[0]).toMatchObject({ input: 'epic:TK-10 ', hint: 'Prihlásenie cez SSO' });
    expect(suggestWorkQuery('type:bug assi', vocab).map((x) => x.input)).toEqual(['type:bug assignee:']);
    expect(suggestWorkQuery('assignee:"ana', vocab).map((x) => x.input)).toEqual(['assignee:"Ana Nováková" ']);
  });

  it('offers nothing for plain words', () => {
    expect(suggestWorkQuery('login ', vocab)).toEqual([]);
    expect(suggestWorkQuery('zzz', vocab)).toEqual([]);
  });
});
