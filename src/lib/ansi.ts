/**
 * Minimal ANSI screen-buffer emulator.
 *
 * This is NOT a full VT100/xterm clone. It implements just enough of the
 * escape-sequence surface area to render tmux + claude's TUI legibly in a
 * plain `<pre>`-style DOM: SGR colors (16/256/24-bit), cursor positioning,
 * clear screen / clear line, CR/LF/BS/TAB, and basic scrolling.
 *
 * We chose this path because xterm.js's renderer silently no-ops after the
 * first write in our Tauri 2.11 + macOS WKWebView setup (see commit log on
 * branch main around 2026-05). The downside is fewer features — no
 * mouse-tracking, no application keypad, no UTF-8 width fixups for wide
 * glyphs, no scrollback beyond the visible window. The upside is that the
 * DOM render path is dirt-simple and provably works.
 */

export interface Cell {
  /** Single grapheme cluster. Empty string represents the trailing half of a
   *  wide glyph; we don't currently produce that — every printable Rune
   *  occupies exactly one cell. */
  ch: string;
  fg: number;
  bg: number;
  attrs: number;
}

/** Attribute bitmask. */
export const ATTR_BOLD = 1 << 0;
export const ATTR_DIM = 1 << 1;
export const ATTR_ITALIC = 1 << 2;
export const ATTR_UNDERLINE = 1 << 3;
export const ATTR_REVERSE = 1 << 4;

/** Sentinel "default" color. Distinct from any 256-color or RGB value. */
export const COLOR_DEFAULT = -1;

/** Encode an RGB triple into a single int (0xRRGGBB | (1<<24)) so we can
 *  share `fg: number` for palette/default/rgb without an extra tag. */
export function rgb(r: number, g: number, b: number): number {
  return (1 << 24) | ((r & 0xff) << 16) | ((g & 0xff) << 8) | (b & 0xff);
}

export function isRgb(c: number): boolean {
  return c >= 0 && (c & (1 << 24)) !== 0;
}

export function rgbToCss(c: number): string {
  return `rgb(${(c >> 16) & 0xff},${(c >> 8) & 0xff},${c & 0xff})`;
}

/**
 * Map 256-color palette indices to CSS. We hardcode the standard xterm
 * 256-color cube; this keeps theming consistent regardless of host colors.
 * Returns a CSS color string or null if the index is out of range.
 */
export function paletteToCss(idx: number): string | null {
  if (idx < 0 || idx > 255) return null;
  if (idx < 16) return BASIC_16[idx];
  if (idx < 232) {
    const n = idx - 16;
    const r = Math.floor(n / 36);
    const g = Math.floor((n % 36) / 6);
    const b = n % 6;
    const conv = (v: number) => (v === 0 ? 0 : 55 + v * 40);
    return `rgb(${conv(r)},${conv(g)},${conv(b)})`;
  }
  const v = 8 + (idx - 232) * 10;
  return `rgb(${v},${v},${v})`;
}

const BASIC_16 = [
  // Standard 16-color palette, tuned to be readable on dark background.
  '#000000', '#cd3131', '#0dbc79', '#e5e510',
  '#2472c8', '#bc3fbc', '#11a8cd', '#e5e5e5',
  '#666666', '#f14c4c', '#23d18b', '#f5f543',
  '#3b8eea', '#d670d6', '#29b8db', '#ffffff',
];

function emptyCell(): Cell {
  return { ch: ' ', fg: COLOR_DEFAULT, bg: COLOR_DEFAULT, attrs: 0 };
}

/**
 * The screen buffer. Owns a 2D grid plus a cursor and pending-attribute
 * state. Resilient to malformed sequences — anything it doesn't recognize
 * is silently discarded so a partial chunk can't poison the state.
 */
export class Screen {
  rows: number;
  cols: number;
  /** Grid is row-major. cells[row][col]. */
  cells: Cell[][];
  cursorRow = 0;
  cursorCol = 0;
  /** Current SGR state — applied to each printed cell. */
  curFg = COLOR_DEFAULT;
  curBg = COLOR_DEFAULT;
  curAttrs = 0;
  /** Partial escape-sequence buffer carried between chunks. Avoids splitting
   *  a CSI across two writes. */
  private pending = '';
  /** Saved cursor (ESC 7 / DECSC). */
  private savedRow = 0;
  private savedCol = 0;

  constructor(rows: number, cols: number) {
    this.rows = Math.max(1, rows);
    this.cols = Math.max(1, cols);
    this.cells = makeGrid(this.rows, this.cols);
  }

  /** Resize the buffer. Existing content is preserved by clamping or
   *  padding rows/cols; cursor is clamped to the new dimensions. */
  resize(rows: number, cols: number): void {
    rows = Math.max(1, rows);
    cols = Math.max(1, cols);
    if (rows === this.rows && cols === this.cols) return;
    const next = makeGrid(rows, cols);
    const rMin = Math.min(rows, this.rows);
    const cMin = Math.min(cols, this.cols);
    for (let r = 0; r < rMin; r++) {
      for (let c = 0; c < cMin; c++) {
        next[r][c] = this.cells[r][c];
      }
    }
    this.cells = next;
    this.rows = rows;
    this.cols = cols;
    if (this.cursorRow >= rows) this.cursorRow = rows - 1;
    if (this.cursorCol >= cols) this.cursorCol = cols - 1;
  }

  /** Feed bytes (already decoded to UTF-16) into the parser. */
  write(input: string): void {
    const s = this.pending + input;
    this.pending = '';
    let i = 0;
    while (i < s.length) {
      const code = s.charCodeAt(i);
      if (code === 0x1b /* ESC */) {
        const consumed = this.parseEscape(s, i);
        if (consumed < 0) {
          // Incomplete — stash everything from here and wait for more.
          this.pending = s.slice(i);
          return;
        }
        i += consumed;
        continue;
      }
      if (code === 0x0a /* LF */) {
        this.lineFeed();
        i++;
        continue;
      }
      if (code === 0x0d /* CR */) {
        this.cursorCol = 0;
        i++;
        continue;
      }
      if (code === 0x08 /* BS */) {
        if (this.cursorCol > 0) this.cursorCol--;
        i++;
        continue;
      }
      if (code === 0x09 /* TAB */) {
        const next = Math.min(this.cols - 1, ((this.cursorCol >> 3) + 1) << 3);
        this.cursorCol = next;
        i++;
        continue;
      }
      if (code === 0x07 /* BEL */) {
        // We don't ring; silently consume.
        i++;
        continue;
      }
      if (code < 0x20) {
        // Unknown control byte — drop.
        i++;
        continue;
      }
      // Printable: write at cursor, advance. Wrap to next row if past edge.
      this.putChar(s.charAt(i));
      i++;
    }
  }

  /** Apply current SGR state to a cell and write a character at cursor. */
  private putChar(ch: string): void {
    if (this.cursorCol >= this.cols) {
      this.cursorCol = 0;
      this.lineFeed();
    }
    const cell = this.cells[this.cursorRow][this.cursorCol];
    cell.ch = ch;
    cell.fg = this.curFg;
    cell.bg = this.curBg;
    cell.attrs = this.curAttrs;
    this.cursorCol++;
  }

  /** LF: move to next row, scrolling up if we'd fall off the bottom. */
  private lineFeed(): void {
    if (this.cursorRow + 1 >= this.rows) {
      this.cells.shift();
      this.cells.push(makeRow(this.cols));
    } else {
      this.cursorRow++;
    }
  }

  /** Parse a single escape sequence starting at `start`. Returns number of
   *  chars consumed including the leading ESC, or -1 if incomplete. */
  private parseEscape(s: string, start: number): number {
    if (start + 1 >= s.length) return -1;
    const intro = s.charAt(start + 1);
    // ESC c — full reset (rare; tmux uses it sometimes during resize).
    if (intro === 'c') {
      this.fullReset();
      return 2;
    }
    if (intro === '7') {
      this.savedRow = this.cursorRow;
      this.savedCol = this.cursorCol;
      return 2;
    }
    if (intro === '8') {
      this.cursorRow = this.savedRow;
      this.cursorCol = this.savedCol;
      return 2;
    }
    if (intro === '[') {
      // CSI: ESC [ <params> <final-byte 0x40..0x7e>
      let end = start + 2;
      while (end < s.length) {
        const ch = s.charCodeAt(end);
        if (ch >= 0x40 && ch <= 0x7e) break;
        end++;
      }
      if (end >= s.length) return -1; // incomplete CSI
      const body = s.slice(start + 2, end);
      const final = s.charAt(end);
      this.applyCsi(body, final);
      return end - start + 1;
    }
    if (intro === ']') {
      // OSC: ESC ] ... BEL  or  ESC ] ... ESC \
      let end = start + 2;
      while (end < s.length) {
        const ch = s.charCodeAt(end);
        if (ch === 0x07) return end - start + 1;
        if (ch === 0x1b && end + 1 < s.length && s.charCodeAt(end + 1) === 0x5c) {
          return end - start + 2;
        }
        end++;
      }
      return -1; // incomplete OSC
    }
    if (intro === '(' || intro === ')') {
      // SCS (charset designation) — ignore designator byte.
      if (start + 2 >= s.length) return -1;
      return 3;
    }
    if (intro === '=' || intro === '>') {
      // Application/numeric keypad — ignore.
      return 2;
    }
    // Unknown / unsupported single-char ESC — drop.
    return 2;
  }

  private fullReset(): void {
    this.cells = makeGrid(this.rows, this.cols);
    this.cursorRow = 0;
    this.cursorCol = 0;
    this.curFg = COLOR_DEFAULT;
    this.curBg = COLOR_DEFAULT;
    this.curAttrs = 0;
  }

  private applyCsi(body: string, final: string): void {
    // Strip leading '?' / '>' / '=' / '!' private-mode markers. We don't
    // implement any private modes, so dropping the marker lets us at least
    // not corrupt the buffer when tmux sends DECSET/DECRST.
    let isPrivate = false;
    if (body.length > 0 && (body[0] === '?' || body[0] === '>' || body[0] === '!')) {
      isPrivate = true;
      body = body.slice(1);
    }
    const params = body.length === 0 ? [] : body.split(';').map((x) => (x === '' ? 0 : parseInt(x, 10) || 0));
    const p0 = params[0] ?? 0;
    const p1 = params[1] ?? 0;

    switch (final) {
      case 'A': // CUU - cursor up
        this.cursorRow = Math.max(0, this.cursorRow - Math.max(1, p0));
        return;
      case 'B': // CUD - cursor down
        this.cursorRow = Math.min(this.rows - 1, this.cursorRow + Math.max(1, p0));
        return;
      case 'C': // CUF - cursor forward
        this.cursorCol = Math.min(this.cols - 1, this.cursorCol + Math.max(1, p0));
        return;
      case 'D': // CUB - cursor back
        this.cursorCol = Math.max(0, this.cursorCol - Math.max(1, p0));
        return;
      case 'E': // CNL - cursor next line
        this.cursorRow = Math.min(this.rows - 1, this.cursorRow + Math.max(1, p0));
        this.cursorCol = 0;
        return;
      case 'F': // CPL - cursor prev line
        this.cursorRow = Math.max(0, this.cursorRow - Math.max(1, p0));
        this.cursorCol = 0;
        return;
      case 'G': // CHA - cursor horizontal absolute
        this.cursorCol = clamp(Math.max(1, p0) - 1, 0, this.cols - 1);
        return;
      case 'H': // CUP
      case 'f':
        this.cursorRow = clamp(Math.max(1, p0) - 1, 0, this.rows - 1);
        this.cursorCol = clamp(Math.max(1, p1) - 1, 0, this.cols - 1);
        return;
      case 'J': // ED - erase in display
        this.eraseInDisplay(p0);
        return;
      case 'K': // EL - erase in line
        this.eraseInLine(p0);
        return;
      case 'L': // IL - insert lines
        if (!isPrivate) this.insertLines(Math.max(1, p0));
        return;
      case 'M': // DL - delete lines
        if (!isPrivate) this.deleteLines(Math.max(1, p0));
        return;
      case 'P': // DCH - delete chars
        if (!isPrivate) this.deleteChars(Math.max(1, p0));
        return;
      case '@': // ICH - insert chars
        if (!isPrivate) this.insertChars(Math.max(1, p0));
        return;
      case 'X': // ECH - erase chars
        if (!isPrivate) this.eraseChars(Math.max(1, p0));
        return;
      case 'd': // VPA - vertical position absolute
        this.cursorRow = clamp(Math.max(1, p0) - 1, 0, this.rows - 1);
        return;
      case 's': // save cursor (ANSI.SYS variant)
        this.savedRow = this.cursorRow;
        this.savedCol = this.cursorCol;
        return;
      case 'u': // restore cursor
        this.cursorRow = this.savedRow;
        this.cursorCol = this.savedCol;
        return;
      case 'm': // SGR - select graphic rendition
        this.applySgr(params);
        return;
      default:
        // Unknown CSI — silently drop.
        return;
    }
  }

  private eraseInDisplay(mode: number): void {
    if (mode === 0) {
      // From cursor to end of screen.
      for (let c = this.cursorCol; c < this.cols; c++) this.clearCell(this.cursorRow, c);
      for (let r = this.cursorRow + 1; r < this.rows; r++)
        for (let c = 0; c < this.cols; c++) this.clearCell(r, c);
    } else if (mode === 1) {
      // From start of screen to cursor.
      for (let r = 0; r < this.cursorRow; r++)
        for (let c = 0; c < this.cols; c++) this.clearCell(r, c);
      for (let c = 0; c <= this.cursorCol; c++) this.clearCell(this.cursorRow, c);
    } else if (mode === 2 || mode === 3) {
      // Whole screen (3 also clears scrollback in real terms; we have none).
      for (let r = 0; r < this.rows; r++)
        for (let c = 0; c < this.cols; c++) this.clearCell(r, c);
    }
  }

  private eraseInLine(mode: number): void {
    if (mode === 0) {
      for (let c = this.cursorCol; c < this.cols; c++) this.clearCell(this.cursorRow, c);
    } else if (mode === 1) {
      for (let c = 0; c <= this.cursorCol; c++) this.clearCell(this.cursorRow, c);
    } else if (mode === 2) {
      for (let c = 0; c < this.cols; c++) this.clearCell(this.cursorRow, c);
    }
  }

  private insertLines(n: number): void {
    for (let i = 0; i < n; i++) {
      if (this.cursorRow >= this.rows) break;
      this.cells.splice(this.cursorRow, 0, makeRow(this.cols));
      if (this.cells.length > this.rows) this.cells.length = this.rows;
    }
  }

  private deleteLines(n: number): void {
    for (let i = 0; i < n; i++) {
      if (this.cursorRow >= this.rows) break;
      this.cells.splice(this.cursorRow, 1);
      this.cells.push(makeRow(this.cols));
    }
  }

  private deleteChars(n: number): void {
    const row = this.cells[this.cursorRow];
    row.splice(this.cursorCol, n);
    while (row.length < this.cols) row.push(emptyCell());
    if (row.length > this.cols) row.length = this.cols;
  }

  private insertChars(n: number): void {
    const row = this.cells[this.cursorRow];
    for (let i = 0; i < n; i++) row.splice(this.cursorCol, 0, emptyCell());
    if (row.length > this.cols) row.length = this.cols;
  }

  private eraseChars(n: number): void {
    for (let i = 0; i < n; i++) {
      const c = this.cursorCol + i;
      if (c >= this.cols) break;
      this.clearCell(this.cursorRow, c);
    }
  }

  private clearCell(r: number, c: number): void {
    const cell = this.cells[r][c];
    cell.ch = ' ';
    cell.fg = COLOR_DEFAULT;
    cell.bg = COLOR_DEFAULT;
    cell.attrs = 0;
  }

  private applySgr(params: number[]): void {
    if (params.length === 0) {
      this.resetSgr();
      return;
    }
    for (let i = 0; i < params.length; i++) {
      const p = params[i];
      if (p === 0) {
        this.resetSgr();
      } else if (p === 1) {
        this.curAttrs |= ATTR_BOLD;
      } else if (p === 2) {
        this.curAttrs |= ATTR_DIM;
      } else if (p === 3) {
        this.curAttrs |= ATTR_ITALIC;
      } else if (p === 4) {
        this.curAttrs |= ATTR_UNDERLINE;
      } else if (p === 7) {
        this.curAttrs |= ATTR_REVERSE;
      } else if (p === 22) {
        this.curAttrs &= ~(ATTR_BOLD | ATTR_DIM);
      } else if (p === 23) {
        this.curAttrs &= ~ATTR_ITALIC;
      } else if (p === 24) {
        this.curAttrs &= ~ATTR_UNDERLINE;
      } else if (p === 27) {
        this.curAttrs &= ~ATTR_REVERSE;
      } else if (p >= 30 && p <= 37) {
        this.curFg = p - 30;
      } else if (p === 38) {
        // 256-color: 38;5;N. 24-bit: 38;2;R;G;B. Consume sub-params.
        if (params[i + 1] === 5 && i + 2 < params.length) {
          this.curFg = params[i + 2];
          i += 2;
        } else if (params[i + 1] === 2 && i + 4 < params.length) {
          this.curFg = rgb(params[i + 2], params[i + 3], params[i + 4]);
          i += 4;
        }
      } else if (p === 39) {
        this.curFg = COLOR_DEFAULT;
      } else if (p >= 40 && p <= 47) {
        this.curBg = p - 40;
      } else if (p === 48) {
        if (params[i + 1] === 5 && i + 2 < params.length) {
          this.curBg = params[i + 2];
          i += 2;
        } else if (params[i + 1] === 2 && i + 4 < params.length) {
          this.curBg = rgb(params[i + 2], params[i + 3], params[i + 4]);
          i += 4;
        }
      } else if (p === 49) {
        this.curBg = COLOR_DEFAULT;
      } else if (p >= 90 && p <= 97) {
        this.curFg = (p - 90) + 8;
      } else if (p >= 100 && p <= 107) {
        this.curBg = (p - 100) + 8;
      }
      // Anything else: silently ignore.
    }
  }

  private resetSgr(): void {
    this.curFg = COLOR_DEFAULT;
    this.curBg = COLOR_DEFAULT;
    this.curAttrs = 0;
  }
}

function makeRow(cols: number): Cell[] {
  const row: Cell[] = new Array(cols);
  for (let i = 0; i < cols; i++) row[i] = emptyCell();
  return row;
}

function makeGrid(rows: number, cols: number): Cell[][] {
  const g: Cell[][] = new Array(rows);
  for (let r = 0; r < rows; r++) g[r] = makeRow(cols);
  return g;
}

function clamp(n: number, lo: number, hi: number): number {
  return n < lo ? lo : n > hi ? hi : n;
}

/** A horizontal run of same-styled cells on a single row. */
export interface Run {
  text: string;
  fg: number;
  bg: number;
  attrs: number;
}

/** Group a row's cells into adjacent runs sharing fg/bg/attrs. Trailing
 *  default-styled blanks are kept so the column grid stays aligned in the
 *  rendered output (we depend on monospace + non-breaking spaces). */
export function rowToRuns(row: Cell[]): Run[] {
  const runs: Run[] = [];
  let cur: Run | null = null;
  for (const cell of row) {
    if (
      cur !== null &&
      cur.fg === cell.fg &&
      cur.bg === cell.bg &&
      cur.attrs === cell.attrs
    ) {
      cur.text += cell.ch;
    } else {
      cur = { text: cell.ch, fg: cell.fg, bg: cell.bg, attrs: cell.attrs };
      runs.push(cur);
    }
  }
  return runs;
}

/** Convert a palette/RGB/default color number into a CSS color string, or
 *  null if it should fall back to the theme default. */
export function colorToCss(c: number): string | null {
  if (c === COLOR_DEFAULT) return null;
  if (isRgb(c)) return rgbToCss(c);
  return paletteToCss(c);
}
