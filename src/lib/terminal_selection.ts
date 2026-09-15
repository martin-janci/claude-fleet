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

/** Selection granularity, chosen by click count: single click / drag selects
 *  cells, double-click words, triple-click whole lines — the text-input
 *  convention macOS terminals follow. */
export type SelectMode = 'cell' | 'word' | 'line';

/** Minimal view of a screen cell the selection helpers need. */
export interface CellLike {
  ch: string;
}

/** Order two endpoints into reading order (row first, then col). */
export function normalizeSelection(a: CellPos, b: CellPos): { start: CellPos; end: CellPos } {
  const before = a.row < b.row || (a.row === b.row && a.col <= b.col);
  return before ? { start: a, end: b } : { start: b, end: a };
}

/** Widen an ordered selection so it never cuts a wide glyph: a start on a
 *  trailing `''` cell moves back to its head, an end on a head moves forward
 *  onto its trailing cell. The copied text (`Screen.selectionText`) always
 *  takes a glyph whole, so this keeps the highlight covering exactly those
 *  cells. Columns outside a row are left as they are. */
export function snapToGlyphs(
  start: CellPos,
  end: CellPos,
  cells: readonly (readonly CellLike[])[],
): { start: CellPos; end: CellPos } {
  let from = start.col;
  if (from > 0 && cells[start.row]?.[from]?.ch === '') from--;
  let to = end.col;
  const endRow = cells[end.row];
  if (endRow && endRow[to] && endRow[to].ch !== '' && endRow[to + 1]?.ch === '') to++;
  return { start: { row: start.row, col: from }, end: { row: end.row, col: to } };
}

/** Build one overlay rect per selected row segment. Endpoints are inclusive.
 *  First row runs from its col to end-of-line, middle rows span the full width,
 *  the last row runs from col 0 to its col. `pad` is the grid's edge padding.
 *  With `cells`, the ends are first snapped to whole glyphs (`snapToGlyphs`). */
export function selectionRects(
  a: CellPos,
  b: CellPos,
  cols: number,
  cellWidth: number,
  cellHeight: number,
  pad: number,
  cells?: readonly (readonly CellLike[])[],
): OverlayRect[] {
  const ordered = normalizeSelection(a, b);
  const { start, end } = cells ? snapToGlyphs(ordered.start, ordered.end, cells) : ordered;
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

/** Click count → selection granularity (`MouseEvent.detail`). */
export function modeForClickCount(detail: number): SelectMode {
  if (detail >= 3) return 'line';
  if (detail === 2) return 'word';
  return 'cell';
}

/** Characters that end a word on double-click. Path/URL punctuation
 *  (`/ - _ . : ~ + = @ #`) stays inside a word so a double-click on a path or
 *  a flag grabs the whole thing; quotes, brackets and separators split.
 *  Box-drawing glyphs (the borders of Claude's prompt box) split too, so the
 *  word next to a `│` never drags the border along. */
const WORD_BREAKERS = new Set([
  '"', "'", '`', '(', ')', '[', ']', '{', '}', '<', '>', '|', ';', ',', '!', '?', '$', '&', '*', '\\',
]);

/** Is this cell part of a word for double-click purposes? A wide glyph's
 *  trailing `''` placeholder belongs to the glyph before it, so it counts as
 *  a word char and the pair is never split. */
export function isWordChar(ch: string): boolean {
  if (ch === '') return true;
  const cp = ch.codePointAt(0)!;
  if (cp <= 0x20 || cp === 0xa0) return false;
  if (cp >= 0x2500 && cp <= 0x257f) return false; // box drawing
  return !WORD_BREAKERS.has(ch);
}

/** Inclusive column span of the "word" under `col` in a row of cells:
 *  the run of word chars around it; for a blank, the run of blanks (as
 *  Terminal.app does); for a lone breaker, just that cell. `col` is clamped
 *  into the row. */
export function wordBoundsAt(row: readonly CellLike[], col: number): { from: number; to: number } {
  const last = row.length - 1;
  if (last < 0) return { from: 0, to: 0 };
  col = Math.max(0, Math.min(last, col));
  const ch = row[col].ch;
  const isBlank = (c: string) => c === ' ' || c === '\t' || c === ' ';
  let same: (c: string) => boolean;
  if (isWordChar(ch)) same = isWordChar;
  else if (isBlank(ch)) same = isBlank;
  else return { from: col, to: col };
  let from = col;
  let to = col;
  while (from > 0 && same(row[from - 1].ch)) from--;
  while (to < last && same(row[to + 1].ch)) to++;
  return { from, to };
}

/** Expand a raw anchor/focus pair to the selection endpoints for a mode:
 *  `cell` keeps them (ordered, snapped to whole wide glyphs), `word` snaps the
 *  start back to its word's first cell and the end forward to its word's last
 *  cell, `line` covers the full rows. Either end may lie before the other; the
 *  result is always in reading order. */
export function expandSelection(
  mode: SelectMode,
  anchor: CellPos,
  focus: CellPos,
  cells: readonly (readonly CellLike[])[],
  cols: number,
): { start: CellPos; end: CellPos } {
  const { start, end } = normalizeSelection(anchor, focus);
  if (mode === 'cell') return snapToGlyphs(start, end, cells);
  if (mode === 'line') {
    return { start: { row: start.row, col: 0 }, end: { row: end.row, col: cols - 1 } };
  }
  const startRow = cells[start.row] ?? [];
  const endRow = cells[end.row] ?? [];
  return {
    start: { row: start.row, col: wordBoundsAt(startRow, start.col).from },
    end: { row: end.row, col: wordBoundsAt(endRow, end.col).to },
  };
}
