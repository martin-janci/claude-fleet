import { describe, expect, it } from 'vitest';
import { eventSentence } from './missions';

describe('the access check in a mission’s log', () => {
  const ev = (payload: unknown) => ({ id: 1, at: 1, kind: 'access_check', actor: 'loop', payload });

  it('names what it shared and how many things wait on a person', () => {
    const s = eventSentence(
      ev({
        targets: [
          { host: 'h1', profile: 'work', shared: ['atlassian', 'papaya'] },
          { host: 'h2', profile: 'work', shared: ['atlassian'] },
        ],
        problems: 1,
      }),
    );
    expect(s).toBe("Checked access on 2 hosts; shared atlassian, papaya with the run's login; 1 thing needs you");
  });

  it('says all is reachable when nothing waits', () => {
    expect(eventSentence(ev({ targets: [{ host: 'h1' }], problems: 0 }))).toBe('Checked access on 1 host; all reachable');
    expect(eventSentence(ev(undefined))).toBe('Checked access on 0 hosts; all reachable');
  });
});
