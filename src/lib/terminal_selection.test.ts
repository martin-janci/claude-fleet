import { describe, it, expect } from 'vitest';
import { normalizeSelection, selectionRects, type CellPos } from './terminal_selection';

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
});
