// TASK-291: the planner waiting on a usage limit, in words.
import { describe, it, expect } from 'vitest';
import { eventSentence, limitWaitSentence, plannerLimitWait, plannerWaiting, type LimitWait } from './missions';

const now = new Date(2026, 9, 19, 10, 0).getTime() / 1000;
const at = new Date(2026, 9, 19, 15, 0).getTime() / 1000;
const time = new Date(at * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });

const login: LimitWait = {
  login: { host: 'mac', profile: null },
  resets_at: at,
  reset_known: true,
  why: "mac's own login is at its usage limit",
};

describe('the planner at a usage limit', () => {
  it('says whose limit it waits on and when it resets', () => {
    expect(limitWaitSentence(login, now)).toBe(`Waiting on the usage limit of mac's own login, resets ${time}`);
    const work = { ...login, login: { host: 'mac', profile: 'work' } };
    expect(limitWaitSentence(work, now)).toBe(`Waiting on the usage limit of profile work on mac, resets ${time}`);
    const all = { ...login, login: null, reset_known: false, said: 'resets 3pm (Europe/Bratislava)' };
    expect(limitWaitSentence(all, now)).toBe(
      `Waiting on a usage limit on every login, asks again at ${time} (Claude: resets 3pm (Europe/Bratislava))`,
    );
  });

  it('a limit_wait event reads as a wait, not a refusal', () => {
    const s = eventSentence({ id: 1, at: now, kind: 'limit_wait', actor: 'planner', payload: { ...login } });
    expect(s.startsWith('Waiting on the usage limit')).toBe(true);
  });

  it('the mission waits only while the planner’s newest word is a wait', () => {
    const ev = (id: number, kind: string, payload?: unknown) => ({ id, at: id, kind, actor: 'planner', payload });
    expect(plannerLimitWait({ events: [ev(3, 'step'), ev(2, 'limit_wait', login), ev(1, 'planned')] })).toEqual(login);
    expect(plannerLimitWait({ events: [ev(3, 'planned'), ev(2, 'limit_wait', login)] })).toBeNull();
    expect(plannerLimitWait({ events: [] })).toBeNull();
    // Review r8: a later login of the same ask answered.
    expect(plannerLimitWait({ events: [ev(3, 'limit_cleared', { host: 'mac', profile: 'work' }), ev(2, 'limit_wait', login), ev(1, 'planned')] })).toBeNull();
    expect(eventSentence(ev(3, 'limit_cleared', { host: 'mac', profile: 'work' }))).toBe('The planner answered on profile work on mac');
  });

  it('reads the hub’s own wait over the events page, which a busy log outgrows (review r9)', () => {
    const ev = (id: number, kind: string, payload?: unknown) => ({ id, at: id, kind, actor: 'planner', payload });
    const busy = Array.from({ length: 50 }, (_, i) => ev(100 - i, 'card'));
    expect(plannerLimitWait({ events: busy, limit_wait: login })).toEqual(login);
    // `null` from the hub: not waiting, whatever the page holds.
    expect(plannerLimitWait({ events: [ev(2, 'limit_wait', login)], limit_wait: null })).toBeNull();
    // An older hub sends no `limit_wait`: the page is read.
    expect(plannerLimitWait({ events: [ev(2, 'limit_wait', login)] })).toEqual(login);
  });

  it('a refusal for signed-out logins says which to sign in again and ends the wait (review r10)', () => {
    const ev = (id: number, kind: string, payload?: unknown) => ({ id, at: id, kind, actor: 'loop', payload });
    const why = "no login the planner may use is signed in: mac's own login is signed out since today";
    expect(eventSentence(ev(3, 'planner_signed_out', { why }))).toBe(`The planner could not be asked: ${why}`);
    expect(plannerLimitWait({ events: [ev(3, 'planner_signed_out', { why }), ev(2, 'limit_wait', login)] })).toBeNull();
  });

  it('the Plan button says it waits, with the reason in Details', () => {
    const all = { ...login, login: null, why: 'every login the planner may use is at its usage limit' };
    const h = plannerWaiting(all, now);
    expect(h.title).toBe('The planner is waiting on a usage limit');
    expect(h.text).toContain(time);
    expect(h.details).toBe(all.why);
  });

  it('one login at its limit while another has headroom is no wait on every account (review r8)', () => {
    const h = plannerWaiting(login, now);
    expect(h.title).toBe('The planner hit a usage limit');
    expect(h.text).toBe(`Only mac's own login is at its usage limit until ${time}. Fleet asks the planner again on a login with headroom.`);
    expect(h.text).not.toContain('accounts');
    expect(h.details).toBe(login.why);
  });
});
