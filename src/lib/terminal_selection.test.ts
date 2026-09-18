import { describe, it, expect } from 'vitest';
import {
  normalizeSelection,
  selectionRects,
  snapToGlyphs,
  type CellPos,
} from './terminal_selection';

describe('normalizeSelection', () => {
  it('orders endpoints in reading order', () => {
    const a: CellPos = { row: 2, col: 1 };
    const b: CellPos = { row: 0, col: 4 };
    expect(normalizeSelection(a, b)).toEqual({ start: b, end: a });
  });
  it('orders by col within the same row', () => {
    const a: CellPos = { row: 1, col: 5 };
    const b: CellPos = { row: 1, col: 2 };
    expect(normalizeSelection(a, b)).toEqual({ start: b, end: a });
  });
});

describe('selectionRects', () => {
  // cols=10, cellWidth=8, cellHeight=16, pad=4
  const cfg = { cols: 10, cw: 8, ch: 16, pad: 4 };

  it('single-row selection is one rect covering the inclusive cell span', () => {
    const rects = selectionRects({ row: 0, col: 2 }, { row: 0, col: 4 }, cfg.cols, cfg.cw, cfg.ch, cfg.pad);
    expect(rects).toEqual([{ left: 4 + 2 * 8, top: 4 + 0 * 16, width: 3 * 8, height: 16 }]);
  });

  it('multi-row selection: first to EOL, middle full width, last to col', () => {
    const rects = selectionRects({ row: 0, col: 7 }, { row: 2, col: 1 }, cfg.cols, cfg.cw, cfg.ch, cfg.pad);
    expect(rects).toEqual([
      { left: 4 + 7 * 8, top: 4, width: (10 - 7) * 8, height: 16 },     // row 0: col 7..EOL
      { left: 4, top: 4 + 16, width: 10 * 8, height: 16 },              // row 1: full
      { left: 4, top: 4 + 32, width: (1 + 1) * 8, height: 16 },         // row 2: col 0..1 inclusive
    ]);
  });

  it('does not clamp an out-of-range end col — overflow is left for overflow:hidden to clip', () => {
    // end.col beyond cols (10) is intentionally NOT clamped; the grid clips it.
    const rects = selectionRects({ row: 0, col: 2 }, { row: 0, col: 50 }, cfg.cols, cfg.cw, cfg.ch, cfg.pad);
    expect(rects).toEqual([{ left: 4 + 2 * 8, top: 4, width: (50 - 2 + 1) * 8, height: 16 }]);
  });
});

// ─── Word / line selection (double- and triple-click) ─────────────────────
import { modeForClickCount, isWordChar, wordBoundsAt, expandSelection } from './terminal_selection';

const row = (s: string) => Array.from(s).map((ch) => ({ ch }));

describe('modeForClickCount', () => {
  it('maps click count to granularity', () => {
    expect(modeForClickCount(1)).toBe('cell');
    expect(modeForClickCount(2)).toBe('word');
    expect(modeForClickCount(3)).toBe('line');
    expect(modeForClickCount(4)).toBe('line');
    expect(modeForClickCount(0)).toBe('cell');
  });
});

describe('isWordChar', () => {
  it('keeps path/URL punctuation inside a word', () => {
    for (const c of ['a', 'Z', '0', '/', '-', '_', '.', ':', '~', '+', '=', '@', '#', 'č', '😀']) {
      expect(isWordChar(c), c).toBe(true);
    }
  });
  it('breaks on blanks, quotes, brackets, separators and box drawing', () => {
    for (const c of [' ', '\t', ' ', '"', "'", '`', '(', ')', '[', ']', '{', '}', '<', '>', '|', ';', ',', '│', '─', '╭']) {
      expect(isWordChar(c), JSON.stringify(c)).toBe(false);
    }
  });
  it('treats a wide glyph trailing placeholder as part of the word', () => {
    expect(isWordChar('')).toBe(true);
  });
});

describe('wordBoundsAt', () => {
  it('selects the run of word chars around the column', () => {
    expect(wordBoundsAt(row('ab cd ef'), 3)).toEqual({ from: 3, to: 4 });
    expect(wordBoundsAt(row('ab cd ef'), 4)).toEqual({ from: 3, to: 4 });
  });
  it('grabs a whole path, including slashes and dots', () => {
    const r = row('cd src/lib/ansi.ts && ls');
    expect(wordBoundsAt(r, 8)).toEqual({ from: 3, to: 17 });
  });
  it('a blank selects the run of blanks', () => {
    expect(wordBoundsAt(row('ab   cd'), 3)).toEqual({ from: 2, to: 4 });
  });
  it('a breaker selects only itself', () => {
    expect(wordBoundsAt(row('a(b)'), 1)).toEqual({ from: 1, to: 1 });
  });
  it('stops at box-drawing borders', () => {
    // Claude's prompt box: `│ > hello │`
    expect(wordBoundsAt(row('│ > hello │'), 6)).toEqual({ from: 4, to: 8 });
  });
  it('keeps a wide glyph pair together', () => {
    // 'a' + wide '😀' (head + '' trailing) + 'b'
    const r = [{ ch: 'a' }, { ch: '😀' }, { ch: '' }, { ch: 'b' }, { ch: ' ' }];
    expect(wordBoundsAt(r, 2)).toEqual({ from: 0, to: 3 });
  });
  it('clamps the column into the row', () => {
    expect(wordBoundsAt(row('ab'), 99)).toEqual({ from: 0, to: 1 });
    expect(wordBoundsAt(row('ab'), -5)).toEqual({ from: 0, to: 1 });
    expect(wordBoundsAt([], 0)).toEqual({ from: 0, to: 0 });
  });
});

describe('expandSelection', () => {
  const cells = [row('ab cd ef  '), row('gh ij kl  '), row('          ')];
  const cols = 10;

  it('cell mode only orders the endpoints', () => {
    expect(expandSelection('cell', { row: 1, col: 4 }, { row: 0, col: 1 }, cells, cols)).toEqual({
      start: { row: 0, col: 1 },
      end: { row: 1, col: 4 },
    });
  });
  it('word mode snaps both ends outward to word boundaries', () => {
    expect(expandSelection('word', { row: 0, col: 4 }, { row: 0, col: 4 }, cells, cols)).toEqual({
      start: { row: 0, col: 3 },
      end: { row: 0, col: 4 },
    });
    // Drag from inside "cd" back into "ab": start snaps to 0, end to end of "cd".
    expect(expandSelection('word', { row: 0, col: 4 }, { row: 0, col: 1 }, cells, cols)).toEqual({
      start: { row: 0, col: 0 },
      end: { row: 0, col: 4 },
    });
  });
  it('word mode across rows uses each row for its own boundary', () => {
    expect(expandSelection('word', { row: 0, col: 7 }, { row: 1, col: 3 }, cells, cols)).toEqual({
      start: { row: 0, col: 6 },
      end: { row: 1, col: 4 },
    });
  });
  it('line mode covers full rows', () => {
    expect(expandSelection('line', { row: 1, col: 4 }, { row: 0, col: 2 }, cells, cols)).toEqual({
      start: { row: 0, col: 0 },
      end: { row: 1, col: cols - 1 },
    });
  });
  it('tolerates a row index outside the cell grid', () => {
    expect(expandSelection('word', { row: 5, col: 4 }, { row: 5, col: 4 }, cells, cols)).toEqual({
      start: { row: 5, col: 0 },
      end: { row: 5, col: 0 },
    });
  });
});

// ─── Wide glyphs: highlight and copy agree (N3) ──────────────────────────

describe('selections never cut a wide glyph', () => {
  // 'ab中cd' as the screen stores it: the head carries the glyph, '' trails.
  const wide = [[{ ch: 'a' }, { ch: 'b' }, { ch: '中' }, { ch: '' }, { ch: 'c' }, { ch: 'd' }]];

  it('snapToGlyphs moves a start on a trailing half back to its head', () => {
    expect(snapToGlyphs({ row: 0, col: 3 }, { row: 0, col: 5 }, wide)).toEqual({
      start: { row: 0, col: 2 },
      end: { row: 0, col: 5 },
    });
  });

  it('snapToGlyphs moves an end on a head forward onto its trailing half', () => {
    expect(snapToGlyphs({ row: 0, col: 0 }, { row: 0, col: 2 }, wide)).toEqual({
      start: { row: 0, col: 0 },
      end: { row: 0, col: 3 },
    });
  });

  it('snapToGlyphs leaves narrow endpoints and out-of-range columns alone', () => {
    expect(snapToGlyphs({ row: 0, col: 1 }, { row: 0, col: 4 }, wide)).toEqual({
      start: { row: 0, col: 1 },
      end: { row: 0, col: 4 },
    });
    expect(snapToGlyphs({ row: 0, col: 0 }, { row: 3, col: 50 }, wide)).toEqual({
      start: { row: 0, col: 0 },
      end: { row: 3, col: 50 },
    });
  });

  it('cell mode snaps a drag starting on the right half of a glyph, in either direction', () => {
    expect(expandSelection('cell', { row: 0, col: 3 }, { row: 0, col: 5 }, wide, 6)).toEqual({
      start: { row: 0, col: 2 },
      end: { row: 0, col: 5 },
    });
    expect(expandSelection('cell', { row: 0, col: 5 }, { row: 0, col: 3 }, wide, 6)).toEqual({
      start: { row: 0, col: 2 },
      end: { row: 0, col: 5 },
    });
    // A single press on either half selects the whole glyph.
    expect(expandSelection('cell', { row: 0, col: 3 }, { row: 0, col: 3 }, wide, 6)).toEqual({
      start: { row: 0, col: 2 },
      end: { row: 0, col: 3 },
    });
  });

  it('selectionRects given the cells highlights the whole glyph', () => {
    // cols=6, cellWidth=8, cellHeight=16, pad=4
    expect(selectionRects({ row: 0, col: 3 }, { row: 0, col: 4 }, 6, 8, 16, 4, wide)).toEqual([
      { left: 4 + 2 * 8, top: 4, width: 3 * 8, height: 16 },
    ]);
    expect(selectionRects({ row: 0, col: 0 }, { row: 0, col: 2 }, 6, 8, 16, 4, wide)).toEqual([
      { left: 4, top: 4, width: 4 * 8, height: 16 },
    ]);
  });
});
