import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { MIN_COLS, MIN_ROWS, fitCells } from './terminal_size';

describe('terminal minimum size', () => {
  it('matches the clamp in pty.rs', () => {
    // The backend clamps every pty_open/pty_resize to its own floor. If the
    // two drift apart, tmux draws for a grid the Screen does not have.
    // vitest runs from the project root.
    const rs = readFileSync(resolve(process.cwd(), 'src-tauri/src/pty.rs'), 'utf8');
    const cols = rs.match(/const MIN_COLS: u16 = (\d+);/);
    const rows = rs.match(/const MIN_ROWS: u16 = (\d+);/);
    expect(cols).not.toBeNull();
    expect(rows).not.toBeNull();
    expect(Number(cols![1])).toBe(MIN_COLS);
    expect(Number(rows![1])).toBe(MIN_ROWS);
  });

  it('never fits below the minimum, however small the pane', () => {
    expect(fitCells(0, 0, 7.8, 16)).toEqual({ cols: MIN_COLS, rows: MIN_ROWS });
    expect(fitCells(1, 1, 7.8, 16)).toEqual({ cols: MIN_COLS, rows: MIN_ROWS });
    // 152px of pane width — the 800px window minimum with the default sidebar
    // and centre column — is a real layout, not an edge case.
    expect(fitCells(144, 292, 7.8, 16)).toEqual({ cols: 18, rows: 18 });
  });

  it('floors to whole cells above the minimum', () => {
    expect(fitCells(712, 292, 7.8, 16)).toEqual({ cols: 91, rows: 18 });
  });
});
