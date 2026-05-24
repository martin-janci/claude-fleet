// Pure selection geometry for the terminal grid. 0-based cell coordinates.
// Kept DOM-free so it can be unit-tested directly.

export interface CellPos {
  row: number;
  col: number;
}

export interface OverlayRect {
  left: number;
  top: number;
  width: number;
  height: number;
}

/** Order two endpoints into reading order (row first, then col). */
export function normalizeSelection(a: CellPos, b: CellPos): { start: CellPos; end: CellPos } {
  const before = a.row < b.row || (a.row === b.row && a.col <= b.col);
  return before ? { start: a, end: b } : { start: b, end: a };
}

/** Build one overlay rect per selected row segment. Endpoints are inclusive.
 *  First row runs from its col to end-of-line, middle rows span the full width,
 *  the last row runs from col 0 to its col. `pad` is the grid's edge padding. */
export function selectionRects(
  a: CellPos,
  b: CellPos,
  cols: number,
  cellWidth: number,
  cellHeight: number,
  pad: number,
): OverlayRect[] {
  const { start, end } = normalizeSelection(a, b);
  const rects: OverlayRect[] = [];
  for (let r = start.row; r <= end.row; r++) {
    const from = r === start.row ? start.col : 0;
    const toInclusive = r === end.row ? end.col : cols - 1;
    rects.push({
      left: pad + from * cellWidth,
      top: pad + r * cellHeight,
      width: (toInclusive - from + 1) * cellWidth,
      height: cellHeight,
    });
  }
  return rects;
}
