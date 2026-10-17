import { describe, it, expect } from 'vitest';
import { followMarks, mutedKeys, stillFollowed, type ControlJournal } from './control_journal';

describe('control_journal follow marks (F10)', () => {
  const journal: ControlJournal = {
    follows: [
      { kind: 'session', id: 4, state: 'following', name: 'a', muted: false },
      { kind: 'session', id: 5, state: 'quiet', name: 'b', muted: false },
      { kind: 'mission', id: 9, state: 'muted', name: 'm', muted: true },
    ],
  };

  it('keys each thread as the threads column does', () => {
    const marks = followMarks(journal)!;
    expect([...marks.keys()]).toEqual(['s:4', 's:5', 'm:9']);
    expect([...mutedKeys(journal)]).toEqual(['m:9']);
    expect(followMarks(null)).toBeNull();
  });

  it('a thread is still followed only when following and not muted; unknown counts as followed', () => {
    const marks = followMarks(journal)!;
    expect(stillFollowed(marks.get('s:4'))).toBe(true);
    expect(stillFollowed(marks.get('s:5'))).toBe(false);
    expect(stillFollowed(marks.get('m:9'))).toBe(false);
    expect(stillFollowed(undefined)).toBe(true);
  });
});
