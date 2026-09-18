/**
 * Golden regression against a real tmux 3.6a recording.
 *
 * `__fixtures__/tmux-attach.raw.txt` is exactly what an 80x24 client received
 * while attaching to a pane (the attach repaint) and then receiving
 * column-addressed updates; `tmux-attach.pane.txt` is tmux's own
 * `capture-pane` of that pane afterwards. If `Screen` disagrees with tmux about
 * how many columns a glyph takes (CJK, emoji, a combining accent, VS16, a ZWJ
 * sequence, a skin-tone modifier), the updates land in the wrong cells and the
 * rows stop matching.
 *
 * Regenerate with `scripts/capture-tmux-fixture.sh` (Linux, needs tmux).
 */
import { describe, it, expect } from 'vitest';
import { Screen } from './ansi';
import raw from './__fixtures__/tmux-attach.raw.txt?raw';
import pane from './__fixtures__/tmux-attach.pane.txt?raw';

const SENTINEL = '@@done@@';
/** Rows capture-pane covers: the 24-row client minus tmux's status line. */
const PANE_ROWS = 24; // the recording runs with tmux's status line off

const cut = raw.indexOf(SENTINEL);
// Everything after the sentinel is detach/teardown noise.
const stream = raw.slice(0, cut + SENTINEL.length);
const want = Array.from({ length: PANE_ROWS }, (_, r) => (pane.split('\n')[r] ?? '').trimEnd());

function paneRows(s: Screen): string[] {
  return s.cells.slice(0, PANE_ROWS).map((row) => row.map((c) => c.ch).join('').trimEnd());
}

/** mulberry32: a tiny deterministic PRNG so a failing chunking replays. */
function prng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

describe('Screen replays a tmux 3.6a attach recording', () => {
  it('the recording contains the sentinel and the rows under test', () => {
    expect(cut).toBeGreaterThan(0);
    expect(want.some((l) => l.startsWith('zwj:'))).toBe(true);
    expect(want.some((l) => l.startsWith('skin:'))).toBe(true);
  });

  it('matches capture-pane when fed in one write', () => {
    const s = new Screen(24, 80);
    s.write(stream);
    expect(paneRows(s)).toEqual(want);
  });

  it('copies the long line tmux left to autowrap as one line, without a newline at the wrap', () => {
    // tmux repaints the 125-char `long:` line as one run and lets the
    // terminal wrap it at column 80 (rows 8-9).
    const s = new Screen(24, 80);
    s.write(stream);
    const long = 'long:' + 'abcdefghij'.repeat(12);
    expect(s.selectionText({ row: 8, col: 0 }, { row: 9, col: 79 })).toBe(long);
  });

  it('matches capture-pane when fed in seeded random 1-64 char chunks', () => {
    for (let seed = 1; seed <= 50; seed++) {
      const rand = prng(seed);
      const s = new Screen(24, 80);
      for (let i = 0; i < stream.length; ) {
        const n = 1 + Math.floor(rand() * 64);
        s.write(stream.slice(i, i + n));
        i += n;
      }
      expect(paneRows(s), `seed ${seed}`).toEqual(want);
    }
  });
});
