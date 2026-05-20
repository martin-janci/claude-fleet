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
