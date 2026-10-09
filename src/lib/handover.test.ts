import { describe, it, expect } from 'vitest';
import { handoverMarker, listItems, parseHandover } from './handover';
import { TASK_223 } from './handover_fixture';

describe('handoverMarker', () => {
  it('reads both markers, chrome and wrapping ignored', () => {
    expect(handoverMarker('WORK_HANDOVER_BEGIN_7c86f9ed8943')).toEqual({ end: false, nonce: '7c86f9ed8943' });
    expect(handoverMarker('⏺ `WORK_HANDOVER_END_ab12`')).toEqual({ end: true, nonce: 'ab12' });
  });
  it('is null for prose that mentions a marker, and for an empty nonce', () => {
    expect(handoverMarker('Put it after WORK_HANDOVER_BEGIN_ab12')).toBeNull();
    expect(handoverMarker('WORK_HANDOVER_BEGIN_')).toBeNull();
    expect(handoverMarker('FLEET_TASK_DONE_ab12')).toBeNull();
  });
});

describe('parseHandover', () => {
  it('reads the title line and every section of a real hand-off', () => {
    const h = parseHandover(TASK_223);
    expect(h.key).toBe('TASK-223');
    expect(h.title).toBe('Polish functionalities');
    expect(h.intro).toBe('');
    expect(h.sections.map((s) => s.kind)).toEqual(['work', 'decisions', 'done', 'where', 'blockers', 'next', 'gotchas']);
    const by = Object.fromEntries(h.sections.map((s) => [s.kind, s]));
    expect(by.work.body).toMatch(/^Unknown\. The only brief/);
    expect(by.decisions.heading).toBe('What the user decided');
    expect(by.where.items).toHaveLength(3);
    expect(by.blockers.heading).toBe('What blocked closing it');
    expect(by.blockers.items).toHaveLength(2);
    expect(by.next.items).toHaveLength(3);
    expect(by.next.items[1]).toBe('If it is still forbidden, ask the user to close the ticket in the Work view.');
    expect(by.gotchas.items).toHaveLength(2);
  });

  it('reads short "Done:" / "Left:" headings and bold or # ones', () => {
    const h = parseHandover('Done: the parser.\nLeft: tests.\n**Gotchas:** none\n## Next steps\n- ship it');
    expect(h.sections.map((s) => [s.kind, s.body])).toEqual([
      ['done', 'the parser.'],
      ['left', 'tests.'],
      ['next', '- ship it'],
      ['gotchas', 'none'],
    ]);
  });

  it('keeps a sentence that only starts like a heading in its section', () => {
    const h = parseHandover('What is done.\nCompleted the migration. Next it needs a test.\nNext, run the tests.');
    expect(h.sections).toHaveLength(1);
    expect(h.sections[0].body).toBe('Completed the migration. Next it needs a test.\nNext, run the tests.');
  });

  it('drops a parenthetical from the heading', () => {
    const h = parseHandover('Where things are (branch, files, commands): branch `x`.');
    expect(h.sections[0]).toMatchObject({ kind: 'where', heading: 'Where things are', body: 'branch `x`.' });
  });

  it('is all intro when no heading is known', () => {
    const h = parseHandover('The parser landed; tests are left.\nAsk before pushing.');
    expect(h.sections).toEqual([]);
    expect(h.intro).toBe('The parser landed; tests are left.\nAsk before pushing.');
    expect(h.key).toBeNull();
    expect(h.words).toBe(9);
  });
});

describe('listItems', () => {
  it('joins indented continuations and is empty for prose', () => {
    expect(listItems('1. one\n   more\n2. two')).toEqual(['one more', 'two']);
    expect(listItems('- a\nprose after')).toEqual([]);
    expect(listItems('just prose')).toEqual([]);
  });
});
