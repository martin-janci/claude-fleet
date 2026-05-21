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

/**
 * DEC Special Graphics character set translation table (VT100 / xterm).
 *
 * When tmux draws box borders it does:
 *   ESC ( 0    — designate G0 as DEC Special Graphics
 *   q q q q    — these now render as ── (horizontal box-drawing line)
 *   ESC ( B    — designate G0 back to US ASCII
 *
 * Without this mapping the `q`, `x`, `l`, `m`, `j`, `k`, `t`, `u`, `v`, `w`,
 * `n` characters that tmux uses for borders show up as literal letters,
 * which makes claude/tmux UIs look like garbled walls of `xxxx` and `qqqq`.
 *
 * Range: 0x5f..0x7e. Anything outside this range falls through unchanged.
 */
const DEC_SPECIAL_GRAPHICS: { [k: string]: string } = {
  '_': ' ',  // NBSP in spec; plain space renders the same in our DOM
  '`': '◆',
  'a': '▒',
  'b': '\t',
  'c': '\f',
  'd': '\r',
  'e': '\n',
  'f': '°',
  'g': '±',
  'h': '␤',
  'i': '␋',
  'j': '┘',
  'k': '┐',
  'l': '┌',
  'm': '└',
  'n': '┼',
  'o': '⎺',
  'p': '⎻',
  'q': '─',
  'r': '⎼',
  's': '⎽',
  't': '├',
  'u': '┤',
  'v': '┴',
  'w': '┬',
  'x': '│',
  'y': '≤',
  'z': '≥',
  '{': 'π',
  '|': '≠',
  '}': '£',
  '~': '·',
};

function emptyCell(): Cell {
  return { ch: ' ', fg: COLOR_DEFAULT, bg: COLOR_DEFAULT, attrs: 0 };
}

/** Snapshot of the primary buffer kept while the alt screen is active. */
interface SavedScreenState {
  cells: Cell[][];
  cursorRow: number;
  cursorCol: number;
  curFg: number;
  curBg: number;
  curAttrs: number;
  savedRow: number;
  savedCol: number;
  g0Graphics: boolean;
  g1Graphics: boolean;
  useG1: boolean;
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
  /** Whether G0 / G1 are currently designated as DEC Special Graphics.
   *  Default: both ASCII. tmux flips G0 around box drawing. */
  private g0Graphics = false;
  private g1Graphics = false;
  /** Active charset selector: false = G0 (default), true = G1.
   *  SO (0x0E) selects G1, SI (0x0F) selects G0. */
  private useG1 = false;
  /** Saved-screen slot for the DEC alt-screen toggle (?1049 / ?47 / ?1047).
   *  Null while on the primary buffer; populated when the alt buffer is
   *  active so a `?...l` can restore exactly. We only support one level —
   *  nesting another ?1049h while already in alt is a no-op. */
  private savedScreen: SavedScreenState | null = null;
  /** Scroll region (DECSTBM) — inclusive top/bottom row indices. The region
   *  defaults to the whole screen (0 .. rows-1). LF/RI/IL/DL all operate
   *  within these bounds; rows outside the region (e.g. a tmux status bar
   *  pinned to the last row) are never scrolled. */
  scrollTop = 0;
  scrollBottom = 0;

  constructor(rows: number, cols: number) {
    this.rows = Math.max(1, rows);
    this.cols = Math.max(1, cols);
    this.cells = makeGrid(this.rows, this.cols);
    this.scrollTop = 0;
    this.scrollBottom = this.rows - 1;
  }

  /** Resize the buffer. Existing content is preserved by clamping or
   *  padding rows/cols; cursor is clamped to the new dimensions. If we're
   *  on the alt screen, the saved primary buffer is resized too so a
   *  later restore doesn't end up at the old dimensions. */
  resize(rows: number, cols: number): void {
    rows = Math.max(1, rows);
    cols = Math.max(1, cols);
    if (rows === this.rows && cols === this.cols) return;
    this.cells = resizeGrid(this.cells, this.rows, this.cols, rows, cols);
    if (this.savedScreen !== null) {
      const saved = this.savedScreen;
      saved.cells = resizeGrid(saved.cells, this.rows, this.cols, rows, cols);
      if (saved.cursorRow >= rows) saved.cursorRow = rows - 1;
      if (saved.cursorCol >= cols) saved.cursorCol = cols - 1;
    }
    this.rows = rows;
    this.cols = cols;
    if (this.cursorRow >= rows) this.cursorRow = rows - 1;
    if (this.cursorCol >= cols) this.cursorCol = cols - 1;
    // A SIGWINCH invalidates the scroll region — programs re-establish it
    // after the resize (that's exactly why a resize "fixes" a garbled pane).
    // Reset to the full new screen so stale margins can't mis-scroll.
    this.scrollTop = 0;
    this.scrollBottom = this.rows - 1;
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
      if (code === 0x0e /* SO — shift out: select G1 */) {
        this.useG1 = true;
        i++;
        continue;
      }
      if (code === 0x0f /* SI — shift in: select G0 */) {
        this.useG1 = false;
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

  /** Apply current SGR state to a cell and write a character at cursor.
   *  If the active charset (G0/G1) is currently DEC Special Graphics, the
   *  char is run through `DEC_SPECIAL_GRAPHICS` first; otherwise it goes
   *  in literally. */
  private putChar(ch: string): void {
    if (this.cursorCol >= this.cols) {
      this.cursorCol = 0;
      this.lineFeed();
    }
    const graphics = this.useG1 ? this.g1Graphics : this.g0Graphics;
    const mapped = graphics ? (DEC_SPECIAL_GRAPHICS[ch] ?? ch) : ch;
    const cell = this.cells[this.cursorRow][this.cursorCol];
    cell.ch = mapped;
    cell.fg = this.curFg;
    cell.bg = this.curBg;
    cell.attrs = this.curAttrs;
    this.cursorCol++;
  }

  /** LF: move to next row, scrolling the active region up if we're at the
   *  bottom margin. Only the rows in [scrollTop..scrollBottom] move; the
   *  cursor is left on the (now-blank) bottom margin row. When the cursor is
   *  not at the bottom margin we just advance (clamped to the last row), so a
   *  cursor parked below the region — e.g. on a status bar — does not scroll
   *  the body. */
  private lineFeed(): void {
    if (this.cursorRow === this.scrollBottom) {
      // Remove the top line of the region; everything above the new top
      // shifts up by one. After the removal the array is one shorter, so
      // inserting at `scrollBottom` lands the blank on the region's last row.
      this.cells.splice(this.scrollTop, 1);
      this.cells.splice(this.scrollBottom, 0, makeRow(this.cols));
    } else if (this.cursorRow < this.rows - 1) {
      this.cursorRow++;
    }
  }

  /** RI (reverse index): move up one row, scrolling the active region down if
   *  we're at the top margin. Mirror of `lineFeed`. */
  private reverseIndex(): void {
    if (this.cursorRow === this.scrollTop) {
      // Remove the bottom line of the region; everything below the cursor up
      // to scrollBottom shifts down. Inserting at `scrollTop` puts the blank
      // on the region's first row.
      this.cells.splice(this.scrollBottom, 1);
      this.cells.splice(this.scrollTop, 0, makeRow(this.cols));
    } else if (this.cursorRow > 0) {
      this.cursorRow--;
    }
  }

  /** SU (scroll up): scroll the active region up by `n` lines irrespective of
   *  the cursor — blanks fill in at the bottom margin, the cursor stays put.
   *  tmux uses this to scroll a pane without parking the cursor on the bottom
   *  row (the case `lineFeed` never reaches), so dropping it drifts our model
   *  out of sync until a full repaint (e.g. a resize) re-syncs it. */
  private scrollUp(n: number): void {
    n = Math.min(n, this.scrollBottom - this.scrollTop + 1);
    for (let i = 0; i < n; i++) {
      this.cells.splice(this.scrollTop, 1);
      this.cells.splice(this.scrollBottom, 0, makeRow(this.cols));
    }
  }

  /** SD (scroll down): mirror of `scrollUp` — blanks fill in at the top
   *  margin, content at the bottom margin falls off, the cursor stays put. */
  private scrollDown(n: number): void {
    n = Math.min(n, this.scrollBottom - this.scrollTop + 1);
    for (let i = 0; i < n; i++) {
      this.cells.splice(this.scrollBottom, 1);
      this.cells.splice(this.scrollTop, 0, makeRow(this.cols));
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
    // ESC M — RI (reverse index): up one row, scroll region down at top margin.
    if (intro === 'M') {
      this.reverseIndex();
      return 2;
    }
    // ESC D — IND (index): identical to LF (down one row / scroll at bottom).
    if (intro === 'D') {
      this.lineFeed();
      return 2;
    }
    // ESC E — NEL (next line): carriage return + line feed.
    if (intro === 'E') {
      this.cursorCol = 0;
      this.lineFeed();
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
      // SCS — designate a charset into G0 (intro='(') or G1 (intro=')'):
      //   ESC ( 0   → G0 = DEC Special Graphics  (box drawing)
      //   ESC ( B   → G0 = US ASCII              (default)
      //   anything else: treat as ASCII (best effort; we don't model
      //   UK, Dutch, etc.).
      if (start + 2 >= s.length) return -1;
      const designator = s.charAt(start + 2);
      const isGraphics = designator === '0';
      if (intro === '(') this.g0Graphics = isGraphics;
      else this.g1Graphics = isGraphics;
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
    this.g0Graphics = false;
    this.g1Graphics = false;
    this.useG1 = false;
    this.scrollTop = 0;
    this.scrollBottom = this.rows - 1;
    // ESC c is a full power-on reset — drop any alt-screen snapshot so
    // we don't pop back into stale content the next time we leave alt.
    this.savedScreen = null;
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
      case 'S': // SU - scroll up (private `?...S` is XTSMGRAPHICS/sixel: skip)
        if (!isPrivate) this.scrollUp(Math.max(1, p0));
        return;
      case 'T': // SD - scroll down (private/`>`-marked T is title-mode reset)
        if (!isPrivate) this.scrollDown(Math.max(1, p0));
        return;
      case 'r': { // DECSTBM - set top/bottom scroll margins
        // A private `?...r` is XTRESTORE (restore DEC private modes), NOT
        // DECSTBM — ignore it so we don't clobber the scroll region.
        if (isPrivate) return;
        const top = p0 ? p0 - 1 : 0;
        const bottom = p1 ? p1 - 1 : this.rows - 1;
        // Valid only if both margins are in range and top is strictly above
        // bottom; otherwise xterm ignores the request and leaves the region
        // unchanged.
        if (top >= 0 && bottom < this.rows && top < bottom) {
          this.scrollTop = top;
          this.scrollBottom = bottom;
          // DECSTBM homes the cursor (origin mode off → screen home).
          this.cursorRow = 0;
          this.cursorCol = 0;
        }
        return;
      }
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
      case 'h':
      case 'l':
        if (isPrivate) this.applyDecPrivate(params, final === 'h');
        return;
      default:
        // Unknown CSI — silently drop.
        return;
    }
  }

  /** Apply a private-mode set/reset (DECSET / DECRST). We implement the
   *  alt-screen flavors used by tmux/vim/htop. Everything else is dropped
   *  silently so unknown modes don't corrupt state. */
  private applyDecPrivate(params: number[], set: boolean): void {
    for (const p of params) {
      if (p === 1049 || p === 47 || p === 1047) {
        // Buffer swap. 1049 is the modern combined form (save cursor +
        // switch + clear / restore on exit). 47 and 1047 are the legacy
        // variants used by older curses apps; we treat them the same as
        // 1049 here — close enough in practice and avoids subtle bugs
        // where an app's `?47` save gets lost during a `?1049` round trip.
        if (set) this.enterAltScreen();
        else this.leaveAltScreen();
      } else if (p === 1048) {
        // DECSC-only — save / restore the cursor without buffer swap.
        if (set) {
          this.savedRow = this.cursorRow;
          this.savedCol = this.cursorCol;
        } else {
          this.cursorRow = this.savedRow;
          this.cursorCol = this.savedCol;
        }
      }
      // Other private modes (?25 cursor visibility, ?1000 mouse, etc.)
      // are intentionally ignored.
    }
  }

  /** Swap to the alt buffer. Snapshots the primary so a later leave can
   *  restore it. Nested entry (already on alt) is a no-op so tmux's own
   *  pane re-entry sequences don't double-save. */
  private enterAltScreen(): void {
    if (this.savedScreen !== null) return;
    this.savedScreen = {
      cells: this.cells,
      cursorRow: this.cursorRow,
      cursorCol: this.cursorCol,
      curFg: this.curFg,
      curBg: this.curBg,
      curAttrs: this.curAttrs,
      savedRow: this.savedRow,
      savedCol: this.savedCol,
      g0Graphics: this.g0Graphics,
      g1Graphics: this.g1Graphics,
      useG1: this.useG1,
    };
    // Hand the alt buffer a clean slate and reset transient state. Per
    // xterm: the alt screen starts blank with cursor at home.
    this.cells = makeGrid(this.rows, this.cols);
    this.cursorRow = 0;
    this.cursorCol = 0;
    this.curFg = COLOR_DEFAULT;
    this.curBg = COLOR_DEFAULT;
    this.curAttrs = 0;
    this.savedRow = 0;
    this.savedCol = 0;
    this.g0Graphics = false;
    this.g1Graphics = false;
    this.useG1 = false;
    // The alt buffer starts with full-screen margins; the program re-sets a
    // region if it wants one. We don't snapshot the primary's margins —
    // programs re-establish them on leave too (see leaveAltScreen).
    this.scrollTop = 0;
    this.scrollBottom = this.rows - 1;
  }

  /** Swap back to the primary buffer. No-op if we weren't on alt — guards
   *  against apps that emit a stray ?1049l on startup. */
  private leaveAltScreen(): void {
    const saved = this.savedScreen;
    if (saved === null) return;
    this.cells = saved.cells;
    this.cursorRow = saved.cursorRow;
    this.cursorCol = saved.cursorCol;
    this.curFg = saved.curFg;
    this.curBg = saved.curBg;
    this.curAttrs = saved.curAttrs;
    this.savedRow = saved.savedRow;
    this.savedCol = saved.savedCol;
    this.g0Graphics = saved.g0Graphics;
    this.g1Graphics = saved.g1Graphics;
    this.useG1 = saved.useG1;
    this.savedScreen = null;
    // Restoring the primary buffer resets margins to full; the program that
    // owns the primary re-establishes its own region after the buffer pops.
    this.scrollTop = 0;
    this.scrollBottom = this.rows - 1;
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
    // IL is bounded by the scroll region: rows from the cursor down shift
    // down, and lines pushed past `scrollBottom` fall off. A cursor outside
    // the region is a no-op (xterm behaviour) so a status bar below the
    // region is never disturbed.
    if (this.cursorRow < this.scrollTop || this.cursorRow > this.scrollBottom) return;
    for (let i = 0; i < n; i++) {
      // Drop the line currently at the region bottom, then insert a blank at
      // the cursor — everything between shifts down by one within the region.
      this.cells.splice(this.scrollBottom, 1);
      this.cells.splice(this.cursorRow, 0, makeRow(this.cols));
    }
  }

  private deleteLines(n: number): void {
    // DL is bounded by the scroll region: rows below the cursor shift up and
    // blanks fill in at `scrollBottom`. A cursor outside the region is a
    // no-op.
    if (this.cursorRow < this.scrollTop || this.cursorRow > this.scrollBottom) return;
    for (let i = 0; i < n; i++) {
      // Remove the cursor line, then insert a blank at the region bottom —
      // everything between shifts up by one within the region.
      this.cells.splice(this.cursorRow, 1);
      this.cells.splice(this.scrollBottom, 0, makeRow(this.cols));
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

/** Resize a grid by allocating a fresh row-major buffer and copying the
 *  intersecting region. Anything outside the new bounds is dropped;
 *  anything new is filled with empty cells. */
function resizeGrid(
  src: Cell[][],
  oldRows: number,
  oldCols: number,
  newRows: number,
  newCols: number,
): Cell[][] {
  const next = makeGrid(newRows, newCols);
  const rMin = Math.min(newRows, oldRows);
  const cMin = Math.min(newCols, oldCols);
  for (let r = 0; r < rMin; r++) {
    for (let c = 0; c < cMin; c++) {
      next[r][c] = src[r][c];
    }
  }
  return next;
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
