import { describe, expect, it } from 'vitest';
import { leashFor } from './leash';

describe('leashFor (HITL audit H-01)', () => {
  it('a session a person started asks before a person’s step, by default', () => {
    const l = leashFor({ agent: 'claude', kind: 'claude', origin: 'person' }, true);
    expect(l?.label).toBe('Asks you first');
    expect(l?.text).toBe('Before merging or readying a PR, pushing to main, or writing to a tracker. Everything else runs unasked.');
    expect(l?.caveat).toContain('the step runs');
  });

  it('a mission worker is refused them, whatever the setting', () => {
    expect(leashFor({ agent: 'claude', kind: 'claude', origin: 'mission' }, false)?.label).toBe('Refused');
  });

  it('with the setting off, says nothing asks', () => {
    expect(leashFor({ agent: 'claude', kind: 'claude', origin: 'person' }, false)?.label).toBe('Unasked');
  });

  it('says nothing for a shell or another agent', () => {
    expect(leashFor({ agent: 'shell', kind: 'shell', origin: 'person' }, true)).toBeNull();
    expect(leashFor({ agent: 'codex', kind: 'claude', origin: 'person' }, true)).toBeNull();
  });
});
