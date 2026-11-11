import { describe, it, expect } from 'vitest';
import { persistentRule } from './answer_guard';

const of = (label: string, persists?: boolean) => persistentRule({ label, persists });

describe('persistentRule', () => {
  it('reads the rule and the folder from Claude Code’s label', () => {
    expect(of('Yes, and don’t ask again for: git push:* in /home/user/claude-fleet')).toEqual({
      rule: 'git push:*',
      folder: '/home/user/claude-fleet',
      saved: true,
    });
  });

  it('reads a rule with no folder, as Claude Code 2.1.267 writes it', () => {
    expect(of('Yes, and don’t ask again for: ls *', true)).toEqual({ rule: 'ls *', folder: null, saved: true });
  });

  it('takes the last " in <path>" as the folder: a rule may hold " in " itself', () => {
    expect(of("Yes, and don't ask again for: for i in a b in /srv/app")).toEqual({
      rule: 'for i in a b',
      folder: '/srv/app',
      saved: true,
    });
  });

  it('still asks when the label names no rule', () => {
    expect(of("Yes, and don't ask again")).toEqual({ rule: null, folder: null, saved: true });
  });

  it('asks for a choice the hub flags though it saves no rule', () => {
    expect(of('Yes, and switch to auto mode · auto mode handles these prompts for you', true)).toEqual({
      rule: null,
      folder: null,
      saved: false,
    });
  });

  it('leaves a session-only choice and plain answers alone', () => {
    expect(of("Yes, and don't ask again this session")).toBeNull();
    expect(of('Yes, allow all edits during this session')).toBeNull();
    expect(of('Yes')).toBeNull();
    expect(of('No, and tell Claude what to do differently', false)).toBeNull();
  });

  it('reads the session phrase at the end only', () => {
    expect(of("Yes, and don't ask again for: ls * in /srv/the session logs")).not.toBeNull();
  });

  it('asks about other wordings of a choice that outlives the session', () => {
    expect(
      of("Yes, and always allow for commands that start with 'python cli.py' (Persist to settings.json)"),
    ).toEqual({ rule: null, folder: null, saved: true });
    expect(of('Yes, always allow git push')).toEqual({ rule: null, folder: null, saved: true });
    // The hub flags a bare "allow all" (pane_intel::persists_label); the card follows the flag.
    expect(of('Yes, allow all edits', true)).toEqual({ rule: null, folder: null, saved: false });
  });
});
