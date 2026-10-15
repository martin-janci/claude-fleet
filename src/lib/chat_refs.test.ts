import { describe, it, expect } from 'vitest';
import { hasRefs, refIndex, refSegments } from './chat_refs';

// Thread room: `#TASK` and `@host` / `@session` in a sent message are chips.

const index = refIndex(
  [
    { id: 3, tmux_name: 'fix-flake', friendly_name: 'Fix hub-e2e flake' },
    { id: 4, tmux_name: 'rel-062', friendly_name: 'release' },
  ],
  ['mercury', 'htz'],
);

describe('chat references', () => {
  it('finds hosts, sessions (by tmux or one-word friendly name) and task keys', () => {
    const segs = refSegments('Ask @fix-flake and @release on @mercury about #PD-2592.', index);
    expect(segs.map((s) => s.text).join('')).toBe('Ask @fix-flake and @release on @mercury about #PD-2592.');
    expect(segs.filter((s) => s.ref).map((s) => s.ref)).toEqual([
      { kind: 'session', id: 3, label: 'Fix hub-e2e flake' },
      { kind: 'session', id: 4, label: 'release' },
      { kind: 'host', alias: 'mercury' },
      { kind: 'task', key: 'PD-2592' },
    ]);
  });

  it('leaves what it does not know as text: an unknown name, an e-mail, a C# or a URL fragment', () => {
    const text = 'mail me@mercury.com, C# rocks, see /docs#intro and @nobody or #notakey';
    const segs = refSegments(text, index);
    expect(hasRefs(segs)).toBe(false);
    expect(segs).toEqual([{ text }]);
  });

  it('a reference at the very start and one in brackets', () => {
    const segs = refSegments('@htz (#FLT-12)', index);
    expect(segs.filter((s) => s.ref).map((s) => s.text)).toEqual(['@htz', '#FLT-12']);
    expect(segs.map((s) => s.text).join('')).toBe('@htz (#FLT-12)');
  });
});
