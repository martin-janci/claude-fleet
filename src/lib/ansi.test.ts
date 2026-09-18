import { describe, it, expect } from 'vitest';
import {
  Screen,
  COLOR_DEFAULT,
  ATTR_BOLD,
  ATTR_ITALIC,
  ATTR_UNDERLINE,
  ATTR_HIDDEN,
  ATTR_STRIKE,
  ATTR_REVERSE,
  runStyleCss,
  rowToRuns,
  runsKey,
  type Run,
  rgb,
  isRgb,
  colorToCss,
  encodeMouse,
} from './ansi';

function rowText(s: Screen, r: number): string {
  return s.cells[r].map((c) => c.ch).join('');
}

describe('ansi.Screen — printable text and cursor motion', () => {
  it('writes plain ASCII into the buffer at the cursor and advances', () => {
    const s = new Screen(3, 10);
    s.write('hello');
    expect(rowText(s, 0).slice(0, 5)).toBe('hello');
    expect(s.cursorRow).toBe(0);
    expect(s.cursorCol).toBe(5);
  });

  it('CR resets column to 0 without changing row', () => {
    const s = new Screen(3, 10);
    s.write('ab\rcd');
    // 'a' at col 0, 'b' at col 1, CR back to col 0, 'c' overwrites 'a',
    // 'd' overwrites 'b'. Row 0 should now read 'cd'.
    expect(rowText(s, 0).slice(0, 2)).toBe('cd');
    expect(s.cursorRow).toBe(0);
    expect(s.cursorCol).toBe(2);
  });

  it('LF moves to next row and keeps column', () => {
    const s = new Screen(3, 10);
    s.write('ab\nc');
    expect(rowText(s, 0).slice(0, 2)).toBe('ab');
    expect(s.cursorRow).toBe(1);
    expect(rowText(s, 1).slice(0, 3)).toBe('  c');
  });

  it('CRLF acts like newline + carriage return', () => {
    const s = new Screen(3, 10);
    s.write('ab\r\ncd');
    expect(rowText(s, 0).slice(0, 2)).toBe('ab');
    expect(rowText(s, 1).slice(0, 2)).toBe('cd');
  });

  it('BS moves cursor back without erasing', () => {
    const s = new Screen(2, 5);
    s.write('abc\b');
    expect(s.cursorCol).toBe(2);
    expect(rowText(s, 0).slice(0, 3)).toBe('abc');
  });

  it('LF at the bottom row scrolls the buffer up', () => {
    const s = new Screen(2, 6);
    // Use \r\n so the cursor returns to col 0 between rows. Pure LF only
    // moves vertically — tmux already emits \r\n.
    s.write('row1\r\nrow2\r\nrow3');
    expect(rowText(s, 0).slice(0, 4)).toBe('row2');
    expect(rowText(s, 1).slice(0, 4)).toBe('row3');
  });

  it('writes past the right edge wrap to the next row', () => {
    const s = new Screen(2, 4);
    s.write('abcdef');
    expect(rowText(s, 0)).toBe('abcd');
    expect(rowText(s, 1).slice(0, 2)).toBe('ef');
  });
});

describe('ansi.Screen — CSI cursor positioning', () => {
  it('ESC[H homes the cursor', () => {
    const s = new Screen(3, 5);
    s.write('xx\nyy\x1b[Hz');
    expect(rowText(s, 0).slice(0, 2)).toBe('zx');
  });

  it('ESC[2;3H positions cursor (1-based row/col)', () => {
    const s = new Screen(3, 6);
    s.write('\x1b[2;3HA');
    expect(rowText(s, 1).slice(0, 4)).toBe('  A ');
  });

  it('ESC[NA / NB / NC / ND move cursor by N', () => {
    const s = new Screen(5, 5);
    s.write('\x1b[3B\x1b[2CX');
    expect(s.cursorRow).toBe(3);
    expect(rowText(s, 3).slice(0, 3)).toBe('  X');
  });

  it('ESC[Nd sets cursor row absolutely', () => {
    const s = new Screen(5, 5);
    s.write('\x1b[3dQ');
    expect(rowText(s, 2).slice(0, 1)).toBe('Q');
  });

  it('cursor positioning clamps to screen bounds', () => {
    const s = new Screen(3, 4);
    s.write('\x1b[99;99HX');
    expect(rowText(s, 2).slice(3)).toBe('X');
  });
});

describe('ansi.Screen — erase operations', () => {
  it('ESC[K erases from cursor to end of line', () => {
    const s = new Screen(2, 6);
    s.write('abcdef\x1b[3G\x1b[K');
    expect(rowText(s, 0)).toBe('ab    ');
  });

  it('ESC[2K erases the whole line', () => {
    const s = new Screen(2, 6);
    s.write('abcdef\x1b[2K');
    expect(rowText(s, 0)).toBe('      ');
  });

  it('ESC[2J erases the whole screen', () => {
    const s = new Screen(2, 4);
    s.write('hello\nworld\x1b[2J');
    expect(rowText(s, 0)).toBe('    ');
    expect(rowText(s, 1)).toBe('    ');
  });

  it('ESC[J (no param) clears from cursor to end of screen', () => {
    const s = new Screen(3, 4);
    s.write('aaaa\r\nbbbb\r\ncccc\x1b[2;2H\x1b[J');
    expect(rowText(s, 0)).toBe('aaaa');
    expect(rowText(s, 1).slice(0, 1)).toBe('b');
    expect(rowText(s, 1).slice(1)).toBe('   ');
    expect(rowText(s, 2)).toBe('    ');
  });
});

describe('ansi.Screen — SGR attributes', () => {
  it('SGR 31 sets foreground to red palette index 1', () => {
    const s = new Screen(1, 3);
    s.write('\x1b[31mX');
    expect(s.cells[0][0].fg).toBe(1);
    expect(s.cells[0][0].ch).toBe('X');
  });

  it('SGR 0 resets all attributes and colors to default', () => {
    const s = new Screen(1, 4);
    s.write('\x1b[1;31mA\x1b[0mB');
    expect(s.cells[0][0].fg).toBe(1);
    expect(s.cells[0][0].attrs & ATTR_BOLD).not.toBe(0);
    expect(s.cells[0][1].fg).toBe(COLOR_DEFAULT);
    expect(s.cells[0][1].attrs).toBe(0);
  });

  it('SGR 38;5;N picks a 256-color palette foreground', () => {
    const s = new Screen(1, 3);
    s.write('\x1b[38;5;202mX');
    expect(s.cells[0][0].fg).toBe(202);
  });

  it('SGR 38;2;R;G;B picks a 24-bit foreground', () => {
    const s = new Screen(1, 3);
    s.write('\x1b[38;2;10;20;30mY');
    const fg = s.cells[0][0].fg;
    expect(isRgb(fg)).toBe(true);
    expect(colorToCss(fg)).toBe('rgb(10,20,30)');
  });

  it('SGR 4/24 toggle underline', () => {
    const s = new Screen(1, 4);
    s.write('\x1b[4mU\x1b[24mu');
    expect(s.cells[0][0].attrs & ATTR_UNDERLINE).not.toBe(0);
    expect(s.cells[0][1].attrs & ATTR_UNDERLINE).toBe(0);
  });

  it('SGR 90 picks a bright (palette index 8) foreground', () => {
    const s = new Screen(1, 2);
    s.write('\x1b[90mD');
    expect(s.cells[0][0].fg).toBe(8);
  });
});

describe('ansi.Screen — robustness', () => {
  it('handles a CSI that is split across two write() calls', () => {
    const s = new Screen(2, 5);
    s.write('A\x1b');
    s.write('[31mB');
    expect(s.cells[0][1].fg).toBe(1);
    expect(s.cells[0][1].ch).toBe('B');
  });

  it('handles a CSI split mid-parameters', () => {
    const s = new Screen(2, 5);
    s.write('\x1b[31');
    s.write(';1mZ');
    expect(s.cells[0][0].fg).toBe(1);
    expect(s.cells[0][0].attrs & ATTR_BOLD).not.toBe(0);
  });

  it('silently drops unknown CSI sequences', () => {
    const s = new Screen(1, 4);
    s.write('A\x1b[?25hB');
    expect(s.cells[0][0].ch).toBe('A');
    expect(s.cells[0][1].ch).toBe('B');
  });

  it('silently drops OSC sequences (titles etc.) with BEL terminator', () => {
    const s = new Screen(1, 4);
    s.write('A\x1b]0;hello\x07B');
    expect(s.cells[0][0].ch).toBe('A');
    expect(s.cells[0][1].ch).toBe('B');
  });

  it('silently drops OSC sequences with ESC \\ terminator', () => {
    const s = new Screen(1, 4);
    s.write('A\x1b]0;hi\x1b\\B');
    expect(s.cells[0][0].ch).toBe('A');
    expect(s.cells[0][1].ch).toBe('B');
  });

  it('ESC c performs a full reset (clears buffer + attrs)', () => {
    const s = new Screen(2, 3);
    s.write('\x1b[31mHI\x1bcX');
    expect(s.cells[0][0].ch).toBe('X');
    expect(s.cells[0][0].fg).toBe(COLOR_DEFAULT);
  });

  it('resize preserves existing content within new bounds', () => {
    const s = new Screen(3, 5);
    s.write('abc\r\ndef');
    s.resize(2, 3);
    expect(rowText(s, 0)).toBe('abc');
    expect(rowText(s, 1)).toBe('def');
  });
});

describe('ansi.Screen — DEC Special Graphics charset', () => {
  it('ESC ( 0 switches G0 to graphics; printable bytes translate to box-drawing', () => {
    // tmux draws a horizontal line as ESC ( 0  qqqq  ESC ( B
    const s = new Screen(1, 6);
    s.write('\x1b(0qqqq\x1b(B');
    expect(rowText(s, 0).slice(0, 4)).toBe('────');
  });

  it('ESC ( 0 maps x → │ and j/k/l/m → corners', () => {
    const s = new Screen(2, 8);
    s.write('\x1b(0lqqk\r\nx  x\x1b(B');
    expect(rowText(s, 0).slice(0, 4)).toBe('┌──┐');
    expect(rowText(s, 1).slice(0, 4)).toBe('│  │');
  });

  it('ESC ( B switches G0 back to ASCII so letters render literally', () => {
    const s = new Screen(1, 6);
    s.write('\x1b(0qq\x1b(Bqq');
    // first two q's translate to ─, next two are literal q
    expect(rowText(s, 0).slice(0, 4)).toBe('──qq');
  });

  it('SO (0x0E) shifts active charset to G1, SI (0x0F) shifts back to G0', () => {
    const s = new Screen(1, 6);
    // G0=ASCII (default), G1=graphics. SO selects G1, SI selects G0.
    s.write('\x1b)0\x0eqq\x0fqq');
    expect(rowText(s, 0).slice(0, 4)).toBe('──qq');
  });

  it('ESC ( 0 leaves chars outside the graphics range alone (digits, A-Z)', () => {
    const s = new Screen(1, 6);
    s.write('\x1b(0AB12\x1b(B');
    expect(rowText(s, 0).slice(0, 4)).toBe('AB12');
  });
});

describe('ansi.Screen — alt screen buffer (DECSET 1049)', () => {
  it('ESC[?1049h switches to a cleared alt buffer; primary content is hidden', () => {
    const s = new Screen(3, 5);
    s.write('hello\r\nworld');
    s.write('\x1b[?1049h');
    // After the switch the buffer is empty — visible cells are all spaces.
    expect(rowText(s, 0)).toBe('     ');
    expect(rowText(s, 1)).toBe('     ');
  });

  it('ESC[?1049l returns to the saved primary buffer with content + cursor', () => {
    const s = new Screen(3, 5);
    s.write('hello\r\nworld');
    // Cursor is at row 1, col 5 (after "world").
    s.write('\x1b[?1049h');
    s.write('\x1b[2;1HALT'); // type into alt buffer at row 2, col 1
    s.write('\x1b[?1049l');
    // Back on primary: content intact, cursor restored.
    expect(rowText(s, 0)).toBe('hello');
    expect(rowText(s, 1)).toBe('world');
    expect(s.cursorRow).toBe(1);
    expect(s.cursorCol).toBe(5);
  });

  it('SGR state set in alt buffer does not leak back to primary on leave', () => {
    const s = new Screen(2, 4);
    s.write('A');
    s.write('\x1b[?1049h\x1b[31mB');
    s.write('\x1b[?1049l');
    // The primary's "A" was written with default fg before the switch.
    expect(s.cells[0][0].fg).toBe(COLOR_DEFAULT);
    // Subsequent writes use the SGR state from BEFORE entering alt.
    s.write('C');
    expect(s.cells[0][1].fg).toBe(COLOR_DEFAULT);
  });

  it('ESC[?1049l without a matching enter is a safe no-op', () => {
    const s = new Screen(2, 4);
    s.write('hi\x1b[?1049lX');
    // We just kept printing; current row should now read "hiX ".
    expect(rowText(s, 0).slice(0, 3)).toBe('hiX');
  });

  it('ESC[?47h / ?47l also swap buffers (legacy)', () => {
    const s = new Screen(2, 4);
    s.write('top\x1b[?47h\x1b[2J\x1b[Hnew\x1b[?47l');
    expect(rowText(s, 0).slice(0, 3)).toBe('top');
  });
});

describe('ansi.Screen — DECSTBM scroll region', () => {
  it('LF scrolls only the region; a pinned status bar on the last row is preserved', () => {
    // The tmux scenario: status bar pinned to the last row, body scrolls
    // within a region above it. This is THE bug — without DECSTBM the LF
    // would scroll the whole screen and carry the status bar away.
    const s = new Screen(5, 6);
    // Status bar on the last row (index 4).
    s.write('\x1b[5;1HSTATUS');
    // Region = rows 1..4 (1-based) → indices 0..3.
    s.write('\x1b[1;4r');
    // After DECSTBM the cursor homes to (0,0). Fill the region: put LINE4 on
    // the bottom region row, then CRLF (scrolls the region) and write LINE5.
    // tmux always emits \r\n, so the CR resets the column for the next line.
    s.write('\x1b[4;1HLINE4');
    s.write('\r\n');
    s.write('LINE5');
    // Body scrolled within the region: LINE4 moved up to row 2, LINE5 landed
    // on the freshly-blanked bottom region row (row 3).
    expect(rowText(s, 2).slice(0, 5)).toBe('LINE4');
    expect(rowText(s, 3).slice(0, 5)).toBe('LINE5');
    // The status bar on the last row is untouched — the whole point of DECSTBM.
    expect(rowText(s, 4)).toBe('STATUS');
  });

  it('LF at the bottom margin scrolls the region up, leaving rows below untouched', () => {
    const s = new Screen(4, 4);
    s.write('\x1b[1;3r'); // region rows 0..2
    s.write('\x1b[1;1HA');
    s.write('\x1b[2;1HB');
    s.write('\x1b[3;1HC');
    s.write('\x1b[4;1HZ'); // row 3, outside the region
    s.write('\x1b[3;1H'); // cursor at row 2 (bottom margin)
    s.write('\n'); // LF at bottom margin → region scrolls up
    expect(rowText(s, 0).slice(0, 1)).toBe('B');
    expect(rowText(s, 1).slice(0, 1)).toBe('C');
    expect(rowText(s, 2)).toBe('    '); // fresh blank at region bottom
    expect(rowText(s, 3).slice(0, 1)).toBe('Z'); // outside region — untouched
  });

  it('RI (ESC M) at the top margin scrolls the region down', () => {
    const s = new Screen(4, 4);
    s.write('\x1b[1;3r'); // region rows 0..2
    s.write('\x1b[1;1HA');
    s.write('\x1b[2;1HB');
    s.write('\x1b[3;1HC');
    s.write('\x1b[4;1HZ'); // row 3, outside the region
    s.write('\x1b[1;1H'); // cursor at row 0 (top margin)
    s.write('\x1bM'); // RI at top margin → region scrolls down
    expect(rowText(s, 0)).toBe('    '); // fresh blank at region top
    expect(rowText(s, 1).slice(0, 1)).toBe('A');
    expect(rowText(s, 2).slice(0, 1)).toBe('B'); // C was pushed off
    expect(rowText(s, 3).slice(0, 1)).toBe('Z'); // outside region — untouched
  });

  it('SU (CSI S) scrolls the region up regardless of cursor position', () => {
    // tmux emits SU to scroll a pane up without parking the cursor on the
    // bottom margin — the case plain LF never reaches. Dropping SU is THE
    // residual drift bug: our model fell out of sync with the real screen.
    const s = new Screen(4, 4);
    s.write('\x1b[1;3r'); // region rows 0..2
    s.write('\x1b[1;1HA');
    s.write('\x1b[2;1HB');
    s.write('\x1b[3;1HC');
    s.write('\x1b[4;1HZ'); // row 3, outside the region
    s.write('\x1b[1;1H'); // cursor at the TOP margin — not the bottom
    s.write('\x1b[2S'); // scroll region up by 2
    expect(rowText(s, 0).slice(0, 1)).toBe('C'); // A,B fell off the top
    expect(rowText(s, 1)).toBe('    '); // fresh blank
    expect(rowText(s, 2)).toBe('    '); // fresh blank
    expect(rowText(s, 3).slice(0, 1)).toBe('Z'); // outside region — untouched
  });

  it('SU (CSI S) defaults to one line and leaves the cursor put', () => {
    const s = new Screen(3, 4);
    s.write('\x1b[1;1HA');
    s.write('\x1b[2;1HB');
    s.write('\x1b[3;1HC');
    s.write('\x1b[2;3H'); // park cursor at row 1, col 2
    s.write('\x1b[S'); // SU default = 1, whole-screen region
    expect(rowText(s, 0).slice(0, 1)).toBe('B');
    expect(rowText(s, 1).slice(0, 1)).toBe('C');
    expect(rowText(s, 2)).toBe('    ');
    // Cursor unmoved by SU: the next char lands at row 1, col 2.
    s.write('X');
    expect(rowText(s, 1).slice(2, 3)).toBe('X');
  });

  it('SD (CSI T) scrolls the region down, filling blanks at the top', () => {
    const s = new Screen(4, 4);
    s.write('\x1b[1;3r'); // region rows 0..2
    s.write('\x1b[1;1HA');
    s.write('\x1b[2;1HB');
    s.write('\x1b[3;1HC');
    s.write('\x1b[4;1HZ'); // row 3, outside the region
    s.write('\x1b[1;1H'); // cursor at the top margin
    s.write('\x1b[2T'); // scroll region down by 2
    expect(rowText(s, 0)).toBe('    '); // fresh blank at top
    expect(rowText(s, 1)).toBe('    '); // fresh blank
    expect(rowText(s, 2).slice(0, 1)).toBe('A'); // A pushed down; B,C fell off
    expect(rowText(s, 3).slice(0, 1)).toBe('Z'); // outside region — untouched
  });

  it('with no DECSTBM the default region is the whole screen (regression guard)', () => {
    const s = new Screen(3, 4);
    s.write('aaaa\r\nbbbb\r\ncccc');
    s.write('\n'); // LF at last row scrolls the whole screen
    expect(rowText(s, 0)).toBe('bbbb');
    expect(rowText(s, 1)).toBe('cccc');
    expect(rowText(s, 2)).toBe('    ');
  });

  it('resize resets the scroll region to the full new screen', () => {
    const s = new Screen(5, 4);
    s.write('\x1b[1;3r'); // region rows 0..2
    s.resize(3, 4); // margins reset to full
    s.write('aaaa\r\nbbbb\r\ncccc');
    s.write('\n'); // LF at last row scrolls the whole (resized) screen
    expect(rowText(s, 0)).toBe('bbbb');
    expect(rowText(s, 1)).toBe('cccc');
    expect(rowText(s, 2)).toBe('    ');
  });

  it('full reset (ESC c) resets the scroll region to the whole screen', () => {
    const s = new Screen(3, 4);
    s.write('\x1b[1;2r'); // region rows 0..1
    s.write('\x1bc'); // full reset
    s.write('aaaa\r\nbbbb\r\ncccc');
    s.write('\n');
    expect(rowText(s, 0)).toBe('bbbb');
    expect(rowText(s, 2)).toBe('    ');
  });

  it('an out-of-range / inverted DECSTBM is ignored (region stays full screen)', () => {
    const s = new Screen(3, 4);
    s.write('\x1b[3;1r'); // top >= bottom → invalid, ignored
    s.write('aaaa\r\nbbbb\r\ncccc');
    s.write('\n');
    expect(rowText(s, 0)).toBe('bbbb');
    expect(rowText(s, 2)).toBe('    ');
  });

  it('IND (ESC D) behaves like LF, scrolling the region at the bottom margin', () => {
    const s = new Screen(4, 4);
    s.write('\x1b[1;3r'); // region rows 0..2
    s.write('\x1b[1;1HA');
    s.write('\x1b[2;1HB');
    s.write('\x1b[3;1HC');
    s.write('\x1b[4;1HZ');
    s.write('\x1b[3;1H'); // cursor at bottom margin
    s.write('\x1bD'); // IND
    expect(rowText(s, 0).slice(0, 1)).toBe('B');
    expect(rowText(s, 1).slice(0, 1)).toBe('C');
    expect(rowText(s, 2)).toBe('    ');
    expect(rowText(s, 3).slice(0, 1)).toBe('Z');
  });

  it('NEL (ESC E) does CR + LF', () => {
    const s = new Screen(3, 6);
    s.write('\x1b[1;3HAB'); // cursor lands at row 0, col 2 then writes "AB"
    s.write('\x1bE'); // NEL → col 0, next row
    s.write('C');
    expect(s.cursorRow).toBe(1);
    expect(rowText(s, 1).slice(0, 1)).toBe('C'); // CR returned to col 0
  });

  it('LF on a row below the scroll region does not scroll the region', () => {
    // Cursor on the status bar (below the region) doing LF must NOT move the
    // body. With scrollBottom < last row, LF at the last row is a no-op.
    const s = new Screen(4, 4);
    s.write('\x1b[1;3r'); // region rows 0..2
    s.write('\x1b[1;1HA');
    s.write('\x1b[2;1HB');
    s.write('\x1b[3;1HC');
    s.write('\x1b[4;1HZ'); // status bar on last row (index 3)
    s.write('\n'); // LF while cursor is at row 3 (below region) — no scroll
    expect(rowText(s, 0).slice(0, 1)).toBe('A');
    expect(rowText(s, 1).slice(0, 1)).toBe('B');
    expect(rowText(s, 2).slice(0, 1)).toBe('C');
    expect(rowText(s, 3).slice(0, 1)).toBe('Z');
  });

  it('IL/DL are bounded by the scroll region', () => {
    // Insert/delete lines must discard/fill at scrollBottom, not the last row,
    // so a pinned status bar below the region is never disturbed.
    const s = new Screen(5, 4);
    s.write('\x1b[1;4r'); // region rows 0..3
    s.write('\x1b[1;1HA');
    s.write('\x1b[2;1HB');
    s.write('\x1b[3;1HC');
    s.write('\x1b[4;1HD');
    s.write('\x1b[5;1HS'); // status bar on last row (index 4)
    // Delete the top line of the region: B,C,D shift up; region bottom blanks.
    s.write('\x1b[1;1H'); // cursor at region top
    s.write('\x1b[M'); // DL 1
    expect(rowText(s, 0).slice(0, 1)).toBe('B');
    expect(rowText(s, 1).slice(0, 1)).toBe('C');
    expect(rowText(s, 2).slice(0, 1)).toBe('D');
    expect(rowText(s, 3)).toBe('    '); // blank filled at region bottom
    expect(rowText(s, 4).slice(0, 1)).toBe('S'); // status bar untouched
  });

  it('alt-screen swap resets the scroll region to full on enter and leave', () => {
    const s = new Screen(4, 4);
    s.write('\x1b[1;2r'); // region rows 0..1 on primary
    s.write('\x1b[?1049h'); // enter alt — margins should reset to full
    s.write('aaaa\r\nbbbb\r\ncccc\r\ndddd');
    s.write('\n'); // LF at last row scrolls whole alt screen
    expect(rowText(s, 0)).toBe('bbbb');
    expect(rowText(s, 3)).toBe('    ');
    s.write('\x1b[?1049l'); // leave alt — margins reset to full again
    s.write('\x1b[1;1Hpppp\r\nqqqq\r\nrrrr\r\nssss');
    s.write('\n'); // LF at last row scrolls the whole primary screen
    expect(rowText(s, 0)).toBe('qqqq');
    expect(rowText(s, 3)).toBe('    ');
  });
});

describe('ansi.rowToRuns', () => {
  it('keeps Greek and Cyrillic in one run: the grid font covers them', () => {
    // Pinning every cell of ordinary non-Latin prose would mean one DOM node
    // per character; Menlo and the fallback stack draw these at one cell.
    const cyr = new Screen(1, 8);
    cyr.write('привет');
    const runs = rowToRuns(cyr.cells[0]);
    expect(runs).toHaveLength(1);
    expect(runs[0].text).toBe('привет  ');
    expect(runs[0].glyph).toBeUndefined();

    const greek = new Screen(1, 6);
    greek.write('αβγ');
    expect(rowToRuns(greek.cells[0])).toHaveLength(1);
  });

  it('groups adjacent cells with identical style into one run', () => {
    const s = new Screen(1, 6);
    s.write('\x1b[31mAB\x1b[0mCD');
    const runs = rowToRuns(s.cells[0]);
    // AB (fg=1) + CD + 2 trailing blanks (both at default style) collapse
    // into 2 runs total.
    expect(runs.length).toBe(2);
    expect(runs[0].text).toBe('AB');
    expect(runs[0].fg).toBe(1);
    expect(runs[1].text).toBe('CD  ');
    expect(runs[1].fg).toBe(COLOR_DEFAULT);
  });

  // The renderer pins every run to `cells` × the measured cell width, so the
  // counts must add up to the row width and a glyph that may come from a
  // fallback font must sit alone in a box of exactly its own cells.
  const shape = (runs: ReturnType<typeof rowToRuns>) =>
    runs.map((r) => [r.text, r.cells, r.glyph ?? false, r.wide ?? false]);

  it('counts the cells every run covers, summing to the row width', () => {
    const s = new Screen(1, 8);
    s.write('\x1b[31mAB\x1b[0m中x');
    const runs = rowToRuns(s.cells[0]);
    expect(shape(runs)).toEqual([
      ['AB', 2, false, false],
      ['中', 2, false, true],
      ['x   ', 4, false, false],
    ]);
    expect(runs.reduce((n, r) => n + r.cells, 0)).toBe(8);
  });

  it('gives each glyph outside the grid font its own 1-cell run (Claude Code bullets)', () => {
    const s = new Screen(1, 10);
    s.write('\u23fa x\u23bf\u273b\u2764y');
    expect(shape(rowToRuns(s.cells[0]))).toEqual([
      ['\u23fa', 1, true, false],
      [' x', 2, false, false],
      ['\u23bf', 1, true, false],
      ['\u273b', 1, true, false],
      ['\u2764', 1, true, false],
      ['y   ', 4, false, false],
    ]);
  });

  it('keeps ASCII, Latin-1, Latin Extended-A/B, box drawing and block elements in one run', () => {
    const s = new Screen(1, 9);
    s.write('a\u00e9\u00a0\u017e\u0192\u2500\u257f\u2588\u259f');
    expect(shape(rowToRuns(s.cells[0]))).toEqual([
      ['a\u00e9\u00a0\u017e\u0192\u2500\u257f\u2588\u259f', 9, false, false],
    ]);
  });

  it('pins a cell carrying a combining mark, and a soft hyphen that draws no advance', () => {
    const s = new Screen(1, 5);
    s.write('a\u0301b\u00adc');
    expect(shape(rowToRuns(s.cells[0]))).toEqual([
      ['a\u0301', 1, true, false],
      ['b', 1, false, false],
      ['\u00ad', 1, true, false],
      ['c ', 2, false, false],
    ]);
  });

  it('pins DEC Special Graphics that map outside the grid font', () => {
    const s = new Screen(1, 4);
    s.write('\x1b(0qo\x1b(B');
    // q → ─ (box drawing, grid font), o → ⎺ scan line 1 (U+23BA, fallback).
    expect(shape(rowToRuns(s.cells[0]))).toEqual([
      ['\u2500', 1, false, false],
      ['\u23ba', 1, true, false],
      ['  ', 2, false, false],
    ]);
  });

  it('draws VS16 and ZWJ clusters as 2-cell wide runs', () => {
    const s = new Screen(1, 6);
    s.write('\u2764\ufe0f\u{1f468}\u200d\u{1f469}z');
    expect(shape(rowToRuns(s.cells[0]))).toEqual([
      ['\u2764\ufe0f', 2, false, true],
      ['\u{1f468}\u200d\u{1f469}', 2, false, true],
      ['z ', 2, false, false],
    ]);
  });

  it('a narrow VS16 cluster at the right edge is a pinned 1-cell glyph', () => {
    const s = new Screen(1, 3);
    s.write('ab\u2764\ufe0f');
    expect(shape(rowToRuns(s.cells[0]))).toEqual([
      ['ab', 2, false, false],
      ['\u2764\ufe0f', 1, true, false],
    ]);
  });
});

describe('ansi.runsKey', () => {
  const run = (text: string, cells: number, extra: Partial<Run> = {}): Run => ({
    text, fg: COLOR_DEFAULT, bg: COLOR_DEFAULT, attrs: 0, cells, ...extra,
  });

  it('is stable for identical runs and distinct per row index', () => {
    const s = new Screen(1, 6);
    s.write('\u23fa ok');
    expect(runsKey(0, rowToRuns(s.cells[0]))).toBe(runsKey(0, rowToRuns(s.cells[0])));
    expect(runsKey(0, rowToRuns(s.cells[0]))).not.toBe(runsKey(1, rowToRuns(s.cells[0])));
  });

  it('changes when only a run\'s cell count changes', () => {
    // A VS16 cluster is a 2-cell pair mid-row but one cell at the right edge.
    expect(runsKey(0, [run('\u2764\ufe0f', 2, { wide: true })])).not.toBe(
      runsKey(0, [run('\u2764\ufe0f', 1, { glyph: true })]),
    );
    expect(runsKey(0, [run('ab', 2)])).not.toBe(runsKey(0, [run('ab', 3)]));
  });

  it('changes when run boundaries move even though the joined text is the same', () => {
    expect(runsKey(0, [run('\u23fa', 1, { glyph: true }), run(' x', 2)])).not.toBe(
      runsKey(0, [run('\u23fa x', 3)]),
    );
  });

  it('changes with style', () => {
    expect(runsKey(0, [run('a', 1)])).not.toBe(runsKey(0, [run('a', 1, { fg: 1 })]));
    expect(runsKey(0, [run('a', 1)])).not.toBe(runsKey(0, [run('a', 1, { bg: 1 })]));
    expect(runsKey(0, [run('a', 1)])).not.toBe(runsKey(0, [run('a', 1, { attrs: 1 })]));
  });
});

describe('ansi.colorToCss', () => {
  it('returns null for the default color sentinel', () => {
    expect(colorToCss(COLOR_DEFAULT)).toBeNull();
  });

  it('returns a hex / rgb string for palette index 0..15', () => {
    expect(colorToCss(0)).toMatch(/^#/);
    expect(colorToCss(15)).toMatch(/^#/);
  });

  it('returns an rgb() string for 256-color cube indices', () => {
    expect(colorToCss(16)).toMatch(/^rgb\(/);
  });

  it('returns an rgb() string for 24-bit colors', () => {
    expect(colorToCss(rgb(10, 20, 30))).toBe('rgb(10,20,30)');
  });
});

describe('ansi.Screen.rowVersion (dirty-row tracking)', () => {
  it('starts every row at a fresh version', () => {
    const s = new Screen(5, 10);
    expect(s.rowVersion).toHaveLength(5);
    expect(s.rowVersion.every((v) => v > 0)).toBe(true);
  });

  it('bumps only the cursor row on a printed char', () => {
    const s = new Screen(5, 10);
    const before = [...s.rowVersion];
    s.write('\x1b[3;1H'); // cursor → row 2 (0-based)
    s.write('x');
    expect(s.rowVersion[2]).not.toBe(before[2]);
    expect(s.rowVersion[0]).toBe(before[0]);
    expect(s.rowVersion[4]).toBe(before[4]);
  });

  it('bumps the scroll region on a bottom-margin line feed', () => {
    const s = new Screen(4, 10);
    s.write('\x1b[4;1H'); // cursor on the last row
    const before = [...s.rowVersion];
    s.write('\n'); // scrolls the whole region up
    expect(s.rowVersion.every((v, r) => v !== before[r])).toBe(true);
  });

  it('bumps a row when it is erased', () => {
    const s = new Screen(5, 10);
    s.write('\x1b[2;1Habc'); // write on row 1
    const before = [...s.rowVersion];
    s.write('\x1b[2;1H\x1b[K'); // erase row 1
    expect(s.rowVersion[1]).not.toBe(before[1]);
    expect(s.rowVersion[3]).toBe(before[3]);
  });

  it('rebuilds rowVersion to the new length on resize', () => {
    const s = new Screen(5, 10);
    s.resize(8, 10);
    expect(s.rowVersion).toHaveLength(8);
    expect(s.rowVersion.every((v) => v > 0)).toBe(true);
  });
});

describe('encodeMouse — SGR encoding (?1006h)', () => {
  it('SGR left press at col=5, row=10', () => {
    expect(encodeMouse(0, 5, 10, false, true)).toBe('\x1b[<0;5;10M');
  });

  it('SGR left release at col=5, row=10', () => {
    expect(encodeMouse(0, 5, 10, true, true)).toBe('\x1b[<0;5;10m');
  });

  it('SGR wheel-up at col=1, row=1', () => {
    expect(encodeMouse(64, 1, 1, false, true)).toBe('\x1b[<64;1;1M');
  });

  it('SGR wheel-down at col=1, row=1', () => {
    expect(encodeMouse(65, 1, 1, false, true)).toBe('\x1b[<65;1;1M');
  });

  it('SGR drag (left held + motion, cb=32) at col=3, row=4', () => {
    expect(encodeMouse(32, 3, 4, false, true)).toBe('\x1b[<32;3;4M');
  });
});

describe('encodeMouse — X10/legacy encoding', () => {
  it('X10 left press at col=1, row=1: bytes [32, 33, 33]', () => {
    const result = encodeMouse(0, 1, 1, false, false);
    expect(result).toBe('\x1b[M' + String.fromCharCode(32, 33, 33));
  });

  it('X10 release (low bits → 3): byte1 = 3+32 = 35', () => {
    const result = encodeMouse(0, 1, 1, true, false);
    // cb=0 release → low bits become 3 → cbX10=3 → byte1=3+32=35
    expect(result).toBe('\x1b[M' + String.fromCharCode(35, 33, 33));
  });

  it('X10 right press (cb=2) at col=1, row=1: byte1 = 2+32 = 34', () => {
    const result = encodeMouse(2, 1, 1, false, false);
    expect(result).toBe('\x1b[M' + String.fromCharCode(34, 33, 33));
  });
});

describe('ansi.Screen — mouse mode tracking', () => {
  it('fresh Screen has all mouse modes false', () => {
    const s = new Screen(24, 80);
    expect(s.mouseEnabled).toBe(false);
    expect(s.mouseButtonMotion).toBe(false);
    expect(s.mouseAnyMotion).toBe(false);
    expect(s.mouseSgr).toBe(false);
  });

  it('?1000h sets mouseEnabled; ?1000l clears it', () => {
    const s = new Screen(24, 80);
    s.write('\x1b[?1000h');
    expect(s.mouseEnabled).toBe(true);
    s.write('\x1b[?1000l');
    expect(s.mouseEnabled).toBe(false);
  });

  it('?1006h sets mouseSgr; ?1006l clears it', () => {
    const s = new Screen(24, 80);
    s.write('\x1b[?1006h');
    expect(s.mouseSgr).toBe(true);
    s.write('\x1b[?1006l');
    expect(s.mouseSgr).toBe(false);
  });

  it('?1000h + ?1006h: both mouseEnabled and mouseSgr true', () => {
    const s = new Screen(24, 80);
    s.write('\x1b[?1000h\x1b[?1006h');
    expect(s.mouseEnabled).toBe(true);
    expect(s.mouseSgr).toBe(true);
  });

  it('?1002h sets mouseEnabled and mouseButtonMotion but not mouseAnyMotion', () => {
    const s = new Screen(24, 80);
    s.write('\x1b[?1002h');
    expect(s.mouseEnabled).toBe(true);
    expect(s.mouseButtonMotion).toBe(true);
    expect(s.mouseAnyMotion).toBe(false);
  });

  it('?1003h sets mouseEnabled, mouseButtonMotion, and mouseAnyMotion', () => {
    const s = new Screen(24, 80);
    s.write('\x1b[?1003h');
    expect(s.mouseEnabled).toBe(true);
    expect(s.mouseButtonMotion).toBe(true);
    expect(s.mouseAnyMotion).toBe(true);
  });

  it('?1003l clears all three motion flags', () => {
    const s = new Screen(24, 80);
    s.write('\x1b[?1003h');
    s.write('\x1b[?1003l');
    expect(s.mouseEnabled).toBe(false);
    expect(s.mouseButtonMotion).toBe(false);
    expect(s.mouseAnyMotion).toBe(false);
  });

  it('cursor is visible by default and toggles with ?25 (DECTCEM)', () => {
    const s = new Screen(24, 80);
    expect(s.cursorVisible).toBe(true);
    s.write('\x1b[?25l');
    expect(s.cursorVisible).toBe(false);
    s.write('\x1b[?25h');
    expect(s.cursorVisible).toBe(true);
  });
});

describe('bracketed paste mode (DECSET ?2004)', () => {
  it('defaults to off', () => {
    expect(new Screen(24, 80).bracketedPaste).toBe(false);
  });
  it('?2004h enables and ?2004l disables', () => {
    const s = new Screen(24, 80);
    s.write('\x1b[?2004h');
    expect(s.bracketedPaste).toBe(true);
    s.write('\x1b[?2004l');
    expect(s.bracketedPaste).toBe(false);
  });
  it('an unrelated private mode leaves it unchanged', () => {
    const s = new Screen(24, 80);
    s.write('\x1b[?2004h');
    s.write('\x1b[?25l'); // hide cursor — must not touch bracketedPaste
    expect(s.bracketedPaste).toBe(true);
  });
});

describe('Screen.selectionText', () => {
  function withText(lines: string[]): Screen {
    const cols = Math.max(...lines.map((l) => l.length), 1);
    const s = new Screen(lines.length, cols);
    lines.forEach((line, r) => {
      for (let c = 0; c < line.length; c++) s.cells[r][c].ch = line[c];
    });
    return s;
  }

  it('returns a single cell', () => {
    const s = withText(['abc']);
    expect(s.selectionText({ row: 0, col: 1 }, { row: 0, col: 1 })).toBe('b');
  });

  it('returns a single-row range inclusive of both ends', () => {
    const s = withText(['hello world']);
    expect(s.selectionText({ row: 0, col: 0 }, { row: 0, col: 4 })).toBe('hello');
  });

  it('spans multiple rows: first row from col, middle rows full, last row to col', () => {
    const s = withText(['abcde', 'fghij', 'klmno']);
    // from (0,2) to (2,1): "cde" + "fghij" + "kl"
    expect(s.selectionText({ row: 0, col: 2 }, { row: 2, col: 1 })).toBe('cde\nfghij\nkl');
  });

  it('normalizes a reversed anchor/focus', () => {
    const s = withText(['abcde']);
    expect(s.selectionText({ row: 0, col: 4 }, { row: 0, col: 0 })).toBe('abcde');
  });

  it('trims trailing whitespace per line (padded cells)', () => {
    const s = withText(['hi', 'yo']);
    expect(s.selectionText({ row: 0, col: 0 }, { row: 1, col: 4 })).toBe('hi\nyo');
  });
});

describe('ansi.Screen — code points and wide glyphs (FE-5)', () => {
  const cells = (s: Screen, r: number) => s.cells[r].map((c) => c.ch);

  it('an astral emoji occupies two cells as one grapheme (head + trailing placeholder)', () => {
    const s = new Screen(2, 6);
    s.write('a😀b');
    expect(cells(s, 0)).toEqual(['a', '😀', '', 'b', ' ', ' ']);
    expect(s.cursorCol).toBe(4);
  });

  it('CJK shifts later columns by two per glyph', () => {
    const s = new Screen(2, 8);
    s.write('中文x');
    expect(cells(s, 0)).toEqual(['中', '', '文', '', 'x', ' ', ' ', ' ']);
    expect(s.cursorCol).toBe(5);
    // CHA to column 5 lands on the 'x' — the wide glyphs really took 4 cells.
    s.write('\x1b[5Gy');
    expect(cells(s, 0)[4]).toBe('y');
  });

  it('a wide glyph that would straddle the right edge wraps, leaving the orphan column blank', () => {
    const s = new Screen(2, 5);
    s.write('abcd中');
    expect(cells(s, 0)).toEqual(['a', 'b', 'c', 'd', ' ']);
    expect(cells(s, 1)).toEqual(['中', '', ' ', ' ', ' ']);
    expect(s.cursorCol).toBe(2);
  });

  it('a surrogate pair split across two writes is still one glyph', () => {
    const s = new Screen(1, 4);
    const [hi, lo] = ['😀'.charAt(0), '😀'.charAt(1)];
    s.write('a' + hi);
    expect(cells(s, 0)).toEqual(['a', ' ', ' ', ' ']);
    s.write(lo + 'b');
    expect(cells(s, 0)).toEqual(['a', '😀', '', 'b']);
  });

  it('an unpaired surrogate renders as U+FFFD in a single cell', () => {
    const s = new Screen(1, 4);
    s.write('a\ud83db');
    expect(cells(s, 0)).toEqual(['a', '�', 'b', ' ']);
  });

  it('combining marks attach to the previous cell', () => {
    const s = new Screen(1, 4);
    s.write('éx');
    expect(cells(s, 0)).toEqual(['é', 'x', ' ', ' ']);
    expect(s.cursorCol).toBe(2);
  });

  it('a variation selector after a wide glyph attaches to its head, not the trailing cell', () => {
    const s = new Screen(1, 4);
    s.write('😀️x');
    expect(cells(s, 0)).toEqual(['😀️', '', 'x', ' ']);
  });

  it('a combining mark with nothing before it on the row is dropped', () => {
    const s = new Screen(1, 3);
    s.write('́a');
    expect(cells(s, 0)).toEqual(['a', ' ', ' ']);
  });

  it('overwriting the trailing half of a wide glyph blanks its head', () => {
    const s = new Screen(1, 4);
    s.write('中x\x1b[2GZ');
    expect(cells(s, 0)).toEqual([' ', 'Z', 'x', ' ']);
  });

  it('overwriting the head of a wide glyph with a narrow char blanks its trailing half', () => {
    const s = new Screen(1, 4);
    s.write('中x\x1b[1GZ');
    expect(cells(s, 0)).toEqual(['Z', ' ', 'x', ' ']);
  });

  it('erasing (ECH) either half of a pair clears both', () => {
    const a = new Screen(1, 4);
    a.write('中x\x1b[2G\x1b[X'); // erase the trailing cell
    expect(cells(a, 0)).toEqual([' ', ' ', 'x', ' ']);
    const b = new Screen(1, 4);
    b.write('中x\x1b[1G\x1b[X'); // erase the head
    expect(cells(b, 0)).toEqual([' ', ' ', 'x', ' ']);
  });

  it('EL from the middle of a pair clears the whole pair', () => {
    const s = new Screen(1, 4);
    s.write('a中b\x1b[3G\x1b[K');
    expect(cells(s, 0)).toEqual(['a', ' ', ' ', ' ']);
  });

  it('DCH on the trailing half deletes the pair; an orphaned trailing half is blanked', () => {
    const a = new Screen(1, 5);
    a.write('中xy\x1b[2G\x1b[P');
    expect(cells(a, 0)).toEqual([' ', 'x', 'y', ' ', ' ']);
    const b = new Screen(1, 5);
    b.write('a中b\x1b[1G\x1b[2P'); // deletes 'a' and the head; the trailing slides into col 0
    expect(cells(b, 0)).toEqual([' ', 'b', ' ', ' ', ' ']);
  });

  it('ICH that pushes a head into the last column blanks it (trailing fell off)', () => {
    const s = new Screen(1, 3);
    s.write('a中\x1b[1G\x1b[@');
    expect(cells(s, 0)).toEqual([' ', 'a', ' ']);
    // With room for the pair it simply shifts intact.
    const t = new Screen(1, 4);
    t.write('a中\x1b[1G\x1b[@');
    expect(cells(t, 0)).toEqual([' ', 'a', '中', '']);
  });

  it('ICH in front of a trailing half breaks the pair', () => {
    const s = new Screen(1, 5);
    s.write('中x\x1b[2G\x1b[@');
    expect(cells(s, 0)).toEqual([' ', ' ', ' ', 'x', ' ']);
  });

  it('narrowing the screen through a pair blanks the cut head', () => {
    const s = new Screen(1, 4);
    s.write('a中');
    s.resize(1, 2);
    expect(cells(s, 0)).toEqual(['a', ' ']);
  });

  it('REP repeats a wide glyph as full pairs', () => {
    const s = new Screen(1, 8);
    s.write('中\x1b[2b');
    expect(cells(s, 0)).toEqual(['中', '', '中', '', '中', '', ' ', ' ']);
  });

  it('selectionText skips trailing placeholders so the copied text has no extra spaces', () => {
    const s = new Screen(1, 6);
    s.write('a😀b');
    expect(s.selectionText({ row: 0, col: 0 }, { row: 0, col: 5 })).toBe('a😀b');
  });

  it('rowToRuns emits a wide glyph as its own 2-cell run and drops the placeholder', () => {
    const s = new Screen(1, 5);
    s.write('a中x');
    const runs = rowToRuns(s.cells[0]);
    expect(runs.map((r) => [r.text, r.wide ?? false])).toEqual([
      ['a', false],
      ['中', true],
      ['x ', false],
    ]);
  });

  it('rowToRuns keeps a styled wide glyph out of its neighbours\' run even when the style matches', () => {
    const s = new Screen(1, 6);
    s.write('\x1b[31m😀😀\x1b[0m');
    const runs = rowToRuns(s.cells[0]);
    expect(runs.map((r) => r.text)).toEqual(['😀', '😀', '  ']);
    expect(runs[0].wide).toBe(true);
    expect(runs[1].wide).toBe(true);
    expect(runs[0].fg).toBe(1);
  });

  it('rowToRuns renders an orphan placeholder as a blank so later columns stay put', () => {
    const s = new Screen(1, 4);
    s.write('abcd');
    s.cells[0][0].ch = ''; // corrupt on purpose: nothing can be its head
    expect(rowToRuns(s.cells[0]).map((r) => r.text)).toEqual([' bcd']);
  });

  it('DEL and C1 controls are dropped, not printed', () => {
    const s = new Screen(1, 4);
    s.write('a\x7fb\x85c');
    expect(cells(s, 0)).toEqual(['a', 'b', 'c', ' ']);
  });
});

describe('ansi.Screen — control strings are swallowed (FE-5)', () => {
  it('a DCS body (ESC P … ESC \\) is invisible', () => {
    const s = new Screen(1, 10);
    s.write('a\x1bPq#0;2;0;0;0#0~~\x1b\\b');
    expect(rowText(s, 0)).toBe('ab        ');
  });

  it('DCS terminated by C1 ST (0x9c) is invisible', () => {
    const s = new Screen(1, 6);
    s.write('a\x1bP1$r0m\x9cb');
    expect(rowText(s, 0)).toBe('ab    ');
  });

  it('APC / PM / SOS bodies are invisible', () => {
    const s = new Screen(1, 8);
    s.write('a\x1b_Gi=1\x1b\\b\x1b^pm\x1b\\c\x1bXsos\x1b\\d');
    expect(rowText(s, 0)).toBe('abcd    ');
  });

  it('a DCS split across writes stays swallowed', () => {
    const s = new Screen(1, 6);
    s.write('a\x1bPhid');
    s.write('den\x1b');
    s.write('\\b');
    expect(rowText(s, 0)).toBe('ab    ');
  });

  it('an unhandled OSC (hyperlink 8) prints nothing', () => {
    const s = new Screen(1, 6);
    s.write('\x1b]8;;http://x\x1b\\ok\x1b]8;;\x1b\\');
    expect(rowText(s, 0)).toBe('ok    ');
  });

  it('ESC + intermediate + final (DECALN) consumes its final byte', () => {
    const s = new Screen(1, 4);
    s.write('a\x1b#8b');
    expect(rowText(s, 0)).toBe('ab  ');
  });

  it('a 1 MB unterminated DCS keeps the parser buffer bounded and later text still renders', () => {
    const s = new Screen(2, 8);
    s.write('a\x1bP' + 'x'.repeat(1 << 20));
    expect(s.bufferedLength).toBe(0);
    for (let i = 0; i < 20; i++) {
      s.write('y'.repeat(100 * 1024));
      expect(s.bufferedLength).toBeLessThanOrEqual(1);
    }
    s.write('\x1b\\b');
    expect(rowText(s, 0)).toBe('ab      ');
  });

  it('an unterminated OSC keeps at most OSC_MAX (64 KiB) of body', () => {
    const s = new Screen(1, 4);
    s.write('\x1b]52;c;' + 'A'.repeat(1 << 20));
    expect(s.bufferedLength).toBeLessThanOrEqual(64 * 1024);
    s.write('B'.repeat(1 << 20));
    expect(s.bufferedLength).toBeLessThanOrEqual(64 * 1024);
    s.write('\x07x');
    expect(s.bufferedLength).toBe(0);
    expect(rowText(s, 0)).toBe('x   ');
  });

  it('a stray DCS opener is ended by the next escape sequence (any ESC ends a string)', () => {
    const s = new Screen(3, 4);
    s.write('\x1bPgarbage');
    s.write('with no ST');
    s.write('\x1b[2;2Hx');
    expect(s.cells[1][1].ch).toBe('x');
  });

  it('CAN / SUB abort a control string', () => {
    const s = new Screen(1, 6);
    s.write('\x1bPabc\x18d\x1b]0;t\x1ae');
    expect(rowText(s, 0)).toBe('de    ');
  });

  it('a CSI with no final byte is abandoned after CSI_MAX chars instead of buffering forever', () => {
    const s = new Screen(1, 4);
    s.write('\x1b[' + '1'.repeat(4000));
    expect(s.bufferedLength).toBeLessThan(1100);
    // The abandoned sequence's leftovers render as text; the parser is back
    // in the ground state, so what follows prints normally.
    s.write('\rzz');
    expect(rowText(s, 0).startsWith('zz')).toBe(true);
  });

  it('a CSI split across chunks is still recognised below the cap', () => {
    const s = new Screen(3, 3);
    s.write('\x1b[');
    s.write('2;');
    expect(s.bufferedLength).toBe(4);
    s.write('2Hx');
    expect(s.bufferedLength).toBe(0);
    expect(s.cells[1][1].ch).toBe('x');
  });

  it('ESC ESC [ A restarts the sequence at the second ESC', () => {
    const s = new Screen(5, 5);
    s.write('\x1b[5;5H\x1b\x1b[A');
    expect(s.cursorRow).toBe(3);
    expect(rowText(s, 3)).toBe('     ');
  });

  it('CAN inside a CSI aborts it; the following text prints', () => {
    const s = new Screen(3, 4);
    s.write('\x1b[3\x18x');
    expect(s.cells[0][0].ch).toBe('x');
    expect(s.cursorRow).toBe(0);
  });

  it('ESC inside a CSI abandons it and starts a new sequence', () => {
    const s = new Screen(3, 3);
    s.write('\x1b[9\x1b[2;2Hx');
    expect(s.cells[1][1].ch).toBe('x');
  });

  it('CSI ? s / CSI ? u (XTSAVE / XTRESTORE, kitty query) do not touch the ANSI saved cursor', () => {
    const s = new Screen(4, 4);
    s.write('\x1b[3;3H\x1b[s\x1b[H\x1b[?1049u\x1b[?u');
    expect([s.cursorRow, s.cursorCol]).toEqual([0, 0]);
    s.write('\x1b[2;2H\x1b[?25s\x1b[u');
    expect([s.cursorRow, s.cursorCol]).toEqual([2, 2]);
  });
});

describe('ansi.Screen — queries and replies (FE-5)', () => {
  it('DSR 6 queues a 1-based cursor-position report', () => {
    const s = new Screen(5, 10);
    s.write('\x1b[3;4H\x1b[6n');
    expect(s.pendingReplies).toEqual(['\x1b[3;4R']);
    expect(s.takeReplies()).toBe('\x1b[3;4R');
    expect(s.takeReplies()).toBe('');
  });

  it('DSR 6 with a deferred-wrap cursor reports the last column', () => {
    const s = new Screen(1, 4);
    s.write('abcd\x1b[6n');
    expect(s.takeReplies()).toBe('\x1b[1;4R');
  });

  it('DECXCPR (CSI ? 6 n) replies with the private form', () => {
    const s = new Screen(3, 3);
    s.write('\x1b[2;2H\x1b[?6n');
    expect(s.takeReplies()).toBe('\x1b[?2;2R');
  });

  it('DSR 5 replies "OK"', () => {
    const s = new Screen(1, 1);
    s.write('\x1b[5n');
    expect(s.takeReplies()).toBe('\x1b[0n');
  });

  it('primary DA (CSI c / CSI 0 c) replies VT100-with-AVO', () => {
    const s = new Screen(1, 1);
    s.write('\x1b[c\x1b[0c');
    expect(s.takeReplies()).toBe('\x1b[?1;2c\x1b[?1;2c');
  });

  it('secondary DA (CSI > c) gets its own reply; nothing is printed', () => {
    const s = new Screen(1, 3);
    s.write('\x1b[>c');
    expect(s.takeReplies()).toBe('\x1b[>1;10;0c');
    expect(rowText(s, 0)).toBe('   ');
  });

  it('a DA reply echoed back as output does not trigger another reply (no feedback loop)', () => {
    const s = new Screen(1, 3);
    s.write('\x1b[>c');
    const reply = s.takeReplies();
    s.write(reply);
    expect(s.takeReplies()).toBe('');
    s.write('\x1b[c');
    const primary = s.takeReplies();
    s.write(primary);
    expect(s.takeReplies()).toBe('');
    expect(rowText(s, 0)).toBe('   ');
  });

  it('DA with extra parameters is not a query', () => {
    const s = new Screen(1, 1);
    s.write('\x1b[0;0c\x1b[>0;1c\x1b[5c');
    expect(s.takeReplies()).toBe('');
  });

  it('multiple queries in one chunk are answered in order', () => {
    const s = new Screen(2, 2);
    s.write('\x1b[6n\x1b[c');
    expect(s.takeReplies()).toBe('\x1b[1;1R\x1b[?1;2c');
  });
});

describe('ansi.Screen — CHT / CBT / REP / DECSCUSR / BCE (FE-5)', () => {
  it('CHT (CSI I) moves forward N tab stops', () => {
    const s = new Screen(1, 40);
    s.write('ab\x1b[IX');
    expect(s.cells[0][8].ch).toBe('X');
    s.write('\x1b[2IY');
    expect(s.cells[0][24].ch).toBe('Y');
  });

  it('CHT clamps at the last column', () => {
    const s = new Screen(1, 10);
    s.write('\x1b[99I');
    expect(s.cursorCol).toBe(9);
  });

  it('CBT (CSI Z) moves back N tab stops', () => {
    const s = new Screen(1, 40);
    s.write('\x1b[20G\x1b[Z');
    expect(s.cursorCol).toBe(16);
    s.write('\x1b[Z');
    expect(s.cursorCol).toBe(8);
    s.write('\x1b[5Z');
    expect(s.cursorCol).toBe(0);
  });

  it('REP (CSI b) repeats the last printed character N times', () => {
    const s = new Screen(1, 8);
    s.write('ab\x1b[3b');
    expect(rowText(s, 0)).toBe('abbbb   ');
    expect(s.cursorCol).toBe(5);
  });

  it('REP with nothing printed yet is a no-op', () => {
    const s = new Screen(1, 4);
    s.write('\x1b[3b');
    expect(rowText(s, 0)).toBe('    ');
    expect(s.cursorCol).toBe(0);
  });

  it('REP keeps the current SGR for the repeats', () => {
    const s = new Screen(1, 4);
    s.write('\x1b[1mx\x1b[2b');
    expect(s.cells[0].slice(0, 3).map((c) => c.attrs)).toEqual([ATTR_BOLD, ATTR_BOLD, ATTR_BOLD]);
  });

  it('DECSCUSR (CSI Ps SP q) stores the cursor style', () => {
    const s = new Screen(1, 1);
    expect(s.cursorStyle).toBe(0);
    s.write('\x1b[5 q');
    expect(s.cursorStyle).toBe(5);
    s.write('\x1b[ q');
    expect(s.cursorStyle).toBe(0);
    s.write('\x1b[99 q');
    expect(s.cursorStyle).toBe(6);
  });

  it('DECSET ?1 toggles application cursor keys', () => {
    const s = new Screen(1, 1);
    expect(s.appCursorKeys).toBe(false);
    s.write('\x1b[?1h');
    expect(s.appCursorKeys).toBe(true);
    s.write('\x1b[?1l');
    expect(s.appCursorKeys).toBe(false);
  });

  it('EL honours background-color-erase: erased cells take the current bg', () => {
    const s = new Screen(1, 4);
    s.write('abcd\x1b[44m\x1b[2G\x1b[K');
    expect(rowText(s, 0)).toBe('a   ');
    expect(s.cells[0].map((c) => c.bg)).toEqual([COLOR_DEFAULT, 4, 4, 4]);
    // fg / attrs are reset, only bg is kept.
    expect(s.cells[0][2].fg).toBe(COLOR_DEFAULT);
    expect(s.cells[0][2].attrs).toBe(0);
  });

  it('ED honours BCE across rows', () => {
    const s = new Screen(2, 2);
    s.write('\x1b[42m\x1b[2J');
    for (const row of s.cells) for (const c of row) expect(c.bg).toBe(2);
    s.write('\x1b[0m\x1b[2J');
    for (const row of s.cells) for (const c of row) expect(c.bg).toBe(COLOR_DEFAULT);
  });

  it('CSI > … m (XTMODKEYS) is not applied as SGR', () => {
    const s = new Screen(1, 1);
    s.write('\x1b[>4;2m');
    expect(s.curAttrs).toBe(0);
    expect(s.curBg).toBe(COLOR_DEFAULT);
  });

  it('a 1-column screen prints a blank for a wide glyph rather than half of it', () => {
    const s = new Screen(2, 1);
    s.write('中a');
    expect(s.cells[0][0].ch).toBe(' ');
    expect(s.cells[1][0].ch).toBe('a');
  });

  it('a saved cursor (ESC 7) is clamped on restore after the screen shrank', () => {
    const s = new Screen(4, 4);
    s.write('\x1b[4;4H\x1b7');
    s.resize(2, 2);
    s.write('\x1b8x');
    expect(s.cursorRow).toBe(1);
    expect(s.cells[1][1].ch).toBe('x');
  });

  it('ED 1 / EL 1 with a deferred-wrap cursor (past the last column) does not throw', () => {
    const s = new Screen(2, 3);
    s.write('abc\x1b[1J');
    expect(rowText(s, 0)).toBe('   ');
    s.write('abc\x1b[1K');
    expect(rowText(s, 0)).toBe('   ');
  });

  it('a huge IL/DL/ICH/DCH count is clamped instead of stalling the parser', () => {
    const s = new Screen(3, 5);
    s.write('abc\x1b[999999999L\x1b[999999999M\x1b[999999999@\x1b[999999999P\x1b[999999999X\x1b[999999999b');
    expect(s.rows).toBe(3);
    expect(s.cursorRow).toBeLessThan(3);
  });
});

describe('ansi.Screen — private-marker CSI is never run as its public form (F13)', () => {
  // A leading parameter byte in 0x3C-0x3F ('<' '=' '>' '?') marks a private
  // sequence (ECMA-48 5.4). tmux 3.6a ignores every one of these; before the
  // fix '<' was not recognised, so kitty's `CSI < u` restored the cursor and
  // an echoed SGR mouse report (`CSI < b;x;y M`) deleted lines.
  function seeded(): Screen {
    const s = new Screen(6, 10);
    s.write('r0\r\nr1\r\nr2\r\nr3\r\nr4');
    s.write('\x1b[1;1H\x1b7\x1b[4;6H');
    return s;
  }

  it('CSI < u / < 1 u / > 1 u / = 1;1 u leave the cursor where it is', () => {
    for (const seq of ['\x1b[<u', '\x1b[<1u', '\x1b[>1u', '\x1b[=1;1u']) {
      const s = seeded();
      s.write(seq);
      expect([seq, s.cursorRow, s.cursorCol]).toEqual([seq, 3, 5]);
    }
  });

  it('CSI < s does not overwrite the saved cursor', () => {
    const s = seeded();
    s.write('\x1b[<s\x1b8');
    expect([s.cursorRow, s.cursorCol]).toEqual([0, 0]);
  });

  it('CSI < 1;4 m is not applied as SGR', () => {
    const s = seeded();
    s.write('\x1b[<1;4m');
    expect(s.curAttrs).toBe(0);
  });

  it('CSI < … M / L / P (an echoed SGR mouse report shape) do not edit lines or chars', () => {
    const s = seeded();
    s.write('\x1b[2;1H\x1b[<0;10;5M\x1b[<5L\x1b[<5P\x1b[<3@\x1b[<2X\x1b[<1S\x1b[<1T');
    expect([0, 1, 2, 3, 4].map((r) => rowText(s, r).trim())).toEqual(['r0', 'r1', 'r2', 'r3', 'r4']);
  });

  it('a marker byte that is not first (CSI 1 > u, CSI 1 < u, CSI 4;>1 m) drops the sequence', () => {
    for (const seq of ['\x1b[1>u', '\x1b[1<u']) {
      const s = seeded();
      s.write(seq);
      expect([seq, s.cursorRow, s.cursorCol]).toEqual([seq, 3, 5]);
    }
    const s = seeded();
    s.write('\x1b[4;>1m');
    expect(s.curAttrs).toBe(0);
  });

  it('private forms of the cursor-motion finals do not move the cursor', () => {
    for (const seq of ['\x1b[>2A', '\x1b[?2B', '\x1b[<2C', '\x1b[=2D', '\x1b[>1E', '\x1b[?1F', '\x1b[>1G', '\x1b[<1;1H', '\x1b[?1;1f', '\x1b[=5d']) {
      const s = seeded();
      s.write(seq);
      expect([seq, s.cursorRow, s.cursorCol]).toEqual([seq, 3, 5]);
    }
  });

  it('non-? private J / K do not erase; DECSED / DECSEL (? J / ? K) still erase', () => {
    const s = seeded();
    s.write('\x1b[1;1H\x1b[>K\x1b[<2J\x1b[=K');
    expect(rowText(s, 0).trim()).toBe('r0');
    expect(rowText(s, 1).trim()).toBe('r1');
    s.write('\x1b[1;1H\x1b[?K');
    expect(rowText(s, 0).trim()).toBe('');
    s.write('\x1b[?2J');
    expect(rowText(s, 1).trim()).toBe('');
  });
});

describe('ansi.Screen — RIS (ESC c) resets modes (F15)', () => {
  it('clears mouse 1000/1002/1003/1006, bracketed paste, app cursor keys; shows the cursor', () => {
    const s = new Screen(3, 10);
    s.write('\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h\x1b[?2004h\x1b[?25l\x1b[?1h\x1b[3 q');
    expect([s.mouseEnabled, s.mouseButtonMotion, s.mouseAnyMotion, s.mouseSgr, s.bracketedPaste]).toEqual([
      true, true, true, true, true,
    ]);
    s.write('\x1bc');
    expect(s.mouseEnabled).toBe(false);
    expect(s.mouseButtonMotion).toBe(false);
    expect(s.mouseAnyMotion).toBe(false);
    expect(s.mouseSgr).toBe(false);
    expect(s.bracketedPaste).toBe(false);
    expect(s.cursorVisible).toBe(true);
    expect(s.appCursorKeys).toBe(false);
    expect(s.cursorStyle).toBe(0);
  });

  it('forgets the DECSC saved cursor', () => {
    const s = new Screen(4, 10);
    s.write('\x1b[3;5H\x1b7\x1bc\x1b[2;2H\x1b8');
    expect([s.cursorRow, s.cursorCol]).toEqual([0, 0]);
  });

  it('matches a freshly constructed Screen for every mode flag', () => {
    const fresh = new Screen(2, 4);
    const s = new Screen(2, 4);
    s.write('\x1b[?1000h\x1b[?1006h\x1b[?2004h\x1b[?25l\x1b[?1h\x1b[5 q\x1bc');
    const modes = (x: Screen) => [
      x.mouseEnabled, x.mouseButtonMotion, x.mouseAnyMotion, x.mouseSgr,
      x.bracketedPaste, x.cursorVisible, x.appCursorKeys, x.cursorStyle,
    ];
    expect(modes(s)).toEqual(modes(fresh));
  });
});

describe('ansi.Screen — copying a soft-wrapped line', () => {
  it('drops the padding blank left by a wide glyph that wrapped early', () => {
    const s = new Screen(3, 4);
    // 'abc' fills columns 0-2; 中 needs two columns, so it wraps and leaves
    // column 3 blank. That blank is padding, not part of the line.
    s.write('abc中');
    expect(rowText(s, 0)).toBe('abc ');
    expect(s.cells[1][0].ch).toBe('中');
    expect(s.selectionText({ row: 0, col: 0 }, { row: 1, col: 3 })).toBe('abc中');
  });
});

describe('ansi.Screen — SGR hidden / strike / ITU colon forms / underline colour (F16)', () => {
  /** SGR `seq` then print X: the cell's style. */
  function sgrCell(seq: string) {
    const s = new Screen(1, 4);
    s.write(`\x1b[${seq}mX`);
    return s.cells[0][0];
  }

  it('handles out-of-range and oversized colour groups as tmux 3.6a does', () => {
    // Measured with `capture-pane -e` after setting red (31):
    //   38:2:300:0:0      -> still red   (out-of-range RGB: group ignored)
    //   38:2::9:9:9:9:9   -> still red   (8+ values: group ignored)
    //   38:5:300, 38:5:   -> emits 39    (bad palette index: back to default)
    //   38;5;300, 38;2;300;0;0 -> emits 39
    expect(sgrCell('31;38:2:300:0:0').fg).toBe(1);
    expect(sgrCell('31;38:2::9:9:9:9:9').fg).toBe(1);
    expect(sgrCell('31;38:5:300').fg).toBe(COLOR_DEFAULT);
    expect(sgrCell('31;38:5:').fg).toBe(COLOR_DEFAULT);
    expect(sgrCell('31;38;5;300').fg).toBe(COLOR_DEFAULT);
    expect(sgrCell('31;38;2;300;0;0').fg).toBe(COLOR_DEFAULT);
    // In-range colours are unaffected by the validation.
    expect(sgrCell('38:2:1:2:3').fg).toBe(rgb(1, 2, 3));
    expect(sgrCell('38:2::4:5:6').fg).toBe(rgb(4, 5, 6));
    expect(sgrCell('48:5:21').bg).toBe(21);
  });

  it('8 / 28 set and clear hidden; 9 / 29 set and clear strikethrough', () => {
    expect(sgrCell('8').attrs).toBe(ATTR_HIDDEN);
    expect(sgrCell('8;28').attrs).toBe(0);
    expect(sgrCell('9').attrs).toBe(ATTR_STRIKE);
    expect(sgrCell('9;29').attrs).toBe(0);
    expect(sgrCell('9;1').attrs).toBe(ATTR_STRIKE | ATTR_BOLD);
  });

  it('colon RGB forms (with and without the colour-space id) set the colour', () => {
    // Expected values are what tmux 3.6a stores for each form (capture-pane -e).
    expect(sgrCell('38:2::255:0:0').fg).toBe(rgb(255, 0, 0));
    expect(sgrCell('38:2:255:0:0').fg).toBe(rgb(255, 0, 0));
    expect(sgrCell('38:2:9:10:20:30').fg).toBe(rgb(10, 20, 30));
    expect(sgrCell('38:2:1:2:3:4:5').fg).toBe(rgb(2, 3, 4));
    expect(sgrCell('48:2::1:2:3').bg).toBe(rgb(1, 2, 3));
  });

  it('colon indexed forms set the colour', () => {
    expect(sgrCell('38:5:196').fg).toBe(196);
    expect(sgrCell('48:5:21').bg).toBe(21);
    expect(sgrCell('1;38:5:3').fg).toBe(3);
  });

  it('a truncated or unknown colon group is ignored on its own', () => {
    const c = sgrCell('38:5;1');
    expect(c.fg).toBe(COLOR_DEFAULT);
    expect(c.attrs).toBe(ATTR_BOLD);
    expect(sgrCell('1:2').attrs).toBe(0);
  });

  it('4:N sets underline for N=1..5, 4:0 clears it, a bare 4: does nothing', () => {
    expect(sgrCell('4:3').attrs).toBe(ATTR_UNDERLINE);
    expect(sgrCell('4;4:0').attrs).toBe(0);
    expect(sgrCell('4:').attrs).toBe(0);
    expect(sgrCell('21').attrs).toBe(ATTR_UNDERLINE);
  });

  it('58 (underline colour) consumes its semicolon arguments instead of running them as SGR', () => {
    const a = sgrCell('58;2;10;20;30;1');
    expect(a.attrs).toBe(ATTR_BOLD);
    expect(a.fg).toBe(COLOR_DEFAULT);
    expect(sgrCell('58;5;4;3').attrs).toBe(ATTR_ITALIC);
    expect(sgrCell('58:2::10:20:30;1').attrs).toBe(ATTR_BOLD);
    expect(sgrCell('58;5;1;3').attrs).toBe(ATTR_ITALIC);
  });

  it('59 is a no-op; a truncated 58 abandons the rest like 38/48', () => {
    expect(sgrCell('59;1').attrs).toBe(ATTR_BOLD);
    expect(sgrCell('58;5').attrs).toBe(0);
  });

  it('semicolon 38/48 forms still work', () => {
    expect(sgrCell('38;5;123').fg).toBe(123);
    expect(sgrCell('48;2;1;2;3').bg).toBe(rgb(1, 2, 3));
    expect(sgrCell(';1').attrs).toBe(ATTR_BOLD);
  });
});

describe('ansi.runStyleCss — attribute → CSS (F16)', () => {
  const run = (attrs: number, fg = COLOR_DEFAULT, bg = COLOR_DEFAULT) => runStyleCss({ fg, bg, attrs });

  it('underline and strikethrough combine into one text-decoration', () => {
    expect(run(ATTR_UNDERLINE)).toContain('text-decoration:underline');
    expect(run(ATTR_STRIKE)).toContain('text-decoration:line-through');
    const both = run(ATTR_UNDERLINE | ATTR_STRIKE);
    expect(both).toContain('text-decoration:underline line-through');
    expect(both.match(/text-decoration/g)).toHaveLength(1);
  });

  it('hidden text is transparent but keeps its background, also under reverse video', () => {
    const h = run(ATTR_HIDDEN, 1, 4);
    expect(h).toContain('color:transparent');
    expect(h).toContain('background:#2472c8');
    expect(h.match(/color:/g)).toHaveLength(1);
    const rh = run(ATTR_HIDDEN | ATTR_REVERSE, 1, COLOR_DEFAULT);
    expect(rh).toContain('color:transparent');
    expect(rh).toContain('background:#cd3131');
  });

  it('keeps the existing bold / dim / italic / reverse mapping', () => {
    expect(run(0, 1)).toBe('color:#cd3131');
    expect(run(ATTR_BOLD | ATTR_ITALIC)).toBe('font-weight:600;font-style:italic');
    expect(run(ATTR_REVERSE)).toBe('color:#0a0a0a;background:#e8e8e8');
  });
});

describe('ansi.Screen — OSC 52 decodes base64 as UTF-8 (F10)', () => {
  function clip(seq: string): string[] {
    const s = new Screen(2, 10);
    const got: string[] = [];
    s.onClipboard = (t) => got.push(t);
    s.write(seq);
    return got;
  }

  it('Slovak diacritics survive (BEL and ESC \\ terminators)', () => {
    // base64 of the UTF-8 bytes of 'čšá'
    expect(clip('\x1b]52;c;xI3FocOh\x07')).toEqual(['čšá']);
    expect(clip('\x1b]52;c;xI3FocOh\x1b\\')).toEqual(['čšá']);
  });

  it('a 4-byte emoji survives, also when the sequence arrives one char at a time', () => {
    expect(clip('\x1b]52;c;8J+klg==\x07')).toEqual(['🤖']);
    const s = new Screen(2, 10);
    const got: string[] = [];
    s.onClipboard = (t) => got.push(t);
    for (const ch of '\x1b]52;c;8J+klg==\x1b\\') s.write(ch);
    expect(got).toEqual(['🤖']);
  });

  it('invalid UTF-8 becomes U+FFFD; invalid base64 is still ignored', () => {
    expect(clip('\x1b]52;c;/w==\x07')).toEqual(['�']);
    expect(clip('\x1b]52;c;!!!\x07')).toEqual([]);
  });
});

describe('ansi.Screen — line operations fill with the current background (N1)', () => {
  // TERM=xterm-256color advertises bce, so tmux sends IL/DL/SD/RI after a
  // bg change and expects the new blank lines in that colour (observed on a
  // live tmux 3.6a: `\e[42m\e[5;1H\e[2L` with no repaint after it).
  const bgs = (s: Screen, r: number) => [...new Set(s.cells[r].map((c) => c.bg))];

  it('IL (CSI L) inserts blank lines in the current bg', () => {
    const s = new Screen(6, 4);
    s.write('\x1b[42m\x1b[3;1H\x1b[2L');
    expect(bgs(s, 2)).toEqual([2]);
    expect(bgs(s, 3)).toEqual([2]);
    expect(bgs(s, 4)).toEqual([COLOR_DEFAULT]);
    expect(s.cells[2][0].fg).toBe(COLOR_DEFAULT);
    expect(s.cells[2][0].attrs).toBe(0);
  });

  it('DL (CSI M) fills the region bottom with the current bg', () => {
    const s = new Screen(6, 4);
    s.write('\x1b[42m\x1b[3;1H\x1b[2M');
    expect(bgs(s, 4)).toEqual([2]);
    expect(bgs(s, 5)).toEqual([2]);
    expect(bgs(s, 3)).toEqual([COLOR_DEFAULT]);
  });

  it('SD (CSI T) and SU (CSI S) fill with the current bg', () => {
    const s = new Screen(4, 4);
    s.write('\x1b[43m\x1b[H\x1b[1T');
    expect(bgs(s, 0)).toEqual([3]);
    expect(bgs(s, 1)).toEqual([COLOR_DEFAULT]);
    s.write('\x1b[44m\x1b[2S');
    expect(bgs(s, 2)).toEqual([4]);
    expect(bgs(s, 3)).toEqual([4]);
  });

  it('RI (ESC M) at the top margin fills with the current bg', () => {
    const s = new Screen(4, 4);
    s.write('\x1b[45m\x1b[H\x1bM\x1bM');
    expect(bgs(s, 0)).toEqual([5]);
    expect(bgs(s, 1)).toEqual([5]);
    expect(bgs(s, 2)).toEqual([COLOR_DEFAULT]);
  });

  it('LF / IND / NEL scrolling at the bottom margin fills with the current bg', () => {
    const s = new Screen(3, 4);
    s.write('\x1b[46m\x1b[3;1H\n');
    expect(bgs(s, 2)).toEqual([6]);
    s.write('\x1b[41m\x1bD');
    expect(bgs(s, 2)).toEqual([1]);
    s.write('\x1b[47m\x1bE');
    expect(bgs(s, 2)).toEqual([7]);
    expect(bgs(s, 1)).toEqual([1]);
  });

  it('RIS, resize and alt-screen entry still blank to the default bg', () => {
    const s = new Screen(2, 3);
    s.write('\x1b[42m\x1bc');
    expect(bgs(s, 0)).toEqual([COLOR_DEFAULT]);
    s.write('\x1b[42m');
    s.resize(4, 5);
    expect(bgs(s, 3)).toEqual([COLOR_DEFAULT]);
    expect(s.cells[0][4].bg).toBe(COLOR_DEFAULT);
    s.write('\x1b[42m\x1b[?1049h');
    expect(bgs(s, 0)).toEqual([COLOR_DEFAULT]);
  });
});

describe('ansi.Screen — DECSTBM clamps an oversize bottom margin (U1)', () => {
  it('an oversize bottom is clamped to the last row and homes the cursor, as tmux 3.6a does', () => {
    // Reference: `\e[1;5r\e[5;5Hab\e[2;99rZ` in a 20x12 tmux 3.6a pane puts
    // Z at row 0 and leaves the scroll region at rows 1..11.
    const s = new Screen(12, 20);
    s.write('\x1b[1;5r\x1b[5;5Hab\x1b[2;99rZ');
    expect(rowText(s, 0).trim()).toBe('Z');
    expect(rowText(s, 4).trimEnd()).toBe('    ab');
    expect([s.scrollTop, s.scrollBottom]).toEqual([1, 11]);
  });

  it('a full-region reset sized for a taller client unsticks an earlier partial region', () => {
    const s = new Screen(10, 20);
    s.write('\x1b[1;5r\x1b[1;38r');
    expect([s.scrollTop, s.scrollBottom]).toEqual([0, 9]);
    s.write('\x1b[10;1Hbottom\n');
    expect(rowText(s, 8).trim()).toBe('bottom');
    expect(rowText(s, 9).trim()).toBe('');
  });

  it('a top margin at or past the last row is still ignored', () => {
    const s = new Screen(6, 10);
    s.write('\x1b[2;4r\x1b[3;3H\x1b[99;99r');
    expect([s.scrollTop, s.scrollBottom]).toEqual([1, 3]);
    expect([s.cursorRow, s.cursorCol]).toEqual([2, 2]);
    s.write('\x1b[6;99r');
    expect([s.scrollTop, s.scrollBottom]).toEqual([1, 3]);
  });

  it('a negative top or bottom margin is ignored, as tmux 3.6a does', () => {
    // The CSI scanner keeps '-' in the body, so `-1` parses as a negative
    // parameter. Reference: `\e[-1;4r` in a 20x8 tmux 3.6a pane leaves the
    // region at rows 0..7.
    const s = new Screen(6, 10);
    s.write('\x1b[2;2H\x1b[-1;4r');
    expect([s.scrollTop, s.scrollBottom]).toEqual([0, 5]);
    expect([s.cursorRow, s.cursorCol]).toEqual([1, 1]);
    s.write('\x1b[-3r\x1b[1;-3r');
    expect([s.scrollTop, s.scrollBottom]).toEqual([0, 5]);
    // A LF on the bottom row still scrolls the whole screen.
    s.write('\x1b[1;1H');
    for (let i = 0; i < 10; i++) s.write(`line${i}\r\n`);
    expect(s.cells.map((_, r) => rowText(s, r).trim())).toEqual([
      'line5', 'line6', 'line7', 'line8', 'line9', '',
    ]);
  });
});

describe('ansi.Screen — DEC Special Graphics never stores control chars (U2)', () => {
  it('b / c / d / e map to the ␉ ␌ ␍ ␊ symbols tmux 3.6a sends, not TAB/FF/CR/LF', () => {
    const s = new Screen(3, 12);
    s.write('\x1b(0abcdefg_\x1b(B|end');
    expect(s.cells[0].map((c) => c.ch)).toEqual(['▒', '␉', '␌', '␍', '␊', '°', '±', ' ', '|', 'e', 'n', 'd']);
    expect([s.cursorRow, s.cursorCol]).toEqual([0, 12]);
  });

  it('no graphics-set character leaves a code point below 0x20 in a cell', () => {
    const s = new Screen(1, 40);
    let all = '';
    for (let c = 0x5f; c <= 0x7e; c++) all += String.fromCharCode(c);
    s.write(`\x1b(0${all}\x1b(B`);
    for (const cell of s.cells[0]) {
      for (const ch of cell.ch) expect(ch.codePointAt(0)!).toBeGreaterThanOrEqual(0x20);
    }
    expect(rowToRuns(s.cells[0]).map((r) => r.text).join('')).not.toMatch(/[\x00-\x1f]/);
  });
});

// ─── Grapheme clusters, as tmux 3.6a joins them (F2) ─────────────────────
//
// tmux 3.6a (variation-selector-always-wide on, its default) joins some
// sequences into one cell in `screen_write_combine`, and the columns it
// addresses afterwards assume the outer terminal did the same. Every
// expectation below was measured on a local tmux 3.6a: the bytes were fed to a
// detached pane (10x4 unless noted), synced on an OSC 2 title sentinel, then
// `#{cursor_x},#{cursor_y}` and `capture-pane -p -N` were read.
//
//   input (CUP = ESC[r;cH)                  tmux cursor  tmux capture-pane row
//   X VS16 Y                                3,0          "X️Y"
//   ❤ VS16 Z                                3,0          "❤️Z"
//   abcdefghi X VS16                        9,0          "abcdefghiX️"
//   abcdefghi X VS16 Z                      10,0         "abcdefghiZ"
//   abcdefghi X VS16 Z W                    1,1          "abcdefghiZ" / "W"
//   abcdefgh X VS16                         10,0         "abcdefghX️"
//   abcdefgh X VS16 Z                       1,1          "abcdefghX️" / "Z"
//   ab:XYZ CUP(1,5) VS16 Q                  6,0          "ab:X️Q"
//   ab:X VS16 X CUP(1,7) Q                  7,0          "ab:X️XQ"
//   abc CUP(1,2) VS16 Q                     3,0          "a️Q"
//   X VS16 VS16 Y                           3,0          "X️️Y"
//   a U+0301 VS16 Y                         3,0          "á️Y"
//   VS16 Y (column 0)                       1,0          "Y"
//   中 CUP(1,2) VS16 Q                       2,0          " Q"
//   中 CUP(1,2) U+0301 Q                     2,0          " Q"
//   q VS16 X  (DEC graphics, ─)             3,0          "q️X" (tmux keeps q + charset attr)
//   1 VS16 U+20E3 X (keycap)                3,0          "1️⃣X"
//   👨 ZWJ 👩 ZWJ 👧 X                         3,0          "👨‍👩‍👧X"
//   👨 ZWJ A X                               4,0          "👨‍AX"
//   👨 ZWJ é X   (also ¡ NBSP ─ 中 🏽)          3,0          "👨‍éX"
//   A ZWJ 👩 X                               2,0          "A‍👩X"
//   👨 ZWJ x y CUP(1,3) é X                  3,0          "👨‍éXy"
//   👨 ZWJ x y CUP(1,5) é X                  6,0          "👨‍xyéX"
//   ab 👨 ZWJ CR 👩 X                         3,0          "👩X"
//   👨 ZWJ SGR(1) 👩 X                        3,0          "👨‍👩X"
//   abcdefgh 👨 ZWJ 👩 X                      1,1          "abcdefgh👨‍👩" / "X"
//   abcdefghi 👨 ZWJ 👩 X                     3,1          "abcdefghi" / "👨‍👩X"
//   👨 ZWJ 👩 ZWJ 👧 ZWJ 👦 X  (20 cols)        3,0          "👨‍👩‍👧‍👦X" (25 bytes)
//   a (ZWJ é)×8 Y  (20 cols)                3,0          "a‍é‍é‍é‍é‍é‍é" "é‍é" "Y"
//                                                        (a 7th ZWJ would pass 32 bytes: dropped)
//   👋 🏽 X                                   3,0          "👋🏽X"
//   👋 🏽 🏽 X                                 3,0          "👋🏽🏽X"
//   🏽 👋 X                                   3,0          "🏽👋X"
//   👨 ZWJ 👩 🏽 X                            3,0          "👨‍👩🏽X"
//   🐶 ZWJ 👨 🏽 X                            5,0          "🐶‍👨🏽X"
//   A 🏽 X / 中 🏽 X / ❤ VS16 🏽 X            4,0 / 5,0 / 5,0 (not joined)
//   ✋ 🏽 X / 🤘 🏽 X  (not in tmux's table)    5,0          "✋🏽X"
//   👋 CUP(1,5) 🏽 X                          7,0          "👋  🏽X"
//   👋 CUP(1,2) 🏽 X                          4,0          " 🏽X"
//   🇺 X                                     2,0          "🇺X"
//   🇺 🇸 X  /  🇺 🇸 🇬 X                       3,0          "🇺🇸X" / "🇺🇸🇬X"
//
// One deliberate divergence: `ab:X中W CUP(1,5) VS16` — tmux widens X over the
// head of 中 and leaves 中's padding cell orphaned in its grid (capture
// "ab:X️W", a later write there wipes X too). We keep the pair invariant
// instead: 中 is blanked whole, as for any other overwrite of a pair.
describe('ansi.Screen — grapheme clusters joined as tmux 3.6a does (F2)', () => {
  const cells = (s: Screen, r: number) => s.cells[r].map((c) => c.ch);
  const text = (s: Screen, r: number) => cells(s, r).join('').trimEnd();
  const ZWJ = '\u200d';
  const VS16 = '\ufe0f';
  const MAN = '\u{1F468}';
  const WOMAN = '\u{1F469}';
  const GIRL = '\u{1F467}';
  const WAVE = '\u{1F44B}';
  const TONE = '\u{1F3FD}';

  describe('VS16 widens a narrow base', () => {
    it('X VS16 Y: the base becomes a 2-cell pair', () => {
      const s = new Screen(4, 10);
      s.write(`X${VS16}Y`);
      expect(cells(s, 0).slice(0, 4)).toEqual([`X${VS16}`, '', 'Y', ' ']);
      expect(s.cursorCol).toBe(3);
    });

    it('❤ VS16 Z', () => {
      const s = new Screen(4, 10);
      s.write(`❤${VS16}Z`);
      expect(s.cursorCol).toBe(3);
      expect(cells(s, 0)[2]).toBe('Z');
    });

    it('at the right edge the base stays one cell and the cursor moves back onto it', () => {
      const s = new Screen(4, 10);
      s.write(`abcdefghiX${VS16}`);
      expect(cells(s, 0)[9]).toBe(`X${VS16}`);
      expect([s.cursorRow, s.cursorCol]).toEqual([0, 9]);
      s.write('Z');
      expect(text(s, 0)).toBe('abcdefghiZ');
      expect([s.cursorRow, s.cursorCol]).toEqual([0, 10]);
      s.write('W');
      expect(text(s, 1)).toBe('W');
      expect([s.cursorRow, s.cursorCol]).toEqual([1, 1]);
    });

    it('a base in the second-to-last column fills the row exactly (deferred wrap)', () => {
      const s = new Screen(4, 10);
      s.write(`abcdefghX${VS16}`);
      expect(cells(s, 0).slice(8)).toEqual([`X${VS16}`, '']);
      expect([s.cursorRow, s.cursorCol]).toEqual([0, 10]);
      s.write('Z');
      expect(text(s, 1)).toBe('Z');
      expect([s.cursorRow, s.cursorCol]).toEqual([1, 1]);
    });

    it('widening overwrites the cell right of the base, and column updates stay aligned', () => {
      const a = new Screen(4, 10);
      a.write(`ab:XYZ\x1b[1;5H${VS16}Q`);
      expect(cells(a, 0).slice(0, 7)).toEqual(['a', 'b', ':', `X${VS16}`, '', 'Q', ' ']);
      expect(a.cursorCol).toBe(6);
      const b = new Screen(4, 10);
      b.write(`ab:X${VS16}X\x1b[1;7HQ`);
      expect(text(b, 0)).toBe(`ab:X${VS16}XQ`);
      expect(b.cursorCol).toBe(7);
    });

    it('joins the glyph left of the cursor even after a cursor move', () => {
      const s = new Screen(4, 10);
      s.write(`abc\x1b[1;2H${VS16}Q`);
      expect(cells(s, 0).slice(0, 4)).toEqual([`a${VS16}`, '', 'Q', ' ']);
      expect(s.cursorCol).toBe(3);
    });

    it('a second VS16, a combined base and column 0', () => {
      const a = new Screen(4, 10);
      a.write(`X${VS16}${VS16}Y`);
      expect(a.cursorCol).toBe(3);
      const b = new Screen(4, 10);
      b.write(`á${VS16}Y`);
      expect(cells(b, 0).slice(0, 3)).toEqual([`á${VS16}`, '', 'Y']);
      const c = new Screen(4, 10);
      c.write(`${VS16}Y`);
      expect(text(c, 0)).toBe('Y');
      expect(c.cursorCol).toBe(1);
    });

    it('a keycap and a DEC graphics base widen too', () => {
      const a = new Screen(4, 10);
      a.write(`1${VS16}\u20e3X`);
      expect(cells(a, 0).slice(0, 3)).toEqual([`1${VS16}\u20e3`, '', 'X']);
      const b = new Screen(4, 10);
      b.write(`\x1b(0q${VS16}\x1b(BX`);
      expect(cells(b, 0).slice(0, 3)).toEqual([`─${VS16}`, '', 'X']);
    });

    it('with the cursor on a trailing half a zero-width mark is dropped, not attached', () => {
      const a = new Screen(4, 10);
      a.write(`中\x1b[1;2H${VS16}`);
      expect(cells(a, 0).slice(0, 2)).toEqual(['中', '']);
      a.write('Q');
      expect(text(a, 0)).toBe(' Q');
      const b = new Screen(4, 10);
      b.write('中\x1b[1;2H\u0301');
      expect(cells(b, 0)[0]).toBe('中');
    });

    it('widening over a wide head blanks that pair whole (pair invariant kept)', () => {
      const s = new Screen(4, 10);
      s.write(`ab:X中W\x1b[1;5H${VS16}`);
      expect(cells(s, 0).slice(0, 7)).toEqual(['a', 'b', ':', `X${VS16}`, '', ' ', 'W']);
      expect(s.cursorCol).toBe(5);
    });

    it('a widened narrow base is a real pair for overwrite, erase, ICH and resize', () => {
      const over = new Screen(1, 4);
      over.write(`X${VS16}\x1b[1;2HQ`);
      expect(cells(over, 0)).toEqual([' ', 'Q', ' ', ' ']);
      const erase = new Screen(1, 4);
      erase.write(`X${VS16}Y\x1b[1;2H\x1b[X`);
      expect(cells(erase, 0)).toEqual([' ', ' ', 'Y', ' ']);
      const ich = new Screen(1, 5);
      ich.write(`abX${VS16}\x1b[1;1H\x1b[2@`);
      expect(cells(ich, 0)).toEqual([' ', ' ', 'a', 'b', ' ']);
      const narrow = new Screen(1, 4);
      narrow.write(`aX${VS16}b`);
      narrow.resize(1, 2);
      expect(cells(narrow, 0)).toEqual(['a', ' ']);
    });

    it('rowToRuns draws a widened base as a 2-cell run', () => {
      const s = new Screen(1, 5);
      s.write(`❤${VS16}Z`);
      expect(rowToRuns(s.cells[0]).map((r) => [r.text, r.wide ?? false])).toEqual([
        [`❤${VS16}`, true],
        ['Z  ', false],
      ]);
    });
  });

  describe('ZWJ sequences', () => {
    it('a family is one 2-cell cluster', () => {
      const s = new Screen(4, 10);
      s.write(`${MAN}${ZWJ}${WOMAN}${ZWJ}${GIRL}X`);
      expect(cells(s, 0).slice(0, 4)).toEqual([`${MAN}${ZWJ}${WOMAN}${ZWJ}${GIRL}`, '', 'X', ' ']);
      expect(s.cursorCol).toBe(3);
    });

    it('any non-ASCII code point joins after a ZWJ; ASCII never does', () => {
      const ascii = new Screen(4, 10);
      ascii.write(`${MAN}${ZWJ}AX`);
      expect(cells(ascii, 0).slice(0, 4)).toEqual([`${MAN}${ZWJ}`, '', 'A', 'X']);
      for (const ch of ['é', '¡', '\u00a0', '─', '中', TONE]) {
        const s = new Screen(4, 10);
        s.write(`${MAN}${ZWJ}${ch}X`);
        expect(s.cursorCol, ch).toBe(3);
        expect(cells(s, 0)[0], ch).toBe(`${MAN}${ZWJ}${ch}`);
      }
    });

    it('the joined cluster keeps its base width', () => {
      const s = new Screen(4, 10);
      s.write(`A${ZWJ}${WOMAN}X`);
      expect(cells(s, 0).slice(0, 3)).toEqual([`A${ZWJ}${WOMAN}`, 'X', ' ']);
      expect(s.cursorCol).toBe(2);
    });

    it('joining looks at the glyph left of the cursor, not at what was printed last', () => {
      const back = new Screen(4, 10);
      back.write(`${MAN}${ZWJ}xy\x1b[1;3HéX`);
      expect(text(back, 0)).toBe(`${MAN}${ZWJ}éXy`);
      expect(back.cursorCol).toBe(3);
      const away = new Screen(4, 10);
      away.write(`${MAN}${ZWJ}xy\x1b[1;5HéX`);
      expect(text(away, 0)).toBe(`${MAN}${ZWJ}xyéX`);
      expect(away.cursorCol).toBe(6);
    });

    it('a CR in between breaks the join; an SGR does not', () => {
      const cr = new Screen(4, 10);
      cr.write(`ab${MAN}${ZWJ}\r${WOMAN}X`);
      expect(text(cr, 0)).toBe(`${WOMAN}X`);
      expect(cr.cursorCol).toBe(3);
      const sgr = new Screen(4, 10);
      sgr.write(`${MAN}${ZWJ}\x1b[1m${WOMAN}X`);
      expect(cells(sgr, 0)[0]).toBe(`${MAN}${ZWJ}${WOMAN}`);
      expect(sgr.cursorCol).toBe(3);
    });

    it('at the right edge: joins while the wrap is deferred, and on the next row after a wrap', () => {
      const pending = new Screen(4, 10);
      pending.write(`abcdefgh${MAN}${ZWJ}${WOMAN}X`);
      expect(text(pending, 0)).toBe(`abcdefgh${MAN}${ZWJ}${WOMAN}`);
      expect(text(pending, 1)).toBe('X');
      expect([pending.cursorRow, pending.cursorCol]).toEqual([1, 1]);
      const wrapped = new Screen(4, 10);
      wrapped.write(`abcdefghi${MAN}${ZWJ}${WOMAN}X`);
      expect(text(wrapped, 0)).toBe('abcdefghi');
      expect(cells(wrapped, 1).slice(0, 3)).toEqual([`${MAN}${ZWJ}${WOMAN}`, '', 'X']);
      expect([wrapped.cursorRow, wrapped.cursorCol]).toEqual([1, 3]);
    });
    it('a family of four still joins; a cell stops growing at tmux\'s 32 UTF-8 bytes', () => {
      const four = new Screen(4, 20);
      four.write(`${MAN}${ZWJ}${WOMAN}${ZWJ}${GIRL}${ZWJ}\u{1F466}X`);
      expect(cells(four, 0)[2]).toBe('X');
      expect(four.cursorCol).toBe(3);
      // 'a' + 6 × (ZWJ é) is 31 bytes; the 7th ZWJ would make 34 and is
      // dropped, so the 7th é starts a cell and the last ZWJ é joins it.
      const pair = `${ZWJ}\u00e9`;
      const chain = new Screen(4, 20);
      chain.write(`a${pair.repeat(8)}Y`);
      expect(cells(chain, 0).slice(0, 3)).toEqual([`a${pair.repeat(6)}`, `\u00e9${pair}`, 'Y']);
      expect(chain.cursorCol).toBe(3);
    });
  });

  describe('skin-tone modifiers and regional indicators', () => {
    it('a modifier joins a base from tmux\'s table, in either order', () => {
      for (const seq of [`${WAVE}${TONE}`, `${WAVE}${TONE}${TONE}`, `${TONE}${WAVE}`, `${MAN}${ZWJ}${WOMAN}${TONE}`]) {
        const s = new Screen(4, 10);
        s.write(`${seq}X`);
        expect(cells(s, 0).slice(0, 3), seq).toEqual([seq, '', 'X']);
        expect(s.cursorCol, seq).toBe(3);
      }
    });

    it('a modifier after anything else is its own wide glyph', () => {
      // Only the first code point of the cluster counts (tmux's mbtowc).
      for (const base of ['A', '中', `❤${VS16}`, '✋', '\u{1F918}', `\u{1F436}${ZWJ}${MAN}`]) {
        const s = new Screen(4, 12);
        s.write(`${base}${TONE}X`);
        const w = base === 'A' ? 1 : 2;
        expect(s.cursorCol, base).toBe(w + 3);
        expect(cells(s, 0).slice(w, w + 2), base).toEqual([TONE, '']);
      }
    });

    it('a modifier joins only a glyph that ends right at the cursor', () => {
      const away = new Screen(4, 10);
      away.write(`${WAVE}\x1b[1;5H${TONE}X`);
      expect(text(away, 0)).toBe(`${WAVE}  ${TONE}X`);
      expect(away.cursorCol).toBe(7);
      const onTrail = new Screen(4, 10);
      onTrail.write(`${WAVE}\x1b[1;2H${TONE}X`);
      expect(cells(onTrail, 0).slice(0, 4)).toEqual([' ', TONE, '', 'X']);
      expect(onTrail.cursorCol).toBe(4);
    });

    it('regional indicators pair into a 2-cell flag', () => {
      const one = new Screen(4, 10);
      one.write('\u{1F1FA}X');
      expect(one.cursorCol).toBe(2);
      const flag = new Screen(4, 10);
      flag.write('\u{1F1FA}\u{1F1F8}X');
      expect(cells(flag, 0).slice(0, 3)).toEqual(['\u{1F1FA}\u{1F1F8}', '', 'X']);
      const three = new Screen(4, 10);
      three.write('\u{1F1FA}\u{1F1F8}\u{1F1EC}X');
      expect(three.cursorCol).toBe(3);
    });
  });
});

describe('Screen.selectionText — wide glyph at a selection edge (N3)', () => {
  // Cells: a b 中 '' c d
  const screen = () => {
    const s = new Screen(2, 10);
    s.write('ab中cd');
    return s;
  };

  it('a start on the trailing half copies the whole glyph', () => {
    expect(screen().selectionText({ row: 0, col: 3 }, { row: 0, col: 5 })).toBe('中cd');
    expect(screen().selectionText({ row: 0, col: 5 }, { row: 0, col: 3 })).toBe('中cd');
    expect(screen().selectionText({ row: 0, col: 3 }, { row: 0, col: 3 })).toBe('中');
  });

  it('an end on either half copies the whole glyph', () => {
    expect(screen().selectionText({ row: 0, col: 0 }, { row: 0, col: 2 })).toBe('ab中');
    expect(screen().selectionText({ row: 0, col: 0 }, { row: 0, col: 3 })).toBe('ab中');
  });

  it('a VS16-widened narrow base behaves the same way', () => {
    const s = new Screen(2, 10);
    s.write('a❤️b');
    expect(s.selectionText({ row: 0, col: 2 }, { row: 0, col: 3 })).toBe('❤️b');
  });

  it('only the first row of a multi-row selection starts mid-glyph', () => {
    const s = new Screen(2, 4);
    s.write('a中\r\n中b');
    expect(s.selectionText({ row: 0, col: 2 }, { row: 1, col: 2 })).toBe('中\n中b');
  });
});

describe('Screen soft wraps — copied text joins wrapped rows (N2)', () => {
  const sel = (s: Screen, r0: number, c0: number, r1: number, c1: number) =>
    s.selectionText({ row: r0, col: c0 }, { row: r1, col: c1 });

  it('a line the terminal wrapped copies back as one line', () => {
    const s = new Screen(5, 20);
    s.write('$ https://example.com/a/very/long/path?q=1\r\n');
    expect(s.wrapped.slice(0, 3)).toEqual([true, true, false]);
    expect(sel(s, 0, 2, 2, 19)).toBe('https://example.com/a/very/long/path?q=1');
  });

  it('a wrapped row keeps its trailing spaces', () => {
    const s = new Screen(3, 5);
    s.write('ab   cd');
    expect(sel(s, 0, 0, 1, 4)).toBe('ab   cd');
  });

  it('a row filled exactly and ended by CR LF is not wrapped', () => {
    const s = new Screen(3, 5);
    s.write('abcde\r\nfg');
    expect(s.wrapped[0]).toBe(false);
    expect(sel(s, 0, 0, 1, 4)).toBe('abcde\nfg');
  });

  it('a wide glyph that straddles the edge wraps its row too', () => {
    const s = new Screen(3, 5);
    s.write('abcd中x');
    expect(s.wrapped[0]).toBe(true);
  });

  it('flags move with rows on LF scroll, SU, RI and SD', () => {
    const lf = new Screen(3, 5);
    lf.write('x\r\nabcdefgh\r\n');
    expect(lf.wrapped).toEqual([true, false, false]);
    expect(sel(lf, 0, 0, 1, 4)).toBe('abcdefgh');
    const su = new Screen(3, 5);
    su.write('x\r\nabcdefgh\x1b[S');
    expect(su.wrapped).toEqual([true, false, false]);
    const ri = new Screen(3, 5);
    ri.write('abcdefgh\x1b[H\x1bM');
    expect(ri.wrapped).toEqual([false, true, false]);
    expect(sel(ri, 1, 0, 2, 4)).toBe('abcdefgh');
    const sd = new Screen(3, 5);
    sd.write('abcdefgh\x1b[T');
    expect(sd.wrapped).toEqual([false, true, false]);
  });

  it('IL / DL move flags, and a row whose continuation moved away is no longer wrapped', () => {
    const dlHead = new Screen(3, 5);
    dlHead.write('abcdefgh\x1b[H\x1b[M');
    expect(dlHead.wrapped).toEqual([false, false, false]);
    const dlTail = new Screen(3, 5);
    dlTail.write('abcdefgh\r\nzz\x1b[2H\x1b[M');
    expect(dlTail.wrapped).toEqual([false, false, false]);
    expect(sel(dlTail, 0, 0, 1, 4)).toBe('abcde\nzz');
    const ilHead = new Screen(3, 5);
    ilHead.write('abcdefgh\x1b[H\x1b[L');
    expect(ilHead.wrapped).toEqual([false, true, false]);
    const ilTail = new Screen(3, 5);
    ilTail.write('abcdefgh\x1b[2H\x1b[L');
    expect(ilTail.wrapped).toEqual([false, false, false]);
    expect(sel(ilTail, 0, 0, 2, 4)).toBe('abcde\n\nfgh');
  });

  it('a scroll region that scrolls a row\'s continuation away clears it', () => {
    const s = new Screen(3, 5);
    s.write('abcdefgh\x1b[2;3r\x1b[S');
    expect(s.wrapped).toEqual([false, false, false]);
  });

  it('erasing the end of a wrapped row clears it; erasing elsewhere does not', () => {
    const el = new Screen(3, 5);
    el.write('abcdefgh\x1b[1;3H\x1b[K');
    expect(el.wrapped[0]).toBe(false);
    const el1 = new Screen(3, 5);
    el1.write('abcdefgh\x1b[1;3H\x1b[1K');
    expect(el1.wrapped[0]).toBe(true);
    const ech = new Screen(3, 5);
    ech.write('abcdefgh\x1b[1;4H\x1b[5X');
    expect(ech.wrapped[0]).toBe(false);
    const ed = new Screen(3, 5);
    ed.write('abcdefghijkl\x1b[2;1H\x1b[J');
    expect(ed.wrapped).toEqual([true, false, false]);
    const ed2 = new Screen(3, 5);
    ed2.write('abcdefgh\x1b[2J');
    expect(ed2.wrapped).toEqual([false, false, false]);
    const ed1 = new Screen(3, 5);
    ed1.write('abcdefghijkl\x1b[2;1H\x1b[1J');
    expect(ed1.wrapped).toEqual([false, true, false]);
  });

  it('RIS clears every flag', () => {
    const s = new Screen(3, 5);
    s.write('abcdefgh\x1bc');
    expect(s.wrapped).toEqual([false, false, false]);
  });

  it('the alt screen starts unwrapped and leaving it restores the primary flags', () => {
    const s = new Screen(3, 5);
    s.write('abcdefgh\x1b[?1049h');
    expect(s.wrapped).toEqual([false, false, false]);
    s.write('\x1b[3;1Hvwxyz12');
    // The bottom row wrapped and scrolled up with its flag.
    expect(s.wrapped).toEqual([false, true, false]);
    s.write('\x1b[?1049l');
    expect(s.wrapped).toEqual([true, false, false]);
    expect(sel(s, 0, 0, 1, 4)).toBe('abcdefgh');
  });

  // tmux repaints a row (window switch, attach, pane redraw) by printing it
  // from column 0 and then either moving the cursor with CUP (the row is not
  // wrapped) or printing on with no cursor move so the terminal's autowrap
  // fires (it is). Measured on tmux 3.6a, 80x24 client, rows 0-1 holding a
  // 125-char wrapped line; bytes tmux sent → `capture-pane -J`:
  //   \e[1;80HX\e[10;1H                   one cell, last column  → joined
  //   \e[HY\e[10;1H                       one cell, column 0     → joined
  //   select-window to a pane whose row 0 is 80 'B' + CR LF, row 1 'next':
  //   \e[H + 80×B + \e[2;1Hnext\e[K                              → two lines
  // So a run printed from column 0 through the last column ends the row's
  // wrap (the autowrap re-sets it); a partial update keeps it. An app that
  // rewrites a wrapped row full-width in place (\e[H + 80 chars + \e[10;1H)
  // reaches the client as the same bytes as the repaint; tmux keeps its
  // stale flag there (still joined), we end the wrap.
  it('a full-width repaint followed by a cursor move ends the row\'s wrap', () => {
    const s = new Screen(3, 10);
    s.write('abcdefghijKL');
    expect(s.wrapped[0]).toBe(true);
    s.write('\x1b[H01234\x1b[1m56789\x1b[m\x1b[2;1Hnext\x1b[K');
    expect(s.wrapped[0]).toBe(false);
    expect(sel(s, 0, 0, 1, 9)).toBe('0123456789\nnext');
    const wide = new Screen(3, 10);
    wide.write('abcdefghijKL\x1b[H01234567中\x1b[2;1H');
    expect(wide.wrapped[0]).toBe(false);
  });

  it('the tmux 3.6a window-switch repaint over a wrapped row copies as two lines', () => {
    const s = new Screen(24, 80);
    s.write(`${'abcdefghij'.repeat(12)}xxxxx\r\n`);
    expect(s.wrapped.slice(0, 2)).toEqual([true, false]);
    s.write(`\x1b[H${'B'.repeat(80)}\x1b[2;1Hnext\x1b[K`);
    expect(s.wrapped[0]).toBe(false);
    expect(sel(s, 0, 0, 1, 79)).toBe(`${'B'.repeat(80)}\nnext`);
  });

  it('a single-cell update at either end of a wrapped row keeps the wrap', () => {
    const last = new Screen(3, 10);
    last.write('abcdefghijKL\x1b[1;10HX\x1b[3;1H');
    expect(last.wrapped[0]).toBe(true);
    expect(sel(last, 0, 0, 1, 9)).toBe('abcdefghiXKL');
    const first = new Screen(3, 10);
    first.write('abcdefghijKL\x1b[1;1HY\x1b[3;1H');
    expect(first.wrapped[0]).toBe(true);
    expect(sel(first, 0, 0, 1, 9)).toBe('YbcdefghijKL');
  });

  it('a repaint that goes on through the autowrap keeps the row wrapped', () => {
    const s = new Screen(3, 10);
    s.write('abcdefghijKL\x1b[H01234\x1b[31m56789KL\x1b[m\x1b[3;1H');
    expect(s.wrapped[0]).toBe(true);
    expect(sel(s, 0, 0, 1, 9)).toBe('0123456789KL');
  });

  // tmux repaints the never-written cells of a row with an erase plus a
  // cursor move, not spaces, so the repaint is not one print run. Measured on
  // tmux 3.6a (80x24 client, select-window from a window whose rows 0-1 hold
  // a 125-char wrapped line); bytes tmux sent → `capture-pane -J`:
  //   row 0 'A\e[78CB\r\n':  \e[HA\e[78X\e[78CB\e[2;1Hnext\e[K   → two lines
  //   row 0 '\e[5C'+75×B:    \e[1;5H\e[1K\e[C+75×B+\e[2;1Hnext   → two lines
  //   app redraws cols 0 and 79 of a wrapped row (\e7\e[1;1HY\e[1;80HX\e8):
  //                          \e[HY\e[78CX                        → joined
  // So the cells covered from column 0 by prints, ECH and EL1 count towards
  // the repaint and a CUF does not: tmux moves the cursor over cells it keeps.
  it('a repaint that erases a gap inside the row (ECH + CUF) ends the row\'s wrap', () => {
    const s = new Screen(3, 10);
    s.write('abcdefghijKL');
    s.write('\x1b[HA\x1b[8X\x1b[8CB\x1b[2;1Hnext\x1b[K');
    expect(s.wrapped[0]).toBe(false);
    expect(sel(s, 0, 0, 1, 9)).toBe('A        B\nnext');
    const tmux = new Screen(24, 80);
    tmux.write(`${'abcdefghij'.repeat(12)}xxxxx\r\n`);
    tmux.write(`\x1b[HA\x1b[78X\x1b[78CB\x1b[2;1Hnext\x1b[K`);
    expect(tmux.wrapped[0]).toBe(false);
    expect(sel(tmux, 0, 0, 1, 79)).toBe(`A${' '.repeat(78)}B\nnext`);
  });

  it('a repaint that erases a leading gap (EL1 + CUF) ends the row\'s wrap', () => {
    const s = new Screen(3, 10);
    s.write('abcdefghijKL');
    s.write(`\x1b[1;5H\x1b[1K\x1b[C${'B'.repeat(5)}\x1b[2;1Hnext\x1b[K`);
    expect(s.wrapped[0]).toBe(false);
    expect(sel(s, 0, 0, 1, 9)).toBe('     BBBBB\nnext');
    const tmux = new Screen(24, 80);
    tmux.write(`${'abcdefghij'.repeat(12)}xxxxx\r\n`);
    tmux.write(`\x1b[1;5H\x1b[1K\x1b[C${'B'.repeat(75)}\x1b[2;1Hnext\x1b[K`);
    expect(tmux.wrapped[0]).toBe(false);
    expect(sel(tmux, 0, 0, 1, 79)).toBe(`     ${'B'.repeat(75)}\nnext`);
  });

  it('an update of both ends of a wrapped row joined by a CUF keeps the wrap', () => {
    const tmux = new Screen(24, 80);
    tmux.write(`L0:${'abcdefghij'.repeat(12)}\r\ntail`);
    tmux.write('\x1b[HY\x1b[78CX\x1b[4;6H');
    expect(tmux.wrapped[0]).toBe(true);
    expect(sel(tmux, 0, 0, 1, 79)).toBe(`Y0:${'abcdefghij'.repeat(7)}abcdefXhij${'abcdefghij'.repeat(4)}`);
    const s = new Screen(3, 10);
    s.write('abcdefghijKL\x1b[HY\x1b[8CX\x1b[3;1H');
    expect(s.wrapped[0]).toBe(true);
    expect(sel(s, 0, 0, 1, 9)).toBe('YbcdefghiXKL');
  });

  it('resize keeps the flags of rows that survive a height change and drops them on a width change', () => {
    const tall = new Screen(3, 5);
    tall.write('abcdefgh');
    tall.resize(5, 5);
    expect(tall.wrapped).toEqual([true, false, false, false, false]);
    tall.resize(1, 5);
    expect(tall.wrapped).toEqual([true]);
    const wide = new Screen(3, 5);
    wide.write('abcdefgh');
    wide.resize(3, 8);
    expect(wide.wrapped).toEqual([false, false, false]);
  });
});
