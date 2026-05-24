import { describe, it, expect } from 'vitest';
import {
  Screen,
  COLOR_DEFAULT,
  ATTR_BOLD,
  ATTR_UNDERLINE,
  rowToRuns,
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
