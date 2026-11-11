import { describe, it, expect } from 'vitest';
import { persistentRule } from './answer_guard';

describe('persistentRule', () => {
  it('reads the rule and the folder from Claude Code’s label', () => {
    expect(persistentRule('Yes, and don’t ask again for: git push:* in /home/user/claude-fleet')).toEqual({
      rule: 'git push:*',
      folder: '/home/user/claude-fleet',
    });
  });

  it('still asks when the label names no rule', () => {
    expect(persistentRule("Yes, and don't ask again")).toEqual({ rule: null, folder: null });
  });

  it('leaves a session-only choice and plain answers alone', () => {
    expect(persistentRule("Yes, and don't ask again this session")).toBeNull();
    expect(persistentRule('Yes, allow all edits during this session')).toBeNull();
    expect(persistentRule('Yes')).toBeNull();
    expect(persistentRule('No, and tell Claude what to do differently')).toBeNull();
    expect(
      persistentRule("Yes, and always allow in this conversation for commands that start with 'python cli.py'"),
    ).toBeNull();
  });

  it('fails closed on other wordings of a rule that outlives the session', () => {
    expect(
      persistentRule("Yes, and always allow for commands that start with 'python cli.py' (Persist to settings.json)"),
    ).toEqual({ rule: null, folder: null });
    expect(persistentRule('Yes, always allow git push')).toEqual({ rule: null, folder: null });
    expect(persistentRule('Yes, allow all edits')).toEqual({ rule: null, folder: null });
  });
});
